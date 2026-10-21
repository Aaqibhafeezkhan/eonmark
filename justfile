# Eonmark task runner. `just` lists recipes; `just ci` mirrors .github/workflows/ci.yml.
# Optional tools (nextest, machete, typos, deny) are skipped with an install hint.

set shell := ["bash", "-euo", "pipefail", "-c"]

# rustup is keg-only in Homebrew; make cargo/rustc visible without a shell profile.
export PATH := "/opt/homebrew/opt/rustup/bin:" + env("PATH")
export CARGO_TERM_COLOR := "always"
export RUSTDOCFLAGS := "-D warnings"
export RUST_BACKTRACE := "1"

# List recipes.
default:
    @just --list --unsorted

# Everything CI runs on the per-PR check job, in the same order.
ci:
    #!/usr/bin/env bash
    set -euo pipefail
    have() { command -v "$1" >/dev/null 2>&1; }
    # cargo plugins may live in ~/.cargo/bin without being on PATH; ask cargo.
    have_cargo() { cargo "$1" --version >/dev/null 2>&1; }
    skip() { echo "skip: $1 not installed ($2)"; }
    step() { echo; echo "==> $*"; }

    step rustfmt
    cargo fmt --all -- --check

    step clippy workspace
    cargo clippy --workspace --all-targets --locked --profile ci -- -D warnings
    step clippy game --features dev
    cargo clippy -p game --features dev --locked --profile ci -- -D warnings

    step tests
    if have_cargo nextest; then
      cargo nextest run --workspace --locked --cargo-profile ci --profile ci
    else
      echo "hint: cargo-nextest not installed (brew install cargo-nextest); falling back to cargo test"
      cargo test --workspace --locked --profile ci
    fi
    step doctests
    cargo test --doc --workspace --locked --profile ci

    step rustdoc
    cargo doc --workspace --no-deps --locked --profile ci

    step unused dependencies
    if have_cargo machete; then cargo machete; else skip cargo-machete "cargo install cargo-machete --locked"; fi
    step spelling
    if have typos; then typos; else skip typos "brew install typos-cli"; fi
    step licenses and advisories
    if have_cargo deny; then cargo deny check; else skip cargo-deny "brew install cargo-deny"; fi

    step asset policy
    scripts/check_assets.sh
    step naming policy
    scripts/check_trademark.sh
    step fixture policy
    # CI diffs against the PR base; locally the merge base with main stands in.
    if git rev-parse --verify -q main >/dev/null && [ "$(git rev-parse HEAD)" != "$(git rev-parse main)" ]; then
      scripts/check_fixture_policy.sh "$(git merge-base main HEAD)"
    else
      echo "skip: on main (fixture policy compares a branch against its base)"
    fi

    step exactly one Bevy
    test "$(cargo tree -i bevy_ecs --depth 0 | wc -l | tr -d ' ')" -eq 1

    step headless replay
    # The game re-simulates the golden through its own driver; the last line
    # is `tick=1200 hash=0x...` and the exit code is 0 only on a hash match.
    cargo run -p game --locked --profile ci -- --headless-run crates/sim/tests/fixtures/move_500.eonreplay | tail -n 1

    echo; echo "ci: all steps passed"

# Format the whole workspace.
fmt:
    cargo fmt --all

# Check formatting without changing files.
fmt-check:
    cargo fmt --all -- --check

# Run the game with the dev feature (dynamic linking, inspector, FPS overlay).
run *ARGS:
    cargo run -p game --features dev -- {{ARGS}}

# Run the simulation headless (N ticks, or a .eonreplay to re-simulate) and print the final hash.
headless RUN="200":
    cargo run -p game --locked --profile ci -- --headless-run {{RUN}}

# M2 acceptance proxies end to end (headless fixture time, 30 vs 120 fps hashes, clean exit, kill -9, hud_click, units200_auto); see scripts/m2_checks.sh. Windowed runs use background mode.
m2-checks SCRATCH="":
    EONMARK_BACKGROUND=1 scripts/m2_checks.sh {{SCRATCH}}

# Determinism self-test: scripted ticks twice on two threads, hashes must match.
selftest:
    cargo run -p sim-cli --locked -- selftest

# Validate every RON file under data/.
data-check:
    cargo run -p sim-cli --locked -- data-check data

# Scripted bot matches (available from M5b).
bots *ARGS:
    @echo "bots: available from M5b (sim-cli play-bots)"

# Sim benchmarks: `just bench` runs the 500-mover crossing, `just bench --astar --budget 4000` one saturated A* tick,
# `just bench --territory --sources 16 --edits 100` incremental territory recompute (M3a acceptance: mean < 2 ms).
bench *ARGS:
    cargo run -p sim-cli --locked --release -- bench {{ARGS}}

# Verify replay fixtures in release: every golden under crates/sim/tests/fixtures/, or the files given.
verify *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ $# -eq 0 ]; then set -- crates/sim/tests/fixtures/*.eonreplay; fi
    for f in "$@"; do
      printf '%s: ' "$f"
      cargo run -q -p sim-cli --locked --release -- verify "$f"
    done

# Re-record every golden fixture (or the names given) after a rules_version bump; .hash comes from verify.
rerecord *NAMES:
    scripts/rerecord_fixtures.sh {{NAMES}}

# Golden fixtures change only with a rules_version bump; compares HEAD against BASE (default: merge base with main).
fixture-policy BASE="":
    #!/usr/bin/env bash
    set -euo pipefail
    base="{{BASE}}"
    if [ -z "$base" ]; then base="$(git merge-base main HEAD)"; fi
    scripts/check_fixture_policy.sh "$base"

# Build the release binary and assemble dist/Eonmark.app, zip and SHA256SUMS.
bundle VERSION="":
    scripts/bundle.sh {{VERSION}}

# Assert the bundle in dist/ is valid: plist, signature, no dynamic linking, arm64.
release-check:
    #!/usr/bin/env bash
    set -euo pipefail
    app=dist/Eonmark.app
    test -d "$app" || { echo "error: $app missing; run 'just bundle' first" >&2; exit 1; }
    plutil -lint "$app/Contents/Info.plist"
    codesign --verify --deep --strict --verbose=2 "$app"
    if otool -L "$app/Contents/MacOS/eonmark" | grep -q bevy_dylib; then
      echo "error: dynamic linking leaked into release" >&2; exit 1
    fi
    test "$(lipo -archs "$app/Contents/MacOS/eonmark")" = "arm64"
    test -d "$app/Contents/MacOS/data"
    (cd dist && shasum -a 256 -c SHA256SUMS)
    echo "release-check: ok"

# Print merged PR titles since the last tag (input for the fortnightly devlog).
devlog:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v gh >/dev/null || { echo "error: gh not installed (brew install gh)" >&2; exit 1; }
    last_tag="$(git describe --tags --abbrev=0 2>/dev/null || true)"
    if [ -n "$last_tag" ]; then
      since="$(git log -1 --format=%cI "$last_tag")"
      echo "Merged PRs since $last_tag ($since):"
    else
      since="1970-01-01T00:00:00Z"
      echo "Merged PRs (no tag yet):"
    fi
    gh pr list --repo tonianev/eonmark --state merged --search "merged:>=$since" --limit 200 \
      --json number,title --jq '.[] | "- #\(.number) \(.title)"'

# Create or update the GitHub labels from scripts/sync_labels.sh.
labels REPO="tonianev/eonmark":
    scripts/sync_labels.sh {{REPO}}
