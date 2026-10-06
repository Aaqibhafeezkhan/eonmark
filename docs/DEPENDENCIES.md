# Dependencies

This document lists every third-party crate Eonmark uses, the version it is pinned to, what it is for and which workspace crate pulls it in. It records the compatibility constraints that make the dev-tooling trio (Bevy, bevy_egui, bevy-inspector-egui) move only together, the rule that every Bevy doc link must be version-pinned, the Dependabot ignore rules, the CI checks that keep the dependency graph honest, and the upgrade policy. Versions below were read from `Cargo.lock` on 2026-10-05; crates marked as not yet used show the version the workspace requirement resolves to on that date. When you re-verify against crates.io, record drift in the table at the end.

## Runtime and build crates

| Crate | Pinned | Purpose | Used by | Notes |
|---|---|---|---|---|
| `bevy` | `=0.19.1` | Window, Metal rendering through wgpu, input, `bevy_ui` HUD, mesh picking, glTF, `bevy_audio` | `game` only | Default features plus `gestures` (needed for `PinchGesture`). Published 2026-08-13, MSRV 1.95. The `dev` feature forwards to `bevy/dynamic_linking` and `bevy/bevy_dev_tools`; never ship it. |
| `bevy_egui` | `=0.40.1` | Dev-only tuning and inspector panels | `game`, behind `dev` | Requires egui ^0.34 and bevy ^0.19. The only bevy_egui line compatible with the inspector below. |
| `bevy-inspector-egui` | `=0.37.0` | Dev-only world inspector | `game`, behind `dev` | Requires egui ^0.34, bevy_egui ^0.40.0, bevy ^0.19.0. |
| `egui` | 0.34.3 | Transitive, via the two crates above | `game`, behind `dev` | Exactly one `egui` must appear in `Cargo.lock`. |
| `wgpu` | 29.0.4 | Transitive, via Bevy | `game` | Metal is a first-class backend. Design notes cited 29.0.3; the lockfile resolved 29.0.4 (patch). |
| `winit` | 0.30.13 | Transitive, via Bevy | `game` | Keep at or above 0.30.12 (macOS 26 crash fix). |
| `cpal` | 0.17.3 | Transitive, via `bevy_audio` | `game` | Resolves through `coreaudio-rs` 0.14.2 (objc2 bindings, no bindgen step). The design text named `coreaudio-sys` 0.2.18; recorded in the drift log. Audio built and linked against the Xcode 27 SDK at M0 ([BUILD_TIMES.md](BUILD_TIMES.md)). If the CoreAudio bindings ever fail to build, fall back to `default-features = false` minus `bevy_audio` and `vorbis` until M7. `cargo tree -i cpal` must show one version. |
| `muda` | 0.21.0 | macOS main menu with a custom Quit item routed to `AppExit` | `game`, `cfg(target_os = "macos")`, from M2 | Requirement `0.21` in `[workspace.dependencies]`; no crate uses it at M0, so it is absent from `Cargo.lock` until the game crate adds it (cargo-machete must stay clean). `PredefinedMenuItem::quit` bypasses Bevy, so Quit is a regular `MenuItem`. |
| `directories` | 6.0.0 | `ProjectDirs::from("com", "tonianev", "Eonmark").data_dir()` for the replay directory | `game`, from M2 (replay writer thread) | Requirement `6` in `[workspace.dependencies]`; no crate uses it at M0, so it is absent from `Cargo.lock` until the replay writer lands. `--replay-dir <path>` (M2) overrides it so CI writes to a temp dir. |
| `fixed` | 1.31.0 | `I32F32` fixed-point numerics for positions, speeds, ranges | `sim` | `serde` feature. MSRV 1.93. Deliberately has no trig; the sim uses none. |
| `serde` | 1.0.229 | Serialization of state, commands, rules | `sim`, `rules` | `derive` feature. |
| `postcard` | 1.1.3 | Canonical binary encoding for replays, snapshots and the hash input | `sim`, `rules` | Stable wire format since 1.0. `alloc` feature. |
| `xxhash-rust` | 0.8.19 | `xxh3_64` state hash, sub-hashes and `rules_hash` | `sim`, `rules` | `xxh3` feature. |
| `rand_pcg` | 0.10.2 | The one `Pcg32` stored inside `Sim` and hashed | `sim` | `serde` feature. |
| `rand_core` | 0.10.1 | `SeedableRng` for the Pcg32 | `sim` | A 0.9.x copy also appears transitively under dev-dependencies; it does not reach the sim's normal graph. |
| `ron` | 0.12.2 | Parsing `data/**/*.ron` | `rules`; declared by `sim` for M1 map tables | Published 2026-06-22. `crates/sim/Cargo.toml` lists `ron`, `pathfinding` and `proptest` under `[package.metadata.cargo-machete] ignored` until M1 uses them. |
| `thiserror` | 2.0.21 | `rules::Error` | `rules` | A 1.x copy appears transitively under Bevy; harmless. |
| `clap` | 4.6.7 | `sim-cli` argument parsing | `sim-cli` | `derive` feature. |

## Dev-dependencies

| Crate | Pinned | Purpose | Used by | Notes |
|---|---|---|---|---|
| `pathfinding` | 4.16.0 | Test oracle for the in-house A*: reachability and cost equality | `sim` (dev only) | Its `astar` runs to completion and has no budget or resume API, so it is never used at runtime. `cargo tree -p sim -e normal` must not list it. |
| `proptest` | 1.11.0 | Random-command determinism, cache on/off equivalence, territory incremental-equals-full | `sim` (dev only) | 32 cases locally; `PROPTEST_CASES=256` in CI. |
| `criterion` | 0.8.2 | `step_500_units`, `territory_recompute`, `astar_budget` benches | `sim` (dev only), from M1 | Declared in `[workspace.dependencies]`; enters `Cargo.lock` when `crates/sim/benches` lands. `sim-cli bench` is the CI-facing wrapper. |

## Toolchain and dev tools

| Tool | Version | Purpose | Notes |
|---|---|---|---|
| Rust | 1.99.0 | `rust-toolchain.toml` channel; workspace `rust-version = "1.95"` | Released 2026-10-01. Bumped in its own PR every 6 weeks. |
| `cargo-nextest` | 0.9.146 (CI pin) | Test runner in CI and `just ci` | `cargo nextest run --workspace --locked --cargo-profile ci --profile ci`. `--cargo-profile` picks the Cargo profile; `--profile` picks the nextest profile from `.config/nextest.toml`. nextest skips doctests, so `cargo test --doc` runs separately. |
| `cargo-deny` | 0.20.2 (`EmbarkStudios/cargo-deny-action@v2`) | License and advisory gate | `deny.toml` starts from Bevy's allowlist. |
| `cargo-machete` | 0.9.2 (CI pin) | Unused-dependency gate | Why `muda` and `directories` are not in the game manifest until used. |
| `typos-cli` | 1.50.3 (CI pin) | Spelling gate | Spelling only. The trademark gate is `scripts/check_trademark.sh`. |
| `just` | 1.58.0 (CI pin) | Task runner | `just ci` mirrors the CI job and skips missing optional tools with an install hint. |

Install the optional tools. cargo-machete has no Homebrew formula, so it comes from `cargo install`:

```bash
brew install just cargo-nextest cargo-deny typos-cli && cargo install cargo-machete --locked
```

Versions used for the M0 local pass: cargo-nextest 0.9.146, cargo-deny 0.20.2, typos-cli 1.50.3, cargo-machete 0.9.2, just 1.58.0. CI pins the same versions in `.github/workflows/ci.yml`; bump them there and here together.

## The egui compatibility constraint

The three dev-tooling crates form a chain that must resolve to a single `egui`.

| Pair | Constraint | Result |
|---|---|---|
| `bevy-inspector-egui 0.37.0` | needs `egui ^0.34`, `bevy_egui ^0.40.0`, `bevy ^0.19.0` | the anchor |
| `bevy_egui 0.40.1` | needs `egui ^0.34`, `bevy ^0.19` | compatible; the only 0.40.x line |
| `bevy_egui 0.41.x` | needs `egui ^0.35` | two `egui` crates; `dev` feature fails to compile |
| `bevy_egui 0.42.0` | needs `egui ^0.36` | two `egui` crates; `dev` feature fails to compile |
| `bevy_egui 0.43.0-rc.1` | targets Bevy 0.20 | would pull a second Bevy into the graph |

Consequences: `bevy_egui` and `bevy-inspector-egui` are pinned with `=` and move only together, Dependabot ignores both, and CI runs `cargo clippy -p game --features dev --locked --profile ci -- -D warnings` on every PR so the `dev` feature can never rot silently.

## The pinned-docs rule

Bevy's `latest` documentation will describe 0.20 APIs within weeks of this writing, and those do not compile on 0.19.1. Every Bevy documentation or example link used in code comments, docs or PR descriptions must be version-pinned:

- `https://docs.rs/bevy/0.19.1/bevy/...`
- `https://github.com/bevyengine/bevy/tree/v0.19.1/examples/...`

Never `docs.rs/bevy/latest` and never `bevyengine/bevy/tree/main`. Prefer a local `cargo doc --open -p bevy`, which is always the right version.

## Dependabot

`.github/dependabot.yml` runs weekly for `cargo` and `github-actions`, groups updates, applies a 7-day cooldown, and ignores `bevy`, `bevy_egui` and `bevy-inspector-egui` for `version-update:semver-minor` and `version-update:semver-major`. Patch bumps of those three still arrive and are acceptable if `just ci` passes and golden fixtures are unchanged.

## CI graph checks

```bash
# Exactly one Bevy in the graph (two would mean an ecosystem crate pulled 0.20).
test "$(cargo tree -i bevy_ecs --depth 0 | wc -l)" -eq 1

# The engine never reaches the sim side.
for c in sim rules ai sim-cli; do
  if cargo tree -p "$c" -e normal | grep -E 'bevy|glam|wgpu|winit'; then exit 1; fi
done

# The oracle stays a dev-dependency.
if cargo tree -p sim -e normal | grep -q pathfinding; then exit 1; fi

# One audio backend.
test "$(cargo tree -i cpal --depth 0 | wc -l)" -le 1
```

## cargo-deny ignores

`deny.toml` points here for the reason behind each advisory ignore.

| Advisory | Path | Reason | Review |
|---|---|---|---|
| RUSTSEC-2026-0192 | `ttf-parser` via `sctk-adwaita` via `winit` | Unmaintained notice, not a vulnerability. Linux-only client-side decorations; not compiled on macOS. | Drop when winit updates sctk-adwaita |
| RUSTSEC-2024-0370 | `proc-macro-error` via `gtk3` via `muda` | Not yet in the graph. Add to the ignore list at M2 when `muda` returns; the comment in `deny.toml` says so. Linux-only transitive dependency. | M2 |

## Upgrade policy

| What | How | When |
|---|---|---|
| Rust toolchain | Bump `rust-toolchain.toml` in its own PR; nothing else in that PR | Every 6 weeks, after the stable release |
| Bevy | Only through [adr/0002-bevy-0-20-migration.md](adr/0002-bevy-0-20-migration.md); never mid-milestone | When the ADR's trigger conditions hold (M9) |
| `bevy_egui`, `bevy-inspector-egui` | Together, in the Bevy migration PR, to versions that share one `egui` | With Bevy |
| Everything else | Dependabot grouped weekly PR, or `cargo update` locally; merge if `just ci` is green and goldens are unchanged | As they arrive |
| Lockfile | `Cargo.lock` is committed; CI uses `--locked` everywhere | Always |

A dependency that changes a sim hash is a behaviour change: bump `SIM_VERSION`, regenerate fixtures and say why in the commit message. See [DETERMINISM.md](DETERMINISM.md).

## Documented but not used

Multiplayer is not in v0.1. The documented path is redundant-input lockstep over `renet 2.0` / `bevy_renet 5.0`, with `ggrs 0.13` in pure lockstep mode as the fallback. Neither is a dependency. Re-verify Bevy compatibility before any networking milestone.

## Drift log

Record each re-verification here. Add a row per crate whose crates.io version differs from the pin above.

| Date | Crate | Pinned | crates.io latest compatible | Action |
|---|---|---|---|---|
| 2026-10-05 | `wgpu` | 29.0.4 (lockfile) | design notes said 29.0.3 | Patch drift inside Bevy's `^29.0.3`; no action |
| 2026-10-05 | `coreaudio-rs` | 0.14.2 (lockfile, via cpal 0.17.3) | design notes said `coreaudio-sys` 0.2.18 | Different crate: cpal 0.17 uses `coreaudio-rs` with objc2 bindings and no bindgen; no action |
| 2026-10-05 | all others | as above | match design | none |
