//! Commands: the only input to the simulation.
//!
//! A command issued during tick `N` is applied at tick `N + cmd_delay` in
//! every mode (local, replay, future lockstep). Before application the
//! commands of a tick are sorted by `(player, seq)`, a total order, so the
//! order in which they arrived never matters.

use crate::fx::FxVec2;
use crate::ids::{
    BuildingId, BuildingKindId, PlayerId, TargetId, TechId, Tile, UnitId, UnitKindId,
};
use serde::{Deserialize, Serialize};

/// A player's order. Validated at apply time; invalid orders are rejected
/// with an event (M1+) and never panic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    /// Move units to a point. `queue` appends to the current order list.
    Move {
        /// Units to move.
        units: Vec<UnitId>,
        /// Destination.
        target: FxVec2,
        /// Append instead of replace.
        queue: bool,
    },
    /// Move and engage anything hostile met on the way.
    AttackMove {
        /// Units to move.
        units: Vec<UnitId>,
        /// Destination.
        target: FxVec2,
    },
    /// Attack a specific target.
    Attack {
        /// Attacking units.
        units: Vec<UnitId>,
        /// Target.
        target: TargetId,
    },
    /// Halt and clear orders.
    Stop {
        /// Units to stop.
        units: Vec<UnitId>,
    },
    /// Send Yeomen to a gathering building.
    Gather {
        /// Workers.
        units: Vec<UnitId>,
        /// Croft, Lumber Yard, Ore Pit or Scriptorium.
        node: BuildingId,
    },
    /// Place a building with one worker.
    Build {
        /// Builder.
        unit: UnitId,
        /// Building kind.
        kind: BuildingKindId,
        /// North-west tile of the footprint.
        tile: Tile,
    },
    /// Cancel a queue slot (training or research) in a building.
    Cancel {
        /// Building.
        building: BuildingId,
        /// Queue slot index.
        slot: u8,
    },
    /// Queue a unit at a training building.
    Train {
        /// Training building.
        building: BuildingId,
        /// Unit kind.
        kind: UnitKindId,
    },
    /// Queue a tech at a Scriptorium or Watchtower.
    Research {
        /// Research building.
        building: BuildingId,
        /// Tech.
        tech: TechId,
    },
    /// Advance to the next age.
    AdvanceAge {
        /// Player advancing.
        player: PlayerId,
    },
    /// Set a building's rally point.
    SetRally {
        /// Building.
        building: BuildingId,
        /// Rally point.
        target: FxVec2,
    },
    /// Resign the match.
    Surrender,
}

/// A command stamped with its issuing player and per-player sequence number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerCommand {
    /// Issuing player.
    pub player: PlayerId,
    /// Monotonic per-player sequence number; the sort tiebreak.
    pub seq: u32,
    /// The order itself.
    pub cmd: Command,
}

impl PlayerCommand {
    /// Construct a stamped command.
    pub fn new(player: PlayerId, seq: u32, cmd: Command) -> PlayerCommand {
        PlayerCommand { player, seq, cmd }
    }

    /// The total-order key used before application.
    pub fn sort_key(&self) -> (PlayerId, u32) {
        (self.player, self.seq)
    }
}

/// Sort commands into application order: by player, then by sequence number.
/// The sort is stable, so two commands with an equal key (a client bug) keep
/// their arrival order; the sim does not depend on that case.
pub fn sort_commands(cmds: &mut [PlayerCommand]) {
    cmds.sort_by_key(PlayerCommand::sort_key);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_sort_total_order() {
        let mk = |p: u8, s: u32| PlayerCommand::new(PlayerId(p), s, Command::Surrender);
        let mut cmds = vec![mk(1, 2), mk(0, 9), mk(1, 0), mk(0, 1), mk(2, 0), mk(0, 0)];
        sort_commands(&mut cmds);
        let keys: Vec<_> = cmds.iter().map(PlayerCommand::sort_key).collect();
        assert_eq!(
            keys,
            vec![
                (PlayerId(0), 0),
                (PlayerId(0), 1),
                (PlayerId(0), 9),
                (PlayerId(1), 0),
                (PlayerId(1), 2),
                (PlayerId(2), 0),
            ]
        );
        // Total order: every pair is comparable and antisymmetric, and the
        // relation is transitive over the whole set.
        for a in &cmds {
            for b in &cmds {
                let ab = a.sort_key().cmp(&b.sort_key());
                let ba = b.sort_key().cmp(&a.sort_key());
                assert_eq!(ab, ba.reverse());
                for c in &cmds {
                    let bc = b.sort_key().cmp(&c.sort_key());
                    if ab != core::cmp::Ordering::Greater && bc != core::cmp::Ordering::Greater {
                        assert_ne!(
                            a.sort_key().cmp(&c.sort_key()),
                            core::cmp::Ordering::Greater
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn commands_roundtrip_through_postcard() {
        let cmd = PlayerCommand::new(
            PlayerId(1),
            7,
            Command::Build {
                unit: UnitId(3),
                kind: BuildingKindId(2),
                tile: Tile::new(10, 11),
            },
        );
        let bytes = postcard::to_allocvec(&cmd).unwrap();
        let back: PlayerCommand = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(cmd, back);
    }
}
