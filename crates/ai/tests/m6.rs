//! M6 bot tests that run whole matches (`cargo test -p ai`): every one
//! plays several games or games longer than 5000 ticks, so each is
//! `#[ignore]`d with an `m6_` prefix; CI's `fun-gate` job runs
//! `cargo test -p ai --locked -- --ignored m6_`.

mod common;

use ai::harness::{BotMatch, Side};
use ai::standard::{BotMemory, WaveKind};
use common::{P0, P1};
use sim::{MatchSetup, PlayerId, Sim};

/// Run `games` on scoped threads, results in input order.
fn par<T: Send>(games: Vec<Box<dyn FnOnce() -> T + Send>>) -> Vec<T> {
    std::thread::scope(|s| {
        let handles: Vec<_> = games.into_iter().map(|g| s.spawn(g)).collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

/// The four shipped personalities.
const PERSONALITIES: [&str; 4] = ["standard", "rush", "boom", "tower"];

/// A `bot`/`personality` seat against Passive, the bot in `seat`.
fn vs_passive(seed: u64, seat: PlayerId, bot: &str, personality: &str) -> MatchSetup {
    let r = common::rules();
    if seat == P0 {
        MatchSetup::bots(&r, seed, (bot, personality), ("passive", ""))
    } else {
        MatchSetup::bots(&r, seed, ("passive", ""), (bot, personality))
    }
}

/// The bot side's facts after running `setup` to `ticks` or the outcome,
/// as the play-bots row reports them (the row adds the `MatchStats`
/// columns, `first_on_land_tick` among them, to the observed sides).
fn bot_side(setup: MatchSetup, seat: PlayerId, ticks: u32) -> (Side, Option<sim::MatchResult>) {
    let r = common::rules();
    let seed = setup.seed;
    let mut m = BotMatch::with_bots(setup, r).unwrap();
    m.run(ticks);
    let row = m.row(seed);
    let sides = row.sides.expect("no panic");
    (sides[usize::from(seat.0)], m.sim.result())
}

/// ROADMAP M6 acceptance 4, first clause, for every personality (Random
/// may pick any of rush, boom and tower) and from both seats: a Hard bot
/// against Passive orders an attack into the enemy's land before tick
/// 14400 (minute 12) and has an armed unit standing on it before then,
/// in seeds 1 to 3.
#[test]
#[ignore = "long: 24 games to 14400 ticks; CI runs --ignored m6_"]
fn m6_hard_attacks_before_14400_vs_passive_every_personality_both_sides() {
    let mut games: Vec<Box<dyn FnOnce() -> (String, Side) + Send>> = Vec::new();
    for p in PERSONALITIES {
        for seat in [P0, P1] {
            for seed in 1..=3u64 {
                games.push(Box::new(move || {
                    let (side, _) = bot_side(vs_passive(seed, seat, "hard", p), seat, 14_400);
                    (format!("{p} seat {} seed {seed}", seat.0), side)
                }));
            }
        }
    }
    for (what, side) in par(games) {
        let attack = side.first_attack_tick.unwrap_or(u32::MAX);
        let on_land = side.first_on_land_tick.unwrap_or(u32::MAX);
        assert!(attack < 14_400, "{what}: first_attack_tick {attack}");
        assert!(on_land < 14_400, "{what}: first_on_land_tick {on_land}");
    }
}

/// The rush personality (at Standard) is the early one: its first attack
/// into Passive's land comes before tick 8400 (minute 7), from either
/// seat, in seeds 1 to 3.
#[test]
#[ignore = "long: six games to 8400 ticks; CI runs --ignored m6_"]
fn m6_rush_first_attack_before_8400_vs_passive() {
    let mut games: Vec<Box<dyn FnOnce() -> (String, Side) + Send>> = Vec::new();
    for seat in [P0, P1] {
        for seed in 1..=3u64 {
            games.push(Box::new(move || {
                let (side, _) = bot_side(vs_passive(seed, seat, "standard", "rush"), seat, 8_400);
                (format!("seat {} seed {seed}", seat.0), side)
            }));
        }
    }
    for (what, side) in par(games) {
        let attack = side.first_attack_tick.unwrap_or(u32::MAX);
        assert!(attack < 8_400, "{what}: first_attack_tick {attack}");
    }
}

/// Every personality at every difficulty takes Passive's Seat before tick
/// 36000 from either seat (seed 1): a decisive `SeatCaptured` victory.
#[test]
#[ignore = "long: 24 games to about 15000-36000 ticks; CI runs --ignored m6_"]
fn m6_every_personality_and_difficulty_beats_passive_by_seat_before_36000() {
    let mut games: Vec<Box<dyn FnOnce() -> (String, PlayerId, Option<sim::MatchResult>) + Send>> =
        Vec::new();
    for bot in ["easy", "standard", "hard"] {
        for p in PERSONALITIES {
            for seat in [P0, P1] {
                games.push(Box::new(move || {
                    let (_, result) = bot_side(vs_passive(1, seat, bot, p), seat, 36_000);
                    (format!("{bot}/{p} seat {}", seat.0), seat, result)
                }));
            }
        }
    }
    for (what, seat, result) in par(games) {
        let result = result.unwrap_or_else(|| panic!("{what}: undecided at 36000"));
        assert_eq!(
            (result.outcome, result.reason),
            (
                sim::Outcome::Victory {
                    player: seat,
                    decisive: true
                },
                sim::OutcomeReason::SeatCaptured
            ),
            "{what}"
        );
    }
}

/// The bot's hashed memory stays under 1 KiB on every tick of two
/// personality matches that use every M6 field (raids, retreats, the
/// deadline raid, reactive towers) to the outcome or tick 30000.
#[test]
#[ignore = "long: two games to 30000 ticks; CI runs --ignored m6_"]
fn m6_memory_stays_under_1_kib() {
    let games: Vec<Box<dyn FnOnce() -> (usize, u32) + Send>> = [
        (("hard", "rush"), ("standard", "tower")),
        (("standard", "boom"), ("easy", "rush")),
    ]
    .into_iter()
    .map(|(a, b)| {
        Box::new(move || {
            let r = common::rules();
            let mut m = BotMatch::with_bots(MatchSetup::bots(&r, 3, a, b), r).unwrap();
            let mut max = 0usize;
            while m.sim.result().is_none() && m.sim.tick() < 30_000 {
                m.step();
                let view = m.sim.view();
                for p in [P0, P1] {
                    max = max.max(view.ai_memory(p).len());
                }
            }
            (max, m.sim.tick())
        }) as Box<_>
    })
    .collect();
    for (max, tick) in par(games) {
        assert!(max < 1024, "{max} bytes by tick {tick}");
        assert!(max > 0);
    }
}

/// Hidden state in the M6 behaviours: a rush-against-tower match (seed 2,
/// where the Standard rush seat raids and the Easy tower seat retreats;
/// a Standard seat retreats as its personality does) restored into a fresh
/// `Sim` with fresh bots from the header, once while a raid is out and
/// once just after a retreat, matches the uninterrupted run, every
/// sub-hash, every tick, for the next 2000 ticks.
#[test]
#[ignore = "long: games to about 20000 ticks; CI runs --ignored m6_"]
fn m6_restore_during_a_raid_idle_and_after_a_retreat_matches_uninterrupted() {
    type Pick = fn(&BotMemory, &BotMemory) -> bool;
    let raid_out: Pick = |_, now| now.wave.as_ref().is_some_and(|w| w.kind == WaveKind::Raid);
    let after_retreat: Pick = |before, now| now.retreats > before.retreats;
    let cases: Vec<(&str, Pick)> = vec![("a raid out", raid_out), ("a retreat", after_retreat)];
    let games: Vec<Box<dyn FnOnce() -> (String, u32) + Send>> = cases
        .into_iter()
        .map(|(what, pick)| {
            Box::new(move || {
                let r = common::rules();
                let setup = MatchSetup::bots(&r, 2, ("standard", "rush"), ("easy", "tower"));
                let fresh = || {
                    Sim::new(
                        setup.clone(),
                        r.clone(),
                        ai::controller_for(&setup, &r).unwrap(),
                    )
                };
                let mut a = fresh();
                let mut before = [BotMemory::default(), BotMemory::default()];
                let mut found = None;
                while a.result().is_none() && a.tick() < 40_000 {
                    a.step(&[]);
                    let view = a.view();
                    for p in [P0, P1] {
                        let now = BotMemory::decode(view.ai_memory(p));
                        if pick(&before[usize::from(p.0)], &now) {
                            found = Some(a.tick());
                        }
                        before[usize::from(p.0)] = now;
                    }
                    if found.is_some() {
                        break;
                    }
                }
                let at = found.unwrap_or_else(|| panic!("no {what} by {}", a.tick()));
                let mut b = fresh();
                b.restore(&a.snapshot()).unwrap();
                for _ in 0..2000 {
                    if a.result().is_some() {
                        break;
                    }
                    a.step(&[]);
                    b.step(&[]);
                    assert_eq!(
                        a.sub_hashes().first_difference(&b.sub_hashes()),
                        None,
                        "{what}: restored at {at}, tick {}",
                        a.tick()
                    );
                }
                (what.to_string(), at)
            }) as Box<_>
        })
        .collect();
    for (what, at) in par(games) {
        println!("m6 restore: {what} at {at}");
    }
}
