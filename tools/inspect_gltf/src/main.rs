//! Command line of `inspect_gltf` (see the library docs and
//! `docs/ART_STYLE.md`): `cargo run -p inspect_gltf --release -- <command>`.

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "inspect_gltf",
    about = "Inspect, compare, bake and check glTF models for the art pass",
    version
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print scenes, the node tree, meshes, images, skins, animations and
    /// bounds of a .gltf or .glb file.
    Inspect {
        /// The file.
        file: PathBuf,
    },
    /// Compare the blue and red variants of pack models and print the team
    /// texel rects for `team_swatches` (pairs: BLUE RED [BLUE RED ...]).
    Swatches {
        /// Alternating blue and red files.
        #[arg(required = true, num_args = 2..)]
        files: Vec<PathBuf>,
    },
    /// Print the base texture's cell grid with each used cell's colour and
    /// the nodes sampling it.
    Palette {
        /// The file.
        file: PathBuf,
        /// Cells per side.
        #[arg(long, default_value_t = 16)]
        grid: u32,
    },
    /// Bake the characters of a bake.ron into static GLB files.
    Bake {
        /// The manifest (tools/assets/bake.ron).
        manifest: PathBuf,
        /// Bake only this kind (the output's file stem).
        #[arg(long)]
        only: Option<String>,
        /// Where the downloaded sources are (the fetch script puts them in
        /// <cache>/files).
        #[arg(long, default_value = "target/asset-cache/files")]
        cache: PathBuf,
        /// The assets directory the outputs are written under.
        #[arg(long, default_value = "assets")]
        assets: PathBuf,
    },
    /// Check every Scene of DATA/visuals.ron against the files under ASSETS.
    Check {
        /// The data directory.
        data: PathBuf,
        /// The assets directory.
        assets: PathBuf,
    },
}

fn run(cli: Cli) -> Result<bool, String> {
    match cli.cmd {
        Cmd::Inspect { file } => {
            print!("{}", inspect_gltf::inspect::inspect(&file)?);
            Ok(true)
        }
        Cmd::Swatches { files } => {
            if files.len() % 2 != 0 {
                return Err("swatches takes pairs: BLUE RED [BLUE RED ...]".into());
            }
            let pairs: Vec<(PathBuf, PathBuf)> = files
                .chunks(2)
                .map(|c| (c[0].clone(), c[1].clone()))
                .collect();
            let report = inspect_gltf::swatches::swatches(&pairs)?;
            print!("{}", report.text);
            Ok(report.shared == 0)
        }
        Cmd::Palette { file, grid } => {
            print!("{}", inspect_gltf::palette::palette(&file, grid)?);
            Ok(true)
        }
        Cmd::Bake {
            manifest,
            only,
            cache,
            assets,
        } => {
            print!(
                "{}",
                inspect_gltf::bake::bake(&manifest, &cache, &assets, only.as_deref())?
            );
            Ok(true)
        }
        Cmd::Check { data, assets } => {
            let report = inspect_gltf::check::check_dirs(&data, &assets)?;
            print!("{}", report.text());
            Ok(report.problems.is_empty())
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
