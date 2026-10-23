//! Combat (`docs/design/combat.md`, "M4a implementation notes"): unit
//! stats resolved through the Modifier rows, the explicit `Attack`, target
//! acquisition by idle and attack-moving units, the Watchtower and Town
//! shooters, simultaneous hits, death and building destruction.
//!
//! Hashed state lives on the units and buildings: `Unit::damage_taken`,
//! `ready_at`, `target` and `intent`, and `Building::hp` and `ready_at`.
//! Nothing else carries over between ticks: `Stats` and `Plan` are
//! rebuilt inside every step, and the rules-only `CombatTables` that
//! `Stats` borrows are derived from the rules alone (built in `Sim::new`
//! and `Sim::rebuild_derived`), so a restore has nothing else to rebuild.
//!
//! Each tick, inside step 5 of `Sim::step`:
//!
//! - 5a `think`, from the start-of-tick positions: every unit with an
//!   attack in `UnitId` order, then every shooter in `BuildingId` order.
//!   - A unit with [`Intent::Attack`] (an explicit `Attack`): a target
//!     that is gone, the unit's own or a building with no hit points left
//!     ends the order. A target in weapon range is held ([`Engage::Hold`]).
//!     One within the unit's reach (the longer of `acquire_range_tiles` and
//!     its range) with a clear straight segment from the unit's position
//!     (to the target, or to the nearest point of a building's footprint),
//!     or a building whose footprint the unit stands beside, is chased
//!     directly ([`Engage::Chase`]), no path and no request; the segment
//!     is the walk the chase steers, so it stays clear while the target
//!     stands still. Otherwise the unit walks a path toward it (a path
//!     chase), which ends on the tile `chase_end` gives: the target's tile
//!     when the unit can walk there; else the tile of the unit's component
//!     nearest to it, as pathing corrects the goal, when the target is in
//!     weapon range from anywhere within the unit's arrive radius of that
//!     tile's centre; for a building, the reachable ring tile nearest to
//!     the unit. A target with no such tile cannot be reached and the order
//!     ends: at once, with no path, and on any later tick of the walk (a
//!     wall can close behind the target), so every attacker of a group
//!     gives up together. A new path chase is a move order to the target's
//!     tile (to the ring tile for a building). Once a unit target has
//!     drifted more than `chase_regoal_tiles` from the goal (or a
//!     building's ring tile is cut off), the order is re-goaled like a
//!     repath: the unit keeps its path until step 4 writes the new one, and
//!     while a request is outstanding it does not arrive at the end of its
//!     path (`Plan::awaiting`). A path chase that has arrived (no order, a
//!     post) walks again when its end tile is no longer the tile it arrived
//!     at (the target moved), and otherwise gives up. There is no leash.
//!   - A unit that acquires: one that is idle (no order and no job, with
//!     or without a post: a guard) or attack-moving ([`Intent::AttackMove`],
//!     while its order lasts; once the order is gone, because it arrived,
//!     the intent ends and the unit guards where it stands). A plain
//!     `Move`, a job walk or a rally walk never engages. Its candidates are
//!     the enemy units (found through the spatial grid) and the enemy
//!     buildings and sites with hit points left that are within its reach,
//!     edge to edge, and in weapon range or directly chaseable (the
//!     explicit attack's test above, `chases_directly`, on the straight
//!     segment from the unit's position): it never walks a path to a
//!     target. They rank by `(tier, out of weapon range, dist_sq)`: tier 0
//!     is enemy units and tier 1 enemy buildings, the other way round for
//!     a kind with `prefer: Buildings` (the Mangonel);
//!     the squared distance is centre to centre for a unit and to the
//!     nearest point of the footprint for a building. Candidates that tie
//!     exactly on that key are put in `TargetId` order and one is drawn
//!     with `rng_index`, the only draw acquisition makes. A unit keeps its
//!     target while it is valid and in weapon range or directly chaseable
//!     (so it is dropped beyond the reach), unless a candidate ranks
//!     strictly better on `(tier, out of weapon range)`: a target out of
//!     weapon range gives way to a candidate of its tier in weapon range,
//!     and a target of the worse tier to any candidate of the better one.
//!     Nothing is searched while the target is of the better tier and in
//!     weapon range. The unit then holds or chases its target directly; an
//!     attack-mover keeps its order meanwhile (it is not followed) and
//!     resumes it when the fight is over, and a guard keeps its post and
//!     walks back to it afterwards. The annex-hold guard rule (M5a): a
//!     guard whose kind `annexation_attackers` selects, standing within
//!     `annexation_adjacent_tiles` of an enemy complete Town at 0 HP, keeps
//!     and takes only targets in weapon range, so it holds and never chases
//!     off the ring it annexes from.
//!   - A shooter: a complete building with an `attack` (the Watchtower and
//!     the Town) and hit points left, whose cooldown has run out, aims at
//!     the nearest enemy unit whose edge is within its range of the
//!     footprint (`dist_sq` to the footprint, then a draw among exact ties
//!     in `UnitId` order). Shooters fire at units only and keep no target:
//!     the aim lasts the tick.
//! - 5b `movement::step` takes the engagements: a holding unit stands and
//!   faces its target, a chasing one steers straight at it. Then 5b'
//!   (`attrition::step`, M5a) adds the attrition pulses to `damage_taken`.
//! - 5c `resolve`: every attacker whose target is still valid and in
//!   weapon range from the post-movement positions, and whose cooldown has
//!   run out, hits, in `UnitId` order, then every shooter whose aim is
//!   still in range, in `BuildingId` order; the hits are summed per target
//!   and applied at once through `apply_damage`, so a unit killed this
//!   tick (by a hit or a pulse) still lands its own and a lower id has no
//!   first strike. Then `reap` removes the dead, attrition's included, and
//!   the destroyed, also those `Command::DebugDamage` dealt in step 3.
//!
//! Range is measured edge to edge: a unit is in range of a unit target when
//! `dist_sq_i64(a, t) <= (range + r_a + r_t)^2`, and of a building when the
//! squared distance to the nearest point of the footprint rectangle (the
//! tiles' area, `[x0, x1 + 1] x [y0, y1 + 1]`) is at most
//! `(range + r_a)^2`, both in `dist_sq_i64`'s scale. A shooter has a unit
//! in range when the squared distance from the unit's centre to the
//! footprint is at most `(range + r_u)^2`.
//!
//! A hit does `floor(floor(raw * counter / 1000) * variance / 1000)`, where
//! `raw` is the attacker's damage with its owner's `UnitDamage` rows (a
//! shooter's `damage` as written: no row applies to buildings), `counter`
//! the counter table's permille for the pair (1000 when not listed, and
//! for every shot) and `variance` 1000, or `1000 + rng_index(2v + 1) - v`
//! when `damage_variance_permille` `v` is not 0, one draw per hit in hit
//! order. Overkill is accepted: attackers do not spread their hits by the
//! damage already dealt this tick.

use crate::construction::{self, Rect, builder_stand, building_rect, order_move};
use crate::fx::{Fx, FxVec2};
use crate::ids::{BuildingId, BuildingKindId, PlayerId, TargetId, Tile, UnitId, UnitKindId};
use crate::map::Map;
use crate::movement::SpatialGrid;
use crate::pathing::PathRequest;
use crate::state::{Construction, Intent, RejectReason, SimEvent, State, Unit, fx_from_x100};
use crate::tech;
use crate::town;
use rand_core::Rng;
use rules::{Defender, Prefer, Rules};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// A player's attack for one unit kind, with the owner's Modifier rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttackStats {
    /// Damage per hit with the `UnitDamage` rows, before counters and
    /// variance; at least 0.
    pub damage: i32,
    /// Weapon range, edge to edge, in tiles.
    pub range: Fx,
    /// How far the unit looks for enemies and chases directly: the longer
    /// of `acquire_range_tiles` and the range, in tiles, edge to edge.
    pub reach: Fx,
    /// Ticks from one hit to the next (`cooldown_ds`, converted once).
    pub cooldown_ticks: u32,
}

/// A player's combat stats for one unit kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitStats {
    /// Hit points with the `UnitHp` rows; at least 1.
    pub max_hp: i32,
    /// `None` for a kind that does not attack.
    pub attack: Option<AttackStats>,
}

/// `p`'s combat stats for unit kind `kind`, resolved now.
///
/// # Panics
///
/// If `kind` is not in the rules.
pub(crate) fn unit_stats(state: &State, rules: &Rules, p: PlayerId, kind: UnitKindId) -> UnitStats {
    let def = rules.unit_kind(kind.0).expect("validated unit kind");
    let attack = def.attack.as_ref().map(|a| AttackStats {
        damage: tech::unit_damage(state, rules, p, kind).expect("the kind attacks"),
        range: fx_from_x100(a.range_tiles_x100),
        reach: fx_from_x100(rules.acquire_reach(a)),
        cooldown_ticks: rules.cooldown_ticks(a),
    });
    UnitStats {
        max_hp: tech::unit_max_hp(state, rules, p, kind),
        attack,
    }
}

/// A shooter's attack (`buildings.ron`): no Modifier row applies to it and
/// no counter row names it.
#[derive(Debug, Clone, Copy)]
struct ShooterStats {
    /// Damage per shot.
    damage: i64,
    /// Range from the footprint to the target's edge, in tiles.
    range: Fx,
    /// Ticks from one shot to the next.
    cooldown_ticks: u32,
}

/// The rules-only combat tables: the counter table, each kind's
/// preference, the largest unit radius, the shooters and the variance.
/// They depend on the rules alone, so the sim builds them once, in
/// `Sim::new`, and again in `Sim::rebuild_derived` (derived state, never
/// hashed or snapshotted), instead of in every step; [`Stats`] borrows
/// them.
pub(crate) struct CombatTables {
    kinds: usize,
    /// Permille, `[attacker * kinds + defender]`.
    counters: Vec<i64>,
    /// Permille against any building, `[attacker]`.
    vs_buildings: Vec<i64>,
    /// Which enemies each unit kind acquires first, `[kind]`.
    prefer: Vec<Prefer>,
    /// The largest radius of any unit kind, which widens grid queries.
    max_radius: Fx,
    /// Each building kind's shot, `[kind]`; `None` for a kind that does
    /// not shoot.
    shooters: Vec<Option<ShooterStats>>,
    /// `damage_variance_permille`.
    variance: i64,
}

impl CombatTables {
    /// Build the tables from the rules.
    pub(crate) fn new(rules: &Rules) -> CombatTables {
        let kinds = rules.units.len();
        let mut counters = Vec::with_capacity(kinds * kinds);
        let mut vs_buildings = Vec::with_capacity(kinds);
        for a in &rules.units {
            for d in &rules.units {
                let permille = rules.counter_permille(&a.id, &Defender::Kind(d.id.clone()));
                counters.push(i64::from(permille));
            }
            vs_buildings.push(i64::from(
                rules.counter_permille(&a.id, &Defender::Buildings),
            ));
        }
        let prefer = rules
            .units
            .iter()
            .map(|k| k.attack.as_ref().map_or(Prefer::Units, |a| a.prefer))
            .collect();
        let max_radius = rules
            .units
            .iter()
            .map(|k| fx_from_x100(k.radius_tiles_x100))
            .max()
            .unwrap_or(Fx::ZERO);
        let shooters = rules
            .buildings
            .iter()
            .map(|b| {
                b.attack.as_ref().map(|a| ShooterStats {
                    damage: i64::from(a.damage),
                    range: fx_from_x100(a.range_tiles_x100),
                    cooldown_ticks: rules.cooldown_ticks(a),
                })
            })
            .collect();
        CombatTables {
            kinds,
            counters,
            vs_buildings,
            prefer,
            max_radius,
            shooters,
            variance: i64::from(rules.damage_variance_permille),
        }
    }
}

/// Every player's [`UnitStats`] per kind, with the rules-only
/// [`CombatTables`], built at the start of step 5 and read by `think` and
/// `resolve`. Research completes in step 6b, so the table holds for the
/// whole of step 5.
pub(crate) struct Stats<'a> {
    tables: &'a CombatTables,
    /// `[player * kinds + kind]`.
    units: Vec<UnitStats>,
}

impl<'a> Stats<'a> {
    /// Resolve every player's stats for every kind.
    pub(crate) fn new(state: &State, rules: &Rules, tables: &'a CombatTables) -> Stats<'a> {
        let kinds = tables.kinds;
        let kind_id = |k: usize| UnitKindId(u16::try_from(k).expect("validated unit count"));
        let mut units = Vec::with_capacity(state.players.len() * kinds);
        for p in &state.players {
            for k in 0..kinds {
                units.push(unit_stats(state, rules, p.id, kind_id(k)));
            }
        }
        Stats { tables, units }
    }

    /// Player `p`'s stats for `kind`.
    pub(crate) fn unit(&self, p: PlayerId, kind: UnitKindId) -> &UnitStats {
        &self.units[usize::from(p.0) * self.tables.kinds + usize::from(kind.0)]
    }

    fn counter(&self, attacker: UnitKindId, defender: UnitKindId) -> i64 {
        self.tables.counters[usize::from(attacker.0) * self.tables.kinds + usize::from(defender.0)]
    }

    /// Permille of `attacker`'s hits against any building.
    fn vs_buildings(&self, attacker: UnitKindId) -> i64 {
        self.tables.vs_buildings[usize::from(attacker.0)]
    }

    /// The largest radius of any unit kind.
    fn max_radius(&self) -> Fx {
        self.tables.max_radius
    }

    /// `damage_variance_permille`.
    fn variance(&self) -> i64 {
        self.tables.variance
    }

    /// The tier of a target, a building or a unit, for an attacker of
    /// `kind`: 0 is taken first (enemy units, or enemy buildings for a kind
    /// with `prefer: Buildings`).
    fn tier(&self, kind: UnitKindId, building: bool) -> u8 {
        let buildings_first = self.tables.prefer[usize::from(kind.0)] == Prefer::Buildings;
        u8::from(building != buildings_first)
    }

    /// The shot of building kind `kind`, if it shoots.
    fn shooter(&self, kind: BuildingKindId) -> Option<ShooterStats> {
        self.tables.shooters[usize::from(kind.0)]
    }
}

/// How a unit engages this tick; decided by `think`, carried out by
/// `movement::step`. Never stored across ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engage {
    /// In weapon range: stand (idle push cap, no path following) and face
    /// `face`.
    Hold {
        /// The target's position, or the nearest point of its footprint.
        face: FxVec2,
    },
    /// Steer straight at this point (the target's position, or the nearest
    /// point of its footprint) at full speed, with the moving push cap.
    Chase(FxVec2),
}

/// What `think` decided for this tick.
#[derive(Debug, Default)]
pub(crate) struct Plan {
    /// Engaged units; every other unit moves as it would without combat.
    pub engage: BTreeMap<UnitId, Engage>,
    /// Path chasers with a path request outstanding (a re-goal or a
    /// repath). They keep following their path but do not arrive at its
    /// end: arriving would end the order, step 4 would then drop the new
    /// path, and the arrived chase would give up or ask again from the back
    /// of the queue. They wait at the end until the new path lands.
    pub awaiting: BTreeSet<UnitId>,
    /// Each ready shooter's aim this tick, in `BuildingId` order; it fires
    /// in step 5c if the unit is still in range.
    pub aims: Vec<(BuildingId, UnitId)>,
}

/// A target as seen by an attacker.
#[derive(Debug, Clone, Copy)]
enum Aim {
    Unit {
        pos: FxVec2,
        radius: Fx,
        kind: UnitKindId,
    },
    Building {
        rect: Rect,
    },
}

impl Aim {
    /// The point an attacker at `from` steers at and faces.
    fn point(&self, from: FxVec2) -> FxVec2 {
        match *self {
            Aim::Unit { pos, .. } => pos,
            Aim::Building { rect } => nearest_point(from, rect),
        }
    }
}

/// `t` as a target for a unit of `p`: it exists, belongs to another player
/// and, for a building, has hit points left (a complete Town at 0 HP and a
/// building about to be destroyed are not targets). `None` otherwise.
fn locate(state: &State, rules: &Rules, p: PlayerId, t: TargetId) -> Option<Aim> {
    match t {
        TargetId::Unit(id) => state
            .units
            .get(&id)
            .filter(|u| u.owner != p)
            .map(|u| Aim::Unit {
                pos: u.pos,
                radius: u.radius,
                kind: u.kind,
            }),
        TargetId::Building(id) => state
            .buildings
            .get(&id)
            .filter(|b| b.owner != p && b.hp > 0)
            .map(|b| Aim::Building {
                rect: building_rect(rules, &state.map, b),
            }),
    }
}

/// `r` squared, in `dist_sq_i64`'s scale.
pub(crate) fn reach_sq(r: Fx) -> i64 {
    (r * r).to_bits()
}

/// The nearest point to `p` of the area the footprint `rect` covers,
/// `[x0, x1 + 1] x [y0, y1 + 1]` in tiles; `p` itself when inside.
pub(crate) fn nearest_point(p: FxVec2, rect: Rect) -> FxVec2 {
    let span = |lo: u16, hi: u16, v: Fx| {
        v.clamp(Fx::from_int(i32::from(lo)), Fx::from_int(i32::from(hi) + 1))
    };
    FxVec2::new(span(rect.x0, rect.x1, p.x), span(rect.y0, rect.y1, p.y))
}

/// Squared distance from `p` to the footprint `rect` (0 inside), in
/// `dist_sq_i64`'s scale.
pub(crate) fn dist_sq_to_rect(p: FxVec2, rect: Rect) -> i64 {
    p.dist_sq_i64(nearest_point(p, rect))
}

/// Whether `aim` lies within `extra` of a unit at `pos` with `radius`,
/// edge to edge.
fn within(pos: FxVec2, radius: Fx, extra: Fx, aim: &Aim) -> bool {
    match *aim {
        Aim::Unit {
            pos: t, radius: rt, ..
        } => pos.dist_sq_i64(t) <= reach_sq(extra + radius + rt),
        Aim::Building { rect } => dist_sq_to_rect(pos, rect) <= reach_sq(extra + radius),
    }
}

/// Whether the straight segment from `a` to `b` stays on passable tiles
/// (tiles of `ignore`, the target's footprint, count as passable): every
/// tile it passes through is passable and, where it passes exactly through
/// a tile corner, both tiles beside the corner are, as for a diagonal step
/// in `Map::neighbors8`. It is the walk `Engage::Chase` steers, so for a
/// target that stands still the test keeps holding from every point the
/// chase reaches. Between two tile centres it passes the tiles of the
/// tile line between them.
pub(crate) fn segment_clear(map: &Map, a: FxVec2, b: FxVec2, ignore: Option<Rect>) -> bool {
    const ONE: i128 = 1 << 32;
    let open = |x: i128, y: i128| {
        let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) else {
            return false;
        };
        let t = Tile::new(x, y);
        map.passable(t) || ignore.is_some_and(|r| r.contains(t))
    };
    let (s, e) = (map.tile_of(a), map.tile_of(b));
    let (mut x, mut y) = (i128::from(s.x), i128::from(s.y));
    let (ex, ey) = (i128::from(e.x), i128::from(e.y));
    let (sx, sy) = ((ex - x).signum(), (ey - y).signum());
    let (nx, ny) = ((ex - x).abs(), (ey - y).abs());
    let (ax, ay) = (i128::from(a.x.to_bits()), i128::from(a.y.to_bits()));
    let dx = (i128::from(b.x.to_bits()) - ax).abs();
    let dy = (i128::from(b.y.to_bits()) - ay).abs();
    if !open(x, y) {
        return false;
    }
    let (mut done_x, mut done_y) = (0, 0);
    while done_x < nx || done_y < ny {
        // Distance from `a` to the next boundary on each axis; the
        // boundary with the smaller distance / |d| is crossed first.
        let to_x = if sx > 0 {
            (x + 1) * ONE - ax
        } else {
            ax - x * ONE
        };
        let to_y = if sy > 0 {
            (y + 1) * ONE - ay
        } else {
            ay - y * ONE
        };
        let first = if done_y == ny {
            Ordering::Less
        } else if done_x == nx {
            Ordering::Greater
        } else {
            (to_x * dy).cmp(&(to_y * dx))
        };
        match first {
            Ordering::Equal => {
                if !open(x + sx, y) || !open(x, y + sy) {
                    return false;
                }
                x += sx;
                y += sy;
                done_x += 1;
                done_y += 1;
            }
            Ordering::Less => {
                x += sx;
                done_x += 1;
            }
            Ordering::Greater => {
                y += sy;
                done_y += 1;
            }
        }
        if !open(x, y) {
            return false;
        }
    }
    true
}

/// A uniform index in `0..n` from one draw of the sim's RNG:
/// `(next_u32 * n) >> 32`. Stable across `rand` versions; `n > 0`.
pub(crate) fn rng_index(rng: &mut rand_pcg::Pcg32, n: u32) -> u32 {
    u32::try_from((u64::from(rng.next_u32()) * u64::from(n)) >> 32).expect("below n")
}

/// Chebyshev distance between two tiles.
fn chebyshev(a: Tile, b: Tile) -> u16 {
    a.x.abs_diff(b.x).max(a.y.abs_diff(b.y))
}

/// Whether `id`'s unit kind attacks.
fn armed(rules: &Rules, kind: UnitKindId) -> bool {
    rules.unit_kind(kind.0).is_some_and(|k| k.attack.is_some())
}

/// Validate `Attack { units, target }` for `p` without changing anything.
/// Checks, first failure wins: the target exists, is not `p`'s and, for a
/// building, has hit points left (`InvalidTarget`); at least one listed
/// unit is `p`'s and of a kind that attacks (`NoValidUnits`). Returns those
/// units, deduplicated, in `UnitId` order.
pub(crate) fn validate_attack(
    state: &State,
    rules: &Rules,
    p: PlayerId,
    units: &[UnitId],
    target: TargetId,
) -> Result<Vec<UnitId>, RejectReason> {
    if locate(state, rules, p, target).is_none() {
        return Err(RejectReason::InvalidTarget);
    }
    let attackers: BTreeSet<UnitId> = units
        .iter()
        .copied()
        .filter(|id| {
            state
                .units
                .get(id)
                .is_some_and(|u| u.owner == p && armed(rules, u.kind))
        })
        .collect();
    if attackers.is_empty() {
        return Err(RejectReason::NoValidUnits);
    }
    Ok(attackers.into_iter().collect())
}

/// Apply a validated `Attack`: each unit, in `UnitId` order, drops its
/// order, post, job and path request and takes `target` with
/// [`Intent::Attack`]. The chase starts in step 5a of the same tick.
pub(crate) fn apply_attack(state: &mut State, attackers: &[UnitId], target: TargetId) {
    for &id in attackers {
        let u = state.units.get_mut(&id).expect("validated unit");
        u.order = None;
        u.post = None;
        u.job = None;
        u.seek_at = None;
        u.target = Some(target);
        u.intent = Intent::Attack;
        state.pathing.cancel(id);
    }
}

/// End `id`'s combat order: no target, [`Intent::Plain`], no order and no
/// path request.
fn give_up(state: &mut State, id: UnitId) {
    let u = state.units.get_mut(&id).expect("unit exists");
    u.disengage();
    u.order = None;
    state.pathing.cancel(id);
}

/// An explicit attacker engages directly: any path-chase order is dropped
/// with its request first (step 4 writes completed paths into whatever
/// order the unit has), and the post a path chase left is spent.
fn engage_directly(state: &mut State, id: UnitId) {
    let u = state.units.get_mut(&id).expect("unit exists");
    u.post = None;
    if u.order.take().is_some() {
        state.pathing.cancel(id);
    }
}

/// Whether a unit standing on `a` can walk to `b`: `a` is passable and
/// `b` is in its component (blocked tiles are in none).
fn walkable(map: &Map, a: Tile, b: Tile) -> bool {
    map.passable(a) && map.component_of(a) == map.component_of(b)
}

/// Whether `u`, with weapon `range`, has the unit target at `pos` with
/// radius `rt` in weapon range from anywhere within `u`'s arrive radius of
/// the centre of `stand`: so a unit that walks to `stand` and arrives
/// there can hit.
fn hittable_from(
    rules: &Rules,
    map: &Map,
    u: &Unit,
    range: Fx,
    pos: FxVec2,
    rt: Fx,
    stand: Tile,
) -> bool {
    let kind = rules.unit_kind(u.kind.0).expect("validated unit kind");
    let edge = range + u.radius + rt - fx_from_x100(kind.arrive_radius_tiles_x100);
    edge > Fx::ZERO && map.center_of(stand).dist_sq_i64(pos) <= reach_sq(edge)
}

/// Where a path chase by `u` (weapon `range`) on `aim` ends, or `None` when
/// the target cannot be reached:
///
/// - a building: the reachable ring tile nearest to the unit, as a
///   builder's (`builder_stand`);
/// - a unit on a tile the attacker can walk to: that tile;
/// - a unit anywhere else (walled off, another component): the tile of the
///   attacker's component nearest to it, the tile pathing corrects the goal
///   to (`Map::nearest_passable`), provided the target is in weapon range
///   from anywhere within the attacker's arrive radius of that tile's
///   centre (`hittable_from`). A melee attacker never hits a target
///   walled off by a whole tile; a Bowman shoots over the wall.
///
/// It depends only on the map, the two positions and the stats, so every
/// attacker of a group decides alike, and an unreachable target costs no
/// path.
fn chase_end(map: &Map, rules: &Rules, u: &Unit, range: Fx, aim: &Aim) -> Option<Tile> {
    match *aim {
        Aim::Building { rect } => builder_stand(map, rect, u.pos).ok(),
        Aim::Unit {
            pos, radius: rt, ..
        } => {
            let (here, there) = (map.tile_of(u.pos), map.tile_of(pos));
            if !map.passable(here) || walkable(map, here, there) {
                // Units never stand on a blocked tile; should one, pathing
                // corrects its start, so nothing is judged from there.
                return Some(there);
            }
            let stand = map.nearest_passable(there, Some(map.component_of(here)))?;
            hittable_from(rules, map, u, range, pos, rt, stand).then_some(stand)
        }
    }
}

/// Re-goal `id`'s path chase to `goal` like a repath: its path and next
/// waypoint are kept until step 4 writes the new path, so a saturated
/// budget delays the chaser instead of parking it, and the request joins
/// the queue with `requested_tick = tick`. The periodic timer restarts.
fn regoal(state: &mut State, rules: &Rules, id: UnitId, goal: FxVec2, tick: u32) {
    let u = state.units.get_mut(&id).expect("unit exists");
    let from = state.map.tile_of(u.pos);
    let order = u.order.as_mut().expect("a path chase has an order");
    order.goal = goal;
    order.repath_at = tick + rules.repath_interval_ticks();
    state.pathing.request(PathRequest {
        requested_tick: tick,
        unit: id,
        from,
        to: state.map.tile_of(goal),
    });
}

/// Whether `u`, at `pos` with `radius`, can chase `aim` directly: within
/// `reach` edge to edge and with a clear straight segment from the unit's
/// position to the target's (to the footprint's nearest point for a
/// building, the footprint itself counting as clear; `segment_clear`), or,
/// for a building, from a tile beside its footprint. The segment is the
/// walk `Engage::Chase` steers, so a chase that starts never loses the test
/// while its target stands still. On a tile beside the footprint the
/// nearest point of the footprint lies on the tile's own edge or corner,
/// so the straight steer never leaves the tile, whatever blocks the tiles
/// around it. The segment test cannot see that: a segment ending on a tile
/// boundary counts the tile beyond it (outside the footprint on its east
/// and south sides), and a corner crossing needs both tiles beside the
/// corner. A building's path chase ends on such a tile, out of melee range
/// when it is a corner one. The explicit `Attack` and acquisition both
/// decide with it, for taking a target and for keeping it.
fn chases_directly(map: &Map, pos: FxVec2, radius: Fx, reach: Fx, aim: &Aim) -> bool {
    if !within(pos, radius, reach, aim) {
        return false;
    }
    match *aim {
        Aim::Building { rect } => {
            rect.chebyshev(map.tile_of(pos)) <= 1
                || segment_clear(map, pos, nearest_point(pos, rect), Some(rect))
        }
        Aim::Unit { pos: t, .. } => segment_clear(map, pos, t, None),
    }
}

/// A building an acquiring unit may take: one with hit points left (so not
/// a complete Town at 0 HP), with its owner and footprint.
type Targetable = (BuildingId, PlayerId, Rect);

/// Step 5a: decide how every unit with an attack engages this tick, in
/// `UnitId` order, then aim every ready shooter, in `BuildingId` order (see
/// the module docs), from the start-of-tick positions in `grid`. Issues and
/// cancels the explicit attackers' path-chase requests and sets the
/// acquiring units' targets; returns the direct engagements, the path
/// chasers waiting for a path for `movement::step`, and the shooters'
/// aims for `resolve`.
pub(crate) fn think(
    state: &mut State,
    rules: &Rules,
    grid: &SpatialGrid,
    stats: &Stats<'_>,
    tick: u32,
) -> Plan {
    let mut plan = Plan::default();
    let units: Vec<(UnitId, Intent)> = state
        .units
        .values()
        .filter(|u| u.intent == Intent::Attack || stats.unit(u.owner, u.kind).attack.is_some())
        .map(|u| (u.id, u.intent))
        .collect();
    let targetable: Vec<Targetable> = state
        .buildings
        .values()
        .filter(|b| b.hp > 0)
        .map(|b| (b.id, b.owner, building_rect(rules, &state.map, b)))
        .collect();
    let ring = AnnexRing::new(state, rules);
    let mut scratch = Vec::new();
    for (id, intent) in units {
        if intent == Intent::Attack {
            think_explicit(state, rules, stats, id, tick, &mut plan);
        } else {
            think_acquiring(
                state,
                rules,
                grid,
                stats,
                &targetable,
                &ring,
                id,
                &mut scratch,
                &mut plan,
            );
        }
    }
    think_shooters(state, rules, grid, stats, tick, &mut plan);
    plan
}

/// 5a for a unit with an explicit `Attack` (see the module docs).
fn think_explicit(
    state: &mut State,
    rules: &Rules,
    stats: &Stats<'_>,
    id: UnitId,
    tick: u32,
    plan: &mut Plan,
) {
    let u = &state.units[&id];
    let attack = stats.unit(u.owner, u.kind).attack;
    let aim = u.target.and_then(|t| locate(state, rules, u.owner, t));
    let (Some(attack), Some(aim)) = (attack, aim) else {
        give_up(state, id);
        return;
    };
    let point = aim.point(u.pos);
    if within(u.pos, u.radius, attack.range, &aim) {
        engage_directly(state, id);
        plan.engage.insert(id, Engage::Hold { face: point });
        return;
    }
    let map = &state.map;
    if chases_directly(map, u.pos, u.radius, attack.reach, &aim) {
        engage_directly(state, id);
        plan.engage.insert(id, Engage::Chase(point));
        return;
    }
    let here = map.tile_of(u.pos);
    // A path chase. Its order's goal is the target's tile (pathing
    // corrects it to the end tile), or a building's ring tile.
    let goal = |end: Tile| match aim {
        Aim::Unit { pos, .. } => map.center_of(map.tile_of(pos)),
        Aim::Building { .. } => map.center_of(end),
    };
    match (&u.order, u.post) {
        // Walking. Whether the target can still be reached is checked
        // every tick, so a wall closing round it ends the chase at once
        // and no group is left jostling for a corrected goal tile it
        // cannot all reach. The cheap checks come first: a unit target
        // the unit can walk to, or one it can hit from the end of its
        // path, or a building whose ring tile it can still walk to;
        // `chase_end` searches only when they fail.
        (Some(order), _) => {
            let goal_tile = map.tile_of(order.goal);
            let to = match aim {
                Aim::Unit {
                    pos, radius: rt, ..
                } => {
                    let there = map.tile_of(pos);
                    let reachable = walkable(map, here, there)
                        || order.path.last().is_some_and(|&end| {
                            walkable(map, here, end)
                                && hittable_from(rules, map, u, attack.range, pos, rt, end)
                        })
                        || chase_end(map, rules, u, attack.range, &aim).is_some();
                    if !reachable {
                        give_up(state, id);
                        return;
                    }
                    let drift = chebyshev(goal_tile, there);
                    (u32::from(drift) > rules.chase_regoal_tiles).then(|| map.center_of(there))
                }
                Aim::Building { .. } if walkable(map, here, goal_tile) => None,
                Aim::Building { .. } => match chase_end(map, rules, u, attack.range, &aim) {
                    Some(end) => Some(goal(end)),
                    None => {
                        give_up(state, id);
                        return;
                    }
                },
            };
            if let Some(to) = to
                && !state.pathing.has_request(id)
            {
                regoal(state, rules, id, to, tick);
            }
            if state.pathing.has_request(id) {
                plan.awaiting.insert(id);
            }
        }
        (None, None) => match chase_end(map, rules, u, attack.range, &aim) {
            Some(end) => {
                let to = goal(end);
                order_move(state, rules, id, to, tick);
            }
            None => give_up(state, id),
        },
        // The path chase arrived and the target is still out of reach.
        // When the end tile is no longer the tile it arrived at (a unit
        // target walked on, by up to `chase_regoal_tiles`, round an
        // obstacle), walk again: each walk needs the target to have
        // moved, so this is no request storm. Otherwise it cannot be
        // reached from here: give up.
        (None, Some(post)) => match chase_end(map, rules, u, attack.range, &aim) {
            Some(end) if end != map.tile_of(post.goal) => {
                let to = goal(end);
                order_move(state, rules, id, to, tick);
            }
            _ => give_up(state, id),
        },
    }
}

/// The annex-hold rule's inputs (M5a, owner decision 6): every complete
/// Town at 0 HP with its owner and footprint (usually none), which kinds
/// `annexation_attackers` selects and how near counts as adjacent.
struct AnnexRing {
    /// `(owner, footprint)` of each complete Town at 0 HP, `BuildingId`
    /// order.
    towns: Vec<(PlayerId, Rect)>,
    /// `[kind]`: selected by `annexation_attackers`.
    attackers: Vec<bool>,
    /// `annexation_adjacent_tiles`.
    adjacent: u16,
}

impl AnnexRing {
    fn new(state: &State, rules: &Rules) -> AnnexRing {
        let towns: Vec<(PlayerId, Rect)> = state
            .buildings
            .values()
            .filter(|b| b.town_state.is_some() && b.hp == 0)
            .map(|b| (b.owner, building_rect(rules, &state.map, b)))
            .collect();
        let attackers = if towns.is_empty() {
            Vec::new()
        } else {
            rules
                .units
                .iter()
                .map(|k| rules.annexation_attackers.iter().any(|s| s.selects(k)))
                .collect()
        };
        AnnexRing {
            towns,
            attackers,
            adjacent: u16::try_from(rules.annexation_adjacent_tiles).expect("validated <= 4"),
        }
    }

    /// Whether `u`, an idle guard, holds an annexation ring: its kind can
    /// annex and it stands within `annexation_adjacent_tiles` of an enemy
    /// complete Town at 0 HP. Such a unit keeps and takes only targets in
    /// weapon range and never chases, so nothing draws it off the ring.
    fn holds(&self, map: &Map, u: &Unit) -> bool {
        if self.towns.is_empty() || !self.attackers[usize::from(u.kind.0)] {
            return false;
        }
        let tile = map.tile_of(u.pos);
        self.towns
            .iter()
            .any(|&(owner, rect)| owner != u.owner && rect.chebyshev(tile) <= self.adjacent)
    }
}

/// What an acquiring unit takes of the enemies of one tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Want {
    /// None of them.
    Nothing,
    /// Only those in weapon range.
    InRange,
    /// Those in weapon range or directly chaseable.
    InReach,
}

/// A target an acquiring unit could take.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    /// `(tier, out of weapon range, squared distance)`, best first.
    key: (u8, bool, i64),
    id: TargetId,
    aim: Aim,
}

/// The candidates `u` would take, ranked by `Candidate::key` (see the
/// module docs), among those that rank strictly better than `limit` on
/// `(tier, out of weapon range)`: the best one, or every one that ties
/// with it exactly, in `TargetId` order. Empty when none is in weapon
/// range or directly chaseable. `max_want` caps what is considered:
/// `Want::InRange` for a unit holding an annexation ring (M5a), which
/// takes only targets in weapon range. Reads `targetable` for buildings and
/// the grid for units; `scratch` is reused between units.
#[allow(clippy::too_many_arguments)]
fn best_candidates(
    state: &State,
    grid: &SpatialGrid,
    stats: &Stats<'_>,
    targetable: &[Targetable],
    u: &Unit,
    attack: &AttackStats,
    limit: (u8, bool),
    max_want: Want,
    scratch: &mut Vec<Candidate>,
) -> Vec<Candidate> {
    scratch.clear();
    let want = |tier: u8| {
        let w = if (tier, false) >= limit {
            Want::Nothing
        } else if (tier, true) >= limit {
            Want::InRange
        } else {
            Want::InReach
        };
        w.min(max_want)
    };
    let mut consider = |want: Want, tier: u8, id: TargetId, aim: Aim, dist_sq: i64| {
        let in_range = within(u.pos, u.radius, attack.range, &aim);
        let ok = match want {
            Want::Nothing => false,
            Want::InRange => in_range,
            Want::InReach => in_range || within(u.pos, u.radius, attack.reach, &aim),
        };
        if ok {
            scratch.push(Candidate {
                key: (tier, !in_range, dist_sq),
                id,
                aim,
            });
        }
    };
    let unit_tier = stats.tier(u.kind, false);
    let units = want(unit_tier);
    if units != Want::Nothing {
        let edge = if units == Want::InRange {
            attack.range
        } else {
            attack.reach
        };
        grid.for_each_in(u.pos, edge + u.radius + stats.max_radius(), |other| {
            let Some(o) = state.units.get(&other).filter(|o| o.owner != u.owner) else {
                return;
            };
            let aim = Aim::Unit {
                pos: o.pos,
                radius: o.radius,
                kind: o.kind,
            };
            consider(
                units,
                unit_tier,
                TargetId::Unit(o.id),
                aim,
                u.pos.dist_sq_i64(o.pos),
            );
        });
    }
    let building_tier = 1 - unit_tier;
    let buildings = want(building_tier);
    if buildings != Want::Nothing {
        for &(id, owner, rect) in targetable {
            if owner != u.owner {
                consider(
                    buildings,
                    building_tier,
                    TargetId::Building(id),
                    Aim::Building { rect },
                    dist_sq_to_rect(u.pos, rect),
                );
            }
        }
    }
    scratch.sort_unstable_by_key(|c| (c.key, c.id));
    // The best candidate in weapon range or directly chaseable; the
    // segment test runs lazily, in rank order.
    let qualifies = |c: &Candidate| {
        !c.key.1 || chases_directly(&state.map, u.pos, u.radius, attack.reach, &c.aim)
    };
    let Some(first) = scratch.iter().position(qualifies) else {
        return Vec::new();
    };
    let key = scratch[first].key;
    scratch[first..]
        .iter()
        .take_while(|c| c.key == key)
        .filter(|c| qualifies(c))
        .copied()
        .collect()
}

/// 5a for a unit that acquires: an idle guard or an attack-mover (see the
/// module docs). Sets or clears its target and engages it. An idle guard
/// holding an annexation ring (`AnnexRing::holds`) keeps a target only in
/// weapon range and takes only targets in weapon range, so it only ever
/// holds (M5a).
#[allow(clippy::too_many_arguments)]
fn think_acquiring(
    state: &mut State,
    rules: &Rules,
    grid: &SpatialGrid,
    stats: &Stats<'_>,
    targetable: &[Targetable],
    ring: &AnnexRing,
    id: UnitId,
    scratch: &mut Vec<Candidate>,
    plan: &mut Plan,
) {
    let u = state.units.get_mut(&id).expect("unit exists");
    match u.intent {
        // A plain move, a job walk, a rally walk or a guard walking a path
        // back to its post never engages.
        Intent::Plain if u.order.is_some() || u.job.is_some() => {
            u.target = None;
            return;
        }
        // The attack-move arrived (step 4 ends it when its path is
        // unreachable, and an engaged attack-mover keeps its order): the
        // unit guards where it stands.
        Intent::AttackMove { .. } if u.order.is_none() => u.intent = Intent::Plain,
        _ => {}
    }
    let u = &state.units[&id];
    let attack = stats
        .unit(u.owner, u.kind)
        .attack
        .expect("only armed units acquire");
    let holding = u.intent == Intent::Plain && ring.holds(&state.map, u);
    // The current target, while it is valid and in weapon range or
    // directly chaseable (so not beyond the reach); only in weapon range
    // while holding a ring.
    let kept = u.target.and_then(|t| {
        let aim = locate(state, rules, u.owner, t)?;
        let in_range = within(u.pos, u.radius, attack.range, &aim);
        (in_range || (!holding && chases_directly(&state.map, u.pos, u.radius, attack.reach, &aim)))
            .then_some((t, aim, in_range))
    });
    let max_want = if holding {
        Want::InRange
    } else {
        Want::InReach
    };
    // Only a candidate that ranks strictly better on (tier, out of weapon
    // range) displaces it; nothing ranks better than an in-range target of
    // the better tier, so then nothing is searched.
    let limit = kept.map_or((2, false), |(t, _, in_range)| {
        (
            stats.tier(u.kind, matches!(t, TargetId::Building(_))),
            !in_range,
        )
    });
    let ties = if limit > (0, false) {
        best_candidates(
            state, grid, stats, targetable, u, &attack, limit, max_want, scratch,
        )
    } else {
        Vec::new()
    };
    let chosen = match ties.len() {
        0 => kept,
        n => {
            let i = if n == 1 {
                0
            } else {
                let n = u32::try_from(n).expect("candidates fit u32");
                usize::try_from(rng_index(&mut state.rng, n)).expect("fits")
            };
            let c = ties[i];
            Some((c.id, c.aim, !c.key.1))
        }
    };
    let u = state.units.get_mut(&id).expect("unit exists");
    u.target = chosen.map(|(t, _, _)| t);
    if let Some((_, aim, in_range)) = chosen {
        let point = aim.point(u.pos);
        let engage = if in_range {
            Engage::Hold { face: point }
        } else {
            Engage::Chase(point)
        };
        plan.engage.insert(id, engage);
    }
}

/// 5a for the shooters: every complete building with an `attack` and hit
/// points left whose cooldown has run out, in `BuildingId` order, aims at
/// the nearest enemy unit in range of its footprint (exact ties drawn).
fn think_shooters(
    state: &mut State,
    rules: &Rules,
    grid: &SpatialGrid,
    stats: &Stats<'_>,
    tick: u32,
    plan: &mut Plan,
) {
    let ready: Vec<(BuildingId, PlayerId, Rect, ShooterStats)> = state
        .buildings
        .values()
        .filter(|b| b.construction == Construction::Complete && b.hp > 0 && tick >= b.ready_at)
        .filter_map(|b| {
            let shot = stats.shooter(b.kind)?;
            Some((b.id, b.owner, building_rect(rules, &state.map, b), shot))
        })
        .collect();
    let mut nearest: Vec<UnitId> = Vec::new();
    for (id, owner, rect, shot) in ready {
        let (w, h) = (rect.x1 - rect.x0 + 1, rect.y1 - rect.y0 + 1);
        let centre = FxVec2::new(
            Fx::from_ratio(i32::from(rect.x0) + i32::from(rect.x1) + 1, 2),
            Fx::from_ratio(i32::from(rect.y0) + i32::from(rect.y1) + 1, 2),
        );
        let half = Fx::from_ratio(i32::from(w.max(h)), 2);
        let mut best = i64::MAX;
        nearest.clear();
        grid.for_each_in(centre, half + shot.range + stats.max_radius(), |uid| {
            let Some(u) = state.units.get(&uid).filter(|u| u.owner != owner) else {
                return;
            };
            let d = dist_sq_to_rect(u.pos, rect);
            if d > reach_sq(shot.range + u.radius) || d > best {
                return;
            }
            if d < best {
                best = d;
                nearest.clear();
            }
            nearest.push(uid);
        });
        let aim = match nearest.len() {
            0 => continue,
            1 => nearest[0],
            n => {
                nearest.sort_unstable();
                let n = u32::try_from(n).expect("units fit u32");
                nearest[usize::try_from(rng_index(&mut state.rng, n)).expect("fits")]
            }
        };
        plan.aims.push((id, aim));
    }
}

/// Step 5c: collect every hit from the post-movement positions, units in
/// `UnitId` order then the shooters' `aims` in `BuildingId` order, apply
/// them summed per target, then `reap` (see the module docs).
pub(crate) fn resolve(
    state: &mut State,
    rules: &Rules,
    stats: &Stats<'_>,
    aims: &[(BuildingId, UnitId)],
    tick: u32,
    events: &mut Vec<SimEvent>,
) {
    // (attacker, target, floor(raw * counter / 1000), cooldown).
    let mut hits: Vec<(TargetId, TargetId, i64, u32)> = Vec::new();
    for u in state.units.values() {
        let Some(t) = u.target else { continue };
        if tick < u.ready_at {
            continue;
        }
        let Some(attack) = stats.unit(u.owner, u.kind).attack else {
            continue;
        };
        let Some(aim) = locate(state, rules, u.owner, t) else {
            continue;
        };
        if !within(u.pos, u.radius, attack.range, &aim) {
            continue;
        }
        let counter = match aim {
            Aim::Unit { kind: defender, .. } => stats.counter(u.kind, defender),
            Aim::Building { .. } => stats.vs_buildings(u.kind),
        };
        let damage = i64::from(attack.damage)
            .checked_mul(counter)
            .expect("damage times counter fits i64")
            / 1000;
        hits.push((TargetId::Unit(u.id), t, damage, attack.cooldown_ticks));
    }
    for &(id, unit) in aims {
        let b = &state.buildings[&id];
        let shot = stats.shooter(b.kind).expect("aimed by a shooter");
        let u = &state.units[&unit];
        if dist_sq_to_rect(u.pos, building_rect(rules, &state.map, b))
            <= reach_sq(shot.range + u.radius)
        {
            hits.push((
                TargetId::Building(id),
                TargetId::Unit(unit),
                shot.damage,
                shot.cooldown_ticks,
            ));
        }
    }
    if hits.is_empty() {
        return reap(state, rules, stats, events);
    }
    let mut dealt: BTreeMap<TargetId, i64> = BTreeMap::new();
    for (attacker, target, base, cooldown) in hits {
        let variance = if stats.variance() == 0 {
            1000
        } else {
            let span = u32::try_from(2 * stats.variance() + 1).expect("variance <= 500");
            1000 + i64::from(rng_index(&mut state.rng, span)) - stats.variance()
        };
        let damage = base * variance / 1000;
        let ready_at = tick.checked_add(cooldown).expect("tick fits u32");
        match attacker {
            TargetId::Unit(id) => state.units.get_mut(&id).expect("collected").ready_at = ready_at,
            TargetId::Building(id) => {
                state.buildings.get_mut(&id).expect("collected").ready_at = ready_at;
            }
        }
        events.push(SimEvent::Hit {
            attacker,
            target,
            damage: i32::try_from(damage).expect("data-check bounds a hit"),
        });
        let sum = dealt.entry(target).or_insert(0);
        *sum = sum.checked_add(damage).expect("damage sum fits i64");
    }
    for (target, damage) in dealt {
        apply_damage(state, target, damage, events);
    }
    reap(state, rules, stats, events);
}

/// `v` clamped into `i32`.
pub(crate) fn clamp_i32(v: i64) -> i32 {
    i32::try_from(v.clamp(i64::from(i32::MIN), i64::from(i32::MAX))).expect("clamped")
}

/// Deal `amount` damage to an existing `target` at once: a unit's
/// `damage_taken` grows (clamped to `i32`); a complete Town's hit points
/// fall and stay at 0, with `TownAtZeroHp` on the step that reaches 0; any
/// other building or site loses `amount` hit points (clamped to `i32`).
/// Nothing is removed here: `reap` (step 5c) removes the dead and the
/// destroyed. `resolve` applies each target's summed hits through it, and
/// `Command::DebugDamage` its amount.
///
/// # Panics
///
/// If `target` does not exist.
pub(crate) fn apply_damage(
    state: &mut State,
    target: TargetId,
    amount: i64,
    events: &mut Vec<SimEvent>,
) {
    match target {
        TargetId::Unit(id) => {
            let u = state.units.get_mut(&id).expect("an existing target");
            u.damage_taken = clamp_i32(i64::from(u.damage_taken).saturating_add(amount));
        }
        TargetId::Building(id) => {
            let b = state.buildings.get_mut(&id).expect("an existing target");
            let hp = i64::from(b.hp).saturating_sub(amount);
            if b.town_state.is_some() {
                // A complete Town is never destroyed: it stays at 0.
                let before = b.hp;
                b.hp = clamp_i32(hp.max(0));
                if before > 0 && b.hp == 0 {
                    events.push(SimEvent::TownAtZeroHp { town: id });
                }
            } else {
                b.hp = clamp_i32(hp);
            }
        }
    }
}

/// Remove every unit whose damage has reached its hit points, in `UnitId`
/// order (its path request is cancelled; its job, slot and pop go with
/// it), then destroy every building or site at 0 HP or below other than a
/// complete Town, in `BuildingId` order: no refund, its queue is lost, its
/// border source is removed, and every destroyed footprint is unblocked by
/// one `set_blocked_many` (one cost-grid generation bump). Each Town a
/// destroyed building was attributed to is then re-derived, in id order,
/// and may fall back to tier 1. Emits `UnitDied`, `BuildingDestroyed` and
/// one `TerritoryChanged` when a border source went.
pub(crate) fn reap(
    state: &mut State,
    rules: &Rules,
    stats: &Stats<'_>,
    events: &mut Vec<SimEvent>,
) {
    let dead: Vec<UnitId> = state
        .units
        .values()
        .filter(|u| u.damage_taken > 0 && u.damage_taken >= stats.unit(u.owner, u.kind).max_hp)
        .map(|u| u.id)
        .collect();
    for id in dead {
        let u = state.units.remove(&id).expect("collected");
        state.pathing.cancel(id);
        events.push(SimEvent::UnitDied {
            unit: id,
            owner: u.owner,
            kind: u.kind,
        });
    }
    let wrecked: Vec<BuildingId> = state
        .buildings
        .values()
        .filter(|b| b.hp <= 0 && b.town_state.is_none())
        .map(|b| b.id)
        .collect();
    if wrecked.is_empty() {
        return;
    }
    let mut tiles = Vec::new();
    let mut source_removed = false;
    let mut towns = BTreeSet::new();
    for id in wrecked {
        let removed = construction::remove_building(state, rules, id);
        tiles.extend(removed.tiles);
        source_removed |= removed.source;
        towns.extend(removed.building.town);
        events.push(SimEvent::BuildingDestroyed {
            building: id,
            owner: removed.building.owner,
            kind: removed.building.kind,
        });
    }
    state.map.set_blocked_many(&tiles, false);
    if source_removed {
        events.push(SimEvent::TerritoryChanged);
    }
    for town in towns {
        town::refresh(state, rules, town, events);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: u16, y0: u16, x1: u16, y1: u16) -> Rect {
        Rect { x0, y0, x1, y1 }
    }

    #[test]
    fn distance_to_a_footprint_is_to_its_nearest_point() {
        let r = rect(4, 4, 6, 5);
        // Inside the area (tiles 4..=6 cover x in [4, 7]): distance 0.
        assert_eq!(dist_sq_to_rect(FxVec2::from_ints(5, 5), r), 0);
        assert_eq!(dist_sq_to_rect(FxVec2::from_ints(7, 6), r), 0);
        // West of it by 1.5 tiles on a row it spans.
        let p = FxVec2::new(Fx::from_ratio(5, 2), Fx::from_int(5));
        assert_eq!(dist_sq_to_rect(p, r), reach_sq(Fx::from_ratio(3, 2)));
        assert_eq!(nearest_point(p, r), FxVec2::from_ints(4, 5));
        // Off the south-east corner (7, 6) by (3, 4): distance 5.
        assert_eq!(
            dist_sq_to_rect(FxVec2::from_ints(10, 10), r),
            reach_sq(Fx::from_int(5))
        );
    }

    /// `segment_clear` between the centres of tiles `a` and `b`.
    fn centres_clear(map: &Map, a: (u16, u16), b: (u16, u16), ignore: Option<Rect>) -> bool {
        let (a, b) = (Tile::new(a.0, a.1), Tile::new(b.0, b.1));
        segment_clear(map, map.center_of(a), map.center_of(b), ignore)
    }

    #[test]
    fn segments_between_tile_centres_respect_blocked_tiles_and_corners() {
        let mut map = Map::open(10, 10);
        assert!(centres_clear(&map, (0, 0), (9, 3), None));
        map.set_blocked(Tile::new(4, 2), true);
        // Row 2 from x 0 to 9 crosses (4, 2).
        assert!(!centres_clear(&map, (0, 2), (9, 2), None));
        // A blocked target footprint does not block its own segment.
        let footprint = rect(4, 2, 4, 2);
        assert!(centres_clear(&map, (0, 2), (4, 2), Some(footprint)));
        assert!(!centres_clear(&map, (0, 2), (9, 2), Some(rect(5, 2, 5, 2))));
        // An exact diagonal through a corner needs both tiles beside it.
        let mut d = Map::open(4, 4);
        assert!(centres_clear(&d, (0, 0), (3, 3), None));
        d.set_blocked(Tile::new(1, 0), true);
        assert!(!centres_clear(&d, (0, 0), (3, 3), None));
        assert!(!centres_clear(&d, (3, 3), (0, 0), None));
        // A steep segment passes tiles a diagonal-only walk would skip.
        let mut s = Map::open(4, 8);
        s.set_blocked(Tile::new(1, 3), true);
        assert!(!centres_clear(&s, (0, 0), (2, 7), None));
        assert!(centres_clear(&s, (0, 0), (0, 7), None));
        assert!(centres_clear(&s, (2, 2), (2, 2), None));
    }

    #[test]
    fn segments_follow_the_straight_walk_not_the_tile_centres() {
        let mut map = Map::open(12, 12);
        for (x, y) in [(6, 4), (7, 4), (6, 5), (7, 5)] {
            map.set_blocked(Tile::new(x, y), true);
        }
        let target = map.center_of(Tile::new(6, 6));
        // From (9, 5)'s centre the segment crosses (7, 5)'s corner exactly:
        // blocked.
        assert!(!segment_clear(
            &map,
            map.center_of(Tile::new(9, 5)),
            target,
            None
        ));
        // A chaser on (9, 5) that came straight from (11, 5)'s centre
        // passes south of it.
        let here = FxVec2::new(Fx::from_ratio(999, 100), Fx::from_ratio(580, 100));
        assert!(segment_clear(&map, here, target, None));
        let mut d = Map::open(4, 4);
        d.set_blocked(Tile::new(1, 0), true);
        assert!(!segment_clear(
            &d,
            d.center_of(Tile::new(0, 0)),
            d.center_of(Tile::new(3, 3)),
            None
        ));
    }

    #[test]
    fn rng_index_spans_its_range() {
        use rand_core::SeedableRng;
        let mut rng = rand_pcg::Pcg32::seed_from_u64(7);
        let mut seen = [0u32; 5];
        for _ in 0..1000 {
            seen[usize::try_from(rng_index(&mut rng, 5)).unwrap()] += 1;
        }
        assert!(seen.iter().all(|&n| n > 150), "{seen:?}");
        assert_eq!(rng_index(&mut rng, 1), 0);
    }
}
