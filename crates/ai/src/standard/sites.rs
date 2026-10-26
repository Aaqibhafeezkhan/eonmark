//! [`MapAnalysis`]: the Standard bot's static site tables, built once per
//! seat from immutable inputs (the map's terrain and starts, the rules and
//! the personality), never from the live view. A controller rebuilt
//! mid-match therefore holds exactly the same tables, which keeps the bot
//! restore-exact.
//!
//! [`site_for`] is the per-plan site choice: it walks a policy's
//! candidates and filters them against the live view, cheapest test
//! first: every footprint tile own land and passable, a free passable ring
//! of `building_gap_tiles` around the footprint (for `Terrain`, only the
//! ring's grass tiles must be free), no unit on the footprint, the
//! policy's spacing, and only then `check_build` (a full-map search), at
//! most `site_checks_per_think` per plan.

use super::ctx::Ctx;
use super::facts::{kind_centre, kind_centre_tile, tiles_sq};
use super::memory::BotMemory;
use rules::{MapDef, Personality, Placement, Rules, Slots, Terrain};
use sim::{BuildingId, BuildingKindId, MatchSetup, PlayerId, Tile, UnitId};
use std::collections::BTreeMap;

/// Expansion candidates kept, best first.
pub const EXPANSION_CANDIDATES: usize = 64;

/// The static site tables of one seat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapAnalysis {
    /// The seat.
    pub me: PlayerId,
    /// The seat's map start (its Seat's centre).
    pub own_start: Tile,
    /// The nearest other player's map start (the enemy Seat's centre,
    /// known from the map, not scouted; docs/design/ai.md).
    pub enemy_start: Tile,
    /// `NearTown` offsets from a Town's centre to a footprint's north-west
    /// tile: Chebyshev rings `near_town_min_tiles..=near_town_max_tiles`,
    /// inner ring first, each ring clockwise from its north-west corner.
    pub near_town_offsets: Vec<(i32, i32)>,
    /// `Terrain` candidates per building kind with `Adjacent` slots:
    /// footprint north-west tiles on grass within `border_scan_tiles` of
    /// the own start with at least one adjacent tile of the kind's terrain,
    /// best first: most adjacent tiles (up to the kind's `max`), then
    /// nearest the own start, then the lowest tile index.
    pub terrain: BTreeMap<BuildingKindId, Vec<Tile>>,
    /// `Expansion` candidates: Town footprint north-west tiles on grass
    /// lying wholly inside the Seat's first border (every footprint tile
    /// closer than `town_radius_tiles` to the own start, so it is own land
    /// from the start) with the centre at least `expansion_min_tiles` from
    /// the own start, best first by score, then the lowest tile index; at
    /// most [`EXPANSION_CANDIDATES`]. The score is the Forest and Mountain
    /// tiles closer than `resource_radius_tiles` to the centre that lie
    /// outside the Seat's first border (what the new Town's border would
    /// bring in), less `toward_enemy_penalty` per tile the centre is closer
    /// to the enemy start than the own start is.
    pub expansion: Vec<Tile>,
    /// `BorderEdge` candidates: grass tiles within `border_scan_tiles` of
    /// the own start, nearest the enemy start first, then the lowest tile
    /// index.
    pub border_edge: Vec<Tile>,
    /// The rally seed: `rally_tiles_from_seat` from the own start toward
    /// the enemy start (the rally is the first passable owned tile of the
    /// spiral around it, chosen at plan time).
    pub rally_seed: Tile,
}

fn d2(a: Tile, b: Tile) -> i64 {
    let dx = i64::from(a.x) - i64::from(b.x);
    let dy = i64::from(a.y) - i64::from(b.y);
    dx * dx + dy * dy
}

fn index(def: &MapDef, t: Tile) -> u32 {
    u32::from(t.y) * u32::from(def.width) + u32::from(t.x)
}

/// The tile `(x, y)` when it is on the map.
fn tile_at(def: &MapDef, x: i32, y: i32) -> Option<Tile> {
    let x = u16::try_from(x).ok()?;
    let y = u16::try_from(y).ok()?;
    (x < def.width && y < def.height).then_some(Tile::new(x, y))
}

/// Whether every tile of the `w` x `h` footprint at `nw` is on the map and
/// grass.
fn footprint_on_grass(def: &MapDef, nw: Tile, w: u16, h: u16) -> bool {
    (0..h).all(|dy| {
        (0..w).all(|dx| {
            tile_at(
                def,
                i32::from(nw.x) + i32::from(dx),
                i32::from(nw.y) + i32::from(dy),
            )
            .is_some_and(|t| def.tile(t.x, t.y) == Terrain::Grass)
        })
    })
}

/// Tiles of `terrain` sharing an edge or a corner with the footprint.
fn adjacent_count(def: &MapDef, nw: Tile, w: u16, h: u16, terrain: Terrain) -> u32 {
    let (x0, y0) = (i32::from(nw.x) - 1, i32::from(nw.y) - 1);
    let (x1, y1) = (
        i32::from(nw.x) + i32::from(w),
        i32::from(nw.y) + i32::from(h),
    );
    let mut n = 0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let ring = x == x0 || x == x1 || y == y0 || y == y1;
            if ring && tile_at(def, x, y).is_some_and(|t| def.tile(t.x, t.y) == terrain) {
                n += 1;
            }
        }
    }
    n
}

impl MapAnalysis {
    /// Build the tables for seat `me` of `setup` under `rules` and the
    /// personality `knobs`.
    ///
    /// # Panics
    ///
    /// If the setup's map is not in the rules or has no start for `me`
    /// (`Sim::new` and `Rules::load` rule both out).
    pub fn new(
        rules: &Rules,
        setup: &MatchSetup,
        me: PlayerId,
        knobs: &Personality,
    ) -> MapAnalysis {
        let def = rules
            .map(&setup.map)
            .expect("the setup's map is in the rules");
        let start = |p: usize| {
            let s = def.starts[p];
            Tile::new(s.x, s.y)
        };
        let own_start = start(usize::from(me.0));
        let enemy_start = setup
            .players
            .iter()
            .filter(|p| p.id != me)
            .map(|p| start(usize::from(p.id.0)))
            .min_by_key(|t| (d2(*t, own_start), index(def, *t)))
            .unwrap_or(own_start);

        let mut near_town_offsets = Vec::new();
        let min = i32::try_from(knobs.near_town_min_tiles).expect("validated <= 64");
        let max = i32::try_from(knobs.near_town_max_tiles).expect("validated <= 64");
        for r in min..=max {
            for x in -r..r {
                near_town_offsets.push((x, -r));
            }
            for y in -r..r {
                near_town_offsets.push((r, y));
            }
            for x in (-r + 1..=r).rev() {
                near_town_offsets.push((x, r));
            }
            for y in (-r + 1..=r).rev() {
                near_town_offsets.push((-r, y));
            }
        }

        let scan = i64::from(knobs.border_scan_tiles);
        let tiles: Vec<Tile> = (0..def.height)
            .flat_map(|y| (0..def.width).map(move |x| Tile::new(x, y)))
            .collect();

        let mut terrain = BTreeMap::new();
        for (i, kind) in rules.buildings.iter().enumerate() {
            let Some(Slots::Adjacent { terrain: t, max }) = kind.gather.as_ref().map(|g| &g.slots)
            else {
                continue;
            };
            let w = u16::try_from(kind.width_tiles).expect("validated <= 8");
            let h = u16::try_from(kind.height_tiles).expect("validated <= 8");
            let mut found: Vec<(u32, i64, u32, Tile)> = tiles
                .iter()
                .filter(|nw| d2(**nw, own_start) <= scan * scan)
                .filter(|nw| footprint_on_grass(def, **nw, w, h))
                .filter_map(|nw| {
                    let adj = adjacent_count(def, *nw, w, h, *t).min(*max);
                    (adj > 0).then(|| (adj, d2(*nw, own_start), index(def, *nw), *nw))
                })
                .collect();
            found.sort_by_key(|(adj, d, i, _)| (core::cmp::Reverse(*adj), *d, *i));
            terrain.insert(
                BuildingKindId(u16::try_from(i).expect("validated len")),
                found.into_iter().map(|c| c.3).collect(),
            );
        }

        let town = rules
            .building_kind(rules.town_kind())
            .expect("rules have a town kind");
        let (tw, th) = (
            u16::try_from(town.width_tiles).expect("validated <= 8"),
            u16::try_from(town.height_tiles).expect("validated <= 8"),
        );
        // A territory kernel of radius R owns the tiles with d² < R²
        // (sim::territory::kernel).
        let seat_r2 = i64::from(rules.town_radius_tiles).pow(2);
        let min_r2 = i64::from(knobs.expansion_min_tiles).pow(2);
        let res_r = i32::try_from(knobs.resource_radius_tiles).expect("validated <= 64");
        let penalty = i64::from(knobs.toward_enemy_penalty);
        let home_to_enemy = d2(own_start, enemy_start).isqrt();
        let mut expansion: Vec<(i64, u32, Tile)> = Vec::new();
        for nw in &tiles {
            let Some(centre) = tile_at(
                def,
                i32::from(nw.x) + i32::from((tw - 1) / 2),
                i32::from(nw.y) + i32::from((th - 1) / 2),
            ) else {
                continue;
            };
            if d2(centre, own_start) < min_r2 || !footprint_on_grass(def, *nw, tw, th) {
                continue;
            }
            let inside = (0..th).all(|dy| {
                (0..tw).all(|dx| d2(Tile::new(nw.x + dx, nw.y + dy), own_start) < seat_r2)
            });
            if !inside {
                continue;
            }
            let mut resources: i64 = 0;
            for dy in -res_r..=res_r {
                for dx in -res_r..=res_r {
                    if dx * dx + dy * dy >= res_r * res_r {
                        continue;
                    }
                    let Some(t) = tile_at(def, i32::from(centre.x) + dx, i32::from(centre.y) + dy)
                    else {
                        continue;
                    };
                    let is_resource =
                        matches!(def.tile(t.x, t.y), Terrain::Forest | Terrain::Mountain);
                    if is_resource && d2(t, own_start) >= seat_r2 {
                        resources += 1;
                    }
                }
            }
            let gain = (home_to_enemy - d2(centre, enemy_start).isqrt()).max(0);
            expansion.push((resources - penalty * gain, index(def, *nw), *nw));
        }
        expansion.sort_by_key(|(score, i, _)| (core::cmp::Reverse(*score), *i));
        expansion.truncate(EXPANSION_CANDIDATES);

        let mut border: Vec<(i64, u32, Tile)> = tiles
            .iter()
            .filter(|t| d2(**t, own_start) <= scan * scan && def.tile(t.x, t.y) == Terrain::Grass)
            .map(|t| (d2(*t, enemy_start), index(def, *t), *t))
            .collect();
        border.sort_by_key(|(d, i, _)| (*d, *i));

        let dx = i64::from(enemy_start.x) - i64::from(own_start.x);
        let dy = i64::from(enemy_start.y) - i64::from(own_start.y);
        let len = d2(own_start, enemy_start).isqrt().max(1);
        let r = i64::from(knobs.rally_tiles_from_seat);
        let clamp =
            |v: i64, hi: u16| u16::try_from(v.clamp(0, i64::from(hi) - 1)).expect("clamped");
        let rally_seed = Tile::new(
            clamp(i64::from(own_start.x) + dx * r / len, def.width),
            clamp(i64::from(own_start.y) + dy * r / len, def.height),
        );

        MapAnalysis {
            me,
            own_start,
            enemy_start,
            near_town_offsets,
            terrain,
            expansion: expansion.into_iter().map(|c| c.2).collect(),
            border_edge: border.into_iter().map(|c| c.2).collect(),
            rally_seed,
        }
    }
}

/// What [`site_for`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteSearch {
    /// `check_build` accepts `builder` building the kind at `tile`.
    Found {
        /// The builder, nearest the footprint centre of the pool.
        builder: UnitId,
        /// The footprint's north-west tile.
        tile: Tile,
    },
    /// No unit may build (the pool is empty): wait.
    NoBuilder,
    /// No candidate passes: the step is blocked.
    NoSite,
    /// The plan's `check_build` probes ran out first: try again next plan.
    OutOfProbes,
}

/// The own complete, uncontested Town a `NearTown` building of `kind` goes
/// around: for a kind with `max_per_town`, the one with the fewest of the
/// kind attributed to it (sites included) while under the limit; for any
/// other kind, the first. Towns are taken Seat first, then by id.
pub fn near_town_anchor(ctx: &Ctx<'_>, kind: BuildingKindId) -> Option<BuildingId> {
    let rules = ctx.view.rules();
    let def = rules.building_kind(kind.0)?;
    let mut towns = ctx
        .facts
        .towns
        .iter()
        .copied()
        .filter(|t| !ctx.facts.contested.contains(t));
    match def.max_per_town {
        None => towns.next(),
        Some(max) => towns
            .map(|t| (ctx.facts.per_town.get(&(t, kind)).copied().unwrap_or(0), t))
            .filter(|(n, _)| *n < max)
            .min_by_key(|(n, _)| *n)
            .map(|(_, t)| t),
    }
}

/// Whether the footprint of `kind` at `nw` passes the cheap tests: on the
/// map, own land, passable and with no unit on it, inside a ring of
/// `building_gap_tiles` that is on the map and passable (for `Terrain`,
/// only the ring's grass tiles need be passable, so a yard may touch its
/// forest, and the ring may leave the map), and with that ring clear of
/// every footprint an earlier `Build` of the same plan places
/// ([`Ctx::planned`]), which the view does not show yet.
pub fn fits(ctx: &mut Ctx<'_>, kind: BuildingKindId, nw: Tile, at: Placement) -> bool {
    let view = ctx.view;
    let rules = view.rules();
    let Some(def) = rules.map(&view.setup().map) else {
        return false;
    };
    let Some(k) = rules.building_kind(kind.0) else {
        return false;
    };
    let (w, h) = (
        i32::try_from(k.width_tiles).expect("validated <= 8"),
        i32::try_from(k.height_tiles).expect("validated <= 8"),
    );
    let map = view.map();
    let territory = view.territory();
    let (x0, y0) = (i32::from(nw.x), i32::from(nw.y));
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let Some(t) = tile_at(def, x, y) else {
                return false;
            };
            if !territory.is_buildable(ctx.me, t) || !map.passable(t) {
                return false;
            }
        }
    }
    let gap = i32::try_from(ctx.knobs.building_gap_tiles).expect("validated <= 64");
    for y in y0 - gap..y0 + h + gap {
        for x in x0 - gap..x0 + w + gap {
            if (x0..x0 + w).contains(&x) && (y0..y0 + h).contains(&y) {
                continue;
            }
            match tile_at(def, x, y) {
                None if at == Placement::Terrain => {}
                None => return false,
                Some(t) => {
                    let grass = def.tile(t.x, t.y) == Terrain::Grass;
                    let needs_free = at != Placement::Terrain || grass;
                    if needs_free && !map.passable(t) {
                        return false;
                    }
                }
            }
        }
    }
    let near_planned = ctx.planned.iter().any(|&(px0, py0, px1, py1)| {
        x0 - gap <= px1 && px0 <= x0 + w - 1 + gap && y0 - gap <= py1 && py0 <= y0 + h - 1 + gap
    });
    if near_planned {
        return false;
    }
    let occupied = ctx.occupied();
    !(y0..y0 + h)
        .any(|y| (x0..x0 + w).any(|x| tile_at(def, x, y).is_some_and(|t| occupied.contains(&t))))
}

/// Choose a site and a builder from `pool` for a building of `kind` placed
/// by policy `at`, for build-order step `step`. The step's remembered site
/// (`mem.site`) is tried first and forgotten when it fails; a site found
/// is remembered. Each candidate that passes [`fits`] and the policy's
/// spacing gets the pool's unit nearest its centre (ties to the lowest
/// id) and one `check_build` probe.
pub fn site_for(
    ctx: &mut Ctx<'_>,
    mem: &mut BotMemory,
    step: u16,
    kind: BuildingKindId,
    at: Placement,
    pool: &[UnitId],
) -> SiteSearch {
    if pool.is_empty() {
        return SiteSearch::NoBuilder;
    }
    if let Some((s, tile)) = mem.site
        && s == step
    {
        match probe(ctx, kind, tile, at, pool) {
            Some(SiteSearch::Found { builder, tile }) => {
                return SiteSearch::Found { builder, tile };
            }
            Some(SiteSearch::OutOfProbes) => return SiteSearch::OutOfProbes,
            _ => mem.site = None,
        }
    }
    let candidates: Vec<Tile> = match at {
        Placement::NearTown => {
            let Some(town) = near_town_anchor(ctx, kind) else {
                return SiteSearch::NoSite;
            };
            let Some(b) = ctx.view.building(town) else {
                return SiteSearch::NoSite;
            };
            let c = kind_centre_tile(ctx.view.rules(), b.kind, b.tile);
            ctx.map
                .near_town_offsets
                .iter()
                .filter_map(|(dx, dy)| {
                    let x = u16::try_from(i32::from(c.x) + dx).ok()?;
                    let y = u16::try_from(i32::from(c.y) + dy).ok()?;
                    Some(Tile::new(x, y))
                })
                .collect()
        }
        Placement::Terrain => ctx.map.terrain.get(&kind).cloned().unwrap_or_default(),
        Placement::Expansion => ctx.map.expansion.clone(),
        Placement::BorderEdge => ctx.map.border_edge.clone(),
    };
    for tile in candidates {
        match probe(ctx, kind, tile, at, pool) {
            Some(SiteSearch::Found { builder, tile }) => {
                mem.site = Some((step, tile));
                return SiteSearch::Found { builder, tile };
            }
            Some(SiteSearch::OutOfProbes) => return SiteSearch::OutOfProbes,
            _ => {}
        }
    }
    SiteSearch::NoSite
}

/// Test one candidate: `None` when it fails a cheap test, else the probe's
/// verdict (`Found`, `OutOfProbes`, or `NoSite` when `check_build`
/// rejects it).
fn probe(
    ctx: &mut Ctx<'_>,
    kind: BuildingKindId,
    tile: Tile,
    at: Placement,
    pool: &[UnitId],
) -> Option<SiteSearch> {
    if !fits(ctx, kind, tile, at) || !spaced(ctx, kind, tile, at) {
        return None;
    }
    if ctx.probes_left == 0 {
        return Some(SiteSearch::OutOfProbes);
    }
    let centre = kind_centre(ctx.view.rules(), kind, tile);
    let builder = pool
        .iter()
        .filter_map(|id| ctx.view.unit(*id))
        .min_by_key(|u| (u.pos.dist_sq_i64(centre), u.id))?
        .id;
    Some(match ctx.check_build(builder, kind, tile) {
        Ok(_) => SiteSearch::Found { builder, tile },
        Err(_) => SiteSearch::NoSite,
    })
}

/// The policy's spacing: a second Town keeps `expansion_min_tiles` from
/// every own Town (sites included), centre to centre; a border building
/// keeps `tower_spacing_tiles` from every own building of a border kind
/// (sites included).
fn spaced(ctx: &Ctx<'_>, kind: BuildingKindId, tile: Tile, at: Placement) -> bool {
    let rules = ctx.view.rules();
    match at {
        Placement::Expansion => {
            let c = kind_centre(rules, kind, tile);
            let min = tiles_sq(ctx.knobs.expansion_min_tiles);
            ctx.facts
                .town_centres
                .iter()
                .all(|t| t.dist_sq_i64(c) >= min)
        }
        Placement::BorderEdge => {
            let c = kind_centre_tile(rules, kind, tile);
            let min = i64::from(ctx.knobs.tower_spacing_tiles).pow(2);
            ctx.facts.towers.iter().all(|t| d2(*t, c) >= min)
        }
        Placement::NearTown | Placement::Terrain => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn rules() -> Rules {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap()
    }

    /// The tables are a pure function of the rules, the setup's map and
    /// starts and the knobs: rebuilt, they are equal, and the seed of the
    /// match does not enter them.
    #[test]
    fn map_analysis_is_a_pure_function_of_the_rules() {
        let r = rules();
        let k = r.ai.personality("standard").unwrap();
        let a = MatchSetup::bots(&r, 1, ("standard", "standard"), ("standard", "standard"));
        let b = MatchSetup::bots(&r, 99, ("standard", "standard"), ("standard", "standard"));
        for p in [PlayerId(0), PlayerId(1)] {
            let one = MapAnalysis::new(&r, &a, p, k);
            assert_eq!(one, MapAnalysis::new(&r, &a, p, k));
            assert_eq!(one, MapAnalysis::new(&r, &b, p, k));
        }
    }

    /// The shapes the site rules rely on, on `plains_1v1`: the starts, the
    /// rings, candidates on grass near the own start, and a rally toward
    /// the enemy.
    #[test]
    fn map_analysis_tables_have_the_documented_shape() {
        let r = rules();
        let k = r.ai.personality("standard").unwrap();
        let setup = MatchSetup::vs_bot(&r, 1, "standard", "standard");
        let def = r.map(&setup.map).unwrap();
        let p0 = MapAnalysis::new(&r, &setup, PlayerId(0), k);
        let p1 = MapAnalysis::new(&r, &setup, PlayerId(1), k);
        assert_eq!(
            (p0.own_start, p0.enemy_start),
            (Tile::new(24, 64), Tile::new(103, 64))
        );
        assert_eq!(
            (p1.own_start, p1.enemy_start),
            (Tile::new(103, 64), Tile::new(24, 64))
        );
        // Rings 4..=12: 8r offsets each, inner first, starting at the
        // north-west corner.
        let rings: usize = (4..=12).map(|r| 8 * r).sum();
        assert_eq!(p0.near_town_offsets.len(), rings);
        assert_eq!(p0.near_town_offsets[0], (-4, -4));
        assert_eq!(*p0.near_town_offsets.last().unwrap(), (-12, -11));
        for (kind, list) in &p0.terrain {
            let def_kind = r.building_kind(kind.0).unwrap();
            assert!(matches!(
                def_kind.gather.as_ref().unwrap().slots,
                Slots::Adjacent { .. }
            ));
            assert!(!list.is_empty(), "{}", def_kind.id);
            for t in list {
                assert!(d2(*t, p0.own_start) <= 40 * 40);
            }
        }
        assert_eq!(p0.terrain.len(), 2, "the Lumber Yard and the Ore Pit");
        assert!(!p0.expansion.is_empty() && p0.expansion.len() <= EXPANSION_CANDIDATES);
        for nw in &p0.expansion {
            assert!(footprint_on_grass(def, *nw, 3, 3));
        }
        assert_eq!(
            def.tile(p0.border_edge[0].x, p0.border_edge[0].y),
            Terrain::Grass
        );
        assert!(
            p0.border_edge[0].x > p0.own_start.x,
            "nearest the enemy first"
        );
        assert!(
            d2(p0.border_edge[0], p0.enemy_start)
                <= d2(*p0.border_edge.last().unwrap(), p0.enemy_start)
        );
        assert_eq!(p0.rally_seed, Tile::new(36, 64));
        assert_eq!(p1.rally_seed, Tile::new(91, 64));
    }

    /// Every expansion candidate lies on own land at the standard start,
    /// at least `expansion_min_tiles` from the Seat, and the site the bot
    /// picks is pinned (computed once, on `plains_1v1`): south of each
    /// Seat, toward the forest beyond the first border. P1's pick (106, 81)
    /// is not the exact mirror of P0's (18, 81), which is (107, 81): equal
    /// scores break to the lowest tile index on both sides.
    #[test]
    fn expansion_site_is_own_buildable_and_pinned() {
        use crate::testkit::{Arena, P0, P1};
        let r = rules();
        let mut setup = MatchSetup::bots(&r, 1, ("standard", "standard"), ("standard", "standard"));
        setup.debug_commands = true;
        let a = Arena::frozen(setup, Vec::new());
        let town = BuildingKindId(r.town_kind());
        for (p, pinned) in [(P0, Tile::new(18, 81)), (P1, Tile::new(106, 81))] {
            let map = a.map(p);
            assert_eq!(map.expansion[0], pinned, "{p:?}");
            let view = a.sim.view();
            for nw in &map.expansion {
                for dy in 0..3 {
                    for dx in 0..3 {
                        let t = Tile::new(nw.x + dx, nw.y + dy);
                        assert!(view.territory().is_buildable(p, t), "{p:?} {nw:?}");
                    }
                }
                let c = Tile::new(nw.x + 1, nw.y + 1);
                assert!(d2(c, map.own_start) >= 14 * 14);
            }
            let mut ctx = a.ctx(p);
            let mut mem = BotMemory::default();
            let pool = view.idle_workers(p);
            let found = site_for(&mut ctx, &mut mem, 9, town, Placement::Expansion, &pool);
            assert!(
                matches!(found, SiteSearch::Found { tile, .. } if tile == pinned),
                "{p:?} {found:?}"
            );
            assert_eq!(mem.site, Some((9, pinned)), "remembered");
            assert_eq!((ctx.probes_left, ctx.probes_made), (3, 1), "one probe");
        }
    }

    /// The border Watchtower goes on the own grass tile nearest the enemy
    /// start that keeps a free ring: (43, 64) for P0 and (84, 64) for P1
    /// (the Seats' first borders end 19 tiles out on row 64).
    #[test]
    fn border_edge_tower_is_the_owned_tile_nearest_the_enemy_start() {
        use crate::testkit::{Arena, P0, P1};
        let r = rules();
        let mut setup = MatchSetup::bots(&r, 1, ("standard", "standard"), ("standard", "standard"));
        setup.debug_commands = true;
        let grants = [
            crate::testkit::grant_all(&r, P0, 100),
            crate::testkit::grant_all(&r, P1, 100),
        ]
        .concat();
        let mut a = Arena::frozen(setup, grants);
        let tower = BuildingKindId(r.building_kind_by_id("watchtower").unwrap().0);
        for (p, want) in [(P0, Tile::new(43, 64)), (P1, Tile::new(84, 64))] {
            let view = a.sim.view();
            let mut ctx = a.ctx(p);
            let mut mem = BotMemory::default();
            let pool = view.idle_workers(p);
            let found = site_for(&mut ctx, &mut mem, 16, tower, Placement::BorderEdge, &pool);
            let SiteSearch::Found { tile, .. } = found else {
                panic!("{p:?} {found:?}");
            };
            assert_eq!(tile, want, "{p:?}");
            let enemy = a.map(p).enemy_start;
            let nearer_owned = (0..128u16)
                .flat_map(|y| (0..128u16).map(move |x| Tile::new(x, y)))
                .filter(|t| view.territory().is_buildable(p, *t))
                .any(|t| d2(t, enemy) < d2(want, enemy));
            assert!(!nearer_owned, "{p:?}: no own tile is nearer the enemy");
        }
        // A second tower keeps `tower_spacing_tiles` from the first.
        a.issue(
            P0,
            vec![crate::testkit::place(&r, P0, "watchtower", 43, 64)],
        );
        let view = a.sim.view();
        let mut ctx = a.ctx(P0);
        let mut mem = BotMemory::default();
        let pool = view.idle_workers(P0);
        let SiteSearch::Found { tile, .. } =
            site_for(&mut ctx, &mut mem, 16, tower, Placement::BorderEdge, &pool)
        else {
            panic!("a second site");
        };
        assert!(d2(tile, Tile::new(43, 64)) >= 8 * 8, "{tile:?}");
    }

    /// Buildings placed one after another by `NearTown` never touch: each
    /// footprint keeps a ring of `building_gap_tiles` free of other
    /// footprints, and the Seat's own ring stays free.
    #[test]
    fn near_town_keeps_a_free_ring() {
        use crate::testkit::{Arena, P0, grant_all, standard_vs_passive};
        let r = rules();
        let setup = [
            grant_all(&r, P0, 2000),
            vec![crate::testkit::spawn(&r, P0, "yeoman", 24, 70, 4)],
        ]
        .concat();
        let mut a = Arena::frozen(standard_vs_passive(&r), setup);
        let kinds = [
            "croft",
            "croft",
            "scriptorium",
            "croft",
            "muster_hall",
            "croft",
            "croft",
            "scriptorium",
        ];
        for kind in kinds {
            let k = BuildingKindId(r.building_kind_by_id(kind).unwrap().0);
            let view = a.sim.view();
            let mut ctx = a.ctx(P0);
            let mut mem = BotMemory::default();
            let pool = view.idle_workers(P0);
            let found = site_for(&mut ctx, &mut mem, 1, k, Placement::NearTown, &pool);
            let SiteSearch::Found { builder, tile } = found else {
                panic!("a site for {kind}: {found:?} {:?}", a.sim.tick());
            };
            a.issue(
                P0,
                vec![sim::Command::Build {
                    unit: builder,
                    kind: k,
                    tile,
                }],
            );
        }
        let view = a.sim.view();
        let rects: Vec<(i32, i32, i32, i32)> = view
            .buildings()
            .filter(|b| b.owner == P0)
            .map(|b| {
                let d = r.building_kind(b.kind.0).unwrap();
                let (x, y) = (i32::from(b.tile.x), i32::from(b.tile.y));
                (
                    x,
                    y,
                    x + i32::try_from(d.width_tiles).unwrap() - 1,
                    y + i32::try_from(d.height_tiles).unwrap() - 1,
                )
            })
            .collect();
        assert_eq!(rects.len(), 1 + kinds.len(), "the Seat and every site");
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                // Grown by the gap of 1, `a` must not overlap `b`.
                let apart = a.2 + 1 < b.0 || b.2 < a.0 - 1 || a.3 + 1 < b.1 || b.3 < a.1 - 1;
                assert!(apart, "{a:?} and {b:?} touch");
            }
        }
    }
}
