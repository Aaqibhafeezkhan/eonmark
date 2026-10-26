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
    /// Send workers to a gathering building (handled from the next M3a
    /// slice).
    Gather {
        /// Workers.
        units: Vec<UnitId>,
        /// Croft, Lumber Yard, Ore Pit or Scriptorium.
        node: BuildingId,
    },
    /// Place a construction site and make `unit` its builder, or, when an
    /// own site of `kind` already stands at `tile`, make `unit` its builder
    /// at no cost. Every footprint tile must be the player's territory.
    Build {
        /// Builder.
        unit: UnitId,
        /// Building kind.
        kind: BuildingKindId,
        /// North-west tile of the footprint.
        tile: Tile,
    },
    /// Cancel a queue slot (training or research) in a building, or a
    /// construction site with slot 0 (full refund).
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
    /// Queue a tech at a building of its `host` kind (every M4a tech is
    /// researched at a Scriptorium). Cost and time are fixed when queued.
    Research {
        /// Research building.
        building: BuildingId,
        /// Tech (index into `techs.ron`).
        tech: TechId,
    },
    /// Advance to the next age at once.
    AdvanceAge {
        /// Player advancing; must be the issuing player
        /// (`RejectReason::PlayerMismatch` otherwise).
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
    /// Spawn `count` units of `kind` for `owner` on the nearest passable
    /// tiles in a square spiral around `at`. Accepted only when
    /// `MatchSetup::debug_commands` is `true` (fixtures, benches, tests,
    /// scenarios); rejected with `RejectReason::DebugCommandsDisabled`
    /// otherwise.
    DebugSpawn {
        /// Owning player.
        owner: PlayerId,
        /// Unit kind (index into `units.ron`).
        kind: UnitKindId,
        /// Spiral centre.
        at: FxVec2,
        /// Units to spawn.
        count: u16,
    },
    /// Place a building of `kind` for `owner` with its north-west corner at
    /// `tile`, as a site or, with `complete`, finished at once. Skips
    /// borders, limits and cost; the footprint must still be on the map,
    /// passable and free of units. Gated like `DebugSpawn`.
    DebugPlaceBuilding {
        /// Owning player.
        owner: PlayerId,
        /// Building kind (index into `buildings.ron`).
        kind: BuildingKindId,
        /// North-west footprint tile.
        tile: Tile,
        /// Finish it at once (a border source projects immediately).
        complete: bool,
    },
    /// Add `amount` of resource `resource` (index into `resources.ron`) to
    /// `owner`'s stockpile, clamped to its `stockpile_cap`. Gated like
    /// `DebugSpawn`.
    DebugGrant {
        /// Receiving player.
        owner: PlayerId,
        /// Resource index.
        resource: u8,
        /// Amount added.
        amount: u32,
    },
    /// Complete `tech` for `owner` at once, skipping its host, cost, time,
    /// prerequisite and age (M4a). Rejected when the tech is researched or
    /// queued already, so a tech never completes twice. Gated like
    /// `DebugSpawn`.
    DebugResearch {
        /// Receiving player.
        owner: PlayerId,
        /// Tech (index into `techs.ron`).
        tech: TechId,
    },
    /// Spawn like [`Command::DebugSpawn`], then move each new unit, in id
    /// order, off its tile centre by a seeded offset: `dx` then `dy`, each
    /// drawn from the sim's RNG uniformly in `-jitter_x100..=jitter_x100`
    /// hundredths of a tile. A point off the map, on a blocked tile or in
    /// another component than its tile keeps the tile centre (the draws are
    /// made anyway). The seeded spawn jitter of the M4a counter tests and
    /// fixtures. Gated like `DebugSpawn`; a rejection draws nothing.
    DebugSpawnJittered {
        /// Owning player.
        owner: PlayerId,
        /// Unit kind (index into `units.ron`).
        kind: UnitKindId,
        /// Spiral centre.
        at: FxVec2,
        /// Units to spawn.
        count: u16,
        /// Largest offset per axis, in hundredths of a tile.
        jitter_x100: u8,
    },
    /// Deal `amount` damage to `target` at once (M5a), as a hit would but
    /// with no attacker, range or counter: a unit's damage grows, a
    /// complete Town falls to at most 0 (`TownAtZeroHp`), any other
    /// building or site loses hit points; step 5c removes the dead and the
    /// destroyed. Any owner's target, the issuer's own included; rejected
    /// with `RejectReason::InvalidTarget` when it does not exist. Draws
    /// nothing from the RNG. Lets tests and scenarios set hit points
    /// without depending on combat tuning. Gated like `DebugSpawn`.
    DebugDamage {
        /// The unit or building.
        target: TargetId,
        /// Damage dealt.
        amount: u32,
    },
    /// Move `owner` into the next age at once (M5b), skipping the age's
    /// cost and its `techs_required`, as `AdvanceAge` would otherwise:
    /// `AgeAdvanced` is emitted and `owner`'s Towns are re-derived.
    /// Rejected with `RejectReason::UnknownPlayer` for a missing owner and
    /// `RejectReason::FinalAge` in the last age. Lets the bot bench and
    /// tests reach an age without depending on a build order. Gated like
    /// `DebugSpawn`; draws nothing.
    DebugAdvanceAge {
        /// The advancing player.
        owner: PlayerId,
    },
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
