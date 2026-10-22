//! M4a acceptance tests. The tech slice: research, ages, the Modifier
//! resolver and what the resolved stats change, in tests named `tech_` so
//! that `cargo test -p sim tech` selects them (the filter matches test
//! paths, not integration-test file names). The combat core and behaviour:
//! hit points, the explicit `Attack`, simultaneous hits, death and
//! destruction, target acquisition by guards and attack-movers, the
//! Watchtower and Town shooters and the `combat_skirmish` golden's beats,
//! in tests named `combat_` (`cargo test -p sim combat`), plus the Arms hit
//! point and damage rows (`tech_arms_*`, `tech_every_arms_*`), the counter
//! triangle over 20 seeds (`counters_*`, acceptance 2), the golden itself
//! (`golden_combat_skirmish`) and the seeded random streams over tech and
//! combat (`m4a_*`).
//!
//! Since the combat behaviour slice an idle armed unit fights any enemy in
//! its reach on its own, so the tests that measure an explicit `Attack`
//! spawn the attacker in the tick its order applies (P0's commands apply
//! before P1's, so its target already exists), or keep the sides out of
//! each other's reach.
//!
//! Most tests run on `MatchSetup::scenario`, an empty start: with no
//! building there is no income, so stockpiles are exact through
//! `DebugGrant`. Where a test needs a Scriptorium (which yields Lore) it
//! compares with a control sim instead. Every rejection is compared with a
//! control sim that receives a rejected no-op with the same `(player,
//! seq)`, on every sub-hash (`rng` included) and the whole hash.

use proptest::prelude::*;
use rules::{ModOp, Modifier, Rules, StatPath, UnitRamp};
use sim::{
    BuildingId, BuildingKindId, Command, Fx, FxVec2, Intent, MatchSetup, PlayerCommand, PlayerId,
    QueueEntry, QueueItem, RejectReason, Sim, SimEvent, SimView, StartKind, TargetId, TechId, Tile,
    UnitId, UnitKindId,
};
use std::path::{Path, PathBuf};

const P0: PlayerId = PlayerId(0);
const P1: PlayerId = PlayerId(1);

struct NoAi;

impl sim::AiController for NoAi {
    fn think(&mut self, _player: PlayerId, _view: &SimView<'_>) -> Vec<Command> {
        Vec::new()
    }
}

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
}

fn load_rules() -> Rules {
    Rules::load(data_dir()).expect("data/ loads")
}

fn tech(rules: &Rules, id: &str) -> TechId {
    TechId(rules.tech_by_id(id).expect("tech in techs.ron").0)
}

fn res(rules: &Rules, id: &str) -> usize {
    rules.resource_index(id).expect("resource in resources.ron")
}

fn res_u8(rules: &Rules, id: &str) -> u8 {
    u8::try_from(res(rules, id)).unwrap()
}

fn bkind(rules: &Rules, id: &str) -> BuildingKindId {
    BuildingKindId(
        rules
            .building_kind_by_id(id)
            .expect("kind in buildings.ron")
            .0,
    )
}

fn ukind(rules: &Rules, id: &str) -> UnitKindId {
    UnitKindId(
        u16::try_from(
            rules
                .units
                .iter()
                .position(|u| u.id == id)
                .expect("kind in units.ron"),
        )
        .unwrap(),
    )
}

fn rejections(events: &[SimEvent]) -> Vec<RejectReason> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::CommandRejected { reason, .. } => Some(*reason),
            _ => None,
        })
        .collect()
}

/// Issue `cmds` this tick, step until they have been applied, and return
/// every event since the last drain.
fn apply(sim: &mut Sim, cmds: Vec<PlayerCommand>) -> Vec<SimEvent> {
    let delay = sim.setup().cmd_delay;
    sim.step(&cmds);
    for _ in 0..delay {
        sim.step(&[]);
    }
    sim.drain_events()
}

/// A sim plus the next sequence number per player, so two sims built by
/// the same steps stamp the same `(player, seq)` keys.
struct M {
    sim: Sim,
    seq: [u32; 2],
}

impl M {
    /// The empty-start scenario (debug commands on).
    fn new(rules: Rules, seed: u64) -> M {
        let setup = MatchSetup::scenario(&rules, seed);
        M {
            sim: Sim::new(setup, rules, Box::new(NoAi)),
            seq: [0; 2],
        }
    }

    /// The scenario from the standard start: P0's Seat is `BuildingId(1)`,
    /// P1's `BuildingId(2)`, P0's Yeomen `UnitId(1..=5)`.
    fn standard(rules: Rules, seed: u64) -> M {
        let mut setup = MatchSetup::scenario(&rules, seed);
        setup.start = StartKind::Standard;
        M {
            sim: Sim::new(setup, rules, Box::new(NoAi)),
            seq: [0; 2],
        }
    }

    fn stamp(&mut self, p: u8, cmd: Command) -> PlayerCommand {
        let i = usize::from(p.min(1));
        let pc = PlayerCommand::new(PlayerId(p), self.seq[i], cmd);
        self.seq[i] += 1;
        pc
    }

    /// Apply `cmds` from player `p` (in this order) and return the events.
    fn run_as(&mut self, p: u8, cmds: Vec<Command>) -> Vec<SimEvent> {
        let pcs = cmds.into_iter().map(|c| self.stamp(p, c)).collect();
        apply(&mut self.sim, pcs)
    }

    fn run(&mut self, cmds: Vec<Command>) -> Vec<SimEvent> {
        self.run_as(0, cmds)
    }

    /// `run`, asserting that nothing was rejected.
    fn ok(&mut self, cmds: Vec<Command>) -> Vec<SimEvent> {
        let events = self.run(cmds);
        assert!(rejections(&events).is_empty(), "{events:?}");
        events
    }

    /// Step `n` ticks with no commands; the events since the last drain.
    fn step(&mut self, n: u32) -> Vec<SimEvent> {
        for _ in 0..n {
            self.sim.step(&[]);
        }
        self.sim.drain_events()
    }

    fn view(&self) -> SimView<'_> {
        self.sim.view()
    }

    fn stock(&self, p: PlayerId) -> Vec<i64> {
        self.view().economy(p).stock.clone()
    }

    fn rules(&self) -> &Rules {
        self.sim.rules()
    }
}

fn grant(resource: usize, amount: u32) -> Command {
    Command::DebugGrant {
        owner: P0,
        resource: u8::try_from(resource).unwrap(),
        amount,
    }
}

fn place(kind: BuildingKindId, x: u16, y: u16, complete: bool) -> Command {
    place_for(P0, kind, x, y, complete)
}

fn place_for(owner: PlayerId, kind: BuildingKindId, x: u16, y: u16, complete: bool) -> Command {
    Command::DebugPlaceBuilding {
        owner,
        kind,
        tile: Tile::new(x, y),
        complete,
    }
}

fn research(building: u32, tech: TechId) -> Command {
    Command::Research {
        building: BuildingId(building),
        tech,
    }
}

fn debug_research(tech: TechId) -> Command {
    Command::DebugResearch { owner: P0, tech }
}

fn advance() -> Command {
    Command::AdvanceAge { player: P0 }
}

fn spawn(kind: UnitKindId, x: i32, y: i32, count: u16) -> Command {
    Command::DebugSpawn {
        owner: P0,
        kind,
        at: FxVec2::from_ints(x, y),
        count,
    }
}

fn train(building: u32, kind: UnitKindId) -> Command {
    Command::Train {
        building: BuildingId(building),
        kind,
    }
}

/// A rejected no-op: `UnitId(999)` never exists.
fn noop() -> Command {
    Command::Stop {
        units: vec![UnitId(999)],
    }
}

/// Build two sims with `setup`, apply `cmd` from `p` to one and a rejected
/// no-op with the same key to the other: `cmd` is rejected with `want`
/// and every sub-hash and the whole hash match the control.
fn rejected_without_change(
    name: &str,
    setup: &dyn Fn() -> M,
    p: u8,
    cmd: Command,
    want: RejectReason,
) {
    let mut sim = setup();
    let mut control = setup();
    let events = sim.run_as(p, vec![cmd]);
    let control_events = control.run_as(p, vec![noop()]);
    assert_eq!(rejections(&events), [want], "{name}: {events:?}");
    assert_eq!(rejections(&control_events).len(), 1, "{name}: control");
    assert_eq!(
        sim.sim.sub_hashes(),
        control.sim.sub_hashes(),
        "{name}: no state change, rng included"
    );
    assert_eq!(sim.sim.hash(), control.sim.hash(), "{name}");
}

/// The Scriptoria, Muster Hall and Town positions in the all-grass west
/// rectangle of `plains_1v1` (x 0..43, y 24..41).
const SCRIPTORIUM_A: (u16, u16) = (4, 26);
const SCRIPTORIUM_B: (u16, u16) = (8, 26);
const MUSTER_HALL: (u16, u16) = (12, 26);
const TOWN: (u16, u16) = (20, 30);

/// An empty start with P0's complete Scriptorium as `BuildingId(1)` and
/// plenty of every resource.
fn lab(rules: &Rules, seed: u64) -> M {
    let mut m = M::new(rules.clone(), seed);
    let s = bkind(rules, "scriptorium");
    m.ok(vec![
        place(s, SCRIPTORIUM_A.0, SCRIPTORIUM_A.1, true),
        grant(res(rules, "grain"), 2000),
        grant(res(rules, "lumber"), 2000),
        grant(res(rules, "ore"), 2000),
        grant(res(rules, "lore"), 999),
    ]);
    m
}

/// `DebugResearch` Letters I and Trade I, grant the Masonry cost and
/// advance; asserts P0 is then in the Masonry Age.
fn reach_masonry(m: &mut M) {
    let rules = m.rules().clone();
    let mut cmds = vec![
        debug_research(tech(&rules, "letters_1")),
        debug_research(tech(&rules, "trade_1")),
    ];
    for (id, &amount) in &rules.ages[1].cost {
        cmds.push(grant(res(&rules, id), amount));
    }
    cmds.push(advance());
    m.ok(cmds);
    assert_eq!(m.view().age(P0), 1);
}

/// Acceptance 1, first clause: the Masonry Age needs two completed techs
/// (a queued one does not count) and exactly 250 Grain and 100 Lore.
#[test]
fn tech_masonry_age_unlocks_only_after_two_techs_and_250_grain_100_lore() {
    let rules = load_rules();
    let (grain, lore) = (res(&rules, "grain"), res(&rules, "lore"));
    let (letters, trade) = (tech(&rules, "letters_1"), tech(&rules, "trade_1"));
    let rich = |m: &mut M| {
        m.ok(vec![grant(grain, 1000), grant(lore, 999)]);
    };
    rejected_without_change(
        "no tech",
        &|| {
            let mut m = M::new(rules.clone(), 1);
            rich(&mut m);
            m
        },
        0,
        advance(),
        RejectReason::NotEnoughTechs,
    );
    rejected_without_change(
        "one tech",
        &|| {
            let mut m = M::new(rules.clone(), 1);
            rich(&mut m);
            m.ok(vec![debug_research(letters)]);
            m
        },
        0,
        advance(),
        RejectReason::NotEnoughTechs,
    );
    rejected_without_change(
        "one tech and one queued",
        &|| {
            let mut m = lab(&rules, 1);
            m.ok(vec![debug_research(letters)]);
            m.ok(vec![research(1, trade)]);
            let queue = &m.view().building(BuildingId(1)).unwrap().queue;
            assert!(matches!(queue[0].item, QueueItem::Research { tech, .. } if tech == trade));
            m
        },
        0,
        advance(),
        RejectReason::NotEnoughTechs,
    );
    let two_techs = |g: u32, l: u32| {
        let rules = rules.clone();
        move || {
            let mut m = M::new(rules.clone(), 1);
            m.ok(vec![
                debug_research(letters),
                debug_research(trade),
                grant(grain, g),
                grant(lore, l),
            ]);
            m
        }
    };
    rejected_without_change(
        "249 Grain",
        &two_techs(249, 100),
        0,
        advance(),
        RejectReason::CannotAfford {
            resource: res_u8(&rules, "grain"),
        },
    );
    rejected_without_change(
        "99 Lore",
        &two_techs(250, 99),
        0,
        advance(),
        RejectReason::CannotAfford {
            resource: res_u8(&rules, "lore"),
        },
    );
    let mut m = two_techs(250, 100)();
    let mut want = vec![0i64; 4];
    want[grain] = 250;
    want[lore] = 100;
    assert_eq!(m.stock(P0), want);
    assert_eq!(m.view().check_advance_age(P0), Ok(want.clone()));
    let events = m.ok(vec![advance()]);
    assert!(events.contains(&SimEvent::AgeAdvanced { player: P0, age: 1 }));
    assert_eq!(m.view().age(P0), 1);
    assert_eq!(
        m.stock(P0),
        vec![0; 4],
        "the stock fell by exactly the cost"
    );
    assert_eq!(m.view().age(P1), 0, "P1 stays in the Hearth Age");
    eprintln!("masonry: 2 techs + 250 Grain + 100 Lore -> age 1, stock {want:?} -> 0");
}

/// Acceptance 1, second clause: the Charter Age needs five completed techs
/// and exactly 500 Grain, 300 Lumber and 300 Lore; it is the last age, and
/// one `AdvanceAge` moves one age.
#[test]
fn tech_charter_age_unlocks_only_after_five_techs_and_500_grain_300_lumber_300_lore() {
    let rules = load_rules();
    let (grain, lumber, lore) = (
        res(&rules, "grain"),
        res(&rules, "lumber"),
        res(&rules, "lore"),
    );
    const FIVE: [&str; 5] = [
        "letters_1",
        "trade_1",
        "arms_1",
        "statecraft_1",
        "letters_2",
    ];
    let in_masonry = |n: usize, g: u32, lu: u32, lo: u32| {
        let rules = rules.clone();
        move || {
            let mut m = M::new(rules.clone(), 2);
            reach_masonry(&mut m);
            let more = FIVE[2..n].iter().map(|id| debug_research(tech(&rules, id)));
            m.ok(more.collect());
            assert_eq!(m.view().researched(P0).len(), n);
            assert_eq!(m.stock(P0), vec![0; 4]);
            m.ok(vec![grant(grain, g), grant(lumber, lu), grant(lore, lo)]);
            m
        }
    };
    rejected_without_change(
        "four techs",
        &in_masonry(4, 500, 300, 300),
        0,
        advance(),
        RejectReason::NotEnoughTechs,
    );
    for (short, name) in [(grain, "grain"), (lumber, "lumber"), (lore, "lore")] {
        let less = |r: usize, v: u32| if r == short { v - 1 } else { v };
        rejected_without_change(
            &format!("one {name} short"),
            &in_masonry(5, less(grain, 500), less(lumber, 300), less(lore, 300)),
            0,
            advance(),
            RejectReason::CannotAfford {
                resource: res_u8(&rules, name),
            },
        );
    }
    let mut m = in_masonry(5, 500, 300, 300)();
    let events = m.ok(vec![advance()]);
    assert!(events.contains(&SimEvent::AgeAdvanced { player: P0, age: 2 }));
    assert_eq!(m.view().age(P0), 2);
    assert_eq!(
        m.stock(P0),
        vec![0; 4],
        "the stock fell by exactly the cost"
    );
    rejected_without_change(
        "the last age",
        &|| {
            let mut m = in_masonry(5, 2000, 2000, 999)();
            m.ok(vec![advance()]);
            m
        },
        0,
        advance(),
        RejectReason::FinalAge,
    );
    // A Hearth player with five techs and plenty of everything moves one
    // age per command.
    let mut m = M::new(rules.clone(), 2);
    let mut cmds: Vec<Command> = FIVE
        .iter()
        .map(|id| debug_research(tech(&rules, id)))
        .collect();
    cmds.extend([grant(grain, 2000), grant(lumber, 2000), grant(lore, 999)]);
    m.ok(cmds);
    let events = m.ok(vec![advance()]);
    let ages: Vec<u8> = events
        .iter()
        .filter_map(|e| match e {
            SimEvent::AgeAdvanced { age, .. } => Some(*age),
            _ => None,
        })
        .collect();
    assert_eq!(ages, [1], "only to Masonry");
    assert_eq!(m.view().age(P0), 1);
    m.ok(vec![advance()]);
    assert_eq!(m.view().age(P0), 2);
}

/// The gates in `ages.ron` are the design's numbers.
#[test]
fn tech_age_gates_match_the_design_numbers() {
    let rules = load_rules();
    let gate = |i: usize| {
        let a = &rules.ages[i];
        let cost: Vec<(&str, u32)> = a.cost.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        (a.id.as_str(), a.techs_required, cost)
    };
    assert_eq!(rules.ages.len(), 3);
    assert_eq!(gate(0), ("hearth", 0, vec![]));
    assert_eq!(gate(1), ("masonry", 2, vec![("grain", 250), ("lore", 100)]));
    assert_eq!(
        gate(2),
        (
            "charter",
            5,
            vec![("grain", 500), ("lore", 300), ("lumber", 300)]
        )
    );
}

/// Acceptance 1, third clause: each Letters level cuts the next research's
/// cost (and time) by 10%, floored after each row. The probe is Arms I,
/// whose 75 Lore makes the floor visible (67.5 -> 67); Trade I and Arms II
/// show that the floor follows each row rather than one floor over 0.729
/// at the end; a real `Research` at Letters III deducts exactly the quote
/// and queues its time.
#[test]
fn tech_each_letters_level_cuts_the_next_research_cost_by_ten_percent_floored() {
    let rules = load_rules();
    let (ore, lore) = (res(&rules, "ore"), res(&rules, "lore"));
    let letters = ["letters_1", "letters_2", "letters_3"];
    for id in letters {
        let (_, t) = rules.tech_by_id(id).unwrap();
        let rows: Vec<(&StatPath, ModOp, i32)> = t
            .modifiers
            .iter()
            .map(|m| (&m.target, m.op, m.value))
            .collect();
        assert_eq!(
            rows,
            [
                (&StatPath::ResearchCost, ModOp::Mul, 900),
                (&StatPath::ResearchTime, ModOp::Mul, 900)
            ],
            "{id}"
        );
    }
    let arms = tech(&rules, "arms_1");
    let probe = rules.tech(arms.0).unwrap();
    assert_ne!(probe.cost["lore"] % 10, 0, "the floor must be visible");
    // `research_quote` prices a tech whatever its age and predecessor.
    let others = [tech(&rules, "trade_1"), tech(&rules, "arms_2")];
    let mut m = M::new(rules.clone(), 4);
    let mut quotes = vec![m.view().research_quote(P0, arms)];
    let mut other_quotes = vec![others.map(|t| m.view().research_quote(P0, t))];
    for id in letters {
        m.ok(vec![debug_research(tech(&rules, id))]);
        quotes.push(m.view().research_quote(P0, arms));
        other_quotes.push(others.map(|t| m.view().research_quote(P0, t)));
    }
    let column = |r: usize| quotes.iter().map(|(c, _)| c[r]).collect::<Vec<i64>>();
    assert_eq!(column(lore), [75, 67, 60, 54]);
    assert_eq!(column(ore), [50, 45, 40, 36]);
    for w in quotes.windows(2) {
        for r in 0..4 {
            assert_eq!(w[1].0[r], w[0].0[r] * 900 / 1000, "floor(prev * 0.9)");
        }
    }
    let ticks: Vec<u32> = quotes.iter().map(|(_, t)| *t).collect();
    let want: Vec<u32> = [300, 270, 243, 218]
        .map(|ds| rules.ticks_from_ds(ds))
        .into();
    assert_eq!(ticks, want, "300 -> 270 -> 243 -> 218 ds");
    for (level, (cost, t)) in quotes.iter().enumerate() {
        eprintln!(
            "letters {level}: arms_1 costs {} Lore + {} Ore, {t} ticks",
            cost[lore], cost[ore]
        );
    }
    // Arms I cannot tell a floor after each row from one floor over the
    // product at the end: 75, 50 and 300 times 0.729 (54.675, 36.45,
    // 218.7) floor to 54, 36 and 218 as well. Trade I and Arms II can: one
    // floor over 0.729 would end at 51 Lore (70 * 0.729 = 51.03), 109 Lore
    // (150 * 0.729 = 109.35) and 328 ds (450 * 0.729 = 328.05).
    let base = |t: TechId, r: &str| i64::from(rules.tech(t.0).unwrap().cost[r]);
    assert_eq!(base(others[0], "lore") * 729 / 1000, 51, "one floor");
    assert_eq!(base(others[1], "lore") * 729 / 1000, 109, "one floor");
    let trade_lore: Vec<i64> = other_quotes.iter().map(|q| q[0].0[lore]).collect();
    assert_eq!(
        trade_lore,
        [70, 63, 56, 50],
        "Trade I, floored after each row"
    );
    let arms_2: Vec<(i64, i64, u32)> = other_quotes
        .iter()
        .map(|q| (q[1].0[lore], q[1].0[ore], q[1].1))
        .collect();
    let want: Vec<(i64, i64, u32)> = [
        (150, 100, 450),
        (135, 90, 405),
        (121, 81, 364),
        (108, 72, 327),
    ]
    .map(|(l, o, ds)| (l, o, rules.ticks_from_ds(ds)))
    .into();
    assert_eq!(arms_2, want, "Arms II: 450 -> 405 -> 364 -> 327 ds");
    for (level, (trade_lore, (lore_2, ore_2, t))) in trade_lore.iter().zip(&arms_2).enumerate() {
        eprintln!(
            "letters {level}: trade_1 costs {trade_lore} Lore; arms_2 costs {lore_2} Lore + {ore_2} Ore, {t} ticks"
        );
    }
    for w in other_quotes.windows(2) {
        for (prev, next) in w[0].iter().zip(&w[1]) {
            for r in 0..4 {
                assert_eq!(next.0[r], prev.0[r] * 900 / 1000, "floor(prev * 0.9)");
            }
        }
    }
    // A real Research at Letters III deducts exactly the quote.
    let setup = || {
        let mut m = lab(&rules, 4);
        m.ok(letters.map(|id| debug_research(tech(&rules, id))).into());
        m
    };
    let mut a = setup();
    let mut b = setup();
    let (cost, t) = a.view().research_quote(P0, arms);
    assert_eq!(
        a.view().check_research(P0, BuildingId(1), arms),
        Ok((cost.clone(), t))
    );
    a.ok(vec![research(1, arms)]);
    b.run(vec![noop()]);
    let paid: Vec<i64> = b
        .stock(P0)
        .iter()
        .zip(a.stock(P0))
        .map(|(b, a)| b - a)
        .collect();
    assert_eq!(paid, cost);
    assert_eq!(
        a.view().building(BuildingId(1)).unwrap().queue,
        [QueueEntry {
            item: QueueItem::Research {
                tech: arms,
                ticks: t
            },
            paid: cost,
            progress: 1,
        }]
    );
}

/// Acceptance 1, fourth clause: Set -> Add -> Mul with a floor after each
/// `Mul`, through `rules::modifiers::resolve`, then in the sim.
#[test]
fn tech_modifier_stacking_set_add_mul_produces_the_documented_values() {
    use rules::modifiers::resolve;
    let row = |op: ModOp, value: i32| Modifier {
        target: StatPath::YieldCap,
        op,
        value,
    };
    let (set, add, mul) = (
        |v| row(ModOp::Set, v),
        |v| row(ModOp::Add, v),
        |v| row(ModOp::Mul, v),
    );
    // The worked examples in tech-and-ages.md and economy.md.
    assert_eq!(resolve(70, &[mul(1250)]), 87);
    assert_eq!(resolve(70, &[set(100), mul(1250)]), 125);
    assert_eq!(resolve(25, &[add(25), add(25)]), 75);
    assert_eq!(resolve(100, &[mul(900)]), 90);
    assert_eq!(resolve(100, &[mul(900), mul(900)]), 81);
    assert_eq!(resolve(100, &[mul(900), mul(900), mul(900)]), 72);
    // A floor after each row: 150 -> 135 -> 121 -> 108; one floor over the
    // product would give 109 (150 * 0.729 = 109.35).
    assert_eq!(resolve(150, &[mul(900), mul(900), mul(900)]), 108);
    assert_eq!(resolve(2, &[add(1)]), 3);
    assert_eq!(resolve(70, &[add(10), mul(1250)]), 100);
    assert_eq!(resolve(70, &[set(100), add(10)]), 110);
    assert_eq!(resolve(100, &[mul(1100), mul(1100)]), 121);
    // Set applies first, then Add, then Mul, in any row order (one Set,
    // one Add, one Mul): (100 + 10) * 1.25 = 137.5 -> 137.
    let rows = [set(100), add(10), mul(1250)];
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let permuted: Vec<&Modifier> = order.iter().map(|&i| &rows[i]).collect();
        assert_eq!(resolve(70, permuted.iter().copied()), 137, "{order:?}");
    }
    // Several Sets: the last in collection order wins.
    assert_eq!(resolve(70, &[set(100), set(150)]), 150);
    assert_eq!(resolve(70, &[set(150), set(100)]), 100);
    // The floor is div_euclid, not truncation: -1 * 1.5 = -1.5 -> -2.
    assert_eq!(resolve(1, &[add(-2), mul(1500)]), -2);
    // In the sim: a faction Mul 1250 and Trade I's Set 100 give 125.
    let mut rules = load_rules();
    rules.factions[0].modifiers.push(mul(1250));
    let grain = res(&rules, "grain");
    let mut m = M::new(rules.clone(), 5);
    assert_eq!(m.view().income(P0)[grain].cap, Some(87));
    m.ok(vec![debug_research(tech(&rules, "trade_1"))]);
    assert_eq!(m.view().income(P0)[grain].cap, Some(125));
    // Pop cap 25 + Arms I and II = 75; town limit 2 + Statecraft I = 3.
    let rules = load_rules();
    let mut m = M::new(rules.clone(), 5);
    m.ok(vec![
        debug_research(tech(&rules, "arms_1")),
        debug_research(tech(&rules, "arms_2")),
        debug_research(tech(&rules, "statecraft_1")),
    ]);
    assert_eq!(m.view().pop(P0).1, 75);
    assert_eq!(m.view().towns(P0).1, 3);
}

/// Acceptance 1, last clause: a faction row `YieldCap Mul 1250` (the M9
/// Tidewater League example, pushed into `factions[0]` in memory) gives a
/// Yield Cap of 87 at Trade level 0: with ten worked Crofts (raw 120) the
/// Grain stockpile grows by exactly 87 per income period.
#[test]
fn tech_faction_yield_cap_mul_1250_gives_cap_87_at_trade_level_0() {
    let mut rules = load_rules();
    rules.factions[0].modifiers.push(Modifier {
        target: StatPath::YieldCap,
        op: ModOp::Mul,
        value: 1250,
    });
    let grain = res(&rules, "grain");
    let (town, croft) = (bkind(&rules, "town"), bkind(&rules, "croft"));
    let period = rules.income_period_ticks();
    let mut m = M::new(rules.clone(), 6);
    assert_eq!(m.view().income(P0)[grain].cap, Some(87));
    assert_eq!(m.view().income(P1)[grain].cap, Some(87), "every player");
    // As in economy_ten_worked_crofts_flatline_at_the_yield_cap: two Towns
    // (BuildingId 1, 2), ten Crofts (3..=12), ten Yeomen (UnitId 1..=10).
    let mut cmds = vec![place(town, 23, 63, true), place(town, 40, 63, true)];
    let sites = [28u16, 31, 34, 37, 40, 43]
        .map(|x| (x, 58u16))
        .into_iter()
        .chain([28, 31, 34, 37].map(|x| (x, 60)));
    for (x, y) in sites {
        cmds.push(place(croft, x, y, true));
    }
    cmds.push(spawn(ukind(&rules, "yeoman"), 32, 67, 10));
    m.ok(cmds);
    let gathers = (0..10u32)
        .map(|i| Command::Gather {
            units: vec![UnitId(1 + i)],
            node: BuildingId(3 + i),
        })
        .collect();
    m.ok(gathers);
    let worked = |m: &M| -> u32 {
        (3..=12)
            .map(|b| u32::from(m.view().worked_slots(BuildingId(b)).unwrap().0))
            .sum()
    };
    let mut waited = 0;
    while worked(&m) < 10 {
        m.step(1);
        waited += 1;
        assert!(waited < 2000, "the Yeomen reach their Crofts");
    }
    let info = m.view().income(P0)[grain];
    assert!(info.raw > 87, "raw {} must exceed the cap", info.raw);
    assert_eq!((info.cap, info.credited), (Some(87), 87));
    for period_no in 0..3 {
        let before = m.stock(P0)[grain];
        m.step(period);
        let gained = m.stock(P0)[grain] - before;
        eprintln!("faction Mul 1250, Trade 0: period {period_no} +{gained} Grain");
        assert_eq!(gained, 87);
    }
}

/// A research entry sits in its host's queue and completes exactly its
/// fixed number of ticks after the step that queued it began it.
#[test]
fn tech_research_completes_at_the_host_after_its_time() {
    let rules = load_rules();
    let letters = tech(&rules, "letters_1");
    let mut m = lab(&rules, 7);
    let (_, ticks) = m.view().research_quote(P0, letters);
    assert_eq!(ticks, rules.ticks_from_ds(250));
    m.ok(vec![research(1, letters)]);
    // Applied (and given its first tick) in the last step of `ok`.
    let started = m.sim.tick() - 1;
    let head = &m.view().building(BuildingId(1)).unwrap().queue[0];
    assert_eq!(
        head.item,
        QueueItem::Research {
            tech: letters,
            ticks
        }
    );
    assert_eq!(head.progress, 1);
    let events = m.step(ticks - 2);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SimEvent::ResearchCompleted { .. }))
    );
    assert!(m.view().researched(P0).is_empty());
    assert_eq!(
        m.view().building(BuildingId(1)).unwrap().queue[0].progress,
        ticks - 1
    );
    let events = m.step(1);
    assert_eq!(m.sim.tick() - 1, started + ticks - 1, "the ticks-th step");
    assert!(events.contains(&SimEvent::ResearchCompleted {
        player: P0,
        tech: letters
    }));
    assert!(m.view().researched(P0).contains(&letters));
    assert!(m.view().building(BuildingId(1)).unwrap().queue.is_empty());
}

/// Every `Research` rejection in check order; each changes nothing.
#[test]
fn tech_research_rejections_change_nothing() {
    let rules = load_rules();
    let t = |id| tech(&rules, id);
    let scribe = ukind(&rules, "scribe");
    let base = || {
        // BuildingId(1) complete Scriptorium (lab), (2) a Scriptorium
        // site, (3) a complete Muster Hall, (4) P1's Scriptorium.
        let mut m = lab(&rules, 8);
        let s = bkind(&rules, "scriptorium");
        m.ok(vec![
            place(s, SCRIPTORIUM_B.0, SCRIPTORIUM_B.1, false),
            place(
                bkind(&rules, "muster_hall"),
                MUSTER_HALL.0,
                MUSTER_HALL.1,
                true,
            ),
            place_for(P1, s, 16, 26, true),
        ]);
        m
    };
    type Setup<'a> = Box<dyn Fn() -> M + 'a>;
    let cases: Vec<(&str, Setup, Command, RejectReason)> = vec![
        (
            "no such building",
            Box::new(base),
            research(99, t("letters_1")),
            RejectReason::UnknownBuilding,
        ),
        (
            "another player's Scriptorium",
            Box::new(base),
            research(4, t("letters_1")),
            RejectReason::UnknownBuilding,
        ),
        (
            "a site",
            Box::new(base),
            research(2, t("letters_1")),
            RejectReason::UnderConstruction,
        ),
        (
            "no such tech",
            Box::new(base),
            research(1, TechId(99)),
            RejectReason::UnknownTech,
        ),
        (
            "at a Muster Hall",
            Box::new(base),
            research(3, t("letters_1")),
            RejectReason::CannotResearchHere,
        ),
        (
            "level 2 without level 1, in the Hearth Age",
            Box::new(base),
            research(1, t("letters_2")),
            RejectReason::PrerequisiteMissing,
        ),
        (
            "level 2 in the Hearth Age",
            Box::new(|| {
                let mut m = base();
                m.ok(vec![debug_research(t("letters_1"))]);
                m
            }),
            research(1, t("letters_2")),
            RejectReason::AgeRequired,
        ),
        (
            "already researched",
            Box::new(|| {
                let mut m = base();
                m.ok(vec![debug_research(t("letters_1"))]);
                m
            }),
            research(1, t("letters_1")),
            RejectReason::AlreadyResearched,
        ),
        (
            "queued at the same Scriptorium",
            Box::new(|| {
                let mut m = base();
                m.ok(vec![research(1, t("letters_1"))]);
                m
            }),
            research(1, t("letters_1")),
            RejectReason::ResearchInProgress,
        ),
        (
            "queued at another own Scriptorium",
            Box::new(|| {
                let mut m = base();
                let s = bkind(&rules, "scriptorium");
                m.ok(vec![place(s, 4, 32, true)]);
                m.ok(vec![research(5, t("letters_1"))]);
                m
            }),
            research(1, t("letters_1")),
            RejectReason::ResearchInProgress,
        ),
        (
            "a full queue",
            Box::new(|| {
                let mut m = base();
                m.ok(vec![train(1, scribe); 5]);
                m
            }),
            research(1, t("letters_1")),
            RejectReason::QueueFull,
        ),
        (
            "no Lore",
            Box::new(|| {
                let mut m = M::new(rules.clone(), 8);
                let s = bkind(&rules, "scriptorium");
                m.ok(vec![place(s, SCRIPTORIUM_A.0, SCRIPTORIUM_A.1, true)]);
                m
            }),
            research(1, t("letters_1")),
            RejectReason::CannotAfford {
                resource: res_u8(&rules, "lore"),
            },
        ),
        (
            "no Ore (the first short resource)",
            Box::new(|| {
                let mut m = M::new(rules.clone(), 8);
                let s = bkind(&rules, "scriptorium");
                m.ok(vec![
                    place(s, SCRIPTORIUM_A.0, SCRIPTORIUM_A.1, true),
                    grant(res(&rules, "lore"), 999),
                ]);
                m
            }),
            research(1, t("arms_1")),
            RejectReason::CannotAfford {
                resource: res_u8(&rules, "ore"),
            },
        ),
    ];
    for (name, setup, cmd, want) in cases {
        rejected_without_change(name, &*setup, 0, cmd, want);
    }
}

/// Every `AdvanceAge` rejection in check order; each changes nothing.
#[test]
fn tech_advance_age_rejections_change_nothing() {
    let rules = load_rules();
    let empty = || M::new(rules.clone(), 9);
    rejected_without_change(
        "naming another player (checked first)",
        &empty,
        0,
        Command::AdvanceAge { player: P1 },
        RejectReason::PlayerMismatch,
    );
    rejected_without_change(
        "naming an unknown player",
        &empty,
        0,
        Command::AdvanceAge {
            player: PlayerId(7),
        },
        RejectReason::PlayerMismatch,
    );
    rejected_without_change(
        "no tech and no stock (techs checked before cost)",
        &empty,
        0,
        advance(),
        RejectReason::NotEnoughTechs,
    );
    rejected_without_change(
        "two techs, no stock",
        &|| {
            let mut m = empty();
            m.ok(vec![
                debug_research(tech(&rules, "letters_1")),
                debug_research(tech(&rules, "trade_1")),
            ]);
            m
        },
        0,
        advance(),
        RejectReason::CannotAfford {
            resource: res_u8(&rules, "grain"),
        },
    );
    let at_charter = || {
        let mut m = empty();
        let mut cmds: Vec<Command> = rules.techs[..5]
            .iter()
            .map(|t| debug_research(tech(&rules, &t.id)))
            .collect();
        cmds.extend([
            grant(res(&rules, "grain"), 2000),
            grant(res(&rules, "lumber"), 2000),
            grant(res(&rules, "lore"), 999),
            advance(),
            advance(),
        ]);
        m.ok(cmds);
        assert_eq!(m.view().age(P0), 2);
        m
    };
    rejected_without_change(
        "the last age",
        &at_charter,
        0,
        advance(),
        RejectReason::FinalAge,
    );
    rejected_without_change(
        "the last age, naming another player",
        &at_charter,
        0,
        Command::AdvanceAge { player: P1 },
        RejectReason::PlayerMismatch,
    );
    rejected_without_change(
        "P1 has no tech",
        &at_charter,
        1,
        Command::AdvanceAge { player: P1 },
        RejectReason::NotEnoughTechs,
    );
}

/// Level k of a line needs level k-1 researched and age k (the data puts
/// level k in age k-1 counting from 0): every tech at every age, with all
/// four level-1 techs researched and with or without Letters II (so level
/// 2 with its predecessor, Letters III with and without its predecessor,
/// the other level-3 techs without it); then level 2 is rejected in the
/// Hearth Age and accepted in Masonry, and level 3 with its predecessor is
/// rejected in Masonry and accepted in Charter, by applying the command.
#[test]
fn tech_level_k_needs_level_k_minus_1_and_age_k() {
    let rules = load_rules();
    for (i, t) in rules.techs.iter().enumerate() {
        let i = u16::try_from(i).unwrap();
        assert_eq!(rules.age_of_tech(i), t.level - 1, "{}", t.id);
        assert_eq!(rules.tech_predecessor(i).is_some(), t.level > 1, "{}", t.id);
    }
    // A lab in each age, with Letters II or without it (`DebugResearch`
    // skips the age check); the Charter Age needs a fifth tech.
    let at_age = |age: u8, letters_2: bool| {
        let mut m = lab(&rules, 10);
        let mut ids = vec!["arms_1", "statecraft_1", "trade_1", "letters_1"];
        if letters_2 {
            ids.push("letters_2");
        }
        let mut cmds: Vec<Command> = ids
            .iter()
            .map(|id| debug_research(tech(&rules, id)))
            .collect();
        cmds.extend((0..age).map(|_| advance()));
        m.ok(cmds);
        m
    };
    let variants = [(0, false), (0, true), (1, false), (1, true), (2, true)];
    for (age, letters_2) in variants {
        let m = at_age(age, letters_2);
        assert_eq!(m.view().age(P0), age);
        for (i, t) in rules.techs.iter().enumerate() {
            let id = TechId(u16::try_from(i).unwrap());
            let researched = m.view().researched(P0).contains(&id);
            let pred_done = rules
                .tech_predecessor(id.0)
                .is_none_or(|p| m.view().researched(P0).contains(&TechId(p)));
            let got = m.view().check_research(P0, BuildingId(1), id).map(|_| ());
            let want = if !pred_done {
                Err(RejectReason::PrerequisiteMissing)
            } else if age < t.level - 1 {
                Err(RejectReason::AgeRequired)
            } else if researched {
                Err(RejectReason::AlreadyResearched)
            } else {
                Ok(())
            };
            assert_eq!(got, want, "{} at age {age}, {letters_2}", t.id);
        }
        // Letters III with its predecessor still waits for the Charter Age.
        let letters_3 = m
            .view()
            .check_research(P0, BuildingId(1), tech(&rules, "letters_3"));
        if letters_2 && age < 2 {
            assert_eq!(letters_3, Err(RejectReason::AgeRequired), "age {age}");
        }
    }
    // The checks are what apply does: level 2 is rejected in Hearth and
    // accepted once in Masonry; level 3 with its predecessor is rejected in
    // Masonry (changing nothing) and accepted in Charter.
    let (letters_2, letters_3) = (tech(&rules, "letters_2"), tech(&rules, "letters_3"));
    let mut m = at_age(0, false);
    let events = m.run(vec![research(1, letters_2)]);
    assert_eq!(rejections(&events), [RejectReason::AgeRequired]);
    let mut m = at_age(1, false);
    m.ok(vec![research(1, letters_2)]);
    rejected_without_change(
        "level 3 in the Masonry Age",
        &|| at_age(1, true),
        0,
        research(1, letters_3),
        RejectReason::AgeRequired,
    );
    let mut m = at_age(2, true);
    m.ok(vec![research(1, letters_3)]);
}

/// Trade I, II and III set the Yield Cap to 100, 150 and 200 for every
/// capped resource; Lore stays uncapped.
#[test]
fn tech_trade_sets_the_yield_cap_100_150_200() {
    let rules = load_rules();
    let mut m = M::new(rules.clone(), 11);
    let caps = |m: &M| -> Vec<Option<u32>> { m.view().income(P0).iter().map(|r| r.cap).collect() };
    let capped: Vec<bool> = rules
        .resources
        .resources
        .iter()
        .map(|r| r.yield_capped)
        .collect();
    let expect = |c: u32| -> Vec<Option<u32>> { capped.iter().map(|&y| y.then_some(c)).collect() };
    assert_eq!(caps(&m), expect(70));
    for (id, cap) in [("trade_1", 100), ("trade_2", 150), ("trade_3", 200)] {
        m.ok(vec![debug_research(tech(&rules, id))]);
        assert_eq!(caps(&m), expect(cap), "{id}");
        assert_eq!(m.view().income(P1)[0].cap, Some(70), "only P0's cap");
    }
    assert_eq!(caps(&m)[res(&rules, "lore")], None);
}

/// Statecraft I, II and III each add 1 to the town limit and 1 tile to the
/// radius of every own Town at once (a field change for each), not to
/// another player's; a Town completed afterwards projects the pushed
/// radius; the field always equals a full recompute.
#[test]
fn tech_statecraft_raises_the_town_limit_and_pushes_every_own_border() {
    let rules = load_rules();
    let town = bkind(&rules, "town");
    let r1 = u16::try_from(rules.town_radius_tiles).unwrap();
    let mut m = M::new(rules.clone(), 12);
    m.ok(vec![
        place(town, 6, 34, true),
        place(town, 30, 34, true),
        place_for(P1, town, 102, 63, true),
        grant(res(&rules, "lumber"), 1000),
        spawn(ukind(&rules, "yeoman"), 10, 38, 1),
    ]);
    let radius = |m: &M, b: u32| m.view().territory().sources()[&BuildingId(b)].radius;
    assert_eq!((radius(&m, 1), radius(&m, 2), radius(&m, 3)), (r1, r1, r1));
    assert_eq!(m.view().towns(P0), (2, 2));
    let third_town = |m: &M| m.view().check_build(P0, UnitId(1), town, Tile::new(14, 26));
    assert_eq!(third_town(&m), Err(RejectReason::TownLimitReached));
    assert_eq!(m.view().check_build_kind(P0, town), third_town(&m));
    let events = m.ok(vec![debug_research(tech(&rules, "statecraft_1"))]);
    assert_eq!(
        events
            .iter()
            .filter(|e| **e == SimEvent::TerritoryChanged)
            .count(),
        2,
        "one change per own Town"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SimEvent::TownTierChanged { .. }))
    );
    assert_eq!(radius(&m, 1), r1 + 1);
    assert_eq!(radius(&m, 2), r1 + 1);
    assert_eq!(radius(&m, 3), r1, "P1's Town is not pushed");
    assert_eq!(radius(&m, 1), 21);
    assert_eq!(m.view().towns(P0), (2, 3));
    assert!(third_town(&m).is_ok(), "{:?}", third_town(&m));
    assert_eq!(m.view().check_build_kind(P0, town), third_town(&m));
    assert_eq!(m.view().territory(), &m.view().territory().recomputed());
    m.ok(vec![place(town, 14, 38, true)]);
    assert_eq!(
        radius(&m, 4),
        r1 + 1,
        "a later Town projects the pushed radius"
    );
    m.ok(vec![debug_research(tech(&rules, "statecraft_2"))]);
    assert_eq!(
        (radius(&m, 1), radius(&m, 2), radius(&m, 4), radius(&m, 3)),
        (r1 + 2, r1 + 2, r1 + 2, r1)
    );
    assert_eq!(m.view().towns(P0), (3, 4));
    assert_eq!(m.view().territory(), &m.view().territory().recomputed());
    let events = m.ok(vec![debug_research(tech(&rules, "statecraft_3"))]);
    assert_eq!(
        events
            .iter()
            .filter(|e| **e == SimEvent::TerritoryChanged)
            .count(),
        3,
        "one change per own Town"
    );
    assert_eq!(
        (radius(&m, 1), radius(&m, 2), radius(&m, 4), radius(&m, 3)),
        (r1 + 3, r1 + 3, r1 + 3, r1)
    );
    assert_eq!(radius(&m, 1), 23);
    assert_eq!(m.view().towns(P0), (3, 5));
    assert_eq!(m.view().territory(), &m.view().territory().recomputed());
}

/// Each Arms level adds 25 to the pop cap, and `Train` honours it.
#[test]
fn tech_arms_raises_the_pop_cap() {
    let rules = load_rules();
    let yeoman = ukind(&rules, "yeoman");
    let mut m = M::new(rules.clone(), 13);
    assert_eq!(m.view().pop(P0), (0, 25));
    m.ok(vec![
        place(bkind(&rules, "town"), TOWN.0, TOWN.1, true),
        grant(res(&rules, "grain"), 5000),
        spawn(yeoman, 10, 36, 25),
    ]);
    assert_eq!(m.view().pop(P0), (25, 25));
    let events = m.run(vec![train(1, yeoman)]);
    assert_eq!(rejections(&events), [RejectReason::PopCapReached]);
    for (id, cap) in [("arms_1", 50), ("arms_2", 75), ("arms_3", 100)] {
        m.ok(vec![debug_research(tech(&rules, id))]);
        assert_eq!(m.view().pop(P0).1, cap, "{id}");
    }
    m.ok(vec![train(1, yeoman)]);
    assert_eq!(m.view().pop(P0), (26, 100));
    assert_eq!(m.view().pop(P1), (0, 25), "only P0's cap");
}

/// A unit kind's `requires_tech` (set in memory: the Outrider needs Arms I)
/// gates `Train` after the age check.
#[test]
fn tech_requires_tech_gates_training() {
    let mut rules = load_rules();
    let outrider = ukind(&rules, "outrider");
    rules.units[usize::from(outrider.0)].requires_tech = Some("arms_1".into());
    let setup = || {
        let mut m = M::new(rules.clone(), 14);
        m.ok(vec![
            place(
                bkind(&rules, "muster_hall"),
                MUSTER_HALL.0,
                MUSTER_HALL.1,
                true,
            ),
            grant(res(&rules, "grain"), 2000),
            grant(res(&rules, "ore"), 2000),
        ]);
        m
    };
    rejected_without_change(
        "Hearth Age: the age first",
        &setup,
        0,
        train(1, outrider),
        RejectReason::AgeRequired,
    );
    let masonry = || {
        let mut m = setup();
        reach_masonry(&mut m);
        m
    };
    rejected_without_change(
        "Masonry without Arms I",
        &masonry,
        0,
        train(1, outrider),
        RejectReason::PrerequisiteMissing,
    );
    let mut m = masonry();
    m.ok(vec![debug_research(tech(&rules, "arms_1"))]);
    m.ok(vec![train(1, outrider)]);
    // With the shipped data no kind names a tech.
    assert!(load_rules().units.iter().all(|u| u.requires_tech.is_none()));
}

/// The Masonry Age gates the Engine Yard, the Masonry units and Town tier
/// 2 (`town_growth_age`): in the Hearth Age a Town with five distinct
/// attributed kinds stays at tier 1, and grows at the `AdvanceAge`.
#[test]
fn tech_masonry_gates_engine_yard_outrider_and_town_tier_two() {
    let rules = load_rules();
    let (yard, hall) = (bkind(&rules, "engine_yard"), bkind(&rules, "muster_hall"));
    let outrider = ukind(&rules, "outrider");
    let (r1, r2) = (
        u16::try_from(rules.town_radius_tiles).unwrap(),
        u16::try_from(rules.town_radius_grown_tiles).unwrap(),
    );
    // Five distinct kinds at the Seat, the Engine Yard by debug placement
    // (which ignores ages): BuildingId 3..=7, the Muster Hall is 5.
    let setup = || {
        let mut m = M::standard(rules.clone(), 15);
        let kinds = [
            ("croft", 28u16, 58u16),
            ("scriptorium", 28, 67),
            ("muster_hall", 32, 67),
            ("engine_yard", 36, 67),
            ("watchtower", 31, 58),
        ];
        let events = m.ok(kinds
            .iter()
            .map(|(id, x, y)| place(bkind(&rules, id), *x, *y, true))
            .collect());
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, SimEvent::TownTierChanged { .. }))
        );
        m.ok(vec![
            grant(res(&rules, "grain"), 1000),
            grant(res(&rules, "ore"), 1000),
            grant(res(&rules, "lumber"), 1000),
        ]);
        assert_eq!(
            m.view().building(BuildingId(5)).unwrap().kind,
            hall,
            "the Muster Hall"
        );
        m
    };
    let seat_tier = |m: &M| {
        m.view()
            .building(BuildingId(1))
            .unwrap()
            .town_state
            .unwrap()
            .tier
    };
    let seat_radius = |m: &M| m.view().territory().sources()[&BuildingId(1)].radius;
    let m = setup();
    assert_eq!((seat_tier(&m), seat_radius(&m)), (1, r1), "Hearth: tier 1");
    let yard_at = |m: &M| m.view().check_build(P0, UnitId(1), yard, Tile::new(19, 67));
    assert_eq!(yard_at(&m), Err(RejectReason::AgeRequired));
    assert_eq!(m.view().check_build_kind(P0, yard), yard_at(&m));
    rejected_without_change(
        "an Engine Yard in the Hearth Age",
        &setup,
        0,
        Command::Build {
            unit: UnitId(1),
            kind: yard,
            tile: Tile::new(19, 67),
        },
        RejectReason::AgeRequired,
    );
    rejected_without_change(
        "an Outrider in the Hearth Age",
        &setup,
        0,
        train(5, outrider),
        RejectReason::AgeRequired,
    );
    // A site placed earlier is resumed whatever the age.
    let mut m = setup();
    m.ok(vec![place(yard, 14, 58, false)]);
    m.ok(vec![Command::Build {
        unit: UnitId(2),
        kind: yard,
        tile: Tile::new(14, 58),
    }]);
    // Masonry: the Seat grows at the advance, and the gates open.
    let mut m = setup();
    m.ok(vec![
        debug_research(tech(&rules, "letters_1")),
        debug_research(tech(&rules, "arms_1")),
        grant(res(&rules, "lore"), 100),
    ]);
    let events = m.ok(vec![advance()]);
    assert!(events.contains(&SimEvent::AgeAdvanced { player: P0, age: 1 }));
    assert!(events.contains(&SimEvent::TownTierChanged {
        town: BuildingId(1),
        tier: 2
    }));
    assert!(events.contains(&SimEvent::TerritoryChanged));
    assert_eq!((seat_tier(&m), seat_radius(&m)), (2, r2));
    assert_eq!(m.view().territory(), &m.view().territory().recomputed());
    assert!(yard_at(&m).is_ok(), "{:?}", yard_at(&m));
    assert_eq!(m.view().check_build_kind(P0, yard), yard_at(&m));
    m.ok(vec![
        Command::Build {
            unit: UnitId(1),
            kind: yard,
            tile: Tile::new(19, 67),
        },
        train(5, outrider),
    ]);
    // With no age gate in rules.ron the Seat grows in the Hearth Age.
    let mut ungated = rules.clone();
    ungated.town_growth_age = None;
    let mut m = M::standard(ungated, 15);
    m.ok([
        ("croft", 28u16, 58u16),
        ("scriptorium", 28, 67),
        ("muster_hall", 32, 67),
        ("engine_yard", 36, 67),
        ("watchtower", 31, 58),
    ]
    .iter()
    .map(|(id, x, y)| place(bkind(&rules, id), *x, *y, true))
    .collect());
    assert_eq!((seat_tier(&m), seat_radius(&m)), (2, r2));
}

/// Cancelling research entries, the head with progress and one behind it,
/// refunds exactly what each paid when it was queued, not the cheaper
/// quote after a later Letters level, with Lore under its cap so nothing
/// is clamped: the state ends equal to a control that never queued them.
/// At the Lore cap a refund leaves the stockpile at the cap, as for a unit.
#[test]
fn tech_research_cancel_refunds_exactly_what_it_paid() {
    let rules = load_rules();
    let (arms, trade) = (tech(&rules, "arms_1"), tech(&rules, "trade_1"));
    let lore = res(&rules, "lore");
    let cap = i64::from(rules.resources.resources[lore].stockpile_cap.unwrap());
    let setup = || {
        let mut m = M::new(rules.clone(), 16);
        m.ok(vec![
            place(
                bkind(&rules, "scriptorium"),
                SCRIPTORIUM_A.0,
                SCRIPTORIUM_A.1,
                true,
            ),
            grant(res(&rules, "ore"), 2000),
            // Well under Lore's stockpile_cap (999): the cap must not be
            // able to hide a wrong Lore refund.
            grant(lore, 400),
        ]);
        m
    };
    let mut a = setup();
    let mut b = setup();
    let quotes = (
        a.view().research_quote(P0, arms),
        a.view().research_quote(P0, trade),
    );
    let before = a.stock(P0);
    a.ok(vec![research(1, arms), research(1, trade)]);
    b.run(vec![noop(), noop()]);
    let queue = &a.view().building(BuildingId(1)).unwrap().queue;
    let paid = [queue[0].paid.clone(), queue[1].paid.clone()];
    assert_eq!(paid, [quotes.0.0.clone(), quotes.1.0.clone()]);
    let spent: Vec<i64> = b
        .stock(P0)
        .iter()
        .zip(a.stock(P0))
        .map(|(b, a)| b - a)
        .collect();
    let both: Vec<i64> = paid[0].iter().zip(&paid[1]).map(|(x, y)| x + y).collect();
    assert_eq!(spent, both);
    assert!(before.iter().zip(&both).all(|(s, c)| s >= c));
    // Letters I after queueing: the quotes drop (75 Lore + 50 Ore to 67 +
    // 45, 70 Lore to 63); what the entries paid must not.
    let letters = tech(&rules, "letters_1");
    a.ok(vec![debug_research(letters)]);
    b.ok(vec![debug_research(letters)]);
    assert_ne!(a.view().research_quote(P0, arms).0, paid[0]);
    assert_ne!(a.view().research_quote(P0, trade).0, paid[1]);
    a.step(100);
    b.step(100);
    assert!(a.view().building(BuildingId(1)).unwrap().queue[0].progress > 100);
    let cancel = |slot| Command::Cancel {
        building: BuildingId(1),
        slot,
    };
    let (a_pre, b_pre) = (a.stock(P0), b.stock(P0));
    let events = a.ok(vec![cancel(1), cancel(0)]);
    b.run(vec![noop(), noop()]);
    assert!(events.contains(&SimEvent::TrainingCancelled {
        building: BuildingId(1),
        slot: 1
    }));
    assert!(events.contains(&SimEvent::TrainingCancelled {
        building: BuildingId(1),
        slot: 0
    }));
    assert!(a.view().building(BuildingId(1)).unwrap().queue.is_empty());
    assert!(
        a.stock(P0)[lore] < cap && b.stock(P0)[lore] < cap,
        "the cap would hide a wrong Lore refund"
    );
    // What came back is exactly what both entries paid, plus the income
    // both sims earned in the cancel's ticks.
    let delta = |now: Vec<i64>, pre: &[i64]| -> Vec<i64> {
        now.iter().zip(pre).map(|(x, y)| x - y).collect()
    };
    let income = delta(b.stock(P0), &b_pre);
    let want: Vec<i64> = both.iter().zip(&income).map(|(p, i)| p + i).collect();
    assert_eq!(
        delta(a.stock(P0), &a_pre),
        want,
        "exactly the queue-time paid amounts back"
    );
    assert_eq!(a.stock(P0), b.stock(P0));
    assert_eq!(a.sim.sub_hashes(), b.sim.sub_hashes());
    assert_eq!(
        a.view().researched(P0).len(),
        1,
        "only the debug Letters I: nothing completed"
    );
    // At the cap: the stockpile stays at the cap, not cap + paid.
    let mut a = lab(&rules, 16);
    let mut b = lab(&rules, 16);
    a.ok(vec![research(1, trade)]);
    b.run(vec![noop()]);
    let paid = a.view().building(BuildingId(1)).unwrap().queue[0].paid[lore];
    a.ok(vec![grant(lore, u32::try_from(paid).unwrap())]);
    b.ok(vec![grant(lore, u32::try_from(paid).unwrap())]);
    assert_eq!((a.stock(P0)[lore], b.stock(P0)[lore]), (cap, cap));
    let events = a.ok(vec![cancel(0)]);
    b.run(vec![noop()]);
    assert!(events.contains(&SimEvent::TrainingCancelled {
        building: BuildingId(1),
        slot: 0
    }));
    assert_eq!(a.stock(P0)[lore], cap, "clamped, not {}", cap + paid);
    assert_eq!(a.stock(P0), b.stock(P0));
    assert_eq!(a.sim.sub_hashes(), b.sim.sub_hashes());
}

/// Research time is fixed when the entry is queued: Letters researched
/// while an entry waits speeds only entries queued afterwards.
#[test]
fn tech_research_time_is_fixed_when_queued() {
    let rules = load_rules();
    let (arms, trade) = (tech(&rules, "arms_1"), tech(&rules, "trade_1"));
    let mut m = lab(&rules, 17);
    let slow = m.view().research_quote(P0, arms).1;
    assert_eq!(slow, rules.ticks_from_ds(300));
    m.ok(vec![research(1, arms)]);
    let started = m.sim.tick() - 1;
    m.ok(vec![debug_research(tech(&rules, "letters_1"))]);
    let fast = m.view().research_quote(P0, trade).1;
    assert_eq!(fast, rules.ticks_from_ds(270));
    m.ok(vec![research(1, trade)]);
    let queue = &m.view().building(BuildingId(1)).unwrap().queue;
    assert_eq!(
        queue.iter().map(|e| e.item).collect::<Vec<_>>(),
        [
            QueueItem::Research {
                tech: arms,
                ticks: slow
            },
            QueueItem::Research {
                tech: trade,
                ticks: fast
            }
        ]
    );
    // Arms I completes in the slow-th step from its first; Trade I starts
    // in the next step and completes fast steps later.
    let mut done = Vec::new();
    while done.len() < 2 {
        for e in m.step(1) {
            if let SimEvent::ResearchCompleted { tech, .. } = e {
                done.push((tech, m.sim.tick() - 1));
            }
        }
        assert!(m.sim.tick() < started + 3000);
    }
    assert_eq!(
        done,
        [
            (arms, started + slow - 1),
            (trade, started + slow + fast - 1)
        ]
    );
}

/// Research entries take no pop and count toward no unit ramp, even for a
/// kind (here the Scribe, made Military in memory) trained at the host.
#[test]
fn tech_queued_research_takes_no_pop_and_no_military_ramp() {
    let mut rules = load_rules();
    let scribe = ukind(&rules, "scribe");
    rules.units[usize::from(scribe.0)].ramp = UnitRamp::Military;
    let setup = || {
        let mut m = lab(&rules, 18);
        let s = bkind(&rules, "scriptorium");
        m.ok(vec![place(s, SCRIPTORIUM_B.0, SCRIPTORIUM_B.1, true)]);
        m.ok(vec![train(2, scribe)]);
        m
    };
    let mut a = setup();
    let mut b = setup();
    a.ok(vec![
        research(1, tech(&rules, "letters_1")),
        research(1, tech(&rules, "trade_1")),
        research(2, tech(&rules, "arms_1")),
    ]);
    b.run(vec![noop(), noop(), noop()]);
    assert_eq!(a.view().building(BuildingId(1)).unwrap().queue.len(), 2);
    assert_eq!(a.view().pop(P0), b.view().pop(P0));
    assert_eq!(a.view().pop(P0).0, 1, "the queued Scribe only");
    for (k, u) in rules.units.iter().enumerate() {
        let k = UnitKindId(u16::try_from(k).unwrap());
        assert_eq!(
            a.view().unit_cost(P0, k),
            b.view().unit_cost(P0, k),
            "{}",
            u.id
        );
    }
    // The queued Scribe does raise the (in-memory) ramp: the check works.
    let mut c = setup();
    let before = c.view().unit_cost(P0, scribe);
    c.ok(vec![train(2, scribe)]);
    assert_ne!(c.view().unit_cost(P0, scribe), before);
}

/// `DebugResearch` completes a tech at once, but never one that is
/// researched or queued, and only when debug commands are on.
#[test]
fn tech_debug_research_rejects_a_queued_tech() {
    let rules = load_rules();
    let letters = tech(&rules, "letters_1");
    let queued = || {
        let mut m = lab(&rules, 19);
        m.ok(vec![research(1, letters)]);
        m
    };
    rejected_without_change(
        "queued",
        &queued,
        0,
        debug_research(letters),
        RejectReason::ResearchInProgress,
    );
    rejected_without_change(
        "researched",
        &|| {
            let mut m = M::new(rules.clone(), 19);
            m.ok(vec![debug_research(letters)]);
            m
        },
        0,
        debug_research(letters),
        RejectReason::AlreadyResearched,
    );
    rejected_without_change(
        "unknown tech",
        &|| M::new(rules.clone(), 19),
        0,
        debug_research(TechId(99)),
        RejectReason::UnknownTech,
    );
    rejected_without_change(
        "unknown owner",
        &|| M::new(rules.clone(), 19),
        0,
        Command::DebugResearch {
            owner: PlayerId(7),
            tech: letters,
        },
        RejectReason::UnknownPlayer,
    );
    // Debug commands off (a skirmish): rejected, changing nothing.
    let skirmish = || M {
        sim: Sim::new(
            MatchSetup::skirmish(&rules, 19),
            rules.clone(),
            Box::new(NoAi),
        ),
        seq: [0; 2],
    };
    rejected_without_change(
        "debug commands off",
        &skirmish,
        0,
        debug_research(letters),
        RejectReason::DebugCommandsDisabled,
    );
    let mut m = skirmish();
    m.run(vec![debug_research(letters)]);
    assert!(m.view().researched(P0).is_empty());
    // The queued one completes exactly once, at its host.
    let mut m = queued();
    let (_, ticks) = m.view().research_quote(P0, letters);
    let events = m.step(ticks);
    let completions = events
        .iter()
        .filter(|e| matches!(e, SimEvent::ResearchCompleted { .. }))
        .count();
    assert_eq!(completions, 1);
    assert!(m.view().researched(P0).contains(&letters));
    // DebugResearch skips host, cost, prerequisite and age.
    let mut m = M::new(rules.clone(), 19);
    let events = m.ok(vec![debug_research(tech(&rules, "trade_3"))]);
    assert!(events.contains(&SimEvent::ResearchCompleted {
        player: P0,
        tech: tech(&rules, "trade_3")
    }));
    assert_eq!(m.stock(P0), vec![0; 4]);
}

/// Snapshot, restore into a fresh sim and keep going: every later tick
/// hashes the same as the uninterrupted run, every sub-hash included. The
/// first restore has research half done and no tech complete, and goes
/// through an `AdvanceAge`, further research and a queue `Cancel`. The
/// second comes after the `AdvanceAge`, with Letters I and Trade I
/// researched and Arms I in flight, before the `Research` of Letters II,
/// which needs the restored age, the restored Letters I and its price.
#[test]
fn tech_restore_mid_research_matches_uninterrupted() {
    let rules = load_rules();
    let t = |id| tech(&rules, id);
    let s = bkind(&rules, "scriptorium");
    let cmd = |seq: u32, c: Command| PlayerCommand::new(P0, seq, c);
    let stream: Vec<(u32, Vec<PlayerCommand>)> = vec![
        (
            0,
            vec![
                cmd(0, place(s, SCRIPTORIUM_A.0, SCRIPTORIUM_A.1, true)),
                cmd(1, place(s, SCRIPTORIUM_B.0, SCRIPTORIUM_B.1, true)),
                cmd(2, grant(res(&rules, "grain"), 2000)),
                cmd(3, grant(res(&rules, "lumber"), 2000)),
                cmd(4, grant(res(&rules, "ore"), 2000)),
                cmd(5, grant(res(&rules, "lore"), 999)),
            ],
        ),
        (
            5,
            vec![
                cmd(6, research(1, t("letters_1"))),
                cmd(7, research(2, t("trade_1"))),
                cmd(8, research(1, t("arms_1"))),
            ],
        ),
        (650, vec![cmd(9, advance())]),
        (
            700,
            vec![
                cmd(10, research(2, t("letters_2"))),
                cmd(11, research(2, t("statecraft_1"))),
            ],
        ),
        (
            800,
            vec![cmd(
                12,
                Command::Cancel {
                    building: BuildingId(2),
                    slot: 1,
                },
            )],
        ),
    ];
    let at = |tick: u32| -> Vec<PlayerCommand> {
        stream
            .iter()
            .find(|(t, _)| *t == tick)
            .map(|(_, c)| c.clone())
            .unwrap_or_default()
    };
    let letters_2 = t("letters_2");
    for snap in [300u32, 680] {
        let mut a = M::new(rules.clone(), 20).sim;
        for tick in 0..snap {
            a.step(&at(tick));
        }
        a.drain_events();
        let q = &a.view().building(BuildingId(1)).unwrap().queue;
        if snap == 300 {
            assert!(q[0].progress > 100 && q.len() == 2, "research half done");
            assert!(a.view().researched(P0).is_empty());
        } else {
            // The AdvanceAge applied during tick 652.
            assert!(q[0].progress > 100 && q.len() == 1, "Arms I in flight");
            assert_eq!(a.view().age(P0), 1);
            assert_eq!(
                a.view().researched(P0),
                &std::collections::BTreeSet::from([t("letters_1"), t("trade_1")])
            );
            assert!(
                a.view()
                    .check_research(P0, BuildingId(2), letters_2)
                    .is_ok()
            );
        }
        let mut b = M::new(rules.clone(), 20).sim;
        b.restore(&a.snapshot()).unwrap();
        assert_eq!(a.hash(), b.hash());
        assert_eq!(a.sub_hashes(), b.sub_hashes());
        for p in [P0, P1] {
            assert_eq!(b.view().age(p), a.view().age(p), "restore at {snap}");
            assert_eq!(
                b.view().researched(p),
                a.view().researched(p),
                "restore at {snap}"
            );
            assert_eq!(
                b.view().research_quote(p, letters_2),
                a.view().research_quote(p, letters_2),
                "restore at {snap}: the Letters price"
            );
            assert_eq!(
                b.view().check_research(p, BuildingId(2), letters_2),
                a.view().check_research(p, BuildingId(2), letters_2),
                "restore at {snap}"
            );
        }
        let mut advanced = false;
        for tick in snap..1500 {
            let cmds = at(tick);
            a.step(&cmds);
            b.step(&cmds);
            assert_eq!(a.hash(), b.hash(), "restore at {snap}, tick {tick}");
            assert_eq!(
                a.sub_hashes(),
                b.sub_hashes(),
                "restore at {snap}, tick {tick}"
            );
            advanced |= a
                .drain_events()
                .contains(&SimEvent::AgeAdvanced { player: P0, age: 1 });
            b.drain_events();
        }
        assert_eq!(
            advanced,
            snap == 300,
            "the stream advances an age after the first restore only"
        );
        let done: Vec<TechId> = a.view().researched(P0).iter().copied().collect();
        assert_eq!(
            done,
            [t("arms_1"), t("trade_1"), t("letters_1"), letters_2],
            "statecraft_1 was cancelled"
        );
        assert_eq!(b.view().researched(P0), a.view().researched(P0));
    }
}

/// `SimView::check_research` and `check_advance_age` return exactly what
/// applying the command does: the rejection reason, or the quote it
/// deducts (and for research the ticks it queues).
#[test]
fn tech_view_checks_agree_with_apply() {
    let rules = load_rules();
    let t = |id| tech(&rules, id);
    type Setup<'a> = Box<dyn Fn() -> M + 'a>;
    let lab_ = || lab(&rules, 21);
    let lettered: Setup = Box::new(|| {
        let mut m = lab(&rules, 21);
        m.ok(vec![
            debug_research(t("letters_1")),
            debug_research(t("trade_1")),
        ]);
        m
    });
    let surrendered: Setup = Box::new(|| {
        let mut m = lab(&rules, 21);
        m.run_as(1, vec![Command::Surrender]);
        m
    });
    let research_cases: Vec<(&str, &Setup, u8, u32, TechId)> = vec![
        ("fresh", &lettered, 0, 1, t("arms_1")),
        ("already", &lettered, 0, 1, t("letters_1")),
        ("age", &lettered, 0, 1, t("letters_2")),
        ("no building", &lettered, 0, 9, t("arms_1")),
        ("unknown tech", &lettered, 0, 1, TechId(400)),
        ("unknown player", &lettered, 7, 1, t("arms_1")),
        ("surrendered", &surrendered, 1, 1, t("arms_1")),
    ];
    for (name, setup, p, b, tech) in research_cases {
        let mut m = setup();
        let mut control = setup();
        let check = m.view().check_research(PlayerId(p), BuildingId(b), tech);
        let events = m.run_as(p, vec![research(b, tech)]);
        control.run_as(p, vec![noop()]);
        match check {
            Err(reason) => assert_eq!(rejections(&events), [reason], "{name}"),
            Ok((cost, ticks)) => {
                assert!(rejections(&events).is_empty(), "{name}: {events:?}");
                let pid = PlayerId(p);
                let paid: Vec<i64> = control
                    .stock(pid)
                    .iter()
                    .zip(m.stock(pid))
                    .map(|(c, s)| c - s)
                    .collect();
                assert_eq!(paid, cost, "{name}");
                let entry = m
                    .view()
                    .building(BuildingId(b))
                    .unwrap()
                    .queue
                    .last()
                    .cloned();
                assert_eq!(
                    entry.map(|e| e.item),
                    Some(QueueItem::Research { tech, ticks }),
                    "{name}"
                );
            }
        }
    }
    let rich_two: Setup = Box::new(|| {
        let mut m = lettered();
        m.ok(vec![grant(res(&rules, "grain"), 300)]);
        m
    });
    let plain_lab: Setup = Box::new(lab_);
    let advance_cases: Vec<(&str, &Setup, u8)> = vec![
        ("affordable", &rich_two, 0),
        ("P1 without techs", &rich_two, 1),
        ("no techs", &plain_lab, 0),
        ("unknown player", &rich_two, 7),
        ("surrendered", &surrendered, 1),
    ];
    for (name, setup, p) in advance_cases {
        let mut m = setup();
        let mut control = setup();
        let pid = PlayerId(p);
        let check = m.view().check_advance_age(pid);
        let events = m.run_as(p, vec![Command::AdvanceAge { player: pid }]);
        control.run_as(p, vec![noop()]);
        match check {
            Err(reason) => assert_eq!(rejections(&events), [reason], "{name}"),
            Ok(cost) => {
                assert!(rejections(&events).is_empty(), "{name}: {events:?}");
                let paid: Vec<i64> = control
                    .stock(pid)
                    .iter()
                    .zip(m.stock(pid))
                    .map(|(c, s)| c - s)
                    .collect();
                assert_eq!(paid, cost, "{name}");
                assert_eq!(m.view().age(pid), control.view().age(pid) + 1, "{name}");
            }
        }
    }
    // A site is UnderConstruction in both.
    let mut m = lab_();
    m.ok(vec![place(
        bkind(&rules, "scriptorium"),
        SCRIPTORIUM_B.0,
        SCRIPTORIUM_B.1,
        false,
    )]);
    assert_eq!(
        m.view().check_research(P0, BuildingId(2), t("letters_1")),
        Err(RejectReason::UnderConstruction)
    );
    let events = m.run(vec![research(2, t("letters_1"))]);
    assert_eq!(rejections(&events), [RejectReason::UnderConstruction]);
}

// ---------------------------------------------------------------------------
// Combat core: hit points, the explicit Attack, simultaneous hits, death and
// destruction (`combat_*`), and the Arms hit point and damage rows.
// ---------------------------------------------------------------------------

fn spawn_for(owner: PlayerId, kind: UnitKindId, x: i32, y: i32, count: u16) -> Command {
    Command::DebugSpawn {
        owner,
        kind,
        at: FxVec2::from_ints(x, y),
        count,
    }
}

fn attack(units: &[u32], target: TargetId) -> Command {
    Command::Attack {
        units: units.iter().map(|&u| UnitId(u)).collect(),
        target,
    }
}

fn unit_t(id: u32) -> TargetId {
    TargetId::Unit(UnitId(id))
}

fn building_t(id: u32) -> TargetId {
    TargetId::Building(BuildingId(id))
}

fn stop(units: &[u32]) -> Command {
    Command::Stop {
        units: units.iter().map(|&u| UnitId(u)).collect(),
    }
}

/// `(attacker, target, damage)` of every `Hit`.
fn hits(events: &[SimEvent]) -> Vec<(TargetId, TargetId, i32)> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::Hit {
                attacker,
                target,
                damage,
            } => Some((*attacker, *target, *damage)),
            _ => None,
        })
        .collect()
}

fn deaths(events: &[SimEvent]) -> Vec<UnitId> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::UnitDied { unit, .. } => Some(*unit),
            _ => None,
        })
        .collect()
}

fn destroyed(events: &[SimEvent]) -> Vec<BuildingId> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::BuildingDestroyed { building, .. } => Some(*building),
            _ => None,
        })
        .collect()
}

impl M {
    /// Issue `p0` from P0 and `p1` from P1 this tick, then step one tick
    /// at a time through the tick they apply in and `n` ticks more: every
    /// event with the tick it happened in.
    fn issue_tagged(&mut self, p0: Vec<Command>, p1: Vec<Command>, n: u32) -> Vec<(u32, SimEvent)> {
        let mut pcs: Vec<PlayerCommand> = p0.into_iter().map(|c| self.stamp(0, c)).collect();
        pcs.extend(p1.into_iter().map(|c| self.stamp(1, c)));
        let tick = self.sim.tick();
        self.sim.step(&pcs);
        let mut out: Vec<(u32, SimEvent)> = self
            .sim
            .drain_events()
            .into_iter()
            .map(|e| (tick, e))
            .collect();
        let delay = self.sim.setup().cmd_delay;
        out.extend(self.step_tagged(delay + n));
        assert!(
            out.iter()
                .all(|(_, e)| !matches!(e, SimEvent::CommandRejected { .. })),
            "{out:?}"
        );
        out
    }

    /// Step one tick at a time, `n` ticks: every event with the tick it
    /// happened in.
    fn step_tagged(&mut self, n: u32) -> Vec<(u32, SimEvent)> {
        let mut out = Vec::new();
        for _ in 0..n {
            let tick = self.sim.tick();
            self.sim.step(&[]);
            out.extend(self.sim.drain_events().into_iter().map(|e| (tick, e)));
        }
        out
    }

    fn unit(&self, id: u32) -> Option<sim::Unit> {
        self.view().unit(UnitId(id)).cloned()
    }

    fn hp(&self, id: u32) -> Option<(i32, i32)> {
        self.view().unit_hp(UnitId(id))
    }
}

/// `(tick, damage)` of every hit by `attacker` among tagged events.
fn hits_by(events: &[(u32, SimEvent)], attacker: TargetId) -> Vec<(u32, i32)> {
    events
        .iter()
        .filter_map(|(t, e)| match e {
            SimEvent::Hit {
                attacker: a,
                damage,
                ..
            } if *a == attacker => Some((*t, *damage)),
            _ => None,
        })
        .collect()
}

/// `(tick, damage)` of every hit by `attacker` on `target` among tagged
/// events.
fn hits_on(events: &[(u32, SimEvent)], attacker: TargetId, target: TargetId) -> Vec<(u32, i32)> {
    events
        .iter()
        .filter_map(|(t, e)| match e {
            SimEvent::Hit {
                attacker: a,
                target: d,
                damage,
            } if (*a, *d) == (attacker, target) => Some((*t, *damage)),
            _ => None,
        })
        .collect()
}

/// The tick a unit died in, among tagged events.
fn died_at(events: &[(u32, SimEvent)], unit: u32) -> Option<u32> {
    events.iter().find_map(|(t, e)| match e {
        SimEvent::UnitDied { unit: u, .. } if *u == UnitId(unit) => Some(*t),
        _ => None,
    })
}

/// Consecutive differences.
fn gaps(ticks: &[u32]) -> Vec<u32> {
    ticks.windows(2).map(|w| w[1] - w[0]).collect()
}

/// A hit does `floor(floor(raw * counter / 1000) * variance / 1000)` with
/// variance 1000 by default: the attacker's damage with its owner's Arms
/// rows, the counter row's permille, floored, once per cooldown. Checked
/// against the data (Skirmisher on Bowman 2500, on buildings 250; the
/// Shieldbearer on a Yeoman has no row) with P0 at Arms I, where the
/// products are not multiples of 1000.
#[test]
fn combat_damage_is_raw_times_counter_floored_every_cooldown() {
    let rules = load_rules();
    let (skirm, bow, shield, yeoman) = (
        ukind(&rules, "skirmisher"),
        ukind(&rules, "bowman"),
        ukind(&rules, "shieldbearer"),
        ukind(&rules, "yeoman"),
    );
    let mut m = M::new(rules.clone(), 30);
    assert_eq!(m.view().unit_stats(P0, skirm).attack.unwrap().damage, 10);
    m.ok(vec![debug_research(tech(&rules, "arms_1"))]);
    let stats = |m: &M, k| m.view().unit_stats(P0, k).attack.expect("the kind attacks");
    let (raw_skirm, raw_shield) = (
        i64::from(stats(&m, skirm).damage),
        i64::from(stats(&m, shield).damage),
    );
    assert_eq!(
        (raw_skirm, raw_shield),
        (11, 13),
        "Arms I: 10 -> 11, 12 -> 13"
    );
    let counter = |a: &str, d: rules::Defender| i64::from(rules.counter_permille(a, &d));
    let vs_bow = raw_skirm * counter("skirmisher", rules::Defender::Kind("bowman".into())) / 1000;
    let vs_building = raw_skirm * counter("skirmisher", rules::Defender::Buildings) / 1000;
    let vs_yeoman =
        raw_shield * counter("shieldbearer", rules::Defender::Kind("yeoman".into())) / 1000;
    assert_eq!(
        (vs_bow, vs_building, vs_yeoman),
        (27, 2, 13),
        "27.5, 2.75, 13"
    );
    let (cd_skirm, cd_shield) = (
        stats(&m, skirm).cooldown_ticks,
        stats(&m, shield).cooldown_ticks,
    );
    assert_eq!((cd_skirm, cd_shield), (20, 24), "10 ds and 12 ds at 20 Hz");
    let damages = |h: &[(u32, i32)]| h.iter().map(|h| i64::from(h.1)).collect::<Vec<_>>();
    let ticks = |h: &[(u32, i32)]| h.iter().map(|h| h.0).collect::<Vec<_>>();

    // P1's Bowman is UnitId(1) and its complete Croft BuildingId(1); P0's
    // Skirmisher UnitId(2) is spawned a tile from the Bowman in the tick its
    // `Attack` applies, so it fights under that order from its first tick.
    // (Idle armed units acquire enemies in reach: the Bowman shoots back.)
    m.run_as(
        1,
        vec![
            spawn_for(P1, bow, 11, 30, 1),
            place_for(P1, bkind(&rules, "croft"), 14, 26, true),
        ],
    );
    let events = m.issue_tagged(
        vec![spawn(skirm, 10, 30, 1), attack(&[2], unit_t(1))],
        vec![],
        60,
    );
    let on_bow = hits_on(&events, unit_t(2), unit_t(1));
    assert_eq!(damages(&on_bow), [vs_bow; 2], "46 hp: two hits");
    assert_eq!(gaps(&ticks(&on_bow)), [cd_skirm]);
    assert_eq!(
        died_at(&events, 1),
        Some(on_bow[1].0),
        "dies at the second hit"
    );
    // The order ended; idle again, the Skirmisher guards and takes the
    // enemy Croft in its reach.
    let u = m.unit(2).unwrap();
    assert_eq!(
        (u.target, u.intent),
        (Some(building_t(1)), Intent::Plain),
        "the order ended"
    );
    let guarded = hits_on(&events, unit_t(2), building_t(1));

    let events = m.issue_tagged(vec![attack(&[2], building_t(1))], vec![], 140);
    let on_croft = hits_on(&events, unit_t(2), building_t(1));
    assert!(on_croft.len() >= 5, "{on_croft:?}");
    assert!(damages(&on_croft).iter().all(|d| *d == vs_building));
    assert!(gaps(&ticks(&on_croft)).iter().all(|g| *g == cd_skirm));
    assert_eq!(hits_by(&events, unit_t(2)), on_croft, "only on the Croft");
    let n = i32::try_from(guarded.len() + on_croft.len()).unwrap();
    assert!(damages(&guarded).iter().all(|d| *d == vs_building));
    assert_eq!(
        m.view().building(BuildingId(1)).unwrap().hp,
        300 - 2 * n,
        "a building loses exactly what hit it"
    );

    // P0's Shieldbearer (UnitId(4)) on P1's Yeoman (UnitId(3)): 13 a hit,
    // 40 hp, so the fourth hit kills, 24 ticks apart.
    m.ok(vec![stop(&[2])]);
    m.run_as(1, vec![spawn_for(P1, yeoman, 11, 36, 1)]);
    let events = m.issue_tagged(
        vec![spawn(shield, 10, 36, 1), attack(&[4], unit_t(3))],
        vec![],
        120,
    );
    let on_yeoman = hits_by(&events, unit_t(4));
    assert_eq!(damages(&on_yeoman), [vs_yeoman; 4]);
    assert_eq!(gaps(&ticks(&on_yeoman)), [cd_shield; 3]);
    assert_eq!(died_at(&events, 3), Some(on_yeoman[3].0));
    eprintln!(
        "damage: skirmisher (Arms I) on bowman {vs_bow} x2 every {cd_skirm} ticks, on a croft {vs_building}; shieldbearer (Arms I) on yeoman {vs_yeoman} x4 every {cd_shield} ticks"
    );
}

/// Two equal Skirmishers attack each other: every hit of a tick is collected
/// from the post-movement positions before any is applied, so both land
/// their eighth hit and die in the same tick, whichever has the lower id.
/// Three ways in: P0's unit spawned first (the lower id) and P1's beside it
/// a few ticks later, or the other way round, the two acquiring each other
/// the moment the second appears; and P1's unit first with both ordered to
/// `Attack` in the tick P0's appears (P0's commands apply first, so its
/// order on P1's unit is valid only that way round).
#[test]
fn combat_hits_resolve_simultaneously_without_id_bias() {
    let rules = load_rules();
    let skirm = ukind(&rules, "skirmisher");
    let mut outcomes = Vec::new();
    for (p0_first, explicit) in [(true, false), (false, false), (false, true)] {
        let mut m = M::new(rules.clone(), 31);
        // The first spawn is UnitId(1), the second UnitId(2).
        let (p0_id, p1_id) = if p0_first { (1, 2) } else { (2, 1) };
        let events = if p0_first {
            m.ok(vec![spawn(skirm, 10, 30, 1)]);
            m.issue_tagged(vec![], vec![spawn_for(P1, skirm, 11, 30, 1)], 200)
        } else {
            m.run_as(1, vec![spawn_for(P1, skirm, 11, 30, 1)]);
            let (p0, p1) = if explicit {
                (
                    vec![spawn(skirm, 10, 30, 1), attack(&[p0_id], unit_t(p1_id))],
                    vec![attack(&[p1_id], unit_t(p0_id))],
                )
            } else {
                (vec![spawn(skirm, 10, 30, 1)], vec![])
            };
            m.issue_tagged(p0, p1, 200)
        };
        let a = hits_by(&events, unit_t(p0_id));
        let b = hits_by(&events, unit_t(p1_id));
        assert_eq!(a.len(), 8, "80 hp at 10 a hit");
        assert_eq!(a, b, "same ticks, same damage");
        let (da, db) = (died_at(&events, p0_id), died_at(&events, p1_id));
        assert!(da.is_some() && da == db, "both die in the same tick");
        assert_eq!(da, Some(a[7].0), "in the tick of their last hits");
        // The tick, relative to the first hit, does not depend on the ids.
        outcomes.push((a[7].0 - a[0].0, a.len()));
    }
    assert!(
        outcomes.windows(2).all(|w| w[0] == w[1]),
        "no first strike for the lower id: {outcomes:?}"
    );
}

/// With `damage_variance_permille` 0 (the shipped value) hits draw
/// nothing: the `rng` sub-hash is the same on every tick of this fight.
/// (The only draw default combat makes is an exact tie when a unit
/// acquires a target or a shooter aims, which this fight never meets:
/// `combat_acquires_the_nearest_enemy_and_breaks_exact_ties_with_the_rng`.)
#[test]
fn combat_zero_variance_draws_no_randomness() {
    let rules = load_rules();
    assert_eq!(rules.damage_variance_permille, 0);
    let (skirm, bow) = (ukind(&rules, "skirmisher"), ukind(&rules, "bowman"));
    let mut m = M::new(rules.clone(), 32);
    // P0 Skirmishers 1..=3, P1 Bowmen 4..=6.
    m.ok(vec![spawn(skirm, 10, 30, 3)]);
    m.run_as(1, vec![spawn_for(P1, bow, 13, 30, 3)]);
    let rng = m.sim.sub_hashes().rng;
    let mut count = 0;
    let p0 = vec![
        attack(&[1], unit_t(4)),
        attack(&[2], unit_t(5)),
        attack(&[3], unit_t(6)),
    ];
    let p1 = vec![attack(&[4, 5, 6], unit_t(1))];
    let events = m.issue_tagged(p0, p1, 0);
    count += hits_by(&events, unit_t(1)).len();
    for _ in 0..200 {
        m.sim.step(&[]);
        count += hits(&m.sim.drain_events()).len();
        assert_eq!(m.sim.sub_hashes().rng, rng, "no draw");
    }
    assert!(count > 0 && m.view().unit_count() < 6, "they fought");
}

/// `damage_variance_permille` 100, set in memory: each hit draws once from
/// the sim's RNG and lands within 900..=1100 permille of the floored
/// counter damage. The same seed gives the same hits and hashes; another
/// seed gives other hits.
#[test]
fn combat_variance_is_seeded_and_reproducible() {
    let mut rules = load_rules();
    rules.damage_variance_permille = 100;
    let (skirm, shield) = (ukind(&rules, "skirmisher"), ukind(&rules, "shieldbearer"));
    let fight = |seed: u64| {
        let mut m = M::new(rules.clone(), seed);
        m.ok(vec![spawn(skirm, 10, 30, 1)]);
        m.run_as(1, vec![spawn_for(P1, shield, 11, 30, 1)]);
        let rng = m.sim.sub_hashes().rng;
        let events = m.issue_tagged(vec![attack(&[1], unit_t(2))], vec![], 400);
        assert_ne!(m.sim.sub_hashes().rng, rng, "hits drew");
        let damage: Vec<i32> = hits_by(&events, unit_t(1)).iter().map(|h| h.1).collect();
        (damage, m.sim.hash())
    };
    let (a, hash_a) = fight(5);
    let (b, hash_b) = fight(5);
    assert_eq!((&a, hash_a), (&b, hash_b), "same seed, same fight");
    // 10 a hit, no counter row: floor(10 * v / 1000) for v in 900..=1100.
    assert!(a.iter().all(|d| (9..=11).contains(d)), "{a:?}");
    assert!(
        a.iter().collect::<std::collections::BTreeSet<_>>().len() > 1,
        "{a:?}"
    );
    let (c, _) = fight(6);
    assert_ne!(a, c, "another seed, other draws");
    eprintln!("variance 100: seed 5 {a:?}, seed 6 {c:?}");
}

/// P1's complete Town at `TOWN` brought to 0 HP by 17 Mangonels (120 a hit
/// on buildings, 2040 against 2000 HP) in one volley: P0's Mangonels are
/// `UnitId(1..=17)`, the Town `BuildingId(1)`. The Mangonels are spawned in
/// the tick their `Attack` applies (idle, they would take the Town on their
/// own); the Town shoots one of them in the same tick, its hit landing with
/// theirs.
fn town_at_zero(rules: &Rules, seed: u64) -> M {
    let mut m = M::new(rules.clone(), seed);
    m.run_as(
        1,
        vec![place_for(P1, bkind(rules, "town"), TOWN.0, TOWN.1, true)],
    );
    let ids: Vec<u32> = (1..=17).collect();
    let events = m.ok(vec![
        spawn(ukind(rules, "mangonel"), 15, 31, 17),
        attack(&ids, building_t(1)),
    ]);
    let volley: Vec<_> = hits(&events)
        .into_iter()
        .filter(|(a, _, _)| matches!(a, TargetId::Unit(_)))
        .collect();
    assert_eq!(volley.len(), 17, "one volley");
    assert_eq!(hits(&events).len(), 18, "and one shot from the Town");
    assert!(events.contains(&SimEvent::TownAtZeroHp {
        town: BuildingId(1)
    }));
    m
}

/// Every `Attack` rejection, first failure wins (the target, then the
/// units), changes nothing, the `rng` sub-hash included: an `Attack` no
/// longer draws from the RNG. `SimView::check_attack` agrees.
#[test]
fn combat_attack_rejections_change_nothing() {
    let rules = load_rules();
    let (skirm, yeoman) = (ukind(&rules, "skirmisher"), ukind(&rules, "yeoman"));
    // P0: Skirmisher 1, Yeoman 2. P1: Skirmisher 3. P0's Croft is
    // BuildingId(1), P1's BuildingId(2).
    let field = || {
        let mut m = M::new(rules.clone(), 33);
        m.ok(vec![
            spawn(skirm, 10, 30, 1),
            spawn(yeoman, 10, 32, 1),
            place(bkind(&rules, "croft"), 4, 26, true),
        ]);
        m.run_as(
            1,
            vec![
                spawn_for(P1, skirm, 20, 30, 1),
                place_for(P1, bkind(&rules, "croft"), 24, 26, true),
            ],
        );
        m
    };
    let cases: Vec<(&str, Vec<u32>, TargetId, RejectReason)> = vec![
        (
            "no such unit",
            vec![1],
            unit_t(99),
            RejectReason::InvalidTarget,
        ),
        (
            "no such building",
            vec![1],
            building_t(99),
            RejectReason::InvalidTarget,
        ),
        ("own unit", vec![1], unit_t(2), RejectReason::InvalidTarget),
        (
            "own building",
            vec![1],
            building_t(1),
            RejectReason::InvalidTarget,
        ),
        (
            "target before units",
            vec![2, 99],
            unit_t(2),
            RejectReason::InvalidTarget,
        ),
        (
            "an unarmed Yeoman",
            vec![2],
            unit_t(3),
            RejectReason::NoValidUnits,
        ),
        (
            "another player's unit",
            vec![3],
            building_t(2),
            RejectReason::NoValidUnits,
        ),
        (
            "no such unit",
            vec![99],
            unit_t(3),
            RejectReason::NoValidUnits,
        ),
        ("no units", vec![], unit_t(3), RejectReason::NoValidUnits),
    ];
    for (name, units, target, want) in cases {
        let m = field();
        assert_eq!(
            m.view().check_attack(
                P0,
                &units.iter().map(|&u| UnitId(u)).collect::<Vec<_>>(),
                target
            ),
            Err(want),
            "{name}"
        );
        rejected_without_change(name, &field, 0, attack(&units, target), want);
    }
    // A complete Town at 0 HP is no target.
    let m = town_at_zero(&rules, 33);
    assert_eq!(
        m.view().check_attack(P0, &[UnitId(1)], building_t(1)),
        Err(RejectReason::InvalidTarget)
    );
    rejected_without_change(
        "a Town at 0 HP",
        &|| town_at_zero(&rules, 33),
        0,
        attack(&[1], building_t(1)),
        RejectReason::InvalidTarget,
    );
    // The accepted case agrees with the view: armed own units, deduplicated.
    let mut m = field();
    assert_eq!(
        m.view()
            .check_attack(P0, &[UnitId(2), UnitId(1), UnitId(1)], unit_t(3)),
        Ok(vec![UnitId(1)])
    );
    m.ok(vec![attack(&[2, 1, 1], unit_t(3))]);
    let (a, y) = (m.unit(1).unwrap(), m.unit(2).unwrap());
    assert_eq!((a.target, a.intent), (Some(unit_t(3)), Intent::Attack));
    assert_eq!(
        (y.target, y.intent),
        (None, Intent::Plain),
        "the Yeoman stays out"
    );
}

/// `Stop` clears the target and the order: no hit follows. `Move` does the
/// same. Two Skirmishers walking a path chase toward a Shieldbearer 20
/// tiles off are stopped and moved away: no enemy is within their reach,
/// so nothing more happens. In reach, the explicit order still ends, but a
/// stopped unit is idle, so it guards: it takes the nearest enemy again
/// (with `Intent::Plain`), while a moved one never engages on its way.
#[test]
fn combat_stop_clears_the_target() {
    let rules = load_rules();
    let (skirm, shield) = (ukind(&rules, "skirmisher"), ukind(&rules, "shieldbearer"));
    let mv = |units: &[u32], to: (i32, i32)| Command::Move {
        units: units.iter().map(|&u| UnitId(u)).collect(),
        target: FxVec2::from_ints(to.0, to.1),
        queue: false,
    };

    // Out of reach: P1's Shieldbearer is UnitId(1), 20 tiles east of P0's
    // Skirmishers 2 and 3.
    let mut m = M::new(rules.clone(), 34);
    m.run_as(1, vec![spawn_for(P1, shield, 30, 30, 1)]);
    m.issue_tagged(
        vec![spawn(skirm, 10, 30, 2), attack(&[2, 3], unit_t(1))],
        vec![],
        20,
    );
    for id in [2, 3] {
        let u = m.unit(id).unwrap();
        assert!(
            u.intent == Intent::Attack && u.order.is_some(),
            "unit {id} walks a path chase: {u:?}"
        );
    }
    m.ok(vec![stop(&[2]), mv(&[3], (4, 36))]);
    for id in [2, 3] {
        let u = m.unit(id).unwrap();
        assert_eq!((u.target, u.intent), (None, Intent::Plain), "unit {id}");
    }
    assert!(m.unit(2).unwrap().order.is_none());
    assert!(!m.view().pathing().has_request(UnitId(2)));
    let later = m.step(300);
    assert!(hits(&later).is_empty(), "{later:?}");
    assert_eq!(m.hp(1), Some((130, 130)), "never hit");

    // In reach: P1's Shieldbearer UnitId(1) between P0's Skirmishers 2 and
    // 3, which attack it from the tick they appear.
    let mut m = M::new(rules.clone(), 34);
    m.run_as(1, vec![spawn_for(P1, shield, 11, 30, 1)]);
    let events = m.issue_tagged(
        vec![
            spawn(skirm, 10, 30, 1),
            spawn(skirm, 12, 30, 1),
            attack(&[2, 3], unit_t(1)),
        ],
        vec![],
        30,
    );
    assert!(!hits_by(&events, unit_t(2)).is_empty());
    assert!(!hits_by(&events, unit_t(3)).is_empty());
    m.ok(vec![stop(&[2]), mv(&[3], (4, 36))]);
    let (stopped, moved) = (m.unit(2).unwrap(), m.unit(3).unwrap());
    assert_eq!((moved.target, moved.intent), (None, Intent::Plain), "moved");
    assert!(moved.order.is_some());
    assert_eq!(
        (stopped.intent, stopped.order.is_none()),
        (Intent::Plain, true),
        "the explicit order ended"
    );
    assert_eq!(
        stopped.target,
        Some(unit_t(1)),
        "idle, it guards: the Shieldbearer in its reach again"
    );
    let later = m.step_tagged(100);
    assert!(
        hits_by(&later, unit_t(3)).is_empty(),
        "a Move never engages"
    );
    assert!(
        !hits_by(&later, unit_t(2)).is_empty(),
        "the guard fights on"
    );
}

/// What one unit did over a run, sampled after every tick.
#[derive(Default, Debug)]
struct Trace {
    /// Ticks with a move order (a path chase).
    with_order: u32,
    /// Ticks without an order while attacking (holding or chasing
    /// directly).
    direct: u32,
    /// Path requests made: ticks at which a request appeared.
    requests: u32,
    /// Distinct order goals.
    goals: Vec<FxVec2>,
    /// Every tile it stood on was passable.
    always_passable: bool,
    /// Largest x reached.
    max_x: Fx,
    /// Ticks that ended with an order whose path was empty although that
    /// order had a path at the end of an earlier tick: a re-goal threw the
    /// path away and parked the unit until the new one landed.
    stalled: u32,
    /// Ticks that ended with a re-goal's path request still outstanding on
    /// which the unit moved: it kept walking its old path meanwhile.
    walked_regoaling: u32,
}

/// Step `n` ticks, tracing unit `id` (while it lives).
fn trace(m: &mut M, id: u32, n: u32) -> (Trace, Vec<(u32, SimEvent)>) {
    let mut t = Trace {
        always_passable: true,
        ..Trace::default()
    };
    let mut had_request = m.view().pathing().has_request(UnitId(id));
    // The current order has had a path; a re-goal's request is pending.
    let (mut had_path, mut regoaling) = (false, false);
    let mut last_pos = m.unit(id).map(|u| u.pos);
    let mut events = Vec::new();
    for _ in 0..n {
        events.extend(m.step_tagged(1));
        let view = m.view();
        let Some(u) = view.unit(UnitId(id)) else {
            break;
        };
        match &u.order {
            Some(o) => {
                t.with_order += 1;
                if t.goals.last() != Some(&o.goal) {
                    // A new goal on an order that already had a path.
                    regoaling = had_path;
                    t.goals.push(o.goal);
                }
                if o.path.is_empty() && had_path {
                    t.stalled += 1;
                }
                had_path |= !o.path.is_empty();
            }
            None => {
                had_path = false;
                if u.intent == Intent::Attack {
                    t.direct += 1;
                }
            }
        }
        let request = view.pathing().has_request(UnitId(id));
        if request && !had_request {
            t.requests += 1;
        }
        had_request = request;
        regoaling &= request;
        if regoaling && last_pos != Some(u.pos) {
            t.walked_regoaling += 1;
        }
        last_pos = Some(u.pos);
        t.always_passable &= view.map().passable(view.map().tile_of(u.pos));
        t.max_x = t.max_x.max(u.pos.x);
    }
    (t, events)
}

/// An explicit `Attack` chases its target with no leash. Within reach (6
/// tiles edge to edge) on a clear line the chase is direct: no order, no
/// path request. Farther away the unit walks a path, re-goaled as the
/// target walks off, and switches to a direct chase once it is close.
#[test]
fn combat_attack_chases_an_ordered_target() {
    let rules = load_rules();
    let (skirm, yeoman) = (ukind(&rules, "skirmisher"), ukind(&rules, "yeoman"));
    // Direct: P1's Yeoman (UnitId(2)) walks east, 4 tiles ahead of P0's
    // faster Skirmisher (UnitId(1)).
    let mut m = M::new(rules.clone(), 35);
    m.ok(vec![spawn(skirm, 16, 30, 1)]);
    m.run_as(1, vec![spawn_for(P1, yeoman, 20, 30, 1)]);
    let walk_off = |m: &mut M, to: (i32, i32)| {
        m.run_as(
            1,
            vec![Command::Move {
                units: vec![UnitId(2)],
                target: FxVec2::from_ints(to.0, to.1),
                queue: false,
            }],
        );
    };
    walk_off(&mut m, (42, 30));
    m.ok(vec![attack(&[1], unit_t(2))]);
    let (t, events) = trace(&mut m, 1, 400);
    assert!(died_at(&events, 2).is_some(), "caught and killed");
    assert_eq!((t.with_order, t.requests), (0, 0), "a direct chase: {t:?}");
    assert!(t.direct > 0);
    let u = m.unit(1).unwrap();
    assert_eq!((u.target, u.intent, u.order), (None, Intent::Plain, None));

    // A path chase: the Yeoman starts 20 tiles away and walks south-east.
    let mut m = M::new(rules.clone(), 35);
    m.ok(vec![spawn(skirm, 6, 26, 1)]);
    m.run_as(1, vec![spawn_for(P1, yeoman, 26, 26, 1)]);
    walk_off(&mut m, (40, 40));
    m.ok(vec![attack(&[1], unit_t(2))]);
    let (t, events) = trace(&mut m, 1, 1200);
    assert!(died_at(&events, 2).is_some(), "caught and killed: {t:?}");
    assert!(t.with_order > 0, "walked a path first: {t:?}");
    assert!(t.goals.len() >= 2, "re-goaled as the target moved: {t:?}");
    assert_eq!(
        t.stalled, 0,
        "a re-goal keeps the old path until the new one lands: {t:?}"
    );
    assert!(t.direct > 0, "then chased directly: {t:?}");
    assert!(
        t.requests <= 2 * u32::try_from(t.goals.len()).unwrap() + 4,
        "a request per re-goal and the periodic repaths: {t:?}"
    );
    eprintln!("chase: {t:?}");
}

/// Within reach but across the river the line is blocked, so the
/// attacker walks a path through the ford instead of steering into the
/// water, and kills the target on the far bank.
#[test]
fn combat_attack_paths_across_the_ford_when_the_line_is_blocked() {
    let rules = load_rules();
    let (skirm, yeoman) = (ukind(&rules, "skirmisher"), ukind(&rules, "yeoman"));
    let mut m = M::new(rules.clone(), 36);
    // Water at x 62..=65 on row 55; the ford is rows 60..=67.
    let map = m.view().map().clone();
    assert!((62..=65).all(|x| !map.passable(Tile::new(x, 55))));
    assert!((62..=65).all(|x| map.passable(Tile::new(x, 60))));
    m.ok(vec![spawn(skirm, 60, 55, 1)]);
    m.run_as(1, vec![spawn_for(P1, yeoman, 66, 55, 1)]);
    let (a, b) = (m.unit(1).unwrap().pos, m.unit(2).unwrap().pos);
    assert_eq!(
        a.dist_sq_i64(b),
        Fx::from_int(36).to_bits(),
        "6 tiles apart"
    );
    m.ok(vec![attack(&[1], unit_t(2))]);
    let (t, events) = trace(&mut m, 1, 600);
    assert!(
        died_at(&events, 2).is_some(),
        "killed on the far bank: {t:?}"
    );
    assert!(t.with_order > 0, "walked a path: {t:?}");
    assert!(t.always_passable, "never in the water");
    assert!(t.max_x > Fx::from_int(65), "crossed: {t:?}");
    eprintln!("ford: {t:?}");
}

/// A target walled in by eight Watchtowers cannot be reached: it is in
/// another component, and a melee attacker cannot hit it from the nearest
/// tile it can walk to. The attacker gives up at once, with no path and no
/// request, and makes none later.
#[test]
fn combat_attack_on_an_unreachable_target_gives_up_without_a_request_storm() {
    let rules = load_rules();
    let (skirm, yeoman) = (ukind(&rules, "skirmisher"), ukind(&rules, "yeoman"));
    let tower = bkind(&rules, "watchtower");
    let mut m = M::new(rules.clone(), 37);
    m.ok(vec![spawn(skirm, 10, 30, 1)]);
    let mut walls = vec![spawn_for(P1, yeoman, 20, 30, 1)];
    for (dx, dy) in [
        (-1, -1),
        (0, -1),
        (1, -1),
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
    ] {
        walls.push(place_for(
            P1,
            tower,
            20u16.wrapping_add_signed(dx),
            30u16.wrapping_add_signed(dy),
            true,
        ));
    }
    m.run_as(1, walls);
    m.ok(vec![attack(&[1], unit_t(2))]);
    assert!(
        m.unit(1).unwrap().order.is_none() && m.view().pathing().is_idle(),
        "no path asked for in the tick the order applied"
    );
    let (t, events) = trace(&mut m, 1, 400);
    assert!(hits(&events.iter().map(|e| e.1.clone()).collect::<Vec<_>>()).is_empty());
    let u = m.unit(1).unwrap();
    assert_eq!(
        (u.target, u.intent),
        (None, Intent::Plain),
        "gave up: {t:?}"
    );
    assert_eq!(
        (t.with_order, t.requests),
        (0, 0),
        "another component costs no path: {t:?}"
    );
    let quiet = trace(&mut m, 1, 200).0;
    assert_eq!(quiet.requests, 0, "no more requests once given up");
    assert!(m.view().pathing().is_idle());
    eprintln!("unreachable: {t:?}");
}

/// The eight tiles round `(20, 30)`.
const RING: [(i16, i16); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
];

/// A Watchtower site of P0's on the tile `d` away from `(20, 30)`: it
/// blocks its tile like a complete tower, but a site does not shoot (a
/// complete P1 tower would shoot the attackers) and an own building is no
/// target for P0's units when they guard after giving up.
fn ring_tower(rules: &Rules, d: (i16, i16)) -> Command {
    place_for(
        P0,
        bkind(rules, "watchtower"),
        20u16.wrapping_add_signed(d.0),
        30u16.wrapping_add_signed(d.1),
        false,
    )
}

/// Every attacker of a group decides alike whether a target can be
/// reached, so none is left behind: six Skirmishers ordered onto a Yeoman
/// ringed by eight Watchtower sites all give up in the tick the order applies,
/// with no path and no request (on one path each they would all walk to
/// the one corrected goal tile beside the wall, which not all of them can
/// come within the arrive radius of, and the rest would jostle there for
/// ever). When the ring closes while the group walks toward its last gap,
/// all six give up in that tick. No hit lands either way and the pathing
/// queue stays idle.
#[test]
fn combat_group_attack_on_an_unreachable_target_gives_up() {
    let rules = load_rules();
    let (skirm, yeoman) = (ukind(&rules, "skirmisher"), ukind(&rules, "yeoman"));
    let group = [1, 2, 3, 4, 5, 6];
    let given_up = |m: &M| {
        group.iter().all(|&id| {
            let u = m.unit(id).unwrap();
            (u.target, u.intent, u.order.is_some()) == (None, Intent::Plain, false)
                && !m.view().pathing().has_request(UnitId(id))
        })
    };
    let quiet = |m: &mut M| {
        let events = m.step_tagged(200);
        assert!(hits(&events.into_iter().map(|e| e.1).collect::<Vec<_>>()).is_empty());
        assert!(given_up(m) && m.view().pathing().is_idle());
    };

    // Walled off from the start.
    let mut m = M::new(rules.clone(), 37);
    m.ok(vec![spawn(skirm, 10, 30, 6)]); // UnitId(1..=6)
    m.run_as(1, vec![spawn_for(P1, yeoman, 20, 30, 1)]); // UnitId(7)
    m.ok(RING.iter().map(|&d| ring_tower(&rules, d)).collect());
    m.issue_tagged(vec![attack(&group, unit_t(7))], vec![], 0);
    assert!(given_up(&m), "in the tick the order applied");
    assert!(m.view().pathing().is_idle(), "no path asked for");
    quiet(&mut m);

    // The ring closes behind the target while the group walks to its gap.
    let mut m = M::new(rules.clone(), 37);
    m.ok(vec![spawn(skirm, 10, 30, 6)]);
    m.run_as(1, vec![spawn_for(P1, yeoman, 20, 30, 1)]);
    m.ok(RING[..7].iter().map(|&d| ring_tower(&rules, d)).collect());
    m.issue_tagged(vec![attack(&group, unit_t(7))], vec![], 20);
    for id in group {
        let u = m.unit(id).unwrap();
        assert!(
            u.intent == Intent::Attack && u.order.as_ref().is_some_and(|o| !o.path.is_empty()),
            "unit {id} walks toward the gap: {u:?}"
        );
    }
    m.ok(vec![ring_tower(&rules, RING[7])]);
    assert!(given_up(&m), "in the tick the ring closed");
    quiet(&mut m);
}

/// A Bowman shoots over a wall: three ordered onto the Yeoman ringed by
/// eight Watchtower sites walk a path toward the nearest tile they can
/// reach, which is in range of it, and kill it from beyond the wall
/// instead of giving up.
#[test]
fn combat_ranged_attack_on_a_walled_off_target_shoots_over_the_wall() {
    let rules = load_rules();
    let (bow, yeoman) = (ukind(&rules, "bowman"), ukind(&rules, "yeoman"));
    let mut m = M::new(rules.clone(), 37);
    m.ok(vec![spawn(bow, 4, 30, 3)]); // UnitId(1..=3), 15 tiles off
    m.run_as(1, vec![spawn_for(P1, yeoman, 20, 30, 1)]); // UnitId(4)
    m.ok(RING.iter().map(|&d| ring_tower(&rules, d)).collect());
    m.ok(vec![attack(&[1, 2, 3], unit_t(4))]);
    let (t, events) = trace(&mut m, 1, 400);
    assert!(died_at(&events, 4).is_some(), "shot over the wall: {t:?}");
    assert!(t.with_order > 0, "walked a path first: {t:?}");
    assert!(!hits_by(&events, unit_t(1)).is_empty());
}

/// A path chase on a building ends on the reachable ring tile nearest to
/// the attacker, here a corner tile, whose centre is 0.71 tiles from the
/// footprint's corner: out of melee range (0.60 for a Skirmisher). From
/// that tile the attacker closes in directly even when a second Croft
/// flush against the target blocks the tile the corner point floors to
/// (NE, SW) or a tile beside the corner crossing (NW), and it hits. The SE
/// case is the control: its corner point floors to the attacker's own tile.
#[test]
fn combat_attack_on_a_building_closes_in_from_a_corner_tile() {
    let rules = load_rules();
    let (skirm, croft) = (ukind(&rules, "skirmisher"), bkind(&rules, "croft"));
    // Target: BuildingId(1) at (20, 30), tiles 20..=21 x 30..=31 (all
    // grass). Per case: the second Croft's NW tile, the spawn tile and the
    // corner ring tile the path chase walks to.
    for (other, from, corner) in [
        ((22, 30), (28, 25), (22, 29)), // NE
        ((20, 32), (14, 36), (19, 32)), // SW
        ((20, 28), (14, 24), (19, 29)), // NW
        ((22, 30), (28, 37), (22, 32)), // SE
    ] {
        let mut m = M::new(rules.clone(), 41);
        m.run_as(
            1,
            vec![
                place_for(P1, croft, 20, 30, true),
                place_for(P1, croft, other.0, other.1, true),
            ],
        );
        m.ok(vec![spawn(skirm, from.0, from.1, 1)]);
        let corner = m.view().map().center_of(Tile::new(corner.0, corner.1));
        let mut events = m.ok(vec![attack(&[1], building_t(1))]);
        let mut walked_to_corner = false;
        for _ in 0..300 {
            events.extend(m.step(1));
            walked_to_corner |= m
                .unit(1)
                .and_then(|u| u.order)
                .is_some_and(|o| o.goal == corner);
        }
        assert!(
            walked_to_corner,
            "{other:?} from {from:?}: the path chase went to the corner"
        );
        assert!(
            hits(&events)
                .iter()
                .any(|(a, t, _)| (*a, *t) == (unit_t(1), building_t(1))),
            "{other:?} from {from:?}: no hit, {:?}",
            m.unit(1)
        );
        let u = m.unit(1).unwrap();
        assert_eq!(
            (u.target, u.intent),
            (Some(building_t(1)), Intent::Attack),
            "{other:?} from {from:?}: still attacking"
        );
    }
}

/// A target that stops within `chase_regoal_tiles` of the path chase's
/// goal but round a corner can still be reached: the attacker arrives at
/// the stale goal, walks on round the corner and kills it instead of
/// giving up.
#[test]
fn combat_attack_walks_round_a_corner_to_a_target_near_its_goal() {
    let rules = load_rules();
    let (skirm, yeoman) = (ukind(&rules, "skirmisher"), ukind(&rules, "yeoman"));
    let hall = bkind(&rules, "muster_hall");
    let mut m = M::new(rules.clone(), 39);
    m.ok(vec![spawn(skirm, 6, 30, 1)]); // UnitId(1)
    // P1's Muster Halls block x 12..=20 on rows 31..=33.
    m.run_as(
        1,
        vec![
            spawn_for(P1, yeoman, 20, 30, 1), // UnitId(2)
            place_for(P1, hall, 12, 31, true),
            place_for(P1, hall, 15, 31, true),
            place_for(P1, hall, 18, 31, true),
        ],
    );
    // The path chase goes to (20, 30); the Yeoman walks round the corner
    // to (21, 32): 2 tiles from that goal (no re-goal), with no clear line
    // from any tile of the approach along row 30.
    let mut events = m.issue_tagged(
        vec![attack(&[1], unit_t(2))],
        vec![Command::Move {
            units: vec![UnitId(2)],
            target: FxVec2::from_ints(21, 32),
            queue: false,
        }],
        0,
    );
    let (t, more) = trace(&mut m, 1, 400);
    events.extend(more);
    assert!(
        events
            .iter()
            .any(|(_, e)| *e == SimEvent::UnitArrived { unit: UnitId(1) }),
        "the path chase arrived at the stale goal: {t:?}"
    );
    assert!(
        died_at(&events, 2).is_some(),
        "walked round and killed it: {t:?}"
    );
    assert!(t.goals.len() >= 2, "walked again after arriving: {t:?}");
    assert!(t.always_passable);
}

/// With the path budget saturated (2 expansions a tick, set in memory),
/// every path request waits several ticks. A path chase that re-goals
/// keeps walking its old path meanwhile instead of standing until the new
/// one lands, never arrives and never gives up before the kill, and
/// catches and kills the target. (That a chaser waiting for a path holds
/// the end of its old path rather than arriving there is
/// `movement::tests::awaiting_units_hold_the_end_of_their_path_instead_of_arriving`.)
#[test]
fn combat_path_chase_keeps_walking_while_pathing_is_saturated() {
    let mut rules = load_rules();
    rules.path_budget_expansions = 2;
    let (skirm, yeoman) = (ukind(&rules, "skirmisher"), ukind(&rules, "yeoman"));
    let mut m = M::new(rules.clone(), 35);
    m.ok(vec![spawn(skirm, 6, 26, 1)]);
    m.run_as(1, vec![spawn_for(P1, yeoman, 26, 26, 1)]);
    m.run_as(
        1,
        vec![Command::Move {
            units: vec![UnitId(2)],
            target: FxVec2::from_ints(40, 40),
            queue: false,
        }],
    );
    m.ok(vec![attack(&[1], unit_t(2))]);
    let start = m.sim.tick();
    let (t, events) = trace(&mut m, 1, 1200);
    let killed = died_at(&events, 2).expect("caught and killed");
    assert!(t.goals.len() >= 2, "re-goaled as the target moved: {t:?}");
    assert_eq!(t.stalled, 0, "never parked by a re-goal: {t:?}");
    assert!(
        t.walked_regoaling > 0,
        "walked on while a re-goal waited: {t:?}"
    );
    assert!(
        !events
            .iter()
            .any(|(at, e)| *at < killed && *e == SimEvent::UnitArrived { unit: UnitId(1) }),
        "never arrived before the kill: {t:?}"
    );
    assert_eq!(
        t.direct + t.with_order,
        killed - start + 1,
        "attacking on every tick until the kill, then given up: {t:?}"
    );
    eprintln!("saturated: {t:?}");
}

/// P1's Mangonels destroy P0's complete Watchtower, one of the five
/// distinct kinds that grew the Seat to tier 2 (no age gate, set in
/// memory): the footprint is free with one generation bump, the tower's
/// border source is gone and the field equals a full recompute, and the
/// Seat falls back to tier 1 and its tier-1 radius.
#[test]
fn combat_watchtower_destroyed_frees_footprint_border_and_tier() {
    let mut rules = load_rules();
    rules.town_growth_age = None;
    let r1 = u16::try_from(rules.town_radius_tiles).unwrap();
    let mut m = M::standard(rules.clone(), 38);
    // BuildingId(3..=7); the Watchtower is 7.
    m.ok([
        ("croft", 28u16, 58u16),
        ("scriptorium", 28, 67),
        ("muster_hall", 32, 67),
        ("engine_yard", 36, 67),
        ("watchtower", 31, 58),
    ]
    .iter()
    .map(|(id, x, y)| place(bkind(&rules, id), *x, *y, true))
    .collect());
    let tower = BuildingId(7);
    assert_eq!(
        m.view().building(tower).unwrap().kind,
        bkind(&rules, "watchtower")
    );
    let seat = |m: &M| {
        (
            m.view()
                .building(BuildingId(1))
                .unwrap()
                .town_state
                .unwrap()
                .tier,
            m.view().territory().sources()[&BuildingId(1)].radius,
        )
    };
    assert_eq!(seat(&m).0, 2, "grown");
    assert!(m.view().territory().sources().contains_key(&tower));
    // Five P1 Mangonels: 5 x 120 against 500 HP, spawned in the tick their
    // `Attack` applies (idle, they would take P0's buildings on their
    // own). P0's Yeomen are UnitId(1..=5), P1's UnitId(6..=10), the
    // Mangonels UnitId(11..=15). The tower shoots one of them in that tick.
    let first = u32::try_from(m.view().unit_count()).unwrap() + 1;
    let ids: Vec<u32> = (first..first + 5).collect();
    let generation = m.view().map().cost_grid_generation();
    assert!(!m.view().map().passable(Tile::new(31, 58)));
    let events = m.run_as(
        1,
        vec![
            spawn_for(P1, ukind(&rules, "mangonel"), 35, 53, 5),
            attack(&ids, TargetId::Building(tower)),
        ],
    );
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert!(ids.iter().all(|id| m.unit(*id).unwrap().owner == P1));
    let volley = hits(&events);
    assert_eq!(
        volley
            .iter()
            .filter(|(a, t, _)| matches!(a, TargetId::Unit(_)) && *t == TargetId::Building(tower))
            .count(),
        5
    );
    assert_eq!(
        volley
            .iter()
            .filter(|(a, _, _)| *a == TargetId::Building(tower))
            .count(),
        1,
        "the tower's last shot lands with the volley"
    );
    assert_eq!(volley.len(), 6);
    assert_eq!(destroyed(&events), [tower]);
    assert!(m.view().building(tower).is_none());
    assert_eq!(m.view().map().cost_grid_generation(), generation + 1);
    assert!(m.view().map().passable(Tile::new(31, 58)), "footprint free");
    assert!(!m.view().territory().sources().contains_key(&tower));
    assert_eq!(m.view().territory(), &m.view().territory().recomputed());
    assert!(events.contains(&SimEvent::TerritoryChanged));
    assert!(events.contains(&SimEvent::TownTierChanged {
        town: BuildingId(1),
        tier: 1
    }));
    assert_eq!(seat(&m), (1, r1), "back to tier 1");
    // Their target gone, the Mangonels give up next tick.
    m.step(1);
    for id in ids {
        let u = m.unit(id).unwrap();
        assert_eq!((u.target, u.intent), (None, Intent::Plain));
    }
}

/// A destroyed Muster Hall takes its training queue with it: no refund,
/// no unit, and the pop it charged is free again.
#[test]
fn combat_destroyed_building_loses_its_queue_and_frees_pop() {
    let rules = load_rules();
    let skirm = ukind(&rules, "skirmisher");
    let mut m = M::new(rules.clone(), 39);
    let p1_grain = Command::DebugGrant {
        owner: P1,
        resource: res_u8(&rules, "grain"),
        amount: 1000,
    };
    m.run_as(
        1,
        vec![
            place_for(P1, bkind(&rules, "muster_hall"), TOWN.0, TOWN.1, true),
            p1_grain,
        ],
    );
    m.run_as(1, vec![train(1, skirm), train(1, skirm), train(1, skirm)]);
    assert_eq!(m.view().building(BuildingId(1)).unwrap().queue.len(), 3);
    assert_eq!(m.view().pop(P1).0, 3);
    let stock = m.stock(P1);
    // Seven Mangonels: 7 x 120 against 800 HP, spawned in the tick their
    // `Attack` applies (idle, they would take the hall on their own).
    let events = m.ok(vec![
        spawn(ukind(&rules, "mangonel"), 15, 31, 7),
        attack(&[1, 2, 3, 4, 5, 6, 7], building_t(1)),
    ]);
    assert_eq!(destroyed(&events), [BuildingId(1)]);
    assert_eq!(m.view().pop(P1).0, 0, "the queued pop is free");
    assert_eq!(m.stock(P1), stock, "no refund");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SimEvent::TrainingCancelled { .. }))
    );
    let later = m.step(400);
    assert!(
        !later
            .iter()
            .any(|e| matches!(e, SimEvent::UnitSpawned { .. })),
        "nothing is trained"
    );
}

/// Sites of every kind, a Town site included, are destroyed at 0 HP, and
/// two buildings destroyed in the same tick free their footprints with one
/// generation bump. The Croft site's builder loses its job.
#[test]
fn combat_site_at_zero_hp_is_destroyed_town_sites_included() {
    let rules = load_rules();
    let (mangonel, yeoman) = (ukind(&rules, "mangonel"), ukind(&rules, "yeoman"));
    let mut m = M::new(rules.clone(), 40);
    // P1's Town site is BuildingId(1) (2000 HP), its Croft site
    // BuildingId(2) (300 HP); its Yeoman UnitId(1) resumes the Croft.
    let croft = bkind(&rules, "croft");
    m.run_as(
        1,
        vec![
            place_for(P1, bkind(&rules, "town"), TOWN.0, TOWN.1, false),
            place_for(P1, croft, 20, 36, false),
            spawn_for(P1, yeoman, 24, 38, 1),
        ],
    );
    m.run_as(
        1,
        vec![Command::Build {
            unit: UnitId(1),
            kind: croft,
            tile: Tile::new(20, 36),
        }],
    );
    assert_eq!(
        m.unit(1).unwrap().job,
        Some(sim::Job::Build {
            site: BuildingId(2)
        })
    );
    assert_eq!(m.view().towns(P1).0, 1, "the Town site counts");
    // UnitId(2..=18) on the Town site, UnitId(19..=21) on the Croft site,
    // spawned in the tick their `Attack`s apply (idle, they would take the
    // sites on their own).
    let generation = m.view().map().cost_grid_generation();
    let events = m.ok(vec![
        spawn(mangonel, 15, 31, 17),
        spawn(mangonel, 14, 38, 3),
        attack(&(2..=18).collect::<Vec<_>>(), building_t(1)),
        attack(&[19, 20, 21], building_t(2)),
    ]);
    assert_eq!(hits(&events).len(), 20, "{events:?}");
    assert_eq!(destroyed(&events), [BuildingId(1), BuildingId(2)]);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SimEvent::TownAtZeroHp { .. })),
        "a site is not a Town yet"
    );
    assert_eq!(
        m.view().map().cost_grid_generation(),
        generation + 1,
        "one bump"
    );
    for (x, y) in [
        (TOWN.0, TOWN.1),
        (TOWN.0 + 2, TOWN.1 + 2),
        (20, 36),
        (21, 37),
    ] {
        assert!(m.view().map().passable(Tile::new(x, y)), "({x}, {y})");
    }
    assert_eq!(m.view().towns(P1).0, 0);
    assert_eq!(m.unit(1).unwrap().job, None, "the builder is idle");
}

/// A complete Town at 0 HP stays: clamped at 0 (17 hits of 120 on 2000 HP
/// overkill it), `TownAtZeroHp` once, its footprint and border kept. It is
/// no longer a target, so its attackers stand down and land nothing more,
/// and its owner may still train there (until M5a's annexation).
#[test]
fn combat_town_at_zero_hp_stays_and_is_not_targeted() {
    let rules = load_rules();
    let mut m = town_at_zero(&rules, 41);
    let town = m.view().building(BuildingId(1)).unwrap().clone();
    assert_eq!(town.hp, 0, "clamped, not negative");
    assert!(town.town_state.is_some());
    assert!(!m.view().map().passable(Tile::new(TOWN.0, TOWN.1)));
    assert!(m.view().territory().sources().contains_key(&BuildingId(1)));
    let later = m.step(300);
    assert!(hits(&later).is_empty());
    assert!(!later.iter().any(|e| matches!(
        e,
        SimEvent::TownAtZeroHp { .. } | SimEvent::BuildingDestroyed { .. }
    )));
    for id in 1..=17 {
        let u = m.unit(id).unwrap();
        assert_eq!((u.target, u.intent), (None, Intent::Plain), "unit {id}");
    }
    assert_eq!(
        m.view().building(BuildingId(1)).unwrap().hp,
        0,
        "no regeneration"
    );
    let events = m.run_as(
        1,
        vec![
            Command::DebugGrant {
                owner: P1,
                resource: res_u8(&rules, "grain"),
                amount: 100,
            },
            train(1, ukind(&rules, "yeoman")),
        ],
    );
    assert!(rejections(&events).is_empty(), "{events:?}");
}

/// A dead worker takes nothing with it: the slot it worked is free, and a
/// unit killed while its path request is still queued (behind 100 longer
/// requests that saturate the budget) leaves no request behind.
#[test]
fn combat_dead_worker_frees_its_slot_and_path_request() {
    let rules = load_rules();
    let (skirm, yeoman) = (ukind(&rules, "skirmisher"), ukind(&rules, "yeoman"));
    // The slot: P1's Yeoman (UnitId(1)) works P1's Croft (BuildingId(1)).
    let mut m = M::new(rules.clone(), 42);
    m.run_as(
        1,
        vec![
            place_for(P1, bkind(&rules, "croft"), 20, 30, true),
            spawn_for(P1, yeoman, 23, 30, 1),
        ],
    );
    m.run_as(
        1,
        vec![Command::Gather {
            units: vec![UnitId(1)],
            node: BuildingId(1),
        }],
    );
    m.step(100);
    assert_eq!(m.view().worked_slots(BuildingId(1)), Some((1, 1, 1)));
    // P0's Skirmishers UnitId(2..=5), spawned in the tick their `Attack`
    // applies (idle, they would take the worker on their own).
    let events = m.issue_tagged(
        vec![spawn(skirm, 24, 33, 4), attack(&[2, 3, 4, 5], unit_t(1))],
        vec![],
        100,
    );
    assert!(died_at(&events, 1).is_some());
    assert_eq!(
        m.view().worked_slots(BuildingId(1)),
        Some((0, 0, 1)),
        "free"
    );

    // The request: 100 P1 Yeomen (UnitId(1..=100)) and the victim
    // (UnitId(101)) are ordered across the river in one Move, so the
    // victim's request queues behind theirs. Two P0 Mangonels
    // (UnitId(102..=103), 30 a hit against 40 HP, in range at once) are
    // spawned and attack it in the same tick (idle, they would take it on
    // their own before the Move) and kill it with their first volley. The
    // control has no Mangonels.
    let setup = || {
        let mut m = M::new(rules.clone(), 43);
        m.run_as(1, vec![spawn_for(P1, yeoman, 8, 34, 100)]);
        m.run_as(1, vec![spawn_for(P1, yeoman, 30, 30, 1)]);
        m
    };
    let go = Command::Move {
        units: (1..=101).map(UnitId).collect(),
        target: FxVec2::from_ints(110, 100),
        queue: false,
    };
    let mut m = setup();
    let mut control = setup();
    let events = m.issue_tagged(
        vec![
            spawn(ukind(&rules, "mangonel"), 30, 33, 2),
            attack(&[102, 103], unit_t(101)),
        ],
        vec![go.clone()],
        0,
    );
    control.issue_tagged(vec![], vec![go], 0);
    let applied = m.sim.tick() - 1;
    assert_eq!(
        died_at(&events, 101),
        Some(applied),
        "dies in the tick of the Move"
    );
    let (view, control_view) = (m.view(), control.view());
    assert!(
        control_view.pathing().has_request(UnitId(101)),
        "without the attack the victim's request is still queued"
    );
    assert!(!view.pathing().has_request(UnitId(101)), "cancelled");
    assert_eq!(
        view.pathing().outstanding() + 1,
        control_view.pathing().outstanding()
    );
}

/// Buildings destroyed while A* searches are suspended (the budget is
/// saturated by 50 long requests) change the cost grid under them: the
/// run is identical tick by tick with the path cache on and off, and with
/// a restore taken right after the destruction. The comparison can fail:
/// two probe Yeomen ask for the same `(start, goal)` key in one generation
/// (the second is served from the first's cache entry), and a third asks
/// for it again after the Watchtower on their detour is destroyed, where
/// a stale entry (a destruction that did not bump the generation, or a key
/// without it) would hand it the detour instead of the straight path.
#[test]
fn combat_destruction_during_a_suspended_search_is_cache_transparent() {
    let rules = load_rules();
    let (mangonel, yeoman) = (ukind(&rules, "mangonel"), ukind(&rules, "yeoman"));
    let cmd = |p: PlayerId, seq: u32, c: Command| PlayerCommand::new(p, seq, c);
    // P1's Yeomen are UnitId(1..=50); P1's two Watchtowers and Croft
    // BuildingId(1..=3) stand beside the block the Yeomen leave. P1's
    // probes UnitId(51..=52) stand on one tile west of Watchtower 1, each
    // with its own Move to the same click east of it: the same key. P0's
    // Mangonels UnitId(53..=67) are spawned in the tick their `Attack`s
    // apply (idle, they would take the buildings on their own before the
    // searches are saturated).
    let probe = |id: u32| Command::Move {
        units: vec![UnitId(id)],
        target: FxVec2::from_ints(20, 30),
        queue: false,
    };
    let mut stream: Vec<(u32, Vec<PlayerCommand>)> = vec![(
        0,
        vec![
            cmd(P1, 0, spawn_for(P1, yeoman, 8, 34, 50)),
            cmd(
                P1,
                1,
                place_for(P1, bkind(&rules, "watchtower"), 16, 30, true),
            ),
            cmd(
                P1,
                2,
                place_for(P1, bkind(&rules, "watchtower"), 18, 31, true),
            ),
            cmd(P1, 3, place_for(P1, bkind(&rules, "croft"), 16, 33, true)),
            cmd(P1, 4, spawn_for(P1, yeoman, 13, 30, 1)),
            cmd(P1, 5, spawn_for(P1, yeoman, 13, 30, 1)),
            cmd(P1, 6, probe(51)),
            cmd(P1, 7, probe(52)),
        ],
    )];
    stream.push((
        2,
        vec![cmd(
            P1,
            8,
            Command::Move {
                units: (1..=50).map(UnitId).collect(),
                target: FxVec2::from_ints(110, 100),
                queue: false,
            },
        )],
    ));
    stream.push((
        4,
        vec![
            cmd(P0, 0, spawn(mangonel, 12, 26, 15)),
            cmd(P0, 1, attack(&[53, 54, 55, 56, 57], building_t(1))),
            cmd(P0, 2, attack(&[58, 59, 60, 61, 62], building_t(2))),
            cmd(P0, 3, attack(&[63, 64, 65, 66, 67], building_t(3))),
        ],
    ));
    // Probe UnitId(68), on the same tile once Watchtower 1 is destroyed
    // (the Mangonels' first volley, tick 6), asks for the same key.
    stream.push((
        5,
        vec![
            cmd(P1, 9, spawn_for(P1, yeoman, 13, 30, 1)),
            cmd(P1, 10, probe(68)),
        ],
    ));
    let at = |tick: u32| -> Vec<PlayerCommand> {
        stream
            .iter()
            .find(|(t, _)| *t == tick)
            .map(|(_, c)| c.clone())
            .unwrap_or_default()
    };
    // The searches are saturated for a few ticks; the requeued ones and the
    // first periodic repaths follow.
    const TICKS: u32 = 75;
    let run = |cache: bool| {
        let mut sim = M::new(rules.clone(), 44).sim;
        sim.set_path_cache_enabled(cache);
        let mut hashes = Vec::new();
        let mut destroyed_with_search = Vec::new();
        let mut gone_total = 0;
        let mut late_probe: Option<Vec<Tile>> = None;
        for tick in 0..TICKS {
            sim.step(&at(tick));
            let gone = destroyed(&sim.drain_events());
            gone_total += gone.len();
            if !gone.is_empty() && sim.view().pathing().active.is_some() {
                destroyed_with_search.push((tick, sim.snapshot()));
            }
            let v = sim.view();
            let path = |id: u32| {
                v.unit(UnitId(id))
                    .and_then(|u| u.order.as_ref())
                    .map(|o| o.path.clone())
                    .filter(|p| !p.is_empty())
            };
            if tick == 2 {
                // Both probes are served in the tick their Moves apply: 51
                // by a search, inserted; 52 from that entry (a hit inserts
                // nothing).
                assert!(path(51).is_some() && path(51) == path(52), "same key");
                assert!(!path(51).unwrap().contains(&Tile::new(16, 30)));
                assert_eq!(
                    v.pathing().cache.next_insert,
                    u64::from(cache),
                    "one search, one hit"
                );
            }
            if late_probe.is_none() {
                late_probe = path(68);
            }
            hashes.push((sim.hash(), sim.sub_hashes()));
        }
        assert_eq!(gone_total, 3, "both Watchtowers and the Croft");
        assert!(
            late_probe
                .expect("68 is served")
                .contains(&Tile::new(16, 30)),
            "68 walks through the destroyed Watchtower, not along the cached detour"
        );
        (hashes, destroyed_with_search)
    };
    let (on, snaps) = run(true);
    let (off, _) = run(false);
    assert!(
        !snaps.is_empty(),
        "a building was destroyed while a search was suspended"
    );
    assert_eq!(
        on, off,
        "hashes agree on every tick with the cache on and off"
    );
    let (tick, snap) = &snaps[0];
    let mut b = M::new(rules.clone(), 44).sim;
    b.restore(snap).unwrap();
    for t in tick + 1..TICKS {
        b.step(&at(t));
        assert_eq!(
            (b.hash(), b.sub_hashes()),
            on[t as usize],
            "restored at {tick}, tick {t}"
        );
    }
}

/// A snapshot mid-fight (damage taken, cooldowns running, one unit on a
/// path chase, one chasing directly) restores to a sim that matches the
/// uninterrupted run on every tick, including 50 units spawned after the
/// restore.
#[test]
fn combat_restore_mid_fight_matches_uninterrupted() {
    let rules = load_rules();
    let (skirm, shield, bow) = (
        ukind(&rules, "skirmisher"),
        ukind(&rules, "shieldbearer"),
        ukind(&rules, "bowman"),
    );
    let cmd = |p: PlayerId, seq: u32, c: Command| PlayerCommand::new(p, seq, c);
    // P0 (whose commands apply first): Skirmishers 1..=4 at (10, 30) and
    // a Bowman 5 far west. P1: Shieldbearers 6..=9 at (13, 30) and a
    // Skirmisher 10 far east.
    let stream: Vec<(u32, Vec<PlayerCommand>)> = vec![
        (
            0,
            vec![
                cmd(P0, 0, spawn(skirm, 10, 30, 4)),
                cmd(P1, 0, spawn_for(P1, shield, 13, 30, 4)),
                cmd(P0, 1, spawn(bow, 2, 38, 1)),
                cmd(P1, 1, spawn_for(P1, skirm, 40, 38, 1)),
            ],
        ),
        (
            1,
            vec![
                cmd(P0, 2, attack(&[1, 2], unit_t(6))),
                cmd(P0, 3, attack(&[3, 4], unit_t(7))),
                cmd(P0, 4, attack(&[5], unit_t(10))),
                cmd(P1, 2, attack(&[6, 7, 8, 9], unit_t(1))),
                cmd(P1, 3, attack(&[10], unit_t(5))),
            ],
        ),
        (
            200,
            vec![
                cmd(P0, 5, spawn(skirm, 6, 26, 25)),
                cmd(P1, 4, spawn_for(P1, shield, 30, 26, 25)),
            ],
        ),
        (210, vec![cmd(P0, 6, stop(&[3]))]),
        // The 25 Skirmishers spawned after the restore are 11..=35, the
        // Shieldbearers 36..=60.
        (
            230,
            vec![cmd(
                P1,
                5,
                attack(&(36..=60).collect::<Vec<_>>(), unit_t(2)),
            )],
        ),
    ];
    let at = |tick: u32| -> Vec<PlayerCommand> {
        stream
            .iter()
            .find(|(t, _)| *t == tick)
            .map(|(_, c)| c.clone())
            .unwrap_or_default()
    };
    let mut a = M::new(rules.clone(), 45).sim;
    let snap_at = 60;
    for tick in 0..snap_at {
        a.step(&at(tick));
    }
    a.drain_events();
    {
        let view = a.view();
        let damaged = view.units().filter(|u| u.damage_taken > 0).count();
        assert!(damaged > 0, "damage taken");
        assert!(
            view.units().any(|u| u.ready_at > a.tick()),
            "a cooldown running"
        );
        assert!(
            view.units()
                .any(|u| u.intent == Intent::Attack && u.order.is_some()),
            "a path chase"
        );
    }
    let mut b = M::new(rules.clone(), 45).sim;
    b.restore(&a.snapshot()).unwrap();
    assert_eq!((a.hash(), a.sub_hashes()), (b.hash(), b.sub_hashes()));
    let mut died = 0;
    for tick in snap_at..600 {
        a.step(&at(tick));
        b.step(&at(tick));
        assert_eq!(a.hash(), b.hash(), "tick {tick}");
        assert_eq!(a.sub_hashes(), b.sub_hashes(), "tick {tick}");
        let ea = a.drain_events();
        assert_eq!(ea, b.drain_events(), "tick {tick}");
        died += deaths(&ea).len();
    }
    assert!(died > 0, "the fight went on after the restore");
}

// ---------------------------------------------------------------------------
// Combat behaviour: acquisition, `AttackMove`, guards and shooters
// (`combat_*`).
// ---------------------------------------------------------------------------

/// The `rng` sub-hash of a fresh sim with `seed` after `draws` draws.
fn rng_after(seed: u64, draws: usize) -> (u64, Vec<u32>) {
    use rand_core::{Rng, SeedableRng};
    let mut rng = rand_pcg::Pcg32::seed_from_u64(seed);
    let values = (0..draws).map(|_| rng.next_u32()).collect();
    (sim::hash::hash_value(&rng), values)
}

/// `rng_index(n)` of a draw: `(draw * n) >> 32`.
fn index_of(draw: u32, n: u32) -> u32 {
    u32::try_from((u64::from(draw) * u64::from(n)) >> 32).unwrap()
}

/// An idle soldier takes the nearest enemy in its reach on its own, with no
/// draw; between enemies at exactly the same distance it draws once from
/// the sim's RNG and takes `ties[rng_index(n)]`, the ties in `TargetId`
/// order, so the seed decides and both get taken over 20 seeds.
#[test]
fn combat_acquires_the_nearest_enemy_and_breaks_exact_ties_with_the_rng() {
    let rules = load_rules();
    let (skirm, yeoman) = (ukind(&rules, "skirmisher"), ukind(&rules, "yeoman"));
    // P1's Yeomen UnitId(1) 3 tiles west and UnitId(2) 4 tiles east of
    // where P0's Skirmisher UnitId(3) appears.
    let mut m = M::new(rules.clone(), 50);
    m.run_as(
        1,
        vec![
            spawn_for(P1, yeoman, 17, 30, 1),
            spawn_for(P1, yeoman, 24, 30, 1),
        ],
    );
    let rng = m.sim.sub_hashes().rng;
    m.ok(vec![spawn(skirm, 20, 30, 1)]);
    let u = m.unit(3).unwrap();
    assert_eq!((u.target, u.intent), (Some(unit_t(1)), Intent::Plain));
    assert_eq!(m.sim.sub_hashes().rng, rng, "a nearest enemy, no draw");
    assert_eq!(m.sim.sub_hashes().rng, rng_after(50, 0).0);

    // Both 3 tiles off: one draw.
    let mut taken = std::collections::BTreeSet::new();
    for seed in 0..20 {
        let mut m = M::new(rules.clone(), seed);
        m.run_as(
            1,
            vec![
                spawn_for(P1, yeoman, 17, 30, 1),
                spawn_for(P1, yeoman, 23, 30, 1),
            ],
        );
        m.ok(vec![spawn(skirm, 20, 30, 1)]);
        let (hash, draws) = rng_after(seed, 1);
        assert_eq!(m.sim.sub_hashes().rng, hash, "seed {seed}: one draw");
        let want = unit_t(1 + index_of(draws[0], 2));
        assert_eq!(m.unit(3).unwrap().target, Some(want), "seed {seed}");
        taken.insert(want);
        // Kept afterwards, with no further draw.
        m.step(5);
        assert_eq!(m.unit(3).unwrap().target, Some(want));
        assert_eq!(m.sim.sub_hashes().rng, hash, "seed {seed}: kept");
    }
    assert_eq!(taken.len(), 2, "the seed decides: {taken:?}");
}

/// Enemy units come before enemy buildings, a building in weapon range
/// included, and the Mangonel (`prefer: Buildings`) the other way round.
/// While it holds a building, a soldier turns on an enemy unit that comes
/// into its reach.
#[test]
fn combat_units_before_buildings_and_mangonel_prefers_buildings() {
    let rules = load_rules();
    let (skirm, yeoman, mangonel) = (
        ukind(&rules, "skirmisher"),
        ukind(&rules, "yeoman"),
        ukind(&rules, "mangonel"),
    );
    let croft = bkind(&rules, "croft");
    // P1's Croft BuildingId(1) touches the tile where P0's Skirmisher
    // UnitId(2) appears (in weapon range); P1's Yeoman UnitId(1) is 4 tiles
    // south (out of weapon range, in reach).
    let mut m = M::new(rules.clone(), 51);
    m.run_as(
        1,
        vec![
            place_for(P1, croft, 21, 30, true),
            spawn_for(P1, yeoman, 20, 34, 1),
        ],
    );
    m.ok(vec![spawn(skirm, 20, 30, 1)]);
    assert_eq!(m.unit(2).unwrap().target, Some(unit_t(1)), "the unit first");
    let events = m.step_tagged(200);
    let killed = died_at(&events, 1).expect("caught and killed");
    assert!(
        hits_on(&events, unit_t(2), building_t(1))
            .iter()
            .all(|h| h.0 > killed),
        "the Croft in weapon range never displaced the Yeoman"
    );
    assert!(
        !hits_on(&events, unit_t(2), building_t(1)).is_empty(),
        "then the Croft"
    );
    assert_eq!(m.unit(2).unwrap().target, Some(building_t(1)));
    // A Yeoman appears 4 tiles off: the Skirmisher leaves the Croft for it.
    let pos = m.unit(2).unwrap().pos;
    let (x, y) = (pos.x.floor_to_int(), pos.y.floor_to_int());
    m.run_as(1, vec![spawn_for(P1, yeoman, x, y + 4, 1)]); // UnitId(3)
    assert_eq!(m.unit(2).unwrap().target, Some(unit_t(3)), "units first");

    // P0's Mangonel UnitId(2) with P1's Yeoman UnitId(1) 2 tiles east and
    // P1's Croft BuildingId(1) 5.5 tiles east: the Croft.
    let mut m = M::new(rules.clone(), 51);
    m.run_as(
        1,
        vec![
            spawn_for(P1, yeoman, 22, 30, 1),
            place_for(P1, croft, 26, 29, true),
        ],
    );
    m.ok(vec![spawn(mangonel, 20, 30, 1)]);
    let u = m.unit(2).unwrap();
    assert_eq!((u.target, u.intent), (Some(building_t(1)), Intent::Plain));
    let events = m.step_tagged(100);
    assert!(!hits_on(&events, unit_t(2), building_t(1)).is_empty());
    assert!(hits_on(&events, unit_t(2), unit_t(1)).is_empty());
}

/// An acquired target out of weapon range gives way to an enemy that comes
/// into weapon range (the melee crowd case: a soldier that cannot get at
/// its target fights what is next to it); one in weapon range is kept
/// against another that comes into range.
#[test]
fn combat_acquired_target_switches_to_an_enemy_in_weapon_range() {
    let rules = load_rules();
    let (skirm, yeoman) = (ukind(&rules, "skirmisher"), ukind(&rules, "yeoman"));
    let mut m = M::new(rules.clone(), 52);
    // P1's Yeoman UnitId(1), 4 tiles east, walks off east; P0's Skirmisher
    // UnitId(2) takes it and chases.
    m.run_as(
        1,
        vec![
            spawn_for(P1, yeoman, 24, 30, 1),
            Command::Move {
                units: vec![UnitId(1)],
                target: FxVec2::from_ints(40, 30),
                queue: false,
            },
        ],
    );
    m.ok(vec![spawn(skirm, 20, 30, 1)]);
    let u = m.unit(2).unwrap();
    assert_eq!(u.target, Some(unit_t(1)));
    // P1's Yeoman UnitId(3) appears on the tile the chaser is about to
    // enter, in its weapon range, while UnitId(1) is still out of it.
    let ahead = m.view().map().tile_of(u.pos);
    m.run_as(
        1,
        vec![spawn_for(
            P1,
            yeoman,
            i32::from(ahead.x) + 1,
            i32::from(ahead.y),
            1,
        )],
    );
    let (u, a, b) = (
        m.unit(2).unwrap(),
        m.unit(1).unwrap().pos,
        m.unit(3).unwrap().pos,
    );
    assert!(
        u.pos.dist_sq_i64(a) > Fx::from_int(1).to_bits(),
        "1 still out of range"
    );
    assert!(u.pos.dist_sq_i64(b) < Fx::from_int(1).to_bits());
    assert_eq!(u.target, Some(unit_t(3)), "switched to the one in range");
    // Held in range, it is kept when another comes into range.
    let here = m.view().map().tile_of(u.pos);
    m.run_as(
        1,
        vec![spawn_for(
            P1,
            yeoman,
            i32::from(here.x),
            i32::from(here.y),
            1,
        )],
    ); // UnitId(4)
    assert_eq!(m.unit(2).unwrap().target, Some(unit_t(3)), "kept");
}

/// `AttackMove` gives armed units `Intent::AttackMove`: a Skirmisher
/// takes the enemy Yeoman beside its route, keeping its order (not
/// followed) while it fights, kills it and walks on to its goal, where it
/// arrives and then guards (`Intent::Plain`). The Yeoman in the same
/// command just moves.
#[test]
fn combat_attack_move_engages_then_resumes_its_route() {
    let rules = load_rules();
    let (skirm, yeoman) = (ukind(&rules, "skirmisher"), ukind(&rules, "yeoman"));
    let mut m = M::new(rules.clone(), 53);
    // P1's Yeoman UnitId(1) 2 tiles off the route; P0's Skirmisher UnitId(2)
    // and Yeoman UnitId(3).
    m.run_as(1, vec![spawn_for(P1, yeoman, 16, 32, 1)]);
    m.ok(vec![spawn(skirm, 6, 30, 1), spawn(yeoman, 6, 28, 1)]);
    let goal = FxVec2::from_ints(36, 30);
    m.ok(vec![Command::AttackMove {
        units: vec![UnitId(2), UnitId(3)],
        target: goal,
    }]);
    let (s, y) = (m.unit(2).unwrap(), m.unit(3).unwrap());
    let s_goal = s.order.as_ref().unwrap().goal;
    assert_eq!(s.intent, Intent::AttackMove { goal: s_goal });
    assert_eq!(y.intent, Intent::Plain, "unarmed: a Move");
    let (mut engaged, mut engaged_with_order, mut arrived) = (0, 0, None);
    let mut events = Vec::new();
    for _ in 0..600 {
        let tick = m.sim.tick();
        let step = m.step_tagged(1);
        let u = m.unit(2).unwrap();
        if u.target.is_some() {
            engaged += 1;
            engaged_with_order +=
                u32::from(u.order.is_some() && u.intent == Intent::AttackMove { goal: s_goal });
        }
        if arrived.is_none()
            && step
                .iter()
                .any(|(_, e)| *e == SimEvent::UnitArrived { unit: UnitId(2) })
        {
            arrived = Some(tick);
        }
        events.extend(step);
    }
    let killed = died_at(&events, 1).expect("the Yeoman beside the route dies");
    assert!(
        engaged > 0 && engaged_with_order == engaged,
        "{engaged} {engaged_with_order}"
    );
    let arrived = arrived.expect("it walked on and arrived");
    assert!(arrived > killed);
    let u = m.unit(2).unwrap();
    assert_eq!(
        (u.intent, u.target, u.order.is_none()),
        (Intent::Plain, None, true)
    );
    let post = u.post.expect("a post at the goal").goal;
    assert!(
        post.dist_sq_i64(s_goal) <= Fx::from_int(1).to_bits(),
        "{post:?}"
    );
    assert!(hits_by(&events, unit_t(3)).is_empty());
    assert!(m.unit(3).unwrap().order.is_none(), "the Yeoman arrived too");
}

/// The attack-move intent ends with its order: on arrival (the next think
/// finds no order and the unit guards), and on an unreachable goal, which
/// pathing corrects to the nearest tile of the unit's component, so the
/// unit arrives there. `combat::think` never issues the order again: no
/// path request follows the arrival. (The defensive `PathUnreachable`
/// branch of step 4 clears the intent too, an explicit `Attack`'s included:
/// `state::tests::path_unreachable_ends_an_explicit_attack_without_a_request_storm`.)
#[test]
fn combat_attack_move_intent_ends_on_arrival_or_unreachable() {
    let rules = load_rules();
    let skirm = ukind(&rules, "skirmisher");
    for (name, click, walls) in [
        ("arrival", (30, 30), false),
        ("unreachable", (20, 30), true),
    ] {
        let mut m = M::new(rules.clone(), 54);
        if walls {
            // A walled pocket round (20, 30): P0's Watchtower sites.
            m.ok(RING.iter().map(|&d| ring_tower(&rules, d)).collect());
        }
        m.ok(vec![spawn(skirm, 6, 30, 1)]); // UnitId(1)
        m.ok(vec![Command::AttackMove {
            units: vec![UnitId(1)],
            target: FxVec2::from_ints(click.0, click.1),
        }]);
        assert!(matches!(
            m.unit(1).unwrap().intent,
            Intent::AttackMove { .. }
        ));
        let mut arrived = false;
        for _ in 0..600 {
            let events = m.step(1);
            if events.contains(&SimEvent::UnitArrived { unit: UnitId(1) }) {
                arrived = true;
                break;
            }
        }
        assert!(arrived, "{name}: arrived");
        assert!(
            matches!(m.unit(1).unwrap().intent, Intent::AttackMove { .. }),
            "{name}: the intent ends at the next think"
        );
        m.step(1);
        let u = m.unit(1).unwrap();
        assert_eq!(
            (u.intent, u.order.is_none()),
            (Intent::Plain, true),
            "{name}"
        );
        let quiet = trace(&mut m, 1, 200).0;
        assert_eq!(
            (quiet.requests, quiet.with_order),
            (0, 0),
            "{name}: no re-issue"
        );
        if walls {
            let map = m.view().map().clone();
            let here = map.tile_of(u.pos);
            assert_ne!(
                map.component_of(here),
                map.component_of(Tile::new(20, 30)),
                "the pocket is another component"
            );
        }
    }
}

/// A plain `Move` never engages, even past enemies in weapon range and
/// while one shoots at it, and neither does a rally walk; both guard once
/// they arrive.
#[test]
fn combat_move_does_not_engage() {
    let rules = load_rules();
    let (skirm, yeoman, bow) = (
        ukind(&rules, "skirmisher"),
        ukind(&rules, "yeoman"),
        ukind(&rules, "bowman"),
    );
    let mut m = M::new(rules.clone(), 55);
    // P1's Yeomen UnitId(1..=2) on the route and a Bowman UnitId(3) beside
    // it; P0's Skirmisher UnitId(4) walks past them.
    m.run_as(
        1,
        vec![
            spawn_for(P1, yeoman, 14, 31, 1),
            spawn_for(P1, yeoman, 20, 29, 1),
            spawn_for(P1, bow, 17, 33, 1),
        ],
    );
    let events = m.issue_tagged(
        vec![
            spawn(skirm, 6, 30, 1),
            Command::Move {
                units: vec![UnitId(4)],
                target: FxVec2::from_ints(38, 30),
                queue: false,
            },
        ],
        vec![],
        0,
    );
    let mut all = events;
    let mut walking = 0;
    let arrived = loop {
        let step = m.step_tagged(1);
        let done = step
            .iter()
            .any(|(_, e)| *e == SimEvent::UnitArrived { unit: UnitId(4) });
        all.extend(step);
        let u = m.unit(4).expect("survives the walk");
        if done {
            break m.sim.tick();
        }
        walking += 1;
        assert_eq!((u.target, u.intent), (None, Intent::Plain));
        assert!(walking < 600);
    };
    assert!(
        hits_by(&all, unit_t(4)).is_empty(),
        "never engaged while walking"
    );
    assert!(
        !hits_on(&all, unit_t(3), unit_t(4)).is_empty(),
        "though shot at"
    );
    let passed = |id: u32| m.unit(id).map(|u| u.pos);
    assert!(
        passed(1).is_some() && passed(2).is_some(),
        "the Yeomen live"
    );
    eprintln!("move: walked {walking} ticks, arrived at {arrived}");

    // A rally walk: P0's Muster Hall BuildingId(1) trains a Skirmisher that
    // walks to the rally point past P1's Yeoman UnitId(1), 2 tiles off its
    // way and 12 from the rally point.
    let mut m = M::new(rules.clone(), 55);
    m.run_as(1, vec![spawn_for(P1, yeoman, 24, 29, 1)]);
    m.ok(vec![
        place(
            bkind(&rules, "muster_hall"),
            MUSTER_HALL.0,
            MUSTER_HALL.1,
            true,
        ),
        grant(res(&rules, "grain"), 100),
    ]);
    m.ok(vec![
        Command::SetRally {
            building: BuildingId(1),
            target: FxVec2::from_ints(36, 27),
        },
        train(1, skirm),
    ]);
    let events = m.step_tagged(700);
    let spawned = events
        .iter()
        .find(|(_, e)| *e == SimEvent::UnitSpawned { unit: UnitId(2) })
        .map(|(t, _)| *t)
        .expect("trained");
    assert!(
        events
            .iter()
            .any(|(t, e)| *t > spawned && *e == SimEvent::UnitArrived { unit: UnitId(2) }),
        "walked to the rally point"
    );
    assert!(hits(&events.iter().map(|e| e.1.clone()).collect::<Vec<_>>()).is_empty());
    assert_eq!(m.hp(1), Some((40, 40)));
}

/// An idle soldier guards its post: it takes an enemy that comes into its
/// reach, chases and kills it, keeping its post, and walks back to the post
/// afterwards.
#[test]
fn combat_idle_soldier_guards_and_returns_to_post() {
    let rules = load_rules();
    let (skirm, yeoman) = (ukind(&rules, "skirmisher"), ukind(&rules, "yeoman"));
    let mut m = M::new(rules.clone(), 56);
    m.ok(vec![spawn(skirm, 6, 30, 1)]); // UnitId(1)
    m.ok(vec![Command::Move {
        units: vec![UnitId(1)],
        target: FxVec2::from_ints(20, 30),
        queue: false,
    }]);
    m.step(300);
    let post = m.unit(1).unwrap().post.expect("arrived").goal;
    // P1's Yeoman UnitId(2) appears 4 tiles off and walks away.
    m.run_as(
        1,
        vec![
            spawn_for(P1, yeoman, 24, 30, 1),
            Command::Move {
                units: vec![UnitId(2)],
                target: FxVec2::from_ints(26, 34),
                queue: false,
            },
        ],
    );
    let u = m.unit(1).unwrap();
    assert_eq!((u.target, u.intent), (Some(unit_t(2)), Intent::Plain));
    let events = m.step_tagged(300);
    assert!(died_at(&events, 2).is_some(), "chased and killed");
    let u = m.unit(1).unwrap();
    assert_eq!(u.post.map(|p| p.goal), Some(post), "the post is kept");
    assert_eq!((u.target, u.order.is_none()), (None, true));
    let arrive = Fx::from_ratio(25, 100);
    assert!(
        u.pos.dist_sq_i64(post) <= (arrive * arrive).to_bits(),
        "back at its post: {:?} vs {post:?}",
        u.pos
    );
}

/// A guard keeps a target its straight chase can reach. P0's own sites
/// block tiles 6..=7 x 30..=31. From (9, 31) the tile line to the Yeoman's
/// tile (6, 32) crossed the corner of (7, 31) exactly, so the guard dropped
/// the target. With a post it then flipped between chase and return every
/// tick; without one it froze 2.7 tiles away.
#[test]
fn combat_guard_keeps_a_target_its_straight_chase_can_reach() {
    let rules = load_rules();
    let (skirm, yeoman, tower) = (
        ukind(&rules, "skirmisher"),
        ukind(&rules, "yeoman"),
        bkind(&rules, "watchtower"),
    );
    for with_post in [false, true] {
        let mut m = M::new(rules.clone(), 58);
        m.ok(vec![
            place(tower, 6, 30, false),
            place(tower, 7, 30, false),
            place(tower, 6, 31, false),
            place(tower, 7, 31, false),
        ]);
        m.run_as(1, vec![spawn_for(P1, yeoman, 6, 32, 1)]); // UnitId(1)
        if with_post {
            m.ok(vec![spawn(skirm, 20, 31, 1)]); // UnitId(2), out of reach
            m.ok(vec![Command::Move {
                units: vec![UnitId(2)],
                target: FxVec2::from_ints(11, 31),
                queue: false,
            }]);
        } else {
            m.ok(vec![spawn(skirm, 11, 31, 1)]);
        }
        let (mut last, mut changes, mut died) = (None, 0, false);
        for _ in 0..600 {
            if died_at(&m.step_tagged(1), 1).is_some() {
                died = true;
                break;
            }
            let t = m.unit(2).unwrap().target;
            if t != last {
                changes += 1;
                last = t;
            }
        }
        assert!(
            died,
            "post {with_post}: the guard reaches and kills the Yeoman"
        );
        assert_eq!(changes, 1, "post {with_post}: taken once and kept");
    }
}

/// A complete Watchtower and a complete Town shoot enemy units in range of
/// their footprints (range plus the unit's radius, edge of the footprint
/// to the unit's centre: 7.35 tiles for a Yeoman), the nearest first, 15
/// a shot every 30 ticks, and nothing else: not a unit 7.5 tiles off, not
/// their owner's units, not enemy buildings; a Watchtower site does not
/// shoot. Between units at exactly the same distance a shooter draws once
/// and takes `ties[rng_index(n)]` in `UnitId` order.
#[test]
fn combat_watchtower_and_town_shoot_enemy_units_in_range() {
    let rules = load_rules();
    let (yeoman, tower, town) = (
        ukind(&rules, "yeoman"),
        bkind(&rules, "watchtower"),
        bkind(&rules, "town"),
    );
    let mut m = M::new(rules.clone(), 57);
    // P1's Town BuildingId(1) (tiles 4..=6 x 30..=32), Watchtower
    // BuildingId(2) at (36, 32) and Watchtower site BuildingId(3) at
    // (20, 38), far from both; P1's own Yeoman UnitId(1) beside the tower.
    // P0's Yeomen: UnitId(2) 6.5 tiles east of the Town, 3 at 7.5; 4 at
    // 6.5 west of the tower, 5 at 7.5; 6 beside the site; P0's Watchtower
    // site BuildingId(4) beside the tower.
    m.run_as(
        1,
        vec![
            place_for(P1, town, 4, 30, true),
            place_for(P1, tower, 36, 32, true),
            place_for(P1, tower, 20, 38, false),
            spawn_for(P1, yeoman, 37, 33, 1),
        ],
    );
    let events = m.issue_tagged(
        vec![
            spawn(yeoman, 13, 31, 1),
            spawn(yeoman, 14, 31, 1),
            spawn(yeoman, 29, 32, 1),
            spawn(yeoman, 28, 32, 1),
            spawn(yeoman, 21, 38, 1),
            place(tower, 37, 31, false),
        ],
        vec![],
        200,
    );
    let by_town = hits_by(&events, building_t(1));
    let by_tower = hits_by(&events, building_t(2));
    assert!(by_town.iter().all(|h| h.1 == 15) && by_tower.iter().all(|h| h.1 == 15));
    let targets = |b: u32| {
        hits(&events.iter().map(|e| e.1.clone()).collect::<Vec<_>>())
            .into_iter()
            .filter(|(a, _, _)| *a == building_t(b))
            .map(|(_, t, _)| t)
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(
        targets(1),
        [unit_t(2)].into(),
        "the Town: only the one in range"
    );
    assert_eq!(
        targets(2),
        [unit_t(4)].into(),
        "the tower: only the one in range"
    );
    assert!(targets(3).is_empty(), "a site does not shoot");
    // 40 hp at 15 a shot: the third kills, 30 ticks apart.
    assert_eq!(by_town.len(), 3);
    assert_eq!(
        gaps(&by_town.iter().map(|h| h.0).collect::<Vec<_>>()),
        [30, 30]
    );
    assert_eq!(died_at(&events, 2), Some(by_town[2].0));
    assert_eq!(died_at(&events, 4), Some(by_tower[2].0));
    for id in [1, 3, 5, 6] {
        assert_eq!(m.hp(id), Some((40, 40)), "unit {id}");
    }
    assert_eq!(m.view().building(BuildingId(4)).unwrap().hp, 500);

    // Two P0 Yeomen 2.5 tiles either side of P1's tower (tiles 30..=30):
    // one draw on the first shot.
    let mut shot_first = std::collections::BTreeSet::new();
    for seed in 0..20 {
        let mut m = M::new(rules.clone(), seed);
        m.run_as(1, vec![place_for(P1, tower, 30, 32, true)]);
        let events = m.issue_tagged(
            vec![spawn(yeoman, 27, 32, 1), spawn(yeoman, 33, 32, 1)],
            vec![],
            0,
        );
        let (hash, draws) = rng_after(seed, 1);
        assert_eq!(m.sim.sub_hashes().rng, hash, "seed {seed}: one draw");
        let shot = hits_by(&events, building_t(1));
        assert_eq!(shot.len(), 1);
        let want = unit_t(1 + index_of(draws[0], 2));
        let first = hits(&events.iter().map(|e| e.1.clone()).collect::<Vec<_>>())[0].1;
        assert_eq!(first, want, "seed {seed}");
        shot_first.insert(want);
    }
    assert_eq!(shot_first.len(), 2, "the seed decides");
}

/// A Town at 0 HP does not shoot: it shot in the tick the volley brought
/// it to 0 (hits are simultaneous), and never again, though 17 enemy
/// Mangonels stand in its range.
#[test]
fn combat_town_at_zero_hp_does_not_shoot() {
    let rules = load_rules();
    let mut m = town_at_zero(&rules, 58);
    let town = m.view().building(BuildingId(1)).unwrap().clone();
    let rect = (
        FxVec2::from_ints(i32::from(town.tile.x), i32::from(town.tile.y)),
        FxVec2::from_ints(i32::from(town.tile.x) + 3, i32::from(town.tile.y) + 3),
    );
    let range = Fx::from_int(7) + Fx::from_ratio(50, 100);
    let in_range = m
        .view()
        .units()
        .filter(|u| {
            let p = FxVec2::new(
                u.pos.x.clamp(rect.0.x, rect.1.x),
                u.pos.y.clamp(rect.0.y, rect.1.y),
            );
            u.pos.dist_sq_i64(p) <= (range * range).to_bits()
        })
        .count();
    assert_eq!(in_range, 17, "every Mangonel in its range");
    let later = m.step_tagged(300);
    assert!(hits_by(&later, building_t(1)).is_empty(), "silent at 0 HP");
}

/// An explicit chase at the edge of its reach does not flip between a
/// direct chase and a path chase (each flip to a path would be a new path
/// request): a Skirmisher ordered onto a runner that starts just beyond
/// its reach (7 tiles off, the reach being 6.7 centre to centre) or just
/// inside it (6 tiles) and walks off at the same speed, a slower one or a
/// faster one, switches mode only when the runner crosses the edge: at most
/// once against an equal or slower runner (path to direct, once it closes
/// in), at most twice against a faster one (out of reach as it pulls away,
/// back in when it stops at its goal). Its path requests are one per goal
/// (the first walk and each re-goal) plus the periodic repath every
/// `repath_interval` ticks of walking.
#[test]
fn combat_attack_at_the_edge_of_reach_does_not_flip_flop() {
    let rules = load_rules();
    let skirm = ukind(&rules, "skirmisher");
    for (runner, start_x) in [
        ("skirmisher", 17),
        ("skirmisher", 16),
        ("yeoman", 17),
        ("yeoman", 16),
        ("outrider", 17),
        ("outrider", 16),
    ] {
        let mut m = M::new(rules.clone(), 60);
        // P1's runner UnitId(1); P0's Skirmisher UnitId(2) appears 6 or 7
        // tiles west of it, ordered onto it in the tick the runner sets off
        // east (a plain Move: it never engages).
        m.run_as(
            1,
            vec![spawn_for(P1, ukind(&rules, runner), start_x, 30, 1)],
        );
        m.issue_tagged(
            vec![spawn(skirm, 10, 30, 1), attack(&[2], unit_t(1))],
            vec![Command::Move {
                units: vec![UnitId(1)],
                target: FxVec2::from_ints(42, 34),
                queue: false,
            }],
            0,
        );
        let mut mode = m.unit(2).unwrap().order.is_some();
        let mut switches = 0;
        let (t, _) = {
            let mut t = Trace::default();
            let mut had_request = m.view().pathing().has_request(UnitId(2));
            for _ in 0..400 {
                m.step(1);
                let Some(u) = m.unit(2) else { break };
                if u.intent != Intent::Attack {
                    break;
                }
                let path = u.order.is_some();
                switches += u32::from(path != mode);
                mode = path;
                t.with_order += u32::from(path);
                if let Some(o) = &u.order
                    && t.goals.last() != Some(&o.goal)
                {
                    t.goals.push(o.goal);
                }
                let request = m.view().pathing().has_request(UnitId(2));
                t.requests += u32::from(request && !had_request);
                had_request = request;
            }
            (t, ())
        };
        let goals = u32::try_from(t.goals.len()).unwrap();
        let most = if runner == "outrider" { 2 } else { 1 };
        assert!(
            switches <= most,
            "{runner} from {start_x}: {switches} switches, {t:?}"
        );
        let repaths = t.with_order / m.rules().repath_interval_ticks();
        assert!(
            t.requests <= goals + repaths,
            "{runner} from {start_x}: {t:?}"
        );
        eprintln!(
            "edge of reach: {runner} from x {start_x}: {switches} switch(es), {} request(s), {goals} goal(s), {} ticks on a path",
            t.requests, t.with_order
        );
    }
}

/// Snapshots mid-skirmish restore to sims that match the uninterrupted run
/// on every tick, events included: on the `combat_skirmish` stream during
/// the line fight (attack-movers engaged with their orders kept, stopped
/// Bowmen guarding with targets, the Town's cooldown running, jittered
/// positions), with 50 jittered units spawned and attack-moving after the
/// restore (`UnitId(99..=148)`, so this run stops before the stream's
/// Mangonel beats, which name `UnitId(99)`), and again, without them, while
/// the Mangonel batters the Watchtower, through the destruction with
/// searches suspended.
#[test]
fn combat_restore_mid_skirmish_matches_uninterrupted() {
    use sim::scenarios::{COMBAT_SKIRMISH_TICKS, combat_skirmish, commands_at};
    let rules = load_rules();
    let (setup, stream) = combat_skirmish(&rules);
    let skirm = ukind(&rules, "skirmisher");
    // The extra 50 (UnitId(99..=148)) during tick 205, after the first
    // restore point; seqs past the stream's.
    let extra = |tick: u32| -> Vec<PlayerCommand> {
        if tick != 205 {
            return Vec::new();
        }
        let side = |p: u8, x: i32, goal: (i32, i32)| {
            [
                PlayerCommand::new(
                    PlayerId(p),
                    1000,
                    Command::DebugSpawnJittered {
                        owner: PlayerId(p),
                        kind: skirm,
                        at: FxVec2::from_ints(x, 26),
                        count: 25,
                        jitter_x100: 100,
                    },
                ),
                PlayerCommand::new(
                    PlayerId(p),
                    1001,
                    Command::AttackMove {
                        units: if p == 0 {
                            (99..=123).map(UnitId).collect()
                        } else {
                            (124..=148).map(UnitId).collect()
                        },
                        target: FxVec2::from_ints(goal.0, goal.1),
                    },
                ),
            ]
        };
        let mut cmds: Vec<PlayerCommand> = side(0, 6, (30, 30)).into();
        cmds.extend(side(1, 33, (8, 30)));
        cmds
    };
    for (snap_at, end) in [(200, 1000), (2200, 2500)] {
        assert!(end <= COMBAT_SKIRMISH_TICKS);
        let at = |tick: u32| -> Vec<PlayerCommand> {
            let mut cmds = commands_at(&stream, tick).to_vec();
            if snap_at == 200 {
                cmds.extend(extra(tick));
            }
            cmds
        };
        let mut a = Sim::new(setup.clone(), rules.clone(), Box::new(NoAi));
        for tick in 0..snap_at {
            a.step(&at(tick));
        }
        a.drain_events();
        {
            let view = a.view();
            let tick = a.tick();
            if snap_at == 200 {
                assert!(
                    view.units()
                        .any(|u| matches!(u.intent, Intent::AttackMove { .. })
                            && u.order.is_some()
                            && u.target.is_some()),
                    "an engaged attack-mover"
                );
                assert!(
                    view.units()
                        .any(|u| u.intent == Intent::Plain && u.target.is_some()),
                    "a guard with a target"
                );
                assert!(
                    view.buildings().any(|b| b.ready_at > tick),
                    "a shooter's cooldown running"
                );
                assert!(view.units().any(|u| u.damage_taken > 0));
            } else {
                let m = view.unit(UnitId(99)).expect("the Mangonel");
                assert!(m.intent == Intent::Attack && m.ready_at > tick, "{m:?}");
                assert!(view.building(BuildingId(4)).unwrap().hp < 500);
            }
        }
        let mut b = Sim::new(setup.clone(), rules.clone(), Box::new(NoAi));
        b.restore(&a.snapshot()).unwrap();
        assert_eq!((a.hash(), a.sub_hashes()), (b.hash(), b.sub_hashes()));
        let (mut hit, mut spawned) = (0, 0);
        for tick in snap_at..end {
            a.step(&at(tick));
            b.step(&at(tick));
            assert_eq!(a.hash(), b.hash(), "restored at {snap_at}, tick {tick}");
            assert_eq!(a.sub_hashes(), b.sub_hashes(), "tick {tick}");
            let ea = a.drain_events();
            assert_eq!(ea, b.drain_events(), "tick {tick}");
            hit += hits(&ea).len();
            spawned += ea
                .iter()
                .filter(
                    |e| matches!(e, SimEvent::UnitSpawned { unit } if (99..=148).contains(&unit.0)),
                )
                .count();
        }
        assert!(hit > 0, "fighting after the restore at {snap_at}");
        if snap_at == 200 {
            assert_eq!(spawned, 50, "the 50 spawned after the restore");
        } else {
            assert!(a.view().building(BuildingId(4)).is_none(), "destroyed");
        }
    }
}

/// Arms raises the hit points and damage of the owner's military units,
/// living ones included: a wounded Skirmisher keeps the damage it took and
/// gains the new maximum's difference, and its hits grow at once. The
/// owner's civilians and the other player's units are untouched. Nobody
/// fights except where the test says so: idle armed units guard, so the
/// Skirmisher is wounded while it walks a plain `Move` (which never
/// engages), its attacker then walks away, and its target, out of its
/// reach between the hits, is an unarmed Supply Wain (no counter row
/// against the Skirmisher, so a hit is its raw damage).
#[test]
fn tech_arms_raises_military_hp_and_damage_including_living_units() {
    let rules = load_rules();
    let (skirm, yeoman, wain) = (
        ukind(&rules, "skirmisher"),
        ukind(&rules, "yeoman"),
        ukind(&rules, "supply_wain"),
    );
    assert_eq!(
        rules.counter_permille("skirmisher", &rules::Defender::Kind("supply_wain".into())),
        1000
    );
    let mut m = M::new(rules.clone(), 46);
    let home = (10, 40);
    let mv = |id: u32, to: (i32, i32)| Command::Move {
        units: vec![UnitId(id)],
        target: FxVec2::from_ints(to.0, to.1),
        queue: false,
    };
    // P0's Yeoman is UnitId(1); P1's Supply Wain UnitId(2) (P0's target, 8
    // tiles east of P0's Skirmisher's home) and Skirmisher UnitId(3). P0's
    // Skirmisher UnitId(4) is spawned beside P1's while walking home; P1's
    // hits it once in that tick and then walks off east.
    m.ok(vec![spawn(yeoman, 4, 26, 1)]);
    m.run_as(1, vec![spawn_for(P1, wain, 18, 40, 1)]);
    m.run_as(1, vec![spawn_for(P1, skirm, 11, 30, 1)]);
    let events = m.issue_tagged(
        vec![spawn(skirm, 10, 30, 1), mv(4, home)],
        vec![attack(&[3], unit_t(4))],
        0,
    );
    assert_eq!(hits_by(&events, unit_t(3)).len(), 1);
    m.run_as(1, vec![mv(3, (40, 26))]);
    let events = m.step_tagged(200);
    assert!(hits(&events.into_iter().map(|e| e.1).collect::<Vec<_>>()).is_empty());
    assert_eq!(m.hp(4), Some((70, 80)), "wounded");
    assert_eq!(m.hp(1), Some((40, 40)));
    let mut seen = Vec::new();
    for (level, id) in ["arms_1", "arms_2", "arms_3"].iter().enumerate() {
        m.ok(vec![debug_research(tech(&rules, id))]);
        let (hp, max) = m.hp(4).unwrap();
        assert_eq!(max - hp, 10, "{id}: the damage taken is kept");
        assert_eq!(m.hp(1), Some((40, 40)), "{id}: civilians unchanged");
        assert_eq!(m.hp(3), Some((80, 80)), "{id}: P1 unchanged");
        assert_eq!(m.hp(2).unwrap().1, 150, "{id}: P1 unchanged");
        // P0's Skirmisher hits P1's Supply Wain, then walks home again.
        let events = m.issue_tagged(vec![attack(&[4], unit_t(2))], vec![], 120);
        let landed = hits_by(&events, unit_t(4));
        m.ok(vec![mv(4, home)]);
        m.step(100);
        let damage = m.view().unit_stats(P0, skirm).attack.unwrap().damage;
        assert!(landed.iter().all(|h| h.1 == damage), "{id}: {landed:?}");
        assert!(!landed.is_empty(), "{id}");
        assert!(
            m.unit(4).unwrap().target.is_none(),
            "{id}: home, the Wain is out of its reach"
        );
        seen.push((level + 1, max, damage));
    }
    // floor(80 * 1.1) = 88, then 96, 105; floor(10 * 1.1) = 11, then 12, 13.
    assert_eq!(seen, [(1, 88, 11), (2, 96, 12), (3, 105, 13)]);
    eprintln!("arms: skirmisher (level, hp, damage) {seen:?}");
}

/// Every Arms level strictly raises the hit points of every military kind
/// and the damage of every one that attacks; range, reach and cooldown do
/// not change, and civilians do not change at all.
#[test]
fn tech_every_arms_level_strictly_raises_every_military_stat() {
    let rules = load_rules();
    let mut m = M::new(rules.clone(), 47);
    let kinds: Vec<UnitKindId> = (0..rules.units.len())
        .map(|k| UnitKindId(u16::try_from(k).unwrap()))
        .collect();
    let snapshot = |m: &M| -> Vec<sim::UnitStats> {
        kinds.iter().map(|k| m.view().unit_stats(P0, *k)).collect()
    };
    let mut levels = vec![snapshot(&m)];
    for id in ["arms_1", "arms_2", "arms_3"] {
        m.ok(vec![debug_research(tech(&rules, id))]);
        levels.push(snapshot(&m));
    }
    for (k, def) in rules.units.iter().enumerate() {
        let military = def.class != rules::UnitClass::Civilian;
        for w in levels.windows(2) {
            let (a, b) = (w[0][k], w[1][k]);
            if military {
                assert!(b.max_hp > a.max_hp, "{}: {a:?} -> {b:?}", def.id);
            } else {
                assert_eq!(a, b, "{}: civilian", def.id);
            }
            match (a.attack, b.attack) {
                (Some(x), Some(y)) => {
                    assert!(y.damage > x.damage, "{}", def.id);
                    assert_eq!(
                        (x.range, x.reach, x.cooldown_ticks),
                        (y.range, y.reach, y.cooldown_ticks),
                        "{}",
                        def.id
                    );
                }
                (None, None) => {}
                other => panic!("{}: {other:?}", def.id),
            }
        }
        assert_eq!(
            levels[0][k].max_hp,
            i32::try_from(def.hp).unwrap(),
            "{}: Arms 0 is the data",
            def.id
        );
        assert_eq!(
            m.view().unit_stats(P1, kinds[k]),
            levels[0][k],
            "{}: P1 researched nothing",
            def.id
        );
    }
    let table: Vec<(String, Vec<i32>)> = rules
        .units
        .iter()
        .enumerate()
        .map(|(k, d)| (d.id.clone(), levels.iter().map(|l| l[k].max_hp).collect()))
        .collect();
    eprintln!("arms hp by level: {table:?}");
}

// ---------------------------------------------------------------------------
// Acceptance 2: the counter triangle over 20 seeds (`counters_*`).
// ---------------------------------------------------------------------------

/// How a counter duel ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct DuelOutcome {
    /// The side with units left; `None` after a timeout, which is no win.
    winner: Option<PlayerId>,
    /// Units left on the winning side (on both after a timeout).
    survivors: u32,
    /// The tick in which the last unit of the losing side died, or the
    /// timeout.
    end_tick: u32,
    /// The survivors' hit points, summed.
    survivor_hp_sum: i32,
}

/// One 10 vs 10 duel, P0's kind `a` against P1's kind `b`, on
/// `sim::scenarios::counter_duel_setup` with `seed`: every 100 ticks each
/// side's idle survivors (no order, no target) attack-move toward the
/// enemy's live centroid, so no duel stalls; it ends in the tick a side is
/// left with no unit, or after `COUNTER_DUEL_TICKS` without a winner.
fn duel(rules: &Rules, a: UnitKindId, b: UnitKindId, seed: u64) -> DuelOutcome {
    use sim::scenarios::{COUNTER_DUEL_TICKS, commands_at, counter_duel_setup};
    let (setup, stream) = counter_duel_setup(rules, a, b, seed);
    let mut sim = Sim::new(setup, rules.clone(), Box::new(NoAi));
    // Past every seq the stream used.
    let mut seq = [1000u32; 2];
    let side = |view: &SimView<'_>, p: PlayerId| -> Vec<sim::Unit> {
        view.units().filter(|u| u.owner == p).cloned().collect()
    };
    let mut started = false;
    for tick in 0..COUNTER_DUEL_TICKS {
        let mut cmds = commands_at(&stream, tick).to_vec();
        if started && tick % 100 == 0 {
            let view = sim.view();
            for (p, enemy) in [(P0, P1), (P1, P0)] {
                let idle: Vec<UnitId> = side(&view, p)
                    .iter()
                    .filter(|u| u.order.is_none() && u.target.is_none())
                    .map(|u| u.id)
                    .collect();
                let enemies = side(&view, enemy);
                if idle.is_empty() || enemies.is_empty() {
                    continue;
                }
                let n = i32::try_from(enemies.len()).unwrap();
                let sum = enemies.iter().fold(FxVec2::ZERO, |s, u| s + u.pos);
                let centroid = FxVec2::new(sum.x.div_int(n), sum.y.div_int(n));
                let i = usize::from(p.0);
                cmds.push(PlayerCommand::new(
                    p,
                    seq[i],
                    Command::AttackMove {
                        units: idle,
                        target: centroid,
                    },
                ));
                seq[i] += 1;
            }
        }
        sim.step(&cmds);
        let view = sim.view();
        let (p0, p1) = (side(&view, P0), side(&view, P1));
        started |= !p0.is_empty() && !p1.is_empty();
        if !started {
            continue;
        }
        let hp = |units: &[sim::Unit]| -> i32 {
            units
                .iter()
                .map(|u| view.unit_hp(u.id).expect("alive").0)
                .sum()
        };
        let count = |units: &[sim::Unit]| u32::try_from(units.len()).unwrap();
        let winner = match (p0.is_empty(), p1.is_empty()) {
            (false, true) => Some((P0, &p0)),
            (true, false) => Some((P1, &p1)),
            (true, true) => {
                return DuelOutcome {
                    winner: None,
                    survivors: 0,
                    end_tick: tick,
                    survivor_hp_sum: 0,
                };
            }
            (false, false) => None,
        };
        if let Some((w, units)) = winner {
            return DuelOutcome {
                winner: Some(w),
                survivors: count(units),
                end_tick: tick,
                survivor_hp_sum: hp(units),
            };
        }
    }
    let view = sim.view();
    let all: Vec<sim::Unit> = view.units().cloned().collect();
    DuelOutcome {
        winner: None,
        survivors: u32::try_from(all.len()).unwrap(),
        end_tick: COUNTER_DUEL_TICKS,
        survivor_hp_sum: all.iter().map(|u| view.unit_hp(u.id).unwrap().0).sum(),
    }
}

/// Seeds per counter matchup (acceptance 2).
const COUNTER_SEEDS: u64 = 20;

/// Run `a` (P0) against `b` (P1) over `COUNTER_SEEDS` seeds and print the
/// matchup's table: wins (a win for `a` counts only with at least
/// `min_survivors` left), survivors min/mean/max over `a`'s wins, the range
/// of end ticks and the number of distinct outcomes. Returns the outcomes
/// and the counted wins.
fn counter_matchup(a: &str, b: &str, min_survivors: u32) -> (Vec<DuelOutcome>, usize) {
    let rules = shared_rules();
    let (ka, kb) = (ukind(rules, a), ukind(rules, b));
    let outcomes: Vec<DuelOutcome> = (0..COUNTER_SEEDS)
        .map(|seed| duel(rules, ka, kb, seed))
        .collect();
    let won: Vec<&DuelOutcome> = outcomes
        .iter()
        .filter(|o| o.winner == Some(P0) && o.survivors >= min_survivors)
        .collect();
    let survivors: Vec<u32> = outcomes
        .iter()
        .filter(|o| o.winner == Some(P0))
        .map(|o| o.survivors)
        .collect();
    let distinct = outcomes
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let ends: Vec<u32> = outcomes.iter().map(|o| o.end_tick).collect();
    // The mean in hundredths, printed as a decimal (no floats in the sim
    // crate, tests included).
    let mean = |v: &[u32]| {
        let x100 = if v.is_empty() {
            0
        } else {
            v.iter().sum::<u32>() * 100 / u32::try_from(v.len()).unwrap()
        };
        format!("{}.{:02}", x100 / 100, x100 % 100)
    };
    let losses = outcomes.iter().filter(|o| o.winner == Some(P1)).count();
    let timeouts = outcomes.iter().filter(|o| o.winner.is_none()).count();
    eprintln!(
        "counters: 10 {a} vs 10 {b} over {COUNTER_SEEDS} seeds: {a} win {}/{COUNTER_SEEDS} (with >= {min_survivors} survivors), {b} win {losses}, timeouts {timeouts}; {a} survivors min {} / mean {} / max {}; end ticks {}..={}; distinct outcomes {distinct}/{COUNTER_SEEDS}",
        won.len(),
        survivors.iter().min().copied().unwrap_or(0),
        mean(&survivors),
        survivors.iter().max().copied().unwrap_or(0),
        ends.iter().min().unwrap(),
        ends.iter().max().unwrap(),
    );
    for (seed, o) in outcomes.iter().enumerate() {
        eprintln!("  seed {seed:2}: {o:?}");
    }
    let wins = won.len();
    (outcomes, wins)
}

/// The seed changes the results, and the test asserts it: at least two
/// distinct outcomes over the 20 seeds. (The final hash cannot show it:
/// the seed itself is hashed state.)
fn assert_outcomes_differ(name: &str, outcomes: &[DuelOutcome]) {
    let distinct = outcomes
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    assert!(distinct >= 2, "{name}: every seed gave {:?}", outcomes[0]);
}

/// Acceptance 2: 10 Skirmishers beat 10 Bowmen in at least 18 of 20 seeds
/// with at least 4 survivors.
#[test]
fn counters_skirmishers_beat_bowmen_with_at_least_four_survivors() {
    let (outcomes, wins) = counter_matchup("skirmisher", "bowman", 4);
    assert!(wins >= 18, "{wins}/20: {outcomes:?}");
    assert_outcomes_differ("skirmisher vs bowman", &outcomes);
}

/// Acceptance 2: 10 Bowmen beat 10 Shieldbearers in at least 18 of 20.
#[test]
fn counters_bowmen_beat_shieldbearers() {
    let (outcomes, wins) = counter_matchup("bowman", "shieldbearer", 1);
    assert!(wins >= 18, "{wins}/20: {outcomes:?}");
    assert_outcomes_differ("bowman vs shieldbearer", &outcomes);
}

/// Acceptance 2: 10 Shieldbearers beat 10 Outriders in at least 18 of 20.
#[test]
fn counters_shieldbearers_beat_outriders() {
    let (outcomes, wins) = counter_matchup("shieldbearer", "outrider", 1);
    assert!(wins >= 18, "{wins}/20: {outcomes:?}");
    assert_outcomes_differ("shieldbearer vs outrider", &outcomes);
}

/// Acceptance 2: 10 Outriders beat 10 Bowmen in at least 18 of 20.
#[test]
fn counters_outriders_beat_bowmen() {
    let (outcomes, wins) = counter_matchup("outrider", "bowman", 1);
    assert!(wins >= 18, "{wins}/20: {outcomes:?}");
    assert_outcomes_differ("outrider vs bowman", &outcomes);
}

/// The seed moves every spawned unit: `DebugSpawnJittered` puts each duel
/// unit within a tile of its tile centre on both axes, the same seed at the
/// same point and the 20 seeds at 20 different sets of points. It draws
/// exactly two numbers per unit from the sim's RNG (`dx`, then `dy`), and a
/// rejected one draws nothing and changes nothing.
#[test]
fn counters_seed_changes_spawn_jitter() {
    use rand_core::{Rng, SeedableRng};
    use sim::scenarios::{
        COUNTER_DUEL_P0_RANKS, COUNTER_DUEL_P1_RANKS, COUNTER_DUEL_ROWS, commands_at,
        counter_duel_setup,
    };
    let rules = shared_rules();
    let (skirm, bow) = (ukind(rules, "skirmisher"), ukind(rules, "bowman"));
    // The tile centre each unit is spawned at, in id order.
    let centres: Vec<FxVec2> = COUNTER_DUEL_P0_RANKS
        .iter()
        .chain(&COUNTER_DUEL_P1_RANKS)
        .flat_map(|&x| {
            COUNTER_DUEL_ROWS
                .iter()
                .map(move |&y| Tile::new(u16::try_from(x).unwrap(), u16::try_from(y).unwrap()))
        })
        .map(|t| sim::Map::open(128, 128).center_of(t))
        .collect();
    let spawn = |seed: u64| {
        let (setup, stream) = counter_duel_setup(rules, skirm, bow, seed);
        let delay = setup.cmd_delay;
        let mut sim = Sim::new(setup, rules.clone(), Box::new(NoAi));
        for tick in 0..=delay {
            sim.step(commands_at(&stream, tick));
        }
        let positions: Vec<FxVec2> = sim.view().units().map(|u| u.pos).collect();
        (positions, sim.sub_hashes().rng)
    };
    let mut seen = std::collections::BTreeSet::new();
    let one_tile = Fx::from_int(1);
    for seed in 0..COUNTER_SEEDS {
        let (positions, rng) = spawn(seed);
        assert_eq!(
            (positions.clone(), rng),
            spawn(seed),
            "seed {seed}: reproducible"
        );
        assert_eq!(positions.len(), centres.len());
        for (p, c) in positions.iter().zip(&centres) {
            assert!(
                (p.x - c.x).abs() <= one_tile && (p.y - c.y).abs() <= one_tile,
                "seed {seed}: {p:?} from {c:?}"
            );
        }
        assert!(positions.iter().zip(&centres).any(|(p, c)| p != c));
        // Two draws per unit and nothing else.
        let mut expected = rand_pcg::Pcg32::seed_from_u64(seed);
        for _ in 0..2 * positions.len() {
            expected.next_u32();
        }
        assert_eq!(rng, sim::hash::hash_value(&expected), "seed {seed}");
        seen.insert(positions);
    }
    assert_eq!(
        seen.len(),
        usize::try_from(COUNTER_SEEDS).unwrap(),
        "every seed jitters differently"
    );

    // A rejected DebugSpawnJittered draws nothing and changes nothing.
    let jittered = |owner: PlayerId, kind: UnitKindId| Command::DebugSpawnJittered {
        owner,
        kind,
        at: FxVec2::from_ints(10, 30),
        count: 3,
        jitter_x100: 100,
    };
    let field = || M::new(rules.clone(), 3);
    rejected_without_change(
        "unknown kind",
        &field,
        0,
        jittered(P0, UnitKindId(99)),
        RejectReason::UnknownUnitKind,
    );
    rejected_without_change(
        "unknown player",
        &field,
        0,
        jittered(PlayerId(7), skirm),
        RejectReason::UnknownPlayer,
    );
    let skirmish = || M {
        sim: Sim::new(
            MatchSetup::skirmish(rules, 3),
            rules.clone(),
            Box::new(NoAi),
        ),
        seq: [0; 2],
    };
    rejected_without_change(
        "debug commands off",
        &skirmish,
        0,
        jittered(P0, skirm),
        RejectReason::DebugCommandsDisabled,
    );
}

/// Acceptance 2, last clause: a Watchtower outranges Skirmishers. In the
/// data its range is longer than the Skirmisher's, and over 20 seeds one P0
/// Skirmisher, jittered by the seed and ordered to `Attack` a complete P1
/// Watchtower from 12 tiles, is hit by the tower strictly before it lands
/// its own first hit (a Skirmisher that never hits would count as later).
#[test]
fn counters_watchtower_outranges_skirmishers() {
    let rules = shared_rules();
    let tower_kind = bkind(rules, "watchtower");
    let tower = rules
        .building_kind(tower_kind.0)
        .unwrap()
        .attack
        .clone()
        .expect("the Watchtower shoots");
    let skirm = ukind(rules, "skirmisher");
    let blade = rules.units[usize::from(skirm.0)]
        .attack
        .clone()
        .expect("the Skirmisher attacks");
    assert!(
        tower.range_tiles_x100 > blade.range_tiles_x100,
        "{tower:?} vs {blade:?}"
    );
    let mut table = Vec::new();
    for seed in 0..COUNTER_SEEDS {
        let mut m = M::new(rules.clone(), seed);
        // P1's Watchtower is BuildingId(1), centred on (30.5, 32.5); P0's
        // Skirmisher UnitId(1) is spawned 12 tiles west of it and ordered
        // to attack it in the same tick.
        m.run_as(1, vec![place_for(P1, tower_kind, 30, 32, true)]);
        let events = m.issue_tagged(
            vec![
                Command::DebugSpawnJittered {
                    owner: P0,
                    kind: skirm,
                    at: FxVec2::from_ints(18, 32),
                    count: 1,
                    jitter_x100: 100,
                },
                attack(&[1], building_t(1)),
            ],
            vec![],
            600,
        );
        let by_tower = hits_by(&events, building_t(1));
        let by_skirm = hits_by(&events, unit_t(1));
        let first_tower = by_tower.first().map(|h| h.0).expect("the tower shoots");
        let first_skirm = by_skirm.first().map_or(u32::MAX, |h| h.0);
        assert!(
            first_tower < first_skirm,
            "seed {seed}: tower {first_tower}, skirmisher {first_skirm}"
        );
        assert!(by_tower.iter().all(|(_, d)| *d == 15), "{by_tower:?}");
        table.push((seed, first_tower, first_skirm, by_tower.len()));
    }
    eprintln!(
        "watchtower vs skirmisher, (seed, tower's first hit, skirmisher's first hit, tower hits): {table:?}"
    );
}

// ---------------------------------------------------------------------------
// The M4a combat golden: `combat_skirmish`.
// ---------------------------------------------------------------------------

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Verify `fixtures/<name>.eonreplay` and compare its final hash with
/// `fixtures/<name>.hash` (as `m1.rs`, `m3a.rs` and `m3b.rs` do).
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

#[test]
fn golden_combat_skirmish() {
    golden("combat_skirmish");
}

/// The combat golden's beats, from the real sim: P0 researches Letters I
/// and then Trade I at the Letters price, cancels the Statecraft I queued
/// behind it (which never completes), enters the Masonry Age and trains a
/// Mangonel (`UnitId(99)`, made by `Train`, not by a debug spawn); the two
/// jittered lines attack-move into each other beside P1's Town and units
/// die on both sides, the Town shoots, and the mid-fight Stop and Move are
/// applied (nothing in the stream is rejected); the Mangonel destroys the
/// Watchtower with five hits from out of its range while a path search is
/// suspended, the footprint is passable again and the Mangonel stands down.
#[test]
fn combat_skirmish_beats_happen() {
    use sim::scenarios::{
        COMBAT_SKIRMISH_MANGONEL_TICK, COMBAT_SKIRMISH_SATURATE_TICK, COMBAT_SKIRMISH_TICKS,
        COMBAT_SKIRMISH_TRAIN_TICK, COMBAT_SKIRMISH_WATCHTOWER, combat_skirmish, commands_at,
    };
    let rules = load_rules();
    let (setup, stream) = combat_skirmish(&rules);
    assert_eq!((setup.seed, setup.start), (21, StartKind::Empty));
    let mangonel = ukind(&rules, "mangonel");
    let (scriptorium, town, tower) = (BuildingId(1), BuildingId(3), BuildingId(4));
    let mut sim = Sim::new(setup, rules.clone(), Box::new(NoAi));
    let mut events: Vec<(u32, SimEvent)> = Vec::new();
    let mut destroyed_with_search = None;
    for t in 0..COMBAT_SKIRMISH_TICKS {
        sim.step(commands_at(&stream, t));
        let tick_events = sim.drain_events();
        if tick_events.iter().any(
            |e| matches!(e, SimEvent::BuildingDestroyed { building, .. } if *building == tower),
        ) {
            destroyed_with_search = Some((t, sim.view().pathing().active.is_some()));
        }
        events.extend(tick_events.into_iter().map(|e| (t, e)));
    }
    let view = sim.view();
    let rejected: Vec<_> = events
        .iter()
        .filter(|(_, e)| matches!(e, SimEvent::CommandRejected { .. }))
        .collect();
    assert!(rejected.is_empty(), "{rejected:?}");
    // Tech.
    let researched: Vec<(u32, TechId)> = events
        .iter()
        .filter_map(|(t, e)| match e {
            SimEvent::ResearchCompleted { player, tech } if *player == P0 => Some((*t, *tech)),
            _ => None,
        })
        .collect();
    assert_eq!(
        researched.iter().map(|r| r.1).collect::<Vec<_>>(),
        [tech(&rules, "letters_1"), tech(&rules, "trade_1")]
    );
    assert!(events.contains(&(
        sim::scenarios::COMBAT_SKIRMISH_CANCEL_TICK + 2,
        SimEvent::TrainingCancelled {
            building: scriptorium,
            slot: 1
        }
    )));
    assert!(!view.researched(P0).contains(&tech(&rules, "statecraft_1")));
    assert!(
        events
            .iter()
            .any(|(_, e)| *e == SimEvent::AgeAdvanced { player: P0, age: 1 })
    );
    let spawned_99 = events
        .iter()
        .find(|(_, e)| *e == SimEvent::UnitSpawned { unit: UnitId(99) })
        .map(|(t, _)| *t)
        .expect("the Mangonel is trained");
    assert!(spawned_99 > COMBAT_SKIRMISH_TRAIN_TICK && spawned_99 < COMBAT_SKIRMISH_MANGONEL_TICK);
    assert!(
        !stream
            .iter()
            .flat_map(|(_, c)| c)
            .any(|c| matches!(c.cmd, Command::DebugSpawn { kind, .. } | Command::DebugSpawnJittered { kind, .. } if kind == mangonel)),
        "no Mangonel is debug-spawned"
    );
    // The line fight.
    let died = |p: PlayerId| {
        events
            .iter()
            .filter(|(_, e)| matches!(e, SimEvent::UnitDied { owner, .. } if *owner == p))
            .count()
    };
    let (died0, died1) = (died(P0), died(P1));
    assert!(
        died0 > 0 && died1 > 0,
        "deaths on both sides: {died0} {died1}"
    );
    let shots = |b: BuildingId| hits_by(&events, TargetId::Building(b)).len();
    assert!(shots(town) > 0, "the Town beside the field shoots");
    // The Watchtower.
    let on_tower = hits_on(&events, unit_t(99), TargetId::Building(tower));
    assert_eq!(
        on_tower.iter().map(|h| h.1).collect::<Vec<_>>(),
        [120; 5],
        "five hits of 30 x 4000 / 1000"
    );
    let tower_hits_99 = hits_on(&events, TargetId::Building(tower), unit_t(99));
    assert!(tower_hits_99.is_empty(), "the Mangonel outranges it");
    let (at, searching) = destroyed_with_search.expect("the Watchtower is destroyed");
    assert_eq!(at, on_tower[4].0);
    assert!(
        at > COMBAT_SKIRMISH_SATURATE_TICK && searching,
        "destroyed while a search is suspended"
    );
    assert!(events.iter().any(|(t, e)| *t == at
        && *e
            == SimEvent::BuildingDestroyed {
                building: tower,
                owner: P1,
                kind: bkind(&rules, "watchtower"),
            }));
    assert!(view.building(tower).is_none());
    assert!(
        view.map().passable(COMBAT_SKIRMISH_WATCHTOWER),
        "footprint free"
    );
    let m99 = view.unit(UnitId(99)).expect("the Mangonel lives");
    assert_eq!(
        (m99.owner, m99.kind, m99.intent),
        (P0, mangonel, Intent::Plain)
    );
    eprintln!(
        "combat_skirmish: research {researched:?}, Mangonel trained at {spawned_99}, deaths P0 {died0} P1 {died1}, Town shots {}, tower hits {on_tower:?}, destroyed at {at} with a search suspended",
        shots(town)
    );
}

// ---------------------------------------------------------------------------
// Seeded random streams over tech and combat (`m4a_*`).
// ---------------------------------------------------------------------------

/// The repo rules, loaded once (loading `data/` per case would dominate).
fn shared_rules() -> &'static Rules {
    static RULES: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    RULES.get_or_init(load_rules)
}

/// A seeded random command stream over both players in the all-grass west
/// rectangle of `plains_1v1`, at most 40 units spawned a side (a spawn
/// that would cross 40 is cut short; the dead still count): P0 has a
/// complete Scriptorium (`BuildingId(1)`) and stock, and opens with
/// Mangonels on P1's complete Watchtower (`BuildingId(2)`, which shoots)
/// and Skirmishers on P1's Bowmen; P1 has a Croft site (`BuildingId(3)`)
/// too. Then, at random: spawns of the armed kinds for either side (P1's
/// armies with `DebugSpawnJittered`, a tile of jitter drawn from the sim's
/// RNG), `Attack` on enemy units or on any building id (many rejected),
/// `AttackMove`, `Move` and `Stop`, and P0's `Research`, `AdvanceAge` and
/// `DebugResearch`. Idle and attack-moving units acquire targets on their
/// own. Unit ids are tracked in allocation order (P0's commands apply
/// first in a tick).
fn random_m4a_commands(seed: u64, ticks: u32) -> Vec<Vec<PlayerCommand>> {
    use rand_core::{Rng, SeedableRng};
    let rules = shared_rules();
    let armed: Vec<UnitKindId> = [
        "skirmisher",
        "shieldbearer",
        "bowman",
        "outrider",
        "mangonel",
    ]
    .iter()
    .map(|id| ukind(rules, id))
    .collect();
    let mut rng = rand_pcg::Pcg32::seed_from_u64(seed);
    let mut seq = [0u32; 2];
    let mut ids: [Vec<u32>; 2] = [Vec::new(), Vec::new()];
    let mut next_id = 1u32;
    let mut out = Vec::with_capacity(ticks as usize);
    let side = |p: usize| PlayerId(u8::try_from(p).unwrap());
    let push = |cmds: &mut Vec<PlayerCommand>, seq: &mut [u32; 2], p: usize, c: Command| {
        cmds.push(PlayerCommand::new(side(p), seq[p], c));
        seq[p] += 1;
    };
    let mut spawned = |ids: &mut [Vec<u32>; 2], p: usize, n: u32| {
        ids[p].extend(next_id..next_id + n);
        next_id += n;
    };
    let pick = |rng: &mut rand_pcg::Pcg32, v: &[u32], n: u32| -> Vec<u32> {
        if v.is_empty() {
            return Vec::new();
        }
        (0..n)
            .map(|_| v[usize::try_from(rng.next_u32()).unwrap() % v.len()])
            .collect()
    };
    let point = |rng: &mut rand_pcg::Pcg32| {
        FxVec2::from_ints(
            2 + i32::try_from(rng.next_u32() % 40).unwrap(),
            25 + i32::try_from(rng.next_u32() % 16).unwrap(),
        )
    };
    for t in 0..ticks {
        let mut cmds = Vec::new();
        if t == 0 {
            let s = bkind(rules, "scriptorium");
            push(
                &mut cmds,
                &mut seq,
                0,
                place(s, SCRIPTORIUM_A.0, SCRIPTORIUM_A.1, true),
            );
            for r in ["grain", "lumber", "ore", "lore"] {
                push(&mut cmds, &mut seq, 0, grant(res(rules, r), 2000));
            }
            push(
                &mut cmds,
                &mut seq,
                0,
                spawn(ukind(rules, "mangonel"), 15, 31, 5),
            );
            spawned(&mut ids, 0, 5);
            push(
                &mut cmds,
                &mut seq,
                0,
                spawn(ukind(rules, "skirmisher"), 14, 36, 6),
            );
            spawned(&mut ids, 0, 6);
            let w = bkind(rules, "watchtower");
            push(&mut cmds, &mut seq, 1, place_for(P1, w, 22, 31, true));
            push(
                &mut cmds,
                &mut seq,
                1,
                place_for(P1, bkind(rules, "croft"), 26, 26, false),
            );
            push(
                &mut cmds,
                &mut seq,
                1,
                Command::DebugSpawnJittered {
                    owner: P1,
                    kind: ukind(rules, "bowman"),
                    at: FxVec2::from_ints(20, 36),
                    count: 6,
                    jitter_x100: 100,
                },
            );
            spawned(&mut ids, 1, 6);
        } else if t == 1 {
            push(&mut cmds, &mut seq, 0, attack(&ids[0][..5], building_t(2)));
            for i in 0..6 {
                push(
                    &mut cmds,
                    &mut seq,
                    0,
                    attack(&[ids[0][5 + i]], unit_t(ids[1][i])),
                );
            }
            push(&mut cmds, &mut seq, 1, attack(&ids[1], unit_t(ids[0][5])));
        } else {
            let roll = rng.next_u32() % 100;
            let p = usize::try_from(rng.next_u32() % 2).unwrap();
            let n = 1 + rng.next_u32() % 4;
            if roll < 6 && ids[p].len() < 40 {
                // Never past 40 a side: the spawn that would cross it is
                // cut short (the draws stay the same).
                let n = n.min(40 - u32::try_from(ids[p].len()).unwrap());
                let k = armed[usize::try_from(rng.next_u32()).unwrap() % armed.len()];
                let at = point(&mut rng);
                let (owner, count) = (side(p), u16::try_from(n).unwrap());
                let cmd = if p == 1 {
                    Command::DebugSpawnJittered {
                        owner,
                        kind: k,
                        at,
                        count,
                        jitter_x100: 100,
                    }
                } else {
                    Command::DebugSpawn {
                        owner,
                        kind: k,
                        at,
                        count,
                    }
                };
                push(&mut cmds, &mut seq, p, cmd);
                spawned(&mut ids, p, n);
            } else if roll < 30 {
                let units = pick(&mut rng, &ids[p], n);
                let target = if rng.next_u32() % 10 < 7 {
                    pick(&mut rng, &ids[1 - p], 1)
                        .first()
                        .map_or(unit_t(0), |u| unit_t(*u))
                } else {
                    building_t(1 + rng.next_u32() % 5)
                };
                push(&mut cmds, &mut seq, p, attack(&units, target));
            } else if roll < 36 {
                let units = pick(&mut rng, &ids[p], n).into_iter().map(UnitId).collect();
                let target = point(&mut rng);
                push(
                    &mut cmds,
                    &mut seq,
                    p,
                    Command::AttackMove { units, target },
                );
            } else if roll < 40 {
                let units = pick(&mut rng, &ids[p], n).into_iter().map(UnitId).collect();
                let target = point(&mut rng);
                push(
                    &mut cmds,
                    &mut seq,
                    p,
                    Command::Move {
                        units,
                        target,
                        queue: false,
                    },
                );
            } else if roll < 44 {
                let units = pick(&mut rng, &ids[p], 1);
                push(&mut cmds, &mut seq, p, stop(&units));
            } else if roll < 47 {
                let tech = TechId(u16::try_from(rng.next_u32() % 12).unwrap());
                push(&mut cmds, &mut seq, 0, research(1, tech));
            } else if roll < 48 {
                push(&mut cmds, &mut seq, 0, advance());
            } else if roll < 50 {
                let tech = TechId(u16::try_from(rng.next_u32() % 12).unwrap());
                push(&mut cmds, &mut seq, 0, debug_research(tech));
            }
        }
        cmds.sort_by_key(PlayerCommand::sort_key);
        out.push(cmds);
    }
    out
}

/// `random_m4a_commands` keeps its documented bound: at most 40 units
/// spawned a side, counted over the stream (dead units are not taken
/// off). Without the cut, 14 of these 64 seeds spawn 41 to 43.
#[test]
fn m4a_random_stream_spawns_at_most_40_a_side() {
    for seed in 0..64 {
        let mut spawned = [0u32; 2];
        for pc in random_m4a_commands(seed, 300).iter().flatten() {
            if let Command::DebugSpawn { owner, count, .. }
            | Command::DebugSpawnJittered { owner, count, .. } = &pc.cmd
            {
                spawned[usize::from(owner.0)] += u32::from(*count);
            }
        }
        assert!(spawned.iter().all(|&s| s <= 40), "seed {seed}: {spawned:?}");
    }
}

/// Run `stream` on the empty-start scenario with `seed`: the hash after
/// every tick, and the events.
fn run_m4a(seed: u64, stream: &[Vec<PlayerCommand>], cache: bool) -> (Vec<u64>, Vec<SimEvent>) {
    let mut sim = Sim::new(
        MatchSetup::scenario(shared_rules(), seed),
        shared_rules().clone(),
        Box::new(NoAi),
    );
    sim.set_path_cache_enabled(cache);
    let mut hashes = Vec::with_capacity(stream.len());
    let mut events = Vec::new();
    for cmds in stream {
        sim.step(cmds);
        hashes.push(sim.hash());
        events.extend(sim.drain_events());
    }
    (hashes, events)
}

/// The random stream really researches, kills and destroys: over 8 seeds
/// of 300 ticks, techs complete, units die on both sides, buildings are
/// destroyed, and Attacks are both accepted and rejected. Units also fight
/// targets no `Attack` named for them (acquisition by guards and
/// attack-movers), and the Watchtower shoots.
#[test]
fn m4a_random_stream_exercises_combat_and_tech() {
    let (mut researched, mut died, mut destroyed_n, mut hit, mut rejected_attacks) =
        (0, [0usize; 2], 0, 0, 0);
    let (mut acquired_hits, mut shots) = (0, 0);
    for seed in 0..8 {
        let stream = random_m4a_commands(seed, 300);
        let attacks: std::collections::BTreeSet<(PlayerId, u32)> = stream
            .iter()
            .flatten()
            .filter(|pc| matches!(pc.cmd, Command::Attack { .. }))
            .map(PlayerCommand::sort_key)
            .collect();
        // Every (unit, target) pair an Attack named.
        let ordered: std::collections::BTreeSet<(TargetId, TargetId)> = stream
            .iter()
            .flatten()
            .filter_map(|pc| match &pc.cmd {
                Command::Attack { units, target } => Some(
                    units
                        .iter()
                        .map(|u| (TargetId::Unit(*u), *target))
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .flatten()
            .collect();
        let (_, events) = run_m4a(seed, &stream, true);
        let mut owner = std::collections::BTreeMap::new();
        for pc in stream.iter().flatten() {
            if let Command::DebugSpawn {
                owner: o, count, ..
            }
            | Command::DebugSpawnJittered {
                owner: o, count, ..
            } = pc.cmd
            {
                for _ in 0..count {
                    let id = u32::try_from(owner.len()).unwrap() + 1;
                    owner.insert(UnitId(id), o);
                }
            }
        }
        for e in &events {
            if let SimEvent::Hit {
                attacker, target, ..
            } = e
            {
                match attacker {
                    TargetId::Building(_) => shots += 1,
                    TargetId::Unit(_) => {
                        acquired_hits += usize::from(!ordered.contains(&(*attacker, *target)));
                    }
                }
            }
            match e {
                SimEvent::ResearchCompleted { .. } => researched += 1,
                SimEvent::UnitDied { unit, owner: o, .. } => {
                    assert_eq!(*o, owner[unit], "UnitDied names the owner");
                    died[usize::from(o.0)] += 1;
                }
                SimEvent::BuildingDestroyed { .. } => destroyed_n += 1,
                SimEvent::Hit { .. } => hit += 1,
                SimEvent::CommandRejected { player, seq, .. }
                    if attacks.contains(&(*player, *seq)) =>
                {
                    rejected_attacks += 1;
                }
                _ => {}
            }
        }
    }
    eprintln!(
        "m4a stream, 8 seeds: researched={researched} died={died:?} destroyed={destroyed_n} hits={hit} (acquired {acquired_hits}, shots {shots}) rejected attacks={rejected_attacks}"
    );
    assert!(researched > 0 && destroyed_n > 0 && hit > 0);
    assert!(acquired_hits > 0 && shots > 0, "{acquired_hits} {shots}");
    assert!(died[0] > 0 && died[1] > 0, "{died:?}");
    assert!(rejected_attacks > 0);
}

/// Proptest case count: `PROPTEST_CASES` when set, else 256 in CI and 8
/// locally (the `cargo test -p sim` budget).
fn proptest_cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if std::env::var_os("CI").is_some() {
            256
        } else {
            8
        })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: proptest_cases(),
        .. ProptestConfig::default()
    })]

    #[test]
    fn m4a_proptest_streams_are_deterministic(seed in any::<u64>(), ticks in 50u32..300) {
        let stream = random_m4a_commands(seed, ticks);
        let (a, _) = run_m4a(seed, &stream, true);
        let (b, _) = run_m4a(seed, &stream, true);
        prop_assert_eq!(a, b);
    }

    #[test]
    fn m4a_path_cache_is_transparent(seed in any::<u64>(), ticks in 50u32..300) {
        let stream = random_m4a_commands(seed, ticks);
        let (on, _) = run_m4a(seed, &stream, true);
        let (off, _) = run_m4a(seed, &stream, false);
        prop_assert_eq!(on, off, "hashes must agree on every tick with the cache on and off");
    }
}
