# Pathfinding and movement

This document specifies how units in Eonmark find and follow paths on the 128x128 tile map: the cost grid, the in-house budgeted A*, the request queue, the path cache, repathing, group orders and the fixed-point movement step. It states the target numbers, the acceptance checklist from the implementing milestone, and the condition under which the deferred flow-field design is allowed back in. Everything here lives in `crates/sim` (`map.rs`, `pathing.rs`, `movement.rs`) and is subject to the determinism rules in [../DETERMINISM.md](../DETERMINISM.md).

Area: `area:sim`. Milestone: M1 (see [../ROADMAP.md](../ROADMAP.md)). Benches re-run at M4a (combat) and M5b (bots).

## Target numbers

| Quantity | Value | Where it is set |
| --- | --- | --- |
| Map size | 128 x 128 tiles, 1 tile = 1 m | `data/maps/plains_1v1.ron` |
| Cost per tile | u8: 1 passable, 255 blocked | `Map.cost: Vec<u8>` |
| Blocked by | water, mountain, forest, buildings | map tiles and `construction.rs` |
| Grid generation counter | `cost_grid_generation: u32`, +1 on every cost change | `Map` |
| Step costs | 10 orthogonal, 14 diagonal (octile) | `pathing.rs` constants |
| A* expansion budget | 4000 node expansions per tick, shared by all requests | `data/rules/rules.ron` (`path_budget_expansions`) |
| Path cache | bounded, insertion-order eviction, size in data, cleared on every grid change and on restore, transparent (see "Path cache") | `data/rules/rules.ron` (`path_cache_entries`) |
| Repath cadence | every 60 ticks (3 s) while moving (`repath_interval_ds: 30`, converted by `Rules::repath_interval_ticks`), or when the next waypoint becomes blocked | `data/rules/rules.ron` |
| Separation push caps | `speed / 2` for a moving unit, `speed / 8` for a unit without an order (`separation_push_moving_div`, `separation_push_idle_div`) | `data/rules/rules.ron` |
| Unit kind stats | `speed_tiles_per_s_x100: 180`, `radius_tiles_x100: 35`, `arrive_radius_tiles_x100: 25`, `arrive_slowdown_radius_tiles_x100: 50`, `waypoint_radius_tiles_x100: 50`, `separation_tiles_x100: 10`, `return_to_post_radius_tiles_x100: 100` for the one M1 kind, `yeoman` | `data/rules/units.ron` |
| Spatial grid | 2 x 2 tile cells, rebuilt every tick, unhashed | `movement.rs` |
| Tick rate | 20 Hz | `rules.ron` (`tick_rate_hz`) |
| Bench stream | `scenarios::bench_cross(n)`: `n` Yeomen in six groups spawned at column 20, rows 16, 35, 54, 73, 92, 111, each group ordered to its mirror point at column 107, so two groups cross at each of the three fords (rows 14-21, 60-67, 106-113 of the river at columns 62-65) | `crates/sim/src/scenarios.rs`, `sim-cli bench --units N` |
| Bench: 500 movers | mean step < 5 ms, p95 < 10 ms, release, dev Mac | `sim-cli bench --units 500 --ticks 1200` |
| Bench: arrival | >= 99 % of 500 units within 3 tiles of the component-corrected goal by tick 2400, 0 moving or displaced at the end (the 87-tile crossing alone takes about 970 ticks at 1.8 tiles/s; the gate first held at tick 1624 on 2026-10-06) | `sim-cli bench --units 500 --ticks 2400`; `--arrival-curve` prints the percentage every 100 ticks and the first tick the gate holds |
| Bench: saturated budget | one tick that spends the full 4000 expansions < 2 ms | `sim-cli bench --astar --budget 4000` |
| Test oracle | `pathfinding` 4.16.0, dev-dependency only | `crates/sim/Cargo.toml` |

The pop cap bounds real unit counts: 25 + 25 per Arms level gives at most 100 units per player and 200 in a two-player match. The 500-unit bench is a stress margin, not an expected load.

## Cost grid

`Map` owns `cost: Vec<u8>` with one byte per tile in row-major order (`index = y * 128 + x`). A tile is passable when its cost is 1 and blocked when it is 255. No other values are used in v0.1; the type is u8 so terrain weights can be added later without a format change.

Water, mountain and forest tiles are blocked by the map loader. A building blocks the tiles of its footprint as soon as its `Build` is accepted, while it is still a site, with one `Map::set_blocked_many` call for the whole footprint (the Seat at the standard start and `DebugPlaceBuilding` do the same). A cancelled site unblocks its footprint the same way, with one call; a destroyed building will too (M4a). Units are never written into the cost grid; unit-to-unit avoidance is handled by the movement step, not by pathfinding.

Every mutation of `cost` goes through `Map::set_blocked(tile, blocked) -> bool` or, for a footprint, `Map::set_blocked_many(tiles, blocked) -> bool` (M3a). Each call bumps `cost_grid_generation` once, and only when at least one cost actually changes. Three derived structures depend on the generation:

| Derived structure | Hashed | Rebuilt |
| --- | --- | --- |
| Path cache | no | cleared wholesale on any generation change and on `restore()` |
| Connected component ids (one u16 per tile, `NO_COMPONENT` for blocked tiles) | no | eagerly, by flood fill inside `set_blocked` and `set_blocked_many` when the generation changes, and in `restore()` |
| Spatial grid of unit positions | no | every tick, and in `restore()` |

Components are rebuilt eagerly rather than on first use so that `Map::component_of` works through a shared `&Map` inside `group_targets` and `Pathing::service` (M1 decision; one flood fill per changed tile is cheap at 16k tiles, and since M3a `set_blocked_many` makes a whole footprint one flood fill and one generation bump, so the path cache and a suspended search see one change per building). `restore()` rebuilds all three before it returns, so a restored sim never serves a stale derived value. `Map` itself (`cost` plus the generation) is hashed state with its own sub-hash, `map`.

Two fixed neighbour orders exist and both are total:

| Function | Order | Used by |
| --- | --- | --- |
| `Map::neighbors8(tile)` | N, NE, E, SE, S, SW, W, NW; passable tiles only; no corner cutting | A*, the component flood fill, the oracle test |
| `Map::nearest_passable(tile, component)` | E, SE, S, SW, W, NW, N, NE; every in-bounds neighbour, blocked ones too | goal correction, group-order click correction |

## The A* search

The search is written in-house in `pathing.rs` (about 150 lines). It is not built on the `pathfinding` crate because that crate's `astar` runs its heap loop to completion and exposes no budget or resumable state (verified 2026-10-05 in its `astar.rs`). A worst-case search on this map touches about 16k nodes and could alone exceed the 5 ms step budget.

Grid and moves. The grid is 8-connected. An orthogonal step costs 10 and a diagonal step costs 14. The heuristic is octile: with `dx = |x1 - x0|` and `dy = |y1 - y0|`, `h = 14 * min(dx, dy) + 10 * (max(dx, dy) - min(dx, dy))`. It is admissible and consistent for these step costs, so the first time the goal is popped the path is optimal.

Corner cutting. The design is silent on diagonal moves past blocked tiles. The conservative choice is to forbid them: a diagonal step from A to B is legal only when both orthogonal neighbours shared by A and B are passable. Units have a radius and would otherwise clip the corner of a building. This rule must be mirrored in the oracle test's neighbour function.

Open set. The open set is a `BinaryHeap` of `(f, g, tile)` entries ordered so that the entry with the smallest `f` pops first; among equal `f` the largest `g` pops first (deeper nodes, fewer re-expansions); among equal `(f, g)` the smallest tile index pops first. The third key makes the order total, so two machines always pop the same node. Stale heap entries (a node already closed with a better `g`) are skipped when popped.

Per-search arrays, sized to the map (`map.len()` entries each):

| Array | Type | Meaning |
| --- | --- | --- |
| `g` | `Vec<u32>`, `G_UNVISITED` (`u32::MAX`) = unvisited | best known cost to each tile |
| `parent` | `Vec<u32>`, `NO_PARENT` (`u32::MAX`) = none | row-major tile index of the predecessor on the best path |
| `closed` | `Vec<bool>` | tile has been expanded |

Resumable API. A search is `AStarSearch { from, to, open: BinaryHeap<Node>, closed, g, parent, expansions }`, where `Node { f, g, tile: u32 }`. It is created with `AStarSearch::new(map, from, to)` (`g[from] = 0`, one open entry `Node { f: octile(from, to), g: 0, tile: idx(from) }`) and driven by one method:

```text
resume(&mut self, map: &Map, budget: &mut u32) -> SearchStatus
SearchStatus = Found(Vec<Tile>) | Exhausted | Suspended
```

`resume` pops and expands nodes until one of three things happens: the goal is popped (`Found`, with the path reconstructed through `parent` from goal to start and reversed, so it runs start..=goal inclusive and is a single tile when start == goal), the heap is empty (`Exhausted`, the goal is unreachable from the start), or `*budget` reaches zero with work left (`Suspended`, the search keeps its state and continues on the next call). Every expansion decrements `*budget` by one, so several searches share one per-tick budget by passing the same counter. A call with `*budget == 0` suspends without expanding. Expansions, not heap pops, are what the budget counts, so skipped stale entries are free.

`AStarSearch` serialises with serde (the heap as a sequence in its internal array order, then the arrays and the counter) and is hashed sim state while it is the active search; see "Snapshot and restore" below.

Path output. A found path is the list of tile centres from the start tile to the goal tile. No smoothing or string pulling is done in v0.1; arrival steering rounds the corners visually. This is a conservative default and is listed under open questions.

## Request queue and the per-tick budget

Units do not search for paths directly. A Move, AttackMove or Gather command pushes a `PathRequest { requested_tick, unit, from, to }` into the sim's request queue through `Pathing::request`, which replaces any earlier request of the same unit. The queue is a `Vec<PathRequest>` kept sorted by `sort_key() = (requested_tick, UnitId)`, so older requests go first and ties are broken by id. `Pathing::cancel(unit)` drops a unit's request (Stop does this).

Each tick `Pathing::service(map, budget, tick)` walks the queue in order with a budget of `path_budget_expansions` expansions (default 4000) and returns `Vec<(UnitId, Option<Vec<Tile>>)>`, where `None` means unreachable. If a search is already `active` (suspended from an earlier tick) it is resumed first and nothing else runs until it finishes. Then, for each queued request:

1. Correct the goal. If the start tile is blocked, the start becomes `nearest_passable(from, None)`. If the goal tile is blocked or lies in a different connected component from the start tile, replace it with `nearest_passable(to, Some(component_of(start)))`, the nearest passable tile of the start's component (see BFS fallback below). If start and corrected goal are equal, the request completes with an empty path.
2. Consult the cache with key `(start_tile, goal_tile, cost_grid_generation)`. A hit completes the request only when `expansions_used <= remaining budget`, charging exactly `expansions_used` against the budget; otherwise the hit is ignored and the real search runs (see "Path cache" for why).
3. Otherwise call `resume(map, &mut remaining)` on the request's search, creating it with `AStarSearch::new` on first contact. `Found` stores the path on the unit's order and inserts `(path, expansions_used)` into the cache. `Exhausted` cannot happen after step 1 but is handled defensively: `service` reports `None`, and `Sim::step` clears the unit's order and emits `SimEvent::PathUnreachable { unit }` (an asynchronous failure has no `seq`, so `CommandRejected { Unreachable }` is reserved for a synchronous reject). `Suspended` stores the request and its search as `active` and ends the tick's pathing work, because the budget is gone.

A unit whose request is still queued or suspended waits in place. It does not drift toward the goal, so the state at the end of the tick is a pure function of the inputs.

Implementation notes (M1, `pathing.rs`). Where the implementation is more specific than the steps above:

- Step 1 also corrects the start: a unit standing on a blocked tile (possible once buildings write into the grid) searches from `nearest_passable(start, any component)`. When the whole map is blocked the request completes as unreachable.
- Step 3's `Exhausted` is reported to `Sim::step` as `None`, which clears the unit's order and emits `SimEvent::PathUnreachable { unit }` (there is no `seq` for an asynchronous failure, so `CommandRejected { Unreachable }` stays reserved for synchronous rejects).
- `resume` checks the budget before each expansion, so a zero budget suspends without touching the heap; an empty heap is `Exhausted` even when the budget is already zero. Stale heap entries are popped for free.
- A request whose `requested_tick` is later than the current tick stays queued; the queue is sorted, so the walk stops at the first such request. `Sim::step` never creates one.
- `Pathing::service_budgeted` is `service` with the budget passed by reference, so tests can assert the exact remainder a cache hit leaves.
- A suspended search is resumed only on the cost grid it started on (`SIM_VERSION` 5). `AStarSearch` records `cost_grid_generation` at creation; when the grid has changed since (a building placed or removed, from M3a), `service` drops the search and requeues its request at the head of the queue, so the next walk corrects start and goal and searches the current grid. Resuming would have kept the `closed`, `g` and `parent` arrays of the old grid, returned a path through a tile blocked since, and cached it under the new generation, where a later hit would differ from what the cache-off run computes. `suspended_search_restarts_when_the_grid_changes` and the wall-toggling `path_cache_is_transparent_at_the_pathing_level` cover it.
- The queue walk stops as soon as the budget is spent (`SIM_VERSION` 2). Before goal correction and before the cache lookup, so the cache on and off take the same exit; a request reached with zero budget stays queued instead of becoming an `active` search with zero expansions, which would have put three map-sized arrays into every snapshot of a saturated tick. Trivial start == goal requests therefore also wait for a tick with budget; `rules.ron` validates the budget `> 0`, so that is at most one tick.

Snapshot and restore. The request queue and the active search are hashed sim state (together they are the `pathing` sub-hash). The M1 decision, recorded in [../DETERMINISM.md](../DETERMINISM.md), is to serialise the suspended search in full: `Pathing { queue, active: Option<(PathRequest, AStarSearch)> }` is part of `State`, and `AStarSearch` serialises its `BinaryHeap<Node>` as a sequence in internal array order plus the `closed`, `g` and `parent` arrays and the expansion counter. `std::collections::BinaryHeap` deserialises by pushing in that order, which reproduces the identical array, so a snapshot taken mid-search restores the exact search and a restored sim pops the same nodes on its next `resume`. The alternative of storing only an expansion counter and replaying `resume(expansions_done)` on restore was rejected because it costs the replayed expansions at restore time and because a counter cannot represent the search that the transparent-cache fallback skipped. A unit test in `pathing.rs` runs a search for k expansions, round-trips the `AStarSearch` through postcard, finishes both copies and asserts identical `Found` paths and identical hashes of the intermediate struct; `snapshot_restore_matches_uninterrupted_including_spawns` covers the same with 500 movers.

## Path cache

The cache maps `(start_tile, goal_tile, cost_grid_generation)` to a `CacheEntry { path, expansions_used }` and holds at most `path_cache_entries` entries. It is cleared wholesale whenever `cost_grid_generation` changes, which is why a hit is always equal to a fresh search: both read the same grid. The cache is derived state. It is never hashed, never serialised (`#[serde(skip)]`), and `restore()` starts with an empty one. A proptest runs the same command stream with the cache disabled and enabled and asserts identical hashes every tick (`path_cache_is_transparent`).

The transparent cache rule (virtual budget). A hit must leave the hashed state at the end of every tick identical to the state the same run reaches with the cache disabled. The active search is hashed state, so a request that the real search would have *suspended* cannot be served from the cache: no cache replay can reproduce the partially built heap. Therefore:

- `expansions_used` records the total number of expansions the original search spent, across however many ticks it was suspended.
- When a request is reached with `remaining` budget left in the tick and `expansions_used <= remaining`, the hit is honoured: the request completes this tick with the cached path and `remaining` drops by exactly `expansions_used`, which is precisely what the real search would have done.
- When `expansions_used > remaining`, the hit is ignored and the real search runs; it suspends naturally and is cached when it completes.

Timing, results and hashes are thereby identical with the cache on or off; the cache only saves work for searches that fit in the remaining budget. This is the strongest reading of `path_cache_is_transparent` and the one the test asserts: equal hashes on every tick, not just at the end.

Eviction is deterministic: entries live in a `BTreeMap` keyed by an insertion counter (`PathCache { entries, by_key, capacity, enabled, next_insert }`) and the smallest counter is evicted at capacity. Whether the cache evicts or hits never changes results, only work, so the eviction policy may change freely as long as it stays deterministic in what it returns.

## Repathing

A moving unit requests a new path in two cases. `movement::step` returns the ids that need one and `Sim::step` issues the `PathRequest`s:

| Trigger | Check | Why |
| --- | --- | --- |
| Next waypoint blocked | the cost of the tile of `order.path[order.next]` is 255 | a building was placed on the route |
| Periodic | `tick >= order.repath_at` while a path is active; `repath_at` is set to `tick + Rules::repath_interval_ticks()` (`repath_interval_ds: 30`, 60 ticks) when the order is given and again when a repath is requested | the route may have become shorter after a blocked tile cleared, and the goal of an AttackMove or Gather may have moved |

Both go through the normal queue with `requested_tick = tick`, so repath cost is bounded by the same budget. Until the new path arrives the unit keeps following the old one unless the next waypoint is blocked, in which case it stops. A `MoveOrder { goal, path, next, repath_at }` with an empty `path` is a unit waiting for its first path; it does not move.

## Group orders: spiral offsets

When one command moves several units to the same point, every unit needs its own target tile or they all push toward one spot. `movement::group_targets(map, click, n) -> Vec<FxVec2>` returns `n` tile centres and the Move handler assigns them as follows:

1. Validate and dedupe the ordered units, then sort them by `UnitId`.
2. Correct the click: `tile_of(click)` is replaced by `nearest_passable(tile, None)` when it is blocked. If the map has no passable tile at all, every target is the raw click position.
3. Walk `Map::spiral` outward from the corrected tile: the tile itself, then the ring at Chebyshev distance 1 starting at the east tile and going clockwise on screen (south first, since `y` grows south), then distance 2, and so on. Every map tile is visited exactly once.
4. Skip any tile that is blocked or whose connected component differs from the corrected click tile's component.
5. Give the next spiral tile's centre (`center_of`, tile + 0.5) to the next unit in id order; the unit's `MoveOrder.goal` is that centre and its `PathRequest.to` is that tile.

Component ids are rebuilt once per `cost_grid_generation`, so step 4 is an array lookup. The spiral is bounded by the map edges; if it runs out of tiles the remaining units share the last assigned target. The `queue` flag on `Command::Move` is ignored in M1: a unit holds one `Option<MoveOrder>`.

## BFS nearest-passable fallback

`Map::nearest_passable(from: Tile, component: Option<u16>) -> Option<Tile>` performs a breadth-first search over the 8-neighbourhood with a fixed neighbour order (east, south-east, south, south-west, west, north-west, north, north-east) over every in-bounds tile, blocked ones included since the search starts from a blocked tile, and returns the first tile whose cost is 1 and, when `component` is `Some`, whose component id equals it. Because the neighbour order is fixed and the queue is FIFO, the result is the same on every machine. The search is bounded by the map, so it always terminates; on a map with no passable tile in the component it returns `None`, which callers treat as "no path" (goal correction then leaves the request to end `Exhausted`, group targets fall back to the raw click).

## Movement step

Movement runs after pathing in the same tick, in `UnitId` order, in fixed point (`Fx = I32F32`, `FxVec2`, see `fx.rs`). There is no trigonometry: a unit stores a facing vector and the renderer derives a heading in f32.

`movement::step(units, map, grid, rules, tick, events) -> Vec<UnitId>` runs after `Pathing::service` and after the spatial grid has been rebuilt for the tick. Unit stats come from `data/rules/units.ron` through `rules.unit_kind(kind)`: `speed` (tiles per tick, `speed_tiles_per_s_x100 / (100 * tick_rate_hz)`) and `radius` (`radius_tiles_x100 / 100`) are converted once at spawn and stored on the `Unit`; `arrive_radius_tiles_x100`, `arrive_slowdown_radius_tiles_x100`, `waypoint_radius_tiles_x100`, `separation_tiles_x100` and `return_to_post_radius_tiles_x100` are read per tick, and the two push caps come from `rules.ron` (`separation_push_moving_div`, `separation_push_idle_div`). There are no movement constants in Rust.

For each moving unit:

1. Arrival steering. The desired velocity points from the unit to its next waypoint `order.path[order.next]` with magnitude `speed`, scaled down linearly inside the kind's arrive slowdown radius around the final waypoint so units stop instead of oscillating. Intermediate waypoints are consumed when the unit is within the kind's waypoint radius (half a tile for the Yeoman) or once it is closer to the following waypoint than the waypoint itself is; the final one when within the arrive radius, which clears the order, records the final waypoint's centre as the unit's post (`Unit.post: Option<Post { goal, returning }>`, hashed state since `SIM_VERSION` 3) and emits `SimEvent::UnitArrived { unit }`.
2. Separation. Neighbours are read from the spatial grid (`SpatialGrid::neighbors(pos, radius)`, the cells intersecting the square `[pos - r, pos + r]`, in ascending id order). For each neighbour closer than `radius_a + radius_b + separation`, a push-away vector is accumulated. Distances are compared with `dist_sq_i64` against the squared threshold in I32F32 scale, never with square roots. The sum is capped at `speed / separation_push_moving_div` for a unit with an order (or returning to its post, below) and `speed / separation_push_idle_div` for one without, so steering keeps the upper hand and parked units yield slowly to a passing crowd.
3. Integrate. `position += clamp(desired + separation, speed)`, one tick per step so there is no `dt` factor.
4. Circle-vs-circle correction. After all units have moved, pairs that still overlap are separated by half the penetration each, visited in `(UnitId, UnitId)` order so the correction is reproducible. Corrections never push a unit into a blocked tile; a correction that would do so is dropped for that unit.

Return to post. A parked unit yields to a passing crowd (step 2), so late arrivals of a group order shove the early ones off their tiles; without a counter-measure a crowd that has fully arrived leaves a few units several tiles from their goal. An arrived unit (no order, a post) that finds itself farther than the kind's `return_to_post_radius_tiles_x100` (one tile) from `post.goal` steers straight back to it with the arrival steering of step 1 (same speed, same slowdown, no path request, since the straight line is a few tiles at most) and sets `post.returning`; it keeps walking until it is within the arrive radius again, then clears the flag. The gap between the two radii is the hysteresis that stops a unit on the boundary from dithering. While `returning` and outside the arrive radius the unit is "moving" for the push cap (`separation_push_moving_div`); back at its post it is parked again (`separation_push_idle_div`), so two neighbours cannot shove each other back and forth forever: a group's posts are distinct spiral tiles (one tile apart, farther than `radius_a + radius_b + separation`), so a block that has converged holds still. If terrain blocks every candidate step of the walk back, the post becomes a `MoveOrder { goal: post.goal, path: [] }` and the unit requests a path like an ordinary move; arriving makes it a post again. A new Move clears the post (the order replaces it), a Stop clears both. The convergence test (`return_to_post_converges_100_units_and_settles`, 100 units on one click, 3000 ticks) asserts every unit within 3 tiles of its own spiral target and a constant `units` sub-hash over the last 100 ticks.

Units never move into a blocked tile. A unit that would cross into one stops at the boundary and triggers the blocked-waypoint repath above. The function returns the ids that need a repath (blocked next waypoint, `tick >= repath_at`, or a blocked return to post); `Sim::step` turns them into `PathRequest`s.

## Interactions with other systems

| System | Interaction |
| --- | --- |
| Construction ([towns.md](towns.md), [economy.md](economy.md)) | an accepted `Build` writes 255 into the site's footprint with one `set_blocked_many` call (one generation bump); cancelling a site writes 1 the same way, and so will destroying a building (M4a). Build sites are validated against the cost grid and against `is_buildable` from [territory.md](territory.md), and the builder walks to the passable tile around the footprint nearest to it. |
| Combat ([combat.md](combat.md)) | AttackMove reuses Move pathing with the target's current tile as the goal; the periodic repath follows a moving target. |
| Economy | Gather paths the Yeoman to an adjacent passable tile of the resource building; forest and mountain tiles themselves are blocked. |
| AI ([ai.md](ai.md)) | the bot issues the same commands as a human and shares the same budget; it never gets its own queue. |
| Snapshot and replay | the request queue and suspended-search progress are hashed; cache, components and spatial grid are rebuilt in `restore()`. |
| Presentation | the renderer interpolates between the previous and current tick positions with `Time<Fixed>::overstep_fraction()`; it never reads paths. |

## Tests and oracle

The `pathfinding` crate is a dev-dependency only. `cargo tree -p sim -e normal` must not list it (a CI step checks this). It is used by two tests:

- `path_exists_iff_connected`: for random start and goal tiles on the plains map and on random grids, the in-house search finds a path exactly when the oracle's `astar` with the same neighbour function and the same corner-cutting rule finds one.
- cost equality: when both find a path, the total octile cost is identical.

Other M1 tests touching this module: `path_cache_is_transparent` (proptest, cache on versus off, equal hashes every tick), `sort_keys_are_total_orders`, `dist_sq_i64_map_corners`, the mid-search snapshot round-trip unit test in `pathing.rs`, `return_to_post_converges_100_units_and_settles`, and the golden fixtures `move_500_short`, `group_spiral` and `snapshot_restore` (in `cargo test`) and `move_500` (verified by `sim-cli verify --release` in CI, on both operating systems).

The bench stream (`sim-cli bench --units N --ticks T`) is `scenarios::bench_cross(N)`, not the `move_500` fixture: `move_500` sends all 500 units from the west start through the middle ford alone, which measures the throughput of one 8-tile ford (about one unit per tick) rather than a crossing, and its stream is frozen as the hash-parity fixture. `bench_cross` spawns the units in six equal groups along the west side (column 20, rows 16 to 111) and orders each group to its mirror point on the east side, so A* sends two groups through each ford. The bench reports the step-time statistics, the arrived percentage at `T` with a breakdown of the units outside 3 tiles (moving, waiting for a path, displaced after arriving, no order), the first tick after which the 99 % gate held and the tick from which it held to the end; `--arrival-curve` adds the percentage every 100 ticks and `--verbose` lists every straggler. At 1.8 tiles/s the 87-column walk alone is about 970 ticks, so the gate tick is the number to read, not the percentage at 1200.

## Acceptance checklist (M1)

Copied from the M1 milestone in [../ROADMAP.md](../ROADMAP.md). All non-owner lines must pass, with command output pasted into the PR, before M2 starts.

- [ ] `cargo test -p sim` (non-ignored suite) passes in under 10 s on the dev Mac and includes same_seed_same_hash_two_threads, snapshot_restore_matches_uninterrupted_including_spawns, restore_rebuilds_caches, proptest_random_commands_are_deterministic, path_cache_is_transparent, path_exists_iff_connected, sort_keys_are_total_orders, dist_sq_i64_map_corners, and the short goldens.
- [ ] `cargo run -p sim-cli --release -- verify crates/sim/tests/fixtures/move_500.eonreplay` prints `OK final_hash=0x...` matching the .hash file and exits 0; flipping one command byte in a copy prints `DIVERGED at tick N (subsystem: units)` and exits 1; editing data/rules/rules.ron without bumping rules_version prints `RULES CHANGED since recording` and exits 1.
- [ ] `cargo run -p sim-cli --release -- bench --units 500 --ticks 1200` reports mean step < 5 ms and p95 < 10 ms on the dev Mac while 500 units cross the map and >= 99% arrive within 3 tiles of their (component-corrected) goal; `bench --astar --budget 4000` reports the per-tick cost of a saturated budget < 2 ms; numbers appended to docs/BUILD_TIMES.md.
- [ ] A scripted sim-cli recording killed with `kill -9` at tick ~600 leaves a file that `verify` accepts (exit 0, reports ticks verified).
- [ ] `cargo run -p sim-cli -- hash-dump <replay> --every 1` prints per-tick sub-hashes for units, pathing, rng; docs/DETERMINISM.md explains bisecting with it.
- [ ] CI hash-parity job is green: ubuntu-24.04 and macos-26 final hashes of move_500 are byte-identical.
- [ ] `cargo tree -p sim -e normal` lists only fixed, serde, postcard, xxhash-rust, rand_pcg, ron, rules and their transitive deps (no slotmap, no pathfinding).

Commands:

```bash
cargo test -p sim
cargo run -p sim-cli --release -- bench --units 500 --ticks 1200
cargo run -p sim-cli --release -- bench --astar --budget 4000
cargo tree -p sim -e normal | grep -E 'bevy|glam|wgpu|winit|pathfinding' && echo "FORBIDDEN DEP" && exit 1
```

## Flow fields: the re-entry condition

Sector/portal hierarchical A* with per-goal flow fields is the documented follow-up, not part of v0.1. It is the most bug-prone deterministic system an RTS can add, and nothing in the v0.1 scope (200 units at the pop cap, one flat map) needs it.

It may be scheduled only when all of the following hold:

1. `sim-cli bench --units 500 --ticks 1200` fails its gate (mean >= 5 ms or p95 >= 10 ms) in release on the dev Mac after the M4a combat load is included.
2. The failure has been profiled (criterion on `crates/sim`, or `trace_tracy` on the game crate) and pathing, not movement or combat, is the dominant cost.
3. Tuning `path_budget_expansions` and `path_cache_entries` in `rules.ron` with a bench table in the PR did not close the gap.

If adopted, it keeps the same sim API (`PathRequest` in, waypoints out, same budget semantics) so no command or replay format changes. The deferred item is tracked in [../ROADMAP.md](../ROADMAP.md).

## Open questions

- Corner cutting is forbidden here as the conservative reading of the design. If playtests show units taking visibly long detours around single buildings, allow diagonals past one blocked neighbour and update the oracle test together.
- Paths are raw tile centres. A later string-pulling pass would need to stay in fixed point and prove it never crosses a blocked tile; file as `needs-design` if arrival looks jagged in M2.
- Suspended-search persistence was decided in M1: the full heap and arrays are serialised (see "Request queue" above and [../DETERMINISM.md](../DETERMINISM.md)). Revisit only if snapshot size becomes a problem; a 128x128 search is at most about 150 KB.
- The default budget of 4000 and the cache size are placeholders until the M1 bench. The `good first issue` "tune the path cache size and A* budget with a bench table" owns the follow-up.
- Movement under a suspended search: a unit waits in place until its path arrives. If the bench shows visible stalls when many units are ordered at once, consider a per-request budget floor; it changes hashes, so it bumps `SIM_VERSION`.
