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
//! | [`state`] | `Sim`, `MatchSetup`, `State`, snapshot/restore |
//! | [`ai_hook`] | `AiController`, `SimView` |
//! | [`hash`] | xxh3 over postcard |
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod ai_hook;
pub mod command;
pub mod fx;
pub mod hash;
pub mod ids;
pub mod state;

pub use ai_hook::{AiController, SimView};
pub use command::{Command, PlayerCommand, sort_commands};
pub use fx::{Fx, FxVec2};
pub use ids::{
    BuildingId, BuildingKindId, IdGen, PlayerId, TargetId, TechId, Tile, UnitId, UnitKindId,
};
pub use rules::Rules;
pub use state::{Building, MatchSetup, Player, PlayerSlot, SIM_VERSION, Sim, SnapshotError, Unit};
