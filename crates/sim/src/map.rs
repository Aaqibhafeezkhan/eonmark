//! The runtime tile map: a cost grid with a generation counter plus the
//! derived connected-component ids.
//!
//! `Map` is hashed sim state (buildings mutate the cost grid from M3a), so
//! `cost` and `cost_grid_generation` serialise; `components` is derived and
//! skipped. [`Map::set_blocked`] bumps the generation and rebuilds the
//! components eagerly, and [`crate::Sim::restore`] calls
//! [`Map::rebuild_components`] before returning, so a `&Map` always serves a
//! fresh component id (see `docs/design/pathing.md`, "Cost grid").
//!
//! Two fixed neighbour orders exist and both are total and documented:
//!
//! - [`Map::neighbors8`] yields passable neighbours in the order
//!   N, NE, E, SE, S, SW, W, NW and forbids corner cutting: a diagonal step
//!   is legal only when both orthogonal tiles it passes are passable. A* and
//!   the component flood fill use it, so "connected" means "A* can path".
//! - [`Map::nearest_passable`] runs its BFS over every in-bounds neighbour
//!   (blocked ones too, since it searches outward from a blocked tile) in the
//!   order E, SE, S, SW, W, NW, N, NE given by `docs/design/pathing.md`.
//!
//! [`Map::spiral`] walks a square spiral from a centre tile: the centre, then
//! each ring at Chebyshev distance `r` starting at the east tile and going
//! clockwise on screen (south first, since `y` grows south).

use crate::fx::{Fx, FxVec2};
use crate::ids::Tile;
use rules::{MapDef, Terrain};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Cost of a passable tile.
pub const COST_PASSABLE: u8 = 1;
/// Cost of a blocked tile.
pub const COST_BLOCKED: u8 = 255;
/// Component id of a blocked tile.
pub const NO_COMPONENT: u16 = u16::MAX;

/// Neighbour offsets `(dx, dy)` in the order N, NE, E, SE, S, SW, W, NW.
pub const NEIGHBOR_OFFSETS: [(i32, i32); 8] = [
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
];

/// Neighbour offsets for the BFS fallback, E, SE, S, SW, W, NW, N, NE.
pub const BFS_NEIGHBOR_OFFSETS: [(i32, i32); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

/// The runtime map. See the module docs for what is hashed and what is derived.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Map {
    width: u16,
    height: u16,
    /// Row-major cost per tile, `index = y * width + x`.
    cost: Vec<u8>,
    /// Incremented on every cost change; keys the path cache and components.
    cost_grid_generation: u32,
    /// Derived: connected component id per tile, `NO_COMPONENT` when blocked.
    #[serde(skip)]
    components: Vec<u16>,
    /// Derived: number of components.
    #[serde(skip)]
    component_count: u16,
}

impl PartialEq for Map {
    fn eq(&self, o: &Map) -> bool {
        self.width == o.width
            && self.height == o.height
            && self.cost == o.cost
            && self.cost_grid_generation == o.cost_grid_generation
    }
}

impl Eq for Map {}

impl Map {
    /// Build the runtime map from a validated map definition: Grass is
    /// passable, every other terrain is blocked. Generation starts at 0.
    pub fn from_def(def: &MapDef) -> Map {
        let cost = def
            .tiles()
            .iter()
            .map(|t| {
                if *t == Terrain::Grass {
                    COST_PASSABLE
                } else {
                    COST_BLOCKED
                }
            })
            .collect();
        let mut map = Map {
            width: def.width,
            height: def.height,
            cost,
            cost_grid_generation: 0,
            components: Vec::new(),
            component_count: 0,
        };
        map.rebuild_components();
        map
    }

    /// An all-passable map, for tests and benches.
    pub fn open(width: u16, height: u16) -> Map {
        let mut map = Map {
            width,
            height,
            cost: vec![COST_PASSABLE; usize::from(width) * usize::from(height)],
            cost_grid_generation: 0,
            components: Vec::new(),
            component_count: 0,
        };
        map.rebuild_components();
        map
    }

    /// Columns.
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Rows.
    pub fn height(&self) -> u16 {
        self.height
    }

    /// Number of tiles.
    pub fn len(&self) -> usize {
        self.cost.len()
    }

    /// `true` for a degenerate empty map.
    pub fn is_empty(&self) -> bool {
        self.cost.is_empty()
    }

    /// The cost grid, row-major.
    pub fn cost(&self) -> &[u8] {
        &self.cost
    }

    /// Bumped on every cost change.
    pub fn cost_grid_generation(&self) -> u32 {
        self.cost_grid_generation
    }

    /// Row-major index of a tile. Panics when out of bounds.
    pub fn idx(&self, t: Tile) -> usize {
        assert!(self.in_bounds(t), "tile ({}, {}) outside map", t.x, t.y);
        usize::from(t.y) * usize::from(self.width) + usize::from(t.x)
    }

    /// Inverse of [`Map::idx`].
    pub fn tile_at(&self, index: usize) -> Tile {
        let w = usize::from(self.width);
        Tile::new(
            u16::try_from(index % w).expect("fits"),
            u16::try_from(index / w).expect("fits"),
        )
    }

    /// `true` when the tile lies inside the map.
    pub fn in_bounds(&self, t: Tile) -> bool {
        t.x < self.width && t.y < self.height
    }

    /// The tile containing a world position: floor of each coordinate,
    /// clamped into the map so positions on the far edge map to the last tile.
    pub fn tile_of(&self, p: FxVec2) -> Tile {
        let clamp = |v: i32, max: u16| -> u16 {
            let hi = i32::from(max) - 1;
            u16::try_from(v.clamp(0, hi)).expect("clamped into u16")
        };
        Tile::new(
            clamp(p.x.floor_to_int(), self.width),
            clamp(p.y.floor_to_int(), self.height),
        )
    }

    /// World position of a tile's centre: `(x + 0.5, y + 0.5)`.
    pub fn center_of(&self, t: Tile) -> FxVec2 {
        FxVec2::new(
            Fx::from_int(i32::from(t.x)) + Fx::HALF,
            Fx::from_int(i32::from(t.y)) + Fx::HALF,
        )
    }

    /// Cost of a tile. Panics when out of bounds.
    pub fn cost_of(&self, t: Tile) -> u8 {
        self.cost[self.idx(t)]
    }

    /// `true` when the tile is inside the map and not blocked.
    pub fn passable(&self, t: Tile) -> bool {
        self.in_bounds(t) && self.cost[self.idx(t)] != COST_BLOCKED
    }

    /// Block or unblock a tile. Returns `true` and bumps the generation (and
    /// rebuilds the components) only when the cost actually changed, so a
    /// no-op write never invalidates the path cache.
    pub fn set_blocked(&mut self, t: Tile, blocked: bool) -> bool {
        let i = self.idx(t);
        let new = if blocked { COST_BLOCKED } else { COST_PASSABLE };
        if self.cost[i] == new {
            return false;
        }
        self.cost[i] = new;
        self.cost_grid_generation = self
            .cost_grid_generation
            .checked_add(1)
            .expect("cost_grid_generation overflow");
        self.rebuild_components();
        true
    }

    /// Block or unblock several tiles at once (a building footprint). Bumps
    /// the generation once and rebuilds the components once, and only when
    /// at least one tile's cost actually changed; returns whether it did.
    /// Equivalent to calling [`Map::set_blocked`] per tile except for the
    /// number of generation bumps, which is what the path cache keys on.
    pub fn set_blocked_many(&mut self, tiles: &[Tile], blocked: bool) -> bool {
        let new = if blocked { COST_BLOCKED } else { COST_PASSABLE };
        let mut changed = false;
        for &t in tiles {
            let i = self.idx(t);
            if self.cost[i] != new {
                self.cost[i] = new;
                changed = true;
            }
        }
        if changed {
            self.cost_grid_generation = self
                .cost_grid_generation
                .checked_add(1)
                .expect("cost_grid_generation overflow");
            self.rebuild_components();
        }
        changed
    }

    /// Offset a tile; `None` when the result leaves the map.
    pub fn offset(&self, t: Tile, dx: i32, dy: i32) -> Option<Tile> {
        let x = i32::from(t.x) + dx;
        let y = i32::from(t.y) + dy;
        if x < 0 || y < 0 || x >= i32::from(self.width) || y >= i32::from(self.height) {
            return None;
        }
        Some(Tile::new(
            u16::try_from(x).expect("checked"),
            u16::try_from(y).expect("checked"),
        ))
    }

    /// Passable 8-neighbours of `t` in the fixed order N, NE, E, SE, S, SW,
    /// W, NW. A diagonal neighbour is yielded only when both orthogonal tiles
    /// between `t` and it are passable (no corner cutting). `t` itself may be
    /// blocked; the rule is evaluated on the neighbours only.
    pub fn neighbors8(&self, t: Tile) -> impl Iterator<Item = Tile> + '_ {
        NEIGHBOR_OFFSETS.iter().filter_map(move |&(dx, dy)| {
            let n = self.offset(t, dx, dy)?;
            if !self.passable(n) {
                return None;
            }
            if dx != 0 && dy != 0 {
                let a = self.offset(t, dx, 0)?;
                let b = self.offset(t, 0, dy)?;
                if !self.passable(a) || !self.passable(b) {
                    return None;
                }
            }
            Some(n)
        })
    }

    /// `true` when `a` and `b` are 8-adjacent and the step is legal under the
    /// corner-cutting rule (both passable, orthogonal tiles clear).
    pub fn step_allowed(&self, a: Tile, b: Tile) -> bool {
        self.passable(a) && self.neighbors8(a).any(|n| n == b)
    }

    /// Connected component id of a tile; `NO_COMPONENT` when blocked.
    pub fn component_of(&self, t: Tile) -> u16 {
        self.components[self.idx(t)]
    }

    /// Number of connected components.
    pub fn component_count(&self) -> u16 {
        self.component_count
    }

    /// Component ids, row-major (derived; never hashed).
    pub fn components(&self) -> &[u16] {
        &self.components
    }

    /// Recompute `components` by flood fill in index order with the
    /// [`Map::neighbors8`] connectivity. Called by [`Map::set_blocked`] and by
    /// `Sim::restore`; idempotent.
    pub fn rebuild_components(&mut self) {
        let n = self.cost.len();
        let mut comps = vec![NO_COMPONENT; n];
        let mut next: u16 = 0;
        let mut stack: Vec<usize> = Vec::new();
        for start in 0..n {
            if self.cost[start] == COST_BLOCKED || comps[start] != NO_COMPONENT {
                continue;
            }
            assert!(next < NO_COMPONENT, "too many components");
            comps[start] = next;
            stack.push(start);
            while let Some(i) = stack.pop() {
                let t = self.tile_at(i);
                for nb in self.neighbors8(t) {
                    let j = self.idx(nb);
                    if comps[j] == NO_COMPONENT {
                        comps[j] = next;
                        stack.push(j);
                    }
                }
            }
            next += 1;
        }
        self.components = comps;
        self.component_count = next;
    }

    /// Breadth-first search outward from `from` over every in-bounds tile in
    /// the fixed order E, SE, S, SW, W, NW, N, NE, returning the first
    /// passable tile whose component equals `want` (any component when
    /// `None`). `from` itself is returned when it qualifies. `None` when no
    /// such tile exists on the map.
    pub fn nearest_passable(&self, from: Tile, want: Option<u16>) -> Option<Tile> {
        if !self.in_bounds(from) {
            return None;
        }
        let ok = |t: Tile| self.passable(t) && want.is_none_or(|w| self.component_of(t) == w);
        if ok(from) {
            return Some(from);
        }
        let mut seen = vec![false; self.cost.len()];
        let mut queue = VecDeque::new();
        seen[self.idx(from)] = true;
        queue.push_back(from);
        while let Some(t) = queue.pop_front() {
            for &(dx, dy) in &BFS_NEIGHBOR_OFFSETS {
                let Some(n) = self.offset(t, dx, dy) else {
                    continue;
                };
                let i = self.idx(n);
                if seen[i] {
                    continue;
                }
                seen[i] = true;
                if ok(n) {
                    return Some(n);
                }
                queue.push_back(n);
            }
        }
        None
    }

    /// Square spiral of in-bounds tiles starting at `center`: the centre,
    /// then each ring at Chebyshev distance 1, 2, ... starting at the ring's
    /// east tile and proceeding clockwise on screen (south, west, north).
    /// Every tile of the map is yielded exactly once; tiles outside the map
    /// are skipped. Passability is not checked; callers filter.
    pub fn spiral(&self, center: Tile) -> Spiral {
        let cx = i32::from(center.x);
        let cy = i32::from(center.y);
        let max_r = [
            cx,
            i32::from(self.width) - 1 - cx,
            cy,
            i32::from(self.height) - 1 - cy,
        ]
        .into_iter()
        .max()
        .unwrap_or(0);
        Spiral {
            width: i32::from(self.width),
            height: i32::from(self.height),
            cx,
            cy,
            r: 0,
            i: 0,
            max_r,
        }
    }
}

/// Iterator returned by [`Map::spiral`].
#[derive(Debug, Clone)]
pub struct Spiral {
    width: i32,
    height: i32,
    cx: i32,
    cy: i32,
    /// Current ring (Chebyshev distance).
    r: i32,
    /// Position along the current ring's perimeter, `0..8r` (`0..1` for `r == 0`).
    i: i32,
    /// Largest ring that still touches the map.
    max_r: i32,
}

impl Spiral {
    /// Tile `i` of ring `r` relative to the centre, clockwise from east.
    fn ring_offset(r: i32, i: i32) -> (i32, i32) {
        debug_assert!(r >= 1 && (0..8 * r).contains(&i));
        if i <= r {
            // East edge, from (r, 0) south to (r, r).
            (r, i)
        } else if i <= 3 * r {
            // South edge, from (r - 1, r) west to (-r, r).
            (r - (i - r), r)
        } else if i <= 5 * r {
            // West edge, from (-r, r - 1) north to (-r, -r).
            (-r, r - (i - 3 * r))
        } else if i <= 7 * r {
            // North edge, from (-r + 1, -r) east to (r, -r).
            (-r + (i - 5 * r), -r)
        } else {
            // East edge again, from (r, -r + 1) south to (r, -1).
            (r, -r + (i - 7 * r))
        }
    }
}

impl Iterator for Spiral {
    type Item = Tile;

    fn next(&mut self) -> Option<Tile> {
        loop {
            if self.r > self.max_r {
                return None;
            }
            let (dx, dy) = if self.r == 0 {
                (0, 0)
            } else {
                Spiral::ring_offset(self.r, self.i)
            };
            // Advance.
            self.i += 1;
            let ring_len = if self.r == 0 { 1 } else { 8 * self.r };
            if self.i >= ring_len {
                self.r += 1;
                self.i = 0;
            }
            let x = self.cx + dx;
            let y = self.cy + dy;
            if x >= 0 && y >= 0 && x < self.width && y < self.height {
                return Some(Tile::new(
                    u16::try_from(x).expect("checked"),
                    u16::try_from(y).expect("checked"),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const ROWS: [&str; 8] = [
        "........", ".f....f.", "........", "..m..m..", "........", "...~~...", "........",
        "........",
    ];

    fn small(rows: &[&str]) -> Map {
        let rows: Vec<String> = rows.iter().map(|r| format!("    {r:?},")).collect();
        let text = format!(
            "(\n  name: \"t\",\n  width: 8,\n  height: 8,\n  symmetry: MirrorX,\n  starts: [(x: 1, y: 4), (x: 6, y: 4)],\n  rows: [\n{}\n  ],\n)",
            rows.join("\n")
        );
        Map::from_def(&MapDef::parse(Path::new("t.ron"), &text).unwrap())
    }

    #[test]
    fn from_def_costs_and_indexing() {
        let m = small(&ROWS);
        assert_eq!(m.len(), 64);
        assert_eq!(m.cost_of(Tile::new(0, 0)), COST_PASSABLE);
        assert_eq!(m.cost_of(Tile::new(1, 1)), COST_BLOCKED);
        assert_eq!(m.cost_of(Tile::new(3, 5)), COST_BLOCKED);
        assert!(m.passable(Tile::new(7, 7)));
        assert!(
            !m.passable(Tile::new(8, 0)),
            "out of bounds is not passable"
        );
        assert_eq!(m.idx(Tile::new(3, 2)), 19);
        assert_eq!(m.tile_at(19), Tile::new(3, 2));
        assert_eq!(m.cost_grid_generation(), 0);
    }

    #[test]
    fn tile_of_and_center_of() {
        let m = small(&ROWS);
        let c = m.center_of(Tile::new(3, 4));
        assert_eq!(c, FxVec2::new(Fx::from_ratio(7, 2), Fx::from_ratio(9, 2)));
        assert_eq!(m.tile_of(c), Tile::new(3, 4));
        assert_eq!(m.tile_of(FxVec2::from_ints(3, 4)), Tile::new(3, 4));
        // Positions on or past the far edge clamp to the last tile; negatives to 0.
        assert_eq!(m.tile_of(FxVec2::from_ints(8, 8)), Tile::new(7, 7));
        assert_eq!(m.tile_of(FxVec2::from_ints(-3, 100)), Tile::new(0, 7));
    }

    #[test]
    fn neighbors8_fixed_order_and_no_corner_cutting() {
        let m = Map::open(8, 8);
        let n: Vec<Tile> = m.neighbors8(Tile::new(3, 3)).collect();
        assert_eq!(
            n,
            vec![
                Tile::new(3, 2),
                Tile::new(4, 2),
                Tile::new(4, 3),
                Tile::new(4, 4),
                Tile::new(3, 4),
                Tile::new(2, 4),
                Tile::new(2, 3),
                Tile::new(2, 2),
            ]
        );
        // Corner: only 3 neighbours.
        let n: Vec<Tile> = m.neighbors8(Tile::new(0, 0)).collect();
        assert_eq!(n, vec![Tile::new(1, 0), Tile::new(1, 1), Tile::new(0, 1)]);

        // Block the tile east of (3,3): NE and SE diagonals are then cut off.
        let mut m = Map::open(8, 8);
        assert!(m.set_blocked(Tile::new(4, 3), true));
        let n: Vec<Tile> = m.neighbors8(Tile::new(3, 3)).collect();
        assert_eq!(
            n,
            vec![
                Tile::new(3, 2),
                Tile::new(3, 4),
                Tile::new(2, 4),
                Tile::new(2, 3),
                Tile::new(2, 2),
            ]
        );
        assert!(!m.step_allowed(Tile::new(3, 3), Tile::new(4, 2)));
        assert!(m.step_allowed(Tile::new(3, 3), Tile::new(2, 2)));
        assert!(
            !m.step_allowed(Tile::new(3, 3), Tile::new(5, 3)),
            "not adjacent"
        );
    }

    #[test]
    fn set_blocked_many_bumps_once_and_matches_per_tile_writes() {
        let wall: Vec<Tile> = (0..4).map(|y| Tile::new(2, y)).collect();
        let mut batched = Map::open(4, 4);
        assert!(batched.set_blocked_many(&wall, true));
        assert_eq!(
            batched.cost_grid_generation(),
            1,
            "one bump for the whole footprint"
        );
        assert_eq!(
            batched.component_count(),
            2,
            "components rebuilt: the wall splits the map"
        );
        let mut single = Map::open(4, 4);
        for &t in &wall {
            single.set_blocked(t, true);
        }
        assert_eq!(batched.cost, single.cost);
        assert_eq!(batched.components, single.components);
        // A write that changes nothing keeps the generation.
        assert!(!batched.set_blocked_many(&wall, true));
        assert!(!batched.set_blocked_many(&[], false));
        assert_eq!(batched.cost_grid_generation(), 1);
        // A partly redundant write still bumps once.
        assert!(batched.set_blocked_many(&[Tile::new(2, 0), Tile::new(0, 0)], false));
        assert_eq!(batched.cost_grid_generation(), 2);
        assert!(batched.passable(Tile::new(2, 0)) && batched.passable(Tile::new(0, 0)));
    }

    #[test]
    fn set_blocked_bumps_generation_only_on_change_and_rebuilds_components() {
        let mut m = Map::open(4, 4);
        assert_eq!(m.component_count(), 1);
        assert!(!m.set_blocked(Tile::new(1, 1), false), "no-op write");
        assert_eq!(m.cost_grid_generation(), 0);
        // A wall down column 2 splits the map in two.
        for y in 0..4 {
            assert!(m.set_blocked(Tile::new(2, y), true));
        }
        assert_eq!(m.cost_grid_generation(), 4);
        assert_eq!(m.component_count(), 2);
        assert_eq!(m.component_of(Tile::new(0, 0)), 0);
        assert_eq!(m.component_of(Tile::new(3, 3)), 1);
        assert_eq!(m.component_of(Tile::new(2, 1)), NO_COMPONENT);
        assert!(m.set_blocked(Tile::new(2, 1), false));
        assert_eq!(m.component_count(), 1);
        assert_eq!(m.cost_grid_generation(), 5);
    }

    #[test]
    fn diagonal_only_contact_is_not_connected() {
        // (0,0) and (1,1) passable, (1,0) and (0,1) blocked: no corner cut, so
        // two components.
        let mut m = Map::open(2, 2);
        m.set_blocked(Tile::new(1, 0), true);
        m.set_blocked(Tile::new(0, 1), true);
        assert_eq!(m.component_count(), 2);
        assert_ne!(
            m.component_of(Tile::new(0, 0)),
            m.component_of(Tile::new(1, 1))
        );
    }

    #[test]
    fn nearest_passable_bfs_order_and_component_filter() {
        let m = small(&ROWS);
        // Passable start returns itself.
        assert_eq!(
            m.nearest_passable(Tile::new(0, 0), None),
            Some(Tile::new(0, 0))
        );
        // Blocked forest at (1,1): first BFS neighbour (east) is passable.
        assert_eq!(
            m.nearest_passable(Tile::new(1, 1), None),
            Some(Tile::new(2, 1))
        );
        // Water at (3,5),(4,5): from (3,5) east is (4,5) water, then SE (4,6).
        assert_eq!(
            m.nearest_passable(Tile::new(3, 5), None),
            Some(Tile::new(4, 6))
        );
        // Component filter on a split map.
        let mut w = Map::open(4, 4);
        for y in 0..4 {
            w.set_blocked(Tile::new(2, y), true);
        }
        let right = w.component_of(Tile::new(3, 0));
        assert_eq!(
            w.nearest_passable(Tile::new(0, 0), Some(right)),
            Some(Tile::new(3, 0))
        );
        assert_eq!(w.nearest_passable(Tile::new(0, 0), Some(77)), None);
        assert_eq!(w.nearest_passable(Tile::new(9, 9), None), None);
        // Fully blocked map: nothing to find.
        let mut b = Map::open(2, 2);
        for i in 0..4 {
            b.set_blocked(b.tile_at(i), true);
        }
        assert_eq!(b.nearest_passable(Tile::new(0, 0), None), None);
    }

    #[test]
    fn spiral_order_and_coverage() {
        let m = Map::open(8, 8);
        let s: Vec<Tile> = m.spiral(Tile::new(3, 3)).take(9).collect();
        assert_eq!(
            s,
            vec![
                Tile::new(3, 3),
                Tile::new(4, 3),
                Tile::new(4, 4),
                Tile::new(3, 4),
                Tile::new(2, 4),
                Tile::new(2, 3),
                Tile::new(2, 2),
                Tile::new(3, 2),
                Tile::new(4, 2),
            ]
        );
        // Ring 2 starts at the east tile.
        let s: Vec<Tile> = m.spiral(Tile::new(3, 3)).skip(9).take(3).collect();
        assert_eq!(s, vec![Tile::new(5, 3), Tile::new(5, 4), Tile::new(5, 5)]);
        // Every tile exactly once, from the centre and from a corner.
        for c in [
            Tile::new(3, 3),
            Tile::new(0, 0),
            Tile::new(7, 7),
            Tile::new(7, 0),
        ] {
            let mut all: Vec<Tile> = m.spiral(c).collect();
            assert_eq!(all.len(), 64, "from {c:?}");
            all.sort();
            all.dedup();
            assert_eq!(all.len(), 64, "duplicates from {c:?}");
        }
        // Ring distances are non-decreasing.
        let c = Tile::new(2, 5);
        let mut last = 0;
        for t in m.spiral(c) {
            let d = (i32::from(t.x) - 2).abs().max((i32::from(t.y) - 5).abs());
            assert!(d >= last);
            last = d;
        }
    }

    #[test]
    fn serde_skips_components_and_rebuild_restores_them() {
        let mut m = small(&ROWS);
        m.set_blocked(Tile::new(0, 7), true);
        let bytes = postcard::to_allocvec(&m).unwrap();
        let mut back: Map = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, m, "hashed part round-trips");
        assert!(
            back.components().is_empty(),
            "derived part is not serialised"
        );
        back.rebuild_components();
        assert_eq!(back.components(), m.components());
        assert_eq!(back.component_count(), m.component_count());
        assert_eq!(postcard::to_allocvec(&back).unwrap(), bytes);
    }

    #[test]
    fn plains_map_loads_with_two_sides_connected() {
        let rules =
            rules::Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap();
        let m = Map::from_def(rules.map("plains_1v1").unwrap());
        assert_eq!((m.width(), m.height()), (128, 128));
        let a = Tile::new(24, 64);
        let b = Tile::new(103, 64);
        assert!(m.passable(a) && m.passable(b));
        assert_eq!(m.component_of(a), m.component_of(b), "starts are connected");
    }
}
