//! Influence maps (M6): per-plan integer grids of army strength, computed
//! in `crates/ai` from the view and never stored.
//!
//! M6 settles where the maps live (docs/design/ai.md, "Influence maps and
//! counter-weighted composition"; docs/DETERMINISM.md): not as an unhashed
//! sim cache refreshed every 10 ticks (which a restore between two
//! refreshes could not reproduce) but as bot state of one plan, built from
//! the view the first time a plan asks for them (`Ctx::influence`). So
//! they are not hashed, need no restore, and a bot rebuilt mid-match reads
//! the same grids. Retreat, the siege target score and the raid target
//! read them; enemy units come through `facts::visible_enemies`, the seam
//! M7's fog filters.
//!
//! The map is cut into `cell` x `cell`-tile cells (`ceil(map / cell)` per
//! side, 32 x 32 on `plains_1v1` at 4 tiles). Each [`Grid`] holds, per
//! cell, the summed current hit points and the summed damage per second
//! (times ten) of what stands in it:
//!
//! - `own`: the planning player's armed units;
//! - `enemy_units`: every other player's armed units;
//! - `enemy_shooters`: every other player's complete buildings with an
//!   attack (Towns and Watchtowers) that still have hit points.
//!
//! An armed unit is one whose kind attacks and is not a civilian, so
//! Yeomen, Scribes and Supply Wains add nothing. A unit adds its current
//! hit points (Arms rows included) and `dps_x10 = 100 x damage /
//! cooldown_ds` with its resolved damage, read once per `(owner, kind)`
//! through a [`StatsCache`]; a building adds its hit points and its
//! `buildings.ron` attack (no Modifier row touches buildings).
//!
//! [`Influence::strength`] is the Lanchester estimate of a block of cells,
//! `sum(hp) x sum(dps_x10)` in `i64`, and [`isqrt`] turns it into an army
//! scale where a score needs one. Everything is integer arithmetic, in
//! id order, with no randomness.

use rules::{Rules, UnitClass};
use sim::{Construction, FxVec2, PlayerId, SimView, UnitKindId, UnitStats};
use std::collections::BTreeMap;

/// One layer of an [`Influence`]: per cell, row-major, the summed hit
/// points and the summed damage per second times ten.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grid {
    /// Summed current hit points per cell.
    pub hp: Vec<i32>,
    /// Summed damage per second, times ten, per cell.
    pub dps_x10: Vec<i32>,
}

impl Grid {
    fn new(cells: usize) -> Grid {
        Grid {
            hp: vec![0; cells],
            dps_x10: vec![0; cells],
        }
    }

    fn add(&mut self, cell: usize, hp: i32, dps_x10: i32) {
        if let (Some(h), Some(d)) = (self.hp.get_mut(cell), self.dps_x10.get_mut(cell)) {
            *h = h.saturating_add(hp.max(0));
            *d = d.saturating_add(dps_x10.max(0));
        }
    }

    /// Summed hit points over every cell.
    pub fn total_hp(&self) -> i64 {
        self.hp.iter().map(|v| i64::from(*v)).sum()
    }

    /// Summed damage per second times ten over every cell.
    pub fn total_dps_x10(&self) -> i64 {
        self.dps_x10.iter().map(|v| i64::from(*v)).sum()
    }
}

/// The combat stats of `(owner, kind)` pairs, resolved through the view's
/// Modifier rows once per plan (a plan reads at most a few kinds per
/// player, so the cache stays small).
#[derive(Debug, Clone, Default)]
pub struct StatsCache {
    stats: BTreeMap<(PlayerId, UnitKindId), UnitStats>,
    resolved: u32,
}

impl StatsCache {
    /// An empty cache.
    pub fn new() -> StatsCache {
        StatsCache::default()
    }

    /// `owner`'s stats for `kind`, resolved on the first ask of the pair.
    pub fn get(&mut self, view: &SimView<'_>, owner: PlayerId, kind: UnitKindId) -> UnitStats {
        if let Some(s) = self.stats.get(&(owner, kind)) {
            return *s;
        }
        let s = view.unit_stats(owner, kind);
        self.resolved = self.resolved.saturating_add(1);
        self.stats.insert((owner, kind), s);
        s
    }

    /// How many `(owner, kind)` pairs were resolved (each once).
    pub fn resolved(&self) -> u32 {
        self.resolved
    }
}

/// Damage per second times ten of `damage` every `cooldown_ds`
/// deciseconds: `100 x damage / cooldown_ds`, 0 for a zero cooldown.
pub fn dps_x10(damage: i32, cooldown_ds: u32) -> i32 {
    if cooldown_ds == 0 {
        return 0;
    }
    let v = i64::from(damage.max(0)) * 100 / i64::from(cooldown_ds);
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// The integer square root, `floor(sqrt(n))`.
pub fn isqrt(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    // Newton's method from an over-estimate; it decreases monotonically
    // to the floor root.
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// One plan's influence maps (see the module doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Influence {
    /// Cells per row.
    pub w: u32,
    /// Cells per column.
    pub h: u32,
    /// Cell side in tiles.
    pub cell: u32,
    /// The planning player's armed units.
    pub own: Grid,
    /// Every other player's armed units.
    pub enemy_units: Grid,
    /// Every other player's complete shooting buildings with hit points.
    pub enemy_shooters: Grid,
}

impl Influence {
    /// Build the maps for `me` from `view` with `cell_tiles`-tile cells (at
    /// least 1): one pass over the units and one over the buildings, unit
    /// stats through `stats`.
    pub fn build(
        view: &SimView<'_>,
        me: PlayerId,
        cell_tiles: u32,
        stats: &mut StatsCache,
    ) -> Influence {
        let rules = view.rules();
        let cell = cell_tiles.max(1);
        let map = view.map();
        let w = u32::from(map.width()).div_ceil(cell).max(1);
        let h = u32::from(map.height()).div_ceil(cell).max(1);
        let cells = usize::try_from(w * h).expect("a few thousand cells");
        let mut out = Influence {
            w,
            h,
            cell,
            own: Grid::new(cells),
            enemy_units: Grid::new(cells),
            enemy_shooters: Grid::new(cells),
        };
        let armed = armed_kinds(rules);
        // Own units, then the enemy's through the fog seam.
        let own = view.units().filter(|u| u.owner == me);
        for u in own.chain(super::facts::visible_enemies(view, me)) {
            if !armed.get(usize::from(u.kind.0)).copied().unwrap_or(false) {
                continue;
            }
            let s = stats.get(view, u.owner, u.kind);
            let hp = s.max_hp.saturating_sub(u.damage_taken);
            if hp <= 0 {
                continue;
            }
            let cooldown_ds = rules
                .unit_kind(u.kind.0)
                .and_then(|k| k.attack.as_ref())
                .map_or(0, |a| a.cooldown_ds);
            let dps = s.attack.map_or(0, |a| dps_x10(a.damage, cooldown_ds));
            let at = out.cell_index(out.cell_at(u.pos));
            if u.owner == me {
                out.own.add(at, hp, dps);
            } else {
                out.enemy_units.add(at, hp, dps);
            }
        }
        for b in view.buildings() {
            if b.owner == me || b.construction != Construction::Complete || b.hp <= 0 {
                continue;
            }
            let Some(kind) = rules.building_kind(b.kind.0) else {
                continue;
            };
            let Some(attack) = kind.attack.as_ref() else {
                continue;
            };
            let damage = i32::try_from(attack.damage).unwrap_or(i32::MAX);
            let centre = sim::construction::centre_fx(kind, b.tile);
            let at = out.cell_index(out.cell_at(centre));
            out.enemy_shooters
                .add(at, b.hp, dps_x10(damage, attack.cooldown_ds));
        }
        out
    }

    /// The cell `(cx, cy)` holding world position `pos`, clamped to the
    /// grid.
    pub fn cell_at(&self, pos: FxVec2) -> (u32, u32) {
        let clamp = |v: i32, n: u32| -> u32 {
            let v = u32::try_from(v.max(0)).unwrap_or(0) / self.cell;
            v.min(n.saturating_sub(1))
        };
        (
            clamp(pos.x.floor_to_int(), self.w),
            clamp(pos.y.floor_to_int(), self.h),
        )
    }

    /// The row-major index of cell `(cx, cy)`.
    pub fn cell_index(&self, (cx, cy): (u32, u32)) -> usize {
        usize::try_from(cy * self.w + cx).expect("a few thousand cells")
    }

    /// The Lanchester strength of `grid` over the `(2r + 1)`-cell square
    /// block centred on `cell` (clipped to the map): `sum(hp) x
    /// sum(dps_x10)`.
    pub fn strength(&self, grid: &Grid, (cx, cy): (u32, u32), r: u32) -> i64 {
        let (mut hp, mut dps) = (0i64, 0i64);
        let x0 = cx.saturating_sub(r);
        let y0 = cy.saturating_sub(r);
        let x1 = cx.saturating_add(r).min(self.w.saturating_sub(1));
        let y1 = cy.saturating_add(r).min(self.h.saturating_sub(1));
        for y in y0..=y1 {
            for x in x0..=x1 {
                let i = self.cell_index((x, y));
                hp += i64::from(grid.hp.get(i).copied().unwrap_or(0));
                dps += i64::from(grid.dps_x10.get(i).copied().unwrap_or(0));
            }
        }
        hp.saturating_mul(dps)
    }
}

/// Per unit kind: armed (it attacks) and not a civilian.
fn armed_kinds(rules: &Rules) -> Vec<bool> {
    rules
        .units
        .iter()
        .map(|u| u.attack.is_some() && u.class != UnitClass::Civilian)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{
        Arena, P0, P1, building_kind, place, research, spawn, standard_vs_passive, unit_kind,
    };
    use sim::{BuildingId, Command, TargetId};

    fn rules() -> Rules {
        crate::testkit::rules()
    }

    #[test]
    fn isqrt_and_dps_are_exact() {
        for (n, r) in [
            (0, 0),
            (1, 1),
            (3, 1),
            (4, 2),
            (99, 9),
            (100, 10),
            (u64::MAX, 4_294_967_295),
        ] {
            assert_eq!(isqrt(n), r, "{n}");
        }
        assert_eq!(dps_x10(12, 15), 80, "12 every 1.5 s is 8.0 a second");
        assert_eq!(dps_x10(10, 0), 0);
        assert_eq!(dps_x10(-5, 10), 0);
    }

    /// M6: a unit adds its current hit points and its damage per second
    /// times ten to its own cell, own and enemy layers apart; Arms I raises
    /// both by its 1100 per mille rows, and the cache resolves each
    /// `(owner, kind)` once.
    #[test]
    fn influence_sums_hp_and_dps_per_cell_with_arms_upgrades() {
        let r = rules();
        let skirmisher = unit_kind(&r, "skirmisher");
        let def = r.unit_kind(skirmisher.0).unwrap();
        let attack = def.attack.as_ref().unwrap();
        let base_dps = dps_x10(i32::try_from(attack.damage).unwrap(), attack.cooldown_ds);
        let base_hp = i32::try_from(def.hp).unwrap();
        let a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                // Two own Skirmishers in the cell of tiles 40..44 x 60..64,
                // one enemy Skirmisher with Arms I four cells east.
                spawn(&r, P0, "skirmisher", 41, 61, 1),
                spawn(&r, P0, "skirmisher", 42, 62, 1),
                research(&r, P1, "arms_1"),
                spawn(&r, P1, "skirmisher", 57, 61, 1),
            ],
        );
        let view = a.sim.view();
        let mut cache = StatsCache::new();
        let inf = Influence::build(&view, P0, 4, &mut cache);
        assert_eq!((inf.w, inf.h, inf.cell), (32, 32, 4));
        let own = inf.cell_index((10, 15));
        assert_eq!(inf.own.hp[own], 2 * base_hp);
        assert_eq!(inf.own.dps_x10[own], 2 * base_dps);
        let enemy = inf.cell_index((14, 15));
        let upgraded = view.unit_stats(P1, skirmisher);
        let arms_hp = upgraded.max_hp;
        assert!(arms_hp > base_hp, "Arms I hit points: {arms_hp}");
        assert_eq!(inf.enemy_units.hp[enemy], arms_hp);
        assert!(inf.enemy_units.dps_x10[enemy] > base_dps, "Arms I damage");
        assert_eq!(
            inf.enemy_units.dps_x10[enemy],
            dps_x10(upgraded.attack.unwrap().damage, attack.cooldown_ds)
        );
        assert_eq!(inf.own.total_hp(), i64::from(2 * base_hp), "nothing else");
        assert_eq!(cache.resolved(), 2, "(P0, skirmisher) and (P1, skirmisher)");
        // The Lanchester block: one cell, then a block reaching the enemy.
        assert_eq!(
            inf.strength(&inf.own, (10, 15), 0),
            i64::from(2 * base_hp) * i64::from(2 * base_dps)
        );
        assert_eq!(inf.strength(&inf.enemy_units, (10, 15), 3), 0);
        assert_eq!(
            inf.strength(&inf.enemy_units, (10, 15), 4),
            i64::from(arms_hp) * i64::from(inf.enemy_units.dps_x10[enemy])
        );
        // A second build with the same cache resolves nothing new.
        let again = Influence::build(&view, P0, 4, &mut cache);
        assert_eq!(again, inf);
        assert_eq!(cache.resolved(), 2);
    }

    /// M6: Yeomen, Supply Wains and a dead unit add nothing; a wounded unit
    /// adds what it has left.
    #[test]
    fn influence_ignores_civilians_wains_and_the_dead() {
        let r = rules();
        let skirmisher = unit_kind(&r, "skirmisher");
        let full = i32::try_from(r.unit_kind(skirmisher.0).unwrap().hp).unwrap();
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                spawn(&r, P1, "supply_wain", 70, 70, 2),
                spawn(&r, P1, "skirmisher", 90, 90, 1),
                spawn(&r, P1, "skirmisher", 100, 100, 1),
            ],
        );
        let skirmishers = a.units_of(P1, "skirmisher");
        let (dead, hurt) = (skirmishers[0], skirmishers[1]);
        a.issue(
            P0,
            vec![
                Command::DebugDamage {
                    target: TargetId::Unit(dead),
                    amount: u32::MAX,
                },
                Command::DebugDamage {
                    target: TargetId::Unit(hurt),
                    amount: 5,
                },
            ],
        );
        let view = a.sim.view();
        assert!(view.unit(dead).is_none(), "the dead are gone");
        let inf = Influence::build(&view, P0, 4, &mut StatsCache::new());
        assert_eq!(inf.enemy_units.total_hp(), i64::from(full - 5));
        assert_eq!(inf.own.total_hp(), 0, "the start Yeomen are civilians");
        let wains = a.units_of(P1, "supply_wain");
        assert_eq!(wains.len(), 2);
        for w in wains {
            let at = inf.cell_index(inf.cell_at(view.unit(w).unwrap().pos));
            assert_eq!(inf.enemy_units.hp[at], 0, "a Wain adds nothing");
        }
    }

    /// M6: the shooter layer holds every complete enemy Town and
    /// Watchtower with hit points, at its footprint centre, with its
    /// `buildings.ron` attack; own shooters and sites add nothing.
    #[test]
    fn influence_shooter_layer_counts_towns_and_watchtowers() {
        let r = rules();
        let tower = building_kind(&r, "watchtower");
        let town = building_kind(&r, "town");
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                place(&r, P1, "watchtower", 80, 60),
                place(&r, P0, "watchtower", 40, 60),
                Command::DebugPlaceBuilding {
                    owner: P1,
                    kind: tower,
                    tile: sim::Tile::new(80, 40),
                    complete: false,
                },
            ],
        );
        let view = a.sim.view();
        let inf = Influence::build(&view, P0, 4, &mut StatsCache::new());
        let shot = |kind: sim::BuildingKindId| {
            let k = r.building_kind(kind.0).unwrap();
            let at = k.attack.as_ref().unwrap();
            (
                i32::try_from(k.hp).unwrap(),
                dps_x10(i32::try_from(at.damage).unwrap(), at.cooldown_ds),
            )
        };
        let (tower_hp, tower_dps) = shot(tower);
        let (town_hp, town_dps) = shot(town);
        assert_eq!(
            inf.enemy_shooters.total_hp(),
            i64::from(tower_hp + town_hp),
            "P1's Seat and its complete Watchtower only"
        );
        assert_eq!(
            inf.enemy_shooters.total_dps_x10(),
            i64::from(tower_dps + town_dps)
        );
        let at = |b: BuildingId| {
            let b = view.building(b).unwrap();
            let k = r.building_kind(b.kind.0).unwrap();
            inf.cell_index(inf.cell_at(sim::construction::centre_fx(k, b.tile)))
        };
        let towers = a.buildings_of(P1, "watchtower");
        assert_eq!(inf.enemy_shooters.hp[at(towers[0])], tower_hp);
        assert_eq!(inf.enemy_shooters.hp[at(a.seat(P1))], town_hp);
        assert_eq!(inf.own.total_hp(), 0, "own buildings are not units");
        // A Town at 0 HP (annexable) no longer shoots.
        let seat = a.seat(P1);
        a.issue(
            P0,
            vec![Command::DebugDamage {
                target: TargetId::Building(seat),
                amount: u32::MAX,
            }],
        );
        let view = a.sim.view();
        let inf = Influence::build(&view, P0, 4, &mut StatsCache::new());
        assert_eq!(inf.enemy_shooters.total_hp(), i64::from(tower_hp));
    }
}
