//! Command-line flags. Hand-rolled over `std::env::args` so the game crate
//! adds no argument-parsing dependency (clap lives in `sim-cli` only).

use std::path::{Path, PathBuf};

/// Printed for `--help` and after a parse error.
pub const USAGE: &str = "\
Usage: eonmark [OPTIONS]

Options:
  --headless-run <TICKS>       Step the simulation TICKS times without a window,
                               print `tick=<n> hash=0x<16 hex>` and exit 0.
  --exit-after-seconds <S>     Send AppExit::Success after S seconds (smoke tests).
  --seed <U64>                 Match seed (default 1).
  --data-dir <PATH>            Override the data directory (default: see below).
  -h, --help                   Show this text.

Data directory resolution order:
  1. --data-dir
  2. $EONMARK_DATA
  3. <workspace>/data when running from a checkout
  4. <directory of the executable>/data
";

/// Parsed flags.
#[derive(Debug, Clone, PartialEq)]
pub struct Cli {
    /// `--headless-run <ticks>`.
    pub headless_run: Option<u32>,
    /// `--exit-after-seconds <s>`.
    pub exit_after_seconds: Option<f64>,
    /// `--seed <u64>`; defaults to 1 so `--headless-run` is reproducible.
    pub seed: u64,
    /// `--data-dir <path>`.
    pub data_dir: Option<PathBuf>,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            headless_run: None,
            exit_after_seconds: None,
            seed: 1,
            data_dir: None,
        }
    }
}

/// Outcome of parsing: run the game, or print usage.
#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    /// Run with these flags.
    Run(Cli),
    /// `--help` was given.
    Help,
}

impl Cli {
    /// Parse the arguments after the program name.
    pub fn parse<I>(args: I) -> Result<Parsed, String>
    where
        I: IntoIterator<Item = String>,
    {
        let mut cli = Cli::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => return Ok(Parsed::Help),
                "--headless-run" => {
                    cli.headless_run = Some(parse_value(&arg, args.next())?);
                }
                "--exit-after-seconds" => {
                    let secs: f64 = parse_value(&arg, args.next())?;
                    if !secs.is_finite() || secs < 0.0 {
                        return Err(format!("{arg}: expected a non-negative number"));
                    }
                    cli.exit_after_seconds = Some(secs);
                }
                "--seed" => cli.seed = parse_value(&arg, args.next())?,
                "--data-dir" => {
                    let value = args.next().ok_or_else(|| format!("{arg}: missing value"))?;
                    cli.data_dir = Some(PathBuf::from(value));
                }
                other => return Err(format!("unknown argument `{other}`")),
            }
        }
        Ok(Parsed::Run(cli))
    }

    /// Locate the `data/` directory (see [`USAGE`] for the order).
    pub fn resolve_data_dir(&self) -> PathBuf {
        if let Some(dir) = &self.data_dir {
            return dir.clone();
        }
        if let Some(dir) = std::env::var_os("EONMARK_DATA") {
            return PathBuf::from(dir);
        }
        let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        if checkout.is_dir() {
            return checkout;
        }
        if let Some(exe_dir) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
        {
            let beside_exe = exe_dir.join("data");
            if beside_exe.is_dir() {
                return beside_exe;
            }
        }
        PathBuf::from("data")
    }
}

fn parse_value<T>(flag: &str, value: Option<String>) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let value = value.ok_or_else(|| format!("{flag}: missing value"))?;
    value
        .parse::<T>()
        .map_err(|e| format!("{flag}: invalid value `{value}`: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Parsed, String> {
        Cli::parse(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn defaults_when_no_args() {
        assert_eq!(parse(&[]), Ok(Parsed::Run(Cli::default())));
    }

    #[test]
    fn parses_every_flag() {
        let parsed = parse(&[
            "--headless-run",
            "200",
            "--exit-after-seconds",
            "6",
            "--seed",
            "42",
            "--data-dir",
            "/tmp/data",
        ])
        .unwrap();
        assert_eq!(
            parsed,
            Parsed::Run(Cli {
                headless_run: Some(200),
                exit_after_seconds: Some(6.0),
                seed: 42,
                data_dir: Some(PathBuf::from("/tmp/data")),
            })
        );
    }

    #[test]
    fn help_short_circuits() {
        assert_eq!(parse(&["--seed", "3", "--help"]), Ok(Parsed::Help));
    }

    #[test]
    fn rejects_unknown_and_malformed() {
        assert!(parse(&["--nope"]).is_err());
        assert!(parse(&["--headless-run"]).is_err());
        assert!(parse(&["--headless-run", "x"]).is_err());
        assert!(parse(&["--exit-after-seconds", "-1"]).is_err());
        assert!(parse(&["--seed", "-1"]).is_err());
    }

    #[test]
    fn explicit_data_dir_wins() {
        let cli = Cli {
            data_dir: Some(PathBuf::from("/x/data")),
            ..Cli::default()
        };
        assert_eq!(cli.resolve_data_dir(), PathBuf::from("/x/data"));
    }
}
