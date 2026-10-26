# Skirmish AI

This document specifies the scripted opponent in `crates/ai`: the `AiController` trait it implements and how the simulation calls it, the build-order executor and the rules layered on top of it, the three personalities, the difficulty table and the income cheat it is built on, the influence maps and counter-weighted composition added in M6, the `sim-cli play-bots` harness and its CSV, the M6 fun gate that the bot must pass, and the late-game pressure knobs available for tuning. The bot is a deterministic function of what it can see; it has no private randomness and no access to the world except through `SimView`.

Area: `area:ai`. Milestones: M5b (first complete opponent, Standard only), M6 (difficulties, personalities, influence maps, retreat, fun gate). See [../ROADMAP.md](../ROADMAP.md).

## Target numbers

| Quantity | Value | Where it is set |
| --- | --- | --- |
| AI cost | < 0.5 ms mean per tick added to the step, 400 units, bots on | `sim-cli bench --units 400 --ticks 1200 --bots` (M5b) |
| Easy | income interval 30 s, defensive | `data/ai/difficulty.ron` |
| Standard | income interval 25 s | `data/ai/difficulty.ron` |
| Hard | income interval 20 s, aggressive, first attack before minute 12 (tick 14400) | `data/ai/difficulty.ron` |
| Standard first wave | >= 8 units including 1 Supply Wain before tick 18000 (15 min), after reaching Masonry Age, in 5/5 seeds vs Passive | M5b acceptance |
| Difficulty ordering | Standard beats Easy >= 15/20 seeds; Hard beats Standard >= 15/20 seeds | M6 acceptance |
| Influence map cells | 4 x 4 tiles (32 x 32 cells on the 128 map), refreshed every 10 ticks, unhashed | `crates/sim/src/influence.rs` |
| Match cap | 45 min = tick 54000, then territory-percentage tiebreak | `data/rules/rules.ron` |
| Personalities | rush, boom, tower | `data/ai/build_orders/*.ron`, `data/ai/personalities.ron` |

All numbers are starting values and may move during the two to three M6 tuning iterations the plan budgets.

## Where the bot runs

`AiController` is defined in `crates/sim` (`ai_hook.rs`) and implemented in `crates/ai`:

```text
trait AiController: Send {
    fn think(&mut self, player: PlayerId, view: &SimView<'_>, memory: &mut AiMemory) -> Vec<Command>;
}
```

The `memory` parameter arrived in M5b (`SIM_VERSION` 17, owner decision 1 of 2026-10-23): it is the player's own `Player::ai_memory`, below.

The controller is injected at `Sim::new(setup, rules, Box<dyn AiController>)` and called from inside `Sim::step`, once per AI player per tick, in `PlayerId` order, before that tick's queued commands are applied. The commands it returns are stamped with the AI's `PlayerId` and a per-player sequence number and enter the same delayed queue as human commands: issued at tick N, applied at tick N + `cmd_delay_ticks`. They are validated like any other command and may be rejected with `SimEvent::CommandRejected`; the bot must tolerate rejection and must not loop on it.

Because the bot runs inside the step, its commands are not written to the replay. A replay holds the human's commands only; re-simulating it re-runs the bot and reproduces its decisions exactly. This is what lets bot-vs-bot games be verified by `sim-cli verify` and what keeps the future lockstep path simple (see [../DETERMINISM.md](../DETERMINISM.md)).

`ai::Passive` is the controller that returns an empty `Vec`. Tests, benches and the M0 skeleton use it.

Determinism constraints on the crate (enforced by `crates/ai/clippy.toml` and CI): no `f32`/`f64`, no `HashMap`/`HashSet`, no `Instant`/`SystemTime`, no `rand::random`/`thread_rng`, no engine crates. Ties are broken by the lowest id or lowest tile index. The design permits randomness only from the single `Pcg32` inside the sim; `think` receives a shared `SimView`, so if a later rule needs a random draw the sim must expose one through the view (for example a per-tick `u64` drawn before `think` runs). No v0.1 rule needs it.

Bot state (M5b). What a bot must remember between plans (wave rosters and timers, the defend latch, the military latch, blocked-step cooldowns, a chosen site) lives in the player's `AiMemory`, opaque bytes the bot encodes itself (the Standard bot uses postcard), stored in `Player::ai_memory` and hashed in the `meta` sub-hash. Step 2 takes every AI player's memory out of the state before it builds the view and hands each bot only its own, so no bot reads another's; a surrendered bot neither thinks nor has its memory touched. Because the memory is hashed state, `Sim::snapshot()` carries it and a restore reproduces the bot's next decision exactly, a lockstep peer agrees on it, and a bot divergence names `meta`. Everything that can be re-derived from `SimView` is recomputed every plan, so the memory stays small (`AI_MEMORY_GUIDE_BYTES`, 4096, is the design ceiling; the default Standard memory encodes to no bytes). Any other state a controller keeps must be a pure function of immutable inputs: the Standard bot's `MapAnalysis` is built once from the map's terrain and starts, the rules and its personality, never from the live view, so a controller rebuilt mid-match issues the same commands. Which bot drives which slot is in the replay header (`MatchSetup.ai_seats`: each AI slot's `bot`, `passive` or `standard`, and `personality`, a `data/ai/personalities.ron` id), and `ai::controller_for` builds the controller from it for every re-simulation. Bot code is versioned by `SIM_VERSION` (owner decision 2): from 17 on, a `crates/ai` change that can alter a decision bumps it, and the fixture policy then forces the `rules_version` bump and a re-record.

Cadence. `think` is called every tick. Keeping under 0.5 ms mean means the bot does its planning on a cadence rather than every call; the natural cadence is the 10-tick influence-map refresh. The cadence and any per-rule timers are data in `difficulty.ron` or `personalities.ron`, not constants in Rust. From M5b the Standard bot plans every `think_every_ds` (5 ds, 10 ticks) on ticks where `tick % every == player x every / players` (P0 on 0, 10, ..., P1 on 5, 15, ...), and `rules::Rules::load` requires that cadence in ticks to exceed `cmd_delay_ticks`, so a plan's commands apply before the next plan reads the view. An off-cadence call, and every call once the match is decided, returns at once.

## Architecture of the scripted bot (M5b)

The bot is a small set of rules evaluated in a fixed order. Each rule reads `SimView`, may emit commands, and may mark resources as reserved for the rest of this call so a later rule does not spend them.

1. Build-order executor. `data/ai/build_orders/<name>.ron` is an ordered list of steps: build a kind, train a kind, research a tech, advance age, found a Town, or wait until a condition (stockpile, unit count, tick). The executor keeps a cursor and advances when the current step is observed complete in the view (the building exists, the unit count is reached, the tech is known). It does not advance on command emission, so a rejected command is simply retried. `standard.ron` carries the Standard bot from the Hearth Age start (1 Town, 5 Yeomen, 150 Grain, 150 Lumber) to the Masonry Age. A `boom` variant is a `good first issue` (filed as #48: a `boom` build order and `personalities.ron` row compared with `play-bots --personality-a boom`, which needs a `rules_version` bump and `just rerecord` of every fixture, `m5_first_match` included). Two more M5b good first issues cover the build-order validators: #49, a `data-check` negative test for a build order whose `Research` host is never built, and #50, `sim-cli data-check --verbose` printing each build order's step count and the step of its first `Advance`.
2. Worker balancing. Each build-order stage names a target Yeoman count and a target split across Grain, Lumber and Ore. The rule trains Yeomen toward the count (respecting the Yield Cap: it stops adding gatherers to a resource whose rate already sits at the cap, see [economy.md](economy.md)) and issues `Gather` commands to move idle or wrongly assigned Yeomen toward the split. It relies on the sim's own idle-worker seeking for the rest.
3. Second Town. The base Town limit is 2 (the Seat counts, see [towns.md](towns.md)), so the limit no longer makes the bot wait for Statecraft I before its second Town; Statecraft raises it for later Towns (see [tech-and-ages.md](tech-and-ages.md)). When the limit allows, the rule picks a site: an owned, buildable tile as far from the Seat as the border allows, in the direction of the most unclaimed resource tiles, and sends a Yeoman to found a Town there. The new Town extends the territory field (see [territory.md](territory.md)).
4. Watchtower at the border edge. The rule finds the owned buildable tile closest to the enemy Seat (the enemy Seat's position is known from the map's mirrored start, not from scouting) and builds a Watchtower there. The Watchtower pushes the border two tiles, gives vision, shoots, and hosts Harrying research, which the tower personality researches early (see [attrition.md](attrition.md)).
5. Timed attack waves. From the build order's "military phase" onward the rule accumulates an army at a rally point near the border Watchtower. When the army reaches the wave size for the current difficulty and personality and the wave timer has elapsed, it issues `AttackMove` toward the nearest known enemy Town, with exactly one Supply Wain attached so the wave does not wither under attrition inside enemy borders. Waves target Towns, not units: the win condition is capital capture.
6. Defend the Seat. If any visible enemy military unit is inside the bot's own borders within a data-defined radius of its Seat, every military unit the bot owns is ordered to `AttackMove` to the Seat, the wave timer is paused, and Yeomen keep working. The rule releases when no enemy military has been visible there for a data-defined number of ticks.
7. Retreat when losing (M6). Using the influence maps below, a wave whose own strength in its cell falls below a data-defined fraction of the enemy strength in the same and neighbouring cells is ordered back to the rally point. The rule has a cooldown so a wave does not oscillate at the border.

Rules 1 to 6 ship in M5b at the single Standard difficulty. Rule 7, the personalities and the difficulty table ship in M6. How M5b implements rules 1 to 6, and where it differs from the text above (rule 3's second-Town site lies inside the Seat's first border; rule 5's rally point is 12 tiles from the Seat toward the enemy, and a wave stages outside enemy land before its assault; rule 6 defends every Town and recalls a wave only in the cases of owner decision 7), is in "M5b implementation notes" below.

## Personalities (M6)

A personality is a build order plus a handful of knobs. The three are seeded into the fun-gate games so mirror matches do not mirror.

| Personality | Build order emphasis | Wave knobs | Defensive knobs |
| --- | --- | --- | --- |
| rush | Muster Hall early, Skirmishers and Bowmen before the second Town | smaller, earlier, more frequent waves | defend radius small |
| boom | second Town and Trade line first, Masonry Age before the first wave | larger, later waves with Mangonels | defend radius large |
| tower | Watchtowers along the border, Harrying I as soon as Masonry allows, Ore for Shieldbearers | waves only after Harrying I; relies on attrition to bleed attackers | defend radius medium, extra Watchtowers on repeated incursions |

The personality id is part of `MatchSetup` so it is in the replay header and the lockstep handshake. `play-bots` selects it with `--personality-a` and `--personality-b`.

## Difficulty table (M6)

Difficulty is a cheat and the documentation says so. The bot does not play better on Hard; it receives more resources and attacks earlier. The cheat is a periodic bonus income: every `income_interval_ds` deciseconds the sim adds a bonus bundle (amounts in `difficulty.ron`) to the AI player's stockpile. A shorter interval is a larger cheat. The bonus is applied by the sim's economy step for players whose difficulty id in `MatchSetup` names one, so it is hashed, replayed and counted in `rules_hash` like every other rule. The human player receives nothing.

| Level | `income_interval_ds` | Aggression | Expected behaviour |
| --- | --- | --- | --- |
| Easy | 300 (30 s) | defensive: waves only after the Masonry Age and only at the largest wave size | lets a new player learn the loop |
| Standard | 250 (25 s) | normal wave timing | the fun-gate baseline |
| Hard | 200 (20 s) | aggressive: first wave before minute 12 (tick 14400), smaller wave size, no Masonry gate | measurable pressure for an experienced player |

The M6 acceptance requires the knob to matter: changing Hard's `income_interval_ds` from 200 to 300 (the design text says `income_interval_s` 20 to 30; the field follows the `_ds` convention) must measurably flip the Hard-versus-Easy result in `play-bots`. Record that run here when it is done (table below is the template).

| Run | Hard interval | Easy interval | Hard wins / 20 | Note |
| --- | --- | --- | --- | --- |
| baseline | 20 | 30 | (fill in at M6) | |
| flipped | 30 | 30 | (fill in at M6) | expected to drop toward 10/20 or below |

The M6 acceptance text names the file `hard.ron`; the crate layout names `data/ai/difficulty.ron`. Either one file with three entries or one file per level is acceptable as long as `data-check` validates it and `rules_hash` covers it. Decide at M6 and update both this table and [../DATA_FORMAT.md](../DATA_FORMAT.md).

## Influence maps and counter-weighted composition (M6)

`crates/sim/src/influence.rs` keeps, per player, two 32 x 32 grids of integer strength over 4 x 4 tile cells: own military strength and visible enemy military strength. Strength is the sum over units of a per-kind value from `units.ron` (a Mangonel counts more than a Skirmisher). The maps are recomputed every 10 ticks from the fog-filtered `SimView` (see [fog.md](fog.md)), so unseen enemies do not appear. They are derived state: unhashed, rebuilt in `restore()`.

The bot uses them for three decisions:

- Retreat when losing (rule 7 above).
- Wave target: among known enemy Towns, prefer the one with the lowest enemy strength in its cell and neighbours, breaking ties by lowest `BuildingId`.
- Counter-weighted composition: the bot reads the visible enemy composition by kind and weights its next training choices by the counter table in [combat.md](combat.md) (Skirmisher > Bowman, Bowman > Shieldbearer, Shieldbearer > Outrider, Outrider > Bowman/Skirmisher/Mangonel, Mangonel > buildings). Weights are integers in permille from `personalities.ron`; the choice is the highest weight with the lowest `UnitKindId` as the tie-break.

## play-bots harness

`sim-cli play-bots` runs seeded bot-versus-bot matches headlessly, one game per seed, seeds in parallel threads (`--jobs N`), each game fully independent, and prints one CSV row per game. It is the tool behind every AI acceptance line and the M6 fun gate.

```bash
cargo run -p sim-cli --release -- play-bots --seeds 1..5 --a standard --b standard --max-ticks 54000 --jobs 5
cargo run -p sim-cli --release -- play-bots --seeds 1..5 --a standard --b passive --max-ticks 36000
cargo run -p sim-cli --release -- play-bots --seeds 1..20 --a standard --b standard \
  --personality-a rush,boom,tower --personality-b rush,boom,tower --max-ticks 72000 --jobs 5
```

CSV columns:

| Column | Meaning |
| --- | --- |
| `seed` | match seed from the range |
| `winner` | `a`, `b`, or `none` when `--max-ticks` was hit before the 45-minute cap |
| `outcome` | `decisive` (capital capture) or `tiebreak` (45-minute territory percentage) |
| `tick` | tick at which the outcome was declared |
| `final_hash` | `Sim::hash()` at the end, printed as `0x` hex; two runs of one seed must match |
| `age_reached_tick` | tick at which player A reached the Masonry Age, or empty |
| `first_attack_tick` | tick of player A's first `AttackMove` into enemy borders, or empty |

Additional flags: `--faction-a`/`--faction-b` (one faction in v0.1), `--personality-a`/`--personality-b` (comma-separated list cycled across seeds). A `--summary` mode printing win rates by personality is a `good first issue`. CI runs five seeded games on ubuntu from M5b and asserts that two runs of seed 7 print identical hashes on both ubuntu and macos from M6.

### As built (M5b)

`sim-cli play-bots --seeds <A..B | N> [--a passive|standard] [--b passive|standard] [--personality-a <id,...>] [--personality-b <id,...>] [--max-ticks <N>] [--jobs <N>] [--allow-undecided]` (`crates/sim-cli/src/play_bots.rs`, the engine-free `ai::harness`). `A..B` is inclusive and `N` is `N..N`; `--a` is player A (`PlayerId(0)`) and `--b` player B, both `standard` by default; personalities default to `standard` (none for `passive`) and cycle over the seeds; `--max-ticks` defaults to the match cap (54000), which decides every game; `--jobs` defaults to 1. Each game is `MatchSetup::bots` from the standard start with the bots `ai::controller_for` builds from the header, owns its `Sim`, `Rules` and controller, and runs under `catch_unwind`, so a panic becomes a `panic` row for its seed. No `--faction` flag exists yet (one faction in v0.1) and no `--summary`.

The CSV goes to stdout in seed order and holds no wall time, so two runs of the same seeds print the same bytes whatever `--jobs` is. The seven columns above come first, unchanged in order; `winner` can also be `draw` (an exact territory tie at the cap, M5a owner decision 1) or `panic`, and `outcome` also `draw`, or empty when undecided. Diagnostics follow:

| Column | Meaning |
| --- | --- |
| `reason` | `seat_captured`, `all_towns_lost`, `surrender`, `territory`, or empty |
| `first_attack_units`, `first_attack_wains` | at `first_attack_tick`: A's non-civilian units committed to B's land (order goal, post goal, attack target or position on it), Supply Wains included, and how many of them supply |
| `second_town_tick`, `tower_tick` | first tick A owns a second complete Town, a complete Watchtower |
| `bo_done_tick_a` | first tick every non-optional step of A's build order is done (a `standard` seat) |
| `age_reached_tick_b`, `first_attack_tick_b`, `first_attack_units_b`, `first_attack_wains_b` | the same facts for B, for the side swap |
| `territory_a`, `territory_b` | owned tiles at the end |
| `units_lost_a`, `units_lost_b` | `UnitDied` events by owner |
| `min_enemy_town_hp_permille_a`, `_b` | the lowest hit points, per mille, any enemy Town reached: the pressure a side put on the other |
| `rejected_a`, `rejected_b` | `CommandRejected` events by player |
| `second_town_tick_b`, `tower_tick_b`, `bo_done_tick_b` | B's second Town, Watchtower and build order |

`first_attack_tick` is the tick (`Sim::tick()` after the step) at which one of A's units first has the `AttackMove` intent with its goal on B's land. The Standard bot attack-moves into enemy land only at a wave's assault: its stage point lies off enemy land with a margin for the group's spread, its defence attack-moves to threats on its own land, it clears defenders and focuses siege with `Attack`, holds the ring with `Move`, and recalls to its own rally. So the first such tick is the first assault order of any wave, in which every armed unit of that wave attack-moves to the target Town and its Wain, if it still has one, moves to the Wain point, all applied in one tick, and the units and Wains committed at that tick are that wave's. A wave that ends before its assault does not count: in Standard vs Standard at seed 1 (the `m5_first_match` game) P0's first wave clashes with P1's on the way and ends before its assault, and `first_attack_tick` 16993 is its second wave's assault, whose Wain was already lost (`first_attack_units` 8, `first_attack_wains` 0). Against Passive nothing stops the first wave, so there the column is the first wave's assault.

stderr gets one line per game as it ends, `game seed=1 ticks=… wall_s=… step_ms_mean=… ai_ms_mean=…` (`ai_ms_mean` is the time inside the bots' `think` per tick), and a summary, `play-bots: games=5 jobs=5 wall_s=… a=… b=… draw=… none=… panic=… decisive=… tiebreak=…`. The exit code is 0 when every game has a declared winner (`a` or `b`), 1 when any row is `draw`, `none` or `panic` (with `--allow-undecided` only a panic exits 1), and 2 for usage, rules or bot-id errors. `sim-cli record --scenario m5_first_match --seed N` records Standard against Standard at seed N to its outcome, and seed 1 is the `m5_first_match` golden.

`scripts/check_first_wave.sh [--side a|b] [--decisive] <csv>` checks ROADMAP M5b's second line on such a CSV: for every row, `age_reached_tick` is set, `age_reached_tick <= first_attack_tick < 18000`, `first_attack_units >= 8` and `first_attack_wains >= 1` (the `_b` columns with `--side b`); `--decisive` adds the gates against Passive: the side wins every game by Seat capture (owner decision 4, from either side) and, as the plan's extra gates from the ROADMAP M5b Goal, its second Town and border Watchtower stand before tick 18000. A usage error (an unknown or missing `--side` value, no CSV) exits 2, a failed row 1. CI's headless job runs it from both sides.

`sim-cli bench --units 400 --ticks 1200 --bots` measures what the bots add per tick (ROADMAP M5b's third line): two Standard seats at Masonry with 2000 of each resource and 200 units a side (half at the own rally with 2 Mangonels and a Supply Wain, half 12 tiles north of the enemy Seat, on its land), a tick-0 stream at sequence numbers from `1 << 30`. It prints `ai_ms` (the time inside `think` per tick: mean, p95, max, the plans, the slowest plan and the commands), the whole step against `control_step_ms` (the same stream with Passive seats), and `waves_launched`, `defends`, `engaged`, `rejected`, `path_queue_max` and the final hash, and exits 1 only when the bots were not exercised (fewer than 200 plans, 2 waves or 1 defence), never on the budget.

## M5b implementation notes

M5b ships rules 1 to 6 above as the Standard bot, `ai::standard::StandardBot` (`crates/ai/src/standard/`, one module per rule; each module's doc is the detailed reference), driven by `data/ai/personalities.ron` and `data/ai/build_orders/standard.ron` (schema and validators in `crates/rules/src/ai.rs`), with the owner's decisions of 2026-10-23 (ROADMAP M5b, "Notes"). At the M5b close the sim is at `SIM_VERSION` 17 and the rules at `rules_version` 24. Where it is more precise than, or differs from, the sections above:

| Topic | As implemented |
| --- | --- |
| Hashed memory | `standard::BotMemory`, encoded with postcard into the player's `Player::ai_memory` (hashed in `meta`, owner decision 1). It holds only what the view cannot give back: the `military` latch; `blocked`, the cooled-down build-order steps with their retry ticks; `site`, the footprint chosen for a `Build` step; `wave`, the active wave (its armed `units`, its `wain`, the `target` Town, the `stage` point, the `phase` `Stage`, `Assault` or `Hold`, `launched_at` and `phase_at`); `next_wave_at` and `waves_launched`; `defend`, the defend latch (`town`, `last_threat`, `since`); and `defends`. The build-order window, the worker targets, the threats and the Wain point are recomputed on every plan; the chosen `site` and the wave's `stage` point (set at launch and on a retarget) are kept in this memory until they fail or the wave ends. The default memory encodes to no bytes, and against Passive the memory stays under 1 KiB at every tick (`m5b_memory_stays_under_1_kib_vs_passive`; `max_wave_units` at most 64 bounds the roster). The controller keeps nothing else but `MapAnalysis`, built from the map's terrain and starts, the rules and the personality, so a bot rebuilt mid-match issues the same commands (`controller_rebuilt_mid_match_issues_the_same_commands`) and a restore at any tick continues exactly (`m5b_restore_at_3003_and_3007_matches_uninterrupted`, `m5b_restore_during_a_wave_matches_uninterrupted`). |
| Cadence | `think_every_ds: 5`, 10 ticks. A bot plans when `tick % 10 == player x 10 / players`: P0 on ticks 0, 10, 20 and P1 on 5, 15, 25, so two bots never plan in the same tick, and `Rules::load` requires the cadence to exceed `cmd_delay_ticks` (2), so a plan's commands apply before the next plan reads the view. An off-cadence call, and every call once the match is decided, returns before the memory is decoded. |
| Pipeline and claims | One `Ctx` per plan runs the rules in a fixed order (`standard::run_rules`): defend, build order, economy, production, waves, rally upkeep. Each rule claims the units it orders and later rules skip claimed units, so a Yeoman just chosen to build is not sent to gather in the same plan. One `Ledger` (the stock less this plan's reservations) pays for every command of the plan. |
| Caps | `max_commands_per_think: 16`; a wave's assault order and its recall wait for a plan with room for all their commands. `site_checks_per_think: 4` `check_build` probes, each a full-map search, counted by `Ctx::check_build` and made only once the ledger shows the step affordable and a candidate passed the cheap filters (own land, a free ring, no unit on the footprint). `rebalance_per_think: 2` gather moves. The busiest plan of the 6000-tick work-cap game issued 3 commands (`m5b_standard_respects_work_caps`); `bench --bots` puts the slowest plan at 1.3 to 1.6 ms and the mean at 0.033 to 0.035 ms a tick at 400 units. |
| The window, cooldowns and the Military latch | The window is the first `lookahead` (3) steps that are not done and not cooling down, and it stops at an unfinished `Wait`. A step is done when the view shows it (the table under rule 1, `build_order::satisfied`), so a lost building reopens its step. Each window step issues its command, saves up (`CannotAfford`: its cost is reserved), waits (a busy or contested trainer or host, a predecessor tech still queued, no free worker, or no probes left: nothing reserved, no cooldown), or is blocked (any other rejection, no trainer or host yet, no site): a blocked step cools down for `blocked_retry_ds` (30 s) and is then retried, never skipped for good. An `Advance` is never cooled down: it waits with no reservation while its techs are missing, then saves at the pace of its scarcest resource (`build_order::paced`), and reserves nothing while the bot defends; the plan's full reservation had starved the army for minutes before the Charter Age. The `Military` latch is set once every non-optional step before the `Military` step is done and is never cleared, so harassment that reopens an earlier step does not stop production; production also runs while the defend latch is set, before the latch. |
| Sites | `NearTown` (rings 4 to 12 tiles out around an own complete, uncontested Town, the Seat first, or for a kind with `max_per_town` the Town with the fewest under the limit), `Terrain` (beside the kind's terrain, most adjacent tiles first), `Expansion` and `BorderEdge` (`sites.rs`). `Expansion` candidates lie wholly inside the Seat's first border with their centre at least `expansion_min_tiles` (14) out, scored by the Forest and Mountain tiles within `resource_radius_tiles` (20) outside that border: P0's second Town goes at (18,81) and P1's at (106,81), the lowest tile index breaking equal scores on both sides. The border Watchtower goes on the owned tile nearest the enemy start that keeps a free ring: (43,64) for P0, (84,64) for P1. A remembered site is tried first, two sites chosen in one plan keep their gap, and a `BorderEdge` step and the site keeper back off from enemy military near the site, so a lost border Watchtower is not rebuilt in front of the army that took it. |
| Siege-gated waves | A launch needs the Masonry Age (`wave_requires_age`), `next_wave_at` (`wave_interval_ds`, 3 min after the last launch), no defence blocking launches, `size(tick)` home armed units assembled within `assemble_radius_tiles` (8) of the rally with at least `min_siege` (1) Mangonel among them, and a Supply Wain at home. `wave_sizes` asks for 8 armed units, 12 from minute 15, 16 from minute 20 and 24 from minute 25, never more than the home armed units plus the free pop. Every assembled armed unit goes, siege first, at most `max_wave_units` (40), plus one Wain; at a full pop one armed unit stays home. The siege gate is there because the line kinds deal 250 per mille of their damage to buildings while a 2000 HP Town shoots back, and the Mangonel (4000 per mille against buildings, range 8.0) outranges the Town and the Watchtower (7.0). Against Passive the first wave assaults at tick 13393 as P0 and 13368 as P1 with 11 armed units and the Wain (`first_attack_units` 12, `first_attack_wains` 1). |
| Staging and the Wain point | The stage point S is the first tile, walking from the target's centre toward the rally, with no other player's land and no enemy shooter's range within `stage_margin_tiles` (2) plus the group's spread, snapped to passable. The wave attack-moves there and assaults once `stage_ready_permille` (750) of its armed units and every siege unit stand within `stage_radius_tiles` (6) of S, or `stage_max_ds` (60 s) after the stage began. In that one plan every armed unit attack-moves to the target and the Wain moves to W: the passable tile of the segment from the target to S nearest the target that lies more than `wain_standoff_tiles` (3) beyond every enemy shooter's range, else S. W is recomputed every plan, so it moves up when a Watchtower falls or the target reaches 0 HP, and the Wain is moved back when it is more than `wain_leash_tiles` (6) from W. |
| Assault, ring hold, continue and the end of a wave | Siege within `siege_focus_tiles` (12) of the target gets an explicit `Attack` on it. At 0 HP up to `ring_units` (4) Infantry or Cavalry take distinct passable ring tiles with single-unit `Move`s (M5a's annex-hold rule keeps them there) and up to `clear_defenders_units` (4) others `Attack` the defenders within the annexation defender radius plus 2. After a capture a wave of at least `continue_min_units` (4) armed units retargets the next enemy Town with a fresh stage point and timer. A wave with no unit that can annex, fewer than `recall_below_units` (3) armed units, or older than `wave_max_ds` (6 min) while no annexation of its own runs at the target comes home. From `all_in_after_ds` (minute 30) every home armed unit joins the wave, a launch needs no siege, Wain or interval, and the wave is never recalled. In `m5_first_match` P0's fourth wave takes P1's Seat at tick 26012, before the all-in. |
| Defend scope and recall | Owner decision 7, which replaces rule 6's "the Seat only, every unit recalled". A threat is an enemy non-civilian unit on own land within `defend_radius_tiles` (16) of the centre of any own complete Town (around a contested Town, whose land went neutral, any enemy military in the radius); the engaged Town is the one with the most threatening pop, the Seat first. The home guard attack-moves to the threatening unit nearest each guard, so a parked raider is killed rather than watched, and enemy siege gets an explicit `Attack` from up to 4 guards. The latch is released after `defend_release_ds` (15 s) without a threat, and the guard attack-moves back to the rally. A defence blocks launches for at most `defend_max_block_ds` (60 s); past that, while a launch is due, the guard shrinks to the units nearest the threat until their pop reaches `defend_guard_pop_ratio` (2) times the threat's, and the wave launches with the rest. A staging wave is recalled only while the defence still blocks launches, any wave when a threatened Town is contested or at or below `recall_wave_town_hp_permille` (500) of its hit points, and none during the all-in. |
| CSV additions and `first_attack_*` | The seven columns above come first and unchanged; `winner` adds `draw` and `panic`, and the diagnostics follow (the table under "As built (M5b)"). `first_attack_tick` is the tick, `Sim::tick()` after the step, at which one of A's units first has the `AttackMove` intent with its goal on B's land; for the Standard bot that is a wave's assault order, and a wave that ends before its assault does not count. `first_attack_units` counts A's non-civilian units committed to B's land at that tick (order goal, post goal, attack target or position on it), Supply Wains included, and `first_attack_wains` those of them that supply. |
| Open question 1 | Answered by owner decision 1: the timers and latches are hashed sim state (`Player::ai_memory`) and the build-order cursor is re-derived every plan, so the bot needs no `snapshot()`/`restore()` of its own; see "Where the bot runs" and [../DETERMINISM.md](../DETERMINISM.md), "What M5b adds". |
| Versioning | Owner decision 2: from `SIM_VERSION` 17 a `crates/ai` change that can alter a decision bumps `SIM_VERSION`, and because `m5_first_match` (the Standard bot in both seats) then changes its hash, the fixture policy forces the `rules_version` bump and a re-record in the same commit. A `data/ai/` edit is a `rules_version` bump and `just rerecord`, as for any RON file (CONTRIBUTING.md, "The data-first path"). |
| Randomness | None. The bot never draws: ties go to the lowest `UnitId`, `BuildingId`, `UnitKindId` or tile index. Standard against Standard is therefore close to one trajectory whatever the seed: every seed of acceptance 1 ends in P0's Seat capture at tick 26012; a seed changes only combat's tie draws, and the final hashes differ because the seed itself is hashed in `meta`. |

M6 notes. Each M6 item has a seam in this code:

- **Influence maps.** The design above keeps them in `crates/sim/src/influence.rs` as an unhashed cache refreshed every 10 ticks and rebuilt in `restore()`. That cache is not restore-exact: restored between two refreshes, it is rebuilt from the current state while the uninterrupted run still reads the last refresh, so the bot's next decision can differ. M5b's recommendation is to compute them in `crates/ai` from `Facts` on the bot's own plan ticks, a pure function of the view; the other way is a sim-side map that is hashed state. Decide at M6 (open questions below; [../DETERMINISM.md](../DETERMINISM.md), "Derived caches").
- **Variety without a generator.** If a personality needs random choices, the sim can expose a stateless `SimView::ai_noise(player, salt) -> u64`, a hash of the seed, the tick, the player and the salt, that never draws from the sim's `Pcg32`, so a bot's draw cannot shift combat's tie draws.
- **Personalities** are rows in `personalities.ron` plus `build_orders/*.ron` files, with no Rust change (`play-bots --personality-a/-b` already cycles them over the seeds); #48 is the first one.
- **Difficulty** keys `difficulty.ron` by `AiSeat.bot`, which every replay header already carries; the income cheat is the sim economy step's (a `SIM_VERSION` bump), and the wave knobs it scales are personality knobs.
- **Retreat** slots in between defend and waves in `standard::run_rules`, with a cooldown field appended to `BotMemory` (a `crates/ai` change, so a `SIM_VERSION` bump).
- **Wave target and composition**: `waves::choose_target` is the one function influence maps replace; counter-weighted composition replaces the deficit share in `army::produce`.

## The M6 fun gate

The gate is quoted verbatim from the design decision "Human fun gate" so that no tuning pass can quietly weaken it:

> M6 cannot close until: 20 seeded Standard-vs-Standard bot games (bots seeded with distinct personalities rush/boom/tower so mirrors do not mirror) all end with a declared winner where territory-tiebreak wins are valid; >= 10/20 are DECISIVE (capital capture) before tick 48000 (40 min); median game length <= 45 min; difficulty ordering holds (>= 15/20); three human-vs-Standard matches logged in docs/PLAYTEST.md with duration 20-40 min, in >= 2 of 3 the AI's army reaches the player's borders and annexes or reduces a Town below 50% HP, the Yield Cap readout turns amber at least once, owner fun rating >= 3/5. A data-driven late-game pressure knob (Charter Age Harrying bonus and optional Yield Cap decay after minute 30, both in rules.ron) exists for tuning; 2-3 tuning iterations are budgeted, not one.

The 45-minute cap with the territory tiebreak makes "no stalemate" structural: every game ends. The decisive ratio and the median length are what the tuning iterations work on. The human matches are logged in [../PLAYTEST.md](../PLAYTEST.md).

## Late-game pressure knobs

Both knobs are data in `rules.ron` and exist so that tuning never needs Rust changes:

| Knob | Effect | Default |
| --- | --- | --- |
| Charter Age Harrying bonus | raises attrition damage for a player in the Charter Age (see [attrition.md](attrition.md)) | set at M5a; present from the first `rules.ron` that has ages. Implemented in the M5a attrition slice (`harrying_bonus_age`, `harrying_bonus_hp`), shipped at 0 until M6 |
| Yield Cap decay after minute 30 | optional: lowers every player's Yield Cap by a data-defined step per interval after tick 36000, so a turtled economy stops growing | off (0) until a fun-gate iteration turns it on. Implemented in the M5a annexation slice (`yield_cap_decay_after_ds`, `yield_cap_decay_every_ds`, `yield_cap_decay_step`; see [economy.md](economy.md), "Yield Cap"), shipped with step 0 |

Turning a knob on is a `rules_version` bump with a one-line reason and a `play-bots` table in the PR.

## Interactions with other systems

| System | Interaction |
| --- | --- |
| Economy ([economy.md](economy.md)) | the bot respects the Yield Cap when assigning workers; the difficulty cheat is applied by the economy step |
| Territory ([territory.md](territory.md)) | Town and Watchtower sites must satisfy `is_buildable`; rejected `Build` commands return `OutsideBorders` |
| Attrition ([attrition.md](attrition.md)) | one Supply Wain per wave; the tower personality researches Harrying early |
| Towns ([towns.md](towns.md)) | waves target Towns; annexation needs adjacent infantry or cavalry and no defender within 6 tiles for 60 s |
| Fog ([fog.md](fog.md)) | the bot sees only the filtered view; influence maps inherit the filter |
| Pathing ([pathing.md](pathing.md)) | bot commands share the human's A* budget |
| Replays ([../DETERMINISM.md](../DETERMINISM.md)) | bot commands are regenerated, never recorded; difficulty and personality ids ride in `MatchSetup` |

## Acceptance checklist

Copied from the M5b and M6 milestones in [../ROADMAP.md](../ROADMAP.md).

M5b:

- [x] `cargo run -p sim-cli --release -- play-bots --seeds 1..5 --a standard --b standard --max-ticks 54000 --jobs 5` ends all 5 games with a declared winner, no panics, under 120 s wall time on the dev Mac (budget derived as ticks x measured M4a mean ms / 1000 x 1.5, recorded in BUILD_TIMES.md), and prints per-seed final hashes identical across two runs; seed 1 is committed as fixtures/m5_first_match.eonreplay and verified in both CI OS jobs.

  Observed on PR #47, 2026-10-26, on the 4-vCPU Intel Xeon 2.1 GHz Linux VM, not the dev Mac (M5b owner decision 8: the owner re-runs it on the Mac): exit 0 both times, shell wall 2.46 s and 2.61 s, `play-bots: games=5 jobs=5 wall_s=2.07 a=5 b=0 draw=0 none=0 panic=0 decisive=5 tiebreak=0` (second run `wall_s=2.28`); every game `a,decisive,26012,...,seat_captured`, one (winner, tick) pair; `diff` of the two CSVs empty; seed 1's `final_hash` `0x3d2552033d51b4a2` equals `m5_first_match.hash`, and `sim-cli verify` of the fixture prints `OK final_hash=0x3d2552033d51b4a2 ticks=26012`. CI's check (macOS) and headless (Linux) jobs run `sim-cli verify --release` on it and hash-parity diffs their two lines (`.github/workflows/ci.yml`), and PR #47's Actions run 40258854764 at head 15de2fe passed: both jobs printed `OK final_hash=0x3d2552033d51b4a2 ticks=26012` and the parity `diff` was empty (pasted on docs/ROADMAP.md, M5b line 1, with the rest of the output).

- [x] `play-bots --seeds 1..5 --a standard --b passive --max-ticks 36000` shows the Standard bot reaching Masonry Age and launching an attack with >= 8 units including 1 Supply Wain before tick 18000 in 5/5 seeds (CSV columns age_reached_tick, first_attack_tick).

  Observed on PR #47, 2026-10-26: exit 0, every seed `age_reached_tick=7933 first_attack_tick=13393 first_attack_units=12 first_attack_wains=1` and a Seat capture at tick 15629; `scripts/check_first_wave.sh` prints `5/5 rows pass (side a)` (with `--decisive` too). Side swap (`--a passive --b standard`): 7948 / 13368 / 12 / 1, capture at 15613, `5/5 rows pass (side b)`.

- [x] `bench --units 400 --ticks 1200 --bots` shows the AI adds < 0.5 ms mean per tick.

  Observed on PR #47, 2026-10-26, three runs at seed 1 on the VM: `ai_ms mean` 0.0332 / 0.0351 / 0.0346 ms (p95 0.206 / 0.280 / 0.217, `plans=240`, `plan_ms_max` 1.60 / 1.28 / 1.37), `waves_launched=2 defends=2`, exit 0 each.

- [ ] [owner] From the window: New Game starts a match vs the Standard bot on plains_1v1; the owner plays to a victory or defeat overlay in one sitting; the auto-recorded replay verifies to the hash shown on the overlay.

  Its automated proxy (M5b owner decision 8) is `just m5b-checks`: the overlay of a `--spectate --seed 1` run fast-forwarded to 400 ticks before `m5_first_match`'s outcome, its replay verifying to the golden's hash, the main menu and a New Game click whose replay verifies with the Standard bot rebuilt from its header ([../PLAYTEST.md](../PLAYTEST.md#automated-proxies-m5b), [ui.md](ui.md#main-menu-m5b)).

M6:

- [ ] `cargo run -p sim-cli --release -- play-bots --seeds 1..20 --a standard --b standard --personality-a rush,boom,tower --personality-b rush,boom,tower --max-ticks 72000 --jobs 5` exits 0 with a winner in all 20 games; >= 10/20 are decisive (capital capture) before tick 48000; median game length <= tick 54000; >= 10/20 last past tick 24000.
- [ ] `play-bots --a easy --b standard --seeds 1..20` gives Standard >= 15/20 wins; `--a standard --b hard` gives Hard >= 15/20 wins.
- [ ] Two `play-bots --seeds 7..7` runs print identical final hashes; CI asserts this on ubuntu and macos.
- [ ] Selecting Hard yields an AI attack on the player's borders before game minute 12 in 3 of 3 seeded games (asserted headlessly via first_attack_tick < 14400 with a passive opponent); changing hard.ron income_interval_s from 20 to 30 measurably flips the hard-vs-easy result (documented in docs/design/ai.md).
- [ ] Game speed x4 and pause work and the replay verifies with the same final hash as the x1 replay of the same scripted input (`--scenario scripted_match --speed 4`).
- [ ] [owner] Three complete human-vs-Standard matches logged in docs/PLAYTEST.md with duration 20-40 min each; in >= 2 of 3 the AI's army reaches the player's borders and annexes or reduces a Town below 50% HP; the Yield Cap readout turns amber at least once; owner fun rating >= 3/5; each failed criterion triggers a rules.ron tuning commit and a replay of the three matches before M7 starts (max 3 iterations before escalating to an ADR on the ruleset).

## Open questions

- `AiController` has no snapshot/restore. If a feature ever restores a match with bots mid-way (save/load is out of scope for v0.1), either add `snapshot()`/`restore()` to the trait or move the bot's cursor and timers into hashed sim state. Record the decision in [../DETERMINISM.md](../DETERMINISM.md). Answered in M5b (owner decision 1 of 2026-10-23): the timers and latches are hashed sim state, `Player::ai_memory`, and the cursor is re-derived every plan (see "Where the bot runs" and DETERMINISM.md, "What M5b adds").
- The difficulty cheat's bonus bundle amounts per interval are not fixed by the design; set them at M6 so that Standard beats Easy and Hard beats Standard at the 15/20 bar, then record them in this document's difficulty table.
- Whether the enemy Seat's position may be assumed from the mirrored map (as above) or must be scouted. Assuming it is simpler and the map is symmetric; a second map would need a scouting rule.
- One difficulty file or one file per level (`hard.ron`). Pick at M6.
- Whether `think` needs a random draw from the sim's `Pcg32` for site selection variety. Not needed for v0.1; if it becomes needed, the sim exposes the value through `SimView` rather than the bot owning a generator. M5b's suggestion for M6 is a stateless `SimView::ai_noise(player, salt)` that hashes the seed, the tick, the player and the salt and never draws from the `Pcg32` ("M5b implementation notes").
- Where the influence maps live (M6). As designed above they are an unhashed sim cache refreshed every 10 ticks and rebuilt in `restore()`, which is not restore-exact between refreshes; M5b recommends computing them in `crates/ai` from the view on the bot's plan ticks, or else making them hashed state. Decide at M6 and record it in [../DETERMINISM.md](../DETERMINISM.md), "Derived caches".
