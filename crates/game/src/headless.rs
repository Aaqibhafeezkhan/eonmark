//! `--headless-run <ticks | path.eonreplay>`: no window, `MinimalPlugins`.
//!
//! `<ticks>` (M0): step the default skirmish `ticks` times with no commands
//! and print `tick=<n> hash=0x<16 hex>`; exit 0. The CI smoke test on both
//! operating systems diffs that line.
//!
//! `<path.eonreplay>` (M2): build the sim from the replay header, drive it
//! with `sim_driver::ReplayInput` through `sim_driver::drive_tick` (no
//! recorder, no fixed clock), and when the source is finished print the same
//! `tick=<n> hash=0x<16 hex>` line as the LAST stdout line and exit 0 if
//! the recorded final hash matches, 1 otherwise. A `RULES CHANGED` or
//! `SIM VERSION MISMATCH` header is reported on stderr with exit 1 before
//! simulating, like `sim-cli verify`; a file that does not decode is
//! `error: ...` with exit 2; a truncated recording (no clean-exit trailer)
//! is replayed up to its last complete record with a `warning:` on stderr.
//! A replay plays on past the match's outcome to its recorded end (M5a:
//! only a live windowed source freezes there), so `capture_seat` ends at
//! its recorded deciding tick and `move_500` at its 1200.
//! `move_500` (1200 ticks) finishes in well under 10 s on the dev Mac in
//! the `ci` profile because the runner steps [`REPLAY_TICKS_PER_UPDATE`]
//! ticks per `Update` and never waits on wall time (`FixedUpdate` would).

use std::path::Path;
use std::time::Duration;

use bevy::app::{AppExit, ScheduleRunnerPlugin};
use bevy::prelude::*;

use sim::SIM_VERSION;
use sim::replay::{ReplayError, VerifyOutcome};

use crate::app::load_rules;
use crate::cli::{Cli, HeadlessRun};
use crate::sim_driver::{
    ActiveInput, DriverStats, HashCadence, PendingCommands, ReplayInput, SimHandle, TickOutcome,
    drive_tick,
};

/// Ticks the replay runner steps per `Update` before letting the schedule
/// run once more: large enough that the runner's per-update cost is noise,
/// small enough that `AppExit` is seen promptly.
pub const REPLAY_TICKS_PER_UPDATE: u32 = 256;

#[derive(Resource)]
struct Target(u32);

/// Run the requested headless mode and return the exit status for `main`.
pub fn run(cli: &Cli, mode: &HeadlessRun) -> AppExit {
    match mode {
        HeadlessRun::Ticks(ticks) => run_ticks(cli, *ticks),
        HeadlessRun::Replay(path) => run_replay(cli, path),
    }
}

/// The exact stdout line both headless modes end with.
pub fn hash_line(tick: u32, hash: u64) -> String {
    format!("tick={tick} hash={hash:#018x}")
}

/// `--headless-run <ticks>`: one sim tick per app update, driven by the
/// runner, never by wall time, so the tick count is exact.
fn run_ticks(cli: &Cli, ticks: u32) -> AppExit {
    let Some(rules) = load_rules(cli) else {
        return AppExit::error();
    };
    let handle = SimHandle::skirmish(rules, cli.seed);

    let mut app = App::new();
    app.add_plugins(MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::ZERO)));
    app.world_mut().insert_non_send(handle);
    app.insert_resource(Target(ticks));
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
        println!("{}", hash_line(sim.tick(), sim.hash()));
        exit.write(AppExit::Success);
    }
}

/// `--headless-run <path.eonreplay>`. See the module docs for the contract.
fn run_replay(cli: &Cli, path: &Path) -> AppExit {
    let Some(rules) = load_rules(cli) else {
        return AppExit::error();
    };
    let (setup, input) = match ReplayInput::open(path) {
        Ok(pair) => pair,
        // A header from another sim version is the verify outcome, exit 1.
        Err(ReplayError::SimVersion { found, .. }) => {
            eprintln!(
                "{}",
                VerifyOutcome::SimVersionMismatch {
                    replay: found,
                    binary: SIM_VERSION,
                }
            );
            return AppExit::error();
        }
        Err(e) => {
            eprintln!("error: {e}");
            return AppExit::from_code(2);
        }
    };
    // Header checks in `sim-cli verify` order: version, then rules, before
    // anything is simulated. Both lines are `VerifyOutcome`'s own text.
    if setup.sim_version != SIM_VERSION {
        eprintln!(
            "{}",
            VerifyOutcome::SimVersionMismatch {
                replay: setup.sim_version,
                binary: SIM_VERSION,
            }
        );
        return AppExit::error();
    }
    if setup.rules_hash != rules.rules_hash() {
        eprintln!(
            "{}",
            VerifyOutcome::RulesChanged {
                replay: setup.rules_hash,
                loaded: rules.rules_hash(),
            }
        );
        return AppExit::error();
    }
    if let Some(warning) = input.truncation_warning() {
        eprintln!("{warning}");
    }
    // The header's own bots (M5b), as `sim-cli verify` builds them; an
    // unknown bot or personality is an error like any unreadable header.
    let handle = match SimHandle::try_from_setup(setup, rules) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("error: {e}");
            return AppExit::from_code(2);
        }
    };

    let mut app = App::new();
    app.add_plugins(MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::ZERO)));
    app.world_mut().insert_non_send(handle);
    app.insert_resource(ActiveInput(Box::new(input)))
        .insert_resource(HashCadence {
            every: cli.hash_every(),
        })
        .init_resource::<PendingCommands>()
        .init_resource::<DriverStats>()
        .add_systems(Update, replay_and_report);
    app.run()
}

/// Step up to [`REPLAY_TICKS_PER_UPDATE`] ticks; at the end of the
/// recording print the hash line and exit 0 (match) or 1 (divergence).
fn replay_and_report(
    mut sim: NonSendMut<SimHandle>,
    mut input: ResMut<ActiveInput>,
    mut pending: ResMut<PendingCommands>,
    cadence: Res<HashCadence>,
    mut stats: ResMut<DriverStats>,
    mut exit: MessageWriter<AppExit>,
) {
    for _ in 0..REPLAY_TICKS_PER_UPDATE {
        match drive_tick(
            &mut sim,
            input.0.as_mut(),
            &mut pending,
            None,
            *cadence,
            &mut stats,
        ) {
            TickOutcome::Stepped { .. } => {}
            TickOutcome::Stalled => {
                // A replay source never stalls before its end; if one did,
                // looping here would spin forever.
                eprintln!("eonmark: replay source stalled at tick {}", sim.tick());
                exit.write(AppExit::error());
                return;
            }
            TickOutcome::Decided { tick } => {
                // Only a live source freezes at the outcome; a replay plays
                // to its recorded end, so this is a driver bug.
                eprintln!("eonmark: replay source froze at its outcome at tick {tick}");
                exit.write(AppExit::error());
                return;
            }
            TickOutcome::Finished(f) => {
                if !f.matches() {
                    eprintln!(
                        "DIVERGED at tick {} (recorded {:#018x}, re-simulated {:#018x})",
                        f.final_tick,
                        f.recorded_hash.unwrap_or_default(),
                        f.sim_hash
                    );
                }
                println!("{}", hash_line(f.final_tick, f.sim_hash));
                exit.write(if f.matches() {
                    AppExit::Success
                } else {
                    AppExit::error()
                });
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_line_is_the_ci_format() {
        assert_eq!(hash_line(200, 0x1f), "tick=200 hash=0x000000000000001f");
    }
}
