//! Command-line flags. Hand-rolled over `std::env::args` so the game crate
//! adds no argument-parsing dependency (clap lives in `sim-cli` only).
//!
//! M2 flags and who wires their behaviour (see the M2 contract):
//! `--headless-run <ticks | path.eonreplay>` (replay mode: agent A),
//! `--scenario <name>` (A), `--max-fps <n>` (A), `--replay-dir <dir>` (A),
//! `--hash-every-tick` (A), `--screenshot <path>` (B, dev builds only),
//! `--background` (`crate::background`; also `EONMARK_BACKGROUND=1`).
//! Parsing is complete and tested here.

use std::path::{Path, PathBuf};

/// Printed for `--help` and after a parse error.
pub const USAGE: &str = "\
Usage: eonmark [OPTIONS]

Options:
  --headless-run <TICKS|FILE>  No window. TICKS: step the skirmish TICKS times.
                               FILE (ending in .eonreplay): re-simulate the replay and
                               exit 0 if the final recorded hash matches. Either way the
                               last stdout line is `tick=<n> hash=0x<16 hex>`.
  --scenario <NAME>            Windowed scenario (sim::scenarios::by_name): the scripted
                               commands run alongside your own input.
  --max-fps <N>                Cap the frame rate at N frames per second (sleep in Last,
                               vsync off); the simulation stays at its tick rate.
  --replay-dir <DIR>           Write this session's replay under DIR instead of the
                               platform data directory.
  --hash-every-tick            Record a hash checkpoint every tick instead of every 20.
  --screenshot <PATH>          Dev builds: save a PNG of the window about one second
                               before --exit-after-seconds fires.
  --screenshot-at <TICK>:<PATH>
                               Save a PNG of the window once the sim has reached TICK
                               (repeatable; scripted scenarios' beats, M3b proxies).
  --quit-via-menu-after-seconds <S>
                               Dev builds, macOS: after S seconds feed the `Quit Eonmark`
                               menu id through the same path a real Cmd-Q takes
                               (AppExit::Success, replay trailer). Ignored elsewhere.
  --close-window-after-seconds <S>
                               Dev builds: after S seconds request the primary window to
                               close, the path the red close button takes (bevy_window
                               despawns it, exit_on_all_closed writes AppExit::Success).
  --exit-after-seconds <S>     Send AppExit::Success after S seconds (smoke tests).
  --camera <X>,<Y>,<ZOOM>      Start the camera over sim tile (X, Y) at ZOOM metres
                               (15 to 60) instead of jumping to the first units
                               (screenshots, stream framing).
  --background                 Automated runs: open the window unfocused, below other
                               windows, in the top-left corner of the primary monitor,
                               and (macOS) hand focus back to the app that had it. Also
                               on when EONMARK_BACKGROUND=1. Ignored by --headless-run.
  --seed <U64>                 Match seed (default 1).
  --data-dir <PATH>            Override the data directory (default: see below).
  -h, --help                   Show this text.

Data directory resolution order:
  1. --data-dir
  2. $EONMARK_DATA
  3. <workspace>/data when running from a checkout
  4. <directory of the executable>/data
";

/// What `--headless-run` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadlessRun {
    /// Step the default skirmish this many ticks with no commands (M0).
    Ticks(u32),
    /// Re-simulate an `.eonreplay` file and compare its final hash (M2).
    Replay(PathBuf),
}

impl HeadlessRun {
    /// `<ticks>` parses as a number; anything ending in `.eonreplay` is a
    /// replay path. A non-numeric value without that suffix is an error so
    /// a typo never silently becomes a path.
    pub fn parse(value: &str) -> Result<HeadlessRun, String> {
        if value.ends_with(".eonreplay") {
            return Ok(HeadlessRun::Replay(PathBuf::from(value)));
        }
        value
            .parse::<u32>()
            .map(HeadlessRun::Ticks)
            .map_err(|e| format!("expected a tick count or a .eonreplay path, got `{value}`: {e}"))
    }
}

/// Where `--camera` starts the camera: over sim tile `(x, y)` (fractional
/// tiles allowed) at `zoom` metres from the ground.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraStart {
    /// Sim tile x.
    pub x: f32,
    /// Sim tile y (world z).
    pub y: f32,
    /// Distance from the focus, metres.
    pub zoom: f32,
}

impl CameraStart {
    /// Parse `<x>,<y>,<zoom>`: three finite numbers, `x` and `y` inside the
    /// 128-tile map, `zoom` within the camera's range.
    pub fn parse(value: &str) -> Result<CameraStart, String> {
        let parts: Vec<f32> = value
            .split(',')
            .map(|p| p.trim().parse::<f32>().map_err(|e| format!("`{p}`: {e}")))
            .collect::<Result<_, _>>()?;
        let [x, y, zoom] = parts[..] else {
            return Err(format!("expected <x>,<y>,<zoom>, got `{value}`"));
        };
        if !(0.0..=128.0).contains(&x) || !(0.0..=128.0).contains(&y) {
            return Err(format!("({x}, {y}) is outside the 128-tile map"));
        }
        if !(crate::camera::ZOOM_MIN..=crate::camera::ZOOM_MAX).contains(&zoom) {
            return Err(format!(
                "zoom {zoom} is outside {}..={}",
                crate::camera::ZOOM_MIN,
                crate::camera::ZOOM_MAX
            ));
        }
        Ok(CameraStart { x, y, zoom })
    }
}

/// Parsed flags.
#[derive(Debug, Clone, PartialEq)]
pub struct Cli {
    /// `--headless-run <ticks | path.eonreplay>`.
    pub headless_run: Option<HeadlessRun>,
    /// `--exit-after-seconds <s>`.
    pub exit_after_seconds: Option<f64>,
    /// `--seed <u64>`; defaults to 1 so `--headless-run` is reproducible.
    pub seed: u64,
    /// `--data-dir <path>`.
    pub data_dir: Option<PathBuf>,
    /// `--scenario <name>`; validated against `sim::scenarios::by_name` at
    /// start-up, not here (the parser knows nothing about rules).
    pub scenario: Option<String>,
    /// `--max-fps <n>`; `None` is uncapped (vsync still applies).
    pub max_fps: Option<u32>,
    /// `--replay-dir <dir>`.
    pub replay_dir: Option<PathBuf>,
    /// `--hash-every-tick`.
    pub hash_every_tick: bool,
    /// `--screenshot <path>`.
    pub screenshot: Option<PathBuf>,
    /// `--quit-via-menu-after-seconds <s>`: the automated Cmd-Q proxy
    /// (`macos_menu`, dev builds on macOS). Parsed everywhere so the flag
    /// is never "unknown"; other configurations print a note and ignore it.
    pub quit_via_menu_after_seconds: Option<f64>,
    /// `--close-window-after-seconds <s>`: the automated red-close-button
    /// proxy (`app::close_window_after`).
    pub close_window_after_seconds: Option<f64>,
    /// `--background`: background mode for automated windowed runs
    /// (`crate::background`). [`Cli::background_mode`] also honours
    /// `EONMARK_BACKGROUND`.
    pub background: bool,
    /// `--camera <x>,<y>,<zoom>`.
    pub camera: Option<CameraStart>,
    /// `--screenshot-at <tick>:<path>`, in the order given.
    pub screenshot_at: Vec<(u32, PathBuf)>,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            headless_run: None,
            exit_after_seconds: None,
            seed: 1,
            data_dir: None,
            scenario: None,
            max_fps: None,
            replay_dir: None,
            hash_every_tick: false,
            screenshot: None,
            quit_via_menu_after_seconds: None,
            close_window_after_seconds: None,
            background: false,
            camera: None,
            screenshot_at: Vec::new(),
        }
    }
}

/// Outcome of parsing: run the game, or print usage. Built once at start-up,
/// so the size of the `Run` variant does not matter.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
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
                    let value = args.next().ok_or_else(|| format!("{arg}: missing value"))?;
                    cli.headless_run =
                        Some(HeadlessRun::parse(&value).map_err(|e| format!("{arg}: {e}"))?);
                }
                "--exit-after-seconds" => {
                    let secs: f64 = parse_value(&arg, args.next())?;
                    if !secs.is_finite() || secs < 0.0 {
                        return Err(format!("{arg}: expected a non-negative number"));
                    }
                    cli.exit_after_seconds = Some(secs);
                }
                "--seed" => cli.seed = parse_value(&arg, args.next())?,
                "--data-dir" => cli.data_dir = Some(parse_path(&arg, args.next())?),
                "--scenario" => {
                    let name = args.next().ok_or_else(|| format!("{arg}: missing value"))?;
                    if name.is_empty() || name.starts_with("--") {
                        return Err(format!("{arg}: expected a scenario name"));
                    }
                    cli.scenario = Some(name);
                }
                "--max-fps" => {
                    let fps: u32 = parse_value(&arg, args.next())?;
                    if fps == 0 {
                        return Err(format!("{arg}: must be at least 1"));
                    }
                    cli.max_fps = Some(fps);
                }
                "--replay-dir" => cli.replay_dir = Some(parse_path(&arg, args.next())?),
                "--hash-every-tick" => cli.hash_every_tick = true,
                "--screenshot" => cli.screenshot = Some(parse_path(&arg, args.next())?),
                "--quit-via-menu-after-seconds" => {
                    let secs: f64 = parse_value(&arg, args.next())?;
                    if !secs.is_finite() || secs < 0.0 {
                        return Err(format!("{arg}: expected a non-negative number"));
                    }
                    cli.quit_via_menu_after_seconds = Some(secs);
                }
                "--close-window-after-seconds" => {
                    let secs: f64 = parse_value(&arg, args.next())?;
                    if !secs.is_finite() || secs < 0.0 {
                        return Err(format!("{arg}: expected a non-negative number"));
                    }
                    cli.close_window_after_seconds = Some(secs);
                }
                "--background" => cli.background = true,
                "--screenshot-at" => {
                    let value = args.next().ok_or_else(|| format!("{arg}: missing value"))?;
                    let (tick, path) = value
                        .split_once(':')
                        .ok_or_else(|| format!("{arg}: expected <tick>:<path>, got `{value}`"))?;
                    let tick: u32 = tick
                        .parse()
                        .map_err(|e| format!("{arg}: tick `{tick}`: {e}"))?;
                    if path.is_empty() {
                        return Err(format!("{arg}: missing path after `{tick}:`"));
                    }
                    cli.screenshot_at.push((tick, PathBuf::from(path)));
                }
                "--camera" => {
                    let value = args.next().ok_or_else(|| format!("{arg}: missing value"))?;
                    cli.camera =
                        Some(CameraStart::parse(&value).map_err(|e| format!("{arg}: {e}"))?);
                }
                other => return Err(format!("unknown argument `{other}`")),
            }
        }
        if cli.headless_run.is_some() {
            if cli.scenario.is_some() {
                return Err("--scenario needs a window; drop --headless-run".into());
            }
            if cli.screenshot.is_some() {
                return Err("--screenshot needs a window; drop --headless-run".into());
            }
            if cli.camera.is_some() {
                return Err("--camera needs a window; drop --headless-run".into());
            }
            if !cli.screenshot_at.is_empty() {
                return Err("--screenshot-at needs a window; drop --headless-run".into());
            }
            if cli.quit_via_menu_after_seconds.is_some() {
                return Err(
                    "--quit-via-menu-after-seconds needs a window; drop --headless-run".into(),
                );
            }
            if cli.close_window_after_seconds.is_some() {
                return Err(
                    "--close-window-after-seconds needs a window; drop --headless-run".into(),
                );
            }
        }
        Ok(Parsed::Run(cli))
    }

    /// Hash checkpoint cadence in ticks: 1 under `--hash-every-tick`, else
    /// the replay default (20).
    pub fn hash_every(&self) -> u32 {
        if self.hash_every_tick {
            1
        } else {
            sim::replay::DEFAULT_HASH_EVERY
        }
    }

    /// Background mode: `--background`, or [`BACKGROUND_ENV`] set to `1`
    /// (or `true` / `yes`) in the environment.
    pub fn background_mode(&self) -> bool {
        self.background || env_enables(std::env::var_os(BACKGROUND_ENV).as_deref())
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

/// Environment variable that turns on background mode like `--background`;
/// `scripts/m2_checks.sh` and the `just` recipes that run the window export
/// it so automated runs never come to the front.
pub const BACKGROUND_ENV: &str = "EONMARK_BACKGROUND";

/// `true` for `1`, `true` or `yes` (any case, surrounding spaces ignored);
/// unset, empty, `0` and anything else leave background mode off.
pub fn env_enables(value: Option<&std::ffi::OsStr>) -> bool {
    value
        .and_then(std::ffi::OsStr::to_str)
        .map(|v| v.trim().to_ascii_lowercase())
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"))
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

fn parse_path(flag: &str, value: Option<String>) -> Result<PathBuf, String> {
    let value = value.ok_or_else(|| format!("{flag}: missing value"))?;
    if value.is_empty() {
        return Err(format!("{flag}: expected a path"));
    }
    Ok(PathBuf::from(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Parsed, String> {
        Cli::parse(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn camera_takes_a_tile_and_a_zoom_inside_the_map() {
        let Ok(Parsed::Run(cli)) = parse(&["--camera", "24,64,60"]) else {
            panic!("--camera parses");
        };
        assert_eq!(
            cli.camera,
            Some(CameraStart {
                x: 24.0,
                y: 64.0,
                zoom: 60.0
            })
        );
        assert!(parse(&["--camera", "24,64"]).is_err(), "three values");
        assert!(parse(&["--camera", "24,200,40"]).is_err(), "inside the map");
        assert!(parse(&["--camera", "24,64,5"]).is_err(), "zoom range");
        assert!(parse(&["--camera", "a,64,40"]).is_err(), "numbers");
        assert!(
            parse(&["--headless-run", "10", "--camera", "24,64,40"]).is_err(),
            "needs a window"
        );
    }

    #[test]
    fn screenshot_at_takes_tick_path_pairs_in_order() {
        let Ok(Parsed::Run(cli)) = parse(&[
            "--screenshot-at",
            "20:/tmp/a.png",
            "--screenshot-at",
            "2100:/tmp/b.png",
        ]) else {
            panic!("--screenshot-at parses");
        };
        assert_eq!(
            cli.screenshot_at,
            vec![
                (20, PathBuf::from("/tmp/a.png")),
                (2100, PathBuf::from("/tmp/b.png"))
            ]
        );
        assert!(parse(&["--screenshot-at", "x:/tmp/a.png"]).is_err());
        assert!(parse(&["--screenshot-at", "/tmp/a.png"]).is_err());
        assert!(parse(&["--screenshot-at", "20:"]).is_err());
    }

    #[test]
    fn defaults_when_no_args() {
        assert_eq!(parse(&[]), Ok(Parsed::Run(Cli::default())));
        assert_eq!(Cli::default().hash_every(), 20);
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
            "--max-fps",
            "30",
            "--replay-dir",
            "/tmp/replays",
            "--hash-every-tick",
        ])
        .unwrap();
        assert_eq!(
            parsed,
            Parsed::Run(Cli {
                headless_run: Some(HeadlessRun::Ticks(200)),
                exit_after_seconds: Some(6.0),
                seed: 42,
                data_dir: Some(PathBuf::from("/tmp/data")),
                scenario: None,
                max_fps: Some(30),
                replay_dir: Some(PathBuf::from("/tmp/replays")),
                hash_every_tick: true,
                screenshot: None,
                quit_via_menu_after_seconds: None,
                close_window_after_seconds: None,
                background: false,
                camera: None,
                screenshot_at: Vec::new(),
            })
        );
        let Parsed::Run(cli) = parsed else {
            unreachable!()
        };
        assert_eq!(cli.hash_every(), 1);
    }

    #[test]
    fn windowed_flags_parse() {
        let parsed = parse(&[
            "--scenario",
            "units200",
            "--screenshot",
            "/tmp/shot.png",
            "--exit-after-seconds",
            "4",
        ])
        .unwrap();
        assert_eq!(
            parsed,
            Parsed::Run(Cli {
                scenario: Some("units200".into()),
                screenshot: Some(PathBuf::from("/tmp/shot.png")),
                exit_after_seconds: Some(4.0),
                ..Cli::default()
            })
        );
    }

    #[test]
    fn quit_via_menu_proxy_flag_parses() {
        let parsed = parse(&[
            "--quit-via-menu-after-seconds",
            "3",
            "--exit-after-seconds",
            "20",
        ])
        .unwrap();
        assert_eq!(
            parsed,
            Parsed::Run(Cli {
                quit_via_menu_after_seconds: Some(3.0),
                exit_after_seconds: Some(20.0),
                ..Cli::default()
            })
        );
        assert!(parse(&["--quit-via-menu-after-seconds"]).is_err());
        assert!(parse(&["--quit-via-menu-after-seconds", "-1"]).is_err());
        assert!(parse(&["--quit-via-menu-after-seconds", "soon"]).is_err());
        assert!(parse(&["--headless-run", "10", "--quit-via-menu-after-seconds", "1"]).is_err());
    }

    #[test]
    fn close_window_proxy_flag_parses() {
        let parsed = parse(&["--close-window-after-seconds", "2.5"]).unwrap();
        assert_eq!(
            parsed,
            Parsed::Run(Cli {
                close_window_after_seconds: Some(2.5),
                ..Cli::default()
            })
        );
        assert!(parse(&["--close-window-after-seconds"]).is_err());
        assert!(parse(&["--close-window-after-seconds", "-1"]).is_err());
        assert!(parse(&["--headless-run", "10", "--close-window-after-seconds", "1"]).is_err());
    }

    #[test]
    fn background_flag_and_environment() {
        assert_eq!(
            parse(&["--background", "--exit-after-seconds", "6"]),
            Ok(Parsed::Run(Cli {
                background: true,
                exit_after_seconds: Some(6.0),
                ..Cli::default()
            }))
        );
        // Scripts export EONMARK_BACKGROUND=1 around headless runs too.
        assert!(parse(&["--headless-run", "10", "--background"]).is_ok());
        let on = |v: &str| env_enables(Some(std::ffi::OsStr::new(v)));
        assert!(on("1") && on("true") && on(" YES "));
        assert!(!on("0") && !on("") && !on("false") && !on("2"));
        assert!(!env_enables(None));
        let flag = Cli {
            background: true,
            ..Cli::default()
        };
        assert!(
            flag.background_mode(),
            "the flag wins whatever the environment says"
        );
    }

    #[test]
    fn headless_run_takes_ticks_or_a_replay_path() {
        assert_eq!(
            parse(&[
                "--headless-run",
                "crates/sim/tests/fixtures/move_500.eonreplay"
            ]),
            Ok(Parsed::Run(Cli {
                headless_run: Some(HeadlessRun::Replay(PathBuf::from(
                    "crates/sim/tests/fixtures/move_500.eonreplay"
                ))),
                ..Cli::default()
            }))
        );
        assert_eq!(HeadlessRun::parse("7"), Ok(HeadlessRun::Ticks(7)));
        assert!(HeadlessRun::parse("seven").is_err());
        assert!(HeadlessRun::parse("match.replay").is_err());
        assert!(HeadlessRun::parse("-1").is_err());
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
        assert!(parse(&["--max-fps", "0"]).is_err());
        assert!(parse(&["--max-fps", "fast"]).is_err());
        assert!(parse(&["--scenario"]).is_err());
        assert!(parse(&["--scenario", "--seed"]).is_err());
        assert!(parse(&["--replay-dir"]).is_err());
        assert!(parse(&["--screenshot", ""]).is_err());
    }

    #[test]
    fn headless_excludes_windowed_only_flags() {
        assert!(parse(&["--headless-run", "10", "--scenario", "units200"]).is_err());
        assert!(parse(&["--headless-run", "10", "--screenshot", "a.png"]).is_err());
        // --max-fps, --replay-dir and --hash-every-tick are allowed headless
        // (ignored or used by the replay runner).
        assert!(
            parse(&[
                "--headless-run",
                "10",
                "--max-fps",
                "30",
                "--hash-every-tick"
            ])
            .is_ok()
        );
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
