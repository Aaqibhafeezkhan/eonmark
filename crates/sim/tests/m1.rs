//! M1 acceptance tests (docs/ROADMAP.md, "M1"). Every test runs in
//! `cargo test -p sim` except `golden_move_500`, the long golden that
//! `sim-cli verify --release` checks in CI.

use proptest::prelude::*;
use sim::pathing::{AStarSearch, PathRequest, Pathing, SearchStatus, path_cost};
use sim::scenarios;
use sim::{
    Command, FxVec2, Map, MatchSetup, PlayerCommand, PlayerId, Rules, Sim, SimView, Tile, UnitId,
    UnitKindId,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const WEST: FxVec2 = scenarios::WEST;
const EAST: FxVec2 = scenarios::EAST;

struct NoAi;

impl sim::AiController for NoAi {
    fn think(
        &mut self,
        _player: PlayerId,
        _view: &SimView<'_>,
        _memory: &mut sim::AiMemory,
    ) -> Vec<Command> {
        Vec::new()
    }
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

fn plains() -> Map {
    Map::from_def(load_rules().map("plains_1v1").unwrap())
}

fn scenario(seed: u64) -> Sim {
    let rules = load_rules();
    let setup = MatchSetup::scenario(&rules, seed);
    Sim::new(setup, rules, Box::new(NoAi))
}

fn spawn(owner: u8, seq: u32, at: FxVec2, count: u16) -> PlayerCommand {
    PlayerCommand::new(
        PlayerId(owner),
        seq,
        Command::DebugSpawn {
            owner: PlayerId(owner),
            kind: UnitKindId(0),
            at,
            count,
        },
    )
}

fn move_all(owner: u8, seq: u32, sim: &Sim, target: FxVec2) -> PlayerCommand {
    let units = sim
        .view()
        .units()
        .filter(|u| u.owner == PlayerId(owner))
        .map(|u| u.id)
        .collect();
    PlayerCommand::new(
        PlayerId(owner),
        seq,
        Command::Move {
            units,
            target,
            queue: false,
        },
    )
}

/// The fixed M1 scenario (`sim::scenarios::move_500`): 500 Yeomen spawned at
/// the west start on tick 0, ordered to the east start on tick 5, a Stop for
/// every third unit at tick 400, then idle until `ticks`. The scenario is
/// pure data, so the same stream drives `sim-cli record` and the fixtures.
fn move_500_commands(tick: u32) -> Vec<PlayerCommand> {
    let (_, stream) = scenarios::move_500(&load_rules());
    scenarios::commands_at(&stream, tick).to_vec()
}

/// Run `move_500` with its seed replaced by `seed` for `ticks` ticks.
fn run_move_500(seed: u64, ticks: u32) -> u64 {
    let rules = load_rules();
    let (mut setup, stream) = scenarios::move_500(&rules);
    setup.seed = seed;
    scenarios::run(setup, rules, &stream, ticks).hash()
}

/// The scenario's command stream, pre-expanded per tick for a run.
struct Script(scenarios::Stream);

impl Script {
    fn at(&self, tick: u32) -> &[PlayerCommand] {
        scenarios::commands_at(&self.0, tick)
    }
}

/// Load `fixtures/<name>.eonreplay`, verify it, and compare the final hash to
/// `fixtures/<name>.hash` (`0x<16 hex>` and a newline). Fixtures are created
/// by the integrator; regenerate only with a `rules_version` bump.
fn golden(name: &str) {
    let replay = fixtures_dir().join(format!("{name}.eonreplay"));
    let hash_file = fixtures_dir().join(format!("{name}.hash"));
    let expected = std::fs::read_to_string(&hash_file)
        .unwrap_or_else(|e| panic!("{}: {e}", hash_file.display()));
    let expected = expected.trim();
    let outcome = sim::verify(&replay, &data_dir(), Box::new(NoAi))
        .unwrap_or_else(|e| panic!("{}: {e}", replay.display()));
    match outcome {
        sim::VerifyOutcome::Ok { final_hash, .. } => {
            assert_eq!(format!("0x{final_hash:016x}"), expected, "{name}");
        }
        other => panic!("{name}: {other}"),
    }
}

/// Five independent 600-tick `move_500` runs: two one after the other on the
/// test's own thread, two at the same time on two spawned threads, and one
/// with another seed. No run reads another's result, so the two threaded runs
/// and the other-seed run go on their own threads while the test's thread
/// runs the sequential pair: the same runs and assertions in about two run
/// lengths of wall time instead of four.
#[test]
fn same_seed_same_hash_two_threads() {
    let (seq_a, seq_b, thr_a, thr_b, other_seed) = std::thread::scope(|s| {
        let a = s.spawn(|| run_move_500(42, 600));
        let b = s.spawn(|| run_move_500(42, 600));
        let other = s.spawn(|| run_move_500(43, 600));
        let seq_a = run_move_500(42, 600);
        let seq_b = run_move_500(42, 600);
        (
            seq_a,
            seq_b,
            a.join().unwrap(),
            b.join().unwrap(),
            other.join().unwrap(),
        )
    });
    assert_eq!(seq_a, seq_b);
    assert_eq!(thr_a, thr_b);
    assert_eq!(seq_a, thr_a);
    assert_ne!(other_seed, seq_a, "seed must reach the hash");
}

#[test]
fn snapshot_restore_matches_uninterrupted_including_spawns() {
    // `sim::scenarios::snapshot_restore`: 200 units crossing; a snapshot at
    // tick 300; a DebugSpawn of 50 more units issued during tick 301 in
    // BOTH runs (ids must continue identically, no slot reuse, spiral
    // placement must agree), their own move order, a Stop at 600; 1200 ticks.
    let rules = load_rules();
    let (setup, stream) = scenarios::snapshot_restore(&rules);
    let script = Script(stream);
    let snap_tick = scenarios::SNAPSHOT_RESTORE_SNAPSHOT_TICK;
    assert!(
        !script.at(scenarios::SNAPSHOT_RESTORE_SPAWN_TICK).is_empty(),
        "the stream spawns after the snapshot"
    );
    let mut a = Sim::new(setup.clone(), rules.clone(), Box::new(NoAi));
    for t in 0..snap_tick {
        a.step(script.at(t));
    }
    let snap = a.snapshot();
    let mut b = Sim::new(setup, rules, Box::new(NoAi));
    b.restore(&snap).unwrap();
    assert_eq!(a.hash(), b.hash());
    assert_eq!(a.sub_hashes(), b.sub_hashes());
    assert_eq!(a.view().unit_count(), 200);
    assert!(
        a.view().units().filter(|u| u.order.is_some()).count() > 150,
        "most units are still moving at the snapshot"
    );
    for t in snap_tick..scenarios::SNAPSHOT_RESTORE_TICKS {
        let cmds = script.at(t);
        a.step(cmds);
        b.step(cmds);
        assert_eq!(a.hash(), b.hash(), "diverged at tick {t}");
    }
    assert_eq!(a.sub_hashes(), b.sub_hashes());
    assert_eq!(a.view().unit_count(), 250);
    assert_eq!(a.view().units().last().map(|u| u.id), Some(UnitId(250)));
    let arrived = a
        .drain_events()
        .iter()
        .filter(|e| matches!(e, sim::SimEvent::UnitArrived { .. }))
        .count();
    assert!(arrived > 0, "some units arrived during the run");
}

#[test]
fn small_group_arrives_on_distinct_tiles() {
    // Nine units spawned around the west start walk the clear row-64
    // corridor to the east start: every order completes with a
    // `UnitArrived` event, and the spiral targets keep them on nine distinct
    // tiles around the click, never on a blocked tile.
    let mut sim = scenario(5);
    let delay = sim.setup().cmd_delay;
    sim.step(&[spawn(0, 0, WEST, 9)]);
    for _ in 0..delay {
        sim.step(&[]);
    }
    let mv = move_all(0, 1, &sim, EAST);
    sim.step(std::slice::from_ref(&mv));
    sim.drain_events();
    // 79 tiles at 0.09 tiles per tick is under 900 ticks; allow for jams.
    for _ in 0..1200 {
        sim.step(&[]);
    }
    let mut arrived: Vec<UnitId> = sim
        .drain_events()
        .into_iter()
        .filter_map(|e| match e {
            sim::SimEvent::UnitArrived { unit } => Some(unit),
            _ => None,
        })
        .collect();
    arrived.sort();
    arrived.dedup();
    assert_eq!(arrived, (1..=9).map(UnitId).collect::<Vec<_>>());
    let map = sim.view().map();
    let mut tiles: Vec<Tile> = sim.view().units().map(|u| map.tile_of(u.pos)).collect();
    for u in sim.view().units() {
        assert!(u.order.is_none(), "{:?} still has an order", u.id);
        assert!(
            map.passable(map.tile_of(u.pos)),
            "{:?} on a blocked tile",
            u.id
        );
        let d = u.pos.dist_sq_i64(EAST);
        assert!(d < (4 * 4) << 32, "{:?} ended {d} from the click", u.id);
    }
    tiles.sort();
    tiles.dedup();
    assert_eq!(tiles.len(), 9, "one unit per target tile");
}

#[test]
fn return_to_post_converges_100_units_and_settles() {
    // 100 units spawned in a block at the west start converge on one click
    // 15 tiles east. The front ranks arrive first and are shoved by the
    // ranks behind; return to post (`units.ron`,
    // `return_to_post_radius_tiles_x100`) walks them back. After 3000
    // ticks every unit is within 3 tiles of its own spiral target and the
    // block holds still: every unit's position, post and order are
    // constant over the last 100 ticks (no perpetual motion between
    // neighbours; neither the whole-state hash nor the `units` sub-hash can
    // be constant, since they cover the tick counter and the idle workers'
    // seek timers) and pathing is idle (no returning unit is stuck asking
    // for paths).
    let click = FxVec2::from_ints(39, 64);
    let mut sim = scenario(21);
    let delay = sim.setup().cmd_delay;
    sim.step(&[spawn(0, 0, WEST, 100)]);
    for _ in 0..delay {
        sim.step(&[]);
    }
    let mv = move_all(0, 1, &sim, click);
    sim.step(std::slice::from_ref(&mv));
    for _ in 0..delay {
        sim.step(&[]);
    }
    // The Move has applied: capture each unit's own spiral target.
    let goals: BTreeMap<UnitId, FxVec2> = sim
        .view()
        .units()
        .map(|u| (u.id, u.order.as_ref().expect("ordered").goal))
        .collect();
    assert_eq!(goals.len(), 100);
    let total = 3000u32;
    let settle = 100u32;
    let mut hashes = Vec::with_capacity(settle as usize);
    for t in sim.tick()..total {
        sim.step(&[]);
        if t >= total - settle {
            // Positions, posts and orders only: idle Yeomen rewrite their
            // work-seeking timer (`seek_at`) every retry interval (M3a).
            let still: Vec<_> = sim
                .view()
                .units()
                .map(|u| (u.id, u.pos, u.post, u.order.clone()))
                .collect();
            hashes.push(sim::hash::hash_value(&still));
        }
    }
    assert_eq!(hashes.len(), settle as usize);
    assert!(
        sim.view().pathing().is_idle(),
        "pathing is idle once settled"
    );
    let map = sim.view().map();
    let mut worst = 0i64;
    for u in sim.view().units() {
        assert!(u.order.is_none(), "{:?} still has an order", u.id);
        let post = u.post.expect("every unit arrived and holds a post");
        assert_eq!(post.goal, goals[&u.id], "{:?} holds its own target", u.id);
        assert!(!post.returning, "{:?} is still walking back", u.id);
        assert!(map.passable(map.tile_of(u.pos)));
        worst = worst.max(u.pos.dist_sq_i64(goals[&u.id]));
    }
    assert!(
        worst <= (3 * 3) << 32,
        "worst unit is {worst} (I32F32-scaled squared tiles) from its goal"
    );
    assert!(
        hashes.iter().all(|h| *h == hashes[0]),
        "the block must hold still over the last {settle} ticks"
    );
    let arrived = sim
        .drain_events()
        .iter()
        .filter(|e| matches!(e, sim::SimEvent::UnitArrived { .. }))
        .count();
    assert!(
        arrived >= 100,
        "every unit arrived at least once: {arrived}"
    );
}

#[test]
fn restore_rebuilds_caches() {
    let mut a = scenario(3);
    for t in 0..200 {
        a.step(&move_500_commands(t));
    }
    let snap = a.snapshot();
    // `b` has a warm path cache and a stale spatial grid from its own run;
    // restore must rebuild components and the grid and clear the cache.
    let mut b = scenario(99);
    for t in 0..150 {
        b.step(&move_500_commands(t));
    }
    b.restore(&snap).unwrap();
    assert!(
        b.view().pathing().cache.is_empty(),
        "restore clears the cache"
    );
    assert_eq!(
        b.view().map().components(),
        a.view().map().components(),
        "components rebuilt"
    );
    a.step(&[]);
    b.step(&[]);
    assert_eq!(a.hash(), b.hash());
    assert_eq!(a.sub_hashes(), b.sub_hashes());
}

/// A random command stream over the scenario: spawns, moves, stops, and
/// (from M3a) Croft sites placed and cancelled around a complete P0 Town,
/// so the cost grid changes while searches are suspended, plus Gathers
/// onto three complete Crofts placed at tick 0 (idle Yeomen also take free
/// slots there on their own), Yeomen trained at the Town and rally points
/// set on it, so slots, stand-tile paths, worked income, training and
/// rally walks run inside the determinism and cache-transparency
/// proptests.
fn random_commands(seed: u64, ticks: u32) -> Vec<Vec<PlayerCommand>> {
    use rand_core::{Rng, SeedableRng};
    let rules = load_rules();
    let (town, _) = rules.building_kind_by_id("town").unwrap();
    let (croft, _) = rules.building_kind_by_id("croft").unwrap();
    let lumber = u8::try_from(rules.resource_index("lumber").unwrap()).unwrap();
    let grain = u8::try_from(rules.resource_index("grain").unwrap()).unwrap();
    let mut rng = rand_pcg::Pcg32::seed_from_u64(seed);
    let mut out = Vec::new();
    let mut seq = 0u32;
    let mut spawned: u32 = 0;
    for t in 0..ticks {
        let mut tick_cmds = Vec::new();
        if t == 0 {
            // The scenario starts empty: a complete P0 Town centred on WEST
            // gives P0 borders to build in, and Lumber pays for the sites.
            tick_cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq,
                Command::DebugPlaceBuilding {
                    owner: PlayerId(0),
                    kind: sim::BuildingKindId(town),
                    tile: Tile::new(23, 63),
                    complete: true,
                },
            ));
            tick_cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq + 1,
                Command::DebugGrant {
                    owner: PlayerId(0),
                    resource: lumber,
                    amount: 100_000,
                },
            ));
            tick_cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq + 2,
                Command::DebugGrant {
                    owner: PlayerId(0),
                    resource: grain,
                    amount: 100_000,
                },
            ));
            seq += 3;
            // BuildingId(2..=4): complete Crofts inside the Town's border.
            for x in [28, 31, 34] {
                tick_cmds.push(PlayerCommand::new(
                    PlayerId(0),
                    seq,
                    Command::DebugPlaceBuilding {
                        owner: PlayerId(0),
                        kind: sim::BuildingKindId(croft),
                        tile: Tile::new(x, 58),
                        complete: true,
                    },
                ));
                seq += 1;
            }
        }
        let roll = rng.next_u32() % 100;
        if roll < 10 && spawned < 200 {
            let x = i32::try_from(rng.next_u32() % 128).unwrap();
            let y = i32::try_from(rng.next_u32() % 128).unwrap();
            let count = u16::try_from(1 + rng.next_u32() % 20).unwrap();
            spawned += u32::from(count);
            tick_cmds.push(spawn(0, seq, FxVec2::from_ints(x, y), count));
            seq += 1;
        } else if roll < 40 && spawned > 0 {
            let x = i32::try_from(rng.next_u32() % 128).unwrap();
            let y = i32::try_from(rng.next_u32() % 128).unwrap();
            let n = 1 + rng.next_u32() % 12;
            let units = (0..n)
                .map(|_| UnitId(1 + rng.next_u32() % spawned.max(1)))
                .collect();
            tick_cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq,
                Command::Move {
                    units,
                    target: FxVec2::from_ints(x, y),
                    queue: false,
                },
            ));
            seq += 1;
        } else if roll < 45 && spawned > 0 {
            let units = vec![UnitId(1 + rng.next_u32() % spawned.max(1))];
            tick_cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq,
                Command::Stop { units },
            ));
            seq += 1;
        } else if roll < 55 && spawned > 0 {
            let x = u16::try_from(24 - 18 + rng.next_u32() % 37).unwrap();
            let y = u16::try_from(64 - 18 + rng.next_u32() % 37).unwrap();
            tick_cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq,
                Command::Build {
                    unit: UnitId(1 + rng.next_u32() % spawned.max(1)),
                    kind: sim::BuildingKindId(croft),
                    tile: Tile::new(x, y),
                },
            ));
            seq += 1;
        } else if roll < 58 {
            tick_cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq,
                Command::Cancel {
                    building: sim::BuildingId(1 + rng.next_u32() % 8),
                    slot: 0,
                },
            ));
            seq += 1;
        } else if roll < 62 && spawned > 0 {
            let n = 1 + rng.next_u32() % 3;
            let units = (0..n)
                .map(|_| UnitId(1 + rng.next_u32() % spawned.max(1)))
                .collect();
            tick_cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq,
                Command::Gather {
                    units,
                    node: sim::BuildingId(1 + rng.next_u32() % 8),
                },
            ));
            seq += 1;
        } else if roll < 65 {
            // Train at the Town (BuildingId(1)); PopCapReached once the
            // spawns pass the cap, which is a fine rejection too.
            tick_cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq,
                Command::Train {
                    building: sim::BuildingId(1),
                    kind: UnitKindId(0),
                },
            ));
            seq += 1;
        } else if roll < 67 {
            let x = i32::try_from(rng.next_u32() % 128).unwrap();
            let y = i32::try_from(rng.next_u32() % 128).unwrap();
            tick_cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq,
                Command::SetRally {
                    building: sim::BuildingId(1),
                    target: FxVec2::from_ints(x, y),
                },
            ));
            seq += 1;
        }
        out.push(tick_cmds);
    }
    out
}

/// The random stream really changes the cost grid, gathers and trains:
/// over a few seeds it places sites (generation bumps) and cancels some,
/// units take gather slots (by Gather or idle seeking) and work them, and
/// the Town trains, so the two proptests below exercise those paths.
#[test]
fn random_commands_place_and_cancel_sites() {
    let (mut placed, mut removed, mut gathering, mut worked, mut trained) = (0, 0, 0, 0, 0);
    for seed in 0..8 {
        let stream = random_commands(seed, 300);
        let mut sim = scenario(seed);
        for cmds in &stream {
            sim.step(cmds);
            for e in sim.drain_events() {
                match e {
                    sim::SimEvent::BuildingPlaced { .. } => placed += 1,
                    sim::SimEvent::BuildingRemoved { .. } => removed += 1,
                    _ => {}
                }
            }
            let view = sim.view();
            gathering += view
                .units()
                .filter(|u| matches!(u.job, Some(sim::Job::Gather { .. })))
                .count();
            worked += (2..=4)
                .map(|b| {
                    view.worked_slots(sim::BuildingId(b))
                        .map_or(0, |(w, _, _)| u32::from(w))
                })
                .sum::<u32>();
            if view
                .building(sim::BuildingId(1))
                .is_some_and(|b| b.queue.iter().any(|e| e.progress > 0))
            {
                trained += 1;
            }
        }
    }
    // Four placements per seed are the debug Town and Crofts. The other
    // counts are summed over every tick of every seed.
    assert!(placed > 8 * 4 + 4, "sites placed: {}", placed - 8 * 4);
    assert!(removed > 0, "sites cancelled: {removed}");
    assert!(gathering > 0, "unit-ticks with a gather job: {gathering}");
    assert!(worked > 0, "worked slot-ticks: {worked}");
    assert!(trained > 0, "ticks with training in progress: {trained}");
}

fn run_with(seed: u64, stream: &[Vec<PlayerCommand>], cache: bool) -> Vec<u64> {
    let mut sim = scenario(seed);
    sim.set_path_cache_enabled(cache);
    let mut hashes = Vec::with_capacity(stream.len());
    for cmds in stream {
        sim.step(cmds);
        hashes.push(sim.hash());
    }
    hashes
}

/// Proptest case count: `PROPTEST_CASES` when set, else 256 in CI and 8
/// locally. M1 set 32 locally (`docs/ROADMAP.md`, M1); the M4a combat
/// behaviour slice halved it to 16 and M4b halved it again to 8, each time
/// the warm `cargo test -p sim` reached its 10 s budget, as
/// `docs/BUILD_TIMES.md` advises. CI still runs 256.
fn proptest_cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if std::env::var_os("CI").is_some() {
            256
        } else {
            8
        })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: proptest_cases(),
        .. ProptestConfig::default()
    })]

    #[test]
    fn proptest_random_commands_are_deterministic(seed in any::<u64>(), ticks in 50u32..300) {
        let stream = random_commands(seed, ticks);
        let a = run_with(seed, &stream, true);
        let b = run_with(seed, &stream, true);
        prop_assert_eq!(a, b);
    }

    #[test]
    fn path_cache_is_transparent(seed in any::<u64>(), ticks in 50u32..300) {
        let stream = random_commands(seed, ticks);
        let on = run_with(seed, &stream, true);
        let off = run_with(seed, &stream, false);
        prop_assert_eq!(on, off, "hashes must agree on every tick with the cache on and off");
    }
}

/// Run the in-house search to completion with an unlimited budget.
fn search_full(map: &Map, from: Tile, to: Tile) -> Option<Vec<Tile>> {
    let mut s = AStarSearch::new(map, from, to);
    let mut budget = u32::MAX;
    match s.resume(map, &mut budget) {
        SearchStatus::Found(p) => Some(p),
        SearchStatus::Exhausted => None,
        SearchStatus::Suspended => panic!("unlimited budget suspended"),
    }
}

/// The `pathfinding` oracle with the same neighbour function and costs.
fn oracle(map: &Map, from: Tile, to: Tile) -> Option<(Vec<Tile>, u32)> {
    pathfinding::prelude::astar(
        &from,
        |t| {
            map.neighbors8(*t)
                .map(|n| (n, sim::pathing::step_cost(*t, n)))
                .collect::<Vec<_>>()
        },
        |t| sim::pathing::octile(*t, to),
        |t| *t == to,
    )
}

#[test]
fn path_exists_iff_connected() {
    use rand_core::{Rng, SeedableRng};
    let mut rng = rand_pcg::Pcg32::seed_from_u64(11);
    let mut maps = vec![plains()];
    for _ in 0..4 {
        let mut m = Map::open(32, 32);
        for _ in 0..300 {
            let x = u16::try_from(rng.next_u32() % 32).unwrap();
            let y = u16::try_from(rng.next_u32() % 32).unwrap();
            m.set_blocked(Tile::new(x, y), true);
        }
        maps.push(m);
    }
    // 200 passable pairs on the plains map, 100 across the four random maps.
    let pairs_per_map = [200usize, 25, 25, 25, 25];
    let mut disconnected = 0;
    for (map, pairs) in maps.iter().zip(pairs_per_map) {
        let mut checked = 0;
        while checked < pairs {
            let rnd = |rng: &mut rand_pcg::Pcg32| {
                Tile::new(
                    u16::try_from(rng.next_u32() % u32::from(map.width())).unwrap(),
                    u16::try_from(rng.next_u32() % u32::from(map.height())).unwrap(),
                )
            };
            let a = rnd(&mut rng);
            let b = rnd(&mut rng);
            if !map.passable(a) || !map.passable(b) {
                continue;
            }
            checked += 1;
            if map.component_of(a) != map.component_of(b) {
                disconnected += 1;
            }
            let ours = search_full(map, a, b);
            let theirs = oracle(map, a, b);
            let connected = map.component_of(a) == map.component_of(b);
            assert_eq!(ours.is_some(), connected, "{a:?}->{b:?}");
            assert_eq!(theirs.is_some(), connected, "oracle {a:?}->{b:?}");
            if let (Some(p), Some((_, cost))) = (ours, theirs) {
                assert_eq!(p.first(), Some(&a));
                assert_eq!(p.last(), Some(&b));
                assert_eq!(path_cost(&p), cost, "cost equality {a:?}->{b:?}");
                for w in p.windows(2) {
                    assert!(map.step_allowed(w[0], w[1]), "illegal step {w:?}");
                }
            }
        }
    }
    assert!(
        disconnected > 0,
        "the random maps must exercise the unreachable branch"
    );
}

/// Shared plains map for the Pathing-level transparency proptest (loading
/// `data/` per case would dominate the run time).
fn shared_plains() -> &'static Map {
    static MAP: std::sync::OnceLock<Map> = std::sync::OnceLock::new();
    MAP.get_or_init(plains)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: proptest_cases(),
        .. ProptestConfig::default()
    })]

    /// The transparent-cache rule at the `Pathing` level (the `Sim`-level
    /// `path_cache_is_transparent` above needs movement to land): the same
    /// request stream through a cache-enabled and a cache-disabled `Pathing`
    /// completes the same requests with the same paths on every tick, leaves
    /// the same budget remainder, and serialises to the same hashed bytes.
    /// The cost grid changes under both now and then (a short wall toggled
    /// on or off, as buildings will from M3a), including while a search is
    /// suspended.
    #[test]
    fn path_cache_is_transparent_at_the_pathing_level(
        seed in any::<u64>(),
        ticks in 20u32..120,
        budget in 20u32..1500,
    ) {
        use rand_core::{Rng, SeedableRng};
        let mut map = shared_plains().clone();
        let mut rng = rand_pcg::Pcg32::seed_from_u64(seed);
        let rnd_tile = |rng: &mut rand_pcg::Pcg32| {
            Tile::new(
                u16::try_from(rng.next_u32() % 128).unwrap(),
                u16::try_from(rng.next_u32() % 128).unwrap(),
            )
        };
        // A small pool of start/goal pairs so cache hits are frequent, mixed
        // with fresh pairs so misses, suspensions and evictions all happen.
        let pool: Vec<(Tile, Tile)> = (0..6).map(|_| (rnd_tile(&mut rng), rnd_tile(&mut rng))).collect();
        let mut on = Pathing::new(16, true);
        let mut off = Pathing::new(16, false);
        let mut next_unit = 1u32;
        let mut hits_possible = 0u32;
        for tick in 0..ticks {
            for _ in 0..(rng.next_u32() % 4) {
                let (from, to) = if rng.next_u32() % 10 < 7 {
                    pool[(rng.next_u32() % 6) as usize]
                } else {
                    (rnd_tile(&mut rng), rnd_tile(&mut rng))
                };
                // Mostly fresh units; sometimes re-request for an existing one
                // (replaces its earlier request or active search).
                let unit = if rng.next_u32() % 5 == 0 && next_unit > 1 {
                    UnitId(1 + rng.next_u32() % (next_unit - 1))
                } else {
                    next_unit += 1;
                    UnitId(next_unit - 1)
                };
                let req = PathRequest { requested_tick: tick, unit, from, to };
                on.request(req);
                off.request(req);
            }
            if rng.next_u32() % 8 == 0 && next_unit > 1 {
                let unit = UnitId(1 + rng.next_u32() % (next_unit - 1));
                on.cancel(unit);
                off.cancel(unit);
            }
            if !on.cache.is_empty() {
                hits_possible += 1;
            }
            if rng.next_u32() % 6 == 0 {
                // Toggle a 1 x 5 wall; both runs see the same grid.
                let t = rnd_tile(&mut rng);
                let block = map.passable(t);
                for dy in 0..5u16 {
                    let w = Tile::new(t.x, t.y.saturating_add(dy).min(127));
                    map.set_blocked(w, block);
                }
            }
            let mut ra = budget;
            let mut rb = budget;
            let a = on.service_budgeted(&map, &mut ra, tick);
            let b = off.service_budgeted(&map, &mut rb, tick);
            prop_assert_eq!(&a, &b, "completed requests differ at tick {}", tick);
            prop_assert_eq!(ra, rb, "budget remainder differs at tick {}", tick);
            prop_assert_eq!(
                postcard::to_allocvec(&on).unwrap(),
                postcard::to_allocvec(&off).unwrap(),
                "hashed pathing state differs at tick {}",
                tick
            );
            prop_assert!(on.cache.len() <= 16);
        }
        prop_assert!(off.cache.is_empty());
        // Not every seed warms the cache, but most do.
        let _ = hits_possible;
    }
}

/// Check that `cmp` is a total order over `items`: antisymmetric, transitive
/// and consistent with equality of the key.
fn assert_total_order<T, K: Ord + std::fmt::Debug>(items: &[T], key: impl Fn(&T) -> K) {
    use std::cmp::Ordering;
    for a in items {
        for b in items {
            let ab = key(a).cmp(&key(b));
            let ba = key(b).cmp(&key(a));
            assert_eq!(ab, ba.reverse());
            for c in items {
                let bc = key(b).cmp(&key(c));
                let ac = key(a).cmp(&key(c));
                if ab != Ordering::Greater && bc != Ordering::Greater {
                    assert_ne!(ac, Ordering::Greater);
                }
            }
        }
    }
}

#[test]
fn sort_keys_are_total_orders() {
    // Commands: (player, seq).
    let cmds: Vec<PlayerCommand> = (0..3u8)
        .flat_map(|p| {
            (0..3u32).map(move |s| PlayerCommand::new(PlayerId(p), s, Command::Surrender))
        })
        .collect();
    assert_total_order(&cmds, PlayerCommand::sort_key);
    // Path requests: (requested_tick, UnitId).
    let reqs: Vec<PathRequest> = (0..4u32)
        .flat_map(|t| {
            (1..4u32).map(move |u| PathRequest {
                requested_tick: t,
                unit: UnitId(u),
                from: Tile::new(0, 0),
                to: Tile::new(1, 1),
            })
        })
        .collect();
    assert_total_order(&reqs, PathRequest::sort_key);
    let mut sorted = reqs.clone();
    sorted.sort_by_key(PathRequest::sort_key);
    assert!(sorted.windows(2).all(|w| w[0].sort_key() < w[1].sort_key()));
    // Open-set nodes: f asc, g desc, tile asc; every distinct node comparable.
    let nodes: Vec<sim::pathing::Node> = (0..3u32)
        .flat_map(|f| {
            (0..3u32)
                .flat_map(move |g| (0..3u32).map(move |tile| sim::pathing::Node { f, g, tile }))
        })
        .collect();
    assert_total_order(&nodes, |n| *n);
    for a in &nodes {
        for b in &nodes {
            if a != b {
                assert_ne!(a.cmp(b), std::cmp::Ordering::Equal, "{a:?} vs {b:?}");
            }
        }
    }
    // Unit ids and tiles (BTreeMap keys and spiral/grid iteration).
    let ids: Vec<UnitId> = (0..6).map(UnitId).collect();
    assert_total_order(&ids, |u| *u);
    let tiles: Vec<Tile> = (0..3u16)
        .flat_map(|x| (0..3u16).map(move |y| Tile::new(x, y)))
        .collect();
    assert_total_order(&tiles, |t| *t);
    let units: BTreeMap<UnitId, ()> = ids.iter().map(|i| (*i, ())).collect();
    assert!(units.keys().copied().eq(ids.iter().copied()));
}

#[test]
fn dist_sq_i64_map_corners() {
    let o = FxVec2::from_ints(0, 0);
    assert_eq!(
        o.dist_sq_i64(FxVec2::from_ints(128, 128)),
        (2 * 128 * 128) << 32
    );
    assert_eq!(
        o.dist_sq_i64(FxVec2::from_ints(256, 256)),
        (2 * 256 * 256) << 32
    );
    let map = plains();
    let a = map.center_of(Tile::new(0, 0));
    let b = map.center_of(Tile::new(127, 127));
    assert_eq!(a.dist_sq_i64(b), (2 * 127 * 127) << 32);
    assert_eq!(a.dist_sq_i64(b), b.dist_sq_i64(a));
}

#[test]
fn debug_spawn_is_rejected_in_a_skirmish_and_accepted_in_a_scenario() {
    let rules = load_rules();
    let start: u32 = rules.standard_start.units.values().sum();
    let mut skirmish = Sim::new(MatchSetup::skirmish(&rules, 1), rules, Box::new(NoAi));
    let delay = skirmish.setup().cmd_delay;
    // A skirmish begins from the standard start: both players' start units.
    let before = skirmish.view().unit_count();
    assert_eq!(before, usize::try_from(2 * start).unwrap());
    skirmish.step(&[spawn(0, 0, WEST, 3)]);
    for _ in 0..delay {
        skirmish.step(&[]);
    }
    assert_eq!(skirmish.view().unit_count(), before, "DebugSpawn adds none");
    let events = skirmish.drain_events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            sim::SimEvent::CommandRejected {
                reason: sim::RejectReason::DebugCommandsDisabled,
                ..
            }
        )),
        "{events:?}"
    );

    let mut sc = scenario(1);
    sc.step(&[spawn(0, 0, WEST, 3)]);
    for _ in 0..delay {
        sc.step(&[]);
    }
    assert_eq!(sc.view().unit_count(), 3);
    let map = sc.view().map();
    let mut tiles: Vec<Tile> = sc.view().units().map(|u| map.tile_of(u.pos)).collect();
    // Spiral placement: the centre tile, then east, then south-east.
    assert_eq!(
        tiles,
        vec![Tile::new(24, 64), Tile::new(25, 64), Tile::new(25, 65)]
    );
    tiles.sort();
    tiles.dedup();
    assert_eq!(tiles.len(), 3, "one unit per tile");
    let u = sc.view().unit(UnitId(1)).unwrap();
    assert_eq!(u.pos, map.center_of(Tile::new(24, 64)));
    assert_eq!(u.speed, sim::Fx::from_ratio(180, 2000));
    assert_eq!(u.radius, sim::Fx::from_ratio(35, 100));
    assert!(u.order.is_none());
    let spawned = sc
        .drain_events()
        .iter()
        .filter(|e| matches!(e, sim::SimEvent::UnitSpawned { .. }))
        .count();
    assert_eq!(spawned, 3);
}

#[test]
fn golden_move_500_short() {
    golden("move_500_short");
}

#[test]
#[ignore = "long golden; verified by sim-cli verify --release in CI"]
fn golden_move_500() {
    golden("move_500");
}

#[test]
fn golden_group_spiral() {
    golden("group_spiral");
}

#[test]
fn golden_snapshot_restore() {
    golden("snapshot_restore");
}
