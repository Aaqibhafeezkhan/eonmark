//! Match state and the `Sim` that owns it.
//!
//! Only [`Sim::step`] mutates the match state. Everything inside it is hashed
//! and snapshotted; derived caches (none yet in M0; influence maps, spatial
//! grid and path cache arrive in M1 and M3a) live outside it and are rebuilt
//! by [`Sim::restore`].

use crate::ai_hook::{AiController, SimView};
use crate::command::{Command, PlayerCommand, sort_commands};
use crate::fx::FxVec2;
use crate::ids::{BuildingId, BuildingKindId, IdGen, PlayerId, Tile, UnitId, UnitKindId};
use rand_core::{Rng, SeedableRng};
use rules::Rules;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Bump when any change alters simulation results for the same inputs.
pub const SIM_VERSION: u32 = 1;

/// Who drives a player slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerSlot {
    /// Slot id.
    pub id: PlayerId,
    /// `true` when the injected [`AiController`] drives this slot.
    pub is_ai: bool,
}

/// Everything needed to reproduce a match from tick 0. Doubles as the replay
/// header and, later, the lockstep handshake payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchSetup {
    /// Hand-bumped whenever sim behaviour changes; `verify` fails fast on mismatch.
    pub sim_version: u32,
    /// Informational only; never compared.
    pub git_sha: String,
    /// Human label from `rules.ron`.
    pub rules_version: u32,
    /// Content hash of the loaded rules (see [`Rules::rules_hash`]).
    pub rules_hash: u64,
    /// Map identifier under `data/maps/`.
    pub map: String,
    /// Seed for the single `Pcg32` inside the sim.
    pub seed: u64,
    /// Ticks per second.
    pub tick_rate_hz: u32,
    /// Command delay in ticks.
    pub cmd_delay: u32,
    /// Player slots in id order.
    pub players: Vec<PlayerSlot>,
}

impl MatchSetup {
    /// A two-player skirmish on the rules' default map: slot 0 is human,
    /// slot 1 is driven by the AI controller.
    pub fn skirmish(rules: &Rules, seed: u64) -> Self {
        Self {
            sim_version: SIM_VERSION,
            git_sha: String::new(),
            rules_version: rules.rules_version,
            rules_hash: rules.rules_hash(),
            map: rules.default_map.clone(),
            seed,
            tick_rate_hz: rules.tick_rate_hz,
            cmd_delay: rules.cmd_delay_ticks,
            players: vec![
                PlayerSlot {
                    id: PlayerId(0),
                    is_ai: false,
                },
                PlayerSlot {
                    id: PlayerId(1),
                    is_ai: true,
                },
            ],
        }
    }
}

/// Per-player state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    /// Slot id.
    pub id: PlayerId,
    /// Driven by the AI controller.
    pub is_ai: bool,
    /// Resigned via [`Command::Surrender`].
    pub surrendered: bool,
    /// Next sequence number the sim stamps on this player's AI commands.
    pub next_ai_seq: u32,
}

/// A unit. M0 placeholder: fields are filled by M1 (movement) and M4a (combat).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unit {
    /// Id.
    pub id: UnitId,
    /// Owner.
    pub owner: PlayerId,
    /// Kind (index into `units.ron`, M3a).
    pub kind: UnitKindId,
    /// Position in tiles.
    pub pos: FxVec2,
}

/// A building. M0 placeholder: fields are filled by M3a (construction).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Building {
    /// Id.
    pub id: BuildingId,
    /// Owner.
    pub owner: PlayerId,
    /// Kind (index into `buildings.ron`, M3a).
    pub kind: BuildingKindId,
    /// North-west footprint tile.
    pub tile: Tile,
}

/// The whole match state. Hashable, snapshot-able, deterministic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct State {
    pub(crate) tick: u32,
    pub(crate) seed: u64,
    pub(crate) rng: rand_pcg::Pcg32,
    pub(crate) ids: IdGen,
    pub(crate) players: Vec<Player>,
    pub(crate) units: BTreeMap<UnitId, Unit>,
    pub(crate) buildings: BTreeMap<BuildingId, Building>,
    /// Commands waiting for their application tick, keyed by that tick.
    pub(crate) pending: BTreeMap<u32, Vec<PlayerCommand>>,
    /// Number of commands applied so far (M0 placeholder statistic).
    pub(crate) commands_applied: u64,
}

/// A snapshot could not be decoded into match state.
#[derive(Debug)]
pub struct SnapshotError(postcard::Error);

impl core::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "snapshot does not decode: {}", self.0)
    }
}

impl std::error::Error for SnapshotError {}

/// The simulation. Construct with [`Sim::new`], drive with [`Sim::step`].
pub struct Sim {
    setup: MatchSetup,
    rules: Rules,
    ai: Box<dyn AiController>,
    state: State,
}

impl Sim {
    /// Create a match. `ai` controls every slot with `is_ai == true`.
    pub fn new(setup: MatchSetup, rules: Rules, ai: Box<dyn AiController>) -> Self {
        let players = setup
            .players
            .iter()
            .map(|s| Player {
                id: s.id,
                is_ai: s.is_ai,
                surrendered: false,
                next_ai_seq: 0,
            })
            .collect();
        let state = State {
            tick: 0,
            seed: setup.seed,
            rng: rand_pcg::Pcg32::seed_from_u64(setup.seed),
            ids: IdGen::new(),
            players,
            units: BTreeMap::new(),
            buildings: BTreeMap::new(),
            pending: BTreeMap::new(),
            commands_applied: 0,
        };
        Self {
            setup,
            rules,
            ai,
            state,
        }
    }

    /// Advance exactly one tick.
    ///
    /// `cmds` are the commands issued *during* this tick; they are queued and
    /// applied at `tick + cmd_delay`, together with the AI's commands for this
    /// tick, after sorting by `(player, seq)`.
    pub fn step(&mut self, cmds: &[PlayerCommand]) {
        let tick = self.state.tick;
        let apply_at = tick + self.setup.cmd_delay;

        // 1. Queue the human commands issued this tick.
        if !cmds.is_empty() {
            self.state
                .pending
                .entry(apply_at)
                .or_default()
                .extend_from_slice(cmds);
        }

        // 2. Let the AI think over a read-only view, then queue its commands.
        let mut ai_batches: Vec<(PlayerId, Vec<Command>)> = Vec::new();
        {
            let view = SimView::new(&self.state, &self.rules);
            for player in &self.state.players {
                if player.is_ai && !player.surrendered {
                    let out = self.ai.think(player.id, &view);
                    if !out.is_empty() {
                        ai_batches.push((player.id, out));
                    }
                }
            }
        }
        for (pid, batch) in ai_batches {
            let player = self
                .state
                .players
                .iter_mut()
                .find(|p| p.id == pid)
                .expect("ai player exists");
            let queue = self.state.pending.entry(apply_at).or_default();
            for cmd in batch {
                queue.push(PlayerCommand::new(pid, player.next_ai_seq, cmd));
                player.next_ai_seq += 1;
            }
        }

        // 3. Apply everything due this tick in total order.
        if let Some(mut due) = self.state.pending.remove(&tick) {
            sort_commands(&mut due);
            for pc in due {
                self.apply(pc);
            }
        }

        // 4. Subsystems tick here from M1 (movement), M3a (economy), ...

        self.state.tick += 1;
    }

    /// Apply one command. M0 placeholder: `Surrender` is honoured; every other
    /// command is counted and advances the rng once so the hash depends on the
    /// command stream until the real handlers land (M1 movement, M3a economy).
    fn apply(&mut self, pc: PlayerCommand) {
        let Some(player) = self.state.players.iter_mut().find(|p| p.id == pc.player) else {
            return; // unknown slot: ignored, never a panic
        };
        if player.surrendered {
            return;
        }
        match pc.cmd {
            Command::Surrender => player.surrendered = true,
            _ => {
                self.state.rng.next_u32();
            }
        }
        self.state.commands_applied += 1;
    }

    /// Current tick (number of completed steps).
    pub fn tick(&self) -> u32 {
        self.state.tick
    }

    /// xxh3 over the canonical postcard encoding of all hashed state.
    pub fn hash(&self) -> u64 {
        crate::hash::hash_value(&self.state)
    }

    /// Serialize the full hashed state. Restoring it with [`Sim::restore`]
    /// reproduces this exact sim, including pending commands and the rng.
    pub fn snapshot(&self) -> Vec<u8> {
        postcard::to_allocvec(&self.state).expect("state serialises")
    }

    /// Replace the state with a snapshot taken by [`Sim::snapshot`] from a sim
    /// with the same setup and rules. Derived caches are rebuilt here before
    /// returning (none exist in M0). On error the sim is left unchanged.
    pub fn restore(&mut self, bytes: &[u8]) -> Result<(), SnapshotError> {
        let state: State = postcard::from_bytes(bytes).map_err(SnapshotError)?;
        self.state = state;
        self.rebuild_derived();
        Ok(())
    }

    /// Rebuild every unhashed derived structure from `state`. Empty in M0;
    /// M1 adds the spatial grid, connected components and path cache clear.
    fn rebuild_derived(&mut self) {}

    /// Read-only view of the current state.
    pub fn view(&self) -> SimView<'_> {
        SimView::new(&self.state, &self.rules)
    }

    /// The match setup this sim was created from.
    pub fn setup(&self) -> &MatchSetup {
        &self.setup
    }

    /// The rules this sim runs under.
    pub fn rules(&self) -> &Rules {
        &self.rules
    }
}
