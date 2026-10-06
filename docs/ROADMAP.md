# Eonmark roadmap

This document is the plan of record for Eonmark from the empty repository to the v0.1.0 release and the first post-release milestone. It lists every milestone in order, its deliverables, its acceptance criteria as a checklist, and the rules for closing it. It also records what is deliberately deferred, the risk register, and the open questions whose default answers the plan assumes. The implementing agent works from this file and from [IMPLEMENTER_NOTES.md](IMPLEMENTER_NOTES.md). Scope changes go through this file and an ADR under [adr/](adr/); nothing else changes scope.

## How to read this document

A **session** is one agent run that ends in a merged pull request with CI green on every job. Estimates below are in sessions. A milestone is closed when its acceptance checklist passes under the rules in [Closing a milestone](#closing-a-milestone).

Acceptance lines marked **[owner]** need the repository owner to play, look or sign something. Every [owner] line has an **automated proxy** (a scripted scenario replay and, where a visual is involved, a `ci_testing` screenshot) that the agent runs and attaches to the PR before asking the owner.

| Milestone | Title | Estimate (sessions) | Depends on |
| --- | --- | --- | --- |
| [M0](#m0-skeleton-toolchain-workspace-ci-window-on-macos-governance) | Skeleton: toolchain, workspace, CI, window on macOS, governance | 4-5 | none |
| [M1](#m1-deterministic-sim-core-movement-and-the-headless-cli) | Deterministic sim core, movement and the headless CLI | 4 | M0 |
| [M2](#m2-presentation-camera-unit-control-and-macos-quit-handling) | Presentation, camera, unit control and macOS quit handling | 3 | M1 |
| [M3a](#m3a-sim-economy-towns-territory-field-and-construction) | Sim: economy, Towns, territory field and construction | 3 | M2 |
| [M3b](#m3b-game-build-placement-command-card-resource-bar-and-the-border-visual) | Game: build placement, command card, resource bar and the border visual | 3 | M3a |
| [M4a](#m4a-sim-tech-lines-ages-and-combat) | Sim: tech lines, ages and combat | 3 | M3a |
| [M4b](#m4b-game-research-panel-age-indicator-health-bars-and-unit-looks) | Game: research panel, age indicator, health bars and unit looks | 2 | M4a, M3b |
| [M5a](#m5a-sim-attrition-supply-annexation-and-victory) | Sim: attrition, supply, annexation and victory | 3 | M4a, M4b |
| [M5b](#m5b-ai-the-first-complete-skirmish-opponent-and-the-first-playable-match) | AI: the first complete skirmish opponent and the first playable match | 3 | M5a |
| [M6](#m6-fun-gate-difficulty-personalities-pacing-game-speed-and-logged-human-matches) | Fun gate: difficulty, personalities, pacing, game speed and logged human matches | 4 | M5b |
| [M7](#m7-fog-of-war-minimap-menus-look-and-audio-pass) | Fog of war, minimap, menus, look and audio pass | 3-4 | M6 |
| [M8](#m8-v010-release-app-bundle-and-contributor-launch) | v0.1.0 release, .app bundle and contributor launch | 3 | M7 |
| [M9](#m9-post-release-second-faction-as-pure-data-bevy-020-migration-gated) | Post-release: second faction as pure data, Bevy 0.20 migration (gated) | 2-3 | M8 |

Running totals:

| Through | Low | High | Design figure |
| --- | --- | --- | --- |
| M6 (fun-gated skirmish) | 32 | 33 | ~33 sessions |
| M8 (v0.1.0) | 38 | 40 | ~43 sessions (the design's figure also covers M9) |
| M9 (post-release) | 40 | 43 | ~43 sessions |

The design quotes "~33 sessions to a fun-gated skirmish and ~43 to v0.1.0". The per-milestone estimates sum to 38-40 through M8 and 40-43 through M9. Treat ~43 as the budget for everything in this file; the three-session gap is slack, not a cut.

## Order of work and dependency graph

Work the milestones strictly in this order:

```
M0 -> M1 -> M2 -> M3a -> M3b -> M4a -> M4b -> M5a -> M5b -> M6 -> M7 -> M8 -> M9
```

Dependencies (an arrow means "must be closed before"):

```
M0 -> M1 -> M2 -> M3a -> M3b
                   |       |
                   v       v
                  M4a --> M4b
                   |       |
                   +---+---+
                       v
                      M5a -> M5b -> M6 -> M7 -> M8 -> M9
```

Notes on the graph:

- M3a and M4a are both sim-only and both depend on M3a's predecessors; M4a formally depends only on M3a, but the order above runs M3b before M4a so the border visual lands before combat work begins.
- M4b needs both M4a (combat exists) and M3b (the command card exists).
- M5a needs M4a and M4b because the victory overlay (a game deliverable) rides on the combat and HUD work.
- Every "a" milestone is sim-side and testable with `cargo test -p sim` and `sim-cli` on any OS. Every "b" milestone is game-side and needs a Mac for the owner proxy screenshots.

## M0: Skeleton: toolchain, workspace, CI, window on macOS, governance

### Goal

A public, contributor-ready github.com/tonianev/eonmark that builds with one command on Apple Silicon, opens a Bevy window with a matte ground plane and camera stub, passes CI on macos-26 and ubuntu-24.04, has data-check and selftest stubs, and has measured build times and a recorded name check.

### Deliverables

- Workspace `Cargo.toml` (resolver 3, exact pins including `bevy_egui =0.40.1` and `bevy-inspector-egui =0.37.0`, lints, dev/ci/release profiles with sim overrides), `rust-toolchain.toml` 1.99.0, `.cargo/config.toml` (MACOSX_DEPLOYMENT_TARGET only), `.config/nextest.toml`, `clippy.toml` for sim/rules/ai, `deny.toml`, `typos.toml` (spelling only), `scripts/check_trademark.sh`, `justfile` with graceful tool skipping.
- `crates/sim` (Fx + FxVec2 + dist_sq_i64, monotonic ids, `Sim::new`/`step`/`hash` stub, Pcg32 in state, clippy bans), `crates/rules` (schema for rules.ron + resources.ron with `deny_unknown_fields`, loader, validator, `rules_hash`), `crates/ai` (`AiController` trait + `Passive` bot), `crates/game` (window 1280x800 titled Eonmark, flat ground plane, DirectionalLight, camera stub with WASD + trackpad pan + wheel/pinch zoom, FPS overlay under `dev`, `dev` and `ci_testing` features, compiles on Linux), `crates/sim-cli` (selftest, data-check).
- `ci.yml` (check on macos-26, headless + deny + game-linux on ubuntu-24.04, hash-parity), `release-check.yml`, `release.yml` stub, `dependabot.yml` with ignore rules, issue forms, tiered PR template, CODEOWNERS, labels created.
- README (with the single nominative sentence), CONTRIBUTING (dual-license clause + CC0 asset dedication + claim protocol + pinned-docs rule), GOVERNANCE (Today / 3+ contributors), CODE_OF_CONDUCT (Contributor Covenant 3.0, both placeholders filled), AI_CONTRIBUTIONS, SECURITY, LICENSE-MIT, LICENSE-APACHE, `docs/ARCHITECTURE.md`, `docs/DETERMINISM.md`, `docs/DATA_FORMAT.md`, `docs/BUILD_TIMES.md`, `docs/NAME.md`, `docs/DEPENDENCIES.md`, `docs/adr/0001-license.md`, `docs/ROADMAP.md` (this file), `docs/design/` headings per subsystem.

### Acceptance

- [ ] `cargo run -p game --features dev` opens a 1280x800 window titled 'Eonmark' showing a flat matte ground plane, grid gizmo and FPS counter; two-finger scroll pans, pinch and wheel zoom, WASD pans; closing via the red button exits with code 0 (`echo $?`).
- [ ] `cargo run -p sim-cli -- selftest` runs 1000 scripted ticks twice on two threads, prints two identical xxh3 hashes and exits 0.
- [ ] `cargo run -p sim-cli -- data-check data/` exits 0; adding an unknown field to `data/rules/rules.ron` makes it exit 1 printing the file path and field name (unit test in `crates/rules`).
- [ ] `cargo tree -p sim -e normal | grep -E 'bevy|glam|wgpu|winit'` prints nothing (same for rules, ai, sim-cli); `cargo clippy -p sim --all-targets -- -D warnings` (and `-p rules`, `-p ai`) fails when `let x: f32 = 1.0;` is added (demonstrated in a scratch branch, documented in CONTRIBUTING).
- [ ] `cargo clippy -p game --features dev --profile ci -- -D warnings` passes (proves the bevy_egui 0.40.1 + inspector 0.37.0 pair resolves one egui); `cargo tree -i bevy_ecs --depth 0 | wc -l` prints 1.
- [ ] On ubuntu-24.04 (CI game-linux job) `cargo clippy -p game --all-targets --locked --profile ci -- -D warnings` passes after `apt-get install libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev`.
- [ ] `just ci` passes locally (fmt --check, clippy workspace + game/dev, nextest --cargo-profile ci --profile ci, test --doc, doc, machete, typos, check_assets, check_trademark) and GitHub Actions is green on main for check, headless, deny, game-linux and hash-parity; `release-check.yml` is green on main and its `otool -L target/release/eonmark | grep -q bevy_dylib` check passes (no match).
- [ ] `grep -c '\[NOTE' CODE_OF_CONDUCT.md` prints 0 and the Contributor Covenant 3.0 attribution line is present; `scripts/check_trademark.sh` exits 0 on the tree and exits 1 when the phrase is added to a file under `crates/`. (Note: `grep -c` exits 1 when it prints 0; check the printed count, not the exit code. See [Known traps](IMPLEMENTER_NOTES.md#known-traps).)
- [ ] `docs/BUILD_TIMES.md` records cold `cargo build -p game --features dev`, incremental after `touch crates/game/src/main.rs`, `cargo test -p sim`, and the first cold and warm CI check-job wall times from `cargo build --timings` and the Actions UI, with chip, macOS and Xcode version noted (and whether the CoreAudio bindings built against the Xcode 27 SDK or the audio-less fallback was needed; at M0 they built, no fallback).
- [ ] `docs/NAME.md` records, with dates and screenshots: crates.io 404, Steam 0, itch 0, GitHub 0, tmsearch.uspto.gov classes 9 and 41, EUIPO eSearch plus, WIPO Global Brand Database, domain availability (eonmark.com/.dev/.games), GitHub user/org 'eonmark', itch.io slug; no placeholder crate is published.
- [ ] `gh repo view tonianev/eonmark` shows public visibility, MIT/Apache license detected, Discussions enabled, topics rts/real-time-strategy/bevy/rust/macos/game; `gh label list` shows good first issue, help wanted, needs-design, status:claimed, status:blocked, platform:macos, area:sim, area:rules, area:ai, area:game, area:ui, area:art, area:docs, area:infra, kind:bug, kind:feature, kind:balance, kind:docs, kind:question.

### Good first issues

- Add a `just fmt` recipe and document it in CONTRIBUTING (any OS). Done at M0: `justfile` has `fmt` and CONTRIBUTING lists it.
- Write `data/rules/README.md` explaining the deciseconds-and-tiles authoring convention with one worked example (any OS). Done at M0.
- Add a second mirrored map under `data/maps/` and a `rules::map` test that checks its symmetry (any OS).
- Make the validator reject `pop_cap_per_arms_level: 0` and `rules_version: 0` in `crates/rules/src/lib.rs`, with a test (any OS).

### Estimate

4-5 sessions.

## M1: Deterministic sim core, movement and the headless CLI

### Goal

A Bevy-free sim that spawns units on the plains_1v1 tile map, moves them along budgeted in-house A* paths under Move/Stop commands at 20 Hz in fixed-point, hashes its state with sub-hashes, records and verifies replays with exact snapshot/restore, and proves determinism on two operating systems.

### Deliverables

- sim: `ids.rs`, `map.rs` (128x128 tiles, u8 cost + generation, components, `plains_1v1.ron` loader), `command.rs`, `state.rs` (step, tick, hash, sub_hashes, exact snapshot/restore with eager derived rebuild, events, `SimView`), `pathing.rs` (own A* with `resume(budget)`, request queue, generation-keyed cache), `movement.rs` (arrival steering, spatial grid separation, circle push, component-aware spiral offsets), `replay.rs` (`MatchSetup` header with `rules_hash`, tick batches, hash every 20 ticks, reader), `hash.rs`.
- sim-cli: `verify` (all four outcomes), `bench`, `hash-dump`, `fuzz`; golden fixtures with `.hash` files (`move_500_short` <= 5000 ticks for `cargo test`; `move_500` full 1200-tick version and `group_spiral`, `snapshot_restore` for `sim-cli verify`).
- Tests: double-run, snapshot/restore at 300 of 1200 then spawn 50 units, restore-clear-caches-step, proptest random commands (32 cases locally, 256 in CI), path cache on/off equivalence, `path_exists_iff_connected` against the `pathfinding` oracle, `sort_keys_are_total_orders`, dist_sq corners; criterion benches `step_500_units` and `astar_budget`.
- CI: hash-parity job asserts ubuntu and macos final hashes of `move_500` are equal; `cargo test -p sim` wall time recorded in `docs/BUILD_TIMES.md`.
- `docs/design/pathing.md` with the algorithm, budgets, resume API and the flow-field re-entry condition.

### Acceptance

- [ ] `cargo test -p sim` (non-ignored suite) passes in under 10 s on the dev Mac and includes `same_seed_same_hash_two_threads`, `snapshot_restore_matches_uninterrupted_including_spawns`, `restore_rebuilds_caches`, `proptest_random_commands_are_deterministic`, `path_cache_is_transparent`, `path_exists_iff_connected`, `sort_keys_are_total_orders`, `dist_sq_i64_map_corners`, and the short goldens.
- [ ] `cargo run -p sim-cli --release -- verify crates/sim/tests/fixtures/move_500.eonreplay` prints `OK final_hash=0x...` matching the `.hash` file and exits 0; flipping one command byte in a copy prints `DIVERGED at tick N (subsystem: units)` and exits 1; editing `data/rules/rules.ron` without bumping `rules_version` prints `RULES CHANGED since recording` and exits 1.
- [ ] `cargo run -p sim-cli --release -- bench --units 500 --ticks 1200` reports mean step < 5 ms and p95 < 10 ms on the dev Mac while 500 units cross the map and >= 99% arrive within 3 tiles of their (component-corrected) goal; `bench --astar --budget 4000` reports the per-tick cost of a saturated budget < 2 ms; numbers appended to `docs/BUILD_TIMES.md`.
- [ ] A scripted sim-cli recording killed with `kill -9` at tick ~600 leaves a file that `verify` accepts (exit 0, reports ticks verified).
- [ ] `cargo run -p sim-cli -- hash-dump <replay> --every 1` prints per-tick sub-hashes for units, pathing, rng; `docs/DETERMINISM.md` explains bisecting with it.
- [ ] CI hash-parity job is green: ubuntu-24.04 and macos-26 final hashes of `move_500` are byte-identical.
- [ ] `cargo tree -p sim -e normal` lists only fixed, serde, postcard, xxhash-rust, rand_pcg, ron, rules and their transitive deps (no slotmap, no pathfinding).

### Good first issues

- Tune the path cache LRU size and A* budget in `rules.ron` with a bench table in the PR (any OS).
- Add a `--csv` flag to `sim-cli bench` (any OS).

### Estimate

4 sessions.

## M2: Presentation, camera, unit control and macOS quit handling

### Goal

The player sees the sim as team-colored primitives, selects units with click/box/groups, issues move orders while the sim runs in FixedUpdate with interpolation; every session records a verifiable replay from an off-thread writer, and Cmd-Q flows through Bevy.

### Deliverables

- game: `sim_driver.rs` (FixedUpdate 20 Hz, max_delta 250 ms, 8-ticks-per-frame cap, `InputSource` trait with Local and Replay sources, `PendingCommands`, replay writer thread to the `directories` data dir or `--replay-dir`), `present.rs` (UnitId -> Entity mirror, prev/current lerp via overstep_fraction, Visual -> primitive + team material), `camera.rs` finished, `selection.rs` (`MeshPickingSettings { require_markers: true }`, `Pickable` on units only, screen-space drag box, shift add, ctrl+1..9, double-click same kind), `orders.rs` (ray-plane ground hit -> Move/Stop/AttackMove placeholder), retained gizmo selection rings and move markers, HUD skeleton with `Pickable::IGNORE` roots.
- `macos_menu.rs`: muda 0.21 menu with About/Hide/Services and a custom 'Quit Eonmark' (Cmd+Q) `MenuItem`; setup and `MenuEvent::receiver().try_recv()` drain systems take `NonSendMarker`; drained into `AppExit::Success`.
- `headless.rs`: `--headless-run <replay>` with `MinimalPlugins` (no `ci_testing` feature); dev flags `--scenario <name>` and `--max-fps N`.
- `docs/PLAYTEST.md` controls checklist.

### Acceptance

- [ ] `cargo run -p game --features dev -- --scenario units200` spawns 200 units; drag-box all, right-click across the map; all arrive within 60 s game time; FPS overlay stays >= 60 on the dev Mac in the dev profile with no periodic hitch visible in the frame-time graph at the 1 s replay flush.
- [ ] Play 2 minutes, press Cmd-Q: the process exits with code 0 and `sim-cli verify` on the newest replay exits 0; `kill -9` mid-session also leaves a replay that verifies.
- [ ] `--scenario scripted_moves --max-fps 30` and `--max-fps 120` produce replays with identical final hashes.
- [ ] `cargo run -p game --profile ci -- --headless-run crates/sim/tests/fixtures/move_500.eonreplay` exits 0 within 10 s with no window; added to the macos-26 check job reusing the ci-profile artifacts.
- [ ] **[owner]** `docs/PLAYTEST.md` checklist signed: two-finger scroll pans, pinch zooms, wheel zooms, WASD pans, edge scroll pans, camera travel identical at 30 and 120 fps, ctrl+1 then 1 reselects, double-click selects same kind on screen, shift-click adds. Automated proxy: the `--scenario scripted_moves` 30 vs 120 fps hash equality above plus a `ci_testing` screenshot of the selection state attached to the PR.
- [ ] Clicking empty HUD area issues no world order; clicking a HUD button does not deselect (automated via `--scenario hud_click` replay + `ci_testing` screenshot for the owner proxy).

### Good first issues

- Add a hotkey cheat-sheet overlay under `dev` generated from the keybinding table (platform:macos, help wanted).
- Make edge-scroll margin and camera speed configurable in `rules.ron` (any OS for the data + validator part).

### Estimate

3 sessions.

## M3a: Sim: economy, Towns, territory field and construction

### Goal

The macro loop exists headlessly: Yeomen auto-gather four resources under the Yield Cap with integer-exact income and ramping costs, Towns project a territory field with a pure tiebreak, and Build is rejected outside it, all defined in RON and proven by tests and benches.

### Deliverables

- sim: `economy.rs` (micro-unit income, cap, Lore exemption, ramping, pop cap, idle-worker seeking), `town.rs` (founding, radius 20 -> 24 at 5 distinct kinds, town limit), `territory.rs` (`TerritoryField`, disc kernels, pure tie-to-neutral, incremental recompute, `is_buildable`), `construction.rs`, Build/Train/Gather/Cancel/SetRally commands, vision radii stored (unused until M7).
- rules: full economy content (`resources.ron`, `buildings.ron` for the 8 kinds, `units.ron` Yeoman/Scribe, ramping params), validator extended; data-check passes.
- Tests: gather rate, cap flatline, Lore exemption, ramping, territory fixture counts, territory proptest (incremental == full), outside-borders rejection, idle-worker delay; bench territory; economy fixture replay.
- `docs/design/economy.md`, `docs/design/territory.md`, `docs/design/towns.md` (Eonmark's own numbers).

### Acceptance

- [ ] `cargo test -p sim economy`: one Yeoman on a Croft yields exactly 10 Grain per 600 ticks; with 10 Crofts worked the Grain rate is clamped to 70 per 600 ticks and the stockpile grows by exactly 70; Lore ignores the Yield Cap; the second Town costs more than the first by exactly the `rules.ron` formula; queuing a Yeoman raises the next Yeoman's cost by 1 Grain before it finishes; the 6th queued Skirmisher is capped at 2.25x base.
- [ ] `cargo test -p sim territory`: the fixture with 2 Towns + 1 Watchtower yields the golden owned-tile count and the mirrored midline tiles are neutral; proptest over 200 random source edits asserts incremental recompute hash == full recompute hash; a Build outside own borders is rejected with `CommandRejected { reason: OutsideBorders }` and no state change.
- [ ] `cargo run -p sim-cli --release -- bench --territory --sources 16 --edits 100` reports mean incremental recompute < 2 ms.
- [ ] Scripted headless economy fixture: from the standard start (1 Town, 5 Yeomen, 150 Grain / 150 Lumber / 0 Ore / 0 Lore) a fixed build order founds a second Town before tick 3600 and the final hash matches the fixture.
- [ ] Data-only contributor path: change croft gather rate from 10 to 12 in `data/rules/buildings.ron`, run `sim-cli data-check` and `cargo test -p sim` (the flatline test reads the number from data) without touching Rust; documented in CONTRIBUTING.

### Good first issues

- Tune the idle-Yeoman delay default and document the play-bots result (any OS).
- Add a Granary-style +20% enhancer building as data plus one test (any OS).

### Estimate

3 sessions.

## M3b: Game: build placement, command card, resource bar and the border visual

### Goal

The player can see borders, place buildings inside them with a green/red ghost, train Yeomen and read income against the Yield Cap; the border visual ships first as a vertex-colored overlay and upgrades to the FieldExt shader within the milestone if time allows.

### Deliverables

- game: `ground.rs` territory overlay mesh (vertex colors, tint + border line) on day one; `FieldExt` `ExtendedMaterial` with field texture upload as the in-milestone upgrade (mandatory by M7); build-placement ghost green/red; command card with build/train buttons and costs; resource bar with per-30 s rate and amber-at-cap readout; population display; rejection message line.
- `--scenario economy_walkthrough` scripted replay exercising place-outside, place-inside, second Town, cap hit, pop cap for the owner proxy.

### Acceptance

- [ ] In-game: place a Croft outside the border and the ghost is red and the HUD says 'Outside your borders'; inside it turns green, Yeomen walk over and build it; founding a second Town visibly expands the tinted territory and border line; the Grain readout turns amber once the cap is reached; at 25/25 pop the Train button is disabled with a tooltip and the sim rejects with `PopCapReached` (`--scenario economy_walkthrough` replay verifies and a `ci_testing` screenshot shows the amber readout).
- [ ] `cargo run --release -p game -- --scenario economy_walkthrough` holds >= 60 fps on the dev Mac with the overlay (and with FieldExt if landed); dev-panel draw calls < 500.
- [ ] **[owner]** Screenshot of the border visual reviewed against `docs/ART_STYLE.md` (12% tint, 1-tile soft line, matte). Automated proxy: the `economy_walkthrough` replay verifies and its `ci_testing` screenshot is attached to the PR.

### Good first issues

- Add hover states and tooltips to command-card buttons (platform:macos, help wanted).

### Estimate

3 sessions.

## M4a: Sim: tech lines, ages and combat

### Goal

Research 12 techs whose effects are permille Modifier rows, advance through three ages, and fight on a readable counter triangle including Watchtowers and Towns that shoot back, all headless.

### Deliverables

- sim: `tech.rs` (4 lines x 3 levels, research queue, Modifier resolver Set -> Add -> Mul with floor, age gates 2/5 + costs, Letters discount, Arms pop/unlock/stat bump, Statecraft town limit/border push, Trade cap raise), `combat.rs` (stats from rules, auto-acquire in range with seeded tie-breaks, seeded +/- 1 tile spawn jitter in fixtures, `damage_variance_permille` knob default 0, Attack/AttackMove, counter multipliers, Watchtower and Town shooters, building destruction, Towns indestructible), Research/AdvanceAge commands.
- rules: `techs.ron`, `ages.ron`, `units.ron` for the six military kinds, counter table, validator checks (StatPath validity, integer/permille values, gate monotonicity, trainer per unit).
- Tests: age gating, Letters discount, Modifier stacking order, seeded counter triangle.
- `docs/design/tech-and-ages.md`, `docs/design/combat.md`.

### Acceptance

- [ ] `cargo test -p sim tech`: Masonry Age unlocks only after 2 techs plus 250 Grain + 100 Lore; Charter Age after 5 techs plus 500 Grain + 300 Lumber + 300 Lore; each Letters level reduces the next research cost by 10% floored; Modifier stacking Set -> Add -> Mul produces the documented values; a faction row `yield_cap Mul 1250` yields cap 87 at Trade level 0.
- [ ] `cargo test -p sim counters` over 20 seeds each (seed perturbs spawn jitter and acquisition tie-breaks, so results differ across seeds and the test asserts they do): 10 Skirmishers vs 10 Bowmen -> Skirmishers win >= 18/20 with >= 4 survivors; 10 Bowmen vs 10 Shieldbearers -> Bowmen >= 18/20; 10 Shieldbearers vs 10 Outriders -> Shieldbearers >= 18/20; 10 Outriders vs 10 Bowmen -> Outriders >= 18/20; a Watchtower outranges Skirmishers.
- [ ] `cargo run -p sim-cli -- data-check data/` passes with 3 ages, 8 unit kinds, 8 building kinds, 12 techs; removing a tech referenced by `ages.ron` exits 1 naming file and field; a Mul value written as `1.25` exits 1.
- [ ] `cargo run -p sim-cli --release -- bench --units 400 --ticks 1200 --combat` keeps mean step < 5 ms with 200 vs 200 fighting.

### Good first issues

- Propose counter multiplier tweaks with a `cargo test -p sim counters` table (any OS).
- Write `strings/en.ron` flavor text for the 12 techs (any OS).

### Estimate

3 sessions.

## M4b: Game: research panel, age indicator, health bars and unit looks

### Goal

The player can research, advance ages, and fight with visible health and distinct primitive silhouettes per unit kind.

### Deliverables

- game: research panel on the command card with prerequisites and costs, age indicator and Advance button, health bars as separate flat entities keyed by UnitId, attack-move key, five primitive looks for unit kinds, `--scenario enemy_outpost`.

### Acceptance

- [ ] In-game: build a Scriptorium, research 2 techs, advance to Masonry Age, build an Engine Yard, train Mangonels and a mixed army, destroy an enemy Watchtower placed via `--scenario enemy_outpost`; the auto-recorded replay verifies (the same sequence exists as a scripted replay for the owner proxy).
- [ ] `cargo run --release -p game -- --scenario enemy_outpost` with 200 vs 200 fighting holds >= 60 fps; health bars are not children of unit entities (asserted by a dev-only hierarchy check).

### Good first issues

- Add 'units lost by kind' tracking to the HUD selection panel (platform:macos, help wanted).

### Estimate

2 sessions.

## M5a: Sim: attrition, supply, annexation and victory

### Goal

Headless completeness: attrition bleeds unsupplied invaders, Towns are annexed not destroyed, capturing the Seat wins, the 45-minute territory tiebreak ends every match, and a scripted `capture_seat` scenario drives the victory overlay.

### Deliverables

- sim: `attrition.rs` + `SupplyGrid` (Harrying I/II at Watchtower, 1 hp per 64 ticks doubling per level, age-gap table, Supply Wain radius 14, immunities), annexation state machine (0 HP + adjacent attacker infantry/cavalry + no defender military within 6 tiles for 1200 ticks; defender arrival resets; neutral pseudo-player source during the timer; Watchtowers stay with defender), `outcome()` with capital capture / all Towns lost / 45-minute territory tiebreak (`Outcome::Victory { player, decisive: bool }`), late-game pressure knobs in `rules.ron`.
- game: attrition indicator on affected units, supply radius ring on selection, annexation timer UI, victory/defeat overlay driven by `--scenario capture_seat`.
- Tests: attrition with/without supply, before research, annexation rules, outcome, tiebreak.
- `docs/design/attrition.md`, `docs/design/towns.md` (annexation section).

### Acceptance

- [ ] `cargo test -p sim attrition`: an enemy unit in researched borders loses exactly 1 HP at ticks 64, 128, 192; with a friendly Supply Wain within 14 tiles it loses 0 HP over 1200 ticks; before Harrying I it loses 0 HP; Harrying II doubles the rate.
- [ ] `cargo test -p sim annexation`: a Town at 0 HP with enemy Skirmishers adjacent and no defender military within 6 tiles flips after 1200 ticks; a defender arriving at tick 600 resets the timer; during the timer `owner_at` returns neutral for its tiles and Train is rejected; capturing the Seat makes `outcome()` return `Victory { decisive: true }`; a 54000-tick scripted stalemate returns `Victory { decisive: false }` for the player with more territory.
- [ ] `cargo run -p game --features dev -- --scenario capture_seat` shows the victory overlay with the final hash and the recorded replay verifies to it.

### Good first issues

- Add an attrition table row for age gap 2 and a test (any OS).

### Estimate

3 sessions.

## M5b: AI: the first complete skirmish opponent and the first playable match

### Goal

A minimal but complete scripted bot (fixed build order to Masonry Age, worker balancing, second Town, Watchtower at the border edge, timed attack waves with a Supply Wain attached, defend-Seat rule) at a single Standard difficulty, so a human can play a full match from New Game to a victory or defeat overlay.

### Deliverables

- ai: build-order executor, worker balancing, expansion, Watchtower placement, attack waves, defend rule; `data/ai/build_orders/standard.ron`; no influence maps yet.
- sim-cli `play-bots` with `--jobs`, CSV summary (seed, winner, decisive|tiebreak, tick, final hash); fixture `m5_first_match.eonreplay` (`#[ignore]` in `cargo test`, verified by sim-cli in CI).
- game: New Game button starting a match vs the bot.
- `docs/design/ai.md`.

### Acceptance

- [ ] `cargo run -p sim-cli --release -- play-bots --seeds 1..5 --a standard --b standard --max-ticks 54000 --jobs 5` ends all 5 games with a declared winner, no panics, under 120 s wall time on the dev Mac (budget derived as ticks x measured M4a mean ms / 1000 x 1.5, recorded in `BUILD_TIMES.md`), and prints per-seed final hashes identical across two runs; seed 1 is committed as `fixtures/m5_first_match.eonreplay` and verified in both CI OS jobs.
- [ ] `play-bots --seeds 1..5 --a standard --b passive --max-ticks 36000` shows the Standard bot reaching Masonry Age and launching an attack with >= 8 units including 1 Supply Wain before tick 18000 in 5/5 seeds (CSV columns `age_reached_tick`, `first_attack_tick`).
- [ ] `bench --units 400 --ticks 1200 --bots` shows the AI adds < 0.5 ms mean per tick.
- [ ] **[owner]** From the window: New Game starts a match vs the Standard bot on plains_1v1; the owner plays to a victory or defeat overlay in one sitting; the auto-recorded replay verifies to the hash shown on the overlay. Automated proxy: `m5_first_match.eonreplay` verifies in CI and a `ci_testing` screenshot of the overlay with its final hash is attached to the PR.

### Good first issues

- Write a second AI build order RON (boom variant) and compare with play-bots (any OS).

### Estimate

3 sessions.

## M6: Fun gate: difficulty, personalities, pacing, game speed and logged human matches

### Goal

The skirmish is worth playing again: three difficulties and three personalities behave as ordered, 20 seeded bot games end with mostly decisive outcomes in 20-40 minutes, game speed and pause exist, the game-over screen reports stats, and three logged human matches show real pressure and rate >= 3/5.

### Deliverables

- ai + `data/ai/difficulty.ron`: Easy (defensive, income every 30 s), Standard (25 s), Hard (aggressive, 20 s, attacks before minute 12); personalities rush/boom/tower; retreat-when-losing; influence maps + counter-weighted composition.
- game: game speed x1/x2/x4 and Space pause (ticks per frame within the 8-tick cap; replay unaffected), New Game difficulty + seed selector, game-over stats (duration, units lost by kind, Towns annexed, ages reached, cap-hit minutes, decisive or tiebreak), Play Again.
- sim-cli `play-bots` CSV and a CI job running 5 seeded bot games on ubuntu.
- `docs/PLAYTEST.md` match log template and three logged human-vs-Standard matches; 2-3 `rules.ron` tuning iterations budgeted.

### Acceptance

- [ ] `cargo run -p sim-cli --release -- play-bots --seeds 1..20 --a standard --b standard --personality-a rush,boom,tower --personality-b rush,boom,tower --max-ticks 72000 --jobs 5` exits 0 with a winner in all 20 games; >= 10/20 are decisive (capital capture) before tick 48000; median game length <= tick 54000; >= 10/20 last past tick 24000.
- [ ] `play-bots --a easy --b standard --seeds 1..20` gives Standard >= 15/20 wins; `--a standard --b hard` gives Hard >= 15/20 wins.
- [ ] Two `play-bots --seeds 7..7` runs print identical final hashes; CI asserts this on ubuntu and macos.
- [ ] Selecting Hard yields an AI attack on the player's borders before game minute 12 in 3 of 3 seeded games (asserted headlessly via `first_attack_tick < 14400` with a passive opponent); changing `hard.ron` `income_interval_s` from 20 to 30 measurably flips the hard-vs-easy result (documented in `docs/design/ai.md`). Note: the design text says `income_interval_s`; the authored field follows the `_ds` convention as `income_interval_ds` (200 to 300), see `docs/design/ai.md`.
- [ ] Game speed x4 and pause work and the replay verifies with the same final hash as the x1 replay of the same scripted input (`--scenario scripted_match --speed 4`).
- [ ] **[owner]** Three complete human-vs-Standard matches logged in `docs/PLAYTEST.md` with duration 20-40 min each; in >= 2 of 3 the AI's army reaches the player's borders and annexes or reduces a Town below 50% HP; the Yield Cap readout turns amber at least once; owner fun rating >= 3/5; each failed criterion triggers a `rules.ron` tuning commit and a replay of the three matches before M7 starts (max 3 iterations before escalating to an ADR on the ruleset). Automated proxy: the 20-game bot run above (winner in all, >= 10/20 decisive, median <= tick 54000) plus a `ci_testing` screenshot of the game-over stats screen. The proxy does not substitute for the owner's rating; this line is the fun gate and is never waived.

### Good first issues

- Name and write copy for the three difficulty levels and personalities in `strings/en.ron` (any OS).
- Add a `--summary` mode to play-bots printing win rates by personality (any OS).

### Estimate

4 sessions.

## M7: Fog of war, minimap, menus, look and audio pass

### Goal

The game looks and sounds like a finished small game: 3-state fog of war through the FieldExt shader, a minimap, main/pause menus, CC0 glTF art via data, Inter font, Kenney panels, a dozen sounds, with all sim hashes unchanged after one `rules_version` bump.

### Deliverables

- sim: `visibility.rs` (per-player u8 grid from vision radii; hashed; `SimView` exposes it so the AI respects fog).
- game: FieldExt shader mandatory now (fog channel 0.0/0.45/1.0), enemy Visibility toggling, minimap Image painted from territory + fog + units each frame with click-to-move and right-click-to-order, main menu, pause menu, `audio.rs` facade with ~10 CC0 OGG clips.
- `assets/`: KayKit Medieval Builder/Hexagon buildings (team colors), KayKit Adventurers units, Kenney Nature Kit trees, Kenney UI Pack + Game Icons, Inter + OFL text; `data/visuals.ron` maps every kind; `tools/inspect_gltf`.
- `docs/ART_STYLE.md` finalized with checklist and import rules; `docs/design/fog.md`; `docs/screenshots/m7_town.png` via `ci_testing`.

### Acceptance

- [ ] `cargo test -p sim fog`: visibility grid matches vision radii for a fixture; an enemy unit outside all vision cells is hidden in `SimView`; all earlier golden fixtures verify after the single `rules_version` bump in this milestone.
- [ ] In-game: unexplored terrain is black, explored darkened, visible lit; enemy units vanish without friendly vision; the minimap mirrors fog and borders within one frame; clicking the minimap moves the camera; right-clicking it orders selected units (`--scenario fog_walkthrough` replay + `ci_testing` screenshots as the owner proxy).
- [ ] `scripts/check_assets.sh` passes: every non-allowlisted file under `assets/` has an `ATTRIBUTION.md` row with CC0-1.0, CC-BY-4.0 or OFL-1.1, every OFL font has a sibling license text, no file > 2 MB, total < 300 MB; injecting an unattributed file fails the script and it prints the unmatched path.
- [ ] `sim-cli data-check` fails if any unit or building kind lacks a `visuals.ron` entry; swapping one entry back to `Primitive(Capsule)` requires no Rust change (walkthrough in `docs/ART_STYLE.md`).
- [ ] `cargo run --release -p game` shows >= 60 fps at 1440p with 400 units + two full Towns + fog; draw calls < 500; `cargo tree -i cpal` shows exactly one cpal version.
- [ ] **[owner]** `docs/screenshots/m7_town.png` signed off against the `ART_STYLE.md` checklist (palette, no glow, readable at 40 m, matte HUD) in the PR. Automated proxy: the screenshot is produced by `ci_testing` from the `fog_walkthrough` scenario and committed; the agent checks it against the written checklist before asking.

### Good first issues

- Replace the Croft primitive with a KayKit field model via `visuals.ron` (platform:macos, help wanted).
- Record or source a CC0 'age advance' chime and add its attribution row (any OS).

### Estimate

3-4 sessions.

## M8: v0.1.0 release, .app bundle and contributor launch

### Goal

A stranger can download a zip from GitHub Releases, open Eonmark.app on an Apple Silicon Mac, and play a complete skirmish; a contributor can clone, run with one command and find labeled work.

### Deliverables

- `scripts/bundle.sh` finished (`RUST_BACKTRACE=1` in `LSEnvironment`, dSYM zip), `release.yml` producing `Eonmark-v0.1.0-macos-arm64.zip` + `Eonmark-v0.1.0.dSYM.zip` + `SHA256SUMS` via `softprops/action-gh-release@v3` (the design text said v2; v3 is the current Node 24 line) with generated notes listing `sim_version`/`rules_version`/`rules_hash`; optional notarization steps gated on secrets.
- README final, CONTRIBUTING final, `docs/RELEASING.md`, `docs/DEPENDENCIES.md` current, `docs/ROADMAP.md` with deferred list and re-entry conditions (see [Deferred](#deferred-out-of-scope-for-v01)).
- Issue backlog: >= 15 good-first-issues (>= 10 completable with `cargo test -p sim` or `sim-cli` on any OS, each with a 'how to verify' section), Discussions categories (Welcome, Ideas, Show and tell), PR to bevyengine/bevy-assets (Apps, TOML entry with < 100-char description and a 16:9 image < 2 MB, max width 600 px), announcement in Bevy Discord #showcase and r/rust_gamedev.
- Tag v0.1.0.

### Acceptance

- [ ] `scripts/bundle.sh` produces `dist/Eonmark.app`; `plutil -lint dist/Eonmark.app/Contents/Info.plist` OK; `codesign --verify --deep --strict --verbose=2 dist/Eonmark.app` exits 0; `open dist/Eonmark.app` launches with Dock name 'Eonmark' and icon; the bevy_dylib check passes; `otool -l` shows `LC_BUILD_VERSION` minos 13.0.
- [ ] Pushing tag v0.1.0 makes `release.yml` publish the zip, dSYM zip and `SHA256SUMS`; `shasum -a 256 -c SHA256SUMS` passes on the download.
- [ ] **[owner]** Downloading via Safari on a second Mac or fresh user account (quarantined), following the README 'Open Anyway' steps, launches the game and a match is completable; a replay from the release build verifies with `sim-cli verify` built from the same tag. Automated proxy: `release-check.yml` green (bundle, `plutil`, `codesign --verify`), plus a local `xattr -w com.apple.quarantine` on the built zip and `open` of the extracted app on the dev Mac.
- [ ] **[owner]** Fresh clone on a second Mac with only Xcode CLT + rustup: `cargo run -p game --features dev` builds and opens the game on the first try; README quotes the cold build time from `docs/BUILD_TIMES.md`. Automated proxy: a clean clone into a new directory on the dev Mac with `CARGO_TARGET_DIR` pointed at an empty folder, timed with `cargo build --timings`.
- [ ] `gh issue list --repo tonianev/eonmark --label 'good first issue' --json number | jq length` >= 15; Discussions has the three categories; the bevy-assets PR is open and the showcase posts exist (links recorded in the [Launch links](#launch-links-m8) section of this file).
- [ ] `cargo deny check`, `cargo machete`, `typos`, `scripts/check_trademark.sh`, `cargo doc --workspace --no-deps` green; at least one Dependabot grouped PR has passed CI; third-party actions SHA-pinned.
- [ ] CI smoke `--headless-run fixtures/smoke.eonreplay` passes; the release-check job's cold wall time is recorded (not gated) in `docs/BUILD_TIMES.md`.

### Good first issues

- Write the Gatekeeper walkthrough with screenshots for README (platform:macos).
- Add a `just release-check` recipe that runs plutil/codesign/otool assertions locally (platform:macos).

### Estimate

3 sessions.

### Launch links (M8)

Filled in when M8 closes.

| Item | Link |
| --- | --- |
| bevy-assets PR | (pending) |
| Bevy Discord #showcase post | (pending) |
| r/rust_gamedev post | (pending) |
| v0.1.0 release | (pending) |

## M9: Post-release: second faction as pure data, Bevy 0.20 migration (gated)

### Goal

Prove the data model by adding a second faction with no Rust changes, and migrate to Bevy 0.20 stable once the trigger conditions hold, with all golden replays unchanged.

### Deliverables

- `data/rules/factions.ron`: 'Tidewater League' (`yield_cap Mul 1250`, lumber gather `Mul 1100`, Watchtower border push `Add -1`) selectable in New Game; strings; play-bots balance table in `docs/design/factions.md`.
- Bevy 0.20 migration PR following the official 0.19 -> 0.20 guide (PointerPress, Hovered/Pressed, `.wgsl` -> `.wesl` for the field shader), bevy_egui/bevy-inspector-egui bumped together, `docs/DEPENDENCIES.md` updated, `docs/adr/0002-bevy-0-20-migration.md` records the date or the unmet trigger conditions.
- `docs/ROADMAP.md` v0.2 plan (heightmap terrain, flow fields if bench demands, mod overlay dirs, lockstep multiplayer spike over renet, goodfirstissue.dev once 10 contributors, optional eonmark-sim/eonmark-rules crates.io publication with real content). See [v0.2 candidates](#v02-candidates-m9).

### Acceptance

- [ ] `git diff v0.1.0 -- crates/ | grep -c .` for the faction commit prints 0 (data-only); `sim-cli data-check` passes; `play-bots --a standard --b standard --faction-a tidewater --faction-b freeholders --seeds 1..20 --jobs 5` reports a win rate between 7/20 and 13/20 after tuning. (Note: `grep -c` exits 1 when it prints 0; check the printed count, not the exit code.)
- [ ] Selecting Tidewater League in New Game shows Yield Cap 87 in the HUD at Trade level 0.
- [ ] If `cargo info bevy` shows a stable 0.20.x AND crates.io shows bevy_egui and bevy-inspector-egui releases depending on bevy ^0.20 that share one egui version: the migration PR lands with `just ci` green on all jobs, all golden fixtures verifying with unchanged hashes, and `cargo run --release -p game` completing a match; otherwise `docs/adr/0002` records the trigger conditions and the milestone closes with the faction work only.
- [ ] `cargo tree -p sim -e normal | grep -cE 'bevy|glam|wgpu|winit'` still prints 0 and `cargo tree -i bevy_ecs --depth 0 | wc -l` prints 1 after the migration.

### Good first issues

- Propose a third faction as a Modifier list with a play-bots table (any OS).
- Translate `strings/en.ron` into a second language file (any OS).

### Estimate

2-3 sessions.

### v0.2 candidates (M9)

These are candidates, not commitments. Each needs an ADR before work starts.

| Candidate | Re-entry condition |
| --- | --- |
| Heightmap terrain | Post-v0.1 issue labelled needs-design; ADR with a renderer and pathing plan |
| Flow fields (or sector/portal pathing) | `sim-cli bench --units 500 --ticks 1200` fails its budget (mean >= 5 ms or p95 >= 10 ms) after profiling |
| Mod overlay directories | A second data contributor asks for it |
| Lockstep multiplayer spike over renet | Hash parity has held across every release since v0.1.0; spike only, no release promise |
| goodfirstissue.dev listing | 10 contributors |
| crates.io publication of eonmark-sim / eonmark-rules | Owner asks; crates have real content (post M1 / M3a); never placeholders |

## Closing a milestone

1. Every acceptance line that is not marked [owner] passes, and the command output for each is pasted into the PR description. A line without pasted output is not closed.
2. [owner] lines must pass before the milestone **after next** starts. Before asking the owner, run the automated proxy named on the line and attach its output (replay verification result, `ci_testing` screenshot) to the PR.
3. If a milestone runs past its estimate, split it into a sim half and a game half with their own acceptance subsets (the M3/M4/M5 splits are the template). Do not cut acceptance criteria.
4. The M6 fun gate and the CI hash-parity job are never negotiable. No tuning or scope decision may waive them.
5. Each milestone writes its `docs/design/*.md` files with target numbers and the acceptance checklist, and files 2-3 `good first issue` tickets with a 'how to verify' section. A `good first issue` must be completable with `cargo test -p sim` or `sim-cli` on any OS; otherwise label it `help wanted` + `platform:macos`.
6. When a bench acceptance passes, append the numbers to `docs/BUILD_TIMES.md`.
7. Golden replay fixtures are regenerated only in a commit that bumps `rules_version` with a one-line reason. A changed hash without that bump is a bug.
8. Scope changes go through this file and an ADR. Only the repository owner directs scope changes.

## Deferred: out of scope for v0.1

Everything in this table is deliberately not in v0.1. Where the design states a re-entry condition it is listed; otherwise the condition is "a needs-design issue and an ADR after v0.1.0".

| Deferred item | Re-entry condition |
| --- | --- |
| Multiplayer networking (API shaped for lockstep; no sockets) | v0.2 candidate: lockstep spike over renet once hash parity has held since v0.1.0 |
| Ages IV+, gunpowder/industrial/modern content, navy, air, missiles, doomsday clock | needs-design issue and ADR after v0.1.0 |
| Wonders, rare resources and merchants, caravans/trade routes, markets, taxation | needs-design issue and ADR after v0.1.0 |
| Multiple factions in v0.1 | Second faction arrives as pure data in M9 |
| Unique units, more than one map, random map generation, map editor, mod overlay directories | Mod overlay dirs: a second data contributor asks; the rest: needs-design issue and ADR |
| Generals, spies, formations beyond spiral offsets, stances, garrisoning, militia, scouts and ruins | needs-design issue and ADR after v0.1.0 |
| Diplomacy, more than 2 players, team games | needs-design issue and ADR after v0.1.0 |
| Heightmap terrain, cliffs, elevation bonuses, water pathing | Post-v0.1 issue labelled needs-design (v0.2 candidate) |
| Fog-of-war ghosting, save/load of in-progress games beyond replays, settings beyond UI scale and volume | needs-design issue and ADR after v0.1.0 |
| Developer ID signing and notarization (99 USD/yr) | External players appear and the owner pays the 99 USD/yr; enabled by adding secrets to `release.yml` |
| Intel/universal binaries, Linux/Windows release builds | needs-design issue and ADR; Linux compiles the game crate and runs headless tests only |
| Steam/itch/Homebrew distribution | needs-design issue and ADR after v0.1.0 |
| Custom shaders beyond the single FieldExt ground material, decals, SSAO, weather, day/night, music, Firewheel/kira audio swap | needs-design issue and ADR; decals are unsupported on Metal (ClusteredDecal); an audio swap would raise the deployment target to 14.2 |
| Campaign or strategic-layer meta game | needs-design issue and ADR after v0.1.0 |
| Sector/portal pathfinding, flow fields, ORCA | Bench-triggered only: a failing `sim-cli bench --units 500 --ticks 1200` after profiling |
| Localization beyond `strings/en.ron`, Git LFS | needs-design issue; a translated strings file is an M9 good first issue |
| goodfirstissue.dev listing | 10 contributors |
| Discord server | 3+ regular contributors (GOVERNANCE.md's "3+ contributors" section) |

## Risk register

| Risk | Likelihood | Impact | Mitigation |
| --- | --- | --- | --- |
| Bevy 0.20 ships mid-build (rc.2 on 2026-09-28); `latest` docs describe 0.20 APIs and ecosystem crates (bevy_egui 0.43) start targeting it, pulling a second Bevy into the graph or tempting an upgrade | high | medium | Pin `=0.19.1`, `=0.40.1`, `=0.37.0`; Dependabot ignores all three; CI fails on more than one `bevy_ecs` in `cargo tree`; implementer uses only version-pinned doc links; migration is M9, gated; never build on an rc |
| Determinism leaks (f32, HashMap iteration, wall clock, unordered sorts, stale caches, non-exact snapshot/restore) silently break replays and the multiplayer door | medium | high | clippy bans in sim, rules and ai; cargo-tree boundary check; BTreeMap + monotonic ids (no slotmap); generation-keyed path cache cleared on grid change; `restore()` rebuilds derived state; overflow-checks in release; tests for double-run, restore-then-spawn, cache on/off, cross-OS hash parity from M1; sub-hashes for bisecting; `rules_hash` in headers |
| Apple Silicon compile times slow the implementer's loop; the CoreAudio bindings could fail against a future SDK on the dev Mac only (they built at M0) | medium | low | M0 measures and publishes times; `dev` feature with dynamic_linking + optimized deps; sim iteration via `cargo test -p sim` (< 10 s, opt-level 3); documented audio-less fallback; single ci profile in the PR job; release build moved to release-check |
| The game is deterministic and complete but not fun (turtle stalemates, AI never attacks) | medium | high | M6 fun gate with personalities, decisive-outcome ratio, median length, difficulty ordering, pressure criteria in human matches, 2-3 budgeted tuning iterations, data-driven late-game pressure knobs; 45-minute cap with territory tiebreak makes an ending structural |
| Per-unit A* spikes a tick with hundreds of movers or group orders target blocked/unreachable tiles | low | medium | In-house A* with a resumable per-tick expansion budget, bench-tuned default, generation-keyed cache, component-aware spiral offsets with BFS fallback, pop cap bounds unit counts, bench gates at M1 and M4a; flow fields documented as the bench-triggered upgrade |
| Cmd-Q or a crash loses the session replay, or the replay flush hitches the frame | high without mitigation | medium | muda custom Quit -> AppExit on the main thread in M2; off-thread writer with write+flush every 20 ticks (no F_FULLFSYNC); `kill -9` then verify is an acceptance test in M1 and M2; frame-time graph checked at the flush cadence |
| Gatekeeper blocks the downloaded .app and testers conclude it is broken | high | medium | README 'Open Anyway' steps and xattr alternative, SHA256SUMS, M8 acceptance tests a Safari download on a second Mac; notarization budgeted later |
| Mesh picking raycasts every mesh on every pointer move once hundreds of units exist | medium | medium | `MeshPickingSettings::require_markers = true` from M2, `Pickable` only on units/buildings, ray-plane ground clicks, screen-space drag box |
| Legal/branding exposure: the inspiration's trademark in product-level places, unit/tech names, or CC-BY-SA wiki prose leaks into the repo | low | high | Original names in RON; `scripts/check_trademark.sh` bans the phrase, its abbreviation and fan-wiki (fandom) URLs everywhere except README's single disclaimed sentence and `docs/design/prior-art.md`; `docs/NAME.md` with USPTO/EUIPO/WIPO checks; CC0/CC-BY-only assets with attribution gate |
| Solo-maintainer archival (every prior Rust RTS died with bus factor 1) and AI-spam or backlash PRs once listed as contributor friendly | medium | high | GOVERNANCE.md with honest 'Today' section and a 7-day response promise, data-only GFIs completable on any OS, AI_CONTRIBUTIONS.md on day one, CI as the single gatekeeper, generated devlog, goal of a second maintainer within three months of v0.1 |
| CI becomes a 60-120 minute per-PR loop on 3-core macOS runners and contributors stop waiting for it | medium | medium | One ci profile for every Bevy compile in the PR job, release build and bundling only in release-check (main/schedule/tag), rust-cache with shared keys, cancel-in-progress, explicit timeouts, measured wall times with a < 25 min warm soft target |
| Shipping `dynamic_linking` or a Bevy dylib in release, or nightly rustflags leaking into `.cargo/config.toml` | low | medium | `grep -q bevy_dylib` inverted assertion in release-check, M8 and `bundle.sh`; `.cargo/config.toml` holds only MACOSX_DEPLOYMENT_TARGET |
| Owner-gated acceptance lines block the implementer on owner availability at every milestone | medium | medium | [owner] lines must pass before the milestone after next starts, all other lines before the next milestone; every [owner] line has an automated proxy (scripted scenario replay + `ci_testing` screenshot) the agent runs first |

## Open questions for the owner

Each question is listed with the default this plan assumes. Work proceeds on the default until the owner answers differently; an answer that changes the default is recorded here and, where it changes scope, in an ADR.

- [ ] **Project name.** Approve 'Eonmark' (cleanest availability on 2026-10-05) or prefer 'Boundstone' / 'Marchfall'. The USPTO (tmsearch.uspto.gov), EUIPO and WIPO searches and a domain purchase happen at M0 and need the owner's accounts. **Default: Eonmark.**
- [ ] **Nominative mention.** Allow the single disclaimed README sentence naming the inspiration and its publisher, for discoverability, or zero mentions outside `docs/design/prior-art.md`. **Default: the single README sentence is allowed; `scripts/check_trademark.sh` enforces that it appears nowhere else.**
- [ ] **CODE_OF_CONDUCT reporting contact.** Both Contributor Covenant 3.0 placeholders must be filled or the CoC is non-functional. **Default: the project lead's address recorded in CODE_OF_CONDUCT.md.**
- [ ] **Fun-gate judge.** The owner signs off the three logged human matches (M6) and the M7 art screenshot. Is 3/5 the right fun threshold, and is a second human tester wanted? **Default: threshold 3/5, owner is the only tester.**
- [ ] **Fourth resource.** Keep 'Lore' (cap-exempt research currency, adds Scribe + Scriptorium slots) or drop to three resources. **Default: keep Lore.**
- [ ] **Naming direction.** Fictional terrains ('Freeholders', 'Tidewater League') vs plain regional descriptors; age names Hearth/Masonry/Charter or something else. **Default: Freeholders and Tidewater League; ages Hearth/Masonry/Charter.**
- [ ] **Apple Developer Program.** Pay 99 USD/yr for notarization once external players appear, or stay ad-hoc + Gatekeeper instructions. **Default: ad-hoc signing through v0.1.0; notarization steps in `release.yml` stay dormant until secrets exist.**
- [ ] **Fog of war in v0.1.** 3-state fog in M7 vs deferring past release to ship sooner. **Default: fog in v0.1 at M7.**
- [ ] **Deployment target.** 13.0 (Ventura) vs 14.2 (needed only if audio moves to Firewheel/kira later). **Default: 13.0.**
- [ ] **Game-speed controls.** x1/x2/x4 in release builds or dev-only. **Default: in release builds (M6).**
- [ ] **crates.io.** Publish real `eonmark-sim` and `eonmark-rules` crates at M1/M3a, or stay GitHub-only until v0.1.0. **Default: no crates.io publication before v0.1.0; never placeholders.**
- [ ] **Session budget.** ~33 sessions to a fun-gated skirmish (M0-M6) and ~43 to v0.1.0; acceptable, or cut scope (fog, fourth resource, second faction) further? **Default: ~43 sessions accepted; no further cuts.**
