//! Scripted scenarios: pure, deterministic command streams shared by the
//! acceptance tests (`crates/sim/tests/m1.rs`, `m2.rs`), the criterion
//! benches, `sim-cli record` / `bench`, the golden fixtures under
//! `crates/sim/tests/fixtures/`, and (M2) the game's `--scenario <name>`
//! flag, which feeds a stream through `game::sim_driver::ScenarioInput`
//! alongside the player's own commands.
//!
//! A scenario is a [`MatchSetup`] (built with [`MatchSetup::scenario`], so
//! debug commands are accepted and both slots are human) plus a [`Stream`]:
//! `(tick, commands)` pairs sorted by tick, meaning "issue these commands
//! during that tick" (`Sim::step(&cmds)`), so they apply `cmd_delay` later.
//!
//! The streams are data, not functions of a running sim, so they name unit
//! ids directly. That relies on two documented invariants: a fresh `Sim`
//! hands out `UnitId(1)` first and ids are monotonic, so a `DebugSpawn` of
//! `n` units on an empty sim creates `UnitId(1..=n)`. Every stream is valid
//! only when fed to a fresh sim from tick 0.
//!
//! Scenarios start empty ([`StartKind::Empty`]) except
//! [`economy_second_town`], which starts from the standard start. On
//! `plains_1v1` that start creates the Seats `BuildingId(1)` (P0) and
//! `BuildingId(2)` (P1), P0's Yeomen `UnitId(1..=5)` and P1's
//! `UnitId(6..=10)`, so a stream's own buildings begin at `BuildingId(3)`.

use crate::ai_hook::{AiController, SimView};
use crate::command::{Command, PlayerCommand};
use crate::fx::FxVec2;
use crate::ids::{BuildingKindId, PlayerId, Tile, UnitId, UnitKindId};
use crate::state::{MatchSetup, Sim, StartKind};
use rules::Rules;

/// A tick-stamped command stream, sorted by tick.
pub type Stream = Vec<(u32, Vec<PlayerCommand>)>;

/// Player 0's start on `plains_1v1`.
pub const WEST: FxVec2 = FxVec2::from_ints(24, 64);
/// Player 1's start on `plains_1v1`.
pub const EAST: FxVec2 = FxVec2::from_ints(103, 64);

/// Length of [`move_500`]: 500 Yeomen cross the map (80 tiles at 1.8
/// tiles/s is 890 ticks plus jams).
pub const MOVE_500_TICKS: u32 = 1200;
/// Length of [`move_500_short`]: the same stream cut for `cargo test`
/// (the Stop during tick 400 lies beyond it; the full [`move_500`] fixture
/// covers it in CI).
pub const MOVE_500_SHORT_TICKS: u32 = 300;
/// Length of [`group_spiral`]: the second order (during tick 300) is still
/// in progress at the end, which is enough to lock the target assignment.
pub const GROUP_SPIRAL_TICKS: u32 = 600;
/// Length of [`snapshot_restore`].
pub const SNAPSHOT_RESTORE_TICKS: u32 = 1200;
/// Tick at which [`snapshot_restore`] expects the snapshot to be taken
/// (before the extra spawn issued during tick [`SNAPSHOT_RESTORE_SPAWN_TICK`]).
pub const SNAPSHOT_RESTORE_SNAPSHOT_TICK: u32 = 300;
/// Tick during which [`snapshot_restore`] issues its second spawn.
pub const SNAPSHOT_RESTORE_SPAWN_TICK: u32 = 301;

/// Length of [`units200`]: 60 s of game time, the M2 acceptance window for
/// 200 units to cross the map after the player's own order.
pub const UNITS200_TICKS: u32 = 1200;
/// Length of [`units200_auto`].
pub const UNITS200_AUTO_TICKS: u32 = 1200;
/// Tick during which [`units200_auto`] orders all 200 units east.
pub const UNITS200_AUTO_MOVE_TICK: u32 = 20;
/// Length of [`scripted_moves`]: the last order is issued during tick 450
/// and the fixture ends while the units walk, enough to lock the stream.
pub const SCRIPTED_MOVES_TICKS: u32 = 600;
/// Length of [`hud_click`]: the automated HUD check in the game crate
/// finishes well inside it (10 s of game time).
pub const HUD_CLICK_TICKS: u32 = 200;
/// Length of [`economy_second_town`]: 3 minutes of game time, the M3a
/// acceptance bound for founding a second Town.
pub const ECONOMY_SECOND_TOWN_TICKS: u32 = 3600;

/// Scenario names accepted by [`by_name`], `sim-cli record --scenario` and
/// the game's `--scenario`. The first four are the M1 golden fixtures; the
/// M2 names are windowed scenarios (they may be recorded too);
/// `economy_second_town` is the M3a golden fixture.
pub const NAMES: [&str; 9] = [
    "move_500",
    "move_500_short",
    "group_spiral",
    "snapshot_restore",
    "units200",
    "units200_auto",
    "scripted_moves",
    "hud_click",
    "economy_second_town",
];

/// Spawn rows of [`bench_cross`]: a band along the west side from the
/// north ford's latitude to the south ford's, 19 rows apart. Rows 16 and 35
/// route through the north ford (rows 14-21 of the river at columns
/// 62-65), 54 and 73 through the middle one (60-67), 92 and 111 through
/// the south one (106-113), so the three fords carry two groups each.
pub const BENCH_CROSS_ROWS: [i32; 6] = [16, 35, 54, 73, 92, 111];
/// Column of the [`bench_cross`] spawn points.
pub const BENCH_CROSS_WEST_X: i32 = 20;
/// Column of the [`bench_cross`] goals: the spawn column mirrored across
/// the map's `MirrorX` midline (`127 - 20`).
pub const BENCH_CROSS_EAST_X: i32 = 107;
/// Default seed of [`bench_cross`] (`sim-cli bench --seed`).
pub const BENCH_CROSS_SEED: u64 = 1;

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

fn ids(range: core::ops::RangeInclusive<u32>) -> Vec<UnitId> {
    range.map(UnitId).collect()
}

fn mv(owner: u8, seq: u32, units: Vec<UnitId>, target: FxVec2) -> PlayerCommand {
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

fn stop(owner: u8, seq: u32, units: Vec<UnitId>) -> PlayerCommand {
    PlayerCommand::new(PlayerId(owner), seq, Command::Stop { units })
}

fn attack_move(owner: u8, seq: u32, units: Vec<UnitId>, target: FxVec2) -> PlayerCommand {
    PlayerCommand::new(PlayerId(owner), seq, Command::AttackMove { units, target })
}

/// The fixed M1 crossing: 500 Yeomen spawned at the west start during tick
/// 0 (ids 1..=500), ordered to the east start during tick 5, a Stop for
/// every id divisible by 3 during tick 400, then idle until
/// [`MOVE_500_TICKS`]. Seed 42.
pub fn move_500(rules: &Rules) -> (MatchSetup, Stream) {
    let setup = MatchSetup::scenario(rules, 42);
    let stream = vec![
        (0, vec![spawn(0, 0, WEST, 500)]),
        (5, vec![mv(0, 1, ids(1..=500), EAST)]),
        (
            400,
            vec![stop(
                0,
                2,
                (1..=500u32)
                    .filter(|i| i.is_multiple_of(3))
                    .map(UnitId)
                    .collect(),
            )],
        ),
    ];
    (setup, stream)
}

/// [`move_500`] cut at [`MOVE_500_SHORT_TICKS`]: the same setup, and the
/// same stream without the commands issued at or after the cut.
pub fn move_500_short(rules: &Rules) -> (MatchSetup, Stream) {
    let (setup, mut stream) = move_500(rules);
    stream.retain(|(t, _)| *t < MOVE_500_SHORT_TICKS);
    (setup, stream)
}

/// Group-order targets: 64 Yeomen spawned at the west start (ids 1..=64)
/// are ordered during tick 5 to the west map edge (0, 64), where the square
/// spiral is cut by the edge (24 tiles, about 270 ticks); during tick 300
/// they are ordered onto a mountain tile (34, 53), so the click is corrected
/// by the BFS fallback and the spiral skips the 5x5 blocked blob (about 36
/// tiles, 400 ticks, so the fixture ends mid-walk). Seed 7. Length
/// [`GROUP_SPIRAL_TICKS`].
pub fn group_spiral(rules: &Rules) -> (MatchSetup, Stream) {
    let setup = MatchSetup::scenario(rules, 7);
    let stream = vec![
        (0, vec![spawn(0, 0, WEST, 64)]),
        (5, vec![mv(0, 1, ids(1..=64), FxVec2::from_ints(0, 64))]),
        (300, vec![mv(0, 2, ids(1..=64), FxVec2::from_ints(34, 53))]),
    ];
    (setup, stream)
}

/// Snapshot/restore stream: 200 Yeomen (ids 1..=200) spawned at the west
/// start during tick 0 and ordered east during tick 5; the test snapshots
/// at tick [`SNAPSHOT_RESTORE_SNAPSHOT_TICK`]; during tick
/// [`SNAPSHOT_RESTORE_SPAWN_TICK`] player 1 spawns 50 more (ids 201..=250)
/// at the east start and orders them west during tick 310; every id
/// divisible by 5 stops during tick 600. Seed 7. Length
/// [`SNAPSHOT_RESTORE_TICKS`].
pub fn snapshot_restore(rules: &Rules) -> (MatchSetup, Stream) {
    let setup = MatchSetup::scenario(rules, 7);
    let stream = vec![
        (0, vec![spawn(0, 0, WEST, 200)]),
        (5, vec![mv(0, 1, ids(1..=200), EAST)]),
        (SNAPSHOT_RESTORE_SPAWN_TICK, vec![spawn(1, 0, EAST, 50)]),
        (310, vec![mv(1, 1, ids(201..=250), WEST)]),
        (
            600,
            vec![stop(
                0,
                2,
                (1..=200u32)
                    .filter(|i| i.is_multiple_of(5))
                    .map(UnitId)
                    .collect(),
            )],
        ),
    ];
    (setup, stream)
}

/// M2 interactive scenario: 200 Yeomen (ids 1..=200) spawned at the west
/// start during tick 0 and nothing else; the player drag-boxes them and
/// orders them across the map (`--scenario units200`). Seed 1. Length
/// [`UNITS200_TICKS`] (informational: the windowed game runs until exit).
pub fn units200(rules: &Rules) -> (MatchSetup, Stream) {
    let setup = MatchSetup::scenario(rules, 1);
    let stream = vec![(0, vec![spawn(0, 0, WEST, 200)])];
    (setup, stream)
}

/// [`units200`] plus a scripted Move of all 200 units to the east start
/// during tick [`UNITS200_AUTO_MOVE_TICK`]: the automated proxy for the
/// units200 acceptance line (the game also prints frame-time stats). Seed 1.
/// Length [`UNITS200_AUTO_TICKS`].
pub fn units200_auto(rules: &Rules) -> (MatchSetup, Stream) {
    let (setup, mut stream) = units200(rules);
    stream.push((UNITS200_AUTO_MOVE_TICK, vec![mv(0, 1, ids(1..=200), EAST)]));
    (setup, stream)
}

/// Scripted orders only, no local input needed: 100 Yeomen (ids 1..=100)
/// spawned at the west start during tick 0; during tick 20 the first half
/// goes to the north band `(20, 35)` and the second half to the south band
/// `(20, 92)`; during tick 200 everyone is ordered to the east start; during
/// tick 400 every fourth id Stops; during tick 450 everyone `AttackMove`s to
/// `(20, 54)` (the M2 placeholder is applied like Move). Recorded by the
/// game at `--max-fps 30` and `--max-fps 120`, the two replays must end on
/// the same hash. Seed 3. Length [`SCRIPTED_MOVES_TICKS`].
pub fn scripted_moves(rules: &Rules) -> (MatchSetup, Stream) {
    let setup = MatchSetup::scenario(rules, 3);
    let stream = vec![
        (0, vec![spawn(0, 0, WEST, 100)]),
        (
            20,
            vec![
                mv(0, 1, ids(1..=50), FxVec2::from_ints(BENCH_CROSS_WEST_X, 35)),
                mv(
                    0,
                    2,
                    ids(51..=100),
                    FxVec2::from_ints(BENCH_CROSS_WEST_X, 92),
                ),
            ],
        ),
        (200, vec![mv(0, 3, ids(1..=100), EAST)]),
        (
            400,
            vec![stop(
                0,
                4,
                (1..=100u32)
                    .filter(|i| i.is_multiple_of(4))
                    .map(UnitId)
                    .collect(),
            )],
        ),
        (
            450,
            vec![attack_move(
                0,
                5,
                ids(1..=100),
                FxVec2::from_ints(BENCH_CROSS_WEST_X, 54),
            )],
        ),
    ];
    (setup, stream)
}

/// 20 Yeomen (ids 1..=20) at the west start during tick 0 for the game's
/// automated HUD check (`--scenario hud_click`): the game selects them, then
/// writes synthetic pointer input over the bottom HUD panel and the Stop
/// button and asserts that no Move was queued and the selection is
/// unchanged. Seed 1. Length [`HUD_CLICK_TICKS`].
pub fn hud_click(rules: &Rules) -> (MatchSetup, Stream) {
    let setup = MatchSetup::scenario(rules, 1);
    let stream = vec![(0, vec![spawn(0, 0, WEST, 20)])];
    (setup, stream)
}

fn build(owner: u8, seq: u32, unit: u32, kind: BuildingKindId, x: u16, y: u16) -> PlayerCommand {
    PlayerCommand::new(
        PlayerId(owner),
        seq,
        Command::Build {
            unit: UnitId(unit),
            kind,
            tile: Tile::new(x, y),
        },
    )
}

/// The M3a economy fixture, from the standard start on `plains_1v1`, with
/// mirrored build orders for both players (P1's footprints are P0's
/// mirrored across the map's `MirrorX` midline, its Yeomen are P0's ids
/// plus 5):
///
/// - during tick 0: Yeoman 1 founds a Town at (40, 63), centred 17 tiles
///   east of the Seat inside its border (the second Town costs 120 Lumber
///   with the building ramp), Yeoman 2 places a Croft at (28, 60)
///   (30 Lumber), spending exactly the 150 starting Lumber, and the Seat
///   queues a sixth Yeoman (25 Grain: the five starting Yeomen count);
/// - during tick 2400: Yeoman 3 places a Croft at (44, 60), attributed to
///   the new Town, paid from both Towns' Lumber income.
///
/// Both second Towns complete well before tick 3600 and extend their
/// owners' borders. Seed 11. Length [`ECONOMY_SECOND_TOWN_TICKS`].
pub fn economy_second_town(rules: &Rules) -> (MatchSetup, Stream) {
    let mut setup = MatchSetup::scenario(rules, 11);
    setup.start = StartKind::Standard;
    let kind = |id: &str| {
        BuildingKindId(
            rules
                .building_kind_by_id(id)
                .unwrap_or_else(|| panic!("buildings.ron has {id}"))
                .0,
        )
    };
    let (town, croft) = (kind("town"), kind("croft"));
    let width = rules.map(&rules.default_map).expect("default map").width;
    // NW corner of the mirror of a footprint `w` wide at column `x`.
    let mirror = |x: u16, w: u16| width - x - w;
    let yeoman = UnitKindId(0);
    let train = |owner: u8, seq: u32, seat: u32| {
        PlayerCommand::new(
            PlayerId(owner),
            seq,
            Command::Train {
                building: crate::ids::BuildingId(seat),
                kind: yeoman,
            },
        )
    };
    let stream = vec![
        (
            0,
            vec![
                build(0, 0, 1, town, 40, 63),
                build(0, 1, 2, croft, 28, 60),
                train(0, 2, 1),
                build(1, 0, 6, town, mirror(40, 3), 63),
                build(1, 1, 7, croft, mirror(28, 2), 60),
                train(1, 2, 2),
            ],
        ),
        (
            2400,
            vec![
                build(0, 3, 3, croft, 44, 60),
                build(1, 3, 8, croft, mirror(44, 2), 60),
            ],
        ),
    ];
    (setup, stream)
}

/// The `sim-cli bench --units n` crossing: `n` Yeomen spawned during tick 0
/// in one group per row of [`BENCH_CROSS_ROWS`], of (as near as possible)
/// equal size, at `(BENCH_CROSS_WEST_X, row)` in
/// row order, so group `k` holds consecutive ids; during tick 5 each group
/// is ordered to its mirrored point `(BENCH_CROSS_EAST_X, row)` with its own
/// Move command (one spiral of targets per group). A* routes the groups
/// through all three fords instead of funnelling every unit through the
/// middle one as [`move_500`] does. `n` smaller than the number of rows
/// fills the first rows only. Not a golden fixture: the stream depends on
/// `n`, and [`move_500`] stays the hash-parity fixture.
pub fn bench_cross(rules: &Rules, n: u16, seed: u64) -> (MatchSetup, Stream) {
    let setup = MatchSetup::scenario(rules, seed);
    let groups = u16::try_from(BENCH_CROSS_ROWS.len()).expect("a handful of rows");
    let base = n / groups;
    let extra = n % groups;
    let mut spawns = Vec::new();
    let mut moves = Vec::new();
    let mut first_id = 1u32;
    for (k, row) in (0u16..).zip(BENCH_CROSS_ROWS) {
        let count = base + u16::from(k < extra);
        if count == 0 {
            break;
        }
        let last_id = first_id + u32::from(count) - 1;
        spawns.push(spawn(
            0,
            u32::from(k),
            FxVec2::from_ints(BENCH_CROSS_WEST_X, row),
            count,
        ));
        moves.push(mv(
            0,
            u32::from(groups + k),
            ids(first_id..=last_id),
            FxVec2::from_ints(BENCH_CROSS_EAST_X, row),
        ));
        first_id = last_id + 1;
    }
    let mut stream = vec![(0, spawns)];
    if !moves.is_empty() {
        stream.push((5, moves));
    }
    (setup, stream)
}

/// Look a scenario up by name: `(setup, stream, ticks)`.
pub fn by_name(name: &str, rules: &Rules) -> Option<(MatchSetup, Stream, u32)> {
    let (setup, stream, ticks) = match name {
        "move_500" => {
            let (s, c) = move_500(rules);
            (s, c, MOVE_500_TICKS)
        }
        "move_500_short" => {
            let (s, c) = move_500_short(rules);
            (s, c, MOVE_500_SHORT_TICKS)
        }
        "group_spiral" => {
            let (s, c) = group_spiral(rules);
            (s, c, GROUP_SPIRAL_TICKS)
        }
        "snapshot_restore" => {
            let (s, c) = snapshot_restore(rules);
            (s, c, SNAPSHOT_RESTORE_TICKS)
        }
        "units200" => {
            let (s, c) = units200(rules);
            (s, c, UNITS200_TICKS)
        }
        "units200_auto" => {
            let (s, c) = units200_auto(rules);
            (s, c, UNITS200_AUTO_TICKS)
        }
        "scripted_moves" => {
            let (s, c) = scripted_moves(rules);
            (s, c, SCRIPTED_MOVES_TICKS)
        }
        "hud_click" => {
            let (s, c) = hud_click(rules);
            (s, c, HUD_CLICK_TICKS)
        }
        "economy_second_town" => {
            let (s, c) = economy_second_town(rules);
            (s, c, ECONOMY_SECOND_TOWN_TICKS)
        }
        _ => return None,
    };
    Some((setup, stream, ticks))
}

/// The commands a stream issues during `tick` (empty for most ticks).
pub fn commands_at(stream: &Stream, tick: u32) -> &[PlayerCommand] {
    match stream.binary_search_by_key(&tick, |(t, _)| *t) {
        Ok(i) => &stream[i].1,
        Err(_) => &[],
    }
}

/// A controller for slots no scenario has: every scenario setup is two
/// human slots, so it is never consulted.
struct NoSlots;

impl AiController for NoSlots {
    fn think(&mut self, _player: PlayerId, _view: &SimView<'_>) -> Vec<Command> {
        Vec::new()
    }
}

/// Build the sim for a scenario and drive it through ticks `0..ticks`,
/// feeding `stream` with [`commands_at`]. Panics when `setup` has an AI slot.
pub fn run(setup: MatchSetup, rules: Rules, stream: &Stream, ticks: u32) -> Sim {
    assert!(
        setup.players.iter().all(|p| !p.is_ai),
        "scenarios have no AI slots"
    );
    let mut sim = Sim::new(setup, rules, Box::new(NoSlots));
    for t in 0..ticks {
        sim.step(commands_at(stream, t));
    }
    sim
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn rules() -> Rules {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap()
    }

    #[test]
    fn streams_are_sorted_and_names_resolve() {
        let rules = rules();
        for name in NAMES {
            let (setup, stream, ticks) = by_name(name, &rules).unwrap_or_else(|| panic!("{name}"));
            assert!(setup.debug_commands, "{name}");
            assert!(stream.windows(2).all(|w| w[0].0 < w[1].0), "{name}");
            assert!(stream.last().is_some_and(|(t, _)| *t < ticks), "{name}");
            assert!(!commands_at(&stream, 0).is_empty(), "{name} spawns at 0");
            assert!(commands_at(&stream, 1).is_empty(), "{name}");
        }
        assert!(by_name("nope", &rules).is_none());
    }

    /// The economy fixture's ids follow from the standard start: its own
    /// builders are the first three Yeomen of each player.
    #[test]
    fn economy_second_town_ids_match_the_standard_start() {
        let rules = rules();
        let (setup, stream) = economy_second_town(&rules);
        let sim = Sim::new(setup, rules, Box::new(NoSlots));
        let view = sim.view();
        for (_, cmds) in &stream {
            for c in cmds {
                match c.cmd {
                    Command::Build { unit, .. } => {
                        assert_eq!(view.unit(unit).unwrap().owner, c.player, "{unit:?}");
                    }
                    Command::Train { building, .. } => {
                        assert_eq!(view.building(building).unwrap().owner, c.player);
                    }
                    _ => panic!("only Builds and Trains"),
                }
            }
        }
        assert_eq!(
            view.building(crate::ids::BuildingId(1)).unwrap().owner,
            PlayerId(0)
        );
        assert_eq!(
            view.building(crate::ids::BuildingId(2)).unwrap().owner,
            PlayerId(1)
        );
    }

    #[test]
    fn group_spiral_second_click_is_on_a_blocked_tile() {
        // The scenario exists to exercise the BFS click correction and the
        // spiral's blocked-tile skipping; keep it honest if the map changes.
        let rules = rules();
        let map = crate::map::Map::from_def(rules.map(&rules.default_map).unwrap());
        let (_, stream) = group_spiral(&rules);
        let Command::Move { target, .. } = &commands_at(&stream, 300)[0].cmd else {
            panic!("second order is a Move");
        };
        assert!(!map.passable(map.tile_of(*target)), "click must be blocked");
        let Command::Move { target, .. } = &commands_at(&stream, 5)[0].cmd else {
            panic!("first order is a Move");
        };
        assert_eq!(map.tile_of(*target).x, 0, "first click is on the west edge");
    }

    #[test]
    fn bench_cross_spawns_a_west_band_and_mirrors_each_group_east() {
        let rules = rules();
        let map = crate::map::Map::from_def(rules.map(&rules.default_map).unwrap());
        let (setup, stream) = bench_cross(&rules, 500, BENCH_CROSS_SEED);
        assert_eq!(setup.seed, BENCH_CROSS_SEED);
        assert!(setup.debug_commands);
        let spawns = commands_at(&stream, 0);
        let moves = commands_at(&stream, 5);
        assert_eq!(spawns.len(), BENCH_CROSS_ROWS.len());
        assert_eq!(moves.len(), BENCH_CROSS_ROWS.len());
        let mut next_id = 1u32;
        let mut total = 0u16;
        for ((s, m), row) in spawns.iter().zip(moves).zip(BENCH_CROSS_ROWS) {
            let Command::DebugSpawn { at, count, .. } = &s.cmd else {
                panic!("tick 0 is spawns");
            };
            let Command::Move { units, target, .. } = &m.cmd else {
                panic!("tick 5 is moves");
            };
            assert_eq!(*at, FxVec2::from_ints(BENCH_CROSS_WEST_X, row));
            assert_eq!(*target, FxVec2::from_ints(BENCH_CROSS_EAST_X, row));
            assert!(map.passable(map.tile_of(*at)), "spawn row {row}");
            assert!(map.passable(map.tile_of(*target)), "goal row {row}");
            assert!((83..=84).contains(count), "500 over 6 groups: {count}");
            assert_eq!(units.len(), usize::from(*count));
            assert_eq!(units[0], UnitId(next_id), "consecutive ids per group");
            next_id += u32::from(*count);
            total += count;
        }
        assert_eq!(total, 500);
        // Seq numbers are unique per player so the sim applies them in order.
        let mut seqs: Vec<u32> = spawns.iter().chain(moves).map(|c| c.seq).collect();
        seqs.sort_unstable();
        seqs.dedup();
        assert_eq!(seqs.len(), 2 * BENCH_CROSS_ROWS.len());
        // The west spawn column mirrors onto the east goal column.
        assert_eq!(
            BENCH_CROSS_WEST_X + BENCH_CROSS_EAST_X,
            i32::from(map.width()) - 1
        );
        // Fewer units than rows: only the first rows are used, no empty groups.
        let (_, small) = bench_cross(&rules, 2, 9);
        assert_eq!(commands_at(&small, 0).len(), 2);
        assert_eq!(commands_at(&small, 5).len(), 2);
        let (_, none) = bench_cross(&rules, 0, 9);
        assert!(commands_at(&none, 0).is_empty());
        assert!(commands_at(&none, 5).is_empty());
        // Every spawned id gets exactly one Move, so the ids match a fresh sim.
        let delay = setup.cmd_delay;
        let sim = run(setup, rules, &stream, 5 + delay + 1);
        assert!(sim.view().units().all(|u| u.order.is_some()));
        assert_eq!(sim.view().unit_count(), 500);
    }

    #[test]
    fn move_500_ids_match_a_fresh_sim() {
        let rules = rules();
        let (setup, stream) = move_500(&rules);
        let delay = setup.cmd_delay;
        let sim = run(setup, rules, &stream, delay + 1);
        let ids: Vec<u32> = sim.view().units().map(|u| u.id.0).collect();
        assert_eq!(ids, (1..=500).collect::<Vec<u32>>());
    }
}
