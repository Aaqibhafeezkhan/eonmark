# Design documents

This directory holds one document per game system. Each states Eonmark's own target numbers, the algorithm or state machine in plain words, how the system touches the others, the acceptance checklist of the milestone that implements it, the GitHub area label, and the open tuning questions. The numbers are starting values that live in `data/rules/*.ron` and are expected to move during the M6 tuning iterations; a doc tells you which file holds each one. The project roadmap, milestone list and session estimates are in `../ROADMAP.md`; the invariants every doc assumes are in `../ARCHITECTURE.md` and `../DETERMINISM.md`; the RON schema conventions are in `../DATA_FORMAT.md`.

## Index

| Document | System | Milestone | Area label |
| --- | --- | --- | --- |
| [economy.md](economy.md) | Four resources, gather slots, Yield Cap, integer income, ramping costs, pop cap, idle workers, standard start | M3a | `area:sim`, `area:rules` |
| [territory.md](territory.md) | `TerritoryField` borders: disc kernels, pure tie-to-neutral, incremental recompute, `is_buildable`, field texture handoff | M3a (sim), M3b (visual) | `area:sim` |
| [towns.md](towns.md) | Founding, town limit, radius tiers, indestructible Towns, annexation state machine, victory and the 45-minute tiebreak | M3a, M5a | `area:sim`, `area:rules` |
| [attrition.md](attrition.md) | Harrying I/II, pulse timing, age-gap table, Supply Wain radius, `SupplyGrid`, immunities | M5a | `area:sim`, `area:rules` |
| [tech-and-ages.md](tech-and-ages.md) | 12 techs in 4 lines, Modifier model (Set -> Add -> Mul, permille), age gates and unlocks, data-only fourth age | M4a | `area:sim`, `area:rules` |
| [combat.md](combat.md) | 8 unit kinds, counter triangle, shooters, seeded acquisition, 20-seed tests, movement summary | M4a | `area:sim`, `area:rules` |
| [pathing.md](pathing.md) | Budgeted in-house A* with `resume(budget)`, request queue, generation-keyed cache, group offsets, flow-field re-entry condition | M1 | `area:sim` |
| [fog.md](fog.md) | Three-state visibility grid from vision radii, field texture B channel, minimap darkening | M7 | `area:sim`, `area:game` |
| [ai.md](ai.md) | Scripted bot inside `Sim::step`, build orders, difficulty table, rush/boom/tower personalities, influence maps | M5b, M6 | `area:ai` |
| [ui.md](ui.md) | HUD, command card, resource bar, menus, controls, camera, game speed and pause, minimap | M2, M3b, M4b, M6, M7 | `area:ui`, `area:game` |
| [macos-packaging.md](macos-packaging.md) | Cmd-Q and replay durability, `Eonmark.app` bundle, ad-hoc signing, Gatekeeper steps, releases, dSYM | M2, M8 | `area:infra`, `platform:macos` |
| [prior-art.md](prior-art.md) | Inspiration, research citations and what Eonmark deliberately does differently | M0 | `area:docs` |

## Conventions

| Convention | Detail |
| --- | --- |
| Tick rate | 20 Hz. 30 s = 600 ticks, 60 s = 1200 ticks, 12 min = 14400, 20 min = 24000, 40 min = 48000, 45 min = 54000. |
| Units of authorship | Integers only: deciseconds in fields ending `_ds`, tiles in fields ending `_tiles`, permille for Modifier `Mul` values; `Add` and `Set` are integers. `Rules::ticks_from_ds` is the single time conversion. See [../DATA_FORMAT.md](../DATA_FORMAT.md). |
| Numbers | Every gameplay constant lives in `data/rules/*.ron`. A constant typed into Rust is a bug. |
| Names | Original names only: ages Hearth, Masonry, Charter; resources Grain, Lumber, Ore, Lore; faction Freeholders; the unit and building names in [combat.md](combat.md) and [economy.md](economy.md). Prior work is cited only in [prior-art.md](prior-art.md). |
| Future work | Marked with the milestone id that delivers it (for example M5a). Nothing here describes a feature outside the milestone plan. |
| Acceptance | Each doc copies the acceptance lines of its milestone as a checkbox list. CI is the gate; the checklist is a guide. |

## Proposing a balance change

Balance changes are data changes. Open an issue with the `kind:balance` form: which RON file, what number, and the `sim-cli play-bots` result. A data PR needs `sim-cli data-check` and `cargo test -p sim` to pass and, when the change touches pacing, a play-bots table. See `../../CONTRIBUTING.md`.

```bash
cargo run -p sim-cli -- data-check data/
cargo test -p sim
cargo run -p sim-cli --release -- play-bots --seeds 1..20 --a standard --b standard --max-ticks 72000 --jobs 5
```

## Adding a design doc

Copy the section order of an existing doc: purpose paragraph, metadata table, target numbers, algorithm, interactions, acceptance checklist, open tuning questions. Add a row to the index above. Cross-link with relative paths. Keep sentences short and put numbers in tables.
