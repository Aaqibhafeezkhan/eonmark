# Contributing to Eonmark

This document covers how to set up a development machine, what to run before you push, the fastest way to contribute (data, not engine code), the determinism rules the simulation crates enforce, the pull request, claim and commit rules, and the license terms you agree to by contributing. Who decides what is in [GOVERNANCE.md](GOVERNANCE.md). How AI tools are used is in [AI_CONTRIBUTIONS.md](AI_CONTRIBUTIONS.md). The [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) applies in every project space.

## Prerequisites

| Tool | Why | Install |
|---|---|---|
| Xcode Command Line Tools | Linker (ld-prime) and the macOS SDK. | `xcode-select --install` |
| rustup | Installs the pinned toolchain from `rust-toolchain.toml` (Rust 1.99.0 with rustfmt and clippy) on your first cargo command. If you installed rustup through Homebrew (keg-only), add `/opt/homebrew/opt/rustup/bin` to `PATH`; the `justfile` does this for its own recipes. | https://rustup.rs |
| just | Task runner for `just ci`, `just run` and the other recipes. | `brew install just` or `cargo install just` |
| cargo-nextest, cargo-deny, typos-cli, cargo-machete | Optional locally. `just ci` skips a missing one and prints an install hint. CI runs them all. cargo-machete has no Homebrew formula. | `brew install cargo-nextest cargo-deny typos-cli && cargo install cargo-machete --locked` |
| gh | Release checks and label management, maintainers only. | `brew install gh` |

Linux: the `game` crate must compile there and CI checks it (the `game-linux` job), but running the game is unsupported. Windows is not checked. The `sim`, `rules`, `ai` and `sim-cli` crates work fully on any OS. On Ubuntu, install the system libraries first:

```bash
sudo apt-get install -y libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev
```

## Build and run

```bash
git clone https://github.com/tonianev/eonmark.git
cd eonmark
cargo run -p game --features dev
```

The first build compiles Bevy and takes several minutes. [docs/BUILD_TIMES.md](docs/BUILD_TIMES.md) has measured numbers. The `dev` feature turns on dynamic linking and the inspector panels. Never ship a build with it. Most iteration happens in `crates/sim` with `cargo test -p sim`, which does not compile Bevy at all.

## Before you push: `just ci`

`just ci` runs the same gates as the CI check job, in this order. CI is the single source of truth; the local run saves you a round trip.

| Step | Command |
|---|---|
| Format | `cargo fmt --all -- --check` |
| Lint | `cargo clippy --workspace --all-targets --locked --profile ci -- -D warnings` |
| Lint the dev feature | `cargo clippy -p game --features dev --locked --profile ci -- -D warnings` |
| Tests | `cargo nextest run --workspace --locked --cargo-profile ci --profile ci` |
| Doctests | `cargo test --doc --workspace --locked --profile ci` |
| Docs | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked --profile ci` |
| Unused dependencies | `cargo machete` |
| Spelling | `typos` |
| Licenses and advisories | `cargo deny check` (a separate `deny` job in CI; skipped locally if cargo-deny is missing) |
| Assets | `scripts/check_assets.sh` |
| Naming | `scripts/check_trademark.sh` |
| One Bevy | `cargo tree -i bevy_ecs --depth 0` must print exactly one line |
| Headless smoke | `cargo run -p game --locked --profile ci -- --headless-run 200` must print a `tick=200 hash=0x...` line (replay fixture from M1) |

Other recipes in the [justfile](justfile):

| Recipe | What it does |
|---|---|
| `just fmt` | Runs `cargo fmt --all`. |
| `just fmt-check` | Runs `cargo fmt --all -- --check`. |
| `just run [ARGS]` | Runs `cargo run -p game --features dev -- ARGS`. |
| `just headless [TICKS]` | Runs the sim headless for TICKS ticks (default 200) and prints the final hash. |
| `just selftest` | Runs `sim-cli selftest`: the scripted match twice on two threads; the hashes must match. |
| `just data-check` | Runs `sim-cli data-check data`. |
| `just verify` | Re-simulates a replay headless with `sim-cli verify` (M1). |
| `just bench` | Runs the `sim-cli bench` performance checks (M1). |
| `just bots` | Runs bot-versus-bot matches over several seeds with `sim-cli play-bots` (M5b). |
| `just bundle` | Builds the release binary and assembles `dist/Eonmark.app`, the zip and `SHA256SUMS` with `scripts/bundle.sh`. |
| `just release-check` | Asserts the existing `dist/Eonmark.app` is valid: `plutil -lint`, `codesign --verify`, no `bevy_dylib`, arm64, `data/` present, `SHA256SUMS`. Run `just bundle` first; the full local sequence is `just bundle && just release-check`. |
| `just labels` | Creates or updates the GitHub labels (maintainers, needs `gh`). |
| `just devlog` | Drafts the fortnightly devlog Discussion post from merged PR titles. |

## The data-first path

Most contributions need no engine knowledge. Every tuning number lives in `data/rules/*.ron` as an integer: time in deciseconds (fields ending in `_ds`) and distance in tiles (fields ending in `_tiles`). The loader converts to ticks once. If you find yourself typing a gameplay constant in Rust, stop and move it to RON. The schema is explained in [docs/DATA_FORMAT.md](docs/DATA_FORMAT.md).

```bash
# 1. Edit a RON file, for example data/rules/rules.ron.
# 2. Validate every file under data/. Errors name the file and the field.
cargo run -p sim-cli -- data-check data
# 3. Run the simulation tests.
cargo test -p sim
```

A changed number changes `rules_hash`. Golden replay fixtures (M1) record the hash they were made with, so a rules change that alters a fixture's final hash must also bump `rules_version` in `data/rules/rules.ron`, with a one-line reason in the commit. A changed hash without that bump is a bug, not a fixture update. Balance PRs attach a `play-bots` table once that command exists (M5b).

## Determinism rules

The full rules and the reasoning are in [docs/DETERMINISM.md](docs/DETERMINISM.md). The short version:

1. Only `Sim::step` mutates simulation state. Everything else goes through commands.
2. No `f32`, `f64`, `HashMap`, `HashSet`, `Instant` or `SystemTime`, and no `rand::random` or `rand::thread_rng`, anywhere in `crates/sim`, `crates/rules` or `crates/ai`. Each crate has a `clippy.toml` that bans them, and CI runs clippy with `-D warnings`.
3. No `bevy`, `glam`, `wgpu` or `winit` in `sim`, `rules`, `ai` or `sim-cli`. CI greps `cargo tree -p sim -e normal` for them.
4. Ids are monotonic and never reused. Every sort uses a total order with an id tiebreak.
5. All randomness comes from the one `Pcg32` inside `Sim`, which is part of the hash.
6. No trigonometry. Units store direction vectors, and distances are compared squared through `dist_sq_i64`.
7. Every gameplay number comes from `data/`.

Clippy enforces rule 2. Add this line anywhere in `crates/sim/src/`:

```rust
let x: f32 = 1.0;
```

and run:

```bash
cargo clippy -p sim --all-targets -- -D warnings
```

It fails with:

```text
error: use of a disallowed type `f32`
  |
  |     let x: f32 = 1.0;
  |            ^^^
  |
  = note: sim numerics are fixed-point (sim::Fx); floats are not deterministic across platforms
  = note: `-D clippy::disallowed-types` implied by `-D warnings`
```

The same holds for `-p rules` and `-p ai`. Use `sim::Fx` for fractional values and `BTreeMap` or `BTreeSet` for collections.

## Bevy policy

- Bevy is pinned to `=0.19.1` and is a dependency of exactly one crate, `crates/game`. `bevy_egui =0.40.1` and `bevy-inspector-egui =0.37.0` are pinned with it and move only together. [docs/DEPENDENCIES.md](docs/DEPENDENCIES.md) lists every engine-side crate, its compatible version and who owns the upgrade.
- Use version-pinned documentation only: `https://docs.rs/bevy/0.19.1/` and `https://github.com/bevyengine/bevy/tree/v0.19.1/examples`. Never `latest` or `main`; they describe APIs that do not compile on 0.19.1. Better still, run `cargo doc --open -p bevy` and read the docs for the version you have.
- Upgrades happen only through the M9 architecture decision record, `docs/adr/0002-bevy-0-20-migration.md`, once Bevy 0.20.0 is stable and bevy_egui and bevy-inspector-egui have releases for it. Do not open a PR that bumps Bevy. Dependabot ignores these three crates.
- CI fails if `cargo tree -i bevy_ecs --depth 0` prints more than one line, which means two Bevy versions are in the graph.

## Pull requests

| Kind of PR | Rule |
|---|---|
| Data (`data/`) | Welcome anytime. Run `data-check` and `cargo test -p sim`. Attach a `play-bots` table if the change affects balance (M5b). |
| Docs | Welcome anytime. Run `typos`. |
| Tests | Welcome anytime. |
| Code | Before v0.1.0, open an issue first so the change can be matched to a milestone. The PR must include a test or a replay that shows the change working. |
| Assets | One row in `assets/ATTRIBUTION.md` per file, and the CC0 dedication checkbox for original files. See [assets/ATTRIBUTION.md](assets/ATTRIBUTION.md). |

The PR template, [.github/PULL_REQUEST_TEMPLATE.md](.github/PULL_REQUEST_TEMPLATE.md), has a checklist per kind, including the AI-assistance checkbox described in [AI_CONTRIBUTIONS.md](AI_CONTRIBUTIONS.md). CI is the gate; the checklist is a guide. Expect a first response within 7 days.

## Claiming an issue

- Comment on the issue to claim it. No assignment is needed. A maintainer adds the `status:claimed` label.
- A claim lapses after 14 days of silence. Anyone may then pick the issue up. Post a short note if you need more time.
- `good first issue` tickets can be finished with `cargo test -p sim` or `sim-cli` on any OS and include a "how to verify" section. Tickets that need a Mac carry `help wanted` and `platform:macos` instead.

## Commit style

- Imperative subject line of 72 characters or fewer: `Add Yield Cap table to rules.ron`, not `Added` or `Adds`.
- A body that says why, when the subject is not enough. Reference issues with `Closes #12`.
- No AI attribution trailers: no `Co-Authored-By` lines for tools and no "generated with" footers. Disclosure belongs in the PR template checkbox, not in the git history.
- A golden fixture regeneration goes in a commit that also bumps `rules_version` and states the reason.

## Names

Eonmark uses original names everywhere: the ages Hearth, Masonry and Charter; the resources Grain, Lumber, Ore and Lore; the Freeholders; and the unit and building names in `data/`. The title of the game that inspired Eonmark appears only in [README.md](README.md) (one disclaimed sentence) and in [docs/design/prior-art.md](docs/design/prior-art.md) (its Inspiration section and source citations), nowhere else, and never as an abbreviation. Do not paste prose, tables or numbers from fan wikis, and do not link to them; the fan wiki domain is grep-banned. `scripts/check_trademark.sh` enforces this in CI.

## License

Code, data, documentation and scripts are dual-licensed under MIT OR Apache-2.0. See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE). There is no CLA and no DCO.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

Original assets under `assets/` are CC0-1.0. See [assets/LICENSE-ASSETS.md](assets/LICENSE-ASSETS.md).

Unless you explicitly state otherwise, any original asset (art, audio, font, model, texture) you intentionally submit under assets/ is dedicated to the public domain under CC0-1.0, and you confirm you hold the rights to do so.

Third-party assets may only be CC0-1.0, CC-BY-4.0 or OFL-1.1, each with a row in [assets/ATTRIBUTION.md](assets/ATTRIBUTION.md). OFL fonts ship their license text beside the font file.
