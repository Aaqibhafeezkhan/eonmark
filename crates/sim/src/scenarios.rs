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
//! The M4a streams use `DebugSpawnJittered` ([`combat_skirmish`],
//! [`counter_duel_setup`], [`combat_bench`]): its offsets come from the
//! sim's own RNG, so a stream stays plain data.
//!
//! Scenarios start empty ([`StartKind::Empty`]) except
//! [`economy_second_town`] and [`economy_walkthrough`], which start from
//! the standard start. On
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

/// The Croft tile of [`economy_walkthrough`] that is outside P0's border
/// during tick 0 (`OutsideBorders`) and inside the second Town's once that
/// is complete (placed during [`ECONOMY_WALKTHROUGH_EXPAND_TICK`]).
pub const ECONOMY_WALKTHROUGH_OUTSIDE_CROFT: Tile = Tile::new(50, 60);
/// Where [`economy_walkthrough`] founds P0's second Town (north-west tile).
pub const ECONOMY_WALKTHROUGH_TOWN: Tile = Tile::new(40, 63);
/// Tick by which [`economy_walkthrough`]'s tick-0 commands have been
/// applied: the `OutsideBorders` rejection is on the message line, the Town
/// and Croft sites are placed and the Seat is training.
pub const ECONOMY_WALKTHROUGH_OUTSIDE_TICK: u32 = 20;
/// Tick during which [`economy_walkthrough`] grants P0 the Lumber for its
/// five later Crofts, before the second Town completes.
pub const ECONOMY_WALKTHROUGH_GRANT_TICK: u32 = 1200;
/// Tick during which [`economy_walkthrough`] places the five later Crofts,
/// the first at [`ECONOMY_WALKTHROUGH_OUTSIDE_CROFT`] again. The second
/// Town is complete and its border drawn by then (it completes at tick 1346).
pub const ECONOMY_WALKTHROUGH_EXPAND_TICK: u32 = 1400;
/// Tick by which [`economy_walkthrough`]'s six Crofts are complete and
/// worked: P0's raw Grain income is over the Yield Cap (the amber readout).
pub const ECONOMY_WALKTHROUGH_AMBER_TICK: u32 = 2100;
/// Tick during which [`economy_walkthrough`] spawns Yeomen up to one below
/// P0's pop cap and grants the Grain for one more.
pub const ECONOMY_WALKTHROUGH_SPAWN_TICK: u32 = 2160;
/// Tick during which [`economy_walkthrough`]'s Seat queues the Yeoman that
/// fills the pop cap (25/25).
pub const ECONOMY_WALKTHROUGH_TRAIN_TICK: u32 = 2200;
/// Tick during which [`economy_walkthrough`]'s Seat is told to train once
/// more and the sim rejects it with `PopCapReached`.
pub const ECONOMY_WALKTHROUGH_REJECT_TICK: u32 = 2240;
/// Tick by which [`economy_walkthrough`]'s pop is 25/25 and the
/// `PopCapReached` rejection is on the message line.
pub const ECONOMY_WALKTHROUGH_POPCAP_TICK: u32 = 2260;
/// Length of [`economy_walkthrough`]: a little after the last beat, and
/// short enough (at most 5000 ticks) for its golden to run in `cargo test`.
pub const ECONOMY_WALKTHROUGH_TICKS: u32 = 2400;

/// Scenario names accepted by [`by_name`], `sim-cli record --scenario` and
/// the game's `--scenario`. The first four are the M1 golden fixtures; the
/// M2 names are windowed scenarios (they may be recorded too);
/// `economy_second_town` is the M3a golden fixture, `economy_walkthrough`
/// the M3b one and `combat_skirmish` the M4a one.
pub const NAMES: [&str; 11] = [
    "move_500",
    "move_500_short",
    "group_spiral",
    "snapshot_restore",
    "units200",
    "units200_auto",
    "scripted_moves",
    "hud_click",
    "economy_second_town",
    "economy_walkthrough",
    "combat_skirmish",
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

/// A building kind's id by its `buildings.ron` id.
fn building_kind(rules: &Rules, id: &str) -> BuildingKindId {
    BuildingKindId(
        rules
            .building_kind_by_id(id)
            .unwrap_or_else(|| panic!("buildings.ron has {id}"))
            .0,
    )
}

/// A unit kind's id by its `units.ron` id.
fn unit_kind(rules: &Rules, id: &str) -> UnitKindId {
    let i = rules
        .units
        .iter()
        .position(|u| u.id == id)
        .unwrap_or_else(|| panic!("units.ron has {id}"));
    UnitKindId(u16::try_from(i).expect("validated unit count"))
}

/// A resource's index by its `resources.ron` id.
fn resource(rules: &Rules, id: &str) -> u8 {
    let i = rules
        .resource_index(id)
        .unwrap_or_else(|| panic!("resources.ron has {id}"));
    u8::try_from(i).expect("resource count fits u8")
}

fn train(owner: u8, seq: u32, building: u32, kind: UnitKindId) -> PlayerCommand {
    PlayerCommand::new(
        PlayerId(owner),
        seq,
        Command::Train {
            building: crate::ids::BuildingId(building),
            kind,
        },
    )
}

fn grant(owner: u8, seq: u32, resource: u8, amount: i64) -> PlayerCommand {
    PlayerCommand::new(
        PlayerId(owner),
        seq,
        Command::DebugGrant {
            owner: PlayerId(owner),
            resource,
            amount: u32::try_from(amount).expect("grant fits u32"),
        },
    )
}

/// The M3b owner-proxy walkthrough (`--scenario economy_walkthrough`), from
/// the standard start on `plains_1v1`. Only P0 acts (P1 keeps its Seat and
/// five idle Yeomen); its Seat is `BuildingId(1)`, its Yeomen `UnitId(1..=5)`,
/// and the Yeoman it trains first is `UnitId(11)` (P1's are `6..=10`).
///
/// - Place outside, place inside, second Town (during tick 0): Yeoman 3
///   tries a Croft at [`ECONOMY_WALKTHROUGH_OUTSIDE_CROFT`], east of the
///   Seat's border radius (`OutsideBorders`);
///   Yeoman 1 founds a Town at [`ECONOMY_WALKTHROUGH_TOWN`] and Yeoman 2
///   places a Croft at (28, 60), spending exactly the 150 starting Lumber
///   (as in [`economy_second_town`]); the Seat queues a Yeoman.
/// - During [`ECONOMY_WALKTHROUGH_GRANT_TICK`] P0 is granted the Lumber for
///   five more Crofts at their ramped costs. The second Town completes
///   before [`ECONOMY_WALKTHROUGH_EXPAND_TICK`], during which Yeoman 3
///   places the Croft at [`ECONOMY_WALKTHROUGH_OUTSIDE_CROFT`] again, now
///   inside the new Town's border, and Yeomen 1, 4, 5 and 11 place four
///   more: three Crofts per Town, under `max_per_town`.
/// - Yield Cap: each builder takes its Croft's slot when it completes, so
///   by [`ECONOMY_WALKTHROUGH_AMBER_TICK`] P0 earns two Towns' base Grain
///   plus six worked Crofts, over the Yield Cap (raw > credited, amber).
/// - Pop cap: during [`ECONOMY_WALKTHROUGH_SPAWN_TICK`] P0 spawns Yeomen up
///   to one Yeoman below the pop cap and is granted the Grain for one more;
///   during [`ECONOMY_WALKTHROUGH_TRAIN_TICK`] the Seat queues it (pop at
///   the cap, the queue nearly empty); during
///   [`ECONOMY_WALKTHROUGH_REJECT_TICK`] the next `Train` is rejected with
///   `PopCapReached`.
///
/// Seed 12. Length [`ECONOMY_WALKTHROUGH_TICKS`].
pub fn economy_walkthrough(rules: &Rules) -> (MatchSetup, Stream) {
    let mut setup = MatchSetup::scenario(rules, 12);
    setup.start = StartKind::Standard;
    let (town, croft) = (building_kind(rules, "town"), building_kind(rules, "croft"));
    let yeoman = unit_kind(rules, "yeoman");
    let yeoman_def = rules.unit_kind(yeoman.0).expect("yeoman kind");
    let croft_def = rules.building_kind(croft.0).expect("croft kind");
    let (lumber, grain) = (resource(rules, "lumber"), resource(rules, "grain"));
    let seat = 1;
    let outside = ECONOMY_WALKTHROUGH_OUTSIDE_CROFT;
    // (builder, north-west tile) of the Crofts placed after the second Town:
    // the retried one and two more by the new Town, two by the Seat.
    let late_crofts: [(u32, u16, u16); 5] = [
        (3, outside.x, outside.y),
        (1, 44, 60),
        (4, 44, 66),
        (5, 20, 60),
        (11, 28, 67),
    ];
    // Every later Croft pays the ramp for the Crofts placed before it (the
    // tick-0 one included).
    let late_lumber: i64 = (1..=u32::try_from(late_crofts.len()).expect("five"))
        .map(|n| rules.building_cost(croft_def, n)[usize::from(lumber)])
        .sum();
    // Pop: the standard start's Yeomen plus the one trained during tick 0;
    // spawn up to one Yeoman below the cap, then train that one, whose
    // ramped cost counts every Yeoman alive before it.
    let cap = rules.pop_cap_base;
    let pop = yeoman_def.pop;
    let alive = (rules.standard_start.units["yeoman"] + 1) * pop;
    let to_spawn = (cap - pop - alive) / pop;
    let last_yeoman = rules.unit_cost(yeoman_def, (cap - pop) / pop, 1);
    let late = (5..)
        .zip(late_crofts)
        .map(|(seq, (unit, x, y))| build(0, seq, unit, croft, x, y))
        .collect();
    // South of the Seat, clear of every footprint; every Croft slot is taken
    // by then, so the spawned Yeomen stay idle there.
    let spawn_at = FxVec2::from_ints(24, 71);
    let stream = vec![
        (
            0,
            vec![
                build(0, 0, 3, croft, outside.x, outside.y),
                build(
                    0,
                    1,
                    1,
                    town,
                    ECONOMY_WALKTHROUGH_TOWN.x,
                    ECONOMY_WALKTHROUGH_TOWN.y,
                ),
                build(0, 2, 2, croft, 28, 60),
                train(0, 3, seat, yeoman),
            ],
        ),
        (
            ECONOMY_WALKTHROUGH_GRANT_TICK,
            vec![grant(0, 4, lumber, late_lumber)],
        ),
        (ECONOMY_WALKTHROUGH_EXPAND_TICK, late),
        (
            ECONOMY_WALKTHROUGH_SPAWN_TICK,
            vec![
                PlayerCommand::new(
                    PlayerId(0),
                    10,
                    Command::DebugSpawn {
                        owner: PlayerId(0),
                        kind: yeoman,
                        at: spawn_at,
                        count: u16::try_from(to_spawn).expect("spawn count fits u16"),
                    },
                ),
                grant(0, 11, grain, last_yeoman[usize::from(grain)]),
            ],
        ),
        (
            ECONOMY_WALKTHROUGH_TRAIN_TICK,
            vec![train(0, 12, seat, yeoman)],
        ),
        (
            ECONOMY_WALKTHROUGH_REJECT_TICK,
            vec![train(0, 13, seat, yeoman)],
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

/// Length of [`combat_skirmish`]: the Watchtower falls well before it,
/// and it is short enough (at most 3000 ticks) for its golden to run in
/// `cargo test`.
pub const COMBAT_SKIRMISH_TICKS: u32 = 3000;
/// [`combat_skirmish`]'s P0 Scriptorium (north-west tile), `BuildingId(1)`.
pub const COMBAT_SKIRMISH_SCRIPTORIUM: Tile = Tile::new(2, 2);
/// [`combat_skirmish`]'s P0 Engine Yard, `BuildingId(2)`.
pub const COMBAT_SKIRMISH_ENGINE_YARD: Tile = Tile::new(30, 12);
/// [`combat_skirmish`]'s P1 Town, `BuildingId(3)`.
pub const COMBAT_SKIRMISH_TOWN: Tile = Tile::new(22, 38);
/// [`combat_skirmish`]'s P1 Watchtower, `BuildingId(4)`.
pub const COMBAT_SKIRMISH_WATCHTOWER: Tile = Tile::new(40, 25);
/// Tick during which [`combat_skirmish`]'s two lines attack-move.
pub const COMBAT_SKIRMISH_ATTACK_MOVE_TICK: u32 = 5;
/// Tick during which [`combat_skirmish`] issues its mid-fight Stop (P0's
/// Bowmen) and Move (P1's Outriders).
pub const COMBAT_SKIRMISH_STOP_TICK: u32 = 150;
/// Tick during which [`combat_skirmish`] queues Trade I and Statecraft I
/// (Letters I, queued during tick 0, is complete by then).
pub const COMBAT_SKIRMISH_RESEARCH_TICK: u32 = 520;
/// Tick during which [`combat_skirmish`] cancels the queued Statecraft I.
pub const COMBAT_SKIRMISH_CANCEL_TICK: u32 = 540;
/// Tick during which [`combat_skirmish`]'s P0 enters the Masonry Age.
pub const COMBAT_SKIRMISH_ADVANCE_TICK: u32 = 1100;
/// Tick during which [`combat_skirmish`]'s Engine Yard queues the Mangonel.
pub const COMBAT_SKIRMISH_TRAIN_TICK: u32 = 1110;
/// Tick during which [`combat_skirmish`]'s trained Mangonel (`UnitId(99)`)
/// is ordered to attack the Watchtower.
pub const COMBAT_SKIRMISH_MANGONEL_TICK: u32 = 1920;
/// Tick during which [`combat_skirmish`]'s 50 P1 Yeomen are ordered across
/// the river, saturating the path budget while the Watchtower falls.
pub const COMBAT_SKIRMISH_SATURATE_TICK: u32 = 2300;

/// The M4a combat golden (`combat_skirmish`, seed 21, empty start on
/// `plains_1v1`): tech and combat beats in one replay, so hash parity
/// covers combat.
///
/// - During tick 0, P0 gets a complete Scriptorium
///   ([`COMBAT_SKIRMISH_SCRIPTORIUM`], `BuildingId(1)`) and Engine Yard
///   ([`COMBAT_SKIRMISH_ENGINE_YARD`], `BuildingId(2)`) and stock, and
///   queues Letters I; P1 gets a complete Town beside the field
///   ([`COMBAT_SKIRMISH_TOWN`], `BuildingId(3)`), which shoots into the
///   fight, and a Watchtower off it ([`COMBAT_SKIRMISH_WATCHTOWER`],
///   `BuildingId(4)`), both in the all-grass west rectangle. Each side spawns six of each line kind with
///   `DebugSpawnJittered` (a tile of jitter): P0's Skirmishers, Shieldbearers,
///   Bowmen and Outriders are `UnitId(1..=24)` west of the field, P1's
///   `UnitId(25..=48)` mirrored east of it; P1 also spawns 50 Yeomen
///   (`UnitId(49..=98)`) far south-east.
/// - During [`COMBAT_SKIRMISH_ATTACK_MOVE_TICK`] the lines attack-move at
///   each other; during [`COMBAT_SKIRMISH_STOP_TICK`], mid-fight, P0's
///   Bowmen Stop and P1's Outriders Move away.
/// - P0 really researches: Letters I from tick 0, Trade I queued during
///   [`COMBAT_SKIRMISH_RESEARCH_TICK`] (at the Letters price), Statecraft I
///   queued behind it and cancelled during [`COMBAT_SKIRMISH_CANCEL_TICK`];
///   it enters the Masonry Age during [`COMBAT_SKIRMISH_ADVANCE_TICK`] and
///   trains a Mangonel (`UnitId(99)`) at the Engine Yard during
///   [`COMBAT_SKIRMISH_TRAIN_TICK`].
/// - During [`COMBAT_SKIRMISH_MANGONEL_TICK`] the Mangonel attacks the
///   Watchtower, which it outranges; during
///   [`COMBAT_SKIRMISH_SATURATE_TICK`] the 50 Yeomen are ordered across the
///   river, so path searches are suspended when the tower is destroyed.
///
/// Length [`COMBAT_SKIRMISH_TICKS`].
pub fn combat_skirmish(rules: &Rules) -> (MatchSetup, Stream) {
    let setup = MatchSetup::scenario(rules, 21);
    let mut seq = [0u32; 2];
    let mut cmd = |owner: u8, c: Command| {
        let i = usize::from(owner);
        seq[i] += 1;
        PlayerCommand::new(PlayerId(owner), seq[i] - 1, c)
    };
    let place = |owner: u8, kind: &str, tile: Tile| Command::DebugPlaceBuilding {
        owner: PlayerId(owner),
        kind: building_kind(rules, kind),
        tile,
        complete: true,
    };
    let tech = |id: &str| {
        crate::ids::TechId(
            rules
                .tech_by_id(id)
                .unwrap_or_else(|| panic!("techs.ron has {id}"))
                .0,
        )
    };
    let stock = |id: &str, amount: u32| Command::DebugGrant {
        owner: PlayerId(0),
        resource: resource(rules, id),
        amount,
    };
    let research = |id: &str| Command::Research {
        building: crate::ids::BuildingId(1),
        tech: tech(id),
    };
    let line = ["skirmisher", "shieldbearer", "bowman", "outrider"];
    // Spiral centres per line kind: P0's west of the field, P1's mirrored
    // across x = 19.5.
    let p0_at = [(8, 28), (8, 36), (4, 32), (12, 32)];
    let jittered =
        |owner: u8, kind: &str, (x, y): (i32, i32), count: u16| Command::DebugSpawnJittered {
            owner: PlayerId(owner),
            kind: unit_kind(rules, kind),
            at: FxVec2::from_ints(x, y),
            count,
            jitter_x100: 100,
        };
    let mut tick0 = vec![
        cmd(0, place(0, "scriptorium", COMBAT_SKIRMISH_SCRIPTORIUM)),
        cmd(0, place(0, "engine_yard", COMBAT_SKIRMISH_ENGINE_YARD)),
        cmd(0, stock("grain", 400)),
        cmd(0, stock("lumber", 300)),
        cmd(0, stock("lore", 500)),
        cmd(0, research("letters_1")),
    ];
    for (kind, at) in line.iter().zip(p0_at) {
        tick0.push(cmd(0, jittered(0, kind, at, 6)));
    }
    tick0.push(cmd(1, place(1, "town", COMBAT_SKIRMISH_TOWN)));
    tick0.push(cmd(1, place(1, "watchtower", COMBAT_SKIRMISH_WATCHTOWER)));
    for (kind, (x, y)) in line.iter().zip(p0_at) {
        tick0.push(cmd(1, jittered(1, kind, (39 - x, y), 6)));
    }
    tick0.push(cmd(
        1,
        Command::DebugSpawn {
            owner: PlayerId(1),
            kind: unit_kind(rules, "yeoman"),
            at: FxVec2::from_ints(100, 106),
            count: 50,
        },
    ));
    let units = |first: u32, n: u32| ids(first..=first + n - 1);
    let am = |units: Vec<UnitId>, x: i32, y: i32| Command::AttackMove {
        units,
        target: FxVec2::from_ints(x, y),
    };
    let stream = vec![
        (0, tick0),
        (
            COMBAT_SKIRMISH_ATTACK_MOVE_TICK,
            vec![
                cmd(0, am(units(1, 24), 27, 32)),
                cmd(1, am(units(25, 24), 12, 32)),
            ],
        ),
        (
            COMBAT_SKIRMISH_STOP_TICK,
            vec![
                cmd(
                    0,
                    Command::Stop {
                        units: units(13, 6),
                    },
                ),
                cmd(
                    1,
                    Command::Move {
                        units: units(43, 6),
                        target: FxVec2::from_ints(40, 40),
                        queue: false,
                    },
                ),
            ],
        ),
        (
            COMBAT_SKIRMISH_RESEARCH_TICK,
            vec![
                cmd(0, research("trade_1")),
                cmd(0, research("statecraft_1")),
            ],
        ),
        (
            COMBAT_SKIRMISH_CANCEL_TICK,
            vec![cmd(
                0,
                Command::Cancel {
                    building: crate::ids::BuildingId(1),
                    slot: 1,
                },
            )],
        ),
        (
            COMBAT_SKIRMISH_ADVANCE_TICK,
            vec![cmd(
                0,
                Command::AdvanceAge {
                    player: PlayerId(0),
                },
            )],
        ),
        (
            COMBAT_SKIRMISH_TRAIN_TICK,
            vec![cmd(
                0,
                Command::Train {
                    building: crate::ids::BuildingId(2),
                    kind: unit_kind(rules, "mangonel"),
                },
            )],
        ),
        (
            COMBAT_SKIRMISH_MANGONEL_TICK,
            vec![cmd(
                0,
                Command::Attack {
                    units: vec![UnitId(99)],
                    target: crate::ids::TargetId::Building(crate::ids::BuildingId(4)),
                },
            )],
        ),
        (
            COMBAT_SKIRMISH_SATURATE_TICK,
            vec![cmd(
                1,
                Command::Move {
                    units: units(49, 50),
                    target: FxVec2::from_ints(20, 104),
                    queue: false,
                },
            )],
        ),
    ];
    (setup, stream)
}

/// Rows of the [`counter_duel_setup`] line: five rows, three tiles apart,
/// inside the all-grass west rectangle of `plains_1v1` (x 0..43, y 24..41).
pub const COUNTER_DUEL_ROWS: [i32; 5] = [26, 29, 32, 35, 38];
/// Columns of P0's two ranks in [`counter_duel_setup`], front rank first.
pub const COUNTER_DUEL_P0_RANKS: [i32; 2] = [10, 7];
/// Columns of P1's two ranks in [`counter_duel_setup`], front rank first.
pub const COUNTER_DUEL_P1_RANKS: [i32; 2] = [30, 33];
/// Where P0 attack-moves in [`counter_duel_setup`]: beyond P1's line.
pub const COUNTER_DUEL_P0_GOAL: FxVec2 = FxVec2::from_ints(36, 32);
/// Where P1 attack-moves in [`counter_duel_setup`]: beyond P0's line.
pub const COUNTER_DUEL_P1_GOAL: FxVec2 = FxVec2::from_ints(4, 32);
/// Spawn jitter of every [`counter_duel_setup`] unit, in hundredths of a
/// tile per axis: up to a tile either way.
pub const COUNTER_DUEL_JITTER_X100: u8 = 100;
/// Ticks after which a counter duel is a timeout, which is no win.
pub const COUNTER_DUEL_TICKS: u32 = 2400;

/// The counter tests' 10 vs 10 duel (`docs/design/combat.md`, "Fixtures
/// and the 20-seed tests"): ten units of kind `a` for P0 and ten of kind
/// `b` for P1 on an empty start (`MatchSetup::scenario` with `seed`), in a
/// line of [`COUNTER_DUEL_ROWS`] by two ranks, P0's at
/// [`COUNTER_DUEL_P0_RANKS`] and P1's at [`COUNTER_DUEL_P1_RANKS`], 20
/// tiles between the front ranks. During tick 0 each unit is spawned by
/// its own `DebugSpawnJittered` (count 1, [`COUNTER_DUEL_JITTER_X100`]),
/// rank by rank and row by row, so P0's are `UnitId(1..=10)` and P1's
/// `UnitId(11..=20)` and the seed moves every unit up to a tile either way.
/// During tick 1 P0 attack-moves to [`COUNTER_DUEL_P0_GOAL`] and P1 to
/// [`COUNTER_DUEL_P1_GOAL`]. The seed perturbs the jitter and the
/// acquisition tie-breaks. The test harness (`crates/sim/tests/m4a.rs`)
/// adds the rest: every 100 ticks each side's idle survivors attack-move
/// toward the live enemy centroid, and the duel ends when a side is empty
/// or after [`COUNTER_DUEL_TICKS`]. Not a golden fixture.
pub fn counter_duel_setup(
    rules: &Rules,
    a: UnitKindId,
    b: UnitKindId,
    seed: u64,
) -> (MatchSetup, Stream) {
    let setup = MatchSetup::scenario(rules, seed);
    let mut spawns = Vec::new();
    for (owner, kind, ranks) in [
        (0u8, a, COUNTER_DUEL_P0_RANKS),
        (1u8, b, COUNTER_DUEL_P1_RANKS),
    ] {
        let mut seq = 0;
        for x in ranks {
            for y in COUNTER_DUEL_ROWS {
                spawns.push(PlayerCommand::new(
                    PlayerId(owner),
                    seq,
                    Command::DebugSpawnJittered {
                        owner: PlayerId(owner),
                        kind,
                        at: FxVec2::from_ints(x, y),
                        count: 1,
                        jitter_x100: COUNTER_DUEL_JITTER_X100,
                    },
                ));
                seq += 1;
            }
        }
    }
    let per_side =
        u32::try_from(COUNTER_DUEL_ROWS.len() * COUNTER_DUEL_P0_RANKS.len()).expect("ten a side");
    let seq = per_side;
    let stream = vec![
        (0, spawns),
        (
            1,
            vec![
                attack_move(0, seq, ids(1..=per_side), COUNTER_DUEL_P0_GOAL),
                attack_move(
                    1,
                    seq,
                    ids(per_side + 1..=2 * per_side),
                    COUNTER_DUEL_P1_GOAL,
                ),
            ],
        ),
    ];
    (setup, stream)
}

/// Line kinds of [`combat_bench`], one group of each per side, in spawn
/// order.
pub const COMBAT_BENCH_KINDS: [&str; 4] = ["skirmisher", "shieldbearer", "bowman", "outrider"];
/// P0's group centres in [`combat_bench`], in [`COMBAT_BENCH_KINDS`] order:
/// two columns of two in the all-grass west rectangle of `plains_1v1`
/// (x 0..43, y 24..41). P1's are these mirrored across x = 22.
pub const COMBAT_BENCH_P0_CENTRES: [(i32, i32); 4] = [(6, 28), (6, 36), (14, 28), (14, 36)];
/// P1's group centres in [`combat_bench`]: P0's mirrored across x = 22.
pub const COMBAT_BENCH_P1_CENTRES: [(i32, i32); 4] = [(38, 28), (38, 36), (30, 28), (30, 36)];
/// Where P0 attack-moves in [`combat_bench`]: the centre of P1's groups.
pub const COMBAT_BENCH_P0_GOAL: FxVec2 = FxVec2::from_ints(34, 32);
/// Where P1 attack-moves in [`combat_bench`]: the centre of P0's groups.
pub const COMBAT_BENCH_P1_GOAL: FxVec2 = FxVec2::from_ints(10, 32);
/// Spawn jitter of every [`combat_bench`] group, in hundredths of a tile
/// per axis: up to a tile either way, as in the counter duels.
pub const COMBAT_BENCH_JITTER_X100: u8 = 100;
/// Default seed of [`combat_bench`] (`sim-cli bench --combat --seed`).
pub const COMBAT_BENCH_SEED: u64 = 1;

/// The `sim-cli bench --combat` battle (ROADMAP M4a: 200 vs 200 fighting at
/// `units` 400), on an empty start on `plains_1v1` with `seed`. During tick
/// 0 each side spawns one `DebugSpawnJittered` group of `units / 8` per
/// kind of [`COMBAT_BENCH_KINDS`] ([`COMBAT_BENCH_JITTER_X100`]): P0 at
/// [`COMBAT_BENCH_P0_CENTRES`], then P1 at [`COMBAT_BENCH_P1_CENTRES`], so
/// P0's units are `UnitId(1..=units / 2)` and P1's the rest (`units` is
/// rounded down to a multiple of 8). During tick 1 P0 attack-moves to
/// [`COMBAT_BENCH_P0_GOAL`] and P1 to [`COMBAT_BENCH_P1_GOAL`]. Not in
/// [`NAMES`]: the stream depends on `units`, and it is not a golden fixture.
/// Fewer than 8 units gives an empty stream.
pub fn combat_bench(rules: &Rules, units: u16, seed: u64) -> (MatchSetup, Stream) {
    let setup = MatchSetup::scenario(rules, seed);
    let per_group = units / 8;
    if per_group == 0 {
        return (setup, Vec::new());
    }
    let per_side = u32::from(per_group) * 4;
    let mut spawns = Vec::new();
    for (owner, centres) in [(0u8, COMBAT_BENCH_P0_CENTRES), (1, COMBAT_BENCH_P1_CENTRES)] {
        for (seq, (kind, (x, y))) in (0u32..).zip(COMBAT_BENCH_KINDS.iter().zip(centres)) {
            spawns.push(PlayerCommand::new(
                PlayerId(owner),
                seq,
                Command::DebugSpawnJittered {
                    owner: PlayerId(owner),
                    kind: unit_kind(rules, kind),
                    at: FxVec2::from_ints(x, y),
                    count: per_group,
                    jitter_x100: COMBAT_BENCH_JITTER_X100,
                },
            ));
        }
    }
    let seq = u32::try_from(COMBAT_BENCH_KINDS.len()).expect("four kinds");
    let stream = vec![
        (0, spawns),
        (
            1,
            vec![
                attack_move(0, seq, ids(1..=per_side), COMBAT_BENCH_P0_GOAL),
                attack_move(
                    1,
                    seq,
                    ids(per_side + 1..=2 * per_side),
                    COMBAT_BENCH_P1_GOAL,
                ),
            ],
        ),
    ];
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
        "economy_walkthrough" => {
            let (s, c) = economy_walkthrough(rules);
            (s, c, ECONOMY_WALKTHROUGH_TICKS)
        }
        "combat_skirmish" => {
            let (s, c) = combat_skirmish(rules);
            (s, c, COMBAT_SKIRMISH_TICKS)
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
            // Seq numbers are unique per player.
            let mut keys: Vec<(PlayerId, u32)> = stream
                .iter()
                .flat_map(|(_, cmds)| cmds.iter().map(|c| (c.player, c.seq)))
                .collect();
            let n = keys.len();
            keys.sort_unstable();
            keys.dedup();
            assert_eq!(keys.len(), n, "{name}");
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
    fn combat_bench_spawns_mirrored_groups_and_attack_moves_them() {
        let rules = rules();
        let map = crate::map::Map::from_def(rules.map(&rules.default_map).unwrap());
        for ((x0, y0), (x1, y1)) in COMBAT_BENCH_P0_CENTRES
            .into_iter()
            .zip(COMBAT_BENCH_P1_CENTRES)
        {
            assert_eq!((x0 + x1, y0), (44, y1), "P1 mirrors P0 across x = 22");
            for (x, y) in [(x0, y0), (x1, y1)] {
                assert!((0..43).contains(&x) && (24..41).contains(&y), "({x}, {y})");
            }
        }
        for goal in [COMBAT_BENCH_P0_GOAL, COMBAT_BENCH_P1_GOAL] {
            assert!(map.passable(map.tile_of(goal)));
        }
        let (setup, stream) = combat_bench(&rules, 403, COMBAT_BENCH_SEED);
        assert_eq!(setup.seed, COMBAT_BENCH_SEED);
        assert_eq!(commands_at(&stream, 0).len(), 8);
        let delay = setup.cmd_delay;
        let sim = run(setup, rules.clone(), &stream, 1 + delay + 1);
        let view = sim.view();
        // 403 rounds down to 400: 50 of each kind per side, P0's ids first.
        assert_eq!(view.unit_count(), 400);
        for u in view.units() {
            let owner = if u.id.0 <= 200 { 0 } else { 1 };
            assert_eq!(u.owner, PlayerId(owner), "{:?}", u.id);
            assert!(u.order.is_some(), "{:?} attack-moves", u.id);
            let group = usize::try_from((u.id.0 - 1) % 200 / 50).unwrap();
            assert_eq!(u.kind, unit_kind(&rules, COMBAT_BENCH_KINDS[group]));
        }
        assert!(combat_bench(&rules, 7, 1).1.is_empty());
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
