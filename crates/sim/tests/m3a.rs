//! M3a acceptance tests: the territory field, the standard start, Towns,
//! construction and base-yield income, gathering, the Yield Cap and idle
//! seeking, training with the unit cost ramps and the pop cap, and the
//! economy fixture. Test names start with
//! `territory_` or `economy_` so that `cargo test -p sim territory` and
//! `cargo test -p sim economy` select them (the filter matches test paths,
//! not integration-test file names).

use proptest::prelude::*;
use rules::Rules;
use sim::{
    BuildingId, BuildingKindId, Command, Construction, FxVec2, Job, MatchSetup, PlayerCommand,
    PlayerId, RejectReason, Sim, SimEvent, SimView, Source, SourceOwner, StartKind, TerritoryField,
    Tile, UnitId, UnitKindId,
};
use std::path::{Path, PathBuf};

struct NoAi;

impl sim::AiController for NoAi {
    fn think(&mut self, _player: PlayerId, _view: &SimView<'_>) -> Vec<Command> {
        Vec::new()
    }
}

/// A two-player scenario (debug commands on) from the standard start.
fn standard(seed: u64) -> Sim {
    let rules = load_rules();
    let mut setup = MatchSetup::scenario(&rules, seed);
    setup.start = StartKind::Standard;
    Sim::new(setup, rules, Box::new(NoAi))
}

fn kind(rules: &Rules, id: &str) -> BuildingKindId {
    BuildingKindId(
        rules
            .building_kind_by_id(id)
            .expect("kind in buildings.ron")
            .0,
    )
}

fn resource(rules: &Rules, id: &str) -> usize {
    rules.resource_index(id).expect("resource in resources.ron")
}

/// Issue `cmds` this tick, step until they have been applied, and return
/// every event since.
fn apply(sim: &mut Sim, cmds: Vec<PlayerCommand>) -> Vec<SimEvent> {
    let delay = sim.setup().cmd_delay;
    sim.step(&cmds);
    for _ in 0..delay {
        sim.step(&[]);
    }
    sim.drain_events()
}

fn build(p: u8, seq: u32, unit: u32, kind: BuildingKindId, x: u16, y: u16) -> PlayerCommand {
    PlayerCommand::new(
        PlayerId(p),
        seq,
        Command::Build {
            unit: UnitId(unit),
            kind,
            tile: Tile::new(x, y),
        },
    )
}

fn rejections(events: &[SimEvent]) -> Vec<RejectReason> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::CommandRejected { reason, .. } => Some(*reason),
            _ => None,
        })
        .collect()
}

fn spawn_kind(p: u8, seq: u32, kind: u16, x: i32, y: i32, count: u16) -> PlayerCommand {
    PlayerCommand::new(
        PlayerId(p),
        seq,
        Command::DebugSpawn {
            owner: PlayerId(p),
            kind: UnitKindId(kind),
            at: FxVec2::from_ints(x, y),
            count,
        },
    )
}

fn stock(sim: &Sim, p: u8, r: usize) -> i64 {
    sim.view().economy(PlayerId(p)).stock[r]
}

/// Step with no commands until `done` holds, at most `max` ticks; returns
/// whether it held.
fn run_until(sim: &mut Sim, max: u32, mut done: impl FnMut(&Sim) -> bool) -> bool {
    for _ in 0..max {
        if done(sim) {
            return true;
        }
        sim.step(&[]);
    }
    done(sim)
}

fn load_rules() -> Rules {
    Rules::load(data_dir()).expect("data/ loads")
}

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Verify `fixtures/<name>.eonreplay` and compare its final hash with
/// `fixtures/<name>.hash` (as `m1.rs` does).
fn golden(name: &str) {
    let replay = fixtures_dir().join(format!("{name}.eonreplay"));
    let hash_file = fixtures_dir().join(format!("{name}.hash"));
    let expected = std::fs::read_to_string(&hash_file)
        .unwrap_or_else(|e| panic!("{}: {e}", hash_file.display()));
    let outcome = sim::verify(&replay, &data_dir(), Box::new(NoAi))
        .unwrap_or_else(|e| panic!("{}: {e}", replay.display()));
    match outcome {
        sim::VerifyOutcome::Ok { final_hash, .. } => {
            assert_eq!(format!("0x{final_hash:016x}"), expected.trim(), "{name}");
        }
        other => panic!("{name}: {other}"),
    }
}

/// 16 cases locally, 256 under CI, `PROPTEST_CASES` overrides (the same
/// switch as m1.rs, which runs 8 locally). M3a set 32 locally; M4b halved
/// it when the warm `cargo test -p sim` reached its 10 s budget, as
/// `docs/BUILD_TIMES.md` advises. CI still runs 256.
fn proptest_cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if std::env::var_os("CI").is_some() {
            256
        } else {
            16
        })
}

/// The Town radii from `rules.ron` (tier 1, tier 2).
fn town_radii(rules: &Rules) -> (u16, u16) {
    (
        u16::try_from(rules.town_radius_tiles).expect("radius fits u16"),
        u16::try_from(rules.town_radius_grown_tiles).expect("radius fits u16"),
    )
}

/// The Watchtower's border radius from `buildings.ron`.
fn watchtower_radius(rules: &Rules) -> u16 {
    let (_, tower) = rules
        .building_kind_by_id("watchtower")
        .expect("buildings.ron has a watchtower");
    u16::try_from(
        tower
            .border_radius_tiles
            .expect("the watchtower projects a border"),
    )
    .expect("radius fits u16")
}

/// Every tile's owner on a `w` x `h` field, checked against the oracle;
/// returns (P0 tiles, P1 tiles, neutral tiles).
fn counts_checked_against_oracle(f: &TerritoryField, sources: &[Source]) -> (u32, u32, u32) {
    let (mut p0, mut p1, mut neutral) = (0, 0, 0);
    for y in 0..f.height() {
        for x in 0..f.width() {
            let t = Tile::new(x, y);
            let owner = f.owner_at(t);
            assert_eq!(owner, oracle_owner(sources, 2, t), "{t:?}");
            match owner {
                Some(PlayerId(0)) => p0 += 1,
                Some(_) => p1 += 1,
                None => neutral += 1,
            }
        }
    }
    assert_eq!(p0, f.owned_tile_count(PlayerId(0)));
    assert_eq!(p1, f.owned_tile_count(PlayerId(1)));
    (p0, p1, neutral)
}

fn source(owner: SourceOwner, x: u16, y: u16, radius: u16) -> Source {
    Source {
        owner,
        centre: Tile::new(x, y),
        radius,
    }
}

fn player(p: u8) -> SourceOwner {
    SourceOwner::Player(PlayerId(p))
}

/// Independent oracle: each tile's owner computed straight from the source
/// list, without the field's layers or incremental bookkeeping.
fn oracle_owner(sources: &[Source], players: u8, t: Tile) -> Option<PlayerId> {
    let mut strength = vec![0u32; usize::from(players) + 1];
    for s in sources {
        let k = sim::territory::kernel(
            s.radius,
            i32::from(t.x) - i32::from(s.centre.x),
            i32::from(t.y) - i32::from(s.centre.y),
        );
        let layer = match s.owner {
            SourceOwner::Neutral => 0,
            SourceOwner::Player(p) => usize::from(p.0) + 1,
        };
        strength[layer] += u32::from(k);
    }
    let best = *strength.iter().max().expect("at least one layer");
    let winners: Vec<usize> = (0..strength.len())
        .filter(|&l| strength[l] == best)
        .collect();
    match winners.as_slice() {
        [l] if best > 0 && *l > 0 => Some(PlayerId(u8::try_from(*l - 1).unwrap())),
        _ => None,
    }
}

fn hash(f: &TerritoryField) -> u64 {
    xxhash_rust::xxh3::xxh3_64(&postcard::to_allocvec(f).expect("field serialises"))
}

/// Two Towns mirrored about the centre line of the 128-wide map, at the
/// tier-1 radius from `rules.ron`. The map is even-width and mirrors x to
/// `127 - x`, so no tile is exactly equidistant from both centres: ties come
/// only from the floored square root, and every one of them is neutral.
#[test]
fn territory_mirrored_towns_tie_only_by_isqrt_floor_and_ties_go_neutral() {
    let rules = load_rules();
    let (r1, _) = town_radii(&rules);
    let map = rules.map(&rules.default_map).expect("default map");
    let (w, h) = (map.width, map.height);
    let west = source(player(0), 48, 64, r1);
    let east = source(player(1), w - 1 - 48, 64, r1);
    let mut f = TerritoryField::new(w, h, 2);
    f.add_source(BuildingId(1), west);
    f.add_source(BuildingId(2), east);

    // The field agrees with the oracle on every tile.
    let sources = [west, east];
    for y in 0..h {
        for x in 0..w {
            let t = Tile::new(x, y);
            assert_eq!(f.owner_at(t), oracle_owner(&sources, 2, t), "{t:?}");
        }
    }
    // The owner map mirrors exactly: what one player owns at x, the other
    // owns at w - 1 - x.
    for y in 0..h {
        for x in 0..w {
            let a = f.owner_at(Tile::new(x, y));
            let b = f.owner_at(Tile::new(w - 1 - x, y));
            assert_eq!(a.map(|p| 1 - p.0), b.map(|p| p.0), "({x}, {y})");
        }
    }
    // Golden counts at R = 20 (regenerate from the oracle above if
    // `town_radius_tiles` moves).
    if r1 == 20 {
        assert_eq!(f.owned_tile_count(PlayerId(0)), 1167);
        assert_eq!(f.owned_tile_count(PlayerId(1)), 1167);
        let p0 = SourceOwner::Player(PlayerId(0));
        let p1 = SourceOwner::Player(PlayerId(1));
        let mut ties: Vec<Tile> = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let t = Tile::new(x, y);
                let a = f.strength_at(p0, t);
                if a > 0 && a == f.strength_at(p1, t) {
                    ties.push(t);
                }
            }
        }
        let expected: Vec<Tile> = [(63, 54), (63, 56), (63, 72), (63, 74)]
            .into_iter()
            .flat_map(|(x, y)| [Tile::new(x, y), Tile::new(x + 1, y)])
            .collect();
        let mut sorted = ties.clone();
        sorted.sort();
        let mut exp = expected;
        exp.sort();
        assert_eq!(sorted, exp);
        for t in ties {
            assert_eq!(f.owner_at(t), None, "tie at {t:?} is neutral");
        }
    }
}

/// On an odd-width field the centre column is exactly equidistant from two
/// mirrored Towns, so every tile there that either kernel reaches is an
/// exact tie, and neutral.
#[test]
fn territory_odd_width_centre_column_is_neutral_wherever_reached() {
    let rules = load_rules();
    let (r1, _) = town_radii(&rules);
    let (w, h) = (127u16, 128u16);
    let mid = (w - 1) / 2;
    let mut f = TerritoryField::new(w, h, 2);
    f.add_source(BuildingId(1), source(player(0), 48, 64, r1));
    f.add_source(BuildingId(2), source(player(1), w - 1 - 48, 64, r1));
    let mut reached = 0;
    for y in 0..h {
        let t = Tile::new(mid, y);
        let a = f.strength_at(player(0), t);
        assert_eq!(a, f.strength_at(player(1), t));
        if a > 0 {
            reached += 1;
        }
        assert_eq!(f.owner_at(t), None, "centre column tile {t:?}");
    }
    assert!(reached > 0, "the kernels reach the centre column");
    if r1 == 20 {
        assert_eq!(reached, 27);
    }
}

/// The M3a territory acceptance fixture at field level: both Seats at the
/// map starts with the tier-1 radius, plus a P0 Watchtower at (43, 64) with
/// its radius from `buildings.ron`. Nothing reaches columns 63 and 64, so
/// the mirrored midline is neutral (the sim-level fixture from the standard
/// start lands with the Town loop).
#[test]
fn territory_fixture_two_towns_and_a_watchtower_golden_counts_field_level() {
    let rules = load_rules();
    let (r1, _) = town_radii(&rules);
    let tower = watchtower_radius(&rules);
    let map = rules.map(&rules.default_map).expect("default map");
    let mut f = TerritoryField::new(map.width, map.height, 2);
    let mut sources = Vec::new();
    for (p, s) in map.starts.iter().enumerate() {
        let src = source(player(u8::try_from(p).unwrap()), s.x, s.y, r1);
        f.add_source(BuildingId(u32::try_from(p).unwrap() + 1), src);
        sources.push(src);
    }
    let watch = source(player(0), 43, 64, tower);
    f.add_source(BuildingId(3), watch);
    sources.push(watch);

    let (p0, p1, neutral) = counts_checked_against_oracle(&f, &sources);
    for y in 0..map.height {
        for x in [map.width / 2 - 1, map.width / 2] {
            assert_eq!(f.owner_at(Tile::new(x, y)), None, "midline ({x}, {y})");
        }
    }
    if (r1, tower, map.width, map.height) == (20, 2, 128, 128) {
        assert_eq!((p0, p1, neutral), (1248, 1245, 13891));
    }
}

/// The contested configuration (mirrored Towns at (48, 64) and (79, 64))
/// plus a P0 Watchtower on the ford at (63, 64): P0 is stronger on its own
/// column, and the three tiles east of it become exact ties, neutral.
#[test]
fn territory_contested_watchtower_on_the_ford_creates_neutral_ties() {
    let rules = load_rules();
    let (r1, _) = town_radii(&rules);
    let tower = watchtower_radius(&rules);
    let map = rules.map(&rules.default_map).expect("default map");
    let (w, h) = (map.width, map.height);
    let sources = [
        source(player(0), 48, 64, r1),
        source(player(1), w - 1 - 48, 64, r1),
        source(player(0), 63, 64, tower),
    ];
    let mut f = TerritoryField::new(w, h, 2);
    for (i, s) in sources.iter().enumerate() {
        f.add_source(BuildingId(u32::try_from(i).unwrap() + 1), *s);
    }
    let counts = counts_checked_against_oracle(&f, &sources);
    if (r1, tower, w, h) == (20, 2, 128, 128) {
        assert_eq!(counts, (1167, 1164, 14053));
        assert_eq!(f.owner_at(Tile::new(63, 64)), Some(PlayerId(0)));
        for y in 63..=65 {
            let t = Tile::new(64, y);
            assert!(f.strength_at(player(0), t) > 0, "{t:?} reached");
            assert_eq!(
                f.strength_at(player(0), t),
                f.strength_at(player(1), t),
                "{t:?} tie"
            );
            assert_eq!(f.owner_at(t), None, "{t:?} neutral");
        }
    }
}

/// One random edit of the source set.
#[derive(Debug, Clone)]
enum Edit {
    /// Add a source under a new id; `mirror` also adds its mirror image for
    /// the other player, which puts ties on the centre line.
    Add {
        owner: u8,
        x: u16,
        y: u16,
        r: usize,
        mirror: bool,
    },
    /// Remove the n-th live source (modulo the count).
    Remove { n: usize },
    /// Change the n-th live source's owner, centre or radius.
    Change {
        n: usize,
        owner: u8,
        x: u16,
        y: u16,
        r: usize,
    },
}

/// Owners 0 and 1 are players, 2 is the neutral layer.
fn owner_of(code: u8) -> SourceOwner {
    match code % 3 {
        2 => SourceOwner::Neutral,
        p => player(p),
    }
}

/// Coordinates biased toward the edges and the centre line, where clipping
/// and ties live.
fn coord(size: u16) -> impl Strategy<Value = u16> {
    let max = size - 1;
    prop_oneof![
        0..size,
        0u16..4,
        (max - 3)..=max,
        (size / 2 - 3)..(size / 2 + 3),
    ]
}

fn edit(w: u16, h: u16) -> impl Strategy<Value = Edit> {
    prop_oneof![
        3 => (0u8..3, coord(w), coord(h), any::<usize>(), any::<bool>())
            .prop_map(|(owner, x, y, r, mirror)| Edit::Add { owner, x, y, r, mirror }),
        1 => any::<usize>().prop_map(|n| Edit::Remove { n }),
        2 => (any::<usize>(), 0u8..3, coord(w), coord(h), any::<usize>())
            .prop_map(|(n, owner, x, y, r)| Edit::Change { n, owner, x, y, r }),
    ]
}

/// Apply `edits` to a field incrementally and assert, every 10 edits and at
/// the end, that it equals a full recompute of its sources: as a value and
/// by hash of its serialised bytes.
fn check_incremental_equals_full(
    w: u16,
    h: u16,
    radii: &[u16],
    edits: &[Edit],
) -> Result<(), TestCaseError> {
    let mut f = TerritoryField::new(w, h, 2);
    let mut next = 1u32;
    for (i, e) in edits.iter().enumerate() {
        let live: Vec<BuildingId> = f.sources().keys().copied().collect();
        match *e {
            Edit::Add {
                owner,
                x,
                y,
                r,
                mirror,
            } => {
                let radius = radii[r % radii.len()];
                f.add_source(BuildingId(next), source(owner_of(owner), x, y, radius));
                next += 1;
                if mirror {
                    let other = match owner_of(owner) {
                        SourceOwner::Player(p) => player(1 - p.0),
                        SourceOwner::Neutral => SourceOwner::Neutral,
                    };
                    f.add_source(BuildingId(next), source(other, w - 1 - x, y, radius));
                    next += 1;
                }
            }
            Edit::Remove { n } if !live.is_empty() => {
                f.remove_source(live[n % live.len()]);
            }
            Edit::Change { n, owner, x, y, r } if !live.is_empty() => {
                let radius = radii[r % radii.len()];
                f.change_source(live[n % live.len()], source(owner_of(owner), x, y, radius));
            }
            Edit::Remove { .. } | Edit::Change { .. } => {}
        }
        if i % 10 == 9 || i + 1 == edits.len() {
            let full = f.recomputed();
            prop_assert_eq!(&f, &full, "incremental != full after edit {}", i);
            prop_assert_eq!(hash(&f), hash(&full));
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: proptest_cases(),
        .. ProptestConfig::default()
    })]

    /// 200 random source edits on the default map's field (adds, removes,
    /// moves, radius changes, owners moving to and from the neutral layer,
    /// mirrored pairs): incremental recompute equals a full recompute, by
    /// value and by hash. Radii are the Town tiers from `rules.ron`, each
    /// plus 0 to 3 for Statecraft (M4a), the Watchtower's from
    /// `buildings.ron` and small ones.
    #[test]
    fn territory_incremental_recompute_equals_full_recompute(
        edits in prop::collection::vec(edit(128, 128), 200),
    ) {
        let rules = load_rules();
        let map = rules.map(&rules.default_map).expect("default map");
        let (r1, r2) = town_radii(&rules);
        let mut radii: Vec<u16> = (0..=3).flat_map(|s| [r1 + s, r2 + s]).collect();
        radii.extend([1, 2, 3, watchtower_radius(&rules)]);
        check_incremental_equals_full(map.width, map.height, &radii, &edits)?;
    }

    /// The same on an odd-width, non-square field, where mirrored pairs
    /// produce exact ties on the centre column.
    #[test]
    fn territory_incremental_recompute_equals_full_on_an_odd_width_field(
        edits in prop::collection::vec(edit(47, 33), 200),
    ) {
        let rules = load_rules();
        let (r1, r2) = town_radii(&rules);
        let tower = watchtower_radius(&rules);
        check_incremental_equals_full(47, 33, &[1, 2, 5, tower, r1, r2], &edits)?;
    }
}

// ---------------------------------------------------------------------------
// The Town loop: standard start, Build, construction, cancel, income
// ---------------------------------------------------------------------------

/// The standard start on `plains_1v1`: Seats `BuildingId(1)` and `(2)`
/// centred on the map starts, P0's Yeomen `UnitId(1..=5)` and P1's
/// `(6..=10)`, the stockpiles from `rules.ron`, both Seats' borders and one
/// cost-grid generation bump per Seat.
#[test]
fn economy_standard_start_layout() {
    let rules = load_rules();
    let sim = standard(1);
    let view = sim.view();
    let def = rules.map(&rules.default_map).unwrap();
    let town = kind(&rules, "town");
    for (i, start) in def.starts.iter().enumerate() {
        let p = PlayerId(u8::try_from(i).unwrap());
        let seat = view
            .building(BuildingId(u32::try_from(i).unwrap() + 1))
            .unwrap();
        assert_eq!((seat.owner, seat.kind), (p, town));
        assert_eq!(seat.tile, Tile::new(start.x - 1, start.y - 1));
        assert_eq!(seat.construction, Construction::Complete);
        let ts = seat.town_state.unwrap();
        assert!(ts.seat);
        assert_eq!(ts.tier, 1);
        assert!(!view.map().passable(Tile::new(start.x, start.y)));
        assert_eq!(
            view.economy(p).stock,
            rules.amounts(&rules.standard_start.stockpile)
        );
        assert_eq!(view.towns(p), (1, rules.town_limit_base));
    }
    let per_player = rules.standard_start.units["yeoman"];
    for u in view.units() {
        let p = u8::try_from((u.id.0 - 1) / per_player).unwrap();
        assert_eq!(
            (u.owner, u.kind),
            (PlayerId(p), UnitKindId(0)),
            "{:?}",
            u.id
        );
        assert!(u.job.is_none() && u.order.is_none());
    }
    assert_eq!(view.unit_count(), usize::try_from(2 * per_player).unwrap());
    assert_eq!(view.map().cost_grid_generation(), 2);
    if rules.town_radius_tiles == 20 {
        assert_eq!(view.territory().owned_tile_count(PlayerId(0)), 1245);
        assert_eq!(view.territory().owned_tile_count(PlayerId(1)), 1245);
    }
}

/// M3a territory acceptance at sim level: from the standard start, a P0
/// Watchtower completed at (43, 64) gives the golden counts and leaves the
/// mirrored midline (columns 63 and 64) neutral.
#[test]
fn territory_fixture_two_towns_and_a_watchtower_has_golden_owned_tiles_and_a_neutral_midline() {
    let rules = load_rules();
    let mut sim = standard(2);
    let events = apply(
        &mut sim,
        vec![PlayerCommand::new(
            PlayerId(0),
            0,
            Command::DebugPlaceBuilding {
                owner: PlayerId(0),
                kind: kind(&rules, "watchtower"),
                tile: Tile::new(43, 64),
                complete: true,
            },
        )],
    );
    assert!(events.contains(&SimEvent::TerritoryChanged), "{events:?}");
    let view = sim.view();
    let field = view.territory();
    for y in 0..field.height() {
        for x in [63, 64] {
            assert_eq!(field.owner_at(Tile::new(x, y)), None, "({x}, {y})");
        }
    }
    let (r1, _) = town_radii(&rules);
    if (r1, watchtower_radius(&rules)) == (20, 2) {
        assert_eq!(field.owned_tile_count(PlayerId(0)), 1248);
        assert_eq!(field.owned_tile_count(PlayerId(1)), 1245);
    }
}

/// A Build outside the player's borders is rejected with `OutsideBorders`
/// and changes nothing: compared with a control sim whose command at the
/// same (player, seq) is rejected for another reason, every hash and
/// sub-hash agrees on every following tick, and no randomness was drawn.
#[test]
fn territory_build_outside_own_borders_is_rejected_without_state_change() {
    let rules = load_rules();
    let croft = kind(&rules, "croft");
    let mut a = standard(3);
    let mut b = standard(3);
    let rng_before = a.sub_hashes().rng;
    let stock_before = a.view().economy(PlayerId(0)).stock.clone();
    let site = Tile::new(60, 64);
    assert_eq!(a.view().territory().owner_at(site), None);
    let ea = apply(&mut a, vec![build(0, 0, 1, croft, site.x, site.y)]);
    let eb = apply(
        &mut b,
        vec![PlayerCommand::new(
            PlayerId(0),
            0,
            Command::Stop {
                units: vec![UnitId(999)],
            },
        )],
    );
    assert_eq!(rejections(&ea), [RejectReason::OutsideBorders]);
    assert_eq!(rejections(&eb), [RejectReason::NoValidUnits]);
    for _ in 0..5 {
        assert_eq!(a.hash(), b.hash());
        assert_eq!(a.sub_hashes(), b.sub_hashes());
        a.step(&[]);
        b.step(&[]);
    }
    assert_eq!(a.sub_hashes().rng, rng_before);
    let view = a.view();
    assert_eq!(view.buildings().count(), 2);
    assert!(view.unit(UnitId(1)).unwrap().job.is_none());
    assert_eq!(
        view.economy(PlayerId(0)).stock[resource(&rules, "lumber")],
        stock_before[resource(&rules, "lumber")]
    );
}

/// The second Town costs exactly the `rules.ron` ramp over the first:
/// the quote, the rules function and the formula from the raw fields agree,
/// and the deduction (against a control sim) equals the quote.
#[test]
fn economy_second_town_costs_more_than_the_first_by_the_rules_formula() {
    let rules = load_rules();
    let town = kind(&rules, "town");
    let town_def = rules.building_kind(town.0).unwrap();
    let mut sim = standard(4);
    let mut control = standard(4);
    let quote = sim.view().building_cost(PlayerId(0), town);
    assert_eq!(quote, rules.building_cost(town_def, 1));
    let lumber = resource(&rules, "lumber");
    let base = i64::from(town_def.cost["lumber"]);
    let step = i64::from(rules.building_ramp_step_permille);
    assert_eq!(quote[lumber], base * (1000 + step) / 1000);
    assert!(quote[lumber] > rules.building_cost(town_def, 0)[lumber]);
    assert_eq!(
        sim.view()
            .check_build(PlayerId(0), UnitId(1), town, Tile::new(40, 63)),
        Ok(quote.clone())
    );

    let events = apply(&mut sim, vec![build(0, 0, 1, town, 40, 63)]);
    apply(&mut control, vec![]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    let paid: Vec<i64> = control
        .view()
        .economy(PlayerId(0))
        .stock
        .iter()
        .zip(&sim.view().economy(PlayerId(0)).stock)
        .map(|(c, s)| c - s)
        .collect();
    assert_eq!(paid, quote);
    let site = sim.view().building(BuildingId(3)).unwrap();
    assert_eq!(site.paid, quote);
    assert_eq!(site.construction, Construction::Site { progress: 0 });
    assert_eq!(sim.view().towns(PlayerId(0)).0, 2, "a Town site counts");
    assert_eq!(
        sim.view().building_cost(PlayerId(0), town)[lumber],
        base * (1000 + 2 * step) / 1000,
        "the next copy ramps again"
    );
}

/// A founded Town extends its owner's borders when it completes, not when
/// it is placed: on completion the field equals a fresh field of the three
/// Town sources.
#[test]
fn territory_second_town_extends_borders_on_completion_not_at_placement() {
    let rules = load_rules();
    let town = kind(&rules, "town");
    let mut sim = standard(5);
    let before = sim.view().territory().clone();
    let events = apply(&mut sim, vec![build(0, 0, 1, town, 40, 63)]);
    assert!(events.contains(&SimEvent::BuildingPlaced {
        building: BuildingId(3)
    }));
    assert_eq!(sim.view().territory(), &before, "a site projects nothing");
    assert_eq!(
        sim.view().unit(UnitId(1)).unwrap().job,
        Some(Job::Build {
            site: BuildingId(3)
        })
    );
    let build_ticks = rules.build_time_ticks(rules.building_kind(town.0).unwrap());
    assert!(run_until(&mut sim, build_ticks + 600, |s| {
        s.view().building(BuildingId(3)).unwrap().construction == Construction::Complete
    }));
    let events = sim.drain_events();
    assert!(events.contains(&SimEvent::BuildingCompleted {
        building: BuildingId(3)
    }));
    assert!(events.contains(&SimEvent::TerritoryChanged));
    assert!(sim.view().unit(UnitId(1)).unwrap().job.is_none());
    let (r1, _) = town_radii(&rules);
    let def = rules.map(&rules.default_map).unwrap();
    let mut expected = TerritoryField::new(def.width, def.height, 2);
    for (i, s) in def.starts.iter().enumerate() {
        expected.add_source(
            BuildingId(u32::try_from(i).unwrap() + 1),
            source(player(u8::try_from(i).unwrap()), s.x, s.y, r1),
        );
    }
    expected.add_source(BuildingId(3), source(player(0), 41, 64, r1));
    assert_eq!(sim.view().territory(), &expected);
    assert!(
        expected.owned_tile_count(PlayerId(0)) > before.owned_tile_count(PlayerId(0)),
        "the second Town claims new tiles"
    );
    let t = sim
        .view()
        .building(BuildingId(3))
        .unwrap()
        .town_state
        .unwrap();
    assert!(!t.seat);
    assert_eq!(t.tier, 1);
}

/// A Town yields exactly its `base_yield` per income period from any phase,
/// and nothing else moves without workers.
#[test]
fn economy_town_base_yield_is_exact_per_income_period() {
    let rules = load_rules();
    let town = rules.building_kind(rules.town_kind()).unwrap();
    let period = rules.income_period_ticks();
    let start = rules.amounts(&rules.standard_start.stockpile);
    let yields = rules.amounts(&town.base_yield);
    let mut sim = standard(6);
    let mut at = Vec::new();
    for t in 0..=(2 * period + 300) {
        if t % 50 == 0 || t == period || t == 2 * period {
            at.push((t, sim.view().economy(PlayerId(0)).stock.clone()));
        }
        sim.step(&[]);
    }
    for (t, stock) in &at {
        for r in 0..start.len() {
            assert_eq!(
                stock[r],
                start[r] + yields[r] * i64::from(*t) / i64::from(period),
                "resource {r} at tick {t}"
            );
        }
    }
    let per = |t: u32| at.iter().find(|(x, _)| *x == t).unwrap().1.clone();
    for phase in [0, 50, 300] {
        let d: Vec<i64> = per(phase + period)
            .iter()
            .zip(per(phase))
            .map(|(a, b)| a - b)
            .collect();
        assert_eq!(d, yields, "one period from tick {phase}");
    }
}

/// Every `Build` rejection reason, each from a fresh standard start, and
/// each leaving the RNG, the stockpile and the building count unchanged.
#[test]
fn economy_build_rejections() {
    let rules = load_rules();
    let town = kind(&rules, "town");
    let croft = kind(&rules, "croft");
    let ore = u8::try_from(resource(&rules, "ore")).unwrap();
    let place = |seq: u32, k: BuildingKindId, x: u16, y: u16| {
        PlayerCommand::new(
            PlayerId(0),
            seq,
            Command::DebugPlaceBuilding {
                owner: PlayerId(0),
                kind: k,
                tile: Tile::new(x, y),
                complete: false,
            },
        )
    };
    type Setup = Box<dyn Fn(&mut Sim)>;
    let no_setup: Setup = Box::new(|_| {});
    let cases: Vec<(&str, Setup, PlayerCommand, RejectReason)> = vec![
        (
            "unknown unit",
            no_setup,
            build(0, 0, 999, croft, 28, 60),
            RejectReason::NoValidUnits,
        ),
        (
            "another player's unit",
            Box::new(|_| {}),
            build(0, 0, 6, croft, 28, 60),
            RejectReason::NoValidUnits,
        ),
        (
            "unknown kind",
            Box::new(|_| {}),
            build(0, 0, 1, BuildingKindId(999), 28, 60),
            RejectReason::UnknownBuildingKind,
        ),
        (
            "not a builder",
            Box::new(|s: &mut Sim| {
                apply(
                    s,
                    vec![PlayerCommand::new(
                        PlayerId(0),
                        100,
                        Command::DebugSpawn {
                            owner: PlayerId(0),
                            kind: UnitKindId(1),
                            at: FxVec2::from_ints(30, 70),
                            count: 1,
                        },
                    )],
                );
            }),
            build(0, 0, 11, croft, 28, 60),
            RejectReason::NotABuilder,
        ),
        (
            "off the map",
            Box::new(|_| {}),
            build(0, 0, 1, croft, 127, 127),
            RejectReason::SiteOutOfBounds,
        ),
        (
            "outside borders",
            Box::new(|_| {}),
            build(0, 0, 1, croft, 60, 64),
            RejectReason::OutsideBorders,
        ),
        (
            "on the Seat",
            Box::new(|_| {}),
            build(0, 0, 1, croft, 24, 64),
            RejectReason::SiteBlocked,
        ),
        (
            "on forest",
            Box::new(|_| {}),
            build(0, 0, 1, croft, 14, 50),
            RejectReason::SiteBlocked,
        ),
        (
            "on a Yeoman",
            Box::new(|_| {}),
            build(0, 0, 2, croft, 26, 64),
            RejectReason::SiteOccupied,
        ),
        (
            "third Town",
            Box::new(move |s: &mut Sim| {
                apply(s, vec![build(0, 100, 1, town, 40, 63)]);
            }),
            build(0, 0, 2, town, 30, 70),
            RejectReason::TownLimitReached,
        ),
        (
            "sixth Croft at the Seat",
            Box::new(move |s: &mut Sim| {
                let crofts = (0..5u16).map(|i| place(100 + u32::from(i), croft, 28 + 3 * i, 58));
                apply(s, crofts.collect());
            }),
            build(0, 0, 1, croft, 28, 60),
            RejectReason::PerTownLimitReached,
        ),
        (
            "Lumber Yard with no forest beside it",
            Box::new(|_| {}),
            build(0, 0, 1, kind(&rules, "lumber_yard"), 28, 60),
            RejectReason::NoAdjacentResource,
        ),
        (
            "Watchtower without Ore",
            Box::new(|_| {}),
            build(0, 0, 1, kind(&rules, "watchtower"), 28, 60),
            RejectReason::CannotAfford { resource: ore },
        ),
    ];
    for (name, setup, cmd, want) in cases {
        // The control sim gets a command at the same (player, seq) that is
        // rejected for an unrelated reason; a rejection that changed any
        // state (a job, an order, the map, a stockpile) or drew randomness
        // would show up as a differing sub-hash.
        let mut sim = standard(7);
        let mut control = standard(7);
        setup(&mut sim);
        setup(&mut control);
        let rng = sim.sub_hashes().rng;
        let stop = PlayerCommand::new(
            cmd.player,
            cmd.seq,
            Command::Stop {
                units: vec![UnitId(999)],
            },
        );
        let events = apply(&mut sim, vec![cmd]);
        let control_events = apply(&mut control, vec![stop]);
        assert_eq!(rejections(&events), [want], "{name}");
        assert_eq!(rejections(&control_events), [RejectReason::NoValidUnits]);
        assert_eq!(
            sim.sub_hashes(),
            control.sub_hashes(),
            "{name}: no state change"
        );
        assert_eq!(sim.hash(), control.hash(), "{name}");
        assert_eq!(sim.sub_hashes().rng, rng, "{name}: no RNG draw");
    }
}

/// A Croft needs a complete Town to belong to: with an empty start and only
/// borders from two debug Watchtowers, `Build` is `PerTownLimitReached`.
#[test]
fn economy_croft_without_a_town_is_rejected() {
    let rules = load_rules();
    let mut setup = MatchSetup::scenario(&rules, 8);
    setup.start = StartKind::Empty;
    let mut sim = Sim::new(setup, load_rules(), Box::new(NoAi));
    let lumber = u8::try_from(resource(&rules, "lumber")).unwrap();
    apply(
        &mut sim,
        vec![
            PlayerCommand::new(
                PlayerId(0),
                0,
                Command::DebugPlaceBuilding {
                    owner: PlayerId(0),
                    kind: kind(&rules, "watchtower"),
                    tile: Tile::new(30, 64),
                    complete: true,
                },
            ),
            PlayerCommand::new(
                PlayerId(0),
                1,
                Command::DebugPlaceBuilding {
                    owner: PlayerId(0),
                    kind: kind(&rules, "watchtower"),
                    tile: Tile::new(33, 64),
                    complete: true,
                },
            ),
            PlayerCommand::new(
                PlayerId(0),
                2,
                Command::DebugSpawn {
                    owner: PlayerId(0),
                    kind: UnitKindId(0),
                    at: FxVec2::from_ints(33, 67),
                    count: 1,
                },
            ),
            PlayerCommand::new(
                PlayerId(0),
                3,
                Command::DebugGrant {
                    owner: PlayerId(0),
                    resource: lumber,
                    amount: 500,
                },
            ),
        ],
    );
    // The two 3x3 blocks touch: (31, 63)..(32, 64) is all P0's and free.
    for t in [(31, 63), (32, 63), (31, 64), (32, 64)] {
        assert!(
            sim.view()
                .territory()
                .is_buildable(PlayerId(0), Tile::new(t.0, t.1))
        );
    }
    let events = apply(
        &mut sim,
        vec![build(0, 4, 1, kind(&rules, "croft"), 31, 63)],
    );
    assert_eq!(rejections(&events), [RejectReason::PerTownLimitReached]);
}

/// Cancelling a site refunds exactly what was paid, unblocks the footprint
/// (one generation bump each way) and idles the builder; the economy is
/// then identical to a sim that never built.
#[test]
fn economy_site_cancel_refunds_and_unblocks() {
    let rules = load_rules();
    let croft = kind(&rules, "croft");
    let mut sim = standard(9);
    let mut control = standard(9);
    let g0 = sim.view().map().cost_grid_generation();
    apply(&mut sim, vec![build(0, 0, 1, croft, 28, 60)]);
    apply(&mut control, vec![]);
    let site = BuildingId(3);
    assert_eq!(sim.view().map().cost_grid_generation(), g0 + 1);
    assert!(!sim.view().map().passable(Tile::new(29, 61)));
    assert_ne!(sim.sub_hashes().economy, control.sub_hashes().economy);
    let events = apply(
        &mut sim,
        vec![PlayerCommand::new(
            PlayerId(0),
            1,
            Command::Cancel {
                building: site,
                slot: 0,
            },
        )],
    );
    apply(&mut control, vec![]);
    assert!(events.contains(&SimEvent::BuildingRemoved { building: site }));
    assert!(sim.view().building(site).is_none());
    assert_eq!(sim.view().map().cost_grid_generation(), g0 + 2);
    assert!(sim.view().map().passable(Tile::new(29, 61)));
    assert_eq!(sim.sub_hashes().economy, control.sub_hashes().economy);
    assert!(sim.view().unit(UnitId(1)).unwrap().job.is_none());

    // Slot 1 of a site, a complete building's empty queue and another
    // player's building are rejected.
    for (seq, building, slot, want) in [
        (2, BuildingId(1), 0, RejectReason::InvalidSlot),
        (3, BuildingId(2), 0, RejectReason::UnknownBuilding),
        (4, site, 0, RejectReason::UnknownBuilding),
    ] {
        let events = apply(
            &mut sim,
            vec![PlayerCommand::new(
                PlayerId(0),
                seq,
                Command::Cancel { building, slot },
            )],
        );
        assert_eq!(rejections(&events), [want], "{building:?} slot {slot}");
    }
    apply(&mut sim, vec![build(0, 5, 1, croft, 28, 60)]);
    let events = apply(
        &mut sim,
        vec![PlayerCommand::new(
            PlayerId(0),
            6,
            Command::Cancel {
                building: BuildingId(4),
                slot: 1,
            },
        )],
    );
    assert_eq!(rejections(&events), [RejectReason::InvalidSlot]);
}

/// A site whose builder was stopped is resumed by another Yeoman issuing
/// the same `Build`: nothing is charged twice, and the site completes.
#[test]
fn economy_builder_can_resume_a_site() {
    let rules = load_rules();
    let croft = kind(&rules, "croft");
    let mut sim = standard(10);
    apply(&mut sim, vec![build(0, 0, 1, croft, 28, 60)]);
    let site = BuildingId(3);
    apply(
        &mut sim,
        vec![PlayerCommand::new(
            PlayerId(0),
            1,
            Command::Stop {
                units: vec![UnitId(1)],
            },
        )],
    );
    assert!(sim.view().unit(UnitId(1)).unwrap().job.is_none());
    for _ in 0..200 {
        sim.step(&[]);
    }
    let Construction::Site { progress } = sim.view().building(site).unwrap().construction else {
        panic!("still a site");
    };
    assert_eq!(progress, 0, "no builder, no progress");
    let lumber = resource(&rules, "lumber");
    let before = sim.view().economy(PlayerId(0)).stock[lumber];
    let events = apply(&mut sim, vec![build(0, 2, 2, croft, 28, 60)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SimEvent::BuildingPlaced { .. }))
    );
    assert!(
        sim.view().economy(PlayerId(0)).stock[lumber] >= before,
        "no charge"
    );
    assert_eq!(
        sim.view().unit(UnitId(2)).unwrap().job,
        Some(Job::Build { site })
    );
    let ticks = rules.build_time_ticks(rules.building_kind(croft.0).unwrap());
    assert!(run_until(&mut sim, ticks + 400, |s| {
        s.view().building(site).unwrap().construction == Construction::Complete
    }));
}

/// Snapshot mid-construction, restore into a fresh sim, keep issuing
/// commands: every later tick hashes the same as the uninterrupted run.
#[test]
fn economy_restore_mid_construction_matches_uninterrupted() {
    let rules = load_rules();
    let town = kind(&rules, "town");
    let croft = kind(&rules, "croft");
    let mut a = standard(11);
    a.step(&[build(0, 0, 1, town, 40, 63)]);
    for _ in 1..600 {
        a.step(&[]);
    }
    assert!(matches!(
        a.view().building(BuildingId(3)).unwrap().construction,
        Construction::Site { progress } if progress > 0
    ));
    let mut b = standard(11);
    b.restore(&a.snapshot()).unwrap();
    assert_eq!(a.hash(), b.hash());
    for t in 600..2000u32 {
        let cmds = if t == 700 {
            vec![build(0, 1, 2, croft, 28, 60)]
        } else {
            Vec::new()
        };
        a.step(&cmds);
        b.step(&cmds);
        assert_eq!(a.hash(), b.hash(), "tick {t}");
    }
    assert_eq!(
        a.view().building(BuildingId(3)).unwrap().construction,
        Construction::Complete
    );
}

// ---------------------------------------------------------------------------
// The economy fixture
// ---------------------------------------------------------------------------

/// The M3a economy fixture: from the standard start, each player's fixed
/// build order founds a second Town that is complete, with its border, at
/// a tick below 3600; every command is accepted, and the owned tile counts
/// grow and stay mirrored.
#[test]
fn economy_second_town_completes_before_tick_3600() {
    let rules = load_rules();
    let (setup, stream) = sim::scenarios::economy_second_town(&rules);
    assert_eq!(setup.start, StartKind::Standard);
    let ticks = sim::scenarios::ECONOMY_SECOND_TOWN_TICKS;
    assert_eq!(ticks, 3600);
    let start_owned = 1245;
    let mut sim = Sim::new(setup, load_rules(), Box::new(NoAi));
    let mut founded = None;
    let mut events = Vec::new();
    for t in 0..ticks {
        sim.step(sim::scenarios::commands_at(&stream, t));
        events.extend(sim.drain_events());
        let both = [PlayerId(0), PlayerId(1)].iter().all(|&p| {
            sim.view()
                .buildings()
                .filter(|b| {
                    b.owner == p
                        && b.kind.0 == rules.town_kind()
                        && b.construction == Construction::Complete
                })
                .count()
                == 2
        });
        if both && founded.is_none() {
            founded = Some(sim.tick());
        }
    }
    let founded = founded.expect("both players found a second Town");
    assert!(founded < 3600, "second Towns complete at tick {founded}");
    assert!(rejections(&events).is_empty(), "{:?}", rejections(&events));
    let placed = events
        .iter()
        .filter(|e| matches!(e, SimEvent::BuildingPlaced { .. }))
        .count();
    assert_eq!(placed, 6, "two Towns and four Crofts");
    let field = sim.view().territory();
    let (p0, p1) = (
        field.owned_tile_count(PlayerId(0)),
        field.owned_tile_count(PlayerId(1)),
    );
    assert_eq!(p0, p1, "mirrored build orders own mirrored territory");
    if town_radii(&rules).0 == 20 {
        assert!(p0 > start_owned, "{p0}");
    }
    // The Crofts placed at tick 2400 belong to the new Towns.
    let new_town = |p: u8| {
        sim.view()
            .buildings()
            .find(|b| {
                b.owner == PlayerId(p)
                    && b.kind.0 == rules.town_kind()
                    && !b.town_state.unwrap().seat
            })
            .unwrap()
            .id
    };
    for p in [0u8, 1] {
        let late = sim
            .view()
            .buildings()
            .filter(|b| b.owner == PlayerId(p) && b.kind == kind(&rules, "croft"))
            .max_by_key(|b| b.id)
            .unwrap();
        assert_eq!(late.town, Some(new_town(p)), "player {p}");
    }
}

#[test]
fn golden_economy_second_town() {
    golden("economy_second_town");
}

/// A builder walks to a tile beside the site that it can reach, not merely
/// the nearest passable one: with the nearest ring tiles sealed in a pocket,
/// it goes round and the site completes. A unit sealed in that pocket can
/// still resume the site from inside, but cannot build anywhere else
/// (`Unreachable`). Resuming a site whose ring is fully blocked is
/// `SiteBlocked`, never a panic, and `check_build` agrees.
#[test]
fn economy_builder_stands_where_it_can_reach() {
    let rules = load_rules();
    let (croft, tower) = (kind(&rules, "croft"), kind(&rules, "watchtower"));
    let place_site = |seq: u32, k: BuildingKindId, x: u16, y: u16| {
        PlayerCommand::new(
            PlayerId(0),
            seq,
            Command::DebugPlaceBuilding {
                owner: PlayerId(0),
                kind: k,
                tile: Tile::new(x, y),
                complete: false,
            },
        )
    };
    let mut sim = standard(40);
    // A wall seals (30, 59) and (31, 59) between row 58 and a Croft site at
    // (30, 60), whose other ring tiles west and east are blocked too.
    let wall = [
        (29, 58),
        (30, 58),
        (31, 58),
        (32, 58),
        (29, 59),
        (32, 59),
        (29, 60),
        (29, 61),
        (32, 60),
        (32, 61),
    ];
    let mut cmds = vec![place_site(0, croft, 30, 60)];
    for (i, (x, y)) in wall.iter().enumerate() {
        cmds.push(place_site(1 + u32::try_from(i).unwrap(), tower, *x, *y));
    }
    let events = apply(&mut sim, cmds);
    assert!(rejections(&events).is_empty(), "{events:?}");
    let site = BuildingId(3);
    // UnitId(11) outside to the north, UnitId(12) inside the pocket.
    let events = apply(
        &mut sim,
        vec![
            spawn_kind(0, 20, 0, 31, 56, 1),
            spawn_kind(0, 21, 0, 30, 59, 1),
        ],
    );
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(
        sim.view()
            .map()
            .tile_of(sim.view().unit(UnitId(12)).unwrap().pos),
        Tile::new(30, 59)
    );
    // The pocket unit cannot reach any other site's ring.
    assert_eq!(
        sim.view()
            .check_build(PlayerId(0), UnitId(12), croft, Tile::new(36, 64)),
        Err(RejectReason::Unreachable)
    );
    let events = apply(&mut sim, vec![build(0, 22, 12, croft, 36, 64)]);
    assert_eq!(rejections(&events), [RejectReason::Unreachable]);
    // The northern unit resumes the site and walks round to the south side.
    let events = apply(&mut sim, vec![build(0, 23, 11, croft, 30, 60)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    let goal = sim
        .view()
        .unit(UnitId(11))
        .unwrap()
        .order
        .clone()
        .unwrap()
        .goal;
    let stand = sim.view().map().tile_of(goal);
    assert_eq!(
        stand.y, 62,
        "a reachable south ring tile, not the pocket: {stand:?}"
    );
    let ticks = rules.build_time_ticks(rules.building_kind(croft.0).unwrap());
    assert!(run_until(&mut sim, ticks + 800, |s| {
        s.view().building(site).unwrap().construction == Construction::Complete
    }));

    // Fully enclosed: block the south ring row too; a resume is SiteBlocked.
    let mut sim = standard(41);
    let mut cmds = vec![place_site(0, croft, 30, 60)];
    let ring = sim::construction::ring(
        sim::construction::Rect {
            x0: 30,
            y0: 60,
            x1: 31,
            y1: 61,
        },
        128,
        128,
    );
    for (i, t) in ring.iter().enumerate() {
        cmds.push(place_site(1 + u32::try_from(i).unwrap(), tower, t.x, t.y));
    }
    let events = apply(&mut sim, cmds);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(
        sim.view()
            .check_build(PlayerId(0), UnitId(1), croft, Tile::new(30, 60)),
        Err(RejectReason::SiteBlocked)
    );
    let events = apply(&mut sim, vec![build(0, 40, 1, croft, 30, 60)]);
    assert_eq!(rejections(&events), [RejectReason::SiteBlocked]);
}

/// Resuming a site with a second Yeoman hands the site over: the first
/// builder goes idle and nothing is charged.
#[test]
fn economy_resume_hands_the_site_to_the_new_builder() {
    let rules = load_rules();
    let croft = kind(&rules, "croft");
    let mut sim = standard(42);
    apply(&mut sim, vec![build(0, 0, 1, croft, 28, 60)]);
    let site = BuildingId(3);
    assert_eq!(
        sim.view().unit(UnitId(1)).unwrap().job,
        Some(Job::Build { site })
    );
    let lumber = resource(&rules, "lumber");
    let before = stock(&sim, 0, lumber);
    let events = apply(&mut sim, vec![build(0, 1, 2, croft, 28, 60)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert!(sim.view().unit(UnitId(1)).unwrap().job.is_none());
    assert_eq!(
        sim.view().unit(UnitId(2)).unwrap().job,
        Some(Job::Build { site })
    );
    assert!(stock(&sim, 0, lumber) >= before, "no second charge");
}

/// Standard-start units never share a tile, across kinds and players.
#[test]
fn economy_standard_start_units_never_share_a_tile() {
    let mut rules = load_rules();
    rules.standard_start.units.insert("scribe".into(), 3);
    let setup = {
        let mut s = MatchSetup::scenario(&rules, 43);
        s.start = StartKind::Standard;
        s
    };
    let sim = Sim::new(setup, rules, Box::new(NoAi));
    let map = sim.view().map();
    let tiles: Vec<Tile> = sim.view().units().map(|u| map.tile_of(u.pos)).collect();
    let mut unique = tiles.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), tiles.len(), "{tiles:?}");
    assert_eq!(tiles.len(), 16);
    assert!(tiles.iter().all(|t| map.passable(*t)));
}

// ---------------------------------------------------------------------------
// Gathering, the Yield Cap, Lore, idle seeking and Town growth
// ---------------------------------------------------------------------------

/// A two-player scenario (debug commands on) with an empty start.
fn empty(seed: u64) -> Sim {
    let rules = load_rules();
    let setup = MatchSetup::scenario(&rules, seed);
    assert_eq!(setup.start, StartKind::Empty);
    Sim::new(setup, rules, Box::new(NoAi))
}

fn place(p: u8, seq: u32, kind: BuildingKindId, x: u16, y: u16, complete: bool) -> PlayerCommand {
    PlayerCommand::new(
        PlayerId(p),
        seq,
        Command::DebugPlaceBuilding {
            owner: PlayerId(p),
            kind,
            tile: Tile::new(x, y),
            complete,
        },
    )
}

fn gather(p: u8, seq: u32, units: &[u32], node: u32) -> PlayerCommand {
    PlayerCommand::new(
        PlayerId(p),
        seq,
        Command::Gather {
            units: units.iter().copied().map(UnitId).collect(),
            node: BuildingId(node),
        },
    )
}

fn unit_kind(rules: &Rules, id: &str) -> u16 {
    u16::try_from(
        rules
            .units
            .iter()
            .position(|u| u.id == id)
            .expect("unit kind"),
    )
    .unwrap()
}

/// Total worked slots over `nodes`.
fn worked(sim: &Sim, nodes: impl IntoIterator<Item = u32>) -> u32 {
    nodes
        .into_iter()
        .map(|n| u32::from(sim.view().worked_slots(BuildingId(n)).unwrap().0))
        .sum()
}

/// M3a acceptance 1a: one Yeoman working a Croft adds exactly the Croft's
/// `rate_per_worker` (from `buildings.ron`) every income period, for three
/// consecutive periods, from whatever phase the work started in.
#[test]
fn economy_one_yeoman_on_a_croft_yields_exactly_the_croft_rate_per_income_period() {
    let rules = load_rules();
    let croft = kind(&rules, "croft");
    let rate_u32 = rules
        .building_kind(croft.0)
        .unwrap()
        .gather
        .as_ref()
        .unwrap()
        .rate_per_worker;
    let rate = i64::from(rate_u32);
    let grain = resource(&rules, "grain");
    let period = rules.income_period_ticks();
    let mut sim = empty(20);
    apply(
        &mut sim,
        vec![
            place(0, 0, croft, 30, 60, true),
            spawn_kind(0, 1, 0, 30, 66, 1),
        ],
    );
    let events = apply(&mut sim, vec![gather(0, 2, &[1], 1)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    // While the Yeoman walks to its stand tile the slot is taken but not
    // worked, and earns nothing.
    let mut walked = 0;
    while worked(&sim, [1]) == 0 {
        assert_eq!(sim.view().worked_slots(BuildingId(1)), Some((0, 1, 1)));
        assert_eq!(sim.view().income(PlayerId(0))[grain].raw, 0);
        assert_eq!(stock(&sim, 0, grain), 0);
        sim.step(&[]);
        walked += 1;
        assert!(walked < 600, "the Yeoman reaches its slot");
    }
    assert!(walked > 0, "the Yeoman had to walk");
    let info = sim.view().income(PlayerId(0))[grain];
    assert_eq!((info.raw, info.credited), (rate_u32, rate_u32));
    for _ in 0..3 {
        let before = stock(&sim, 0, grain);
        for _ in 0..period {
            sim.step(&[]);
        }
        assert_eq!(stock(&sim, 0, grain) - before, rate);
    }
}

/// M3a acceptance 1b: with 10 Crofts worked (two complete Towns host them,
/// placed by `DebugPlaceBuilding`, owner decision D2) the raw Grain rate is
/// above the Yield Cap, the credited rate is the cap, and the stockpile
/// grows by exactly the cap per income period.
#[test]
fn economy_ten_worked_crofts_flatline_at_the_yield_cap() {
    let rules = load_rules();
    let (town, croft) = (kind(&rules, "town"), kind(&rules, "croft"));
    let grain = resource(&rules, "grain");
    let rate = rules
        .building_kind(croft.0)
        .unwrap()
        .gather
        .as_ref()
        .unwrap()
        .rate_per_worker;
    let town_grain = rules.building_kind(town.0).unwrap().base_yield["grain"];
    let cap = rules.yield_cap_base;
    let period = rules.income_period_ticks();
    let mut sim = empty(21);
    let mut cmds = vec![
        place(0, 0, town, 23, 63, true),
        place(0, 1, town, 40, 63, true),
    ];
    let sites: Vec<(u16, u16)> = [28, 31, 34, 37, 40, 43]
        .map(|x| (x, 58))
        .into_iter()
        .chain([28, 31, 34, 37].map(|x| (x, 60)))
        .collect();
    for (i, (x, y)) in sites.iter().enumerate() {
        cmds.push(place(0, 2 + u32::try_from(i).unwrap(), croft, *x, *y, true));
    }
    cmds.push(spawn_kind(0, 20, 0, 32, 67, 10));
    apply(&mut sim, cmds);
    // Towns are BuildingId(1) and (2); Crofts (3..=12); Yeomen UnitId(1..=10).
    let gathers = (0..10u32)
        .map(|i| gather(0, 21 + i, &[1 + i], 3 + i))
        .collect();
    let events = apply(&mut sim, gathers);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert!(run_until(&mut sim, 2000, |s| worked(s, 3..=12) == 10));
    let info = sim.view().income(PlayerId(0))[grain];
    assert_eq!(info.raw, 10 * rate + 2 * town_grain);
    assert!(info.raw > cap, "the setup must exceed the cap");
    assert_eq!((info.cap, info.credited), (Some(cap), cap));
    for _ in 0..3 {
        let before = stock(&sim, 0, grain);
        for _ in 0..period {
            sim.step(&[]);
        }
        assert_eq!(stock(&sim, 0, grain) - before, i64::from(cap));
    }
}

/// M3a acceptance 1c: Lore is exempt from the Yield Cap. Three Scriptoria
/// with three Scribes each produce more than the cap, and all of it is
/// credited (the smallest such count is derived from the data).
#[test]
fn economy_lore_ignores_the_yield_cap() {
    let rules = load_rules();
    let scriptorium = kind(&rules, "scriptorium");
    let def = rules.building_kind(scriptorium.0).unwrap();
    let g = def.gather.as_ref().unwrap();
    let rules::Slots::Fixed(slots) = g.slots else {
        panic!("Scriptorium slots are fixed");
    };
    let per = def.base_yield["lore"] + slots * g.rate_per_worker;
    let cap = rules.yield_cap_base;
    let n = cap / per + 1;
    assert!(n * per > cap);
    assert!(n <= 3, "the test places at most 3 Scriptoria");
    let lore = resource(&rules, "lore");
    let period = rules.income_period_ticks();
    let mut sim = empty(22);
    let mut cmds: Vec<PlayerCommand> = [(28u16, 59u16), (32, 59), (36, 59)]
        .iter()
        .take(usize::try_from(n).unwrap())
        .enumerate()
        .map(|(i, (x, y))| place(0, u32::try_from(i).unwrap(), scriptorium, *x, *y, true))
        .collect();
    let scribes = u16::try_from(n * slots).unwrap();
    cmds.push(spawn_kind(
        0,
        10,
        unit_kind(&rules, "scribe"),
        32,
        66,
        scribes,
    ));
    apply(&mut sim, cmds);
    let gathers: Vec<PlayerCommand> = (0..n)
        .map(|b| {
            let units: Vec<u32> = (0..slots).map(|k| 1 + b * slots + k).collect();
            gather(0, 11 + b, &units, 1 + b)
        })
        .collect();
    let events = apply(&mut sim, gathers);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert!(run_until(&mut sim, 2000, |s| worked(s, 1..=n) == n * slots));
    let info = sim.view().income(PlayerId(0))[lore];
    assert_eq!(info.cap, None, "Lore is not capped");
    assert_eq!((info.raw, info.credited), (n * per, n * per));
    let before = stock(&sim, 0, lore);
    for _ in 0..period {
        sim.step(&[]);
    }
    assert_eq!(stock(&sim, 0, lore) - before, i64::from(n * per));
}

/// Lore stops at its `stockpile_cap`, whether it comes from income or a
/// grant, and never goes past it.
#[test]
fn economy_lore_stockpile_stops_at_its_cap() {
    let rules = load_rules();
    let lore = resource(&rules, "lore");
    let cap = i64::from(rules.resources.resources[lore].stockpile_cap.unwrap());
    let mut sim = empty(23);
    apply(
        &mut sim,
        vec![
            place(0, 0, kind(&rules, "scriptorium"), 30, 59, true),
            PlayerCommand::new(
                PlayerId(0),
                1,
                Command::DebugGrant {
                    owner: PlayerId(0),
                    resource: u8::try_from(lore).unwrap(),
                    amount: u32::try_from(cap - 9).unwrap(),
                },
            ),
        ],
    );
    let mut reached = false;
    for _ in 0..(rules.income_period_ticks() * 2) {
        sim.step(&[]);
        let s = stock(&sim, 0, lore);
        assert!(s <= cap, "{s} > {cap}");
        reached |= s == cap;
    }
    assert!(reached, "the Scriptorium's base Lore fills the stockpile");
    let events = apply(
        &mut sim,
        vec![PlayerCommand::new(
            PlayerId(0),
            2,
            Command::DebugGrant {
                owner: PlayerId(0),
                resource: u8::try_from(lore).unwrap(),
                amount: 500,
            },
        )],
    );
    assert!(rejections(&events).is_empty());
    assert_eq!(stock(&sim, 0, lore), cap);
}

/// A Yeoman with no job and no order takes a free slot on its own exactly
/// `idle_seek_delay` ticks after it became idle, not one tick earlier.
#[test]
fn economy_idle_yeoman_seeks_work_after_the_delay() {
    let rules = load_rules();
    let mut sim = empty(24);
    apply(
        &mut sim,
        vec![
            place(0, 0, kind(&rules, "croft"), 30, 60, true),
            spawn_kind(0, 1, 0, 30, 66, 1),
        ],
    );
    let u = sim.view().unit(UnitId(1)).unwrap();
    let at = u.seek_at.expect("an idle Yeoman starts its timer");
    let spawned = sim.tick() - 1;
    assert_eq!(at, spawned + rules.idle_seek_delay_ticks());
    assert!(run_until(&mut sim, at + 1, |s| s.tick() == at));
    assert!(
        sim.view().unit(UnitId(1)).unwrap().job.is_none(),
        "not before the delay"
    );
    sim.step(&[]);
    let u = sim.view().unit(UnitId(1)).unwrap();
    assert_eq!(
        u.job,
        Some(Job::Gather {
            node: BuildingId(1),
            slot: 0
        })
    );
    assert_eq!(u.seek_at, None);
    assert!(u.order.is_some(), "it walks to the slot's stand tile");
    assert!(run_until(&mut sim, 600, |s| worked(s, [1]) == 1));
}

/// With nothing to work, an idle Yeoman retries every `idle_seek_retry`
/// ticks, and takes a slot at the first retry after one appears.
#[test]
fn economy_idle_seek_retries_on_the_retry_interval() {
    let rules = load_rules();
    let mut sim = empty(25);
    apply(&mut sim, vec![spawn_kind(0, 0, 0, 30, 66, 1)]);
    let first = sim.view().unit(UnitId(1)).unwrap().seek_at.unwrap();
    assert!(run_until(&mut sim, first + 2, |s| s.tick() == first + 1));
    let second = sim.view().unit(UnitId(1)).unwrap().seek_at.unwrap();
    assert_eq!(second, first + rules.idle_seek_retry_ticks());
    assert!(sim.view().unit(UnitId(1)).unwrap().job.is_none());
    apply(
        &mut sim,
        vec![place(0, 1, kind(&rules, "croft"), 30, 60, true)],
    );
    assert!(sim.tick() < second);
    assert!(run_until(&mut sim, second + 1, |s| s.tick() == second));
    assert!(sim.view().unit(UnitId(1)).unwrap().job.is_none());
    sim.step(&[]);
    assert!(matches!(
        sim.view().unit(UnitId(1)).unwrap().job,
        Some(Job::Gather { .. })
    ));
}

/// Idle seeking picks the nearest free building by squared distance to its
/// footprint centre, and the lower `BuildingId` on an exact tie.
#[test]
fn economy_idle_seek_orders_by_distance_then_building_id() {
    let rules = load_rules();
    let croft = kind(&rules, "croft");
    // Equidistant by footprint centre: Crofts at x 27 (id 1) and 32 (id 2)
    // around a Yeoman at x 30.5; the lower id wins. Measured to the NW tile
    // instead, id 2 would be nearer, so this also pins the metric.
    let mut sim = empty(26);
    apply(
        &mut sim,
        vec![
            place(0, 0, croft, 27, 62, true),
            place(0, 1, croft, 32, 62, true),
            spawn_kind(0, 2, 0, 30, 66, 1),
        ],
    );
    let c1 = sim.view().building(BuildingId(1)).unwrap().tile;
    let c2 = sim.view().building(BuildingId(2)).unwrap().tile;
    // Centres (in half tiles) 2 * x + 2 sit 5 half tiles either side of
    // the Yeoman's 61.
    let half = |x: u16| 2 * i32::from(x) + 2;
    assert_eq!(61 - half(c1.x), half(c2.x) - 61);
    assert!((61 - (2 * i32::from(c1.x) + 1)) > (2 * i32::from(c2.x) + 1) - 61);
    let at = sim.view().unit(UnitId(1)).unwrap().seek_at.unwrap();
    assert!(run_until(&mut sim, at + 2, |s| s.tick() == at + 1));
    assert!(matches!(
        sim.view().unit(UnitId(1)).unwrap().job,
        Some(Job::Gather {
            node: BuildingId(1),
            ..
        })
    ));
    // Nearer beats a lower id.
    let mut sim = empty(27);
    apply(
        &mut sim,
        vec![
            place(0, 0, croft, 40, 62, true),
            place(0, 1, croft, 31, 62, true),
            spawn_kind(0, 2, 0, 30, 66, 1),
        ],
    );
    let at = sim.view().unit(UnitId(1)).unwrap().seek_at.unwrap();
    assert!(run_until(&mut sim, at + 2, |s| s.tick() == at + 1));
    assert!(matches!(
        sim.view().unit(UnitId(1)).unwrap().job,
        Some(Job::Gather {
            node: BuildingId(2),
            ..
        })
    ));
}

/// Every `Gather` rejection reason, each leaving the RNG and every job
/// unchanged.
#[test]
fn economy_gather_rejections() {
    let rules = load_rules();
    let (croft, town) = (kind(&rules, "croft"), kind(&rules, "town"));
    let scribe = unit_kind(&rules, "scribe");
    let mut sim = empty(28);
    apply(
        &mut sim,
        vec![
            place(0, 0, croft, 30, 60, true),
            place(0, 1, croft, 34, 60, false),
            place(0, 2, town, 40, 63, true),
            place(0, 3, kind(&rules, "scriptorium"), 34, 70, true),
            place(1, 0, croft, 90, 60, true),
            spawn_kind(0, 4, 0, 30, 66, 2),
            spawn_kind(0, 5, scribe, 36, 68, 1),
        ],
    );
    assert_eq!(sim.view().buildings().count(), 5, "every placement landed");
    // Croft 6 at (50, 64) is sealed: a Watchtower site on every ring tile.
    let ring = sim::construction::ring(
        sim::construction::Rect {
            x0: 50,
            y0: 64,
            x1: 51,
            y1: 65,
        },
        128,
        128,
    );
    let mut seal = vec![place(0, 20, croft, 50, 64, true)];
    for (i, t) in ring.iter().enumerate() {
        seal.push(place(
            0,
            21 + u32::try_from(i).unwrap(),
            kind(&rules, "watchtower"),
            t.x,
            t.y,
            false,
        ));
    }
    let events = apply(&mut sim, seal);
    assert!(rejections(&events).is_empty(), "{events:?}");
    // Croft 1 takes Yeoman 1.
    apply(&mut sim, vec![gather(0, 40, &[1], 1)]);
    let cases = [
        (gather(0, 7, &[2], 1), RejectReason::NoFreeSlot),
        (gather(0, 8, &[2], 2), RejectReason::UnderConstruction),
        (gather(0, 9, &[2], 3), RejectReason::NotAGatherNode),
        (gather(0, 10, &[2], 5), RejectReason::UnknownBuilding),
        (gather(0, 11, &[2], 99), RejectReason::UnknownBuilding),
        (gather(0, 12, &[2], 4), RejectReason::NoValidUnits),
        (gather(0, 13, &[3], 1), RejectReason::NoValidUnits),
        (gather(0, 14, &[999], 1), RejectReason::NoValidUnits),
        // No tile beside the sealed Croft can be reached.
        (gather(0, 15, &[2], 6), RejectReason::NoValidUnits),
    ];
    for (cmd, want) in cases {
        let rng = sim.sub_hashes().rng;
        let jobs: Vec<_> = sim.view().units().map(|u| u.job).collect();
        let events = apply(&mut sim, vec![cmd.clone()]);
        assert_eq!(rejections(&events), [want], "{cmd:?}");
        assert_eq!(sim.sub_hashes().rng, rng);
        let after: Vec<_> = sim.view().units().map(|u| u.job).collect();
        assert_eq!(jobs, after, "no job changes");
    }
    // Re-issuing the order to the full Croft's own worker is accepted and
    // changes nothing, exactly as on a node with free slots.
    let u = sim.view().unit(UnitId(1)).unwrap();
    let before = (u.job, u.order.clone(), u.post);
    let events = apply(&mut sim, vec![gather(0, 16, &[1], 1)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    let u = sim.view().unit(UnitId(1)).unwrap();
    assert_eq!((u.job, u.order.clone(), u.post), before);
}

/// An idle Yeoman whose only Croft is sealed (no ring tile reachable) keeps
/// retrying and never takes the slot: no panic, no job.
#[test]
fn economy_idle_seek_skips_a_sealed_croft() {
    let rules = load_rules();
    let croft = kind(&rules, "croft");
    let mut sim = empty(35);
    let ring = sim::construction::ring(
        sim::construction::Rect {
            x0: 50,
            y0: 64,
            x1: 51,
            y1: 65,
        },
        128,
        128,
    );
    let mut cmds = vec![place(0, 0, croft, 50, 64, true)];
    for (i, t) in ring.iter().enumerate() {
        cmds.push(place(
            0,
            1 + u32::try_from(i).unwrap(),
            kind(&rules, "watchtower"),
            t.x,
            t.y,
            false,
        ));
    }
    cmds.push(spawn_kind(0, 30, 0, 45, 64, 1));
    let events = apply(&mut sim, cmds);
    assert!(rejections(&events).is_empty(), "{events:?}");
    let first = sim.view().unit(UnitId(1)).unwrap().seek_at.unwrap();
    let retry = rules.idle_seek_retry_ticks();
    assert!(run_until(&mut sim, first + 3 * retry + 2, |s| s.tick()
        == first + 3 * retry + 1));
    let u = sim.view().unit(UnitId(1)).unwrap();
    assert!(u.job.is_none() && u.order.is_none());
    assert_eq!(u.seek_at, Some(first + 4 * retry), "still retrying");
}

/// A Lumber Yard's slots are the forest tiles around its footprint (edge
/// or corner), counted from the map at placement and capped at `max`; with
/// none, `Build` is rejected.
#[test]
fn economy_adjacent_slots_count_forest_and_mountain_tiles() {
    let rules = load_rules();
    let def = rules.map(&rules.default_map).unwrap();
    for (id, nw) in [("lumber_yard", (13u16, 57u16)), ("ore_pit", (31, 57))] {
        let k = kind(&rules, id);
        let bk = rules.building_kind(k.0).unwrap();
        let rules::Slots::Adjacent { terrain, max } = bk.gather.as_ref().unwrap().slots else {
            panic!("{id} counts adjacent tiles");
        };
        let rect =
            sim::construction::footprint(bk, Tile::new(nw.0, nw.1), def.width, def.height).unwrap();
        let count = sim::construction::ring(rect, def.width, def.height)
            .iter()
            .filter(|t| def.tile(t.x, t.y) == terrain)
            .count();
        let want = u8::try_from(count.min(usize::try_from(max).unwrap())).unwrap();
        assert!(want > 0, "{id} at {nw:?} touches {terrain:?}");
        let mut sim = empty(29);
        apply(&mut sim, vec![place(0, 0, k, nw.0, nw.1, true)]);
        assert_eq!(
            sim.view().building(BuildingId(1)).unwrap().slots,
            want,
            "{id}"
        );
        if id == "lumber_yard" {
            assert_eq!(want, 3, "golden: three forest tiles touch (13, 57)");
        }
    }
}

/// A worked Yeoman shoved off its stand tile by a crowd walking through
/// keeps earning: work is judged by its post, not its live position.
#[test]
fn economy_pushed_worker_keeps_working() {
    let rules = load_rules();
    let croft = kind(&rules, "croft");
    let grain = resource(&rules, "grain");
    let rate = rules
        .building_kind(croft.0)
        .unwrap()
        .gather
        .as_ref()
        .unwrap()
        .rate_per_worker;
    let rect = sim::construction::Rect {
        x0: 30,
        y0: 60,
        x1: 31,
        y1: 61,
    };
    let mut sim = empty(30);
    apply(
        &mut sim,
        vec![
            place(0, 0, croft, 30, 60, true),
            spawn_kind(0, 1, 0, 30, 66, 1),
        ],
    );
    apply(&mut sim, vec![gather(0, 2, &[1], 1)]);
    assert!(run_until(&mut sim, 600, |s| worked(s, [1]) == 1));
    let stand = sim.view().unit(UnitId(1)).unwrap().post.unwrap().goal;
    // 30 Yeomen from the north-east pile up just west of the worker's stand
    // tile, the Croft's north-west ring corner, and shove it west off the
    // ring.
    apply(&mut sim, vec![spawn_kind(0, 3, 0, 36, 56, 30)]);
    apply(
        &mut sim,
        vec![PlayerCommand::new(
            PlayerId(0),
            4,
            Command::Move {
                units: (2..=31).map(UnitId).collect(),
                target: FxVec2::from_ints(28, 58),
                queue: false,
            },
        )],
    );
    let mut off_ring = 0;
    for _ in 0..600 {
        sim.step(&[]);
        let view = sim.view();
        let u = view.unit(UnitId(1)).unwrap();
        if rect.chebyshev(view.map().tile_of(u.pos)) != 1 {
            off_ring += 1;
        }
        assert_eq!(u.post.unwrap().goal, stand, "the post never moves");
        assert_eq!(worked(&sim, [1]), 1, "the slot stays worked");
        assert_eq!(sim.view().income(PlayerId(0))[grain].raw, rate);
    }
    assert!(off_ring > 0, "the crowd must push the worker off the ring");
}

/// Ordering a worker elsewhere frees its slot; another idle Yeoman takes it
/// at its next seek.
#[test]
fn economy_a_freed_slot_is_taken_by_an_idle_worker() {
    let rules = load_rules();
    let mut sim = empty(31);
    apply(
        &mut sim,
        vec![
            place(0, 0, kind(&rules, "croft"), 30, 60, true),
            spawn_kind(0, 1, 0, 30, 66, 2),
        ],
    );
    apply(&mut sim, vec![gather(0, 2, &[1], 1)]);
    assert!(run_until(&mut sim, 600, |s| worked(s, [1]) == 1));
    let events = apply(
        &mut sim,
        vec![PlayerCommand::new(
            PlayerId(0),
            3,
            Command::Move {
                units: vec![UnitId(1)],
                target: FxVec2::from_ints(20, 70),
                queue: false,
            },
        )],
    );
    assert!(rejections(&events).is_empty());
    assert!(sim.view().unit(UnitId(1)).unwrap().job.is_none());
    let retry = rules.idle_seek_delay_ticks() + rules.idle_seek_retry_ticks();
    assert!(run_until(&mut sim, retry + 600, |s| {
        s.view().unit(UnitId(2)).unwrap().job
            == Some(Job::Gather {
                node: BuildingId(1),
                slot: 0,
            })
    }));
}

/// The builder of a Croft takes its slot when the Croft completes.
#[test]
fn economy_croft_builder_takes_its_slot() {
    let rules = load_rules();
    let croft = kind(&rules, "croft");
    let mut sim = standard(32);
    apply(&mut sim, vec![build(0, 0, 1, croft, 28, 60)]);
    let done = rules.build_time_ticks(rules.building_kind(croft.0).unwrap());
    assert!(run_until(&mut sim, done + 600, |s| {
        s.view().building(BuildingId(3)).unwrap().construction == Construction::Complete
    }));
    assert_eq!(
        sim.view().unit(UnitId(1)).unwrap().job,
        Some(Job::Gather {
            node: BuildingId(3),
            slot: 0
        })
    );
    assert!(run_until(&mut sim, 300, |s| worked(s, [3]) == 1));
}

/// Bring P0 into the Masonry Age, which `town_growth_age` requires for tier
/// 2 since the M4a tech slice: `DebugResearch` two techs that leave
/// territory alone (Letters I, Arms I), grant the age's cost and advance.
/// Uses P0 sequence numbers `seq..seq + 5`.
fn reach_masonry(sim: &mut Sim, rules: &Rules, seq: u32) {
    let p = PlayerId(0);
    let tech = |id: &str| sim::TechId(rules.tech_by_id(id).expect("tech in techs.ron").0);
    let masonry = &rules.ages[1];
    assert_eq!(rules.town_growth_age.as_deref(), Some(masonry.id.as_str()));
    let mut cmds: Vec<Command> = ["letters_1", "arms_1"]
        .map(|id| Command::DebugResearch {
            owner: p,
            tech: tech(id),
        })
        .into();
    for (id, &amount) in &masonry.cost {
        cmds.push(Command::DebugGrant {
            owner: p,
            resource: u8::try_from(resource(rules, id)).unwrap(),
            amount,
        });
    }
    cmds.push(Command::AdvanceAge { player: p });
    let cmds = (seq..)
        .zip(cmds)
        .map(|(s, c)| PlayerCommand::new(p, s, c))
        .collect();
    let events = apply(sim, cmds);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(sim.view().age(p), 1);
}

/// A Town grows to tier 2 when its complete attributed buildings cover
/// `town_growth_distinct_buildings` distinct kinds (its owner in the
/// Masonry Age): its radius moves to `town_radius_grown_tiles` and the
/// field equals a fresh one.
#[test]
fn territory_town_grows_to_tier_two_at_five_distinct_kinds() {
    let rules = load_rules();
    let need = usize::try_from(rules.town_growth_distinct_buildings).unwrap();
    let kinds = [
        ("croft", 28u16, 58u16),
        ("scriptorium", 28, 67),
        ("muster_hall", 32, 67),
        ("engine_yard", 36, 67),
        ("watchtower", 31, 58),
    ];
    assert_eq!(kinds.len(), need, "one placement per kind needed");
    let mut sim = standard(33);
    reach_masonry(&mut sim, &rules, 100);
    for (i, (id, x, y)) in kinds.iter().enumerate() {
        let seq = u32::try_from(i).unwrap();
        let events = apply(
            &mut sim,
            vec![place(0, seq, kind(&rules, id), *x, *y, true)],
        );
        assert!(rejections(&events).is_empty(), "{id}: {events:?}");
        let b = BuildingId(3 + seq);
        assert_eq!(
            sim.view().building(b).unwrap().town,
            Some(BuildingId(1)),
            "{id}"
        );
        let tier = sim
            .view()
            .building(BuildingId(1))
            .unwrap()
            .town_state
            .unwrap()
            .tier;
        let grew = events.contains(&SimEvent::TownTierChanged {
            town: BuildingId(1),
            tier: 2,
        });
        if i + 1 < need {
            assert_eq!(tier, 1, "after {id}");
            assert!(!grew);
        } else {
            assert_eq!(tier, 2, "after {id}");
            assert!(grew && events.contains(&SimEvent::TerritoryChanged));
        }
    }
    let (r1, r2) = town_radii(&rules);
    let def = rules.map(&rules.default_map).unwrap();
    let mut expected = TerritoryField::new(def.width, def.height, 2);
    expected.add_source(
        BuildingId(1),
        source(player(0), def.starts[0].x, def.starts[0].y, r2),
    );
    expected.add_source(
        BuildingId(2),
        source(player(1), def.starts[1].x, def.starts[1].y, r1),
    );
    expected.add_source(
        BuildingId(7),
        source(player(0), 31, 58, watchtower_radius(&rules)),
    );
    assert_eq!(sim.view().territory(), &expected);
}

/// Snapshot mid-gather (workers walking and working, idle timers running),
/// restore into a fresh sim, keep issuing commands: every later tick hashes
/// the same as the uninterrupted run.
#[test]
fn economy_restore_mid_gather_matches_uninterrupted() {
    let rules = load_rules();
    let croft = kind(&rules, "croft");
    let mut a = standard(34);
    a.step(&[build(0, 0, 1, croft, 28, 60), build(0, 1, 2, croft, 32, 60)]);
    for _ in 1..700 {
        a.step(&[]);
    }
    let mut b = standard(34);
    b.restore(&a.snapshot()).unwrap();
    for t in 700..2400u32 {
        let cmds = if t == 900 {
            vec![build(0, 2, 3, croft, 36, 60)]
        } else {
            Vec::new()
        };
        a.step(&cmds);
        b.step(&cmds);
        assert_eq!(a.hash(), b.hash(), "tick {t}");
    }
    assert!(worked(&a, [3, 4, 5]) >= 2);
}

/// Tier growth counts distinct kinds, per Town, among complete buildings
/// (P0 in the Masonry Age throughout, so the age gate is met): five Crofts
/// are one kind; five kinds split across two Towns grow neither; a site of
/// the fifth kind does not count until it completes.
#[test]
fn territory_town_tier_counts_distinct_complete_kinds_per_town() {
    let rules = load_rules();
    assert_eq!(
        rules.town_growth_distinct_buildings, 5,
        "layouts below place 5 kinds"
    );
    let tier = |sim: &Sim, t: u32| {
        sim.view()
            .building(BuildingId(t))
            .unwrap()
            .town_state
            .unwrap()
            .tier
    };
    let grew = |events: &[SimEvent]| {
        events
            .iter()
            .any(|e| matches!(e, SimEvent::TownTierChanged { .. }))
    };

    // Five Crofts at the Seat: one kind.
    let croft = kind(&rules, "croft");
    let mut sim = standard(50);
    reach_masonry(&mut sim, &rules, 100);
    let crofts = (0..5u16)
        .map(|i| place(0, u32::from(i), croft, 28 + 3 * i, 58, true))
        .collect();
    let events = apply(&mut sim, crofts);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(tier(&sim, 1), 1);
    assert!(!grew(&events));

    // Five kinds split between the Seat (3) and a second Town (2).
    let mut sim = standard(51);
    reach_masonry(&mut sim, &rules, 100);
    let events = apply(
        &mut sim,
        vec![place(0, 0, kind(&rules, "town"), 40, 63, true)],
    );
    assert!(rejections(&events).is_empty(), "{events:?}");
    let split = [
        ("croft", 28, 58, 1),
        ("scriptorium", 28, 67, 1),
        ("muster_hall", 28, 70, 1),
        ("engine_yard", 44, 59, 3),
        ("watchtower", 45, 64, 3),
    ];
    for (i, (id, x, y, town)) in split.iter().enumerate() {
        let seq = 1 + u32::try_from(i).unwrap();
        let events = apply(
            &mut sim,
            vec![place(0, seq, kind(&rules, id), *x, *y, true)],
        );
        assert!(rejections(&events).is_empty(), "{id}: {events:?}");
        let b = sim.view().building(BuildingId(3 + seq)).unwrap();
        assert_eq!(b.town, Some(BuildingId(*town)), "{id} attribution");
        assert!(!grew(&events), "after {id}");
    }
    assert_eq!((tier(&sim, 1), tier(&sim, 3)), (1, 1));

    // Four complete kinds at the Seat plus an Engine Yard site: tier 1
    // until the site completes.
    let mut sim = standard(52);
    reach_masonry(&mut sim, &rules, 100);
    let lumber = u8::try_from(resource(&rules, "lumber")).unwrap();
    let mut cmds = vec![PlayerCommand::new(
        PlayerId(0),
        0,
        Command::DebugGrant {
            owner: PlayerId(0),
            resource: lumber,
            amount: 1000,
        },
    )];
    for (i, (id, x, y)) in [
        ("croft", 28u16, 58u16),
        ("scriptorium", 28, 67),
        ("muster_hall", 28, 70),
        ("watchtower", 20, 60),
    ]
    .iter()
    .enumerate()
    {
        cmds.push(place(
            0,
            1 + u32::try_from(i).unwrap(),
            kind(&rules, id),
            *x,
            *y,
            true,
        ));
    }
    let events = apply(&mut sim, cmds);
    assert!(rejections(&events).is_empty(), "{events:?}");
    let yard = kind(&rules, "engine_yard");
    let events = apply(&mut sim, vec![build(0, 10, 2, yard, 19, 67)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    let site = BuildingId(7);
    assert_eq!(sim.view().building(site).unwrap().town, Some(BuildingId(1)));
    let ticks = rules.build_time_ticks(rules.building_kind(yard.0).unwrap());
    let mut events = Vec::new();
    for _ in 0..ticks + 600 {
        sim.step(&[]);
        events.extend(sim.drain_events());
        let complete = sim.view().building(site).unwrap().construction == Construction::Complete;
        assert_eq!(tier(&sim, 1), if complete { 2 } else { 1 });
        if complete {
            break;
        }
    }
    assert_eq!(tier(&sim, 1), 2, "the Engine Yard completed");
    assert!(events.contains(&SimEvent::TownTierChanged {
        town: BuildingId(1),
        tier: 2
    }));
}

/// A Yeoman that arrives from a Move holds a post and is idle: it takes a
/// free slot exactly `idle_seek_delay` ticks after arriving.
#[test]
fn economy_idle_seek_starts_when_a_moved_worker_arrives() {
    let rules = load_rules();
    let mut sim = empty(53);
    apply(
        &mut sim,
        vec![
            place(0, 0, kind(&rules, "croft"), 30, 60, true),
            spawn_kind(0, 1, 0, 36, 68, 1),
            PlayerCommand::new(
                PlayerId(0),
                2,
                Command::Move {
                    units: vec![UnitId(1)],
                    target: FxVec2::from_ints(40, 70),
                    queue: false,
                },
            ),
        ],
    );
    assert_eq!(
        sim.view().unit(UnitId(1)).unwrap().seek_at,
        None,
        "busy walking"
    );
    assert!(run_until(&mut sim, 600, |s| s
        .view()
        .unit(UnitId(1))
        .unwrap()
        .order
        .is_none()));
    let arrived = sim.tick() - 1;
    let u = sim.view().unit(UnitId(1)).unwrap();
    assert!(u.post.is_some(), "it holds the post it arrived at");
    let at = arrived + rules.idle_seek_delay_ticks();
    assert_eq!(u.seek_at, Some(at));
    assert!(run_until(&mut sim, at + 1, |s| s.tick() == at));
    assert!(sim.view().unit(UnitId(1)).unwrap().job.is_none());
    sim.step(&[]);
    assert_eq!(
        sim.view().unit(UnitId(1)).unwrap().job,
        Some(Job::Gather {
            node: BuildingId(1),
            slot: 0
        })
    );
}

/// `Adjacent` slots are capped at `max`: with the cap lowered below the
/// three forest tiles beside (13, 57), the Lumber Yard gets `max` slots.
#[test]
fn economy_adjacent_slots_are_capped_at_max() {
    let mut rules = load_rules();
    let (yard, _) = rules.building_kind_by_id("lumber_yard").unwrap();
    let g = rules.buildings[usize::from(yard)].gather.as_mut().unwrap();
    let rules::Slots::Adjacent { max, .. } = &mut g.slots else {
        panic!("the Lumber Yard counts adjacent forest");
    };
    *max = 2;
    let setup = MatchSetup::scenario(&rules, 54);
    let mut sim = Sim::new(setup, rules, Box::new(NoAi));
    let events = apply(
        &mut sim,
        vec![place(0, 0, BuildingKindId(yard), 13, 57, true)],
    );
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(sim.view().building(BuildingId(1)).unwrap().slots, 2);
}

// ---------------------------------------------------------------------------
// Training, the unit cost ramps and the pop cap
// ---------------------------------------------------------------------------

fn train(p: u8, seq: u32, building: u32, kind: u16) -> PlayerCommand {
    PlayerCommand::new(
        PlayerId(p),
        seq,
        Command::Train {
            building: BuildingId(building),
            kind: UnitKindId(kind),
        },
    )
}

fn cancel(p: u8, seq: u32, building: u32, slot: u8) -> PlayerCommand {
    PlayerCommand::new(
        PlayerId(p),
        seq,
        Command::Cancel {
            building: BuildingId(building),
            slot,
        },
    )
}

fn grant(p: u8, seq: u32, resource: usize, amount: u32) -> PlayerCommand {
    PlayerCommand::new(
        PlayerId(p),
        seq,
        Command::DebugGrant {
            owner: PlayerId(p),
            resource: u8::try_from(resource).unwrap(),
            amount,
        },
    )
}

/// M3a acceptance 1e: queuing a Yeoman raises the next Yeoman's cost by the
/// Yeoman's per-unit step (`units.ron`) before it finishes. The five
/// standard-start Yeomen count (owner decision D11), the deduction is the
/// quote, and the quote does not move again when the trainee appears.
#[test]
fn economy_queuing_a_yeoman_raises_the_next_cost_before_it_finishes() {
    let rules = load_rules();
    let yeoman = unit_kind(&rules, "yeoman");
    let def = rules.unit_kind(yeoman).unwrap();
    let grain = resource(&rules, "grain");
    let base = i64::from(def.cost["grain"]);
    let rules::UnitRamp::PerUnit(step) = &def.ramp else {
        panic!("the Yeoman ramps per unit");
    };
    let step = i64::from(step["grain"]);
    let start = i64::from(rules.standard_start.units["yeoman"]);
    let mut sim = standard(60);
    let mut control = standard(60);
    let quote = |s: &Sim| s.view().unit_cost(PlayerId(0), UnitKindId(yeoman))[grain];
    let q0 = quote(&sim);
    assert_eq!(q0, base + step * start, "the starting Yeomen count");
    let events = apply(&mut sim, vec![train(0, 0, 1, yeoman)]);
    apply(&mut control, vec![]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(
        stock(&control, 0, grain) - stock(&sim, 0, grain),
        q0,
        "the deduction is the quote"
    );
    let queued = &sim.view().building(BuildingId(1)).unwrap().queue;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].paid[grain], q0);
    assert_eq!(quote(&sim), q0 + step, "queued, not yet trained");
    assert_eq!(
        sim.view().pop(PlayerId(0)).0,
        u32::try_from(start + 1).unwrap()
    );
    let ticks = rules.train_time_ticks(def);
    assert!(run_until(&mut sim, ticks + 5, |s| s.view().unit_count() == 11));
    assert!(sim.view().building(BuildingId(1)).unwrap().queue.is_empty());
    assert_eq!(quote(&sim), q0 + step, "the trainee alive counts the same");
}

/// M3a acceptance 1f: the military ramp. Two Muster Halls share one
/// counter: five Skirmishers queued at the first and a sixth at the second
/// pay `floor(base * mult / 1000)` with `mult = min(cap, 1000 + step *
/// T(k - 1))`, and the sixth is capped at `military_ramp_cap_permille`
/// (2.25x). Cancelling lowers `k` for the next order and refunds exactly
/// what each entry paid.
#[test]
fn economy_sixth_queued_skirmisher_is_capped() {
    let rules = load_rules();
    let hall = kind(&rules, "muster_hall");
    let sk = unit_kind(&rules, "skirmisher");
    let grain = resource(&rules, "grain");
    let base = u64::from(rules.unit_kind(sk).unwrap().cost["grain"]);
    let step = u64::from(rules.military_ramp_step_permille);
    let cap = u64::from(rules.military_ramp_cap_permille);
    let mult = |k: u64| (1000 + step * (k - 1) * k / 2).min(cap);
    let paid_at = |k: u64| i64::try_from(base * mult(k) / 1000).unwrap();
    assert!(1000 + step * 5 * 6 / 2 >= cap, "the 6th reaches the cap");
    assert!(mult(5) < cap, "the 5th does not");
    let mut sim = empty(61);
    apply(
        &mut sim,
        vec![
            place(0, 0, hall, 28, 58, true),
            place(0, 1, hall, 34, 58, true),
            grant(0, 2, grain, 100_000),
        ],
    );
    let five: Vec<PlayerCommand> = (0..5).map(|i| train(0, 3 + i, 1, sk)).collect();
    let events = apply(&mut sim, five);
    assert!(rejections(&events).is_empty(), "{events:?}");
    let events = apply(&mut sim, vec![train(0, 8, 1, sk)]);
    assert_eq!(rejections(&events), [RejectReason::QueueFull]);
    let events = apply(&mut sim, vec![train(0, 9, 2, sk)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    let view = sim.view();
    let paid: Vec<i64> = view
        .building(BuildingId(1))
        .unwrap()
        .queue
        .iter()
        .chain(&view.building(BuildingId(2)).unwrap().queue)
        .map(|e| e.paid[grain])
        .collect();
    let want: Vec<i64> = (1..=6).map(paid_at).collect();
    assert_eq!(paid, want);
    assert_eq!(
        paid[5],
        i64::try_from(base * cap / 1000).unwrap(),
        "capped at 2.25x"
    );
    if (base, step, cap) == (40, 100, 2250) {
        assert_eq!(paid, [40, 44, 52, 64, 80, 90]);
    }
    // Cancel two entries in the middle of the first hall's queue: slot 1
    // (the 2nd Skirmisher), then slot 2 of what remains (the 4th). Each
    // refunds exactly what that entry paid, the others keep their places,
    // and the next quote falls back to k = 5.
    let before = stock(&sim, 0, grain);
    let events = apply(&mut sim, vec![cancel(0, 10, 1, 1), cancel(0, 11, 1, 2)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert!(events.contains(&SimEvent::TrainingCancelled {
        building: BuildingId(1),
        slot: 1
    }));
    assert_eq!(stock(&sim, 0, grain) - before, paid_at(2) + paid_at(4));
    let left: Vec<i64> = sim
        .view()
        .building(BuildingId(1))
        .unwrap()
        .queue
        .iter()
        .map(|e| e.paid[grain])
        .collect();
    assert_eq!(
        left,
        [paid_at(1), paid_at(3), paid_at(5)],
        "the named entries left"
    );
    assert_eq!(
        sim.view().unit_cost(PlayerId(0), UnitKindId(sk))[grain],
        paid_at(5),
        "k fell from 7 to 5"
    );
}

/// The ramps count the right things. The Yeoman's `n` counts own Yeomen
/// only, alive or queued; the military `k` counts entries queued at own
/// buildings of the trainer kind only, so Yeomen queued at the Seat leave
/// it alone and a finished Skirmisher stops counting; pop and both ramps
/// ignore the other player.
#[test]
fn economy_unit_ramps_count_only_their_kind_class_and_owner() {
    let rules = load_rules();
    let hall = kind(&rules, "muster_hall");
    let yeoman = unit_kind(&rules, "yeoman");
    let sk = unit_kind(&rules, "skirmisher");
    let grain = resource(&rules, "grain");
    let sk_base = i64::from(rules.unit_kind(sk).unwrap().cost["grain"]);
    let rules::UnitRamp::PerUnit(ystep) = &rules.unit_kind(yeoman).unwrap().ramp else {
        panic!("the Yeoman ramps per unit");
    };
    let ystep = i64::from(ystep["grain"]);
    let mut sim = standard(67);
    // P0's Muster Hall is BuildingId(3), P1's BuildingId(4).
    let events = apply(
        &mut sim,
        vec![
            place(0, 0, hall, 28, 58, true),
            grant(0, 1, grain, 10_000),
            place(1, 0, hall, 96, 58, true),
            grant(1, 1, grain, 10_000),
        ],
    );
    assert!(rejections(&events).is_empty(), "{events:?}");
    let quote = |s: &Sim, k: u16| s.view().unit_cost(PlayerId(0), UnitKindId(k))[grain];
    let (y0, pop0) = (quote(&sim, yeoman), sim.view().pop(PlayerId(0)));
    assert_eq!(quote(&sim, sk), sk_base, "k = 1");

    // P1 queues Yeomen and Skirmishers: nothing of P0's moves.
    let p1 = vec![
        train(1, 2, 2, yeoman),
        train(1, 3, 2, yeoman),
        train(1, 4, 4, sk),
        train(1, 5, 4, sk),
    ];
    let events = apply(&mut sim, p1);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(quote(&sim, yeoman), y0, "P1's Yeomen are not P0's");
    assert_eq!(quote(&sim, sk), sk_base, "P1's halls are not P0's");
    assert_eq!(
        sim.view().pop(PlayerId(0)),
        pop0,
        "P1's queues are not P0's pop"
    );

    // P0 queues two Yeomen at the Seat: the Yeoman ramp moves by two steps,
    // the military ramp does not (the Seat is not a Muster Hall).
    let events = apply(
        &mut sim,
        vec![train(0, 2, 1, yeoman), train(0, 3, 1, yeoman)],
    );
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(quote(&sim, yeoman), y0 + 2 * ystep);
    assert_eq!(quote(&sim, sk), sk_base, "Seat queues are not military");

    // A live Skirmisher moves neither ramp: n counts Yeomen, k counts queues.
    let events = apply(&mut sim, vec![spawn_kind(0, 4, sk, 34, 70, 1)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(
        quote(&sim, yeoman),
        y0 + 2 * ystep,
        "a Skirmisher is not a Yeoman"
    );
    assert_eq!(quote(&sim, sk), sk_base, "alive is not queued");

    // A queued Skirmisher raises k; once it is trained, k is back to 1.
    let events = apply(&mut sim, vec![train(0, 5, 3, sk)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert!(quote(&sim, sk) > sk_base, "queued raises k");
    assert_eq!(
        quote(&sim, yeoman),
        y0 + 2 * ystep,
        "a queued Skirmisher is not a Yeoman"
    );
    let ticks = rules.train_time_ticks(rules.unit_kind(sk).unwrap());
    assert!(run_until(&mut sim, ticks + 5, |s| s
        .view()
        .building(BuildingId(3))
        .unwrap()
        .queue
        .is_empty()));
    let trained = sim
        .view()
        .units()
        .filter(|u| u.owner == PlayerId(0) && u.kind == UnitKindId(sk))
        .count();
    assert_eq!(trained, 2, "the spawned and the trained Skirmisher");
    assert_eq!(quote(&sim, sk), sk_base, "finishing lowers k");
}

/// Pop is charged when a unit is queued: at the cap, `Train` is
/// `PopCapReached`, and a queued unit counts as much as a live one.
#[test]
fn economy_train_is_rejected_at_the_pop_cap() {
    let rules = load_rules();
    let yeoman = unit_kind(&rules, "yeoman");
    let cap = rules.pop_cap_base;
    let start = rules.standard_start.units["yeoman"];
    let mut sim = standard(62);
    let grain = resource(&rules, "grain");
    apply(
        &mut sim,
        vec![
            spawn_kind(0, 0, 0, 34, 70, u16::try_from(cap - start - 1).unwrap()),
            grant(0, 1, grain, 1000),
        ],
    );
    assert_eq!(sim.view().pop(PlayerId(0)), (cap - 1, cap));
    let events = apply(&mut sim, vec![train(0, 2, 1, yeoman)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(
        sim.view().pop(PlayerId(0)),
        (cap, cap),
        "a queued unit counts"
    );
    let events = apply(&mut sim, vec![train(0, 3, 1, yeoman)]);
    assert_eq!(rejections(&events), [RejectReason::PopCapReached]);
}

/// Every `Train`, `SetRally` and queue `Cancel` rejection reason, each
/// against a control sim whose command at the same (player, seq) is
/// rejected for an unrelated reason: no state changes and no randomness
/// drawn.
#[test]
fn economy_train_rejections() {
    let rules = load_rules();
    let yeoman = unit_kind(&rules, "yeoman");
    let sk = unit_kind(&rules, "skirmisher");
    let scribe = unit_kind(&rules, "scribe");
    let base = |s: &mut Sim| {
        apply(
            s,
            vec![
                place(0, 100, kind(&rules, "muster_hall"), 28, 58, true),
                place(0, 101, kind(&rules, "scriptorium"), 28, 67, false),
            ],
        );
    };
    let fill = |s: &mut Sim| {
        let five = (0..5).map(|i| train(0, 110 + i, 1, yeoman)).collect();
        let events = apply(s, five);
        assert!(rejections(&events).is_empty(), "{events:?}");
    };
    let crowd = |s: &mut Sim| {
        let room = rules.pop_cap_base - rules.standard_start.units["yeoman"];
        let spawn = spawn_kind(0, 120, 0, 34, 70, u16::try_from(room).unwrap());
        let events = apply(s, vec![spawn]);
        assert!(rejections(&events).is_empty(), "{events:?}");
        assert_eq!(s.view().pop(PlayerId(0)).0, rules.pop_cap_base);
    };
    type Setup<'a> = Box<dyn Fn(&mut Sim) + 'a>;
    let none: Setup = Box::new(|_| {});
    let cases: Vec<(&str, Setup, PlayerCommand, RejectReason)> = vec![
        (
            "another player's Seat",
            none,
            train(0, 0, 2, yeoman),
            RejectReason::UnknownBuilding,
        ),
        (
            "no such building",
            Box::new(|_| {}),
            train(0, 0, 99, yeoman),
            RejectReason::UnknownBuilding,
        ),
        (
            "a site",
            Box::new(|_| {}),
            train(0, 0, 4, scribe),
            RejectReason::UnderConstruction,
        ),
        (
            "no such kind",
            Box::new(|_| {}),
            train(0, 0, 1, 99),
            RejectReason::UnknownUnitKind,
        ),
        (
            "Skirmisher at the Seat",
            Box::new(|_| {}),
            train(0, 0, 1, sk),
            RejectReason::CannotTrainHere,
        ),
        (
            "Yeoman at a Muster Hall",
            Box::new(|_| {}),
            train(0, 0, 3, yeoman),
            RejectReason::CannotTrainHere,
        ),
        (
            "a full queue",
            Box::new(fill),
            train(0, 0, 1, yeoman),
            RejectReason::QueueFull,
        ),
        (
            "at the pop cap",
            Box::new(crowd),
            train(0, 0, 1, yeoman),
            RejectReason::PopCapReached,
        ),
        (
            "Grain spent on Yeomen",
            Box::new(fill),
            train(0, 0, 3, sk),
            RejectReason::CannotAfford {
                resource: u8::try_from(resource(&rules, "grain")).unwrap(),
            },
        ),
        (
            "rally on another player's Seat",
            Box::new(|_| {}),
            PlayerCommand::new(
                PlayerId(0),
                0,
                Command::SetRally {
                    building: BuildingId(2),
                    target: FxVec2::from_ints(30, 70),
                },
            ),
            RejectReason::UnknownBuilding,
        ),
        (
            "a queue slot past the end",
            Box::new(fill),
            cancel(0, 0, 1, 5),
            RejectReason::InvalidSlot,
        ),
        (
            "a Muster Hall's empty queue",
            Box::new(|_| {}),
            cancel(0, 0, 3, 0),
            RejectReason::InvalidSlot,
        ),
        (
            "cancel at another player's Seat",
            Box::new(|_| {}),
            cancel(0, 0, 2, 0),
            RejectReason::UnknownBuilding,
        ),
    ];
    for (name, setup, cmd, want) in cases {
        let mut sim = standard(63);
        let mut control = standard(63);
        base(&mut sim);
        base(&mut control);
        setup(&mut sim);
        setup(&mut control);
        let stop = PlayerCommand::new(
            cmd.player,
            cmd.seq,
            Command::Stop {
                units: vec![UnitId(999)],
            },
        );
        let events = apply(&mut sim, vec![cmd]);
        apply(&mut control, vec![stop]);
        assert_eq!(rejections(&events), [want], "{name}");
        assert_eq!(
            sim.sub_hashes(),
            control.sub_hashes(),
            "{name}: no state change"
        );
    }
}

/// Only the head of a queue trains. A Yeoman queued into an empty queue
/// appears in the `train_time`-th step counting the one that queued it; the
/// entry behind it gains nothing meanwhile and appears `train_time` steps
/// later. A trained unit appears beside the building, on the first
/// passable tile of the spiral around the footprint centre in the spawn
/// component (here the open map; the tile may be occupied), and walks to
/// the rally point.
#[test]
fn economy_trained_unit_spawns_beside_the_building_and_walks_to_the_rally() {
    let rules = load_rules();
    let yeoman = unit_kind(&rules, "yeoman");
    let ticks = rules.train_time_ticks(rules.unit_kind(yeoman).unwrap());
    let rally = FxVec2::from_ints(34, 72);
    let mut sim = standard(64);
    let events = apply(
        &mut sim,
        vec![
            PlayerCommand::new(
                PlayerId(0),
                0,
                Command::SetRally {
                    building: BuildingId(1),
                    target: rally,
                },
            ),
            train(0, 1, 1, yeoman),
            train(0, 2, 1, yeoman),
        ],
    );
    assert!(rejections(&events).is_empty(), "{events:?}");
    let queued_at = sim.tick() - 1;
    let spawn_at = queued_at + ticks;
    assert!(run_until(&mut sim, ticks + 2, |s| s.tick() == spawn_at - 1));
    assert!(sim.view().unit(UnitId(11)).is_none(), "not one tick early");
    let queue = &sim.view().building(BuildingId(1)).unwrap().queue;
    assert_eq!(queue[0].progress, ticks - 1);
    assert_eq!(queue[1].progress, 0, "only the head trains");
    sim.step(&[]);
    let u = sim.view().unit(UnitId(11)).expect("trained on time");
    let map = sim.view().map();
    let seat = sim::construction::Rect {
        x0: 23,
        y0: 63,
        x1: 25,
        y1: 65,
    };
    assert_eq!(seat.chebyshev(map.tile_of(u.pos)), 1, "beside the Seat");
    assert!(u.order.is_some(), "walking to the rally point");
    assert!(run_until(&mut sim, 600, |s| s
        .view()
        .unit(UnitId(11))
        .unwrap()
        .order
        .is_none()));
    let at = sim.view().unit(UnitId(11)).unwrap().pos;
    assert!(
        at.dist_sq_i64(rally) <= (2 * 2) << 32,
        "arrived near the rally point"
    );
    // The second entry started in the step after the first finished.
    let second_at = spawn_at + ticks;
    assert!(run_until(&mut sim, ticks, |s| s.tick() == second_at - 1));
    assert!(sim.view().unit(UnitId(12)).is_none(), "not one tick early");
    sim.step(&[]);
    assert!(
        sim.view().unit(UnitId(12)).is_some(),
        "one train_time later"
    );
}

/// A pocket that neighbouring buildings seal against a trainer takes the
/// trained unit only when the rally point is inside it. Four Crofts around
/// a Town's east side leave two tiles there cut off, and the spiral reaches
/// them first. With no rally, a rally outside, a rally on the Town itself
/// (whose corrected goal lies in the pocket) or a rally into a far pocket
/// that does not touch the Town, the unit appears on the open side; a rally
/// inside the pocket puts it there.
#[test]
fn economy_trained_unit_spawns_in_a_sealed_pocket_only_when_rallied_into_it() {
    let rules = load_rules();
    let (town, croft) = (kind(&rules, "town"), kind(&rules, "croft"));
    let tower = kind(&rules, "watchtower");
    let yeoman = unit_kind(&rules, "yeoman");
    let ticks = rules.train_time_ticks(rules.unit_kind(yeoman).unwrap());
    let mut sim = empty(68);
    let events = apply(
        &mut sim,
        vec![
            place(0, 0, town, 23, 63, true),
            place(0, 1, croft, 26, 62, true),
            place(0, 2, croft, 27, 64, true),
            place(0, 3, croft, 25, 66, true),
            place(0, 4, croft, 27, 66, true),
            grant(0, 5, resource(&rules, "grain"), 1000),
            // A one-tile pocket far from the Town.
            place(0, 6, tower, 41, 42, true),
            place(0, 7, tower, 43, 42, true),
            place(0, 8, tower, 42, 41, true),
            place(0, 9, tower, 42, 43, true),
        ],
    );
    assert!(rejections(&events).is_empty(), "{events:?}");
    let map = sim.view().map();
    let (pocket, open, far) = (Tile::new(26, 64), Tile::new(22, 64), Tile::new(42, 42));
    assert!(map.passable(pocket) && map.passable(Tile::new(26, 65)));
    assert_ne!(
        map.component_of(pocket),
        map.component_of(open),
        "the layout seals a pocket"
    );
    assert!(map.passable(far));
    assert_ne!(map.component_of(far), map.component_of(open));
    assert_ne!(map.component_of(far), map.component_of(pocket));
    let comp = |s: &Sim, u: u32| {
        let map = s.view().map();
        map.component_of(map.tile_of(s.view().unit(UnitId(u)).unwrap().pos))
    };
    let rally = |seq: u32, x: i32, y: i32| {
        PlayerCommand::new(
            PlayerId(0),
            seq,
            Command::SetRally {
                building: BuildingId(1),
                target: FxVec2::from_ints(x, y),
            },
        )
    };
    let open_comp = map.component_of(open);
    let pocket_comp = map.component_of(pocket);

    // No rally: the open side.
    let events = apply(&mut sim, vec![train(0, 10, 1, yeoman)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert!(run_until(&mut sim, ticks + 5, |s| s.view().unit_count() == 1));
    assert_eq!(comp(&sim, 1), open_comp, "no rally");

    // A rally outside: the open side, and the unit gets there.
    let events = apply(&mut sim, vec![rally(11, 20, 70), train(0, 12, 1, yeoman)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert!(run_until(&mut sim, ticks + 5, |s| s.view().unit_count() == 2));
    assert_eq!(comp(&sim, 2), open_comp, "rally outside");
    assert!(run_until(&mut sim, 600, |s| s
        .view()
        .unit(UnitId(2))
        .unwrap()
        .order
        .is_none()));
    let at = sim.view().unit(UnitId(2)).unwrap().pos;
    assert!(at.dist_sq_i64(FxVec2::from_ints(20, 70)) <= (2 * 2) << 32);

    // A rally on the Town itself: its tile is blocked, so it names no side.
    let events = apply(&mut sim, vec![rally(13, 24, 64), train(0, 14, 1, yeoman)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert!(run_until(&mut sim, ticks + 5, |s| s.view().unit_count() == 3));
    assert_eq!(comp(&sim, 3), open_comp, "rally on the trainer");

    // A rally into a pocket that does not touch the Town: not a side of it.
    let events = apply(&mut sim, vec![rally(15, 42, 42), train(0, 16, 1, yeoman)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert!(run_until(&mut sim, ticks + 5, |s| s.view().unit_count() == 4));
    assert_eq!(comp(&sim, 4), open_comp, "rally in a far pocket");

    // A rally inside the pocket: the pocket.
    let events = apply(&mut sim, vec![rally(17, 26, 65), train(0, 18, 1, yeoman)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert!(run_until(&mut sim, ticks + 5, |s| s.view().unit_count() == 5));
    assert_eq!(comp(&sim, 5), pocket_comp, "rally inside the pocket");
}

/// A Town walled in by Watchtowers: the spawn component is the largest of
/// the pockets on its ring (ties to the lower id), and with every ring tile
/// blocked it is the largest component on the map unless the rally point
/// names another.
#[test]
fn economy_trained_unit_at_an_enclosed_trainer_follows_the_component_rule() {
    let rules = load_rules();
    let town = kind(&rules, "town");
    let tower = kind(&rules, "watchtower");
    let yeoman = unit_kind(&rules, "yeoman");
    let ticks = rules.train_time_ticks(rules.unit_kind(yeoman).unwrap());
    // The Town covers (23..=25, 63..=65); its ring is the square at
    // Chebyshev distance 1 from the footprint, the wall the one at 2.
    let square = |d: u16| -> Vec<(u16, u16)> {
        let (x0, y0, x1, y1) = (23 - d, 63 - d, 25 + d, 65 + d);
        (y0..=y1)
            .flat_map(|y| (x0..=x1).map(move |x| (x, y)))
            .filter(|&(x, y)| x == x0 || x == x1 || y == y0 || y == y1)
            .collect()
    };
    // Train one Yeoman at a Town with Watchtowers on `blocked`, optionally
    // rallied to `rally`, and return the trained unit's tile with the map.
    let run = |blocked: Vec<(u16, u16)>, rally: Option<(i32, i32)>| {
        let mut sim = empty(69);
        let mut cmds = vec![
            place(0, 0, town, 23, 63, true),
            grant(0, 1, resource(&rules, "grain"), 1000),
        ];
        let mut seq = 2;
        for (x, y) in blocked {
            cmds.push(place(0, seq, tower, x, y, true));
            seq += 1;
        }
        if let Some((x, y)) = rally {
            cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq,
                Command::SetRally {
                    building: BuildingId(1),
                    target: FxVec2::from_ints(x, y),
                },
            ));
            seq += 1;
        }
        cmds.push(train(0, seq, 1, yeoman));
        let events = apply(&mut sim, cmds);
        assert!(rejections(&events).is_empty(), "{events:?}");
        assert!(run_until(&mut sim, ticks + 5, |s| s.view().units().count() == 1));
        let u = sim.view().units().next().unwrap();
        let tile = sim.view().map().tile_of(u.pos);
        (sim, tile)
    };
    let comp = |sim: &Sim, t: Tile| sim.view().map().component_of(t);
    let walled = |open: &[(u16, u16)]| -> Vec<(u16, u16)> {
        square(1)
            .into_iter()
            .filter(|t| !open.contains(t))
            .chain(square(2))
            .collect()
    };

    // Two pockets on the ring: one tile north (lower id, row-major) and two
    // tiles south (higher id). The larger wins.
    let (sim, tile) = run(walled(&[(24, 62), (23, 66), (24, 66)]), None);
    let (north, south) = (Tile::new(24, 62), Tile::new(24, 66));
    assert!(comp(&sim, north) < comp(&sim, south), "ids follow rows");
    assert_eq!(comp(&sim, tile), comp(&sim, south), "the larger pocket");

    // Two one-tile pockets: the lower id wins.
    let (sim, tile) = run(walled(&[(24, 62), (24, 66)]), None);
    assert_eq!(
        comp(&sim, tile),
        comp(&sim, north),
        "a tie goes to the lower id"
    );

    // Every ring tile blocked, and a one-tile pocket at (27, 64), which is
    // the first tile of the next spiral ring. The unit appears in the
    // largest component on the map instead.
    let mut sealed = square(1);
    sealed.extend([(27, 63), (27, 65), (28, 63), (28, 64), (28, 65)]);
    let pocket = Tile::new(27, 64);
    let (sim, tile) = run(sealed.clone(), None);
    assert!(sim.view().map().passable(pocket));
    assert_ne!(comp(&sim, pocket), comp(&sim, Tile::new(20, 70)));
    assert_eq!(
        comp(&sim, tile),
        comp(&sim, Tile::new(20, 70)),
        "the main area"
    );

    // The same with the rally point inside that pocket: any component is a
    // candidate when the ring is blocked, so the pocket.
    let (_, tile) = run(sealed, Some((27, 64)));
    assert_eq!(tile, pocket, "rallied into the pocket");
}

/// `SimView::check_train` and `check_build` agree with applying the command
/// for an unknown player and for one that has surrendered.
#[test]
fn economy_view_checks_agree_with_apply_for_unknown_and_surrendered_players() {
    let rules = load_rules();
    let yeoman = unit_kind(&rules, "yeoman");
    let town = kind(&rules, "town");
    let mut sim = standard(70);
    let view = sim.view();
    let nobody = PlayerId(9);
    assert_eq!(
        view.check_train(nobody, BuildingId(1), UnitKindId(yeoman)),
        Err(RejectReason::UnknownPlayer)
    );
    assert_eq!(
        view.check_build(nobody, UnitId(1), town, Tile::new(40, 63)),
        Err(RejectReason::UnknownPlayer)
    );
    assert!(
        view.check_train(PlayerId(0), BuildingId(1), UnitKindId(yeoman))
            .is_ok()
    );
    assert!(
        view.check_build(PlayerId(0), UnitId(1), town, Tile::new(40, 63))
            .is_ok()
    );
    let events = apply(
        &mut sim,
        vec![train(9, 0, 1, yeoman), build(9, 1, 1, town, 40, 63)],
    );
    assert_eq!(
        rejections(&events),
        [RejectReason::UnknownPlayer, RejectReason::UnknownPlayer]
    );

    let events = apply(
        &mut sim,
        vec![PlayerCommand::new(PlayerId(0), 0, Command::Surrender)],
    );
    assert!(rejections(&events).is_empty(), "{events:?}");
    let view = sim.view();
    assert_eq!(
        view.check_train(PlayerId(0), BuildingId(1), UnitKindId(yeoman)),
        Err(RejectReason::Surrendered)
    );
    assert_eq!(
        view.check_build(PlayerId(0), UnitId(1), town, Tile::new(40, 63)),
        Err(RejectReason::Surrendered)
    );
    let events = apply(
        &mut sim,
        vec![train(0, 1, 1, yeoman), build(0, 2, 1, town, 40, 63)],
    );
    assert_eq!(
        rejections(&events),
        [RejectReason::Surrendered, RejectReason::Surrendered]
    );
}

/// A Scribe trained at a Scriptorium takes a slot there at once and works
/// it once it reaches the slot's stand tile.
#[test]
fn economy_scribe_trained_at_a_scriptorium_takes_a_slot() {
    let rules = load_rules();
    let scribe = unit_kind(&rules, "scribe");
    let mut sim = empty(65);
    apply(
        &mut sim,
        vec![
            place(0, 0, kind(&rules, "scriptorium"), 30, 59, true),
            grant(0, 1, resource(&rules, "grain"), 500),
        ],
    );
    let events = apply(&mut sim, vec![train(0, 2, 1, scribe)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    let ticks = rules.train_time_ticks(rules.unit_kind(scribe).unwrap());
    assert!(run_until(&mut sim, ticks + 5, |s| s.view().unit_count() == 1));
    assert_eq!(
        sim.view().unit(UnitId(1)).unwrap().job,
        Some(Job::Gather {
            node: BuildingId(1),
            slot: 0
        })
    );
    assert!(run_until(&mut sim, 300, |s| worked(s, [1]) == 1));
}

/// Snapshot with a queue half trained, restore into a fresh sim, keep
/// issuing commands: every later tick hashes the same as the uninterrupted
/// run.
#[test]
fn economy_restore_mid_training_matches_uninterrupted() {
    let rules = load_rules();
    let yeoman = unit_kind(&rules, "yeoman");
    let mut a = standard(66);
    a.step(&[
        train(0, 0, 1, yeoman),
        train(0, 1, 1, yeoman),
        train(0, 2, 1, yeoman),
    ]);
    for _ in 1..150 {
        a.step(&[]);
    }
    let head = &a.view().building(BuildingId(1)).unwrap().queue[0];
    assert!(head.progress > 0, "mid training");
    let mut b = standard(66);
    b.restore(&a.snapshot()).unwrap();
    for t in 150..1500u32 {
        let cmds = match t {
            300 => vec![PlayerCommand::new(
                PlayerId(0),
                3,
                Command::SetRally {
                    building: BuildingId(1),
                    target: FxVec2::from_ints(34, 70),
                },
            )],
            400 => vec![train(0, 4, 1, yeoman), cancel(0, 5, 1, 1)],
            _ => Vec::new(),
        };
        a.step(&cmds);
        b.step(&cmds);
        assert_eq!(a.hash(), b.hash(), "tick {t}");
    }
    assert!(a.view().unit_count() > 10);
}
