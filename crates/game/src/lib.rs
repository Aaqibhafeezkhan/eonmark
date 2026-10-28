//! Eonmark's engine-side crate: the Bevy presentation layer. The `eonmark`
//! binary (`src/main.rs`) is a thin wrapper that parses the command line and
//! calls [`app::run`] or [`headless::run`]; everything else lives here as a
//! library so integration tests (`tests/`) can drive the real `SimPlugin`.
//!
//! Module map (M2):
//! - [`cli`]: hand-rolled argument parsing and data and assets directory
//!   resolution.
//! - [`app`]: the windowed Bevy app and plugin wiring.
//! - [`background`]: `--background` / `EONMARK_BACKGROUND=1` for automated
//!   windowed runs (unfocused, below other windows; macOS hands focus back).
//! - [`monitor_loss`]: keeps the window open when its monitor disappears
//!   (display sleep, screen lock), which Bevy 0.19.1 would treat as closed.
//! - [`sim_driver`]: `SimHandle`, the `FixedUpdate` driver, `InputSource`
//!   (local, replay, scenario), `PendingCommands`, the replay recorder thread.
//! - [`present`]: `UnitId -> Entity` mirror, interpolation, visuals (M4b:
//!   one low-poly, flat-shaded look per unit kind and its height).
//! - [`selection`]: picking, click/box/group selection, rings and markers.
//! - [`orders`]: ground ray-plane hit to `Move` / `Stop` / `AttackMove`,
//!   the contextual right click (M4b: `Attack` on an enemy) and the
//!   attack-move cursor; orders name only the local player's units.
//! - [`health_bars`]: team-coloured health bars over damaged or selected
//!   units and buildings, flat entities keyed by `TargetId` (M4b), and the
//!   dev-only hierarchy check.
//! - [`hud`]: top and bottom bars, `PointerOverUi`, the `hud_click` check.
//! - [`age_hud`]: the age readout and the Advance button on a strip under
//!   the top bar (M4b), greyed by `SimView::check_advance_age`.
//! - [`camera`]: the yaw-locked RTS camera.
//! - [`ground`] and [`palette`]: the flat 128 x 128 m ground mesh and
//!   colours; the debug grid, off by default since the art pass (G).
//! - [`headless`]: `--headless-run <ticks | replay>` on `MinimalPlugins`.
//! - [`frame_stats`]: frame-time and arrival lines printed on exit
//!   (`--scenario units200_auto`, the `enemy_outpost` scenarios, `art_bench`,
//!   `--max-fps`, `--draw-stats`); for the `enemy_outpost` scenarios and
//!   `art_bench` also the 200 vs 200 fight window (M4b).
//! - [`draw_stats`]: the draw-call counter (art pass), rebuilt from Bevy's
//!   public render-phase data in the render world; the dev panel row and
//!   the `draws` and `triangles` exit lines (`--draw-stats`, `dev`).
//! - [`markers`]: the attrition indicator under bleeding units, the supply
//!   ring around a selected Supply Wain, the annexation timer around a
//!   contested Town and the annexation readout (M5a).
//! - [`outcome`]: the victory, defeat or draw overlay with the final hash
//!   over the frozen world once the sim decides the match (M5a).
//! - [`menu`]: `AppState` (`Menu`, `Skirmish`) and the main menu a plain
//!   launch opens on, with New Game and Quit (M5b).
//! - [`director`]: camera and selection beats for
//!   `--scenario enemy_outpost_walkthrough` (M4b) and `art_bench`'s
//!   select-all (art pass); never issues a command.
//! - `macos_menu`: muda menu with the custom Cmd-Q item (macOS only).
//! - `dev_tools`: FPS overlay, egui inspector (F12) and sim panel (`dev` only).
#![forbid(unsafe_code)]

pub mod age_hud;
pub mod app;
pub mod background;
pub mod buildings;
pub mod camera;
pub mod cli;
pub mod command_card;
#[cfg(feature = "dev")]
pub mod dev_tools;
pub mod director;
pub mod draw_stats;
pub mod economy_hud;
pub mod field;
pub mod frame_stats;
pub mod ground;
pub mod headless;
pub mod health_bars;
pub mod hud;
#[cfg(target_os = "macos")]
pub mod macos_menu;
pub mod markers;
pub mod menu;
pub mod monitor_loss;
pub mod orders;
pub mod outcome;
pub mod palette;
pub mod placement;
pub mod present;
pub mod render_watchdog;
pub mod selection;
pub mod sim_driver;
