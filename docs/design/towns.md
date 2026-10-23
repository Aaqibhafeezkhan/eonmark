# Towns

This document specifies Towns: how they are founded, how many a player may hold, how their border radius grows, why they cannot be destroyed, the annexation state machine that transfers them, and the victory conditions that depend on them. Every number here is a starting value in `data/rules/rules.ron` or `data/rules/buildings.ron` and may move during M6 tuning. Founding, limits and radius growth land in M3a (`crates/sim/src/town.rs`); annexation and victory land in M5a. For the design lineage see [prior-art.md](prior-art.md).

| Field | Value |
| --- | --- |
| Milestone | M3a (founding, limit, radius), M5a (annexation, victory) |
| Labels | `area:sim`, `area:rules` |
| Rules files | `data/rules/rules.ron` (town radius, town limit `town_limit_base`, annexation 60 s, defender radius 6, match cap 45 min), `data/rules/buildings.ron` (Town footprint, HP, cost, build time, base yield; the Croft limit `max_per_town`; shooter stats from M4a) |
| Sim modules | `crates/sim/src/town.rs`, annexation in `crates/sim/src/annexation.rs` (M5a), the outcome in `crates/sim/src/victory.rs` with `Sim::outcome()` in `state.rs` |
| Sub-hash | `buildings`, `territory`, `meta` (the match outcome, M5a) |

## Target numbers

| Parameter | Start value | Where |
| --- | --- | --- |
| Town limit | 2 + Statecraft level (the Seat and Town sites count) | `rules.ron` `town_limit_base: 2`, `techs.ron` row `town_limit Add 1` |
| Border radius, tier 1 | 20 tiles | `rules.ron` |
| Border radius, tier 2 | 24 tiles, once 5 distinct building kinds are attributed to the Town | `rules.ron` |
| Town base yield | 10 Grain and 10 Lumber per 30 s | `buildings.ron` |
| Crofts per Town | 5 | `buildings.ron` (`croft` `max_per_town: Some(5)`) |
| Town footprint, HP, cost, build time | 3x3 tiles, 2000 HP, 100 Lumber (120 for the second, with the building ramp), 600 ds = 60 s | `buildings.ron` |
| Annexation timer | 60 s = 1200 ticks | `rules.ron` |
| Defender exclusion radius | 6 tiles | `rules.ron` |
| Match cap | 45 min = 54000 ticks | `rules.ron` |

| Statecraft level | Town limit |
| --- | --- |
| 0 | 2 |
| 1 | 3 |
| 2 | 4 |
| 3 | 5 |

The base limit is 2 by owner decision, so a second Town can be founded in M3a, before Statecraft exists (it arrives in M4a); the M3a economy fixture founds one. The design's earlier reading was 1 + Statecraft level.

## Founding

A Town is a building kind. A Yeoman founds one through `Build { unit, kind: Town, tile }`, so it obeys every construction rule: the tile must be inside the founder's borders (`is_buildable`), passable and unoccupied, the player must be under the town limit, and the ramped cost (base times 1.2 for the second copy, 120 Lumber, see [economy.md](economy.md)) must be affordable. The second Town is therefore placed at the edge of current borders and extends them by its own radius when complete.

| Rejection | Reason |
| --- | --- |
| Outside borders | `OutsideBorders` |
| At the town limit | `TownLimitReached` (proposed name) |
| Cannot pay | `CannotAfford` |

The first Town a player owns at match start is the Seat. The Seat flag never moves to another Town.

## Radius growth

Each non-Town building is attributed to one Town at placement: the nearest own Town by `dist_sq_i64`, `BuildingId` as tiebreak. A Town whose attributed buildings cover 5 distinct kinds (for example Croft, Lumber Yard, Ore Pit, Scriptorium, Muster Hall) moves to tier 2 and its radius grows from 20 to 24. Losing buildings can drop it back; the field is recomputed either way (see [territory.md](territory.md)).

The age list says the Masonry Age "unlocks the second Town growth tier". M3a implemented the building-kind condition alone because ages did not exist yet; since the M4a tech slice the tier-2 radius also requires the Masonry Age (`town_growth_age` in `rules.ron`, open question 4, decided). See "M4a implementation notes" below.

## Towns are indestructible

Town HP can reach 0, but the building is never removed from the map. A Town at 0 HP keeps its border kernel (until annexation starts), keeps yielding its base income, and keeps training unless the annexation timer is running. It does not shoot and cannot be attacked (open question 2, decided). Every other building kind is destroyed at 0 HP, and so is a Town site (see [combat.md](combat.md)). Since the M4a combat core slice this is the sim's behaviour: see "M4a implementation notes" below.

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

## M3a implementation notes

The first M3a slice (the Town loop, `SIM_VERSION` 6) implements the standard start, founding and the town limit in `crates/sim/src/town.rs` and `construction.rs`; the gathering slice (`SIM_VERSION` 8) adds tier-2 growth. Where it is more precise than the sections above:

| Topic | As implemented |
| --- | --- |
| Standard start | With `MatchSetup.start: Standard` (the skirmish default), `Sim::new` places per player, in id order, the Seat (a complete Town, `TownState { seat: true, tier: 1 }`, centred on the map start and projecting its border), then the start units on the nearest passable tiles, then the stockpile. On `plains_1v1` the Seats are `BuildingId(1)` (P0) and `BuildingId(2)` (P1), P0's Yeomen `UnitId(1..=5)` and P1's `UnitId(6..=10)`, and each Seat owns 1245 tiles. |
| Borders | A founded Town projects its border when it completes, not when its site is placed; a site claims nothing. The kernel centre is the footprint's centre tile. |
| Town limit | Counts every Town-kind building of the player, sites and the Seat included, against `town_limit_base` plus the Statecraft level (0 until M4a; since the M4a tech slice the limit is resolved from the `TownLimit` rows, see below). The proposed name `TownLimitReached` is the one used. |
| Attribution | Fixed at placement: the nearest own complete Town by `dist_sq_i64` between footprint centres, then `BuildingId`. Town sites do not count. A kind with `max_per_town` (the Croft) placed while the player has no complete Town is rejected with `PerTownLimitReached`; any other non-Town kind is then placed with no Town. |
| Tier 2 | `town::recompute_tier` runs when a building attributed to a Town completes. The Town is tier 2 when the owner's complete buildings attributed to it cover at least `town_growth_distinct_buildings` (5) distinct non-Town kinds, else tier 1. Sites do not count, and attribution is fixed at placement: a building counts for the Town it was attributed to then, even after a nearer Town completes. A tier change moves the Town's border source to the tier's radius (`town_radius_grown_tiles`, 24, plus Statecraft, 0 until M4a) with `TerritoryField::change_source` and emits `TownTierChanged { town, tier }` and `TerritoryChanged`. The rule would also drop a Town back to tier 1, but nothing removes a complete building before combat (M4a). The Masonry Age gate waits for ages in M4a (open question 4). Since the M4a tech slice the function is `town::refresh` and the gate applies (see "M4a implementation notes"). `territory_town_grows_to_tier_two_at_five_distinct_kinds` checks the radius and that the field equals a fresh recompute. |

`Build` checks run in this order and the first failure is the reason (the one the M3b HUD will show); a rejected `Build` changes nothing and draws no randomness:

1. the unit exists and is the player's (`NoValidUnits`);
2. the kind exists (`UnknownBuildingKind`);
3. the unit's kind can build (`NotABuilder`);
4. an own site of the same kind at the same tile is resumed instead, at no cost (see [economy.md](economy.md)), whatever the player's age;
5. the player has reached the age that unlocks the kind in `ages.ron` (`AgeRequired`; since the M4a tech slice, so in the starting data the Engine Yard needs the Masonry Age);
6. the footprint is on the map (`SiteOutOfBounds`);
7. every footprint tile is the player's territory (`OutsideBorders`);
8. every footprint tile is passable (`SiteBlocked`);
9. no unit of any player stands on it (`SiteOccupied`);
10. a Town keeps the player within the town limit, `town_limit_base` with the `TownLimit` rows (`TownLimitReached`);
11. a kind with `max_per_town` has a complete Town to belong to and room under its limit there, sites included (`PerTownLimitReached`);
12. a gatherer with `Adjacent` slots has at least one tile of its terrain beside the footprint (`NoAdjacentResource`);
13. the builder can walk to a tile around the footprint, with the footprint already blocked (`SiteBlocked` when none is passable, `Unreachable` when none is reachable; a resume, check 4, is held to the same rule);
14. the player can pay the ramped cost (`CannotAfford`, naming the first short resource).

## M4a implementation notes

The M4a tech slice (`SIM_VERSION` 12, `rules_version` 14) gives Towns their tech and age inputs; the combat core slice (`SIM_VERSION` 13, `rules_version` 15) adds Towns at 0 HP and the destruction of other buildings; the combat behaviour slice (`SIM_VERSION` 14, `rules_version` 16) makes a complete Town with hit points left shoot.

| Topic | As implemented |
| --- | --- |
| Town limit | `tech::town_limit`: `town_limit_base` (2) with the owner's `TownLimit` rows, the last `Set`, the `Add`s, then each `Mul` floored; each Statecraft level adds 1. `SimView::towns` returns `(count, limit)` with it, and `Build` check 10 uses it. `tech_statecraft_raises_the_town_limit_and_pushes_every_own_border` shows a third Town rejected with `TownLimitReached` before Statecraft I and accepted after. |
| Tier and radius | `town::refresh(town)` replaces `recompute_tier`. It re-derives the tier (2 when the attributed complete buildings cover `town_growth_distinct_buildings` distinct kinds **and** the owner has reached `town_growth_age`, else 1) and then the radius, `tech::town_radius(owner, tier)`: the tier's base radius with the owner's `TownBorderRadius` rows, clamped to 1..=64 (data-check keeps the worst case within 64). A tier change emits `TownTierChanged`; a radius change, from the tier or from Statecraft, moves the border source with one `change_source` and emits `TerritoryChanged`. Comparing the radius as well as the tier is what lets a Statecraft level push a Town whose tier did not change. |
| When it runs | When a building attributed to the Town completes (as in M3a), and for every complete Town of the player, in `BuildingId` order (`town::refresh_all`), when the player completes a tech (research or `DebugResearch`) or advances an age. A Town that completes projects `tech::town_radius(owner, 1)` at once, so a Town founded after Statecraft starts pushed; the Seat at the standard start uses the same function. Since the combat core slice, destruction refreshes each Town a destroyed building was attributed to, in `BuildingId` order, after the footprints are unblocked; losing a kind can drop the Town back to tier 1 and radius 20 (`combat_watchtower_destroyed_frees_footprint_border_and_tier`). |
| Age gate | `town_growth_age: Some("masonry")` in `rules.ron` (owner decision 2026-10-21). A Town with five distinct kinds in the Hearth Age stays at tier 1 and grows in the same apply as its owner's `AdvanceAge` into Masonry (`TownTierChanged` and `TerritoryChanged` with the `AgeAdvanced` event). `None` removes the gate with no Rust change; `tech_masonry_gates_engine_yard_outrider_and_town_tier_two` checks both settings and that the field equals a full recompute. |
| Build | Check 5, `AgeRequired`, comes right after the resume branch, so a site that exists already (placed by `DebugPlaceBuilding`, which ignores ages) can always be resumed. |
| Town at 0 HP | Since the combat core slice a complete Town's hit points are clamped at 0: it stays with its footprint, border kernel and queue, emits `TownAtZeroHp { town }` once, is no longer a target for `Attack` (`InvalidTarget`; units attacking it stand down the next tick) and does not regenerate. `Train` there is still accepted until annexation (M5a). A Town site at 0 HP is destroyed like any site, which frees a slot under the town limit (`combat_town_at_zero_hp_stays_and_is_not_targeted`, `combat_site_at_zero_hp_is_destroyed_town_sites_included`). Annexation needs infantry or cavalry next to the Town; since the combat behaviour slice idle soldiers acquire the nearest enemy in their reach and may leave it for another target, which M5a has to allow for. |
| Town shooter | Since the combat behaviour slice a complete Town with hit points left shoots like a Watchtower (`attack` in `buildings.ron`: 15 every 15 ds, 7 tiles from its footprint to the target's edge): the nearest enemy unit in range, units only, an exact tie drawn from the sim's RNG; a site does not shoot, and a Town at 0 HP neither shoots nor is shot at (`combat_watchtower_and_town_shoot_enemy_units_in_range`, `combat_town_at_zero_hp_does_not_shoot`; see [combat.md](combat.md), "Shooters"). |

## M5a implementation notes

The M5a annexation and outcome slice (`SIM_VERSION` 16, `rules_version` 20) implements the state machine above in `crates/sim/src/annexation.rs` (step 5d) and the victory conditions in `crates/sim/src/victory.rs` (step 6e), with the owner's decisions of 2026-10-22 (ROADMAP M5a, "Notes"). Where it is more precise than the sections above:

| Topic | As implemented |
| --- | --- |
| State | `TownState::hold: Hold`, `Held` or `Contested { attacker, ticks }`, hashed in `buildings`. `SimView::annexation(town)` returns `AnnexView { attacker, ticks, total }` while the timer runs; `total - ticks` more steps flip the Town. |
| When it runs | Step 5d, right after `combat::resolve`, on the post-movement, post-reap positions, for every complete Town at 0 HP in `BuildingId` order (none, the usual case, costs nothing: only a flip restores hit points). |
| Attackers | Units of another player whose kind `annexation_attackers` selects (`[Class(Infantry), Class(Cavalry)]`) on a tile within `annexation_adjacent_tiles` (1) of the footprint, Chebyshev. Mangonels and Supply Wains beside the Town do not start it (`annexation_mangonels_and_supply_wains_adjacent_do_not_start_it`). |
| Defenders | Units of the Town's owner whose kind `annexation_defenders` selects (`[Military]`: every non-Civilian kind, the Mangonel and the Supply Wain included; Yeomen and Scribes do not defend) within `annexation_defender_radius_tiles` (6.0) of the footprint's centre, centre to centre, inclusive (`annexation_defender_radius_is_6_tiles_inclusive`: 6.0 tiles straight south resets, 7.0 does not, and off the axis 5.39 resets while 6.08 does not, so the radius is a Euclidean disc). A Town above 0 HP never starts the timer, whoever stands beside it (`annexation_town_above_0_hp_never_starts_the_timer`). |
| Eligible | Hit points 0, exactly one player owns attackers and there is no defender (`annexation::eligible`). Two attacking players hold the Town as it is. |
| Entry | The first eligible step sets `Contested { attacker, ticks: 0 }` and moves the Town's border source to the neutral layer (`AnnexationStarted`, `TerritoryChanged`). Each later eligible step adds 1; the Town flips in the step that brings `ticks` to `annexation_ds` in ticks (1200), so 1200 steps after the entry step. |
| Reset | Any ineligible step (a defender within 6 tiles, no attacker left, another player's attackers) returns the Town to `Held` and the source to its owner's layer (`AnnexationReset`, `TerritoryChanged`); a different sole attacker is a reset and a new entry in the same step. |
| During the timer | The Town's land is neutral: its kernel stamps the neutral layer, so `owner_at` is `None` wherever no other kernel wins and the field equals a full recompute. `Train` and `Research` there are rejected with `TownContested` (check 3, after `UnderConstruction`; no tech is hosted by a Town in the shipped data, but the `Research` rejection comes before the tech and host checks, so it is reachable) and `SimView::check_train` and `SimView::check_research` agree (`annexation_during_the_timer_research_is_rejected_and_a_research_entry_pauses`); the queue is paused (the head gains no progress) while `Cancel` and `SetRally` still work; base income continues; a new building of the owner is attributed to its nearest other complete Town (`town::attribute` skips contested Towns). Neutral land applies no attrition (`annexation_neutral_land_under_the_timer_applies_no_attrition`). |
| Flip | The Town and every building and site of the old owner attributed to it change hands, except kinds with `stays_on_annexation` (the Watchtower, sites included; owner decision 4: Muster Halls and Engine Yards transfer). Each building that changes hands has its queue refunded to the old owner, clamped like `Cancel`, with a `TrainingCancelled` per entry (owner decision 3); a site's `paid` is set to 0 with no refund; its rally point is cleared; a complete one that projects its own border (a kind with `border_radius_tiles` whose data does not keep it with the old owner) has its source moved to the new owner's layer, centre and radius unchanged, so its land changes hands with it (`annexation_flip_moves_a_transferred_border_source_to_the_new_owner`, with the Watchtower set to transfer in memory: the shipped Watchtower stays). Every unit of another player whose `Build` or `Gather` job names one of them goes idle. The Town is `Held` again with `annexation_town_hp_permille` (250) of its kind's hit points, 500 of 2000 (owner decision 2), so it shoots and can be attacked again; the seat flag stays on the building (owner decision 13); its source moves to the new owner's layer and `town::refresh` re-derives its tier and radius for the new owner. Each staying building is attributed again to the old owner's nearest uncontested complete Town, or none, and each Town that gains one is re-derived, so it may grow a tier (`annexation_flip_attributes_staying_buildings_to_the_old_owner_s_nearest_town`). Emits `TownAnnexed { town, from, to }` and `TerritoryChanged`. The town limit does not apply to a flip; only new Town sites are gated (owner decision 8). A restored Town that drops to 0 again emits `TownAtZeroHp` again. |
| Annex-hold guard rule | Owner decision 6. An idle guard (no order, no job, no combat order) whose kind `annexation_attackers` selects and that stands within `annexation_adjacent_tiles` of an enemy complete Town at 0 HP keeps a target only while it is in weapon range and takes only targets in weapon range, so it holds and never chases off the ring (`annexation_adjacent_guards_hold_the_ring`). With the Town still at 0 HP, a guard that had chased a target out of weapon range drops it in the step the Town falls (`annexation_adjacent_guards_drop_a_target_out_of_weapon_range`). Away from Towns at 0 HP acquisition is M4a's: none of the eight re-recorded fixtures' Towns ever loses a hit point (only `capture_seat`, a new recording, takes P1's Seat to 0), so their units are identical tick for tick across the re-record. |
| Randomness | Steps 5d and 6e draw nothing (`annexation_draws_no_randomness`). |

The outcome, as implemented (`Sim::outcome() -> Option<Outcome>`, `Sim::result() -> Option<MatchResult { outcome, reason, tick }>`, both also on `SimView`). Step 6e runs last in the step while no outcome is set; the first condition that holds decides, the result's tick is `Sim::tick()` after the step, `MatchDecided` is emitted once, and the result never changes while the sim keeps stepping:

| Order | Condition | Result |
| --- | --- | --- |
| 1 | A player has surrendered and exactly one has not | `Victory { that player, decisive: true }`, `Surrender`; everybody surrendered: `Draw`, `Surrender` |
| 2 | A Seat flipped this step (step 5d) | `Victory { its new owner, decisive: true }`, `SeatCaptured`; Seats flipped to two players in one step: `Draw`, `SeatCaptured` (`annexation_outcome_same_step_mutual_seat_capture_is_a_draw`) |
| 3 | A flip this step left its old owner with no complete Town (a Town site does not count, owner decision 7) | `Victory { the one player left, decisive: true }`, `AllTownsLost`; nobody left: `Draw` (unreachable with two players: every flip hands its new owner a complete Town, so a same-step mutual capture of two Towns that are not Seats leaves each player a Town and decides nothing, `annexation_outcome_same_step_mutual_town_capture_plays_on`) |
| 4 | The step reaches `match_cap_ds` (27000 ds, 54000 ticks) | `Victory { the player with the most owned tiles, decisive: false }`, `Territory`; an exact tie for the most: `Draw` (owner decision 1) |

So a surrender beats a same-step Seat capture (`annexation_outcome_surrender_beats_a_same_step_seat_capture`), and capturing the last Town that is also a Seat reads `SeatCaptured`. The `capture_seat` scenario decides `Victory { P0, decisive: true }`, `SeatCaptured`, at tick 1628; the `stalemate_45min` scenario `Victory { P0, decisive: false }`, `Territory`, at tick 54000 (1248 tiles against 1245).

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

- [x] `cargo test -p sim economy`: the second Town costs more than the first by exactly the rules.ron formula (part of the economy test line).
- [x] Scripted headless economy fixture: from the standard start (1 Town, 5 Yeomen, 150 Grain / 150 Lumber / 0 Ore / 0 Lore) a fixed build order founds a second Town before tick 3600 and the final hash matches the fixture.

From M5a:

- [x] `cargo test -p sim annexation`: a Town at 0 HP with enemy Skirmishers adjacent and no defender military within 6 tiles flips after 1200 ticks; a defender arriving at tick 600 resets the timer; during the timer `owner_at` returns neutral for its tiles and Train is rejected; capturing the Seat makes `outcome()` return `Victory { decisive: true }`; a 54000-tick scripted stalemate returns `Victory { decisive: false }` for the player with more territory.

  Observed on PR #43, 2026-10-23, with `cargo test -p sim --profile ci annexation -- --nocapture`: `test result: ok. 37 passed; 0 failed; 0 ignored; 0 measured; 22 filtered out` in m5a and 1 passed in the lib. Entry in step 1 and the flip in step 1201 (500 HP, P0 owning 1245 tiles); a defender arriving in step 601 reset the timer at 599, which restarted in step 651 and flipped in step 1851; during the timer the Town's 1245 tiles were neutral and `Train` was rejected `TownContested`; `capture_seat` ends `Victory { player: PlayerId(0), decisive: true }`, reason SeatCaptured, at tick 1628 with hash `0x2c9a9ef52a5184d0`; the 54000-step stalemate, run and not ignored, ends `Victory { player: PlayerId(0), decisive: false }`, reason Territory, at tick 54000 with 1248 tiles against 1245 (0.25 to 0.30 s).

- [x] `cargo run -p game --features dev -- --scenario capture_seat` shows the victory overlay with the final hash and the recorded replay verifies to it.

  Observed on PR #43, 2026-10-23, on a Linux VM under Xvfb with lavapipe (not the dev Mac), with background mode and `--exit-after-seconds 240` appended and `--profile ci`: exit 0, no ERROR line, `outcome: victory player=0 decisive=true reason=seat_captured tick=1628 hash=0x2c9a9ef52a5184d0` 82 s in and the game frozen at tick 1628 until the deadline; the screenshot at tick 1628 shows "Victory", "The Seat was captured", "1:21, tick 1628" and the final hash; `sim-cli verify` of its replay prints `OK final_hash=0x2c9a9ef52a5184d0 ticks=1628`, `capture_seat.hash`. `M5A_SECONDS=240 just m5a-checks` passes every step (docs/ROADMAP.md, M5a line 3, has the full output). The owner's Mac run is open ([../PLAYTEST.md](../PLAYTEST.md), "Controls checklist (M5a)").

```bash
cargo test -p sim annexation
cargo run -p game --features dev -- --scenario capture_seat
```

Good first issue from M5a: a test that a Statecraft level researched mid-timer pushes the neutral kernel's radius and the field still equals `recomputed()` (any OS; `town::refresh` keeps the neutral layer, territory.md). Filed as #45.

## Open tuning questions

1. Town HP after a flip: the design is silent. Options are keeping 0 HP (the defender can immediately contest it back) or restoring a data-defined fraction. M5a must pick and put the value in `rules.ron`. Decided 2026-10-22 (owner decision 2): `annexation_town_hp_permille: 250`, 500 HP; see "M5a implementation notes".
2. Does a Town at 0 HP keep shooting? Does Town HP regenerate? Decided 2026-10-21 (owner accepted the M4a plan defaults): it does not shoot, and it is not targeted. Decided 2026-10-21 by the M4a plan (an implementation choice, not one of the nine owner decisions in ROADMAP.md; the owner may revisit): its hit points do not regenerate. Town shooters arrived in the M4a combat behaviour slice, which skips a Town at 0 HP.
3. Exact territory tie at tick 54000: lowest `PlayerId`, or a `Draw` outcome variant. Decided 2026-10-22 (owner decision 1): `Outcome::Draw`.
4. Does the Masonry Age gate tier-2 radius growth, or does the age list only mean that tier 2 is usually reached in Masonry? Decided 2026-10-21 (owner accepted the M4a plan defaults): it gates it. `town_growth_age: Some("masonry")` in `rules.ron` names the age, the sim applies it from the M4a tech slice, and `None` removes the gate with no Rust change.
5. Should Muster Halls and Engine Yards transfer on a flip? This doc treats every kind except Watchtower as civilian for transfer purposes. Decided 2026-10-22 (owner decision 4): they transfer; only kinds with `stays_on_annexation` (the Watchtower) stay, attributed again to the defender's nearest uncontested Town or none.
6. Queued training at a flipped Town: cancelled with or without refund to the old owner. Decided 2026-10-22 (owner decision 3): every queue entry of a building that changes hands is refunded to the old owner, clamped like `Cancel`; a site's `paid` is zeroed with no refund.
7. Is 60 s the right timer against a Hard bot that attacks before minute 12? M6 play-bots decide.
