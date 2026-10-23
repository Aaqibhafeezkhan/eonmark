//! Movement: arrival steering, spatial-grid separation, circle correction and
//! group-order target assignment. Specification: `docs/design/pathing.md`,
//! sections "Movement step", "Group orders: spiral offsets" and "BFS
//! nearest-passable fallback".
//!
//! Everything here is a pure function of the hashed state passed in. The
//! [`SpatialGrid`] is derived: rebuilt every tick by `Sim::step` and by
//! `Sim::restore`, never hashed.
//!
//! Per tick, for every unit in `UnitId` order:
//!
//! 1. Arrival steering (units with a path) toward `order.path[order.next]`
//!    at `unit.speed` (tiles per tick), scaled down linearly inside the
//!    kind's `arrive_slowdown_radius` around the final waypoint. A waypoint
//!    is consumed when the unit is within the kind's `waypoint_radius` of
//!    it, or once the unit is closer to the following waypoint than the
//!    waypoint itself is; the final one when within the arrive radius,
//!    which clears the order, records the final waypoint's centre as the
//!    unit's [`Post`] and emits [`SimEvent::UnitArrived`].
//!    Return to post: an arrived unit (no order, a post) that has been
//!    pushed farther than the kind's `return_to_post_radius` from its post
//!    steers straight back to it with the same arrival steering, no path
//!    request, until it is within the arrive radius again (`Post::returning`
//!    carries the hysteresis across ticks). If terrain blocks the straight
//!    line the post becomes a `MoveOrder` and the unit requests a path like
//!    an ordinary move.
//! 2. Separation from neighbours read from the grid: for each neighbour
//!    closer than `radius_a + radius_b + separation`, accumulate a push-away
//!    vector of the penetration depth. Compare with `dist_sq_i64`, never a
//!    square root. The sum is capped at `speed / separation_push_moving_div`
//!    (an order, or returning to post) or `speed / separation_push_idle_div`
//!    (parked) so steering keeps the upper hand.
//! 3. Integrate: `pos += clamp(desired + separation, speed)`, sliding along
//!    a blocked tile's wall (x-only, then y-only) instead of entering it.
//! 4. After every unit has moved, circle-vs-circle correction over
//!    overlapping pairs in `(UnitId, UnitId)` order, half the penetration
//!    each; a correction that would enter a blocked tile is dropped.
//!
//! A unit that `combat::think` engaged this tick (step 5a) replaces step 1
//! with its [`Engage`]: hold (no desired velocity, idle push cap, facing
//! the target) or chase (straight at the target's point, moving push cap);
//! steps 2 to 4 apply as to any unit. A path chaser that `think` lists as
//! awaiting (a path request outstanding) follows its path as usual but
//! does not arrive at the final waypoint: it keeps steering at it and keeps
//! its order until the new path lands.
//!
//! Units never enter a blocked tile. Repath triggers (each re-issues a
//! `PathRequest` through `Pathing::request`, unless one is still
//! outstanding): the next waypoint's tile is blocked, `tick >=
//! order.repath_at` (every `repath_interval_ds` while moving), or terrain
//! blocked every candidate step this tick. See [`step`] for the decisions
//! taken where the design is silent and the crowd measurements behind them.
//!
//! Every number here comes from `data/rules/`: the per-kind radii from
//! `units.ron` and the push caps and repath cadence from `rules.ron`.

use crate::combat::Engage;
use crate::fx::{Fx, FxVec2};
use crate::ids::UnitId;
use crate::map::Map;
use crate::state::{MoveOrder, Post, SimEvent, Unit};
use rules::Rules;
use std::collections::{BTreeMap, BTreeSet};

/// Side of one spatial-grid cell in tiles.
pub const CELL_TILES: u16 = 2;

/// Uniform grid of unit ids by position, 2x2 tiles per cell. Derived state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpatialGrid {
    cells_x: u16,
    cells_y: u16,
    /// `cells_x * cells_y` buckets, each in ascending `UnitId` order because
    /// `rebuild` walks the units map in id order.
    cells: Vec<Vec<UnitId>>,
}

impl SpatialGrid {
    /// An empty grid sized for `map`.
    pub fn new(map: &Map) -> SpatialGrid {
        let cells_x = map.width().div_ceil(CELL_TILES);
        let cells_y = map.height().div_ceil(CELL_TILES);
        SpatialGrid {
            cells_x,
            cells_y,
            cells: vec![Vec::new(); usize::from(cells_x) * usize::from(cells_y)],
        }
    }

    /// Grid dimensions in cells.
    pub fn dims(&self) -> (u16, u16) {
        (self.cells_x, self.cells_y)
    }

    /// Cell coordinates of a position, clamped into the grid.
    fn cell_of(&self, p: FxVec2) -> (u16, u16) {
        let clamp = |v: i32, max: u16| -> u16 {
            let hi = i32::from(max) - 1;
            u16::try_from(v.clamp(0, hi)).expect("clamped")
        };
        let cx = p.x.floor_to_int().div_euclid(i32::from(CELL_TILES));
        let cy = p.y.floor_to_int().div_euclid(i32::from(CELL_TILES));
        (clamp(cx, self.cells_x), clamp(cy, self.cells_y))
    }

    fn cell_index(&self, cx: u16, cy: u16) -> usize {
        usize::from(cy) * usize::from(self.cells_x) + usize::from(cx)
    }

    /// Clear and refill from the units map. Buckets end up in id order.
    pub fn rebuild(&mut self, units: &BTreeMap<UnitId, Unit>) {
        for c in &mut self.cells {
            c.clear();
        }
        for u in units.values() {
            let (cx, cy) = self.cell_of(u.pos);
            let i = self.cell_index(cx, cy);
            self.cells[i].push(u.id);
        }
    }

    /// Ids of every unit whose cell intersects the square
    /// `[pos - radius, pos + radius]`, in ascending `UnitId` order. The
    /// caller filters by exact distance; `pos`'s own unit is included.
    pub fn neighbors(&self, pos: FxVec2, radius: Fx) -> impl Iterator<Item = UnitId> + '_ {
        let (x0, y0) = self.cell_of(FxVec2::new(pos.x - radius, pos.y - radius));
        let (x1, y1) = self.cell_of(FxVec2::new(pos.x + radius, pos.y + radius));
        let mut out: Vec<UnitId> = Vec::new();
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                out.extend_from_slice(&self.cells[self.cell_index(cx, cy)]);
            }
        }
        out.sort_unstable();
        out.into_iter()
    }

    /// Call `f` with the id of every unit whose cell intersects the square
    /// `[pos - radius, pos + radius]`, without allocating: cell by cell,
    /// row-major, each cell in ascending `UnitId` order (not ascending
    /// overall, unlike [`SpatialGrid::neighbors`]). For callers that reduce
    /// the candidates by a total key, so the visiting order cannot matter.
    /// `pos`'s own unit is included.
    pub fn for_each_in(&self, pos: FxVec2, radius: Fx, mut f: impl FnMut(UnitId)) {
        let (x0, y0) = self.cell_of(FxVec2::new(pos.x - radius, pos.y - radius));
        let (x1, y1) = self.cell_of(FxVec2::new(pos.x + radius, pos.y + radius));
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                for &id in &self.cells[self.cell_index(cx, cy)] {
                    f(id);
                }
            }
        }
    }

    /// Total units indexed.
    pub fn len(&self) -> usize {
        self.cells.iter().map(Vec::len).sum()
    }

    /// `true` when no units are indexed.
    pub fn is_empty(&self) -> bool {
        self.cells.iter().all(Vec::is_empty)
    }
}

/// Per-kind movement parameters, indexed by `UnitKindId`
/// (`data/rules/units.ron`, converted once per tick).
struct KindParams {
    /// Final-arrival radius.
    arrive: Fx,
    /// The linear slowdown toward the final waypoint starts inside this.
    slowdown: Fx,
    /// Squared radius, in `dist_sq_i64` scale, inside which a non-final
    /// waypoint is consumed.
    waypoint_sq: i64,
    /// Separation margin added to the sum of radii.
    separation: Fx,
    /// An arrived unit pushed farther than this from its post walks back.
    return_radius: Fx,
}

fn kind_params(rules: &Rules) -> Vec<KindParams> {
    rules
        .units
        .iter()
        .map(|k| {
            let waypoint = crate::state::fx_from_x100(k.waypoint_radius_tiles_x100);
            KindParams {
                arrive: crate::state::fx_from_x100(k.arrive_radius_tiles_x100),
                slowdown: crate::state::fx_from_x100(k.arrive_slowdown_radius_tiles_x100),
                waypoint_sq: (waypoint * waypoint).to_bits(),
                separation: crate::state::fx_from_x100(k.separation_tiles_x100),
                return_radius: crate::state::fx_from_x100(k.return_to_post_radius_tiles_x100),
            }
        })
        .collect()
}

/// Velocity of magnitude `speed` from `pos` toward `target`, scaled down
/// linearly inside `slowdown` of it and never longer than the remaining
/// distance, so the unit stops on the target instead of overshooting.
/// Returns the velocity and the distance. Zero velocity at the target.
fn arrival_steer(pos: FxVec2, target: FxVec2, speed: Fx, slowdown: Fx) -> (FxVec2, Fx) {
    let delta = sub(pos, target);
    let dist = length(delta);
    if dist == Fx::ZERO {
        return (FxVec2::ZERO, dist);
    }
    let mut mag = speed;
    if dist < slowdown {
        mag = speed * dist / slowdown;
    }
    mag = mag.min(dist);
    (rescale(delta, mag, dist), dist)
}

/// `speed / div` for a `rules.ron` divisor (validated `> 0`).
fn push_cap(speed: Fx, div: u32) -> Fx {
    speed.div_int(i32::try_from(div).expect("push divisor validated > 0 and fits i32"))
}

/// Length of a vector: the integer square root of its squared length,
/// exact to one `I32F32` bit (`isqrt(len_sq << 32)` where `len_sq` is the
/// `I32F32`-scaled square, so the result is `len << 32`). Deterministic;
/// no floats.
pub(crate) fn length(v: FxVec2) -> Fx {
    let sq = u128::try_from(v.length_sq_i64()).expect("squared length is non-negative");
    let bits = (sq << 32).isqrt();
    Fx::from_bits(i64::try_from(bits).expect("length fits I32F32"))
}

/// `b - a`.
fn sub(a: FxVec2, b: FxVec2) -> FxVec2 {
    FxVec2::new(b.x - a.x, b.y - a.y)
}

/// `v * k / d` componentwise (multiply first for precision). `d > 0`.
fn rescale(v: FxVec2, k: Fx, d: Fx) -> FxVec2 {
    FxVec2::new(v.x * k / d, v.y * k / d)
}

/// `true` when `p` lies inside the map on a passable tile.
fn walkable(map: &Map, p: FxVec2) -> bool {
    p.x >= Fx::ZERO
        && p.y >= Fx::ZERO
        && p.x < Fx::from_int(i32::from(map.width()))
        && p.y < Fx::from_int(i32::from(map.height()))
        && map.passable(map.tile_of(p))
}

/// Move from `from` by `v`, never entering a blocked tile: the full step,
/// else the x-only step, else the y-only step (sliding along walls), else
/// stay. Returns the new position and whether the unit moved at all.
fn slide(map: &Map, from: FxVec2, v: FxVec2) -> (FxVec2, bool) {
    if v == FxVec2::ZERO {
        return (from, false);
    }
    let full = from + v;
    let candidates = [
        full,
        FxVec2::new(full.x, from.y),
        FxVec2::new(from.x, full.y),
    ];
    for c in candidates {
        if c != from && walkable(map, c) {
            return (c, true);
        }
    }
    (from, false)
}

/// Advance every unit with an order by one tick (see the module docs).
/// `grid` was rebuilt from `units` at the start of this tick. A unit in
/// `engage` (step 5a, `combat::think`) is steered by its engagement instead
/// of its order or post: [`Engage::Hold`] has no desired velocity, the idle
/// push cap and faces its target; [`Engage::Chase`] steers straight at its
/// point at full speed with the moving push cap. Neither follows a path,
/// consumes waypoints, arrives, returns to a post or asks for a repath. A
/// unit in `awaiting` (a path chaser whose path request is outstanding,
/// step 5a) does not arrive at its final waypoint: it keeps its order and
/// steers at the waypoint until step 4 writes the new path. Every other
/// unit moves exactly as without combat. Emits
/// [`SimEvent::UnitArrived`] when an order completes. Repath requests are
/// not issued here; the function returns the ids of units that need one
/// (blocked next waypoint, `tick >= repath_at`, or a unit whose every
/// candidate step was blocked by terrain), in id order, and `Sim::step`
/// pushes the `PathRequest`s.
///
/// Decisions where the design is silent (all deterministic, no state added):
///
/// - The final waypoint is `path.last()` (the component-corrected goal
///   tile), not `order.goal`, so a unit in another component than the click
///   still arrives. Arrival is `dist <= arrive_radius`; the linear slowdown
///   applies inside `arrive_slowdown_radius` (half a tile for the Yeoman),
///   and a step is never longer than the remaining distance, so a unit
///   cannot overshoot its waypoint.
/// - A non-final waypoint is consumed when the unit is within the kind's
///   `waypoint_radius` (half a tile) of it *or* is already closer to the
///   following waypoint than the waypoint itself is (the unit was pushed
///   past it; steering back would be a detour that stalls a crowd). This
///   second rule is what lets 500 units flow through the 8-tile water gap
///   at about one unit per tick.
/// - Separation pushes a unit away from each neighbour closer than
///   `radius_a + radius_b + separation` by the penetration depth along the
///   centre line; the sum is capped at `speed / separation_push_moving_div`
///   (2) for moving units and `speed / separation_push_idle_div` (8) for
///   units without an order, which also take part so a parked block yields
///   to a crowd instead of jamming it (an infinite divisor) or being flung
///   tiles away from where it arrived (a divisor of 2); measured on the
///   500-unit crossing, 8 keeps 95% of arrived units within 3 tiles of
///   their goal. Two coincident units push along the x axis, the higher id
///   eastward. Units later in id order see earlier units' updated positions.
/// - Return to post (`SIM_VERSION` 3) closes the remaining gap: a parked
///   unit pushed more than `return_to_post_radius_tiles_x100` (one tile)
///   from the centre it arrived at walks back with the normal arrival
///   steering and the moving push cap, and is parked again (idle cap) once
///   within the arrive radius. Posts of a group order are distinct spiral
///   tiles, so a block that has converged holds still: the 100-unit
///   convergence test asserts a constant hash over its last 100 ticks.
///   Two units that share one post (a spiral cut by the map edge pads with
///   the last target) jostle around it; that is the documented padding
///   case, not the normal one.
/// - A step that would enter a blocked tile (or leave the map) slides along
///   the wall (x-only, then y-only) and otherwise stays put.
/// - `facing` is the normalised velocity of the last tick the unit moved
///   (`v / |v|`, exact to one `I32F32` bit); it is untouched while standing
///   or while being pushed without an order.
/// - Circle correction is one pass over every unit (orders or not) in
///   `(a, b)` id order with `a < b`; coincident pairs separate along the x
///   axis. More passes were measured to slow the crowd, not help it.
#[allow(clippy::too_many_arguments)]
pub fn step(
    units: &mut BTreeMap<UnitId, Unit>,
    map: &Map,
    grid: &SpatialGrid,
    rules: &Rules,
    engage: &BTreeMap<UnitId, Engage>,
    awaiting: &BTreeSet<UnitId>,
    tick: u32,
    events: &mut Vec<SimEvent>,
) -> Vec<UnitId> {
    let params = kind_params(rules);
    let moving_div = rules.separation_push_moving_div;
    let idle_div = rules.separation_push_idle_div;
    let max_radius = units.values().map(|u| u.radius).max().unwrap_or(Fx::ZERO);
    let max_speed = units.values().map(|u| u.speed).max().unwrap_or(Fx::ZERO);
    let max_sep = params
        .iter()
        .map(|p| p.separation)
        .max()
        .unwrap_or(Fx::ZERO);
    // Grid cells hold start-of-tick positions; widen the query by the
    // largest move a unit can make this tick so no neighbour is missed.
    let query_radius = max_radius + max_radius + max_sep + max_speed;
    let ids: Vec<UnitId> = units.keys().copied().collect();
    let mut repath = Vec::new();

    for id in &ids {
        let u = &units[id];
        let p = &params[usize::from(u.kind.0)];

        // 1. Arrival steering (units with a path), or the walk back to the
        //    post (arrived units pushed away from it). Units without either,
        //    or waiting for their path, have no desired velocity but still
        //    take part in separation below, so a moving crowd can shove a
        //    parked unit aside instead of jamming against it.
        let mut desired = FxVec2::ZERO;
        let mut next: Option<usize> = None;
        let mut needs_repath = false;
        // `Some(active)` for a unit holding a post: whether it walks back
        // this tick, which becomes its new `Post::returning`.
        let mut returning: Option<bool> = None;
        let engaged = engage.get(id).copied();
        if let Some(e) = engaged {
            if let Engage::Chase(point) = e {
                desired = arrival_steer(u.pos, point, u.speed, Fx::ZERO).0;
            }
        } else if let Some(order) = &u.order {
            if !order.path.is_empty() {
                let last = order.path.len() - 1;
                let mut i = usize::try_from(order.next).expect("fits").min(last);
                while i < last {
                    let here = map.center_of(order.path[i]);
                    let ahead = map.center_of(order.path[i + 1]);
                    // Consumed when within the waypoint radius, or once the
                    // unit is already closer to the following waypoint than
                    // this one is (it has been passed; steering back would
                    // be a detour).
                    if u.pos.dist_sq_i64(here) < p.waypoint_sq
                        || u.pos.dist_sq_i64(ahead) < here.dist_sq_i64(ahead)
                    {
                        i += 1;
                    } else {
                        break;
                    }
                }
                let target = map.center_of(order.path[i]);
                let slowdown = if i == last { p.slowdown } else { Fx::ZERO };
                let (v, dist) = arrival_steer(u.pos, target, u.speed, slowdown);
                if i == last && dist <= p.arrive && !awaiting.contains(id) {
                    let u = units.get_mut(id).expect("exists");
                    u.order = None;
                    u.post = Some(Post {
                        goal: target,
                        returning: false,
                    });
                    events.push(SimEvent::UnitArrived { unit: *id });
                    continue;
                }
                desired = v;
                needs_repath = !map.passable(order.path[i]) || tick >= order.repath_at;
                next = Some(i);
            }
        } else if let Some(post) = u.post {
            // Return to post: start walking back once pushed beyond the
            // kind's return radius, keep walking until back within the
            // arrive radius (the gap between the two is the hysteresis that
            // stops a unit on the boundary from dithering).
            let delta = sub(u.pos, post.goal);
            let dist = length(delta);
            let active = if post.returning {
                dist > p.arrive
            } else {
                dist > p.return_radius
            };
            if active {
                desired = arrival_steer(u.pos, post.goal, u.speed, p.slowdown).0;
            }
            returning = Some(active);
        }

        // 2. Separation.
        let mut push = FxVec2::ZERO;
        for nb in grid.neighbors(u.pos, query_radius) {
            if nb == *id {
                continue;
            }
            let Some(o) = units.get(&nb) else {
                continue;
            };
            let thresh = u.radius + o.radius + p.separation;
            if u.pos.dist_sq_i64(o.pos) >= (thresh * thresh).to_bits() {
                continue;
            }
            let away = sub(o.pos, u.pos);
            let d = length(away);
            if d == Fx::ZERO {
                let dir = if *id > nb { thresh } else { -thresh };
                push.x += dir;
            } else {
                push += rescale(away, thresh - d, d);
            }
        }

        // 3. Integrate with the speed clamp. The push is capped first (the
        //    `separation_push_*_div` rules) so the desired direction always
        //    keeps the upper hand: in a dense crowd the sum of pushes would
        //    otherwise drown the steering and stall the flow. A unit walking
        //    back to its post counts as moving only while it is outside the
        //    arrive radius; once back it yields like any parked unit, so two
        //    neighbours cannot shove each other back and forth forever.
        let plen = length(push);
        let moving = match engaged {
            Some(Engage::Hold { .. }) => false,
            Some(Engage::Chase(_)) => true,
            None => u.order.is_some() || returning == Some(true),
        };
        let cap = if moving {
            push_cap(u.speed, moving_div)
        } else {
            push_cap(u.speed, idle_div)
        };
        if plen > cap {
            push = rescale(push, cap, plen);
        }
        let mut v = desired + push;
        let vlen = length(v);
        if vlen > u.speed {
            v = rescale(v, u.speed, vlen);
        }
        let (new_pos, moved) = slide(map, u.pos, v);
        let blocked = v != FxVec2::ZERO && !moved;
        let facing = if vlen > Fx::ZERO {
            Some(FxVec2::new(v.x / vlen, v.y / vlen))
        } else {
            None
        };

        let u = units.get_mut(id).expect("exists");
        u.pos = new_pos;
        if let Some(e) = engaged {
            match e {
                Engage::Hold { face } => {
                    let toward = sub(new_pos, face);
                    let len = length(toward);
                    if len > Fx::ZERO {
                        u.facing = FxVec2::new(toward.x / len, toward.y / len);
                    }
                }
                Engage::Chase(_) => {
                    if let Some(f) = facing {
                        u.facing = f;
                    }
                }
            }
            continue;
        }
        match (next, returning) {
            (Some(next), _) => {
                if let Some(f) = facing {
                    u.facing = f;
                }
                if let Some(order) = &mut u.order {
                    order.next = u32::try_from(next).expect("fits");
                }
                if needs_repath || blocked {
                    repath.push(*id);
                }
            }
            (None, Some(active)) => {
                if let Some(post) = &mut u.post {
                    post.returning = active;
                }
                if active {
                    if let Some(f) = facing {
                        u.facing = f;
                    }
                    if blocked {
                        // Terrain lies between the unit and its post: path
                        // there like an ordinary move (`Sim::step` turns
                        // the returned id into the `PathRequest`); arriving
                        // makes it a post again.
                        let goal = u.post.take().expect("returning units hold a post").goal;
                        u.order = Some(MoveOrder {
                            goal,
                            path: Vec::new(),
                            next: 0,
                            repath_at: tick + rules.repath_interval_ticks(),
                        });
                        repath.push(*id);
                    }
                }
            }
            // No path and no post (or no order yet): only the push applied
            // and the facing is kept.
            (None, None) => {}
        }
    }

    // 4. Circle-vs-circle correction in (a, b) id order.
    let pair_radius = max_radius + max_radius + max_speed;
    for a_id in &ids {
        let (mut a_pos, a_r) = {
            let a = &units[a_id];
            (a.pos, a.radius)
        };
        for b_id in grid.neighbors(a_pos, pair_radius) {
            if b_id <= *a_id {
                continue;
            }
            let Some(b) = units.get(&b_id) else {
                continue;
            };
            let sum = a_r + b.radius;
            if a_pos.dist_sq_i64(b.pos) >= (sum * sum).to_bits() {
                continue;
            }
            let away = sub(b.pos, a_pos);
            let d = length(away);
            let shift = if d == Fx::ZERO {
                // Coincident: the lower id (a) moves west, the higher east.
                FxVec2::new(-sum.div_int(2), Fx::ZERO)
            } else {
                rescale(away, (sum - d).div_int(2), d)
            };
            let a_new = a_pos + shift;
            let b_new = FxVec2::new(b.pos.x - shift.x, b.pos.y - shift.y);
            if walkable(map, a_new) {
                a_pos = a_new;
            }
            if walkable(map, b_new) {
                units.get_mut(&b_id).expect("exists").pos = b_new;
            }
        }
        units.get_mut(a_id).expect("exists").pos = a_pos;
    }

    repath
}

/// Targets for `n` units ordered to `click`, one per unit, in the order the
/// caller assigns them (callers sort the units by `UnitId`). The click tile
/// is corrected first with `Map::nearest_passable(click_tile, None)`; then a
/// square spiral from the corrected tile yields tile centres, skipping
/// blocked tiles and tiles outside the corrected tile's component. If the
/// spiral is exhausted the remaining entries repeat the last assigned
/// target; if the map has no passable tile at all the result repeats
/// `click`. Length is always `n`.
pub fn group_targets(map: &Map, click: FxVec2, n: usize) -> Vec<FxVec2> {
    let Some(start) = map.nearest_passable(map.tile_of(click), None) else {
        return vec![click; n];
    };
    let comp = map.component_of(start);
    let mut out: Vec<FxVec2> = map
        .spiral(start)
        .filter(|t| map.passable(*t) && map.component_of(*t) == comp)
        .take(n)
        .map(|t| map.center_of(t))
        .collect();
    let Some(&last) = out.last() else {
        return vec![click; n];
    };
    out.resize(n, last);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{PlayerId, UnitKindId};

    fn unit(id: u32, x: i32, y: i32) -> Unit {
        Unit {
            id: UnitId(id),
            owner: PlayerId(0),
            kind: UnitKindId(0),
            pos: FxVec2::from_ints(x, y),
            facing: FxVec2::from_ints(1, 0),
            radius: Fx::from_ratio(35, 100),
            speed: Fx::from_ratio(9, 100),
            order: None,
            post: None,
            job: None,
            seek_at: None,
            damage_taken: 0,
            ready_at: 0,
            target: None,
            intent: crate::state::Intent::Plain,
            exposed_ticks: 0,
        }
    }

    #[test]
    fn grid_buckets_by_2x2_cells_and_returns_id_order() {
        let map = Map::open(8, 8);
        let mut units = BTreeMap::new();
        for (id, x, y) in [(5, 0, 0), (2, 1, 1), (9, 3, 3), (1, 7, 7), (4, 2, 0)] {
            units.insert(UnitId(id), unit(id, x, y));
        }
        let mut grid = SpatialGrid::new(&map);
        assert_eq!(grid.dims(), (4, 4));
        grid.rebuild(&units);
        assert_eq!(grid.len(), 5);
        // Cell (0,0) holds ids 2 and 5; radius 0 at (0,0) sees only that cell.
        let n: Vec<UnitId> = grid.neighbors(FxVec2::from_ints(0, 0), Fx::ZERO).collect();
        assert_eq!(n, vec![UnitId(2), UnitId(5)]);
        // Radius 1 around (1.5, 1.5) spans cells (0..1, 0..1): ids 2, 4, 5, 9.
        let n: Vec<UnitId> = grid
            .neighbors(
                FxVec2::new(Fx::from_ratio(3, 2), Fx::from_ratio(3, 2)),
                Fx::ONE,
            )
            .collect();
        assert_eq!(n, vec![UnitId(2), UnitId(4), UnitId(5), UnitId(9)]);
        // Out-of-map positions clamp into the grid.
        let n: Vec<UnitId> = grid
            .neighbors(FxVec2::from_ints(50, 50), Fx::ZERO)
            .collect();
        assert_eq!(n, vec![UnitId(1)]);
        // The visitor sees the same ids, cell by cell, without allocating.
        let mut seen = Vec::new();
        grid.for_each_in(
            FxVec2::new(Fx::from_ratio(3, 2), Fx::from_ratio(3, 2)),
            Fx::ONE,
            |id| seen.push(id),
        );
        assert_eq!(seen, vec![UnitId(2), UnitId(5), UnitId(4), UnitId(9)]);
        grid.rebuild(&BTreeMap::new());
        assert!(grid.is_empty());
    }

    fn rules() -> Rules {
        Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap()
    }

    /// Run `step` for `ticks` ticks on a fresh grid each tick, like `Sim::step`.
    fn run(
        units: &mut BTreeMap<UnitId, Unit>,
        map: &Map,
        rules: &Rules,
        ticks: u32,
    ) -> (Vec<SimEvent>, Vec<UnitId>) {
        let mut grid = SpatialGrid::new(map);
        let mut events = Vec::new();
        let mut repath = Vec::new();
        for t in 0..ticks {
            grid.rebuild(units);
            repath = step(
                units,
                map,
                &grid,
                rules,
                &BTreeMap::new(),
                &BTreeSet::new(),
                t,
                &mut events,
            );
        }
        (events, repath)
    }

    fn order(path: Vec<crate::ids::Tile>, map: &Map) -> crate::state::MoveOrder {
        crate::state::MoveOrder {
            goal: map.center_of(*path.last().unwrap()),
            path,
            next: 0,
            repath_at: u32::MAX,
        }
    }

    #[test]
    fn length_is_exact_integer_sqrt() {
        assert_eq!(length(FxVec2::from_ints(3, 4)), Fx::from_int(5));
        assert_eq!(length(FxVec2::from_ints(0, -7)), Fx::from_int(7));
        assert_eq!(length(FxVec2::ZERO), Fx::ZERO);
        // sqrt(2) to 32 fractional bits, truncated.
        assert_eq!(length(FxVec2::from_ints(1, 1)).to_bits(), 6_074_000_999);
    }

    #[test]
    fn unit_walks_its_path_and_arrives_once() {
        use crate::ids::Tile;
        let rules = rules();
        let map = Map::open(16, 16);
        let mut units = BTreeMap::new();
        let mut u = unit(1, 2, 2);
        u.pos = map.center_of(Tile::new(2, 2));
        let path: Vec<Tile> = (2..=8).map(|x| Tile::new(x, 2)).collect();
        u.order = Some(order(path, &map));
        units.insert(UnitId(1), u);
        // 6 tiles at 0.09 per tick is 67 ticks; the arrive radius saves a few.
        let (events, repath) = run(&mut units, &map, &rules, 100);
        let arrived = events
            .iter()
            .filter(|e| matches!(e, SimEvent::UnitArrived { unit } if *unit == UnitId(1)))
            .count();
        assert_eq!(arrived, 1);
        assert!(repath.is_empty());
        let u = &units[&UnitId(1)];
        assert!(u.order.is_none());
        let goal = map.center_of(Tile::new(8, 2));
        assert!(
            u.pos.dist_sq_i64(goal)
                <= (Fx::from_ratio(25, 100) * Fx::from_ratio(25, 100)).to_bits()
        );
        // Facing is unit-length-ish: `v / |v|` with `|v|` truncated to one
        // I32F32 bit, so the x component may exceed 1 by a few bits.
        assert_eq!(u.facing.y, Fx::ZERO, "facing east");
        assert!(u.facing.x >= Fx::ONE && u.facing.x < Fx::ONE + Fx::from_ratio(1, 1000));
        // It never left row 2 and moved at most `speed` per tick: check a
        // fresh unit's first step exactly.
        let mut one = BTreeMap::new();
        let mut v = unit(2, 0, 0);
        v.pos = map.center_of(Tile::new(0, 0));
        v.order = Some(order(vec![Tile::new(0, 0), Tile::new(5, 0)], &map));
        one.insert(UnitId(2), v);
        run(&mut one, &map, &rules, 1);
        let moved = one[&UnitId(2)].pos;
        assert_eq!(moved.y, Fx::HALF);
        assert_eq!(moved.x, Fx::HALF + Fx::from_ratio(9, 100));
    }

    #[test]
    fn waypoint_is_consumed_when_passed_sideways() {
        use crate::ids::Tile;
        let rules = rules();
        let map = Map::open(16, 16);
        let mut units = BTreeMap::new();
        let mut u = unit(1, 0, 0);
        // Heading for waypoint (3,3) but pushed to (4.8, 4.0): closer to
        // (4,3) than (3,3) is, and then closer to (9,3) than (4,3) is, so
        // both are consumed and the unit must not walk back west.
        u.pos = FxVec2::new(Fx::from_ratio(48, 10), Fx::from_int(4));
        let mut o = order(
            vec![
                Tile::new(2, 3),
                Tile::new(3, 3),
                Tile::new(4, 3),
                Tile::new(9, 3),
            ],
            &map,
        );
        o.next = 1;
        u.order = Some(o);
        units.insert(UnitId(1), u);
        run(&mut units, &map, &rules, 1);
        let u = &units[&UnitId(1)];
        assert_eq!(u.order.as_ref().unwrap().next, 3);
        assert!(u.pos.x > Fx::from_ratio(48, 10), "moving east, not back");
        // Directly south of (4,3) at exactly one tile the rule does not fire
        // (equal distances), so the unit steers to (3,3) as before.
        let mut units = BTreeMap::new();
        let mut v = unit(2, 0, 0);
        v.pos = FxVec2::new(Fx::from_ratio(45, 10), Fx::from_ratio(45, 10));
        let mut o = order(
            vec![Tile::new(3, 3), Tile::new(4, 3), Tile::new(9, 3)],
            &map,
        );
        o.next = 0;
        v.order = Some(o);
        units.insert(UnitId(2), v);
        run(&mut units, &map, &rules, 1);
        assert_eq!(units[&UnitId(2)].order.as_ref().unwrap().next, 0);
    }

    #[test]
    fn units_never_enter_blocked_tiles_and_report_a_blocked_step() {
        use crate::ids::Tile;
        let rules = rules();
        let mut map = Map::open(8, 8);
        for y in 0..8 {
            map.set_blocked(Tile::new(4, y), true);
        }
        let mut units = BTreeMap::new();
        let mut u = unit(1, 3, 3);
        // Start just west of the wall, steering straight into it.
        u.pos = FxVec2::new(Fx::from_ratio(395, 100), Fx::from_ratio(35, 10));
        u.order = Some(order(vec![Tile::new(3, 3), Tile::new(6, 3)], &map));
        units.insert(UnitId(1), u);
        let (_, repath) = run(&mut units, &map, &rules, 5);
        let u = &units[&UnitId(1)];
        assert!(map.passable(map.tile_of(u.pos)));
        assert!(u.pos.x < Fx::from_int(4));
        assert_eq!(
            repath,
            vec![UnitId(1)],
            "a fully blocked step asks for a repath"
        );
    }

    #[test]
    fn arrival_records_a_post() {
        use crate::ids::Tile;
        let rules = rules();
        let map = Map::open(16, 16);
        let mut units = BTreeMap::new();
        let mut u = unit(1, 2, 2);
        u.pos = map.center_of(Tile::new(2, 2));
        u.order = Some(order(vec![Tile::new(2, 2), Tile::new(4, 2)], &map));
        units.insert(UnitId(1), u);
        run(&mut units, &map, &rules, 60);
        let u = &units[&UnitId(1)];
        assert!(u.order.is_none());
        assert_eq!(
            u.post,
            Some(Post {
                goal: map.center_of(Tile::new(4, 2)),
                returning: false
            })
        );
    }

    /// A path chaser waiting for a new path (`awaiting`) walks to the end
    /// of its current path and holds there with its order instead of
    /// arriving; once it no longer waits, it arrives as usual.
    #[test]
    fn awaiting_units_hold_the_end_of_their_path_instead_of_arriving() {
        use crate::ids::Tile;
        let rules = rules();
        let map = Map::open(16, 16);
        let mut units = BTreeMap::new();
        let mut u = unit(1, 2, 2);
        u.pos = map.center_of(Tile::new(2, 2));
        u.order = Some(order(vec![Tile::new(2, 2), Tile::new(4, 2)], &map));
        units.insert(UnitId(1), u);
        let awaiting = BTreeSet::from([UnitId(1)]);
        let mut grid = SpatialGrid::new(&map);
        let mut events = Vec::new();
        for t in 0..60 {
            grid.rebuild(&units);
            let repath = step(
                &mut units,
                &map,
                &grid,
                &rules,
                &BTreeMap::new(),
                &awaiting,
                t,
                &mut events,
            );
            assert!(repath.is_empty());
        }
        assert!(events.is_empty(), "{events:?}");
        let u = &units[&UnitId(1)];
        assert!(u.order.is_some() && u.post.is_none(), "{u:?}");
        let end = map.center_of(Tile::new(4, 2));
        let arrive = Fx::from_ratio(25, 100);
        assert!(
            u.pos.dist_sq_i64(end) <= (arrive * arrive).to_bits(),
            "at the end"
        );
        let (events, _) = run(&mut units, &map, &rules, 1);
        assert_eq!(events, vec![SimEvent::UnitArrived { unit: UnitId(1) }]);
        assert!(units[&UnitId(1)].order.is_none());
    }

    #[test]
    fn displaced_unit_returns_to_post_and_parks_again() {
        use crate::ids::Tile;
        let rules = rules();
        let map = Map::open(16, 16);
        let goal = map.center_of(Tile::new(8, 8));
        let post = Some(Post {
            goal,
            returning: false,
        });
        // Pushed 0.9 tiles: inside the return radius (1 tile), stays put.
        let mut units = BTreeMap::new();
        let mut near = unit(1, 0, 0);
        near.pos = FxVec2::new(goal.x + Fx::from_ratio(9, 10), goal.y);
        near.post = post;
        units.insert(UnitId(1), near);
        let before = units[&UnitId(1)].pos;
        let (events, repath) = run(&mut units, &map, &rules, 5);
        assert!(events.is_empty() && repath.is_empty());
        assert_eq!(units[&UnitId(1)].pos, before);
        assert_eq!(units[&UnitId(1)].post, post);
        // Pushed 1.5 tiles east: walks back west at full speed, faces west,
        // is `returning` while outside the arrive radius and parked again
        // (returning cleared) once inside it, where it stays.
        let mut units = BTreeMap::new();
        let mut far = unit(2, 0, 0);
        far.pos = FxVec2::new(goal.x + Fx::from_ratio(3, 2), goal.y);
        far.post = post;
        units.insert(UnitId(2), far);
        run(&mut units, &map, &rules, 1);
        let u = &units[&UnitId(2)];
        assert_eq!(
            u.pos.x,
            goal.x + Fx::from_ratio(3, 2) - Fx::from_ratio(9, 100)
        );
        assert_eq!(u.pos.y, goal.y);
        assert!(u.post.unwrap().returning);
        assert!(
            u.facing.x < Fx::ZERO && u.facing.y == Fx::ZERO,
            "faces west"
        );
        // 1.25 tiles to the arrive radius at 0.09 per tick is 14 ticks; the
        // slowdown inside half a tile adds a few.
        let (events, repath) = run(&mut units, &map, &rules, 40);
        assert!(events.is_empty(), "no UnitArrived for a return");
        assert!(repath.is_empty());
        let u = &units[&UnitId(2)];
        let arrive = Fx::from_ratio(25, 100);
        assert!(u.pos.dist_sq_i64(goal) <= (arrive * arrive).to_bits());
        assert!(!u.post.unwrap().returning, "parked again");
        let settled = u.pos;
        run(&mut units, &map, &rules, 20);
        assert_eq!(units[&UnitId(2)].pos, settled, "a parked unit holds still");
    }

    #[test]
    fn returning_unit_blocked_by_terrain_asks_for_a_path() {
        use crate::ids::Tile;
        let rules = rules();
        let mut map = Map::open(8, 8);
        for y in 0..8 {
            map.set_blocked(Tile::new(4, y), true);
        }
        let goal = map.center_of(Tile::new(6, 3));
        let mut units = BTreeMap::new();
        let mut u = unit(1, 3, 3);
        // Against the west face of the wall, post on the far side.
        u.pos = FxVec2::new(Fx::from_ratio(395, 100), Fx::from_ratio(35, 10));
        u.post = Some(Post {
            goal,
            returning: false,
        });
        units.insert(UnitId(1), u);
        let (events, repath) = run(&mut units, &map, &rules, 1);
        assert!(events.is_empty());
        assert_eq!(
            repath,
            vec![UnitId(1)],
            "the blocked return asks for a path"
        );
        let u = &units[&UnitId(1)];
        assert!(u.post.is_none());
        let order = u.order.as_ref().expect("the post became an order");
        assert_eq!(order.goal, goal);
        assert!(order.path.is_empty(), "waiting for the path");
        assert_eq!(order.repath_at, rules.repath_interval_ticks());
        assert!(map.passable(map.tile_of(u.pos)));
    }

    #[test]
    fn coincident_units_separate_deterministically_along_x() {
        let rules = rules();
        let map = Map::open(8, 8);
        let mut units = BTreeMap::new();
        let mut a = unit(1, 3, 3);
        let mut b = unit(2, 3, 3);
        a.pos = map.center_of(crate::ids::Tile::new(3, 3));
        b.pos = a.pos;
        units.insert(UnitId(1), a);
        units.insert(UnitId(2), b);
        run(&mut units, &map, &rules, 1);
        let (a, b) = (&units[&UnitId(1)], &units[&UnitId(2)]);
        assert!(a.pos.x < b.pos.x, "lower id west, higher id east");
        assert_eq!(a.pos.y, b.pos.y);
        // Circle correction alone resolves the whole overlap in one pass.
        let sum = a.radius + b.radius;
        assert!(a.pos.dist_sq_i64(b.pos) >= (sum * sum).to_bits());
        // Idle units keep their facing.
        assert_eq!(a.facing, FxVec2::from_ints(1, 0));
    }

    #[test]
    fn group_targets_correct_the_click_skip_blocked_tiles_and_pad() {
        use crate::ids::Tile;
        let mut map = Map::open(8, 8);
        map.set_blocked(Tile::new(3, 3), true);
        map.set_blocked(Tile::new(4, 3), true);
        // Click on a blocked tile: the BFS tries E (4,3), blocked, then SE
        // (4,4), so the spiral starts at (4,4): centre, east, south-east,
        // south. Ring 1 continues with (3,5), (3,4) and then (3,3) and
        // (4,3), which are skipped as blocked.
        let t = group_targets(&map, FxVec2::from_ints(3, 3), 8);
        assert_eq!(
            t,
            vec![
                map.center_of(Tile::new(4, 4)),
                map.center_of(Tile::new(5, 4)),
                map.center_of(Tile::new(5, 5)),
                map.center_of(Tile::new(4, 5)),
                map.center_of(Tile::new(3, 5)),
                map.center_of(Tile::new(3, 4)),
                map.center_of(Tile::new(5, 3)),
                map.center_of(Tile::new(6, 4)),
            ]
        );
        // A wall splits the map; targets stay in the click's component and
        // pad with the last one when the component runs out.
        let mut w = Map::open(4, 4);
        for y in 0..4 {
            w.set_blocked(Tile::new(2, y), true);
        }
        let t = group_targets(&w, FxVec2::from_ints(3, 0), 10);
        assert_eq!(t.len(), 10);
        assert!(
            t.iter().all(|p| p.x > Fx::from_int(3)),
            "east component only"
        );
        assert_eq!(t[4..], vec![t[3]; 6][..], "padded with the last target");
        // No passable tile at all: the click repeats.
        let mut b = Map::open(2, 2);
        for i in 0..4 {
            b.set_blocked(b.tile_at(i), true);
        }
        let click = FxVec2::from_ints(1, 1);
        assert_eq!(group_targets(&b, click, 3), vec![click; 3]);
        assert!(group_targets(&map, FxVec2::from_ints(0, 0), 0).is_empty());
    }
}
