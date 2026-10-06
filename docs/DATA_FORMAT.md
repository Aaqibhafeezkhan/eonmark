# Data format

This document explains how Eonmark's game data is authored: where the files live, the unit conventions, the `Modifier` type that expresses every tech and faction effect, the integer income model, how unknown fields are rejected and how `sim-cli data-check` reports problems. It is the overview. The authoritative, field-by-field schema and the worked authoring example live in `data/rules/README.md`; where this document and that README disagree, the README wins. The Rust schema is `crates/rules/src/lib.rs`.

## Principles

- Every gameplay number is RON under `data/`. There are no tuning constants in Rust. If a value is needed and no field exists, add the field, a `data-check` rule and a line in `data/rules/README.md`.
- Integers only. The `rules`, `sim` and `ai` crates ban `f32` and `f64` through `clippy.toml`, so a RON float literal has nowhere to go and fails to parse.
- Distances are tiles, in fields ending in `_tiles`. One tile is one metre in the renderer and `Fx::ONE` in the sim.
- Durations are authored in deciseconds, in fields ending in `_ds`. The one conversion is `Rules::ticks_from_ds`: `ticks = round(ds * tick_rate_hz / 10)`, rounding to nearest with halves up. At 20 Hz one decisecond is exactly two ticks: `32` ds is 64 ticks, `600` ds is 1200 ticks. The validator does not require a whole tick count; pick values that give one when you can. The exceptions are `tick_rate_hz` and `cmd_delay_ticks` in `rules.ron`, which define the clock itself.
- Conversion happens in one place. Authored values stay in `Rules` as integers; the sim reads them only through tick and `Fx` accessors such as `Rules::attrition_interval_ticks()` and `Rules::annexation_ticks()`, which call `Rules::ticks_from_ds`. No other code converts.
- Rates are per 30 seconds. "10 Grain per 30 s" is written as `10` in a field documented as per-30-s.
- `rules_version` is a human label bumped when golden replays regenerate; `rules_hash` is computed over everything `Rules::load` reads (match rules, resources and every map at M0). Both go into every replay header. See [DETERMINISM.md](DETERMINISM.md).

## The `Modifier` type

Every tech and faction effect is a row of this shape (M4a):

```ron
Modifier(target: YieldCap, op: Mul, value: 1250)
```

| Field | Type | Meaning |
|---|---|---|
| `target` | `StatPath` | A closed enum of stats the effect may touch (yield cap, unit hp, research cost, border push, ...). `data-check` rejects a target that is not in the enum. |
| `op` | `Set`, `Add` or `Mul` | How the value combines with the base. |
| `value` | `i32` | For `Set` and `Add`, an absolute amount in the stat's own unit. For `Mul`, a permille multiplier: `1000` is unchanged, `1250` is x1.25, `900` is x0.9. |

Resolution order is `Set`, then `Add`, then `Mul`, in integer math with floor rounding:

```
result = floor( (set_or_base + sum(add)) * product(mul) / 1000^n )
```

| Base | Set | Add | Mul (permille) | Result | Note |
|---|---|---|---|---|---|
| 70 | none | none | 1250 | 87 | `70 * 1250 / 1000 = 87.5`, floored |
| 70 | none | +10 | 1250 | 100 | `(70 + 10) * 1250 / 1000 = 100` |
| 70 | 100 | +10 | none | 110 | `Set` replaces the base first |
| 100 | none | none | 1100, 1100 | 121 | `100 * 1100 / 1000 = 110`, then `110 * 1100 / 1000 = 121` |

`data-check` rejects non-integer `Add` and `Set` values (they cannot parse as `i32`) and a `Mul` value of `0` or below.

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

The Yield Cap is applied to the rate before accumulation, so production above the cap is discarded and the HUD readout turns amber (M3b). Lore is exempt from the Yield Cap and has its own stockpile cap of 999. Cap values (70, then 100/150/200 via Trade techs) are data in `rules.ron` (M3a).

## Schema strictness

Every struct in `crates/rules` is `#[serde(deny_unknown_fields)]`. A misspelled or obsolete field fails to load with the file path and field name. New optional fields use `#[serde(default)]` so existing data keeps loading. Files other than `rules.ron` (`resources.ron`, every map under `maps/`) are parsed separately and attached to the `Rules` struct by `Rules::load`, so one struct carries everything and `rules_hash` covers it all.

Cross-reference checks run after parsing: every unit has a trainer building and a visual, every building and unit kind referenced by a tech or build order exists, age gates are monotonic, every `Modifier` target is a valid `StatPath`, `Add` and `Set` values are integers and `Mul` values are permille.

## How `data-check` reports errors

```bash
cargo run -p sim-cli -- data-check data/
```

Exit code 0 prints one line such as `OK rules_version=1 rules_hash=0xfbd6ae4c125b88a2 resources=4 maps=plains_1v1`. Exit code 1 prints one line starting with `error:` in one of three shapes, from `rules::Error`:

| Variant | Format | Example cause |
|---|---|---|
| `Io` | `<path>: <os error>` | File missing or unreadable |
| `Parse` | `<path>: <line>:<col>-<line>:<col>: <parser message>` | Unknown field, wrong type, RON syntax; for example `` data/rules/rules.ron: 7:3-7:18: Unexpected field named `tick_rate_hertz` in `Rules`, expected one of `rules_version`, `tick_rate_hz`, ... instead `` (ron 0.12 wording) |
| `Invalid` | `<path>: <field>: <message>` | Parsed fine but failed a range or cross-reference rule, for example `tick_rate_hz: must be > 0` |

The same loader runs at game start, so a broken file is caught before a window opens. CI runs `data-check` in the headless job on every PR.

## File inventory

| File | Holds | Arrives |
|---|---|---|
| `data/rules/rules.ron` | `rules_version`, `tick_rate_hz`, `cmd_delay_ticks`, `default_map`, the Yield Cap table, pop cap base and per-Arms-level, Town radii, growth threshold and limit, supply radius, attrition interval and annexation timer (`_ds`), vision radii, path budget and cache size | M0 (full v0.1 field set; values for later milestones are validated but unused until then). Ramping parameters, Harrying doubling, the age-gap table, the 45-minute cap and late-game pressure knobs are added by M3a, M4a and M5a |
| `data/rules/resources.ron` | Grain, Lumber, Ore, Lore: id, display name, gathering buildings, role, `yield_capped`, `stockpile_cap` | M0 |
| `data/rules/buildings.ron` | The eight building kinds: costs, build time, slots, border push, hp | M3a |
| `data/rules/units.ron` | Yeoman and Scribe; then the six military kinds with stats and counter table | M3a, M4a |
| `data/rules/techs.ron` | Twelve techs as `Modifier` lists across four lines | M4a |
| `data/rules/ages.ron` | Hearth, Masonry, Charter: tech-count gates and costs | M4a |
| `data/rules/factions.ron` | Freeholders with an empty `Modifier` list; second faction as pure data | M4a, M9 |
| `data/rules/strings/en.ron` | UI strings | M3b |
| `data/rules/README.md` | Authoritative schema and worked example | M0 |
| `data/README.md` | Layout, conventions and the license statement for `data/` (MIT OR Apache-2.0) | M0 |
| `data/maps/README.md`, `data/ai/README.md` | Per-directory schema in plain English | M0 |
| `data/maps/plains_1v1.ron` | The one 128x128 map: `name`, `width`, `height`, `symmetry` (`MirrorX`), two `starts`, and `rows` of `.` grass, `f` forest, `m` mountain, `~` water | M0 |
| `data/visuals.ron` | `Visual::Primitive(kind)` or `Visual::Scene(path, scale, y_offset, yaw)` per unit and building kind | M2 (primitives), M7 (glTF) |
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
