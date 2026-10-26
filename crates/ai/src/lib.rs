//! Scripted opponents. Every bot is a deterministic function of the
//! [`sim::SimView`] and its own hashed [`sim::AiMemory`] (M5b); no bot owns
//! randomness, a clock or a float (`clippy.toml` bans them).
//!
//! M0 shipped two bots that need no data: [`Passive`] (never acts) and
//! [`Scripted`] (replays a fixed tick-stamped command list). M5b adds the
//! data-driven [`standard::StandardBot`] (knobs and build orders from
//! `data/ai/`, attached to `Rules::ai`), [`controller_for`], which builds
//! the controller for a match header's AI seats, and the engine-free
//! [`harness`] behind `sim-cli play-bots`.
//!
//! Module map:
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`seats`] | [`seats::Seats`]: one controller per AI slot, dispatched by player |
//! | [`harness`] | `BotMatch`, the `Observer` of the play-bots CSV facts, `GameRow`, `CSV_HEADER`, `exit_code` |
//! | [`standard`] | the Standard bot: cadence gate, memory codec, `Facts`, `Ctx`, `Ledger`, `MapAnalysis`, build-order satisfaction |
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod harness;
pub mod seats;
pub mod standard;

// The unit tests share the integration tests' testkit, which names this
// crate `ai`.
#[cfg(test)]
extern crate self as ai;
#[cfg(test)]
#[path = "../tests/common/mod.rs"]
pub(crate) mod testkit;

use rules::Rules;
use sim::{
    AiController, AiMemory, Command, FxVec2, MatchSetup, PlayerCommand, PlayerId, SimView, UnitId,
};
use std::collections::BTreeMap;

/// The bot id of [`Passive`] in `MatchSetup.ai_seats`.
pub const BOT_PASSIVE: &str = "passive";
/// The bot id of [`standard::StandardBot`] in `MatchSetup.ai_seats`.
pub const BOT_STANDARD: &str = "standard";
/// Every bot id [`controller_for`] accepts.
pub const BOTS: [&str; 2] = [BOT_PASSIVE, BOT_STANDARD];

/// Why [`controller_for`] could not build a controller for a header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotError {
    /// `ai_seats` does not name exactly the `is_ai` slots, once each, in
    /// id order (what `Sim::new` asserts).
    SlotMismatch {
        /// The players `ai_seats` names, in order.
        seats: Vec<PlayerId>,
        /// The `is_ai` slots, in order.
        ai_slots: Vec<PlayerId>,
    },
    /// A seat names a bot this build does not know.
    UnknownBot {
        /// The seat.
        player: PlayerId,
        /// The bot id.
        bot: String,
    },
    /// A seat names a personality its bot cannot take: one that is not in
    /// `data/ai/personalities.ron`, or any for `passive`.
    UnknownPersonality {
        /// The seat.
        player: PlayerId,
        /// The bot id.
        bot: String,
        /// The personality id.
        personality: String,
    },
}

impl core::fmt::Display for BotError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BotError::SlotMismatch { seats, ai_slots } => write!(
                f,
                "the header's ai_seats name players {:?} but its AI slots are {:?}",
                seats.iter().map(|p| p.0).collect::<Vec<_>>(),
                ai_slots.iter().map(|p| p.0).collect::<Vec<_>>()
            ),
            BotError::UnknownBot { player, bot } => write!(
                f,
                "player {}: unknown bot {bot:?} (known: {})",
                player.0,
                BOTS.join(", ")
            ),
            BotError::UnknownPersonality {
                player,
                bot,
                personality,
            } if bot == BOT_PASSIVE => write!(
                f,
                "player {}: bot \"passive\" takes no personality, got {personality:?}",
                player.0
            ),
            BotError::UnknownPersonality {
                player,
                bot,
                personality,
            } => write!(
                f,
                "player {}: bot {bot:?} has no personality {personality:?} in data/ai/personalities.ron",
                player.0
            ),
        }
    }
}

impl std::error::Error for BotError {}

/// Build the controller for `setup`'s AI seats under `rules`: one bot per
/// seat behind a [`seats::Seats`]. Every re-simulation (`sim-cli verify`,
/// `hash-dump`, `record`, the game and its `--headless-run`) builds bots
/// through this, so a header always gets the bots it was recorded with.
/// The slot check comes first, so a header `Sim::new` would reject is an
/// error here instead of a panic.
pub fn controller_for(
    setup: &MatchSetup,
    rules: &Rules,
) -> Result<Box<dyn AiController>, BotError> {
    let ai_slots: Vec<PlayerId> = setup
        .players
        .iter()
        .filter(|s| s.is_ai)
        .map(|s| s.id)
        .collect();
    let seats: Vec<PlayerId> = setup.ai_seats.iter().map(|s| s.player).collect();
    if seats != ai_slots {
        return Err(BotError::SlotMismatch { seats, ai_slots });
    }
    let mut by_player: BTreeMap<PlayerId, Box<dyn AiController>> = BTreeMap::new();
    for seat in &setup.ai_seats {
        let unknown_personality = || BotError::UnknownPersonality {
            player: seat.player,
            bot: seat.bot.clone(),
            personality: seat.personality.clone(),
        };
        let bot: Box<dyn AiController> = match seat.bot.as_str() {
            BOT_PASSIVE if seat.personality.is_empty() => Box::new(Passive),
            BOT_PASSIVE => return Err(unknown_personality()),
            BOT_STANDARD => {
                let personality = rules
                    .ai
                    .personality(&seat.personality)
                    .ok_or_else(unknown_personality)?;
                Box::new(standard::StandardBot::new(
                    rules,
                    setup,
                    seat.player,
                    personality,
                ))
            }
            _ => {
                return Err(BotError::UnknownBot {
                    player: seat.player,
                    bot: seat.bot.clone(),
                });
            }
        };
        by_player.insert(seat.player, bot);
    }
    Ok(Box::new(seats::Seats::new(by_player)))
}

/// One line about `player`'s bot for the dev panel and debugging (M5b):
/// for a Standard seat, the build-order window (step index and label),
/// the `Military` latch, the wave (phase and size, the next launch tick,
/// waves launched) and the defend latch (Town and age, defences
/// released), read from the view and the player's memory; `passive` for a
/// Passive seat, `human` for a slot with no seat. Call it outside
/// `think`, where `view.ai_memory` shows the memory.
pub fn describe(view: &SimView<'_>, player: PlayerId) -> String {
    let Some(seat) = view.setup().ai_seats.iter().find(|s| s.player == player) else {
        return "human".to_string();
    };
    if seat.bot != BOT_STANDARD {
        return seat.bot.clone();
    }
    let rules = view.rules();
    let Some(knobs) = rules.ai.personality(&seat.personality) else {
        return format!("standard {}: unknown personality", seat.personality);
    };
    let Some(order) = rules.ai.build_order(&knobs.build_order) else {
        return format!("standard {}: unknown build order", seat.personality);
    };
    let mem = standard::BotMemory::decode(view.ai_memory(player));
    let facts = standard::Facts::gather(view, player);
    let lookahead = usize::try_from(knobs.lookahead).unwrap_or(1);
    let window: Vec<String> =
        standard::build_order::window_of(order, &facts, rules, &mem, lookahead)
            .into_iter()
            .map(|i| format!("{i} {}", standard::build_order::step_label(&order.steps[i])))
            .collect();
    let wave = mem.wave.as_ref().map_or_else(
        || "none".to_string(),
        |w| format!("{:?}x{}", w.phase, w.units.len()),
    );
    let defend = mem.defend.map_or_else(
        || "none".to_string(),
        |d| {
            format!(
                "town {} for {}",
                d.town.0,
                view.tick.saturating_sub(d.since)
            )
        },
    );
    format!(
        "standard {} window=[{}] military={} wave={} next_wave={} waves={} defend={} defends={}",
        seat.personality,
        window.join(", "),
        if mem.military { "yes" } else { "no" },
        wave,
        mem.next_wave_at,
        mem.waves_launched,
        defend,
        mem.defends
    )
}

/// A bot that never acts and never writes memory. Used by tests, benches
/// and the M0 skeleton, and behind every `passive` seat.
#[derive(Debug, Default, Clone, Copy)]
pub struct Passive;

impl AiController for Passive {
    fn think(
        &mut self,
        _player: PlayerId,
        _view: &SimView<'_>,
        _memory: &mut AiMemory,
    ) -> Vec<Command> {
        Vec::new()
    }
}

/// A bot that issues a fixed list of `(tick, command)` pairs, in list order
/// within a tick. Used by `sim-cli selftest` and `hash-dump`.
///
/// `think` is a pure function of the view: it looks the tick up in the
/// sorted script and keeps no cursor, so a sim restored to an earlier or a
/// later tick gets exactly that tick's commands (a cursor would skip the
/// commands already issued once, making the controller's output depend on
/// the host's history rather than on the hashed state).
#[derive(Debug, Clone)]
pub struct Scripted {
    script: Vec<(u32, Command)>,
}

impl Scripted {
    /// Build a bot from a script. The script is stably sorted by tick so the
    /// caller may list entries in any order.
    pub fn new(mut script: Vec<(u32, Command)>) -> Scripted {
        script.sort_by_key(|(tick, _)| *tick);
        Scripted { script }
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

    /// Number of script entries.
    pub fn len(&self) -> usize {
        self.script.len()
    }

    /// `true` when the script is empty.
    pub fn is_empty(&self) -> bool {
        self.script.is_empty()
    }
}

impl AiController for Scripted {
    fn think(
        &mut self,
        _player: PlayerId,
        view: &SimView<'_>,
        _memory: &mut AiMemory,
    ) -> Vec<Command> {
        let start = self.script.partition_point(|(t, _)| *t < view.tick);
        let end = self.script.partition_point(|(t, _)| *t <= view.tick);
        self.script[start..end]
            .iter()
            .map(|(_, cmd)| cmd.clone())
            .collect()
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
    use sim::{AiSeat, Sim};
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
        assert_eq!(bot.len(), 2);
        assert!(!bot.is_empty());
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
    fn scripted_is_a_pure_function_of_the_tick_after_restore() {
        // Two sims with their own Scripted controllers: `a` runs straight
        // through; `b` is restored to a's tick-3 snapshot after it already
        // issued the tick-5 Surrender once, and must issue it again.
        let r = rules();
        let script = vec![(5, Command::Surrender)];
        let setup = MatchSetup::skirmish(&r, 3);
        let mut a = Sim::new(
            setup.clone(),
            r.clone(),
            Box::new(Scripted::new(script.clone())),
        );
        let mut b = Sim::new(setup, r, Box::new(Scripted::new(script)));
        for _ in 0..3 {
            a.step(&[]);
            b.step(&[]);
        }
        let snap = a.snapshot();
        for _ in 0..10 {
            b.step(&[]);
        }
        assert!(b.view().player(PlayerId(1)).unwrap().surrendered);
        b.restore(&snap).unwrap();
        assert!(!b.view().player(PlayerId(1)).unwrap().surrendered);
        for _ in 0..10 {
            a.step(&[]);
            b.step(&[]);
            assert_eq!(a.hash(), b.hash());
        }
        assert!(a.view().player(PlayerId(1)).unwrap().surrendered);
        assert!(b.view().player(PlayerId(1)).unwrap().surrendered);
    }

    #[test]
    fn selftest_script_is_deterministic() {
        let a = Scripted::selftest();
        let b = Scripted::selftest();
        assert_eq!(a.script, b.script);
        assert!(a.len() > 100);
        let total: usize = (0..1000).map(|t| selftest_human_commands(t).len()).sum();
        assert_eq!(total, 143 + 20);
        for t in 0..1000 {
            let cmds = selftest_human_commands(t);
            assert!(cmds.windows(2).all(|w| w[0].sort_key() < w[1].sort_key()));
        }
    }

    /// M5b: `controller_for` reads the header's seats: the slot check
    /// first, then each seat's bot and personality.
    #[test]
    fn controller_for_checks_slots_bots_and_personalities() {
        let r = rules();
        for setup in [
            MatchSetup::skirmish(&r, 1),
            MatchSetup::vs_bot(&r, 1, BOT_STANDARD, "standard"),
            MatchSetup::bots(&r, 1, ("standard", "standard"), ("passive", "")),
            MatchSetup::scenario(&r, 1),
        ] {
            assert!(controller_for(&setup, &r).is_ok(), "{:?}", setup.ai_seats);
        }
        let err = |setup: &MatchSetup| controller_for(setup, &r).err().unwrap();
        let mut s = MatchSetup::skirmish(&r, 1);
        s.ai_seats.clear();
        assert_eq!(
            err(&s),
            BotError::SlotMismatch {
                seats: vec![],
                ai_slots: vec![PlayerId(1)]
            }
        );
        s.ai_seats = vec![
            AiSeat {
                player: PlayerId(1),
                bot: "passive".into(),
                personality: String::new(),
            };
            2
        ];
        assert!(matches!(err(&s), BotError::SlotMismatch { .. }));
        // An unknown bot is reported even after a valid seat.
        let s = MatchSetup::bots(&r, 1, ("passive", ""), ("hard", "standard"));
        assert_eq!(
            err(&s).to_string(),
            "player 1: unknown bot \"hard\" (known: passive, standard)"
        );
        let s = MatchSetup::vs_bot(&r, 1, "standard", "rush");
        assert_eq!(
            err(&s).to_string(),
            "player 1: bot \"standard\" has no personality \"rush\" in data/ai/personalities.ron"
        );
        let s = MatchSetup::vs_bot(&r, 1, "passive", "standard");
        assert_eq!(
            err(&s).to_string(),
            "player 1: bot \"passive\" takes no personality, got \"standard\""
        );
    }
}
