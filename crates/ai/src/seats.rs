//! [`Seats`]: the one controller a `Sim` holds, dispatching each AI slot
//! to its own bot (M5b).

use sim::{AiController, AiMemory, Command, PlayerId, SimView};
use std::collections::BTreeMap;

/// One controller per AI slot, keyed by player. Built by
/// [`crate::controller_for`] from a header's `ai_seats`; the sim calls it
/// once per thinking AI player per tick, in id order, with that player's
/// memory.
pub struct Seats {
    by_player: BTreeMap<PlayerId, Box<dyn AiController>>,
}

impl Seats {
    /// Seats from a controller per player.
    pub fn new(by_player: BTreeMap<PlayerId, Box<dyn AiController>>) -> Seats {
        Seats { by_player }
    }

    /// The seated players, in id order.
    pub fn players(&self) -> impl Iterator<Item = PlayerId> + '_ {
        self.by_player.keys().copied()
    }
}

impl AiController for Seats {
    /// The seat's bot thinks; a player without a seat (which `Sim::new`
    /// rules out for AI slots) issues nothing.
    fn think(
        &mut self,
        player: PlayerId,
        view: &SimView<'_>,
        memory: &mut AiMemory,
    ) -> Vec<Command> {
        self.by_player
            .get_mut(&player)
            .map_or_else(Vec::new, |bot| bot.think(player, view, memory))
    }
}
