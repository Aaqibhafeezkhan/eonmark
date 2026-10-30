//! The Standard bot (M5b): the single v0.1 opponent.
//!
//! Each call of [`StandardBot::think`] is either an off-cadence call, which
//! returns at once, or a plan: decode the memory, gather the [`Facts`], run
//! the rules over one [`Ctx`] in a fixed order, write the memory back if it
//! changed, and return the plan's commands. A bot plans on ticks where
//! `tick % every == phase`, `every` being the personality's
//! `think_every_ds` in ticks (more than the command delay, so a plan's
//! commands apply before the next plan reads the view) and the phase
//! `((player + seed) mod players) × every / players` ([`phase`]), so two
//! bots never plan on the same tick and, since M6, which seat plans first
//! alternates with the seed's parity.
//!
//! The rules run in a fixed order over one plan context, each claiming
//! the units it orders so a later rule leaves them alone:
//!
//! 1. [`defend`]: the defend latch, the home guard (M6: Wains first for
//!    a `defend_wains_first` personality), threats to Towns and (M6) to
//!    worked gathering buildings, wave recall;
//! 2. [`retreat`] (M6): a losing wave comes home with a plain `Move`;
//! 3. [`build_order`]: the `Military` latch, the window, the site keeper,
//!    (M6) the reactive Watchtower;
//! 4. [`economy`]: idle workers and the gather split;
//! 5. [`army::produce`]: Supply Wains, then the army mix weighted by
//!    counters ([`compose`], M6), and before a difficulty's deadline raid
//!    emergency production;
//! 6. [`waves`]: launch, stage, assault, ring hold, continue, recall,
//!    all-in; (M6) raids, the deadline raid, the Wain rule, `seat_last`
//!    and influence-scored targets;
//! 7. [`army::rally_upkeep`]: trainers rallied to the army's rally point.
//!
//! [`influence`] holds the per-plan strength grids (from the view, never
//! stored), which retreat and the target scores read through
//! [`Ctx::influence`].

pub mod army;
pub mod build_order;
pub mod compose;
pub mod ctx;
pub mod defend;
pub mod economy;
pub mod facts;
pub mod influence;
pub mod memory;
pub mod retreat;
pub mod sites;
pub mod waves;

pub use ctx::{Ctx, Ledger, goal_or_post, on_station};
pub use facts::Facts;
pub use memory::{BotMemory, BotStats, WaveKind, stats};
pub use sites::MapAnalysis;

use rules::{BuildOrder, Difficulty, FirstRaid, Personality, Pressure, Rules};
use sim::{AiController, AiMemory, Command, MatchSetup, PlayerId, SimView};

/// The Standard bot of one seat. Its only state besides the hashed memory
/// is the [`MapAnalysis`] and (M6) the knobs and first raid resolved from
/// its personality and difficulty, all pure functions of immutable inputs
/// (the rules and the header), so a bot rebuilt mid-match decides exactly
/// as the one it replaces.
#[derive(Debug, Clone)]
pub struct StandardBot {
    player: PlayerId,
    difficulty: String,
    knobs: Personality,
    order: BuildOrder,
    first_raid: Option<FirstRaid>,
    pressure: Pressure,
    map: MapAnalysis,
}

/// The tick offset of `me`'s plans within the cadence: `((me + seed) mod
/// players) × every / players`, so the seats of a match plan on different
/// ticks and (M6) which one plans first alternates with the seed's parity
/// in a two-player match.
pub fn phase(me: PlayerId, every: u32, setup: &MatchSetup) -> u32 {
    let players = u32::try_from(setup.players.len().max(1)).expect("at most 255 players");
    let slot = (u64::from(me.0) + setup.seed % u64::from(players)) % u64::from(players);
    u32::try_from(slot).expect("below the player count") * every / players
}

impl StandardBot {
    /// The bot for seat `player` with `personality` (a row of `rules.ai`)
    /// at `difficulty` (M6): it plays `rules::ai::resolve(personality,
    /// difficulty)`; for `standard` that is the personality with its all-in
    /// at 36:00 (`rules_version` 30; its `pressure` block is read by the
    /// bot, not by `resolve`).
    pub fn new(
        rules: &Rules,
        setup: &MatchSetup,
        player: PlayerId,
        personality: &Personality,
        difficulty: &Difficulty,
    ) -> StandardBot {
        let knobs = rules::ai::resolve(personality, difficulty);
        let order = rules
            .ai
            .build_order(&knobs.build_order)
            .map(|o| rules::ai::resolve_order(o, difficulty))
            .expect("Rules::load checks every personality's build order");
        StandardBot {
            player,
            difficulty: difficulty.id.clone(),
            map: MapAnalysis::new(rules, setup, player, &knobs),
            knobs,
            order,
            first_raid: difficulty.first_raid,
            pressure: difficulty.pressure,
        }
    }

    /// The seat this bot plays.
    pub fn player(&self) -> PlayerId {
        self.player
    }

    /// The difficulty id the bot plays at.
    pub fn difficulty(&self) -> &str {
        &self.difficulty
    }

    /// The bot's static map analysis.
    pub fn map(&self) -> &MapAnalysis {
        &self.map
    }

    /// The knobs the bot plays: its personality resolved for its
    /// difficulty (`rules::ai::resolve`).
    pub fn knobs(&self) -> &Personality {
        &self.knobs
    }

    /// The difficulty's guaranteed early raid (M6, [`waves`]).
    pub fn first_raid(&self) -> Option<FirstRaid> {
        self.first_raid
    }

    /// How the difficulty presses its sieges (M6 owner decision 12).
    pub fn pressure(&self) -> Pressure {
        self.pressure
    }

    /// A fresh plan context for `me` over `view` with the bot's knobs, map
    /// analysis and (M6) resolved build order, first raid and pressure.
    pub fn ctx<'a>(&'a self, view: SimView<'a>, me: PlayerId) -> Ctx<'a> {
        let mut ctx = Ctx::new(view, me, &self.knobs, &self.map);
        ctx.order = &self.order;
        ctx.first_raid = self.first_raid;
        ctx.pressure = self.pressure;
        ctx
    }

    /// The build order the bot plays: its personality's, with the
    /// difficulty's `worker_target_permille` applied
    /// (`rules::ai::resolve_order`, M6).
    pub fn order(&self) -> &BuildOrder {
        &self.order
    }

    /// One plan, whatever the tick: the pipeline over a fresh [`Ctx`],
    /// the memory written back when it changed, and the plan's commands.
    /// [`AiController::think`] calls it on the bot's cadence; tests call it
    /// directly.
    pub fn plan(&self, me: PlayerId, view: &SimView<'_>, memory: &mut AiMemory) -> Vec<Command> {
        self.plan_with_stats(me, view, memory).0
    }

    /// [`StandardBot::plan`], and what the plan used of its caps.
    pub fn plan_with_stats(
        &self,
        me: PlayerId,
        view: &SimView<'_>,
        memory: &mut AiMemory,
    ) -> (Vec<Command>, PlanStats) {
        let mut mem = BotMemory::decode(memory.bytes());
        let mut ctx = self.ctx(*view, me);
        run_rules(&mut ctx, &mut mem);
        let bytes = mem.encode();
        if bytes != memory.bytes() {
            memory.set(bytes);
        }
        let stats = PlanStats {
            probes: ctx.probes_made,
            influence: ctx.influence_built,
        };
        let cmds = ctx.into_commands();
        (cmds, stats)
    }

    /// [`AiController::think`], and the plan's [`PlanStats`] on a plan
    /// tick (`None` off the cadence or once the match is decided).
    pub fn think_with_stats(
        &self,
        me: PlayerId,
        view: &SimView<'_>,
        memory: &mut AiMemory,
    ) -> (Vec<Command>, Option<PlanStats>) {
        if view.outcome().is_some() {
            return (Vec::new(), None);
        }
        let every = self.knobs.think_every_ticks(view.rules()).max(1);
        if view.tick % every != phase(me, every, view.setup()) {
            return (Vec::new(), None);
        }
        let (cmds, stats) = self.plan_with_stats(me, view, memory);
        (cmds, Some(stats))
    }
}

/// What one plan used of its caps (the work-cap test,
/// `m5b_standard_respects_work_caps`, reads it).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanStats {
    /// `check_build` probes made (each a full-map search), counted by
    /// [`Ctx::check_build`]; at most `site_checks_per_think`.
    pub probes: u32,
    /// Whether the plan built the influence maps (M6; `bench --bots`
    /// counts these plans).
    pub influence: bool,
}

/// The plan pipeline over `ctx` and `mem`, in its fixed order.
pub fn run_rules(ctx: &mut Ctx<'_>, mem: &mut BotMemory) {
    defend::step(ctx, mem);
    retreat::step(ctx, mem);
    build_order::step(ctx, mem);
    economy::step(ctx, mem);
    army::produce(ctx, mem);
    waves::step(ctx, mem);
    army::rally_upkeep(ctx);
}

impl AiController for StandardBot {
    fn think(&mut self, me: PlayerId, view: &SimView<'_>, memory: &mut AiMemory) -> Vec<Command> {
        self.think_with_stats(me, view, memory).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M6: which seat plans first alternates with the seed's parity: P0 on
    /// even seeds, P1 on odd ones, half the cadence apart either way.
    #[test]
    fn phase_alternates_with_the_seed() {
        let r = crate::testkit::rules();
        for seed in 0..6u64 {
            let setup =
                MatchSetup::bots(&r, seed, ("standard", "standard"), ("standard", "standard"));
            let (p0, p1) = (
                phase(PlayerId(0), 10, &setup),
                phase(PlayerId(1), 10, &setup),
            );
            let want = if seed % 2 == 0 { (0, 5) } else { (5, 0) };
            assert_eq!((p0, p1), want, "seed {seed}");
        }
        // A one-player header never divides by zero.
        let mut solo = MatchSetup::bots(&r, 3, ("standard", "standard"), ("standard", "standard"));
        solo.players.truncate(1);
        assert_eq!(phase(PlayerId(0), 10, &solo), 0);
    }
}
