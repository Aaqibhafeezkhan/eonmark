//! `--headless-run <ticks>`: step the simulation on `MinimalPlugins` with no
//! window, print the final tick and hash, exit 0. Used by the CI headless
//! job as a smoke test that the game crate and the sim agree.

use std::time::Duration;

use bevy::app::{AppExit, ScheduleRunnerPlugin};
use bevy::prelude::*;

use crate::app::{SimHandle, load_rules};
use crate::cli::Cli;

#[derive(Resource)]
struct Target(u32);

/// Run `ticks` steps and return the exit status for `main`.
pub fn run(cli: &Cli, ticks: u32) -> AppExit {
    let Some(rules) = load_rules(cli) else {
        return AppExit::error();
    };
    let handle = SimHandle::skirmish(rules, cli.seed);

    let mut app = App::new();
    app.add_plugins(MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::ZERO)));
    app.world_mut().insert_non_send(handle);
    app.insert_resource(Target(ticks));
    // One sim tick per app update, driven by the runner, never by wall time:
    // the fixed schedule is not used here so the tick count is exact.
    app.add_systems(Update, step_and_report);
    app.run()
}

fn step_and_report(
    mut sim: NonSendMut<SimHandle>,
    target: Res<Target>,
    mut exit: MessageWriter<AppExit>,
) {
    if sim.tick() < target.0 {
        sim.step_once();
    }
    if sim.tick() >= target.0 {
        println!("tick={} hash={:#018x}", sim.tick(), sim.hash());
        exit.write(AppExit::Success);
    }
}
