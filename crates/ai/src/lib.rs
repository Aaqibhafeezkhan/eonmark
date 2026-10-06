//! Scripted opponents. Every bot is a deterministic function of the
//! [`sim::SimView`] and the `Pcg32` stream inside the sim; no bot owns
//! randomness, a clock or a float (`clippy.toml` bans them).
//!
//! M0 ships two bots: [`Passive`] (never acts) and [`Scripted`] (replays a
//! fixed tick-stamped command list). The build-order executor, personalities
//! and the difficulty table from `data/ai/` arrive in M5b.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use sim::{AiController, Command, FxVec2, PlayerCommand, PlayerId, SimView, UnitId};

/// A bot that never acts. Used by tests, benches and the M0 skeleton.
#[derive(Debug, Default, Clone, Copy)]
pub struct Passive;

impl AiController for Passive {
    fn think(&mut self, _player: PlayerId, _view: &SimView<'_>) -> Vec<Command> {
        Vec::new()
    }
}

/// A bot that issues a fixed list of `(tick, command)` pairs, in list order
/// within a tick. Used by `sim-cli selftest` and `hash-dump`.
#[derive(Debug, Clone)]
pub struct Scripted {
    script: Vec<(u32, Command)>,
    next: usize,
}

impl Scripted {
    /// Build a bot from a script. The script is stably sorted by tick so the
    /// caller may list entries in any order.
    pub fn new(mut script: Vec<(u32, Command)>) -> Scripted {
        script.sort_by_key(|(tick, _)| *tick);
        Scripted { script, next: 0 }
    }

    /// The fixed script used by `sim-cli selftest`: a Stop every 10 ticks, a
    /// Move every 25 ticks across the first 1000 ticks. Never surrenders, so
    /// the whole stream stays in play.
    pub fn selftest() -> Scripted {
        let mut script = Vec::new();
        for tick in 0..1000u32 {
            if tick % 10 == 4 {
                script.push((
                    tick,
                    Command::Stop {
                        units: vec![UnitId(5)],
                    },
                ));
            }
            if tick.is_multiple_of(25) {
                let t = i32::try_from(tick % 128).expect("fits");
                script.push((
                    tick,
                    Command::Move {
                        units: vec![UnitId(5), UnitId(6)],
                        target: FxVec2::from_ints(127 - t, t),
                        queue: false,
                    },
                ));
            }
        }
        Scripted::new(script)
    }

    /// Entries not yet issued.
    pub fn remaining(&self) -> usize {
        self.script.len() - self.next
    }
}

impl AiController for Scripted {
    fn think(&mut self, _player: PlayerId, view: &SimView<'_>) -> Vec<Command> {
        let mut out = Vec::new();
        while let Some((tick, cmd)) = self.script.get(self.next) {
            if *tick > view.tick {
                break;
            }
            // Entries for ticks already past (possible after a restore to an
            // earlier tick is never done, but after one to a later tick) are
            // skipped rather than replayed late.
            if *tick == view.tick {
                out.push(cmd.clone());
            }
            self.next += 1;
        }
        out
    }
}

/// The fixed human-slot command stream used by `sim-cli selftest` and
/// `hash-dump`: a two-unit Move every 7 ticks, a Stop every 50 ticks.
/// Sequence numbers are unique per tick so the sort key is a total order.
pub fn selftest_human_commands(tick: u32) -> Vec<PlayerCommand> {
    let p = PlayerId(0);
    let mut out = Vec::new();
    if tick.is_multiple_of(7) {
        let t = i32::try_from(tick % 128).expect("fits");
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

#[cfg(test)]
mod tests {
    use super::*;
    use rules::Rules;
    use sim::{MatchSetup, Sim};
    use std::path::Path;

    fn rules() -> Rules {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap()
    }

    #[test]
    fn passive_never_changes_the_hash_beyond_ticking() {
        let r = rules();
        let mut a = Sim::new(MatchSetup::skirmish(&r, 1), r.clone(), Box::new(Passive));
        let mut b = Sim::new(MatchSetup::skirmish(&r, 1), r, Box::new(Passive));
        for _ in 0..50 {
            a.step(&[]);
            b.step(&[]);
        }
        assert_eq!(a.hash(), b.hash());
        assert_eq!(a.view().commands_applied(), 0);
    }

    #[test]
    fn scripted_issues_commands_on_their_tick_only() {
        let r = rules();
        let script = vec![
            (5, Command::Surrender),
            (
                2,
                Command::Stop {
                    units: vec![UnitId(1)],
                },
            ),
        ];
        let bot = Scripted::new(script);
        assert_eq!(bot.remaining(), 2);
        let mut sim = Sim::new(MatchSetup::skirmish(&r, 3), r, Box::new(bot));
        let delay = sim.setup().cmd_delay;
        for _ in 0..(5 + delay) {
            sim.step(&[]);
            assert!(!sim.view().player(PlayerId(1)).unwrap().surrendered);
        }
        sim.step(&[]);
        assert!(sim.view().player(PlayerId(1)).unwrap().surrendered);
        // Stop at tick 2 and Surrender at tick 5 both applied.
        assert_eq!(sim.view().commands_applied(), 2);
    }

    #[test]
    fn selftest_script_is_deterministic() {
        let a = Scripted::selftest();
        let b = Scripted::selftest();
        assert_eq!(a.script, b.script);
        assert!(a.remaining() > 100);
        let total: usize = (0..1000).map(|t| selftest_human_commands(t).len()).sum();
        assert_eq!(total, 143 + 20);
        for t in 0..1000 {
            let cmds = selftest_human_commands(t);
            assert!(cmds.windows(2).all(|w| w[0].sort_key() < w[1].sort_key()));
        }
    }
}
