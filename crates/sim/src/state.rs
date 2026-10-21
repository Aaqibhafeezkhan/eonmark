//! Match state and the `Sim` that owns it.
//!
//! Only [`Sim::step`] mutates the match state. Everything inside the private
//! `State` is hashed and snapshotted. Derived structures live outside it on
//! [`Sim`] or behind `#[serde(skip)]` and are rebuilt by the private
//! `Sim::rebuild_derived`, which [`Sim::restore`] calls before returning: the
//! map's connected components, the spatial grid, and the (cleared) path cache.
//!
//! Tick order inside [`Sim::step`]:
//!
//! 1. Queue the human commands issued this tick for `tick + cmd_delay`.
//! 2. Let the AI think over a read-only view; queue its commands likewise.
//! 3. Apply every command due this tick in `(player, seq)` order.
//! 4. Pathing: `Pathing::service` with `rules.path_budget_expansions`;
//!    completed paths are stored on the units' orders.
//! 5. Movement: rebuild the spatial grid, then `movement::step`; units that
//!    need a repath get a new `PathRequest`.
//! 6. Economy and Towns, each in id order:
//!    - 6a construction: sites whose builder is at work gain progress; a
//!      finished site completes (a Town or Watchtower adds its border, its
//!      Town may grow a tier) and its builder may take a slot there;
//!    - 6b training: every complete building's queue head gains a tick; a
//!      finished unit appears beside the building and walks to its rally
//!      point or takes a slot there;
//!    - 6c jobs: a job whose target is gone, or whose worker stopped
//!      somewhere it cannot work, is dropped; then idle workers past their
//!      timer take the nearest free slot;
//!    - 6d income: every player's base yields and worked slots go through
//!      the Yield Cap into the accumulators and stockpiles.
//! 7. `tick += 1`.

use crate::ai_hook::{AiController, SimView};
use crate::command::{Command, PlayerCommand, sort_commands};
use crate::construction::{self, BuildPlan};
use crate::economy::{self, Economy};
use crate::fx::{Fx, FxVec2};
use crate::gather;
use crate::ids::{BuildingId, BuildingKindId, IdGen, PlayerId, Tile, UnitId, UnitKindId};
use crate::map::Map;
use crate::movement::{self, SpatialGrid};
use crate::pathing::{PathRequest, Pathing};
use crate::territory::TerritoryField;
use crate::town;
use crate::training;
use rand_core::{Rng, SeedableRng};
use rules::{Rules, UnitKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Bump when any change alters simulation results for the same inputs.
///
/// 2: `Pathing::service` stops walking the queue once the tick's budget is
/// spent instead of parking a zero-expansion search as `active`.
///
/// 3: an arrived unit keeps a [`Post`] (`Unit::post`, new hashed state) and
/// walks straight back to it when pushed farther than its kind's
/// `return_to_post_radius_tiles_x100`.
///
/// 4: `Command::AttackMove` is applied exactly like `Command::Move` (M2
/// placeholder until combat in M4a) instead of being rejected with
/// `NotImplemented` and drawing from the RNG.
///
/// 5: a search suspended on an older cost grid is not resumed: `Pathing`
/// requeues its request at the head and searches the current grid
/// (`AStarSearch::generation`, new hashed state).
///
/// 6: M3a Town loop. New hashed state: stockpiles and accumulators
/// (`economy`), the territory field and the M3a building and unit fields.
/// The replay header (`MatchSetup`) grew by `start`; the reader peeks
/// `sim_version` first. The standard start (`StartKind::Standard`, the
/// skirmish default), `Build`, construction, site `Cancel`, the town limit,
/// borders on Town completion, base-yield income, `DebugPlaceBuilding` and
/// `DebugGrant`; `Build` and `Cancel` no longer draw from the RNG.
///
/// 7: a `Build` builder walks to the nearest ring tile it can reach with
/// the footprint blocked (`Unreachable` when there is none) instead of the
/// nearest passable one, which could be sealed off; resuming a site checks
/// the same, so it can no longer panic on an enclosed site. Standard-start
/// units spawn after every Seat is placed, on tiles not already taken.
///
/// 8: gathering. `Gather` assigns slots (it no longer draws from the RNG),
/// worked slots earn income, idle workers set `Unit::seek_at` and take free
/// slots on their own, a finished builder takes a slot at its building, and
/// a Town grows to tier 2 at enough distinct attributed kinds.
///
/// 9: training. `Train` queues units (cost and pop charged at queue time,
/// the Yeoman's per-unit ramp and the shared military ramp), the queue head
/// progresses in step 6b, `Cancel` refunds a queue entry, `SetRally` sets
/// where trained units walk. `Train` and `SetRally` no longer draw from the
/// RNG.
///
/// 10: a trained unit spawns in the component of the rally goal, or else of
/// the largest component touching the building's ring, instead of on the
/// first passable spiral tile, which could lie in a pocket sealed off by
/// neighbouring buildings.
///
/// 11: the spawn component is the rally point's only when the rally point's
/// own tile is passable (a rally on a building names no side), and with
/// every ring tile blocked the candidates are every component, not just the
/// first passable spiral tile's.
pub const SIM_VERSION: u32 = 11;

/// Who drives a player slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerSlot {
    /// Slot id.
    pub id: PlayerId,
    /// `true` when the injected [`AiController`] drives this slot.
    pub is_ai: bool,
}

/// What each player has at tick 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartKind {
    /// Nothing: no buildings, no units, empty stockpiles. Scripted
    /// scenarios and fixtures spawn what they need with debug commands.
    Empty,
    /// The standard start from `rules.ron` (`standard_start`): the Seat
    /// centred on each player's map start, the start units and stockpiles.
    Standard,
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
    /// Accept [`Command::DebugSpawn`]. `false` for real matches; `true` for
    /// fixtures, benches, tests and scenarios ([`MatchSetup::scenario`]).
    pub debug_commands: bool,
    /// What each player starts with. Last field: the header only grows at
    /// the end, and the replay reader checks `sim_version` before decoding.
    pub start: StartKind,
}

impl MatchSetup {
    /// A two-player skirmish on the rules' default map from the standard
    /// start: slot 0 is human, slot 1 is driven by the AI controller. Debug
    /// commands are rejected.
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
            debug_commands: false,
            start: StartKind::Standard,
        }
    }

    /// A scripted scenario on the default map: two human slots (no AI), an
    /// empty start, with debug commands accepted so tests, benches and
    /// fixtures can spawn units and place buildings. Set `start` to
    /// [`StartKind::Standard`] for a scenario from the standard start.
    pub fn scenario(rules: &Rules, seed: u64) -> Self {
        let mut s = MatchSetup::skirmish(rules, seed);
        s.players = vec![
            PlayerSlot {
                id: PlayerId(0),
                is_ai: false,
            },
            PlayerSlot {
                id: PlayerId(1),
                is_ai: false,
            },
        ];
        s.debug_commands = true;
        s.start = StartKind::Empty;
        s
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

/// A unit's current movement order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveOrder {
    /// Final destination (the unit's own spiral-offset target).
    pub goal: FxVec2,
    /// Waypoint tiles from the unit's start tile to the goal tile; empty
    /// while the path request is outstanding.
    pub path: Vec<Tile>,
    /// Index of the next waypoint in `path`.
    pub next: u32,
    /// Tick at which the periodic repath fires next.
    pub repath_at: u32,
}

/// Where an arrived unit stands: set when a [`MoveOrder`] completes, cleared
/// by a new order or a Stop. A unit pushed farther than its kind's
/// `return_to_post_radius_tiles_x100` from `goal` walks straight back
/// (`returning`) until it is within the arrive radius again; see
/// `movement::step`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Post {
    /// Centre of the final waypoint the unit arrived at (the
    /// component-corrected goal tile).
    pub goal: FxVec2,
    /// `true` while the unit is walking back after a displacement.
    pub returning: bool,
}

/// A unit. Combat fields arrive in M4a.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unit {
    /// Id.
    pub id: UnitId,
    /// Owner.
    pub owner: PlayerId,
    /// Kind (index into `units.ron`).
    pub kind: UnitKindId,
    /// Position in tiles.
    pub pos: FxVec2,
    /// Unit-length-ish facing vector (the renderer derives a heading).
    pub facing: FxVec2,
    /// Collision radius in tiles (from `radius_tiles_x100` at spawn).
    pub radius: Fx,
    /// Speed in tiles per tick (from `speed_tiles_per_s_x100 / tick_rate_hz`
    /// at spawn).
    pub speed: Fx,
    /// Current movement order, if any.
    pub order: Option<MoveOrder>,
    /// The spot an arrived unit holds, if any (`None` while it has an order
    /// or after a Stop).
    pub post: Option<Post>,
    /// What the unit works on, if anything. The single source of truth for
    /// who builds a site and who works a slot.
    pub job: Option<Job>,
    /// Tick at which an idle worker next looks for work; `None` while busy
    /// or not yet idle for a tick.
    pub seek_at: Option<u32>,
}

impl Unit {
    /// A new unit of `kind` standing at the centre of `tile`, facing east,
    /// with no order and no job; speed and radius are converted from the
    /// kind once, here.
    pub fn spawn(
        id: UnitId,
        owner: PlayerId,
        kind_id: UnitKindId,
        tile: Tile,
        kind: &UnitKind,
        map: &Map,
        tick_rate_hz: u32,
    ) -> Unit {
        Unit {
            id,
            owner,
            kind: kind_id,
            pos: map.center_of(tile),
            facing: FxVec2::from_ints(1, 0),
            radius: fx_from_x100(kind.radius_tiles_x100),
            speed: speed_per_tick(kind, tick_rate_hz),
            order: None,
            post: None,
            job: None,
            seek_at: None,
        }
    }
}

/// A unit's work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Job {
    /// Construct a site.
    Build {
        /// The site.
        site: BuildingId,
    },
    /// Work a gather slot.
    Gather {
        /// The gathering building.
        node: BuildingId,
        /// The slot, which fixes the worker's stand tile.
        slot: u8,
    },
}

/// Whether a building is still being built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Construction {
    /// A site: blocks its footprint, yields nothing, projects no border.
    Site {
        /// Ticks of work done; complete at the kind's build time.
        progress: u32,
    },
    /// Finished.
    Complete,
}

/// What only a Town has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TownState {
    /// The player's first Town, placed by the standard start. Never moves.
    pub seat: bool,
    /// Border tier: 1 when founded, 2 once grown.
    pub tier: u8,
}

/// One entry of a training (later also research) queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    /// What is being made.
    pub item: QueueItem,
    /// What was deducted when it was queued; `Cancel` refunds exactly this.
    pub paid: Vec<i64>,
    /// Ticks of work done (only the head progresses).
    pub progress: u32,
}

/// Something a building can queue. Research joins in M4a.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueItem {
    /// A unit of this kind.
    Unit(UnitKindId),
}

/// A building or a construction site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Building {
    /// Id.
    pub id: BuildingId,
    /// Owner.
    pub owner: PlayerId,
    /// Kind (index into `buildings.ron`).
    pub kind: BuildingKindId,
    /// North-west footprint tile.
    pub tile: Tile,
    /// Hit points (damage arrives in M4a).
    pub hp: i32,
    /// Site or complete.
    pub construction: Construction,
    /// What placing it cost, `resources.ron` order; a site `Cancel` refunds
    /// it. Zeros for the Seat and debug placement.
    pub paid: Vec<i64>,
    /// The Town it is attributed to, fixed at placement; `None` for Towns
    /// and when the owner had no complete Town.
    pub town: Option<BuildingId>,
    /// `Some` once a Town is complete.
    pub town_state: Option<TownState>,
    /// Worker slots, fixed at placement (0 for non-gatherers).
    pub slots: u8,
    /// Training queue, head first.
    pub queue: Vec<QueueEntry>,
    /// Where trained units walk, if set.
    pub rally: Option<FxVec2>,
}

/// Why a command was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    /// The issuing slot does not exist.
    UnknownPlayer,
    /// The player has surrendered.
    Surrendered,
    /// No unit in the command exists and belongs to the player.
    NoValidUnits,
    /// `Command::DebugSpawn` while `MatchSetup::debug_commands` is `false`.
    DebugCommandsDisabled,
    /// `UnitKindId` not in `units.ron`.
    UnknownUnitKind,
    /// The goal has no reachable tile: for `Build`, no tile around the
    /// footprint can be walked to by the builder (for a move, defensive;
    /// pathing step 1 prevents it).
    Unreachable,
    /// The command's handler arrives in a later milestone.
    NotImplemented,
    /// A footprint tile is not the player's territory.
    OutsideBorders,
    /// The player cannot pay; `resource` is the first short one, by index
    /// in `resources.ron`.
    CannotAfford {
        /// Index of the first resource the stockpile lacks.
        resource: u8,
    },
    /// The player already has as many Towns (sites included) as the town
    /// limit allows.
    TownLimitReached,
    /// The kind's per-Town limit is reached at the Town the building would
    /// belong to, or the player has no complete Town to attribute it to.
    PerTownLimitReached,
    /// `BuildingKindId` not in `buildings.ron`.
    UnknownBuildingKind,
    /// The building does not exist or is not the player's.
    UnknownBuilding,
    /// Resource index not in `resources.ron`.
    UnknownResource,
    /// The unit's kind cannot construct buildings.
    NotABuilder,
    /// The footprint leaves the map.
    SiteOutOfBounds,
    /// A footprint tile is impassable (terrain or another building), or no
    /// tile around it is free for the builder.
    SiteBlocked,
    /// A unit stands on the footprint.
    SiteOccupied,
    /// A gatherer that needs adjacent resource tiles has none.
    NoAdjacentResource,
    /// The queue slot does not exist (a site has only slot 0).
    InvalidSlot,
    /// The building is still a construction site.
    UnderConstruction,
    /// The building has no gather slots.
    NotAGatherNode,
    /// The unit would take the player over the pop cap (alive plus queued).
    PopCapReached,
    /// This building's kind does not train that unit kind.
    CannotTrainHere,
    /// The building's training queue is full (`train_queue_max`).
    QueueFull,
    /// Every gather slot of the building is taken. (A worker that cannot
    /// reach any tile beside the building is not a valid unit for it:
    /// `NoValidUnits`.)
    NoFreeSlot,
}

/// Something the presenter or a test may want to know about a tick. Events
/// are not hashed state; they are drained with [`Sim::drain_events`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SimEvent {
    /// A command was not applied.
    CommandRejected {
        /// Issuing player.
        player: PlayerId,
        /// The command's sequence number.
        seq: u32,
        /// Why.
        reason: RejectReason,
    },
    /// A unit was created by training or by `DebugSpawn`.
    UnitSpawned {
        /// The unit.
        unit: UnitId,
    },
    /// A unit completed its movement order.
    UnitArrived {
        /// The unit.
        unit: UnitId,
    },
    /// A unit's order was dropped because no path exists.
    PathUnreachable {
        /// The unit.
        unit: UnitId,
    },
    /// A construction site was placed (or a building by debug placement).
    BuildingPlaced {
        /// The building.
        building: BuildingId,
    },
    /// A site finished construction.
    BuildingCompleted {
        /// The building.
        building: BuildingId,
    },
    /// A building or site was removed (a cancelled site).
    BuildingRemoved {
        /// The building.
        building: BuildingId,
    },
    /// The territory field changed: a border source was added, moved or
    /// removed.
    TerritoryChanged,
    /// A training queue entry was cancelled and refunded.
    TrainingCancelled {
        /// The building.
        building: BuildingId,
        /// The queue slot it held.
        slot: u8,
    },
    /// A Town's border tier changed.
    TownTierChanged {
        /// The Town.
        town: BuildingId,
        /// Its new tier (1 or 2).
        tier: u8,
    },
}

/// Per-subsystem hashes for localising a divergence. Field order is the
/// comparison order of [`SubHashes::first_difference`]. Subsystems that do
/// not exist yet hash an empty placeholder so the layout is stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SubHashes {
    /// `BTreeMap<UnitId, Unit>`.
    pub units: u64,
    /// `BTreeMap<BuildingId, Building>`.
    pub buildings: u64,
    /// Stockpiles and accumulators.
    pub economy: u64,
    /// Territory field.
    pub territory: u64,
    /// Researched techs and ages (M4a); placeholder.
    pub tech: u64,
    /// Path request queue and the active search (not the cache).
    pub pathing: u64,
    /// The `Pcg32`.
    pub rng: u64,
    /// Cost grid and its generation.
    pub map: u64,
    /// Tick, seed, id allocator, players, pending commands, counters.
    pub meta: u64,
}

impl SubHashes {
    /// Names in field order.
    pub const NAMES: [&'static str; 9] = [
        "units",
        "buildings",
        "economy",
        "territory",
        "tech",
        "pathing",
        "rng",
        "map",
        "meta",
    ];

    /// Values in field order.
    pub fn values(&self) -> [u64; 9] {
        [
            self.units,
            self.buildings,
            self.economy,
            self.territory,
            self.tech,
            self.pathing,
            self.rng,
            self.map,
            self.meta,
        ]
    }

    /// Name of the first subsystem whose hash differs, or `None` when equal.
    pub fn first_difference(&self, o: &SubHashes) -> Option<&'static str> {
        self.values()
            .iter()
            .zip(o.values())
            .zip(SubHashes::NAMES)
            .find(|((a, b), _)| **a != *b)
            .map(|(_, name)| name)
    }
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
    /// Number of commands processed so far (applied or rejected).
    pub(crate) commands_applied: u64,
    /// Cost grid (derived components inside are `serde(skip)`).
    pub(crate) map: Map,
    /// Path request queue and active search (derived cache inside is `serde(skip)`).
    pub(crate) pathing: Pathing,
    /// Stockpiles and income accumulators.
    pub(crate) economy: Economy,
    /// Border strength per owner and the owner of every tile.
    pub(crate) territory: TerritoryField,
}

/// Placeholder hashed for subsystems that do not exist yet.
#[derive(Serialize)]
struct Empty;

/// A snapshot could not be decoded into match state.
#[derive(Debug)]
pub struct SnapshotError(postcard::Error);

impl core::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "snapshot does not decode: {}", self.0)
    }
}

impl std::error::Error for SnapshotError {}

/// Fixed-point value from a `_x100` rules field: `v / 100` tiles.
pub fn fx_from_x100(v: u32) -> Fx {
    Fx::from_ratio(
        i32::try_from(v).expect("x100 value validated <= 100000"),
        100,
    )
}

/// Speed in tiles per tick from `speed_tiles_per_s_x100` at `tick_rate_hz`.
pub fn speed_per_tick(kind: &UnitKind, tick_rate_hz: u32) -> Fx {
    let den = i32::try_from(100 * u64::from(tick_rate_hz)).expect("tick rate validated <= 1000");
    Fx::from_ratio(
        i32::try_from(kind.speed_tiles_per_s_x100).expect("x100 value validated <= 100000"),
        den,
    )
}

/// The simulation. Construct with [`Sim::new`], drive with [`Sim::step`].
pub struct Sim {
    setup: MatchSetup,
    rules: Rules,
    ai: Box<dyn AiController>,
    state: State,
    /// Derived: rebuilt every tick and on restore.
    grid: SpatialGrid,
    /// Whether the (derived) path cache is in use; survives restore.
    path_cache_enabled: bool,
    /// Not hashed; drained by the caller.
    events: Vec<SimEvent>,
}

impl Sim {
    /// Create a match. `ai` controls every slot with `is_ai == true`.
    /// Panics when `setup.map` is not in `rules` (the setup was built from
    /// different rules), or when the slots are not `PlayerId(0)`,
    /// `PlayerId(1)`, ... in order (stockpiles and territory layers are
    /// indexed by id).
    pub fn new(setup: MatchSetup, rules: Rules, ai: Box<dyn AiController>) -> Self {
        for (i, slot) in setup.players.iter().enumerate() {
            assert_eq!(
                usize::from(slot.id.0),
                i,
                "player slots must be PlayerId(0), PlayerId(1), ... in order"
            );
        }
        let players: Vec<Player> = setup
            .players
            .iter()
            .map(|s| Player {
                id: s.id,
                is_ai: s.is_ai,
                surrendered: false,
                next_ai_seq: 0,
            })
            .collect();
        let def = rules
            .map(&setup.map)
            .unwrap_or_else(|| panic!("map {:?} is not in the loaded rules", setup.map));
        let map = Map::from_def(def);
        let player_count = u8::try_from(players.len()).expect("at most 255 players");
        let territory = TerritoryField::new(map.width(), map.height(), player_count);
        let economy = Economy::new(players.len(), rules.resources.resources.len());
        let mut state = State {
            tick: 0,
            seed: setup.seed,
            rng: rand_pcg::Pcg32::seed_from_u64(setup.seed),
            ids: IdGen::new(),
            players,
            units: BTreeMap::new(),
            buildings: BTreeMap::new(),
            pending: BTreeMap::new(),
            commands_applied: 0,
            map,
            pathing: Pathing::new(rules.path_cache_entries, true),
            economy,
            territory,
        };
        if setup.start == StartKind::Standard {
            assert!(
                setup.players.len() <= def.starts.len(),
                "the map has a start for every player"
            );
            town::apply_standard_start(&mut state, &rules, def, rules.tick_rate_hz);
        }
        let grid = SpatialGrid::new(&state.map);
        Self {
            setup,
            rules,
            ai,
            state,
            grid,
            path_cache_enabled: true,
            events: Vec::new(),
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
            let view = SimView::new(&self.state, &self.rules, &self.setup);
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

        // 4. Pathing. Completed paths land on the units' orders. An empty
        //    path (start tile == corrected goal tile) becomes the unit's own
        //    tile so movement walks to its centre and arrives there instead
        //    of waiting forever.
        let completed =
            self.state
                .pathing
                .service(&self.state.map, self.rules.path_budget_expansions, tick);
        for (unit, path) in completed {
            let Some(u) = self.state.units.get_mut(&unit) else {
                continue;
            };
            match path {
                Some(path) => {
                    if let Some(order) = &mut u.order {
                        order.path = if path.is_empty() {
                            vec![self.state.map.tile_of(u.pos)]
                        } else {
                            path
                        };
                        order.next = 0;
                    }
                }
                None => {
                    u.order = None;
                    self.events.push(SimEvent::PathUnreachable { unit });
                }
            }
        }

        // 5. Movement. A unit that still has a request outstanding (queued
        //    or suspended) is not re-requested: a replacement would move it
        //    to the back of the queue and could starve it under a saturated
        //    budget. Its periodic timer is pushed back either way.
        self.grid.rebuild(&self.state.units);
        let repath = movement::step(
            &mut self.state.units,
            &self.state.map,
            &self.grid,
            &self.rules,
            tick,
            &mut self.events,
        );
        for unit in repath {
            let Some(u) = self.state.units.get(&unit) else {
                continue;
            };
            let Some(order) = &u.order else {
                continue;
            };
            if !self.state.pathing.has_request(unit) {
                let req = PathRequest {
                    requested_tick: tick,
                    unit,
                    from: self.state.map.tile_of(u.pos),
                    to: self.state.map.tile_of(order.goal),
                };
                self.state.pathing.request(req);
            }
            if let Some(order) = self
                .state
                .units
                .get_mut(&unit)
                .and_then(|u| u.order.as_mut())
            {
                order.repath_at = tick + self.rules.repath_interval_ticks();
            }
        }

        // 6. Economy and Towns: construction, training, jobs and idle
        //    seeking, income.
        construction::step(&mut self.state, &self.rules, tick, &mut self.events);
        training::step(&mut self.state, &self.rules, tick, &mut self.events);
        economy::jobs_step(&mut self.state, &self.rules, tick);
        economy::income_step(&mut self.state, &self.rules);

        self.state.tick += 1;
    }

    fn reject(&mut self, pc: &PlayerCommand, reason: RejectReason) {
        self.events.push(SimEvent::CommandRejected {
            player: pc.player,
            seq: pc.seq,
            reason,
        });
    }

    /// The subset of `ids` that exist and belong to `owner`, deduplicated,
    /// in ascending id order.
    fn owned_units(&self, owner: PlayerId, ids: &[UnitId]) -> Vec<UnitId> {
        ids.iter()
            .copied()
            .filter(|id| self.state.units.get(id).is_some_and(|u| u.owner == owner))
            .collect::<BTreeSet<UnitId>>()
            .into_iter()
            .collect()
    }

    /// Order the owned subset of `units` to `target`: one spiral-offset goal
    /// per unit, a path request each, posts cleared. Shared by `Move` and
    /// the M2 `AttackMove` placeholder. Rejects with `NoValidUnits` when no
    /// unit qualifies.
    fn apply_move(&mut self, pc: &PlayerCommand, units: &[UnitId], target: FxVec2) {
        let tick = self.state.tick;
        let movers = self.owned_units(pc.player, units);
        if movers.is_empty() {
            self.reject(pc, RejectReason::NoValidUnits);
            return;
        }
        let targets = movement::group_targets(&self.state.map, target, movers.len());
        let repath_at = tick + self.rules.repath_interval_ticks();
        for (unit, goal) in movers.into_iter().zip(targets) {
            let u = self.state.units.get_mut(&unit).expect("validated");
            u.order = Some(MoveOrder {
                goal,
                path: Vec::new(),
                next: 0,
                repath_at,
            });
            u.post = None;
            u.job = None;
            u.seek_at = None;
            let from = self.state.map.tile_of(u.pos);
            let to = self.state.map.tile_of(goal);
            self.state.pathing.request(PathRequest {
                requested_tick: tick,
                unit,
                from,
                to,
            });
        }
    }

    /// Apply one command. Every command, applied or rejected, increments
    /// `commands_applied` so the hash reflects the command stream.
    fn apply(&mut self, pc: PlayerCommand) {
        self.state.commands_applied += 1;
        let Some(player) = self.state.players.iter().find(|p| p.id == pc.player) else {
            self.reject(&pc, RejectReason::UnknownPlayer);
            return;
        };
        if player.surrendered {
            self.reject(&pc, RejectReason::Surrendered);
            return;
        }
        match &pc.cmd {
            Command::Surrender => {
                if let Some(p) = self.state.players.iter_mut().find(|p| p.id == pc.player) {
                    p.surrendered = true;
                }
            }
            Command::Move { units, target, .. } => {
                // `queue` is ignored in M1: a unit holds one order. Queued
                // orders arrive with the command card in M2.
                self.apply_move(&pc, units, *target);
            }
            Command::AttackMove { units, target } => {
                // M2 placeholder: identical to Move until combat (M4a) adds
                // target acquisition on the way. The sim must accept it so
                // the game's A-then-click order never produces a rejection.
                self.apply_move(&pc, units, *target);
            }
            Command::Stop { units } => {
                let stopped = self.owned_units(pc.player, units);
                if stopped.is_empty() {
                    self.reject(&pc, RejectReason::NoValidUnits);
                    return;
                }
                for unit in stopped {
                    if let Some(u) = self.state.units.get_mut(&unit) {
                        u.order = None;
                        u.post = None;
                        u.job = None;
                        u.seek_at = None;
                    }
                    self.state.pathing.cancel(unit);
                }
            }
            Command::DebugSpawn {
                owner,
                kind,
                at,
                count,
            } => {
                if !self.setup.debug_commands {
                    self.reject(&pc, RejectReason::DebugCommandsDisabled);
                    return;
                }
                if !self.state.players.iter().any(|p| p.id == *owner) {
                    self.reject(&pc, RejectReason::UnknownPlayer);
                    return;
                }
                let Some(kind_def) = self.rules.unit_kind(kind.0) else {
                    self.reject(&pc, RejectReason::UnknownUnitKind);
                    return;
                };
                let center = self.state.map.tile_of(*at);
                let tiles: Vec<Tile> = self
                    .state
                    .map
                    .spiral(center)
                    .filter(|t| self.state.map.passable(*t))
                    .take(usize::from(*count))
                    .collect();
                // One unit per passable tile; if the map runs out of tiles the
                // remaining units are not spawned.
                for tile in tiles {
                    let id = self.state.ids.unit();
                    let unit = Unit::spawn(
                        id,
                        *owner,
                        *kind,
                        tile,
                        kind_def,
                        &self.state.map,
                        self.rules.tick_rate_hz,
                    );
                    self.state.units.insert(id, unit);
                    self.events.push(SimEvent::UnitSpawned { unit: id });
                }
            }
            Command::Build { unit, kind, tile } => self.apply_build(&pc, *unit, *kind, *tile),
            Command::Cancel { building, slot } => self.apply_cancel(&pc, *building, *slot),
            Command::DebugPlaceBuilding {
                owner,
                kind,
                tile,
                complete,
            } => self.apply_debug_place(&pc, *owner, *kind, *tile, *complete),
            Command::DebugGrant {
                owner,
                resource,
                amount,
            } => {
                if !self.setup.debug_commands {
                    self.reject(&pc, RejectReason::DebugCommandsDisabled);
                    return;
                }
                if !self.state.players.iter().any(|p| p.id == *owner) {
                    self.reject(&pc, RejectReason::UnknownPlayer);
                    return;
                }
                let r = usize::from(*resource);
                if r >= self.rules.resources.resources.len() {
                    self.reject(&pc, RejectReason::UnknownResource);
                    return;
                }
                let mut amount_v = vec![0; self.rules.resources.resources.len()];
                amount_v[r] = i64::from(*amount);
                economy::refund(
                    &self.rules,
                    &mut self.state.economy.player_mut(*owner).stock,
                    &amount_v,
                );
            }
            Command::Gather { units, node } => {
                match gather::validate_gather(&self.state, &self.rules, pc.player, units, *node) {
                    Err(reason) => self.reject(&pc, reason),
                    Ok(plan) => {
                        let tick = self.state.tick;
                        gather::apply_gather(&mut self.state, &self.rules, *node, &plan, tick);
                    }
                }
            }
            Command::Train { building, kind } => {
                match training::validate_train(
                    &self.state,
                    &self.rules,
                    pc.player,
                    *building,
                    *kind,
                ) {
                    Err(reason) => self.reject(&pc, reason),
                    Ok(cost) => {
                        training::enqueue(&mut self.state, pc.player, *building, *kind, cost)
                    }
                }
            }
            Command::SetRally { building, target } => {
                match training::validate_rally(&self.state, pc.player, *building) {
                    Err(reason) => self.reject(&pc, reason),
                    Ok(()) => training::set_rally(&mut self.state, *building, *target),
                }
            }
            Command::Attack { .. } | Command::Research { .. } | Command::AdvanceAge { .. } => {
                // Handlers arrive in M4a. Until then the command is counted
                // and advances the rng once, as in M0, so the hash keeps
                // depending on the full command stream.
                self.state.rng.next_u32();
                self.reject(&pc, RejectReason::NotImplemented);
            }
        }
    }

    /// `Build`: validate (pure), then either resume an own site with this
    /// builder or deduct the cost, place a new site and send the builder.
    fn apply_build(&mut self, pc: &PlayerCommand, unit: UnitId, kind: BuildingKindId, tile: Tile) {
        let tick = self.state.tick;
        let def = self
            .rules
            .map(&self.setup.map)
            .expect("setup map is in the rules");
        match construction::validate_build(
            &self.state,
            &self.rules,
            def,
            pc.player,
            unit,
            kind,
            tile,
        ) {
            Err(reason) => self.reject(pc, reason),
            Ok(BuildPlan::Resume { site }) => {
                construction::assign_builder(&mut self.state, &self.rules, unit, site, tick);
            }
            Ok(BuildPlan::New { cost, slots, town }) => {
                economy::deduct(&mut self.state.economy.player_mut(pc.player).stock, &cost);
                let id = construction::place(
                    &mut self.state,
                    &self.rules,
                    pc.player,
                    kind,
                    tile,
                    cost,
                    slots,
                    town,
                );
                self.events.push(SimEvent::BuildingPlaced { building: id });
                construction::assign_builder(&mut self.state, &self.rules, unit, id, tick);
            }
        }
    }

    /// `Cancel`: slot 0 of an own site cancels it with a full refund; slot
    /// `i` of an own complete building's training queue cancels that entry
    /// and refunds what it paid. Anything else is `InvalidSlot`.
    fn apply_cancel(&mut self, pc: &PlayerCommand, building: BuildingId, slot: u8) {
        let Some(b) = self
            .state
            .buildings
            .get(&building)
            .filter(|b| b.owner == pc.player)
        else {
            self.reject(pc, RejectReason::UnknownBuilding);
            return;
        };
        match b.construction {
            Construction::Site { .. } if slot == 0 => {
                construction::cancel_site(&mut self.state, &self.rules, building, &mut self.events);
            }
            Construction::Complete if usize::from(slot) < b.queue.len() => {
                training::cancel_slot(
                    &mut self.state,
                    &self.rules,
                    building,
                    slot,
                    &mut self.events,
                );
            }
            _ => self.reject(pc, RejectReason::InvalidSlot),
        }
    }

    /// `DebugPlaceBuilding`: gated like `DebugSpawn`; skips borders, limits
    /// and cost, attributes like `Build`, and with `complete` finishes the
    /// building at once.
    fn apply_debug_place(
        &mut self,
        pc: &PlayerCommand,
        owner: PlayerId,
        kind: BuildingKindId,
        tile: Tile,
        complete: bool,
    ) {
        if !self.setup.debug_commands {
            self.reject(pc, RejectReason::DebugCommandsDisabled);
            return;
        }
        if !self.state.players.iter().any(|p| p.id == owner) {
            self.reject(pc, RejectReason::UnknownPlayer);
            return;
        }
        if self.rules.building_kind(kind.0).is_none() {
            self.reject(pc, RejectReason::UnknownBuildingKind);
            return;
        }
        let def = self
            .rules
            .map(&self.setup.map)
            .expect("setup map is in the rules");
        match construction::validate_debug_place(&self.state, &self.rules, def, owner, kind, tile) {
            Err(reason) => self.reject(pc, reason),
            Ok((slots, town)) => {
                let paid = vec![0; self.rules.resources.resources.len()];
                let id = construction::place(
                    &mut self.state,
                    &self.rules,
                    owner,
                    kind,
                    tile,
                    paid,
                    slots,
                    town,
                );
                self.events.push(SimEvent::BuildingPlaced { building: id });
                if complete {
                    construction::complete(&mut self.state, &self.rules, id, &mut self.events);
                }
            }
        }
    }

    /// Current tick (number of completed steps).
    pub fn tick(&self) -> u32 {
        self.state.tick
    }

    /// xxh3 over the canonical postcard encoding of all hashed state.
    pub fn hash(&self) -> u64 {
        crate::hash::hash_value(&self.state)
    }

    /// One xxh3 per subsystem (see [`SubHashes`]).
    pub fn sub_hashes(&self) -> SubHashes {
        let s = &self.state;
        SubHashes {
            units: crate::hash::hash_value(&s.units),
            buildings: crate::hash::hash_value(&s.buildings),
            economy: crate::hash::hash_value(&s.economy),
            territory: crate::hash::hash_value(&s.territory),
            tech: crate::hash::hash_value(&Empty),
            pathing: crate::hash::hash_value(&(&s.pathing.queue, &s.pathing.active)),
            rng: crate::hash::hash_value(&s.rng),
            map: crate::hash::hash_value(&s.map),
            meta: crate::hash::hash_value(&(
                s.tick,
                s.seed,
                &s.ids,
                &s.players,
                &s.pending,
                s.commands_applied,
            )),
        }
    }

    /// Serialize the full hashed state. Restoring it with [`Sim::restore`]
    /// reproduces this exact sim, including pending commands, the rng, the
    /// cost grid and any suspended path search.
    pub fn snapshot(&self) -> Vec<u8> {
        postcard::to_allocvec(&self.state).expect("state serialises")
    }

    /// Replace the state with a snapshot taken by [`Sim::snapshot`] from a sim
    /// with the same setup and rules, then rebuild every derived structure
    /// before returning. On error the sim is left unchanged.
    pub fn restore(&mut self, bytes: &[u8]) -> Result<(), SnapshotError> {
        let state: State = postcard::from_bytes(bytes).map_err(SnapshotError)?;
        self.state = state;
        self.rebuild_derived();
        Ok(())
    }

    /// Rebuild every unhashed derived structure from `state`: map components,
    /// spatial grid, and the path cache (empty, with its capacity and the
    /// sim's enabled flag re-applied since `serde(skip)` reset it).
    fn rebuild_derived(&mut self) {
        self.state.map.rebuild_components();
        self.grid = SpatialGrid::new(&self.state.map);
        self.grid.rebuild(&self.state.units);
        self.state.pathing.cache =
            crate::pathing::PathCache::new(self.rules.path_cache_entries, self.path_cache_enabled);
        self.events.clear();
    }

    /// Take every event emitted since the last drain, in emission order.
    pub fn drain_events(&mut self) -> Vec<SimEvent> {
        std::mem::take(&mut self.events)
    }

    /// Enable or disable the path cache (the transparency test flips it).
    /// Disabling clears it. Results never depend on this setting.
    pub fn set_path_cache_enabled(&mut self, enabled: bool) {
        self.path_cache_enabled = enabled;
        self.state.pathing.set_cache_enabled(enabled);
    }

    /// Whether the path cache is in use.
    pub fn path_cache_enabled(&self) -> bool {
        self.path_cache_enabled
    }

    /// Read-only view of the current state.
    pub fn view(&self) -> SimView<'_> {
        SimView::new(&self.state, &self.rules, &self.setup)
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
