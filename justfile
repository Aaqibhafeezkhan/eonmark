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

    step exactly one Bevy
    test "$(cargo tree -i bevy_ecs --depth 0 | wc -l | tr -d ' ')" -eq 1

    step headless smoke
    # M1: switch to fixture (crates/sim/tests/fixtures/smoke.eonreplay).
    cargo run -p game --locked --profile ci -- --headless-run 200 | grep '^tick='

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

# Run the simulation headless for N ticks and print the final hash.
headless TICKS="200":
    cargo run -p game --locked --profile ci -- --headless-run {{TICKS}}

# Determinism self-test: scripted ticks twice on two threads, hashes must match.
selftest:
    cargo run -p sim-cli --locked -- selftest

# Validate every RON file under data/.
data-check:
    cargo run -p sim-cli --locked -- data-check data

# Scripted bot matches (available from M5b).
bots *ARGS:
    @echo "bots: available from M5b (sim-cli play-bots)"

# Sim benchmarks (available from M1).
bench *ARGS:
    @echo "bench: available from M1 (sim-cli bench)"

# Verify replay fixtures (available from M1).
verify *ARGS:
    @echo "verify: available from M1 (sim-cli verify)"

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
