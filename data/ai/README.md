# data/ai/

This directory holds the scripted opponent's data: the personalities and
their build orders (M5b), and later the difficulty table (M6). The schema
lives in `crates/rules/src/ai.rs`; `rules::Rules::load` reads this directory
last, after the maps, validates it against everything before it and attaches
it as `Rules::ai`, so it is part of `rules_hash`. An edit here without a
`rules_version` bump makes `sim-cli verify` report `RULES CHANGED since
recording`; the data-only path is the one in
[`../rules/README.md`](../rules/README.md) ("Worked example"): edit, bump
`rules_version`, `just rerecord`, run the tests; for a build order or a
personality, [`../../CONTRIBUTING.md`](../../CONTRIBUTING.md) ("A build order
or personality") adds the `play-bots` comparison. `sim-cli data-check` prints
the counts as `personalities=1 build_orders=1`.

| Path | Contents | Arrives |
|------|----------|---------|
| `personalities.ron` | One row per personality: its build order and every knob of the bot's rules | M5b (`standard`); M6 adds rush, boom and tower |
| `build_orders/<id>.ron` | One build order per file; the `id` must equal the file stem. Non-`.ron` files here are ignored | M5b (`standard.ron`) |
| `difficulty.ron` | Easy, Standard, Hard: the bot income interval (documented as a cheat) and aggression | M6 |

The bots themselves are in `crates/ai`: `ai::Passive` (never acts), the
data-driven `ai::standard::StandardBot`, and `ai::Scripted` (a fixed command
list for `sim-cli selftest`). A match header names each AI slot's bot and
personality (`MatchSetup.ai_seats`, for example `standard` with personality
`standard`), and `ai::controller_for` builds the bots from it.

Rule for every bot: it is a deterministic function of `sim::SimView` and its
own hashed memory (`Player::ai_memory`). No clocks, no floats, no randomness
of its own; `crates/ai/clippy.toml` enforces this.

## Units

Integers only, as everywhere under `data/`: fields ending `_ds` are
deciseconds (converted to ticks once by `Rules::ticks_from_ds`), `_tiles`
tiles, `_permille` per mille.

## Build orders (`build_orders/<id>.ron`)

```ron
(
  id: "standard",
  steps: [
    Workers(count: 7, split: {"grain": 3, "lumber": 4}),
    Build(kind: "croft", count: 2, at: NearTown),
    Military,
    Research(tech: "arms_1"),
    Advance(age: "masonry"),
    // ...
  ],
)
```

The bot keeps satisfying the steps from the top. A step is done when the
view shows it, never when its command was issued, so a rejected command is
simply tried again:

| Step | Done when | Notes |
|------|-----------|-------|
| `Workers(count, split)` | workers (the kind the Town trains that builds: the Yeoman) alive plus queued reach `count` | `split` weights per resource id: at least one entry, each at least 1, summing to at most `count`, every resource gathered by the worker kind. The latest `Workers` step at or before the window's first step sets the gather split |
| `Build(kind, count, at, optional)` | own buildings of `kind`, sites included, reach `count` (the Seat counts for the Town kind) | `at`: `NearTown` (around an own Town), `Terrain` (beside the resource terrain; only kinds with `Adjacent` slots), `Expansion` (a second Town; only the Town kind), `BorderEdge` (the owned tile nearest the enemy start; only kinds with `border_radius_tiles`) |
| `Train(kind, count, optional)` | units of `kind` alive plus queued reach `count` | |
| `Research(tech, optional)` | `tech` is researched or queued | at a building of the tech's host kind, so `harrying_1` goes to the Watchtower |
| `Advance(age)` | the age is reached | never optional |
| `Military` | always | crossing it starts army production for good |
| `Wait(until_ds)` | the match time reaches `until_ds` | the window stops at it until then |

`count` is 1 to 200 and a build order has 1 to 64 steps. Static
reachability is checked at load so a build order cannot stall for ever: a
`Build`, `Train` or `Research` step must be marked `optional: true` when no
earlier `Advance` (or the start age) unlocks it, when no earlier
non-optional step builds its trainer or host (the Seat counts as built),
when no earlier non-optional step researches its prerequisite (the level
below in its line, or a unit's `requires_tech`), or when its `count` is more
than `max_per_town` times the Towns the earlier non-optional steps build.
An `Advance` is never optional: its age must be later than the age before
it, and the earlier non-optional `Research` steps must cover its
`techs_required` techs that count toward an age.

How the Standard bot works through the steps (`crates/ai/src/standard/`,
from M5b's economy slice). Every plan (`think_every_ds`) it takes the
**window**: the first `lookahead` steps that are not done and not cooling
down, in order. For each, within the plan's caps:

- it **issues** the step's command when the sim's own check accepts it and
  the plan's stock, less what earlier steps of the plan spent or reserved,
  pays for it: a worker or unit at the free trainer with the shortest
  queue (at most `max_queue_per_building` entries), a tech at a free host
  of its kind, the age, or a building at a site with a builder (one new
  site per step and plan);
- it **saves up** when the stock is short: the step's cost is reserved,
  so later steps and army production cannot spend it. An `Advance` saves
  up only once the techs it needs are researched, and at the pace of its
  scarcest resource: each resource of its cost is held at the share of
  that cost the scarcest one has reached (with 4 of the Charter's 300
  Lore in stock it holds 6 of its 500 Grain), so the surplus of the
  others still pays for the army while the scarce one comes in, and the
  whole cost is held once every resource has reached it. While the bot
  defends an `Advance` keeps its place but reserves nothing, so the stock
  goes to the army first;
- it **waits** while every trainer or host of the step is busy or stands
  in a contested Town, a predecessor tech is still queued, every worker is
  busy, the plan's `check_build` probes ran out, or (an `Advance`) the
  techs it needs are still missing;
- it is **blocked** for any other reason (no trainer or host built yet,
  the age not reached, no site (a `NearTown` building with no uncontested
  own Town to go beside has none), the pop cap, or, for a `BorderEdge`
  building, enemy military near the site found: any other player's armed
  unit within the farthest reach of any unit kind, `acquire_range_tiles`
  or its longest weapon range, plus `stage_margin_tiles`; a border edge
  site faces the enemy outside the defence's radius): it reserves nothing
  and leaves the window for `blocked_retry_ds`, then is tried again. It
  never becomes a permanent skip, and an `Advance` is never blocked.

Builders are idle workers, or, when none is idle, the gatherers of the
resource most over its share. Sites come from the `at` policy: `NearTown`
walks rings `near_town_min_tiles..=near_town_max_tiles` around the own
Town with the fewest of the kind (Seat first); `Terrain` the static spots
beside the kind's resource terrain, most resource tiles first;
`Expansion` the static Town spots wholly inside the Seat's first border
and at least `expansion_min_tiles` out, scored by the Forest and Mountain
tiles within `resource_radius_tiles` (20, the radius of a new Town's
border) outside the first border, less `toward_enemy_penalty` per tile
gained toward the enemy; `BorderEdge` the own grass tile nearest the enemy
start, `tower_spacing_tiles` from other border buildings. Every site keeps
a passable ring of `building_gap_tiles` free of other buildings, the sites
chosen earlier in the same plan included (a yard or pit may touch its
resource), has no unit on it, and passes `check_build`. A site without a
builder gets one again (a free resume), unless it is a border
building's (a kind with `border_radius_tiles`, the Watchtower) and enemy
military stands that near it. Crossing the `Military` step
(every non-optional step before it done) latches army production for
good; a defence also starts it. Production keeps the army near the
`army_mix` weights: each free trainer trains the kind it can train that
is most under its share, and a kind over its share of the kinds the bot
can field at all (a trainer of it standing, free or busy, and the stock
paying for it) is never trained, so a busy Muster Hall does not let the
Engine Yard fill the army with Mangonels.

## Personalities (`personalities.ron`)

```ron
(
  personalities: [
    (id: "standard", name: "Standard", build_order: "standard", think_every_ds: 5, ...),
  ],
)
```

| Field | Meaning | Range |
|-------|---------|-------|
| `id`, `name` | Stable id (the header's `personality`) and display name | unique, non-empty id |
| `build_order` | A file stem under `build_orders/` | must exist |
| `think_every_ds` | The bot plans every this many deciseconds | 1 to 600; in ticks more than `cmd_delay_ticks`, so a plan's commands apply before the next plan reads the view |
| `lookahead`, `max_commands_per_think`, `site_checks_per_think`, `rebalance_per_think` | Build-order steps worked at once; commands, `check_build` probes (a full-map search each) and gatherer moves per plan | 1 to 64 |
| `blocked_retry_ds` | A step blocked for a reason other than cost is retried after this long | 1 to 72000 |
| `building_gap_tiles`, `near_town_min_tiles`, `near_town_max_tiles`, `expansion_min_tiles`, `resource_radius_tiles`, `border_scan_tiles`, `tower_spacing_tiles` | Site policies: the free ring around every footprint, the `NearTown` ring band, the second Town's distance and resource radius, the scan radius of terrain and border sites, the spacing of border Watchtowers | 1 to 64; min at most max |
| `toward_enemy_penalty` | Expansion score lost per tile gained toward the enemy | 0 to 1000 |
| `army_mix` | `[(kind, weight)]`: the army's composition | non-empty; armed, non-civilian kinds, each once; weights 1 to 1000 |
| `supply_kind` | The kind each wave takes along against attrition | a kind that `supplies` |
| `max_queue_per_building` | Entries the bot keeps queued per building (1 keeps the military ramp at base cost) | 1 to 64, at most `train_queue_max` |
| `rally_tiles_from_seat`, `assemble_radius_tiles`, `goal_tolerance_tiles` | The rally point's distance from the Seat toward the enemy, the radius in which units count as assembled, and how close an order's goal must be to count as already given | 1 to 64 |
| `wave_requires_age`, `first_wave_not_before_ds` | No wave before this age (`None` for no gate) and this match time | an age id or `None`; 0 to 72000 |
| `wave_sizes` | `[(from_ds, armed)]`: armed units a wave needs from each match time on | from 0, times strictly increasing, sizes not decreasing, 1 to `max_wave_units` |
| `max_wave_units`, `wave_interval_ds`, `min_siege`, `wains_per_wave` | Largest wave (bounds the hashed memory), time between launches, siege units a launch needs, supply units kept for the waves | 1 to 64; 1 to 72000; 0 to `max_wave_units`; 1 to 64 |
| `stage_margin_tiles`, `stage_radius_tiles`, `stage_ready_permille`, `stage_max_ds` | The stage point's margin beyond the enemy shooters' reach, the radius that counts as staged, the share staged before the assault, the longest wait | 1 to 64; 1 to 64; 1 to 1000; 1 to 72000 |
| `siege_focus_tiles`, `wain_standoff_tiles`, `wain_leash_tiles` | Siege this close attacks the target; the Wain's margin outside every enemy shooter's range and its leash | 1 to 64 |
| `ring_units`, `clear_defenders_units`, `continue_min_units`, `recall_below_units` | Units posted on a Town's ring to annex it, sent at its defenders, needed to go on to the next Town, below which a wave comes home | 1 to 64 |
| `wave_max_ds`, `all_in_after_ds` | A wave ends after this long unless it is annexing; from this match time every armed unit joins and nothing is recalled | 1 to 72000; 0 to 72000 |
| `defend_radius_tiles`, `defend_regoal_tiles`, `defend_release_ds`, `defend_max_block_ds` | Enemy military on own land this close to an own Town is a threat; a guard is re-ordered beyond this distance from the nearest threat; defence ends after this long with no threat; it blocks launches, and calls a staging wave home, for at most this long | 1 to 64; 1 to 64; 1 to 72000; 1 to 72000 |
| `defend_guard_pop_ratio` | Past `defend_max_block_ds`, while a launch is due, the home guard is the home armed units nearest the threat (siege last) until their pop reaches this many times the threat's pop, at least one unit; the rest go back to the rally, and those at the rally are left to the wave | 1 to 64 |
| `recall_wave`, `recall_wave_town_hp_permille` | `WhileStaging` (a staging wave while the defence still blocks launches, or an assaulting one when any threatened Town, the engaged one or another, is contested or at or below the share of its hit points) or `Always` | 0 to 1000 |

Since M5b's waves slice the bot reads every knob. A **defence** (enemy
military on own land near an own Town) sends every home armed unit at the
nearest threat and blocks launches for at most `defend_max_block_ds`;
past that, while a launch is due, the guard shrinks to
`defend_guard_pop_ratio` times the threat's pop and the units at the rally
launch the wave though the raid goes on. A **wave** launches when
no wave is out, the defence does not block launches, the bot has reached
`wave_requires_age`, the tick has reached the last launch plus
`wave_interval_ds` and `first_wave_not_before_ds`, and at least the
`wave_sizes` row of the tick (at most the home army plus the free pop)
armed units stand within `assemble_radius_tiles` of the rally, `min_siege`
of them siege, with a Supply Wain at home; at a full pop one armed unit
stays home. Every assembled armed unit goes (siege first, at most
`max_wave_units`) with one Wain: they stage at a point off enemy land and
beyond the enemy shooters' reach (`stage_margin_tiles` plus the group's
spread), assault the enemy Town nearest the rally once `stage_ready_permille`
of them and all their siege stand within `stage_radius_tiles` of it (or
after `stage_max_ds`), the Wain keeping `wain_standoff_tiles` outside every
enemy shooter's range on the way back to the stage point, siege within
`siege_focus_tiles` attacking the Town itself; at 0 HP `ring_units`
annexing units hold its ring while `clear_defenders_units` attack each
defender near it; a captured Town sends a wave of at least
`continue_min_units` on to the next, and a wave with no annexing unit,
fewer than `recall_below_units`, or older than `wave_max_ds` and not
annexing comes home. From `all_in_after_ds` every home armed unit joins the
wave out and nothing is recalled. The rules and their order are in
docs/design/ai.md and `crates/ai/src/standard/waves.rs`.
