//! M0 determinism tests: same inputs give the same hash on any thread, and a
//! snapshot restores exactly.

use sim::{
    AiController, Command, FxVec2, MatchSetup, PlayerCommand, PlayerId, Rules, Sim, SimView, UnitId,
};
use std::path::Path;

fn load_rules() -> Rules {
    Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).expect("data/ loads")
}

/// A fixed, tick-dependent command stream for the human slot.
fn human_commands(tick: u32) -> Vec<PlayerCommand> {
    let p = PlayerId(0);
    let mut out = Vec::new();
    if tick.is_multiple_of(7) {
        let t = i32::try_from(tick % 128).unwrap();
        out.push(PlayerCommand::new(
            p,
            tick * 2,
            Command::Move {
                units: vec![UnitId(1), UnitId(2)],
                target: FxVec2::from_ints(t, 127 - t),
                queue: tick.is_multiple_of(14),
            },
        ));
    }
    if tick % 50 == 25 {
        out.push(PlayerCommand::new(
            p,
            tick * 2 + 1,
            Command::Stop {
                units: vec![UnitId(2)],
            },
        ));
    }
    out
}

/// A bot that stops its (future) units every 10 ticks and resigns at tick 900.
struct TestBot;

impl AiController for TestBot {
    fn think(&mut self, _player: PlayerId, view: &SimView<'_>) -> Vec<Command> {
        let mut out = Vec::new();
        if view.tick % 10 == 3 {
            out.push(Command::Stop {
                units: vec![UnitId(9)],
            });
        }
        if view.tick == 900 {
            out.push(Command::Surrender);
        }
        out
    }
}

fn new_sim(seed: u64) -> Sim {
    let rules = load_rules();
    let setup = MatchSetup::skirmish(&rules, seed);
    Sim::new(setup, rules, Box::new(TestBot))
}

fn run(seed: u64, ticks: u32) -> u64 {
    let mut sim = new_sim(seed);
    for t in 0..ticks {
        sim.step(&human_commands(t));
    }
    assert_eq!(sim.tick(), ticks);
    sim.hash()
}

#[test]
fn same_seed_same_hash_two_threads() {
    let seq_a = run(42, 1000);
    let seq_b = run(42, 1000);
    assert_eq!(seq_a, seq_b, "sequential runs differ");

    let (thr_a, thr_b) = std::thread::scope(|s| {
        let a = s.spawn(|| run(42, 1000));
        let b = s.spawn(|| run(42, 1000));
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_eq!(thr_a, thr_b, "threaded runs differ");
    assert_eq!(seq_a, thr_a, "threaded run differs from sequential run");

    // The hash is sensitive to the inputs it claims to cover.
    assert_ne!(run(43, 1000), seq_a, "different seed must change the hash");
    let mut quiet = new_sim(42);
    for _ in 0..1000 {
        quiet.step(&[]);
    }
    assert_ne!(quiet.hash(), seq_a, "commands must change the hash");
}

#[test]
fn snapshot_restore_roundtrip() {
    let mut a = new_sim(7);
    for t in 0..300 {
        a.step(&human_commands(t));
    }
    let snap = a.snapshot();
    let hash_at_300 = a.hash();

    let mut b = new_sim(7);
    b.restore(&snap).expect("snapshot restores");
    assert_eq!(b.tick(), 300);
    assert_eq!(
        b.hash(),
        hash_at_300,
        "restore reproduces the hash immediately"
    );
    assert_eq!(
        b.snapshot(),
        snap,
        "snapshot of a restored sim is byte-identical"
    );

    for t in 300..1000 {
        a.step(&human_commands(t));
        b.step(&human_commands(t));
        assert_eq!(a.hash(), b.hash(), "diverged at tick {t}");
    }
    assert_ne!(a.hash(), hash_at_300);

    let mut c = new_sim(7);
    assert!(
        c.restore(&[0xff, 0x00, 0x13]).is_err(),
        "garbage must not restore"
    );
    assert_eq!(c.tick(), 0, "failed restore leaves the sim unchanged");
}

#[test]
fn commands_apply_after_cmd_delay_and_surrender_is_honoured() {
    let mut sim = new_sim(1);
    let delay = sim.setup().cmd_delay;
    assert!(delay >= 1);
    let human = PlayerCommand::new(PlayerId(0), 0, Command::Surrender);
    sim.step(std::slice::from_ref(&human));
    for _ in 0..delay - 1 {
        assert!(!sim.view().player(PlayerId(0)).unwrap().surrendered);
        sim.step(&[]);
    }
    sim.step(&[]);
    assert!(
        sim.view().player(PlayerId(0)).unwrap().surrendered,
        "applied at issue tick + cmd_delay"
    );

    // The bot resigns at tick 900; its command lands `delay` ticks later.
    let mut sim = new_sim(1);
    for _ in 0..=900 + delay {
        sim.step(&[]);
    }
    assert!(sim.view().player(PlayerId(1)).unwrap().surrendered);
    assert!(!sim.view().player(PlayerId(0)).unwrap().surrendered);
}

#[test]
fn commands_sort_total_order() {
    let mk = |p: u8, s: u32| PlayerCommand::new(PlayerId(p), s, Command::Surrender);
    let mut cmds = vec![mk(1, 5), mk(0, 3), mk(1, 1), mk(0, 0), mk(3, 2), mk(2, 2)];
    sim::sort_commands(&mut cmds);
    assert!(cmds.windows(2).all(|w| w[0].sort_key() < w[1].sort_key()));
    // Application order is independent of arrival order.
    let mut rev = cmds.clone();
    rev.reverse();
    sim::sort_commands(&mut rev);
    assert_eq!(rev, cmds);
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
    assert_eq!(
        FxVec2::from_ints(256, 0).dist_sq_i64(FxVec2::from_ints(0, 256)),
        (2 * 256 * 256) << 32
    );
}

#[test]
fn match_setup_matches_rules() {
    let rules = load_rules();
    let setup = MatchSetup::skirmish(&rules, 5);
    assert_eq!(setup.sim_version, sim::SIM_VERSION);
    assert_eq!(setup.rules_hash, rules.rules_hash());
    assert_eq!(setup.map, "plains_1v1");
    assert_eq!(setup.tick_rate_hz, 20);
    assert_eq!(setup.cmd_delay, 2);
    assert_eq!(setup.players.len(), 2);
}
