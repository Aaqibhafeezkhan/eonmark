//! Headless command-line tooling for the Eonmark simulation.
//!
//! Runs on any OS without a GPU. M0 implements `selftest`, `data-check` and
//! `hash-dump`; `verify`, `bench` and `fuzz` land in M1 and `play-bots` in
//! M5b (they exit 2 until then).
#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use sim::{MatchSetup, Rules, Sim};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "sim-cli",
    about = "Headless Eonmark simulation tooling",
    version
)]
struct Cli {
    /// Data directory (rules, maps).
    #[arg(long, global = true, default_value = "data")]
    data: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run scripted ticks twice on two threads and compare hashes.
    Selftest {
        /// Ticks to simulate in each run.
        #[arg(long, default_value_t = 1000)]
        ticks: u32,
        /// Match seed.
        #[arg(long, default_value_t = 42)]
        seed: u64,
    },
    /// Load and validate everything under a data directory.
    DataCheck {
        /// Path to the data directory (overrides --data).
        dir: Option<PathBuf>,
    },
    /// Run the scripted match once and print the state hash every tick.
    HashDump {
        /// Ticks to simulate.
        #[arg(long, default_value_t = 100)]
        ticks: u32,
        /// Match seed.
        #[arg(long, default_value_t = 42)]
        seed: u64,
    },
    /// Re-simulate a replay and compare hashes (M1).
    Verify,
    /// Measure step time with N units (M1).
    Bench,
    /// Run seeded bot-vs-bot matches in parallel (M5b).
    PlayBots,
    /// Feed random command streams to the sim (M1).
    Fuzz,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Selftest { ticks, seed } => selftest(&cli.data, ticks, seed),
        Cmd::DataCheck { dir } => data_check(dir.as_deref().unwrap_or(&cli.data)),
        Cmd::HashDump { ticks, seed } => hash_dump(&cli.data, ticks, seed),
        Cmd::Verify => not_yet("verify", "M1"),
        Cmd::Bench => not_yet("bench", "M1"),
        Cmd::Fuzz => not_yet("fuzz", "M1"),
        Cmd::PlayBots => not_yet("play-bots", "M5b"),
    }
}

fn not_yet(name: &str, milestone: &str) -> ExitCode {
    eprintln!("{name}: not implemented until {milestone}");
    ExitCode::from(2)
}

/// Build the scripted M0 match: human slot driven by
/// `ai::selftest_human_commands`, AI slot by `ai::Scripted::selftest`.
fn scripted_sim(data: &Path, seed: u64) -> Result<Sim, rules::Error> {
    let rules = Rules::load(data)?;
    let setup = MatchSetup::skirmish(&rules, seed);
    Ok(Sim::new(setup, rules, Box::new(ai::Scripted::selftest())))
}

fn run_scripted(data: &Path, ticks: u32, seed: u64) -> Result<u64, rules::Error> {
    let mut sim = scripted_sim(data, seed)?;
    for t in 0..ticks {
        sim.step(&ai::selftest_human_commands(t));
    }
    Ok(sim.hash())
}

fn selftest(data: &Path, ticks: u32, seed: u64) -> ExitCode {
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| run_scripted(data, ticks, seed));
        let b = s.spawn(|| run_scripted(data, ticks, seed));
        (a.join().expect("thread a"), b.join().expect("thread b"))
    });
    let (a, b) = match (a, b) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };
    println!("selftest ticks={ticks} seed={seed}");
    println!("thread A hash=0x{a:016x}");
    println!("thread B hash=0x{b:016x}");
    if a == b {
        println!("OK");
        ExitCode::SUCCESS
    } else {
        println!("MISMATCH");
        ExitCode::from(1)
    }
}

fn data_check(dir: &Path) -> ExitCode {
    match Rules::load(dir) {
        Ok(r) => {
            println!(
                "OK rules_version={} rules_hash=0x{:016x} resources={} maps={}",
                r.rules_version,
                r.rules_hash(),
                r.resources.resources.len(),
                r.maps.keys().cloned().collect::<Vec<_>>().join(",")
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn hash_dump(data: &Path, ticks: u32, seed: u64) -> ExitCode {
    let mut sim = match scripted_sim(data, seed) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };
    println!("tick hash");
    for t in 0..ticks {
        sim.step(&ai::selftest_human_commands(t));
        println!("{} 0x{:016x}", sim.tick(), sim.hash());
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    #[test]
    fn scripted_run_is_repeatable() {
        let a = run_scripted(&data(), 200, 42).unwrap();
        let b = run_scripted(&data(), 200, 42).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, run_scripted(&data(), 200, 43).unwrap());
    }

    #[test]
    fn cli_parses_stubs() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["sim-cli", "verify"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Verify));
        let cli =
            Cli::try_parse_from(["sim-cli", "--data", "x", "hash-dump", "--ticks", "3"]).unwrap();
        assert_eq!(cli.data, PathBuf::from("x"));
        assert!(matches!(cli.cmd, Cmd::HashDump { ticks: 3, .. }));
    }
}
