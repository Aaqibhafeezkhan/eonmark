//! Scripted scenarios for `record`, `bench` and `fuzz`.
//!
//! The four golden-fixture streams (`move_500`, `move_500_short`,
//! `group_spiral`, `snapshot_restore`) are defined once in
//! [`sim::scenarios`] and shared with `crates/sim/tests/m1.rs` and the
//! criterion benches, so a recording, a test and a bench that name the same
//! scenario agree command for command. This module adds the streams only
//! the CLI needs: `selftest` (no Move, so it runs on any sim), the bench
//! `Crossing`, and the seeded random generator behind `fuzz`.
//!
//! Every scenario runs on [`MatchSetup::scenario`] (two human slots, debug
//! commands accepted, no AI seat), built through `ai::controller_for` like
//! every replay.

use rand_core::{Rng, SeedableRng};
use sim::scenarios::{BENCH_CROSS_SEED, EAST, Stream, WEST};
use sim::{
    BuildingId, BuildingKindId, Command, FxVec2, MatchSetup, PlayerCommand, PlayerId, Rules, Sim,
    TargetId, TechId, Tile, UnitId, UnitKindId,
};

/// A point in the river (impassable) north of the central ford. `bench
/// --astar` searches towards it so the open set never reaches the goal and
/// every expansion of the budget is spent.
pub const RIVER: FxVec2 = FxVec2::from_ints(63, 40);

/// The Yeoman, the only unit kind in M1.
pub const YEOMAN: UnitKindId = UnitKindId(0);

/// Fixture length of [`Scenario::Selftest`].
pub const SELFTEST_TICKS: u32 = 400;

/// Default seed for the CLI-only scenarios (`selftest`, `Crossing`); the
/// fixture scenarios carry their own seed in [`sim::scenarios`].
pub const DEFAULT_SEED: u64 = BENCH_CROSS_SEED;

/// A scripted command stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// One of [`sim::scenarios::NAMES`]: stream, seed and fixture length
    /// come from [`sim::scenarios::by_name`].
    Fixture(&'static str),
    /// Spawns, Stops and rejected commands only (an `AdvanceAge` every 7
    /// ticks rejected with `NotEnoughTechs`, an unknown unit kind on tick
    /// 100, an unknown player on tick 200): no Move, so it exercises the
    /// delay queue and rejections without pathing. Since the M4a tech
    /// slice nothing in the stream draws from the RNG (and since the M4a
    /// combat core slice no rejected command does), so the `rng` sub-hash
    /// keeps its seeded value for the whole run. 400 ticks.
    Selftest,
    /// `units` Yeomen crossing west to east in a band of groups, each to its
    /// mirrored goal, through all three fords ([`sim::scenarios::bench_cross`],
    /// the `bench` stream).
    Crossing {
        /// Units to spawn.
        units: u16,
    },
    /// `m5_first_match` (M5b): the Standard bot against the Standard bot
    /// from the standard start on the default map, seed
    /// [`FIRST_MATCH_SEED`], no human input, recorded to the step that
    /// decides the match (at the latest the 54000-tick cap). The first
    /// playable match's golden, and `play-bots --seeds 1` reproduces it:
    /// `record --scenario m5_first_match --seed N` records any
    /// Standard-vs-Standard seed. Outside [`sim::scenarios::NAMES`]: the
    /// sim cannot build bots.
    FirstMatch,
}

/// The `m5_first_match` fixture's seed.
pub const FIRST_MATCH_SEED: u64 = 1;

/// Ticks between `Hash` records in the `m5_first_match` recording.
pub const FIRST_MATCH_HASH_EVERY: u32 = 100;

impl Scenario {
    /// Names accepted by `--scenario`.
    pub fn names() -> Vec<&'static str> {
        let mut names = sim::scenarios::NAMES.to_vec();
        names.push("selftest");
        names.push("m5_first_match");
        names
    }

    /// Parse a `--scenario` name.
    pub fn parse(name: &str) -> Option<Scenario> {
        if name == "selftest" {
            return Some(Scenario::Selftest);
        }
        if name == "m5_first_match" {
            return Some(Scenario::FirstMatch);
        }
        sim::scenarios::NAMES
            .iter()
            .find(|n| **n == name)
            .map(|n| Scenario::Fixture(n))
    }

    /// The tick count the fixture of this scenario is recorded with.
    pub fn default_ticks(self, rules: &Rules) -> u32 {
        match self {
            Scenario::Fixture(name) => {
                sim::scenarios::by_name(name, rules)
                    .expect("Fixture names come from sim::scenarios::NAMES")
                    .2
            }
            Scenario::Selftest => SELFTEST_TICKS,
            Scenario::Crossing { .. } => sim::scenarios::MOVE_500_TICKS,
            Scenario::FirstMatch => rules.match_cap_ticks(),
        }
    }

    /// Ticks between `Hash` records in this scenario's recording:
    /// [`sim::scenarios::STALEMATE_45MIN_HASH_EVERY`] (1000) for the
    /// 54000-tick `stalemate_45min`, [`FIRST_MATCH_HASH_EVERY`] (100) for
    /// `m5_first_match`, the replay default (20) otherwise.
    /// `record --hash-every` and `--hash-every-tick` override it.
    pub fn default_hash_every(self) -> u32 {
        match self {
            Scenario::Fixture("stalemate_45min") => sim::scenarios::STALEMATE_45MIN_HASH_EVERY,
            Scenario::FirstMatch => FIRST_MATCH_HASH_EVERY,
            _ => sim::replay::DEFAULT_HASH_EVERY,
        }
    }

    /// Whether a recording of this scenario ends right after the step that
    /// decides the match (M5a): `capture_seat`, whose by-name length is
    /// only an upper bound, and `m5_first_match` (M5b). Its fixture then
    /// ends at the deciding tick, so its final hash is the one the game
    /// shows on the outcome overlay.
    pub fn stops_at_outcome(self) -> bool {
        matches!(
            self,
            Scenario::Fixture("capture_seat") | Scenario::FirstMatch
        )
    }

    /// The tick before whose step a recording snapshots the sim, builds a
    /// fresh one and restores into it, so `verify` passing on the fixture
    /// proves the restore was exact. Only `snapshot_restore` has one.
    pub fn snapshot_tick(self) -> Option<u32> {
        match self {
            Scenario::Fixture("snapshot_restore") => {
                Some(sim::scenarios::SNAPSHOT_RESTORE_SNAPSHOT_TICK)
            }
            _ => None,
        }
    }

    /// The match setup and command stream for a run of `ticks` ticks. A
    /// fixture scenario uses its own seed unless `seed` overrides it; the
    /// CLI-only scenarios use `seed` or [`DEFAULT_SEED`].
    pub fn setup_and_stream(
        self,
        rules: &Rules,
        seed: Option<u64>,
        ticks: u32,
    ) -> (MatchSetup, Stream) {
        match self {
            Scenario::Fixture(name) => {
                let (mut setup, stream, _) = sim::scenarios::by_name(name, rules)
                    .expect("Fixture names come from sim::scenarios::NAMES");
                if let Some(seed) = seed {
                    setup.seed = seed;
                }
                (setup, stream)
            }
            Scenario::Selftest => {
                let setup = MatchSetup::scenario(rules, seed.unwrap_or(DEFAULT_SEED));
                let stream = (0..ticks)
                    .filter_map(|t| {
                        let cmds = selftest(t);
                        (!cmds.is_empty()).then_some((t, cmds))
                    })
                    .collect();
                (setup, stream)
            }
            Scenario::Crossing { units } => {
                sim::scenarios::bench_cross(rules, units, seed.unwrap_or(DEFAULT_SEED))
            }
            Scenario::FirstMatch => (
                first_match(rules, seed.unwrap_or(FIRST_MATCH_SEED)),
                Vec::new(),
            ),
        }
    }
}

/// The `m5_first_match` header: the Standard bot (personality `standard`)
/// in both seats, from the standard start, no debug commands.
pub fn first_match(rules: &Rules, seed: u64) -> MatchSetup {
    MatchSetup::bots(
        rules,
        seed,
        (ai::BOT_STANDARD, "standard"),
        (ai::BOT_STANDARD, "standard"),
    )
}

/// The first sequence number of the `bench --bots` stream's commands, far
/// above any a bot reaches (each bot numbers its own from 0).
pub const BOTS_BENCH_SEQ: u32 = 1 << 30;

/// Resources each side of `bench --bots` is granted, per resource.
pub const BOTS_BENCH_GRANT: u32 = 2000;

/// The armed kinds `bench --bots` spawns in equal parts per side.
pub const BOTS_BENCH_KINDS: [&str; 4] = ["skirmisher", "bowman", "shieldbearer", "outrider"];

/// How far north of the enemy Seat's centre each side's forward half
/// stands, in tiles: inside the enemy's land and its `defend_radius_tiles`
/// (16), beyond the Seat's shot (7 tiles from its 3x3 footprint).
pub const BOTS_BENCH_FORWARD_TILES: i32 = 12;

/// The `sim-cli bench --bots` match (M5b, ROADMAP acceptance 3): the
/// Standard bot in both seats (`passive` seats for the control run) from
/// the standard start with debug commands on, and a tick-0 stream issued
/// by P0 with sequence numbers from [`BOTS_BENCH_SEQ`]. For each player:
/// `DebugAdvanceAge` (Masonry, so waves may launch), a `DebugGrant` of
/// [`BOTS_BENCH_GRANT`] of every resource, and `units / 2 - 8` armed units
/// in equal parts of [`BOTS_BENCH_KINDS`] (`DebugSpawnJittered`), half at
/// the player's own rally seed together with 2 Mangonels and a Supply
/// Wain, half [`BOTS_BENCH_FORWARD_TILES`] north of the enemy Seat. With
/// the 5 Yeomen of the standard start that is exactly `units / 2` units a
/// side when `units / 2 - 8` divides by 4 (200 a side at `--units 400`).
/// The first plans defend against the other side's forward half on their
/// land (which holds launches for up to `defend_max_block_ds`), then each
/// seat launches a wave of the army assembled, sieged and supplied at its
/// rally; meanwhile the build order runs on the grants and production
/// meets a full pop.
pub fn bots_bench(rules: &Rules, units: u16, seed: u64, standard: bool) -> (MatchSetup, Stream) {
    let seat = if standard {
        (ai::BOT_STANDARD, "standard")
    } else {
        (ai::BOT_PASSIVE, "")
    };
    let mut setup = MatchSetup::bots(rules, seed, seat, seat);
    setup.debug_commands = true;
    let kind = |id: &str| {
        let i = rules
            .units
            .iter()
            .position(|u| u.id == id)
            .expect("the bench's unit kinds are in units.ron");
        UnitKindId(u16::try_from(i).expect("few kinds"))
    };
    let knobs = rules
        .ai
        .personality("standard")
        .expect("the standard personality is in data/ai");
    let def = rules
        .map(&setup.map)
        .expect("the setup's map is in the rules");
    let start =
        |p: usize| FxVec2::from_ints(i32::from(def.starts[p].x), i32::from(def.starts[p].y));
    let armed = (units / 2).saturating_sub(8);
    let per_kind = armed / 4;
    let home = per_kind / 2;
    let mut cmds = Vec::new();
    let mut seq = BOTS_BENCH_SEQ;
    let mut push = |cmd: Command| {
        cmds.push(PlayerCommand::new(PlayerId(0), seq, cmd));
        seq += 1;
    };
    for p in 0..2u8 {
        let owner = PlayerId(p);
        let enemy = usize::from(1 - p);
        let rally = ai::standard::MapAnalysis::new(rules, &setup, owner, knobs).rally_seed;
        let rally = FxVec2::from_ints(i32::from(rally.x), i32::from(rally.y));
        let forward = start(enemy) + FxVec2::from_ints(0, -BOTS_BENCH_FORWARD_TILES);
        push(Command::DebugAdvanceAge { owner });
        for r in 0..rules.resources.resources.len() {
            push(Command::DebugGrant {
                owner,
                resource: u8::try_from(r).expect("few resources"),
                amount: BOTS_BENCH_GRANT,
            });
        }
        for id in BOTS_BENCH_KINDS {
            for (at, count) in [(rally, home), (forward, per_kind - home)] {
                if count > 0 {
                    push(Command::DebugSpawnJittered {
                        owner,
                        kind: kind(id),
                        at,
                        count,
                        jitter_x100: sim::scenarios::COMBAT_BENCH_JITTER_X100,
                    });
                }
            }
        }
        for (id, count) in [("mangonel", 2), ("supply_wain", 1)] {
            push(Command::DebugSpawnJittered {
                owner,
                kind: kind(id),
                at: rally,
                count,
                jitter_x100: sim::scenarios::COMBAT_BENCH_JITTER_X100,
            });
        }
    }
    (setup, vec![(0, cmds)])
}

/// A sim for a scenario setup, with the bots its header names
/// (`ai::controller_for`; the scenarios here have none).
///
/// # Panics
///
/// If the header names a bot this build does not know.
pub fn build(setup: &MatchSetup, rules: &Rules) -> Sim {
    let controller = ai::controller_for(setup, rules).expect("a scenario's bots are known");
    Sim::new(setup.clone(), rules.clone(), controller)
}

/// `DebugSpawn` of `count` Yeomen for `owner` around `at`.
pub fn spawn(owner: u8, seq: u32, at: FxVec2, count: u16) -> PlayerCommand {
    PlayerCommand::new(
        PlayerId(owner),
        seq,
        Command::DebugSpawn {
            owner: PlayerId(owner),
            kind: YEOMAN,
            at,
            count,
        },
    )
}

/// Spawns for both players on tick 0, an `AdvanceAge` every 7 ticks
/// (counted and rejected with `NotEnoughTechs`, drawing nothing from the
/// rng), a Stop every 50 ticks, an unknown unit kind on tick 100 and an
/// unknown player on tick 200. No Move, so the stream never touches
/// pathing or movement.
fn selftest(tick: u32) -> Vec<PlayerCommand> {
    let mut out = Vec::new();
    match tick {
        0 => {
            out.push(spawn(0, 0, WEST, 8));
            out.push(spawn(1, 0, EAST, 8));
        }
        100 => out.push(PlayerCommand::new(
            PlayerId(0),
            100,
            Command::DebugSpawn {
                owner: PlayerId(0),
                kind: UnitKindId(99),
                at: WEST,
                count: 1,
            },
        )),
        200 => out.push(PlayerCommand::new(
            PlayerId(7),
            0,
            Command::Stop {
                units: vec![UnitId(1)],
            },
        )),
        _ => {}
    }
    if tick % 7 == 3 {
        // Neither player has researched a tech, so `AdvanceAge` is rejected
        // with `NotEnoughTechs` (since the M4a tech slice; before it the
        // command was `NotImplemented` and drew from the rng). `AttackMove`
        // was used here through M1; since M2 the sim applies it like `Move`,
        // which would break this stream's no-movement promise.
        let player = u8::try_from(tick % 2).expect("0 or 1");
        out.push(PlayerCommand::new(
            PlayerId(player),
            tick,
            Command::AdvanceAge {
                player: PlayerId(player),
            },
        ));
    }
    if tick % 50 == 25 {
        out.push(PlayerCommand::new(
            PlayerId(0),
            tick,
            Command::Stop {
                units: vec![UnitId(1 + tick % 8)],
            },
        ));
    }
    out
}

/// A seeded random command stream for `fuzz`: valid spawns, Moves and
/// Stops for player 0, and invalid commands of every rejectable kind
/// (unknown player, no valid units, unknown unit kind, not enough techs,
/// unknown building (`Cancel`, `Gather`, `Train`, `SetRally`, `Research`),
/// a `Build` by a unit that does not exist, an `Attack` or a `DebugDamage`
/// on a unit that does not exist, a `DebugAdvanceAge` for a player that
/// does not exist, and everything player 1 issues after it surrenders). Returns the per-tick commands and the `(player, seq)` keys
/// the sim must reject.
/// `movement` enables Move commands.
pub fn random_commands(
    seed: u64,
    ticks: u32,
    movement: bool,
) -> (Vec<Vec<PlayerCommand>>, Vec<(PlayerId, u32)>) {
    let mut rng = rand_pcg::Pcg32::seed_from_u64(seed);
    let mut stream = Vec::with_capacity(ticks as usize);
    let mut rejected = Vec::new();
    let mut seq = [0u32; 2];
    let mut spawned: u32 = 0;
    let mut surrendered = false;
    let point = |rng: &mut rand_pcg::Pcg32| {
        let x = i32::try_from(rng.next_u32() % 128).expect("fits");
        let y = i32::try_from(rng.next_u32() % 128).expect("fits");
        FxVec2::from_ints(x, y)
    };
    for _ in 0..ticks {
        let mut cmds = Vec::new();
        let roll = rng.next_u32() % 100;
        // Player 0: valid commands.
        if roll < 10 && spawned < 300 {
            let count = u16::try_from(1 + rng.next_u32() % 20).expect("fits");
            spawned += u32::from(count);
            let at = point(&mut rng);
            cmds.push(spawn(0, seq[0], at, count));
            seq[0] += 1;
        } else if roll < 40 && spawned > 0 && movement {
            let n = 1 + rng.next_u32() % 12;
            let units = (0..n)
                .map(|_| UnitId(1 + rng.next_u32() % spawned))
                .collect();
            let target = point(&mut rng);
            cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq[0],
                Command::Move {
                    units,
                    target,
                    queue: false,
                },
            ));
            seq[0] += 1;
        } else if roll < 45 && spawned > 0 {
            let units = vec![UnitId(1 + rng.next_u32() % spawned)];
            cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq[0],
                Command::Stop { units },
            ));
            seq[0] += 1;
        }
        // Invalid commands, each rejected for a known reason.
        let bad = rng.next_u32() % 100;
        if bad < 8 {
            // UnknownPlayer.
            cmds.push(PlayerCommand::new(
                PlayerId(7),
                0,
                Command::Stop {
                    units: vec![UnitId(1)],
                },
            ));
            rejected.push((PlayerId(7), 0));
        } else if bad < 16 {
            // NoValidUnits: UnitId(0) is never allocated.
            cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq[0],
                Command::Stop {
                    units: vec![UnitId(0)],
                },
            ));
            rejected.push((PlayerId(0), seq[0]));
            seq[0] += 1;
        } else if bad < 24 {
            // UnknownUnitKind.
            let at = point(&mut rng);
            cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq[0],
                Command::DebugSpawn {
                    owner: PlayerId(0),
                    kind: UnitKindId(99),
                    at,
                    count: 1,
                },
            ));
            rejected.push((PlayerId(0), seq[0]));
            seq[0] += 1;
        } else if bad < 36 {
            // NotEnoughTechs: player 0 owns no building, so it never
            // researches anything. (Through M3b this was `NotImplemented`,
            // which drew from the rng.)
            cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq[0],
                Command::AdvanceAge {
                    player: PlayerId(0),
                },
            ));
            rejected.push((PlayerId(0), seq[0]));
            seq[0] += 1;
        } else if bad < 40 {
            // Player 1 owns nothing: NoValidUnits, or Surrendered afterwards.
            cmds.push(PlayerCommand::new(
                PlayerId(1),
                seq[1],
                Command::Stop {
                    units: vec![UnitId(1)],
                },
            ));
            rejected.push((PlayerId(1), seq[1]));
            seq[1] += 1;
        } else if bad < 41 && !surrendered {
            // The surrender itself is applied; everything after it is not.
            cmds.push(PlayerCommand::new(PlayerId(1), seq[1], Command::Surrender));
            seq[1] += 1;
            surrendered = true;
        } else if (41..45).contains(&bad) {
            // UnknownBuilding: BuildingId(0) is never allocated.
            cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq[0],
                Command::Cancel {
                    building: BuildingId(0),
                    slot: 0,
                },
            ));
            rejected.push((PlayerId(0), seq[0]));
            seq[0] += 1;
        } else if (53..57).contains(&bad) {
            // UnknownBuilding: a Train or a SetRally on BuildingId(0).
            let cmd = if bad < 55 {
                Command::Train {
                    building: BuildingId(0),
                    kind: UnitKindId(0),
                }
            } else {
                Command::SetRally {
                    building: BuildingId(0),
                    target: FxVec2::from_ints(1, 1),
                }
            };
            cmds.push(PlayerCommand::new(PlayerId(0), seq[0], cmd));
            rejected.push((PlayerId(0), seq[0]));
            seq[0] += 1;
        } else if (49..53).contains(&bad) {
            // UnknownBuilding: a Gather onto BuildingId(0).
            cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq[0],
                Command::Gather {
                    units: vec![UnitId(1)],
                    node: BuildingId(0),
                },
            ));
            rejected.push((PlayerId(0), seq[0]));
            seq[0] += 1;
        } else if (45..49).contains(&bad) {
            // NoValidUnits: a Build by UnitId(0).
            cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq[0],
                Command::Build {
                    unit: UnitId(0),
                    kind: BuildingKindId(0),
                    tile: Tile::new(0, 0),
                },
            ));
            rejected.push((PlayerId(0), seq[0]));
            seq[0] += 1;
        } else if (57..61).contains(&bad) {
            // UnknownBuilding: a Research at BuildingId(0).
            cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq[0],
                Command::Research {
                    building: BuildingId(0),
                    tech: TechId(0),
                },
            ));
            rejected.push((PlayerId(0), seq[0]));
            seq[0] += 1;
        } else if (61..65).contains(&bad) {
            // InvalidTarget: UnitId(0) is never allocated, and the target
            // is checked before the units. (Through the M4a tech slice an
            // Attack was `NotImplemented` and drew from the rng.)
            cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq[0],
                Command::Attack {
                    units: vec![UnitId(1)],
                    target: TargetId::Unit(UnitId(0)),
                },
            ));
            rejected.push((PlayerId(0), seq[0]));
            seq[0] += 1;
        } else if (65..69).contains(&bad) {
            // InvalidTarget: a DebugDamage on UnitId(0), which is never
            // allocated (M5a).
            cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq[0],
                Command::DebugDamage {
                    target: TargetId::Unit(UnitId(0)),
                    amount: 1 + rng.next_u32() % 100,
                },
            ));
            rejected.push((PlayerId(0), seq[0]));
            seq[0] += 1;
        } else if (69..73).contains(&bad) {
            // UnknownPlayer: a DebugAdvanceAge for a player that does not
            // exist (M5b); the issuer exists, so only the owner check
            // rejects it.
            cmds.push(PlayerCommand::new(
                PlayerId(0),
                seq[0],
                Command::DebugAdvanceAge { owner: PlayerId(7) },
            ));
            rejected.push((PlayerId(0), seq[0]));
            seq[0] += 1;
        }
        stream.push(cmds);
    }
    (stream, rejected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn rules() -> Rules {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap()
    }

    #[test]
    fn names_parse_and_default_ticks_match_the_roadmap() {
        let rules = rules();
        for name in Scenario::names() {
            let s = Scenario::parse(name).unwrap_or_else(|| panic!("{name}"));
            assert!(s.default_ticks(&rules) > 0, "{name}");
        }
        assert_eq!(Scenario::parse("nope"), None);
        assert_eq!(
            Scenario::parse("move_500"),
            Some(Scenario::Fixture("move_500"))
        );
        assert_eq!(Scenario::Fixture("move_500").default_ticks(&rules), 1200);
        assert!(Scenario::Fixture("move_500_short").default_ticks(&rules) <= 5000);
        assert_eq!(
            Scenario::Fixture("snapshot_restore").snapshot_tick(),
            Some(sim::scenarios::SNAPSHOT_RESTORE_SNAPSHOT_TICK)
        );
        assert_eq!(Scenario::Fixture("move_500").snapshot_tick(), None);
        assert_eq!(Scenario::Selftest.snapshot_tick(), None);
    }

    /// `m5_first_match` is the Standard bot against itself from the
    /// standard start at seed 1, with no input of its own: what `play-bots
    /// --seeds 1` plays; `--seed` overrides the seed.
    #[test]
    fn first_match_is_standard_against_standard_with_no_stream() {
        let rules = rules();
        let (setup, stream) = Scenario::FirstMatch.setup_and_stream(&rules, None, 54_000);
        assert!(stream.is_empty());
        assert_eq!(setup, first_match(&rules, 1));
        assert_eq!(
            setup,
            MatchSetup::bots(
                &rules,
                1,
                ("standard", "standard"),
                ("standard", "standard")
            )
        );
        assert!(!setup.debug_commands);
        let (setup, _) = Scenario::FirstMatch.setup_and_stream(&rules, Some(4), 54_000);
        assert_eq!(setup.seed, 4);
        assert!(!sim::scenarios::NAMES.contains(&"m5_first_match"));
    }

    /// `bench --bots`: the stream comes from P0 at sequence numbers from
    /// `1 << 30`, and the control run's header has Passive seats.
    #[test]
    fn bots_bench_stream_and_seats() {
        let rules = rules();
        let (setup, stream) = bots_bench(&rules, 400, 1, true);
        assert!(setup.debug_commands);
        assert!(setup.ai_seats.iter().all(|s| s.bot == "standard"));
        assert_eq!(stream.len(), 1);
        assert_eq!(stream[0].0, 0);
        assert!(stream[0].1.iter().all(|c| c.player == PlayerId(0)));
        assert!(stream[0].1.iter().all(|c| c.seq >= BOTS_BENCH_SEQ));
        let (control, control_stream) = bots_bench(&rules, 400, 1, false);
        assert!(control.ai_seats.iter().all(|s| s.bot == "passive"));
        assert_eq!(control_stream, stream);
    }

    #[test]
    fn fixture_streams_are_the_shared_ones_and_seed_overrides() {
        let rules = rules();
        let (setup, stream) = Scenario::Fixture("move_500").setup_and_stream(&rules, None, 1200);
        let (expected_setup, expected_stream) = sim::scenarios::move_500(&rules);
        assert_eq!(setup, expected_setup);
        assert_eq!(stream, expected_stream);
        let (setup, _) = Scenario::Fixture("move_500").setup_and_stream(&rules, Some(9), 1200);
        assert_eq!(setup.seed, 9);
    }

    #[test]
    fn selftest_stream_has_no_move() {
        let rules = rules();
        let (setup, stream) = Scenario::Selftest.setup_and_stream(&rules, None, SELFTEST_TICKS);
        assert_eq!(setup.seed, DEFAULT_SEED);
        assert!(stream.windows(2).all(|w| w[0].0 < w[1].0));
        for (t, cmds) in &stream {
            assert!(!cmds.is_empty(), "tick {t}");
            for pc in cmds {
                assert!(!matches!(pc.cmd, Command::Move { .. }), "tick {t}");
            }
        }
        assert_eq!(sim::scenarios::commands_at(&stream, 0).len(), 2);
        assert!(
            sim::scenarios::commands_at(&stream, 100)
                .iter()
                .any(|c| matches!(
                    c.cmd,
                    Command::DebugSpawn {
                        kind: UnitKindId(99),
                        ..
                    }
                ))
        );
    }

    #[test]
    fn crossing_is_the_shared_bench_cross_stream() {
        let rules = rules();
        let (setup, stream) = Scenario::Crossing { units: 12 }.setup_and_stream(&rules, None, 10);
        let (expected_setup, expected_stream) =
            sim::scenarios::bench_cross(&rules, 12, DEFAULT_SEED);
        assert_eq!(setup, expected_setup);
        assert_eq!(stream, expected_stream);
        let moved: usize = sim::scenarios::commands_at(&stream, 5)
            .iter()
            .map(|c| match &c.cmd {
                Command::Move { units, .. } => units.len(),
                _ => 0,
            })
            .sum();
        assert_eq!(moved, 12, "every spawned unit is moved");
        let (setup, _) = Scenario::Crossing { units: 12 }.setup_and_stream(&rules, Some(4), 10);
        assert_eq!(setup.seed, 4);
    }

    #[test]
    fn random_commands_are_repeatable_and_track_rejections() {
        let (a, ra) = random_commands(5, 200, false);
        let (b, rb) = random_commands(5, 200, false);
        assert_eq!(a, b);
        assert_eq!(ra, rb);
        assert!(!ra.is_empty());
        assert!(
            a.iter()
                .flatten()
                .any(|c| matches!(c.cmd, Command::DebugSpawn { .. }))
        );
        let (c, _) = random_commands(6, 200, false);
        assert_ne!(a, c);
    }
}
