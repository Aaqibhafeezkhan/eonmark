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
| `muda` | 0.21.0 | macOS main menu with a custom `Quit Eonmark` item (id `quit`, Cmd+Q) routed to `AppExit` (`crates/game/src/macos_menu.rs`) | `game`, under `[target.'cfg(target_os = "macos")'.dependencies]`, in use since M2 | Requirement `0.21` in `[workspace.dependencies]`; the game crate declares it with `default-features = false`, which drops muda's `gtk3` default so `gtk` and `proc-macro-error` never enter `Cargo.lock` (see the cargo-deny table). On macOS it brings only `objc2`, `objc2-app-kit`, `keyboard-types` and `crossbeam-channel`, all already in the Bevy graph. `PredefinedMenuItem::quit` bypasses Bevy, so Quit is a regular `MenuItem`; `MenuEvent::send` is `pub(crate)`, so the synthetic-quit proxy builds the event struct and skips the channel. API verified against the registry source on 2026-10-06 (`Accelerator::new(Modifiers, Code)` takes no `Option`). |
| `objc2`, `objc2-app-kit` | 0.6.5, 0.3.2 | Background mode on macOS (`crates/game/src/background.rs`): `NSApplication::setActivationPolicy(Accessory)`, `NSWorkspace::frontmostApplication` and `NSRunningApplication::activateWithOptions` to hand focus back after winit activates the app | `game`, under `[target.'cfg(target_os = "macos")'.dependencies]`, since M2 | Requirements `0.6.5` / `0.3.2` in `[workspace.dependencies]` with `default-features = false`: the exact versions muda already pulls in, so `Cargo.lock` gained no package (the `cargo tree -d` set is unchanged; winit 0.30 keeps its own objc2 0.5.3 / objc2-app-kit 0.2.2 pair as before). Features `std`, `NSApplication`, `NSResponder`, `NSRunningApplication`, `NSWorkspace`. Every call used is a safe fn in objc2-app-kit 0.3.2 (checked in the registry source on 2026-10-07), so the workspace `unsafe_code = "deny"` and the crate's `forbid(unsafe_code)` stand. |
| `directories` | 6.0.0 | `ProjectDirs::from("com", "tonianev", "Eonmark").data_dir().join("replays")` for the replay directory (`sim_driver::default_replay_dir`) | `game`, in use since M2 (replay recorder thread) | Requirement `6` in `[workspace.dependencies]`. `--replay-dir <path>` overrides it so CI and `scripts/m2_checks.sh` write to a temp dir; when the platform has no data directory the fallback is `./replays`. |
| `fixed` | 1.31.0 | `I32F32` fixed-point numerics for positions, speeds, ranges | `sim` | `serde` feature. MSRV 1.93. Deliberately has no trig; the sim uses none. |
| `serde` | 1.0.229 | Serialization of state, commands, rules; `tools/inspect_gltf` reads `bake.ron` and writes glTF JSON with it | `sim`, `rules`, `inspect_gltf` | `derive` feature. |
| `postcard` | 1.1.3 | Canonical binary encoding for replays, snapshots and the hash input | `sim`, `rules` | Stable wire format since 1.0. `alloc` feature. |
| `xxhash-rust` | 0.8.19 | `xxh3_64` state hash, sub-hashes and `rules_hash` | `sim`, `rules` | `xxh3` feature. |
| `rand_pcg` | 0.10.2 | The one `Pcg32` stored inside `Sim` and hashed | `sim` | `serde` feature. |
| `rand_core` | 0.10.1 | `SeedableRng` for the Pcg32 | `sim` | A 0.9.x copy also appears transitively under dev-dependencies; it does not reach the sim's normal graph. |
| `ron` | 0.12.2 | Parsing `data/**/*.ron`, and `tools/assets/bake.ron` in `inspect_gltf` | `rules`, `inspect_gltf`; declared by `sim` for M1 map tables | Published 2026-06-22. `crates/sim/Cargo.toml` lists `ron`, `pathfinding` and `proptest` under `[package.metadata.cargo-machete] ignored` until M1 uses them. |
| `thiserror` | 2.0.21 | `rules::Error` | `rules` | A 1.x copy appears transitively under Bevy; harmless. |
| `clap` | 4.6.7 | `sim-cli` and `inspect_gltf` argument parsing | `sim-cli`, `inspect_gltf` | `derive` feature. |
| `gltf` (+ `gltf-json`, `gltf-derive`) | `=1.4.1` | `tools/inspect_gltf` (art pass): parse and validate glTF and GLB files, read accessors, skins and animations through the `utils` reader, and write the bake's GLB through the `gltf::json` serde re-export | `inspect_gltf`; Bevy's `bevy_gltf` pulls the same version for the game | Declared with `default-features = false` and `features = ["utils", "names", "extras"]`: the default `import` feature would pull `base64` 0.13 and `urlencoding`, which are not in `Cargo.lock`; the tool reads GLB blobs and relative `.bin` and image URIs itself, and `extras` lets the bake record its Modifications text in `asset.extras`. Pinned `=` to the version Bevy 0.19.1 resolves, so adding the tool added only the `inspect_gltf` package to `Cargo.lock`. |
| `png` | 0.18.1 | `inspect_gltf`: decode base textures (any colour type, expanded to RGBA8) and encode the bake's embedded texture | `inspect_gltf`; Bevy's image stack pulls the same version | Requirement `0.18`. The tool uses no `image` crate and no `glam`: its 4x4, quaternion and normal-matrix maths are hand-written and unit-tested (`tools/inspect_gltf/src/math.rs`). |
| `rules` (workspace) | path | `inspect_gltf check` loads `data/` with `rules::Rules::load` to read `visuals.ron`, unit radii and building footprints | `sim`, `ai`, `sim-cli`, `game`, `inspect_gltf` | Engine-free; CI's boundary loop covers `inspect_gltf` too. |

## Dev-dependencies

| Crate | Pinned | Purpose | Used by | Notes |
|---|---|---|---|---|
| `pathfinding` | 4.16.0 | Test oracle for the in-house A*: reachability and cost equality | `sim` (dev only) | Its `astar` runs to completion and has no budget or resume API, so it is never used at runtime. `cargo tree -p sim -e normal` must not list it. |
| `proptest` | 1.11.0 | Random-command determinism, cache on/off equivalence, territory incremental-equals-full | `sim` (dev only) | Locally 8 cases in `m1.rs` (32 until the M4a combat behaviour slice, then 16 until the M4b combat UI slice, each lowered to keep `cargo test -p sim` under 10 s), 16 in `m3a.rs` (32 until the M4b combat UI slice), 8 in `m4a.rs`; `PROPTEST_CASES=256` in CI. |
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

# The engine never reaches the sim side or the asset tool.
for c in sim rules ai sim-cli inspect_gltf; do
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
| RUSTSEC-2026-0344 | `ruzstd` 0.8.3 via `bevy_image` 0.19.1 (Bevy's default `zstd_rust` feature) | Decompression bomb, denial of service only (no memory corruption). `bevy_image` 0.19.1 requires `ruzstd ^0.8`; the fix is 0.9.1, so no lockfile update reaches it. The game decodes zstd only inside KTX2 textures: Bevy's tonemapping LUTs embedded in the binary and repo-controlled files under `assets/`, never a file a player supplies (replays are postcard, not zstd). Switching to `zstd_c` would mean turning off Bevy's default features and relisting them. Advisory published 2026-10; ignored 2026-10-14. | Drop at the Bevy bump that takes `ruzstd` 0.9.1 or later (M9 at the latest) |
| RUSTSEC-2024-0370 | `proc-macro-error` via `gtk3` via `muda` | Not in the graph and not ignored: M2 added `muda` with `default-features = false`, so its `gtk3` feature (the only path to `gtk` and `proc-macro-error`) is off and `cargo deny check` passes without an entry. Re-check if muda's features or defaults change. | Bevy bump (M9) |

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
