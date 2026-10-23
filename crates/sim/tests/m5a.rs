//! M5a tests: attrition, supply, annexation and the match outcome
//! (`docs/design/attrition.md`, `docs/design/towns.md`).
//!
//! The rules-data slice (`rules_version` 18) adds the Watchtower's Harrying
//! I and II, side techs that count toward no age. Their tests are named
//! `tech_harrying_*`, so `cargo test -p sim tech` selects them with the M4a
//! tech tests (the filter matches test paths, not integration-test file
//! names).
//!
//! The attrition slice (`SIM_VERSION` 15, `rules_version` 19) adds the
//! `attrition_*` tests, which `cargo test -p sim attrition` selects
//! (acceptance line 1), and the seeded random stream `m5a_attrition_*`.
//! They run with `cmd_delay = 0` ([`M::delay0`]): a command issued in a
//! step applies in that step, so the tick an assertion names is the
//! literal `sim.tick()` after the step, and the field is set up in step 0.
//!
//! The annexation and outcome slice (`SIM_VERSION` 16, `rules_version` 20)
//! adds the `annexation_*` tests, which `cargo test -p sim annexation`
//! selects (acceptance line 2): the timer, its resets and the flip around
//! P1's Town at 0 HP beside P0's Skirmishers ([`annex_setup`], delay 0,
//! entry in step [`T0`]), the `capture_seat` scenario, the 54000-tick
//! stalemate (not ignored) and the outcome rules. They read
//! `annexation_ds`, the defender radius, the flipped hit points and the
//! match cap from `Rules`, as the attrition tests read the interval. The
//! goldens `golden_capture_seat` and `golden_stalemate_45min` (ignored:
//! 54000 ticks, verified in CI) and the `m5a_random_*` streams over
//! attrition and annexation come with it.
//!
//! The tests run on `MatchSetup::scenario`, an empty start: with no Town
//! there is no income, so stockpiles are exact through `DebugGrant`, and
//! a Scriptorium's Lore is the only income where one is placed. Every
//! rejection is compared with a control sim that receives a rejected no-op
//! with the same `(player, seq)`, on every sub-hash (`rng` included) and
//! the whole hash, as in `m4a.rs`.

use proptest::prelude::*;
use rand_core::{Rng, SeedableRng};
use rules::Rules;
use sim::{
    AnnexView, BuildingId, BuildingKindId, Command, Exposure, FxVec2, Hold, MatchResult,
    MatchSetup, Outcome, OutcomeReason, PlayerCommand, PlayerId, RejectReason, Sim, SimEvent,
    SimView, TargetId, TechId, Tile, UnitId, UnitKindId,
};
use std::collections::BTreeMap;
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

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn tech(rules: &Rules, id: &str) -> TechId {
    TechId(rules.tech_by_id(id).expect("tech in techs.ron").0)
}

fn res(rules: &Rules, id: &str) -> usize {
    rules.resource_index(id).expect("resource in resources.ron")
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

    /// [`M::new`] with `cmd_delay = 0`: a command issued in a step applies
    /// in that step (P0's before P1's), so `sim.tick()` after the step is
    /// the tick an acceptance line names.
    fn delay0(rules: Rules, seed: u64) -> M {
        let mut setup = MatchSetup::scenario(&rules, seed);
        setup.cmd_delay = 0;
        M {
            sim: Sim::new(setup, rules, Box::new(NoAi)),
            seq: [0; 2],
        }
    }

    /// Stamp `cmds` (issuing player, command) and step once; with delay 0
    /// they apply in this step. The step's events.
    fn step_with(&mut self, cmds: Vec<(u8, Command)>) -> Vec<SimEvent> {
        let pcs: Vec<PlayerCommand> = cmds.into_iter().map(|(p, c)| self.stamp(p, c)).collect();
        self.sim.step(&pcs);
        self.sim.drain_events()
    }

    /// A second sim restored from this one's snapshot, with the same
    /// sequence numbers: it continues exactly as this one would.
    fn clone_at_tick(&self) -> M {
        let mut m = M {
            sim: Sim::new(
                self.sim.setup().clone(),
                self.sim.rules().clone(),
                Box::new(NoAi),
            ),
            seq: self.seq,
        };
        m.sim.restore(&self.sim.snapshot()).expect("restores");
        m
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
}

fn grant(resource: usize, amount: u32) -> Command {
    Command::DebugGrant {
        owner: P0,
        resource: u8::try_from(resource).unwrap(),
        amount,
    }
}

/// Grants of exactly `cost`, a vector in `resources.ron` order.
fn grants(cost: &[i64]) -> Vec<Command> {
    cost.iter()
        .enumerate()
        .filter(|(_, c)| **c > 0)
        .map(|(r, c)| grant(r, u32::try_from(*c).unwrap()))
        .collect()
}

fn place(kind: BuildingKindId, x: u16, y: u16, complete: bool) -> Command {
    Command::DebugPlaceBuilding {
        owner: P0,
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

/// Harrying I and II count toward no age (M5a rules data): with Letters I
/// and both Harrying techs the Masonry Age still needs a second
/// Scriptorium tech, and with four Scriptorium techs and both Harrying
/// techs the Charter Age still needs a fifth. Each rejection changes
/// nothing, `SimView::age_tech_count` and `check_advance_age` agree with
/// apply, and the missing Scriptorium tech opens the age.
#[test]
fn tech_harrying_does_not_count_toward_ages() {
    let rules = load_rules();
    let t = |id: &str| tech(&rules, id);
    let harrying = ["harrying_1", "harrying_2"];
    // An empty start that has researched `ids` (`DebugResearch` skips the
    // age check) and holds exactly the next age's cost after `masonry`
    // AdvanceAges.
    let after = |masonry: bool, ids: &[&str]| {
        let rules = rules.clone();
        let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        move || {
            let mut m = M::new(rules.clone(), 3);
            if masonry {
                let mut cmds = vec![
                    debug_research(tech(&rules, "letters_1")),
                    debug_research(tech(&rules, "trade_1")),
                ];
                cmds.extend(grants(&rules.amounts(&rules.ages[1].cost)));
                cmds.push(advance());
                m.ok(cmds);
                assert_eq!(m.view().age(P0), 1);
            }
            let next = usize::from(masonry) + 1;
            let mut cmds: Vec<Command> = ids
                .iter()
                .map(|id| debug_research(tech(&rules, id)))
                .collect();
            cmds.extend(grants(&rules.amounts(&rules.ages[next].cost)));
            m.ok(cmds);
            m
        }
    };
    let cases: [(&str, bool, Vec<&str>, u32, &str); 2] = [
        (
            "masonry",
            false,
            [&["letters_1"][..], &harrying].concat(),
            1,
            "trade_1",
        ),
        (
            "charter",
            true,
            [&["arms_1", "statecraft_1"][..], &harrying].concat(),
            4,
            "letters_2",
        ),
    ];
    for (age, masonry, ids, counting, missing) in cases {
        let setup = after(masonry, &ids);
        let m = setup();
        let researched = m.view().researched(P0).len();
        let required = rules.ages[usize::from(masonry) + 1].techs_required;
        assert_eq!(m.view().age_tech_count(P0), counting, "{age}");
        assert_eq!(researched, usize::try_from(counting).unwrap() + 2, "{age}");
        assert!(
            u32::try_from(researched).unwrap() >= required && counting < required,
            "{age}: the Harrying techs alone would meet the gate"
        );
        assert_eq!(
            m.view().check_advance_age(P0),
            Err(RejectReason::NotEnoughTechs),
            "{age}"
        );
        println!(
            "{age}: {researched} techs researched, {counting} count toward an age, {required} required -> NotEnoughTechs"
        );
        rejected_without_change(
            &format!("{age} with both Harrying techs"),
            &setup,
            0,
            advance(),
            RejectReason::NotEnoughTechs,
        );
        let mut m = setup();
        m.ok(vec![debug_research(t(missing))]);
        assert_eq!(m.view().age_tech_count(P0), counting + 1);
        assert!(m.view().check_advance_age(P0).is_ok(), "{age}");
        let events = m.ok(vec![advance()]);
        let reached = u8::from(masonry) + 1;
        assert!(events.contains(&SimEvent::AgeAdvanced {
            player: P0,
            age: reached
        }));
        assert_eq!(m.stock(P0), vec![0; 4], "{age}: paid exactly the cost");
    }
}

/// Harrying I is researched at a Watchtower from the Masonry Age and
/// Harrying II from the Charter Age (M5a rules data): rejected with
/// `AgeRequired` before, with `CannotResearchHere` at a Scriptorium and
/// `PrerequisiteMissing` out of order, each changing nothing; accepted at
/// the Watchtower, it pays the quoted cost and completes in its quoted
/// time (800 and 1200 ticks with no Letters research).
#[test]
fn tech_harrying_is_researched_at_a_watchtower_from_masonry() {
    let rules = load_rules();
    let t = |id: &str| tech(&rules, id);
    let (h1, h2) = (t("harrying_1"), t("harrying_2"));
    // `BuildingId(1)` a complete Scriptorium, (2) a complete Watchtower,
    // plenty of every resource; then each AdvanceAge after the Scriptorium
    // techs it needs (no Letters, so every quote is the base cost).
    let setup = |age: u8, extra: &[&str]| {
        let rules = rules.clone();
        let extra: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
        move || {
            let mut m = M::new(rules.clone(), 4);
            let mut cmds = vec![
                place(bkind(&rules, "scriptorium"), 4, 26, true),
                place(bkind(&rules, "watchtower"), 2, 26, true),
                grant(res(&rules, "grain"), 2000),
                grant(res(&rules, "lumber"), 2000),
                grant(res(&rules, "ore"), 2000),
                grant(res(&rules, "lore"), 999),
            ];
            let age_techs = ["arms_1", "trade_1", "statecraft_1", "arms_2", "trade_2"];
            let needed = rules.ages[usize::from(age)].techs_required;
            cmds.extend(
                age_techs[..usize::try_from(needed).unwrap()]
                    .iter()
                    .map(|id| debug_research(tech(&rules, id))),
            );
            cmds.extend((0..age).map(|_| advance()));
            cmds.extend(extra.iter().map(|id| debug_research(tech(&rules, id))));
            m.ok(cmds);
            assert_eq!(m.view().age(P0), age);
            m
        }
    };
    let cases = [
        (
            "Harrying I in the Hearth Age",
            0,
            vec![],
            research(2, h1),
            RejectReason::AgeRequired,
        ),
        (
            "Harrying I at a Scriptorium",
            1,
            vec![],
            research(1, h1),
            RejectReason::CannotResearchHere,
        ),
        (
            "Arms II at the Watchtower",
            1,
            vec![],
            research(2, t("arms_2")),
            RejectReason::CannotResearchHere,
        ),
        (
            "Harrying II before Harrying I",
            2,
            vec![],
            research(2, h2),
            RejectReason::PrerequisiteMissing,
        ),
        (
            "Harrying II in the Masonry Age",
            1,
            vec!["harrying_1"],
            research(2, h2),
            RejectReason::AgeRequired,
        ),
    ];
    for (name, age, extra, cmd, want) in cases {
        rejected_without_change(name, &setup(age, &extra), 0, cmd, want);
    }
    // Accepted at the Watchtower: Harrying I in Masonry, then Harrying II
    // in Charter, each deducting its quote and completing in its time.
    let mut m = setup(1, &[])();
    for (level, id, next_age) in [(1, h1, 2u8), (2, h2, 0)] {
        let def = rules.tech(id.0).unwrap();
        let (cost, ticks) = m.view().research_quote(P0, id);
        assert_eq!(cost, rules.amounts(&def.cost), "{}: the base cost", def.id);
        assert_eq!(ticks, rules.ticks_from_ds(def.research_time_ds));
        assert_eq!(
            m.view().check_research(P0, BuildingId(2), id),
            Ok((cost.clone(), ticks))
        );
        let counting = m.view().age_tech_count(P0);
        let before = m.stock(P0);
        m.ok(vec![research(2, id)]);
        let paid: Vec<i64> = before.iter().zip(m.stock(P0)).map(|(b, a)| b - a).collect();
        assert_eq!(paid, cost, "{}", def.id);
        // Applied (and given its first tick) in the last step of `ok`.
        let started = m.sim.tick() - 1;
        let events = m.step(ticks - 2);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, SimEvent::ResearchCompleted { .. })),
            "{}",
            def.id
        );
        let events = m.step(1);
        assert_eq!(m.sim.tick() - 1, started + ticks - 1, "the ticks-th step");
        assert!(events.contains(&SimEvent::ResearchCompleted {
            player: P0,
            tech: id
        }));
        assert!(m.view().researched(P0).contains(&id));
        assert_eq!(m.view().age_tech_count(P0), counting, "a side tech");
        println!(
            "{} (level {level}) at the Watchtower in age {}: paid {paid:?}, completed after {ticks} ticks",
            def.name,
            m.view().age(P0)
        );
        if next_age == 2 {
            // Three more Scriptorium techs, five in all, none of them
            // Letters (whose rows would lower the next quote); the start's
            // grants pay the Charter Age.
            m.ok(vec![
                debug_research(t("statecraft_1")),
                debug_research(t("arms_2")),
                debug_research(t("trade_2")),
                advance(),
            ]);
            assert_eq!(m.view().age(P0), next_age);
        }
    }
}

// ---------------------------------------------------------------------------
// Attrition (slice 2): `cargo test -p sim attrition`.
// ---------------------------------------------------------------------------

/// P1's Town in the attrition field: complete, north-west tile (20,30), so
/// its centre tile is (21,31) and its tier-1 border (radius 20) owns every
/// tile with `dx² + dy² < 400` from there.
const FIELD_TOWN: (u16, u16) = (20, 30);

/// The intruder's tile: inside P1's border (dx 14), 12.15 tiles edge to
/// edge from the Town's footprint, beyond every unit's reach (6) and the
/// Town's range (7).
const INTRUDER: (i32, i32) = (35, 31);

fn spawn_for(owner: PlayerId, kind: UnitKindId, (x, y): (i32, i32)) -> Command {
    Command::DebugSpawn {
        owner,
        kind,
        at: FxVec2::from_ints(x, y),
        count: 1,
    }
}

fn place_for(owner: PlayerId, kind: BuildingKindId, (x, y): (u16, u16)) -> Command {
    Command::DebugPlaceBuilding {
        owner,
        kind,
        tile: Tile::new(x, y),
        complete: true,
    }
}

fn move_to(unit: UnitId, (x, y): (i32, i32)) -> Command {
    Command::Move {
        units: vec![unit],
        target: FxVec2::from_ints(x, y),
        queue: false,
    }
}

fn damage(target: TargetId, amount: u32) -> Command {
    Command::DebugDamage { target, amount }
}

/// The first `n` Harrying techs, as P1's `DebugResearch` commands.
fn harrying_for(rules: &Rules, owner: PlayerId, n: usize) -> Vec<(u8, Command)> {
    ["harrying_1", "harrying_2"][..n]
        .iter()
        .map(|id| {
            (
                owner.0,
                Command::DebugResearch {
                    owner,
                    tech: tech(rules, id),
                },
            )
        })
        .collect()
}

/// The attrition field, all in step 0 with delay 0: P0's Skirmisher at
/// [`INTRUDER`] (`UnitId(1)`), then `extra` in order (P0's commands apply
/// before P1's), then P1's complete Town at [`FIELD_TOWN`] and its first
/// `harrying` Harrying techs. Returns the sim after step 0 (`tick() == 1`)
/// and the step's events.
fn attrition_field(
    rules: &Rules,
    harrying: usize,
    extra: Vec<(u8, Command)>,
) -> (M, Vec<SimEvent>) {
    let mut m = M::delay0(rules.clone(), 5);
    let mut cmds = vec![(0, spawn_for(P0, ukind(rules, "skirmisher"), INTRUDER))];
    cmds.extend(extra);
    cmds.push((1, place_for(P1, bkind(rules, "town"), FIELD_TOWN)));
    cmds.extend(harrying_for(rules, P1, harrying));
    let events = m.step_with(cmds);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(m.sim.tick(), 1);
    (m, events)
}

fn hp(m: &M, u: UnitId) -> i32 {
    m.view().unit_hp(u).expect("the unit is alive").0
}

fn exposed(m: &M, u: UnitId) -> u32 {
    m.view().unit(u).expect("the unit is alive").exposed_ticks
}

fn unit_tile(m: &M, u: UnitId) -> Tile {
    let view = m.view();
    view.map()
        .tile_of(view.unit(u).expect("the unit is alive").pos)
}

/// `(unit, damage)` of every `Attrition` event in `events`.
fn pulses(events: &[SimEvent]) -> Vec<(UnitId, i32)> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::Attrition { unit, damage } => Some((*unit, *damage)),
            _ => None,
        })
        .collect()
}

fn hits(events: &[SimEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, SimEvent::Hit { .. }))
        .count()
}

/// Ticks between pulses at Harrying I, read from the data: 64 with the
/// shipped 32 ds (the rules crate's `loads_repo_data` pins that). The
/// attrition tests derive their ticks, pulse counts and hit points from
/// it, so a data-only edit of `attrition_interval_ds`, the worked example
/// in `data/rules/README.md`, moves their numbers instead of failing them
/// (CONTRIBUTING.md: read a number the data controls from `Rules`).
fn interval1(rules: &Rules) -> u32 {
    rules.attrition_interval_ticks()
}

/// Ticks between pulses at Harrying II: half of [`interval1`], at least 1
/// (32 with the shipped data).
fn interval2(rules: &Rules) -> u32 {
    (interval1(rules) / 2).max(1)
}

/// How many pulses a unit exposed from step 0 takes by tick `t` at
/// `interval` (its counter is `t` then): `t / interval`, as an `i32`.
fn pulses_by(t: u32, interval: u32) -> i32 {
    i32::try_from(t / interval).unwrap()
}

/// Acceptance 1, first clause: an enemy unit in researched borders loses
/// exactly 1 HP at ticks 64, 128 and 192 (the multiples of [`interval1`]
/// with the shipped data). P0's Skirmisher stands inside P1's border from
/// step 0, P1 has Harrying I and both players are in the Hearth Age (gap
/// 0, 1 HP). Its hit points are 80 through tick 63, 79 at 64 to 127, 78 at
/// 128 to 191 and 77 at 192 to 200, with exactly three
/// `Attrition { damage: 1 }` events, in the steps that end at those ticks,
/// and no `Hit`.
#[test]
fn attrition_enemy_unit_in_researched_borders_loses_exactly_1_hp_at_ticks_64_128_192() {
    let rules = load_rules();
    let i = interval1(&rules);
    let end = 3 * i + 8;
    let u = UnitId(1);
    let (mut m, events) = attrition_field(&rules, 1, vec![]);
    let tile = unit_tile(&m, u);
    assert_eq!(m.view().territory().owner_at(tile), Some(P1), "{tile:?}");
    assert_eq!(m.view().harrying_level(P1), 1);
    assert_eq!(m.view().harrying_level(P0), 0);
    assert_eq!(
        m.view().attrition(u),
        Some(Exposure {
            owner: P1,
            interval_ticks: i,
            hp: 1
        })
    );
    assert!(pulses(&events).is_empty() && hits(&events) == 0);
    let mut hp_at = BTreeMap::from([(1, hp(&m, u))]);
    let mut landed = Vec::new();
    let mut hit_count = 0;
    while m.sim.tick() < end {
        let events = m.step(1);
        hit_count += hits(&events);
        for (unit, dmg) in pulses(&events) {
            landed.push((m.sim.tick(), unit, dmg));
        }
        hp_at.insert(m.sim.tick(), hp(&m, u));
    }
    for (&t, &h) in &hp_at {
        let want = 80 - i32::from(t >= i) - i32::from(t >= 2 * i) - i32::from(t >= 3 * i);
        assert_eq!(h, want, "hit points after tick {t}");
    }
    assert_eq!(landed, [(i, u, 1), (2 * i, u, 1), (3 * i, u, 1)]);
    assert_eq!(hit_count, 0, "nothing but attrition touched it");
    for t in [i - 1, i, 2 * i - 1, 2 * i, 3 * i - 1, 3 * i, end] {
        if let Some(h) = hp_at.get(&t) {
            println!("tick {t}: hp {h}");
        }
    }
    println!("Attrition events (tick, unit, damage): {landed:?}; Hit events: {hit_count}");
}

/// Acceptance 1, second clause: with a friendly Supply Wain within 14
/// tiles the unit loses 0 HP over 1200 ticks, and its counter never
/// leaves 0. The Wain stands 14 tiles south of the intruder's tile
/// (`dy² = 196`, inclusive); the control Wain 15 tiles south does not
/// cover it, and the intruder ends at 62 HP (18 pulses, `1200 /`
/// [`interval1`]). The Wain itself is immune.
#[test]
fn attrition_friendly_supply_wain_within_14_tiles_means_0_hp_lost_over_1200_ticks() {
    let rules = load_rules();
    let (u, w) = (UnitId(1), UnitId(2));
    let wain = ukind(&rules, "supply_wain");
    let n = pulses_by(1200, interval1(&rules));
    let cases = [
        ((35, 45), true, 80, 0),
        ((35, 46), false, 80 - n, usize::try_from(n).unwrap()),
    ];
    for (at, covered, want_hp, want_pulses) in cases {
        let (mut m, _) = attrition_field(&rules, 1, vec![(0, spawn_for(P0, wain, at))]);
        let at_tile = Tile::new(u16::try_from(at.0).unwrap(), u16::try_from(at.1).unwrap());
        assert_eq!(m.view().supply_radius_tiles(), 14);
        assert_eq!(m.view().supply_wains(P0), [at_tile]);
        assert_eq!(m.view().supplied(P0, unit_tile(&m, u)), covered, "{at:?}");
        assert_eq!(m.view().attrition(u).is_none(), covered);
        let mut landed = 0;
        while m.sim.tick() < 1200 {
            let events = m.step(1);
            landed += pulses(&events).len();
            if covered {
                assert_eq!((hp(&m, u), exposed(&m, u)), (80, 0), "{}", m.sim.tick());
            }
            assert_eq!(exposed(&m, w), 0, "the Wain is immune");
        }
        assert_eq!((hp(&m, u), landed), (want_hp, want_pulses), "{at:?}");
        assert_eq!(hp(&m, w), 150);
        println!(
            "Wain at {at:?} ({}): hp {} after {} ticks, {landed} pulses",
            if covered { "covers" } else { "does not cover" },
            hp(&m, u),
            m.sim.tick()
        );
    }
}

/// Acceptance 1, third clause: before Harrying I an enemy unit loses 0 HP:
/// with no Harrying tech P1's level is 0 and the intruder stays at 80 HP
/// with a counter of 0 for 1200 ticks, with no event. Harrying I
/// researched during tick 600 starts the exposure there: the first pulse
/// lands at tick 664 (600 + [`interval1`]).
#[test]
fn attrition_before_harrying_one_an_enemy_unit_loses_0_hp() {
    let rules = load_rules();
    let due = 600 + interval1(&rules);
    let u = UnitId(1);
    let (mut m, _) = attrition_field(&rules, 0, vec![]);
    assert_eq!(m.view().harrying_level(P1), 0);
    assert_eq!(m.view().attrition_pulse(P1, P0), None);
    assert_eq!(m.view().attrition(u), None);
    let mut quiet = m.clone_at_tick();
    while m.sim.tick() < 1200 {
        let events = m.step(1);
        assert!(pulses(&events).is_empty() && hits(&events) == 0);
        assert_eq!((hp(&m, u), exposed(&m, u)), (80, 0), "{}", m.sim.tick());
    }
    println!("no Harrying: hp {} after {} ticks", hp(&m, u), m.sim.tick());
    // The same field, researching Harrying I during tick 600.
    quiet.step(600 - quiet.sim.tick());
    assert_eq!(quiet.sim.tick(), 600);
    let events = quiet.step_with(harrying_for(&rules, P1, 1));
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(quiet.view().harrying_level(P1), 1);
    assert_eq!(exposed(&quiet, u), 1, "exposed from step 600");
    let mut first = None;
    while quiet.sim.tick() < due + 36 {
        let events = quiet.step(1);
        if first.is_none() && !pulses(&events).is_empty() {
            first = Some(quiet.sim.tick());
        }
        let t = quiet.sim.tick();
        assert_eq!(hp(&quiet, u), 80 - i32::from(t >= due), "{t}");
    }
    assert_eq!(first, Some(due));
    println!("Harrying I during tick 600: first pulse at tick {first:?}");
}

/// Acceptance 1, fourth clause: Harrying II doubles the rate. With both
/// Harrying techs the interval is 32 ticks instead of 64 ([`interval2`]
/// and [`interval1`]), and at every multiple of 64 up to 1152 the loss is
/// exactly twice Harrying I's; after 1200 ticks the intruder has 43 HP
/// against 62.
#[test]
fn attrition_harrying_two_doubles_the_rate() {
    let rules = load_rules();
    let (i1, i2) = (interval1(&rules), interval2(&rules));
    let u = UnitId(1);
    let (mut one, _) = attrition_field(&rules, 1, vec![]);
    let (mut two, _) = attrition_field(&rules, 2, vec![]);
    assert_eq!(
        (one.view().harrying_level(P1), two.view().harrying_level(P1)),
        (1, 2)
    );
    let interval = |m: &M| m.view().attrition(u).map(|e| e.interval_ticks);
    assert_eq!((interval(&one), interval(&two)), (Some(i1), Some(i2)));
    assert_eq!(two.view().attrition_pulse(P1, P0), Some((i2, 1)));
    let mut loss = [vec![0; 1201], vec![0; 1201]];
    while one.sim.tick() < 1200 {
        one.step(1);
        two.step(1);
        let t = usize::try_from(one.sim.tick()).unwrap();
        loss[0][t] = 80 - hp(&one, u);
        loss[1][t] = 80 - hp(&two, u);
    }
    let multiples: Vec<usize> = (1..=1200 / i1)
        .map(|k| usize::try_from(k * i1).unwrap())
        .collect();
    assert!(!multiples.is_empty());
    for (k, &t) in (1..).zip(&multiples) {
        assert_eq!(loss[0][t], k, "Harrying I at {t}");
        assert_eq!(loss[1][t], 2 * loss[0][t], "Harrying II at {t}");
    }
    assert_eq!(
        (hp(&one, u), hp(&two, u)),
        (80 - pulses_by(1200, i1), 80 - pulses_by(1200, i2))
    );
    println!(
        "after 1200 ticks: Harrying I hp {} (interval {i1}), Harrying II hp {} (interval {i2}); loss at {i1}k: {:?} vs {:?}",
        hp(&one, u),
        hp(&two, u),
        multiples.iter().map(|&t| loss[0][t]).collect::<Vec<_>>(),
        multiples.iter().map(|&t| loss[1][t]).collect::<Vec<_>>()
    );
}

/// The supply disc is 14 tiles, tile to tile and inclusive, and per
/// player: P0 Wains 14 tiles south or at (10, 9) cover the intruder
/// (`dx² + dy² = 196`, `181`), at 15 tiles south or (10, 10) (`225`,
/// `200`) they do not, and P1's Wain 9 tiles south never covers P0's unit.
/// After 200 ticks (three [`interval1`] pulses and 8 ticks) a covered
/// intruder has 80 HP and an uncovered one 77.
#[test]
fn attrition_supply_radius_is_14_tiles_inclusive_and_per_player() {
    let rules = load_rules();
    let end = 3 * interval1(&rules) + 8;
    let u = UnitId(1);
    let wain = ukind(&rules, "supply_wain");
    let cases = [
        (P0, (35, 45), true),
        (P0, (35, 46), false),
        (P0, (45, 40), true),
        (P0, (45, 41), false),
        (P1, (35, 40), false),
    ];
    for (owner, at, covers) in cases {
        let (mut m, _) = attrition_field(&rules, 1, vec![(owner.0, spawn_for(owner, wain, at))]);
        let t = unit_tile(&m, u);
        let (dx, dy) = (at.0 - i32::from(t.x), at.1 - i32::from(t.y));
        assert_eq!(dx * dx + dy * dy <= 196 && owner == P0, covers, "{at:?}");
        assert_eq!(m.view().supplied(P0, t), covers, "{owner:?} {at:?}");
        assert_eq!(m.view().supplied(P1, t), owner == P1);
        m.step(end - m.sim.tick());
        assert_eq!(hp(&m, u), if covers { 80 } else { 77 }, "{owner:?} {at:?}");
    }
}

/// Walking in and out leaves no hidden damage: the counter drops to 0 the
/// step the unit stands on its owner's land, on neutral land or under its
/// own Supply Wain, and restarts from 0 when it is exposed again. P1's
/// border and P0's touch on row 31 (P0's Town centre is 39 tiles east of
/// P1's), so the Skirmisher walks straight from P1's land onto P0's; it
/// then walks back, out to neutral land north of the border, and back
/// again, and a Wain walks in to cover it and away. Every step checks the
/// counter against the exposure the view reports after the step, and every
/// pulse against the counter, so the hit points lost are exactly the
/// pulses landed.
#[test]
fn attrition_counter_resets_when_supplied_friendly_or_neutral() {
    let rules = load_rules();
    let (u, w) = (UnitId(1), UnitId(2));
    let (mut m, _) = attrition_field(
        &rules,
        1,
        vec![
            (0, spawn_for(P0, ukind(&rules, "supply_wain"), (35, 62))),
            (0, place_for(P0, bkind(&rules, "town"), (59, 30))),
        ],
    );
    let script: BTreeMap<u32, Command> = BTreeMap::from([
        (100, move_to(u, (50, 31))),
        (300, move_to(u, INTRUDER)),
        (450, move_to(u, (35, 8))),
        (650, move_to(u, INTRUDER)),
        (850, move_to(w, (35, 44))),
        (1100, move_to(w, (35, 62))),
    ]);
    let mut counter = exposed(&m, u);
    let (mut lost, mut landed) = (0, 0);
    let mut resets: BTreeMap<&str, u32> = BTreeMap::new();
    while m.sim.tick() < 1300 {
        let cmds = script
            .get(&m.sim.tick())
            .map(|c| vec![(0, c.clone())])
            .unwrap_or_default();
        let events = m.step_with(cmds);
        assert!(rejections(&events).is_empty(), "{events:?}");
        let t = m.sim.tick();
        let tile = unit_tile(&m, u);
        let view = m.view();
        let e = view.attrition(u);
        let before = counter;
        counter = if e.is_some() { counter + 1 } else { 0 };
        assert_eq!(exposed(&m, u), counter, "tick {t} at {tile:?}");
        let want: Vec<(UnitId, i32)> = e
            .filter(|e| counter.is_multiple_of(e.interval_ticks))
            .map(|e| (u, e.hp))
            .into_iter()
            .collect();
        assert_eq!(pulses(&events), want, "tick {t}");
        lost += want.iter().map(|p| p.1).sum::<i32>();
        landed += want.len();
        if e.is_none() && before > 0 {
            let why = match view.territory().owner_at(tile) {
                None => "neutral",
                Some(P0) => "own",
                Some(_) if view.supplied(P0, tile) => "supplied",
                Some(_) => "other",
            };
            *resets.entry(why).or_default() += 1;
        }
    }
    assert_eq!(80 - hp(&m, u), lost, "no hidden damage");
    assert!(landed >= 6, "{landed}");
    assert_eq!(
        resets.keys().copied().collect::<Vec<_>>(),
        ["neutral", "own", "supplied"],
        "{resets:?}"
    );
    println!("resets {resets:?}, {landed} pulses, {lost} HP lost");
}

/// Only Scribes and Supply Wains are immune (`immune_to_attrition`):
/// over 1200 ticks a Yeoman goes from 40 to 22 HP and a Skirmisher from 80
/// to 62 (18 pulses, `1200 /` [`interval1`]), while a Scribe and a Supply
/// Wain (out of range of the others) keep every hit point and a counter of
/// 0.
#[test]
fn attrition_scribes_and_supply_wains_are_immune_yeomen_are_not() {
    let rules = load_rules();
    let n = pulses_by(1200, interval1(&rules));
    let k = |id: &str| ukind(&rules, id);
    let (mut m, _) = attrition_field(
        &rules,
        1,
        vec![
            (0, spawn_for(P0, k("yeoman"), (35, 33))),
            (0, spawn_for(P0, k("scribe"), (35, 29))),
            (0, spawn_for(P0, k("supply_wain"), (21, 49))),
        ],
    );
    let ids = [UnitId(1), UnitId(2), UnitId(3), UnitId(4)];
    for id in ids {
        assert_eq!(m.view().territory().owner_at(unit_tile(&m, id)), Some(P1));
    }
    assert!(!m.view().supplied(P0, unit_tile(&m, UnitId(2))));
    while m.sim.tick() < 1200 {
        m.step(1);
        assert_eq!((exposed(&m, UnitId(3)), exposed(&m, UnitId(4))), (0, 0));
    }
    let got: Vec<i32> = ids.iter().map(|&id| hp(&m, id)).collect();
    assert_eq!(
        got,
        [80 - n, 40 - n, 30, 150],
        "skirmisher, yeoman, scribe, wain"
    );
    assert_eq!(m.view().attrition(UnitId(3)), None);
    assert_eq!(m.view().attrition(UnitId(4)), None);
    println!("after 1200 ticks: skirmisher, yeoman, scribe, wain hp {got:?}");
}

/// The age-gap table and the late Harrying bonus: a Masonry owner deals 2
/// per pulse to a Hearth intruder (gap 1); a Hearth owner deals 1 to a
/// Masonry intruder (a negative gap reads the gap-0 row); with an in-memory
/// `harrying_bonus_hp` of 3 a Charter owner deals 5 to a Hearth intruder
/// (gap 2 clamps to the last row, 2, plus 3). After 200 ticks (3 pulses of
/// [`interval1`], and 8 ticks) the intruder has 74, 77 and 65 HP.
#[test]
fn attrition_age_gap_table_and_late_harrying_bonus() {
    let base = load_rules();
    let i = interval1(&base);
    let mut bonus = base.clone();
    bonus.harrying_bonus_hp = 3;
    // The commands that take `p` to `age` (DebugResearch skips the age
    // checks; DebugGrant pays exactly the ages' costs).
    let to_age = |rules: &Rules, p: PlayerId, age: usize| {
        let techs = [
            "letters_1",
            "trade_1",
            "arms_1",
            "statecraft_1",
            "letters_2",
        ];
        let needed = usize::try_from(rules.ages[age].techs_required).unwrap();
        let mut cmds: Vec<(u8, Command)> = techs[..needed]
            .iter()
            .map(|id| {
                (
                    p.0,
                    Command::DebugResearch {
                        owner: p,
                        tech: tech(rules, id),
                    },
                )
            })
            .collect();
        for a in 1..=age {
            for (r, amount) in rules.amounts(&rules.ages[a].cost).into_iter().enumerate() {
                if amount > 0 {
                    cmds.push((
                        p.0,
                        Command::DebugGrant {
                            owner: p,
                            resource: u8::try_from(r).unwrap(),
                            amount: u32::try_from(amount).unwrap(),
                        },
                    ));
                }
            }
            cmds.push((p.0, Command::AdvanceAge { player: p }));
        }
        cmds
    };
    let cases = [
        ("Masonry owner, Hearth intruder", &base, 1, 0, 2, 74),
        ("Hearth owner, Masonry intruder", &base, 0, 1, 1, 77),
        ("Charter owner with a bonus of 3", &bonus, 2, 0, 5, 65),
    ];
    for (name, rules, owner_age, intruder_age, per_pulse, want) in cases {
        let mut extra = to_age(rules, P0, intruder_age);
        extra.extend(to_age(rules, P1, owner_age));
        let (mut m, _) = attrition_field(rules, 1, extra);
        assert_eq!(
            (m.view().age(P1), m.view().age(P0)),
            (
                u8::try_from(owner_age).unwrap(),
                u8::try_from(intruder_age).unwrap()
            ),
            "{name}"
        );
        assert_eq!(
            m.view().attrition_pulse(P1, P0),
            Some((i, per_pulse)),
            "{name}"
        );
        let mut landed = Vec::new();
        while m.sim.tick() < 3 * i + 8 {
            landed.extend(pulses(&m.step(1)).into_iter().map(|p| p.1));
        }
        assert_eq!(landed, [per_pulse; 3], "{name}");
        assert_eq!(hp(&m, UnitId(1)), want, "{name}");
        println!(
            "{name}: {per_pulse} HP per pulse, hp {want} after {} ticks",
            m.sim.tick()
        );
    }
}

/// Attrition kills through the common death path: a Yeoman at 1 HP
/// (`DebugDamage` 39) walking inside P1's border dies to the pulse at tick
/// 64 ([`interval1`]): `UnitDied` with its owner and kind in that step, no
/// `Hit` in the whole run, and no path request left behind, although the
/// walk's periodic repath asked for one in the same step (the control,
/// without the damage, still has it pending after that step).
#[test]
fn attrition_kills_through_the_common_death_path() {
    let rules = load_rules();
    let yeoman = ukind(&rules, "yeoman");
    let u = UnitId(2);
    let i = interval1(&rules);
    let repath = rules.repath_interval_ticks();
    // The killing pulse lands at the first multiple of the interval that
    // leaves room for the walk to start in step 3 or later: tick 64 with
    // the shipped data. The pulses before it (none with the shipped data)
    // are paid for by a smaller `DebugDamage`.
    let kill_at = (repath + 4).div_ceil(i) * i;
    let earlier = pulses_by(kill_at - 1, i);
    // The walk's first periodic repath falls in step `kill_at - 1`, with
    // the pulse: a Move applied in step s repaths in step s + repath
    // (3 + 60 = 63 with the shipped data).
    let walk_from = kill_at - 1 - repath;
    for kill in [true, false] {
        let (mut m, _) = attrition_field(&rules, 1, vec![(0, spawn_for(P0, yeoman, (35, 33)))]);
        let mut all = Vec::new();
        if kill {
            let amount = u32::try_from(39 - earlier).unwrap();
            all.extend(m.step_with(vec![(0, damage(TargetId::Unit(u), amount))]));
            assert_eq!(hp(&m, u), 1 + earlier);
        } else {
            all.extend(m.step(1));
        }
        all.extend(m.step(walk_from - m.sim.tick()));
        assert_eq!(m.sim.tick(), walk_from);
        all.extend(m.step_with(vec![(0, move_to(u, (35, 20)))]));
        all.extend(m.step(kill_at - 1 - m.sim.tick()));
        assert_eq!(m.sim.tick(), kill_at - 1);
        assert_eq!(
            m.view().territory().owner_at(unit_tile(&m, u)),
            Some(P1),
            "still walking inside the border"
        );
        assert!(m.view().unit(u).unwrap().order.is_some());
        if kill {
            assert_eq!(hp(&m, u), 1, "one pulse from death");
        }
        let last = m.step(1);
        all.extend(last.iter().cloned());
        assert_eq!(m.sim.tick(), kill_at);
        assert!(pulses(&last).contains(&(u, 1)), "{last:?}");
        let died = SimEvent::UnitDied {
            unit: u,
            owner: P0,
            kind: yeoman,
        };
        if kill {
            assert!(last.contains(&died), "{last:?}");
            assert!(m.view().unit(u).is_none());
            assert!(!m.view().pathing().has_request(u), "the request went too");
        } else {
            assert!(!last.contains(&died));
            assert!(
                m.view().pathing().has_request(u),
                "the repath of step {}",
                kill_at - 1
            );
        }
        assert_eq!(hits(&all), 0);
    }
}

/// Attrition draws no randomness: with Harrying II the `rng` sub-hash
/// equals the no-Harrying control after every step of 600, while the
/// units differ (pulses landed, and killed a Yeoman left at 10 HP).
#[test]
fn attrition_draws_no_randomness() {
    let rules = load_rules();
    let extra = || vec![(0, spawn_for(P0, ukind(&rules, "yeoman"), (35, 33)))];
    let (mut a, _) = attrition_field(&rules, 2, extra());
    let (mut b, _) = attrition_field(&rules, 0, extra());
    let hurt = || vec![(0, damage(TargetId::Unit(UnitId(2)), 30))];
    let mut landed = pulses(&a.step_with(hurt())).len();
    b.step_with(hurt());
    let mut died = 0;
    for _ in 0..600 {
        let events = a.step(1);
        landed += pulses(&events).len();
        died += events
            .iter()
            .filter(|e| matches!(e, SimEvent::UnitDied { .. }))
            .count();
        b.step(1);
        assert_eq!(a.sim.sub_hashes().rng, b.sim.sub_hashes().rng);
    }
    assert_ne!(a.sim.sub_hashes().units, b.sim.sub_hashes().units);
    assert!(landed > 0 && died == 1, "{landed} {died}");
}

/// A restore mid-exposure continues exactly: the snapshot is taken when
/// the Skirmisher's counter is 37, while a Supply Wain walks toward it
/// (its tile changes after the restore, then it covers the Skirmisher) and
/// a Yeoman bleeds out of its range; Harrying II arrives later, the Wain
/// walks away again. Every sub-hash, the whole hash and the events agree
/// after every one of 600 steps, and again for 200 steps after 50 units
/// spawn inside P1's border.
#[test]
fn attrition_restore_mid_exposure_matches_uninterrupted() {
    let rules = load_rules();
    let (u, w) = (UnitId(1), UnitId(2));
    let (mut a, _) = attrition_field(
        &rules,
        1,
        vec![
            (0, spawn_for(P0, ukind(&rules, "supply_wain"), (35, 50))),
            (0, spawn_for(P0, ukind(&rules, "yeoman"), (30, 22))),
        ],
    );
    a.step_with(vec![(0, move_to(w, (35, 40)))]);
    while exposed(&a, u) < 37 {
        a.step(1);
    }
    assert_eq!((a.sim.tick(), exposed(&a, u)), (37, 37));
    let wain_tile = unit_tile(&a, w);
    let mut b = a.clone_at_tick();
    assert_eq!(a.sim.sub_hashes(), b.sim.sub_hashes());
    let script = |t: u32, rules: &Rules| -> Vec<(u8, Command)> {
        match t {
            100 => harrying_for(rules, P1, 2)[1..].to_vec(),
            300 => vec![(0, move_to(w, (35, 62)))],
            _ => Vec::new(),
        }
    };
    let (mut resets, mut landed, mut spawned) = (0, 0, 0);
    let mut wain_moved = false;
    for i in 0..800 {
        let t = a.sim.tick();
        let mut cmds = script(t, &rules);
        if i == 600 {
            cmds.push((
                0,
                Command::DebugSpawn {
                    owner: P0,
                    kind: ukind(&rules, "skirmisher"),
                    at: FxVec2::from_ints(30, 40),
                    count: 50,
                },
            ));
        }
        let before = a.view().unit(u).map_or(0, |x| x.exposed_ticks);
        let ea = a.step_with(cmds.clone());
        let eb = b.step_with(cmds);
        assert_eq!(ea, eb, "events after tick {t}");
        assert_eq!(a.sim.sub_hashes(), b.sim.sub_hashes(), "after tick {t}");
        assert_eq!(a.sim.hash(), b.sim.hash());
        landed += pulses(&ea).len();
        spawned += ea
            .iter()
            .filter(|e| matches!(e, SimEvent::UnitSpawned { .. }))
            .count();
        let after = a.view().unit(u).map_or(0, |x| x.exposed_ticks);
        resets += usize::from(before > 0 && after == 0);
        wain_moved |= unit_tile(&a, w) != wain_tile;
    }
    assert_eq!(spawned, 50);
    assert!(
        wain_moved && resets >= 1 && landed > 20,
        "{resets} {landed}"
    );
}

/// What `SimView::attrition` reports before a step is what the step
/// applies to a unit that stands still: a pulse of `hp` when the counter
/// reaches a multiple of `interval_ticks`, and no pulse and a counter of 0
/// when it reports `None`. P1 is in the Masonry Age with both Harrying
/// techs (32 ticks, [`interval2`], and 2 HP against Hearth intruders); the
/// units are an exposed Skirmisher and Yeoman, an immune Scribe, a
/// Skirmisher under its own Wain, one on neutral land, and P1 units on
/// their own and on neutral land.
#[test]
fn attrition_view_reports_the_pulse_the_step_applies() {
    let rules = load_rules();
    let k = |id: &str| ukind(&rules, id);
    let mut extra = vec![
        (0, spawn_for(P0, k("yeoman"), (35, 27))),
        (0, spawn_for(P0, k("scribe"), (35, 35))),
        (0, spawn_for(P0, k("skirmisher"), (21, 45))),
        (0, spawn_for(P0, k("supply_wain"), (21, 52))),
        (0, spawn_for(P0, k("skirmisher"), (45, 31))),
        (1, spawn_for(P1, k("bowman"), (5, 31))),
        (1, spawn_for(P1, k("bowman"), (5, 60))),
        (
            1,
            Command::DebugResearch {
                owner: P1,
                tech: tech(&rules, "letters_1"),
            },
        ),
        (
            1,
            Command::DebugResearch {
                owner: P1,
                tech: tech(&rules, "trade_1"),
            },
        ),
    ];
    for (r, amount) in rules.amounts(&rules.ages[1].cost).into_iter().enumerate() {
        if amount > 0 {
            extra.push((
                1,
                Command::DebugGrant {
                    owner: P1,
                    resource: u8::try_from(r).unwrap(),
                    amount: u32::try_from(amount).unwrap(),
                },
            ));
        }
    }
    extra.push((1, Command::AdvanceAge { player: P1 }));
    let (mut m, _) = attrition_field(&rules, 2, extra);
    let i2 = interval2(&rules);
    assert_eq!(m.view().attrition_pulse(P1, P0), Some((i2, 2)));
    assert_eq!(m.view().attrition_pulse(P0, P1), None);
    let ids: Vec<UnitId> = m.view().units().map(|u| u.id).collect();
    assert_eq!(ids.len(), 8);
    let (mut some, mut none) = (0, 0);
    for _ in 0..300 {
        let before: Vec<(UnitId, Option<Exposure>, u32, i32)> = ids
            .iter()
            .map(|&id| (id, m.view().attrition(id), exposed(&m, id), hp(&m, id)))
            .collect();
        let events = m.step(1);
        let landed: BTreeMap<UnitId, i32> = pulses(&events).into_iter().collect();
        for (id, e, n, h) in before {
            let next = if e.is_some() { n + 1 } else { 0 };
            let want = e
                .filter(|e| next.is_multiple_of(e.interval_ticks))
                .map(|e| e.hp);
            assert_eq!(landed.get(&id).copied(), want, "{id:?}");
            assert_eq!(exposed(&m, id), next, "{id:?}");
            assert_eq!(hp(&m, id), h - want.unwrap_or(0), "{id:?}");
            if e.is_some() {
                some += 1;
            } else {
                none += 1;
            }
        }
    }
    let exposed_units: Vec<UnitId> = ids
        .iter()
        .copied()
        .filter(|&id| m.view().attrition(id).is_some())
        .collect();
    assert_eq!(exposed_units, [UnitId(1), UnitId(2)]);
    assert_eq!(
        m.view().attrition(UnitId(1)),
        Some(Exposure {
            owner: P1,
            interval_ticks: i2,
            hp: 2
        })
    );
    assert!(some == 600 && none == 1800, "{some} {none}");
}

/// `DebugDamage` deals damage at once and leaves removal to step 5c: a
/// Skirmisher at 1 HP lives, one more point kills it (`UnitDied`); a
/// Watchtower at 1 HP stands, one more destroys it; a site is destroyed
/// like any building below 1 HP; a complete Town clamps at 0 with one
/// `TownAtZeroHp` and stays; a player may damage its own buildings; and
/// nothing draws from the RNG.
#[test]
fn attrition_debug_damage_kills_through_reap_and_clamps_towns() {
    let rules = load_rules();
    let mut m = M::delay0(rules.clone(), 6);
    let events = m.step_with(vec![
        (0, spawn_for(P0, ukind(&rules, "skirmisher"), (60, 60))),
        (0, place_for(P0, bkind(&rules, "croft"), (64, 64))),
        (1, place_for(P1, bkind(&rules, "watchtower"), (10, 10))),
        (1, place_for(P1, bkind(&rules, "town"), (20, 30))),
        (
            1,
            Command::DebugPlaceBuilding {
                owner: P1,
                kind: bkind(&rules, "croft"),
                tile: Tile::new(14, 14),
                complete: false,
            },
        ),
    ]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    let rng = m.sim.sub_hashes().rng;
    let (u, croft, tower, town, site) = (
        UnitId(1),
        BuildingId(1),
        BuildingId(2),
        BuildingId(3),
        BuildingId(4),
    );
    let bhp = |m: &M, b| m.view().building_hp(b).map(|h| h.0);
    let unit = TargetId::Unit(u);
    let events = m.step_with(vec![
        (0, damage(unit, 79)),
        (0, damage(TargetId::Building(tower), 499)),
        (0, damage(TargetId::Building(town), u32::MAX)),
        (0, damage(TargetId::Building(croft), 100)),
        (0, damage(TargetId::Building(site), 300)),
    ]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(hp(&m, u), 1);
    assert_eq!(bhp(&m, tower), Some(1));
    assert_eq!(bhp(&m, town), Some(0));
    assert_eq!(bhp(&m, croft), Some(200), "an own building");
    assert_eq!(bhp(&m, site), None, "a site at 0 is destroyed");
    let zero = SimEvent::TownAtZeroHp { town };
    assert_eq!(events.iter().filter(|e| **e == zero).count(), 1);
    assert!(events.contains(&SimEvent::BuildingDestroyed {
        building: site,
        owner: P1,
        kind: bkind(&rules, "croft"),
    }));
    let events = m.step_with(vec![
        (1, damage(unit, 1)),
        (1, damage(TargetId::Building(tower), 1)),
        (1, damage(TargetId::Building(town), 5)),
    ]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert!(events.contains(&SimEvent::UnitDied {
        unit: u,
        owner: P0,
        kind: ukind(&rules, "skirmisher"),
    }));
    assert!(events.contains(&SimEvent::BuildingDestroyed {
        building: tower,
        owner: P1,
        kind: bkind(&rules, "watchtower"),
    }));
    assert!(!events.contains(&zero), "only on the step that reaches 0");
    assert_eq!(bhp(&m, town), Some(0), "a complete Town stays at 0");
    assert!(m.view().unit(u).is_none() && bhp(&m, tower).is_none());
    assert_eq!(m.sim.sub_hashes().rng, rng, "no draw");
    assert_eq!(hits(&events), 0);
}

/// Rejected `DebugDamage` changes nothing but `commands_applied`: a unit
/// or building that does not exist, or a unit that has died, is
/// `InvalidTarget`; with `debug_commands` off it is
/// `DebugCommandsDisabled`, before the target is looked at. Each is
/// compared with a control on every sub-hash.
#[test]
fn attrition_debug_damage_rejections_change_nothing() {
    let rules = load_rules();
    let with_units = || {
        let mut m = M::new(rules.clone(), 7);
        m.ok(vec![
            spawn_for(P0, ukind(&rules, "skirmisher"), (60, 60)),
            spawn_for(P0, ukind(&rules, "yeoman"), (62, 60)),
        ]);
        m.ok(vec![damage(TargetId::Unit(UnitId(2)), 1000)]);
        assert!(m.view().unit(UnitId(2)).is_none());
        m
    };
    let no_debug = || {
        let mut setup = MatchSetup::scenario(&rules, 7);
        setup.debug_commands = false;
        M {
            sim: Sim::new(setup, rules.clone(), Box::new(NoAi)),
            seq: [0; 2],
        }
    };
    let cases: Vec<(&str, &dyn Fn() -> M, Command, RejectReason)> = vec![
        (
            "a unit that never existed",
            &with_units,
            damage(TargetId::Unit(UnitId(999)), 5),
            RejectReason::InvalidTarget,
        ),
        (
            "a unit that died",
            &with_units,
            damage(TargetId::Unit(UnitId(2)), 5),
            RejectReason::InvalidTarget,
        ),
        (
            "a building that does not exist",
            &with_units,
            damage(TargetId::Building(BuildingId(0)), 5),
            RejectReason::InvalidTarget,
        ),
        (
            "debug commands off",
            &no_debug,
            damage(TargetId::Unit(UnitId(999)), 5),
            RejectReason::DebugCommandsDisabled,
        ),
    ];
    for (name, setup, cmd, want) in cases {
        for p in [0, 1] {
            rejected_without_change(name, setup, p, cmd.clone(), want);
        }
    }
}

/// The rules of the M5a random streams: the shipped data with
/// `attrition_interval_ds: 4` (8 ticks, 4 at Harrying II), so a short
/// stream lands many pulses.
fn stream_rules() -> &'static Rules {
    static RULES: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    RULES.get_or_init(|| {
        let mut r = load_rules();
        r.attrition_interval_ds = 4;
        assert_eq!(r.attrition_interval_ticks(), 8);
        r
    })
}

/// A seeded random stream over both players for the delay-0 sim with
/// [`stream_rules`]. Step 0: P1's Town at [`FIELD_TOWN`] with Harrying I
/// (and II on odd seeds), P0's Town at (59,30) (the borders touch on row
/// 31) with Harrying I on seeds divisible by 3, and units of both sides on
/// each other's land: armed units, a Yeoman and a Scribe each, and a
/// Supply Wain each out of their range.
/// Then each tick may move a few units of one side to a random point of
/// the strip between the Towns, stop one, damage a random unit id (dead
/// or never spawned ones are rejected), research Harrying II for a random
/// player (rejected once researched) or spawn a unit.
fn random_attrition_commands(seed: u64, ticks: u32) -> Vec<Vec<PlayerCommand>> {
    let rules = stream_rules();
    let k = |id: &str| ukind(rules, id);
    let mut rng = rand_pcg::Pcg32::seed_from_u64(seed);
    let mut seq = [0u32; 2];
    let mut ids: [Vec<u32>; 2] = [Vec::new(), Vec::new()];
    let mut next_id = 1u32;
    let mut out = Vec::with_capacity(usize::try_from(ticks).unwrap());
    let kinds = ["skirmisher", "bowman", "yeoman", "scribe", "supply_wain"];
    let point = |rng: &mut rand_pcg::Pcg32| {
        (
            10 + i32::try_from(rng.next_u32() % 60).unwrap(),
            14 + i32::try_from(rng.next_u32() % 36).unwrap(),
        )
    };
    for t in 0..ticks {
        let mut cmds: Vec<PlayerCommand> = Vec::new();
        let mut push = |p: usize, c: Command| {
            cmds.push(PlayerCommand::new(
                PlayerId(u8::try_from(p).unwrap()),
                seq[p],
                c,
            ));
            seq[p] += 1;
        };
        if t == 0 {
            for (p, at) in [(0usize, (35, 31)), (1, (47, 31))] {
                let owner = PlayerId(u8::try_from(p).unwrap());
                for (i, kind) in kinds.iter().enumerate() {
                    // Two tiles apart; the Wain (last) 20 tiles south of
                    // the others, out of their supply range.
                    let dy = if *kind == "supply_wain" {
                        20
                    } else {
                        2 * i32::try_from(i).unwrap() - 4
                    };
                    push(p, spawn_for(owner, k(kind), (at.0, at.1 + dy)));
                    ids[p].push(next_id);
                    next_id += 1;
                }
            }
            push(0, place_for(P0, bkind(rules, "town"), (59, 30)));
            if seed.is_multiple_of(3) {
                push(0, harrying_for(rules, P0, 1)[0].1.clone());
            }
            push(1, place_for(P1, bkind(rules, "town"), FIELD_TOWN));
            for (_, c) in harrying_for(rules, P1, 1 + usize::from(seed % 2 == 1)) {
                push(1, c);
            }
        } else {
            let roll = rng.next_u32() % 100;
            let p = usize::try_from(rng.next_u32() % 2).unwrap();
            if roll < 14 {
                let n = 1 + rng.next_u32() % 3;
                let units = (0..n)
                    .map(|_| {
                        UnitId(ids[p][usize::try_from(rng.next_u32()).unwrap() % ids[p].len()])
                    })
                    .collect();
                let (x, y) = point(&mut rng);
                push(
                    p,
                    Command::Move {
                        units,
                        target: FxVec2::from_ints(x, y),
                        queue: false,
                    },
                );
            } else if roll < 17 {
                let u = ids[p][usize::try_from(rng.next_u32()).unwrap() % ids[p].len()];
                push(
                    p,
                    Command::Stop {
                        units: vec![UnitId(u)],
                    },
                );
            } else if roll < 22 {
                let target = UnitId(rng.next_u32() % (next_id + 2));
                let amount = 1 + rng.next_u32() % 30;
                push(p, damage(TargetId::Unit(target), amount));
            } else if roll < 23 {
                let owner = PlayerId(u8::try_from(p).unwrap());
                push(p, harrying_for(rules, owner, 2)[1].1.clone());
            } else if roll < 25 {
                let owner = PlayerId(u8::try_from(p).unwrap());
                let kind = kinds[usize::try_from(rng.next_u32()).unwrap() % kinds.len()];
                push(p, spawn_for(owner, k(kind), point(&mut rng)));
                ids[p].push(next_id);
                next_id += 1;
            }
        }
        out.push(cmds);
    }
    out
}

/// The delay-0 sim of the random streams, with [`stream_rules`] and `seed`.
fn stream_sim(seed: u64) -> Sim {
    let mut setup = MatchSetup::scenario(stream_rules(), seed);
    setup.cmd_delay = 0;
    Sim::new(setup, stream_rules().clone(), Box::new(NoAi))
}

/// Run `stream` on [`stream_sim`], restoring into a fresh sim after step
/// `restore_after` (never when it is past the end): the hash after every
/// step, and the events.
fn run_attrition_stream(
    seed: u64,
    stream: &[Vec<PlayerCommand>],
    restore_after: Option<usize>,
) -> (Vec<u64>, Vec<SimEvent>) {
    let fresh = || stream_sim(seed);
    let mut sim = fresh();
    let mut hashes = Vec::with_capacity(stream.len());
    let mut events = Vec::new();
    for (i, cmds) in stream.iter().enumerate() {
        sim.step(cmds);
        hashes.push(sim.hash());
        events.extend(sim.drain_events());
        if restore_after == Some(i) {
            let mut restored = fresh();
            restored.restore(&sim.snapshot()).expect("restores");
            sim = restored;
        }
    }
    (hashes, events)
}

/// The random attrition stream really exercises attrition: over 8 seeds
/// of 200 ticks, pulses land on both players' units (P0's Harrying comes on
/// seeds divisible by 3), some at Harrying II's interval (two pulses on one
/// unit [`interval2`] ticks apart, which only Harrying II allows: at
/// Harrying I they are at least [`interval1`] apart, a reset in between
/// included), units die, some of them to an attrition pulse alone
/// (`UnitDied` in the step that pulsed them, with no `Hit` on them and no
/// `DebugDamage` aimed at them in that step), counters reset (a living
/// unit's counter drops from above 0 to 0), and `DebugDamage` is both
/// accepted and rejected (`InvalidTarget`, which no other command in the
/// stream can cause).
#[test]
fn m5a_attrition_stream_exercises_attrition() {
    let rules = stream_rules();
    let (i1, i2) = (interval1(rules), interval2(rules));
    assert!(i2 < i1, "{i1} {i2}");
    let (mut on, mut died, mut rejected) = ([0usize; 2], 0, 0);
    let (mut killed, mut fast, mut resets, mut accepted) = (0, 0, 0, 0);
    for seed in 0..8 {
        let stream = random_attrition_commands(seed, 200);
        let owner: BTreeMap<UnitId, PlayerId> = {
            let mut next = 0u32;
            let mut map = BTreeMap::new();
            for pc in stream.iter().flatten() {
                if let Command::DebugSpawn { owner, .. } = pc.cmd {
                    next += 1;
                    map.insert(UnitId(next), owner);
                }
            }
            map
        };
        let mut sim = stream_sim(seed);
        let mut last_pulse: BTreeMap<UnitId, u32> = BTreeMap::new();
        for cmds in &stream {
            let before: BTreeMap<UnitId, u32> = sim
                .view()
                .units()
                .map(|u| (u.id, u.exposed_ticks))
                .collect();
            sim.step(cmds);
            let events = sim.drain_events();
            let t = sim.tick();
            let pulsed: Vec<UnitId> = pulses(&events).into_iter().map(|p| p.0).collect();
            let struck: Vec<TargetId> = cmds
                .iter()
                .filter_map(|pc| match pc.cmd {
                    Command::DebugDamage { target, .. } => Some(target),
                    _ => None,
                })
                .chain(events.iter().filter_map(|e| match e {
                    SimEvent::Hit { target, .. } => Some(*target),
                    _ => None,
                }))
                .collect();
            for &unit in &pulsed {
                on[usize::from(owner[&unit].0)] += 1;
                if let Some(prev) = last_pulse.insert(unit, t) {
                    assert!(t - prev >= i2, "{unit:?} pulsed at {prev} and {t}");
                    fast += usize::from(t - prev == i2);
                }
            }
            let mut refused = Vec::new();
            for e in &events {
                match e {
                    SimEvent::UnitDied { unit, .. } => {
                        died += 1;
                        killed += usize::from(
                            pulsed.contains(unit) && !struck.contains(&TargetId::Unit(*unit)),
                        );
                    }
                    SimEvent::CommandRejected {
                        player,
                        seq,
                        reason: RejectReason::InvalidTarget,
                    } => {
                        rejected += 1;
                        refused.push((*player, *seq));
                    }
                    _ => {}
                }
            }
            accepted += cmds
                .iter()
                .filter(|pc| matches!(pc.cmd, Command::DebugDamage { .. }))
                .filter(|pc| !refused.contains(&(pc.player, pc.seq)))
                .count();
            resets += sim
                .view()
                .units()
                .filter(|u| u.exposed_ticks == 0 && before.get(&u.id).is_some_and(|&n| n > 0))
                .count();
        }
    }
    eprintln!(
        "m5a attrition stream, 8 seeds: pulses on P0, P1 units {on:?}, {fast} at Harrying II's interval ({i2} ticks), died {died} ({killed} to a pulse alone), counters reset {resets}, DebugDamage accepted {accepted} and rejected {rejected}"
    );
    assert!(on[0] > 0 && on[1] > 0, "{on:?}");
    assert!(fast > 0, "no pulse at Harrying II's interval");
    assert!(died > 0 && killed > 0, "{died} {killed}");
    assert!(resets > 0);
    assert!(accepted > 0 && rejected > 0, "{accepted} {rejected}");
}

/// Proptest case count: `PROPTEST_CASES` when set, else 256 in CI and 8
/// locally (the `cargo test -p sim` budget), as in `m4a.rs`.
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

    /// A random attrition stream gives the same hash after every step on
    /// a second run that restores from a snapshot at a random step.
    #[test]
    fn m5a_attrition_stream_is_deterministic_with_restore(
        seed in any::<u64>(),
        ticks in 60u32..240,
        cut in 0u32..1000,
    ) {
        let stream = random_attrition_commands(seed, ticks);
        let (a, _) = run_attrition_stream(seed, &stream, None);
        let at = usize::try_from(cut % ticks).unwrap();
        let (b, _) = run_attrition_stream(seed, &stream, Some(at));
        prop_assert_eq!(a, b, "restored after step {}", at);
    }
}

// ---------------------------------------------------------------------------
// Annexation and the outcome (slice 3): `cargo test -p sim annexation`.
// ---------------------------------------------------------------------------

/// P1's Town in the annexation tests ([`annex_with`]), placed first, at
/// [`FIELD_TOWN`] (north-west tile (20,30), centre (21.5,31.5), footprint
/// x 20-22, y 30-32), all in the all-grass west rectangle of
/// `plains_1v1`.
const TOWN: BuildingId = BuildingId(1);

/// The tiles P0's Skirmishers stand on in [`annex_setup`]: the north row
/// of the Town's ring (Chebyshev 1 from the footprint). Every probe and
/// every other P1 unit or building of these tests stands at least 7.25
/// tiles edge to edge from them, beyond every unit's reach (6), so nothing
/// acquires anything and the results do not depend on the annex-hold rule.
const POSTS: [(i32, i32); 4] = [(20, 29), (21, 29), (22, 29), (23, 29)];

/// The step in which [`annex_setup`]'s timer starts (`T0`): the attackers
/// spawn and the Town drops to 0 in step 1 (delay 0).
const T0: u32 = 1;

/// The south probe tile at exactly the defender radius from the Town's
/// centre (6.0 with the shipped data: (21,37), centre (21.5,37.5)).
fn probe_in(rules: &Rules) -> (i32, i32) {
    (
        21,
        31 + i32::try_from(rules.annexation_defender_radius_tiles).unwrap(),
    )
}

/// One tile farther south than [`probe_in`] (7.0: (21,38)).
fn probe_out(rules: &Rules) -> (i32, i32) {
    let (x, y) = probe_in(rules);
    (x, y + 1)
}

/// Steps from the entry step to the flip (`Rules::annexation_ticks()`,
/// 1200 with the shipped data: the annexation tests read it, as the
/// attrition tests read the interval).
fn total(rules: &Rules) -> u32 {
    rules.annexation_ticks()
}

/// The Town's hit points after a flip: `annexation_town_hp_permille` of
/// its kind's (500 with the shipped data).
fn flipped_hp(rules: &Rules) -> i32 {
    let hp = i64::from(rules.building_kind_by_id("town").unwrap().1.hp);
    i32::try_from(hp * i64::from(rules.annexation_town_hp_permille) / 1000).unwrap()
}

/// A Skirmisher of P0 on each of [`POSTS`].
fn on_posts(rules: &Rules) -> Vec<(u8, Command)> {
    POSTS
        .iter()
        .map(|&at| (0, spawn_for(P0, ukind(rules, "skirmisher"), at)))
        .collect()
}

/// [`annex_with`]'s sim after step `T0` and what it saw.
struct Annex {
    m: M,
    /// The events of step `T0`.
    entry: Vec<SimEvent>,
    /// The tiles P1 owned after step 0.
    p1_tiles: Vec<Tile>,
}

/// Delay 0. Step 0: P1 places its complete Town at [`FIELD_TOWN`]
/// ([`TOWN`]), then `step0` (P1's commands, after the Town). Step `T0`:
/// P0's `DebugDamage` takes the Town to 0, then `step1` in order (P0's
/// before P1's). Nothing is rejected.
fn annex_with(rules: &Rules, step0: Vec<Command>, step1: Vec<(u8, Command)>) -> Annex {
    let mut m = M::delay0(rules.clone(), 8);
    let mut cmds = vec![(1, place_for(P1, bkind(rules, "town"), FIELD_TOWN))];
    cmds.extend(step0.into_iter().map(|c| (1, c)));
    let events = m.step_with(cmds);
    assert!(rejections(&events).is_empty(), "{events:?}");
    let field = m.view().territory();
    let p1_tiles: Vec<Tile> = (0..field.height())
        .flat_map(|y| (0..field.width()).map(move |x| Tile::new(x, y)))
        .filter(|t| field.owner_at(*t) == Some(P1))
        .collect();
    let mut cmds = vec![(0, damage(TargetId::Building(TOWN), u32::MAX))];
    cmds.extend(step1);
    let entry = m.step_with(cmds);
    assert!(rejections(&entry).is_empty(), "{entry:?}");
    assert_eq!(m.sim.tick(), T0 + 1);
    Annex { m, entry, p1_tiles }
}

/// The standard annexation field: [`annex_with`] with P0's Skirmishers on
/// [`POSTS`] and nothing else; the timer starts in step `T0`.
fn annex_setup(rules: &Rules) -> Annex {
    annex_with(rules, vec![], on_posts(rules))
}

/// Step `m` until `sim.tick() == t` (the next step is the one at tick
/// `t`), collecting the events.
fn step_to(m: &mut M, t: u32) -> Vec<SimEvent> {
    let mut events = Vec::new();
    while m.sim.tick() < t {
        events.extend(m.step(1));
    }
    events
}

fn started(events: &[SimEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, SimEvent::AnnexationStarted { .. }))
        .count()
}

fn resets(events: &[SimEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, SimEvent::AnnexationReset { .. }))
        .count()
}

fn annexed(events: &[SimEvent]) -> Vec<(BuildingId, PlayerId, PlayerId)> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::TownAnnexed { town, from, to } => Some((*town, *from, *to)),
            _ => None,
        })
        .collect()
}

fn decided(events: &[SimEvent]) -> Vec<MatchResult> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::MatchDecided { result } => Some(*result),
            _ => None,
        })
        .collect()
}

fn hold(m: &M, b: BuildingId) -> Hold {
    m.view().building(b).unwrap().town_state.unwrap().hold
}

/// Whether the territory field equals a full recompute of its sources.
fn field_is_exact(m: &M) -> bool {
    let field = m.view().territory();
    *field == field.recomputed()
}

/// Acceptance 2, first clause: a Town at 0 HP with enemy Skirmishers
/// adjacent and no defender military within 6 tiles flips after 1200
/// ticks ([`total`]). `AnnexationStarted { town: 1, attacker: P0 }` lands
/// in step `T0`; after step `T0 + k`, for every `k` below 1200,
/// `SimView::annexation` is `{ P0, k, 1200 }` and nothing has flipped; in
/// step `T0 + 1200` the Town flips to P0 (`TownAnnexed`), returns `Held`
/// at 500 HP ([`flipped_hp`]), and P0 owns exactly the tiles P1 owned
/// (1245), with the field equal to a full recompute. P1 has no complete
/// Town left, so the same step decides `Victory { P0, decisive: true }`,
/// reason `AllTownsLost`, at tick `T0 + 1201`.
#[test]
fn annexation_town_at_0_hp_with_skirmishers_adjacent_and_no_defender_flips_after_1200_ticks() {
    let rules = load_rules();
    let n = total(&rules);
    let Annex {
        mut m,
        entry,
        p1_tiles,
    } = annex_setup(&rules);
    assert!(
        entry.contains(&SimEvent::AnnexationStarted {
            town: TOWN,
            attacker: P0
        }),
        "{entry:?}"
    );
    assert!(entry.contains(&SimEvent::TownAtZeroHp { town: TOWN }));
    let view_at = |m: &M| m.view().annexation(TOWN);
    assert_eq!(
        view_at(&m),
        Some(AnnexView {
            attacker: P0,
            ticks: 0,
            total: n
        })
    );
    for k in 1..n {
        let events = m.step(1);
        assert!(annexed(&events).is_empty(), "no flip in step T0 + {k}");
        assert_eq!(
            view_at(&m),
            Some(AnnexView {
                attacker: P0,
                ticks: k,
                total: n
            }),
            "after step T0 + {k}"
        );
        assert_eq!(m.view().outcome(), None);
    }
    assert_eq!(m.sim.tick(), T0 + n);
    let events = m.step(1);
    assert_eq!(
        annexed(&events),
        [(TOWN, P1, P0)],
        "the flip in step T0 + {n}"
    );
    let view = m.view();
    let town = view.building(TOWN).unwrap();
    assert_eq!(town.owner, P0);
    assert_eq!(town.town_state.unwrap().hold, Hold::Held);
    assert_eq!(view.building_hp(TOWN).unwrap().0, flipped_hp(&rules));
    assert_eq!(view.annexation(TOWN), None);
    let p0 = view.territory().owned_tile_count(P0);
    assert_eq!(p0, u32::try_from(p1_tiles.len()).unwrap());
    assert_eq!(view.territory().owned_tile_count(P1), 0);
    assert!(field_is_exact(&m));
    let want = MatchResult {
        outcome: Outcome::Victory {
            player: P0,
            decisive: true,
        },
        reason: OutcomeReason::AllTownsLost,
        tick: T0 + n + 1,
    };
    assert_eq!(m.sim.result(), Some(want));
    assert_eq!(decided(&events), [want]);
    println!(
        "entry in step {T0}, flip in step {} (TownAnnexed {:?}), hp {}, P0 owns {p0} tiles, result {want:?}",
        T0 + n,
        annexed(&events),
        flipped_hp(&rules)
    );
}

/// Acceptance 2, second clause: a defender arriving at tick 600 resets the
/// timer. A P1 Shieldbearer appears on [`probe_in`] (exactly 6.0 tiles
/// from the Town's centre) in step `T0 + 600`, when the timer had counted
/// 599: the same step emits `AnnexationReset`, the Town is `Held` and P1
/// owns its 1245 tiles again. During `T0 + 650` a `Move` takes it to
/// (21,40), beyond the radius: the timer starts again in a later step `T1`
/// and the Town flips in exactly `T1 + 1200`, not at `T0 + 1200`. No `Hit`
/// lands in the whole run.
#[test]
fn annexation_defender_arriving_at_tick_600_resets_the_timer() {
    let rules = load_rules();
    let n = total(&rules);
    let Annex {
        mut m, p1_tiles, ..
    } = annex_setup(&rules);
    let mut all = step_to(&mut m, T0 + 600);
    assert_eq!(m.view().annexation(TOWN).unwrap().ticks, 599);
    let shieldbearer = UnitId(5);
    let events = m.step_with(vec![(
        1,
        spawn_for(P1, ukind(&rules, "shieldbearer"), probe_in(&rules)),
    )]);
    assert!(events.contains(&SimEvent::UnitSpawned { unit: shieldbearer }));
    assert!(
        events.contains(&SimEvent::AnnexationReset { town: TOWN }),
        "{events:?}"
    );
    assert_eq!(hold(&m, TOWN), Hold::Held);
    assert_eq!(m.view().annexation(TOWN), None);
    assert_eq!(
        m.view().territory().owned_tile_count(P1),
        u32::try_from(p1_tiles.len()).unwrap()
    );
    assert!(field_is_exact(&m));
    all.extend(events);
    all.extend(step_to(&mut m, T0 + 650));
    assert_eq!(hold(&m, TOWN), Hold::Held, "held while the defender stays");
    let mut t1 = None;
    let mut cmds = vec![(1, move_to(shieldbearer, (21, 40)))];
    while t1.is_none() {
        assert!(m.sim.tick() < T0 + 800, "the timer starts again");
        let t = m.sim.tick();
        let events = m.step_with(std::mem::take(&mut cmds));
        if started(&events) == 1 {
            t1 = Some(t);
        }
        all.extend(events);
    }
    let t1 = t1.unwrap();
    while m.sim.tick() < t1 + n {
        let t = m.sim.tick();
        let events = m.step(1);
        assert!(annexed(&events).is_empty(), "no flip in step {t}");
        all.extend(events);
    }
    let events = m.step(1);
    assert_eq!(
        annexed(&events),
        [(TOWN, P1, P0)],
        "the flip in step T1 + {n}"
    );
    all.extend(events);
    assert_eq!(hits(&all), 0, "nothing fought");
    assert!(t1 + n > T0 + n);
    println!(
        "reset in step {} (timer at 599), restarted in step T1 = {t1}, flip in step {}",
        T0 + 600,
        t1 + n
    );
}

/// Acceptance 2, third clause: during the timer `owner_at` returns neutral
/// for the Town's tiles and Train is rejected. Every tile P1 owned before
/// the entry is neutral, P1 owns none, the Town's source stamps the
/// neutral layer and the field equals a full recompute. A P1 `Train` of a
/// Yeoman at the Town, with the Grain granted, is rejected
/// `TownContested`, and `check_train` agrees; every sub-hash equals a
/// control whose rejected command is a `Train` at a building that does not
/// exist. The Yeoman queued before the entry keeps its progress for the
/// whole timer; after a reset (a P1 Shieldbearer on [`probe_in`]) the
/// queue moves again and `Train` is accepted.
#[test]
fn annexation_during_the_timer_owner_at_is_neutral_and_train_is_rejected() {
    let rules = load_rules();
    let yeoman = ukind(&rules, "yeoman");
    let grain = u8::try_from(res(&rules, "grain")).unwrap();
    let p1_grant = |amount| Command::DebugGrant {
        owner: P1,
        resource: grain,
        amount,
    };
    let train = |b: BuildingId| Command::Train {
        building: b,
        kind: yeoman,
    };
    let setup = || annex_with(&rules, vec![p1_grant(200), train(TOWN)], on_posts(&rules));
    let Annex {
        mut m, p1_tiles, ..
    } = setup();
    let mut control = setup().m;
    let progress = |m: &M| m.view().building(TOWN).unwrap().queue[0].progress;
    let queued = progress(&m);
    assert_eq!(queued, 1, "the head trained during step 0 only");
    for _ in 0..50 {
        m.step(1);
        control.step(1);
        let view = m.view();
        let field = view.territory();
        assert!(p1_tiles.iter().all(|t| field.owner_at(*t).is_none()));
        assert_eq!(field.owned_tile_count(P1), 0);
        assert_eq!(field.sources()[&TOWN].owner, sim::SourceOwner::Neutral);
        assert_eq!(progress(&m), queued, "the queue is paused");
    }
    assert!(field_is_exact(&m));
    assert_eq!(
        m.view().check_train(P1, TOWN, yeoman),
        Err(RejectReason::TownContested)
    );
    let events = m.step_with(vec![(1, train(TOWN))]);
    let control_events = control.step_with(vec![(1, train(BuildingId(99)))]);
    assert_eq!(rejections(&events), [RejectReason::TownContested]);
    assert_eq!(rejections(&control_events), [RejectReason::UnknownBuilding]);
    assert_eq!(m.sim.sub_hashes(), control.sim.sub_hashes());
    assert_eq!(m.sim.hash(), control.sim.hash());
    assert_eq!(m.view().building(TOWN).unwrap().queue.len(), 1);
    // A defender resets the timer; the queue moves again and Train is
    // accepted.
    let events = m.step_with(vec![(
        1,
        spawn_for(P1, ukind(&rules, "shieldbearer"), probe_in(&rules)),
    )]);
    assert_eq!(resets(&events), 1);
    assert_eq!(m.view().check_train(P1, TOWN, yeoman).map(|_| ()), Ok(()));
    let events = m.step_with(vec![(1, train(TOWN))]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(m.view().building(TOWN).unwrap().queue.len(), 2);
    assert!(progress(&m) > queued, "the queue moves again");
    let owned = m.view().territory().owned_tile_count(P1);
    assert_eq!(owned, u32::try_from(p1_tiles.len()).unwrap());
    println!(
        "during the timer: {} tiles neutral, P1 owns 0, Train rejected TownContested; after the reset P1 owns {owned} and Train is accepted",
        p1_tiles.len()
    );
}

/// Owner decision 5, the Research half: during the timer a new `Research`
/// at the contested Town is rejected `TownContested` (check 3, before the
/// tech and host checks) and a research entry queued there pauses.
///
/// With the shipped rules no tech is hosted by a Town, but the rejection
/// is reachable: `check_research` of Letters I there returns
/// `TownContested` (`CannotResearchHere` without the check), and so does
/// a `Research` of the tech id past the last (`UnknownTech` without it),
/// which changes no sub-hash compared with a control whose rejected
/// command is a `Research` at a building that does not exist.
///
/// With Letters I and Trade I hosted by the Town in memory, Letters I,
/// queued there in step 0, keeps its progress for 50 steps of the timer; a
/// new `Research` of Trade I is rejected `TownContested`, as
/// `check_research` says, with every sub-hash equal to the control's.
/// After a reset (a P1 Shieldbearer on [`probe_in`]) Letters I moves again
/// and Trade I is accepted.
#[test]
fn annexation_during_the_timer_research_is_rejected_and_a_research_entry_pauses() {
    let shipped = load_rules();
    let research = |building, tech| (1, Command::Research { building, tech });
    // The shipped rules: the rejection, before the tech and host checks.
    let setup = || annex_setup(&shipped).m;
    let (mut m, mut control) = (setup(), setup());
    m.step(10);
    control.step(10);
    let letters = tech(&shipped, "letters_1");
    assert_eq!(
        m.view().check_research(P1, TOWN, letters).map(|_| ()),
        Err(RejectReason::TownContested)
    );
    let past = TechId(u16::try_from(shipped.techs.len()).unwrap());
    let events = m.step_with(vec![research(TOWN, past)]);
    let control_events = control.step_with(vec![research(BuildingId(99), past)]);
    assert_eq!(rejections(&events), [RejectReason::TownContested]);
    assert_eq!(rejections(&control_events), [RejectReason::UnknownBuilding]);
    assert_eq!(m.sim.sub_hashes(), control.sim.sub_hashes());
    assert_eq!(m.sim.hash(), control.sim.hash());
    // Letters I and Trade I hosted by the Town, in memory.
    let mut rules = shipped.clone();
    let trade = tech(&rules, "trade_1");
    for t in [letters, trade] {
        rules.techs[usize::from(t.0)].host = "town".to_string();
    }
    let mut step0: Vec<Command> = (0..rules.resources.resources.len())
        .map(|r| Command::DebugGrant {
            owner: P1,
            resource: u8::try_from(r).unwrap(),
            amount: 500,
        })
        .collect();
    step0.push(Command::Research {
        building: TOWN,
        tech: letters,
    });
    let setup = || annex_with(&rules, step0.clone(), on_posts(&rules)).m;
    let (mut m, mut control) = (setup(), setup());
    let progress = |m: &M| m.view().building(TOWN).unwrap().queue[0].progress;
    let queued = progress(&m);
    assert_eq!(queued, 1, "Letters I researched during step 0 only");
    for _ in 0..50 {
        m.step(1);
        control.step(1);
        assert_eq!(progress(&m), queued, "the research entry is paused");
    }
    assert_eq!(
        m.view().check_research(P1, TOWN, trade).map(|_| ()),
        Err(RejectReason::TownContested)
    );
    let events = m.step_with(vec![research(TOWN, trade)]);
    let control_events = control.step_with(vec![research(BuildingId(99), trade)]);
    assert_eq!(rejections(&events), [RejectReason::TownContested]);
    assert_eq!(rejections(&control_events), [RejectReason::UnknownBuilding]);
    assert_eq!(m.sim.sub_hashes(), control.sim.sub_hashes());
    assert_eq!(m.sim.hash(), control.sim.hash());
    assert_eq!(m.view().building(TOWN).unwrap().queue.len(), 1);
    // A defender resets the timer: Letters I moves again and Trade I is
    // accepted.
    let events = m.step_with(vec![(
        1,
        spawn_for(P1, ukind(&rules, "shieldbearer"), probe_in(&rules)),
    )]);
    assert_eq!(resets(&events), 1);
    assert_eq!(m.view().check_research(P1, TOWN, trade).map(|_| ()), Ok(()));
    let events = m.step_with(vec![research(TOWN, trade)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(m.view().building(TOWN).unwrap().queue.len(), 2);
    assert!(progress(&m) > queued, "the research entry moves again");
    println!(
        "during the timer: Research rejected TownContested (shipped rules and Town-hosted techs), Letters I paused at {queued}; after the reset at {} with Trade I queued",
        progress(&m)
    );
}

/// The `capture_seat` scenario (`sim::scenarios::capture_seat`) run to the
/// first step with an outcome, at most `CAPTURE_SEAT_MAX_TICKS`.
fn capture_seat_to_outcome(rules: &Rules) -> (Sim, sim::scenarios::Stream, Vec<(u32, SimEvent)>) {
    let (setup, stream, cap) = sim::scenarios::by_name("capture_seat", rules).unwrap();
    let mut sim = Sim::new(setup, rules.clone(), Box::new(NoAi));
    let mut events = Vec::new();
    while sim.outcome().is_none() {
        let t = sim.tick();
        assert!(t < cap, "decided within {cap} ticks");
        sim.step(sim::scenarios::commands_at(&stream, t));
        events.extend(sim.drain_events().into_iter().map(|e| (t, e)));
    }
    (sim, stream, events)
}

/// Acceptance 2, fourth clause: capturing the Seat makes `outcome()` return
/// `Victory { decisive: true }`. The `capture_seat` stream decides
/// `Victory { P0, decisive: true }`, reason `SeatCaptured`, at
/// `result.tick == sim.tick()`; P1's Seat (`BuildingId(2)`) is P0's and
/// still carries the seat flag; 300 more steps change nothing about the
/// result and decide nothing again; and the golden fixture ends at this
/// decision tick, with this hash.
#[test]
fn annexation_capturing_the_seat_makes_outcome_a_decisive_victory() {
    let rules = load_rules();
    let (mut sim, stream, events) = capture_seat_to_outcome(&rules);
    let seat = BuildingId(2);
    let result = sim.result().unwrap();
    assert_eq!(
        result.outcome,
        Outcome::Victory {
            player: P0,
            decisive: true
        }
    );
    assert_eq!(sim.outcome(), Some(result.outcome));
    assert_eq!(result.reason, OutcomeReason::SeatCaptured);
    assert_eq!(result.tick, sim.tick());
    let b = sim.view().building(seat).unwrap().clone();
    assert_eq!(b.owner, P0);
    assert!(b.town_state.unwrap().seat, "the seat flag stays");
    let last: Vec<SimEvent> = events
        .iter()
        .filter(|(t, _)| *t + 1 == result.tick)
        .map(|(_, e)| e.clone())
        .collect();
    assert_eq!(annexed(&last), [(seat, P1, P0)]);
    assert_eq!(decided(&last), [result]);
    let (decision_hash, decision_tick) = (sim.hash(), sim.tick());
    for _ in 0..300 {
        let t = sim.tick();
        sim.step(sim::scenarios::commands_at(&stream, t));
        assert!(decided(&sim.drain_events()).is_empty());
    }
    assert_eq!(sim.result(), Some(result), "latched");
    let file = sim::ReplayReader::open(&fixtures_dir().join("capture_seat.eonreplay")).unwrap();
    let last_hash = file
        .records
        .iter()
        .filter_map(|r| match r {
            sim::replay::Record::Hash(h) => Some((h.tick, h.hash)),
            sim::replay::Record::Tick(_) => None,
        })
        .next_back()
        .unwrap();
    assert_eq!(
        last_hash,
        (decision_tick, decision_hash),
        "the fixture ends at the decision"
    );
    println!(
        "capture_seat: {:?} reason {:?} at tick {} (hash 0x{decision_hash:016x}); Seat owner {:?}, seat flag {}",
        result.outcome,
        result.reason,
        result.tick,
        b.owner,
        b.town_state.unwrap().seat
    );
}

/// Acceptance 2, fifth clause: a 54000-tick scripted stalemate returns
/// `Victory { decisive: false }` for the player with more territory. The
/// `stalemate_45min` stream (no unit ever exists; P0 owns 1248 tiles, P1
/// 1245) is undecided after 53999 steps and decided after 54000
/// (`Rules::match_cap_ticks()`): `Victory { P0, decisive: false }`, reason
/// `Territory`, tick 54000. Not ignored: it is the acceptance line.
#[test]
fn annexation_54000_tick_scripted_stalemate_is_a_tiebreak_victory_for_more_territory() {
    let rules = load_rules();
    let cap = rules.match_cap_ticks();
    let (setup, stream, ticks) = sim::scenarios::by_name("stalemate_45min", &rules).unwrap();
    assert_eq!(ticks, cap);
    let mut sim = sim::scenarios::run(setup, rules.clone(), &stream, cap - 1);
    assert_eq!(sim.tick(), cap - 1);
    assert_eq!(sim.view().unit_count(), 0);
    assert_eq!(sim.outcome(), None, "undecided after {} steps", cap - 1);
    sim.drain_events();
    sim.step(&[]);
    let view = sim.view();
    let (p0, p1) = (
        view.territory().owned_tile_count(P0),
        view.territory().owned_tile_count(P1),
    );
    assert!(p0 > p1, "{p0} > {p1}");
    let want = MatchResult {
        outcome: Outcome::Victory {
            player: P0,
            decisive: false,
        },
        reason: OutcomeReason::Territory,
        tick: cap,
    };
    assert_eq!(sim.result(), Some(want));
    assert_eq!(decided(&sim.drain_events()), [want]);
    println!(
        "stalemate after {cap} steps: P0 {p0} tiles, P1 {p1}; {:?} reason {:?} at tick {}",
        want.outcome, want.reason, want.tick
    );
}

/// The defender radius is 6 tiles from the Town's centre, inclusive, centre
/// to centre (Euclidean): a P1 Shieldbearer appearing on [`probe_in`]
/// (exactly the radius) during the timer resets it; one on [`probe_out`]
/// (a tile farther) does not, and neither does one on the tile east of
/// [`probe_in`] (`sqrt(r² + 1)`, 6.08 with the shipped data), which a
/// square (Chebyshev) radius or one half a tile wider would count; both
/// let the timer run on to the flip. One on (19,36), off the axis inside
/// the radius (`sqrt(29)`, 5.39), resets it in the step it appears, which
/// a diamond (Manhattan, 7) radius would not; that probe stands closer to
/// the posts than the others, so the test stops there.
#[test]
fn annexation_defender_radius_is_6_tiles_inclusive() {
    let rules = load_rules();
    let n = total(&rules);
    let (x, y) = probe_in(&rules);
    let east = (x + 1, y);
    let diagonal = (19, 36);
    let dist_sq = |(px, py): (i32, i32)| (2 * px + 1 - 43).pow(2) + (2 * py + 1 - 63).pow(2);
    let r2 = 4 * i32::try_from(rules.annexation_defender_radius_tiles.pow(2)).unwrap();
    assert!(dist_sq(east) > r2 && dist_sq(diagonal) <= r2);
    let probes = [
        (probe_in(&rules), true, true),
        (probe_out(&rules), false, true),
        (east, false, true),
        (diagonal, true, false),
    ];
    for (probe, inside, to_the_flip) in probes {
        let Annex { mut m, .. } = annex_setup(&rules);
        step_to(&mut m, T0 + 10);
        let events = m.step_with(vec![(
            1,
            spawn_for(P1, ukind(&rules, "shieldbearer"), probe),
        )]);
        assert_eq!(resets(&events), usize::from(inside), "{probe:?}");
        if !to_the_flip {
            assert_eq!(hold(&m, TOWN), Hold::Held, "{probe:?}");
            println!("defender at {probe:?}: resets the timer {inside}");
            continue;
        }
        let events = step_to(&mut m, T0 + n + 1);
        if inside {
            assert_eq!(hold(&m, TOWN), Hold::Held, "{probe:?}");
            assert!(annexed(&events).is_empty());
        } else {
            assert_eq!(annexed(&events), [(TOWN, P1, P0)], "{probe:?}");
            assert_eq!(hits(&events), 0);
        }
        println!("defender at {probe:?}: resets the timer {inside}");
    }
}

/// A Town above 0 HP never starts the timer, even with enemy Skirmishers
/// beside it and no defender: P0's four Skirmishers on [`POSTS`] beside
/// P1's Town at full hit points chip at it for 200 steps (it keeps hit
/// points and they all live), with no `AnnexationStarted`, the Town `Held`
/// and P1's land unchanged; the step whose `DebugDamage` takes it to 0
/// starts the timer.
#[test]
fn annexation_town_above_0_hp_never_starts_the_timer() {
    let rules = load_rules();
    let mut m = M::delay0(rules.clone(), 8);
    let e = m.step_with(vec![(1, place_for(P1, bkind(&rules, "town"), FIELD_TOWN))]);
    assert!(rejections(&e).is_empty(), "{e:?}");
    let p1 = m.view().territory().owned_tile_count(P1);
    let mut events = m.step_with(on_posts(&rules));
    assert!(rejections(&events).is_empty(), "{events:?}");
    for i in 0..200 {
        let hp = m.view().building_hp(TOWN).unwrap().0;
        assert!(hp > 0, "step {i}: hp {hp}");
        assert!(
            (1..=4).all(|u| m.view().unit(UnitId(u)).is_some()),
            "step {i}"
        );
        assert_eq!(started(&events), 0, "step {i}: hp {hp}");
        assert_eq!(hold(&m, TOWN), Hold::Held, "step {i}");
        assert_eq!(m.view().territory().owned_tile_count(P1), p1, "step {i}");
        events = m.step(1);
    }
    let hp = m.view().building_hp(TOWN).unwrap().0;
    let events = m.step_with(vec![(0, damage(TargetId::Building(TOWN), u32::MAX))]);
    assert_eq!(started(&events), 1, "the step that takes it to 0 starts it");
    println!("the Town kept {hp} hit points for 200 steps beside the Skirmishers without a timer");
}

/// Which units defend a Town (`annexation_defenders`, every non-Civilian
/// kind): a P1 unit of each kind on [`probe_in`] in step `T0` keeps the
/// timer from starting when the rules select its kind, Mangonel and Supply
/// Wain included, and does not when they do not: the Yeoman and the
/// Scribe.
#[test]
fn annexation_any_non_civilian_defender_blocks_but_yeomen_and_scribes_do_not() {
    let rules = load_rules();
    let mut blocking = Vec::new();
    for (k, kind) in rules.units.iter().enumerate() {
        let defends = rules.annexation_defenders.iter().any(|s| s.selects(kind));
        if rules.annexation_defenders == [rules::UnitSel::Military] {
            assert_eq!(defends, kind.class != rules::units::UnitClass::Civilian);
        }
        let mut step1 = on_posts(&rules);
        step1.push((
            1,
            spawn_for(P1, UnitKindId(u16::try_from(k).unwrap()), probe_in(&rules)),
        ));
        let Annex { m, entry, .. } = annex_with(&rules, vec![], step1);
        assert_eq!(started(&entry), usize::from(!defends), "{}", kind.id);
        assert_eq!(m.view().annexation(TOWN).is_none(), defends, "{}", kind.id);
        if defends {
            blocking.push(kind.id.as_str());
        }
    }
    for civilian in ["yeoman", "scribe"] {
        assert!(!blocking.contains(&civilian), "{civilian}");
    }
    println!("defenders on the probe that block the timer: {blocking:?}");
}

/// The attackers leaving resets the timer: once no P0 Skirmisher stands
/// beside the Town (they walk north), the step that sees none emits
/// `AnnexationReset`, the Town is `Held` and P1 owns its tiles again.
#[test]
fn annexation_attackers_leaving_reset_the_timer() {
    let rules = load_rules();
    let Annex {
        mut m, p1_tiles, ..
    } = annex_setup(&rules);
    step_to(&mut m, T0 + 100);
    let attackers: Vec<UnitId> = (1..=4).map(UnitId).collect();
    let events = m.step_with(vec![(
        0,
        Command::Move {
            units: attackers,
            target: FxVec2::from_ints(21, 20),
            queue: false,
        },
    )]);
    let mut all = events;
    let mut at = None;
    while at.is_none() {
        assert!(m.sim.tick() < T0 + 200, "the attackers leave");
        let t = m.sim.tick();
        let events = m.step(1);
        if resets(&events) == 1 {
            at = Some(t);
        }
        all.extend(events);
    }
    assert_eq!(resets(&all), 1);
    assert_eq!(hold(&m, TOWN), Hold::Held);
    assert_eq!(
        m.view().territory().owned_tile_count(P1),
        u32::try_from(p1_tiles.len()).unwrap()
    );
    assert!(field_is_exact(&m));
    step_to(&mut m, T0 + total(&rules) + 10);
    assert_eq!(m.view().building(TOWN).unwrap().owner, P1);
    println!("the attackers left; reset in step {}", at.unwrap());
}

/// Mangonels and Supply Wains beside the Town do not start the timer (the
/// shipped `annexation_attackers` select infantry and cavalry only); a
/// Skirmisher joining them does.
#[test]
fn annexation_mangonels_and_supply_wains_adjacent_do_not_start_it() {
    let rules = load_rules();
    let selects = |id: &str| {
        let kind = rules.unit_kind_by_id(id).unwrap();
        rules.annexation_attackers.iter().any(|s| s.selects(kind))
    };
    assert!(!selects("mangonel") && !selects("supply_wain"));
    let step1 = vec![
        (0, spawn_for(P0, ukind(&rules, "mangonel"), POSTS[0])),
        (0, spawn_for(P0, ukind(&rules, "mangonel"), POSTS[1])),
        (0, spawn_for(P0, ukind(&rules, "supply_wain"), POSTS[2])),
    ];
    let Annex { mut m, entry, .. } = annex_with(&rules, vec![], step1);
    let mut all = entry;
    all.extend(step_to(&mut m, T0 + 60));
    assert_eq!(started(&all), 0);
    assert_eq!(hold(&m, TOWN), Hold::Held);
    let events = m.step_with(vec![(
        0,
        spawn_for(P0, ukind(&rules, "skirmisher"), POSTS[3]),
    )]);
    assert_eq!(started(&events), 1);
    println!(
        "Mangonels and a Supply Wain held the ring for 60 steps without a timer; a Skirmisher started it"
    );
}

/// A flip transfers the Town with every building and site attributed to it
/// except the kinds that `stays_on_annexation`, and idles the old owner's
/// workers and builders there. P1 has, attributed to the Town, a Croft
/// worked by a Yeoman, a Muster Hall with a rally point, a Croft site whose
/// Yeoman builder took the job just before the flip, and a Watchtower and
/// a Watchtower site, the second with its own Yeoman builder. The builders
/// start 100 steps before the flip, so both sites have progress then.
/// After the flip the Croft, the Muster Hall and the Croft site are P0's
/// (rally cleared, the site's progress exactly what it was before the
/// flip step), both Yeomen's jobs on them are gone, and
/// the Watchtower and its site are still P1's, attributed to no Town (P1
/// has none left), with their builder still at work.
#[test]
fn annexation_flip_transfers_civilian_buildings_and_sites_keeps_watchtowers_and_clears_jobs() {
    let rules = load_rules();
    let n = total(&rules);
    let k = |id: &str| bkind(&rules, id);
    let site = |kind, (x, y): (u16, u16)| Command::DebugPlaceBuilding {
        owner: P1,
        kind,
        tile: Tile::new(x, y),
        complete: false,
    };
    let yeoman = ukind(&rules, "yeoman");
    let (croft, hall, croft_site, tower, tower_site) = (
        BuildingId(2),
        BuildingId(3),
        BuildingId(4),
        BuildingId(5),
        BuildingId(6),
    );
    let (worker, builder, tower_builder) = (UnitId(1), UnitId(2), UnitId(3));
    let step0 = vec![
        place_for(P1, k("croft"), (14, 40)),
        place_for(P1, k("muster_hall"), (8, 33)),
        site(k("croft"), (17, 44)),
        place_for(P1, k("watchtower"), (6, 40)),
        site(k("watchtower"), (6, 44)),
        spawn_for(P1, yeoman, (13, 38)),
        spawn_for(P1, yeoman, (19, 47)),
        spawn_for(P1, yeoman, (5, 46)),
        Command::Gather {
            units: vec![worker],
            node: croft,
        },
        Command::SetRally {
            building: hall,
            target: FxVec2::from_ints(10, 38),
        },
    ];
    let Annex { mut m, .. } = annex_with(&rules, step0, on_posts(&rules));
    for b in [croft, hall, croft_site, tower, tower_site] {
        assert_eq!(m.view().building(b).unwrap().town, Some(TOWN), "{b:?}");
    }
    step_to(&mut m, T0 + n - 100);
    let build = |unit, kind, (x, y): (u16, u16)| Command::Build {
        unit,
        kind,
        tile: Tile::new(x, y),
    };
    let events = m.step_with(vec![
        (1, build(builder, k("croft"), (17, 44))),
        (1, build(tower_builder, k("watchtower"), (6, 44))),
    ]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    let job = |m: &M, u| m.view().unit(u).unwrap().job;
    assert!(job(&m, worker).is_some() && job(&m, builder).is_some());
    let progress = |m: &M| m.view().building(croft_site).unwrap().construction;
    step_to(&mut m, T0 + n);
    let before = progress(&m);
    assert!(
        matches!(before, sim::Construction::Site { progress } if progress > 0),
        "{before:?}"
    );
    let events = m.step(1);
    assert_eq!(annexed(&events), [(TOWN, P1, P0)]);
    let view = m.view();
    for b in [TOWN, croft, hall, croft_site] {
        assert_eq!(view.building(b).unwrap().owner, P0, "{b:?} changed hands");
    }
    for b in [tower, tower_site] {
        let b = view.building(b).unwrap();
        assert_eq!((b.owner, b.town), (P1, None), "{:?} stayed", b.id);
    }
    assert_eq!(view.building(hall).unwrap().rally, None);
    assert_eq!(progress(&m), before, "the Croft site keeps its progress");
    assert_eq!(job(&m, worker), None, "the worker left the Croft");
    assert_eq!(job(&m, builder), None, "the builder left the site");
    assert_eq!(
        job(&m, tower_builder),
        Some(sim::Job::Build { site: tower_site }),
        "the Watchtower site stays with its builder"
    );
    assert!(field_is_exact(&m));
    println!(
        "after the flip: Town, Croft, Muster Hall and Croft site ({before:?}) are P0's; Watchtower and its site stay P1's with no Town"
    );
}

/// A staying building is attributed again to the old owner's nearest
/// uncontested complete Town, and that Town is re-derived (owner decision
/// 4). P1 has a second complete Town at nw (20,70) (`BuildingId(2)`) with
/// a Croft, a Scriptorium, a Muster Hall and an Engine Yard attributed to
/// it, and has reached the Masonry Age (`town_growth_age`), so that Town
/// is one distinct kind short of tier 2. Its Watchtower at (6,40) and a
/// Watchtower site at (6,44), with a Yeoman building it, are nearer the
/// annexed Town and attributed to it. In the flip step both are attributed
/// to `BuildingId(2)`, the builder keeps its job, and the regained
/// Watchtower takes that Town to tier 2 (`TownTierChanged`, a wider
/// border), with the field equal to a full recompute.
#[test]
fn annexation_flip_attributes_staying_buildings_to_the_old_owner_s_nearest_town() {
    let rules = load_rules();
    let n = total(&rules);
    let k = |id: &str| bkind(&rules, id);
    let other = BuildingId(2);
    let (tower, tower_site) = (BuildingId(3), BuildingId(4));
    let builder = UnitId(1);
    let mut step0 = vec![
        place_for(P1, k("town"), (20, 70)),
        place_for(P1, k("watchtower"), (6, 40)),
        Command::DebugPlaceBuilding {
            owner: P1,
            kind: k("watchtower"),
            tile: Tile::new(6, 44),
            complete: false,
        },
        spawn_for(P1, ukind(&rules, "yeoman"), (5, 46)),
        place_for(P1, k("croft"), (26, 70)),
        place_for(P1, k("scriptorium"), (26, 66)),
        place_for(P1, k("muster_hall"), (14, 70)),
        place_for(P1, k("engine_yard"), (14, 65)),
    ];
    let masonry = &rules.ages[1];
    let needed = usize::try_from(masonry.techs_required).unwrap();
    step0.extend(
        ["letters_1", "trade_1", "arms_1"][..needed]
            .iter()
            .map(|id| Command::DebugResearch {
                owner: P1,
                tech: tech(&rules, id),
            }),
    );
    step0.extend(
        rules
            .amounts(&masonry.cost)
            .iter()
            .enumerate()
            .filter(|(_, c)| **c > 0)
            .map(|(r, c)| Command::DebugGrant {
                owner: P1,
                resource: u8::try_from(r).unwrap(),
                amount: u32::try_from(*c).unwrap(),
            }),
    );
    step0.push(Command::AdvanceAge { player: P1 });
    let Annex { mut m, .. } = annex_with(&rules, step0, on_posts(&rules));
    assert_eq!(m.view().age(P1), 1, "the Masonry Age");
    for b in [tower, tower_site] {
        assert_eq!(m.view().building(b).unwrap().town, Some(TOWN), "{b:?}");
    }
    for b in 5..=8 {
        assert_eq!(m.view().building(BuildingId(b)).unwrap().town, Some(other));
    }
    let tier = |m: &M| m.view().building(other).unwrap().town_state.unwrap().tier;
    assert_eq!(tier(&m), 1, "four distinct kinds");
    let radius = |m: &M| m.view().territory().sources()[&other].radius;
    let narrow = radius(&m);
    step_to(&mut m, T0 + n - 100);
    let events = m.step_with(vec![(
        1,
        Command::Build {
            unit: builder,
            kind: k("watchtower"),
            tile: Tile::new(6, 44),
        },
    )]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    step_to(&mut m, T0 + n);
    let events = m.step(1);
    assert_eq!(annexed(&events), [(TOWN, P1, P0)]);
    let view = m.view();
    for b in [tower, tower_site] {
        let b = view.building(b).unwrap();
        assert_eq!((b.owner, b.town), (P1, Some(other)), "{:?}", b.id);
    }
    assert_eq!(
        view.unit(builder).unwrap().job,
        Some(sim::Job::Build { site: tower_site }),
        "the builder stays on the Watchtower site"
    );
    assert!(
        events.contains(&SimEvent::TownTierChanged {
            town: other,
            tier: 2
        }),
        "{events:?}"
    );
    assert_eq!(tier(&m), 2);
    assert!(radius(&m) > narrow, "{} > {narrow}", radius(&m));
    assert!(field_is_exact(&m));
    println!(
        "the Watchtower and its site went to {other:?}, which reached tier 2 (radius {narrow} -> {})",
        radius(&m)
    );
}

/// A transferred building that projects its own border takes its land to
/// its new owner. The shipped Watchtower stays on annexation, so these
/// rules set its `stays_on_annexation` to false in memory, a data-only
/// edit the validator accepts. P1's complete Watchtower at (21,54),
/// attributed to the Town but beyond its border (23 tiles south of its
/// centre tile, radius 20), owns the tiles of its own kernel before the
/// flip; after it the tower is P0's, its source stamps P0's layer with the
/// same centre and radius, its tile is P0's, P1 owns no tile and the field
/// equals a full recompute.
#[test]
fn annexation_flip_moves_a_transferred_border_source_to_the_new_owner() {
    let mut rules = load_rules();
    let kind = bkind(&rules, "watchtower");
    rules.buildings[usize::from(kind.0)].stays_on_annexation = false;
    assert!(rules.buildings[usize::from(kind.0)].projects_border());
    let tower = BuildingId(2);
    let at = (21, 54);
    let tile = Tile::new(at.0, at.1);
    let Annex { mut m, .. } = annex_with(&rules, vec![place_for(P1, kind, at)], on_posts(&rules));
    let view = m.view();
    assert_eq!(view.building(tower).unwrap().town, Some(TOWN));
    let before = view.territory().sources()[&tower];
    assert_eq!(before.owner, sim::SourceOwner::Player(P1));
    assert_eq!(view.territory().owner_at(tile), Some(P1));
    let p1_during = view.territory().owned_tile_count(P1);
    let events = step_to(&mut m, T0 + total(&rules) + 1);
    assert_eq!(annexed(&events), [(TOWN, P1, P0)]);
    let view = m.view();
    assert_eq!(view.building(tower).unwrap().owner, P0);
    let field = view.territory();
    let after = field.sources()[&tower];
    assert_eq!(
        after.owner,
        sim::SourceOwner::Player(P0),
        "the tower's layer"
    );
    assert_eq!((after.centre, after.radius), (before.centre, before.radius));
    assert_eq!(field.owner_at(tile), Some(P0));
    assert_eq!(field.owned_tile_count(P1), 0, "P1 keeps no land");
    assert!(field_is_exact(&m));
    println!(
        "the tower's {p1_during} tiles were P1's during the timer; after the flip P0 owns {}, P1 {}",
        field.owned_tile_count(P0),
        field.owned_tile_count(P1)
    );
}

/// A flip refunds every queue entry of the buildings that change hands to
/// the old owner, clamped like `Cancel`, with a `TrainingCancelled` per
/// entry, and zeroes what a site paid with no refund. P1 queues two
/// Yeomen at the Town before the timer (paused since) and a Skirmisher at
/// its Muster Hall just before the flip, and places a Croft site with a
/// real `Build` (it paid Lumber; its builder is stopped). After the flip
/// P1's stockpile has grown by exactly the queued entries' cost, the
/// site's `paid` is zero and P0's stockpile is unchanged.
#[test]
fn annexation_flip_refunds_queues_to_the_old_owner_and_zeroes_site_paid() {
    let rules = load_rules();
    let n = total(&rules);
    let grant = |resource: &str, amount| Command::DebugGrant {
        owner: P1,
        resource: u8::try_from(res(&rules, resource)).unwrap(),
        amount,
    };
    let (hall, croft_site) = (BuildingId(2), BuildingId(3));
    let builder = UnitId(1);
    let train = |building, kind: &str| Command::Train {
        building,
        kind: ukind(&rules, kind),
    };
    let step0 = vec![
        grant("grain", 500),
        grant("lumber", 500),
        train(TOWN, "yeoman"),
        train(TOWN, "yeoman"),
        place_for(P1, bkind(&rules, "muster_hall"), (8, 33)),
        spawn_for(P1, ukind(&rules, "yeoman"), (16, 45)),
        Command::Build {
            unit: builder,
            kind: bkind(&rules, "croft"),
            tile: Tile::new(14, 44),
        },
        Command::Stop {
            units: vec![builder],
        },
    ];
    let Annex { mut m, .. } = annex_with(&rules, step0, on_posts(&rules));
    let site_paid = m.view().building(croft_site).unwrap().paid.clone();
    assert!(site_paid.iter().any(|v| *v > 0), "{site_paid:?}");
    step_to(&mut m, T0 + n - 20);
    let events = m.step_with(vec![(1, train(hall, "skirmisher"))]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    step_to(&mut m, T0 + n);
    let paid: Vec<i64> = [TOWN, hall]
        .iter()
        .flat_map(|b| m.view().building(*b).unwrap().queue.clone())
        .fold(vec![0; rules.resources.resources.len()], |mut acc, e| {
            for (a, p) in acc.iter_mut().zip(&e.paid) {
                *a += p;
            }
            acc
        });
    assert_eq!(m.view().building(TOWN).unwrap().queue.len(), 2);
    assert_eq!(m.view().building(hall).unwrap().queue.len(), 1);
    let (p0_before, p1_before) = (m.stock(P0), m.stock(P1));
    let events = m.step(1);
    assert_eq!(annexed(&events), [(TOWN, P1, P0)]);
    let cancelled: Vec<(BuildingId, u8)> = events
        .iter()
        .filter_map(|e| match e {
            SimEvent::TrainingCancelled { building, slot } => Some((*building, *slot)),
            _ => None,
        })
        .collect();
    assert_eq!(cancelled, [(TOWN, 0), (TOWN, 1), (hall, 0)]);
    let mut want = p1_before.clone();
    for (w, p) in want.iter_mut().zip(&paid) {
        *w += p;
    }
    for (w, res) in want.iter_mut().zip(&rules.resources.resources) {
        if let Some(cap) = res.stockpile_cap {
            *w = (*w).min(i64::from(cap));
        }
    }
    assert_eq!(m.stock(P1), want, "the queues' cost, and not the site's");
    assert_eq!(m.stock(P0), p0_before);
    let view = m.view();
    for b in [TOWN, hall] {
        assert!(view.building(b).unwrap().queue.is_empty(), "{b:?}");
    }
    let site = view.building(croft_site).unwrap();
    assert_eq!(site.owner, P0);
    assert!(site.paid.iter().all(|v| *v == 0), "{:?}", site.paid);
    println!(
        "flip refunds {paid:?} to P1 ({p1_before:?} -> {:?}); the site paid {site_paid:?}, now zero",
        m.stock(P1)
    );
}

/// A flip may take the new owner over its town limit; only new Town
/// sites are gated. P0 holds two complete Towns (the limit) before it
/// annexes P1's: it then owns three, and `Build` of another Town is
/// rejected `TownLimitReached`.
#[test]
fn annexation_flip_may_exceed_the_town_limit() {
    let rules = load_rules();
    let mut step1 = on_posts(&rules);
    for at in [(23, 63), (102, 63)] {
        step1.push((0, place_for(P0, bkind(&rules, "town"), at)));
    }
    let Annex { mut m, .. } = annex_with(&rules, vec![], step1);
    let (count, limit) = m.view().towns(P0);
    assert_eq!(count, limit, "at the limit before the flip");
    let events = step_to(&mut m, T0 + total(&rules) + 1);
    assert_eq!(annexed(&events), [(TOWN, P1, P0)]);
    assert_eq!(m.view().towns(P0), (limit + 1, limit));
    assert_eq!(
        m.view().check_build_kind(P0, bkind(&rules, "town")),
        Err(RejectReason::TownLimitReached)
    );
    println!("P0 owns {} Towns over a limit of {limit}", limit + 1);
}

/// While a Town's timer runs its queue is paused and a new building of
/// its owner is attributed to the owner's nearest other complete Town
/// (`town::attribute` skips a contested Town); once the timer resets, the
/// queue moves again and new buildings attribute to it again. P1 has a
/// second Town at nw (20,70).
#[test]
fn annexation_contested_town_queue_pauses_and_new_buildings_attribute_elsewhere() {
    let rules = load_rules();
    let other = BuildingId(2);
    let grain = u8::try_from(res(&rules, "grain")).unwrap();
    let step0 = vec![
        place_for(P1, bkind(&rules, "town"), (20, 70)),
        Command::DebugGrant {
            owner: P1,
            resource: grain,
            amount: 100,
        },
        Command::Train {
            building: TOWN,
            kind: ukind(&rules, "yeoman"),
        },
    ];
    let Annex { mut m, .. } = annex_with(&rules, step0, on_posts(&rules));
    let progress = |m: &M| m.view().building(TOWN).unwrap().queue[0].progress;
    let paused = progress(&m);
    step_to(&mut m, T0 + 40);
    assert_eq!(progress(&m), paused);
    let croft = |x, y| (1, place_for(P1, bkind(&rules, "croft"), (x, y)));
    let events = m.step_with(vec![croft(14, 40)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    let during = BuildingId(3);
    assert_eq!(m.view().building(during).unwrap().town, Some(other));
    let events = m.step_with(vec![(
        1,
        spawn_for(P1, ukind(&rules, "shieldbearer"), probe_in(&rules)),
    )]);
    assert_eq!(resets(&events), 1);
    let events = m.step_with(vec![croft(14, 36)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(m.view().building(BuildingId(4)).unwrap().town, Some(TOWN));
    assert!(progress(&m) > paused, "the queue moves after the reset");
    println!(
        "paused at {paused}; a Croft during the timer went to {other:?}, one after the reset to {TOWN:?}"
    );
}

/// The neutral land under a running timer applies no attrition: P1 has
/// Harrying I, and its land under P0's Skirmishers bites in step `T0`
/// (their counters reach 1 before step 5d turns it neutral); from step
/// `T0 + 1` through the flip and after it every counter stays 0.
#[test]
fn annexation_neutral_land_under_the_timer_applies_no_attrition() {
    let rules = load_rules();
    let step0 = vec![Command::DebugResearch {
        owner: P1,
        tech: tech(&rules, "harrying_1"),
    }];
    let Annex { mut m, .. } = annex_with(&rules, step0, on_posts(&rules));
    let ids: Vec<UnitId> = (1..=4).map(UnitId).collect();
    for &u in &ids {
        assert_eq!(exposed(&m, u), 1, "P1's land bit in step T0");
    }
    let mut landed = 0;
    while m.sim.tick() < T0 + total(&rules) + 100 {
        let events = m.step(1);
        landed += pulses(&events).len();
        for &u in &ids {
            assert_eq!(exposed(&m, u), 0, "tick {}", m.sim.tick());
        }
    }
    assert_eq!(landed, 0);
    assert_eq!(m.view().building(TOWN).unwrap().owner, P0);
}

/// The annex-hold guard rule (owner decision 6): an idle infantry or
/// cavalry guard beside an enemy Town at 0 HP takes only targets in weapon
/// range and never chases. A P1 Croft stands 3.5 tiles from the nearest
/// post, inside every guard's reach and directly chaseable, out of weapon
/// range: the guards stay on their posts for 300 steps with no target and
/// no `Hit`, and the timer runs. With the rule off for Skirmishers (the
/// control rules select only cavalry as attackers, so no timer runs either)
/// a guard takes the Croft and chases it.
#[test]
fn annexation_adjacent_guards_hold_the_ring() {
    let rules = load_rules();
    let mut off = rules.clone();
    off.annexation_attackers = vec![rules::UnitSel::Class(rules::units::UnitClass::Cavalry)];
    let croft = BuildingId(2);
    let mut seen = Vec::new();
    for (name, r) in [("rule on", &rules), ("rule off", &off)] {
        let step0 = vec![place_for(P1, bkind(r, "croft"), (27, 26))];
        let Annex { mut m, .. } = annex_with(r, step0, on_posts(r));
        let posts: Vec<Tile> = (1..=4).map(|u| unit_tile(&m, UnitId(u))).collect();
        let mut events = Vec::new();
        for _ in 0..300 {
            events.extend(m.step(1));
        }
        let moved = (1..=4)
            .filter(|&u| {
                m.view().unit(UnitId(u)).is_none_or(|x| {
                    m.view().map().tile_of(x.pos) != posts[usize::try_from(u - 1).unwrap()]
                })
            })
            .count();
        let on_croft = hits_on_building(&events, croft);
        let targets: Vec<Option<TargetId>> = (1..=4)
            .map(|u| m.view().unit(UnitId(u)).and_then(|x| x.target))
            .collect();
        if r.annexation_attackers == rules.annexation_attackers {
            assert_eq!(moved, 0, "{name}: nobody leaves the ring");
            assert_eq!(on_croft, 0, "{name}");
            assert!(targets.iter().all(Option::is_none), "{name}: {targets:?}");
            assert!(
                m.view().annexation(TOWN).is_some(),
                "{name}: the timer runs"
            );
        } else {
            assert!(
                moved > 0 && on_croft > 0,
                "{name}: {moved} moved, {on_croft} hits"
            );
            assert!(targets.contains(&Some(TargetId::Building(croft))), "{name}");
        }
        seen.push((name, moved, on_croft));
    }
    println!("guards that left their post and hits on the Croft: {seen:?}");
}

/// The annex-hold rule's other half: a holding guard keeps a target only
/// while it is in weapon range. P0's Skirmishers appear on [`POSTS`]
/// beside P1's Town at full hit points, with a P1 Yeoman 3 tiles north of
/// the middle posts, within their reach and out of weapon range: enemy
/// units outrank the Town for them, so every guard takes the Yeoman. In
/// the next step, with every guard still on its post tile, P0 takes the
/// Town to 0. With the rule on, that step leaves every guard without a
/// target, and for 200 more steps none leaves its post tile and nothing
/// hits the Yeoman while the timer runs. With the rule off for
/// Skirmishers (the Cavalry-only control rules) the guards keep the
/// Yeoman, leave the ring and hit it.
#[test]
fn annexation_adjacent_guards_drop_a_target_out_of_weapon_range() {
    let rules = load_rules();
    let mut off = rules.clone();
    off.annexation_attackers = vec![rules::UnitSel::Class(rules::units::UnitClass::Cavalry)];
    let yeoman = UnitId(5);
    let guards: Vec<UnitId> = (1..=4).map(UnitId).collect();
    let mut seen = Vec::new();
    for (name, r) in [("rule on", &rules), ("rule off", &off)] {
        let on = r.annexation_attackers == rules.annexation_attackers;
        let mut m = M::delay0(r.clone(), 8);
        let events = m.step_with(vec![(1, place_for(P1, bkind(r, "town"), FIELD_TOWN))]);
        assert!(rejections(&events).is_empty(), "{events:?}");
        let mut cmds = on_posts(r);
        cmds.push((1, spawn_for(P1, ukind(r, "yeoman"), (21, 26))));
        let events = m.step_with(cmds);
        assert!(rejections(&events).is_empty(), "{events:?}");
        let posts: Vec<Tile> = POSTS
            .iter()
            .map(|&(x, y)| Tile::new(u16::try_from(x).unwrap(), u16::try_from(y).unwrap()))
            .collect();
        let target = |m: &M, u: UnitId| m.view().unit(u).and_then(|x| x.target);
        let off_post = |m: &M| {
            guards
                .iter()
                .zip(&posts)
                .filter(|(u, p)| m.view().unit(**u).is_none_or(|_| unit_tile(m, **u) != **p))
                .count()
        };
        for &u in &guards {
            assert_eq!(
                target(&m, u),
                Some(TargetId::Unit(yeoman)),
                "{name}: {u:?} takes the Yeoman"
            );
        }
        assert_eq!(off_post(&m), 0, "{name}");
        let mut events = m.step_with(vec![(0, damage(TargetId::Building(TOWN), u32::MAX))]);
        assert!(events.contains(&SimEvent::TownAtZeroHp { town: TOWN }));
        let kept: Vec<Option<TargetId>> = guards.iter().map(|&u| target(&m, u)).collect();
        let mut left = off_post(&m);
        for _ in 0..200 {
            events.extend(m.step(1));
            left = left.max(off_post(&m));
        }
        let on_yeoman = events
            .iter()
            .filter(
                |e| matches!(e, SimEvent::Hit { target, .. } if *target == TargetId::Unit(yeoman)),
            )
            .count();
        if on {
            assert!(kept.iter().all(Option::is_none), "{name}: {kept:?}");
            assert_eq!(left, 0, "{name}: nobody leaves the ring");
            assert_eq!(on_yeoman, 0, "{name}");
            assert!(
                m.view().annexation(TOWN).is_some(),
                "{name}: the timer runs"
            );
        } else {
            assert!(
                kept.contains(&Some(TargetId::Unit(yeoman))),
                "{name}: {kept:?}"
            );
            assert!(
                left > 0 && on_yeoman > 0,
                "{name}: {left} left, {on_yeoman} hits"
            );
        }
        seen.push((name, kept, left, on_yeoman));
    }
    println!(
        "targets after the Town drops, guards that left their post, hits on the Yeoman: {seen:?}"
    );
}

fn hits_on_building(events: &[SimEvent], b: BuildingId) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, SimEvent::Hit { target: TargetId::Building(t), .. } if *t == b))
        .count()
}

/// The territory field equals a full recompute of its sources right after
/// the entry, after a reset and after the flip.
#[test]
fn annexation_field_equals_full_recompute_at_entry_reset_and_flip() {
    let rules = load_rules();
    let Annex { mut m, entry, .. } = annex_setup(&rules);
    assert_eq!(started(&entry), 1);
    assert!(field_is_exact(&m), "entry");
    let shieldbearer = UnitId(5);
    let events = m.step_with(vec![(
        1,
        spawn_for(P1, ukind(&rules, "shieldbearer"), probe_in(&rules)),
    )]);
    assert_eq!(resets(&events), 1);
    assert!(field_is_exact(&m), "reset");
    let events = m.step_with(vec![(1, move_to(shieldbearer, (21, 40)))]);
    let mut restarted = started(&events);
    let mut flipped = Vec::new();
    while flipped.is_empty() {
        let events = m.step(1);
        restarted += started(&events);
        flipped = annexed(&events);
    }
    assert_eq!(restarted, 1);
    assert!(field_is_exact(&m), "flip");
}

/// A restore in the middle of the timer continues exactly: a snapshot at
/// step `T0 + 600` restored into a fresh sim gives the same events, every
/// sub-hash and the whole hash after every step through the flip and 50
/// steps more, and the same result.
#[test]
fn annexation_restore_mid_timer_matches_uninterrupted() {
    let rules = load_rules();
    let Annex { mut m, .. } = annex_setup(&rules);
    step_to(&mut m, T0 + 600);
    let mut b = m.clone_at_tick();
    assert_eq!(m.sim.sub_hashes(), b.sim.sub_hashes());
    let mut flips = 0;
    while m.sim.tick() < T0 + total(&rules) + 50 {
        let t = m.sim.tick();
        let ea = m.step(1);
        let eb = b.step(1);
        assert_eq!(ea, eb, "events in step {t}");
        assert_eq!(m.sim.sub_hashes(), b.sim.sub_hashes(), "after step {t}");
        assert_eq!(m.sim.hash(), b.sim.hash());
        flips += annexed(&ea).len();
    }
    assert_eq!(flips, 1);
    assert!(m.sim.result().is_some());
    assert_eq!(m.sim.result(), b.sim.result());
}

/// Annexation draws nothing from the RNG: the `rng` sub-hash keeps its
/// value from the setup through the timer, the flip and the outcome.
#[test]
fn annexation_draws_no_randomness() {
    let rules = load_rules();
    let Annex { mut m, .. } = annex_setup(&rules);
    let rng = m.sim.sub_hashes().rng;
    let events = step_to(&mut m, T0 + total(&rules) + 20);
    assert_eq!(annexed(&events).len(), 1);
    assert_eq!(decided(&events).len(), 1);
    assert_eq!(m.sim.sub_hashes().rng, rng);
}

/// `SimView::annexation` says how many steps are left: after step `T0 +
/// k`, `total - ticks` more steps flip the Town, for several `k`; the last
/// of them, and no earlier one, emits `TownAnnexed`.
#[test]
fn annexation_view_remaining_ticks_agrees_with_the_flip() {
    let rules = load_rules();
    let n = total(&rules);
    for k in [0, 1, n / 2, n - 2, n - 1] {
        let Annex { mut m, .. } = annex_setup(&rules);
        step_to(&mut m, T0 + k + 1);
        let v = m.view().annexation(TOWN).unwrap();
        assert_eq!(v.ticks, k);
        let left = v.total - v.ticks;
        for i in 1..=left {
            let events = m.step(1);
            assert_eq!(
                annexed(&events).len(),
                usize::from(i == left),
                "k {k}, step {i} of {left}"
            );
        }
    }
}

/// The `capture_seat` scenario's beats, from the real sim: every P1 unit is
/// dead by tick 3; the Seat (`BuildingId(2)`) reaches 0 HP before tick 300;
/// the timer starts once and never resets; the Bowman bleeds (at least one
/// `Attrition`) while the Skirmishers and Mangonels, covered by the Wain,
/// never do; and no command is rejected.
#[test]
fn annexation_capture_seat_scenario_beats() {
    use sim::scenarios::{CAPTURE_SEAT_MOVE_TICK, CAPTURE_SEAT_RING};
    let rules = load_rules();
    let (sim, _, events) = capture_seat_to_outcome(&rules);
    let seat = BuildingId(2);
    let rejected: Vec<_> = events
        .iter()
        .filter(|(_, e)| matches!(e, SimEvent::CommandRejected { .. }))
        .collect();
    assert!(rejected.is_empty(), "{rejected:?}");
    let p1_dead: Vec<u32> = events
        .iter()
        .filter_map(|(t, e)| match e {
            SimEvent::UnitDied { owner, .. } if *owner == P1 => Some(*t),
            _ => None,
        })
        .collect();
    assert_eq!(p1_dead.len(), 5, "P1's five Yeomen");
    assert!(p1_dead.iter().all(|t| *t < 3), "{p1_dead:?}");
    let zero = events
        .iter()
        .find(|(_, e)| *e == SimEvent::TownAtZeroHp { town: seat })
        .map(|(t, _)| *t)
        .expect("the Seat reaches 0");
    assert!(zero < CAPTURE_SEAT_MOVE_TICK, "{zero}");
    let all: Vec<SimEvent> = events.iter().map(|(_, e)| e.clone()).collect();
    assert_eq!(started(&all), 1);
    assert_eq!(resets(&all), 0);
    let entry = events
        .iter()
        .find(|(_, e)| matches!(e, SimEvent::AnnexationStarted { .. }))
        .unwrap()
        .0;
    let bled = |u: u32| pulses(&all).iter().filter(|p| p.0 == UnitId(u)).count();
    assert!(bled(24) > 0, "the Bowman bleeds");
    for u in 11..=22 {
        assert_eq!(bled(u), 0, "{u}");
    }
    assert_eq!(CAPTURE_SEAT_RING.len(), 8);
    println!(
        "capture_seat: P1's Yeomen dead at {p1_dead:?}, Seat at 0 HP in step {zero}, timer from step {entry}, decided at tick {} ({:?}), the Bowman took {} pulses",
        sim.tick(),
        sim.result().unwrap().reason,
        bled(24)
    );
}

/// `capture_seat` survives data whose Town has no more hit points than
/// `CAPTURE_SEAT_SEAT_HP` (300), a data-only edit the validator accepts:
/// with the Town's `hp` at 250 and at 300 in memory the scenario builds,
/// its tick-0 `DebugDamage` on P1's Seat is 0 (it saturates instead of
/// underflowing), and the Mangonels' first volley still takes the Seat to
/// 0 before `CAPTURE_SEAT_MOVE_TICK`.
#[test]
fn annexation_capture_seat_builds_when_the_town_has_at_most_the_seat_hp() {
    use sim::scenarios::{CAPTURE_SEAT_MOVE_TICK, CAPTURE_SEAT_SEAT_HP};
    let seat = BuildingId(2);
    for hp in [CAPTURE_SEAT_SEAT_HP - 50, CAPTURE_SEAT_SEAT_HP] {
        let mut rules = load_rules();
        let town = bkind(&rules, "town");
        rules.buildings[usize::from(town.0)].hp = hp;
        let (setup, stream, _) = sim::scenarios::by_name("capture_seat", &rules).unwrap();
        let on_seat: Vec<u32> = sim::scenarios::commands_at(&stream, 0)
            .iter()
            .filter_map(|c| match c.cmd {
                Command::DebugDamage {
                    target: TargetId::Building(b),
                    amount,
                } if b == seat => Some(amount),
                _ => None,
            })
            .collect();
        assert_eq!(on_seat, [0], "Town hp {hp}");
        let mut sim = Sim::new(setup, rules, Box::new(NoAi));
        let mut zero = None;
        while zero.is_none() && sim.tick() < CAPTURE_SEAT_MOVE_TICK {
            let t = sim.tick();
            sim.step(sim::scenarios::commands_at(&stream, t));
            let events = sim.drain_events();
            assert!(rejections(&events).is_empty(), "{events:?}");
            if events.contains(&SimEvent::TownAtZeroHp { town: seat }) {
                zero = Some(t);
            }
        }
        let zero = zero.expect("the Seat reaches 0 before the ring walk");
        println!("Town hp {hp}: no tick-0 damage on the Seat, at 0 HP in step {zero}");
    }
}

/// The shipped rules with an in-memory `match_cap_ds` of 50 (100 ticks),
/// so the outcome tests reach the cap quickly (`Rules::match_cap_ticks()`
/// is read, not hard-coded).
fn short_cap_rules() -> Rules {
    let mut r = load_rules();
    r.match_cap_ds = 50;
    r
}

/// A surrender is decisive for the opponent: P1's `Surrender` in step 0
/// (delay 0) decides `Victory { P0, decisive: true }`, reason `Surrender`,
/// at tick 1, with one `MatchDecided`.
#[test]
fn annexation_outcome_surrender_is_decisive_for_the_opponent() {
    let mut m = M::delay0(load_rules(), 9);
    let events = m.step_with(vec![(1, Command::Surrender)]);
    let want = MatchResult {
        outcome: Outcome::Victory {
            player: P0,
            decisive: true,
        },
        reason: OutcomeReason::Surrender,
        tick: 1,
    };
    assert_eq!(decided(&events), [want]);
    assert_eq!(m.sim.result(), Some(want));
    assert_eq!(m.view().result(), Some(want));
    assert_eq!(m.view().outcome(), Some(want.outcome));
}

/// Both players surrendering in the same step is a `Draw`, reason
/// `Surrender` (P0's applies first, then P1's: nobody is left).
#[test]
fn annexation_outcome_double_surrender_is_a_draw() {
    let mut m = M::delay0(load_rules(), 9);
    let events = m.step_with(vec![(0, Command::Surrender), (1, Command::Surrender)]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    let want = MatchResult {
        outcome: Outcome::Draw,
        reason: OutcomeReason::Surrender,
        tick: 1,
    };
    assert_eq!(decided(&events), [want]);
}

/// Losing the last complete Town is decisive, checked at the flip, and a
/// Town site does not count as a Town (owner decision 7): P1 with a Town
/// site besides the annexed Town loses (`AllTownsLost` in the flip's
/// step), while P1 with a second complete Town plays on.
#[test]
fn annexation_outcome_all_towns_lost_is_decisive() {
    let rules = load_rules();
    let town = bkind(&rules, "town");
    let cases = [("a Town site", false), ("a second complete Town", true)];
    for (name, complete) in cases {
        let step0 = vec![Command::DebugPlaceBuilding {
            owner: P1,
            kind: town,
            tile: Tile::new(20, 70),
            complete,
        }];
        let Annex { mut m, .. } = annex_with(&rules, step0, on_posts(&rules));
        let events = step_to(&mut m, T0 + total(&rules) + 1);
        assert_eq!(annexed(&events), [(TOWN, P1, P0)], "{name}");
        if complete {
            assert_eq!(m.sim.result(), None, "{name}: P1 plays on");
        } else {
            assert_eq!(
                m.sim.result(),
                Some(MatchResult {
                    outcome: Outcome::Victory {
                        player: P0,
                        decisive: true
                    },
                    reason: OutcomeReason::AllTownsLost,
                    tick: T0 + total(&rules) + 1,
                }),
                "{name}"
            );
        }
    }
}

/// Each player's four Skirmishers on the north row of the other player's
/// Town: P0's on (101..=104, 62) beside the east start (nw (102,63)), P1's
/// on (22..=25, 62) beside the west start (nw (23,63)).
fn mutual_rings(rules: &Rules) -> Vec<(u8, Command)> {
    let skirmisher = ukind(rules, "skirmisher");
    let p0 = (101..=104).map(|x| (0, spawn_for(P0, skirmisher, (x, 62))));
    let p1 = (22..=25).map(|x| (1, spawn_for(P1, skirmisher, (x, 62))));
    p0.chain(p1).collect()
}

/// Step `m` until a step flips a Town, at most `limit` steps; that step's
/// tick (the step at `sim.tick() == t`) and its events.
fn step_to_flip(m: &mut M, limit: u32) -> (u32, Vec<SimEvent>) {
    loop {
        let t = m.sim.tick();
        assert!(t <= limit, "a flip by step {limit}");
        let events = m.step(1);
        if !annexed(&events).is_empty() {
            return (t, events);
        }
    }
}

/// Seats captured by both players in the same step are a `Draw`, reason
/// `SeatCaptured` (step 6e's symmetric rule). From the standard start with
/// delay 0, P0 removes all ten Yeomen and takes both Seats to 0 in step 0,
/// and each player's Skirmishers stand beside the other's Seat
/// ([`mutual_rings`]). Both timers start in step 0, both Seats flip in step
/// [`total`], and that step decides `Draw`, `SeatCaptured`, at tick
/// `total + 1`.
#[test]
fn annexation_outcome_same_step_mutual_seat_capture_is_a_draw() {
    let rules = load_rules();
    let n = total(&rules);
    let mut setup = MatchSetup::scenario(&rules, 9);
    setup.start = sim::StartKind::Standard;
    setup.cmd_delay = 0;
    let mut m = M {
        sim: Sim::new(setup, rules.clone(), Box::new(NoAi)),
        seq: [0; 2],
    };
    let (west, east) = (BuildingId(1), BuildingId(2));
    let mut cmds: Vec<(u8, Command)> = (1..=10)
        .map(|u| (0, damage(TargetId::Unit(UnitId(u)), u32::MAX)))
        .collect();
    for seat in [west, east] {
        cmds.push((0, damage(TargetId::Building(seat), u32::MAX)));
    }
    cmds.extend(mutual_rings(&rules));
    let events = m.step_with(cmds);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(started(&events), 2, "{events:?}");
    let (t, events) = step_to_flip(&mut m, n);
    assert_eq!(t, n, "both flip 1200 steps after step 0");
    assert_eq!(annexed(&events), [(west, P0, P1), (east, P1, P0)]);
    for seat in [west, east] {
        assert!(m.view().building(seat).unwrap().town_state.unwrap().seat);
    }
    let want = MatchResult {
        outcome: Outcome::Draw,
        reason: OutcomeReason::SeatCaptured,
        tick: n + 1,
    };
    assert_eq!(decided(&events), [want]);
    assert_eq!(m.sim.result(), Some(want));
    println!("both Seats flipped in step {t}: {want:?}");
}

/// A same-step mutual capture of two Towns that are not Seats decides
/// nothing: the all-Towns-lost check reads the owners after every flip of
/// the step, and each player then owns the other's Town. Empty start,
/// delay 0: complete Towns at the two starts in step 0, both taken to 0
/// in step 1 with [`mutual_rings`]; both flip in step `1 + total`, the
/// owners swap and the result stays `None`.
#[test]
fn annexation_outcome_same_step_mutual_town_capture_plays_on() {
    let rules = load_rules();
    let n = total(&rules);
    let mut m = M::delay0(rules.clone(), 9);
    let (west, east) = (BuildingId(1), BuildingId(2));
    let town = bkind(&rules, "town");
    let events = m.step_with(vec![
        (0, place_for(P0, town, (23, 63))),
        (1, place_for(P1, town, (102, 63))),
    ]);
    assert!(rejections(&events).is_empty(), "{events:?}");
    let mut cmds: Vec<(u8, Command)> = [west, east]
        .iter()
        .map(|&b| (0, damage(TargetId::Building(b), u32::MAX)))
        .collect();
    cmds.extend(mutual_rings(&rules));
    let events = m.step_with(cmds);
    assert!(rejections(&events).is_empty(), "{events:?}");
    assert_eq!(started(&events), 2, "{events:?}");
    let (t, events) = step_to_flip(&mut m, 1 + n);
    assert_eq!(t, 1 + n);
    assert_eq!(annexed(&events), [(west, P0, P1), (east, P1, P0)]);
    assert!(decided(&events).is_empty(), "{events:?}");
    assert_eq!(m.sim.result(), None, "each player owns a Town");
    assert_eq!(m.view().towns(P0).0, 1);
    assert_eq!(m.view().towns(P1).0, 1);
}

/// An empty start never ends before the cap: with nothing owned and nobody
/// surrendering, the outcome is `None` after every step up to the cap and
/// decided in the step that reaches it, a `Draw` (0 tiles each), reason
/// `Territory`.
#[test]
fn annexation_outcome_empty_start_never_ends_before_the_cap() {
    let rules = short_cap_rules();
    let cap = rules.match_cap_ticks();
    let mut m = M::delay0(rules, 9);
    while m.sim.tick() < cap - 1 {
        m.step(1);
        assert_eq!(m.sim.outcome(), None, "tick {}", m.sim.tick());
    }
    let events = m.step(1);
    let want = MatchResult {
        outcome: Outcome::Draw,
        reason: OutcomeReason::Territory,
        tick: cap,
    };
    assert_eq!(decided(&events), [want]);
}

/// The territory tiebreak at the cap (with [`short_cap_rules`]).
/// `towns_and_towers` are complete placements in step 0.
fn territory_result(placements: &[(u8, &str, (u16, u16))]) -> (MatchResult, u32, u32) {
    let rules = short_cap_rules();
    let cap = rules.match_cap_ticks();
    let cmds: Vec<(u8, Command)> = placements
        .iter()
        .map(|&(p, kind, at)| (p, place_for(PlayerId(p), bkind(&rules, kind), at)))
        .collect();
    let mut m = M::delay0(rules, 9);
    let events = m.step_with(cmds);
    assert!(rejections(&events).is_empty(), "{events:?}");
    step_to(&mut m, cap);
    let f = m.view().territory();
    let (p0, p1) = (f.owned_tile_count(P0), f.owned_tile_count(P1));
    (m.sim.result().expect("decided at the cap"), p0, p1)
}

/// An exact territory tie at the cap is a `Draw` (owner decision 1): the
/// mirrored Towns at the two starts own 1245 tiles each.
#[test]
fn annexation_outcome_exact_territory_tie_is_a_draw() {
    let (r, p0, p1) = territory_result(&[(0, "town", (23, 63)), (1, "town", (102, 63))]);
    assert_eq!(p0, p1);
    assert_eq!(
        (r.outcome, r.reason),
        (Outcome::Draw, OutcomeReason::Territory)
    );
    println!("tie {p0} : {p1} -> {:?}", r.outcome);
}

/// The tiebreak goes to whichever player owns more: a Watchtower at
/// (43,64) gives P0 three more tiles, the mirrored one at (84,64) gives P1
/// three more; each wins `Victory { decisive: false }`, reason
/// `Territory`.
#[test]
fn annexation_outcome_tiebreak_goes_to_either_player() {
    for (p, tower) in [(0u8, (43, 64)), (1, (84, 64))] {
        let (r, p0, p1) = territory_result(&[
            (0, "town", (23, 63)),
            (1, "town", (102, 63)),
            (p, "watchtower", tower),
        ]);
        let winner = PlayerId(p);
        assert!(if p == 0 { p0 > p1 } else { p1 > p0 }, "{p0} {p1}");
        assert_eq!(
            (r.outcome, r.reason),
            (
                Outcome::Victory {
                    player: winner,
                    decisive: false
                },
                OutcomeReason::Territory
            )
        );
        println!("P0 {p0}, P1 {p1} -> {:?}", r.outcome);
    }
}

/// A surrender beats a Seat capture in the same step: `capture_seat` with
/// P0's `Surrender` applying in the very step its Skirmishers flip P1's
/// Seat decides `Victory { P1 }`, reason `Surrender`, although the Seat
/// still changes hands in that step.
#[test]
fn annexation_outcome_surrender_beats_a_same_step_seat_capture() {
    let rules = load_rules();
    let (sim, _, _) = capture_seat_to_outcome(&rules);
    let decision = sim.result().unwrap().tick;
    let (setup, stream, _) = sim::scenarios::by_name("capture_seat", &rules).unwrap();
    let delay = setup.cmd_delay;
    let seq = stream
        .iter()
        .flat_map(|(_, c)| c)
        .filter(|c| c.player == P0)
        .map(|c| c.seq + 1)
        .max()
        .unwrap();
    let mut sim = Sim::new(setup, rules, Box::new(NoAi));
    let issue = decision - 1 - delay;
    let surrender = PlayerCommand::new(P0, seq, Command::Surrender);
    let mut last = Vec::new();
    while sim.tick() < decision {
        let t = sim.tick();
        let mut cmds = sim::scenarios::commands_at(&stream, t).to_vec();
        if t == issue {
            cmds.push(surrender.clone());
        }
        sim.step(&cmds);
        last = sim.drain_events();
    }
    assert_eq!(annexed(&last), [(BuildingId(2), P1, P0)], "the Seat flips");
    let want = MatchResult {
        outcome: Outcome::Victory {
            player: P1,
            decisive: true,
        },
        reason: OutcomeReason::Surrender,
        tick: decision,
    };
    assert_eq!(decided(&last), [want]);
}

/// The outcome is latched: P1 surrenders during the timer (`Victory { P0
/// }`, `Surrender`); the later flip of its last Town decides nothing
/// again, and neither does P0's own surrender after it.
#[test]
fn annexation_outcome_is_latched() {
    let rules = load_rules();
    let Annex { mut m, .. } = annex_setup(&rules);
    let events = m.step_with(vec![(1, Command::Surrender)]);
    let result = m.sim.result().unwrap();
    assert_eq!(decided(&events), [result]);
    assert_eq!(result.reason, OutcomeReason::Surrender);
    let mut events = step_to(&mut m, T0 + total(&rules) + 1);
    assert_eq!(annexed(&events), [(TOWN, P1, P0)], "annexation goes on");
    events.extend(m.step_with(vec![(0, Command::Surrender)]));
    events.extend(m.step(10));
    assert!(decided(&events).is_empty());
    assert_eq!(m.sim.result(), Some(result));
}

/// The late-game Yield Cap decay (`ai.md`, M6's pressure knob) is off in
/// the shipped data (`yield_cap_decay_step: 0`): even with the decay
/// starting at tick 0 the cap stays `yield_cap_base`. With an in-memory
/// step of 10 from 5 ds (10 ticks) every 5 ds, the cap a player's income
/// reads falls by 10 at tick 10, by 20 at tick 20, and so on, floored at
/// 0. Deliberately not prefixed: it is neither attrition nor annexation.
#[test]
fn late_game_yield_cap_decay_is_off_in_the_shipped_data_and_lowers_the_cap_when_set() {
    let shipped = load_rules();
    assert_eq!(shipped.yield_cap_decay_step, 0, "off until an M6 iteration");
    let grain = res(&shipped, "grain");
    let cap = |m: &M| m.view().income(P0)[grain].cap.unwrap();
    let base = shipped.yield_cap_base;
    let mut from_zero = shipped.clone();
    from_zero.yield_cap_decay_after_ds = 0;
    let mut m = M::delay0(from_zero, 9);
    for _ in 0..30 {
        assert_eq!(cap(&m), base);
        m.step(1);
    }
    let mut on = shipped.clone();
    on.yield_cap_decay_after_ds = 5;
    on.yield_cap_decay_every_ds = 5;
    on.yield_cap_decay_step = 10;
    let (after, every) = on.yield_cap_decay_ticks();
    assert_eq!((after, every), (10, 10));
    let mut m = M::delay0(on, 9);
    let mut seen = Vec::new();
    while m.sim.tick() <= 20 * every {
        let t = m.sim.tick();
        let want = if t < after {
            base
        } else {
            base.saturating_sub(10 * (1 + (t - after) / every))
        };
        assert_eq!(cap(&m), want, "tick {t}");
        if t.is_multiple_of(every) {
            seen.push((t, cap(&m)));
        }
        m.step(1);
    }
    assert_eq!(cap(&m), 0, "floored at 0");
    println!("decay from tick {after} every {every}: {seen:?}");
}

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

/// The M5a capture golden: `capture_seat.eonreplay` verifies to the hash
/// in `capture_seat.hash`, the state at the tick the match is decided.
#[test]
fn golden_capture_seat() {
    golden("capture_seat");
}

/// The M5a 45-minute golden: 54000 ticks, verified by `sim-cli verify
/// --release` in CI (rule 18: goldens over 5000 ticks are ignored
/// locally). The acceptance test
/// `annexation_54000_tick_scripted_stalemate_is_a_tiebreak_victory_for_more_territory`
/// runs the same stream and is not ignored.
#[test]
#[ignore = "long golden (54000 ticks); verified by sim-cli verify --release in CI"]
fn golden_stalemate_45min() {
    golden("stalemate_45min");
}

// ---------------------------------------------------------------------------
// Seeded random streams over attrition and annexation (`m5a_*`).
// ---------------------------------------------------------------------------

/// The rules of the M5a annexation streams: [`stream_rules`] (attrition
/// every 8 ticks at Harrying I) with an in-memory `annexation_ds` of 50
/// (100 ticks), so a short stream sees Towns change hands.
fn annex_stream_rules() -> &'static Rules {
    static RULES: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    RULES.get_or_init(|| {
        let mut r = stream_rules().clone();
        r.annexation_ds = 50;
        assert_eq!(r.annexation_ticks(), 100);
        r
    })
}

/// A seeded random stream over both players for the delay-0 sim with
/// [`annex_stream_rules`], extending the attrition stream with Towns at 0
/// HP, defenders, attacks and a surrender. Step 0: P0's complete Town at
/// (59,30) (`BuildingId(1)`), P1's at [`FIELD_TOWN`] (`BuildingId(2)`),
/// Harrying for P1 (I, and II on odd seeds) and for P0 on seeds divisible
/// by 3; P0's four Skirmishers on [`POSTS`], beside P1's Town, and a Bowman,
/// a Yeoman, a Scribe and a Supply Wain each side away from the Towns.
/// Step 1: P0 takes P1's Town to 0 HP (except on seeds `3 mod 4`) and P1
/// takes P0's to 0 on seeds divisible by 3. Then each tick may: move a few
/// units of one side to a random point of the strip between the Towns, to
/// a tile of the enemy Town's ring or near their own Town; stop one; damage
/// a random unit id or a Town; order a unit to attack a random enemy unit
/// id; research Harrying II; spawn a unit at a random point or near the
/// spawner's own Town (a defender); and late in the stream, rarely,
/// surrender (that player's commands then stop). Many commands are
/// rejected, as intended.
fn random_m5a_commands(seed: u64, ticks: u32) -> Vec<Vec<PlayerCommand>> {
    let rules = annex_stream_rules();
    let k = |id: &str| ukind(rules, id);
    let mut rng = rand_pcg::Pcg32::seed_from_u64(seed ^ 0x5a5a);
    let mut seq = [0u32; 2];
    let mut ids: [Vec<u32>; 2] = [Vec::new(), Vec::new()];
    let mut next_id = 1u32;
    let mut gone = [false; 2];
    let towns = [(59i32, 30i32), (20, 30)];
    let ring = |town: (i32, i32), i: u32| -> (i32, i32) {
        // The 16 ring tiles of a 3x3 footprint, clockwise from the north-west.
        let i = i32::try_from(i % 16).unwrap();
        let (x0, y0) = (town.0 - 1, town.1 - 1);
        match i {
            0..=4 => (x0 + i, y0),
            5..=8 => (x0 + 4, y0 + i - 4),
            9..=12 => (x0 + 4 - (i - 8), y0 + 4),
            _ => (x0, y0 + 4 - (i - 12)),
        }
    };
    let mut out = Vec::with_capacity(usize::try_from(ticks).unwrap());
    for t in 0..ticks {
        let mut cmds: Vec<PlayerCommand> = Vec::new();
        let mut push = |p: usize, c: Command| {
            cmds.push(PlayerCommand::new(
                PlayerId(u8::try_from(p).unwrap()),
                seq[p],
                c,
            ));
            seq[p] += 1;
        };
        match t {
            0 => {
                push(0, place_for(P0, bkind(rules, "town"), (59, 30)));
                if seed.is_multiple_of(3) {
                    push(0, harrying_for(rules, P0, 1)[0].1.clone());
                }
                for at in POSTS {
                    push(0, spawn_for(P0, k("skirmisher"), at));
                    ids[0].push(next_id);
                    next_id += 1;
                }
                for (p, at) in [(0usize, (35, 40)), (1, (47, 31))] {
                    let owner = PlayerId(u8::try_from(p).unwrap());
                    let kinds = ["bowman", "yeoman", "scribe", "supply_wain", "skirmisher"];
                    for (i, kind) in kinds.iter().enumerate() {
                        let dy = 2 * i32::try_from(i).unwrap() - 4;
                        push(p, spawn_for(owner, k(kind), (at.0, at.1 + dy)));
                        ids[p].push(next_id);
                        next_id += 1;
                    }
                }
                push(1, place_for(P1, bkind(rules, "town"), FIELD_TOWN));
                for (_, c) in harrying_for(rules, P1, 1 + usize::from(seed % 2 == 1)) {
                    push(1, c);
                }
            }
            1 => {
                if seed % 4 != 3 {
                    push(0, damage(TargetId::Building(BuildingId(2)), u32::MAX));
                }
                if seed.is_multiple_of(3) {
                    push(1, damage(TargetId::Building(BuildingId(1)), u32::MAX));
                }
            }
            _ => {
                let roll = rng.next_u32() % 100;
                let p = usize::try_from(rng.next_u32() % 2).unwrap();
                if gone[p] {
                    out.push(cmds);
                    continue;
                }
                let owner = PlayerId(u8::try_from(p).unwrap());
                let own = towns[p];
                let enemy = towns[1 - p];
                let pick = |rng: &mut rand_pcg::Pcg32, v: &Vec<u32>| {
                    UnitId(v[usize::try_from(rng.next_u32()).unwrap() % v.len()])
                };
                let near_own = |rng: &mut rand_pcg::Pcg32| {
                    (
                        own.0 + 1 + i32::try_from(rng.next_u32() % 9).unwrap() - 4,
                        own.1 + 5 + i32::try_from(rng.next_u32() % 3).unwrap(),
                    )
                };
                if roll < 10 {
                    let n = 1 + rng.next_u32() % 3;
                    let units = (0..n).map(|_| pick(&mut rng, &ids[p])).collect();
                    let (x, y) = match rng.next_u32() % 10 {
                        0..=3 => (
                            10 + i32::try_from(rng.next_u32() % 60).unwrap(),
                            14 + i32::try_from(rng.next_u32() % 36).unwrap(),
                        ),
                        4..=6 => ring(enemy, rng.next_u32()),
                        _ => near_own(&mut rng),
                    };
                    push(
                        p,
                        Command::Move {
                            units,
                            target: FxVec2::from_ints(x, y),
                            queue: false,
                        },
                    );
                } else if roll < 13 {
                    let u = pick(&mut rng, &ids[p]);
                    push(p, Command::Stop { units: vec![u] });
                } else if roll < 18 {
                    let target = if rng.next_u32().is_multiple_of(4) {
                        TargetId::Building(BuildingId(1 + rng.next_u32() % 2))
                    } else {
                        TargetId::Unit(UnitId(rng.next_u32() % (next_id + 2)))
                    };
                    let amount = if rng.next_u32().is_multiple_of(3) {
                        u32::MAX
                    } else {
                        1 + rng.next_u32() % 30
                    };
                    push(p, damage(target, amount));
                } else if roll < 20 {
                    let u = pick(&mut rng, &ids[p]);
                    let target = TargetId::Unit(pick(&mut rng, &ids[1 - p]));
                    push(
                        p,
                        Command::Attack {
                            units: vec![u],
                            target,
                        },
                    );
                } else if roll < 21 {
                    push(p, harrying_for(rules, owner, 2)[1].1.clone());
                } else if roll < 27 {
                    let kinds = [
                        "skirmisher",
                        "bowman",
                        "shieldbearer",
                        "yeoman",
                        "supply_wain",
                    ];
                    let kind = kinds[usize::try_from(rng.next_u32()).unwrap() % kinds.len()];
                    let at = if rng.next_u32().is_multiple_of(2) {
                        near_own(&mut rng)
                    } else {
                        (
                            10 + i32::try_from(rng.next_u32() % 60).unwrap(),
                            14 + i32::try_from(rng.next_u32() % 36).unwrap(),
                        )
                    };
                    push(p, spawn_for(owner, k(kind), at));
                    ids[p].push(next_id);
                    next_id += 1;
                } else if roll == 99 && t * 4 > ticks * 3 {
                    push(p, Command::Surrender);
                    gone[p] = true;
                }
            }
        }
        out.push(cmds);
    }
    out
}

/// The delay-0 sim of the annexation streams.
fn annex_stream_sim(seed: u64) -> Sim {
    let mut setup = MatchSetup::scenario(annex_stream_rules(), seed);
    setup.cmd_delay = 0;
    Sim::new(setup, annex_stream_rules().clone(), Box::new(NoAi))
}

/// Run `stream` on [`annex_stream_sim`], restoring into a fresh sim after
/// step `restore_after`: the hash after every step, and the events.
fn run_m5a_stream(
    seed: u64,
    stream: &[Vec<PlayerCommand>],
    restore_after: Option<usize>,
) -> (Vec<u64>, Vec<SimEvent>, Option<MatchResult>) {
    let mut sim = annex_stream_sim(seed);
    let mut hashes = Vec::with_capacity(stream.len());
    let mut events = Vec::new();
    for (i, cmds) in stream.iter().enumerate() {
        sim.step(cmds);
        hashes.push(sim.hash());
        events.extend(sim.drain_events());
        if restore_after == Some(i) {
            let mut restored = annex_stream_sim(seed);
            restored.restore(&sim.snapshot()).expect("restores");
            sim = restored;
        }
    }
    let result = sim.result();
    (hashes, events, result)
}

/// The annexation stream really exercises the M5a sim: over 8 seeds of 240
/// ticks it lands attrition pulses, starts, resets and completes
/// annexations, every timer starts at a Town at 0 HP, and every territory
/// change it makes leaves the field equal to a full recompute at the end.
#[test]
fn m5a_random_stream_exercises_attrition_and_annexation() {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    let mut results = Vec::new();
    for seed in 0..8 {
        let stream = random_m5a_commands(seed, 240);
        let mut sim = annex_stream_sim(seed);
        for cmds in &stream {
            sim.step(cmds);
            for e in sim.drain_events() {
                if let SimEvent::AnnexationStarted { town, .. } = e {
                    let hp = sim.view().building_hp(town).unwrap().0;
                    assert_eq!(hp, 0, "seed {seed}: {town:?} started at {hp} HP");
                }
                let key = match e {
                    SimEvent::Attrition { .. } => "Attrition",
                    SimEvent::AnnexationStarted { .. } => "AnnexationStarted",
                    SimEvent::AnnexationReset { .. } => "AnnexationReset",
                    SimEvent::TownAnnexed { .. } => "TownAnnexed",
                    SimEvent::MatchDecided { .. } => "MatchDecided",
                    SimEvent::TownAtZeroHp { .. } => "TownAtZeroHp",
                    _ => continue,
                };
                *seen.entry(key).or_default() += 1;
            }
        }
        let field = sim.view().territory();
        assert_eq!(*field, field.recomputed(), "seed {seed}");
        results.push(sim.result().map(|r| (r.reason, r.tick)));
    }
    eprintln!("m5a annexation stream, 8 seeds: {seen:?}; results {results:?}");
    for key in [
        "Attrition",
        "AnnexationStarted",
        "AnnexationReset",
        "TownAnnexed",
    ] {
        assert!(seen.get(key).is_some_and(|n| *n > 0), "no {key}: {seen:?}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: proptest_cases(),
        .. ProptestConfig::default()
    })]

    /// A random annexation stream gives the same hash after every step, and
    /// the same result, on a second run that restores from a snapshot at a
    /// random step.
    #[test]
    fn m5a_random_streams_are_deterministic_with_restore(
        seed in any::<u64>(),
        ticks in 60u32..240,
        cut in 0u32..1000,
    ) {
        let stream = random_m5a_commands(seed, ticks);
        let (a, _, ra) = run_m5a_stream(seed, &stream, None);
        let at = usize::try_from(cut % ticks).unwrap();
        let (b, _, rb) = run_m5a_stream(seed, &stream, Some(at));
        prop_assert_eq!(a, b, "restored after step {}", at);
        prop_assert_eq!(ra, rb);
    }
}
