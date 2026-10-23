# Attrition and supply

This document specifies attrition, the mechanic that makes enemy land units wither inside your borders once Harrying is researched, and supply, the Supply Wain radius that cancels it. Attrition is what turns borders from a building rule into a military one: an army that marches in without a Wain loses the war of time. Every number here is a starting value in `data/rules/rules.ron` and `data/rules/techs.ron` and may move during M6 tuning. The sim code is `crates/sim/src/attrition.rs` with its `SupplyGrid`. For the design lineage see [prior-art.md](prior-art.md).

| Field | Value |
| --- | --- |
| Milestone | M5a |
| Labels | `area:sim`, `area:rules`, `kind:balance` for tuning |
| Rules files | `data/rules/rules.ron` (`attrition_interval_ds: 32`, that is 3.2 s = 64 ticks; `supply_radius_tiles: 14`; age-gap table, Charter Age Harrying bonus), `data/rules/techs.ron` (Harrying I and II, hosted by the Watchtower), `data/rules/units.ron` (immunity flags) |
| Sim module | `crates/sim/src/attrition.rs` (attrition pass and `SupplyGrid`) |
| Sub-hash | `units` (each unit's `exposed_ticks` counter and the damage pulses deal); the supply grid is derived inside each step from the Supply Wains' tiles and never stored, so `restore()` has nothing to rebuild (M5a, see "M5a implementation notes") |

## Target numbers

| Parameter | Start value | Where |
| --- | --- | --- |
| Harrying I | researched at a Watchtower; available in the Masonry Age | `techs.ron` |
| Harrying II | researched at a Watchtower; available in the Charter Age | `techs.ron` |
| Harrying cost and time | Harrying I 75 Ore + 100 Lore in 40 s, Harrying II 150 Ore + 200 Lore in 60 s (M5a owner decision 9) | `techs.ron` |
| Pulse interval at Harrying I | `attrition_interval_ds: 32` (3.2 s = 64 ticks) | `rules.ron` |
| Pulse interval at Harrying II | 32 ticks (rate doubles per level) | derived |
| Pulse damage, age gap 0 | 1 HP | `rules.ron` age-gap table |
| Pulse damage, age gap 1 | 2 HP (M5a owner decision 9) | `rules.ron` age-gap table |
| Pulse damage, age gap 2 | placeholder, good first issue | `rules.ron` age-gap table |
| Supply Wain radius | 14 tiles | `rules.ron` |
| Immune unit kinds | Scribe, Supply Wain | `units.ron` |
| Charter Age Harrying bonus | late-game pressure knob: `harrying_bonus_age: Some("charter")`, `harrying_bonus_hp: 0` until M6 | `rules.ron` |
| Attrition's step cost | `sim-cli bench --units 400 --ticks 1200 --combat --harrying`: mean step < 5 ms, and at most 0.1 ms above the plain `--combat` battle's mean (M5a plan, slice 5) | release, measured in `docs/BUILD_TIMES.md` |

Harrying I and II are not part of the 12 Scriptorium techs and do not count toward age gates (see [tech-and-ages.md](tech-and-ages.md)).

### Age-gap table (placeholder)

The gap is `max(0, age(territory owner) - age(intruder))`. The table gives integer HP per pulse so no floor is needed.

| Age gap | HP per pulse | Status |
| --- | --- | --- |
| 0 | 1 | fixed by the acceptance test (both players at the same age) |
| 1 | 2 | M5a (owner decision 9) |
| 2 | to be set | good first issue: add the row and a test |

`sim-cli data-check` requires the table to be non-decreasing in the gap and to have a row for gap 0.

## Algorithm

Each tick, after movement and before combat resolution, the sim runs one pass over units in `UnitId` order.

```text
for unit in units:                                   // UnitId order
    if unit.kind.immune_to_attrition: continue       // Scribe, Supply Wain
    owner = territory.owner_at(unit.tile)
    if owner is None or owner == unit.player:
        unit.exposed_ticks = 0; continue
    level = players[owner].harrying_level             // 0, 1 or 2
    if level == 0:
        unit.exposed_ticks = 0; continue
    if supply[unit.player].covers(unit.tile):
        unit.exposed_ticks = 0; continue

    unit.exposed_ticks += 1
    interval = pulse_interval_ticks >> (level - 1)    // 64 at I, 32 at II
    if unit.exposed_ticks >= interval:
        unit.exposed_ticks = 0
        gap    = min(age(owner) - age(unit.player), last table row), floored at 0
        damage = age_gap_table[gap]
        if age(owner) == Charter: damage += charter_harrying_bonus_hp
        unit.hp -= damage                             // death handled by the common path
```

Properties the tests rely on:

| Property | Consequence |
| --- | --- |
| Per-unit counter starting at exposure | A unit placed in researched enemy borders at tick 0 loses 1 HP at ticks 64, 128 and 192. |
| Counter resets when supplied, friendly, neutral or unresearched | Walking in and out of a Wain's radius never accumulates hidden damage. |
| Interval halves per level | Harrying II delivers exactly twice the HP of Harrying I at any tick that is a multiple of 64. |
| Integer HP per pulse from the table | No permille, no floor, no float. |
| Neutral land is safe | During an annexation timer the Town's own land is neutral (see [towns.md](towns.md)); only the defender's other kernels still bite. |

Yeomen are affected: only Scribes and Supply Wains are immune. All v0.1 units are land units.

## SupplyGrid

```text
SupplyGrid { covered: [player] -> Vec<u8> (128 x 128), generation: u32 }

rebuild(player):
    clear covered[player]
    for wain in player's Supply Wains (UnitId order):
        for tiles with dx*dx + dy*dy <= 14*14 around wain.tile: covered = 1
```

The design above is a raster rebuilt whenever a Wain moves to a new tile, spawns or dies, and rebuilt by `restore()`. M5a builds something simpler instead (see "M5a implementation notes"): each step collects every player's Wain tiles and tests a tile against them directly, so nothing is stored, nothing can go stale and `restore()` has nothing to rebuild. A disc of radius 14 covers 613 tiles (`dx² + dy² <= 196`; an earlier draft said about 617), and there are few Wains, so either way is cheap. Exporting coverage to the A channel of the field texture, so the shader and minimap can show it (see [territory.md](territory.md)), waits for M7 with fog (M5a owner decision 10); the game draws a radius ring when a Wain is selected (M5a).

Supply is per player: a Wain covers only its owner's units. Alliances are out of scope.

## Interactions

| System | Interaction |
| --- | --- |
| [territory.md](territory.md) | `owner_at` decides exposure. Pushing your border over the enemy's staging ground is an attack. |
| [towns.md](towns.md) | Annexation takes 1200 ticks; an unsupplied attacker at Harrying I takes 18 pulses, 18 HP, during the timer. |
| [combat.md](combat.md) | Supply Wain has no attack and is the Outrider's natural prey; Mangonel escorts need a Wain. Attrition deaths use the normal death path. |
| [tech-and-ages.md](tech-and-ages.md) | Harrying I needs the Masonry Age, Harrying II the Charter Age; the age gap reads each player's current age. |
| [ai.md](ai.md) | From M5b every bot attack wave includes 1 Supply Wain; the Standard bot must attack with at least 8 units including a Wain before tick 18000. |
| [ui.md](ui.md) | Attrition indicator on affected units; supply ring on selection (M5a game half). |
| M6 fun gate | The Charter Age Harrying bonus is the designed late-game pressure knob against turtle stalemates. |

## Acceptance checklist (from M5a)

- [x] `cargo test -p sim attrition`: an enemy unit in researched borders loses exactly 1 HP at ticks 64, 128, 192; with a friendly Supply Wain within 14 tiles it loses 0 HP over 1200 ticks; before Harrying I it loses 0 HP; Harrying II doubles the rate.

  Observed on PR #43, 2026-10-23, with `cargo test -p sim --profile ci attrition -- --nocapture` (the `ci` profile inherits `dev`; the tests are the same): `test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 41 filtered out` in m5a and 4 passed in the lib. The unit lost 1 HP at ticks 64, 128 and 192 (`[(64, UnitId(1), 1), (128, UnitId(1), 1), (192, UnitId(1), 1)]`, no `Hit`); a Wain 14 tiles away kept it at 80 HP for 1200 ticks with 0 pulses (one 15 tiles away: 62 HP, 18 pulses); with no Harrying it kept 80 HP (Harrying I researched during tick 600: first pulse at 664); Harrying II lost exactly twice Harrying I's HP at every multiple of 64 (43 against 62 HP after 1200 ticks, intervals 32 and 64). The Harrying bench ("The Harrying bench" below) adds about 0.05 ms a step to the plain combat bench's mean. The good first issues below stay open for contributors.

Good first issues from M5a:

- [ ] Add an attrition table row for age gap 2 and a test (any OS). Filed as #44.
- [ ] `sim-cli data-check --verbose` prints the pulse table (HP per pulse by age gap, with the late bonus) and the Harrying intervals per level (any OS). Filed as #46.

```bash
cargo test -p sim attrition
cargo run -p sim-cli -- data-check data/
```

## M5a implementation notes

The attrition slice (`SIM_VERSION` 15, `rules_version` 19) implements this document in `crates/sim/src/attrition.rs`. Where it departs from the pseudo-code above, the notes say so.

**Step.** Attrition is step 5b' of `Sim::step`: after movement and its repath requests (5b), before `combat::resolve` (5c), on the post-movement positions. It walks the units in `UnitId` order and draws nothing from the RNG.

**Hashed state.** One field, `Unit::exposed_ticks: u32` (sub-hash `units`): the number of consecutive steps the unit has been exposed, with a saturating add (it would saturate after 2^32 - 1 steps, about 6.8 years at 20 Hz, far past the 45-minute cap). Everything else is computed inside the step: the per-player levels, intervals and pulse sizes, and the supply grid.

**Exposure.** A unit is exposed in a step when its kind is not `immune_to_attrition` (the Scribe and the Supply Wain), its tile (`Map::tile_of` of its position) is owned by another player (`TerritoryField::owner_at`; neutral land, the Town under an annexation timer included, is safe), that player's Harrying level is at least 1, and none of the unit's owner's Supply Wains covers the tile. An exposed unit's counter goes up by one; every other unit's counter is set to 0 (open question 3: a reset, not a pause), so walking in and out of cover never accumulates hidden damage. When no player has a Harrying level the step only zeroes the counters and builds nothing.

**Pulse.** A pulse lands when the counter is a multiple of the owner's interval: `damage_taken` grows by the pulse's hit points (clamped to `i32`) and the step emits `SimEvent::Attrition { unit, damage }`. The pseudo-code resets the counter to 0 at each pulse; M5a keeps counting instead, which lands the same pulses while the interval is constant, lets the game read `exposed_ticks > 0` as "bleeding" without a flicker at each pulse, and decides the one case where they differ: a level gained mid-exposure (Harrying II completing) lands the next pulse on the next multiple of the new interval, with no catch-up pulse. With `cmd_delay = 0` and exposure from step 0, the unit's hit points are unchanged through `sim.tick() == 63` and drop at 64, 128 and 192.

**Level and interval.** The level is the `HarryingLevel` rows of the owner's faction and researched techs from base 0 (each Harrying tech is `Add 1`), clamped to `0..=MAX_HARRYING_LEVEL` (8, which data-check keeps the worst case within). Harrying II's row is its own `Add 1`, so level 2 needs both techs. The interval at level `L >= 1` is `max(1, attrition_interval_ticks() >> (L - 1))` with `checked_shr`: 64 ticks at Harrying I and 32 at Harrying II (open question 1: doubling halves the interval).

**Pulse size.** `attrition_age_gap_hp[min(gap, len - 1)]` with `gap = owner age - intruder age` floored at 0, so a more advanced intruder takes the gap-0 row, 1 HP (open question 2: integers cannot go lower), and gap 2 takes the last row until the good first issue adds one; plus `harrying_bonus_hp` once the owner's age index is at least `harrying_bonus_age`'s (open question 4: the Charter Age bonus is 0 until M6). With the shipped data a Masonry owner deals 2 HP per pulse to a Hearth intruder.

**Supply.** A `SupplyGrid` is built once per step that can expose anyone: for each player, the tiles its `supplies` units (the Supply Wain) stand on, in `UnitId` order, and `supply_radius_tiles²`. A tile is covered for player `p` when `(t.x - w.x)² + (t.y - w.y)² <= r²` in `i64` for one of `p`'s Wain tiles `w`: tile to tile, inclusive, 613 tiles at radius 14, and only for `p`'s own units. The validator bounds the radius to `1..=64`.

**Death.** A pulse never removes a unit itself. `combat::reap`, which `resolve` always ends with, removes every unit whose damage has reached the maximum hit points of the `Stats` built at the start of step 5, through the common death path: `UnitDied` with its owner and kind, its path request cancelled, its job and pop gone. A unit that a pulse kills still lands its own hit in 5c, as with simultaneous hits ([combat.md](combat.md)).

**Yeomen** are exposed (open question 5, M5a owner decision 11). **Costs** (open question 6): Harrying I 75 Ore + 100 Lore in 40 s, Harrying II 150 Ore + 200 Lore in 60 s, both at the Watchtower (owner decision 9).

**View.** `SimView::harrying_level(p)`, `supply_radius_tiles()`, `supply_wains(p)` (the disc centres), `supplied(p, tile)`, `attrition_pulse(owner, intruder)` (the step's own `(interval_ticks, hp)`) and `attrition(unit)` (the `Exposure { owner, interval_ticks, hp }` the step would apply to the unit where it stands, built per call for single units). The game reads `exposed_ticks`.

**Tests.** `cargo test -p sim attrition` runs the four acceptance clauses in `crates/sim/tests/m5a.rs` with `cmd_delay = 0` (an enemy Skirmisher 14 tiles east of a P1 Town's centre loses 1 HP at ticks 64, 128 and 192; a P0 Wain 14 tiles away keeps it at 80 HP for 1200 ticks, one 15 tiles away does not; no Harrying, no loss, and Harrying I researched during tick 600 lands its first pulse at 664; Harrying II loses exactly twice as much at every multiple of 64), the supporting tests (the inclusive, per-player radius; resets on own, neutral and supplied tiles; immunities; the age-gap table and the bonus; death through `reap`; no RNG draw; a restore mid-exposure; the view against the step; `DebugDamage`), the lib tests in `attrition.rs` (613 tiles, the grid against a stamped raster, the halving interval, the gap clamp) and the seeded random stream `m5a_attrition_stream_is_deterministic_with_restore` (8 cases locally, 256 in CI, with an in-memory `attrition_interval_ds: 4`).

### The Harrying bench

`cargo run -p sim-cli --release -- bench --units 400 --ticks 1200 --combat --harrying [--seed S]` times the M4a combat bench ([combat.md](combat.md), "The combat bench") with attrition switched on, through `sim::scenarios::combat_bench_harrying(rules, units, seed)` (outside `NAMES`, never a fixture). The stream is `combat_bench`'s, so both armies keep their ids, kinds, places and orders, plus three things. During tick 0 both players `DebugResearch` Harrying I and II (level 2, a pulse every 32 ticks), so the step's fast path never applies, and P1 places a complete Town at nw (21, 12), north of the arena: its footprint is at least 12 tiles from the rows where the groups spawn and fight, so it neither shoots nor is hit (seeds 1 to 10), while its 20-tile border reaches row 33 on the battle line and covers P0's north-east group, so P0's army fights on P1's land. During tick 1 each side `DebugSpawn`s a Supply Wain in the middle of its own groups (P0's is `UnitId(units + 1)`, P1's the next, with `units` rounded down to a multiple of 8: 401 and 402 at 400) and puts it in its attack-move, so P0's army is supplied until its Wain is killed in the battle, and bleeds after. P1's units stand on their own land and never bleed. Only `Sim::step` is timed. The report is the plain bench's with ` harrying` on its first line and one more line before the hash, `attrition_pulses=<Attrition events> supplied=<unit-ticks> exposed=<unit-ticks>`, both unit-tick counts read after each step outside the timed region: `supplied` counts units standing where attrition would bite (not immune, on another player's land whose Harrying level is at least 1) that one of their own Wains covers, and `exposed` counts units whose `exposed_ticks` is above 0. At seed 1 the run prints `attrition_pulses=466 supplied=19951 exposed=15738`, P0's Wain dying during tick 346; the battle stays close to the plain one (`hits=2797 deaths_p0=168 deaths_p1=201` against `hits=2836 deaths_p0=173 deaths_p1=200`; a Wain's death counts). `bench_combat_harrying_small_run_bleeds_and_supplies` runs 40 units for 400 ticks (pulses, supplied and exposed above 0, the Town never destroyed, the same hash twice, a hash unlike the plain run's, the report's lines), `bench_attrition_counts_agree_with_the_view` checks both counts against `SimView::supplied` and `SimView::attrition` unit by unit for 400 ticks, and the sim's `combat_bench_harrying_adds_a_town_both_harrying_levels_and_a_wain_a_side` checks the stream. The measured numbers are in `docs/BUILD_TIMES.md`.

## Open tuning questions

M5a answers all six in "M5a implementation notes" above (1: the interval halves; 2: the gap-0 row is the floor; 3: reset; 4: 0 until M6; 5: Yeomen are exposed; 6: the costs of owner decision 9). They stay listed as the questions the design asked.

1. Doubling per level is read as halving the pulse interval (32 ticks at Harrying II). The alternative, 2 HP every 64 ticks, gives the same total at multiples of 64 but coarser pulses.
2. Age gap direction and clamp: this doc uses `max(0, owner age - intruder age)`. Should a more advanced intruder take less than 1 HP per pulse (which integers cannot express), or is the floor at the gap-0 row intended?
3. Counter reset versus pause when a Wain arrives. Reset is simpler and is what the tests assume.
4. Charter Age Harrying bonus default: 0 until M6 play-bots show stalemates, or on from the start?
5. Should Yeomen be exposed? The design immunizes only Scribes and Supply Wains; a raid that bleeds enemy workers may be too strong or exactly right.
6. Harrying I and II cost and research time belong in `techs.ron` and are unset in the design.
