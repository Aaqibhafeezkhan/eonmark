//! The Standard bot (M5b): the single v0.1 opponent.
//!
//! Each call of [`StandardBot::think`] is either an off-cadence call, which
//! returns at once, or a plan: decode the memory, gather the [`Facts`], run
//! the rules over one [`Ctx`] in a fixed order, write the memory back if it
//! changed, and return the plan's commands. A bot plans on ticks where
//! `tick % every == player × every / players`, `every` being the
//! personality's `think_every_ds` in ticks (more than the command delay,
//! so a plan's commands apply before the next plan reads the view), so two
//! bots never plan on the same tick.
//!
//! The rules run in a fixed order over one plan context, each claiming
//! the units it orders so a later rule leaves them alone:
//!
//! 1. [`defend`]: the defend latch, the home guard, wave recall;
//! 2. [`build_order`]: the `Military` latch, the window, the site keeper;
//! 3. [`economy`]: idle workers and the gather split;
//! 4. [`army::produce`]: Supply Wains, then the army mix;
//! 5. [`waves`]: launch, stage, assault, ring hold, continue, recall,
//!    all-in;
//! 6. [`army::rally_upkeep`]: trainers rallied to the army's rally point.

pub mod army;
pub mod build_order;
pub mod ctx;
pub mod defend;
pub mod economy;
pub mod facts;
pub mod memory;
pub mod sites;
pub mod waves;

pub use ctx::{Ctx, Ledger, goal_or_post, on_station};
pub use facts::Facts;
pub use memory::{BotMemory, BotStats, stats};
pub use sites::MapAnalysis;

use rules::{Personality, Rules};
use sim::{AiController, AiMemory, Command, MatchSetup, PlayerId, SimView};

/// The Standard bot of one seat. Its only state besides the hashed memory
/// is the [`MapAnalysis`], a pure function of immutable inputs, so a bot
/// rebuilt mid-match decides exactly as the one it replaces.
#[derive(Debug, Clone)]
pub struct StandardBot {
    player: PlayerId,
    personality: String,
    map: MapAnalysis,
}

/// The tick offset of `me`'s plans within the cadence: `me × every /
/// players`, so the seats of a match plan on different ticks.
pub fn phase(me: PlayerId, every: u32, setup: &MatchSetup) -> u32 {
    let players = u32::try_from(setup.players.len().max(1)).expect("at most 255 players");
    u32::from(me.0) * every / players
}

impl StandardBot {
    /// The bot for seat `player` with `personality` (a row of
    /// `rules.ai`).
    pub fn new(
        rules: &Rules,
        setup: &MatchSetup,
        player: PlayerId,
        personality: &Personality,
    ) -> StandardBot {
        StandardBot {
            player,
            personality: personality.id.clone(),
            map: MapAnalysis::new(rules, setup, player, personality),
        }
    }

    /// The seat this bot plays.
    pub fn player(&self) -> PlayerId {
        self.player
    }

    /// The bot's static map analysis.
    pub fn map(&self) -> &MapAnalysis {
        &self.map
    }

    /// The personality's knobs, looked up in the rules the sim runs on.
    ///
    /// # Panics
    ///
    /// If the rules have no such personality (`ai::controller_for` checks
    /// it before building the bot).
    pub fn knobs<'r>(&self, rules: &'r Rules) -> &'r Personality {
        rules
            .ai
            .personality(&self.personality)
            .expect("controller_for checked the personality")
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
        let knobs = self.knobs(view.rules());
        let mut mem = BotMemory::decode(memory.bytes());
        let mut ctx = Ctx::new(*view, me, knobs, &self.map);
        run_rules(&mut ctx, &mut mem);
        let bytes = mem.encode();
        if bytes != memory.bytes() {
            memory.set(bytes);
        }
        let stats = PlanStats {
            probes: ctx.probes_made,
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
        let every = self
            .knobs(view.rules())
            .think_every_ticks(view.rules())
            .max(1);
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
}

/// The plan pipeline over `ctx` and `mem`, in its fixed order.
pub fn run_rules(ctx: &mut Ctx<'_>, mem: &mut BotMemory) {
    defend::step(ctx, mem);
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
