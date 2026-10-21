//! The hook through which scripted opponents run *inside* [`crate::Sim::step`].
//!
//! Running the AI inside the step means bot-vs-bot replays and future
//! lockstep peers agree on every decision: the bot sees only the hashed
//! state, draws no randomness of its own, and its commands go through the
//! same delay queue as a human's.

use crate::command::Command;
use crate::construction::{self, BuildPlan};
use crate::economy::{self, PlayerEconomy, RateInfo};
use crate::gather;
use crate::ids::{BuildingId, BuildingKindId, PlayerId, Tile, UnitId, UnitKindId};
use crate::map::Map;
use crate::pathing::Pathing;
use crate::state::{Building, MatchSetup, Player, RejectReason, State, Unit};
use crate::territory::TerritoryField;
use crate::town;
use crate::training;
use rules::Rules;

/// Read-only view of the sim handed to AI controllers and the presenter.
/// Every economy accessor calls the same function `Sim::step` uses, so a
/// quote or a check here always matches what applying the command does.
pub struct SimView<'a> {
    /// Current tick.
    pub tick: u32,
    state: &'a State,
    rules: &'a Rules,
    setup: &'a MatchSetup,
}

impl<'a> SimView<'a> {
    pub(crate) fn new(state: &'a State, rules: &'a Rules, setup: &'a MatchSetup) -> SimView<'a> {
        SimView {
            tick: state.tick,
            state,
            rules,
            setup,
        }
    }

    /// The match setup.
    pub fn setup(&self) -> &'a MatchSetup {
        self.setup
    }

    /// A player's stockpiles and accumulators.
    ///
    /// # Panics
    ///
    /// If the player does not exist.
    pub fn economy(&self, p: PlayerId) -> &'a PlayerEconomy {
        self.state.economy.player(p)
    }

    /// A player's income this tick per resource, `resources.ron` order.
    pub fn income(&self, p: PlayerId) -> Vec<RateInfo> {
        economy::rates(self.state, self.rules, p)
    }

    /// The territory field.
    pub fn territory(&self) -> &'a TerritoryField {
        &self.state.territory
    }

    /// One building or site, if it exists.
    pub fn building(&self, id: BuildingId) -> Option<&'a Building> {
        self.state.buildings.get(&id)
    }

    /// A gathering building's slots: `(worked, taken, slots)`. Taken slots
    /// have a unit assigned (walking or working); worked ones earn income.
    /// `None` when the building does not exist.
    pub fn worked_slots(&self, id: BuildingId) -> Option<(u8, u8, u8)> {
        let b = self.state.buildings.get(&id)?;
        let (worked, taken) = gather::slot_counts(self.state, self.rules, id);
        Some((worked, taken, b.slots))
    }

    /// `p`'s idle workers in id order: units of a kind that works some
    /// building's slots, with no job and no order.
    pub fn idle_workers(&self, p: PlayerId) -> Vec<UnitId> {
        let workers = gather::worker_kinds(self.rules);
        self.state
            .units
            .values()
            .filter(|u| {
                u.owner == p && workers.contains(&u.kind) && u.job.is_none() && u.order.is_none()
            })
            .map(|u| u.id)
            .collect()
    }

    /// `p`'s pop: `(used, cap)`, used counting units alive and queued.
    pub fn pop(&self, p: PlayerId) -> (u32, u32) {
        (
            training::pop_used(self.state, self.rules, p),
            training::pop_cap(self.rules, p),
        )
    }

    /// What one more unit of `kind` costs `p` now (the ramped cost `Train`
    /// would deduct), `resources.ron` order.
    ///
    /// # Panics
    ///
    /// If `kind` is not in the rules.
    pub fn unit_cost(&self, p: PlayerId, kind: UnitKindId) -> Vec<i64> {
        training::unit_cost_for(self.state, self.rules, p, kind)
    }

    /// Whether `Train { building, kind }` from `p` would be accepted now,
    /// and what it would deduct.
    pub fn check_train(
        &self,
        p: PlayerId,
        building: BuildingId,
        kind: UnitKindId,
    ) -> Result<Vec<i64>, RejectReason> {
        self.check_player(p)?;
        training::validate_train(self.state, self.rules, p, building, kind)
    }

    /// The checks `Sim::apply` makes on the issuing player before any
    /// command's own: it exists (`UnknownPlayer`) and has not surrendered
    /// (`Surrendered`).
    fn check_player(&self, p: PlayerId) -> Result<(), RejectReason> {
        match self.player(p) {
            None => Err(RejectReason::UnknownPlayer),
            Some(player) if player.surrendered => Err(RejectReason::Surrendered),
            Some(_) => Ok(()),
        }
    }

    /// Town-kind buildings `p` owns, sites included, and the town limit.
    pub fn towns(&self, p: PlayerId) -> (u32, u32) {
        (
            town::town_count(self.state, self.rules, p),
            self.rules.town_limit(town::statecraft(p)),
        )
    }

    /// What a building of `kind` costs `p` now (the ramped cost `Build`
    /// would deduct), `resources.ron` order.
    ///
    /// # Panics
    ///
    /// If `kind` is not in the rules.
    pub fn building_cost(&self, p: PlayerId, kind: BuildingKindId) -> Vec<i64> {
        economy::building_cost_for(self.state, self.rules, p, kind)
    }

    /// Whether `Build { unit, kind, tile }` from `p` would be accepted now,
    /// and what it would deduct (zeros when it resumes an own site).
    pub fn check_build(
        &self,
        p: PlayerId,
        unit: UnitId,
        kind: BuildingKindId,
        tile: Tile,
    ) -> Result<Vec<i64>, RejectReason> {
        self.check_player(p)?;
        let def = self
            .rules
            .map(&self.setup.map)
            .expect("setup map is in the rules");
        match construction::validate_build(self.state, self.rules, def, p, unit, kind, tile)? {
            BuildPlan::Resume { .. } => Ok(vec![0; self.rules.resources.resources.len()]),
            BuildPlan::New { cost, .. } => Ok(cost),
        }
    }

    /// The rules this match runs under.
    pub fn rules(&self) -> &'a Rules {
        self.rules
    }

    /// Match seed (informational; the bot must not reseed anything from it).
    pub fn seed(&self) -> u64 {
        self.state.seed
    }

    /// All players in slot order.
    pub fn players(&self) -> &'a [Player] {
        &self.state.players
    }

    /// One player, if the slot exists.
    pub fn player(&self, id: PlayerId) -> Option<&'a Player> {
        self.state.players.iter().find(|p| p.id == id)
    }

    /// Units in id order.
    pub fn units(&self) -> impl Iterator<Item = &'a Unit> + 'a {
        self.state.units.values()
    }

    /// One unit, if it exists.
    pub fn unit(&self, id: UnitId) -> Option<&'a Unit> {
        self.state.units.get(&id)
    }

    /// Number of units.
    pub fn unit_count(&self) -> usize {
        self.state.units.len()
    }

    /// The tile map (cost grid, generation, components).
    pub fn map(&self) -> &'a Map {
        &self.state.map
    }

    /// The pathing subsystem (queue, active search, cache statistics).
    pub fn pathing(&self) -> &'a Pathing {
        &self.state.pathing
    }

    /// Buildings in id order.
    pub fn buildings(&self) -> impl Iterator<Item = &'a Building> + 'a {
        self.state.buildings.values()
    }

    /// Commands applied so far (M0 placeholder statistic).
    pub fn commands_applied(&self) -> u64 {
        self.state.commands_applied
    }
}

/// A scripted opponent that runs *inside* [`crate::Sim::step`] so bot-vs-bot
/// replays and future lockstep peers agree on every decision.
///
/// Implementations must be deterministic functions of the view (and, later,
/// of randomness handed to them by the sim). `Send` is required so matches
/// can run on worker threads in `sim-cli play-bots`.
pub trait AiController: Send {
    /// Produce this tick's commands for `player` from a read-only view. The
    /// sim stamps them with the player's next sequence numbers and delays
    /// them by `cmd_delay_ticks` like any other command.
    fn think(&mut self, player: PlayerId, view: &SimView<'_>) -> Vec<Command>;
}
