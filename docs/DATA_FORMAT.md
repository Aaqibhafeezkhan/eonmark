# Data format

This document explains how Eonmark's game data is authored: where the files live, the unit conventions, the `Modifier` type that expresses every tech and faction effect, the integer income model, how unknown fields are rejected and how `sim-cli data-check` reports problems. It is the overview. The authoritative, field-by-field schema and the worked authoring example live in `data/rules/README.md`; where this document and that README disagree, the README wins. The Rust schema is `crates/rules/src/lib.rs`.

## Principles

- Every gameplay number is RON under `data/`. There are no tuning constants in Rust. If a value is needed and no field exists, add the field, a `data-check` rule and a line in `data/rules/README.md`.
- Integers only. The `rules`, `sim` and `ai` crates ban `f32` and `f64` through `clippy.toml`, so a RON float literal has nowhere to go and fails to parse.
- Distances are tiles, in fields ending in `_tiles`. One tile is one metre in the renderer and `Fx::ONE` in the sim.
- Fractional tiles and rates are scaled integers, with the scale in the field name: fields ending in `_x100` hold hundredths (`speed_tiles_per_s_x100: 180` is 1.8 tiles per second, `radius_tiles_x100: 35` is 0.35 tiles). The sim converts them to `Fx` exactly once; speeds become tiles per tick (`x100 / (100 * tick_rate_hz)`) at spawn. The validator caps every `_x100` field at `100000` (`rules::units::MAX_X100`).
- Durations are authored in deciseconds, in fields ending in `_ds`. The one conversion is `Rules::ticks_from_ds`: `ticks = round(ds * tick_rate_hz / 10)`, rounding to nearest with halves up. At 20 Hz one decisecond is exactly two ticks: `32` ds is 64 ticks, `600` ds is 1200 ticks. The validator does not require a whole tick count; pick values that give one when you can. The exceptions are `tick_rate_hz` and `cmd_delay_ticks` in `rules.ron`, which define the clock itself.
- Conversion happens in one place. Authored values stay in `Rules` as integers; the sim reads them only through tick and `Fx` accessors such as `Rules::attrition_interval_ticks()` and `Rules::annexation_ticks()`, which call `Rules::ticks_from_ds`. No other code converts.
- Rates are per 30 seconds. "10 Grain per 30 s" is written as `10` in a field documented as per-30-s.
- `rules_version` is a human label bumped when golden replays regenerate; `rules_hash` is computed over everything `Rules::load` reads (match rules, resources and every map at M0; `units.ron` from M1; `visuals.ron` from M2; `buildings.ron` from M3a; `techs.ron`, `ages.ron` and `factions.ron` from M4a). Both go into every replay header. See [DETERMINISM.md](DETERMINISM.md).

## The `Modifier` type

Every tech and faction effect is a row of this shape (M4a, `crates/rules/src/modifiers.rs`), in the `modifiers` list of a tech in `techs.ron` or a faction in `factions.ron`:

```ron
(target: YieldCap, op: Mul, value: 1250)
```

| Field | Type | Meaning |
|---|---|---|
| `target` | `StatPath` | A closed enum of stats the effect may touch: `YieldCap`, `PopCap`, `TownLimit`, `TownBorderRadius`, `ResearchCost`, `ResearchTime`, `UnitHp(sel)` and `UnitDamage(sel)`, where `sel` is `Military`, `Class(<UnitClass>)` or `Kind("<unit id>")`, and from M5a `HarryingLevel` (the attrition level a player's borders impose: base 0, each Harrying tech `Add 1`, at most 8 in the worst case). A name that is not in the enum fails to parse, naming the file, the span and `StatPath` or `UnitSel`. |
| `op` | `Set`, `Add` or `Mul` | How the value combines with the base. |
| `value` | `i32` | For `Set` and `Add`, an absolute amount in the stat's own unit. For `Mul`, a permille multiplier: `1000` is unchanged, `1250` is x1.25, `900` is x0.9. |

Resolution order is `Set`, then `Add`, then `Mul`, in integer math, flooring after each `Mul` row:

```
v = the last Set in collection order, else the base
v = v + sum(Add)
for each Mul row m, in collection order:  v = floor(v * m / 1000)
```

Collection order is the faction's rows, then each researched tech in `TechId` order with its rows in file order. Flooring after each row is what makes Letters stack as 0.9, 0.81, 0.729 of the base, each floored: 100 becomes 90, 81, 72, and 150 becomes 135, 121, 108 (one floor over 0.729 would give 109).

| Base | Set | Add | Mul (permille) | Result | Note |
|---|---|---|---|---|---|
| 70 | none | none | 1250 | 87 | `70 * 1250 / 1000 = 87.5`, floored |
| 70 | none | +10 | 1250 | 100 | `(70 + 10) * 1250 / 1000 = 100` |
| 70 | 100 | +10 | none | 110 | `Set` replaces the base first |
| 100 | none | none | 1100, 1100 | 121 | `100 * 1100 / 1000 = 110`, then `110 * 1100 / 1000 = 121` |

`data-check` rejects a non-integer value (`1.25` cannot parse as `i32`, so the error names the file and the span), a `Mul` outside `1..=100000`, an `Add` or `Set` outside `±1000000` and a `Kind` selector that names no unit kind. It also bounds the worst case of every stat over every research order, so the sim's checked math cannot overflow and a Town radius stays within 64 tiles.

## Income in micro-units

Income is exact by construction. For each `(player, resource)` the sim holds an `i64` accumulator `acc`.

```
acc += min(sum_rate_per_30s, yield_cap)      every tick
stockpile += acc / 600
acc %= 600
```

At 20 Hz, 30 seconds is 600 ticks, so a rate of `r` per 30 s adds exactly `r` to the stockpile every 600 ticks with no rounding drift.

| Tick | rate | acc before | acc after `+=` | stockpile `+=` | acc after `%=` |
|---|---|---|---|---|---|
| 1 | 10 | 0 | 10 | 0 | 10 |
| 60 | 10 | 590 | 600 | 1 | 0 |
| 120 | 10 | 590 | 600 | 1 | 0 |
| 600 | 10 | 590 | 600 | 1 (total 10) | 0 |

The Yield Cap is applied to the rate before accumulation, so production above the cap is discarded and the HUD readout turns amber (M3b). Lore is exempt from the Yield Cap and has its own stockpile cap of 999. The cap is data: the base 70 is `yield_cap_base` in `rules.ron`, and the Trade techs' `YieldCap Set` rows in `techs.ron` raise it to 100, 150 and 200.

## Schema strictness

Every struct in `crates/rules` is `#[serde(deny_unknown_fields)]`. A misspelled or obsolete field fails to load with the file path and field name. New optional fields use `#[serde(default)]` so existing data keeps loading. Files other than `rules.ron` (`resources.ron`, `units.ron`, `buildings.ron`, `techs.ron`, `ages.ron`, `factions.ron`, `visuals.ron`, every map under `maps/`) are parsed separately and attached to the `Rules` struct by `Rules::load`, so one struct carries everything and `rules_hash` covers it all.

Cross-reference checks run after parsing. Implemented as of M3a: every cost, yield and gather entry names a resource id; every unit's `trainer` names a building kind and every unit has a visual; every building's gather `worker` names a unit kind; each resource's `gathered_from` equals the building kinds that yield or gather it (checked both ways); exactly one building kind is the Town; `standard_start` names known units and lists every resource; every map start fits the Town footprint, in bounds and all grass. Added in M4a, with the `ages.ron` references resolved first so a removed tech is reported where `ages.ron` names it: every id an age unlocks exists and every unit kind, building kind and tech is unlocked by exactly one age, never before its trainer, host, predecessor level or `requires_tech`; age gates are monotonic and reachable; each tech line's levels come in order; a tech's host and a unit's `requires_tech` exist; every `Modifier` target is a valid `StatPath` with integer values and `Mul` in permille; tech and age costs fit the capped stockpiles; every attack lasts at least one tick and every hit does at least 1 damage; and every stat's worst case fits its type. Added in M5a: the age gates count only the techs that count toward an age (`counts_toward_age`, so not the Harrying side techs); `harrying_bonus_age` names an age; every `Kind` in `annexation_attackers` and `annexation_defenders` names a unit kind, each list reaches at least one kind and no attacker selector reaches a `Civilian` kind; the `HarryingLevel` rows reach at most level 8 and the attrition interval halved per level above 1 stays at least one tick; and the Town kind never has `stays_on_annexation`. Build orders join in M5b. The field-by-field list is in `data/rules/README.md`.

## How `data-check` reports errors

```bash
cargo run -p sim-cli -- data-check data/
```

Exit code 0 prints one line such as `OK rules_version=20 rules_hash=0x<16 hex> resources=4 units=8 buildings=8 ages=3 techs=12 side_techs=2 factions=1 maps=plains_1v1 strings=en` (`techs` counts the techs that count toward an age and `side_techs` the rest, from M5a the Watchtower's Harrying I and II; the hash changes whenever any loaded file changes; M1 added `units.ron` to it, M2 `visuals.ron`, M3a `buildings.ron`, M4a `techs.ron`, `ages.ron` and `factions.ron`; the M3b UI strings are checked but not hashed). Exit code 1 prints one line starting with `error:` in one of three shapes, from `rules::Error`:

| Variant | Format | Example cause |
|---|---|---|
| `Io` | `<path>: <os error>` | File missing or unreadable |
| `Parse` | `<path>: <line>:<col>-<line>:<col>: <parser message>` | Unknown field, wrong type, RON syntax; for example `` data/rules/rules.ron: 8:3-8:18: Unexpected field named `tick_rate_hertz` in `Rules`, expected one of `rules_version`, `tick_rate_hz`, ... instead `` (ron 0.12 wording) |
| `Invalid` | `<path>: <field>: <message>` | Parsed fine but failed a range or cross-reference rule, for example `tick_rate_hz: must be > 0` |

The same loader runs at game start, so a broken file is caught before a window opens. CI runs `data-check` in the headless job on every PR.

## File inventory

| File | Holds | Arrives |
|---|---|---|
| `data/rules/rules.ron` | `rules_version`, `tick_rate_hz`, `cmd_delay_ticks`, `default_map`, `yield_cap_base` and `pop_cap_base`, Town radii, growth threshold and limit (`town_limit_base: 2`), supply radius, attrition interval and annexation timer (`_ds`), path budget and cache size, the movement fields; from M3a the building and military ramps (`building_ramp_step_permille`, `military_ramp_step_permille`, `military_ramp_cap_permille`), `train_queue_max`, `idle_seek_delay_ds`, `idle_seek_retry_ds` and `standard_start`; from M4a `town_growth_age`, `damage_variance_permille`, `acquire_range_tiles` and `chase_regoal_tiles`; from M5a the attrition age-gap table `attrition_age_gap_hp`, the late Harrying bonus (`harrying_bonus_age`, `harrying_bonus_hp`), the annexation knobs (`annexation_adjacent_tiles`, `annexation_defender_radius_tiles`, `annexation_attackers`, `annexation_defenders`, `annexation_town_hp_permille`), the match cap `match_cap_ds` and the Yield Cap decay (`yield_cap_decay_after_ds`, `yield_cap_decay_every_ds`, `yield_cap_decay_step`) | M0 (values for later milestones are validated but unused until then). M3a added the ramp, queue, idle-seek and standard start fields and removed the placeholder `vision` radii (vision is per kind in `units.ron` and `buildings.ron`). M4a replaced the Yield Cap table with `yield_cap_base` and removed `pop_cap_per_arms_level` (the Trade and Arms rows in `techs.ron` hold those values) and added the growth age and the combat knobs. M5a's rules data (`rules_version` 18) added the age-gap table, the 45-minute cap, the annexation fields and the late-game pressure knobs |
| `data/rules/resources.ron` | Grain, Lumber, Ore, Lore: id, display name, gathering buildings, role, `yield_capped`, `stockpile_cap` | M0 |
| `data/rules/buildings.ron` | The eight building kinds (Town, Croft, Lumber Yard, Ore Pit, Scriptorium, Muster Hall, Watchtower, Engine Yard); the list index is the `BuildingKindId`. Fields: `id`, `name`, `town` (exactly one kind), `width_tiles`, `height_tiles`, `cost`, `build_time_ds`, `hp`, `vision_tiles`, `base_yield`, `gather` (`resource`, `worker`, `rate_per_worker`, `slots: Fixed(n) \| Adjacent(terrain, max)`), `max_per_town` (the Croft limit), `border_radius_tiles` (the Watchtower), `attack` (M4a: the Town and Watchtower shoot), `stays_on_annexation` (M5a, default `false`: the Watchtower stays with its owner when its Town is annexed; never on the Town). Schema `crates/rules/src/buildings.rs`, `deny_unknown_fields`; errors name `buildings[i].<field>` | M3a |
| `data/rules/units.ron` | Unit kinds; the list index is the `UnitKindId`. M1: `yeoman` (index 0) with the movement fields `speed_tiles_per_s_x100: 180`, `radius_tiles_x100: 35`, `arrive_radius_tiles_x100: 25`, `arrive_slowdown_radius_tiles_x100: 50`, `waypoint_radius_tiles_x100: 50`, `separation_tiles_x100: 10`, `return_to_post_radius_tiles_x100: 100` (the last one `#[serde(default)]` to 100: an arrived unit pushed farther than this from its post walks back; schema `crates/rules/src/units.rs`, `deny_unknown_fields`; validation: at least one kind, unique lowercase ids, speed, radius, waypoint radius and return radius > 0, slowdown radius and return radius >= arrive radius, every `_x100` at most 100000, errors name `units[i].<field>`). M3a adds `scribe` and `skirmisher` and the training fields `class`, `cost`, `ramp` (`Flat`, `PerUnit({...})` or `Military`), `pop`, `train_time_ds`, `trainer` (a `buildings.ron` id), `builder` and `vision_tiles`. M4a adds the remaining five military kinds (8 kinds in all), `hp`, `attack` (damage, `range_tiles_x100`, `cooldown_ds`, `prefer`), `requires_tech`, a radius cap of 50 and the top-level `counters` table in permille. M5a adds `immune_to_attrition` (the Scribe and the Supply Wain) and `supplies` (the Supply Wain), both `false` when absent | M1, M3a, M4a, M5a |
| `data/rules/techs.ron` | Twelve Scriptorium techs across four lines (Arms, Statecraft, Trade, Letters), and from M5a the Watchtower's Harrying I and II: `id`, `name`, `line`, `level`, `host`, `cost`, `research_time_ds`, the `modifiers` rows and `counts_toward_age` (default `true`; `false` for the Harrying side techs, which count toward no age gate); the list index is the `TechId`. Schema `crates/rules/src/techs.rs` | M4a, M5a |
| `data/rules/ages.ron` | Hearth, Masonry, Charter: `techs_required`, `cost` and `unlocks` (the unit kinds, building kinds and techs each age makes available; every id is listed by exactly one age). Schema `crates/rules/src/ages.rs` | M4a |
| `data/rules/factions.ron` | Freeholders with an empty `Modifier` list; second faction as pure data. Schema `crates/rules/src/factions.rs` | M4a, M9 |
| `data/rules/strings/en.ron` | UI strings (`reject` per `RejectReason`, `hud` labels). Not part of `rules_hash`: the sim never reads them, so editing them needs no `rules_version` bump; `data-check` validates keys and placeholders | M3b |
| `data/rules/README.md` | Authoritative schema and worked example | M0 |
| `data/README.md` | Layout, conventions and the license statement for `data/` (MIT OR Apache-2.0) | M0 |
| `data/maps/README.md`, `data/ai/README.md` | Per-directory schema in plain English | M0 |
| `data/maps/plains_1v1.ron` | The one 128x128 map: `name`, `width`, `height`, `symmetry` (`MirrorX`), two `starts`, and `rows` of `.` grass, `f` forest, `m` mountain, `~` water | M0 |
| `data/visuals.ron` | `(units: { "<kind id>": <Visual>, ... })`, keyed by `units.ron` `id`; every kind needs one. `Visual` is `Primitive(Capsule \| Cuboid \| Cylinder \| Sphere \| Cone)` (one shared mesh sized from the kind's `radius_tiles_x100`: a Capsule 3.4 radii tall, the others 2 radii; tinted with the owner's team colour), the struct variant `Body(shape: <Primitive>, height_tiles_x100: <u32>, lying: <bool, default false>)` (M4b: the same primitive two radii wide and `height_tiles_x100` tall, or, with `lying: true`, that long along the unit's facing and two radii tall) or the struct variant `Scene(path: "models/x.glb", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0)` (glTF under `assets/`; parsed and validated now, used from M7). Validation (`Visuals::validate`): a kind without a visual or a key naming no kind is an error naming `units["<id>"]`; a `Body` shape must not be `Sphere` (`units["<id>"].shape`), only a `Capsule` or `Cylinder` may lie (`.lying`), `height_tiles_x100` must be in `20..=400` and a `Capsule`'s at least twice the kind's `radius_tiles_x100` (`.height_tiles_x100`); a `Scene` path must end in `.glb` or `.gltf`, `scale_x100` must be in `1..=10000`, `yaw_deg` in `-360..=360`, errors naming `units["<id>"].<field>`; an unknown shape is a parse error with line and column; `deny_unknown_fields`. Schema `crates/rules/src/visuals.rs`, loaded by `Rules::load`, part of `rules_hash` (so a visual change still marks older replays `RULES CHANGED` and needs a `rules_version` bump with `just rerecord`; the `.hash` files stay the same); the sim never reads it, the game's `present.rs` does and draws every look low-poly and flat-shaded. M4b entries (`rules_version` 17): `yeoman` Primitive(Capsule), `scribe` Primitive(Cylinder), `skirmisher` Body Cone 150, `shieldbearer` Body Cuboid 130, `bowman` Body Cylinder 190, `outrider` Body Capsule 180 lying, `mangonel` Primitive(Sphere), `supply_wain` Body Cuboid 60. Building kinds are drawn by the game's `buildings.rs` | M2 (primitives), M4b (`Body`, `Cone`), M7 (glTF) |
| `data/ai/build_orders/*.ron` | Scripted build orders | M5b |
| `data/ai/difficulty.ron` | Easy, Standard, Hard: income interval and aggression flag | M6 |
| `data/ai/personalities.ron` | rush, boom, tower | M6 |

Every subdirectory carries a README with its schema in plain English and one worked example. `data/` is licensed like code; see [adr/0001-license.md](adr/0001-license.md).

## Adding a field

1. Add the field to the struct in `crates/rules/src/lib.rs` with `#[serde(default)]` if existing files must keep loading.
2. Add a validation rule if the field has a range or references another id.
3. Add the value to the RON file in authored units (tiles, deciseconds with `_ds`, per-30-s rates, permille).
4. Convert once in `crates/rules`: durations through `Rules::ticks_from_ds` behind an accessor such as `attrition_interval_ticks()`; expose ticks, `i32`, `i64` or `Fx` to the sim.
5. Document the field in `data/rules/README.md`.
6. Run `cargo run -p sim-cli -- data-check data/` and `cargo test -p rules`.
7. If the change alters simulation results, bump `rules_version` and regenerate fixtures in the same commit.
