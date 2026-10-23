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
| `map.rs` | `Map`: 128x128 tiles, `u8` cost grid (1 passable, 255 blocked), `cost_grid_generation`, `set_blocked` with eager component rebuild, `set_blocked_many` (a whole footprint for one generation bump and one rebuild), `neighbors8` (no corner cutting), `nearest_passable` BFS, `spiral`; hashed state with derived components | M1 (done; the RON map loader is `rules::map` from M0), M3a (`set_blocked_many`, done) |
| `command.rs` | The full `Command` enum, `PlayerCommand`, `sort_commands` | M0 (enum and sorting); handlers M1 (Move, Stop, DebugSpawn; done), M2 (AttackMove applied like Move through `apply_move`, a placeholder; done), M3a (Build, Cancel, Gather, Train, SetRally, and the appended `DebugPlaceBuilding` and `DebugGrant`: done), M4a (Research, AdvanceAge and the appended `DebugResearch`: done in the tech slice; Attack: done in the combat core slice; real AttackMove and the appended `DebugSpawnJittered`: done in the combat behaviour slice) |
| `state.rs` | `Sim`, `MatchSetup` (with `debug_commands`, `start: StartKind`, `scenario`), `State`, `Unit` (with `job` and `seek_at`, and from M4a `damage_taken`, `ready_at`, `target` and `intent`), `Intent`, `Job`, `Building` (with `construction`, `paid`, `town`, `town_state`, `slots`, `queue`, `rally`, and from M4a `ready_at`), `Construction`, `TownState`, `MoveOrder`, `Post` (the spot an arrived unit holds and returns to), `SimEvent`, `RejectReason`, `SubHashes`, `step` and the tick order, `tick`, `hash`, `sub_hashes`, `snapshot`/`restore` with `rebuild_derived`, `drain_events`, the command delay queue, `SIM_VERSION` (14 since the M4a combat behaviour slice) | M0 (skeleton); M1 (done: sub-hashes, events, Move/Stop/DebugSpawn handlers, pathing and movement ticks, return to post); M2 (done: `apply_move` shared by Move and AttackMove); M3a (Town loop done: standard start, Build, site Cancel, step 6; gathering done: the Gather handler, `TownTierChanged`; training done: the Train, queue Cancel and SetRally handlers, step 6b, `TrainingCancelled`); M4a (tech slice done: `State.tech`, `QueueItem::Research`, the Research, AdvanceAge and DebugResearch handlers, `ResearchCompleted`, `AgeAdvanced`; combat core slice done: the unit and building combat fields, steps 5a-5c, the Attack handler, `InvalidTarget` replacing `NotImplemented`, `Hit`, `UnitDied`, `BuildingDestroyed`, `TownAtZeroHp`; behaviour slice done: `AttackMove` sets `Intent::AttackMove` for armed units, `DebugSpawnJittered`, `UnitDied` and `BuildingDestroyed` carry the owner and kind) |
| `scenarios.rs` | Pure-data command streams (`Stream = Vec<(tick, Vec<PlayerCommand>)>`) with their `MatchSetup` and lengths: `move_500`, `move_500_short`, `group_spiral`, `snapshot_restore` (M1), `units200`, `units200_auto`, `scripted_moves`, `hud_click` (M2), `economy_second_town` (M3a), `economy_walkthrough` (M3b owner proxy; these two start from the standard start), `combat_skirmish` (the M4a combat golden), `unit_looks` (M4b: one unit of every kind per player in two rows, windowed only), `enemy_outpost` (M4b: from the standard start, a 200 vs 200 prologue that P1 with Arms I to III wins on its side of the river, freeing the player's pop, and a lone P1 Watchtower at (58, 64); the interactive acceptance scenario) and `enemy_outpost_walkthrough` (the same setup plus P0's real line-1 commands at beat ticks derived from the rules by `enemy_outpost_beats`, with grants from `outpost_grants`; the M4b scripted owner proxy and golden); `NAMES` and `by_name` shared by the tests, the benches, `sim-cli record` and the game's `--scenario`; outside `NAMES`, `bench_cross` (the `sim-cli bench` crossing), `counter_duel_setup` (the counter tests' 10 vs 10 duels) and `combat_bench` (the 200 vs 200 battle of `sim-cli bench --combat` and the `combat_step` criterion bench) | M1 (done), M2 (done), M3a (done), M3b (`economy_walkthrough`), M4a (`combat_skirmish`, `counter_duel_setup`, `combat_bench`: done), M4b (`unit_looks`, `enemy_outpost`, `enemy_outpost_walkthrough`) |
| `pathing.rs` | In-house A* `AStarSearch::resume(map, &mut budget)`, `Pathing` request queue sorted by `(requested_tick, UnitId)`, hashed active search, transparent generation-keyed `PathCache` | M1 (done) |
| `movement.rs` | `SpatialGrid` (2x2 tiles, derived; `for_each_in`, a visitor that does not allocate, from M4a), `step` (arrival steering, separation, circle correction, repath triggers; from M4a the combat engagements of step 5a, hold or chase, replace a unit's own steering), `group_targets` (component-aware spiral offsets) | M1 (done), M4a (done) |
| `replay.rs` | `.eonreplay` v1: `MAGIC`, header, `TickBatch`, `HashRecord` with sub-hashes, `ReplayWriter` (clean-exit trailer), truncation-tolerant `ReplayReader` (`ReplayFile`, `Corrupt` error; peeks `sim_version` before decoding the header, `SimVersion` error), `verify` with the four `VerifyOutcome`s | M1 (done; the writer thread lives in `game`, M2), M3a (version peek, done) |
| `hash.rs` | `hash_value`: xxh3 over postcard | M0; per-subsystem sub-hashes M1 (done) |
| `ai_hook.rs` | `AiController` trait, `SimView`; from M3a the accessors `setup`, `economy`, `income`, `territory`, `building`, `towns`, `building_cost`, `check_build`, `worked_slots`, `idle_workers`, `pop`, `unit_cost` and `check_train`, and from M4a `age`, `researched`, `research_quote`, `check_research`, `check_advance_age` and `check_build_kind` (the `Build` checks that need no builder or tile, for the command card), then `unit_stats`, `unit_hp` and `check_attack`, which call the same functions `Sim::step` uses; from M4b `building_hp` (hit points and the kind's maximum, for health bars) | M0, M3a (done), M4a (done), M4b |
| `economy.rs` | `Economy` (per player, `i64` stockpiles and income accumulators per resource), `RateInfo` and the computed rates (base yields plus worked slots), exact income through the Yield Cap and `stockpile_cap` (step 6d), the jobs step (6c, which ends with idle seeking from `gather.rs`), cost helpers (ramped building cost, deduct, refund) | M3a (done) |
| `gather.rs` | `validate_gather` (pure) and `apply_gather`, slots taken through `Job::Gather`, stand tiles, worked slots and their raw income, gather-job validity for step 6c, `idle_seek_step` (idle workers seeking a free slot, the second half of 6c), `unit_takes_slot` (a finished builder, or a unit just trained there, taking its building's slot) | M3a (done) |
| `training.rs` | `validate_train` (9 ordered checks, pure; the age and `requires_tech` checks from M4a) and `enqueue`, pop used (unit entries only), `unit_cost_for` (the counts `n` and `k` that `Rules::unit_cost` prices), `cancel_slot` (queue Cancel with an exact refund, units and research alike), the training step (6b: head progress, spawn beside the building, rally walk or a slot there; a research head completes its tech), `validate_rally` and `set_rally` | M3a (done), M4a (research entries) |
| `town.rs` | Standard start, town count (sites included) and limit, attribution to the nearest own complete Town; tier and radius refresh (`refresh`, `refresh_all`: the `town_growth_age` gate and the Statecraft push, from M4a); annexation state machine | M3a (done), M4a (done), M5a |
| `territory.rs` | `TerritoryField`, disc kernels, pure tie-to-neutral, incremental recompute, `is_buildable` | M3a (done; hashed in `State` since the Town loop: the Seats project at the start, a Town or Watchtower when it completes, and a Town that reaches tier 2 changes its source) |
| `construction.rs` | Footprints (`Rect`, `footprint`, `centre_tile`, `centre_fx`, `ring`), `validate_build` (14 ordered checks from M4a, pure) and `validate_build_kind` (its checks that need no builder or tile), `place`, `assign_builder`, the construction step (6a), `complete`, `remove_building` (shared by `cancel_site` and combat destruction; the caller unblocks the footprints) and `cancel_site`, `validate_debug_place` | M3a (done), M4a (`remove_building`) |
| `tech.rs` | `TechState` (per player the age and the researched techs; sub-hash `tech`), the Modifier-resolved player stats (`stat`, `yield_cap`, `pop_cap`, `town_limit`, `town_radius`, `research_quote`, and the unit stats `unit_max_hp` and `unit_damage` that combat reads), `validate_research` (10 ordered checks) and `enqueue_research`, `validate_advance` (4) and `advance`, `DebugResearch`, `complete` | M4a (done) |
| `combat.rs` | `Stats` (every player's unit stats per kind, the counter table and the shooters, built at the start of step 5), `think` (5a: an explicit `Attack` holds, chases directly, walks a path or gives up; an idle guard or an attack-mover keeps, switches or acquires a target, seeded draws breaking exact ties, and holds or chases it directly; the ready Watchtowers and Towns aim), `resolve` (5c: simultaneous hits by units and shooters, counter multipliers, variance) and `reap` (death, destruction, Town at 0 HP), `chases_directly` and `segment_clear` (the direct-chase test: a clear straight segment from the unit's position, the walk the chase steers), `validate_attack` and `apply_attack`, `rng_index` | M4a (done) |
| `attrition.rs` | `SupplyGrid`, Harrying levels, immunities | M5a |
| `visibility.rs` | Per-player `u8` grid (0 unexplored, 1 explored, 2 visible), hashed | M7 |
| `influence.rs` | 4x4-tile influence maps, derived and unhashed | M6 |
| `tests/` | `m0.rs`, `m1.rs` (determinism, snapshot/restore, proptests, path oracle, goldens), `m2.rs` (the four M2 scenarios, AttackMove-as-Move), `m3a.rs` (`territory_*` and `economy_*` tests, the `economy_second_town` golden), `m3b.rs` (the `economy_walkthrough` beats and golden), `m4a.rs` (`tech_*`: research, ages, the Modifier resolver, Arms hit points and damage; `combat_*`: damage, simultaneous hits, the explicit Attack, death and destruction, acquisition, AttackMove, guards, shooters and the `combat_skirmish` beats; `counters_*`: the counter triangle over 20 seeds; `golden_combat_skirmish`; `m4a_*`: seeded random streams over tech and combat), `m4b.rs` (`enemy_outpost_*`: the shared setup, the tower's reach, the walkthrough's commands and grants, and, ignored locally, the prologue, the walkthrough's beats and its golden); fixtures `move_500_short`, `move_500`, `group_spiral`, `snapshot_restore`, `economy_second_town`, `economy_walkthrough`, `combat_skirmish`, `enemy_outpost_walkthrough` under `tests/fixtures/` with `.hash` siblings; attrition tests later | M1 (done), M2 (done), M3a (done) onward |
| `benches/` | criterion benches `step_500_units`, `astar_budget` (M1, done), `territory_recompute` (M3a, done), `combat_step` (M4a, done: 20 steps of the 400-unit battle from its first fighting tick, on a restored snapshot) | M1, M3a, M4a |

State layout is not an ECS. Units and buildings live in `BTreeMap<UnitId, Unit>` and `BTreeMap<BuildingId, Building>` with the `IdGen` allocator (`IdGen::unit()`, `IdGen::building()`), itself hashed state. Iteration is always id order or sorted order with an id tiebreak. Derived structures (influence maps, spatial grid, connected components, path cache) are rebuilt, never hashed.

At M0 the crate holds `Fx` and `FxVec2`, every id type and the `IdGen` allocator, the full `Command` enum with `sort_commands`, `MatchSetup` (including player slots), `SimView`, the `AiController` trait, and a `Sim` whose hashed state is `{ tick, seed, rng: Pcg32, ids, players, units, buildings, pending, commands_applied }`. `step` queues the tick's commands for `tick + cmd_delay`, runs the AI and queues its commands the same way, then applies everything due this tick in `(player, seq)` order; only `Surrender` has a real handler, and every other command advances the RNG once so the hash already depends on the command stream. `hash`, `snapshot` and `restore` are the real scheme (xxh3 over postcard, `restore` calls a `rebuild_derived` that is empty until M1). Subsystem ticks start in M1.

At M1 the hashed state gains `map` and `pathing` (queue plus the active A* search; the cache is `serde(skip)`). `Sim::step` runs queue -> AI -> apply -> `Pathing::service` under `path_budget_expansions` -> `SpatialGrid::rebuild` and `movement::step` -> `tick += 1`. `Move` assigns `group_targets` and issues path requests, `Stop` clears the order and cancels the request, and `DebugSpawn` (accepted only when `MatchSetup.debug_commands` is true) places units on a square spiral of passable tiles. `sub_hashes()` returns nine named hashes (`units`, `buildings`, `economy`, `territory`, `tech`, `pathing`, `rng`, `map`, `meta`), `drain_events()` hands out `UnitSpawned`, `UnitArrived`, `PathUnreachable` and `CommandRejected`, and `rebuild_derived()` rebuilds components and the spatial grid and clears the path cache. Unit stats come from `data/rules/units.ron` and are converted once at spawn (`speed` in tiles per tick, `radius` in tiles).

At M2 the sim changes in one place: `Command::AttackMove { units, target }` is applied exactly like `Move` through the shared `Sim::apply_move`, so the game's attack-move placeholder is accepted instead of being rejected with an RNG draw; real attack-move needs combat (M4a). That is a behaviour change, so `SIM_VERSION` is 4 and `rules_version` is 4 (`visuals.ron` joined `rules_hash` at the same time); the golden fixtures were re-recorded with unchanged hashes. `scenarios.rs` gains the four M2 streams. Everything else M2 builds is on the game side.

At M3a the Town loop lands first (`SIM_VERSION` 6, `rules_version` 7). The hashed state gains `economy` and `territory`, and `MatchSetup` gains `start`: `Standard` (the skirmish default) places each player's Seat, start units and stockpile from `standard_start`, `Empty` (the scenario default) places nothing. `Sim::step` gains step 6 after movement, each part in id order: 6a construction, 6b training (from the training slice, below), 6c jobs, 6d income; then `tick += 1`. `Build` validates purely in `construction::validate_build`, then either resumes an own site or deducts the ramped cost, places a site (its footprint blocked with one `set_blocked_many`) and sends the builder; a site completes when its builder has been at work for the kind's build time, and a completed Town or Watchtower projects its border. `Cancel` of slot 0 on an own site refunds it in full. `Build` and `Cancel` no longer draw from the RNG; `DebugPlaceBuilding` and `DebugGrant` join the debug commands. `drain_events()` adds `BuildingPlaced`, `BuildingCompleted`, `BuildingRemoved` and `TerritoryChanged`.

After a construction fix (`SIM_VERSION` 7, `rules_version` 8: builders walk only to ring tiles they can reach), gathering lands (`SIM_VERSION` 8, `rules_version` 9). `gather.rs` gives `Gather` a real handler, validated purely in `gather::validate_gather`, so a rejection draws no randomness. Step 6c (`economy::jobs_step`) now also drops a gather job whose building is gone, not the unit's owner's, not complete or not worked by the unit's kind, or whose worker has no order and is not at work there, and then runs `gather::idle_seek_step`: an idle Yeoman or Scribe (no job, no order) starts its `seek_at` timer and, when it runs out, takes the lowest free slot of the nearest own complete building its kind works, or retries later. Step 6a lets a finished builder take a slot at its building, and a completion re-derives the attributed Town's tier (`town::recompute_tier`), which can move its border source to the tier-2 radius. Step 6d credits worked slots with the base yields. `SimView` gains `worked_slots` and `idle_workers`, and `drain_events()` adds `TownTierChanged`.

Training lands last (`SIM_VERSION` 9, `rules_version` 10). `training.rs` gives `Train` a real handler, validated purely in `training::validate_train` (own complete building, unit kind, trainer, queue room, pop cap with queued units counted, ramped cost), which deducts the cost and queues the entry, so cost and pop are charged at queue time. Step 6b (`training::step`) gives the queue head of every complete building one tick of progress in `BuildingId` order; a finished head becomes a unit beside the building that walks to the building's rally point, takes a slot there or starts idle. Review fixes (`SIM_VERSION` 11, `rules_version` 12) spawn it in the rally point's component when the rally point's tile is passable and that component touches the building's ring (any component when the ring is fully blocked), or else in the largest such component, so a pocket sealed by neighbouring buildings takes the unit only when the rally point is inside it. `Cancel` of a queue slot refunds exactly what the entry paid, and `SetRally` sets the rally point. `Train` and `SetRally` no longer draw from the RNG, which leaves `Attack`, `Research` and `AdvanceAge` as the M4a placeholders. `SimView` gains `pop`, `unit_cost` and `check_train`, and `drain_events()` adds `TrainingCancelled`.

M4a lands in slices. After the rules data (`rules_version` 13), the tech slice (`SIM_VERSION` 12, `rules_version` 14) adds `tech.rs` and the hashed `State.tech` (per player the age and the researched techs, the `tech` sub-hash). `Research` queues a `QueueItem::Research { tech, ticks }` entry in the host's training queue with its Letters-resolved cost deducted and its time fixed, and step 6b completes it; `AdvanceAge` is instant; `DebugResearch` is appended to `Command`. The Yield Cap, pop cap, town limit and Town radius are resolved from the faction's and the researched techs' Modifier rows whenever they are read. `town::refresh` re-derives a Town's tier and radius on completion, research and age advance, and tier 2 needs `town_growth_age` (the Masonry Age). `Train` and `Build` check the age, `Train` also the kind's `requires_tech`. None of these commands draws from the RNG; `Attack` stayed the last placeholder until the combat core slice. See [design/tech-and-ages.md](design/tech-and-ages.md), "M4a implementation notes".

The combat core slice (`SIM_VERSION` 13, `rules_version` 15) adds `combat.rs`. `Unit` gains `damage_taken`, `ready_at`, `target` and `intent`, `Building` gains `ready_at`; a unit's hit points and damage are resolved every tick from the kind and the owner's `UnitHp` and `UnitDamage` rows (Arms), never stored. Step 5 becomes: rebuild the spatial grid, 5a `combat::think` (every unit with an explicit `Attack`, in id order, from the start-of-tick positions: hold in weapon range, chase directly within its reach on a clear straight segment from its position, otherwise walk a path toward the target, re-goaled like a repath, or give up when the target cannot be reached from the unit's component, as "Chase and give-up" in design/combat.md defines it), 5b `movement::step` with those engagements and the repath requests, 5c `combat::resolve` (every hit collected from the post-movement positions, summed per target and applied at once, then the dead removed and every building or site at 0 HP other than a complete Town destroyed, the tick's footprints unblocked by one `set_blocked_many`; a complete Town stays at 0 HP). Then step 6 as before. `Attack` has a real handler and no rejection draws from the RNG; `RejectReason::NotImplemented` is gone and `InvalidTarget` is new; `drain_events()` adds `Hit`, `UnitDied`, `BuildingDestroyed` and `TownAtZeroHp`. See [design/combat.md](design/combat.md), "M4a implementation notes".

The combat behaviour slice (`SIM_VERSION` 14, `rules_version` 16) widens 5a: every unit with an attack is visited in id order; an explicit `Attack` runs as above, while an idle unit (a guard, with or without a post) or an attack-mover keeps its acquired target, switches to a better one or acquires one among the enemy units in the spatial grid and the enemy buildings within its reach that are in weapon range or directly chaseable (enemy units first, the Mangonel's `prefer: Buildings` the other way round; in weapon range first; then the nearest; a seeded `rng_index` draw among exact ties) and holds or chases it directly, never along a path; then every complete Watchtower and Town with hit points left whose cooldown has run out aims at the nearest enemy unit in range, in id order, and 5c fires the aims after the units' hits. `AttackMove` is no longer a `Move`: armed units get `Intent::AttackMove`, fight on the way with their order kept, resume it and guard once it is gone. `Command::DebugSpawnJittered` is appended (seeded spawn jitter for the counter tests and fixtures). `sim::scenarios` gains `combat_skirmish`, the combat golden that CI verifies on both operating systems for hash parity, and `counter_duel_setup`.

The M4a close keeps `SIM_VERSION` 14 and `rules_version` 16. It adds `sim::scenarios::combat_bench`, the 200 vs 200 battle that `sim-cli bench --combat` times (the wall clock stays in `sim-cli`), and the criterion bench `combat_step`. A review fix makes the direct-chase test of 5a trace the straight segment the chase walks from the unit's position (`combat::segment_clear`, exact integer arithmetic) instead of the line between tile centres, for acquiring, keeping and the explicit `Attack` alike, so a guard beside a blocked corner no longer drops and retakes its target; no fixture, counter table or bench hash changed. So M4a's sim is: `tech.rs` (research in the training queues, three ages, the Modifier-resolved player and unit stats), `combat.rs` (the explicit `Attack` with direct and path chases, acquisition by guards and attack-movers, the Watchtower and Town shooters, simultaneous hits with counter multipliers, death and destruction), the real `AttackMove`, `DebugResearch` and `DebugSpawnJittered`, and seven golden fixtures, `combat_skirmish` among them. See [DETERMINISM.md](DETERMINISM.md), "What M4a adds".

### `crates/rules/`

Schema, loader and validator for everything under `data/`. `Rules::load(dir)` reads the RON files, checks every cross-reference, converts authored units to ticks and integers exactly once, and computes `rules_hash`. Errors name the file path and field. The clippy ban list applies here too, so a float can never enter through data.

At M0: `lib.rs` (the `Rules` struct with the full v0.1 field set from `rules.ron`, a validator, `ticks_from_ds` and `rules_hash`), `resources.rs` (the four resources) and `map.rs` (`MapDef`, the tile-row map loader and symmetry validator). `Rules::load` attaches resources and every map under `data/maps/` to the struct, so `rules_hash` covers all three. M1 adds `units.rs` (`UnitKind` with the `_x100` movement fields, `Units::validate`, `Rules::unit_kind(u16)` and `unit_kind_by_id(&str)`; `units.ron` is attached by `load` and is part of `rules_hash`). M2 adds `visuals.rs` (`Primitive`, `Visual::{Primitive, Scene}`, `Visuals::validate` against the unit kinds, `Rules::visual(kind)`; `data/visuals.ron` is attached by `load` and is part of `rules_hash` although only the game reads it). M3a adds `buildings.rs` (`BuildingKind`, `GatherSpec`, `Slots::{Fixed, Adjacent}`, `Buildings::validate`; `buildings.ron` is attached by `load` and is part of `rules_hash`), the training fields of `UnitKind` (`UnitClass`, `UnitRamp::{Flat, PerUnit, Military}`), the ramp, queue, idle-seek and `standard_start` fields of `Rules`, the checks that span files (trainers, `gathered_from` both ways, the standard start, map starts against the Town footprint) and `costs.rs`, the pure cost functions (`building_cost`, `unit_cost`, `building_mult_permille`, `military_mult_permille`) that the sim, the HUD and the bot share. M4a's rules data adds `modifiers.rs` (`Modifier`, `StatPath`, `UnitSel`, `ModOp` and the per-row floored `resolve`), `attack.rs` (`Attack`, `Prefer`), `techs.rs`, `ages.rs` and `factions.rs` (each file attached by `load` and part of `rules_hash`), `hp`, `attack` and `requires_tech` on `UnitKind`, the counter table, building attacks, `Rules::modifier_rows` and the tech, age and combat accessors, and the checks that span the new files (the `ages.ron` references resolved first, unlocks exactly once and in order, capped costs, at least one tick per cooldown and research, at least 1 damage per hit, worst-case bounds per stat). The AI data arrives with M5b.

### `crates/ai/`

Scripted skirmish opponents implementing `sim::AiController`. Every bot is a deterministic function of `SimView` and the `Pcg32` stream inside the sim. The clippy ban list applies.

At M0: `Passive`, a bot that never acts, and `Scripted`, which replays a fixed command list so `sim-cli selftest` exercises the delay queue and the RNG. M5b adds the first complete opponent (build-order executor, worker balancing, expansion, Watchtower placement, attack waves, defend rule). M6 adds difficulties, personalities and influence-map counter composition.

### `crates/game/`

The only Bevy crate and the `eonmark` binary. Bevy ECS is used here for presentation only.

| Module | Holds | Arrives |
|---|---|---|
| `main.rs` | Entry point: dispatches to `headless` or `app` | M0 |
| `cli.rs` | Hand-rolled flags: `--headless-run <ticks \| file.eonreplay>`, `--scenario <name>`, `--max-fps <n>`, `--replay-dir <dir>`, `--hash-every-tick`, `--screenshot <path>` (dev), `--quit-via-menu-after-seconds <s>` (dev, macOS), `--close-window-after-seconds <s>` (dev), `--background` (also `$EONMARK_BACKGROUND=1`), `--exit-after-seconds <s>`, `--seed <u64>`, `--data-dir <path>`; data-dir resolution (flag, `$EONMARK_DATA`, checkout `data/`, `<exe dir>/data`) | M0, M2 (done) |
| `app.rs` | Windowed app: 1280x800 window titled Eonmark, `load_rules`, `build_match` (skirmish with `LocalInput`, or a scenario with `ScenarioInput`), plugin wiring, the replay path, `--exit-after-seconds`, `--close-window-after-seconds` (writes `WindowCloseRequested`, the red-button proxy), `FrameLimiter` (`--max-fps` sleep in `Last`), `ScreenshotRequest`, `default_framing` (the camera start a scenario gets without `--camera`: `unit_looks` at 40 m, the `enemy_outpost` scenarios on their 200 vs 200 prologue at 55 m, M4b), frame-time lines on exit for `units200_auto`, the `enemy_outpost` scenarios (with the fight window, M4b) and `--max-fps`, the director for `enemy_outpost_walkthrough` (M4b) | M0; M2 (done; `SimHandle` moved to `sim_driver.rs`); states `Menu`, `Skirmish`, `Paused`, `GameOver` with the menus (M5b, M6) |
| `background.rs` | Background mode for automated windowed runs (`--background` or `EONMARK_BACKGROUND=1`): unfocused, always-on-bottom window in the top-left corner of the primary monitor, `WinitSettings::continuous()`, occlusion log; on macOS (`objc2-app-kit`, no `unsafe`) the `Accessory` activation policy and the focus hand-back to the previously frontmost app | M2 (done) |
| `monitor_loss.rs` | `MonitorLossPlugin`: a `Despawn` observer on `HasWindows` that detaches every window from a monitor bevy_winit despawns (display sleep, screen lock, an unplugged display), so Bevy 0.19.1's `linked_spawn` does not despawn the window and `exit_on_all_closed` does not end the session | M2 (done) |
| `sim_driver.rs` | The meeting point: `SimHandle` non-send resource (the only caller of `Sim::step`), `FixedUpdate` driver at `tick_rate_hz` with `Time<Virtual>::max_delta` 250 ms and the 8-ticks-per-frame cap (`discard_overstep`), `DriverStats`, `PendingCommands` (seq stamped at push), `InputSource` trait with `LocalInput`, `ReplayInput` and `ScenarioInput`, `drive_tick`, `ReplayFinished`, the recorder thread (`RecorderMessage`, `Recorder`, `writer_thread`, `finish_recorder_on_exit` in `Last` after `bevy_window::ExitSystems`), replay path helpers | M2 (done) |
| `present.rs` | `UnitEntities: HashMap<UnitId, Entity>` mirror synced after each tick (`sync_lifecycle`), per-unit `Interp` lerped by `Time<Fixed>::overstep_fraction()` in `PostUpdate` (`sync_transforms`), yaw from the facing vector in f32, `UnitVisuals` (one mesh and one material per kind and team, and each kind's look height) from `data/visuals.ron`: `body_mesh` builds every `Primitive` and `Body` look low-poly and flat-shaded (10 facets, at most 600 vertices, a lying `Body` along the facing), `look_top_y` gives its height analytically; `world_from_sim` / `sim_from_world` | M2 (primitives, done), M4b (one look per kind: S1 done), M7 (glTF `Visual::Scene`) |
| `camera.rs` | Pitch-locked perspective RTS camera: WASD and arrow pan, edge scroll, trackpad `Pixel` pan, wheel `Line` zoom, `PinchGesture` zoom; `RtsCamera` marker carries `MeshPickingCamera` | M0 (pan, zoom, pinch), M2 (done) |
| `selection.rs` | `MeshPickingPlugin` with `require_markers: true`, `Pickable` on units and buildings; click, shift add, double-click same kind on screen, screen-space drag box, ctrl+1..9 groups; `Selection` resource (units, or one own building since M3b, outlined on the ground); retained-gizmo rings, move markers and (M4b) the attack marker, a ring with a cross, as separate entities keyed by `UnitId`; `prune_dead` drops ids that left the sim (units that died and buildings destroyed in combat since M4a, cancelled sites) from the selection and the groups | M2 (done), M3b (buildings) |
| `health_bars.rs` | `HealthBars: HashMap<TargetId, Bar>`: a track and a team-coloured fill quad per damaged or selected unit or building, top-level entities (no `ChildOf`, no `Pickable`, no shadows) spawned and despawned lazily after a tick or a selection change (`sync_bar_targets`; unit maxima once per owner and kind from `SimView::unit_stats`, buildings from `SimView::building_hp`), billboarded in `PostUpdate` (`place_bars`) from `UnitVisuals::top_y`; one shared quad and three unlit materials; the dev-only `BarHierarchyCheckPlugin` (exit 1 on a parented bar or ring, `hierarchy_check:` line on exit) | M4b |
| `orders.rs` | Ray-plane ground hit (`Camera::viewport_to_world`, y = 0) to `Move` (shift queues), `Stop` (S), `AttackMove` (A then click; armed units fight on the way since M4a); since M3b a contextual right click: `Build` on an own site (resume), `Gather` on an own gather building, `SetRally` with a building selected; since M4b `Attack` on an enemy unit or building when `SimView::check_attack` accepts it (`contexts_of` ranks what the hovered entity and its `ChildOf` ancestors mirror, `context_order` picks the command), orders for the own selected units only (`own_selected_units`), `IssuedAttacks` (an `InvalidTarget` for a target that died before the command applied is not shown), the crosshair cursor in attack-move mode; `OrderMode::Place` for placement; ignores world clicks while `PointerOverUi` | M2 (done), M3b (context, placement mode), M4b (attack) |
| `ground.rs` | Grid mesh with vertex colors from the map's terrain (grass, forest floor, rock, water), render world only; drawn with `field.rs`'s `GroundMaterial` | M0 (plane), M3b (terrain) |
| `field.rs` | `GroundMaterial` = `ExtendedMaterial<StandardMaterial, FieldExt>`: the field texture (one RGBA8 texel per tile: R owner, G strength, B fog, A supply) and `field.wgsl` (embedded with `embedded_asset!`), which tints owned land 12% toward the team colour and draws the soft 1-tile border isoline before lighting; `sync_field` rewrites the texture after a stepped tick when the territory sources change. `scripts/render_smoke.sh` (CI game-linux) fails on a shader that does not compile, and `render_watchdog.rs` turns a pipeline that never becomes ready (an unresolvable `#import`) into an ERROR line |
| `render_watchdog.rs` | Render-app system: `error!` for every render or compute pipeline still not ready 20 s after it was first seen waiting (Bevy retries a missing `#import` silently every frame); runs in every windowed build | M3b | M3b (territory), M5a (supply), M7 (fog) |
| `buildings.rs` | `BuildingEntities: HashMap<BuildingId, Entity>` mirror synced after each tick (`sync_buildings`): one shared unit cube scaled to the footprint, a team-coloured material, a site in scaffold colour rising with its progress; buildings are `Pickable` | M3b |
| `command_card.rs` | The bottom bar's command card and selection panel: Build buttons for selected builders (ramped cost, greyed with the reason before the kind's age, at the town limit or when unaffordable, from `SimView::check_build_kind`; a click enters placement), Train buttons for a selected complete building (`SimView::check_train` decides, the tooltip is the rejection's message), Cancel for a selected site, and since M4b an Attack-move button for armed selections without builders and the research panel: on a selected Scriptorium one Research button per tech line with the line's next level (`research_actions`; `SimView::check_research` greys it, the second line is the quoted cost or the blocker, the tooltip what the level requires); the panel names the selection (unit counts, or the building with progress, tier, workers, the techs researched there and the queue) and its hit points for one selected unit of either player or the selected own building (another player's building is not selectable yet) | M3b, M4b |
| `placement.rs` | `OrderMode::Place`: a translucent ghost of the footprint under the cursor, green or red by `SimView::check_build` (cached per tile, builder and tick), the reason on the message line, a footprint outline on the ground; a left click issues `Build` with the nearest idle selected builder | M3b |
| `age_hud.rs` | The age strip hanging under the right end of the top bar: the age readout and the Advance button, which reads the tech count ("Advance: 1 / 2 techs") or the next age and its cost ("Advance to Masonry Age: 250 Grain 100 Lore"), greyed exactly when `SimView::check_advance_age` rejects, the reason in the tooltip under the strip, a click sends `AdvanceAge`, hidden in the last age | M4b |
| `economy_hud.rs` | Resource bar (stockpile, credited rate per 30 s, amber while the Yield Cap discards income) and pop readout on the top bar, one line each that never wraps, and the message line fed by `SimEventMsg` (the local player's rejections in the words of `data/rules/strings/en.ron`, building completions, and since M4b `notice_text`: their techs researched, their age reached, buildings destroyed or lost, their Town at 0 hit points); `HUD_KEYS`, every HUD string key the game reads with the placeholders it fills | M3b, M4b |
| `palette.rs` | The matte ground greens, grid, sky, sun and ambient colours; `TEAM` colours, ring, marker, drag-box and HUD colours | M0, M2 (done) |
| `hud.rs` | `PointerOverUi` from the picking `HoverMap`; top bar, bottom bar, Stop button; the `hud_click` automated check. The M3b resource bar and message line live in `economy_hud.rs`, the command card and selection panel in `command_card.rs` | M2 (skeleton, done), M3b, M4b, M7 (minimap) |
| `menu/` | Main menu, pause menu, game-over screen | M5b (New Game), M6, M7 |
| `macos_menu.rs` | `cfg(target_os = "macos")`: muda menu with a custom `Quit Eonmark` item (id `quit`, Cmd+Q) routed to `AppExit::Success`; `install_menu` (Startup) and `drain_menu_events` (Update) take `NonSendMarker`; `--quit-via-menu-after-seconds` proxy | M2 (done) |
| `audio.rs` | Thin facade over `bevy_audio` | M7 |
| `dev_tools.rs` | FPS overlay, hash readout, `DriverStats`, selection and command counters, mesh/material counts, `bevy_egui` panels; behind the `dev` feature | M0 (FPS), M2 onward |
| `frame_stats.rs` | Frame-time lines printed on exit with the `frame_stats: ` prefix: mean, p95 and max frame time, the rate, the worst frames, focused and covered frame counts, ticks and dropped ticks; for `units200_auto` the arrival proxy; since M4b, for the `enemy_outpost` scenarios, the fight window of acceptance line 2 (`FightWindow`: frames within 20 ticks of a `Hit` while both players keep 50 armed units, with their mean, p95 and fps, the fighting ticks, `on_screen_max` (the most armed units on screen in one fight frame, by the drag box's projection over the view between the HUD bars) and `on_screen_min` (the fight frame that showed the smallest share of the armed units then alive, as `<on screen>/<alive>`, so a pan or zoom during the fight shows), and each player's armed units at the first `Hit`) | M2 (done), M4b |
| `director.rs` | `--scenario enemy_outpost_walkthrough` only: a table of beats (`walkthrough_beats`, derived from `sim::scenarios::enemy_outpost_beats`) that moves the camera and the selection when the sim reaches their ticks, so a windowed run shows the research card, the queues, the Advance button and the siege; presentation only, it never pushes a command | M4b |
| `headless.rs` | `--headless-run` with `MinimalPlugins`, printing `tick=<n> hash=0x<16 hex>` as the last stdout line: `<ticks>` steps the skirmish; `<file.eonreplay>` re-simulates a replay through `ReplayInput` and `drive_tick` and exits 0 only when the final hashes match (the CI hash-parity line on both OSes) | M0 (`<ticks>`), M2 (`<replay>`, done) |

Features: `dev` (dynamic linking, dev tools, egui panels; never shipped) and `ci_testing` (owner screenshot capture; not used in CI). The crate must keep compiling on Linux; CI's `game-linux` job enforces this with clippy, then builds the game and renders the walkthrough under Xvfb on lavapipe (`scripts/render_smoke.sh`), failing on any ERROR log line. Running on Linux is unsupported for players.

At M0: a window titled Eonmark, a flat matte ground plane with a grid, a directional light, a pan/zoom/pinch camera, a `FixedUpdate` sim driver, `--headless-run <ticks>`, and an FPS overlay, inspector and a sim panel (seed, tick, hash) under `dev`.

At M2: the sim runs in `FixedUpdate` from `sim_driver.rs` behind an `InputSource`; units appear as team-coloured primitives interpolated between ticks; left click, shift, double-click, drag box and control groups select; right click moves, S stops, A then click attack-moves; a two-bar HUD with a Stop button blocks world picking; every windowed session writes a replay from the recorder thread; Cmd-Q goes through the muda menu into `AppExit`; `--headless-run <file.eonreplay>` verifies a replay without a window; `--scenario`, `--max-fps`, `--replay-dir`, `--hash-every-tick`, `--screenshot` and `--quit-via-menu-after-seconds` drive the automated checks in [PLAYTEST.md](PLAYTEST.md).

### `crates/sim-cli/`

Headless tooling that runs on an Ubuntu CI runner with no GPU.

| Subcommand | Does | Arrives |
|---|---|---|
| `selftest --ticks N --seed S` | Runs the scripted match twice on two threads and compares hashes | M0 |
| `data-check [dir]` | Loads and validates everything under `data/` | M0 |
| `hash-dump --ticks N --seed S` | Prints the whole-state hash after every tick of the scripted match | M0 |
| `hash-dump <replay> --every N` | Re-simulates a replay and prints `tick hash units buildings economy territory tech pathing rng map meta` every N ticks, for bisecting | M1 (done) |
| `verify <replay>` | Re-simulates a replay and prints exactly one of `OK final_hash=0x... ticks=N`, `DIVERGED at tick N (subsystem: X)`, `SIM VERSION MISMATCH (...)`, `RULES CHANGED since recording (...)`; exit 0 only for OK | M1 (done) |
| `bench --units N --ticks T` / `bench --astar --budget B` / `bench --territory --sources S --edits E` / `bench --units N --ticks T --combat` | Mean and p95 step time plus arrival percentage for N movers crossing the map, the cost of one saturated A* tick, mean and p95 incremental territory recompute over E random source edits with a full-recompute equality check (exit 1 on mismatch), or mean, p95 and fighting-tick step time while N units of four line kinds fight, half on each side (`sim::scenarios::combat_bench`; exit 1 when fewer than 100 ticks were fighting); wall clock is allowed in sim-cli, not in sim | M1 (done), M3a (`--territory`, done), M4a (`--combat`, done); `--bots` later |
| `fuzz --ticks N --seed S --cases C` | Seeded random command streams run twice and compared | M1 (done) |
| `record --scenario <name> --ticks N --out <file> [--hash-every-tick]` | Records a scripted scenario (any name in `sim::scenarios::NAMES`: `move_500`, `move_500_short`, `group_spiral`, `snapshot_restore`, from M2 `units200`, `units200_auto`, `scripted_moves`, `hud_click`, from M3a `economy_second_town`, from M3b `economy_walkthrough`, from M4a `combat_skirmish`, and from M4b `unit_looks`, `enemy_outpost` and `enemy_outpost_walkthrough`) to an `.eonreplay`; the goldens are made with it | M1 (done), M2 (names), M3a (name), M3b (name), M4a (name), M4b (names) |
| `play-bots` | Seeded bot-vs-bot games in parallel with a CSV summary | M5b |

A global `--data <dir>` flag (default `data`) selects the data directory. Subcommands that have not landed yet exit with code 2 and print `not implemented until <milestone>`. Scenario command streams are defined once, in an engine-free crate, and shared by `record` and the tests in `crates/sim/tests/m1.rs`, so a fixture and the test that checks it can never disagree about the input.

### `data/`

All tuning as RON. See [DATA_FORMAT.md](DATA_FORMAT.md) for the conventions and the file inventory. Licensed like code (MIT OR Apache-2.0).

At M0: `data/rules/rules.ron` with the full v0.1 field set (values for later milestones are validated but unused until those milestones), `data/rules/resources.ron`, `data/maps/plains_1v1.ron`, and a README in `data/`, `data/rules/`, `data/maps/` and `data/ai/`. `data/ai/` holds only its README until M5b.

### `assets/`

Runtime assets: fonts, UI 9-slice panels, icons, glTF models, OGG clips, `ATTRIBUTION.md` and `LICENSE-ASSETS.md`. Custom shaders are not here: `field.wgsl` is embedded from `crates/game/src` with `embedded_asset!` (see the shader trap in [IMPLEMENTER_NOTES.md](IMPLEMENTER_NOTES.md)). Only CC0, CC-BY-4.0 and OFL-1.1 material. `scripts/check_assets.sh` fails CI if any non-allowlisted file lacks an attribution row or exceeds the size caps. See [ART_STYLE.md](ART_STYLE.md).

Arrives M7, except the font and UI pack which may land earlier with the HUD (M3b).

### `scripts/`, `justfile`

`bundle.sh` assembles `Eonmark.app` and signs it last (M8). `check_assets.sh` and `check_trademark.sh` are CI gates (M0). `render_smoke.sh` renders a scenario under Xvfb and lavapipe and fails on any ERROR line (M3b, CI's game-linux job). `m4b_outpost_drive.sh` plays ROADMAP M4b acceptance line 1 on the interactive `--scenario enemy_outpost` with real X input (xdotool under Xvfb and lavapipe) and verifies the recorded replay (M4b; run by hand, not in CI). The `justfile` wraps the CI commands so `just ci` runs locally what GitHub runs remotely; it skips a missing optional tool with an install hint instead of failing. There is no `xtask` crate.

### `.github/`

`ci.yml` runs five jobs: `check` on macOS (fmt, clippy, nextest, doc, machete, typos, asset and trademark gates, single-Bevy check, the game's headless replay of the `move_500` fixture, `sim-cli verify --release` of that fixture and, from M4a, of `combat_skirmish`, and from M4b of `enemy_outpost_walkthrough`), `headless` on Ubuntu (sim, rules, ai and sim-cli tests with `PROPTEST_CASES=256`, from M4b the ignored `enemy_outpost` tests, data-check, selftest, `verify --release` of the `move_500`, `group_spiral`, `snapshot_restore`, `economy_second_town`, `economy_walkthrough`, `combat_skirmish` and `enemy_outpost_walkthrough` fixtures, engine boundary check, `pathfinding`-is-dev-only check, the same headless replay run and, from M4b, the game's `--headless-run` of `enemy_outpost_walkthrough`), `game-linux` on Ubuntu (clippy of the game crate, then `cargo build` and the Xvfb + lavapipe render smoke test `scripts/render_smoke.sh` over the `economy_walkthrough` and, from M4b, the `unit_looks` scenario, which fails on any ERROR log line and uploads the `render-smoke` artifact), `deny` (cargo-deny) and `hash-parity` (diffs the `tick=1200 hash=...` replay line and the `move_500` and `combat_skirmish` `OK final_hash=...` lines produced on each OS; until M2 the first line was the 200-tick skirmish smoke). `release-check.yml` builds release and bundles on pushes to main. `release.yml` publishes on tags. Dependabot ignores Bevy and the two egui crates for minor and major bumps. See [RELEASING.md](RELEASING.md).

### `docs/`

This file, [DETERMINISM.md](DETERMINISM.md), [DATA_FORMAT.md](DATA_FORMAT.md), [DEPENDENCIES.md](DEPENDENCIES.md), [ART_STYLE.md](ART_STYLE.md), [BUILD_TIMES.md](BUILD_TIMES.md), [NAME.md](NAME.md), [PLAYTEST.md](PLAYTEST.md), [RELEASING.md](RELEASING.md), [ROADMAP.md](ROADMAP.md), per-subsystem design docs under `design/`, and architecture decision records under [adr/](adr/README.md). Each design doc states Eonmark's own target numbers and an acceptance checklist and arrives with the milestone that implements its system.

## One frame, end to end

The sim driver and presenter landed in M2. This is the shape they take (schedule names are Bevy 0.19.1's).

```
one rendered frame in crates/game

 mouse / keyboard / trackpad / macOS menu
          |
          v
 [PreUpdate]
   bevy_picking     hover map from the mesh backend (units only, require_markers) and the UI backend
   hud.rs           PointerOverUi = any hovered entity has a Node
   sim_driver.rs    reset_frame_budget (ticks this frame = 0)
          |
          v
 [Update]
   camera.rs        moves the camera in f32; never touches the sim
   selection.rs     click / shift / double-click observer on Pointer<Click>, drag box, ctrl+1..9;
                    maintains Selection (render-side state); skipped while PointerOverUi
   orders.rs        right click -> ground_hit (ray-plane y = 0) -> Command::Move (shift queues);
                    S -> Stop; A then click -> AttackMove; skipped while PointerOverUi
   hud.rs           Stop button (Pointer<Click> observer) -> Command::Stop
                    (M3b+: command-card clicks -> Build / Train / Research)
   macos_menu.rs    drains MenuEvent; the `quit` id becomes AppExit::Success
   app.rs           --exit-after-seconds -> AppExit::Success; --screenshot
          |
          v
 PendingCommands (Resource)
   Vec<PlayerCommand> for the local player; push_local stamps seq in issue order
          |
          v
 [FixedUpdate, Time<Fixed>::from_hz(20), Time<Virtual>::max_delta 250 ms, runs 0 to 8 times]
   sim_driver.rs step_sim -> drive_tick, for each tick T that is due:
     cmds = InputSource::commands_for(T)     the commands issued during tick T
         Local    : drains PendingCommands, sorted by (player, seq)
         Scenario : the scripted stream's commands for T, then the local ones
         Replay   : the recorded TickBatch for T (Some(empty) when none; end -> ReplayFinished)
         None     : stall; no step this frame, counted in DriverStats
     sim.step(&cmds)                     the only mutation of sim state:
                                         queues cmds for tick T + cmd_delay, runs the AI and
                                         queues its commands the same way, then applies every
                                         command due at T in (player, seq) order
     recorder.send(Tick { T, cmds })     every tick
     recorder.send(Hash { T, hash, sub }) every 20 ticks (every tick under --hash-every-tick)
                                         the writer thread appends; flushes every 20 ticks
   present.rs sync_lifecycle (after SimSystems::Step)
                    spawn one Entity per new UnitId (mesh + material per kind and team,
                    Pickable::default()), despawn missing ids, Interp::advance for the rest;
                    UnitEntities: HashMap<UnitId, Entity> lives here, on the render side
   buildings.rs sync_buildings, field.rs sync_field (after SimSystems::Step, M3b)
                    building entities likewise; the field texture rewritten when the
                    territory sources changed (fog and supply join it in M7 and M5a)
   after 8 ticks in one frame: discard_overstep, the remainder is dropped as time dilation
          |
          v
 [PostUpdate, before TransformSystems::Propagate]
   present.rs sync_transforms   Transform = lerp(prev, curr, Time<Fixed>::overstep_fraction());
                                yaw derived in f32 from the sim's facing vector
   selection.rs                 ring entities follow their unit's interpolated Transform
   hud.rs                       (M3b+) read SimView for stockpiles, rates, cap state, population
          |
          v
 [Last]
   bevy_window close_when_requested        despawns a window whose close button was clicked
   bevy_window exit_on_all_closed          (ExitSystems) no window left -> AppExit::Success
   sim_driver.rs finish_recorder_on_exit   .after(ExitSystems): on any AppExit this frame,
                                           final Hash, Finish, join the writer (the runner exits
                                           right after this frame, so the order is load-bearing)
   app.rs limit_frame_rate                 --max-fps sleep
          |
          v
 render through wgpu on Metal, PresentMode::AutoVsync
```

Three consequences fall out of this shape.

- Frame rate never changes the result. The sim sees tick-stamped commands and nothing else, so 30 fps and 120 fps produce the same hash. Game speed (M6) changes how many ticks are due per frame within the 8-tick cap; replays are unaffected.
- The scripted AI's commands never appear in `PendingCommands` or the replay. The AI runs inside `step`, its commands are stamped and delayed exactly like a human's, and verification reproduces them by running the same AI. In-flight commands sit in the sim's own delay queue, which is hashed state, so a snapshot carries them.
- A network peer is one more `InputSource`. Lockstep multiplayer is documented, not built, in v0.1. The headless replay runner is the first proof: it swaps `LocalInput` for `ReplayInput`, keeps `drive_tick`, drops the recorder, and runs the same `Sim::step` calls without a window.

## Cross-cutting concerns

### Determinism

Covered in full by [DETERMINISM.md](DETERMINISM.md). The short form: fixed-point numbers, ordered containers, monotonic ids, one seeded RNG, commands sorted by `(player, seq)`, derived caches never hashed, and a cross-OS hash-parity job in CI.

### Data-driven numbers

Covered by [DATA_FORMAT.md](DATA_FORMAT.md). `crates/rules` is the only place that converts authored units to ticks and integers, and it does so once at load. The sim receives `i32`, `i64` and `Fx` and nothing else. `rules_hash` is stored in every replay header so an edited RON file is reported as `RULES CHANGED` rather than mistaken for a sim bug.

### Error handling

Commands are validated at apply time against ownership, age, tech, borders, stockpiles and population cap. A rejected command emits `SimEvent::CommandRejected { player, seq, reason }` and never panics. Data errors are reported by `rules::Error` with file path and field. Overflow in the sim is a deterministic panic in release, by design.

### Testing

`cargo test -p sim` is the inner loop: no GPU, no Bevy compile, under 10 seconds for the non-ignored suite. Golden replay fixtures up to 5000 ticks run there; longer ones are `#[ignore]` and verified by `sim-cli verify` in release in the headless CI job. The game crate is smoke-tested by `--headless-run` on a fixture replay using `MinimalPlugins`. Property tests use `proptest` with 256 cases in CI (`PROPTEST_CASES=256`, or `CI` set) and fewer locally to keep `cargo test -p sim` under 10 s: 8 in `m1.rs` (32 until the M4a combat behaviour slice, then 16 until the M4b combat UI slice, each lowered for that test-time budget), 16 in `m3a.rs` (32 until the M4b combat UI slice) and 8 in `m4a.rs` (see [DEPENDENCIES.md](DEPENDENCIES.md)). The in-house A* is checked against the `pathfinding` crate as a dev-only oracle.

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
| M4a / M4b | Tech, ages, combat | Research panel, age indicator, health bars, unit silhouettes, attack orders, the `enemy_outpost` scenarios with the walkthrough director and the fight window | Tech and combat design docs; `bench --combat`, the criterion `combat_step` and the `combat_skirmish` golden (M4a); the `enemy_outpost_walkthrough` golden and the `scripts/m4b_outpost_drive.sh` real-input drive (M4b) |
| M5a / M5b | Attrition, supply, annexation, victory; first complete bot | Victory overlay, New Game | `play-bots`, first-match fixture |
| M6 | Difficulties, personalities, influence maps | Game speed, pause, game-over stats | Fun gate, match log |
| M7 | Visibility grid | Fog, minimap, menus, glTF, audio | Finalized [ART_STYLE.md](ART_STYLE.md) |
| M8 | none | none | `bundle.sh`, release workflow, v0.1.0 |
| M9 | Second faction as data | Bevy 0.20 migration (gated) | [adr/0002](adr/0002-bevy-0-20-migration.md) |
