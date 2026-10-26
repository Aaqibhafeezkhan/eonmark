//! Developer-only overlays, compiled only with `--features dev`:
//! Bevy's FPS overlay, the egui world inspector (hidden until F12) and a
//! small sim panel. Since M5b the panel has one line per bot seat,
//! `bot P1 standard: <ai::describe>` (the build-order window, the Military
//! latch, the wave, the next wave tick and the defend latch).

use std::time::Duration;

use bevy::dev_tools::fps_overlay::{FpsOverlayConfig, FpsOverlayPlugin, FrameTimeGraphConfig};
use bevy::input::common_conditions::input_toggle_active;
use bevy::prelude::*;
use bevy::text::FontSize;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
use bevy_inspector_egui::quick::WorldInspectorPlugin;

use crate::camera::RtsCamera;
use crate::health_bars::{HealthBar, HealthBars};
use crate::present::UnitVisuals;
use crate::selection::Selection;
use crate::sim_driver::{
    ActiveInput, DriverStats, PendingCommands, PendingRecording, ReplayRecorder, SimHandle,
};

/// Adds every dev overlay. Never shipped.
pub struct DevToolsPlugin;

impl Plugin for DevToolsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            FpsOverlayPlugin {
                config: FpsOverlayConfig {
                    text_config: TextFont {
                        font_size: FontSize::Px(14.0),
                        ..default()
                    },
                    text_color: Color::srgb(0.96, 0.96, 0.92),
                    enabled: true,
                    refresh_interval: Duration::from_millis(250),
                    frame_time_graph_config: FrameTimeGraphConfig {
                        enabled: false,
                        ..default()
                    },
                },
            },
            // The inspector asserts that EguiPlugin was added before it.
            EguiPlugin::default(),
            // Hidden until F12: listing every entity (200 units, 200 rings)
            // each frame costs several milliseconds and covers the map.
            WorldInspectorPlugin::new().run_if(input_toggle_active(false, KeyCode::F12)),
        ))
        .register_type::<RtsCamera>()
        .add_systems(EguiPrimaryContextPass, sim_panel);
    }
}

#[allow(clippy::too_many_arguments)] // one egui panel reads many resources
fn sim_panel(
    mut contexts: EguiContexts,
    sim: NonSend<SimHandle>,
    fixed: Res<Time<Fixed>>,
    stats: Res<DriverStats>,
    pending: Res<PendingCommands>,
    input: Res<ActiveInput>,
    recorder: Res<ReplayRecorder>,
    to_record: Res<PendingRecording>,
    selection: Res<Selection>,
    visuals: Res<UnitVisuals>,
    cameras: Query<&RtsCamera>,
    bars: Res<HealthBars>,
    bar_entities: Query<(), With<HealthBar>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("Sim")
        .default_pos([12.0, 48.0])
        .resizable(false)
        // The bot lines wrap instead of stretching the panel across the map.
        .max_width(460.0)
        .show(ctx, |ui| {
            egui::Grid::new("sim-grid").num_columns(2).show(ui, |ui| {
                ui.label("tick");
                ui.monospace(sim.tick().to_string());
                ui.end_row();
                ui.label("hash");
                ui.monospace(format!("{:#018x}", sim.hash()));
                ui.end_row();
                ui.label("seed");
                ui.monospace(sim.seed().to_string());
                ui.end_row();
                ui.label("tick rate");
                ui.monospace(format!("{} Hz", sim.tick_rate_hz()));
                ui.end_row();
                ui.label("overstep");
                ui.monospace(format!("{:.2}", fixed.overstep_fraction()));
                ui.end_row();
                ui.label("ticks / frame");
                ui.monospace(stats.ticks_last_frame.to_string());
                ui.end_row();
                ui.label("dropped ticks");
                ui.monospace(stats.dropped_ticks.to_string());
                ui.end_row();
                ui.label("stalled ticks");
                ui.monospace(stats.stalled_ticks.to_string());
                ui.end_row();
                ui.label("units");
                ui.monospace(sim.view().unit_count().to_string());
                ui.end_row();
                ui.label("selected");
                ui.monospace(selection.units.len().to_string());
                ui.end_row();
                ui.label("input");
                ui.monospace(input.0.describe());
                ui.end_row();
                ui.label("pending cmds");
                let kinds: Vec<&str> = pending
                    .peek()
                    .iter()
                    .map(|c| command_kind(&c.cmd))
                    .collect();
                ui.monospace(format!("{} {}", pending.len(), kinds.join(" ")));
                ui.end_row();
                ui.label("recorded cmds");
                ui.monospace(stats.commands_recorded.to_string());
                ui.end_row();
                ui.label("rejected / events");
                ui.monospace(format!(
                    "{} / {}",
                    stats.commands_rejected, stats.events_seen
                ));
                ui.end_row();
                ui.label("recording");
                // File name only: the full path would stretch the panel
                // across the window and hide the units (it is on stdout as
                // `replay: <path>`, and here on hover).
                match recorder.0.as_ref() {
                    // M5b: the recorder starts with the first stepped tick.
                    None if to_record.0.is_some() => {
                        ui.monospace("from the first tick");
                    }
                    None => {
                        ui.monospace("off");
                    }
                    Some(r) => {
                        let path = r.path();
                        let name = path.file_name().map_or_else(
                            || path.display().to_string(),
                            |n| n.to_string_lossy().into_owned(),
                        );
                        ui.monospace(name).on_hover_text(path.display().to_string());
                    }
                }
                ui.end_row();
                ui.label("flushes");
                ui.monospace(stats.flushes_sent.to_string());
                ui.end_row();
                // M5b: one line per bot seat (`ai::describe`); `view` shows
                // each bot's memory outside its `think`.
                let view = sim.view();
                for seat in &sim.setup().ai_seats {
                    ui.label(format!("bot P{} {}", seat.player.0, seat.bot));
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(ai::describe(&view, seat.player)).monospace(),
                        )
                        .wrap(),
                    );
                    ui.end_row();
                }
                let (meshes, materials) = visuals.counts();
                ui.label("meshes / materials");
                ui.monospace(format!("{meshes} / {materials}"));
                ui.end_row();
                // Bars are lazy: a target has one only while it is damaged
                // or selected, and two quad entities (track and fill).
                ui.label("bar targets / entities");
                ui.monospace(format!(
                    "{} / {}",
                    bars.0.len(),
                    bar_entities.iter().count()
                ));
                ui.end_row();
                if let Ok(cam) = cameras.single() {
                    ui.label("focus");
                    ui.monospace(format!("{:.1}, {:.1}", cam.focus.x, cam.focus.z));
                    ui.end_row();
                    ui.label("zoom");
                    ui.monospace(format!("{:.1} m", cam.zoom));
                    ui.end_row();
                }
            });
        });
    Ok(())
}

/// Short label for a queued command.
fn command_kind(cmd: &sim::Command) -> &'static str {
    match cmd {
        sim::Command::Move { .. } => "Move",
        sim::Command::Stop { .. } => "Stop",
        sim::Command::AttackMove { .. } => "AttackMove",
        sim::Command::Attack { .. } => "Attack",
        sim::Command::Train { .. } => "Train",
        sim::Command::Research { .. } => "Research",
        sim::Command::AdvanceAge { .. } => "AdvanceAge",
        sim::Command::DebugSpawn { .. } => "DebugSpawn",
        sim::Command::Surrender => "Surrender",
        _ => "other",
    }
}
