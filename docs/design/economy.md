# Economy

This document specifies Eonmark's economy: the four resources, where they come from, the Yield Cap that flattens income, the integer micro-unit income algorithm, the three ramping-cost rules, the population cap, gather slots, idle-worker seeking and the standard start. Every number here is a starting value that lives in `data/rules/*.ron` and is expected to move during the M6 tuning iterations. The sim code that implements this is `crates/sim/src/economy.rs`, with gather slots and idle-worker seeking in `crates/sim/src/gather.rs` and training, unit costs and pop in `crates/sim/src/training.rs`; the pure cost functions live in `crates/rules`. For the design lineage see [prior-art.md](prior-art.md).

| Field | Value |
| --- | --- |
| Milestone | M3a (sim), M3b (HUD readout) |
| Labels | `area:sim`, `area:rules`, `kind:balance` for tuning |
| Rules files | `data/rules/rules.ron` (Yield Cap base `yield_cap_base`, pop cap base `pop_cap_base`, building and military ramp params, `train_queue_max`, `idle_seek_delay_ds`, `idle_seek_retry_ds`, `standard_start`, late-game knobs), `data/rules/resources.ron`, `data/rules/buildings.ron` (costs, base yields, gather rates, slots, Croft limit `max_per_town`), `data/rules/units.ron` (every unit kind's cost, ramp, pop, training time, trainer), `data/rules/techs.ron` (the Trade and Arms rows that raise the Yield Cap and the pop cap, M4a) |
| Sim module | `crates/sim/src/economy.rs`, `crates/sim/src/gather.rs`, `crates/sim/src/training.rs` |
| Sub-hash | `economy` |

## Resources

| Resource | Gathered from | Role | Capped by Yield Cap |
| --- | --- | --- | --- |
| Grain | Crofts (max 5 per Town, 1 Yeoman each, 10 per 30 s). Each Town also yields 10 Grain per 30 s by itself. | Primary unit cost (Yeoman 20 Grain, +1 per Yeoman), age advance cost | Yes |
| Lumber | Lumber Yards beside forest tiles (slots = adjacent forest tiles, max 6, 10 per Yeoman per 30 s). Each Town also yields 10 Lumber per 30 s. | Building cost; Bowman, Mangonel and Supply Wain cost | Yes |
| Ore | Ore Pits beside mountain tiles (slots = adjacent mountain tiles, max 6, 10 per Yeoman per 30 s) | Shieldbearer, Outrider, Watchtower, Arms-line techs. The scarce military resource. | Yes |
| Lore | Scriptorium: 10 per 30 s base plus 5 per Scribe (max 3 Scribes per Scriptorium). Scribes cost 30 Grain and 1 pop. | Research and age-advance currency. Not tradeable. | No. Own stockpile cap of 999. |

Lore exists so the Letters tech line and the Scriptorium have a purpose. Whether to keep a fourth resource is an open question for the owner (see `docs/ROADMAP.md`); the sim treats the resource list as data.

## Gather slots and rates

Rates are authored per 30 seconds in `data/rules/buildings.ron`. At 20 Hz, 30 s is 600 ticks.

| Building | Resource | Slots | Rate per worker per 30 s | Base rate per 30 s with no worker |
| --- | --- | --- | --- | --- |
| Town | Grain, Lumber | none | n/a | 10 Grain, 10 Lumber |
| Croft | Grain | 1 Yeoman; at most 5 Crofts per Town (`max_per_town`) | 10 | 0 |
| Lumber Yard | Lumber | one per adjacent forest tile, max 6 | 10 | 0 |
| Ore Pit | Ore | one per adjacent mountain tile, max 6 | 10 | 0 |
| Scriptorium | Lore | 3 Scribes | 5 | 10 |

Slot counts for Lumber Yards and Ore Pits are fixed at placement from the tiles adjacent to the building footprint. Adjacency means sharing an edge or a corner with the footprint (8-neighbourhood); footprint sizes are in `buildings.ron`. This adjacency definition is the M3a reading of the design and is listed under open questions.

## Yield Cap

The Yield Cap is a per-resource ceiling on income rate, not on the stockpile. Income above the cap is discarded and the HUD readout for that resource turns amber (M3b). The cap is raised by the Trade tech line through `yield_cap Set` Modifier rows (see [tech-and-ages.md](tech-and-ages.md)). Both are data: the level-0 cap is `yield_cap_base: 70` in `rules.ron`, and the rows are `(target: YieldCap, op: Set, value: 100)`, `150` and `200` in `techs.ron` (M4a; the sim resolves them since the M4a tech slice, `SIM_VERSION` 12).

| Trade level | Yield Cap per resource per 30 s |
| --- | --- |
| 0 | 70 |
| 1 | 100 |
| 2 | 150 |
| 3 | 200 |

Lore is exempt from the Yield Cap. Instead its stockpile stops at 999; Lore income beyond that is discarded. Grain, Lumber and Ore have no stockpile cap in v0.1.

A faction Modifier can scale the cap: `yield_cap Mul 1250` gives 70 * 1250 / 1000 = 87 at Trade level 0 (floor). This is the M9 Tidewater League example and the M4a acceptance test.

An optional late-game pressure knob in `rules.ron` can decay the Yield Cap after minute 30. It exists for the M6 fun gate and is off by default. Since the M5a annexation slice (`SIM_VERSION` 16) the sim implements it in `tech::yield_cap`: from `yield_cap_decay_after_ds` (18000 ds, tick 36000) on, the resolved cap (with the Trade rows) loses `yield_cap_decay_step * (1 + (tick - after) / every)`, `every` being `yield_cap_decay_every_ds` in ticks (3000 ds, 6000 ticks), floored at 0. It reads the sim's tick, so step 6d and `SimView::income` agree. The shipped `yield_cap_decay_step` is 0, so the cap never moves until a fun-gate iteration sets it (`late_game_yield_cap_decay_is_off_in_the_shipped_data_and_lowers_the_cap_when_set`); the validator keeps the step at most `yield_cap_base`.

## Income algorithm (integer micro-units)

Income is exact by construction. Per (player, resource) the sim keeps an `i64` accumulator in micro-units of 1/600 resource. Each tick:

```text
rate = sum of per-30 s rates from every worked slot and every Town base yield
acc[player][res] += min(rate, yield_cap[player][res])      // Lore: += rate, no cap
stockpile[player][res] += acc / 600
acc %= 600
stockpile[player][Lore] = min(stockpile[player][Lore], 999)
```

Worked example, one Yeoman on a Croft, no Town income counted:

| Tick | acc after step | Stockpile delta |
| --- | --- | --- |
| 1 | 10 | 0 |
| 59 | 590 | 0 |
| 60 | 0 | +1 |
| 600 | 0 | +10 total |

10 per 30 s is exactly 10 per 600 ticks. There is no float, no epsilon and no drift. This is why the design rejected fixed-point accumulators: 10/600 is not representable in I32F32, and 600 truncated adds yield 9.999.

Cap example: 10 worked Crofts plus a Town give a Grain rate of 110 per 30 s. The accumulator adds min(110, 70) = 70 per tick, so the stockpile grows by exactly 70 per 600 ticks. The acceptance test reads the Croft rate from data, so changing it in `buildings.ron` keeps the test valid.

## Ramping costs

Everything gets pricier the more you own. Three rules, all integer, all in `crates/rules` so the HUD and the sim agree.

### Yeomen: +1 Grain each

The Yeoman's entry in `units.ron` has `cost: {"grain": 20}` and `ramp: PerUnit({"grain": 1})`: each `PerUnit` amount is added once per unit of the kind the player has alive or queued (`Rules::unit_cost`).

```text
cost = cost["grain"] + ramp["grain"] * n
     = 20 + 1 * n
n    = Yeomen alive plus Yeomen currently queued
```

Queuing a Yeoman raises the next Yeoman's cost by 1 Grain before it finishes. The 5 starting Yeomen count toward `n`, so the first trained Yeoman costs 25 Grain (open question 1, answered for M3a).

### Buildings: +20% of base per copy owned or queued

```text
cost = floor(base * (1000 + building_ramp_step_permille * n) / 1000)   // step 200
n    = copies of that building kind owned or under construction, the Seat included
```

| n | Multiplier |
| --- | --- |
| 0 | 1.00 |
| 1 | 1.20 |
| 2 | 1.40 |
| 3 | 1.60 |
| 4 | 1.80 |

The second Town costs exactly 1.2 times the first: 120 Lumber against the Town's base 100 in `buildings.ron`. That plus one 30 Lumber Croft is exactly the 150 starting Lumber, which the M3a economy fixture relies on. The acceptance test checks this against the `rules.ron` formula, not a hard-coded number.

### Military units: triangular per training-building class, capped at 2.25x

The ramp counts units currently queued, not units alive, so it throttles production bursts rather than army size. The counter is shared across all buildings of the same class (every Muster Hall shares one counter; every Engine Yard shares another) and across every unit kind that class trains, so building a second Muster Hall does not reset the ramp.

```text
k       = 1 + units already queued at buildings of this class
T(m)    = m * (m + 1) / 2                       // triangular number
mult    = min(military_ramp_cap_permille, 1000 + military_ramp_step_permille * T(k - 1))
cost    = floor(base * mult / 1000)             // per resource
```

A unit kind opts in with `ramp: Military` in `units.ron`, as the Skirmisher does. With the starting step of 100 permille (`military_ramp_step_permille: 100`) and the cap of 2250 permille (`military_ramp_cap_permille: 2250`):

| k (position in class queue) | T(k-1) | Multiplier |
| --- | --- | --- |
| 1 | 0 | 1.00 |
| 2 | 1 | 1.10 |
| 3 | 3 | 1.30 |
| 4 | 6 | 1.60 |
| 5 | 10 | 2.00 |
| 6 | 15 | 2.50, capped to 2.25 |

The 6th queued Skirmisher is capped at 2.25x base. The design fixes the cap and that the 6th unit reaches it; 100 permille is the smallest round step that satisfies both (any step from 84 to 124 permille would). Finishing or cancelling a unit lowers `k` for the next order.

## Population cap

```text
pop_cap = 25 + 25 * arms_level
```

Both numbers are data: the base is `pop_cap_base: 25` in `rules.ron`, and each Arms level adds 25 through its `(target: PopCap, op: Add, value: 25)` row in `techs.ron` (M4a; the sim resolves the rows since the M4a tech slice, `SIM_VERSION` 12).

| Arms level | Pop cap |
| --- | --- |
| 0 | 25 |
| 1 | 50 |
| 2 | 75 |
| 3 | 100 |

Scribes cost 1 pop. Per-kind pop costs for the other units are in `units.ron`. A Train command that would exceed the cap is rejected with `CommandRejected { reason: PopCapReached }` and the HUD disables the button with a tooltip (M3b).

## Idle Yeoman work seeking

A Yeoman that has been idle for `idle_seek_delay_ds` (50 ds = 5 s = 100 ticks) looks for work on its own.

```text
candidates = gather buildings owned by the player with a free slot
order by (dist_sq_i64(yeoman, building), BuildingId)
if any: issue an internal Gather order to the first
else:   stay idle and retry after idle_seek_retry_ds (50 ds = 100 ticks)
```

The ordering is a total order with an id tiebreak, as every sort in the sim must be (see `docs/DETERMINISM.md`). The retry interval is an M3a choice, not a design number: it is its own field, `idle_seek_retry_ds` in `rules.ron`, starting equal to the delay. The delay is a good first issue: tune `idle_seek_delay_ds` (starting value 50, that is 5 s) in `rules.ron` and document the play-bots result.

## Standard start

| Item | Value |
| --- | --- |
| Towns | 1 (this is the Seat) |
| Yeomen | 5 |
| Grain | 150 |
| Lumber | 150 |
| Ore | 0 |
| Lore | 0 |
| Age | Hearth |
| Yield Cap | 70 |
| Pop cap | 25 |

The Seat, the Yeomen and the stockpiles are data: `standard_start` in `rules.ron` holds `units: {"yeoman": 5}` and `stockpile: {"grain": 150, "lumber": 150, "ore": 0, "lore": 0}`, and the Seat is one Town of the `town: true` kind in `buildings.ron`, centred on the map start (the map validator checks that the 3x3 footprint fits there on grass). By owner decision the sim selects it through `MatchSetup.start: StartKind { Empty, Standard }`, added in the first M3a slice: skirmish matches use `Standard`, scenarios `Empty` unless they set `Standard` (the economy fixture does). The resulting ids are in [towns.md](towns.md) ("M3a implementation notes").

## Commands and rejections

The economy consumes `Build { unit, kind, tile }`, `Train { building, kind }`, `Gather { units, node }`, `Cancel { building, slot }` and `SetRally { building, target }`. Costs are deducted when the order is accepted; `Cancel` refunds the deducted cost. Rejections never panic; they emit `SimEvent::CommandRejected { player, seq, reason }` with reasons including `OutsideBorders` (see [territory.md](territory.md)), `PopCapReached` and `CannotAfford`.

## M3a implementation notes

The first M3a slice (the Town loop, `SIM_VERSION` 6) implements income from base yields, building costs, construction and the site `Cancel`. The gathering slice (`SIM_VERSION` 8, `rules_version` 9) adds gather slots, worked-slot income and idle-worker seeking in `crates/sim/src/gather.rs`. The training slice (`SIM_VERSION` 9, `rules_version` 10) adds training queues, the unit cost ramps, the pop cap, the queue `Cancel` and `SetRally` in `crates/sim/src/training.rs`; with it all six M3a slices are implemented. Where it is more precise than the sections above:

| Topic | As implemented |
| --- | --- |
| Income | Raw income is the `base_yield` of every complete building (the Town's 10 Grain and 10 Lumber, the Scriptorium's 10 Lore per 30 s) plus `rate_per_worker` for every worked slot. The Yield Cap applies per resource to every resource with `yield_capped`; it is `yield_cap_base` (70) with the player's `YieldCap` rows, resolved whenever it is read (since the M4a tech slice; Trade sets 100, 150 and 200), and Lore is exempt. After each tick's credit every stockpile with a `stockpile_cap` is clamped to it, so Lore stops at 999. Ten Crofts need two Towns (five per Town), so the flatline test's raw Grain rate is 120 (ten Crofts plus two Towns), not the 110 of the cap example above. Only stockpiles and accumulators are stored; rates, pop and ramp counters are computed when needed. `SimView::income` returns `RateInfo { raw, cap, credited }` per resource for the M3b readout. |
| Building cost | Deducted when `Build` is accepted. The ramp's `n` counts every own building of the kind, sites and the Seat included. `SimView::building_cost` and `SimView::check_build` quote with the same functions `Build` uses. |
| Construction | One builder per site. The builder walks to the tile around the footprint nearest to it that it can reach with the footprint blocked (ties by tile); `Build` is `Unreachable` when there is none. The site gains one tick of progress per tick its builder is at work: no order, and its post (the tile it arrived at) touches the footprint (Chebyshev distance 1). The post counts, not the live position, so separation jostle never pauses work. A `Move`, `AttackMove` or `Stop` ends the job, and a builder whose path ends anywhere else loses it; the site keeps its progress and waits. |
| Resuming | Re-issuing `Build` with the same kind and tile on an own site, by any builder, makes that unit its builder at no cost; a previous builder goes idle. |
| Site cancel | `Cancel { building: site, slot: 0 }` refunds `paid` in full, clamped to the stockpile caps, unblocks the footprint and idles the builder. Any other slot of a site is `InvalidSlot`; the slots of a complete building are its training queue (see Queue cancel); a missing building or another player's is `UnknownBuilding`. |
| Rejections | `Build`, `Cancel`, `Gather`, `Train` and `SetRally` rejections change no state and draw no randomness; the full `Build` check order is in [towns.md](towns.md) ("M3a implementation notes"). Since the M4a tech slice `Research` and `AdvanceAge` have handlers whose rejections draw nothing either (check orders in [tech-and-ages.md](tech-and-ages.md), "M4a implementation notes"), and since the M4a combat core slice so does `Attack` (rejected with `InvalidTarget` or `NoValidUnits`, see [combat.md](combat.md)): no rejection draws from the RNG. |
| Gather | `Gather { units, node }` is validated by a pure function (`gather::validate_gather`), and the first failure is the reason: the node is the player's (`UnknownBuilding`), is complete (`UnderConstruction`) and has gather slots (`NotAGatherNode`); at least one given unit is the player's, of the node's `worker` kind and able to walk to a tile beside the footprint (`NoValidUnits`); a slot is free (`NoFreeSlot`), checked only when some valid unit is not already on the node: re-issuing the order to the node's own workers is accepted and changes nothing, whether or not the node is full. The valid units not already on the node take the lowest free slots in `UnitId` order; units left over when the slots run out are untouched. A unit that had another job leaves it. `Gather` no longer draws from the RNG. |
| Slots | A slot is taken only by a unit holding `Job::Gather { node, slot }`; nothing else records occupancy, so nothing can fall out of sync. The stand tile of slot `k` for a worker is the `(k mod n)`-th of the `n` passable ring tiles (around the footprint, clockwise from the north-west corner) in the worker's connected component; a complete building's footprint is blocked, so these are exactly the ring tiles the worker can reach. `Adjacent` slot counts are fixed at placement from the 8-neighbourhood (open question 4). `SimView::worked_slots(id)` returns `(worked, taken, slots)`. |
| Worked slots | A slot is worked, and earns `rate_per_worker`, while its unit is at work: no order, and its post adjacent to the footprint (Chebyshev distance 1), the same rule as a builder's. The post counts, not the live position, so a worker pushed off its stand tile keeps earning. Step 6c drops a gather job when its building is gone, is not the unit's owner's, is not complete or is not worked by the unit's kind, or when the unit has no order and is not at work there; `Move`, `AttackMove` and `Stop` end it too. Either way the slot is free again. |
| Idle seeking | A unit of a kind that works some building (the Yeoman and the Scribe) is idle when it has no job and no order; holding a post is still idle. In step 6c an idle unit without a timer sets `seek_at` to the current tick plus `idle_seek_delay_ds` (100 ticks). When the timer runs out it takes the lowest free slot of the nearest own complete building its kind works, among those with a free slot and a ring tile it can reach, ordered by `dist_sq_i64` from its position to the footprint centre, then `BuildingId`, and walks to the slot's stand tile. It takes the slot directly; no command is issued. With no such building it tries again after `idle_seek_retry_ds` (100 ticks). A unit with a job or an order has no timer. Units are visited in `UnitId` order and see the slots taken earlier in the same tick. `SimView::idle_workers(p)` lists a player's idle workers. |
| Builder takes its slot | When a site completes, its builder takes the lowest free slot there if its kind is the building's `worker` and a slot is free, so a Croft's builder takes its slot at once and works it once it reaches the slot's stand tile, beside the footprint it already stands next to; otherwise the builder goes idle. |
| Train | `Train { building, kind }` is validated by a pure function (`training::validate_train`, also behind `SimView::check_train`), and the first failure is the reason: the building is the player's (`UnknownBuilding`) and complete (`UnderConstruction`) and, from M5a, not a Town whose annexation timer runs (`TownContested`, see [towns.md](towns.md)); the unit kind exists (`UnknownUnitKind`) and the building's kind is its `trainer` (`CannotTrainHere`); from M4a, the player has reached the age that unlocks the kind (`AgeRequired`) and researched its `requires_tech`, if it names one (`PrerequisiteMissing`); the queue holds fewer than `train_queue_max` (5) entries (`QueueFull`); pop used plus the kind's `pop` stays within the pop cap (`PopCapReached`); the player can pay the ramped cost (`CannotAfford`, naming the first short resource). Pop used is every own unit alive plus every unit entry queued at an own building, each at its kind's `pop` (research entries take none); the cap is `pop_cap_base` (25) with the player's `PopCap` rows (each Arms level adds 25, since the M4a tech slice). An accepted `Train` deducts the cost and appends `QueueEntry { item: Unit(kind), paid, progress: 0 }`, so cost and pop are both charged when the unit is queued. `SimView::pop(p)` returns `(used, cap)`. `Train` no longer draws from the RNG. |
| Training | In step 6b every complete building, in `BuildingId` order, adds one tick of progress to whichever entry is the head of its queue when 6b reaches it; the other entries gain nothing. The head becomes a unit in the step where its progress reaches the kind's `train_time_ds` (Yeoman 120 ds = 240 ticks), that is, in the `train_time`-th step in which it was the head. An entry queued into an empty queue is the head in the very step that applied the `Train`, so it appears `train_time` - 1 steps after that step; the entry behind a finished head becomes the head after 6b has passed the building, so it starts in the next step and a busy building turns out exactly one unit every `train_time` steps. The unit appears beside the building (`UnitSpawned`): on the first passable tile of the spiral around the footprint's centre tile that lies in the spawn component. The candidates are the components of the passable tiles on the building's ring, or every component when the whole ring is blocked. The spawn component is the rally point's when the rally point's own tile is passable and its component is a candidate (the player pointed into it; a rally on a building names no side), and otherwise the largest candidate by tile count, then the lower id. So a pocket that neighbouring buildings seal against the building takes the unit only when the rally point is inside it (`SIM_VERSION` 11). The tile may already hold a unit; separation pushes them apart. With a rally point set the unit walks there, the point corrected to a passable tile as for a `Move`; otherwise it takes the lowest free slot of its own building when its kind is the building's `worker` (a Scribe trained at a Scriptorium takes a slot there at once and works it once it reaches the slot's stand tile); otherwise it starts idle: a worker kind (Yeoman, Scribe) then seeks work after `idle_seek_delay_ds`, and any other kind (the Skirmisher) stands where it appeared. |
| Unit cost | `training::unit_cost_for` takes the counts and `Rules::unit_cost` turns them into the price. `n` is the player's units of the kind alive plus entries of the kind queued at its buildings; the five standard-start Yeomen count (owner decision D11, open question 1), so the first trained Yeoman costs 25 Grain. `k` is 1 plus every unit entry, of any unit kind, queued at the player's buildings of the kind's trainer kind, so every Muster Hall shares one counter. Research entries (M4a), which share a Scriptorium's queue, count toward neither `n` nor `k`. `Flat` costs the base, `PerUnit` the base plus `step * n` (the Yeoman 20 + `n` Grain) and `Military` `floor(base * min(cap, 1000 + step * T(k - 1)) / 1000)` with the 100 permille step and the 2250 cap: the Skirmisher's 40 Grain gives 40, 44, 52, 64, 80 and 90 for `k` = 1 to 6. An entry counts toward `k` until it finishes or is cancelled; a finished unit keeps counting toward `n` while it is alive. `SimView::unit_cost(p, kind)` quotes with the same function `Train` uses. |
| Queue cancel | `Cancel { building, slot }` on an own complete building with `slot` below its queue length removes that entry, a unit or (from M4a) a research entry, and refunds exactly its `paid`, clamped to the stockpile caps, and emits `TrainingCancelled { building, slot }`; a cancelled head loses its progress. A slot at or past the end of the queue is `InvalidSlot`. |
| Rally | `SetRally { building, target }` on an own building, site or complete, sets its rally point (otherwise `UnknownBuilding`); units trained there from then on walk to it. `SetRally` no longer draws from the RNG. |

## Interactions

| System | Interaction |
| --- | --- |
| [territory.md](territory.md) | Build is legal only on owned tiles. Towns are the border sources, so expansion and economy are the same decision. |
| [towns.md](towns.md) | Each Town yields 10 Grain and 10 Lumber per 30 s and hosts up to 5 Crofts. The town limit bounds Croft count. |
| [tech-and-ages.md](tech-and-ages.md) | Trade raises the Yield Cap. Arms raises the pop cap. Letters lowers research cost. Age advances spend Grain, Lumber and Lore. |
| [combat.md](combat.md) | Ore is the military bottleneck: Shieldbearers, Outriders, Watchtowers and Arms techs all need it. |
| [ai.md](ai.md) | The bot balances workers across slots and reads the same cost functions from `crates/rules`. Difficulty adds income on an interval (documented as a cheat). |
| [ui.md](ui.md) | Resource bar shows stockpile and per-30 s rate; the readout turns amber when income is discarded. |

## Acceptance checklist (from M3a)

- [x] `cargo test -p sim economy`: one Yeoman on a Croft yields exactly 10 Grain per 600 ticks; with 10 Crofts worked the Grain rate is clamped to 70 per 600 ticks and the stockpile grows by exactly 70; Lore ignores the Yield Cap; the second Town costs more than the first by exactly the rules.ron formula; queuing a Yeoman raises the next Yeoman's cost by 1 Grain before it finishes; the 6th queued Skirmisher is capped at 2.25x base.

  Note: the first three clauses are tested by `economy_one_yeoman_on_a_croft_yields_exactly_the_croft_rate_per_income_period`, `economy_ten_worked_crofts_flatline_at_the_yield_cap` (two complete Towns and ten Crofts placed with `DebugPlaceBuilding`, by owner decision D2; raw 120 against the cap of 70) and `economy_lore_ignores_the_yield_cap` (three Scriptoria with three Scribes each, 75 Lore against 70, all credited). Each reads its rates from data. The second-Town clause is `economy_second_town_costs_more_than_the_first_by_the_rules_formula`. The Yeoman clause is `economy_queuing_a_yeoman_raises_the_next_cost_before_it_finishes` (the quote rises by the `PerUnit` step from `units.ron` as soon as the Yeoman is queued, the deduction equals the quote, and the quote does not move again when the unit appears). The Skirmisher clause is `economy_sixth_queued_skirmisher_is_capped`: five Skirmishers queued at one Muster Hall and the sixth at a second one, so the shared counter is what reaches the cap (a sixth at the first hall is `QueueFull`); cancelling two entries refunds exactly what they paid and the next quote falls back to `k` = 5.

- [x] Scripted headless economy fixture: from the standard start (1 Town, 5 Yeomen, 150 Grain / 150 Lumber / 0 Ore / 0 Lore) a fixed build order founds a second Town before tick 3600 and the final hash matches the fixture.
- [x] Data-only contributor path: change croft gather rate from 10 to 12 in data/rules/buildings.ron, run `sim-cli data-check` and `cargo test -p sim` (the flatline test reads the number from data) without touching Rust; documented in CONTRIBUTING.

  Note (owner decision): any RON edit changes `rules_hash`, so the documented path also bumps `rules_version` and runs `just rerecord` between `data-check` and `cargo test -p sim`; otherwise the golden fixtures report `RULES CHANGED since recording`. See CONTRIBUTING, "The data-first path".

The complete M3a list, including the territory lines, is in [territory.md](territory.md) and `docs/ROADMAP.md`.

```bash
cargo test -p sim economy
cargo run -p sim-cli -- data-check data/
```

## Open tuning questions

1. Do the 5 starting Yeomen count toward the Yeoman ramp? If yes, the first trained Yeoman costs 25 Grain; if no, 20. Answered for M3a: they count, so the first trained Yeoman costs 25 Grain.
2. The military ramp step (100 permille) is derived, not designed. Confirm it, and confirm that the class counter spans every unit kind the class trains. M3a implements that reading: `k` counts every entry queued at the player's buildings of the trainer kind, whatever unit kind it is.
3. Should the idle-Yeoman retry interval equal the initial delay (100 ticks), or be longer to reduce path requests? Answered in data: `idle_seek_retry_ds` is its own field in `rules.ron`, starting at 50 ds (100 ticks), equal to `idle_seek_delay_ds`, and tunable without Rust.
4. Adjacency for Lumber Yard and Ore Pit slots: 8-neighbourhood of the footprint is the M3a reading. Implemented that way: a Lumber Yard at (13, 57) on `plains_1v1` touches three forest tiles and gets three slots (`economy_adjacent_slots_count_forest_and_mountain_tiles`).
5. Should Grain, Lumber and Ore have a stockpile cap at all? v0.1 says no.
6. Is the Yield Cap decay after minute 30 ever on by default? M6 decides from the 20 seeded bot games.
7. Croft gather rate 10 versus 12 is the documented data-only contributor path; the balance issue form asks for a play-bots table.
