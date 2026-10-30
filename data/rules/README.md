# data/rules/

This directory is where every gameplay number lives. `rules.ron` holds the
match-wide values, `resources.ron` describes the four resources,
`units.ron` lists the unit kinds and the counter table, `buildings.ron` the
building kinds, `techs.ron` the techs, `ages.ron` the ages and
`factions.ron` the factions (how a kind is drawn is not a gameplay number
and lives one level up, in `../visuals.ron`). All are loaded by
`rules::Rules::load` in that order (`rules.ron`, `resources.ron`,
`units.ron`, `buildings.ron`, `techs.ron`, `ages.ron`, `factions.ron`, each
checked on its own; then the checks that span files, which resolve the
`ages.ron` references first; then `../visuals.ron`, `../maps/` and, from
M5b, the bot data in `../ai/`, see [`../ai/README.md`](../ai/README.md))
and validated before the simulation starts; their content is part of
`rules_hash`, so editing them without bumping `rules_version` makes
`sim-cli verify` report `RULES CHANGED since recording` on older replays.
`../visuals.ron` is the exception: it is validated with the rest but, since
`rules_version` 25, not hashed
([ADR 0003](../../docs/adr/0003-visuals-outside-rules-hash.md)), because
the simulation never reads it.
Changing a number here needs no Rust; see the worked example at the end for
the full data-only path.

## Authoring convention: integers in deciseconds and tiles

The simulation runs at a fixed tick rate (`tick_rate_hz`, 20). Rules are
not authored in ticks because a tick rate change would then rewrite every
file. They are not authored as decimal seconds because the data crate
contains no floating point. The convention is:

| Quantity | Unit | Field suffix | Example |
|----------|------|--------------|---------|
| Time | deciseconds (tenths of a second) | `_ds` | `attrition_interval_ds: 32` is 3.2 s |
| Distance, radius | tiles | `_tiles` | `supply_radius_tiles: 14` |
| Counts, caps, versions | plain integers | none | `pop_cap_base: 25` |
| Multipliers | permille (1000 = x1.0) | `_permille`, `Mul` rows, `permille` | `building_ramp_step_permille: 200` |
| Tick-native values | ticks | `_ticks` | `cmd_delay_ticks: 2` (only this one) |
| Fractional tiles or rates | hundredths | `_x100` | `speed_tiles_per_s_x100: 180` is 1.8 tiles/s |

The single conversion lives in `Rules::ticks_from_ds`:

```text
ticks = round(ds * tick_rate_hz / 10)
```

At 20 Hz: 32 ds = 64 ticks, 600 ds = 1200 ticks, 1 ds = 2 ticks. Rounding is
to nearest with halves rounding up. Pick values whose tick count is whole
when you can; the validator does not require it.

## rules.ron

| Field | Meaning | Value | Used from |
|-------|---------|-------|-----------|
| `rules_version` | Bump whenever golden replays are regenerated, and for any edit to a RON file under `data/rules/` or `data/ai/` except the UI strings in `data/rules/strings/` (those files are in `rules_hash`; the strings are not, see `docs/DATA_FORMAT.md`); the comment above it gives the reason for the current number | 30 | M0 |
| `tick_rate_hz` | Simulation ticks per second | 20 | M0 |
| `cmd_delay_ticks` | A command issued at tick N applies at N + this | 2 | M0 |
| `default_map` | Map for `MatchSetup::skirmish`; must exist under `data/maps/` | `"plains_1v1"` | M0 |
| `yield_cap_base` | Income ceiling per yield-capped resource per 30 s before any `YieldCap` row; the Trade techs in `techs.ron` set it to 100, 150 and 200 | 70 | M3a (M4a: replaces `yield_cap_table`) |
| `pop_cap_base` | Population cap before any `PopCap` row; each Arms level in `techs.ron` adds 25 | 25 | M3a |
| `building_ramp_step_permille` | Building ramp: the `n`-th copy of a kind (0-based, counting the copies the player already owns, sites and the Seat included) costs `floor(base * (1000 + this * n) / 1000)` per resource. 200 makes the second Town 1.2x | 200 | M3a |
| `military_ramp_step_permille` | Military ramp step: the multiplier is `1000 + this * T(k - 1)` permille, where `k = 1 + entries already queued at the player's buildings of the trainer kind` and `T(m) = m * (m + 1) / 2` | 100 | M3a |
| `military_ramp_cap_permille` | Ceiling on the military multiplier (2250 = 2.25x base; with step 100 the 6th queued unit reaches it) | 2250 | M3a |
| `train_queue_max` | Most entries one building's training queue holds | 5 | M3a |
| `idle_seek_delay_ds` | An idle worker looks for a free gather slot on its own after this long (50 = 5 s = 100 ticks) | 50 | M3a |
| `idle_seek_retry_ds` | An idle worker that found no free slot tries again after this long (50 = 100 ticks, equal to the delay) | 50 | M3a |
| `standard_start` | What each player starts with besides the Seat (one Town of the `town: true` kind, centred on the map start): `units` per unit id and `stockpile` per resource id. The sim reads it in `Sim::new` when `MatchSetup.start` is `Standard`, the skirmish default; scenarios start `Empty` unless they set it | `units: {"yeoman": 5}`, `stockpile: {"grain": 150, "lumber": 150, "ore": 0, "lore": 0}` | M3a |
| `town_radius_tiles` | Border radius of a new Town | 20 | M3a |
| `town_radius_grown_tiles` | Radius once the Town has enough distinct building kinds | 24 | M3a |
| `town_growth_distinct_buildings` | Distinct kinds needed to grow | 5 | M3a |
| `town_growth_age` | `Some("<ages.ron id>")`: a Town grows to tier 2 only once its owner has reached this age; `None` for no age gate | `Some("masonry")` | M4a |
| `town_limit_base` | Towns a player may own (sites included) before any `TownLimit` row; the Seat counts. 2 is an owner decision so a second Town can be founded in M3a, before Statecraft exists; each Statecraft level adds 1 (2, 3, 4, 5) | 2 | M3a |
| `damage_variance_permille` | Every hit is scaled by a seeded draw in `1000 - v ..= 1000 + v` permille; 0 makes combat exactly reproducible from stats and draws nothing | 0 | M4a |
| `acquire_range_tiles` | Units acquire enemies within this many tiles, or within their weapon range when that is longer | 6 | M4a |
| `chase_regoal_tiles` | An explicit chase re-goals its path once the target's tile is more than this many tiles (Chebyshev) from the path's goal | 2 | M4a |
| `supply_radius_tiles` | A Supply Wain (a kind with `supplies` in `units.ron`) cancels attrition for its owner's units within this radius; `1..=64` | 14 | M5a |
| `attrition_interval_ds` | Deciseconds between attrition pulses at Harrying I (32 = 3.2 s = 64 ticks); each Harrying level above 1 halves it | 32 | M5a |
| `annexation_ds` | Uncontested time before a Town at 0 HP flips | 600 | M5a |
| `attrition_age_gap_hp` | HP one pulse takes by age gap, `max(0, territory owner's age - intruder's age)`; the last entry covers every larger gap. Non-empty, each entry in `1..=1000`, never decreasing | `[1, 2]` | M5a |
| `harrying_bonus_age` | `Some("<ages.ron id>")`: from this age of the territory owner on, each pulse takes `harrying_bonus_hp` more (the late-game pressure knob); `None` for no bonus | `Some("charter")` | M5a |
| `harrying_bonus_hp` | Extra HP per pulse from `harrying_bonus_age` on; `0..=1000`, 0 until the M6 fun gate | 0 | M5a |
| `annexation_adjacent_tiles` | An attacker counts toward annexing a Town within this many tiles (Chebyshev, from its tile to the footprint); `1..=4` | 1 | M5a |
| `annexation_defender_radius_tiles` | A defender within this many tiles of the Town's centre (centre to centre, inclusive) blocks or resets the annexation; `1..=16` | 6 | M5a |
| `annexation_attackers` | Unit selectors (`Military`, `Class(..)`, `Kind("<unit id>")`, as in "Modifier rows") whose units annex an enemy Town; every `Kind` names a unit, the list reaches at least one kind and no `Civilian` kind | `[Class(Infantry), Class(Cavalry)]` | M5a |
| `annexation_defenders` | Unit selectors whose units defend their own Town; every `Kind` names a unit and the list reaches at least one kind | `[Military]` | M5a |
| `annexation_town_hp_permille` | A flipped Town's HP in permille of its kind's `hp`; `0..=1000` (250 is 500 of 2000) | 250 | M5a |
| `match_cap_ds` | The match ends at this time with the territory tiebreak (27000 = 45 min = 54000 ticks); longer than `annexation_ds` | 27000 | M5a |
| `yield_cap_decay_after_ds` | The late-game Yield Cap decay starts at this match time (18000 = minute 30 = tick 36000); 0 or more (0 starts it at tick 0), and its tick count must fit `u32` | 18000 | M5a |
| `yield_cap_decay_every_ds` | The decay steps once per this long from then on (3000 = 5 min); at least one tick | 3000 | M5a |
| `yield_cap_decay_step` | How much each decay step lowers the Yield Cap, at most `yield_cap_base`; 0 turns the knob off until an M6 fun-gate iteration | 0 | M5a |
| `path_budget_expansions` | A* expansions shared per tick | 4000 | M1 |
| `path_cache_entries` | Bounded path cache size | 1024 | M1 |
| `repath_interval_ds` | Deciseconds between periodic repaths while a unit is moving (30 = 3 s = 60 ticks) | 30 | M1 |
| `separation_push_moving_div` | The separation push on a moving unit is capped at `speed / this` so steering keeps the upper hand | 2 | M1 |
| `separation_push_idle_div` | The separation push on a unit without an order is capped at `speed / this`: parked units yield slowly to a passing crowd instead of jamming it | 8 | M1 |

Of the M3a fields, the Town loop (the first M3a slice) reads
`standard_start`, `building_ramp_step_permille`, the Yield Cap at Trade
level 0, `town_limit_base` and `town_radius_tiles`. Gathering
(`rules_version` 9) adds `idle_seek_delay_ds`, `idle_seek_retry_ds`,
`town_growth_distinct_buildings` and `town_radius_grown_tiles`. Training
(`rules_version` 10) adds `pop_cap_base`, `train_queue_max`,
`military_ramp_step_permille` and `military_ramp_cap_permille`, and the
`standard_start` units count toward the Yeoman's per-unit ramp, so the
first trained Yeoman costs 25 Grain.

M4a's rules data (`rules_version` 13) replaced `yield_cap_table` with
`yield_cap_base` and removed `pop_cap_per_arms_level`: the Trade rows
(`YieldCap Set 100`, `150`, `200`) and the Arms rows (`PopCap Add 25`) in
`techs.ron` are now the only source of the higher values, so a file that
still has either old field fails to load naming it. Since the M4a tech
slice (`rules_version` 14) the sim resolves `yield_cap_base`,
`pop_cap_base`, `town_limit_base` and the Town radii through the
faction's and the researched techs' Modifier rows and reads
`town_growth_age`; `damage_variance_permille`, `acquire_range_tiles` and
`chase_regoal_tiles` are validated now and read from the M4a combat
slices.

The M0 `vision` block (`unit_tiles`, `building_tiles`, `town_tiles`) was
removed at `rules_version` 6: vision is now `vision_tiles` on every kind in
`units.ron` and `buildings.ron`, so an old file that still has the block
fails to load naming `vision`.

Validation: every plain count, radius and timer in the table is > 0
(`rules_version` and `yield_cap_base` included; `yield_cap_decay_after_ds`
may be 0) and `tick_rate_hz` is at most 1000;
`town_radius_grown_tiles >= town_radius_tiles`; `default_map` resolves to a
file; `town_growth_age` is not `Some("")`. The ramp, queue,
idle-seek and combat fields have ranges instead:
`building_ramp_step_permille` and `military_ramp_step_permille` in
`0..=10000`, `military_ramp_cap_permille` in `1000..=100000`,
`train_queue_max` in `1..=255`, `idle_seek_delay_ds` and
`idle_seek_retry_ds` in `1..=36000` (one hour),
`damage_variance_permille` in `0..=500`, `acquire_range_tiles` and
`chase_regoal_tiles` in `1..=16`. The M5a fields have the ranges in the
table above; `attrition_age_gap_hp` errors name the entry
(`attrition_age_gap_hp[1]`); `harrying_bonus_age` is not `Some("")`; every
timer converts to a tick count that fits `u32` (an error, not a panic, at
1000 Hz), `annexation_ds` and `yield_cap_decay_every_ds` to at least one
tick, and `match_cap_ds` to more ticks than `annexation_ds`.

Checks that need the other files run once every file under `rules/` is
loaded, and name `rules.ron` and the field:

- `town_growth_distinct_buildings` is at most the number of non-Town kinds
  in `buildings.ron` (7).
- `town_growth_age` names an age in `ages.ron`, and so does
  `harrying_bonus_age`.
- `annexation_attackers` and `annexation_defenders`: every `Kind` names a
  unit kind in `units.ron` (`annexation_attackers[j]`), each list reaches
  at least one kind, and no attacker selector reaches a `Civilian` kind.
- The worst case of every player-wide stat fits: `yield_cap_base`,
  `pop_cap_base` and `town_limit_base` stay within `u32` with every row of
  that stat in `techs.ron` and the faction, and the larger Town radius
  stays within 64 tiles (`town_radius_grown_tiles` 24 plus the three
  Statecraft rows is 27). The worst case is
  `(|base| + max |Set| + sum |Add|)`, then each `Mul` above 1000 applied and
  rounded up, so it bounds every research order. The `HarryingLevel` rows
  (base 0; the two Harrying techs reach 2) may reach at most 8, an error
  naming `techs.ron`, and `attrition_interval_ds` in ticks, halved once per
  level above 1, must stay at least one tick at the highest level reachable
  (64 ticks allow level 7), an error naming `attrition_interval_ds`.
- `standard_start.units`: every key is a unit id in `units.ron`, every
  count is in `1..=255`, and the units' total `pop` fits `pop_cap_base`.
- `standard_start.stockpile`: lists every resource id in `resources.ron`,
  zeros included, and no other key; each value is at most the resource's
  `stockpile_cap` (999 for Lore) and at most 1000000.

M5a's rules data (`rules_version` 18) added the fields the design named
for it: the age-gap table, the late Harrying bonus, the annexation knobs,
the 45-minute cap and the Yield Cap decay, each with its validator rules.
The attrition slice (`rules_version` 19) reads the attrition fields: the
age-gap table and the late Harrying bonus, besides `supply_radius_tiles`
and `attrition_interval_ds`. The annexation slice (`rules_version` 20)
reads the rest: `annexation_ds`, the annexation knobs and selectors,
`annexation_town_hp_permille`, `match_cap_ds` and the Yield Cap decay
(`docs/design/towns.md` and `docs/design/economy.md`, "M5a implementation
notes"). `stays_on_annexation` in `buildings.ron` is read from the same
slice. M5b's first slice (`rules_version` 21) changed no value here: the
bump re-recorded every fixture for the new replay header field `ai_seats`
and the hashed bot memory (`SIM_VERSION` 17), and `rules_hash` now also
covers `../ai/`. M5b's economy slice (`rules_version` 22) changed only
`../ai/` (the Standard build order and `resource_radius_tiles`); the
re-recorded fixtures differ in their headers only. Its waves slice
(`rules_version` 23) again changed only `../ai/` (the Standard build
order), with the same header-only re-record, and added the
`m5_first_match` golden; its review fixes (`rules_version` 24) added the
`defend_guard_pop_ratio` knob to `../ai/personalities.ron`, again with a
header-only re-record (only `m5_first_match`, new in that pull request,
moved with the bot's code). The art pass's ADR 0003 (`rules_version` 25)
changed no value either: `../visuals.ron` left `rules_hash`, so every
fixture was re-recorded for its header alone and no `.hash` moved; a look
edit needs no bump from then on. M6 changed no value here either:
`rules_version` 26 added `../ai/difficulty/` (the disclosed bot bonus, paid
by step 6d' under `SIM_VERSION` 18), 27 the rush, boom and tower
personalities and the new bot knobs, 28 the tuned Standard build
order and Easy's Yeomen handicap, 29 the retune groundwork (the
difficulty files' `pressure` block and `noise_border_top_k`, all neutral,
and a siege wave's memory of its target's last hit points), 30 the retune
itself (M6 owner decision 12: siege pressure, the all-in times, Easy's
and `tower`'s Statecraft II) and 31 Standard's retreat restored after its
review, all under `../ai/`; in each only `m5_first_match.hash` moved.

## resources.ron

A list of resources in display order. Each entry:

| Field | Meaning |
|-------|---------|
| `id` | Lowercase identifier other files use (`grain`, `lumber`, `ore`, `lore`) |
| `name` | Display name |
| `gathered_from` | Building ids that produce it; non-empty. Cross-checked both ways against `buildings.ron`: as a set it must equal the kinds whose `base_yield` or `gather.resource` names this resource, so a missing id and an extra id both fail with `resources[i].gathered_from` |
| `role` | One sentence on what it pays for |
| `yield_capped` | `true` if income flatlines at the Yield Cap: `yield_cap_base` in `rules.ron`, changed by the `YieldCap` rows of the player's faction and researched techs |
| `stockpile_cap` | `Some(n)` hard ceiling; required when `yield_capped` is `false` |

Lore is the only uncapped resource; it has `stockpile_cap: Some(999)` and is
not tradeable.

## units.ron

A list of unit kinds, then the counter table; the list index is the sim's
`UnitKindId`. M1 shipped one kind, `yeoman`, which stays at index 0
(scenarios and tests spawn `UnitKindId(0)`). M3a added `scribe` and
`skirmisher` and the training fields (`class` to `vision_tiles` below). M4a
adds the other five kinds (`shieldbearer`, `bowman`, `outrider`, `mangonel`,
`supply_wain`), the combat fields `hp`, `attack` and `requires_tech`, and
the `counters` table, for 8 kinds: Yeoman, Scribe and six military kinds.

How the movement fields are used is specified in
`docs/design/pathing.md` ("Movement step"): speed is converted once at
spawn to tiles per tick (`x100 / (100 * tick_rate_hz)`), the radius to
fixed-point tiles; the arrive, separation and return-to-post radii are read
per tick. Of the training fields the sim reads `builder` from the M3a Town
loop (a `Build` by a kind without it is rejected with `NotABuilder`), and
training (`rules_version` 10) reads `cost`, `ramp`, `pop`, `train_time_ds`
and `trainer`. The sim reads `class` since the M4a combat core slice
(`rules_version` 15), through the `UnitSel::Military` and `Class(..)`
selectors of the `UnitHp` and `UnitDamage` Modifier rows (Arms); the
validator also forbids a `Military` ramp on a `Civilian`. `vision_tiles` is
read in M7. The combat fields and the counter table are validated, and the
sim reads `hp`, the `attack` damage, range and cooldown and the counter
table since the M4a combat core slice (`rules_version` 15); `prefer` is
read since the combat behaviour slice (`rules_version` 16), by target
acquisition.

| Field | Meaning |
|-------|---------|
| `id` | Lowercase identifier other files use |
| `name` | Display name |
| `speed_tiles_per_s_x100` | Movement speed in tiles per second, times 100; converted once to tiles per tick at spawn (180 is 1.8 tiles/s, 0.09 tiles/tick at 20 Hz) |
| `radius_tiles_x100` | Collision radius in tiles, times 100; at most 50, because movement's neighbour query grows with the largest radius of any kind |
| `arrive_radius_tiles_x100` | The unit has arrived (order cleared, `UnitArrived` emitted) within this radius of its final waypoint |
| `arrive_slowdown_radius_tiles_x100` | Arrival steering slows the unit linearly inside this radius of its final waypoint; at least the arrive radius |
| `waypoint_radius_tiles_x100` | A non-final waypoint counts as reached within this radius (50 = half a tile) |
| `separation_tiles_x100` | Margin added to the sum of two radii before separation pushes neighbours apart |
| `return_to_post_radius_tiles_x100` | An arrived unit pushed farther than this from the tile centre it arrived at (its post) walks straight back to it and is parked again once within the arrive radius; at least the arrive radius; `100` (one tile) when the field is absent |
| `class` | `Civilian`, `Infantry`, `Cavalry`, `Siege` or `Support`. Civilians gather and build; every other class is military, may use the `Military` ramp and is reached by the `UnitSel::Military` Modifier selector |
| `hp` | Hit points before Arms rows, in `1..=1000000` (M4a) |
| `attack` | `Some((damage, range_tiles_x100, cooldown_ds, prefer))` for a kind that fights, `None` when absent (M4a). `damage` per hit before Arms rows and counters, in `1..=10000`; `range_tiles_x100` measured edge to edge, in `1..=1600`; `cooldown_ds` between hits, in `1..=600`; `prefer: Units` (the default) or `Buildings` (acquire buildings first) |
| `cost` | Base training cost per resource id; zero entries are omitted |
| `ramp` | How the cost grows. `Flat` (the default when absent) is always the base. `PerUnit({...})` adds these amounts per unit of this kind the player has alive or queued. `Military` multiplies the base by the military ramp of the trainer kind's shared queue (`military_ramp_*` in `rules.ron`) |
| `pop` | Population taken from the cap, charged when the unit is queued |
| `train_time_ds` | Training time at the trainer |
| `trainer` | Building id in `buildings.ron` that trains the kind |
| `requires_tech` | `Some("<techs.ron id>")`: training also needs that tech researched, on top of the age that unlocks the kind in `ages.ron`; `None` when absent, as for every kind in the starting data (M4a) |
| `builder` | Whether the kind can construct buildings; `false` when absent |
| `vision_tiles` | Vision radius in tiles (fog of war, M7) |
| `immune_to_attrition` | Whether attrition never damages the kind (M5a); `false` when absent. `true` on the Scribe and the Supply Wain; Yeomen are exposed (ROADMAP M5a Notes, decision 11) |
| `supplies` | Whether the kind covers its owner's units against attrition within `supply_radius_tiles` of `rules.ron` (M5a); `false` when absent. `true` on the Supply Wain only |

Every kind uses `arrive_radius_tiles_x100: 25`,
`arrive_slowdown_radius_tiles_x100: 50`, `waypoint_radius_tiles_x100: 50`,
`separation_tiles_x100: 10` and `return_to_post_radius_tiles_x100: 100`.
The rest:

| Kind (`id`) | Class | HP | Attack (damage, range x100, cooldown ds) | Speed x100 | Radius x100 | Cost | Ramp | Pop | `train_time_ds` | Trainer | Vision |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Yeoman (`yeoman`) | Civilian | 40 | none | 180 | 35 | 20 Grain | `PerUnit({"grain": 1})` | 1 | 120 (12 s) | `town` | 8 |
| Scribe (`scribe`) | Civilian | 30 | none | 180 | 35 | 30 Grain | `Flat` | 1 | 150 | `scriptorium` | 8 |
| Skirmisher (`skirmisher`) | Infantry | 80 | 10, 25, 10 | 220 | 35 | 40 Grain | `Military` | 1 | 150 | `muster_hall` | 8 |
| Shieldbearer (`shieldbearer`) | Infantry | 130 | 12, 25, 12 | 140 | 40 | 20 Grain, 40 Ore | `Military` | 1 | 200 | `muster_hall` | 8 |
| Bowman (`bowman`) | Infantry | 46 | 10, 500, 15 | 180 | 35 | 20 Grain, 40 Lumber | `Military` | 1 | 180 | `muster_hall` | 9 |
| Outrider (`outrider`) | Cavalry | 100 | 12, 25, 10 | 320 | 45 | 30 Grain, 60 Ore | `Military` | 2 | 250 | `muster_hall` | 10 |
| Mangonel (`mangonel`) | Siege | 120 | 30, 800, 40, `prefer: Buildings` | 100 | 50 | 160 Lumber | `Military` | 3 | 400 | `engine_yard` | 8 |
| Supply Wain (`supply_wain`) | Support | 150 | none | 160 | 50 | 100 Lumber | `Military` | 2 | 300 | `engine_yard` | 8 |

Only the Yeoman has `builder: true`. The Bowman's 46 HP is deliberate:
the Skirmisher's counter hit (10 x 2.5 = 25) kills it in two hits at every
Arms level (46 HP grows to 50, 55 and 60, the hit to 27, 30 and 32), and 51
to 73 HP would take three. The stats and counters are starting values that
the seeded counter tests of `docs/design/combat.md` must accept; propose a
change as a hits-to-kill table per Arms level. The M4a combat behaviour
slice ran those tests on them (`cargo test -p sim counters`): each of the
four 10 vs 10 matchups wins 20 of 20 seeds with the values above (the
tables are in `docs/design/combat.md`, "Counter tests"), so none changed.

The costs come from the pure functions in `crates/rules/src/costs.rs`
(`unit_cost`, `building_cost`), which the sim, the HUD and the bot share.
The Yeoman's `n` counts every Yeoman the player has alive or queued, the
five from the standard start included, so the first trained Yeoman costs
20 + 5 = 25 Grain (the M3a reading of `docs/design/economy.md`).

### The counter table

`counters` follows the unit list: `(attacker: "<unit id>", defender:
Kind("<unit id>") | Buildings, permille: n)`. A hit by `attacker` on
`defender` is scaled by `permille`; `Buildings` covers every building kind,
sites included; a pair that is not listed is 1000 (x1.0).

| Attacker | Defender | Permille |
|---|---|---|
| `skirmisher` | `Kind("bowman")` | 2500 |
| `bowman` | `Kind("shieldbearer")` | 3000 |
| `shieldbearer` | `Kind("outrider")` | 3000 |
| `outrider` | `Kind("bowman")` | 2500 |
| `outrider` | `Kind("skirmisher")` | 1500 |
| `outrider` | `Kind("mangonel")` | 3000 |
| `mangonel` | `Buildings` | 4000 |
| `skirmisher`, `shieldbearer`, `bowman`, `outrider` | `Buildings` | 250 each |

Validation: at least one kind, ids unique and lowercase, `speed_tiles_per_s_x100 > 0`,
`radius_tiles_x100` in `1..=50`, `waypoint_radius_tiles_x100 > 0`,
`arrive_slowdown_radius_tiles_x100 >= arrive_radius_tiles_x100`,
`return_to_post_radius_tiles_x100 > 0` and `>= arrive_radius_tiles_x100`, every `_x100`
field at most 100000. Training fields: every `cost` and `PerUnit` key is a
resource id in `resources.ron` with a value in `1..=100000`; `Military` is
rejected on a `Civilian` kind; `pop` is in `0..=100`, `train_time_ds` in
`1..=36000`, `vision_tiles` in `1..=64`. Combat fields: `hp` and the
`attack` ranges above; a counter row's attacker is a kind with an attack,
a `Kind` defender is a kind, each pair is listed once and `permille` is in
`1..=10000`. Errors name the file and `units[i].<field>` or
`counters[j].<field>`. Checked once the other files are loaded, and
reported against `units.ron`:

- `trainer` names a building kind in `buildings.ron` and `requires_tech` a
  tech in `techs.ron`.
- Every attack's `cooldown_ds` is at least one tick at `tick_rate_hz`.
- Every hit does at least 1 damage: for every attacker and every defender
  (each unit kind and `Buildings`), under every faction's rows and at the
  lowest variance draw, `floor(floor(damage * permille / 1000) * (1000 - v)
  / 1000) >= 1`, where `v` is `damage_variance_permille`. The error names
  the counter row when one scales the hit, else `units[i].attack.damage`.
- `hp` and `attack.damage` stay within 1000000 with every `UnitHp` and
  `UnitDamage` row of `techs.ron` and the faction that reaches the kind.

Every kind also needs an entry in `../visuals.ron` and a listing in
exactly one age of `ages.ron`.

## buildings.ron

A list of building kinds, added in M3a; the list index is the sim's
`BuildingKindId`, but code looks kinds up by `id`, never by index. Costs,
yields and rates are integers per resource id; rates are per 30 s (600
ticks at 20 Hz); footprints and radii are in tiles. A building's tile is
the north-west corner of its footprint. The sim reads it from the M3a Town
loop: `town`, the footprint, `cost` with the building ramp,
`build_time_ds`, `hp`, `base_yield`, `max_per_town`, `gather.slots`
(counted at placement) and `border_radius_tiles` (tests reach a complete
Watchtower only through `DebugPlaceBuilding` until a `Build` of one is
tested). Gathering (`rules_version` 9) reads the rest of `gather`:
`resource`, `worker` (which unit kind may take the slots and seeks them
when idle) and `rate_per_worker` (credited per worked slot). `vision_tiles`
is read in M7, `attack` since the M4a combat behaviour slice
(`rules_version` 16: the Watchtower and Town shooters). `hp` is a building's and a site's hit points from
placement; since the M4a combat core slice it takes damage, and every
building or site at 0 HP is destroyed except a complete Town, which stays
at 0.

| Field | Meaning |
|-------|---------|
| `id` | Lowercase identifier other files use (`town`, `croft`, ...) |
| `name` | Display name |
| `town` | `true` for exactly one kind, the Town: the border source every player starts with (the Seat) and founds more of, bounded by `town_limit_base` in `rules.ron`. `false` when absent |
| `width_tiles`, `height_tiles` | Footprint in tiles, x and y |
| `cost` | Base cost per resource id, before the building ramp (`building_ramp_step_permille` in `rules.ron`); zero entries are omitted |
| `build_time_ds` | Construction time with one builder at work |
| `hp` | Hit points of the complete building |
| `vision_tiles` | Vision radius in tiles (fog of war, M7) |
| `base_yield` | Income per resource id per 30 s with no worker, while complete; empty when absent |
| `gather` | `Some((resource, worker, rate_per_worker, slots))` for a gathering building, `None` when absent. `resource` is the resource id the workers produce, `worker` the unit id that works the slots (`yeoman`, `scribe`), `rate_per_worker` the income per worked slot per 30 s |
| `gather.slots` | `Fixed(n)`: always `n` slots. `Adjacent(terrain: Forest \| Mountain, max: n)`: one slot per tile of that terrain sharing an edge or a corner with the footprint, at most `n`, counted when the building is placed; a placement with none is rejected |
| `max_per_town` | `Some(n)`: at most `n` of this kind per Town (each building counts for its nearest own Town), sites included. `None` when absent. The Croft limit lives here |
| `border_radius_tiles` | `Some(r)`: the complete building is a border source of radius `r` (the Watchtower). `None` when absent. The Town's radius is `town_radius_tiles` in `rules.ron` instead |
| `attack` | `Some((damage, range_tiles_x100, cooldown_ds))`: the complete building shoots enemy units, with the same block and ranges as `units.ron`; `prefer` must stay `Units` (or be omitted). `None` when absent (M4a) |
| `stays_on_annexation` | `true`: buildings and sites of the kind stay with their owner when the Town they belong to is annexed, and are re-attributed to another of the owner's Towns (M5a; the Watchtower). Every other kind changes hands with the Town, and one with `border_radius_tiles` takes its border with it (its source moves to the new owner's layer). `false` when absent; never `true` on the Town kind |

| Kind (`id`) | Footprint | Cost | `build_time_ds` | HP | Vision | Base yield per 30 s | Gather (resource, worker, rate, slots) | Other |
|---|---|---|---|---|---|---|---|---|
| Town (`town`) | 3x3 | 100 Lumber | 600 (60 s) | 2000 | 14 | 10 Grain, 10 Lumber | none | `town: true`; `attack` 15 damage, range 700 (7 tiles), cooldown 15 ds |
| Croft (`croft`) | 2x2 | 30 Lumber | 150 | 300 | 6 | none | Grain, Yeoman, 10, `Fixed(1)` | `max_per_town: Some(5)` |
| Lumber Yard (`lumber_yard`) | 2x2 | 50 Lumber | 200 | 400 | 6 | none | Lumber, Yeoman, 10, `Adjacent(terrain: Forest, max: 6)` | |
| Ore Pit (`ore_pit`) | 2x2 | 50 Lumber | 200 | 400 | 6 | none | Ore, Yeoman, 10, `Adjacent(terrain: Mountain, max: 6)` | |
| Scriptorium (`scriptorium`) | 3x3 | 80 Lumber | 300 | 600 | 8 | 10 Lore | Lore, Scribe, 5, `Fixed(3)` | trains the Scribe |
| Muster Hall (`muster_hall`) | 3x3 | 100 Lumber | 400 | 800 | 8 | none | none | trains the Skirmisher, Shieldbearer, Bowman and Outrider |
| Watchtower (`watchtower`) | 1x1 | 40 Ore | 300 | 500 | 12 | none | none | `border_radius_tiles: Some(2)`; `attack` 15 damage, range 700, cooldown 15 ds; `stays_on_annexation: true`; researches Harrying I and II (`techs.ron`) |
| Engine Yard (`engine_yard`) | 3x3 | 150 Lumber | 500 | 900 | 8 | none | none | trains the Mangonel and Supply Wain |

With the 200 permille building ramp the second Town costs 120 Lumber
(100 * 1.2). That plus one 30 Lumber Croft is exactly the 150 starting
Lumber, which the M3a economy fixture relies on, so a change to either cost
or to the ramp step has to keep that sum within the starting Lumber or
update the fixture's build order. A Watchtower radius of 2 claims a 3x3
block around the tower; see `docs/design/territory.md` ("Sources and
kernels").

Validation (`Buildings::validate`, then the checks that span files):

- At least one kind and at most 65535; exactly one kind has `town: true`.
- Ids unique, non-empty, lowercase ascii with underscores; names non-empty.
- `width_tiles` and `height_tiles` in `1..=8`, `build_time_ds` in
  `1..=36000`, `hp` in `1..=1000000`, `vision_tiles` in `1..=64`.
- Every `cost` and `base_yield` key is a resource id in `resources.ron`
  with a value in `1..=100000` (omit zero entries).
- `gather.resource` is a resource id, `gather.worker` a unit id in
  `units.ron`, `rate_per_worker` in `1..=100000`; `Fixed(n)` and
  `Adjacent` `max` in `1..=255`; `Adjacent` `terrain` is `Forest` or
  `Mountain`.
- `max_per_town` in `1..=255` and `border_radius_tiles` in `1..=64`;
  neither is allowed on the Town kind, which the town limit and
  `town_radius_tiles` in `rules.ron` cover instead. Nor is
  `stays_on_annexation: true`: an annexed Town is the building that
  changes hands.
- A border source (the Town, or any kind with `border_radius_tiles`) has
  odd width and height, so its centre is a tile.
- `attack`: `damage` in `1..=10000`, `range_tiles_x100` in `1..=1600`,
  `cooldown_ds` in `1..=600` and at least one tick, `prefer` is `Units`;
  every hit does at least 1 damage at the lowest variance draw.
- Across files: every `resources.ron` `gathered_from` equals the kinds that
  yield or gather that resource (both ways); every unit's `trainer` names a
  kind here; every kind is unlocked by exactly one age in `ages.ron`;
  `town_growth_distinct_buildings` is at most the number of non-Town
  kinds; every start of every map fits the Town footprint centred on it,
  in bounds and all grass (the error names `starts[i]` in the map's file).

Errors name the file and `buildings[i].<field>`, for example
`buildings[1].cost["timber"]`.

## Modifier rows

Every tech and faction effect is a row `(target: StatPath, op: Set | Add |
Mul, value: n)` with an integer `value` (`crates/rules/src/modifiers.rs`).

| `target` | Base it changes |
|---|---|
| `YieldCap` | `yield_cap_base` |
| `PopCap` | `pop_cap_base` |
| `TownLimit` | `town_limit_base` |
| `TownBorderRadius` | `town_radius_tiles` or `town_radius_grown_tiles` by the Town's tier |
| `ResearchCost` | every resource of a tech's `cost` |
| `ResearchTime` | a tech's `research_time_ds`, before the tick conversion |
| `UnitHp(sel)`, `UnitDamage(sel)` | a unit kind's `hp` or `attack.damage`, for the kinds `sel` reaches: `Military` (every class but `Civilian`), `Class(Infantry)` (one class) or `Kind("bowman")` (one kind) |
| `HarryingLevel` | the attrition level a player's borders impose on intruders (M5a): base 0, each Harrying tech `Add 1`, at most 8 in the worst case |

`Set` and `Add` values are amounts in the stat's own unit, within
`±1000000`. `Mul` values are permille in `1..=100000` (`900` is x0.9). One
stat resolves as: the last `Set` replaces the base, then every `Add` is
summed, then each `Mul` row is applied in order and floored,
`v = floor(v * m / 1000)`. Rows are collected in a fixed order: the
faction's rows, then each researched tech in `techs.ron` order with its
rows in file order. So Letters I, II and III take 100 to 90, 81 and 72.
A name that is not in the enum, such as `target: ResearchSpeed`, fails to
parse with the file, the span and the enum (`StatPath` or `UnitSel`); a
float such as `value: 1.25` fails to parse with the file and the span.

## techs.ron

A list of techs; the list index is the sim's `TechId`. M4a ships the 12
Scriptorium techs, four lines of three levels, each line's levels in order.
M5a appends the Watchtower's Harrying I and II (`TechId` 12 and 13), side
techs that count toward no age gate.

| Field | Meaning |
|-------|---------|
| `id` | Lowercase identifier other files use; digits allowed (`arms_1`) |
| `name` | Display name |
| `line` | The line the tech belongs to (`arms`, `statecraft`, `trade`, `letters`) |
| `level` | Level within the line, from 1; level k requires level k-1 of the same line researched |
| `host` | Building id in `buildings.ron` that researches it |
| `cost` | Cost per resource id before the `ResearchCost` rows; zero entries are omitted |
| `research_time_ds` | Research time before the `ResearchTime` rows |
| `modifiers` | The rows researching it adds |
| `counts_toward_age` | Whether the tech counts toward an age's `techs_required` (M5a); `true` when absent, `false` for the Harrying side techs |

| Line | Rows per level | Cost, levels 1 / 2 / 3 | `research_time_ds` |
|---|---|---|---|
| `arms` | `PopCap Add 25`, `UnitHp(Military) Mul 1100`, `UnitDamage(Military) Mul 1100` | 75 Lore + 50 Ore / 150 + 100 / 250 + 150 | 300 / 450 / 600 |
| `statecraft` | `TownLimit Add 1`, `TownBorderRadius Add 1` | 80 Lore / 160 / 260 | 300 / 450 / 600 |
| `trade` | `YieldCap Set 100` / `Set 150` / `Set 200` | 70 Lore / 150 / 250 | 300 / 450 / 600 |
| `letters` | `ResearchCost Mul 900`, `ResearchTime Mul 900` | 60 Lore / 120 / 200 | 250 / 400 / 550 |
| `harrying` (M5a, levels 1 and 2 only) | `HarryingLevel Add 1` | 75 Ore + 100 Lore / 150 Ore + 200 Lore | 400 / 600 |

Every host is `scriptorium` except the Harrying line's, `watchtower`, and
only the Harrying techs set `counts_toward_age: false`. The file order is
arms 1 to 3, statecraft, trade, letters, then harrying 1 and 2, so
`letters_3` is `TechId` 11 and `harrying_2` is 13. Arms I at Letters III costs
75 x 0.9 x 0.9 x 0.9, floored after each row: 67, 60, 54 Lore. The sim
reads this file from the M4a tech slice.

Validation, naming `techs[i].<field>`: ids unique and lowercase; `line`
lowercase; `level >= 1`; each `(line, level)` pair once; `host` a building
id; `cost` keys resource ids with values in `1..=100000` and, for a capped
resource, at most its `stockpile_cap` (999 Lore), so it can be paid;
`research_time_ds` in `1..=36000`; every row as in "Modifier rows", a
`Kind` selector naming a unit kind. Once `ages.ron` resolves: level k > 1
of a line follows level k-1 earlier in the file; every research lasts at
least one tick with every `ResearchTime` row applied; the worst case of
each cost stays within `u32`, and of each `research_time_ds` within
`u32::MAX / 100` (42949672 ds), so its tick count fits `u32` at up to
1000 Hz.

## ages.ron

A list of ages; the list index is the age a player is in, and every player
starts in `ages[0]`.

| Field | Meaning |
|-------|---------|
| `id` | Lowercase identifier (`hearth`, `masonry`, `charter`); `town_growth_age` in `rules.ron` names one |
| `name` | Display name |
| `techs_required` | Researched techs needed to advance into the age (queued ones do not count, nor, from M5a, the side techs with `counts_toward_age: false`) |
| `cost` | Cost of advancing into the age per resource id; empty when absent |
| `unlocks` | `(units: [...], buildings: [...], techs: [...])`: the ids that become available in the age; each list is empty when absent |

| Age (`id`) | `techs_required` | Cost | Unlocks |
|---|---|---|---|
| Hearth Age (`hearth`) | 0 | none | units `yeoman`, `scribe`, `skirmisher`, `shieldbearer`, `bowman`; buildings `town`, `croft`, `lumber_yard`, `ore_pit`, `scriptorium`, `muster_hall`, `watchtower`; techs `arms_1`, `statecraft_1`, `trade_1`, `letters_1` |
| Masonry Age (`masonry`) | 2 | 250 Grain, 100 Lore | units `outrider`, `mangonel`, `supply_wain`; building `engine_yard`; techs `arms_2`, `statecraft_2`, `trade_2`, `letters_2`, `harrying_1` |
| Charter Age (`charter`) | 5 | 500 Grain, 300 Lumber, 300 Lore | techs `arms_3`, `statecraft_3`, `trade_3`, `letters_3`, `harrying_2` |

Level k of every Scriptorium line is unlocked in age k; the Watchtower's
Harrying line starts in the Masonry Age (M5a). The cheapest route to Masonry
is Letters I (60 Lore), then Trade I at the Letters price (63), plus the
age's 100 Lore: 223 Lore in all (230 if Trade I is queued before Letters I
completes). The sim
reads this file from the M4a tech slice.

Validation, naming `ages[i].<field>`: 1 to 255 ages; ids unique and
lowercase; the first age needs 0 techs and costs nothing;
`techs_required` and every resource of `cost` (a missing one counts as 0)
never decrease from one age to the next; `cost` keys are resource ids with
values in `1..=100000` and at most the stockpile cap of a capped resource.
Once the other files are loaded, in this order: every unlock id names a
unit kind, building kind or tech (`ages[i].unlocks.techs[j]`), so removing
a tech from `techs.ron` that an age lists fails here, naming `ages.ron`
and that entry; every unit kind, building kind and tech is unlocked by
exactly one age; nothing is unlocked before what it needs (a tech after
its host and the level below it, a unit after its trainer and its
`requires_tech`); and an age's `techs_required` can be met with the techs
the ages before it unlock, counting only those that count toward an age.

## factions.ron

A list of factions. A faction's `modifiers` are rows in the `techs.ron`
shape that apply to its players from the start, before any tech's. M4a
ships one faction, `(id: "freeholders", name: "Freeholders", modifiers:
[])`; every player uses the first faction until the match setup chooses
one (M9). A second faction is data only, for example a Yield Cap of 87 at
Trade level 0 is `(target: YieldCap, op: Mul, value: 1250)`.

Validation, naming `factions[i].<field>`: 1 to 255 factions, ids unique
and lowercase, names non-empty, every row as in "Modifier rows". Each
faction's rows also count in the worst-case and minimum-hit checks above.

## Worked example: a data-only change (slower attrition)

To make attrition tick every 4 seconds instead of 3.2, edit `rules.ron`:

```text
attrition_interval_ds: 40,
```

Any edit to a hashed RON file (every one but `../visuals.ron` and
`strings/`) changes `rules_hash`, so without the next steps every golden
fixture reports `RULES CHANGED since recording` and `cargo test -p sim`
fails. The data-only path is the same for every hashed RON file and
touches no Rust:

1. Edit the value, as above.
2. Bump `rules_version` in `rules.ron` (28 to 29), update the one-line
   reason in the comment above it and put the same reason in the commit
   message.
3. Check the data, re-record the golden fixtures and run the sim tests:

```bash
cargo run -p sim-cli -- data-check data
just rerecord
cargo test -p sim
```

`data-check` prints
`OK rules_version=29 rules_hash=0x<16 hex> resources=4 units=8 buildings=8 ages=3 techs=12 side_techs=2 factions=1 personalities=4 build_orders=4 difficulties=3 maps=plains_1v1 strings=en visuals=units:8 buildings:8 terrain:3 models=36`
(the visuals counts follow `../visuals.ron`; the repository is at 28, so before the edit it prints `rules_version=28`;
the hash is new because a file changed).
`just rerecord` runs `scripts/rerecord_fixtures.sh`: it re-records every
fixture under `crates/sim/tests/fixtures/` in release, writes each `.hash`
from an independent `sim-cli verify` OK line, then fails if a fixture
differs from the merge base with main while the working-tree
`rules_version` is not larger than the base's (CI runs the same check,
`scripts/check_fixture_policy.sh`, on the commits of every pull request).
The sim reads `attrition_interval_ds` since M5a's attrition slice, and
one golden fixture researches Harrying: in `capture_seat` P1 has Harrying
I and P0's Bowman bleeds on its land until the annexation timer turns it
neutral, so its pulses land every 80 ticks instead of 64 and
`capture_seat.hash` changes. Every other fixture has no Harrying, so
attrition never bites in them: their `.hash` files stay the same and only
the replay headers change. `cargo test -p sim` passes too, because the
attrition tests read the interval from `Rules` (64 ticks at 32 ds, 80 at
40) instead of hard-coding it, and the annexation and outcome tests read
`annexation_ds`, the defender radius, the flipped Town's hit points and
the match cap the same way. `cargo test -p rules` passes but for one
test, `loads_repo_data`, which pins the shipped values on purpose
(`attrition_interval_ticks() == 64` among them): it is the one test a
data edit must update, in the same commit, to the new value (80). A
change that a fixture does exercise, such as a unit's speed, changes its
`.hash` too, and the bump in the same commit is what makes that legal. `just rerecord move_500 group_spiral` re-records
only the fixtures named.

A typo such as `attrition_interval_s` fails `data-check` with the file
path, the span and the field:

```text
error: data/rules/rules.ron: 63:3-63:23: Unexpected field named `attrition_interval_s` in `Rules`, expected one of `rules_version`, ... instead
```
