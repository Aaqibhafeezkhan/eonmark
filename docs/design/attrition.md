# Attrition and supply

This document specifies attrition, the mechanic that makes enemy land units wither inside your borders once Harrying is researched, and supply, the Supply Wain radius that cancels it. Attrition is what turns borders from a building rule into a military one: an army that marches in without a Wain loses the war of time. Every number here is a starting value in `data/rules/rules.ron` and `data/rules/techs.ron` and may move during M6 tuning. The sim code is `crates/sim/src/attrition.rs` with its `SupplyGrid`. For the design lineage see [prior-art.md](prior-art.md).

| Field | Value |
| --- | --- |
| Milestone | M5a |
| Labels | `area:sim`, `area:rules`, `kind:balance` for tuning |
| Rules files | `data/rules/rules.ron` (`attrition_interval_ds: 32`, that is 3.2 s = 64 ticks; `supply_radius_tiles: 14`; age-gap table, Charter Age Harrying bonus), `data/rules/techs.ron` (Harrying I and II, hosted by the Watchtower), `data/rules/units.ron` (immunity flags) |
| Sim module | `crates/sim/src/attrition.rs` (attrition pass and `SupplyGrid`) |
| Sub-hash | `units` (HP changes); the supply grid is part of hashed state and rebuilt on `restore()` |

## Target numbers

| Parameter | Start value | Where |
| --- | --- | --- |
| Harrying I | researched at a Watchtower; available in the Masonry Age | `techs.ron` |
| Harrying II | researched at a Watchtower; available in the Charter Age | `techs.ron` |
| Harrying cost and time | set in `techs.ron` at M5a | `techs.ron` |
| Pulse interval at Harrying I | `attrition_interval_ds: 32` (3.2 s = 64 ticks) | `rules.ron` |
| Pulse interval at Harrying II | 32 ticks (rate doubles per level) | derived |
| Pulse damage, age gap 0 | 1 HP | `rules.ron` age-gap table |
| Pulse damage, age gap 1 | placeholder, set at M5a | `rules.ron` age-gap table |
| Pulse damage, age gap 2 | placeholder, good first issue | `rules.ron` age-gap table |
| Supply Wain radius | 14 tiles | `rules.ron` |
| Immune unit kinds | Scribe, Supply Wain | `units.ron` |
| Charter Age Harrying bonus | late-game pressure knob, value set at M6 | `rules.ron` |

Harrying I and II are not part of the 12 Scriptorium techs and do not count toward age gates (see [tech-and-ages.md](tech-and-ages.md)).

### Age-gap table (placeholder)

The gap is `max(0, age(territory owner) - age(intruder))`. The table gives integer HP per pulse so no floor is needed.

| Age gap | HP per pulse | Status |
| --- | --- | --- |
| 0 | 1 | fixed by the acceptance test (both players at the same age) |
| 1 | to be set | M5a |
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

The grid is rebuilt for a player whenever one of that player's Supply Wains moves to a new tile, spawns or dies. A disc of radius 14 covers about 617 tiles, and there are few Wains, so the rebuild is cheap. `restore()` rebuilds every player's grid before returning (see `docs/DETERMINISM.md`). The grid is exported to the A channel of the field texture so the shader and minimap can show coverage (see [territory.md](territory.md)), and the game draws a radius ring when a Wain is selected (M5a).

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

- [ ] `cargo test -p sim attrition`: an enemy unit in researched borders loses exactly 1 HP at ticks 64, 128, 192; with a friendly Supply Wain within 14 tiles it loses 0 HP over 1200 ticks; before Harrying I it loses 0 HP; Harrying II doubles the rate.

Good first issue from M5a:

- [ ] Add an attrition table row for age gap 2 and a test (any OS).

```bash
cargo test -p sim attrition
cargo run -p sim-cli -- data-check data/
```

## Open tuning questions

1. Doubling per level is read as halving the pulse interval (32 ticks at Harrying II). The alternative, 2 HP every 64 ticks, gives the same total at multiples of 64 but coarser pulses.
2. Age gap direction and clamp: this doc uses `max(0, owner age - intruder age)`. Should a more advanced intruder take less than 1 HP per pulse (which integers cannot express), or is the floor at the gap-0 row intended?
3. Counter reset versus pause when a Wain arrives. Reset is simpler and is what the tests assume.
4. Charter Age Harrying bonus default: 0 until M6 play-bots show stalemates, or on from the start?
5. Should Yeomen be exposed? The design immunizes only Scribes and Supply Wains; a raid that bleeds enemy workers may be too strong or exactly right.
6. Harrying I and II cost and research time belong in `techs.ron` and are unset in the design.
