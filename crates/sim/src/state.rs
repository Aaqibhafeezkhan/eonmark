//! Match state and the `Sim` that owns it.
//!
//! Only [`Sim::step`] mutates the match state. Everything inside the private
//! `State` is hashed and snapshotted. Derived structures live outside it on
//! [`Sim`] or behind `#[serde(skip)]` and are rebuilt by the private
//! `Sim::rebuild_derived`, which [`Sim::restore`] calls before returning: the
//! map's connected components, the spatial grid, the rules-only combat
//! tables, and the (cleared) path cache.
//!
//! Tick order inside [`Sim::step`]:
//!
//! 1. Queue the human commands issued this tick for `tick + cmd_delay`.
//! 2. Let the AI think over a read-only view; queue its commands likewise.
//! 3. Apply every command due this tick in `(player, seq)` order.
//! 4. Pathing: `Pathing::service` with `rules.path_budget_expansions`;
//!    completed paths are stored on the units' orders. An unreachable goal
//!    clears the unit's order and any combat intent.
//! 5. Movement and combat: rebuild the spatial grid (start-of-tick
//!    positions), then
//!    - 5a `combat::think`: each armed unit in id order: one with an
//!      explicit `Attack` holds, chases its target directly, walks a path
//!      toward it or gives up; an idle guard or an attack-mover keeps,
//!      switches or acquires a target (seeded draws break exact ties) and
//!      holds or chases it directly; then each ready Watchtower and Town,
//!      in id order, aims at the nearest enemy unit in range;
//!    - 5b `movement::step` with those engagements; units that need a
//!      repath get a new `PathRequest`;
//!    - 5b' `attrition::step` (M5a): each unit in id order on another
//!      player's land with Harrying and no friendly Supply Wain in range
//!      counts one more exposed tick, any other unit's count drops to 0,
//!      and a count that reaches a multiple of the owner's interval takes
//!      a pulse of hit points;
//!    - 5c `combat::resolve`: hits from the post-movement positions (units,
//!      then the shooters' aims), summed per target and applied at once,
//!      then the dead units (attrition's included) and the destroyed
//!      buildings are removed;
//!    - 5d `annexation::step` (M5a): each complete Town at 0 HP, in id
//!      order, with exactly one enemy owner's infantry or cavalry beside it
//!      and no defender within 6 tiles starts or advances its timer (its
//!      border source on the neutral layer); any other step resets it; a
//!      timer that reaches `annexation_ds` flips the Town and the buildings
//!      attributed to it that do not stay. The annex-hold guard rule lives
//!      in 5a: an idle infantry or cavalry guard beside an enemy Town at 0
//!      HP takes only targets in weapon range.
//! 6. Economy and Towns, each in id order:
//!    - 6a construction: sites whose builder is at work gain progress; a
//!      finished site completes (a Town or Watchtower adds its border, its
//!      Town may grow a tier) and its builder may take a slot there;
//!    - 6b training and research: every complete building's queue head
//!      gains a tick (not at a Town whose annexation timer runs, M5a); a
//!      finished unit appears beside the building and walks
//!      to its rally point or takes a slot there, a finished tech applies
//!      its rows and re-derives the owner's Towns;
//!    - 6c jobs: a job whose target is gone, or whose worker stopped
//!      somewhere it cannot work, is dropped; then idle workers past their
//!      timer take the nearest free slot;
//!    - 6d income: every player's base yields and worked slots go through
//!      the Yield Cap (less the late-game decay, M5a, off in the shipped
//!      data) into the accumulators and stockpiles;
//!    - 6e `victory::step` (M5a): while no outcome is set, Surrender, a
//!      Seat flipped in 5d, a player left with no complete Town by a 5d
//!      flip, then the match cap (territory) decide it, once.
//! 7. `tick += 1`.

use crate::ai_hook::{AiController, AiMemory, SimView};
use crate::annexation;
use crate::attrition;
use crate::combat;
use crate::command::{Command, PlayerCommand, sort_commands};
use crate::construction::{self, BuildPlan};
use crate::economy::{self, Economy};
use crate::fx::{Fx, FxVec2};
use crate::gather;
use crate::ids::{
    BuildingId, BuildingKindId, IdGen, PlayerId, TargetId, TechId, Tile, UnitId, UnitKindId,
};
use crate::map::Map;
use crate::movement::{self, SpatialGrid};
use crate::pathing::{PathRequest, Pathing};
use crate::tech::{self, TechState};
use crate::territory::TerritoryField;
use crate::town;
use crate::training;
use crate::victory;
use rand_core::SeedableRng;
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
///
/// 12: M4a tech. New hashed state `State.tech` (ages and researched
/// techs, sub-hash `tech`). `Research` and `AdvanceAge` have real handlers
/// and no longer draw from the RNG; a research entry
/// (`QueueItem::Research`) fixes its cost and time when it is queued and
/// completes in step 6b. The yield cap, pop cap, town limit and Town
/// radius are resolved from the faction's and the researched techs'
/// Modifier rows; a Town's radius is re-derived on research, age advance
/// and completion; tier 2 needs `town_growth_age` (the Masonry Age).
/// `Train` and `Build` check the age (and `Train` the kind's
/// `requires_tech`). `DebugResearch` is appended to `Command`.
///
/// 13: M4a combat core. New hashed state: `Unit::damage_taken`,
/// `ready_at`, `target` and `intent` (sub-hash `units`) and
/// `Building::ready_at` (`buildings`). `Attack` has a real handler
/// (`InvalidTarget`, `NoValidUnits`; `NotImplemented` is gone) and no
/// longer draws from the RNG, so no rejection draws. Step 5 runs
/// `combat::think` before movement (an explicit attacker holds, chases its
/// target directly or along a path, or gives up; no leash) and
/// `combat::resolve` after it: hits from the post-movement positions,
/// summed per target, then the dead are removed and buildings at 0 HP
/// destroyed, sites of every kind included, while a complete Town stays at
/// 0 HP. The Arms `UnitHp` and `UnitDamage` rows apply.
///
/// 14: M4a combat behaviour. Idle armed units (guards, with or without a
/// post) and attack-movers acquire targets in `combat::think`: enemy units
/// before enemy buildings (the Mangonel the reverse), in weapon range
/// before out of it, then by `dist_sq`, with a seeded `rng_index` draw
/// among exact ties; a target out of weapon range gives way to one of its
/// tier in range, and a building target to a unit. `AttackMove` is no
/// longer a `Move`: armed units get `Intent::AttackMove`, engage on the
/// way, keep their order and resume it, and guard once it is gone.
/// Complete Watchtowers and Towns with hit points left shoot the nearest
/// enemy unit in range (`Building::ready_at`). `DebugSpawnJittered` is
/// appended to `Command` and draws two offsets per spawned unit.
///
/// 15: M5a attrition. New hashed state `Unit::exposed_ticks` (sub-hash
/// `units`). Step 5b' applies Harrying (the `HarryingLevel` rows: a pulse
/// every 64 ticks at Harrying I, the interval halved per level), cancelled
/// by the unit owner's Supply Wains within `supply_radius_tiles` (14),
/// skipped for `immune_to_attrition` kinds, with the age-gap table and the
/// late Harrying bonus; its dead are reaped in 5c. `Command::DebugDamage`
/// is appended to `Command`, and `combat::apply_damage` applies both its
/// damage and every hit. `AdvanceAge` counts only the techs that count
/// toward an age from `rules_version` 18 on (behaviour-neutral for older
/// data, which has no side tech).
///
/// 16: M5a annexation and outcome. New hashed state `TownState::hold`
/// (sub-hash `buildings`) and `State::outcome` (`meta`). Step 5d enters,
/// resets and flips Towns at 0 HP: one enemy owner's adjacent infantry or
/// cavalry and no defender within 6 tiles move the Town's border source to
/// the neutral layer, and after `annexation_ds` (1200 ticks) the Town and
/// the buildings attributed to it that do not stay change hands (queues
/// refunded to the old owner, sites' `paid` zeroed, border sources moved to
/// the new owner's layer, 250 per mille of the Town's hit points
/// restored). `Train` and `Research` at a contested Town
/// are rejected with `TownContested` and its queue pauses; new buildings
/// are not attributed to it. The annex-hold guard rule: an idle infantry
/// or cavalry guard adjacent to an enemy Town at 0 HP keeps and takes only
/// targets in weapon range and never chases. Step 6e decides the outcome
/// (Surrender, Seat captured, all Towns lost and the 45-minute territory
/// cap, with a Draw for an exact tie). The Yield Cap decay knob
/// (`yield_cap_decay_*`, off in the shipped data).
///
/// 17: M5b bots: `MatchSetup.ai_seats` (header, last field); hashed
/// `Player::ai_memory` (`meta`); `AiController::think` receives its own
/// memory and every other bot's is hidden; `Command::DebugAdvanceAge`.
/// From 17 on, a `crates/ai` change that can alter a bot decision bumps
/// this number (and, through the fixture policy, `rules_version`).
pub const SIM_VERSION: u32 = 17;

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
    /// What each player starts with.
    pub start: StartKind,
    /// Which bot drives each AI slot (M5b): exactly the `is_ai` slots, once
    /// each, in id order ([`Sim::new`] asserts it). Last field: the header
    /// only grows at the end, and the replay reader checks `sim_version`
    /// before decoding.
    pub ai_seats: Vec<AiSeat>,
}

/// The bot behind one AI slot (M5b), named in the replay header so every
/// re-simulation rebuilds the same controller (`ai::controller_for`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiSeat {
    /// The AI slot.
    pub player: PlayerId,
    /// The bot: `passive` or `standard` (M6 adds `easy` and `hard`); the
    /// difficulty id of docs/design/ai.md.
    pub bot: String,
    /// A `data/ai/personalities.ron` id; empty for `passive`.
    pub personality: String,
}

impl MatchSetup {
    /// A two-player skirmish on the rules' default map from the standard
    /// start: slot 0 is human, slot 1 is driven by the AI controller, which
    /// the header names `passive` (M5b; see [`MatchSetup::vs_bot`] for a
    /// real opponent). Debug commands are rejected.
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
            ai_seats: vec![AiSeat {
                player: PlayerId(1),
                bot: "passive".to_string(),
                personality: String::new(),
            }],
        }
    }

    /// A skirmish against a bot (M5b): slot 0 human, slot 1 driven by
    /// `bot` with `personality` (empty for `passive`), the standard start,
    /// no debug commands.
    pub fn vs_bot(rules: &Rules, seed: u64, bot: &str, personality: &str) -> Self {
        let mut s = MatchSetup::skirmish(rules, seed);
        s.ai_seats = vec![AiSeat {
            player: PlayerId(1),
            bot: bot.to_string(),
            personality: personality.to_string(),
        }];
        s
    }

    /// A bot-versus-bot match (M5b): both slots AI, slot 0 driven by
    /// `a = (bot, personality)` and slot 1 by `b`, the standard start, no
    /// debug commands. `sim-cli play-bots` and the game's `--spectate`.
    pub fn bots(rules: &Rules, seed: u64, a: (&str, &str), b: (&str, &str)) -> Self {
        let mut s = MatchSetup::skirmish(rules, seed);
        s.players = vec![
            PlayerSlot {
                id: PlayerId(0),
                is_ai: true,
            },
            PlayerSlot {
                id: PlayerId(1),
                is_ai: true,
            },
        ];
        s.ai_seats = [a, b]
            .iter()
            .enumerate()
            .map(|(i, (bot, personality))| AiSeat {
                player: PlayerId(u8::try_from(i).expect("two slots")),
                bot: (*bot).to_string(),
                personality: (*personality).to_string(),
            })
            .collect();
        s
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
        s.ai_seats = Vec::new();
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
    /// The bot's memory between plans (M5b), hashed in `meta`. Empty for a
    /// human slot. Step 2 hands it to the bot alone: while the bots think,
    /// every player's memory reads empty in the view.
    pub ai_memory: AiMemory,
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

/// What a unit means to do about combat (`docs/design/combat.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Intent {
    /// No combat order.
    #[default]
    Plain,
    /// Attack-moving toward `goal` (the unit's own spiral goal), set by
    /// `AttackMove` for a unit with an attack: it acquires enemies on the
    /// way and engages them while keeping its order, which it resumes
    /// afterwards. The intent ends with the order (arrival, an unreachable
    /// path or a new command); the unit then guards where it stands.
    AttackMove {
        /// The unit's goal.
        goal: FxVec2,
    },
    /// Attacking `Unit::target`, ordered by `Command::Attack`: chased with
    /// no leash until it dies, is destroyed, becomes invalid or the unit
    /// gives up on an unreachable target.
    Attack,
}

/// A unit.
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
    /// Damage taken so far. The unit's hit points are its kind's maximum,
    /// resolved through the owner's `UnitHp` rows every tick, minus this,
    /// so an Arms level raises a living unit's hit points too; the unit
    /// dies once this reaches the maximum.
    pub damage_taken: i32,
    /// First tick at which the unit may hit again (the cooldown).
    pub ready_at: u32,
    /// What the unit attacks, if anything.
    pub target: Option<TargetId>,
    /// Why it has that target.
    pub intent: Intent,
    /// Consecutive steps the unit has been exposed to attrition (M5a,
    /// saturating): on another player's land, where that player has a
    /// Harrying level, with no own Supply Wain in range and a kind that is
    /// not immune. Any other step sets it to 0. A pulse lands when it is a
    /// multiple of the owner's interval; the game reads `> 0` as
    /// "bleeding".
    pub exposed_ticks: u32,
}

impl Unit {
    /// A new unit of `kind` standing at the centre of `tile`, facing east,
    /// with no order, no job and no damage taken; speed and radius are
    /// converted from the kind once, here.
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
            damage_taken: 0,
            ready_at: 0,
            target: None,
            intent: Intent::Plain,
            exposed_ticks: 0,
        }
    }

    /// Forget any combat order: no target, [`Intent::Plain`]. Every command
    /// that gives the unit something else to do (`Move`, `AttackMove`,
    /// `Stop`, `Gather`, `Build`) calls it.
    pub(crate) fn disengage(&mut self) {
        self.target = None;
        self.intent = Intent::Plain;
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
    /// The player's first Town, placed by the standard start. Never moves,
    /// and stays on the building when it is annexed (M5a).
    pub seat: bool,
    /// Border tier: 1 when founded, 2 once grown.
    pub tier: u8,
    /// Whether the Town's annexation timer runs (M5a, step 5d).
    pub hold: Hold,
}

/// A Town's annexation state (`docs/design/towns.md`, "Annexation state
/// machine"; M5a step 5d, `annexation::step`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Hold {
    /// Its owner holds it: its border source stamps the owner's layer.
    #[default]
    Held,
    /// At 0 HP with one enemy owner's infantry or cavalry adjacent and no
    /// defender near: its border source stamps the neutral layer, `Train`
    /// and `Research` there are rejected (`TownContested`) and its queue
    /// pauses. It flips to `attacker` once `ticks` reaches
    /// `Rules::annexation_ticks()`.
    Contested {
        /// The one enemy owner whose units are adjacent.
        attacker: PlayerId,
        /// Eligible steps since the step that entered the timer (0 in that
        /// step).
        ticks: u32,
    },
}

/// How a match ended (M5a, step 6e, `victory::step`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// `player` won: decisively by capture, elimination or surrender, or
    /// not decisively on territory at the match cap.
    Victory {
        /// The winner.
        player: PlayerId,
        /// `false` only for the territory tiebreak at the match cap.
        decisive: bool,
    },
    /// Nobody won: an exact territory tie at the cap, or a symmetric
    /// same-step capture, elimination or double surrender.
    Draw,
}

/// Why a match ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutcomeReason {
    /// A Seat was annexed.
    SeatCaptured,
    /// A player's last complete Town was annexed.
    AllTownsLost,
    /// Every player but one (or all) surrendered.
    Surrender,
    /// The match cap (`match_cap_ds`, 45 minutes) was reached: the player
    /// with the most owned tiles wins.
    Territory,
}

/// The decided result of a match: latched in `State::outcome` (sub-hash
/// `meta`) in the step that decides it and never changed afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchResult {
    /// Who won, or a draw.
    pub outcome: Outcome,
    /// Which condition decided it.
    pub reason: OutcomeReason,
    /// `Sim::tick()` after the deciding step.
    pub tick: u32,
}

/// One entry of a building's queue: a unit in training or a tech in
/// research.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    /// What is being made.
    pub item: QueueItem,
    /// What was deducted when it was queued; `Cancel` refunds exactly this.
    pub paid: Vec<i64>,
    /// Ticks of work done (only the head progresses).
    pub progress: u32,
}

/// Something a building can queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueItem {
    /// A unit of this kind.
    Unit(UnitKindId),
    /// A tech, researched at its host (M4a).
    Research {
        /// The tech.
        tech: TechId,
        /// Research time in ticks, fixed when queued (the Letters rows
        /// researched by then apply).
        ticks: u32,
    },
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
    /// Hit points: the kind's `hp` at placement, sites included, less the
    /// damage taken. A building other than a complete Town is destroyed at
    /// 0 or below; a complete Town stays, clamped at 0.
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
    /// Training and research queue, head first.
    pub queue: Vec<QueueEntry>,
    /// Where trained units walk, if set.
    pub rally: Option<FxVec2>,
    /// First tick at which the building may shoot again (a complete
    /// Watchtower or Town with hit points left shoots); 0 at placement.
    pub ready_at: u32,
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
    /// `TechId` not in `techs.ron`.
    UnknownTech,
    /// The building's kind is not the tech's `host`.
    CannotResearchHere,
    /// `Research`: the level below the tech in its line is not researched
    /// yet. `Train`: the unit kind's `requires_tech` is not researched yet.
    PrerequisiteMissing,
    /// The player has not reached the age (`ages.ron`) that unlocks the
    /// tech, unit kind or building kind.
    AgeRequired,
    /// The player has researched the tech already.
    AlreadyResearched,
    /// The tech is queued at one of the player's buildings.
    ResearchInProgress,
    /// `AdvanceAge` before the player has researched the next age's
    /// `techs_required` techs (queued ones and the Harrying side techs,
    /// `counts_toward_age: false`, do not count).
    NotEnoughTechs,
    /// `AdvanceAge` in the last age of `ages.ron`.
    FinalAge,
    /// `AdvanceAge { player }` names a player other than the issuer.
    PlayerMismatch,
    /// `Attack`: the target does not exist, is the player's own, or is a
    /// building with no hit points left (a complete Town at 0 HP).
    /// `DebugDamage`: the target does not exist.
    InvalidTarget,
    /// `Train` or `Research` at a Town whose annexation timer runs
    /// (M5a): its queue is paused until the timer resets or it flips.
    TownContested,
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
    /// A player completed a tech (research or `DebugResearch`).
    ResearchCompleted {
        /// The player.
        player: PlayerId,
        /// The tech.
        tech: TechId,
    },
    /// A player entered the next age.
    AgeAdvanced {
        /// The player.
        player: PlayerId,
        /// The age entered (index into `ages.ron`).
        age: u8,
    },
    /// An attack landed. Every hit of a tick is collected before any is
    /// applied, so a unit killed in the same tick still lands its own.
    Hit {
        /// Who hit.
        attacker: TargetId,
        /// Who was hit.
        target: TargetId,
        /// Damage after the counter multiplier and any variance.
        damage: i32,
    },
    /// A unit's damage reached its hit points; it was removed. Its owner
    /// and kind are captured before the removal, so a presenter need not
    /// have seen the unit.
    UnitDied {
        /// The unit.
        unit: UnitId,
        /// Its owner.
        owner: PlayerId,
        /// Its kind.
        kind: UnitKindId,
    },
    /// A building or site reached 0 HP and was removed (no refund; its
    /// queue is lost). Its owner and kind are captured before the removal.
    BuildingDestroyed {
        /// The building.
        building: BuildingId,
        /// Its owner.
        owner: PlayerId,
        /// Its kind.
        kind: BuildingKindId,
    },
    /// A complete Town reached 0 HP. It stays, clamped at 0; it can no
    /// longer be attacked.
    TownAtZeroHp {
        /// The Town.
        town: BuildingId,
    },
    /// An attrition pulse took hit points from a unit exposed on enemy land
    /// (step 5b'); a unit it kills is removed in 5c with `UnitDied`.
    Attrition {
        /// The unit.
        unit: UnitId,
        /// Hit points taken.
        damage: i32,
    },
    /// A Town's annexation timer started (step 5d): its land turned
    /// neutral.
    AnnexationStarted {
        /// The Town.
        town: BuildingId,
        /// The player whose units are adjacent.
        attacker: PlayerId,
    },
    /// A Town's annexation timer stopped before the flip (a defender came
    /// near, the attackers left or another player's took their place); its
    /// land returned to its owner.
    AnnexationReset {
        /// The Town.
        town: BuildingId,
    },
    /// A Town changed hands (step 5d), with the buildings attributed to it
    /// that do not stay.
    TownAnnexed {
        /// The Town.
        town: BuildingId,
        /// Its old owner.
        from: PlayerId,
        /// Its new owner.
        to: PlayerId,
    },
    /// The match was decided (step 6e); emitted once.
    MatchDecided {
        /// The result, as `Sim::result` returns it from now on.
        result: MatchResult,
    },
}

/// Per-subsystem hashes for localising a divergence. Field order is the
/// comparison order of [`SubHashes::first_difference`]. Every field of
/// the hashed state belongs to exactly one of them.
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
    /// Every player's age and researched techs ([`TechState`]); research
    /// entries live in `buildings`.
    pub tech: u64,
    /// Path request queue and the active search (not the cache).
    pub pathing: u64,
    /// The `Pcg32`.
    pub rng: u64,
    /// Cost grid and its generation.
    pub map: u64,
    /// Tick, seed, id allocator, players, pending commands, counters and,
    /// since M5a, the match result (`State::outcome`).
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
    /// Every player's age and researched techs.
    pub(crate) tech: TechState,
    /// The match result once step 6e decided it (M5a); sub-hash `meta`.
    pub(crate) outcome: Option<MatchResult>,
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
    /// Derived from the rules alone: built here and in `rebuild_derived`.
    combat_tables: combat::CombatTables,
    /// Whether the (derived) path cache is in use; survives restore.
    path_cache_enabled: bool,
    /// Not hashed; drained by the caller.
    events: Vec<SimEvent>,
}

impl Sim {
    /// Create a match. `ai` controls every slot with `is_ai == true`.
    /// Panics when `setup.map` is not in `rules` (the setup was built from
    /// different rules), when the slots are not `PlayerId(0)`,
    /// `PlayerId(1)`, ... in order (stockpiles and territory layers are
    /// indexed by id), or when `setup.ai_seats` does not name exactly the
    /// `is_ai` slots, once each, in id order (M5b; `ai::controller_for`
    /// reports the same mismatch as an error before a sim is built).
    pub fn new(setup: MatchSetup, rules: Rules, ai: Box<dyn AiController>) -> Self {
        for (i, slot) in setup.players.iter().enumerate() {
            assert_eq!(
                usize::from(slot.id.0),
                i,
                "player slots must be PlayerId(0), PlayerId(1), ... in order"
            );
        }
        let ai_slots: Vec<PlayerId> = setup
            .players
            .iter()
            .filter(|s| s.is_ai)
            .map(|s| s.id)
            .collect();
        let seats: Vec<PlayerId> = setup.ai_seats.iter().map(|s| s.player).collect();
        assert_eq!(
            seats, ai_slots,
            "ai_seats must name exactly the AI slots, once each, in id order"
        );
        let players: Vec<Player> = setup
            .players
            .iter()
            .map(|s| Player {
                id: s.id,
                is_ai: s.is_ai,
                surrendered: false,
                next_ai_seq: 0,
                ai_memory: AiMemory::default(),
            })
            .collect();
        let def = rules
            .map(&setup.map)
            .unwrap_or_else(|| panic!("map {:?} is not in the loaded rules", setup.map));
        let map = Map::from_def(def);
        let player_count = u8::try_from(players.len()).expect("at most 255 players");
        let territory = TerritoryField::new(map.width(), map.height(), player_count);
        let economy = Economy::new(players.len(), rules.resources.resources.len());
        let tech = TechState::new(players.len());
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
            tech,
            outcome: None,
        };
        if setup.start == StartKind::Standard {
            assert!(
                setup.players.len() <= def.starts.len(),
                "the map has a start for every player"
            );
            town::apply_standard_start(&mut state, &rules, def, rules.tick_rate_hz);
        }
        let grid = SpatialGrid::new(&state.map);
        let combat_tables = combat::CombatTables::new(&rules);
        Self {
            setup,
            rules,
            ai,
            state,
            grid,
            combat_tables,
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
        //    Every AI player's memory is taken out of the state first, so
        //    no bot reads another's through `view.players()`; each thinking
        //    bot gets its own, in id order, and every memory goes back. A
        //    surrendered bot neither thinks nor has its memory changed.
        let mut memories: Vec<(PlayerId, bool, AiMemory)> = self
            .state
            .players
            .iter_mut()
            .filter(|p| p.is_ai)
            .map(|p| (p.id, !p.surrendered, std::mem::take(&mut p.ai_memory)))
            .collect();
        let mut ai_batches: Vec<(PlayerId, Vec<Command>)> = Vec::new();
        {
            let view = SimView::new(&self.state, &self.rules, &self.setup);
            for (pid, thinks, memory) in &mut memories {
                if *thinks {
                    let out = self.ai.think(*pid, &view, memory);
                    if !out.is_empty() {
                        ai_batches.push((*pid, out));
                    }
                }
            }
        }
        for (pid, _, memory) in memories {
            self.state
                .players
                .iter_mut()
                .find(|p| p.id == pid)
                .expect("ai player exists")
                .ai_memory = memory;
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
                    u.disengage();
                    self.events.push(SimEvent::PathUnreachable { unit });
                }
            }
        }

        // 5. Movement and combat. 5a: explicit attackers decide whether to
        //    hold, chase directly, walk a path or give up, guards and
        //    attack-movers acquire, and the shooters aim (from the
        //    start-of-tick positions). 5b: movement, with those engagements;
        //    a path chaser waiting for a path does not arrive.
        //    A unit that still has a request outstanding (queued or
        //    suspended) is not re-requested: a replacement would move it to
        //    the back of the queue and could starve it under a saturated
        //    budget. Its periodic timer is pushed back either way.
        self.grid.rebuild(&self.state.units);
        let stats = combat::Stats::new(&self.state, &self.rules, &self.combat_tables);
        let plan = combat::think(&mut self.state, &self.rules, &self.grid, &stats, tick);
        let repath = movement::step(
            &mut self.state.units,
            &self.state.map,
            &self.grid,
            &self.rules,
            &plan.engage,
            &plan.awaiting,
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
        // 5b'. Attrition from the post-movement positions; its dead are
        //      reaped in 5c with the rest.
        attrition::step(&mut self.state, &self.rules, &mut self.events);
        // 5c. Hits from the post-movement positions, then the dead and the
        //     destroyed.
        combat::resolve(
            &mut self.state,
            &self.rules,
            &stats,
            &plan.aims,
            tick,
            &mut self.events,
        );
        // 5d. Annexation of Towns at 0 HP, from the post-movement,
        //     post-reap positions.
        let flips = annexation::step(&mut self.state, &self.rules, &mut self.events);

        // 6. Economy and Towns: construction, training, jobs and idle
        //    seeking, income; then the match outcome.
        construction::step(&mut self.state, &self.rules, tick, &mut self.events);
        training::step(&mut self.state, &self.rules, tick, &mut self.events);
        economy::jobs_step(&mut self.state, &self.rules, tick);
        economy::income_step(&mut self.state, &self.rules);
        victory::step(&mut self.state, &self.rules, tick, &flips, &mut self.events);

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
    /// per unit, a path request each, posts, jobs and combat orders
    /// cleared. Shared by `Move` and `AttackMove`, which then sets the
    /// armed units' intent. Returns each unit with its goal, in id order;
    /// rejects with `NoValidUnits` (returning nothing) when no unit
    /// qualifies.
    fn apply_move(
        &mut self,
        pc: &PlayerCommand,
        units: &[UnitId],
        target: FxVec2,
    ) -> Vec<(UnitId, FxVec2)> {
        let tick = self.state.tick;
        let movers = self.owned_units(pc.player, units);
        if movers.is_empty() {
            self.reject(pc, RejectReason::NoValidUnits);
            return Vec::new();
        }
        let targets = movement::group_targets(&self.state.map, target, movers.len());
        let repath_at = tick + self.rules.repath_interval_ticks();
        let goals: Vec<(UnitId, FxVec2)> = movers.into_iter().zip(targets).collect();
        for &(unit, goal) in &goals {
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
            u.disengage();
            let from = self.state.map.tile_of(u.pos);
            let to = self.state.map.tile_of(goal);
            self.state.pathing.request(PathRequest {
                requested_tick: tick,
                unit,
                from,
                to,
            });
        }
        goals
    }

    /// `DebugSpawn` and `DebugSpawnJittered`: gated by `debug_commands`,
    /// `UnknownPlayer`, `UnknownUnitKind`; then up to `count` units of
    /// `kind` on the passable tiles of the square spiral around `at`, one
    /// per tile, at the tile centres (the map may run out of tiles).
    /// Returns the new units in id order.
    fn debug_spawn(
        &mut self,
        pc: &PlayerCommand,
        owner: PlayerId,
        kind: UnitKindId,
        at: FxVec2,
        count: u16,
    ) -> Vec<(UnitId, Tile)> {
        if !self.setup.debug_commands {
            self.reject(pc, RejectReason::DebugCommandsDisabled);
            return Vec::new();
        }
        if !self.state.players.iter().any(|p| p.id == owner) {
            self.reject(pc, RejectReason::UnknownPlayer);
            return Vec::new();
        }
        let Some(kind_def) = self.rules.unit_kind(kind.0) else {
            self.reject(pc, RejectReason::UnknownUnitKind);
            return Vec::new();
        };
        let center = self.state.map.tile_of(at);
        let tiles: Vec<Tile> = self
            .state
            .map
            .spiral(center)
            .filter(|t| self.state.map.passable(*t))
            .take(usize::from(count))
            .collect();
        let mut spawned = Vec::with_capacity(tiles.len());
        for tile in tiles {
            let id = self.state.ids.unit();
            let unit = Unit::spawn(
                id,
                owner,
                kind,
                tile,
                kind_def,
                &self.state.map,
                self.rules.tick_rate_hz,
            );
            self.state.units.insert(id, unit);
            self.events.push(SimEvent::UnitSpawned { unit: id });
            spawned.push((id, tile));
        }
        spawned
    }

    /// `DebugSpawnJittered`'s offsets: for each unit in id order, `dx` then
    /// `dy` are drawn from the sim's RNG (`combat::rng_index`) uniformly in
    /// `-jitter_x100..=jitter_x100` hundredths of a tile and added to the
    /// tile centre. A point off the map, on a blocked tile or in another
    /// component than its tile keeps the centre; the draws are made anyway.
    fn jitter(&mut self, spawned: &[(UnitId, Tile)], jitter_x100: u8) {
        let j = i32::from(jitter_x100);
        let span = u32::from(jitter_x100) * 2 + 1;
        for &(id, tile) in spawned {
            let mut offset = || {
                let d = i32::try_from(combat::rng_index(&mut self.state.rng, span)).expect("fits");
                Fx::from_ratio(d - j, 100)
            };
            let (dx, dy) = (offset(), offset());
            let map = &self.state.map;
            let centre = map.center_of(tile);
            let p = FxVec2::new(centre.x + dx, centre.y + dy);
            let on_map = p.x >= Fx::ZERO
                && p.y >= Fx::ZERO
                && p.x < Fx::from_int(i32::from(map.width()))
                && p.y < Fx::from_int(i32::from(map.height()));
            if on_map
                && map.passable(map.tile_of(p))
                && map.component_of(map.tile_of(p)) == map.component_of(tile)
            {
                self.state.units.get_mut(&id).expect("just spawned").pos = p;
            }
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
                // A Move whose armed units acquire enemies on the way
                // (`combat::think`); a unit without an attack just moves.
                for (unit, goal) in self.apply_move(&pc, units, *target) {
                    let u = self.state.units.get_mut(&unit).expect("moved");
                    if self
                        .rules
                        .unit_kind(u.kind.0)
                        .is_some_and(|k| k.attack.is_some())
                    {
                        u.intent = Intent::AttackMove { goal };
                    }
                }
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
                        u.disengage();
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
                self.debug_spawn(&pc, *owner, *kind, *at, *count);
            }
            Command::DebugSpawnJittered {
                owner,
                kind,
                at,
                count,
                jitter_x100,
            } => {
                let spawned = self.debug_spawn(&pc, *owner, *kind, *at, *count);
                self.jitter(&spawned, *jitter_x100);
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
            Command::Research { building, tech } => {
                match tech::validate_research(&self.state, &self.rules, pc.player, *building, *tech)
                {
                    Err(reason) => self.reject(&pc, reason),
                    Ok((cost, ticks)) => tech::enqueue_research(
                        &mut self.state,
                        pc.player,
                        *building,
                        *tech,
                        cost,
                        ticks,
                    ),
                }
            }
            Command::AdvanceAge { player } => {
                match tech::validate_advance(&self.state, &self.rules, pc.player, *player) {
                    Err(reason) => self.reject(&pc, reason),
                    Ok(cost) => tech::advance(
                        &mut self.state,
                        &self.rules,
                        pc.player,
                        &cost,
                        &mut self.events,
                    ),
                }
            }
            Command::DebugResearch { owner, tech } => {
                if !self.setup.debug_commands {
                    self.reject(&pc, RejectReason::DebugCommandsDisabled);
                    return;
                }
                if !self.state.players.iter().any(|p| p.id == *owner) {
                    self.reject(&pc, RejectReason::UnknownPlayer);
                    return;
                }
                match tech::validate_debug_research(&self.state, &self.rules, *owner, *tech) {
                    Err(reason) => self.reject(&pc, reason),
                    Ok(()) => tech::complete(
                        &mut self.state,
                        &self.rules,
                        *owner,
                        *tech,
                        &mut self.events,
                    ),
                }
            }
            Command::Attack { units, target } => {
                // Validated purely, so a rejection changes nothing and,
                // since the M4a combat core slice, draws nothing from the rng.
                match combat::validate_attack(&self.state, &self.rules, pc.player, units, *target) {
                    Err(reason) => self.reject(&pc, reason),
                    Ok(attackers) => combat::apply_attack(&mut self.state, &attackers, *target),
                }
            }
            Command::DebugDamage { target, amount } => {
                if !self.setup.debug_commands {
                    self.reject(&pc, RejectReason::DebugCommandsDisabled);
                    return;
                }
                // Existence only, not `combat::locate`: any owner, a
                // player's own buildings and a Town at 0 HP included.
                let exists = match target {
                    TargetId::Unit(id) => self.state.units.contains_key(id),
                    TargetId::Building(id) => self.state.buildings.contains_key(id),
                };
                if !exists {
                    self.reject(&pc, RejectReason::InvalidTarget);
                    return;
                }
                combat::apply_damage(
                    &mut self.state,
                    *target,
                    i64::from(*amount),
                    &mut self.events,
                );
            }
            Command::DebugAdvanceAge { owner } => {
                if !self.setup.debug_commands {
                    self.reject(&pc, RejectReason::DebugCommandsDisabled);
                    return;
                }
                if !self.state.players.iter().any(|p| p.id == *owner) {
                    self.reject(&pc, RejectReason::UnknownPlayer);
                    return;
                }
                let age = usize::from(self.state.tech.player(*owner).age);
                if age + 1 >= self.rules.ages.len() {
                    self.reject(&pc, RejectReason::FinalAge);
                    return;
                }
                let free = vec![0; self.rules.resources.resources.len()];
                tech::advance(
                    &mut self.state,
                    &self.rules,
                    *owner,
                    &free,
                    &mut self.events,
                );
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

    /// How the match ended, once step 6e decided it (M5a); `None` while it
    /// runs. Latched: it never changes once set, and the sim keeps
    /// stepping.
    pub fn outcome(&self) -> Option<Outcome> {
        self.state.outcome.map(|r| r.outcome)
    }

    /// The full result (outcome, reason and deciding tick), once decided.
    pub fn result(&self) -> Option<MatchResult> {
        self.state.outcome
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
            tech: crate::hash::hash_value(&s.tech),
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
                &s.outcome,
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

    /// Rebuild every unhashed derived structure from `state` and the rules:
    /// map components, spatial grid, the combat tables and the path cache
    /// (empty, with its capacity and the sim's enabled flag re-applied since
    /// `serde(skip)` reset it).
    fn rebuild_derived(&mut self) {
        self.state.map.rebuild_components();
        self.grid = SpatialGrid::new(&self.state.map);
        self.grid.rebuild(&self.state.units);
        self.combat_tables = combat::CombatTables::new(&self.rules);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::ai_hook::NoAi;

    /// Step 4's `None` branch, which pathing reaches only defensively (it
    /// corrects every goal into the unit's component, so on a real map a
    /// path always exists): an attack-mover whose request finds no
    /// passable tile at all loses its order, and the sim emits
    /// `PathUnreachable`. Every tile is blocked under the request to get
    /// there. The test pins the event and the order clear: with the order
    /// gone, think's `AttackMove` arm would end the intent even if step 4
    /// did not (`path_unreachable_ends_an_explicit_attack_without_a_request_storm`
    /// covers step 4's intent clear).
    #[test]
    fn path_unreachable_ends_an_attack_move() {
        let rules = Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap();
        let skirm = UnitKindId(
            u16::try_from(
                rules
                    .units
                    .iter()
                    .position(|u| u.id == "skirmisher")
                    .unwrap(),
            )
            .unwrap(),
        );
        let mut sim = Sim::new(MatchSetup::scenario(&rules, 9), rules, Box::new(NoAi));
        let delay = sim.setup.cmd_delay;
        let stamp = |seq, cmd| PlayerCommand::new(PlayerId(0), seq, cmd);
        sim.step(&[stamp(
            0,
            Command::DebugSpawn {
                owner: PlayerId(0),
                kind: skirm,
                at: FxVec2::from_ints(10, 30),
                count: 1,
            },
        )]);
        for _ in 0..delay {
            sim.step(&[]);
        }
        sim.step(&[stamp(
            1,
            Command::AttackMove {
                units: vec![UnitId(1)],
                target: FxVec2::from_ints(30, 30),
            },
        )]);
        for _ in 1..delay {
            sim.step(&[]);
        }
        sim.drain_events();
        // The AttackMove applies, and its request is served, in the next
        // step: block the whole map first.
        let all: Vec<Tile> = (0..sim.state.map.len())
            .map(|i| sim.state.map.tile_at(i))
            .collect();
        sim.state.map.set_blocked_many(&all, true);
        sim.step(&[]);
        let events = sim.drain_events();
        assert!(
            events.contains(&SimEvent::PathUnreachable { unit: UnitId(1) }),
            "{events:?}"
        );
        let u = &sim.state.units[&UnitId(1)];
        assert_eq!(
            (u.order.is_none(), u.intent, u.target),
            (true, Intent::Plain, None)
        );
    }

    /// Step 4's defensive `None` branch ends an explicit `Attack` too.
    /// Without the clear, `think_explicit`'s `(None, None)` arm would
    /// re-order the unit every tick (`chase_end` answers `Some` from a
    /// blocked tile), and every step 4 would answer `None` again: a request
    /// and a `PathUnreachable` every tick.
    #[test]
    fn path_unreachable_ends_an_explicit_attack_without_a_request_storm() {
        let rules = Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap();
        let kind = |name: &str| {
            UnitKindId(
                u16::try_from(rules.units.iter().position(|u| u.id == name).unwrap()).unwrap(),
            )
        };
        let (skirm, bow) = (kind("skirmisher"), kind("bowman"));
        let mut sim = Sim::new(MatchSetup::scenario(&rules, 9), rules, Box::new(NoAi));
        let delay = sim.setup.cmd_delay;
        let stamp = |seq, cmd| PlayerCommand::new(PlayerId(0), seq, cmd);
        sim.step(&[
            // UnitId(1)
            stamp(
                0,
                Command::DebugSpawn {
                    owner: PlayerId(0),
                    kind: skirm,
                    at: FxVec2::from_ints(10, 30),
                    count: 1,
                },
            ),
            // UnitId(2), beyond reach
            stamp(
                1,
                Command::DebugSpawn {
                    owner: PlayerId(1),
                    kind: bow,
                    at: FxVec2::from_ints(30, 30),
                    count: 1,
                },
            ),
        ]);
        for _ in 0..delay {
            sim.step(&[]);
        }
        sim.step(&[stamp(
            2,
            Command::Attack {
                units: vec![UnitId(1)],
                target: crate::ids::TargetId::Unit(UnitId(2)),
            },
        )]);
        for _ in 1..delay {
            sim.step(&[]);
        }
        sim.step(&[]); // applies; 5a path-chases: order_move + request
        assert!(sim.state.pathing.has_request(UnitId(1)));
        sim.drain_events();
        let all: Vec<Tile> = (0..sim.state.map.len())
            .map(|i| sim.state.map.tile_at(i))
            .collect();
        sim.state.map.set_blocked_many(&all, true);
        sim.step(&[]); // step 4 serves the request: None
        let events = sim.drain_events();
        assert!(
            events.contains(&SimEvent::PathUnreachable { unit: UnitId(1) }),
            "{events:?}"
        );
        let u = &sim.state.units[&UnitId(1)];
        assert_eq!(
            (u.order.is_none(), u.intent, u.target),
            (true, Intent::Plain, None)
        );
        assert!(
            !sim.state.pathing.has_request(UnitId(1)),
            "no re-request in the same step"
        );
        sim.step(&[]);
        assert!(
            !sim.drain_events()
                .iter()
                .any(|e| matches!(e, SimEvent::PathUnreachable { .. })),
            "no storm"
        );
    }

    fn data_rules() -> Rules {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap()
    }

    /// What one `think` saw: the thinking player, the tick, the memory it
    /// was handed, and every player's memory as the view shows it.
    type Seen = (PlayerId, u32, Vec<u8>, Vec<Vec<u8>>);

    /// A bot that writes `[player + 1, tick]` into its memory on every
    /// think and logs what it saw, and surrenders `surrender.0` at tick
    /// `surrender.1`.
    struct Peek {
        log: std::sync::Arc<std::sync::Mutex<Vec<Seen>>>,
        surrender: Option<(PlayerId, u32)>,
    }

    impl AiController for Peek {
        fn think(
            &mut self,
            player: PlayerId,
            view: &SimView<'_>,
            memory: &mut AiMemory,
        ) -> Vec<Command> {
            let shown = view
                .players()
                .iter()
                .map(|p| {
                    assert_eq!(view.ai_memory(p.id), p.ai_memory.bytes());
                    p.ai_memory.bytes().to_vec()
                })
                .collect();
            self.log
                .lock()
                .unwrap()
                .push((player, view.tick, memory.bytes().to_vec(), shown));
            memory.set(vec![player.0 + 1, u8::try_from(view.tick % 256).unwrap()]);
            match self.surrender {
                Some((p, t)) if p == player && t == view.tick => vec![Command::Surrender],
                _ => Vec::new(),
            }
        }
    }

    fn peek_sim(
        surrender: Option<(PlayerId, u32)>,
    ) -> (Sim, std::sync::Arc<std::sync::Mutex<Vec<Seen>>>) {
        let rules = data_rules();
        let setup = MatchSetup::bots(&rules, 5, ("peek", ""), ("peek", ""));
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let bot = Peek {
            log: log.clone(),
            surrender,
        };
        (Sim::new(setup, rules, Box::new(bot)), log)
    }

    /// M5b: each bot is handed its own memory (what it wrote the tick
    /// before), in id order, while every player's memory reads empty in
    /// the view; after the step both memories are back in the state.
    #[test]
    fn bots_never_see_another_bots_memory() {
        let (mut sim, log) = peek_sim(None);
        for _ in 0..4 {
            sim.step(&[]);
        }
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 8, "two bots think every tick");
        for (i, (player, tick, handed, shown)) in log.iter().enumerate() {
            assert_eq!(*player, PlayerId(u8::try_from(i % 2).unwrap()), "id order");
            assert_eq!(*tick, u32::try_from(i / 2).unwrap());
            let own_last = if *tick == 0 {
                Vec::new()
            } else {
                vec![player.0 + 1, u8::try_from(tick - 1).unwrap()]
            };
            assert_eq!(*handed, own_last, "the bot gets its own memory");
            assert!(
                shown.iter().all(Vec::is_empty),
                "no memory is visible in the view: {shown:?}"
            );
        }
        let view = sim.view();
        assert_eq!(view.ai_memory(PlayerId(0)), &[1, 3]);
        assert_eq!(view.ai_memory(PlayerId(1)), &[2, 3]);
        assert_eq!(view.ai_memory(PlayerId(9)), &[] as &[u8]);
    }

    /// M5b: `Player::ai_memory` is hashed in `meta` (and only there), and a
    /// snapshot carries it: a fresh sim restored from it, with a controller
    /// that writes nothing, shows the same bytes and hash.
    #[test]
    fn ai_memory_is_hashed_in_meta_and_carried_by_snapshots() {
        let (mut a, _) = peek_sim(None);
        let rules = data_rules();
        let mut b = Sim::new(
            MatchSetup::bots(&rules, 5, ("peek", ""), ("peek", "")),
            rules.clone(),
            Box::new(NoAi),
        );
        assert_eq!(a.hash(), b.hash());
        a.step(&[]);
        b.step(&[]);
        let (sa, sb) = (a.sub_hashes(), b.sub_hashes());
        assert_eq!(sa.first_difference(&sb), Some("meta"));
        assert_ne!(a.hash(), b.hash());
        let only_meta = SubHashes {
            meta: sb.meta,
            ..sa
        };
        assert_eq!(only_meta, sb, "every other sub-hash is equal");

        let snap = a.snapshot();
        let mut c = Sim::new(
            MatchSetup::bots(&rules, 5, ("peek", ""), ("peek", "")),
            rules,
            Box::new(NoAi),
        );
        c.restore(&snap).unwrap();
        assert_eq!(c.hash(), a.hash());
        assert_eq!(c.view().ai_memory(PlayerId(0)), &[1, 0]);
        assert_eq!(c.view().ai_memory(PlayerId(1)), &[2, 0]);
        // NoAi leaves the restored memories as they are.
        c.step(&[]);
        assert_eq!(c.view().ai_memory(PlayerId(1)), &[2, 0]);
    }

    /// M5b: a bot that has surrendered no longer thinks, and its memory is
    /// left exactly as it last wrote it (still hashed, still readable).
    #[test]
    fn surrendered_bot_keeps_its_memory_and_stops_thinking() {
        let (mut sim, log) = peek_sim(Some((PlayerId(1), 2)));
        let delay = sim.setup().cmd_delay;
        for _ in 0..20 {
            sim.step(&[]);
        }
        let surrendered_at = 2 + delay;
        assert!(sim.view().player(PlayerId(1)).unwrap().surrendered);
        let log = log.lock().unwrap();
        let p1: Vec<u32> = log
            .iter()
            .filter(|e| e.0 == PlayerId(1))
            .map(|e| e.1)
            .collect();
        assert_eq!(p1, (0..=surrendered_at).collect::<Vec<_>>());
        assert_eq!(log.iter().filter(|e| e.0 == PlayerId(0)).count(), 20);
        let last = u8::try_from(surrendered_at).unwrap();
        assert_eq!(sim.view().ai_memory(PlayerId(1)), &[2, last]);
        assert!(
            log.iter()
                .filter(|e| e.1 > surrendered_at)
                .all(|e| e.3.iter().all(Vec::is_empty)),
            "the surrendered bot's memory is hidden from the other bot too"
        );
    }

    /// M5b: `ai_seats` must name exactly the AI slots.
    #[test]
    #[should_panic(expected = "ai_seats must name exactly the AI slots")]
    fn ai_seats_must_match_the_ai_slots() {
        let rules = data_rules();
        let mut setup = MatchSetup::skirmish(&rules, 1);
        setup.ai_seats[0].player = PlayerId(0);
        let _ = Sim::new(setup, rules, Box::new(NoAi));
    }

    /// M5b: `DebugAdvanceAge { owner }` is rejected in a match without
    /// debug commands, then moves any existing owner one age on with no
    /// cost and no techs, and is rejected with `UnknownPlayer` and, in the
    /// last age, `FinalAge`.
    #[test]
    fn debug_advance_age_is_gated_and_skips_cost_and_techs() {
        let rules = data_rules();
        let rejections = |sim: &mut Sim| -> Vec<(u32, RejectReason)> {
            sim.drain_events()
                .into_iter()
                .filter_map(|e| match e {
                    SimEvent::CommandRejected { seq, reason, .. } => Some((seq, reason)),
                    _ => None,
                })
                .collect()
        };
        let stamp = |seq, cmd| PlayerCommand::new(PlayerId(0), seq, cmd);
        let adv = |owner: u8| Command::DebugAdvanceAge {
            owner: PlayerId(owner),
        };

        let mut gated = Sim::new(
            MatchSetup::skirmish(&rules, 1),
            rules.clone(),
            Box::new(NoAi),
        );
        let delay = gated.setup().cmd_delay;
        gated.step(&[stamp(0, adv(0))]);
        for _ in 0..delay {
            gated.step(&[]);
        }
        assert_eq!(
            rejections(&mut gated),
            [(0, RejectReason::DebugCommandsDisabled)]
        );
        assert_eq!(gated.view().age(PlayerId(0)), 0);

        let mut setup = MatchSetup::scenario(&rules, 1);
        setup.start = StartKind::Standard;
        let mut sim = Sim::new(setup, rules.clone(), Box::new(NoAi));
        let stock = sim.view().economy(PlayerId(0)).stock.clone();
        let rng = sim.sub_hashes().rng;
        sim.step(&[
            stamp(
                0,
                Command::AdvanceAge {
                    player: PlayerId(0),
                },
            ),
            stamp(1, adv(0)),
            stamp(2, adv(1)),
            stamp(3, adv(7)),
        ]);
        for _ in 0..delay {
            sim.step(&[]);
        }
        let events = sim.drain_events();
        let advanced: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, SimEvent::AgeAdvanced { .. }))
            .collect();
        assert_eq!(
            advanced,
            [
                &SimEvent::AgeAdvanced {
                    player: PlayerId(0),
                    age: 1
                },
                &SimEvent::AgeAdvanced {
                    player: PlayerId(1),
                    age: 1
                }
            ]
        );
        let rejected: Vec<(u32, RejectReason)> = events
            .into_iter()
            .filter_map(|e| match e {
                SimEvent::CommandRejected { seq, reason, .. } => Some((seq, reason)),
                _ => None,
            })
            .collect();
        assert_eq!(
            rejected,
            [
                (0, RejectReason::NotEnoughTechs),
                (3, RejectReason::UnknownPlayer)
            ]
        );
        assert_eq!(sim.view().economy(PlayerId(0)).stock, stock, "no cost");
        assert!(sim.view().researched(PlayerId(0)).is_empty(), "no techs");
        assert_eq!(sim.sub_hashes().rng, rng, "draws nothing");

        let last = u8::try_from(rules.ages.len() - 1).unwrap();
        let mut seq = 4;
        while sim.view().age(PlayerId(0)) < last {
            sim.step(&[stamp(seq, adv(0))]);
            seq += 1;
            for _ in 0..delay {
                sim.step(&[]);
            }
        }
        rejections(&mut sim);
        sim.step(&[stamp(seq, adv(0))]);
        for _ in 0..delay {
            sim.step(&[]);
        }
        assert_eq!(rejections(&mut sim), [(seq, RejectReason::FinalAge)]);
        assert_eq!(sim.view().age(PlayerId(0)), last);
    }
}
