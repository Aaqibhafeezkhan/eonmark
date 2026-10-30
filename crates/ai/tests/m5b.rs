//! M5b bot tests that run whole matches (`cargo test -p ai`). The costly
//! ones (several games, or games longer than 5000 ticks) are `#[ignore]`d
//! with an `m5b_` prefix and run in CI with
//! `cargo test -p ai --locked -- --ignored m5b_`. One planned behaviour
//! test stays in the default run although its game is longer:
//! `no_unit_gets_the_same_order_twice_in_three_plans` (11000 ticks, about
//! 0.4 s).

mod common;

use ai::harness::{BotMatch, Side};
use ai::standard::{BotMemory, phase};
use common::{Arena, P0, P1};
use rules::{Rules, UnitClass};
use sim::{
    AiController, AiMemory, BuildingId, Command, FxVec2, MatchSetup, PlayerId, Sim, SimEvent,
    SimView, TargetId, UnitId,
};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// A controller that logs, per call, the player, the tick and how many
/// commands the bot inside it returned.
struct Ticks {
    inner: Box<dyn AiController>,
    calls: Arc<Mutex<Vec<(PlayerId, u32, usize)>>>,
}

impl AiController for Ticks {
    fn think(&mut self, p: PlayerId, view: &SimView<'_>, memory: &mut AiMemory) -> Vec<Command> {
        let out = self.inner.think(p, view, memory);
        self.calls.lock().unwrap().push((p, view.tick, out.len()));
        out
    }
}

/// The Standard bot plans every 10 ticks, the two seats 5 ticks apart
/// (M6: at seed 1, an odd seed, P1 on ticks 0, 10, ... and P0 on ticks 5,
/// 15, ...; the order alternates with the seed's parity): commands come
/// only from plan ticks, both seats act from their first plan (a Yeoman,
/// builders, gatherers), and a plan of the arena starts from the
/// documented context.
#[test]
fn standard_keeps_its_cadence() {
    let r = common::rules();
    let setup = MatchSetup::bots(&r, 1, ("standard", "standard"), ("standard", "standard"));
    let every = r.ai.personality("standard").unwrap().think_every_ticks(&r);
    assert_eq!(every, 10);
    assert_eq!(
        (
            phase(PlayerId(0), every, &setup),
            phase(PlayerId(1), every, &setup)
        ),
        (5, 0)
    );
    let calls = Arc::new(Mutex::new(Vec::new()));
    let inner = ai::controller_for(&setup, &r).unwrap();
    let mut m = BotMatch::new(
        setup.clone(),
        r.clone(),
        Box::new(Ticks {
            inner,
            calls: calls.clone(),
        }),
    );
    for _ in 0..40 {
        m.step();
    }
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 80, "the sim calls both seats every tick");
    for (p, tick, n) in calls.iter() {
        if *n > 0 {
            assert_eq!(*tick % every, phase(*p, every, &setup), "{p:?} at {tick}");
        }
    }
    for p in [PlayerId(0), PlayerId(1)] {
        assert!(
            calls
                .iter()
                .any(|(q, t, n)| *q == p && *t == phase(p, every, &setup) && *n > 0),
            "{p:?} acts at its first plan"
        );
    }
    assert!(m.sim.view().commands_applied() > 0);
    let arena = Arena::new(setup, Vec::new());
    let ctx = arena.ctx(PlayerId(0));
    assert_eq!(ctx.facts.workers.len(), 5, "the standard start's Yeomen");
    assert_eq!(ctx.facts.towns.len(), 1, "the Seat");
    assert_eq!(ctx.facts.enemy_towns.len(), 1);
    assert_eq!(ctx.ledger.stock, ctx.facts.stock);
    assert_eq!(ctx.commands_left, 16);
    assert_eq!(ctx.probes_left, 4);
    let (cmds, _) = arena.plan(PlayerId(1));
    assert!(!cmds.is_empty() && cmds.len() <= 16);
}

/// A Standard-vs-Standard match is the same match on two threads, and a
/// header naming the Standard bot rebuilds it (`controller_for`) so a
/// fresh run reproduces the hash.
#[test]
fn standard_vs_standard_is_deterministic_across_threads() {
    let r = common::rules();
    let setup = MatchSetup::bots(&r, 3, ("standard", "standard"), ("standard", "standard"));
    let run = || {
        let mut m = BotMatch::with_bots(setup.clone(), r.clone()).unwrap();
        m.run(600);
        m.sim.hash()
    };
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(run);
        let b = s.spawn(run);
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_eq!(a, b);
    assert_eq!(a, run());
}

/// Every plan's commands, as `(tick, player, commands)`.
type Log = Arc<Mutex<Vec<(u32, PlayerId, Vec<Command>)>>>;

/// A controller that logs every non-empty batch of the controller inside
/// it, and on `raid_at` makes `raider` debug-spawn `raid` (with
/// `debug_commands` on).
struct Logged {
    inner: Box<dyn AiController>,
    log: Log,
    raid: Option<(u32, PlayerId, Command)>,
}

impl AiController for Logged {
    fn think(&mut self, p: PlayerId, view: &SimView<'_>, memory: &mut AiMemory) -> Vec<Command> {
        let mut out = self.inner.think(p, view, memory);
        if !out.is_empty() {
            self.log.lock().unwrap().push((view.tick, p, out.clone()));
        }
        if let Some((at, raider, cmd)) = &self.raid
            && view.tick == *at
            && p == *raider
        {
            out.push(cmd.clone());
        }
        out
    }
}

fn logged(
    setup: &MatchSetup,
    r: &Rules,
    raid: Option<(u32, PlayerId, Command)>,
) -> (BotMatch, Log) {
    let log: Log = Arc::default();
    let inner = ai::controller_for(setup, r).unwrap();
    let m = BotMatch::new(
        setup.clone(),
        r.clone(),
        Box::new(Logged {
            inner,
            log: log.clone(),
            raid,
        }),
    );
    (m, log)
}

/// What an order asks of one unit, comparable across plans.
fn unit_orders(cmd: &Command) -> Vec<(UnitId, String)> {
    let each = |units: &[UnitId], what: String| {
        units.iter().map(|u| (*u, what.clone())).collect::<Vec<_>>()
    };
    match cmd {
        Command::Move { units, target, .. } => each(units, format!("move {target:?}")),
        Command::AttackMove { units, target } => each(units, format!("amove {target:?}")),
        Command::Attack { units, target } => each(units, format!("attack {target:?}")),
        Command::Gather { units, node } => each(units, format!("gather {node:?}")),
        Command::Build { unit, kind, tile } => vec![(*unit, format!("build {kind:?} {tile:?}"))],
        _ => Vec::new(),
    }
}

/// No unit gets the same order again within three plans of its seat
/// (Critique A #3: the on-station rule and the claims stop re-issue
/// storms), through a raid that engages the defence: Standard (P0)
/// against Passive (P1), which debug-spawns three Skirmishers 12 tiles
/// north of P0's Seat at tick 9000, when P0 has an army.
#[test]
fn no_unit_gets_the_same_order_twice_in_three_plans() {
    let r = common::rules();
    let setup = common::standard_vs_passive(&r);
    let raid_at = 9000;
    let raid = common::spawn(&r, P1, "skirmisher", 24, 52, 3);
    let (mut m, log) = logged(&setup, &r, Some((raid_at, P1, raid)));
    m.run(11_000);
    let every = r.ai.personality("standard").unwrap().think_every_ticks(&r);
    let log = log.lock().unwrap();
    let mut last: BTreeMap<(UnitId, String), u32> = BTreeMap::new();
    let mut orders = 0;
    for (tick, p, cmds) in log.iter() {
        assert_eq!(*p, P0, "only the Standard seat plans");
        for c in cmds {
            for key in unit_orders(c) {
                orders += 1;
                if let Some(prev) = last.get(&key) {
                    assert!(
                        tick - prev >= 3 * every,
                        "{key:?} at {prev} and again at {tick}"
                    );
                }
                last.insert(key, *tick);
            }
        }
    }
    assert!(orders > 25, "{orders} unit orders");
    assert!(
        log.iter()
            .any(|(t, _, c)| *t > raid_at
                && c.iter().any(|c| matches!(c, Command::AttackMove { .. }))),
        "the raid was answered"
    );
}

/// A Standard bot built afresh mid-match (`controller_for` on the
/// header, at tick 1500) issues exactly the commands the running one
/// issues, and leaves the same memory, on every tick of both seats for
/// the next 3000 ticks: the bot holds nothing but its hashed memory and
/// tables computed from immutable inputs. M6: also for a Hard seat against
/// an Easy one, whose knobs `rules::ai::resolve` derives from the header
/// and whose disclosed bonus is paid from tick 4000 (Hard) and 4200
/// (Easy), inside the compared span.
#[test]
fn controller_rebuilt_mid_match_issues_the_same_commands() {
    let r = common::rules();
    for seats in [
        (("standard", "standard"), ("standard", "standard")),
        (("hard", "standard"), ("easy", "standard")),
    ] {
        let setup = MatchSetup::bots(&r, 2, seats.0, seats.1);
        rebuilt_bot_issues_the_same_commands(&setup, &r);
    }
}

/// [`controller_rebuilt_mid_match_issues_the_same_commands`] for one
/// header.
fn rebuilt_bot_issues_the_same_commands(setup: &MatchSetup, r: &Rules) {
    let (mut m, log) = logged(setup, r, None);
    while m.sim.tick() < 1500 {
        m.step();
    }
    let mut fresh = ai::controller_for(setup, r).unwrap();
    let mut compared = 0;
    while m.sim.tick() < 4500 {
        let at = m.sim.tick();
        let view = m.sim.view();
        let mut expected = Vec::new();
        for p in [P0, P1] {
            let mut memory = AiMemory::default();
            memory.set(view.ai_memory(p).to_vec());
            let cmds = fresh.think(p, &view, &mut memory);
            expected.push((p, cmds, memory));
        }
        m.step();
        let log = log.lock().unwrap();
        for (p, cmds, memory) in expected {
            let issued: Vec<Command> = log
                .iter()
                .filter(|(t, q, _)| *t == at && *q == p)
                .flat_map(|(_, _, c)| c.clone())
                .collect();
            assert_eq!(cmds, issued, "{p:?} at {at}");
            assert_eq!(memory.bytes(), m.sim.view().ai_memory(p), "{p:?} at {at}");
            compared += usize::from(!cmds.is_empty());
        }
    }
    assert!(
        compared >= 20,
        "{:?}: {compared} non-empty plans compared",
        setup.ai_seats
    );
}

/// `ai::describe` names the window, the latches and the wave.
#[test]
fn describe_reports_the_window_and_latches() {
    let r = common::rules();
    let setup = MatchSetup::bots(&r, 1, ("standard", "standard"), ("passive", ""));
    let mut m = BotMatch::with_bots(setup, r).unwrap();
    m.run(20);
    let view = m.sim.view();
    let line = ai::describe(&view, P0);
    assert!(
        line.starts_with("standard standard window=[0 workers 7, "),
        "{line}"
    );
    assert!(
        line.ends_with("military=no wave=none next_wave=0 waves=0 defend=none defends=0"),
        "{line}"
    );
    assert_eq!(ai::describe(&view, P1), "passive");
    let human = MatchSetup::skirmish(m.sim.rules(), 1);
    let sim = Sim::new(human, m.sim.rules().clone(), Box::new(sim::NoAi));
    assert_eq!(ai::describe(&sim.view(), P0), "human");
}

/// Runs `games` in parallel, one thread each.
fn par<T: Send>(games: Vec<Box<dyn FnOnce() -> T + Send>>) -> Vec<T> {
    std::thread::scope(|s| {
        let handles: Vec<_> = games.into_iter().map(|g| s.spawn(g)).collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

/// A Standard-against-Passive game's end: the Standard side's observed
/// facts, the tick reached and whether the match was decided.
struct VsPassive {
    side: Side,
    tick: u32,
    decided: bool,
}

/// Standard against Passive under rules `r`, with the Standard bot in
/// `seat`, to `ticks` or the outcome.
fn vs_passive_under(r: Rules, seed: u64, seat: PlayerId, ticks: u32) -> VsPassive {
    let (a, b) = if seat == P0 {
        (("standard", "standard"), ("passive", ""))
    } else {
        (("passive", ""), ("standard", "standard"))
    };
    let setup = MatchSetup::bots(&r, seed, a, b);
    let mut m = BotMatch::with_bots(setup, r).unwrap();
    m.run(ticks);
    VsPassive {
        side: m.observer.sides()[usize::from(seat.0)],
        tick: m.sim.tick(),
        decided: m.sim.result().is_some(),
    }
}

/// The rules with every wave held back past the match cap
/// (`first_wave_not_before_ds` at its maximum). The first wave takes
/// Passive's Seat around tick 15600, the match is then decided and the
/// bots stop; held back, a game against Passive runs its whole length
/// undecided while the army is still trained and paid for as in a real
/// game, so the build order competes for the same stock. (The all-in, at
/// `all_in_after_ds` = tick 36000, would launch anyway; no game here runs
/// past 36000.)
fn waves_held_back() -> Rules {
    let mut r = common::rules();
    for p in &mut r.ai.personalities {
        p.first_wave_not_before_ds = rules::ai::MAX_AI_DS;
    }
    assert!(r.ticks_from_ds(rules::ai::MAX_AI_DS) > r.match_cap_ticks());
    r
}

/// The build order never stalls: every non-optional step is done before
/// tick 30000 in five seeds, from either seat, in games that run the whole
/// 30000 ticks undecided with the waves held back ([`waves_held_back`]).
#[test]
#[ignore = "long: five 30000-tick games a side; CI runs --ignored m5b_"]
fn m5b_build_order_completes_vs_passive_by_30000() {
    let r = waves_held_back();
    for seat in [P0, P1] {
        let games: Vec<Box<dyn FnOnce() -> VsPassive + Send>> = (1..=5)
            .map(|seed| {
                let r = r.clone();
                Box::new(move || vs_passive_under(r, seed, seat, 30_000)) as Box<_>
            })
            .collect();
        for (i, game) in par(games).into_iter().enumerate() {
            assert!(
                !game.decided && game.tick == 30_000,
                "{seat:?} seed {}: undecided at 30000, not {} ({})",
                i + 1,
                game.tick,
                game.decided
            );
            let done = game.side.bo_done_tick;
            assert!(
                done.is_some_and(|t| t < 30_000),
                "{seat:?} seed {}: {done:?}",
                i + 1
            );
        }
    }
}

/// The bot checks its commands before it issues them: fewer than 20 are
/// rejected in 36000 ticks against Passive, from either seat. With waves
/// the first one takes Passive's Seat around tick 15600 and the bots stop
/// there, so each seat plays two games: one with the waves held back
/// ([`waves_held_back`]), which runs the whole 36000 ticks undecided
/// (asserted) and covers every build-order step, and one with the waves,
/// which covers their commands up to the capture.
#[test]
#[ignore = "long: four games, two of them 36000 ticks; CI runs --ignored m5b_"]
fn m5b_rejections_stay_under_20_per_36000_ticks_vs_passive() {
    let held = waves_held_back();
    let cases = [(P0, true), (P1, true), (P0, false), (P1, false)];
    let games: Vec<Box<dyn FnOnce() -> VsPassive + Send>> = cases
        .into_iter()
        .map(|(seat, held_back)| {
            let r = if held_back {
                held.clone()
            } else {
                common::rules()
            };
            Box::new(move || vs_passive_under(r, 1, seat, 36_000)) as Box<_>
        })
        .collect();
    for ((seat, held_back), game) in cases.into_iter().zip(par(games)) {
        if held_back {
            assert!(
                !game.decided && game.tick == 36_000,
                "{seat:?}, waves held back: undecided at 36000, not {} ({})",
                game.tick,
                game.decided
            );
        }
        assert!(
            game.side.rejected < 20,
            "{seat:?}, waves held back {held_back}: {} rejected by {}",
            game.side.rejected,
            game.tick
        );
    }
}

/// Own armed units of `me`: non-civilian kinds with an attack.
fn armed(r: &Rules, view: &SimView<'_>, me: PlayerId) -> usize {
    view.units()
        .filter(|u| u.owner == me)
        .filter(|u| {
            r.unit_kind(u.kind.0)
                .is_some_and(|k| k.class != UnitClass::Civilian && k.attack.is_some())
        })
        .count()
}

/// Steps `m` to tick `until`, checking after every step from Masonry on
/// that a full pop never comes with no armed unit at home (the units of
/// the wave in memory are away). Returns the ticks with a full pop.
fn full_pop_ticks(r: &Rules, m: &mut Sim, until: u32) -> u32 {
    let masonry = r.age_index("masonry").unwrap();
    let mut full = 0;
    while m.tick() < until {
        m.step(&[]);
        let view = m.view();
        if view.age(P0) >= masonry {
            let (used, cap) = view.pop(P0);
            let mem = BotMemory::decode(view.ai_memory(P0));
            let away = mem.wave.map_or(0, |w| w.units.len());
            assert!(
                used < cap || armed(r, &view, P0) > away,
                "tick {}: pop {used}/{cap} and no armed unit at home",
                view.tick
            );
            full += u32::from(used >= cap);
        }
    }
    full
}

/// The pop cap never locks the army out: at least 8 armed units by tick
/// 18000, and after Masonry no tick sees a full pop with no armed unit at
/// home. The natural game (Standard P0 against Passive) never fills its
/// pop before 18000 (52/75 at most), so a second game fills it: as soon
/// as P0 reaches Masonry, P0 is given Yeomen up to two below its cap; the
/// full-pop branch must then be met at least once, and the bot must still
/// field at least 8 armed units by 18000. Until the waves of
/// M5b slice 3 exist no unit is ever away, so the check reads "a full pop
/// holds at least one armed unit"; slice 3 gives the wave a roster here.
#[test]
#[ignore = "long: two 18000-tick games; CI runs --ignored m5b_"]
fn m5b_pop_never_deadlocks() {
    let games: Vec<Box<dyn FnOnce() -> (u32, usize) + Send>> = [false, true]
        .into_iter()
        .map(|fill| {
            Box::new(move || {
                let r = common::rules();
                let mut a = Arena::new(common::standard_vs_passive(&r), Vec::new());
                if fill {
                    let masonry = r.age_index("masonry").unwrap();
                    while a.sim.view().age(P0) < masonry {
                        a.run(1);
                    }
                    let (used, cap) = a.sim.view().pop(P0);
                    let n = u16::try_from(cap.saturating_sub(used + 2)).unwrap();
                    assert!(n > 0, "pop {used}/{cap} at Masonry");
                    a.issue(P0, vec![common::spawn(&r, P0, "yeoman", 20, 70, n)]);
                }
                let full = full_pop_ticks(&r, &mut a.sim, 18_000);
                (full, armed(&r, &a.sim.view(), P0))
            }) as Box<_>
        })
        .collect();
    let [(_, natural), (full, filled)] = par(games)[..] else {
        unreachable!()
    };
    assert!(natural >= 8, "{natural} armed units at 18000");
    assert!(full > 0, "the filled game never had a full pop");
    assert!(
        filled >= 8,
        "{filled} armed units at 18000 in the filled game"
    );
}

/// The bot never walls itself in: after 20000 ticks every one of its
/// buildings has a ring tile in the walkable component of the Seat's
/// ring. The waves are held back ([`waves_held_back`]), so the game is
/// still undecided at 20000 (asserted) and the build order has placed its
/// later buildings, the second Muster Hall among them.
#[test]
#[ignore = "long: a 20000-tick game; CI runs --ignored m5b_"]
fn m5b_sites_keep_connectivity() {
    let r = waves_held_back();
    let setup = common::standard_vs_passive(&r);
    let mut m = BotMatch::with_bots(setup, r.clone()).unwrap();
    m.run(20_000);
    assert!(
        m.sim.result().is_none() && m.sim.tick() == 20_000,
        "undecided at 20000, not {} ({:?})",
        m.sim.tick(),
        m.sim.result()
    );
    let view = m.sim.view();
    let muster = common::building_kind(&r, "muster_hall");
    assert!(
        view.buildings()
            .filter(|b| b.owner == P0 && b.kind == muster)
            .count()
            >= 2,
        "the second Muster Hall (a NearTown site of slice 3's steps) is placed"
    );
    let map = view.map();
    let ring = |b: &sim::Building| {
        let d = r.building_kind(b.kind.0).unwrap();
        let (x0, y0) = (i32::from(b.tile.x) - 1, i32::from(b.tile.y) - 1);
        let (w, h) = (
            i32::try_from(d.width_tiles).unwrap(),
            i32::try_from(d.height_tiles).unwrap(),
        );
        let mut out = Vec::new();
        for y in y0..=y0 + h + 1 {
            for x in x0..=x0 + w + 1 {
                let edge = x == x0 || y == y0 || x == x0 + w + 1 || y == y0 + h + 1;
                if edge
                    && let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y))
                    && map.passable(sim::Tile::new(x, y))
                {
                    out.push(map.component_of(sim::Tile::new(x, y)));
                }
            }
        }
        out
    };
    let seat = view
        .buildings()
        .find(|b| b.owner == P0 && b.town_state.is_some_and(|t| t.seat))
        .unwrap();
    let home = ring(seat);
    assert!(!home.is_empty());
    let mut checked = 0;
    for b in view.buildings().filter(|b| b.owner == P0) {
        let comps = ring(b);
        assert!(
            comps.iter().any(|c| home.contains(c)),
            "{} at {:?} is walled off",
            r.building_kind(b.kind.0).unwrap().id,
            b.tile
        );
        checked += 1;
    }
    assert!(checked >= 15, "{checked} buildings");
}

/// Whether `cmd` trains a non-civilian unit.
fn trains_army(r: &Rules, cmd: &Command) -> bool {
    matches!(cmd, Command::Train { kind, .. } if r.unit_kind(kind.0).is_some_and(|k| k.class != UnitClass::Civilian))
}

/// The bot answers a raid at minute 5: Passive debug-spawns six
/// Skirmishers 10 tiles north of the Standard bot's Seat (P1) at tick
/// 6000. In this game P1's `Military` step is already latched (tick 4906,
/// its Muster Hall complete at 5729), so production is running before the
/// raid; emergency production before the latch is covered by
/// `standard::defend::tests::emergency_production_before_the_military_latch`.
/// Within 200 ticks the defend latch is set and a guard engages: P1
/// issues an `AttackMove` within `defend_regoal_tiles` of a living raider
/// (or an `Attack` on one). Army production goes on through the raid; the
/// bot then kills the raiders, releases the latch, and keeps every Town.
#[test]
#[ignore = "long: a 12000-tick game; CI runs --ignored m5b_"]
fn m5b_defend_answers_a_raid_at_minute_5() {
    let r = common::rules();
    let mut setup = MatchSetup::bots(&r, 1, ("passive", ""), ("standard", "standard"));
    setup.debug_commands = true;
    let raid_at = 6000;
    let raid = common::spawn(&r, P0, "skirmisher", 103, 54, 6);
    let (mut m, log) = logged(&setup, &r, Some((raid_at, P0, raid)));
    while m.sim.tick() < raid_at {
        m.step();
    }
    assert!(
        BotMemory::decode(m.sim.view().ai_memory(P1)).military,
        "the setup's premise: P1's Military step is latched before the raid"
    );
    while m.sim.tick() < raid_at + 3 {
        m.step();
    }
    let raiders: Vec<UnitId> = m
        .sim
        .view()
        .units()
        .filter(|u| u.owner == P0 && u.kind == common::unit_kind(&r, "skirmisher"))
        .map(|u| u.id)
        .collect();
    assert_eq!(raiders.len(), 6);
    let towns_before: Vec<BuildingId> = m
        .sim
        .view()
        .buildings()
        .filter(|b| b.owner == P1 && b.town_state.is_some())
        .map(|b| b.id)
        .collect();
    // Where the living raiders stand at each tick a plan may read.
    let mut seen: BTreeMap<u32, Vec<FxVec2>> = BTreeMap::new();
    while m.sim.tick() < raid_at + 200 {
        let view = m.sim.view();
        seen.insert(
            view.tick,
            raiders
                .iter()
                .filter_map(|u| view.unit(*u).map(|u| u.pos))
                .collect(),
        );
        m.step();
    }
    let mem = BotMemory::decode(m.sim.view().ai_memory(P1));
    assert!(mem.defend.is_some(), "the defend latch is set");
    {
        let regoal = r.ai.personality("standard").unwrap().defend_regoal_tiles;
        let r2 = ai::standard::facts::tiles_sq(regoal);
        let log = log.lock().unwrap();
        let engaged = log.iter().any(|(t, p, cmds)| {
            *p == P1
                && *t > raid_at
                && cmds.iter().any(|c| match c {
                    Command::AttackMove { target, .. } => seen
                        .get(t)
                        .is_some_and(|at| at.iter().any(|x| x.dist_sq_i64(*target) <= r2)),
                    Command::Attack {
                        target: TargetId::Unit(u),
                        ..
                    } => raiders.contains(u),
                    _ => false,
                })
        });
        assert!(engaged, "a guard engages a raider within 200 ticks");
        let trained = log.iter().any(|(t, p, cmds)| {
            *p == P1 && *t > raid_at && cmds.iter().any(|c| trains_army(&r, c))
        });
        assert!(trained, "army production goes on through the raid");
    }
    let mut cleared = None;
    while m.sim.tick() < 12_000 {
        m.step();
        let view = m.sim.view();
        if cleared.is_none() && raiders.iter().all(|u| view.unit(*u).is_none()) {
            cleared = Some(view.tick);
        }
    }
    let view = m.sim.view();
    let cleared = cleared.expect("the raiders die");
    let mem = BotMemory::decode(view.ai_memory(P1));
    assert!(
        mem.defend.is_none() && mem.defends >= 1,
        "{mem:?}, cleared at {cleared}"
    );
    for t in towns_before {
        assert_eq!(view.building(t).map(|b| b.owner), Some(P1), "no flip");
    }
    let _ = SimEvent::TerritoryChanged;
}

/// A defence outranks saving for an age: Standard (P0) against Passive
/// (P1), which debug-spawns three Skirmishers 13 tiles north of P0's Seat
/// at tick 6700. P0's Muster Hall is up and idle and its `Military` step
/// latched, but the Masonry `Advance` in its window is saving up its 250
/// Grain and 100 Lore: its techs are researched and the stock is short
/// (from tick 6412 to 7971 in this game, the Muster Hall idle all along).
/// While the bot defends, that saving no longer holds the stock back from
/// production, so P0 queues an armed unit within 200 ticks of the raid.
/// M6: the game runs on a copy of the rules whose `standard` level pays no
/// bonus (`income_interval_ds` 0), so it keeps the M5b timeline above; the
/// Standard level's disclosed bonus from tick 4100 would fill the stock
/// and keep the Muster Hall busy before the raid.
#[test]
#[ignore = "long: a 6900-tick game; CI runs --ignored m5b_"]
fn m5b_defend_outranks_saving_for_an_age() {
    let mut r = common::rules();
    r.ai.difficulties
        .get_mut("standard")
        .expect("the standard level")
        .income_interval_ds = 0;
    let setup = common::standard_vs_passive(&r);
    assert!(sim::bonus_table(&setup, &r).is_empty(), "no bonus is paid");
    let raid_at = 6700;
    let raid = common::spawn(&r, P1, "skirmisher", 24, 51, 3);
    let (mut m, log) = logged(&setup, &r, Some((raid_at, P1, raid)));
    while m.sim.tick() < raid_at {
        m.step();
    }
    let view = m.sim.view();
    let mem = BotMemory::decode(view.ai_memory(P0));
    let line = ai::describe(&view, P0);
    assert!(
        mem.military && line.contains("advance masonry"),
        "the setup's premise: production latched, Masonry saved for: {line}"
    );
    assert!(
        matches!(
            view.check_advance_age(P0),
            Err(sim::RejectReason::CannotAfford { .. })
        ),
        "the setup's premise: Masonry's techs are done and its cost is short"
    );
    let muster = common::building_kind(&r, "muster_hall");
    assert!(
        view.buildings()
            .any(|b| b.owner == P0 && b.kind == muster && b.queue.is_empty()),
        "the setup's premise: an idle Muster Hall"
    );
    while m.sim.tick() <= raid_at + 200 {
        m.step();
    }
    assert!(
        BotMemory::decode(m.sim.view().ai_memory(P0))
            .defend
            .is_some(),
        "the defend latch is set"
    );
    let log = log.lock().unwrap();
    let first = log
        .iter()
        .filter(|(t, p, _)| *p == P0 && *t > raid_at)
        .find(|(_, _, cmds)| cmds.iter().any(|c| trains_army(&r, c)))
        .map(|(t, _, _)| *t);
    assert!(
        first.is_some_and(|t| t <= raid_at + 200),
        "the first armed unit queued after the raid at {raid_at}: {first:?}"
    );
}

/// Hidden state: restoring a Standard-vs-Standard match at tick 3003 or
/// 3007 (one in each seat's planning phase) into a fresh `Sim` with
/// fresh bots from the header matches the uninterrupted run, every
/// sub-hash, every tick, for the next 3000 ticks. M6: also a Hard seat
/// against an Easy one restored at 3003, so the disclosed bonus schedule
/// (rebuilt from the header) pays on the same ticks after the restore.
#[test]
#[ignore = "long: 6000-tick games; CI runs --ignored m5b_"]
fn m5b_restore_at_3003_and_3007_matches_uninterrupted() {
    let r = common::rules();
    let setup = MatchSetup::bots(&r, 5, ("standard", "standard"), ("standard", "standard"));
    let levels = MatchSetup::bots(&r, 5, ("hard", "standard"), ("easy", "standard"));
    let games: Vec<Box<dyn FnOnce() -> u32 + Send>> =
        [(&setup, 3003u32), (&setup, 3007), (&levels, 3003)]
            .into_iter()
            .map(|(setup, at)| {
                let (setup, r) = (setup.clone(), r.clone());
                Box::new(move || {
                    let fresh = || {
                        Sim::new(
                            setup.clone(),
                            r.clone(),
                            ai::controller_for(&setup, &r).unwrap(),
                        )
                    };
                    let mut a = fresh();
                    while a.tick() < at {
                        a.step(&[]);
                    }
                    let mut b = fresh();
                    b.restore(&a.snapshot()).unwrap();
                    for _ in 0..3000 {
                        a.step(&[]);
                        b.step(&[]);
                        let (ha, hb) = (a.sub_hashes(), b.sub_hashes());
                        assert_eq!(
                            ha.first_difference(&hb),
                            None,
                            "restored at {at}, tick {}",
                            a.tick()
                        );
                    }
                    assert!(
                        !a.view().ai_memory(P0).is_empty() && !a.view().ai_memory(P1).is_empty()
                    );
                    a.tick()
                }) as Box<_>
            })
            .collect();
    assert_eq!(par(games), [6003, 6007, 6003]);
}

// ---------------------------------------------------------------------------
// M5b slice 3: waves, the capture, the acceptance-2 facts.
// ---------------------------------------------------------------------------

/// Player `p`'s non-civilian units committed to `enemy`'s land at this
/// tick (order goal, post goal, attack target or position on it, as the
/// play-bots observer counts them): `(armed, wains, mangonels)`.
fn committed_now(r: &Rules, view: &SimView<'_>, p: PlayerId, enemy: PlayerId) -> (u32, u32, u32) {
    let on = |at: FxVec2| view.territory().owner_at(view.map().tile_of(at)) == Some(enemy);
    let (mut armed, mut wains, mut mangonels) = (0, 0, 0);
    for u in view.units().filter(|u| u.owner == p) {
        let k = r.unit_kind(u.kind.0).unwrap();
        if k.class == UnitClass::Civilian {
            continue;
        }
        let target_at = u.target.and_then(|t| match t {
            TargetId::Unit(id) => view.unit(id).map(|t| t.pos),
            TargetId::Building(id) => view.building(id).map(|b| view.map().center_of(b.tile)),
        });
        let committed = [
            u.order.as_ref().map(|o| o.goal),
            u.post.as_ref().map(|p| p.goal),
            target_at,
            Some(u.pos),
        ]
        .into_iter()
        .flatten()
        .any(on);
        if !committed {
            continue;
        }
        if k.supplies {
            wains += 1;
        } else if k.attack.is_some() {
            armed += 1;
            if k.class == UnitClass::Siege {
                mangonels += 1;
            }
        }
    }
    (armed, wains, mangonels)
}

/// Acceptance 2 (ROADMAP M5b): against Passive, the Standard bot (P0)
/// reaches Masonry and then, before tick 18000, attack-moves into the
/// enemy's land with at least 8 armed units, exactly one Supply Wain and
/// at least one Mangonel, all committed in the same tick (re-checked here
/// from the view at the observer's `first_attack_tick`), in five seeds.
/// The five seeds play close to one trajectory: the RNG is drawn only for
/// exact combat ties.
#[test]
#[ignore = "long: five games to about 15600 ticks; CI runs --ignored m5b_"]
fn m5b_standard_vs_passive_reaches_masonry_then_a_wave_of_8_with_a_wain_before_18000() {
    let games: Vec<Box<dyn FnOnce() -> (Side, (u32, u32, u32)) + Send>> = (1..=5u64)
        .map(|seed| {
            Box::new(move || {
                let r = common::rules();
                let setup = MatchSetup::bots(&r, seed, ("standard", "standard"), ("passive", ""));
                let mut m = BotMatch::with_bots(setup, r.clone()).unwrap();
                while m.sim.result().is_none() && m.sim.tick() < 18_000 {
                    m.step();
                    if m.observer.sides()[0].first_attack_tick == Some(m.sim.tick()) {
                        let seen = committed_now(&r, &m.sim.view(), P0, P1);
                        return (m.observer.sides()[0], seen);
                    }
                }
                (m.observer.sides()[0], (0, 0, 0))
            }) as Box<_>
        })
        .collect();
    for (i, (side, (armed, wains, mangonels))) in par(games).into_iter().enumerate() {
        let seed = i + 1;
        let age = side.age_reached_tick.expect("Masonry");
        let attack = side.first_attack_tick.expect("an attack before 18000");
        assert!(
            age <= attack && attack < 18_000,
            "seed {seed}: {age} {attack}"
        );
        assert!(armed >= 8, "seed {seed}: {armed} armed units");
        assert_eq!(wains, 1, "seed {seed}: one Supply Wain");
        assert!(mangonels >= 1, "seed {seed}: no Mangonel");
        assert_eq!(side.first_attack_units, Some(armed + wains), "seed {seed}");
        assert_eq!(side.first_attack_wains, Some(1), "seed {seed}");
    }
}

/// The capture: against Passive the first wave drops the Seat and holds
/// its ring, so the annexation started by P0 runs to the flip with no
/// reset, and the match is a decisive `SeatCaptured` victory for P0.
#[test]
#[ignore = "long: a game to about 15600 ticks; CI runs --ignored m5b_"]
fn m5b_standard_vs_passive_captures_the_seat_by_ring_hold() {
    let r = common::rules();
    let setup = MatchSetup::bots(&r, 1, ("standard", "standard"), ("passive", ""));
    let mut m = BotMatch::with_bots(setup, r).unwrap();
    let mut started = Vec::new();
    let mut resets = 0;
    while m.sim.result().is_none() && m.sim.tick() < 36_000 {
        m.step_sim();
        for e in m.sim.drain_events() {
            match e {
                SimEvent::AnnexationStarted { town, attacker } => started.push((town, attacker)),
                SimEvent::AnnexationReset { .. } => resets += 1,
                _ => {}
            }
        }
    }
    let seat = m
        .sim
        .view()
        .buildings()
        .find(|b| b.town_state.is_some_and(|t| t.seat) && b.owner == P0 && b.id != BuildingId(1))
        .map(|b| b.id);
    let result = m.sim.result().expect("decided");
    assert_eq!(
        result.outcome,
        sim::Outcome::Victory {
            player: P0,
            decisive: true
        }
    );
    assert_eq!(result.reason, sim::OutcomeReason::SeatCaptured);
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(Some(started[0].0), seat, "the annexed Town is P1's Seat");
    assert_eq!(started[0].1, P0);
    assert_eq!(resets, 0, "no reset");
}

/// The Standard bot takes Passive's Seat from either side, three seeds
/// each: a decisive `SeatCaptured` victory every time.
#[test]
#[ignore = "long: six games to about 15600 ticks; CI runs --ignored m5b_"]
fn m5b_standard_wins_from_either_side_vs_passive() {
    let mut games: Vec<Box<dyn FnOnce() -> (PlayerId, Option<sim::MatchResult>) + Send>> =
        Vec::new();
    for seat in [P0, P1] {
        for seed in 1..=3u64 {
            games.push(Box::new(move || {
                let r = common::rules();
                let (a, b) = if seat == P0 {
                    (("standard", "standard"), ("passive", ""))
                } else {
                    (("passive", ""), ("standard", "standard"))
                };
                let mut m = BotMatch::with_bots(MatchSetup::bots(&r, seed, a, b), r).unwrap();
                m.run(36_000);
                (seat, m.sim.result())
            }));
        }
    }
    for (seat, result) in par(games) {
        let result = result.expect("decided before 36000");
        assert_eq!(
            (result.outcome, result.reason),
            (
                sim::Outcome::Victory {
                    player: seat,
                    decisive: true
                },
                sim::OutcomeReason::SeatCaptured
            ),
            "{seat:?}"
        );
    }
}

/// A controller that plans with the Standard bot of each seat and keeps
/// every plan's command count and `check_build` probes.
struct Stats {
    bots: BTreeMap<PlayerId, ai::standard::StandardBot>,
    plans: Arc<Mutex<Vec<(PlayerId, u32, usize, u32)>>>,
}

impl AiController for Stats {
    fn think(&mut self, p: PlayerId, view: &SimView<'_>, memory: &mut AiMemory) -> Vec<Command> {
        let Some(bot) = self.bots.get(&p) else {
            return Vec::new();
        };
        let (cmds, stats) = bot.think_with_stats(p, view, memory);
        if let Some(s) = stats {
            self.plans
                .lock()
                .unwrap()
                .push((p, view.tick, cmds.len(), s.probes));
        }
        cmds
    }
}

/// The work caps hold on every plan of a Standard-vs-Standard game's first
/// 6000 ticks (the building-heavy opening): at most
/// `max_commands_per_think` commands and `site_checks_per_think`
/// `check_build` probes, each counted where it is made
/// (`Ctx::check_build`, which every probe of the bot goes through:
/// `ctx::tests::every_probe_goes_through_the_counter`).
#[test]
#[ignore = "long: a 6000-tick game; CI runs --ignored m5b_"]
fn m5b_standard_respects_work_caps() {
    let r = common::rules();
    let setup = MatchSetup::bots(&r, 1, ("standard", "standard"), ("standard", "standard"));
    let k = r.ai.personality("standard").unwrap().clone();
    let level = r.ai.difficulty("standard").unwrap().clone();
    let plans = Arc::new(Mutex::new(Vec::new()));
    let bots = [P0, P1]
        .into_iter()
        .map(|p| (p, ai::standard::StandardBot::new(&r, &setup, p, &k, &level)))
        .collect();
    let mut m = BotMatch::new(
        setup,
        r,
        Box::new(Stats {
            bots,
            plans: plans.clone(),
        }),
    );
    m.run(6000);
    let plans = plans.lock().unwrap();
    assert_eq!(plans.len(), 1200, "every 10 ticks, two seats");
    let cmds = usize::try_from(k.max_commands_per_think).unwrap();
    for (p, t, n, probes) in plans.iter() {
        assert!(*n <= cmds, "{p:?} tick {t}: {n} commands");
        assert!(
            *probes <= k.site_checks_per_think,
            "{p:?} tick {t}: {probes} probes"
        );
    }
    assert!(
        plans.iter().any(|(_, _, _, probes)| *probes > 0),
        "sites were probed"
    );
    let busiest = plans.iter().map(|(_, _, n, _)| *n).max().unwrap();
    println!("busiest plan: {busiest} commands");
}

/// The bot's memory stays small: under 1 KiB at every tick of a game
/// against Passive (to the capture), from either side.
#[test]
#[ignore = "long: two games to about 15600 ticks; CI runs --ignored m5b_"]
fn m5b_memory_stays_under_1_kib_vs_passive() {
    let games: Vec<Box<dyn FnOnce() -> usize + Send>> = [P0, P1]
        .into_iter()
        .map(|seat| {
            Box::new(move || {
                let r = common::rules();
                let (a, b) = if seat == P0 {
                    (("standard", "standard"), ("passive", ""))
                } else {
                    (("passive", ""), ("standard", "standard"))
                };
                let mut m = BotMatch::with_bots(MatchSetup::bots(&r, 1, a, b), r).unwrap();
                let mut most = 0;
                while m.sim.result().is_none() && m.sim.tick() < 36_000 {
                    m.step();
                    let n = m.sim.view().ai_memory(seat).len();
                    assert!(n < 1024, "{seat:?} tick {}: {n} bytes", m.sim.tick());
                    most = most.max(n);
                }
                most
            }) as Box<_>
        })
        .collect();
    for most in par(games) {
        assert!(most > 0, "the memory was written");
    }
}

/// Every wave reaches the enemy's land: while a wave is out (in the
/// memory), one of its units stands on the enemy's land within 2400 ticks
/// of its launch, so no wave jams on the way. A wave that ends sooner
/// (recalled after a clash, or destroyed) is no jam. Standard against
/// Passive from either side (each wave must arrive there), and both seats
/// of a Standard-vs-Standard game, to tick 30000.
#[test]
#[ignore = "long: three games to 30000 ticks; CI runs --ignored m5b_"]
fn m5b_waves_reach_enemy_land_within_2400_ticks_of_launch() {
    let configs: [(&str, &str); 3] = [
        ("standard", "passive"),
        ("passive", "standard"),
        ("standard", "standard"),
    ];
    let games: Vec<Box<dyn FnOnce() -> (u32, u32) + Send>> = configs
        .into_iter()
        .map(|(a, b)| {
            Box::new(move || {
                let r = common::rules();
                let pers = |bot: &str| if bot == "standard" { "standard" } else { "" };
                let setup = MatchSetup::bots(&r, 1, (a, pers(a)), (b, pers(b)));
                let mut m = BotMatch::with_bots(setup, r).unwrap();
                // (player, launch tick) of every wave out and not yet seen
                // on enemy land.
                let mut open: Vec<(PlayerId, u32)> = Vec::new();
                let mut launched = [0u16; 2];
                let (mut reached, mut ended) = (0u32, 0u32);
                while m.sim.result().is_none() && m.sim.tick() < 30_000 {
                    m.step();
                    let view = m.sim.view();
                    let mems = [P0, P1].map(|p| BotMemory::decode(view.ai_memory(p)));
                    for (i, p) in [P0, P1].into_iter().enumerate() {
                        if mems[i].waves_launched > launched[i] {
                            launched[i] = mems[i].waves_launched;
                            let w = mems[i].wave.as_ref().expect("a wave launched this plan");
                            open.push((p, w.launched_at));
                        }
                    }
                    open.retain(|(p, at)| {
                        let i = usize::from(p.0);
                        let enemy = if *p == P0 { P1 } else { P0 };
                        let Some(w) = mems[i].wave.as_ref().filter(|w| w.launched_at == *at) else {
                            ended += 1;
                            return false;
                        };
                        let there = w.units.iter().chain(w.wain.iter()).any(|u| {
                            view.unit(*u).is_some_and(|x| {
                                view.territory().owner_at(view.map().tile_of(x.pos)) == Some(enemy)
                            })
                        });
                        if there {
                            reached += 1;
                            return false;
                        }
                        assert!(
                            view.tick < at + 2400,
                            "{p:?}: the wave launched at {at} is not on enemy land at {}",
                            view.tick
                        );
                        true
                    });
                }
                (reached, ended)
            }) as Box<_>
        })
        .collect();
    let seen = par(games);
    for (i, (reached, ended)) in seen.iter().enumerate().take(2) {
        assert!(*reached >= 1 && *ended == 0, "game {i}: {seen:?}");
    }
    println!("(reached, ended early) per game: {seen:?}");
}

/// Hidden state during a wave: a Standard-vs-Passive match restored into a
/// fresh `Sim` with fresh bots 300 ticks after the first launch (the
/// roster, stage point and phase in the memory) matches the uninterrupted
/// run, every sub-hash, every tick, through the assault and the ring hold
/// to the outcome.
#[test]
#[ignore = "long: two games to about 15600 ticks; CI runs --ignored m5b_"]
fn m5b_restore_during_a_wave_matches_uninterrupted() {
    let r = common::rules();
    let setup = MatchSetup::bots(&r, 2, ("standard", "standard"), ("passive", ""));
    let fresh = || {
        Sim::new(
            setup.clone(),
            r.clone(),
            ai::controller_for(&setup, &r).unwrap(),
        )
    };
    let mut a = fresh();
    while BotMemory::decode(a.view().ai_memory(P0)).wave.is_none() {
        a.step(&[]);
        assert!(a.tick() < 18_000, "no wave");
    }
    let at = a.tick() + 300;
    while a.tick() < at {
        a.step(&[]);
    }
    assert!(BotMemory::decode(a.view().ai_memory(P0)).wave.is_some());
    let mut b = fresh();
    b.restore(&a.snapshot()).unwrap();
    while a.result().is_none() {
        a.step(&[]);
        b.step(&[]);
        assert_eq!(
            a.sub_hashes().first_difference(&b.sub_hashes()),
            None,
            "restored at {at}, tick {}",
            a.tick()
        );
        assert!(a.tick() < 36_000, "no outcome");
    }
    assert_eq!(a.result(), b.result());
}

/// The siege path end to end (not ignored): a human P0 whose Yeomen are
/// killed at once, so only the Seat's fire defends, and the Standard bot as
/// P1, moved to Masonry and given 8 Skirmishers, 2 Mangonels and a Supply
/// Wain at its rally. P1 launches at its first plan, stages outside P0's
/// border, drops the Seat with its siege, holds the ring and takes the
/// Seat: a decisive `SeatCaptured` victory well inside 4000 ticks, with no
/// annexation reset and no rejected command.
#[test]
fn bot_takes_seat() {
    let r = common::rules();
    let mut setup = MatchSetup::vs_bot(&r, 1, "standard", "standard");
    setup.debug_commands = true;
    let mut a = Arena::new(setup, Vec::new());
    let mut cmds: Vec<Command> = a
        .units_of(P0, "yeoman")
        .into_iter()
        .map(|u| Command::DebugDamage {
            target: TargetId::Unit(u),
            amount: 1000,
        })
        .collect();
    cmds.push(Command::DebugAdvanceAge { owner: P1 });
    let rally = a.map(P1).rally_seed;
    let (x, y) = (i32::from(rally.x), i32::from(rally.y));
    cmds.push(common::spawn(&r, P1, "skirmisher", x, y, 8));
    cmds.push(common::spawn(&r, P1, "mangonel", x, y, 2));
    cmds.push(common::spawn(&r, P1, "supply_wain", x, y, 1));
    a.issue(P0, cmds);
    assert!(a.units_of(P0, "yeoman").is_empty());
    let mut launched_at = None;
    let mut started = Vec::new();
    let mut resets = 0;
    let mut rejected = 0;
    while a.sim.result().is_none() && a.sim.tick() < 4000 {
        a.run(1);
        for e in a.sim.drain_events() {
            match e {
                SimEvent::AnnexationStarted { attacker, .. } => started.push(attacker),
                SimEvent::AnnexationReset { .. } => resets += 1,
                SimEvent::CommandRejected { player, .. } if player == P1 => rejected += 1,
                _ => {}
            }
        }
        if launched_at.is_none() && a.mem(P1).wave.is_some() {
            launched_at = Some(a.sim.tick());
        }
    }
    let launched_at = launched_at.expect("a launch");
    // M6: the first wave waits up to `noise_first_wave_ds` more, by the
    // bot's noise (`waves::first_wave_extra`), then launches at once.
    let k = r.ai.personality("standard").unwrap();
    let extra_ds = sim::ai_noise(1, P1, ai::standard::ctx::salt::FIRST_WAVE)
        % (u64::from(k.noise_first_wave_ds) + 1);
    let extra = r.ticks_from_ds(u32::try_from(extra_ds).unwrap());
    assert!(
        launched_at <= 20 + extra,
        "launched at {launched_at} (first-wave wait {extra})"
    );
    let result = a.sim.result().expect("decided before 4000");
    assert_eq!(
        (result.outcome, result.reason),
        (
            sim::Outcome::Victory {
                player: P1,
                decisive: true
            },
            sim::OutcomeReason::SeatCaptured
        )
    );
    assert_eq!(started, [P1]);
    assert_eq!((resets, rejected), (0, 0));
    println!(
        "bot_takes_seat: launched {launched_at}, decided {}",
        result.tick
    );
}

/// The first match's golden (ROADMAP M5b acceptance 1):
/// `m5_first_match.eonreplay`, the Standard bot against itself at seed 1
/// to the step that decides the match, verifies with the bots rebuilt
/// from its header (`ai::controller_for`) to the hash in
/// `m5_first_match.hash`, which `play-bots --seeds 1..5 --a standard --b
/// standard` prints for seed 1. Long (about 26000 ticks), so verified by
/// `sim-cli verify --release` in CI (rule 18).
#[test]
#[ignore = "long golden; verified by sim-cli verify --release in CI"]
fn golden_m5_first_match() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../sim/tests/fixtures");
    let replay = dir.join("m5_first_match.eonreplay");
    let expected = std::fs::read_to_string(dir.join("m5_first_match.hash")).unwrap();
    let outcome = sim::verify_with(
        &replay,
        &common::data_dir(),
        Box::new(|setup: &MatchSetup, rules: &Rules| {
            ai::controller_for(setup, rules).map_err(|e| e.to_string())
        }),
    )
    .unwrap();
    match outcome {
        sim::VerifyOutcome::Ok { final_hash, .. } => {
            assert_eq!(format!("0x{final_hash:016x}"), expected.trim());
        }
        other => panic!("m5_first_match: {other}"),
    }
}
