//! M3b acceptance tests, sim side: the `economy_walkthrough` scenario that
//! the owner proxy replays (`--scenario economy_walkthrough`) and its golden
//! fixture. Test names start with `economy_` or `golden_` so that
//! `cargo test -p sim economy` selects them (the filter matches test paths,
//! not integration-test file names).

use rules::{Rules, Slots};
use sim::scenarios::{
    self, ECONOMY_WALKTHROUGH_AMBER_TICK, ECONOMY_WALKTHROUGH_EXPAND_TICK,
    ECONOMY_WALKTHROUGH_GRANT_TICK, ECONOMY_WALKTHROUGH_OUTSIDE_CROFT,
    ECONOMY_WALKTHROUGH_OUTSIDE_TICK, ECONOMY_WALKTHROUGH_POPCAP_TICK,
    ECONOMY_WALKTHROUGH_REJECT_TICK, ECONOMY_WALKTHROUGH_SPAWN_TICK, ECONOMY_WALKTHROUGH_TICKS,
    ECONOMY_WALKTHROUGH_TOWN, ECONOMY_WALKTHROUGH_TRAIN_TICK,
};
use sim::{
    BuildingId, BuildingKindId, Command, Construction, PlayerCommand, PlayerId, RejectReason, Sim,
    SimEvent, SimView, StartKind, Tile, UnitId, UnitKindId,
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
/// `fixtures/<name>.hash` (as `m1.rs` and `m3a.rs` do).
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

fn kind(rules: &Rules, id: &str) -> BuildingKindId {
    BuildingKindId(
        rules
            .building_kind_by_id(id)
            .expect("kind in buildings.ron")
            .0,
    )
}

fn unit_kind(rules: &Rules, id: &str) -> UnitKindId {
    let i = rules
        .units
        .iter()
        .position(|u| u.id == id)
        .expect("kind in units.ron");
    UnitKindId(u16::try_from(i).expect("validated unit count"))
}

/// Every `Build` in `cmds`: `(seq, unit, kind, tile)`.
fn builds(cmds: &[PlayerCommand]) -> Vec<(u32, UnitId, BuildingKindId, Tile)> {
    cmds.iter()
        .filter_map(|c| match c.cmd {
            Command::Build { unit, kind, tile } => Some((c.seq, unit, kind, tile)),
            _ => None,
        })
        .collect()
}

/// The seq of the one `Train` in `cmds`.
fn train_seq(cmds: &[PlayerCommand]) -> u32 {
    let trains: Vec<u32> = cmds
        .iter()
        .filter(|c| matches!(c.cmd, Command::Train { .. }))
        .map(|c| c.seq)
        .collect();
    assert_eq!(trains.len(), 1, "{cmds:?}");
    trains[0]
}

/// The M3b owner-proxy walkthrough, beat by beat, with the real sim and
/// only `SimView` reads: a Croft outside the Seat's border is rejected
/// with `OutsideBorders`; the same tile stays outside until the second
/// Town completes, which grows P0's territory over it, after which the
/// Croft is accepted there; six worked Crofts over two Towns put the Grain
/// income over the Yield Cap (raw > credited, the HUD's amber); Yeomen fill
/// the pop to one below the cap, a `Train` reaches it and the next `Train`
/// is rejected with `PopCapReached`. Every number comes from the data, and
/// every beat happens by the tick constant the game crate screenshots.
#[test]
fn economy_walkthrough_beats_happen_in_order() {
    let rules = load_rules();
    let (setup, stream) = scenarios::economy_walkthrough(&rules);
    assert_eq!(setup.start, StartKind::Standard);
    let delay = setup.cmd_delay;
    let ticks = ECONOMY_WALKTHROUGH_TICKS;
    let beats = [
        ECONOMY_WALKTHROUGH_OUTSIDE_TICK,
        ECONOMY_WALKTHROUGH_GRANT_TICK,
        ECONOMY_WALKTHROUGH_EXPAND_TICK,
        ECONOMY_WALKTHROUGH_AMBER_TICK,
        ECONOMY_WALKTHROUGH_SPAWN_TICK,
        ECONOMY_WALKTHROUGH_TRAIN_TICK,
        ECONOMY_WALKTHROUGH_REJECT_TICK,
        ECONOMY_WALKTHROUGH_POPCAP_TICK,
        ticks,
    ];
    assert!(beats.windows(2).all(|w| w[0] < w[1]), "{beats:?}");
    assert!(delay < ECONOMY_WALKTHROUGH_OUTSIDE_TICK);
    assert!(ECONOMY_WALKTHROUGH_REJECT_TICK + delay < ECONOMY_WALKTHROUGH_POPCAP_TICK);
    assert!(ticks <= 5000, "the golden runs in cargo test (rule 18)");

    let p0 = PlayerId(0);
    let seat = BuildingId(1);
    let (town, croft) = (kind(&rules, "town"), kind(&rules, "croft"));
    let yeoman = unit_kind(&rules, "yeoman");
    let croft_def = rules.building_kind(croft.0).expect("croft");
    let town_def = rules.building_kind(town.0).expect("town");
    let yeoman_pop = rules.unit_kind(yeoman.0).expect("yeoman").pop;
    let grain = rules.resource_index("grain").expect("grain");
    let cap = rules.pop_cap_base;
    assert!(
        stream.iter().flat_map(|(_, c)| c).all(|c| c.player == p0),
        "only P0 acts"
    );

    // The tick-0 Croft at the outside tile, and the later one at the same
    // tile; the Train that must be rejected.
    let outside = ECONOMY_WALKTHROUGH_OUTSIDE_CROFT;
    let at_outside = |t: u32| {
        builds(scenarios::commands_at(&stream, t))
            .into_iter()
            .find(|b| b.3 == outside)
            .unwrap_or_else(|| panic!("a Croft at {outside:?} during tick {t}"))
    };
    let (outside_seq, outside_unit, outside_kind, _) = at_outside(0);
    assert_eq!(outside_kind, croft);
    let (_, retry_unit, _, _) = at_outside(ECONOMY_WALKTHROUGH_EXPAND_TICK);
    assert_eq!(retry_unit, outside_unit, "the same Yeoman tries again");
    let late = builds(scenarios::commands_at(
        &stream,
        ECONOMY_WALKTHROUGH_EXPAND_TICK,
    ));
    let reject_seq = train_seq(scenarios::commands_at(
        &stream,
        ECONOMY_WALKTHROUGH_REJECT_TICK,
    ));

    // Grain with every Croft worked: both Towns' base yield plus each
    // Croft's slots at the Croft rate, over the Yield Cap.
    let gather = croft_def.gather.as_ref().expect("a Croft gathers");
    assert_eq!(gather.resource, "grain");
    let Slots::Fixed(croft_slots) = gather.slots else {
        panic!("Croft slots are fixed");
    };
    let crofts = 1 + u32::try_from(late.len()).expect("a handful");
    let full_raw = 2 * town_def.base_yield["grain"] + crofts * croft_slots * gather.rate_per_worker;
    let yield_cap = rules.yield_cap_base;
    assert!(full_raw > yield_cap, "{full_raw} must exceed {yield_cap}");

    let mut sim = Sim::new(setup, rules.clone(), Box::new(NoAi));
    let start_owned = sim.view().territory().owned_tile_count(p0);
    assert_eq!(
        sim.view()
            .check_build(p0, outside_unit, croft, outside)
            .unwrap_err(),
        RejectReason::OutsideBorders,
        "the tile is outside the Seat's border at the start"
    );

    let mut rejected = Vec::new();
    let mut placed = 0;
    let mut town_done = None;
    for t in 0..ticks {
        let view = sim.view();
        assert_eq!(view.tick, t);
        let complete = view.buildings().any(|b| {
            b.owner == p0
                && b.kind == town
                && b.tile == ECONOMY_WALKTHROUGH_TOWN
                && b.construction == Construction::Complete
        });
        let owned = view.territory().owned_tile_count(p0);
        if complete {
            if town_done.is_none() {
                town_done = Some(t);
                assert!(owned > start_owned, "{owned} tiles at tick {t}");
                assert!(
                    footprint_owned(&view, croft_def, outside),
                    "the new border covers the rejected Croft"
                );
            }
        } else {
            assert_eq!(
                view.check_build(p0, outside_unit, croft, outside),
                Err(RejectReason::OutsideBorders),
                "tick {t}"
            );
            assert_eq!(owned, start_owned, "tick {t}");
        }
        match t {
            ECONOMY_WALKTHROUGH_EXPAND_TICK => {
                assert!(complete, "the second Town is complete by tick {t}");
                assert_eq!(view.towns(p0).0, 2);
                let quote = rules.building_cost(croft_def, 1);
                for &(_, unit, k, tile) in &late {
                    assert_eq!(k, croft);
                    assert_eq!(
                        view.check_build(p0, unit, k, tile),
                        Ok(quote.clone()),
                        "{unit:?} at {tile:?}"
                    );
                }
                let g = view.income(p0)[grain];
                assert!(g.raw <= g.credited, "not amber yet: {g:?}");
            }
            ECONOMY_WALKTHROUGH_AMBER_TICK => {
                let mine: Vec<_> = view
                    .buildings()
                    .filter(|b| b.owner == p0 && b.kind == croft)
                    .collect();
                assert_eq!(
                    u32::try_from(mine.len()).unwrap(),
                    crofts,
                    "every Croft placed"
                );
                let limit = croft_def.max_per_town.expect("Crofts are per Town");
                for town_id in [seat, town_id(&view, town)] {
                    let here = mine.iter().filter(|b| b.town == Some(town_id)).count();
                    assert!(here > 0, "{town_id:?} has Crofts");
                    assert!(u32::try_from(here).unwrap() <= limit, "{town_id:?}");
                }
                for b in &mine {
                    assert_eq!(b.construction, Construction::Complete, "{:?}", b.id);
                    let slots = u8::try_from(croft_slots).unwrap();
                    assert_eq!(
                        view.worked_slots(b.id),
                        Some((slots, slots, slots)),
                        "{:?} is worked",
                        b.id
                    );
                }
                let g = view.income(p0)[grain];
                assert_eq!(g.raw, full_raw);
                assert_eq!(g.cap, Some(yield_cap));
                assert_eq!(g.credited, yield_cap);
                assert!(g.raw > g.credited, "amber: {g:?}");
            }
            ECONOMY_WALKTHROUGH_TRAIN_TICK => {
                assert_eq!(view.pop(p0), (cap - yeoman_pop, cap));
                assert!(view.check_train(p0, seat, yeoman).is_ok());
            }
            ECONOMY_WALKTHROUGH_REJECT_TICK => {
                assert_eq!(view.pop(p0), (cap, cap), "a queued Yeoman counts");
                let queued = view.building(seat).expect("the Seat").queue.len();
                assert!(
                    u32::try_from(queued).unwrap() < rules.train_queue_max,
                    "the queue has room, so the reason is the pop cap"
                );
                assert_eq!(
                    view.check_train(p0, seat, yeoman),
                    Err(RejectReason::PopCapReached)
                );
            }
            ECONOMY_WALKTHROUGH_POPCAP_TICK => {
                assert_eq!(view.pop(p0), (cap, cap));
                assert!(
                    rejected.iter().any(|r| *r
                        == (
                            ECONOMY_WALKTHROUGH_REJECT_TICK + delay,
                            p0,
                            reject_seq,
                            RejectReason::PopCapReached
                        )),
                    "{rejected:?}"
                );
            }
            _ => {}
        }
        sim.step(scenarios::commands_at(&stream, t));
        for e in sim.drain_events() {
            match e {
                SimEvent::CommandRejected {
                    player,
                    seq,
                    reason,
                } => rejected.push((t, player, seq, reason)),
                SimEvent::BuildingPlaced { .. } => placed += 1,
                _ => {}
            }
        }
    }
    let done = town_done.expect("the second Town completes");
    assert!(done < ECONOMY_WALKTHROUGH_EXPAND_TICK, "tick {done}");
    assert_eq!(
        rejected,
        [
            (delay, p0, outside_seq, RejectReason::OutsideBorders),
            (
                ECONOMY_WALKTHROUGH_REJECT_TICK + delay,
                p0,
                reject_seq,
                RejectReason::PopCapReached
            ),
        ],
        "only the two scripted rejections"
    );
    assert_eq!(placed, 1 + crofts, "the second Town and every Croft");
}

/// Whether P0 owns every tile of a `kind` footprint at `nw`.
fn footprint_owned(view: &SimView<'_>, kind: &rules::BuildingKind, nw: Tile) -> bool {
    let (w, h) = (
        u16::try_from(kind.width_tiles).unwrap(),
        u16::try_from(kind.height_tiles).unwrap(),
    );
    (nw.y..nw.y + h).all(|y| {
        (nw.x..nw.x + w).all(|x| view.territory().is_buildable(PlayerId(0), Tile::new(x, y)))
    })
}

/// P0's Town that is not the Seat.
fn town_id(view: &SimView<'_>, town: BuildingKindId) -> BuildingId {
    view.buildings()
        .find(|b| b.owner == PlayerId(0) && b.kind == town && b.tile == ECONOMY_WALKTHROUGH_TOWN)
        .expect("the second Town")
        .id
}

#[test]
fn golden_economy_walkthrough() {
    golden("economy_walkthrough");
}
