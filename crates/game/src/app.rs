//! The windowed Bevy app plus the shared simulation driver.
//!
//! The simulation is a `!Send` value at M0 (`Box<dyn AiController>` carries
//! no `Send + Sync` bound), so it lives in a non-send resource and is only
//! touched by main-thread systems. See `notesForOtherFiles` in the M0 report.

use std::time::Duration;

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::window::{PresentMode, PrimaryWindow, WindowResolution};
use rules::Rules;
use sim::{MatchSetup, PlayerCommand, Sim};

use crate::cli::Cli;
use crate::{camera, ground, palette};

/// Hard cap on simulation ticks stepped in one rendered frame. With
/// `Time<Virtual>::max_delta` at 250 ms and a 20 Hz tick the fixed schedule
/// never asks for more than 5, so this only guards against a future
/// higher tick rate or a larger max delta.
pub const MAX_TICKS_PER_FRAME: u32 = 8;

/// Default logical window size.
pub const WINDOW_SIZE: (u32, u32) = (1280, 800);

/// The running match. Non-send resource; see the module docs.
pub struct SimHandle {
    sim: Sim,
    /// Ticks skipped because [`MAX_TICKS_PER_FRAME`] was reached.
    pub dropped_ticks: u32,
}

impl SimHandle {
    /// Build a skirmish from loaded rules and a seed with the passive bot.
    pub fn skirmish(rules: Rules, seed: u64) -> Self {
        let setup = MatchSetup::skirmish(&rules, seed);
        Self {
            sim: Sim::new(setup, rules, Box::new(ai::Passive)),
            dropped_ticks: 0,
        }
    }

    /// Advance one tick with no human commands (M0: no input mapping yet).
    pub fn step_once(&mut self) {
        let no_commands: [PlayerCommand; 0] = [];
        self.sim.step(&no_commands);
    }

    /// Completed ticks.
    pub fn tick(&self) -> u32 {
        self.sim.tick()
    }

    /// Hash of the current simulation state.
    pub fn hash(&self) -> u64 {
        self.sim.hash()
    }

    /// Ticks per second the match runs at.
    pub fn tick_rate_hz(&self) -> u32 {
        self.sim.rules().tick_rate_hz
    }

    /// Seed the match was created with.
    pub fn seed(&self) -> u64 {
        self.sim.setup().seed
    }
}

/// Load `data/` for `cli`, or print why it failed.
pub fn load_rules(cli: &Cli) -> Option<Rules> {
    let dir = cli.resolve_data_dir();
    match Rules::load(&dir) {
        Ok(rules) => Some(rules),
        Err(err) => {
            eprintln!("eonmark: cannot load rules from {}: {err}", dir.display());
            None
        }
    }
}

#[derive(Resource, Default)]
struct TickBudget {
    used: u32,
}

/// Install the sim as a non-send resource and drive it from `FixedUpdate`
/// at the rules' tick rate. Not a `Plugin`: `Plugin` values must be
/// `Send + Sync` and the sim is not (see the module docs).
pub fn install_sim(app: &mut App, handle: SimHandle) {
    let hz = f64::from(handle.tick_rate_hz());
    app.world_mut().insert_non_send(handle);
    app.insert_resource(Time::<Fixed>::from_hz(hz))
        .init_resource::<TickBudget>()
        .add_systems(Startup, configure_virtual_time)
        .add_systems(PreUpdate, reset_tick_budget)
        .add_systems(FixedUpdate, step_sim);
}

fn configure_virtual_time(mut time: ResMut<Time<Virtual>>) {
    time.set_max_delta(Duration::from_millis(250));
}

fn reset_tick_budget(mut budget: ResMut<TickBudget>) {
    budget.used = 0;
}

fn step_sim(mut sim: NonSendMut<SimHandle>, mut budget: ResMut<TickBudget>) {
    if budget.used >= MAX_TICKS_PER_FRAME {
        sim.dropped_ticks += 1;
        return;
    }
    budget.used += 1;
    sim.step_once();
}

/// `--exit-after-seconds`: wall-clock deadline for smoke tests.
#[derive(Resource)]
struct ExitAfter(Duration);

/// Run the windowed game. Returns the exit status for `main`.
pub fn run(cli: &Cli) -> AppExit {
    let Some(rules) = load_rules(cli) else {
        return AppExit::error();
    };
    let handle = SimHandle::skirmish(rules, cli.seed);

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Eonmark".into(),
            // Logical size: winit reads this as a LogicalSize at creation.
            resolution: WindowResolution::new(WINDOW_SIZE.0, WINDOW_SIZE.1),
            present_mode: PresentMode::AutoVsync,
            ..default()
        }),
        ..default()
    }));
    app.insert_resource(ClearColor(palette::SKY));
    install_sim(&mut app, handle);
    app.add_plugins((camera::CameraPlugin, ground::GroundPlugin));
    app.add_systems(Update, show_tick_in_title);

    #[cfg(feature = "dev")]
    app.add_plugins(crate::dev_tools::DevToolsPlugin);

    if let Some(secs) = cli.exit_after_seconds {
        app.insert_resource(ExitAfter(Duration::from_secs_f64(secs)))
            .add_systems(Update, exit_after);
    }

    app.run()
}

fn show_tick_in_title(
    sim: NonSend<SimHandle>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut last_tick: Local<Option<u32>>,
) {
    let tick = sim.tick();
    if *last_tick == Some(tick) {
        return;
    }
    *last_tick = Some(tick);
    if let Ok(mut window) = windows.single_mut() {
        window.title = format!(
            "Eonmark  seed {}  tick {tick}  hash {:#018x}",
            sim.seed(),
            sim.hash()
        );
    }
}

fn exit_after(
    time: Res<Time<Real>>,
    deadline: Res<ExitAfter>,
    sim: NonSend<SimHandle>,
    mut exit: MessageWriter<AppExit>,
) {
    if time.elapsed() >= deadline.0 {
        info!(
            "exit-after-seconds reached: tick={} hash={:#018x}",
            sim.tick(),
            sim.hash()
        );
        exit.write(AppExit::Success);
    }
}
