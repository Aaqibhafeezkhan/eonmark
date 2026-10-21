//! Budgeted, resumable in-house A* on the tile map, the per-tick request
//! queue and the transparent path cache. Specification:
//! `docs/design/pathing.md`.
//!
//! # What is hashed
//!
//! [`Pathing::queue`] and [`Pathing::active`] (the request being searched
//! and its full [`AStarSearch`]: open heap as a sequence, closed/g/parent
//! arrays, expansion counter) are hashed sim state and serialise with the
//! snapshot, so a snapshot taken mid-search restores the exact search.
//! `std::collections::BinaryHeap` serialises in internal array order and
//! deserialises by pushing in that order, which reproduces the identical
//! array; implementer A adds a test that a mid-search snapshot/restore is
//! byte-exact. [`Pathing::cache`] is derived, `#[serde(skip)]`, never hashed,
//! and cleared on every `cost_grid_generation` change and on restore.
//!
//! # Grid, costs, heuristic
//!
//! 8-connected; an orthogonal step costs [`COST_ORTHOGONAL`] (10) and a
//! diagonal [`COST_DIAGONAL`] (14). Neighbours come from
//! [`Map::neighbors8`], which already applies the no-corner-cutting rule.
//! The heuristic is [`octile`], admissible and consistent for these costs.
//!
//! # Open set order
//!
//! [`Node`]'s `Ord` is defined so that `BinaryHeap` (a max-heap) pops the
//! smallest `f` first, then the largest `g`, then the smallest tile index:
//! a total order, so every machine pops the same node. Stale entries (a tile
//! already closed) are skipped when popped and do not count as expansions.
//!
//! # Budget and `resume`
//!
//! [`AStarSearch::resume`] expands nodes until the goal is popped
//! (`Found`), the heap empties (`Exhausted`) or `*budget` reaches zero
//! (`Suspended`). It decrements `*budget` by the expansions it performed so
//! several searches share one per-tick budget
//! (`rules.path_budget_expansions`). Expansions, not pops, are charged.
//!
//! # Request queue
//!
//! Requests are kept sorted by `(requested_tick, UnitId)`. Each tick
//! [`Pathing::service`] walks the queue in that order: it corrects the goal
//! (blocked or in another component -> `Map::nearest_passable` within the
//! start's component), completes trivially when start == goal, then consults
//! the cache and finally runs or resumes the search. A `Suspended` result
//! ends the tick's pathing work; the request stays at the head as `active`.
//! If the cost grid changes while a search is suspended, the next service
//! drops the search and requeues its request at the head, so the answer is
//! always for the current grid (`SIM_VERSION` 5).
//! The walk also stops as soon as the budget is spent, so the head request
//! of a saturated tick stays queued instead of becoming an `active` search
//! with zero expansions (`SIM_VERSION` 2).
//!
//! # The transparent cache rule (virtual budget)
//!
//! A cache entry stores the path *and* `expansions_used`, the total number of
//! expansions the original search spent. A hit must leave the hashed state at
//! the end of every tick identical to the state the same run reaches with the
//! cache disabled, so that `path_cache_is_transparent` can assert equal
//! hashes every tick. Therefore a hit is honoured only when
//! `expansions_used <= *budget` at the moment the request is reached: the
//! request then completes this tick with the cached path and `*budget` is
//! reduced by exactly `expansions_used`, which is precisely what the real
//! search would have done. When `expansions_used > *budget` the real search
//! would have suspended with a partially built heap in hashed state; no cache
//! replay can reproduce that, so the hit is ignored and a real search runs
//! (it suspends naturally and is cached when it completes). Timing, results
//! and hashes are thereby identical with the cache on or off; the cache only
//! saves work for searches that fit in the remaining budget.
//!
//! Cache key: `(start tile index, goal tile index, cost_grid_generation)`.
//! Eviction is deterministic: entries are stored in a `BTreeMap` keyed by an
//! insertion counter and the smallest counter is evicted at capacity. Whether
//! the cache evicts or hits never changes results, only work.

use crate::ids::{Tile, UnitId};
use crate::map::Map;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap};

/// Cost of an orthogonal step.
pub const COST_ORTHOGONAL: u32 = 10;
/// Cost of a diagonal step.
pub const COST_DIAGONAL: u32 = 14;
/// `g` value of an unvisited tile.
pub const G_UNVISITED: u32 = u32::MAX;
/// `parent` value of a tile without a predecessor (the start, or unvisited).
pub const NO_PARENT: u32 = u32::MAX;

/// Octile heuristic: `14 * min(dx, dy) + 10 * (max(dx, dy) - min(dx, dy))`.
pub fn octile(a: Tile, b: Tile) -> u32 {
    let dx = u32::from(a.x.abs_diff(b.x));
    let dy = u32::from(a.y.abs_diff(b.y));
    let (lo, hi) = if dx < dy { (dx, dy) } else { (dy, dx) };
    COST_DIAGONAL * lo + COST_ORTHOGONAL * (hi - lo)
}

/// Step cost between two 8-adjacent tiles.
pub fn step_cost(a: Tile, b: Tile) -> u32 {
    if a.x != b.x && a.y != b.y {
        COST_DIAGONAL
    } else {
        COST_ORTHOGONAL
    }
}

/// Total octile cost of a path given as consecutive tiles.
pub fn path_cost(path: &[Tile]) -> u32 {
    path.windows(2).map(|w| step_cost(w[0], w[1])).sum()
}

/// Outcome of one [`AStarSearch::resume`] call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchStatus {
    /// The goal was popped; the path runs from the start tile to the goal
    /// tile inclusive (a single tile when start == goal).
    Found(Vec<Tile>),
    /// The open set emptied: the goal is unreachable from the start.
    Exhausted,
    /// The budget ran out; call `resume` again next tick.
    Suspended,
}

/// An open-set entry. Ordered for a `BinaryHeap` so that the smallest `f`
/// pops first, then the largest `g`, then the smallest tile index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// `g + h`.
    pub f: u32,
    /// Cost from the start.
    pub g: u32,
    /// Row-major tile index.
    pub tile: u32,
}

impl Ord for Node {
    fn cmp(&self, o: &Node) -> Ordering {
        o.f.cmp(&self.f)
            .then(self.g.cmp(&o.g))
            .then(o.tile.cmp(&self.tile))
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, o: &Node) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// A resumable A* search. Hashed state while it is `Pathing::active`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AStarSearch {
    /// Start tile.
    pub from: Tile,
    /// Goal tile.
    pub to: Tile,
    /// Open set.
    pub open: BinaryHeap<Node>,
    /// Expanded tiles, one flag per map tile.
    pub closed: Vec<bool>,
    /// Best known cost per tile; [`G_UNVISITED`] when untouched.
    pub g: Vec<u32>,
    /// Predecessor tile index per tile; [`NO_PARENT`] when none.
    pub parent: Vec<u32>,
    /// Expansions performed so far across every `resume` call.
    pub expansions: u32,
    /// `Map::cost_grid_generation` the search started on. A suspended search
    /// whose grid has changed since is restarted by [`Pathing::service`]
    /// rather than resumed (`SIM_VERSION` 5).
    pub generation: u32,
}

impl PartialEq for AStarSearch {
    fn eq(&self, o: &AStarSearch) -> bool {
        self.from == o.from
            && self.to == o.to
            && self.expansions == o.expansions
            && self.generation == o.generation
            && self.closed == o.closed
            && self.g == o.g
            && self.parent == o.parent
            && self.open.len() == o.open.len()
            && self.open.iter().zip(o.open.iter()).all(|(a, b)| a == b)
    }
}

impl Eq for AStarSearch {}

impl AStarSearch {
    /// Start a search from `from` to `to` on `map`: arrays sized to the map,
    /// the start pushed with `g = 0`, `f = octile(from, to)`.
    pub fn new(map: &Map, from: Tile, to: Tile) -> AStarSearch {
        let n = map.len();
        let start = map.idx(from);
        let mut g = vec![G_UNVISITED; n];
        g[start] = 0;
        let mut open = BinaryHeap::new();
        open.push(Node {
            f: octile(from, to),
            g: 0,
            tile: tile_index(start),
        });
        AStarSearch {
            from,
            to,
            open,
            closed: vec![false; n],
            g,
            parent: vec![NO_PARENT; n],
            expansions: 0,
            generation: map.cost_grid_generation(),
        }
    }

    /// Pop and expand nodes until the goal is popped (`Found`), the heap is
    /// empty (`Exhausted`) or `*budget` reaches zero (`Suspended`).
    /// Decrements `*budget` by one per expansion. Skipped stale pops are free.
    /// The found path runs start -> goal inclusive, reconstructed through
    /// `parent` and reversed.
    ///
    /// An empty heap is reported as `Exhausted` even when `*budget` is zero;
    /// a zero budget with work left suspends without expanding anything.
    pub fn resume(&mut self, map: &Map, budget: &mut u32) -> SearchStatus {
        let goal = tile_index(map.idx(self.to));
        loop {
            let Some(&top) = self.open.peek() else {
                return SearchStatus::Exhausted;
            };
            let i = top.tile as usize;
            if self.closed[i] {
                // Stale entry: a better `g` already closed this tile.
                self.open.pop();
                continue;
            }
            if *budget == 0 {
                return SearchStatus::Suspended;
            }
            self.open.pop();
            self.closed[i] = true;
            self.expansions += 1;
            *budget -= 1;
            if top.tile == goal {
                return SearchStatus::Found(self.reconstruct(map));
            }
            let here = map.tile_at(i);
            for nb in map.neighbors8(here) {
                let j = map.idx(nb);
                if self.closed[j] {
                    continue;
                }
                let ng = top.g + step_cost(here, nb);
                if ng < self.g[j] {
                    self.g[j] = ng;
                    self.parent[j] = top.tile;
                    self.open.push(Node {
                        f: ng + octile(nb, self.to),
                        g: ng,
                        tile: tile_index(j),
                    });
                }
            }
        }
    }

    /// Walk `parent` from the goal back to the start and reverse.
    fn reconstruct(&self, map: &Map) -> Vec<Tile> {
        let mut path = Vec::new();
        let mut i = tile_index(map.idx(self.to));
        loop {
            path.push(map.tile_at(i as usize));
            let p = self.parent[i as usize];
            if p == NO_PARENT {
                break;
            }
            i = p;
        }
        path.reverse();
        debug_assert_eq!(path.first(), Some(&self.from));
        path
    }
}

/// Row-major index as the `u32` stored in [`Node::tile`] and `parent`.
fn tile_index(i: usize) -> u32 {
    u32::try_from(i).expect("tile index fits u32")
}

/// A unit's outstanding request for a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PathRequest {
    /// Tick the request was made; the primary sort key.
    pub requested_tick: u32,
    /// Requesting unit; the tiebreak. At most one request per unit is queued.
    pub unit: UnitId,
    /// Start tile (the unit's tile when the request was made).
    pub from: Tile,
    /// Goal tile (component-corrected again by `service`).
    pub to: Tile,
}

impl PathRequest {
    /// The total-order key the queue is sorted by.
    pub fn sort_key(&self) -> (u32, UnitId) {
        (self.requested_tick, self.unit)
    }
}

/// Cache key: `(start tile index, goal tile index, cost_grid_generation)`.
pub type CacheKey = (u32, u32, u32);

/// A cached search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    /// Path from start to goal inclusive.
    pub path: Vec<Tile>,
    /// Expansions the original search spent; charged on every hit.
    pub expansions_used: u32,
}

/// Bounded path cache with deterministic insertion-order eviction. Derived
/// state: never hashed, never serialised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathCache {
    /// Entries keyed by insertion counter (smallest is evicted first).
    pub entries: BTreeMap<u64, CacheEntry>,
    /// Lookup from cache key to insertion counter.
    pub by_key: BTreeMap<CacheKey, u64>,
    /// Maximum number of entries (`rules.path_cache_entries`).
    pub capacity: u32,
    /// `false` disables lookups and inserts (the transparency test flips it).
    pub enabled: bool,
    /// Next insertion counter.
    pub next_insert: u64,
}

impl Default for PathCache {
    fn default() -> PathCache {
        PathCache::new(0, false)
    }
}

impl PathCache {
    /// An empty cache.
    pub fn new(capacity: u32, enabled: bool) -> PathCache {
        PathCache {
            entries: BTreeMap::new(),
            by_key: BTreeMap::new(),
            capacity,
            enabled,
            next_insert: 0,
        }
    }

    /// Look up an entry. Always `None` when disabled.
    pub fn get(&self, key: CacheKey) -> Option<&CacheEntry> {
        if !self.enabled {
            return None;
        }
        self.by_key.get(&key).and_then(|i| self.entries.get(i))
    }

    /// Insert or replace an entry, evicting the oldest insertion at capacity.
    /// A no-op when disabled or when `capacity == 0`.
    pub fn insert(&mut self, key: CacheKey, entry: CacheEntry) {
        if !self.enabled || self.capacity == 0 {
            return;
        }
        if let Some(old) = self.by_key.remove(&key) {
            self.entries.remove(&old);
        }
        while self.entries.len() >= self.capacity as usize {
            let Some((&oldest, _)) = self.entries.iter().next() else {
                break;
            };
            self.entries.remove(&oldest);
            self.by_key.retain(|_, v| *v != oldest);
        }
        let i = self.next_insert;
        self.next_insert += 1;
        self.entries.insert(i, entry);
        self.by_key.insert(key, i);
    }

    /// Drop every entry; capacity and `enabled` are kept.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.by_key.clear();
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when no entries are stored.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The pathing subsystem: request queue, the active search and the cache.
///
/// Equality compares the hashed fields only (`queue` and `active`), like
/// `Map`'s, so two sims that agree on hashed state compare equal whether or
/// not their derived caches do.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pathing {
    /// Outstanding requests sorted by `(requested_tick, UnitId)`, excluding
    /// the active one.
    pub queue: Vec<PathRequest>,
    /// The request whose search is in progress (suspended across ticks).
    pub active: Option<(PathRequest, AStarSearch)>,
    /// Derived, unhashed path cache.
    #[serde(skip)]
    pub cache: PathCache,
}

impl PartialEq for Pathing {
    fn eq(&self, o: &Pathing) -> bool {
        self.queue == o.queue && self.active == o.active
    }
}

impl Eq for Pathing {}

impl Pathing {
    /// Empty subsystem with a cache of `cache_entries` capacity.
    pub fn new(cache_entries: u32, cache_enabled: bool) -> Pathing {
        Pathing {
            queue: Vec::new(),
            active: None,
            cache: PathCache::new(cache_entries, cache_enabled),
        }
    }

    /// Queue a request, replacing any earlier request or active search for
    /// the same unit, and keep the queue sorted.
    pub fn request(&mut self, req: PathRequest) {
        self.cancel(req.unit);
        let pos = self
            .queue
            .binary_search_by_key(&req.sort_key(), PathRequest::sort_key)
            .unwrap_or_else(|p| p);
        self.queue.insert(pos, req);
    }

    /// Drop the unit's queued request and its active search, if any.
    pub fn cancel(&mut self, unit: UnitId) {
        self.queue.retain(|r| r.unit != unit);
        if self.active.as_ref().is_some_and(|(r, _)| r.unit == unit) {
            self.active = None;
        }
    }

    /// `true` when nothing is queued or in progress.
    pub fn is_idle(&self) -> bool {
        self.queue.is_empty() && self.active.is_none()
    }

    /// Requests outstanding (queued plus active).
    pub fn outstanding(&self) -> usize {
        self.queue.len() + usize::from(self.active.is_some())
    }

    /// `true` when the unit has a queued or active request.
    pub fn has_request(&self, unit: UnitId) -> bool {
        self.queue.iter().any(|r| r.unit == unit)
            || self.active.as_ref().is_some_and(|(r, _)| r.unit == unit)
    }

    /// Spend up to `budget` expansions on the queue in order (see the module
    /// docs for goal correction, the cache rule and suspension). Returns the
    /// requests completed this tick in completion order: `Some(path)` with
    /// the start -> goal tiles (empty when start == corrected goal) or
    /// `None` when the goal is unreachable (defensive; the caller clears the
    /// unit's order and emits an event). Clears the cache first when
    /// `map.cost_grid_generation()` differs from the generation the cache
    /// was filled under (compare against the stored keys' third element).
    ///
    /// Requests whose `requested_tick` is later than `tick` are left queued
    /// (the queue is sorted, so the walk stops at the first one).
    pub fn service(
        &mut self,
        map: &Map,
        budget: u32,
        tick: u32,
    ) -> Vec<(UnitId, Option<Vec<Tile>>)> {
        let mut remaining = budget;
        self.service_budgeted(map, &mut remaining, tick)
    }

    /// [`Pathing::service`] with the budget passed by reference: on return
    /// `*remaining` holds the expansions left unspent this tick (always zero
    /// after a suspension). Tests use it to assert that the cache charges
    /// exactly what the real search would have; `Sim::step` calls `service`.
    pub fn service_budgeted(
        &mut self,
        map: &Map,
        remaining: &mut u32,
        tick: u32,
    ) -> Vec<(UnitId, Option<Vec<Tile>>)> {
        let generation = map.cost_grid_generation();
        if self
            .cache
            .by_key
            .keys()
            .next()
            .is_some_and(|k| k.2 != generation)
        {
            self.cache.clear();
        }
        let mut done = Vec::new();

        // 0. A search suspended on an older cost grid cannot be resumed: its
        //    `closed`, `g` and `parent` arrays describe the old grid, so it
        //    could return a path through a tile blocked since, and the cache
        //    would store that path under the new generation. Put its request
        //    back at the head of the queue (it is the oldest outstanding one)
        //    so step 2 corrects start and goal and searches the new grid.
        if let Some((req, _)) = self
            .active
            .take_if(|(_, search)| search.generation != generation)
        {
            self.request(req);
        }

        // 1. The suspended search goes first; it already owns the queue head.
        if let Some((req, search)) = self.active.as_mut() {
            match search.resume(map, remaining) {
                SearchStatus::Found(path) => {
                    let key = (
                        tile_index(map.idx(search.from)),
                        tile_index(map.idx(search.to)),
                        generation,
                    );
                    self.cache.insert(
                        key,
                        CacheEntry {
                            path: path.clone(),
                            expansions_used: search.expansions,
                        },
                    );
                    done.push((req.unit, Some(path)));
                    self.active = None;
                }
                SearchStatus::Exhausted => {
                    done.push((req.unit, None));
                    self.active = None;
                }
                SearchStatus::Suspended => return done,
            }
        }

        // 2. Queued requests in (requested_tick, UnitId) order. With no
        //    budget left nothing can complete (a trivial start == goal
        //    request is the one exception and simply waits a tick), so the
        //    walk stops before goal correction and before the cache lookup:
        //    a fresh search would only suspend with zero expansions and sit
        //    in hashed state, bloating every snapshot by its three
        //    map-sized arrays. Cache on and off take the same early exit.
        let mut consumed = 0;
        while consumed < self.queue.len() && *remaining > 0 {
            let req = self.queue[consumed];
            if req.requested_tick > tick {
                break;
            }
            consumed += 1;

            // Correct the start (a unit standing on a blocked tile) and the
            // goal (blocked, or in another component).
            let start = if map.passable(req.from) {
                req.from
            } else if let Some(s) = map.nearest_passable(req.from, None) {
                s
            } else {
                done.push((req.unit, None));
                continue;
            };
            let Some(goal) = map.nearest_passable(req.to, Some(map.component_of(start))) else {
                done.push((req.unit, None));
                continue;
            };
            if start == goal {
                done.push((req.unit, Some(Vec::new())));
                continue;
            }

            // Transparent cache: honour a hit only when the real search would
            // also have completed within this tick's remaining budget.
            let key = (
                tile_index(map.idx(start)),
                tile_index(map.idx(goal)),
                generation,
            );
            if let Some(entry) = self.cache.get(key)
                && entry.expansions_used <= *remaining
            {
                *remaining -= entry.expansions_used;
                done.push((req.unit, Some(entry.path.clone())));
                continue;
            }

            let mut search = AStarSearch::new(map, start, goal);
            match search.resume(map, remaining) {
                SearchStatus::Found(path) => {
                    self.cache.insert(
                        key,
                        CacheEntry {
                            path: path.clone(),
                            expansions_used: search.expansions,
                        },
                    );
                    done.push((req.unit, Some(path)));
                }
                SearchStatus::Exhausted => done.push((req.unit, None)),
                SearchStatus::Suspended => {
                    self.active = Some((req, search));
                    break;
                }
            }
        }
        self.queue.drain(..consumed);
        done
    }

    /// Drop every cache entry (grid change, restore, or the transparency test).
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// Enable or disable the cache (tests). Disabling clears it.
    pub fn set_cache_enabled(&mut self, enabled: bool) {
        self.cache.enabled = enabled;
        if !enabled {
            self.cache.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octile_matches_definition() {
        assert_eq!(octile(Tile::new(0, 0), Tile::new(0, 0)), 0);
        assert_eq!(octile(Tile::new(0, 0), Tile::new(3, 0)), 30);
        assert_eq!(octile(Tile::new(0, 0), Tile::new(3, 3)), 42);
        assert_eq!(octile(Tile::new(5, 1), Tile::new(0, 3)), 2 * 14 + 3 * 10);
        assert_eq!(step_cost(Tile::new(1, 1), Tile::new(2, 2)), COST_DIAGONAL);
        assert_eq!(step_cost(Tile::new(1, 1), Tile::new(1, 2)), COST_ORTHOGONAL);
        assert_eq!(
            path_cost(&[Tile::new(0, 0), Tile::new(1, 1), Tile::new(1, 2)]),
            24
        );
    }

    #[test]
    fn node_order_pops_min_f_then_max_g_then_min_tile() {
        let mut heap = BinaryHeap::new();
        heap.push(Node {
            f: 20,
            g: 5,
            tile: 3,
        });
        heap.push(Node {
            f: 10,
            g: 2,
            tile: 9,
        });
        heap.push(Node {
            f: 10,
            g: 7,
            tile: 8,
        });
        heap.push(Node {
            f: 10,
            g: 7,
            tile: 1,
        });
        let popped: Vec<Node> = std::iter::from_fn(|| heap.pop()).collect();
        assert_eq!(
            popped,
            vec![
                Node {
                    f: 10,
                    g: 7,
                    tile: 1
                },
                Node {
                    f: 10,
                    g: 7,
                    tile: 8
                },
                Node {
                    f: 10,
                    g: 2,
                    tile: 9
                },
                Node {
                    f: 20,
                    g: 5,
                    tile: 3
                },
            ]
        );
    }

    #[test]
    fn binary_heap_serde_round_trip_preserves_array_order() {
        let mut heap = BinaryHeap::new();
        for i in 0..64u32 {
            heap.push(Node {
                f: (i * 7919) % 23,
                g: (i * 31) % 11,
                tile: i,
            });
        }
        heap.pop();
        heap.pop();
        let before: Vec<Node> = heap.iter().copied().collect();
        let bytes = postcard::to_allocvec(&heap).unwrap();
        let back: BinaryHeap<Node> = postcard::from_bytes(&bytes).unwrap();
        let after: Vec<Node> = back.iter().copied().collect();
        assert_eq!(before, after, "heap array order survives serde");
        assert_eq!(postcard::to_allocvec(&back).unwrap(), bytes);
    }

    #[test]
    fn queue_is_sorted_and_one_request_per_unit() {
        let mut p = Pathing::new(8, true);
        let mk = |t: u32, u: u32| PathRequest {
            requested_tick: t,
            unit: UnitId(u),
            from: Tile::new(0, 0),
            to: Tile::new(1, 1),
        };
        p.request(mk(5, 2));
        p.request(mk(3, 9));
        p.request(mk(5, 1));
        p.request(mk(3, 9)); // replaces itself
        let keys: Vec<_> = p.queue.iter().map(PathRequest::sort_key).collect();
        assert_eq!(keys, vec![(3, UnitId(9)), (5, UnitId(1)), (5, UnitId(2))]);
        assert!(p.has_request(UnitId(2)));
        p.cancel(UnitId(2));
        assert!(!p.has_request(UnitId(2)));
        assert_eq!(p.outstanding(), 2);
        assert!(!p.is_idle());
    }

    /// A serpentine maze: every odd row but the last is a wall with a
    /// one-tile gap at alternating ends, so the only route winds through
    /// every lane. The last row is always open.
    fn serpentine(w: u16, h: u16) -> Map {
        let mut m = Map::open(w, h);
        for y in (1..h - 1).step_by(2) {
            let gap = if (y / 2) % 2 == 0 { w - 1 } else { 0 };
            for x in 0..w {
                if x != gap {
                    m.set_blocked(Tile::new(x, y), true);
                }
            }
        }
        m
    }

    fn run_full(map: &Map, from: Tile, to: Tile) -> (SearchStatus, u32) {
        let mut s = AStarSearch::new(map, from, to);
        let mut budget = u32::MAX;
        let status = s.resume(map, &mut budget);
        (status, s.expansions)
    }

    #[test]
    fn new_pushes_the_start_and_a_trivial_search_is_one_expansion() {
        let m = Map::open(8, 8);
        let s = AStarSearch::new(&m, Tile::new(2, 3), Tile::new(5, 3));
        assert_eq!(s.open.len(), 1);
        assert_eq!(
            s.open.peek(),
            Some(&Node {
                f: 30,
                g: 0,
                tile: 26
            })
        );
        assert_eq!(s.g[26], 0);
        assert!(s.g.iter().filter(|&&g| g != G_UNVISITED).count() == 1);
        assert!(s.parent.iter().all(|&p| p == NO_PARENT));
        assert_eq!(s.expansions, 0);

        let (status, expansions) = run_full(&m, Tile::new(2, 3), Tile::new(2, 3));
        assert_eq!(status, SearchStatus::Found(vec![Tile::new(2, 3)]));
        assert_eq!(expansions, 1);

        let (status, _) = run_full(&m, Tile::new(0, 0), Tile::new(3, 3));
        let SearchStatus::Found(p) = status else {
            panic!("{status:?}");
        };
        assert_eq!(p.len(), 4, "pure diagonal");
        assert_eq!(path_cost(&p), 42);
        assert_eq!((p[0], p[3]), (Tile::new(0, 0), Tile::new(3, 3)));
    }

    #[test]
    fn budget_suspends_and_resumes_to_the_same_path() {
        let m = serpentine(16, 16);
        let from = Tile::new(0, 0);
        let to = Tile::new(15, 15);
        let (full, full_expansions) = run_full(&m, from, to);
        let SearchStatus::Found(full_path) = full else {
            panic!("{full:?}");
        };
        assert!(full_expansions > 100, "maze forces a long search");

        // Zero budget: suspended, nothing expanded, heap untouched.
        let mut s = AStarSearch::new(&m, from, to);
        let before = s.clone();
        let mut zero = 0;
        assert_eq!(s.resume(&m, &mut zero), SearchStatus::Suspended);
        assert_eq!(s, before);

        // One expansion per call reaches the identical path and count.
        let mut calls = 0;
        let found = loop {
            let mut one = 1;
            calls += 1;
            match s.resume(&m, &mut one) {
                SearchStatus::Suspended => {
                    assert_eq!(one, 0, "a suspended call spends its budget");
                }
                SearchStatus::Found(p) => break p,
                SearchStatus::Exhausted => panic!("reachable"),
            }
        };
        assert_eq!(found, full_path);
        assert_eq!(s.expansions, full_expansions);
        assert_eq!(calls, full_expansions);

        // A budget larger than needed is only partially spent.
        let mut s = AStarSearch::new(&m, from, to);
        let mut big = full_expansions + 1000;
        assert_eq!(s.resume(&m, &mut big), SearchStatus::Found(full_path));
        assert_eq!(big, 1000);
    }

    #[test]
    fn exhausted_when_the_goal_is_walled_off() {
        let mut m = Map::open(8, 8);
        for y in 0..8 {
            m.set_blocked(Tile::new(4, y), true);
        }
        let (status, expansions) = run_full(&m, Tile::new(0, 0), Tile::new(7, 7));
        assert_eq!(status, SearchStatus::Exhausted);
        assert_eq!(expansions, 32, "the whole start component is expanded");
        // Exhausted is reported even when the budget is already zero.
        let mut s = AStarSearch::new(&m, Tile::new(0, 0), Tile::new(7, 7));
        let mut b = 40;
        assert_eq!(s.resume(&m, &mut b), SearchStatus::Exhausted);
        let mut zero = 0;
        assert_eq!(s.resume(&m, &mut zero), SearchStatus::Exhausted);
    }

    #[test]
    fn mid_search_snapshot_restore_is_byte_exact() {
        let m = serpentine(24, 24);
        let from = Tile::new(0, 0);
        let to = Tile::new(23, 23);
        let mut a = AStarSearch::new(&m, from, to);
        let mut k = 97;
        assert_eq!(a.resume(&m, &mut k), SearchStatus::Suspended);
        assert_eq!(a.expansions, 97);
        let bytes = postcard::to_allocvec(&a).unwrap();
        let mut b: AStarSearch = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(a, b, "heap array order, arrays and counter restore exactly");
        assert_eq!(postcard::to_allocvec(&b).unwrap(), bytes);
        assert_eq!(crate::hash::hash_value(&a), crate::hash::hash_value(&b));

        // Step both in lockstep: every intermediate state stays identical.
        loop {
            let (mut ba, mut bb) = (7, 7);
            let ra = a.resume(&m, &mut ba);
            let rb = b.resume(&m, &mut bb);
            assert_eq!(ra, rb);
            assert_eq!(ba, bb);
            assert_eq!(a, b);
            assert_eq!(
                postcard::to_allocvec(&a).unwrap(),
                postcard::to_allocvec(&b).unwrap()
            );
            match ra {
                SearchStatus::Suspended => {}
                SearchStatus::Found(p) => {
                    let (full, _) = run_full(&m, from, to);
                    assert_eq!(full, SearchStatus::Found(p));
                    break;
                }
                SearchStatus::Exhausted => panic!("maze is connected"),
            }
        }
    }

    fn req(tick: u32, unit: u32, from: Tile, to: Tile) -> PathRequest {
        PathRequest {
            requested_tick: tick,
            unit: UnitId(unit),
            from,
            to,
        }
    }

    #[test]
    fn service_corrects_start_and_goal_and_completes_trivial_requests() {
        // Column 4 is a wall; (6,2) is a blocked island on the right.
        let mut m = Map::open(8, 8);
        for y in 0..8 {
            m.set_blocked(Tile::new(4, y), true);
        }
        m.set_blocked(Tile::new(6, 2), true);
        let mut p = Pathing::new(8, true);
        // Goal in the other component: corrected to the nearest passable tile
        // of the start's component (BFS from (7,7): E, SE, S, SW, W ... first
        // left-side tile is reached at (3,7)).
        p.request(req(0, 1, Tile::new(0, 0), Tile::new(7, 7)));
        // Goal blocked: corrected to (7,2) (east first).
        p.request(req(0, 2, Tile::new(5, 0), Tile::new(6, 2)));
        // Start == goal: empty path, no expansions.
        p.request(req(0, 3, Tile::new(1, 1), Tile::new(1, 1)));
        // Start blocked: corrected to (5,3) (east of the wall), then a path.
        p.request(req(0, 4, Tile::new(4, 3), Tile::new(7, 3)));
        let mut remaining = 1000;
        let done = p.service_budgeted(&m, &mut remaining, 0);
        assert_eq!(done.len(), 4);
        assert_eq!(done[0].0, UnitId(1));
        let p1 = done[0].1.as_ref().unwrap();
        assert_eq!(p1.first(), Some(&Tile::new(0, 0)));
        assert_eq!(p1.last(), Some(&Tile::new(3, 7)));
        let p2 = done[1].1.as_ref().unwrap();
        assert_eq!(p2.last(), Some(&Tile::new(7, 2)));
        assert_eq!(done[2], (UnitId(3), Some(vec![])));
        let p4 = done[3].1.as_ref().unwrap();
        assert_eq!(p4.first(), Some(&Tile::new(5, 3)));
        assert_eq!(p4.last(), Some(&Tile::new(7, 3)));
        assert!(p.is_idle());
        assert!(remaining < 1000 && remaining > 900);
        assert_eq!(p.cache.len(), 3, "three real searches cached");

        // A fully blocked map: nothing reachable.
        let mut b = Map::open(2, 2);
        for i in 0..4 {
            b.set_blocked(b.tile_at(i), true);
        }
        let mut q = Pathing::new(8, true);
        q.request(req(0, 9, Tile::new(0, 0), Tile::new(1, 1)));
        assert_eq!(q.service(&b, 10, 0), vec![(UnitId(9), None)]);
    }

    #[test]
    fn service_suspends_across_ticks_and_shares_one_budget() {
        let m = serpentine(16, 16);
        let far = Tile::new(15, 15);
        let (_, full) = run_full(&m, Tile::new(0, 0), far);
        let mut p = Pathing::new(8, false);
        p.request(req(0, 2, Tile::new(0, 0), far));
        p.request(req(0, 1, Tile::new(0, 0), Tile::new(3, 0)));
        p.request(req(1, 3, Tile::new(0, 0), Tile::new(1, 0)));
        // Tick 0: unit 1 (same tick, lower id) completes with 4 expansions,
        // unit 2 takes the rest and suspends; unit 3 (tick 1) is not touched.
        let mut remaining = 20;
        let done = p.service_budgeted(&m, &mut remaining, 0);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].0, UnitId(1));
        assert_eq!(remaining, 0);
        let active = p.active.as_ref().expect("suspended");
        assert_eq!(active.0.unit, UnitId(2));
        assert_eq!(active.1.expansions, 16);
        assert_eq!(p.queue.len(), 1);
        assert!(p.has_request(UnitId(2)) && p.has_request(UnitId(3)));
        // Later ticks: the active search resumes first and charges the shared
        // budget before the queue is touched.
        let mut ticks = 1;
        let mut total_remaining = 0;
        let done = loop {
            let mut r = 20;
            let d = p.service_budgeted(&m, &mut r, ticks);
            total_remaining += r;
            if !d.is_empty() {
                break d;
            }
            ticks += 1;
        };
        assert_eq!(done[0].0, UnitId(2));
        let path = done[0].1.as_ref().unwrap();
        assert_eq!(path.last(), Some(&far));
        // `full - 16` expansions remained after tick 0, 20 per tick.
        assert_eq!(ticks, (full - 16).div_ceil(20));
        assert_eq!(done.len(), 2, "unit 3 finishes in the same tick");
        assert_eq!(done[1].0, UnitId(3));
        assert_eq!(total_remaining, 20 * ticks - (full - 16) - 2);
        assert!(p.is_idle());
        assert!(p.cache.is_empty(), "disabled cache stores nothing");
    }

    #[test]
    fn service_leaves_future_requests_queued() {
        let m = Map::open(8, 8);
        let mut p = Pathing::new(8, true);
        p.request(req(5, 1, Tile::new(0, 0), Tile::new(1, 0)));
        assert!(p.service(&m, 100, 4).is_empty());
        assert_eq!(p.queue.len(), 1);
        assert_eq!(p.service(&m, 100, 5).len(), 1);
        assert!(p.is_idle());
    }

    #[test]
    fn cache_hit_charges_expansions_and_is_ignored_when_it_does_not_fit() {
        let m = serpentine(16, 16);
        let from = Tile::new(0, 0);
        let to = Tile::new(15, 4);
        let (_, cost) = run_full(&m, from, to);
        assert!(cost > 10);
        let mut p = Pathing::new(8, true);
        p.request(req(0, 1, from, to));
        let mut r = cost + 5;
        let first = p.service_budgeted(&m, &mut r, 0);
        assert_eq!(r, 5);
        let key = (
            0,
            u32::from(to.y) * 16 + u32::from(to.x),
            m.cost_grid_generation(),
        );
        assert_eq!(p.cache.get(key).unwrap().expansions_used, cost);

        // Fits: the hit completes the request and charges exactly `cost`.
        p.request(req(1, 2, from, to));
        let mut r = cost;
        let hit = p.service_budgeted(&m, &mut r, 1);
        assert_eq!(hit[0].1, first[0].1);
        assert_eq!(r, 0);
        assert!(p.is_idle());

        // Does not fit: the hit is ignored and a real search suspends with
        // exactly the same state a cache-less run reaches.
        p.request(req(2, 3, from, to));
        let mut r = cost - 1;
        assert!(p.service_budgeted(&m, &mut r, 2).is_empty());
        assert_eq!(r, 0);
        let active = p.active.clone().expect("real search suspended");
        assert_eq!(active.1.expansions, cost - 1);
        let mut plain = Pathing::new(8, false);
        plain.request(req(2, 3, from, to));
        let mut r = cost - 1;
        assert!(plain.service_budgeted(&m, &mut r, 2).is_empty());
        assert_eq!(plain.active, Some(active));
        assert_eq!(
            postcard::to_allocvec(&p).unwrap(),
            postcard::to_allocvec(&plain).unwrap()
        );
        // Both finish on the next tick with one more expansion.
        let mut r = 10;
        let a = p.service_budgeted(&m, &mut r, 3);
        let mut r2 = 10;
        let b = plain.service_budgeted(&m, &mut r2, 3);
        assert_eq!(a, b);
        assert_eq!((r, r2), (9, 9));
        assert_eq!(a[0].1, first[0].1);
    }

    #[test]
    fn cache_cleared_on_generation_change() {
        let mut m = Map::open(8, 8);
        let mut p = Pathing::new(8, true);
        p.request(req(0, 1, Tile::new(0, 0), Tile::new(7, 0)));
        p.service(&m, 100, 0);
        assert_eq!(p.cache.len(), 1);
        assert!(p.cache.get((0, 7, 0)).is_some());
        // A no-op write keeps the generation and the cache.
        assert!(!m.set_blocked(Tile::new(3, 3), false));
        p.request(req(1, 2, Tile::new(0, 0), Tile::new(7, 0)));
        p.service(&m, 100, 1);
        assert_eq!(p.cache.len(), 1);
        // A real change clears it on the next service and re-keys new entries.
        assert!(m.set_blocked(Tile::new(3, 3), true));
        p.request(req(2, 3, Tile::new(0, 0), Tile::new(7, 0)));
        p.service(&m, 100, 2);
        assert_eq!(p.cache.len(), 1);
        assert!(p.cache.get((0, 7, 0)).is_none(), "old generation gone");
        assert!(p.cache.get((0, 7, 1)).is_some());
        // Even with nothing queued the stale cache is dropped.
        m.set_blocked(Tile::new(3, 4), true);
        assert!(p.service(&m, 100, 3).is_empty());
        assert!(p.cache.is_empty());
    }

    /// A search suspended on one cost grid and resumed after the grid
    /// changed must answer for the new grid: the same path a fresh search
    /// finds, cached under the new generation, never through a tile the
    /// change blocked. Without the restart the resumed search keeps the
    /// `closed` and `parent` arrays it built on the old grid.
    #[test]
    fn suspended_search_restarts_when_the_grid_changes() {
        let mut m = Map::open(32, 32);
        let from = Tile::new(0, 16);
        let to = Tile::new(31, 16);
        let mut p = Pathing::new(8, true);
        p.request(req(0, 1, from, to));
        // 25 expansions walk the straight row past column 16 and suspend.
        assert!(
            p.service(&m, 25, 0).is_empty(),
            "suspends on a small budget"
        );
        assert!(
            p.active
                .as_ref()
                .is_some_and(|(_, s)| s.closed[m.idx(Tile::new(20, 16))])
        );

        // A wall down column 16 with one gap at the top cuts the row the search
        // already expanded through, so its parent chain crosses the wall.
        for y in 1..32 {
            m.set_blocked(Tile::new(16, y), true);
        }
        let done = p.service(&m, u32::MAX, 1);
        let [(unit, Some(path))] = done.as_slice() else {
            panic!("expected one completed path, got {done:?}");
        };
        assert_eq!(*unit, UnitId(1));
        assert!(
            path.iter().all(|t| m.passable(*t)),
            "the resumed path crosses a tile the change blocked: {path:?}"
        );
        let (fresh, fresh_expansions) = run_full(&m, from, to);
        assert_eq!(fresh, SearchStatus::Found(path.clone()));
        let cached = p
            .cache
            .get((
                tile_index(m.idx(from)),
                tile_index(m.idx(to)),
                m.cost_grid_generation(),
            ))
            .expect("cached under the new generation");
        assert_eq!(&cached.path, path);
        assert_eq!(
            cached.expansions_used, fresh_expansions,
            "a later cache hit charges what a fresh search on this grid costs"
        );
    }

    #[test]
    fn cache_is_bounded_with_oldest_first_eviction_and_skipped_by_serde() {
        let mut c = PathCache::new(2, true);
        let e = |n: u32| CacheEntry {
            path: vec![],
            expansions_used: n,
        };
        c.insert((0, 1, 0), e(1));
        c.insert((0, 2, 0), e(2));
        c.insert((0, 3, 0), e(3));
        assert_eq!(c.len(), 2);
        assert!(c.get((0, 1, 0)).is_none(), "oldest evicted");
        assert_eq!(c.get((0, 3, 0)).unwrap().expansions_used, 3);
        c.insert((0, 2, 0), e(22));
        assert_eq!(c.get((0, 2, 0)).unwrap().expansions_used, 22);
        assert_eq!(c.len(), 2);
        c.clear();
        assert!(c.is_empty());
        let mut d = PathCache::new(2, false);
        d.insert((0, 1, 0), e(1));
        assert!(d.get((0, 1, 0)).is_none(), "disabled cache stores nothing");

        let mut p = Pathing::new(4, true);
        p.cache.insert((1, 2, 3), e(9));
        let bytes = postcard::to_allocvec(&p).unwrap();
        let back: Pathing = postcard::from_bytes(&bytes).unwrap();
        assert!(back.cache.is_empty(), "cache is not serialised");
        assert_eq!(back.queue, p.queue);
        assert_eq!(back, p, "equality ignores the derived cache");
        let mut q = p.clone();
        q.cache.clear();
        assert_eq!(q, p);
        q.request(req(0, 1, Tile::new(0, 0), Tile::new(1, 1)));
        assert_ne!(q, p, "equality covers the queue");
    }
}
