//! The Eonmark simulation: deterministic, fixed-tick, fixed-point, engine-free.
//!
//! Invariants (see docs/DETERMINISM.md):
//! - Only [`Sim::step`] mutates state.
//! - No floats, no `HashMap`, no wall clock, no engine types in this crate
//!   (`clippy.toml` bans them; CI greps `cargo tree` for bevy/glam/wgpu/winit).
//! - Every container iterates in a total order; ids are monotonic and never reused.
//! - A command issued during tick `N` applies at tick `N + cmd_delay`.
//! - The scripted AI runs inside `step` through [`AiController`].
//!
//! Module map:
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`fx`] | `Fx` 32.32 fixed point, `FxVec2`, `dist_sq_i64` |
//! | [`ids`] | `PlayerId`, `UnitId`, `BuildingId`, kind ids, `Tile`, `IdGen` |
//! | [`command`] | `Command`, `PlayerCommand`, `sort_commands` |
//! | [`state`] | `Sim`, `MatchSetup`, `StartKind`, `State`, `Unit` (with its combat `Intent`), `Building`, `TownState` (with its annexation `Hold`), `MoveOrder`, `SimEvent`, `Outcome` and `MatchResult`, `SubHashes`, snapshot/restore, the tick order |
//! | [`map`] | `Map`: cost grid, generation, components, neighbours, BFS, spiral |
//! | [`pathing`] | budgeted resumable A*, request queue, transparent cache |
//! | [`movement`] | `SpatialGrid`, movement step, group targets |
//! | [`replay`] | `.eonreplay` writer, reader, `verify` |
//! | [`scenarios`] | scripted command streams shared by tests, benches, `record` and fixtures (the M4a counter duels and `combat_skirmish`, the M4b `enemy_outpost` pair among them) |
//! | [`territory`] | `TerritoryField`: integer disc kernels per owner layer, tie-to-neutral ownership |
//! | [`economy`] | `Economy` stockpiles and accumulators, `RateInfo`, exact income, jobs step, cost helpers |
//! | [`construction`] | footprints, `Build` validation, sites, construction, site cancel |
//! | [`gather`] | `Gather`, slots and stand tiles, worked-slot income, idle workers seeking work |
//! | [`town`] | standard start, town count and limit, attribution to the nearest Town, tier and radius refresh |
//! | [`training`] | `Train`, queue `Cancel`, `SetRally`, pop, the unit cost ramps, the training and research step |
//! | [`tech`] | `TechState` (ages, researched techs), Modifier-resolved player stats, `Research`, `AdvanceAge`, `DebugResearch` |
//! | [`attrition`] | Harrying levels, pulse intervals and sizes, the per-step `SupplyGrid` of Supply Wain tiles, immunities, step 5b' (M5a) |
//! | [`annexation`] | the annexation timer of Towns at 0 HP (attackers, defenders, entry, reset), the flip and its transfers, step 5d (M5a) |
//! | [`victory`] | the match outcome: Surrender, Seat captured, all Towns lost and the territory cap, latched in step 6e (M5a) |
//! | [`combat`] | unit combat stats from the Modifier rows, explicit `Attack` (hold, direct chase, path chase, give up), target acquisition by guards and attack-movers, Watchtower and Town shooters, simultaneous hits, death and building destruction |
//! | [`ai_hook`] | `AiController`, `SimView` (read-only quotes and checks; `building_hp` from M4b; Harrying, supply, exposure, annexation timers and the outcome from M5a) |
//! | [`hash`] | xxh3 over postcard |
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod ai_hook;
pub mod annexation;
pub mod attrition;
pub mod combat;
pub mod command;
pub mod construction;
pub mod economy;
pub mod fx;
pub mod gather;
pub mod hash;
pub mod ids;
pub mod map;
pub mod movement;
pub mod pathing;
pub mod replay;
pub mod scenarios;
pub mod state;
pub mod tech;
pub mod territory;
pub mod town;
pub mod training;
pub mod victory;

pub use ai_hook::{AiController, SimView};
pub use annexation::AnnexView;
pub use attrition::Exposure;
pub use combat::{AttackStats, Engage, UnitStats};
pub use command::{Command, PlayerCommand, sort_commands};
pub use economy::{Economy, PlayerEconomy, RateInfo};
pub use fx::{Fx, FxVec2};
pub use ids::{
    BuildingId, BuildingKindId, IdGen, PlayerId, TargetId, TechId, Tile, UnitId, UnitKindId,
};
pub use map::Map;
pub use movement::SpatialGrid;
pub use pathing::{AStarSearch, PathRequest, Pathing, SearchStatus};
pub use replay::{ReplayError, ReplayFile, ReplayReader, ReplayWriter, VerifyOutcome, verify};
pub use rules::Rules;
pub use state::{
    Building, Construction, Hold, Intent, Job, MatchResult, MatchSetup, MoveOrder, Outcome,
    OutcomeReason, Player, PlayerSlot, Post, QueueEntry, QueueItem, RejectReason, SIM_VERSION, Sim,
    SimEvent, SnapshotError, StartKind, SubHashes, TownState, Unit,
};
pub use tech::{PlayerTech, TechState};
pub use territory::{InvalidTerritoryField, Source, SourceOwner, TerritoryField};
