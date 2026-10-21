//! Eonmark's engine-side crate: the Bevy presentation layer. The `eonmark`
//! binary (`src/main.rs`) is a thin wrapper that parses the command line and
//! calls [`app::run`] or [`headless::run`]; everything else lives here as a
//! library so integration tests (`tests/`) can drive the real `SimPlugin`.
//!
//! Module map (M2):
//! - [`cli`]: hand-rolled argument parsing and data directory resolution.
//! - [`app`]: the windowed Bevy app and plugin wiring.
//! - [`background`]: `--background` / `EONMARK_BACKGROUND=1` for automated
//!   windowed runs (unfocused, below other windows; macOS hands focus back).
//! - [`monitor_loss`]: keeps the window open when its monitor disappears
//!   (display sleep, screen lock), which Bevy 0.19.1 would treat as closed.
//! - [`sim_driver`]: `SimHandle`, the `FixedUpdate` driver, `InputSource`
//!   (local, replay, scenario), `PendingCommands`, the replay recorder thread.
//! - [`present`]: `UnitId -> Entity` mirror, interpolation, visuals.
//! - [`selection`]: picking, click/box/group selection, rings and markers.
//! - [`orders`]: ground ray-plane hit to `Move` / `Stop` / `AttackMove`.
//! - [`hud`]: top and bottom bars, `PointerOverUi`, the `hud_click` check.
//! - [`camera`]: the yaw-locked RTS camera.
//! - [`ground`] and [`palette`]: the flat 128 x 128 m ground mesh and colours.
//! - [`headless`]: `--headless-run <ticks | replay>` on `MinimalPlugins`.
//! - [`frame_stats`]: frame-time and arrival lines printed on exit
//!   (`--scenario units200_auto`, `--max-fps`).
//! - `macos_menu`: muda menu with the custom Cmd-Q item (macOS only).
//! - `dev_tools`: FPS overlay, egui inspector (F12) and sim panel (`dev` only).
#![forbid(unsafe_code)]

pub mod app;
pub mod background;
pub mod buildings;
pub mod camera;
pub mod cli;
pub mod command_card;
#[cfg(feature = "dev")]
pub mod dev_tools;
pub mod economy_hud;
pub mod field;
pub mod frame_stats;
pub mod ground;
pub mod headless;
pub mod hud;
#[cfg(target_os = "macos")]
pub mod macos_menu;
pub mod monitor_loss;
pub mod orders;
pub mod palette;
pub mod placement;
pub mod present;
pub mod render_watchdog;
pub mod selection;
pub mod sim_driver;
