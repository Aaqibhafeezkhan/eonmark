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
trait AiController {
    fn think(&mut self, player: PlayerId, view: &SimView<'_>) -> Vec<Command>;
}
```

The controller is injected at `Sim::new(setup, rules, Box<dyn AiController>)` and called from inside `Sim::step`, once per AI player per tick, in `PlayerId` order, before that tick's queued commands are applied. The commands it returns are stamped with the AI's `PlayerId` and a per-player sequence number and enter the same delayed queue as human commands: issued at tick N, applied at tick N + `cmd_delay_ticks`. They are validated like any other command and may be rejected with `SimEvent::CommandRejected`; the bot must tolerate rejection and must not loop on it.

Because the bot runs inside the step, its commands are not written to the replay. A replay holds the human's commands only; re-simulating it re-runs the bot and reproduces its decisions exactly. This is what lets bot-vs-bot games be verified by `sim-cli verify` and what keeps the future lockstep path simple (see [../DETERMINISM.md](../DETERMINISM.md)).

`ai::Passive` is the controller that returns an empty `Vec`. Tests, benches and the M0 skeleton use it.

Determinism constraints on the crate (enforced by `crates/ai/clippy.toml` and CI): no `f32`/`f64`, no `HashMap`/`HashSet`, no `Instant`/`SystemTime`, no `rand::random`/`thread_rng`, no engine crates. Ties are broken by the lowest id or lowest tile index. The design permits randomness only from the single `Pcg32` inside the sim; `think` receives a shared `SimView`, so if a later rule needs a random draw the sim must expose one through the view (for example a per-tick `u64` drawn before `think` runs). No v0.1 rule needs it.

Bot state. The controller may keep private state across ticks (build-order cursor, wave timers, last known enemy positions). `AiController` has no snapshot or restore method, so this state is not captured by `Sim::snapshot()`. In v0.1 nothing restores a match with bots mid-way (fixtures are verified from tick 0), but the bot should still derive what it can from `SimView` each call, so that the private state stays small. See open questions.

Cadence. `think` is called every tick. Keeping under 0.5 ms mean means the bot does its planning on a cadence rather than every call; the natural cadence is the 10-tick influence-map refresh. The cadence and any per-rule timers are data in `difficulty.ron` or `personalities.ron`, not constants in Rust.

## Architecture of the scripted bot (M5b)

The bot is a small set of rules evaluated in a fixed order. Each rule reads `SimView`, may emit commands, and may mark resources as reserved for the rest of this call so a later rule does not spend them.

1. Build-order executor. `data/ai/build_orders/<name>.ron` is an ordered list of steps: build a kind, train a kind, research a tech, advance age, found a Town, or wait until a condition (stockpile, unit count, tick). The executor keeps a cursor and advances when the current step is observed complete in the view (the building exists, the unit count is reached, the tech is known). It does not advance on command emission, so a rejected command is simply retried. `standard.ron` carries the Standard bot from the Hearth Age start (1 Town, 5 Yeomen, 150 Grain, 150 Lumber) to the Masonry Age. A `boom` variant is a `good first issue`.
2. Worker balancing. Each build-order stage names a target Yeoman count and a target split across Grain, Lumber and Ore. The rule trains Yeomen toward the count (respecting the Yield Cap: it stops adding gatherers to a resource whose rate already sits at the cap, see [economy.md](economy.md)) and issues `Gather` commands to move idle or wrongly assigned Yeomen toward the split. It relies on the sim's own idle-worker seeking for the rest.
3. Second Town. Once Statecraft I raises the Town limit (see [tech-and-ages.md](tech-and-ages.md)), the rule picks a site: an owned, buildable tile as far from the Seat as the border allows, in the direction of the most unclaimed resource tiles, and sends a Yeoman to found a Town there. The new Town extends the territory field (see [territory.md](territory.md)).
4. Watchtower at the border edge. The rule finds the owned buildable tile closest to the enemy Seat (the enemy Seat's position is known from the map's mirrored start, not from scouting) and builds a Watchtower there. The Watchtower pushes the border two tiles, gives vision, shoots, and hosts Harrying research, which the tower personality researches early (see [attrition.md](attrition.md)).
5. Timed attack waves. From the build order's "military phase" onward the rule accumulates an army at a rally point near the border Watchtower. When the army reaches the wave size for the current difficulty and personality and the wave timer has elapsed, it issues `AttackMove` toward the nearest known enemy Town, with exactly one Supply Wain attached so the wave does not wither under attrition inside enemy borders. Waves target Towns, not units: the win condition is capital capture.
6. Defend the Seat. If any visible enemy military unit is inside the bot's own borders within a data-defined radius of its Seat, every military unit the bot owns is ordered to `AttackMove` to the Seat, the wave timer is paused, and Yeomen keep working. The rule releases when no enemy military has been visible there for a data-defined number of ticks.
7. Retreat when losing (M6). Using the influence maps below, a wave whose own strength in its cell falls below a data-defined fraction of the enemy strength in the same and neighbouring cells is ordered back to the rally point. The rule has a cooldown so a wave does not oscillate at the border.

Rules 1 to 6 ship in M5b at the single Standard difficulty. Rule 7, the personalities and the difficulty table ship in M6.

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

## The M6 fun gate

The gate is quoted verbatim from the design decision "Human fun gate" so that no tuning pass can quietly weaken it:

> M6 cannot close until: 20 seeded Standard-vs-Standard bot games (bots seeded with distinct personalities rush/boom/tower so mirrors do not mirror) all end with a declared winner where territory-tiebreak wins are valid; >= 10/20 are DECISIVE (capital capture) before tick 48000 (40 min); median game length <= 45 min; difficulty ordering holds (>= 15/20); three human-vs-Standard matches logged in docs/PLAYTEST.md with duration 20-40 min, in >= 2 of 3 the AI's army reaches the player's borders and annexes or reduces a Town below 50% HP, the Yield Cap readout turns amber at least once, owner fun rating >= 3/5. A data-driven late-game pressure knob (Charter Age Harrying bonus and optional Yield Cap decay after minute 30, both in rules.ron) exists for tuning; 2-3 tuning iterations are budgeted, not one.

The 45-minute cap with the territory tiebreak makes "no stalemate" structural: every game ends. The decisive ratio and the median length are what the tuning iterations work on. The human matches are logged in [../PLAYTEST.md](../PLAYTEST.md).

## Late-game pressure knobs

Both knobs are data in `rules.ron` and exist so that tuning never needs Rust changes:

| Knob | Effect | Default |
| --- | --- | --- |
| Charter Age Harrying bonus | raises attrition damage for a player in the Charter Age (see [attrition.md](attrition.md)) | set at M5a; present from the first `rules.ron` that has ages |
| Yield Cap decay after minute 30 | optional: lowers every player's Yield Cap by a data-defined step per interval after tick 36000, so a turtled economy stops growing | off (0) until a fun-gate iteration turns it on |

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

- [ ] `cargo run -p sim-cli --release -- play-bots --seeds 1..5 --a standard --b standard --max-ticks 54000 --jobs 5` ends all 5 games with a declared winner, no panics, under 120 s wall time on the dev Mac (budget derived as ticks x measured M4a mean ms / 1000 x 1.5, recorded in BUILD_TIMES.md), and prints per-seed final hashes identical across two runs; seed 1 is committed as fixtures/m5_first_match.eonreplay and verified in both CI OS jobs.
- [ ] `play-bots --seeds 1..5 --a standard --b passive --max-ticks 36000` shows the Standard bot reaching Masonry Age and launching an attack with >= 8 units including 1 Supply Wain before tick 18000 in 5/5 seeds (CSV columns age_reached_tick, first_attack_tick).
- [ ] `bench --units 400 --ticks 1200 --bots` shows the AI adds < 0.5 ms mean per tick.
- [ ] [owner] From the window: New Game starts a match vs the Standard bot on plains_1v1; the owner plays to a victory or defeat overlay in one sitting; the auto-recorded replay verifies to the hash shown on the overlay.

M6:

- [ ] `cargo run -p sim-cli --release -- play-bots --seeds 1..20 --a standard --b standard --personality-a rush,boom,tower --personality-b rush,boom,tower --max-ticks 72000 --jobs 5` exits 0 with a winner in all 20 games; >= 10/20 are decisive (capital capture) before tick 48000; median game length <= tick 54000; >= 10/20 last past tick 24000.
- [ ] `play-bots --a easy --b standard --seeds 1..20` gives Standard >= 15/20 wins; `--a standard --b hard` gives Hard >= 15/20 wins.
- [ ] Two `play-bots --seeds 7..7` runs print identical final hashes; CI asserts this on ubuntu and macos.
- [ ] Selecting Hard yields an AI attack on the player's borders before game minute 12 in 3 of 3 seeded games (asserted headlessly via first_attack_tick < 14400 with a passive opponent); changing hard.ron income_interval_s from 20 to 30 measurably flips the hard-vs-easy result (documented in docs/design/ai.md).
- [ ] Game speed x4 and pause work and the replay verifies with the same final hash as the x1 replay of the same scripted input (`--scenario scripted_match --speed 4`).
- [ ] [owner] Three complete human-vs-Standard matches logged in docs/PLAYTEST.md with duration 20-40 min each; in >= 2 of 3 the AI's army reaches the player's borders and annexes or reduces a Town below 50% HP; the Yield Cap readout turns amber at least once; owner fun rating >= 3/5; each failed criterion triggers a rules.ron tuning commit and a replay of the three matches before M7 starts (max 3 iterations before escalating to an ADR on the ruleset).

## Open questions

- `AiController` has no snapshot/restore. If a feature ever restores a match with bots mid-way (save/load is out of scope for v0.1), either add `snapshot()`/`restore()` to the trait or move the bot's cursor and timers into hashed sim state. Record the decision in [../DETERMINISM.md](../DETERMINISM.md).
- The difficulty cheat's bonus bundle amounts per interval are not fixed by the design; set them at M6 so that Standard beats Easy and Hard beats Standard at the 15/20 bar, then record them in this document's difficulty table.
- Whether the enemy Seat's position may be assumed from the mirrored map (as above) or must be scouted. Assuming it is simpler and the map is symmetric; a second map would need a scouting rule.
- One difficulty file or one file per level (`hard.ron`). Pick at M6.
- Whether `think` needs a random draw from the sim's `Pcg32` for site selection variety. Not needed for v0.1; if it becomes needed, the sim exposes the value through `SimView` rather than the bot owning a generator.
