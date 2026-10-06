# Combat

This document specifies combat: the eight unit kinds and their roles, the counter triangle, the Watchtower and Town shooters, deterministic target acquisition, the seeded fixtures the counter tests run on, and a summary of movement. Combat in Eonmark is meant to be readable: a small roster, hard counters, no random damage by default. Stat values (hit points, damage, range, speed, cooldown, cost, pop) are not fixed by the design; they are set in `data/rules/units.ron` and `data/rules/buildings.ron` at M4a and must satisfy the seeded counter tests below. For the design lineage see [prior-art.md](prior-art.md).

| Field | Value |
| --- | --- |
| Milestone | M4a (sim), M4b (health bars, unit looks, attack-move key) |
| Labels | `area:sim`, `area:rules`, `kind:balance` for multiplier tweaks |
| Rules files | `data/rules/units.ron` (8 kinds, classes, stats, trainer, counter table in permille), `data/rules/buildings.ron` (Watchtower and Town shooter stats, HP), `data/rules/rules.ron` (`damage_variance_permille`, acquisition settings) |
| Sim modules | `crates/sim/src/combat.rs`, `crates/sim/src/movement.rs` |
| Sub-hash | `units`, `buildings`, `rng` |

## Unit kinds

| Kind | Class | Age | Trainer | Role | Costs mainly | Pop |
| --- | --- | --- | --- | --- | --- | --- |
| Yeoman | civilian | Hearth | Town | Gathers Grain, Lumber, Ore; builds everything | 20 Grain, +1 per Yeoman | `units.ron` |
| Scribe | civilian | Hearth | Scriptorium | Gathers Lore (+5 per 30 s each, 3 per Scriptorium); immune to attrition | 30 Grain | 1 |
| Skirmisher | infantry | Hearth | Muster Hall | Light infantry; beats Bowmen | Grain | `units.ron` |
| Shieldbearer | infantry | Hearth | Muster Hall | Heavy infantry; beats Outriders | Ore | `units.ron` |
| Bowman | infantry (ranged) | Hearth | Muster Hall | Ranged; beats Shieldbearers | Lumber | `units.ron` |
| Outrider | cavalry | Masonry | Muster Hall | Fast raider; beats Bowmen, Skirmishers, Mangonels | Ore | `units.ron` |
| Mangonel | siege | Masonry | Engine Yard | Beats buildings; brings Towns to 0 HP | Lumber | `units.ron` |
| Supply Wain | support | Masonry | Engine Yard | No attack; cancels attrition within 14 tiles; immune to attrition | Lumber | `units.ron` |

Classes drive other systems: infantry and cavalry adjacent to a Town at 0 HP start annexation, and every non-civilian kind counts as "defender military" (see [towns.md](towns.md)). Trainers for Scribe and Outrider are the M4a starting data and are open questions. The 8 kinds and 8 building kinds are counted by `sim-cli data-check`.

## Counter triangle

Damage from attacker to defender is scaled by a permille multiplier from the counter table in `units.ron`. Entries not listed are 1000 (x1.0). The design fixes which pairs are counters, not the multiplier values.

| Attacker | Bonus against |
| --- | --- |
| Skirmisher | Bowman |
| Bowman | Shieldbearer |
| Shieldbearer | Outrider |
| Outrider | Bowman, Skirmisher, Mangonel |
| Mangonel | buildings |

The multiplier values must make each 10 vs 10 matchup below win at least 18 of 20 seeds. Proposing tweaks with a `cargo test -p sim counters` table is a good first issue.

## Damage

```text
raw      = attacker.damage (after Arms Mul 1100 per level, see tech-and-ages.md)
counter  = counter_table[attacker.kind][defender.kind]        // permille, default 1000
variance = 1000 + rng.range(-damage_variance_permille ..= damage_variance_permille)
damage   = floor(floor(raw * counter / 1000) * variance / 1000)
defender.hp -= damage
```

`damage_variance_permille` defaults to 0 in `rules.ron`, so combat is exactly reproducible from stats alone and stays readable. Setting it above 0 draws from the single `Pcg32` in the sim, which is hashed, so replays still verify. Hits land immediately in the sim; projectiles are render-side only. Cooldowns are authored in deciseconds (`_ds` fields) and converted once by `Rules::ticks_from_ds`.

## Shooters

Watchtowers and Towns attack on their own. Their range, damage and cooldown are in `buildings.ron`.

| Shooter | Notes |
| --- | --- |
| Watchtower | Outranges Skirmishers (acceptance test). Also a border source of radius 2 and the host of Harrying research. Destroyed at 0 HP. Costs Ore. |
| Town | Shoots back. Indestructible: at 0 HP it stays and becomes annexable (see [towns.md](towns.md)). |

## Buildings in combat

Every building has HP. A building at 0 HP is removed, except Towns. Removal frees its footprint in the cost grid, which bumps `cost_grid_generation` and clears the path cache (see [pathing.md](pathing.md)); a removed Watchtower is also removed from the territory field. Mangonels carry the building counter and are the intended answer to Watchtowers and Towns.

## Target acquisition

Units acquire targets deterministically.

```text
each tick, for each unit that is idle, attack-moving or holding a dead target (UnitId order):
    candidates = enemy units and buildings within acquisition range,
                 found through the spatial grid (2 x 2 tile cells)
    sort candidates by (dist_sq_i64, id)
    tied = candidates sharing the smallest dist_sq_i64
    target = if tied.len() == 1 { tied[0] } else { tied[rng.range(0..tied.len())] }
```

The seeded tie-break is the only randomness in default combat. A unit keeps its target until the target dies, leaves range or the unit receives a new order. `Attack { units, target }` chases a specific target; `AttackMove { units, target }` moves toward a point and engages anything acquired on the way, then resumes. `Stop` clears the target. The attack-move key is `A` in the game (M4b).

## Fixtures and the 20-seed tests

Counter tests spawn 10 vs 10 of two kinds on an open field with a seeded +/- 1 tile spawn jitter per unit. The seed perturbs jitter and acquisition tie-breaks, so results differ across seeds, and the test asserts that they do. Each matchup runs 20 seeds.

| Matchup | Expectation |
| --- | --- |
| 10 Skirmishers vs 10 Bowmen | Skirmishers win at least 18 of 20, with at least 4 survivors |
| 10 Bowmen vs 10 Shieldbearers | Bowmen win at least 18 of 20 |
| 10 Shieldbearers vs 10 Outriders | Shieldbearers win at least 18 of 20 |
| 10 Outriders vs 10 Bowmen | Outriders win at least 18 of 20 |
| Watchtower vs Skirmishers | the Watchtower outranges them |

## Performance budget

| Bench | Budget |
| --- | --- |
| `sim-cli bench --units 400 --ticks 1200 --combat` (200 vs 200 fighting) | mean step < 5 ms in release on the dev Mac |
| `--scenario enemy_outpost` with 200 vs 200 (M4b) | at least 60 fps; health bars are separate entities, never children of units |

Numbers are appended to `docs/BUILD_TIMES.md` when the bench passes.

## Movement summary

Movement is specified in [pathing.md](pathing.md); the parts combat depends on:

| Step | Rule |
| --- | --- |
| Pathing | Budgeted in-house A* on the 128 x 128 cost grid; repath every 60 ticks while moving or when the next waypoint becomes blocked. |
| Arrival steering | Fixed-point steering toward the next waypoint with slow-down at the goal; no trig, direction vectors only. |
| Separation | Push away from neighbours found in the 2 x 2 tile spatial grid, rebuilt each tick. |
| Circle correction | Circle-vs-circle overlap resolved in `UnitId` order so the result is independent of iteration luck. |
| Group orders | Square-spiral offsets assigned in `UnitId` order, skipping blocked tiles and tiles outside the click's connected component. |

Units without a path yet wait in place; nothing in combat depends on wall-clock time.

## Interactions

| System | Interaction |
| --- | --- |
| [economy.md](economy.md) | Military ramp is triangular per training-building class; Ore gates Shieldbearers, Outriders and Watchtowers; pop cap bounds army size. |
| [tech-and-ages.md](tech-and-ages.md) | Arms gives +10% stats per level; Masonry unlocks Outrider, Mangonel, Supply Wain and the Engine Yard. |
| [attrition.md](attrition.md) | Unsupplied armies bleed inside enemy borders; Supply Wains and Scribes are immune. |
| [towns.md](towns.md) | Towns shoot and cannot be destroyed; infantry and cavalry annex them. |
| [territory.md](territory.md) | Watchtowers are border sources. |
| [ai.md](ai.md) | Influence maps (4 x 4 tile cells, every 10 ticks) and counter-weighted composition arrive in M6; retreat-when-losing. |
| [fog.md](fog.md) | From M7 acquisition respects the visibility grid through `SimView`. |
| [ui.md](ui.md) | Health bars, five primitive silhouettes, attack-move key, selection panel (M4b). |

## Acceptance checklist (from M4a)

- [ ] `cargo test -p sim counters` over 20 seeds each (seed perturbs spawn jitter and acquisition tie-breaks, so results differ across seeds and the test asserts they do): 10 Skirmishers vs 10 Bowmen -> Skirmishers win >= 18/20 with >= 4 survivors; 10 Bowmen vs 10 Shieldbearers -> Bowmen >= 18/20; 10 Shieldbearers vs 10 Outriders -> Shieldbearers >= 18/20; 10 Outriders vs 10 Bowmen -> Outriders >= 18/20; a Watchtower outranges Skirmishers.
- [ ] `cargo run -p sim-cli --release -- bench --units 400 --ticks 1200 --combat` keeps mean step < 5 ms with 200 vs 200 fighting.

Good first issue from M4a:

- [ ] Propose counter multiplier tweaks with a `cargo test -p sim counters` table (any OS).

```bash
cargo test -p sim counters
cargo run -p sim-cli --release -- bench --units 400 --ticks 1200 --combat
```

## Open tuning questions

1. Counter multiplier values: the design fixes the pairs and the 18/20 bar, not the numbers.
2. Trainer for Outrider (Muster Hall or Engine Yard) and for Scribe (Scriptorium or Town).
3. Is Bowman an infantry class for annexation purposes? This doc says yes.
4. Acquisition range versus weapon range: equal in the starting data, or a slightly larger acquisition range so units close in.
5. Chase leash for `Attack` on a fleeing target.
6. Whether `damage_variance_permille` ever leaves 0 in a shipped ruleset.
