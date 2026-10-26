//! M2 sim-side tests: the `AttackMove` placeholder and the M2 scenario
//! streams (`units200`, `units200_auto`, `scripted_moves`, `hud_click`).

use sim::scenarios::{self, Stream};
use sim::{
    Command, FxVec2, Map, MatchSetup, PlayerCommand, PlayerId, Rules, Sim, SimEvent, SimView,
    UnitId, UnitKindId,
};
use std::path::{Path, PathBuf};

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

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
}

fn load_rules() -> Rules {
    Rules::load(data_dir()).expect("data/ loads")
}

fn scenario(seed: u64) -> Sim {
    let rules = load_rules();
    let setup = MatchSetup::scenario(&rules, seed);
    Sim::new(setup, rules, Box::new(NoAi))
}

fn spawn(count: u16) -> PlayerCommand {
    PlayerCommand::new(
        PlayerId(0),
        0,
        Command::DebugSpawn {
            owner: PlayerId(0),
            kind: UnitKindId(0),
            at: scenarios::WEST,
            count,
        },
    )
}

fn ids(n: u32) -> Vec<UnitId> {
    (1..=n).map(UnitId).collect()
}

/// Run `n` spawned units for 120 ticks under `order` issued during tick 1.
fn run_with(order: Command) -> (Sim, Vec<SimEvent>) {
    let mut sim = scenario(5);
    sim.step(&[spawn(20)]);
    sim.step(&[PlayerCommand::new(PlayerId(0), 1, order)]);
    for _ in 0..120 {
        sim.step(&[]);
    }
    let events = sim.drain_events();
    (sim, events)
}

#[test]
fn attack_move_is_applied_exactly_like_move() {
    let target = FxVec2::from_ints(40, 64);
    let (moved, move_events) = run_with(Command::Move {
        units: ids(20),
        target,
        queue: false,
    });
    let (attacked, attack_events) = run_with(Command::AttackMove {
        units: ids(20),
        target,
    });
    assert_eq!(moved.hash(), attacked.hash(), "same orders, same state");
    assert_eq!(moved.sub_hashes(), attacked.sub_hashes());
    let rejected = |events: &[SimEvent]| {
        events
            .iter()
            .any(|e| matches!(e, SimEvent::CommandRejected { .. }))
    };
    assert!(!rejected(&move_events), "{move_events:?}");
    assert!(!rejected(&attack_events), "{attack_events:?}");
    // Something actually moved: nobody is still at the spawn column.
    assert!(attacked.view().units().all(|u| u.pos.x > scenarios::WEST.x));
}

#[test]
fn attack_move_with_no_owned_units_is_rejected_not_panicking() {
    let mut sim = scenario(5);
    sim.step(&[spawn(3)]);
    // Player 1 owns nothing; the command is counted and rejected.
    sim.step(&[PlayerCommand::new(
        PlayerId(1),
        0,
        Command::AttackMove {
            units: ids(3),
            target: scenarios::EAST,
        },
    )]);
    for _ in 0..4 {
        sim.step(&[]);
    }
    let events = sim.drain_events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            SimEvent::CommandRejected {
                player: PlayerId(1),
                reason: sim::RejectReason::NoValidUnits,
                ..
            }
        )),
        "{events:?}"
    );
}

fn moves_of(stream: &Stream) -> Vec<(u32, FxVec2)> {
    stream
        .iter()
        .flat_map(|(t, cmds)| {
            cmds.iter().filter_map(move |c| match &c.cmd {
                Command::Move { target, .. } | Command::AttackMove { target, .. } => {
                    Some((*t, *target))
                }
                _ => None,
            })
        })
        .collect()
}

#[test]
fn m2_scenarios_resolve_with_their_lengths() {
    let rules = load_rules();
    for (name, ticks) in [
        ("units200", scenarios::UNITS200_TICKS),
        ("units200_auto", scenarios::UNITS200_AUTO_TICKS),
        ("scripted_moves", scenarios::SCRIPTED_MOVES_TICKS),
        ("hud_click", scenarios::HUD_CLICK_TICKS),
    ] {
        let (setup, stream, len) =
            scenarios::by_name(name, &rules).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(len, ticks, "{name}");
        assert!(setup.debug_commands, "{name}");
        assert!(scenarios::NAMES.contains(&name), "{name} is listed");
        assert!(
            !scenarios::commands_at(&stream, 0).is_empty(),
            "{name} spawns at 0"
        );
    }
}

#[test]
fn units200_spawns_200_yeomen_and_nothing_else() {
    let rules = load_rules();
    let (setup, stream) = scenarios::units200(&rules);
    assert_eq!(
        stream.len(),
        1,
        "only the tick-0 spawn; the player interacts"
    );
    let delay = setup.cmd_delay;
    let sim = scenarios::run(setup, rules, &stream, delay + 1);
    assert_eq!(sim.view().unit_count(), 200);
    assert!(sim.view().units().all(|u| u.order.is_none()));
}

#[test]
fn units200_auto_orders_everyone_east_during_tick_20() {
    let rules = load_rules();
    let (setup, stream) = scenarios::units200_auto(&rules);
    let moves = moves_of(&stream);
    assert_eq!(
        moves,
        vec![(scenarios::UNITS200_AUTO_MOVE_TICK, scenarios::EAST)]
    );
    let Command::Move { units, .. } =
        &scenarios::commands_at(&stream, scenarios::UNITS200_AUTO_MOVE_TICK)[0].cmd
    else {
        panic!("tick 20 is a Move");
    };
    assert_eq!(units.len(), 200);
    let delay = setup.cmd_delay;
    let sim = scenarios::run(
        setup,
        rules,
        &stream,
        scenarios::UNITS200_AUTO_MOVE_TICK + delay + 1,
    );
    assert_eq!(sim.view().unit_count(), 200);
    assert!(sim.view().units().all(|u| u.order.is_some()));
}

#[test]
fn scripted_moves_targets_are_passable_and_the_stream_runs_clean() {
    let rules = load_rules();
    let map = Map::from_def(rules.map(&rules.default_map).unwrap());
    let (setup, stream) = scenarios::scripted_moves(&rules);
    let moves = moves_of(&stream);
    assert!(moves.len() >= 3, "several scripted orders: {moves:?}");
    for (t, target) in &moves {
        assert!(
            map.passable(map.tile_of(*target)),
            "tick {t} target {target:?}"
        );
    }
    assert!(
        stream.iter().any(|(_, cmds)| cmds
            .iter()
            .any(|c| matches!(c.cmd, Command::AttackMove { .. }))),
        "the stream exercises the AttackMove placeholder"
    );
    let mut sim = scenarios::run(setup, rules, &stream, scenarios::SCRIPTED_MOVES_TICKS);
    let events = sim.drain_events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SimEvent::CommandRejected { .. })),
        "{events:?}"
    );
    assert_eq!(sim.tick(), scenarios::SCRIPTED_MOVES_TICKS);
}

#[test]
fn scripted_moves_hash_is_independent_of_how_ticks_are_batched() {
    // The game steps 0..8 ticks per frame; the stream is tick-stamped so
    // the batching cannot matter. Simulate two batchings by hand.
    let rules = load_rules();
    let (setup, stream) = scenarios::scripted_moves(&rules);
    let a = scenarios::run(
        setup.clone(),
        rules.clone(),
        &stream,
        scenarios::SCRIPTED_MOVES_TICKS,
    );
    let mut b = Sim::new(setup, rules, Box::new(NoAi));
    let mut t = 0;
    let mut burst = 1;
    while t < scenarios::SCRIPTED_MOVES_TICKS {
        for _ in 0..burst {
            if t >= scenarios::SCRIPTED_MOVES_TICKS {
                break;
            }
            b.step(scenarios::commands_at(&stream, t));
            t += 1;
        }
        burst = burst % 8 + 1;
    }
    assert_eq!(a.hash(), b.hash());
}

#[test]
fn hud_click_spawns_20_units() {
    let rules = load_rules();
    let (setup, stream) = scenarios::hud_click(&rules);
    assert_eq!(stream.len(), 1);
    let delay = setup.cmd_delay;
    let sim = scenarios::run(setup, rules, &stream, delay + 1);
    assert_eq!(sim.view().unit_count(), 20);
}
