//! The engine-free bot-match harness behind `sim-cli play-bots` and the
//! bot tests (M5b): [`BotMatch`] steps one match with no human input,
//! [`Observer`] collects the CSV facts from the hashed state and the
//! drained events after every step, and [`GameRow`] is one CSV row. No
//! I/O and no clock here: `sim-cli` times the steps and prints the rows.
//!
//! Player A is `PlayerId(0)` and player B `PlayerId(1)`.

use crate::standard::{Facts, build_order};
use crate::{BotError, bot_stats, controller_for, is_standard_bot};
use rules::{BuildOrder, Rules, UnitClass};
use sim::{
    AiController, BuildingKindId, Construction, FxVec2, Intent, MatchResult, MatchSetup,
    MatchStats, Outcome, OutcomeReason, PlayerId, Sim, SimEvent, SimView, TargetId, Unit,
};

/// The CSV header: the seven columns of docs/design/ai.md first, in their
/// order, then the M5b diagnostics (B's `second_town_tick`, `tower_tick`
/// and `bo_done_tick` last among them, so the side swap reports what A's
/// columns report), then the M6 columns ([`M6_COLUMNS`]).
pub const CSV_HEADER: &str = "seed,winner,outcome,tick,final_hash,age_reached_tick,first_attack_tick,reason,first_attack_units,first_attack_wains,second_town_tick,tower_tick,bo_done_tick_a,age_reached_tick_b,first_attack_tick_b,first_attack_units_b,first_attack_wains_b,territory_a,territory_b,units_lost_a,units_lost_b,min_enemy_town_hp_permille_a,min_enemy_town_hp_permille_b,rejected_a,rejected_b,second_town_tick_b,tower_tick_b,bo_done_tick_b,bot_a,personality_a,bot_b,personality_b,waves_a,waves_b,raids_a,raids_b,retreats_a,retreats_b,wiped_a,wiped_b,towns_taken_a,towns_taken_b,bonus_a,bonus_b,income_a,income_b,cap_s_a,cap_s_b,first_contact_tick,max_quiet_ticks,first_on_land_tick_a,first_on_land_tick_b,min_own_town_hp_permille_a,min_own_town_hp_permille_b";

/// The columns M5b printed, which stay first and in order.
pub const M5B_COLUMNS: usize = 28;
/// The columns M6 appends (docs/design/ai.md, the CSV table).
pub const M6_COLUMNS: usize = 26;

/// The age whose arrival `age_reached_tick` records (by id).
pub const AGE_REACHED: &str = "masonry";

const A: PlayerId = PlayerId(0);
const B: PlayerId = PlayerId(1);

/// Who won a game, as the `winner` column prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winner {
    /// Player A (`PlayerId(0)`).
    A,
    /// Player B (`PlayerId(1)`).
    B,
    /// The match ended in a Draw (an exact territory tie at the cap).
    Draw,
    /// `--max-ticks` came before an outcome.
    None,
    /// The game panicked.
    Panic,
}

impl Winner {
    /// The `winner` column.
    pub fn as_str(self) -> &'static str {
        match self {
            Winner::A => "a",
            Winner::B => "b",
            Winner::Draw => "draw",
            Winner::None => "none",
            Winner::Panic => "panic",
        }
    }

    /// Whether the row names a winner (`a` or `b`).
    pub fn declared(self) -> bool {
        matches!(self, Winner::A | Winner::B)
    }
}

/// What the observer records for one side.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Side {
    /// First tick at which the side's age reached [`AGE_REACHED`].
    pub age_reached_tick: Option<u32>,
    /// First tick at which one of the side's units attack-moves to a goal
    /// on the other side's land (docs/design/ai.md's "first `AttackMove`
    /// into enemy borders").
    pub first_attack_tick: Option<u32>,
    /// At that tick: the side's non-civilian units committed to the other
    /// side's land (order goal, post goal, attack target or position on
    /// it), Supply Wains included.
    pub first_attack_units: Option<u32>,
    /// At that tick: the side's committed units of kinds that supply.
    pub first_attack_wains: Option<u32>,
    /// First tick at which the side owns a second complete Town.
    pub second_town_tick: Option<u32>,
    /// First tick at which the side owns a complete Watchtower (a
    /// non-Town kind with `border_radius_tiles`).
    pub tower_tick: Option<u32>,
    /// First tick at which every non-optional step of the side's build
    /// order is done (a `standard` seat only).
    pub bo_done_tick: Option<u32>,
    /// Owned tiles at the end ([`BotMatch::row`] reads them once; the
    /// observer does not count them every step).
    pub territory: u32,
    /// The side's units that died (`UnitDied`).
    pub units_lost: u32,
    /// The lowest hit points, per mille, any complete Town of the other
    /// side reached.
    pub min_enemy_town_hp_permille: u32,
    /// The side's rejected commands (`CommandRejected`).
    pub rejected: u32,
    /// M6, from the side's bot memory ([`crate::bot_stats`]; `None` for a
    /// Passive seat): waves launched.
    pub waves: Option<u16>,
    /// Raids launched.
    pub raids: Option<u16>,
    /// Waves called home by the retreat rule.
    pub retreats: Option<u16>,
    /// Waves that ended below `recall_below_units`.
    pub wiped: Option<u16>,
    /// M6, from [`MatchStats`]: Towns the side annexed.
    pub towns_taken: u32,
    /// Bonus resources the side received (`BonusIncome`), every resource.
    pub bonus: i64,
    /// Credited income in whole resources, every resource, from the
    /// once-a-second samples.
    pub income: i64,
    /// Seconds with any resource at its Yield Cap.
    pub cap_s: u32,
    /// The first tick an armed unit of the side stood on the other's land.
    pub first_on_land_tick: Option<u32>,
    /// The lowest hit points, per mille, of a Town the side started with or
    /// founded, while it owned it (0 once one was annexed from it).
    pub min_own_town_hp_permille: u32,
}

/// What the observer records for the match as a whole (M6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MatchFacts {
    /// The first tick a hit landed between the two players' entities.
    pub first_contact_tick: Option<u32>,
    /// The longest span with no cross-player hit after first contact, up
    /// to the outcome.
    pub max_quiet_ticks: Option<u32>,
}

/// Collects the play-bots facts after every step from the view and the
/// step's events. Everything it reads is hashed state or an event, so two
/// runs of one seed observe the same row. Since M6 it also feeds a
/// [`MatchStats`], the source of the M6 columns.
#[derive(Debug, Clone)]
pub struct Observer {
    sides: [Side; 2],
    orders: [Option<BuildOrder>; 2],
    age_reached: Option<u8>,
    stats: MatchStats,
}

impl Observer {
    /// An observer for `setup` under `rules`: each Standard seat's build
    /// order (any difficulty, M6, resolved for the level's
    /// `worker_target_permille`) is read from the rules for its
    /// `bo_done_tick`.
    pub fn new(setup: &MatchSetup, rules: &Rules) -> Observer {
        let order = |p: PlayerId| {
            let seat = setup
                .ai_seats
                .iter()
                .find(|s| s.player == p && is_standard_bot(rules, &s.bot))?;
            let level = rules.ai.difficulty(&seat.bot)?;
            let k = rules.ai.personality(&seat.personality)?;
            rules
                .ai
                .build_order(&k.build_order)
                .map(|o| rules::ai::resolve_order(o, level))
        };
        Observer {
            sides: [
                Side {
                    min_enemy_town_hp_permille: 1000,
                    ..Side::default()
                },
                Side {
                    min_enemy_town_hp_permille: 1000,
                    ..Side::default()
                },
            ],
            orders: [order(A), order(B)],
            age_reached: rules.age_index(AGE_REACHED),
            stats: MatchStats::new(setup, rules),
        }
    }

    /// The facts so far: `[A, B]`.
    pub fn sides(&self) -> &[Side; 2] {
        &self.sides
    }

    /// The match statistics so far (M6).
    pub fn stats(&self) -> &MatchStats {
        &self.stats
    }

    /// Record one step: `view` is the sim after it, `events` what it
    /// emitted.
    pub fn observe(&mut self, view: &SimView<'_>, events: &[SimEvent]) {
        self.stats.observe(view, events);
        let tick = view.tick;
        let rules = view.rules();
        for e in events {
            match e {
                SimEvent::UnitDied { owner, .. } => {
                    if let Some(s) = self.side_mut(*owner) {
                        s.units_lost += 1;
                    }
                }
                SimEvent::CommandRejected { player, .. } => {
                    if let Some(s) = self.side_mut(*player) {
                        s.rejected += 1;
                    }
                }
                _ => {}
            }
        }
        let town_kind = BuildingKindId(rules.town_kind());
        for (i, (me, enemy)) in [(A, B), (B, A)].into_iter().enumerate() {
            let side = &mut self.sides[i];
            if side.age_reached_tick.is_none()
                && self.age_reached.is_some_and(|a| view.age(me) >= a)
            {
                side.age_reached_tick = Some(tick);
            }
            let mut own_towns = 0;
            let mut tower = false;
            for b in view.buildings() {
                if b.construction != Construction::Complete {
                    continue;
                }
                if b.owner == me {
                    if b.kind == town_kind {
                        own_towns += 1;
                    } else if rules
                        .building_kind(b.kind.0)
                        .is_some_and(|d| d.border_radius_tiles.is_some())
                    {
                        tower = true;
                    }
                } else if b.owner == enemy
                    && b.kind == town_kind
                    && let Some((hp, max)) = view.building_hp(b.id)
                {
                    let permille = i64::from(hp.max(0)) * 1000 / i64::from(max.max(1));
                    let permille = u32::try_from(permille).unwrap_or(0);
                    side.min_enemy_town_hp_permille = side.min_enemy_town_hp_permille.min(permille);
                }
            }
            if side.second_town_tick.is_none() && own_towns >= 2 {
                side.second_town_tick = Some(tick);
            }
            if side.tower_tick.is_none() && tower {
                side.tower_tick = Some(tick);
            }
            if side.first_attack_tick.is_none() && attack_moves_into(view, me, enemy) {
                let (units, wains) = committed(view, me, enemy);
                side.first_attack_tick = Some(tick);
                side.first_attack_units = Some(units);
                side.first_attack_wains = Some(wains);
            }
            if side.bo_done_tick.is_none()
                && let Some(order) = &self.orders[i]
            {
                let facts = Facts::gather(view, me);
                if build_order::first_unsatisfied(order, &facts, rules, true).is_none() {
                    side.bo_done_tick = Some(tick);
                }
            }
        }
    }

    fn side_mut(&mut self, p: PlayerId) -> Option<&mut Side> {
        self.sides.get_mut(usize::from(p.0))
    }
}

/// Whether `p` owns the tile under `at`.
fn on_land_of(view: &SimView<'_>, at: FxVec2, p: PlayerId) -> bool {
    view.territory().owner_at(view.map().tile_of(at)) == Some(p)
}

/// Whether one of `me`'s units attack-moves to a goal on `enemy`'s land.
fn attack_moves_into(view: &SimView<'_>, me: PlayerId, enemy: PlayerId) -> bool {
    view.units().any(|u| {
        u.owner == me
            && matches!(u.intent, Intent::AttackMove { goal } if on_land_of(view, goal, enemy))
    })
}

/// Whether `u` is committed to `enemy`'s land: its order goal, its post
/// goal, its attack target or its position is on it.
fn is_committed(view: &SimView<'_>, u: &Unit, enemy: PlayerId) -> bool {
    let target_at = u.target.and_then(|t| match t {
        TargetId::Unit(id) => view.unit(id).map(|t| t.pos),
        TargetId::Building(id) => view.building(id).map(|b| view.map().center_of(b.tile)),
    });
    [
        u.order.as_ref().map(|o| o.goal),
        u.post.as_ref().map(|p| p.goal),
        target_at,
        Some(u.pos),
    ]
    .into_iter()
    .flatten()
    .any(|at| on_land_of(view, at, enemy))
}

/// `me`'s non-civilian units committed to `enemy`'s land, and how many of
/// them supply.
fn committed(view: &SimView<'_>, me: PlayerId, enemy: PlayerId) -> (u32, u32) {
    let rules = view.rules();
    let mut units = 0;
    let mut wains = 0;
    for u in view.units().filter(|u| u.owner == me) {
        let Some(kind) = rules.unit_kind(u.kind.0) else {
            continue;
        };
        if kind.class == UnitClass::Civilian || !is_committed(view, u, enemy) {
            continue;
        }
        units += 1;
        if kind.supplies {
            wains += 1;
        }
    }
    (units, wains)
}

/// One bot match: a sim with no human input and its observer.
pub struct BotMatch {
    /// The sim.
    pub sim: Sim,
    /// The observer, fed after every step.
    pub observer: Observer,
}

impl BotMatch {
    /// A match of `setup` under `rules` with `controller` in the AI seats.
    pub fn new(setup: MatchSetup, rules: Rules, controller: Box<dyn AiController>) -> BotMatch {
        let observer = Observer::new(&setup, &rules);
        BotMatch {
            sim: Sim::new(setup, rules, controller),
            observer,
        }
    }

    /// A match with the header's own bots ([`controller_for`]).
    pub fn with_bots(setup: MatchSetup, rules: Rules) -> Result<BotMatch, BotError> {
        let controller = controller_for(&setup, &rules)?;
        Ok(BotMatch::new(setup, rules, controller))
    }

    /// Step the sim once with no human commands (the bots think inside).
    pub fn step_sim(&mut self) {
        self.sim.step(&[]);
    }

    /// Drain the step's events into the observer.
    pub fn observe(&mut self) {
        let events = self.sim.drain_events();
        self.observer.observe(&self.sim.view(), &events);
    }

    /// [`BotMatch::step_sim`] then [`BotMatch::observe`].
    pub fn step(&mut self) {
        self.step_sim();
        self.observe();
    }

    /// Step until the match is decided or the sim reaches `max_ticks`.
    pub fn run(&mut self, max_ticks: u32) {
        while self.sim.result().is_none() && self.sim.tick() < max_ticks {
            self.step();
        }
    }

    /// The row for `seed` at the current state, with the territory counted
    /// now, the bot statistics read from the memories now (M6), and the
    /// [`MatchStats`] columns.
    pub fn row(&self, seed: u64) -> GameRow {
        let view = self.sim.view();
        let stats = self.observer.stats();
        let mut sides = *self.observer.sides();
        for (i, side) in sides.iter_mut().enumerate() {
            let p = PlayerId(u8::try_from(i).expect("two sides"));
            side.territory = view.territory().owned_tile_count(p);
            let bot = bot_stats(&view, p);
            side.waves = bot.map(|b| b.waves_launched);
            side.raids = bot.map(|b| b.raids);
            side.retreats = bot.map(|b| b.retreats);
            side.wiped = bot.map(|b| b.wiped);
            let s = stats.player(p);
            side.towns_taken = s.towns_taken;
            side.bonus = s.bonus_total();
            side.income = stats.income_total(p);
            side.cap_s = s.cap_seconds;
            side.first_on_land_tick = s.first_on_land_tick;
            side.min_own_town_hp_permille = s.min_own_town_hp_permille;
        }
        let mut row = GameRow::finished(
            seed,
            self.sim.result(),
            self.sim.tick(),
            self.sim.hash(),
            sides,
        );
        row.seats = seats_of(self.sim.setup());
        row.facts = Some(MatchFacts {
            first_contact_tick: stats.first_contact_tick(),
            max_quiet_ticks: stats.max_quiet_ticks(),
        });
        row
    }
}

/// The header's `(bot, personality)` of A and B (empty for a human slot).
pub fn seats_of(setup: &MatchSetup) -> [(String, String); 2] {
    [A, B].map(|p| {
        setup
            .ai_seats
            .iter()
            .find(|s| s.player == p)
            .map(|s| (s.bot.clone(), s.personality.clone()))
            .unwrap_or_default()
    })
}

/// How a decided match ended, as the `outcome` column prints it.
fn outcome_str(r: &MatchResult) -> &'static str {
    match r.outcome {
        Outcome::Draw => "draw",
        Outcome::Victory { decisive: true, .. } => "decisive",
        Outcome::Victory {
            decisive: false, ..
        } => "tiebreak",
    }
}

/// The `reason` column.
pub fn reason_str(r: OutcomeReason) -> &'static str {
    match r {
        OutcomeReason::SeatCaptured => "seat_captured",
        OutcomeReason::AllTownsLost => "all_towns_lost",
        OutcomeReason::Surrender => "surrender",
        OutcomeReason::Territory => "territory",
    }
}

/// One play-bots CSV row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameRow {
    /// The match seed.
    pub seed: u64,
    /// Who won.
    pub winner: Winner,
    /// The result, once decided.
    pub result: Option<MatchResult>,
    /// The deciding tick, or the last tick run; `None` after a panic.
    pub tick: Option<u32>,
    /// `Sim::hash()` there; `None` after a panic.
    pub final_hash: Option<u64>,
    /// The observed facts, `[A, B]`; `None` after a panic.
    pub sides: Option<[Side; 2]>,
    /// The header's `(bot, personality)` of A and B (M6; empty when not
    /// known).
    pub seats: [(String, String); 2],
    /// The whole-match facts (M6); `None` after a panic.
    pub facts: Option<MatchFacts>,
}

impl GameRow {
    /// The row of a match that ran to `tick` with `result` (if decided)
    /// and the observed `sides`.
    pub fn finished(
        seed: u64,
        result: Option<MatchResult>,
        tick: u32,
        final_hash: u64,
        sides: [Side; 2],
    ) -> GameRow {
        let winner = match result.map(|r| r.outcome) {
            Some(Outcome::Victory { player, .. }) if player == A => Winner::A,
            Some(Outcome::Victory { player, .. }) if player == B => Winner::B,
            Some(Outcome::Victory { .. }) | None => Winner::None,
            Some(Outcome::Draw) => Winner::Draw,
        };
        GameRow {
            seed,
            winner,
            result,
            tick: Some(result.map_or(tick, |r| r.tick)),
            final_hash: Some(final_hash),
            sides: Some(sides),
            seats: Default::default(),
            facts: Some(MatchFacts::default()),
        }
    }

    /// The row of a game that panicked.
    pub fn panicked(seed: u64) -> GameRow {
        GameRow {
            seed,
            winner: Winner::Panic,
            result: None,
            tick: None,
            final_hash: None,
            sides: None,
            seats: Default::default(),
            facts: None,
        }
    }

    /// The row in [`CSV_HEADER`]'s column order; an unknown value is
    /// empty.
    pub fn csv(&self) -> String {
        fn opt<T: ToString>(v: Option<T>) -> String {
            v.map(|v| v.to_string()).unwrap_or_default()
        }
        let r = self.result;
        let blank = Side::default();
        let (a, b, known) = match &self.sides {
            Some([a, b]) => (a, b, true),
            None => (&blank, &blank, false),
        };
        let num = |v: u32| if known { v.to_string() } else { String::new() };
        let num64 = |v: i64| if known { v.to_string() } else { String::new() };
        let facts = self.facts.unwrap_or_default();
        let [(bot_a, personality_a), (bot_b, personality_b)] = &self.seats;
        let cols = [
            self.seed.to_string(),
            self.winner.as_str().to_string(),
            r.as_ref().map(outcome_str).unwrap_or_default().to_string(),
            opt(self.tick),
            self.final_hash
                .map(|h| format!("0x{h:016x}"))
                .unwrap_or_default(),
            opt(a.age_reached_tick),
            opt(a.first_attack_tick),
            r.map(|r| reason_str(r.reason))
                .unwrap_or_default()
                .to_string(),
            opt(a.first_attack_units),
            opt(a.first_attack_wains),
            opt(a.second_town_tick),
            opt(a.tower_tick),
            opt(a.bo_done_tick),
            opt(b.age_reached_tick),
            opt(b.first_attack_tick),
            opt(b.first_attack_units),
            opt(b.first_attack_wains),
            num(a.territory),
            num(b.territory),
            num(a.units_lost),
            num(b.units_lost),
            num(a.min_enemy_town_hp_permille),
            num(b.min_enemy_town_hp_permille),
            num(a.rejected),
            num(b.rejected),
            opt(b.second_town_tick),
            opt(b.tower_tick),
            opt(b.bo_done_tick),
            // M6.
            bot_a.clone(),
            personality_a.clone(),
            bot_b.clone(),
            personality_b.clone(),
            opt(a.waves),
            opt(b.waves),
            opt(a.raids),
            opt(b.raids),
            opt(a.retreats),
            opt(b.retreats),
            opt(a.wiped),
            opt(b.wiped),
            num(a.towns_taken),
            num(b.towns_taken),
            num64(a.bonus),
            num64(b.bonus),
            num64(a.income),
            num64(b.income),
            num(a.cap_s),
            num(b.cap_s),
            opt(facts.first_contact_tick),
            opt(facts.max_quiet_ticks),
            opt(a.first_on_land_tick),
            opt(b.first_on_land_tick),
            num(a.min_own_town_hp_permille),
            num(b.min_own_town_hp_permille),
        ];
        cols.join(",")
    }
}

/// The play-bots exit status: 1 when any game panicked, or when any row
/// names no winner (`draw`, `none`) unless `allow_undecided`; 0 otherwise.
pub fn exit_code(rows: &[GameRow], allow_undecided: bool) -> u8 {
    let panicked = rows.iter().any(|r| r.winner == Winner::Panic);
    let undecided = rows.iter().any(|r| !r.winner.declared());
    if panicked || (undecided && !allow_undecided) {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::{Command, PlayerCommand, UnitId, UnitKindId};
    use std::path::Path;

    fn rules() -> Rules {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap()
    }

    /// M6: the 26 columns appended after M5b's 28, in order, and a real
    /// row of a Hard bot against Passive fills them.
    #[test]
    fn csv_appends_the_m6_columns_after_the_m5b_ones() {
        let cols: Vec<&str> = CSV_HEADER.split(',').collect();
        assert_eq!(cols.len(), M5B_COLUMNS + M6_COLUMNS);
        assert_eq!(
            cols[M5B_COLUMNS..],
            [
                "bot_a",
                "personality_a",
                "bot_b",
                "personality_b",
                "waves_a",
                "waves_b",
                "raids_a",
                "raids_b",
                "retreats_a",
                "retreats_b",
                "wiped_a",
                "wiped_b",
                "towns_taken_a",
                "towns_taken_b",
                "bonus_a",
                "bonus_b",
                "income_a",
                "income_b",
                "cap_s_a",
                "cap_s_b",
                "first_contact_tick",
                "max_quiet_ticks",
                "first_on_land_tick_a",
                "first_on_land_tick_b",
                "min_own_town_hp_permille_a",
                "min_own_town_hp_permille_b"
            ]
        );
        let r = rules();
        let setup = MatchSetup::bots(&r, 1, ("hard", "standard"), ("passive", ""));
        let mut m = BotMatch::with_bots(setup, r.clone()).unwrap();
        m.run(4400);
        let row = m.row(1).csv();
        let got: Vec<&str> = row.split(',').collect();
        assert_eq!(got.len(), cols.len(), "{row}");
        let col = |name: &str| got[cols.iter().position(|c| *c == name).unwrap()];
        assert_eq!(
            [
                col("bot_a"),
                col("personality_a"),
                col("bot_b"),
                col("personality_b")
            ],
            ["hard", "standard", "passive", ""]
        );
        // Hard pays from 3:00 every 20 s: ticks 4000 and 4400.
        let bundle: i64 = r.ai.difficulty("hard").unwrap().bundle(&r).iter().sum();
        assert_eq!(col("bonus_a"), (2 * bundle).to_string());
        assert_eq!(col("bonus_b"), "0");
        assert_eq!(col("waves_a"), "0");
        assert_eq!(col("waves_b"), "", "Passive has no bot memory");
        assert!(col("income_a").parse::<i64>().unwrap() > 0);
        assert_eq!(col("min_own_town_hp_permille_b"), "1000");
        assert_eq!(col("first_contact_tick"), "", "no fight by 4400");
        assert_eq!(col("max_quiet_ticks"), "");
    }

    #[test]
    fn csv_header_starts_with_the_seven_design_columns() {
        let cols: Vec<&str> = CSV_HEADER.split(',').collect();
        assert_eq!(
            cols[..7],
            [
                "seed",
                "winner",
                "outcome",
                "tick",
                "final_hash",
                "age_reached_tick",
                "first_attack_tick"
            ]
        );
        assert_eq!(cols.len(), M5B_COLUMNS + M6_COLUMNS);
        assert_eq!(
            cols[25..M5B_COLUMNS],
            ["second_town_tick_b", "tower_tick_b", "bo_done_tick_b"]
        );
        let row = GameRow::panicked(4).csv();
        assert_eq!(row.split(',').count(), cols.len());
        assert_eq!(row, format!("4,panic{}", ",".repeat(cols.len() - 2)));
    }

    #[test]
    fn exit_code_is_1_for_draw_none_and_panic() {
        let row = |winner| GameRow {
            winner,
            ..GameRow::panicked(1)
        };
        let all = [
            Winner::A,
            Winner::B,
            Winner::Draw,
            Winner::None,
            Winner::Panic,
        ];
        for w in all {
            let code = exit_code(&[row(Winner::A), row(w)], false);
            assert_eq!(code, u8::from(!w.declared()), "{w:?}");
            let allowed = exit_code(&[row(w), row(Winner::B)], true);
            assert_eq!(allowed, u8::from(w == Winner::Panic), "{w:?} allowed");
        }
        assert_eq!(exit_code(&[], false), 0);
    }

    /// `first_attack_tick` is the first tick one of A's units attack-moves
    /// to a goal on B's land: a Move there, or an attack-move to neutral
    /// ground, does not count. At that tick, `first_attack_units` counts
    /// every non-civilian unit of A whose order goal, post goal, attack
    /// target or position is on B's land, and `first_attack_wains` the
    /// Supply Wains among them.
    #[test]
    fn first_attack_counts_goal_post_target_or_position_on_enemy_land() {
        let r = rules();
        let kind = |id: &str| {
            UnitKindId(u16::try_from(r.units.iter().position(|u| u.id == id).unwrap()).unwrap())
        };
        let mut setup = MatchSetup::bots(&r, 1, ("passive", ""), ("passive", ""));
        setup.debug_commands = true;
        let mut m = BotMatch::with_bots(setup, r.clone()).unwrap();
        let delay = m.sim.setup().cmd_delay;
        let mut seq = 1 << 30;
        let mut cmd = |c: Command| {
            seq += 1;
            PlayerCommand::new(A, seq, c)
        };
        let spawn = |k: &str, x: i32, y: i32| Command::DebugSpawn {
            owner: A,
            kind: kind(k),
            at: FxVec2::from_ints(x, y),
            count: 1,
        };
        let mv = |u: u32, x: i32, y: i32| Command::Move {
            units: vec![UnitId(u)],
            target: FxVec2::from_ints(x, y),
            queue: false,
        };
        let amove = |u: u32, x: i32, y: i32| Command::AttackMove {
            units: vec![UnitId(u)],
            target: FxVec2::from_ints(x, y),
        };
        // The standard start holds UnitId(1..=10). Neutral ground east of
        // the river: (70, 60..=68); B's land: x >= 90 near row 64.
        let tick0 = vec![
            cmd(spawn("skirmisher", 70, 60)), // 11: attack-moves, staging then assault
            cmd(spawn("skirmisher", 70, 62)), // 12: moves in at the assault
            cmd(spawn("supply_wain", 70, 64)), // 13: the Wain moves in
            cmd(spawn("skirmisher", 115, 75)), // 14: stands on B's land
            cmd(spawn("skirmisher", 70, 66)), // 15: attacks B's Seat
            cmd(spawn("skirmisher", 70, 68)), // 16: moved in early, no attack-move
            cmd(spawn("skirmisher", 72, 64)), // 17: stays on neutral ground
            cmd(spawn("yeoman", 115, 72)),    // 18: a civilian on B's land
        ];
        m.sim.step(&tick0);
        m.observe();
        while m.sim.tick() < 5 {
            m.step();
        }
        m.sim.step(&[cmd(amove(11, 75, 62)), cmd(mv(16, 95, 70))]);
        m.observe();
        while m.sim.tick() < 20 {
            m.step();
        }
        let a = m.observer.sides()[0];
        assert_eq!(
            a.first_attack_tick, None,
            "staging and a plain Move do not count"
        );
        for u in 11..=18 {
            assert!(m.sim.view().unit(UnitId(u)).is_some(), "unit {u} spawned");
        }
        let seat_b = sim::BuildingId(2);
        assert_eq!(m.sim.view().building(seat_b).unwrap().owner, B);
        m.sim.step(&[
            cmd(amove(11, 100, 60)),
            cmd(mv(12, 100, 66)),
            cmd(mv(13, 98, 64)),
            cmd(Command::Attack {
                units: vec![UnitId(15)],
                target: TargetId::Building(seat_b),
            }),
        ]);
        m.observe();
        for _ in 0..delay {
            m.step();
        }
        let a = m.observer.sides()[0];
        assert_eq!(a.first_attack_tick, Some(21 + delay));
        assert_eq!(
            (a.first_attack_units, a.first_attack_wains),
            (Some(6), Some(1)),
            "11, 12, 13 (the Wain), 14, 15 and 16; not 17 or the Yeoman"
        );
        assert_eq!(m.observer.sides()[1].first_attack_tick, None);
    }
}
