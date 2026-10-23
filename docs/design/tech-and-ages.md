# Tech lines and ages

This document specifies research and age advancement: the 12 Scriptorium techs in four lines of three levels, the integer Modifier model every effect is expressed in, the tech-count gates that open the Masonry and Charter Ages, and what each age unlocks. Every number here is a starting value in `data/rules/techs.ron`, `data/rules/ages.ron` and `data/rules/rules.ron` and may move during M6 tuning. The sim code is `crates/sim/src/tech.rs`; the Modifier resolver and validator rules live in `crates/rules`. For the design lineage see [prior-art.md](prior-art.md).

| Field | Value |
| --- | --- |
| Milestone | M4a (sim), M4b (research panel and age indicator) |
| Labels | `area:sim`, `area:rules` |
| Rules files | `data/rules/techs.ron` (12 techs as Modifier lists; Harrying I/II join in M5a), `data/rules/ages.ron` (3 ages, gates, unlock lists), `data/rules/rules.ron` (base values the Modifiers act on), `data/rules/factions.ron` (faction Modifier lists; Freeholders has none) |
| Sim module | `crates/sim/src/tech.rs` |
| Sub-hash | `tech` |

## The 12 techs

Four lines, three levels each, researched at a Scriptorium. Level k of a line requires level k-1 of the same line.

| Line | Per level | Modifier rows (starting data) | Pays mainly in |
| --- | --- | --- | --- |
| Arms | +25 pop cap; unlocks the age's units; +10% unit stats | `pop_cap Add 25`; unit stat rows `Mul 1100`; units are gated by age, and a unit kind may also name a tech in `requires_tech` (none does in the starting data) | Ore and Lore |
| Statecraft | +1 Town limit; +1 border push | `town_limit Add 1`; `town_border_radius Add 1` | Lore |
| Trade | Yield Cap 100, then 150, then 200 | `yield_cap Set 100` / `Set 150` / `Set 200` | Lore |
| Letters | -10% research cost and time, floored | `research_cost Mul 900`; `research_time Mul 900` | Lore |

Resulting values by level:

| Level | Pop cap (Arms) | Town limit (Statecraft) | Border push (Statecraft) | Yield Cap (Trade) | Research cost factor (Letters) |
| --- | --- | --- | --- | --- | --- |
| 0 | 25 | 2 | +0 | 70 | 1.000 |
| 1 | 50 | 3 | +1 | 100 | 0.900 |
| 2 | 75 | 4 | +2 | 150 | 0.810 |
| 3 | 100 | 5 | +3 | 200 | 0.729 |

The Town limit starts at 2 (`town_limit_base` in `rules.ron`; the Seat counts), by owner decision, so a second Town needs no Statecraft research; each Statecraft level adds 1 (see [towns.md](towns.md)).

Tech costs and research times are not fixed by the design; they are set in `techs.ron` at M4a. Every tech costs Lore; Arms techs also cost Ore (see [economy.md](economy.md)).

### Arms and unit unlocks

The design says Arms "unlocks the age's units" and also lists Skirmisher, Shieldbearer and Bowman as available in the starting age. This doc resolves it as data: `ages.ron` unlocks every unit kind in exactly one age, and every unit kind in `units.ron` has an optional `requires_tech` naming a tech it also needs (M4a; it replaces the `requires_arms` level first proposed here). In the starting data no kind sets it: the Hearth kinds are available from the start, and the Masonry kinds (Outrider, Mangonel, Supply Wain) require the Masonry Age. Whether they should also require Arms I was open question 1. The +10% stat bump applies to the unit StatPaths named by the Arms rows (hit points and damage in the starting data). One stat bump per age: Arms III is the Charter Age bump.

### Age requirement per level

Level k of every line requires age k: level 1 in the Hearth Age, level 2 in the Masonry Age, level 3 in the Charter Age. This follows "one stat bump per age via Arms techs" and the Charter unlock list, and it means the Masonry gate (2 techs) is met with two level-1 techs. It was open question 2, decided by the M4a plan.

## Modifier model

Every tech and faction effect is one row:

```text
Modifier { target: StatPath, op: Set | Add | Mul, value: i32 }
```

`StatPath` is a closed enum in `crates/rules` (`YieldCap`, `PopCap`, `TownLimit`, `TownBorderRadius`, `ResearchCost`, `ResearchTime`, `UnitHp(sel)` and `UnitDamage(sel)`, with `sel` one of `Military`, `Class(<UnitClass>)` or `Kind("<unit id>")`; gather-rate and Watchtower border paths join with the M9 factions). `Mul` values are permille: 1250 means x1.25. `Add` and `Set` values are plain integers. `sim-cli data-check` rejects a float anywhere in a Modifier and an unknown StatPath.

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
| research_cost 150 | Letters I, II, III `Mul 900` x3 | 150 -> 135 -> 121 -> 108 (one floor over 0.729 would give 109) |
| town_limit 2 | Statecraft I `Add 1` | 3 |

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

Each Scriptorium researches one tech at a time; several Scriptoria research in parallel. A `Cancel` refunds the cost. Rejections emit `CommandRejected` with `CannotAfford`, `AgeRequired` or `PrerequisiteMissing`, among others (the full check order is in "M4a implementation notes").

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
| Every Modifier `target` is a known StatPath | exit 1: a parse error naming the file, the position and `StatPath` or `UnitSel` |
| `Add` and `Set` values are integers; `Mul` values are integers read as permille | a `Mul` written as `1.25` exits 1 |
| Age gates are monotonic | exit 1 |
| Every tech referenced by `ages.ron` exists | removing one exits 1 naming file and field |
| Every unit kind has a trainer and every level has its predecessor | exit 1 |
| Counts: 3 ages, 8 unit kinds, 8 building kinds, 12 techs | printed in the summary |

## M4a implementation notes

The tech slice (`SIM_VERSION` 12, `rules_version` 14) implements research, ages and the Modifier-resolved player stats in `crates/sim/src/tech.rs`, on the rules data of the slice before (`rules_version` 13). The Arms rows for unit hit points and damage took effect with the M4a combat core slice (`SIM_VERSION` 13, `rules_version` 15; see the Arms row below). Where it is more precise than the sections above:

| Topic | As implemented |
| --- | --- |
| State | `State.tech: TechState { players: Vec<PlayerTech> }`, indexed by `PlayerId`, with `PlayerTech { age: u8, researched: BTreeSet<TechId> }`, hashed as the `tech` sub-hash. Every player starts in `ages[0]` with nothing researched. A tech being researched is not in `researched`: it is a `QueueItem::Research { tech, ticks }` entry in its host's queue, hashed in `buildings`. |
| Resolution | `tech::stat(p, path, base)` collects the rows of `factions[0]` (every player's faction until M9), then each researched tech's rows in `TechId` order and file order, keeps those whose target is `path` and passes them to `rules::modifiers::resolve`: the last `Set`, then the sum of the `Add`s, then each `Mul` as `v = (v * m).div_euclid(1000)`, in checked `i64`. The wrappers clamp: the Yield Cap, the pop cap, the town limit and each research cost at 0 or more, the Town radius to 1..=64, the research time to at least 1 tick after `Rules::ticks_from_ds`. A `ResearchCost` row applies to each resource listed in the tech's `cost`; unlisted resources cost 0. Nothing resolved is stored: `SimView::income`, `pop` and `towns` and every check call the same functions as `Sim::step`. |
| `Research` | Check order, the first failure wins: (1) the building is the player's (`UnknownBuilding`) and (2) complete (`UnderConstruction`); (3) the tech exists (`UnknownTech`); (4) the building's kind is the tech's `host` (`CannotResearchHere`); (5) the level below it in its line is researched (`PrerequisiteMissing`); (6) the player has reached the age that unlocks it (`AgeRequired`); (7) it is not researched (`AlreadyResearched`) and (8) not queued at any of the player's buildings (`ResearchInProgress`); (9) the queue holds fewer than `train_queue_max` (5) entries (`QueueFull`); (10) the player can pay the quote (`CannotAfford`, the first short resource). An accepted `Research` deducts the quote and appends `QueueEntry { item: Research { tech, ticks }, paid, progress: 0 }`. |
| Queue | Research shares the host's training queue (owner decision 2026-10-21): only the head progresses, one tick per step 6b, so a Scriptorium that researches trains no Scribe until the entry finishes or is cancelled, and several Scriptoria research in parallel. Research entries take no pop and count toward no unit cost ramp. `Cancel` of a research entry refunds exactly its `paid`, clamped to the stockpile caps, and emits `TrainingCancelled`, as for a unit. |
| Cost and time | Fixed when queued (owner decision 2026-10-21): `tech::research_quote` applies the Letters rows researched at that moment and the entry keeps the cost in `paid` and the time in `ticks`, so Letters speeds only research queued after it. Arms I at Letters 0, I, II and III quotes 75, 67, 60 and 54 Lore, 50, 45, 40 and 36 Ore, and 300, 270, 243 and 218 ds (600, 540, 486 and 436 ticks): each step is `floor(previous * 900 / 1000)`. |
| Completion | A research head completes in the step its progress reaches `ticks`, that is the `ticks`-th step in which it was the head (an entry queued into an empty queue is the head in the step that applied the command). `tech::complete` adds the tech to `researched`, emits `ResearchCompleted { player, tech }` and re-derives every complete Town of the player (`town::refresh_all`; Statecraft pushes the borders, see [towns.md](towns.md)). |
| `AdvanceAge` | Check order: (1) `player` is the issuer (`PlayerMismatch`; owner decision 2026-10-21); (2) the player is not in the last age (`FinalAge`); (3) the player has completed at least the next age's `techs_required` techs, queued ones not counting (`NotEnoughTechs`); (4) the player can pay the next age's cost (`CannotAfford`). An accepted `AdvanceAge` deducts the cost, enters the next age at once, emits `AgeAdvanced { player, age }` and re-derives every complete Town of the player (Masonry opens tier 2). One command moves one age. |
| Age and tech gates | `Train` checks `AgeRequired` and then `PrerequisiteMissing` (the kind's `requires_tech`) after `CannotTrainHere`; `Build` checks `AgeRequired` right after the resume branch (the full list is in [towns.md](towns.md)); `SimView::check_build_kind(p, kind)` makes the `Build` checks that need neither a builder nor a tile (the age, the town limit and the cost, in that order), which the M3b command card uses to grey a Build button. `DebugSpawn` and `DebugPlaceBuilding` ignore ages. |
| `DebugResearch` | `DebugResearch { owner, tech }` is appended to `Command`. Checks: `DebugCommandsDisabled`, `UnknownPlayer`, `UnknownTech`, `AlreadyResearched`, `ResearchInProgress` (so a tech never completes twice); it skips host, cost, time, prerequisite and age and calls `tech::complete`. |
| `SimView` | `age(p)`, `researched(p)`, `research_quote(p, tech)` (cost and ticks), `check_research(p, building, tech)`, `check_advance_age(p)` and `check_build_kind(p, kind)` (see the gates row); each check runs the player checks `Sim::apply` makes first (`UnknownPlayer`, `Surrendered`). `income`, `pop` and `towns` return the resolved Yield Cap, pop cap and town limit. |
| Arms hit points and damage | Since the combat core slice `tech::unit_max_hp` and `tech::unit_damage` resolve a unit kind's `hp` and attack `damage` through the owner's rows whose target is `UnitHp(sel)` or `UnitDamage(sel)` with a selector that reaches the kind (`Military` is every class but `Civilian`), in the same order as the player stats, clamped to at least 1 and at least 0. They are read every tick and never stored: a unit keeps only the damage it has taken, so an Arms level completed mid-fight raises a living unit's hit points by as much as its maximum. Each level's `Mul 1100` is floored per row: the Skirmisher goes 80, 88, 96, 105 hit points and 10, 11, 12, 13 damage, the Bowman 46, 50, 55, 60. `SimView::unit_stats(p, kind)` and `unit_hp(id)` read them. Tests: `tech_arms_raises_military_hp_and_damage_including_living_units` and `tech_every_arms_level_strictly_raises_every_military_stat`. |
| RNG | `Research`, `AdvanceAge` and `DebugResearch` draw nothing, accepted or rejected. |

Tests (`crates/sim/tests/m4a.rs`, `cargo test -p sim tech`), by acceptance clause: the Masonry and Charter gates (`tech_masonry_age_unlocks_only_after_two_techs_and_250_grain_100_lore`, `tech_charter_age_unlocks_only_after_five_techs_and_500_grain_300_lumber_300_lore`, `tech_age_gates_match_the_design_numbers`), the Letters discount (`tech_each_letters_level_cuts_the_next_research_cost_by_ten_percent_floored`), Modifier stacking (`tech_modifier_stacking_set_add_mul_produces_the_documented_values`) and the faction cap (`tech_faction_yield_cap_mul_1250_gives_cap_87_at_trade_level_0`). Supporting tests cover completion at the host after the fixed time, every rejection against a control sim (every sub-hash, `rng` included), level k needing level k-1 and age k for all 12 techs, Trade, Statecraft and Arms at every level, `requires_tech` (set in memory), the Masonry gates on the Engine Yard, the Outrider and Town tier 2, the exact refund on `Cancel`, the time fixed at queue time, no pop and no ramp for research entries, `DebugResearch`, restore mid-research and the view checks against apply.

## M4b notes (game)

The research panel lives on the Scriptorium's command card (`crates/game/src/command_card.rs`; [ui.md](ui.md#research-panel-m4b)): one button per tech line with the line's next level, its quoted cost from `SimView::research_quote`, greyed exactly when `SimView::check_research` rejects it, with the blocker on the button ("Masonry Age", "Researching 40%", "Researched") and the level before plus the age in the tooltip. The age readout and the Advance button sit on the age strip under the right end of the top bar (`crates/game/src/age_hud.rs`; [ui.md](ui.md#age-indicator-and-advance-button-m4b)): the button is greyed exactly when `SimView::check_advance_age` rejects, with the tech count ("Advance: 1 / 2 techs") while that is short (owner decision 7), else showing the next age's cost ("Advance to Masonry Age: 250 Grain 100 Lore"), and hidden in the last age. The message line says "Letters I researched" and "Masonry Age reached" for the local player's `ResearchCompleted` and `AgeAdvanced`. The game re-implements none of the checks above. Because `Research` checks the age (6) before "already researched" (7), a finished line's top level reads "Researched" only in the age that unlocks it; in play that is always the case, since level 3 needs the Charter Age.

ROADMAP M4b acceptance line 1 exercises all of it in one match: build a Scriptorium, research two techs, advance to the Masonry Age, build an Engine Yard, train Mangonels and a mixed army, and destroy the Watchtower of `--scenario enemy_outpost`. `enemy_outpost_walkthrough` is the scripted sequence (Letters I, then Trade I at the Letters price, the Masonry Age during tick 1850, the golden fixture), and `scripts/m4b_outpost_drive.sh` plays the same sequence on the interactive scenario with real X input under Xvfb, its recorded replay verifying ([../PLAYTEST.md](../PLAYTEST.md#automated-proxies-m4b)).

## Interactions

| System | Interaction |
| --- | --- |
| [economy.md](economy.md) | Trade sets the Yield Cap; Arms raises the pop cap; Letters lowers research cost; ages cost Grain, Lumber and Lore; Lore is the research currency. |
| [territory.md](territory.md) | Statecraft adds 1 to every own Town radius; each level triggers a field recompute. |
| [towns.md](towns.md) | Statecraft raises the town limit; Masonry gates tier-2 radius. |
| [combat.md](combat.md) | Arms applies the +10% stat bump; ages unlock unit kinds and the Engine Yard. |
| [attrition.md](attrition.md) | Harrying I and II are Watchtower researches gated by Masonry and Charter. |
| [ai.md](ai.md) | The bot's build order reaches the Masonry Age before tick 18000 (M5b acceptance). |
| [ui.md](ui.md) | Research panel with prerequisites and costs (M4b, done: the next level of each line on the Scriptorium's card), age indicator and Advance button (M4b, done: on a strip under the top bar, greyed until the gate is met). |

## Acceptance checklist (from M4a)

- [x] `cargo test -p sim tech`: Masonry Age unlocks only after 2 techs plus 250 Grain + 100 Lore; Charter Age after 5 techs plus 500 Grain + 300 Lumber + 300 Lore; each Letters level reduces the next research cost by 10% floored; Modifier stacking Set -> Add -> Mul produces the documented values; a faction row `yield_cap Mul 1250` yields cap 87 at Trade level 0.

  Observed on PR #35, 2026-10-22, with `cargo test -p sim --profile ci tech -- --nocapture` (the `ci` profile inherits `dev`; the tests are the same): `test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 44 filtered out` in m4a. `masonry: 2 techs + 250 Grain + 100 Lore -> age 1, stock [250, 0, 0, 100] -> 0`; the Charter gate in `tech_charter_age_unlocks_only_after_five_techs_and_500_grain_300_lumber_300_lore`; `letters 0: arms_1 costs 75 Lore + 50 Ore, 600 ticks` down to `letters 3: arms_1 costs 54 Lore + 36 Ore, 436 ticks`; the stacking values of the Modifier model (87, 72 with a floor after each `Mul`, 137 for Set 100, Add 10 and Mul 1250 in every row order); `faction Mul 1250, Trade 0: period 0 +87 Grain`, and the same in periods 1 and 2.

- [x] `cargo run -p sim-cli -- data-check data/` passes with 3 ages, 8 unit kinds, 8 building kinds, 12 techs; removing a tech referenced by ages.ron exits 1 naming file and field; a Mul value written as 1.25 exits 1.

  Observed on PR #35, 2026-10-22: `cargo run -p sim-cli --profile ci -- data-check data` prints `OK rules_version=16 rules_hash=0xaee9a61e782125ee resources=4 units=8 buildings=8 ages=3 techs=12 factions=1 maps=plains_1v1 strings=en` (the same for `data/`) and exits 0; `cargo test -p sim-cli --profile ci --test data_check` passes its 3 tests, which run the binary on edited copies of `data/`. By hand on the same copies: `error: <copy>/rules/ages.ron: ages[1].unlocks.techs[1]: "statecraft_2" is not an id in techs.ron` and `error: <copy>/rules/techs.ron: 143:49: Expected comma`, both exit 1.

```bash
cargo test -p sim tech
cargo run -p sim-cli -- data-check data/
```

## Open tuning questions

1. Should Outrider, Mangonel and Supply Wain require Arms I as well as the Masonry Age? The age list gates them by age; the Arms line says it unlocks the age's units. This doc follows the age list and keeps the Arms gate as a data field. Decided 2026-10-21 (owner accepted the M4a plan defaults): no, the Masonry Age alone gates them. The data field is `requires_tech` on the unit kind (`None` for all three), so an Arms I gate is one data line.
2. Does level k of every line require age k? This doc says yes. Without it a Hearth player could research Trade III for a cap of 200. Decided 2026-10-21 by the M4a plan (an implementation choice, not one of the nine owner decisions in ROADMAP.md; the owner may revisit): yes. `ages.ron` unlocks level k of every line in age k (the Hearth Age the four level-1 techs, Masonry level 2, Charter level 3), `Research` rejects an earlier level with `AgeRequired`, and `data-check` keeps each level unlocked no earlier than the level before it.
3. Letters stacking: multiplicative with floor after each row (0.729 at level 3) versus additive (-30%). Decided 2026-10-21 by the M4a plan (an implementation choice, not one of the nine owner decisions in ROADMAP.md; the owner may revisit): multiplicative, floored after each `Mul 900` row, so a base of 100 becomes 90, 81 and 72.
4. Instant age advance versus a timed advance in `ages.ron`. Decided 2026-10-21 by the M4a plan (an implementation choice, not one of the nine owner decisions in ROADMAP.md; the owner may revisit): instant; `ages.ron` has no advance time.
5. Tech costs and research times are unset; M4a picks them and the balance issue form asks for a play-bots table with any change. Decided 2026-10-21 (owner accepted the M4a plan defaults): the starting values in `techs.ron` are, for levels 1 / 2 / 3, Arms 75 Lore + 50 Ore / 150 Lore + 100 Ore / 250 Lore + 150 Ore in 300 / 450 / 600 ds; Statecraft 80 / 160 / 260 Lore in 300 / 450 / 600 ds; Trade 70 / 150 / 250 Lore in 300 / 450 / 600 ds; Letters 60 / 120 / 200 Lore in 250 / 400 / 550 ds; all before the Letters rows. The cheapest way into Masonry (Letters I, Trade I and the age's 100 Lore) costs 230 Lore, or 223 when Trade I is queued after Letters I completes; that is flagged for the M5b bot's tick-18000 target. They are tuned in the M4a counters slice and revisited with the M5b and M6 play-bots tables.
6. Which unit StatPaths the Arms bump touches: hit points and damage in the starting data; range and speed are candidates. Decided 2026-10-21 by the M4a plan (an implementation choice, not one of the nine owner decisions in ROADMAP.md; the owner may revisit): hit points and damage only, `UnitHp(Military)` and `UnitDamage(Military)` `Mul 1100` per Arms level, where `Military` is every class but `Civilian`; range and speed stay as they are. Unit hit points stay between 30 and 150 before Arms so M5a's 1-HP attrition pulses matter. The tech slice applied the `PopCap` row; the combat core slice (`SIM_VERSION` 13) applies these two.
