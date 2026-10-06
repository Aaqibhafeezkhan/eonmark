//! The `eonmark` binary: the windowed game, or a headless simulation run.
//!
//! Module map (M0):
//! - [`cli`]: hand-rolled argument parsing and data directory resolution.
//! - [`app`]: the windowed Bevy app, the `SimHandle` non-send resource and
//!   the fixed-step driver.
//! - [`camera`]: the yaw-locked RTS camera.
//! - [`ground`] and [`palette`]: the flat 128 x 128 m ground mesh and colours.
//! - [`headless`]: `--headless-run <ticks>` on `MinimalPlugins`.
//! - `dev_tools`: FPS overlay, egui inspector and sim panel (`dev` only).
#![forbid(unsafe_code)]

mod app;
mod camera;
mod cli;
#[cfg(feature = "dev")]
mod dev_tools;
mod ground;
mod headless;
mod palette;

use std::process::ExitCode;

use bevy::app::AppExit;

fn main() -> ExitCode {
    let cli = match cli::Cli::parse(std::env::args().skip(1)) {
        Ok(cli::Parsed::Run(cli)) => cli,
        Ok(cli::Parsed::Help) => {
            print!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("eonmark: {message}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };

    let exit = match cli.headless_run {
        Some(ticks) => headless::run(&cli, ticks),
        None => app::run(&cli),
    };

    match exit {
        AppExit::Success => ExitCode::SUCCESS,
        AppExit::Error(code) => ExitCode::from(code.get()),
    }
}
