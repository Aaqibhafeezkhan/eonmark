//! Command-line flags. Hand-rolled over `std::env::args` so the game crate
//! adds no argument-parsing dependency (clap lives in `sim-cli` only).
//!
//! M2 flags and who wires their behaviour (see the M2 contract):
//! `--headless-run <ticks | path.eonreplay>` (replay mode: agent A),
//! `--scenario <name>` (A), `--max-fps <n>` (A), `--replay-dir <dir>` (A),
//! `--hash-every-tick` (A), `--screenshot <path>` (B),
//! `--background` (`crate::background`; also `EONMARK_BACKGROUND=1`).
//! M5b adds the main menu's flags: `--skip-menu`, `--spectate`,
//! `--fast-forward-to <tick>` (with `--spectate` only) and the New Game
//! click proxy `--new-game-after-seconds <s>` (`crate::menu`), and
//! [`Cli::seed_explicit`], so a plain launch without `--seed` plays a fresh
//! wall-clock seed (`app::match_seed`).
//! The art pass adds `--assets-dir <dir>` ([`Cli::resolve_assets_dir`], also
//! `EONMARK_ASSETS`), `--window-size <W>x<H>`, `--grid` (the debug grid,
//! off by default; G toggles it), `--draw-stats` (the draw-call counter,
//! `crate::draw_stats`), `--click-check` (every unit selectable by a
//! real click, `crate::click_check`) and `--minimap-check` (the minimap's
//! input with real clicks, `crate::minimap_check`).
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
  --screenshot <PATH>          Save a PNG of the window about one second before
                               --exit-after-seconds fires.
  --screenshot-at <TICK>:<PATH>
                               Save a PNG of the window once the sim has reached TICK
                               (repeatable; scripted scenarios' beats, M3b proxies).
  --quit-via-menu-after-seconds <S>
                               Dev builds, macOS: after S seconds feed the `Quit Eonmark`
                               menu id through the same path a real Cmd-Q takes
                               (AppExit::Success, replay trailer). Ignored elsewhere.
  --close-window-after-seconds <S>
                               After S seconds request the primary window to close, the
                               path the red close button takes (bevy_window despawns
                               it, exit_on_all_closed writes AppExit::Success).
  --exit-after-seconds <S>     Send AppExit::Success after S seconds (smoke tests).
  --camera <X>,<Y>,<ZOOM>      Start the camera over sim tile (X, Y) at ZOOM metres
                               (15 to 60) instead of jumping to the first units
                               (screenshots, stream framing).
  --background                 Automated runs: open the window unfocused, below other
                               windows, in the top-left corner of the primary monitor,
                               and (macOS) hand focus back to the app that had it. Also
                               on when EONMARK_BACKGROUND=1. Ignored by --headless-run.
  --seed <U64>                 Match seed. Without it a plain launch, --skip-menu and
                               --spectate use a fresh seed from the wall clock (shown on
                               the menu and in the replay's name), a scenario keeps its
                               own, and --headless-run uses 1.
  --skip-menu                  Start the match against the Standard bot at once instead
                               of opening the main menu.
  --spectate                   Watch the Standard bot play itself (both slots AI; with
                               --seed 1 it is the m5_first_match setup, otherwise a
                               fresh seed). You can select units and move the camera
                               but give no orders. No menu.
  --fast-forward-to <TICK>     With --spectate: simulate up to TICK before the window
                               opens (refused if the match is decided by then); the
                               replay is recorded from there and still verifies.
  --new-game-after-seconds <S>
                               The New Game proxy: after S seconds click the menu's real
                               New Game button with a synthetic mouse (picking path).
  --data-dir <PATH>            Override the data directory (default: see below).
  --assets-dir <PATH>          Override the assets directory (default: see below).
  --window-size <W>x<H>        Logical window size (default 1280x800); the frame_stats
                               `window` line prints the physical size it got.
  --grid                       Draw the 8-tile debug grid (off by default; G toggles it).
  --draw-stats                 Count the draw calls the render passes issue each frame
                               and print `frame_stats: draws ...` and `frame_stats:
                               triangles ...` on exit (always on in dev builds, which
                               also show it in the Sim panel).
  --click-check                With --scenario unit_looks: at tick 60 print a
                               `click_check: target ...` pixel for every unit, judge
                               each primary click against the next target, then print
                               `click_check: ok <n>/<n>` (exit 0) or `click_check: FAIL
                               ...` (exit 1). A driver clicks with real X input.
  --minimap-check              With --scenario economy_walkthrough: from tick 40 print
                               the minimap's rect and four steps (`minimap_check: step
                               <n> ...`: press, drag, right click, A then left click),
                               judge each, then print `minimap_check: ok` (exit 0) or
                               `minimap_check: FAIL ...` (exit 1). A driver performs the
                               steps with real X input.
  -h, --help                   Show this text.

Data directory resolution order:
  1. --data-dir
  2. $EONMARK_DATA
  3. <workspace>/data when running from a checkout
  4. <directory of the executable>/data

Assets directory resolution order (models; the shaders are built in):
  1. --assets-dir
  2. $EONMARK_ASSETS
  3. <workspace>/assets when running from a checkout
  4. <directory of the executable>/assets
  5. assets (relative to the working directory)
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
    /// `true` when `--seed` was given (M5b): a scenario's own seed and a
    /// plain launch's fresh wall-clock seed are overridden only then.
    pub seed_explicit: bool,
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
    /// `--skip-menu` (M5b): start the vs-bot match without the main menu.
    pub skip_menu: bool,
    /// `--spectate` (M5b): the Standard bot in both slots, no local input.
    pub spectate: bool,
    /// `--fast-forward-to <tick>` (M5b, with `--spectate` only).
    pub fast_forward_to: Option<u32>,
    /// `--new-game-after-seconds <s>` (M5b): the menu's New Game click proxy.
    pub new_game_after_seconds: Option<f64>,
    /// `--assets-dir <dir>` (art pass): the asset root, see
    /// [`Cli::resolve_assets_dir`].
    pub assets_dir: Option<PathBuf>,
    /// `--window-size <w>x<h>` (art pass): the logical window size;
    /// `None` is `app::WINDOW_SIZE`.
    pub window_size: Option<(u32, u32)>,
    /// `--grid` (art pass): start with the debug grid shown.
    pub grid: bool,
    /// `--draw-stats` (art pass): count draw calls and print the exit lines.
    pub draw_stats: bool,
    /// `--click-check` (art pass): print every unit's click target and
    /// judge real clicks against them (`crate::click_check`).
    pub click_check: bool,
    /// `--minimap-check` (art pass): print the minimap's steps and judge
    /// real input against them (`crate::minimap_check`).
    pub minimap_check: bool,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            headless_run: None,
            exit_after_seconds: None,
            seed: 1,
            seed_explicit: false,
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
            skip_menu: false,
            spectate: false,
            fast_forward_to: None,
            new_game_after_seconds: None,
            assets_dir: None,
            window_size: None,
            grid: false,
            draw_stats: false,
            click_check: false,
            minimap_check: false,
        }
    }
}

/// Smallest `--window-size` side, logical pixels.
pub const MIN_WINDOW_SIDE: u32 = 320;
/// Largest `--window-size` side, logical pixels (8K).
pub const MAX_WINDOW_SIDE: u32 = 7680;

/// Parse `--window-size`'s `<w>x<h>`: two whole numbers of logical pixels,
/// each in [`MIN_WINDOW_SIDE`]`..=`[`MAX_WINDOW_SIDE`].
pub fn parse_window_size(value: &str) -> Result<(u32, u32), String> {
    let (w, h) = value
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("expected <w>x<h>, got `{value}`"))?;
    let side = |s: &str| -> Result<u32, String> {
        let n: u32 = s
            .trim()
            .parse()
            .map_err(|e| format!("`{s}` in `{value}`: {e}"))?;
        if (MIN_WINDOW_SIDE..=MAX_WINDOW_SIDE).contains(&n) {
            Ok(n)
        } else {
            Err(format!(
                "{n} in `{value}` is outside {MIN_WINDOW_SIDE}..={MAX_WINDOW_SIDE}"
            ))
        }
    };
    Ok((side(w)?, side(h)?))
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
                "--seed" => {
                    cli.seed = parse_value(&arg, args.next())?;
                    cli.seed_explicit = true;
                }
                "--skip-menu" => cli.skip_menu = true,
                "--spectate" => cli.spectate = true,
                "--fast-forward-to" => cli.fast_forward_to = Some(parse_value(&arg, args.next())?),
                "--new-game-after-seconds" => {
                    let secs: f64 = parse_value(&arg, args.next())?;
                    if !secs.is_finite() || secs < 0.0 {
                        return Err(format!("{arg}: expected a non-negative number"));
                    }
                    cli.new_game_after_seconds = Some(secs);
                }
                "--data-dir" => cli.data_dir = Some(parse_path(&arg, args.next())?),
                "--assets-dir" => cli.assets_dir = Some(parse_path(&arg, args.next())?),
                "--window-size" => {
                    let value = args.next().ok_or_else(|| format!("{arg}: missing value"))?;
                    cli.window_size =
                        Some(parse_window_size(&value).map_err(|e| format!("{arg}: {e}"))?);
                }
                "--grid" => cli.grid = true,
                "--draw-stats" => cli.draw_stats = true,
                "--click-check" => cli.click_check = true,
                "--minimap-check" => cli.minimap_check = true,
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
            for (given, flag) in [
                (cli.skip_menu, "--skip-menu"),
                (cli.spectate, "--spectate"),
                (cli.window_size.is_some(), "--window-size"),
                (cli.grid, "--grid"),
                (cli.draw_stats, "--draw-stats"),
                (cli.click_check, "--click-check"),
                (cli.minimap_check, "--minimap-check"),
                (cli.fast_forward_to.is_some(), "--fast-forward-to"),
                (
                    cli.new_game_after_seconds.is_some(),
                    "--new-game-after-seconds",
                ),
            ] {
                if given {
                    return Err(format!("{flag} needs a window; drop --headless-run"));
                }
            }
        }
        if cli.click_check && cli.headless_run.is_none() && cli.scenario.is_none() {
            return Err(
                "--click-check judges clicks on a scenario's units; add --scenario unit_looks"
                    .into(),
            );
        }
        if cli.minimap_check && cli.headless_run.is_none() && cli.scenario.is_none() {
            return Err(
                "--minimap-check drives a scenario's minimap; add --scenario economy_walkthrough"
                    .into(),
            );
        }
        if cli.spectate && cli.scenario.is_some() {
            return Err("--spectate builds its own match; drop --scenario".into());
        }
        if cli.fast_forward_to.is_some() && !cli.spectate {
            return Err(
                "--fast-forward-to needs --spectate (only a match without human input can skip ahead and still record a replay that verifies)"
                    .into(),
            );
        }
        if cli.new_game_after_seconds.is_some() && !cli.starts_in_menu() {
            return Err(
                "--new-game-after-seconds clicks the main menu's New Game; drop --skip-menu, --spectate and --scenario"
                    .into(),
            );
        }
        Ok(Parsed::Run(cli))
    }

    /// `true` when a windowed launch opens on the main menu (M5b owner
    /// decision 5): a plain launch. `--scenario`, `--skip-menu` and
    /// `--spectate` start the match at once, and `--headless-run` has no
    /// window at all.
    pub fn starts_in_menu(&self) -> bool {
        self.scenario.is_none() && !self.skip_menu && !self.spectate
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

    /// Locate the `assets/` directory the game loads models from (see
    /// [`USAGE`] for the order), the way [`Cli::resolve_data_dir`] finds
    /// `data/`: Bevy's own default under `cargo run` would be
    /// `crates/game/assets`, which does not exist. `app::run` hands the
    /// result to `AssetPlugin::file_path`.
    pub fn resolve_assets_dir(&self) -> PathBuf {
        resolve_assets_dir_with(
            self.assets_dir.as_deref(),
            std::env::var_os(ASSETS_ENV).as_deref(),
            Path::new(env!("CARGO_MANIFEST_DIR")),
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf))
                .as_deref(),
        )
    }
}

/// Environment variable naming the assets directory, after `--assets-dir`.
pub const ASSETS_ENV: &str = "EONMARK_ASSETS";

/// [`Cli::resolve_assets_dir`] with its inputs passed in: `flag`
/// (`--assets-dir`), then `env` ([`ASSETS_ENV`]), then
/// `<manifest>/../../assets` when that is a directory (a checkout), then
/// `<exe_dir>/assets` when that is one (`bundle.sh` copies `assets/` beside
/// the executable), then `assets`.
pub fn resolve_assets_dir_with(
    flag: Option<&Path>,
    env: Option<&std::ffi::OsStr>,
    manifest: &Path,
    exe_dir: Option<&Path>,
) -> PathBuf {
    if let Some(dir) = flag {
        return dir.to_path_buf();
    }
    if let Some(dir) = env.filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let checkout = manifest.join("../../assets");
    if checkout.is_dir() {
        return checkout;
    }
    if let Some(beside_exe) = exe_dir.map(|d| d.join("assets"))
        && beside_exe.is_dir()
    {
        return beside_exe;
    }
    PathBuf::from("assets")
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
                seed_explicit: true,
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
                skip_menu: false,
                spectate: false,
                fast_forward_to: None,
                new_game_after_seconds: None,
                assets_dir: None,
                window_size: None,
                grid: false,
                draw_stats: false,
                click_check: false,
                minimap_check: false,
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

    /// M5b: the menu flags parse, `--seed` is remembered as explicit,
    /// `--fast-forward-to` needs `--spectate`, the New Game proxy needs the
    /// menu, and none of them runs headless.
    #[test]
    fn m5b_flags_parse_and_fast_forward_needs_spectate() {
        let Ok(Parsed::Run(plain)) = parse(&[]) else {
            panic!("no flags parse");
        };
        assert!(!plain.seed_explicit && plain.starts_in_menu());
        let Ok(Parsed::Run(seeded)) = parse(&["--seed", "7"]) else {
            panic!("--seed parses");
        };
        assert_eq!((seeded.seed, seeded.seed_explicit), (7, true));
        // `--seed 1` is explicit too, although 1 is the default value.
        let Ok(Parsed::Run(one)) = parse(&["--seed", "1"]) else {
            panic!("--seed 1 parses");
        };
        assert!(one.seed_explicit);
        assert_eq!(
            parse(&["--skip-menu"]),
            Ok(Parsed::Run(Cli {
                skip_menu: true,
                ..Cli::default()
            }))
        );
        assert_eq!(
            parse(&["--spectate", "--seed", "1", "--fast-forward-to", "25612"]),
            Ok(Parsed::Run(Cli {
                spectate: true,
                seed: 1,
                seed_explicit: true,
                fast_forward_to: Some(25612),
                ..Cli::default()
            }))
        );
        assert_eq!(
            parse(&[
                "--new-game-after-seconds",
                "3",
                "--exit-after-seconds",
                "45"
            ]),
            Ok(Parsed::Run(Cli {
                new_game_after_seconds: Some(3.0),
                exit_after_seconds: Some(45.0),
                ..Cli::default()
            }))
        );
        for skipping in [
            vec!["--skip-menu"],
            vec!["--spectate"],
            vec!["--scenario", "units200"],
        ] {
            let Ok(Parsed::Run(cli)) = parse(&skipping) else {
                panic!("{skipping:?} parses");
            };
            assert!(!cli.starts_in_menu(), "{skipping:?} skips the menu");
            let mut with_proxy = skipping.clone();
            with_proxy.extend(["--new-game-after-seconds", "1"]);
            assert!(
                parse(&with_proxy).is_err(),
                "{with_proxy:?}: no menu to click"
            );
        }
        // Fast-forward only skips a match nobody plays.
        assert!(parse(&["--fast-forward-to", "100"]).is_err());
        assert!(parse(&["--skip-menu", "--fast-forward-to", "100"]).is_err());
        assert!(parse(&["--spectate", "--fast-forward-to"]).is_err());
        assert!(parse(&["--spectate", "--fast-forward-to", "-5"]).is_err());
        assert!(parse(&["--spectate", "--scenario", "units200"]).is_err());
        assert!(parse(&["--new-game-after-seconds", "-1"]).is_err());
        assert!(parse(&["--new-game-after-seconds", "soon"]).is_err());
        for flag in [
            vec!["--skip-menu"],
            vec!["--spectate"],
            vec!["--spectate", "--fast-forward-to", "10"],
            vec!["--new-game-after-seconds", "1"],
        ] {
            let mut args = vec!["--headless-run", "10"];
            args.extend(flag.iter().copied());
            assert!(parse(&args).is_err(), "{args:?} needs a window");
        }
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

    /// The art pass's flags: `--assets-dir`, `--window-size`, `--grid`,
    /// `--draw-stats` and `--click-check` parse, the size is checked, the
    /// four that need a window refuse `--headless-run`, and `--click-check`
    /// needs a scenario.
    #[test]
    fn art_flags_parse() {
        assert_eq!(
            parse(&[
                "--assets-dir",
                "/tmp/assets",
                "--window-size",
                "1280x720",
                "--grid",
                "--draw-stats",
            ]),
            Ok(Parsed::Run(Cli {
                assets_dir: Some(PathBuf::from("/tmp/assets")),
                window_size: Some((1280, 720)),
                grid: true,
                draw_stats: true,
                ..Cli::default()
            }))
        );
        assert_eq!(parse_window_size("2560X1440"), Ok((2560, 1440)));
        assert_eq!(parse_window_size("320x320"), Ok((320, 320)));
        assert_eq!(parse_window_size("7680x4320"), Ok((7680, 4320)));
        for bad in [
            "1280",
            "1280x",
            "x720",
            "1280x719.5",
            "319x720",
            "1280x7681",
            "-1x720",
        ] {
            assert!(parse_window_size(bad).is_err(), "{bad}");
            assert!(parse(&["--window-size", bad]).is_err(), "{bad}");
        }
        assert!(parse(&["--window-size"]).is_err());
        assert!(parse(&["--assets-dir"]).is_err());
        assert!(parse(&["--assets-dir", ""]).is_err());
        assert_eq!(
            parse(&["--scenario", "unit_looks", "--click-check"]),
            Ok(Parsed::Run(Cli {
                scenario: Some("unit_looks".into()),
                click_check: true,
                ..Cli::default()
            }))
        );
        let err = parse(&["--click-check"]).unwrap_err();
        assert!(err.contains("--scenario unit_looks"), "{err}");
        assert_eq!(
            parse(&["--scenario", "economy_walkthrough", "--minimap-check"]),
            Ok(Parsed::Run(Cli {
                scenario: Some("economy_walkthrough".into()),
                minimap_check: true,
                ..Cli::default()
            }))
        );
        let err = parse(&["--minimap-check"]).unwrap_err();
        assert!(err.contains("--scenario economy_walkthrough"), "{err}");
        for flag in [
            vec!["--window-size", "1280x720"],
            vec!["--grid"],
            vec!["--draw-stats"],
            vec!["--click-check"],
            vec!["--minimap-check"],
        ] {
            let mut args = vec!["--headless-run", "10"];
            args.extend(flag.iter().copied());
            assert!(parse(&args).is_err(), "{args:?} needs a window");
        }
        // The asset root is only a path; a headless run may name it.
        assert!(parse(&["--headless-run", "10", "--assets-dir", "/x"]).is_ok());
    }

    /// `--assets-dir`, then `EONMARK_ASSETS`, then the checkout's
    /// `assets/`, then `assets/` beside the executable, then `assets`.
    #[test]
    fn resolve_assets_dir_order() {
        let root = std::env::temp_dir().join(format!("eonmark-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let manifest = root.join("checkout/crates/game");
        let exe_dir = root.join("bundle");
        std::fs::create_dir_all(&manifest).unwrap();
        std::fs::create_dir_all(&exe_dir).unwrap();
        let env = std::ffi::OsStr::new("/from/env");
        let flag = Path::new("/from/flag");
        let resolve = |flag, env| resolve_assets_dir_with(flag, env, &manifest, Some(&exe_dir));
        // Nothing exists yet: the relative fallback.
        assert_eq!(resolve(None, None), PathBuf::from("assets"));
        assert_eq!(
            resolve(None, Some(std::ffi::OsStr::new(""))),
            PathBuf::from("assets"),
            "an empty variable is unset"
        );
        // Beside the executable.
        std::fs::create_dir_all(exe_dir.join("assets")).unwrap();
        assert_eq!(resolve(None, None), exe_dir.join("assets"));
        // The checkout's wins over the bundle's.
        std::fs::create_dir_all(root.join("checkout/assets")).unwrap();
        assert_eq!(resolve(None, None), manifest.join("../../assets"));
        // The variable wins over both, and the flag over everything.
        assert_eq!(resolve(None, Some(env)), PathBuf::from("/from/env"));
        assert_eq!(resolve(Some(flag), Some(env)), PathBuf::from("/from/flag"));
        let _ = std::fs::remove_dir_all(&root);
        // In this checkout the default is the repository's assets/.
        let here = Cli::default();
        if std::env::var_os(ASSETS_ENV).is_none() {
            let dir = here.resolve_assets_dir();
            assert!(dir.join("ATTRIBUTION.md").is_file(), "{}", dir.display());
        }
        let flagged = Cli {
            assets_dir: Some(PathBuf::from("/x/assets")),
            ..Cli::default()
        };
        assert_eq!(flagged.resolve_assets_dir(), PathBuf::from("/x/assets"));
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
