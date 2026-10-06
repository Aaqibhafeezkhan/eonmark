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
| Path cache | bounded LRU, size in data, cleared on every grid change | `data/rules/rules.ron` (`path_cache_entries`) |
| Repath cadence | every 60 ticks (3 s) while moving, or when the next waypoint becomes blocked | `movement.rs` |
| Spatial grid | 2 x 2 tile cells, rebuilt every tick, unhashed | `movement.rs` |
| Tick rate | 20 Hz | `rules.ron` (`tick_rate_hz`) |
| Bench: 500 movers | mean step < 5 ms, p95 < 10 ms, release, dev Mac | `sim-cli bench --units 500 --ticks 1200` |
| Bench: arrival | >= 99 % of 500 units within 3 tiles of the component-corrected goal | same bench |
| Bench: saturated budget | one tick that spends the full 4000 expansions < 2 ms | `sim-cli bench --astar --budget 4000` |
| Test oracle | `pathfinding` 4.16.0, dev-dependency only | `crates/sim/Cargo.toml` |

The pop cap bounds real unit counts: 25 + 25 per Arms level gives at most 100 units per player and 200 in a two-player match. The 500-unit bench is a stress margin, not an expected load.

## Cost grid

`Map` owns `cost: Vec<u8>` with one byte per tile in row-major order (`index = y * 128 + x`). A tile is passable when its cost is 1 and blocked when it is 255. No other values are used in v0.1; the type is u8 so terrain weights can be added later without a format change.

Water, mountain and forest tiles are blocked by the map loader. Buildings block the tiles of their footprint when construction starts and unblock them when the building is destroyed. Units are never written into the cost grid; unit-to-unit avoidance is handled by the movement step, not by pathfinding.

Every mutation of `cost` bumps `cost_grid_generation`. Three derived structures depend on the generation and are rebuilt lazily when it changes:

| Derived structure | Hashed | Rebuilt |
| --- | --- | --- |
| Path cache | no | cleared wholesale on any generation change |
| Connected component ids (one u16 per tile) | no | recomputed by flood fill on first use after a generation change |
| Spatial grid of unit positions | no | every tick |

`restore()` rebuilds all three before it returns, so a restored sim never serves a stale derived value.

## The A* search

The search is written in-house in `pathing.rs` (about 150 lines). It is not built on the `pathfinding` crate because that crate's `astar` runs its heap loop to completion and exposes no budget or resumable state (verified 2026-10-05 in its `astar.rs`). A worst-case search on this map touches about 16k nodes and could alone exceed the 5 ms step budget.

Grid and moves. The grid is 8-connected. An orthogonal step costs 10 and a diagonal step costs 14. The heuristic is octile: with `dx = |x1 - x0|` and `dy = |y1 - y0|`, `h = 14 * min(dx, dy) + 10 * (max(dx, dy) - min(dx, dy))`. It is admissible and consistent for these step costs, so the first time the goal is popped the path is optimal.

Corner cutting. The design is silent on diagonal moves past blocked tiles. The conservative choice is to forbid them: a diagonal step from A to B is legal only when both orthogonal neighbours shared by A and B are passable. Units have a radius and would otherwise clip the corner of a building. This rule must be mirrored in the oracle test's neighbour function.

Open set. The open set is a `BinaryHeap` of `(f, g, tile)` entries ordered so that the entry with the smallest `f` pops first; among equal `f` the largest `g` pops first (deeper nodes, fewer re-expansions); among equal `(f, g)` the smallest tile index pops first. The third key makes the order total, so two machines always pop the same node. Stale heap entries (a node already closed with a better `g`) are skipped when popped.

Per-search arrays, sized to the map:

| Array | Type | Meaning |
| --- | --- | --- |
| `g` | `Vec<u32>`, `u32::MAX` = unvisited | best known cost to each tile |
| `parent` | `Vec<u16>` | tile index of the predecessor on the best path (16384 tiles fit in u16) |
| `closed` | `Vec<bool>` or a bitset | tile has been expanded |

Resumable API. A search is a struct holding the heap, the three arrays and an expansion counter. Its only entry point is:

```text
resume(&mut self, budget: u32) -> SearchStatus
SearchStatus = Found(Vec<Tile>) | Exhausted | Suspended
```

`resume` pops and expands nodes until one of three things happens: the goal is popped (`Found`, with the path reconstructed through `parent` from goal to start and reversed), the heap is empty (`Exhausted`, the goal is unreachable from the start), or `budget` expansions have been spent (`Suspended`, the search keeps its state and continues on the next call). Expansions, not heap pops, are what the budget counts, so skipped stale entries are free.

Path output. A found path is the list of tile centres from the start tile to the goal tile. No smoothing or string pulling is done in v0.1; arrival steering rounds the corners visually. This is a conservative default and is listed under open questions.

## Request queue and the per-tick budget

Units do not search for paths directly. A Move, AttackMove or Gather command pushes a `PathRequest { unit, start_tile, goal_tile, requested_tick }` into the sim's request queue. The queue is a `Vec<PathRequest>` kept sorted by `(requested_tick, UnitId)`, so older requests go first and ties are broken by id.

Each tick the pathing step walks the queue in order with a budget of `path_budget_expansions` expansions (default 4000). For each request:

1. Correct the goal. If the goal tile is blocked or lies in a different connected component from the start tile, replace it with the nearest passable tile of the start's component (see BFS fallback below). If start and corrected goal are equal, the request completes with an empty path.
2. Consult the cache with key `(start_tile, goal_tile, cost_grid_generation)`. A hit completes the request with the cached path and spends no budget.
3. Otherwise call `resume(remaining_budget)` on the request's search, creating it on first contact. `Found` stores the path on the unit and in the cache. `Exhausted` cannot happen after step 1 but is handled defensively by clearing the unit's order and emitting `SimEvent::CommandRejected { reason: Unreachable }`. `Suspended` leaves the request at its position in the queue and ends the tick's pathing work, because the budget is gone.

A unit whose request is still queued or suspended waits in place. It does not drift toward the goal, so the state at the end of the tick is a pure function of the inputs.

Snapshot and restore. The request queue is hashed sim state (it has its own sub-hash, `pathing`). A suspended search also carries progress that decides on which tick a path appears. The design does not say how that progress is saved; the conservative choice is to store only the request plus its `expansions_done` counter, and on `restore()` rebuild the search by running `resume(expansions_done)` against the restored grid. The A* is deterministic, so this reproduces the exact heap and arrays. Serialising the full heap is an acceptable alternative if the implementer prefers it; either way the M1 test `snapshot_restore_matches_uninterrupted_including_spawns` must pass with 500 movers.

## Path cache

The cache maps `(start_tile, goal_tile, cost_grid_generation)` to a path and holds at most `path_cache_entries` entries with least-recently-used eviction. It is cleared wholesale whenever `cost_grid_generation` changes, which is why a hit is always equal to a fresh search: both read the same grid. The cache is derived state. It is never hashed, never serialised, and `restore()` starts with an empty one. A proptest runs the same command stream with the cache disabled and enabled and asserts identical hashes every tick (`path_cache_is_transparent`).

The LRU order itself does not affect sim results, only which paths are recomputed, so it may use any container as long as it is deterministic in what it returns.

## Repathing

A moving unit requests a new path in two cases:

| Trigger | Check | Why |
| --- | --- | --- |
| Next waypoint blocked | the cost of the next waypoint's tile is 255 | a building was placed on the route |
| Periodic | `(tick - path_start_tick) % 60 == 0` while a path is active | the route may have become shorter after a blocked tile cleared, and the goal of an AttackMove or Gather may have moved |

Both go through the normal queue, so repath cost is bounded by the same budget. Until the new path arrives the unit keeps following the old one unless the next waypoint is blocked, in which case it stops.

## Group orders: spiral offsets

When one command moves several units to the same point, every unit needs its own target tile or they all push toward one spot. The sim assigns targets as follows:

1. Sort the ordered units by `UnitId`.
2. Walk a square spiral outward from the clicked tile: the click tile itself, then the ring at Chebyshev distance 1 in a fixed order (east, then clockwise), then distance 2, and so on.
3. Skip any tile that is blocked, or whose connected component differs from the click tile's component, or that has already been assigned in this order.
4. Give the next unassigned spiral tile to the next unit in id order.

Component ids are recomputed once per `cost_grid_generation`, so step 3 is an array lookup. The spiral is bounded by the map edges; if it runs out of tiles the remaining units share the last assigned tile.

When the click tile itself is blocked, or lies in a component none of the ordered units can reach, the click is first corrected by the BFS fallback from the perspective of the first unit in id order, and the spiral starts from the corrected tile.

## BFS nearest-passable fallback

`nearest_passable(from: Tile, component: u16) -> Tile` performs a breadth-first search over the 8-neighbourhood with a fixed neighbour order (east, south-east, south, south-west, west, north-west, north, north-east) and returns the first tile whose cost is 1 and whose component id equals `component`. Because the neighbour order is fixed and the queue is FIFO, the result is the same on every machine. The search is bounded by the map, so it always terminates; on a map with no passable tile in the component it returns `from`, which callers treat as "no path".

## Movement step

Movement runs after pathing in the same tick, in `UnitId` order, in fixed point (`Fx = I32F32`, `FxVec2`, see `fx.rs`). There is no trigonometry: a unit stores a facing vector and the renderer derives a heading in f32.

For each moving unit:

1. Arrival steering. The desired velocity points from the unit to its next waypoint with magnitude `speed`, scaled down linearly inside an arrival radius around the final waypoint so units stop instead of oscillating. Waypoints are consumed when the unit is within half a tile.
2. Separation. Neighbours are read from the spatial grid (the unit's cell and the eight around it). For each neighbour closer than the sum of radii plus a margin, a push-away vector is accumulated. Distances are compared with `dist_sq_i64`, never with square roots.
3. Integrate. `position += clamp(desired + separation, speed) * dt`, where `dt` is one tick.
4. Circle-vs-circle correction. After all units have moved, pairs that still overlap are separated by half the penetration each, visited in `(UnitId, UnitId)` order so the correction is reproducible. Corrections never push a unit into a blocked tile; a correction that would do so is dropped for that unit.

Units never move into a blocked tile. A unit that would cross into one stops at the boundary and triggers the blocked-waypoint repath above.

## Interactions with other systems

| System | Interaction |
| --- | --- |
| Construction ([towns.md](towns.md), [economy.md](economy.md)) | placing a building writes 255 into its footprint and bumps the generation; destroying one writes 1. Build sites are validated against the cost grid and against `is_buildable` from [territory.md](territory.md). |
| Combat ([combat.md](combat.md)) | AttackMove reuses Move pathing with the target's current tile as the goal; the periodic repath follows a moving target. |
| Economy | Gather paths the Yeoman to an adjacent passable tile of the resource building; forest and mountain tiles themselves are blocked. |
| AI ([ai.md](ai.md)) | the bot issues the same commands as a human and shares the same budget; it never gets its own queue. |
| Snapshot and replay | the request queue and suspended-search progress are hashed; cache, components and spatial grid are rebuilt in `restore()`. |
| Presentation | the renderer interpolates between the previous and current tick positions with `Time<Fixed>::overstep_fraction()`; it never reads paths. |

## Tests and oracle

The `pathfinding` crate is a dev-dependency only. `cargo tree -p sim -e normal` must not list it (a CI step checks this). It is used by two tests:

- `path_exists_iff_connected`: for random start and goal tiles on the plains map and on random grids, the in-house search finds a path exactly when the oracle's `astar` with the same neighbour function and the same corner-cutting rule finds one.
- cost equality: when both find a path, the total octile cost is identical.

Other M1 tests touching this module: `path_cache_is_transparent` (proptest, cache on versus off), `sort_keys_are_total_orders`, `dist_sq_i64_map_corners`, and the golden fixtures `move_500_short` (in `cargo test`), `move_500` and `group_spiral` (verified by `sim-cli verify` in CI).

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
- Suspended-search persistence: expansion counter replay (recommended above) versus serialising the heap. Decide in M1 and record in [../DETERMINISM.md](../DETERMINISM.md).
- The default budget of 4000 and the cache size are placeholders until the M1 bench. The `good first issue` "tune the path cache LRU size and A* budget with a bench table" owns the follow-up.
