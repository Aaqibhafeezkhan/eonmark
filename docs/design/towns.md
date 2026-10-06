# Towns

This document specifies Towns: how they are founded, how many a player may hold, how their border radius grows, why they cannot be destroyed, the annexation state machine that transfers them, and the victory conditions that depend on them. Every number here is a starting value in `data/rules/rules.ron` and may move during M6 tuning. Founding, limits and radius growth land in M3a (`crates/sim/src/town.rs`); annexation and victory land in M5a. For the design lineage see [prior-art.md](prior-art.md).

| Field | Value |
| --- | --- |
| Milestone | M3a (founding, limit, radius), M5a (annexation, victory) |
| Labels | `area:sim`, `area:rules` |
| Rules files | `data/rules/rules.ron` (town radius, town limit, annexation 60 s, defender radius 6, match cap 45 min), `data/rules/buildings.ron` (Town HP, cost, shooter stats) |
| Sim modules | `crates/sim/src/town.rs`, annexation in `crates/sim/src/attrition.rs` neighbourhood or its own module, `outcome()` in `state.rs` |
| Sub-hash | `buildings`, `territory` |

## Target numbers

| Parameter | Start value | Where |
| --- | --- | --- |
| Town limit | 1 + Statecraft level | `rules.ron`, `techs.ron` row `town_limit Add 1` |
| Border radius, tier 1 | 20 tiles | `rules.ron` |
| Border radius, tier 2 | 24 tiles, once 5 distinct building kinds are attributed to the Town | `rules.ron` |
| Town base yield | 10 Grain and 10 Lumber per 30 s | `buildings.ron` |
| Crofts per Town | 5 | `rules.ron` |
| Town HP and cost | set in `buildings.ron` at M3a | `buildings.ron` |
| Annexation timer | 60 s = 1200 ticks | `rules.ron` |
| Defender exclusion radius | 6 tiles | `rules.ron` |
| Match cap | 45 min = 54000 ticks | `rules.ron` |

| Statecraft level | Town limit |
| --- | --- |
| 0 | 1 |
| 1 | 2 |
| 2 | 3 |
| 3 | 4 |

## Founding

A Town is a building kind. A Yeoman founds one through `Build { unit, kind: Town, tile }`, so it obeys every construction rule: the tile must be inside the founder's borders (`is_buildable`), passable and unoccupied, the player must be under the town limit, and the ramped cost (base times 1.2 for the second copy, see [economy.md](economy.md)) must be affordable. The second Town is therefore placed at the edge of current borders and extends them by its own radius when complete.

| Rejection | Reason |
| --- | --- |
| Outside borders | `OutsideBorders` |
| At the town limit | `TownLimitReached` (proposed name) |
| Cannot pay | `CannotAfford` |

The first Town a player owns at match start is the Seat. The Seat flag never moves to another Town.

## Radius growth

Each non-Town building is attributed to one Town at placement: the nearest own Town by `dist_sq_i64`, `BuildingId` as tiebreak. A Town whose attributed buildings cover 5 distinct kinds (for example Croft, Lumber Yard, Ore Pit, Scriptorium, Muster Hall) moves to tier 2 and its radius grows from 20 to 24. Losing buildings can drop it back; the field is recomputed either way (see [territory.md](territory.md)).

The age list says the Masonry Age "unlocks the second Town growth tier". M3a implements the building-kind condition alone because ages do not exist yet; from M4a the tier-2 radius also requires the Masonry Age. This reading is listed under open questions.

## Towns are indestructible

Town HP can reach 0, but the building is never removed from the map. A Town at 0 HP keeps its border kernel (until annexation starts), keeps yielding its base income, and keeps training unless the annexation timer is running. Whether a Town at 0 HP still shoots is an open question. Every other building kind is destroyed at 0 HP (see [combat.md](combat.md)).

## Annexation state machine

Towns change hands by annexation, not destruction.

```text
enum Hold { Held, Contested { attacker: PlayerId, ticks: u32 } }

each tick, for each Town in BuildingId order:
    if town.hp > 0:                       state = Held; continue
    attackers = enemy units of class infantry or cavalry on tiles adjacent
                to the Town footprint
    defenders = owner's military units (every kind except Yeoman and Scribe)
                within 6 tiles of the Town centre
    if attackers is empty or defenders is non-empty:
        if state was Contested: return the kernel to the owner's layer
        state = Held
        continue
    match state:
        Held       => state = Contested { attacker, ticks: 0 }
                      move the Town's kernel to the neutral layer
        Contested  => ticks += 1
    if ticks >= 1200: flip(town, attacker)
```

Rules the machine encodes:

| Rule | Detail |
| --- | --- |
| Entry | HP is 0, at least one attacker infantry or cavalry unit is adjacent, and no defender military unit is within 6 tiles. |
| Reset | A defender military unit arriving within 6 tiles resets the timer to zero and returns the Town to `Held`. The attacker must re-establish the conditions. Attackers leaving also resets. |
| During the timer | The Town's land is neutral: its kernel is stamped into the neutral pseudo-player layer, so `owner_at` returns `None` where no other kernel wins. `Train` at the Town is rejected. |
| Flip | After 1200 continuous ticks the Town and its attributed civilian buildings transfer to the attacker. Watchtowers attributed to the Town stay with the defender. Civilian means every building kind except Watchtower. |
| Attacker identity | The owner of the adjacent units. With two players this is unambiguous; more players are out of scope for v0.1. |

Infantry and cavalry classes are defined per unit kind in `units.ron`: Skirmisher, Shieldbearer and Bowman are infantry, Outrider is cavalry (see [combat.md](combat.md)). Mangonels and Supply Wains adjacent to a Town do not start the timer.

Attackers standing adjacent for 60 s inside the defender's remaining borders are exposed to attrition unless a Supply Wain is within 14 tiles; at Harrying I that is 18 pulses of 1 HP (see [attrition.md](attrition.md)). The neutral land under the Town itself does not apply attrition.

## Victory and defeat

`Sim::outcome() -> Option<Outcome>` with `Outcome::Victory { player, decisive: bool }`. Once set it never changes.

| Condition | Checked | Result |
| --- | --- | --- |
| A player's Seat is annexed | at flip | `Victory { player: attacker, decisive: true }` |
| A player owns no Towns | each tick | `Victory { player: opponent, decisive: true }` (implied by Seat capture in a 2-player match; kept for generality) |
| `Surrender` command | at apply | `Victory { player: opponent, decisive: true }` |
| Tick 54000 reached with no outcome | at tick 54000 | `Victory { player with more owned tiles, decisive: false }` |

Territory percentage is `owned_tile_count / 16384` from the field. The 45-minute cap makes "no stalemate" structural: every match ends. An exact tile tie at tick 54000 is not covered by the design and is an open question.

The game shows the victory or defeat overlay with the final hash; the recorded replay must verify to it (`--scenario capture_seat`, M5a).

## Pacing targets (M6 fun gate)

These are measured with `sim-cli play-bots`, not enforced by the sim.

| Target | Value |
| --- | --- |
| Target match length | 20 to 40 min (24000 to 48000 ticks) |
| Decisive outcomes (capital capture) before tick 48000 | at least 10 of 20 seeded Standard-vs-Standard games |
| Median game length | at most tick 54000 |
| Games lasting past tick 24000 | at least 10 of 20 |

## Interactions

| System | Interaction |
| --- | --- |
| [territory.md](territory.md) | Towns are the border sources; tier changes, Statecraft and annexation edit the field. |
| [economy.md](economy.md) | Town base yield, Croft limit, ramped Town cost, Yeoman and Scribe training. |
| [tech-and-ages.md](tech-and-ages.md) | Statecraft raises the town limit and the radius; Masonry gates tier 2 from M4a. |
| [combat.md](combat.md) | Towns shoot; infantry and cavalry classes drive annexation; Mangonels bring Town HP to 0. |
| [attrition.md](attrition.md) | Attackers at a Town need a Supply Wain or they bleed through the timer. |
| [ai.md](ai.md) | Defend-Seat rule, second-Town expansion, attack waves with a Supply Wain attached (M5b). |
| [ui.md](ui.md) | Annexation timer UI and the victory/defeat overlay (M5a game half). |

## Acceptance checklist

From M3a:

- [ ] `cargo test -p sim economy`: the second Town costs more than the first by exactly the rules.ron formula (part of the economy test line).
- [ ] Scripted headless economy fixture: from the standard start (1 Town, 5 Yeomen, 150 Grain / 150 Lumber / 0 Ore / 0 Lore) a fixed build order founds a second Town before tick 3600 and the final hash matches the fixture.

From M5a:

- [ ] `cargo test -p sim annexation`: a Town at 0 HP with enemy Skirmishers adjacent and no defender military within 6 tiles flips after 1200 ticks; a defender arriving at tick 600 resets the timer; during the timer `owner_at` returns neutral for its tiles and Train is rejected; capturing the Seat makes `outcome()` return `Victory { decisive: true }`; a 54000-tick scripted stalemate returns `Victory { decisive: false }` for the player with more territory.
- [ ] `cargo run -p game --features dev -- --scenario capture_seat` shows the victory overlay with the final hash and the recorded replay verifies to it.

```bash
cargo test -p sim annexation
cargo run -p game --features dev -- --scenario capture_seat
```

## Open tuning questions

1. Town HP after a flip: the design is silent. Options are keeping 0 HP (the defender can immediately contest it back) or restoring a data-defined fraction. M5a must pick and put the value in `rules.ron`.
2. Does a Town at 0 HP keep shooting? Does Town HP regenerate?
3. Exact territory tie at tick 54000: lowest `PlayerId`, or a `Draw` outcome variant.
4. Does the Masonry Age gate tier-2 radius growth, or does the age list only mean that tier 2 is usually reached in Masonry?
5. Should Muster Halls and Engine Yards transfer on a flip? This doc treats every kind except Watchtower as civilian for transfer purposes.
6. Queued training at a flipped Town: cancelled with or without refund to the old owner.
7. Is 60 s the right timer against a Hard bot that attacks before minute 12? M6 play-bots decide.
