# Eonmark

Eonmark is an open-source, Mac-native real-time strategy game written in Rust. Towns project borders. Armies wither on hostile ground. Ages turn with what you learn. A deterministic, fixed-point simulation runs at 20 ticks per second from a command log, and Bevy draws it as calm, matte, flat-shaded low-poly on Apple Silicon (at M0 the renderer draws a ground plane only). Every rule lives in RON data, so you can rebalance a unit or add a faction without touching the engine.

Eonmark is an original game inspired by Rise of Nations (Big Huge Games, 2003). It is not affiliated with or endorsed by Microsoft.

[![CI](https://github.com/tonianev/eonmark/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/tonianev/eonmark/actions/workflows/ci.yml) [![release-check](https://github.com/tonianev/eonmark/actions/workflows/release-check.yml/badge.svg?branch=main)](https://github.com/tonianev/eonmark/actions/workflows/release-check.yml)

## What you do

Planned play, delivered in M3a to M5b. None of this is implemented at M0.

- Found Towns. Each Town projects a border, and you can only build inside your borders.
- Put Yeomen to work. Grain, Lumber and Ore come in at a rate and flatline at a Yield Cap until Trade techs raise it. Lore comes from the Scriptorium and the Scribes you station there, and is exempt from the cap.
- Everything gets pricier the more you own, so growth is a choice, not a default.
- Research twelve techs across four lines at the Scriptorium. The number of techs you know turns the age: Hearth, then Masonry, then Charter.
- Fight on a counter triangle. Once Harrying is researched, unsupplied enemies inside your borders wither unless a Supply Wain is with them.
- Towns cannot be destroyed, only annexed. Capture the enemy Seat to win.

A match is meant to last 20 to 40 minutes. All numbers are starting values and will move during tuning (M6).

## Status

Pre-alpha. This is the M0 skeleton: toolchain, workspace, CI, a window on macOS, and the governance documents. Nothing is playable yet. The plan, with milestone ids M0 to M9, is in [docs/ROADMAP.md](docs/ROADMAP.md). The first playable skirmish is M5b. The first release, v0.1.0, is M8.

## Quick start for contributors

You need the Xcode Command Line Tools and rustup. The pinned toolchain (Rust 1.99.0) installs itself from `rust-toolchain.toml` on your first cargo command.

```bash
xcode-select --install
# install rustup from https://rustup.rs if you do not have it
git clone https://github.com/tonianev/eonmark.git
cd eonmark
cargo run -p game --features dev
```

The first build compiles Bevy and takes several minutes (2 min 34 s cold on an M5 Max). Measured numbers for the development Mac and for CI are in [docs/BUILD_TIMES.md](docs/BUILD_TIMES.md). Later builds are much faster because `dev` turns on dynamic linking. Never ship a build with `dev`.

To run the CI gates locally, install `just` (`brew install just`) and run `just ci`; see [CONTRIBUTING.md](CONTRIBUTING.md).

## Headless path

Most of Eonmark is a plain Rust library with no engine dependency. You can work on it on any OS, without a GPU, and the tests finish in seconds:

```bash
cargo test -p sim
cargo run -p sim-cli -- selftest
cargo run -p sim-cli -- data-check data
```

`selftest` runs the same scripted ticks twice on two threads and prints two identical hashes. `data-check` loads and validates everything under `data/`. See [CONTRIBUTING.md](CONTRIBUTING.md) for the data-first workflow.

## Platforms

Eonmark is supported and released on Apple Silicon macOS. The `game` crate must keep compiling on Linux, and CI enforces this (the `game-linux` job), so contributors on Linux can work on it. Running the game there is unsupported. Windows is not checked. The `sim`, `rules`, `ai` and `sim-cli` crates run everywhere.

## Installing a release

From v0.1.0 (M8). There are no releases yet.

Releases are published on the GitHub Releases page as `Eonmark-<version>-macos-arm64.zip` with a `SHA256SUMS` file. The app is ad-hoc signed and not notarized, so macOS blocks the first launch.

1. Download the zip with Safari. `curl` and `gh` skip the quarantine flag, so the steps below will not match.
2. Unzip and move `Eonmark.app` to Applications.
3. Double-click it. macOS says the app cannot be opened. Click Done.
4. Open System Settings > Privacy & Security, scroll down, and click Open Anyway next to the Eonmark message. The button is shown for about an hour after the blocked attempt.
5. Confirm with your password or Touch ID. The app opens. Later launches are not blocked.

Terminal alternative to steps 3 to 5:

```bash
xattr -dr com.apple.quarantine /Applications/Eonmark.app
```

Verify the download, in the folder that holds the zip and `SHA256SUMS`:

```bash
shasum -a 256 -c SHA256SUMS
```

## Project layout

| Path | What it is |
|---|---|
| `crates/sim` | Deterministic fixed-point simulation. Pure Rust, no engine dependency. |
| `crates/rules` | RON schema, loader and validator for `data/`. Computes `rules_hash`. |
| `crates/ai` | Scripted skirmish opponent behind the `AiController` trait. |
| `crates/game` | The Bevy presentation layer and the `eonmark` binary. The only engine crate. |
| `crates/sim-cli` | Headless tooling: `selftest`, `data-check` and `hash-dump` now; `verify`, `bench` (M1) and `play-bots` (M5b) later. |
| `data/` | Every rule and number, in RON. Integers only: time in deciseconds, distance in tiles. |
| `assets/` | Fonts, UI, models, audio. Each file has a row in `assets/ATTRIBUTION.md`. |
| `docs/` | Architecture, determinism, data format, build times, roadmap, design notes, ADRs. |
| `scripts/` | Bundle, asset check and naming check scripts used by CI and `just`. |

## Design principles

1. The simulation is deterministic. Same seed and commands give the same hash on every machine. Fixed-point math, no floats, one seeded RNG. See [docs/DETERMINISM.md](docs/DETERMINISM.md).
2. Everything is in data. A gameplay constant in Rust is a bug. See [docs/DATA_FORMAT.md](docs/DATA_FORMAT.md).
3. Calm, matte look. Flat-shaded low-poly, no glow, no bloom, readable at a glance.
4. Replays are always on. Every session writes a replay (from M2), and `sim-cli verify` re-simulates it (M1).
5. CI is the gate. `just ci` runs locally what CI runs. The PR checklist is a guide; CI decides.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) for setup, the data-first path and the PR rules, [GOVERNANCE.md](GOVERNANCE.md) for who decides what, and [AI_CONTRIBUTIONS.md](AI_CONTRIBUTIONS.md) for how AI tools are used here. Issues labeled `good first issue` can be finished with `cargo test -p sim` or `sim-cli` on any OS. Questions go to GitHub Discussions. The [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) applies in every project space.

## License

Code and data (`crates/`, `data/`, `docs/`, `scripts/`) are dual-licensed under MIT OR Apache-2.0, at your option. See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).

Original assets under `assets/` are dedicated to the public domain under CC0-1.0. See [assets/LICENSE-ASSETS.md](assets/LICENSE-ASSETS.md).

Third-party assets under `assets/` keep their own licenses (CC0-1.0, CC-BY-4.0 or OFL-1.1) and are listed in [assets/ATTRIBUTION.md](assets/ATTRIBUTION.md).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
