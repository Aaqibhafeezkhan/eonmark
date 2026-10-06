# Architecture

This document is the map of the Eonmark codebase. It says what each crate and directory is for, how a frame flows from input to pixels, which rules every change must respect, and which parts exist today versus which milestone adds them. Read it once before your first change and again whenever a directory confuses you. It describes structure, not game rules; the game rules live in `docs/design/` and the numbers live in `data/`.

## Bird's-eye view

Eonmark is a real-time strategy game for Apple Silicon Macs. It is split into two worlds that meet at exactly one point.

The first world is the simulation. It is plain Rust with no engine, no floats, no wall clock and no hash maps. It advances in fixed 20 Hz ticks. The only way to change it is to call `Sim::step` with a list of player commands. Given the same rules, seed and commands it produces the same state and the same 64-bit hash on every machine and operating system. The scripted opponent runs inside this world.

The second world is presentation. It is the one Bevy crate. It owns the window, the camera, picking, the HUD, audio and the macOS menu. It drives the simulation from Bevy's `FixedUpdate` schedule, mirrors simulation ids to entities, and interpolates between ticks for smooth motion. It never reaches into simulation state except through commands and the read-only `SimView`.

The meeting point is `crates/game/src/sim_driver.rs` (M2). Everything above it is engine; everything below it is deterministic.

Every number that affects play (costs, rates, radii, timers) is RON data under `data/`, loaded once by `crates/rules` and handed to the simulation as integers and fixed-point values. Every session, including bot-vs-bot runs, writes a replay that `sim-cli verify` can re-simulate (replay writer from M2, `verify` from M1).

## Invariants

These four rules are enforced by CI and must survive every PR.

1. The simulation never imports Bevy. `crates/sim`, `crates/rules`, `crates/ai` and `crates/sim-cli` have no dependency on `bevy`, `glam`, `wgpu` or `winit`. CI runs `cargo tree -p sim -e normal` and fails if any of those names appear.
2. Only `Sim::step` mutates simulation state. Presentation code holds a non-send `SimHandle` resource (`Sim` is not `Send` until `AiController` gains a `Sync` bound, so it is inserted with `insert_non_send` and read through `NonSend`/`NonSendMut`) and calls `step`, `view`, `hash`, `tick`, `snapshot` and `restore`. Nothing else writes.
3. Every number comes from `data/`. If you are typing a gameplay constant or a float literal in Rust, stop and move it to a RON file with a `data-check` rule. See [DATA_FORMAT.md](DATA_FORMAT.md).
4. Every session records a replay (from M2). The writer runs on its own thread, flushes every 20 ticks and is a pure sink. A hard kill loses at most one second. See [DETERMINISM.md](DETERMINISM.md).

Two more rules are structural rather than gameplay-related.

- Bevy is a dependency of exactly one crate, pinned with `=`. CI fails if `cargo tree -i bevy_ecs --depth 0` prints more than one line.
- No slotmap, no `HashMap`, no `f32` in the engine-free crates. `clippy.toml` in each of `sim`, `rules` and `ai` lists the banned types and methods; CI runs clippy with `-D warnings`.

## Dependency direction

Arrows point from a crate to the crate it depends on.

```
 engine-free (clippy bans + cargo tree check)          engine (Bevy 0.19.1, one crate)
 +-----------------------------------------------+     +----------------------------+
 |                                               |     |                            |
 |   sim-cli ----> ai ----> sim ----> rules      |     |   game ----> sim, rules, ai |
 |      |                    ^         ^         |     |   (binary: eonmark)        |
 |      +--------------------+---------+         |     |                            |
 |                                               |     |                            |
 +-----------------------------------------------+     +----------------------------+
```

`ai` implements the `sim::AiController` trait and is injected into `Sim::new`, so `sim` never names a concrete bot. `rules` depends on nothing in the workspace.

## Code map

Each entry says what the directory holds, what exists at M0, and which milestone fills it in. Milestone ids are defined in [ROADMAP.md](ROADMAP.md).

### `Cargo.toml`, `rust-toolchain.toml`, `.cargo/config.toml`

Workspace root with exact pins, lints and profiles. Three profile facts matter: `[profile.dev.package."*"] opt-level = 3` does not cover workspace members, so `sim` has its own `opt-level = 3` override to keep `cargo test -p sim` fast; `[profile.ci]` is the single profile for every Bevy compile in the per-PR CI job so one target directory is cached; `[profile.release.package.sim] overflow-checks = true` makes integer overflow panic in release instead of wrapping. The toolchain is pinned to 1.99.0 and bumped in its own PR every six weeks. `.cargo/config.toml` holds only `MACOSX_DEPLOYMENT_TARGET`; no linker flags, no nightly flags. See [DEPENDENCIES.md](DEPENDENCIES.md).

Exists at M0.

### `crates/sim/`

The deterministic simulation library. Pure Rust, `#![forbid(unsafe_code)]`, single-threaded, fixed-point.

| Module | Holds | Arrives |
|---|---|---|
| `lib.rs` | `Sim`, `MatchSetup`, `PlayerId`, `Command`, `PlayerCommand`, `SimView`, `AiController`, `SIM_VERSION` | M0 (skeleton), grows every sim milestone |
| `fx.rs` | `Fx(I32F32)` newtype, `FxVec2`, `dist_sq_i64` | M0 (types), M1 (vector math) |
| `ids.rs` | `PlayerId`, monotonic `UnitId` and `BuildingId` from `IdGen`, kind ids, `Tile` | M0 |
| `map.rs` | 128x128 tiles, `u8` cost grid, `cost_grid_generation`, connected components | M1 (the RON map loader is `rules::map` from M0) |
| `command.rs` | The full `Command` enum, `PlayerCommand`, `sort_commands` | M0 (enum and sorting); handlers M1 (Move, Stop), M3a (Build, Train, Gather, Cancel, SetRally), M4a (Research, AdvanceAge, Attack, AttackMove) |
| `state.rs` | `Sim`, `MatchSetup`, `State`, `step`, `tick`, `hash`, `snapshot`/`restore`, the command delay queue | M0 (skeleton); `sub_hashes`, events and subsystem ticks from M1 |
| `pathing.rs` | In-house A* with `resume(budget)`, request queue, generation-keyed cache | M1 |
| `movement.rs` | Arrival steering, spatial-grid separation, circle push, spiral group offsets | M1 |
| `replay.rs` | `MatchSetup` header, tick batches, hash records, reader | M1 (writer thread lives in `game`) |
| `hash.rs` | `hash_value`: xxh3 over postcard | M0; per-subsystem sub-hashes M1 |
| `ai_hook.rs` | `AiController` trait, `SimView` | M0 |
| `economy.rs` | Integer micro-unit income, Yield Cap, gather slots, ramping costs, pop cap, idle-worker seeking | M3a |
| `town.rs` | Founding, radius growth, Town limit, annexation state machine | M3a, M5a |
| `territory.rs` | `TerritoryField`, disc kernels, pure tie-to-neutral, incremental recompute, `is_buildable` | M3a |
| `construction.rs` | Build sites, progress, cancel | M3a |
| `tech.rs` | Four lines, three ages, permille `Modifier` resolver | M4a |
| `combat.rs` | Stats, auto-acquire, counter multipliers, Watchtower and Town shooters | M4a |
| `attrition.rs` | `SupplyGrid`, Harrying levels, immunities | M5a |
| `visibility.rs` | Per-player `u8` grid (0 unexplored, 1 explored, 2 visible), hashed | M7 |
| `influence.rs` | 4x4-tile influence maps, derived and unhashed | M6 |
| `tests/` | Determinism, economy, territory, tech, combat, attrition tests; golden fixtures under `tests/fixtures/` | M1 onward |
| `benches/` | criterion benches `step_500_units`, `territory_recompute`, `astar_budget` | M1, M3a |

State layout is not an ECS. Units and buildings live in `BTreeMap<UnitId, Unit>` and `BTreeMap<BuildingId, Building>` with the `IdGen` allocator (`IdGen::unit()`, `IdGen::building()`), itself hashed state. Iteration is always id order or sorted order with an id tiebreak. Derived structures (influence maps, spatial grid, connected components, path cache) are rebuilt, never hashed.

At M0 the crate holds `Fx` and `FxVec2`, every id type and the `IdGen` allocator, the full `Command` enum with `sort_commands`, `MatchSetup` (including player slots), `SimView`, the `AiController` trait, and a `Sim` whose hashed state is `{ tick, seed, rng: Pcg32, ids, players, units, buildings, pending, commands_applied }`. `step` queues the tick's commands for `tick + cmd_delay`, runs the AI and queues its commands the same way, then applies everything due this tick in `(player, seq)` order; only `Surrender` has a real handler, and every other command advances the RNG once so the hash already depends on the command stream. `hash`, `snapshot` and `restore` are the real scheme (xxh3 over postcard, `restore` calls a `rebuild_derived` that is empty until M1). Subsystem ticks start in M1.

### `crates/rules/`

Schema, loader and validator for everything under `data/`. `Rules::load(dir)` reads the RON files, checks every cross-reference, converts authored units to ticks and integers exactly once, and computes `rules_hash`. Errors name the file path and field. The clippy ban list applies here too, so a float can never enter through data.

At M0: `lib.rs` (the `Rules` struct with the full v0.1 field set from `rules.ron`, a validator, `ticks_from_ds` and `rules_hash`), `resources.rs` (the four resources) and `map.rs` (`MapDef`, the tile-row map loader and symmetry validator). `Rules::load` attaches resources and every map under `data/maps/` to the struct, so `rules_hash` covers all three. Content tables (units, buildings, techs, ages, factions, visuals, AI data) arrive with their milestones (M2, M3a, M4a, M5b).

### `crates/ai/`

Scripted skirmish opponents implementing `sim::AiController`. Every bot is a deterministic function of `SimView` and the `Pcg32` stream inside the sim. The clippy ban list applies.

At M0: `Passive`, a bot that never acts, and `Scripted`, which replays a fixed command list so `sim-cli selftest` exercises the delay queue and the RNG. M5b adds the first complete opponent (build-order executor, worker balancing, expansion, Watchtower placement, attack waves, defend rule). M6 adds difficulties, personalities and influence-map counter composition.

### `crates/game/`

The only Bevy crate and the `eonmark` binary. Bevy ECS is used here for presentation only.

| Module | Holds | Arrives |
|---|---|---|
| `main.rs` | Entry point: dispatches to `headless` or `app` | M0 |
| `cli.rs` | Flags `--headless-run <ticks>`, `--exit-after-seconds <s>`, `--seed <u64>`, `--data-dir <path>`; data-dir resolution (flag, `$EONMARK_DATA`, checkout `data/`, `<exe dir>/data`) | M0 |
| `app.rs` | Windowed app: 1280x800 window, `SimHandle` non-send resource, `FixedUpdate` driver at `tick_rate_hz`, window-title tick/hash readout | M0; states `Menu`, `Skirmish`, `Paused`, `GameOver` in M2 |
| `sim_driver.rs` | `FixedUpdate` driver at 20 Hz, `InputSource` trait (Local, Replay), `PendingCommands`, replay writer thread, 8-ticks-per-frame cap | M2 |
| `present.rs` | `UnitId -> Entity` mirror, prev/current interpolation, `Visual` to primitive or glTF | M2 (primitives), M7 (glTF) |
| `camera.rs` | Pitch-locked perspective RTS camera: WASD and arrow pan, edge scroll, trackpad `Pixel` pan, wheel `Line` zoom, `PinchGesture` zoom | M0 (pan, zoom, pinch), M2 (finished with selection and orders) |
| `selection.rs` | Click, drag box, shift add, control groups, double-click same kind | M2 |
| `orders.rs` | Ray-plane ground hit to `Move`, `Stop`, `AttackMove` | M2 |
| `ground.rs` | Grid mesh with vertex colors, territory overlay mesh, then `FieldExt` extended material and field texture upload | M0 (plane), M3b (overlay and shader) |
| `palette.rs` | The matte ground greens, grid, sky, sun and ambient colours | M0 |
| `hud/` | Resource bar, selection panel, command card, minimap, message line | M2 (skeleton), M3b, M4b, M7 (minimap) |
| `menu/` | Main menu, pause menu, game-over screen | M5b (New Game), M6, M7 |
| `macos_menu.rs` | `muda` menu with a custom Quit item routed to `AppExit`, `NonSendMarker` systems | M2 |
| `audio.rs` | Thin facade over `bevy_audio` | M7 |
| `dev_tools.rs` | FPS overlay, hash readout, draw-call counter, `bevy_egui` panels; behind the `dev` feature | M0 (FPS), M2 onward |
| `headless.rs` | `--headless-run` with `MinimalPlugins`, printing `tick=<n> hash=0x<16 hex>`; the CI smoke test on both OSes | M0 (`<ticks>`), M2 (`<replay>`) |

Features: `dev` (dynamic linking, dev tools, egui panels; never shipped) and `ci_testing` (owner screenshot capture; not used in CI). The crate must keep compiling on Linux; CI enforces this with a compile-only clippy job. Running on Linux is unsupported.

At M0: a window titled Eonmark, a flat matte ground plane with a grid, a directional light, a pan/zoom/pinch camera, a `FixedUpdate` sim driver whose tick and hash appear in the window title, `--headless-run <ticks>`, and an FPS overlay, inspector and sim panel under `dev`.

### `crates/sim-cli/`

Headless tooling that runs on an Ubuntu CI runner with no GPU.

| Subcommand | Does | Arrives |
|---|---|---|
| `selftest --ticks N --seed S` | Runs the scripted match twice on two threads and compares hashes | M0 |
| `data-check [dir]` | Loads and validates everything under `data/` | M0 |
| `hash-dump --ticks N --seed S` | Prints the whole-state hash after every tick of the scripted match | M0 |
| `hash-dump <replay> --every N` | Prints per-tick sub-hashes of a replay for bisecting | M1 |
| `verify <replay>` | Re-simulates a replay and reports one of four outcomes | M1 |
| `bench` | Timing runs with `--units`, `--ticks`, `--combat`, `--bots`, `--territory` | M1 |
| `fuzz --ticks N` | Random command streams | M1 |
| `play-bots` | Seeded bot-vs-bot games in parallel with a CSV summary | M5b |

A global `--data <dir>` flag (default `data`) selects the data directory. Subcommands that have not landed yet exit with code 2 and print `not implemented until <milestone>`.

### `data/`

All tuning as RON. See [DATA_FORMAT.md](DATA_FORMAT.md) for the conventions and the file inventory. Licensed like code (MIT OR Apache-2.0).

At M0: `data/rules/rules.ron` with the full v0.1 field set (values for later milestones are validated but unused until those milestones), `data/rules/resources.ron`, `data/maps/plains_1v1.ron`, and a README in `data/`, `data/rules/`, `data/maps/` and `data/ai/`. `data/ai/` holds only its README until M5b.

### `assets/`

Runtime assets: fonts, UI 9-slice panels, icons, glTF models, OGG clips, the field shader, `ATTRIBUTION.md` and `LICENSE-ASSETS.md`. Only CC0, CC-BY-4.0 and OFL-1.1 material. `scripts/check_assets.sh` fails CI if any non-allowlisted file lacks an attribution row or exceeds the size caps. See [ART_STYLE.md](ART_STYLE.md).

Arrives M7, except the font and UI pack which may land earlier with the HUD (M3b).

### `scripts/`, `justfile`

`bundle.sh` assembles `Eonmark.app` and signs it last (M8). `check_assets.sh` and `check_trademark.sh` are CI gates (M0). The `justfile` wraps the CI commands so `just ci` runs locally what GitHub runs remotely; it skips a missing optional tool with an install hint instead of failing. There is no `xtask` crate.

### `.github/`

`ci.yml` runs five jobs: `check` on macOS (fmt, clippy, nextest, doc, machete, typos, asset and trademark gates, single-Bevy check, headless smoke run), `headless` on Ubuntu (sim, rules, ai and sim-cli tests, data-check, boundary check, headless smoke run; `verify` from M1), `game-linux` on Ubuntu (compile-only clippy of the game crate), `deny` (cargo-deny) and `hash-parity` (diffs the final hash produced on each OS). `release-check.yml` builds release and bundles on pushes to main. `release.yml` publishes on tags. Dependabot ignores Bevy and the two egui crates for minor and major bumps. See [RELEASING.md](RELEASING.md).

### `docs/`

This file, [DETERMINISM.md](DETERMINISM.md), [DATA_FORMAT.md](DATA_FORMAT.md), [DEPENDENCIES.md](DEPENDENCIES.md), [ART_STYLE.md](ART_STYLE.md), [BUILD_TIMES.md](BUILD_TIMES.md), [NAME.md](NAME.md), [PLAYTEST.md](PLAYTEST.md), [RELEASING.md](RELEASING.md), [ROADMAP.md](ROADMAP.md), per-subsystem design docs under `design/`, and architecture decision records under [adr/](adr/README.md). Each design doc states Eonmark's own target numbers and an acceptance checklist and arrives with the milestone that implements its system.

## One frame, end to end

The sim driver and presenter arrive in M2. This is the shape they take.

```
one rendered frame in crates/game

 mouse / keyboard / trackpad / macOS menu
          |
          v
 [Update]
   camera.rs      moves the camera in f32; never touches the sim
   selection.rs   maintains the selection set (render-side state)
   orders.rs      turns a right-click into Command::Move / AttackMove / Stop
   hud/           turns a command-card click into Command::Build / Train / Research
   macos_menu.rs  drains MenuEvent; Quit becomes AppExit::Success
          |
          v
 PendingCommands (Resource)
   Vec<PlayerCommand> for the local player, seq stamped in issue order
          |
          v
 [FixedUpdate, Time<Fixed> at 20 Hz, runs 0 to 8 times this frame]
   sim_driver.rs, for each tick T that is due:
     cmds = InputSource::commands_for(T)     the commands issued during tick T
         Local  : drains PendingCommands
         Replay : reads the TickCommands batch for T from the file
         None   : stall; no step this frame
     sim.step(cmds)                      the only mutation of sim state:
                                         queues cmds for tick T + cmd_delay, runs the AI and
                                         queues its commands the same way, then applies every
                                         command due at T in (player, seq) order
     every 20 ticks: h = sim.hash()
     channel.send(postcard(TickCommands(T, cmds)), and h when taken)
                                         the writer thread appends and flushes
   if more than 8 ticks are due, the remainder is dropped as time dilation
          |
          v
 [after FixedUpdate]
   present.rs
     sync_lifecycle      spawn or despawn one Entity per new or removed id;
                         HashMap<UnitId, Entity> lives here, on the render side
     sync_transforms     Transform = lerp(prev, current, Time<Fixed>::overstep_fraction());
                         heading derived in f32 from the sim's facing vector
     sync_field_texture  upload territory, fog and supply into the 128x128 field texture (M3b+)
   hud/                  read SimView for stockpiles, rates, cap state, population, selection
          |
          v
 render through wgpu on Metal, PresentMode::AutoVsync
```

Three consequences fall out of this shape.

- Frame rate never changes the result. The sim sees tick-stamped commands and nothing else, so 30 fps and 120 fps produce the same hash. Game speed (M6) changes how many ticks are due per frame within the 8-tick cap; replays are unaffected.
- The scripted AI's commands never appear in `PendingCommands` or the replay. The AI runs inside `step`, its commands are stamped and delayed exactly like a human's, and verification reproduces them by running the same AI. In-flight commands sit in the sim's own delay queue, which is hashed state, so a snapshot carries them.
- A network peer is one more `InputSource`. Lockstep multiplayer is documented, not built, in v0.1.

## Cross-cutting concerns

### Determinism

Covered in full by [DETERMINISM.md](DETERMINISM.md). The short form: fixed-point numbers, ordered containers, monotonic ids, one seeded RNG, commands sorted by `(player, seq)`, derived caches never hashed, and a cross-OS hash-parity job in CI.

### Data-driven numbers

Covered by [DATA_FORMAT.md](DATA_FORMAT.md). `crates/rules` is the only place that converts authored units to ticks and integers, and it does so once at load. The sim receives `i32`, `i64` and `Fx` and nothing else. `rules_hash` is stored in every replay header so an edited RON file is reported as `RULES CHANGED` rather than mistaken for a sim bug.

### Error handling

Commands are validated at apply time against ownership, age, tech, borders, stockpiles and population cap. A rejected command emits `SimEvent::CommandRejected { player, seq, reason }` and never panics. Data errors are reported by `rules::Error` with file path and field. Overflow in the sim is a deterministic panic in release, by design.

### Testing

`cargo test -p sim` is the inner loop: no GPU, no Bevy compile, under 10 seconds for the non-ignored suite. Golden replay fixtures up to 5000 ticks run there; longer ones are `#[ignore]` and verified by `sim-cli verify` in release in the headless CI job. The game crate is smoke-tested by `--headless-run` on a fixture replay using `MinimalPlugins`. Property tests use `proptest` with 32 cases locally and 256 in CI. The in-house A* is checked against the `pathfinding` crate as a dev-only oracle.

### Performance

Targets are listed in [BUILD_TIMES.md](BUILD_TIMES.md) alongside measured numbers. The important shape decisions: flat entity layout with health bars and selection rings as separate entities (never children of moving units), one shared mesh and material per kind, a draw-call budget under 500, `MeshPickingSettings { require_markers: true }` so only units and buildings are raycast, and a budgeted resumable A* so one pathological search cannot blow a tick.

### macOS specifics

Cmd-Q through winit's default menu terminates the process without running Bevy exit logic, so M2 installs a `muda` menu with a custom Quit item that emits a `MenuEvent`, drained on the AppKit main thread via `NonSendMarker` systems into `AppExit`. The replay writer never calls `sync_all` during play because on Apple it is `F_FULLFSYNC` and stalls for milliseconds. Trackpad two-finger scroll arrives as `MouseScrollUnit::Pixel` (pan), the wheel as `Line` (zoom), pinch via `PinchGesture` (requires the Bevy `gestures` feature). `ClusteredDecal` and `PresentMode::Mailbox` are unavailable on Metal and are not used. Bundling and signing are in [RELEASING.md](RELEASING.md).

### Naming and licensing

All names are original. The trademark gate `scripts/check_trademark.sh` runs in CI. Code and data are MIT OR Apache-2.0; original assets are CC0. See [adr/0001-license.md](adr/0001-license.md) and [NAME.md](NAME.md).

## What exists at each milestone

| Milestone | Simulation | Presentation | Tooling and docs |
|---|---|---|---|
| M0 | `Fx`, monotonic ids, full `Command` enum, `Sim` skeleton with delay queue, Pcg32, xxh3 hash, snapshot/restore; `Passive` and `Scripted` bots; `rules` loads `rules.ron`, `resources.ron` and the map | Window, ground plane, light, pan/zoom/pinch camera, `FixedUpdate` driver, `--headless-run <ticks>`, FPS overlay and inspector under `dev` | CI including the hash-parity job (200-tick smoke; fixtures from M1), `selftest`, `data-check`, scripted `hash-dump`, governance and these docs |
| M1 | Map, movement, A*, replay format, snapshot/restore, hashing with sub-hashes | none | `verify`, `bench`, replay `hash-dump`, `fuzz`, golden fixtures; hash-parity switches to fixtures |
| M2 | none | Sim driver, presenter, selection, orders, camera finished, macOS menu, replay writer thread, headless run | Controls checklist in [PLAYTEST.md](PLAYTEST.md) |
| M3a / M3b | Economy, Towns, territory, construction | Border overlay then `FieldExt` shader, build ghost, command card, resource bar | Economy, territory and towns design docs |
| M4a / M4b | Tech, ages, combat | Research panel, age indicator, health bars, unit silhouettes | Tech and combat design docs |
| M5a / M5b | Attrition, supply, annexation, victory; first complete bot | Victory overlay, New Game | `play-bots`, first-match fixture |
| M6 | Difficulties, personalities, influence maps | Game speed, pause, game-over stats | Fun gate, match log |
| M7 | Visibility grid | Fog, minimap, menus, glTF, audio | Finalized [ART_STYLE.md](ART_STYLE.md) |
| M8 | none | none | `bundle.sh`, release workflow, v0.1.0 |
| M9 | Second faction as data | Bevy 0.20 migration (gated) | [adr/0002](adr/0002-bevy-0-20-migration.md) |
