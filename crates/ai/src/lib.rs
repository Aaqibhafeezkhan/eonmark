//! Scripted opponents. Every bot is a deterministic function of the
//! [`sim::SimView`] and its own hashed [`sim::AiMemory`] (M5b); no bot owns
//! randomness, a clock or a float (`clippy.toml` bans them).
//!
//! M0 shipped two bots that need no data: [`Passive`] (never acts) and
//! [`Scripted`] (replays a fixed tick-stamped command list). M5b adds the
//! data-driven [`standard::StandardBot`] (knobs and build orders from
//! `data/ai/`, attached to `Rules::ai`), [`controller_for`], which builds
//! the controller for a match header's AI seats, and the engine-free
//! [`harness`] behind `sim-cli play-bots`. M6 makes a seat's `bot` a
//! difficulty id (`easy`, `standard`, `hard`: [`bot_ids`],
//! [`is_standard_bot`]); every difficulty is the Standard bot playing the
//! knobs `rules::ai::resolve` derives, while the sim pays the level's
//! disclosed bonus.
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
pub const BOT_PASSIVE: &str = rules::ai::PASSIVE_ID;
/// The default bot id: the `standard` difficulty, played by
/// [`standard::StandardBot`] (M6: any difficulty id of
/// `data/ai/difficulty/` is a Standard bot, [`is_standard_bot`]).
pub const BOT_STANDARD: &str = rules::ai::STANDARD_DIFFICULTY;
/// The personality pseudo-id that `play-bots` and the game resolve per
/// seed with [`resolve_personality`] (M6).
pub const RANDOM_PERSONALITY: &str = "random";

/// Every bot id [`controller_for`] accepts under `rules` (M6): `passive`,
/// then the difficulty ids of `data/ai/difficulty/` in id order.
pub fn bot_ids(rules: &Rules) -> Vec<String> {
    std::iter::once(BOT_PASSIVE.to_string())
        .chain(rules.ai.difficulties.keys().cloned())
        .collect()
}

/// Whether `bot` is played by [`standard::StandardBot`]: it names a
/// difficulty level of `rules` (M6).
pub fn is_standard_bot(rules: &Rules, bot: &str) -> bool {
    rules.ai.difficulty(bot).is_some()
}

/// The salt of [`resolve_personality`]'s hash.
const RANDOM_PERSONALITY_SALT: u64 = 0x5241_4e44_4f4d;

/// The personality the `random` choice gives `player` at `seed` (M6): one
/// of the personalities with `random: true` in
/// `data/ai/personalities.ron`, in file order, picked by a hash of the
/// seed and the player (`sim::ai_noise`, never the sim's `Pcg32`), so a
/// seed always resolves to the same style and the two seats of a
/// bot-versus-bot match draw independently. The result is written to the
/// header, so a replay never resolves it again. Adding a personality to
/// the pool remaps the seeds.
///
/// # Errors
///
/// When no personality is in the pool.
pub fn resolve_personality(rules: &Rules, seed: u64, player: PlayerId) -> Result<String, String> {
    let pool: Vec<&str> = rules
        .ai
        .personalities
        .iter()
        .filter(|p| p.random)
        .map(|p| p.id.as_str())
        .collect();
    if pool.is_empty() {
        return Err(
            "\"random\" needs a personality with `random: true` in data/ai/personalities.ron; none has it"
                .to_string(),
        );
    }
    let n = u64::try_from(pool.len()).expect("few personalities");
    let pick = sim::ai_noise(seed, player, RANDOM_PERSONALITY_SALT) % n;
    Ok(pool[usize::try_from(pick).expect("below the pool length")].to_string())
}

/// The bot statistics of `player`'s seat (M6), read from its memory
/// outside `think`: waves, raids, retreats and wiped waves, for the
/// play-bots CSV and the game-over screen. `None` for a human slot or a
/// `passive` seat.
pub fn bot_stats(view: &SimView<'_>, player: PlayerId) -> Option<standard::BotStats> {
    let seat = view.setup().ai_seats.iter().find(|s| s.player == player)?;
    if !is_standard_bot(view.rules(), &seat.bot) {
        return None;
    }
    Some(standard::stats(view.ai_memory(player)))
}

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
    /// A seat names a bot the rules do not know.
    UnknownBot {
        /// The seat.
        player: PlayerId,
        /// The bot id.
        bot: String,
        /// The bot ids the rules know ([`bot_ids`]).
        known: Vec<String>,
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
            BotError::UnknownBot { player, bot, known } => write!(
                f,
                "player {}: unknown bot {bot:?} (known: {})",
                player.0,
                known.join(", ")
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
/// error here instead of a panic. A seat's `bot` is `passive` or a
/// difficulty id (M6), whose Standard bot plays its personality's knobs
/// resolved for that level.
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
            id => {
                let Some(difficulty) = rules.ai.difficulty(id) else {
                    return Err(BotError::UnknownBot {
                        player: seat.player,
                        bot: seat.bot.clone(),
                        known: bot_ids(rules),
                    });
                };
                let personality = rules
                    .ai
                    .personality(&seat.personality)
                    .ok_or_else(unknown_personality)?;
                Box::new(standard::StandardBot::new(
                    rules,
                    setup,
                    seat.player,
                    personality,
                    difficulty,
                ))
            }
        };
        by_player.insert(seat.player, bot);
    }
    Ok(Box::new(seats::Seats::new(by_player)))
}

/// One line about `player`'s bot for the dev panel and debugging (M5b):
/// for a Standard seat, its difficulty and personality, the build-order
/// window (step index and label), the difficulty's first raid (M6: its
/// deadline tick, or `none`) with the raids and retreats so far, the
/// `Military` latch, the wave (phase and size, the next launch tick, waves
/// launched) and the defend latch (Town and age, defences released), read
/// from the view and the player's memory; `passive` for a Passive seat,
/// `human` for a slot with no seat. Call it outside `think`, where
/// `view.ai_memory` shows the memory.
pub fn describe(view: &SimView<'_>, player: PlayerId) -> String {
    let Some(seat) = view.setup().ai_seats.iter().find(|s| s.player == player) else {
        return "human".to_string();
    };
    let rules = view.rules();
    let Some(level) = rules.ai.difficulty(&seat.bot) else {
        return seat.bot.clone();
    };
    let Some(personality) = rules.ai.personality(&seat.personality) else {
        return format!("{} {}: unknown personality", seat.bot, seat.personality);
    };
    let knobs = rules::ai::resolve(personality, level);
    let Some(order) = rules
        .ai
        .build_order(&knobs.build_order)
        .map(|o| rules::ai::resolve_order(o, level))
    else {
        return format!("{} {}: unknown build order", seat.bot, seat.personality);
    };
    let order = &order;
    let stats = standard::stats(view.ai_memory(player));
    let raid = level.first_raid.map_or_else(
        || "none".to_string(),
        |r| format!("by {}", rules.ticks_from_ds(r.by_ds)),
    );
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
        "{} {} window=[{}] raid={raid} raids={} retreats={} military={} wave={} next_wave={} waves={} defend={} defends={}",
        seat.bot,
        seat.personality,
        window.join(", "),
        stats.raids,
        stats.retreats,
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

    /// M6: the bot ids are `passive` and the difficulty ids; only the
    /// difficulties are Standard bots.
    #[test]
    fn controller_for_builds_every_difficulty_and_lists_ids_on_error() {
        let r = rules();
        assert_eq!(bot_ids(&r), ["passive", "easy", "hard", "standard"]);
        for id in ["easy", "standard", "hard"] {
            assert!(is_standard_bot(&r, id), "{id}");
            let setup = MatchSetup::bots(&r, 3, (id, "standard"), (id, "standard"));
            let mut sim = Sim::new(
                setup.clone(),
                r.clone(),
                controller_for(&setup, &r).unwrap(),
            );
            for _ in 0..30 {
                sim.step(&[]);
            }
            assert!(sim.view().commands_applied() > 0, "{id} plays");
        }
        assert!(!is_standard_bot(&r, BOT_PASSIVE));
        assert!(!is_standard_bot(&r, "nightmare"));
    }

    /// M6: a Standard bot's knobs are `resolve(personality, difficulty)`,
    /// a pure function of the rules and the header: two bots built for
    /// the same seat agree, and `standard` plays the personality with its
    /// level's all-in time and (M6 owner decision 12) no retreat.
    #[test]
    fn knobs_are_pure_functions_of_rules_and_header() {
        let r = rules();
        let p = r.ai.personality("standard").unwrap();
        let setup = MatchSetup::bots(&r, 1, ("hard", "standard"), ("easy", "standard"));
        for (seat, id) in [(PlayerId(0), "hard"), (PlayerId(1), "easy")] {
            let level = r.ai.difficulty(id).unwrap();
            let a = standard::StandardBot::new(&r, &setup, seat, p, level);
            let b = standard::StandardBot::new(&r, &setup, seat, p, level);
            assert_eq!(a.knobs(), b.knobs());
            assert_eq!(a.knobs(), &rules::ai::resolve(p, level));
            assert_eq!(a.first_raid(), level.first_raid);
            assert_eq!(a.difficulty(), id);
            // The build order it plays is the level's resolved order (M6
            // owner decision 12: Easy's Statecraft II appended).
            let order = r.ai.build_order(&p.build_order).unwrap();
            assert_eq!(a.order(), &rules::ai::resolve_order(order, level));
        }
        let first_workers = |o: &rules::BuildOrder| {
            o.steps.iter().find_map(|s| match s {
                rules::Step::Workers { count, .. } => Some(*count),
                _ => None,
            })
        };
        let easy = r.ai.difficulty("easy").unwrap();
        let easy_bot = standard::StandardBot::new(&r, &setup, PlayerId(1), p, easy);
        assert_eq!(first_workers(easy_bot.order()), Some(7));
        let shipped = r.ai.build_order(&p.build_order).unwrap();
        assert_eq!(easy_bot.order().steps.len(), shipped.steps.len() + 1);
        assert_eq!(
            easy_bot.order().steps.last(),
            Some(&rules::Step::Research {
                tech: "statecraft_2".into(),
                optional: true
            })
        );
        let standard = r.ai.difficulty("standard").unwrap();
        let bot = standard::StandardBot::new(&r, &setup, PlayerId(0), p, standard);
        let mut want = p.clone();
        // Standard retreats as its personality does (the review of the M6
        // retune put it back); only its all-in time moves.
        want.all_in_after_ds = 21600;
        assert_eq!(bot.knobs(), &want);
        assert_eq!(bot.order(), shipped);
        assert_eq!(first_workers(bot.order()), Some(7));
        // `describe` reads the same resolved window for the Easy seat (at
        // tick 0, before the first plan, the window starts at step 0).
        let sim = Sim::new(
            setup.clone(),
            r.clone(),
            controller_for(&setup, &r).unwrap(),
        );
        let line = describe(&sim.view(), PlayerId(1));
        assert!(
            line.starts_with("easy standard window=[0 workers 7, "),
            "{line}"
        );
        let line = describe(&sim.view(), PlayerId(0));
        assert!(
            line.starts_with("hard standard window=[0 workers 7, "),
            "{line}"
        );
    }

    /// M6: `random` resolves per seed and seat from the pool of
    /// personalities with `random: true`, the same way every time; with
    /// no such personality it is an error.
    #[test]
    fn resolve_personality_draws_from_the_random_pool() {
        let mut r = rules();
        // The shipped pool (slice 2): rush, boom and tower, never the
        // Balanced `standard`, an explicit choice.
        let shipped: Vec<String> = (0..60)
            .map(|seed| resolve_personality(&r, seed, PlayerId(1)).unwrap())
            .collect();
        for id in ["rush", "boom", "tower"] {
            assert!(shipped.iter().any(|p| p == id), "{id} is drawn");
        }
        assert!(!shipped.iter().any(|p| p == "standard"));
        for p in &mut r.ai.personalities {
            p.random = false;
        }
        let err = resolve_personality(&r, 1, PlayerId(1)).unwrap_err();
        assert!(err.contains("random: true"), "{err}");
        let mut extra = |id: &str| {
            let mut p = r.ai.personality("standard").unwrap().clone();
            p.id = id.to_string();
            p.random = true;
            r.ai.personalities.push(p);
        };
        extra("a");
        extra("b");
        extra("c");
        let picks: Vec<String> = (0..60)
            .map(|seed| resolve_personality(&r, seed, PlayerId(1)).unwrap())
            .collect();
        for id in ["a", "b", "c"] {
            assert!(picks.iter().any(|p| p == id), "{id} is drawn");
        }
        assert!(!picks.iter().any(|p| p == "standard"), "not in the pool");
        let again: Vec<String> = (0..60)
            .map(|seed| resolve_personality(&r, seed, PlayerId(1)).unwrap())
            .collect();
        assert_eq!(picks, again);
        let other_seat: Vec<String> = (0..60)
            .map(|seed| resolve_personality(&r, seed, PlayerId(0)).unwrap())
            .collect();
        assert_ne!(picks, other_seat, "the seats draw independently");
    }

    /// M6: `bot_stats` reads a Standard seat's memory and is `None` for a
    /// human slot or a Passive seat.
    #[test]
    fn bot_stats_reads_standard_seats_only() {
        let r = rules();
        let setup = MatchSetup::bots(&r, 1, ("hard", "standard"), ("passive", ""));
        let sim = Sim::new(
            setup.clone(),
            r.clone(),
            controller_for(&setup, &r).unwrap(),
        );
        let view = sim.view();
        assert_eq!(
            bot_stats(&view, PlayerId(0)),
            Some(standard::BotStats::default())
        );
        assert_eq!(bot_stats(&view, PlayerId(1)), None);
        let human = Sim::new(MatchSetup::skirmish(&r, 1), r, Box::new(Passive));
        assert_eq!(bot_stats(&human.view(), PlayerId(0)), None);
    }

    /// M5b: `controller_for` reads the header's seats: the slot check
    /// first, then each seat's bot and personality. M6: every difficulty
    /// id is a bot, and the unknown ones are ids no data file defines.
    #[test]
    fn controller_for_checks_slots_bots_and_personalities() {
        let r = rules();
        for setup in [
            MatchSetup::skirmish(&r, 1),
            MatchSetup::vs_bot(&r, 1, BOT_STANDARD, "standard"),
            MatchSetup::bots(&r, 1, ("standard", "standard"), ("passive", "")),
            MatchSetup::bots(&r, 1, ("easy", "standard"), ("hard", "standard")),
            MatchSetup::vs_bot(&r, 1, "hard", "standard"),
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
        // An unknown bot is reported even after a valid seat, with the ids
        // the rules know.
        let s = MatchSetup::bots(&r, 1, ("passive", ""), ("nightmare", "standard"));
        assert_eq!(
            err(&s).to_string(),
            "player 1: unknown bot \"nightmare\" (known: passive, easy, hard, standard)"
        );
        let s = MatchSetup::vs_bot(&r, 1, "standard", "zerg");
        assert_eq!(
            err(&s).to_string(),
            "player 1: bot \"standard\" has no personality \"zerg\" in data/ai/personalities.ron"
        );
        let s = MatchSetup::vs_bot(&r, 1, "hard", "zerg");
        assert_eq!(
            err(&s).to_string(),
            "player 1: bot \"hard\" has no personality \"zerg\" in data/ai/personalities.ron"
        );
        let s = MatchSetup::vs_bot(&r, 1, "passive", "standard");
        assert_eq!(
            err(&s).to_string(),
            "player 1: bot \"passive\" takes no personality, got \"standard\""
        );
    }
}
