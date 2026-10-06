//! The hook through which scripted opponents run *inside* [`crate::Sim::step`].
//!
//! Running the AI inside the step means bot-vs-bot replays and future
//! lockstep peers agree on every decision: the bot sees only the hashed
//! state, draws no randomness of its own, and its commands go through the
//! same delay queue as a human's.

use crate::command::Command;
use crate::ids::PlayerId;
use crate::state::{Building, Player, State, Unit};
use rules::Rules;

/// Read-only view of the sim handed to AI controllers and the presenter.
pub struct SimView<'a> {
    /// Current tick.
    pub tick: u32,
    state: &'a State,
    rules: &'a Rules,
}

impl<'a> SimView<'a> {
    pub(crate) fn new(state: &'a State, rules: &'a Rules) -> SimView<'a> {
        SimView {
            tick: state.tick,
            state,
            rules,
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
