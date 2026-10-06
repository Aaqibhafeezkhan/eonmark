# Tech lines and ages

This document specifies research and age advancement: the 12 Scriptorium techs in four lines of three levels, the integer Modifier model every effect is expressed in, the tech-count gates that open the Masonry and Charter Ages, and what each age unlocks. Every number here is a starting value in `data/rules/techs.ron`, `data/rules/ages.ron` and `data/rules/rules.ron` and may move during M6 tuning. The sim code is `crates/sim/src/tech.rs`; the Modifier resolver and validator rules live in `crates/rules`. For the design lineage see [prior-art.md](prior-art.md).

| Field | Value |
| --- | --- |
| Milestone | M4a (sim), M4b (research panel and age indicator) |
| Labels | `area:sim`, `area:rules` |
| Rules files | `data/rules/techs.ron` (12 techs as Modifier lists, Harrying I/II), `data/rules/ages.ron` (3 ages, gates, unlock lists), `data/rules/rules.ron` (base values the Modifiers act on), `data/rules/factions.ron` (faction Modifier lists; Freeholders has none) |
| Sim module | `crates/sim/src/tech.rs` |
| Sub-hash | `tech` |

## The 12 techs

Four lines, three levels each, researched at a Scriptorium. Level k of a line requires level k-1 of the same line.

| Line | Per level | Modifier rows (starting data) | Pays mainly in |
| --- | --- | --- | --- |
| Arms | +25 pop cap; unlocks the age's units; +10% unit stats | `pop_cap Add 25`; unit stat rows `Mul 1100`; unit kinds list the Arms level they require | Ore and Lore |
| Statecraft | +1 Town limit; +1 border push | `town_limit Add 1`; `town_border_radius Add 1` | Lore |
| Trade | Yield Cap 100, then 150, then 200 | `yield_cap Set 100` / `Set 150` / `Set 200` | Lore |
| Letters | -10% research cost and time, floored | `research_cost Mul 900`; `research_time Mul 900` | Lore |

Resulting values by level:

| Level | Pop cap (Arms) | Town limit (Statecraft) | Border push (Statecraft) | Yield Cap (Trade) | Research cost factor (Letters) |
| --- | --- | --- | --- | --- | --- |
| 0 | 25 | 1 | +0 | 70 | 1.000 |
| 1 | 50 | 2 | +1 | 100 | 0.900 |
| 2 | 75 | 3 | +2 | 150 | 0.810 |
| 3 | 100 | 4 | +3 | 200 | 0.729 |

Tech costs and research times are not fixed by the design; they are set in `techs.ron` at M4a. Every tech costs Lore; Arms techs also cost Ore (see [economy.md](economy.md)).

### Arms and unit unlocks

The design says Arms "unlocks the age's units" and also lists Skirmisher, Shieldbearer and Bowman as available in the starting age. This doc resolves it as data: every unit kind in `units.ron` has an optional `requires_arms` level. In the starting data the Hearth kinds require none, and the Masonry kinds (Outrider, Mangonel, Supply Wain) require the Masonry Age. Whether they should also require Arms I is open question 1. The +10% stat bump applies to the unit StatPaths named by the Arms rows (hit points and damage in the starting data). One stat bump per age: Arms III is the Charter Age bump.

### Age requirement per level

Level k of every line requires age k: level 1 in the Hearth Age, level 2 in the Masonry Age, level 3 in the Charter Age. This follows "one stat bump per age via Arms techs" and the Charter unlock list, and it means the Masonry gate (2 techs) is met with two level-1 techs. It is open question 2.

## Modifier model

Every tech and faction effect is one row:

```text
Modifier { target: StatPath, op: Set | Add | Mul, value: i32 }
```

`StatPath` is a closed enum in `crates/rules` (for example `yield_cap`, `pop_cap`, `town_limit`, `town_border_radius`, `research_cost`, `research_time`, per-kind unit stats, `watchtower_border_push`, `lumber_gather_rate`). `Mul` values are permille: 1250 means x1.25. `Add` and `Set` values are plain integers. `sim-cli data-check` rejects a float anywhere in a Modifier and an unknown StatPath.

Resolution for one StatPath, in integer math:

```text
fn resolve(base: i32, rows: &[Modifier]) -> i32 {
    let mut v = base;
    if let Some(m) = rows.iter().filter(|m| m.op == Set).last() { v = m.value; }  // Set
    for m in rows.iter().filter(|m| m.op == Add) { v += m.value; }                // Add
    for m in rows.iter().filter(|m| m.op == Mul) { v = v * m.value / 1000; }      // Mul, floor
    v
}
```

Rows are collected in a fixed order: faction rows first, then researched techs in `TechId` order, then each tech's rows in file order. Integer division truncates toward zero, which equals floor for the non-negative stats the game uses; floor is the documented rule.

Worked examples:

| Base | Rows | Result |
| --- | --- | --- |
| yield_cap 70 | faction `Mul 1250` | 70 * 1250 / 1000 = 87 |
| yield_cap 70 | Trade I `Set 100`, faction `Mul 1250` | 100 * 1250 / 1000 = 125 |
| pop_cap 25 | Arms I, II `Add 25` x2 | 75 |
| research_cost 100 | Letters I, II, III `Mul 900` x3 | 100 -> 90 -> 81 -> 72 |
| town_limit 1 | Statecraft I `Add 1` | 2 |

The faction example is the M9 Tidewater League row (`yield_cap Mul 1250`, `lumber gather Mul 1100`, `Watchtower border push Add -1`) and the M4a acceptance test. Freeholders, the v0.1 faction, has an empty Modifier list.

## Research flow

```text
Research { building: Scriptorium, tech }
  validate: owner, building kind, previous level done, age requirement met,
            not already researched or in progress, stockpiles
  deduct cost (after Letters discount, floored)
  progress in ticks (Rules::ticks_from_ds(research_time_ds), after Letters discount, floored)
  on completion: add the tech's rows; recompute derived values
               (pop cap, yield cap, town limit, territory radii, unit stats)
```

Each Scriptorium researches one tech at a time; several Scriptoria research in parallel. A `Cancel` refunds the cost. Rejections emit `CommandRejected` with `CannotAfford`, `AgeRequired` or `PrerequisiteMissing` (proposed names).

## Ages

Ages are gated by the cumulative count of researched Scriptorium techs plus a resource cost. Harrying I and II do not count.

| Age | Cumulative techs | Grain | Lumber | Ore | Lore |
| --- | --- | --- | --- | --- | --- |
| Hearth (I) | start | | | | |
| Masonry (II) | 2 (any) | 250 | 0 | 0 | 100 |
| Charter (III) | 5 | 500 | 300 | 0 | 300 |

`AdvanceAge { player }` is validated against the current age, the tech count and the stockpiles, deducts the cost and advances immediately. An optional advance time in `ages.ron` is open question 4.

### What each age unlocks

| Age | Buildings | Units | Other |
| --- | --- | --- | --- |
| Hearth (I) | Croft, Lumber Yard, Ore Pit, Scriptorium, Muster Hall, Watchtower (and Town) | Yeoman, Scribe, Skirmisher, Shieldbearer, Bowman | starting age |
| Masonry (II) | Engine Yard | Outrider, Mangonel, Supply Wain | Harrying I research at the Watchtower; second Town growth tier (radius 24, see [towns.md](towns.md)) |
| Charter (III) | none new | none new | age-III stat bump for all unit lines (Arms III); Harrying II; the late-game pressure bonus (see [attrition.md](attrition.md)) |

### A fourth age is data

`ages.ron` is a list. A fourth age (working name "Powder Age") is a community issue, not a v0.1 feature: it would add rows to `ages.ron`, `units.ron`, `buildings.ron` and `techs.ron` with no Rust change. `data-check` enforces gate monotonicity (tech counts and costs never decrease from one age to the next) and that every unlock reference resolves, so the list can grow safely. Content for gunpowder and later eras is out of scope for v0.1 (see `docs/ROADMAP.md`).

## Validator rules (`sim-cli data-check`)

| Check | Failure |
| --- | --- |
| Every Modifier `target` is a known StatPath | exit 1 naming file and field |
| `Add` and `Set` values are integers; `Mul` values are integers read as permille | a `Mul` written as `1.25` exits 1 |
| Age gates are monotonic | exit 1 |
| Every tech referenced by `ages.ron` exists | removing one exits 1 naming file and field |
| Every unit kind has a trainer and every level has its predecessor | exit 1 |
| Counts: 3 ages, 8 unit kinds, 8 building kinds, 12 techs | printed in the summary |

## Interactions

| System | Interaction |
| --- | --- |
| [economy.md](economy.md) | Trade sets the Yield Cap; Arms raises the pop cap; Letters lowers research cost; ages cost Grain, Lumber and Lore; Lore is the research currency. |
| [territory.md](territory.md) | Statecraft adds 1 to every own Town radius; each level triggers a field recompute. |
| [towns.md](towns.md) | Statecraft raises the town limit; Masonry gates tier-2 radius. |
| [combat.md](combat.md) | Arms applies the +10% stat bump; ages unlock unit kinds and the Engine Yard. |
| [attrition.md](attrition.md) | Harrying I and II are Watchtower researches gated by Masonry and Charter. |
| [ai.md](ai.md) | The bot's build order reaches the Masonry Age before tick 18000 (M5b acceptance). |
| [ui.md](ui.md) | Research panel with prerequisites and costs, age indicator and Advance button (M4b). |

## Acceptance checklist (from M4a)

- [ ] `cargo test -p sim tech`: Masonry Age unlocks only after 2 techs plus 250 Grain + 100 Lore; Charter Age after 5 techs plus 500 Grain + 300 Lumber + 300 Lore; each Letters level reduces the next research cost by 10% floored; Modifier stacking Set -> Add -> Mul produces the documented values; a faction row `yield_cap Mul 1250` yields cap 87 at Trade level 0.
- [ ] `cargo run -p sim-cli -- data-check data/` passes with 3 ages, 8 unit kinds, 8 building kinds, 12 techs; removing a tech referenced by ages.ron exits 1 naming file and field; a Mul value written as 1.25 exits 1.

```bash
cargo test -p sim tech
cargo run -p sim-cli -- data-check data/
```

## Open tuning questions

1. Should Outrider, Mangonel and Supply Wain require Arms I as well as the Masonry Age? The age list gates them by age; the Arms line says it unlocks the age's units. This doc follows the age list and keeps `requires_arms` as a data field.
2. Does level k of every line require age k? This doc says yes. Without it a Hearth player could research Trade III for a cap of 200.
3. Letters stacking: multiplicative with floor after each row (0.729 at level 3) versus additive (-30%).
4. Instant age advance versus a timed advance in `ages.ron`.
5. Tech costs and research times are unset; M4a picks them and the balance issue form asks for a play-bots table with any change.
6. Which unit StatPaths the Arms bump touches: hit points and damage in the starting data; range and speed are candidates.
