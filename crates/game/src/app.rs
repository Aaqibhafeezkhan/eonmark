//! The windowed Bevy app: window, plugins, the sim driver with its input
//! source and recorder, and the `--exit-after-seconds` / `--max-fps` /
//! `--screenshot` smoke-test helpers.
//!
//! Which input source runs (M2 decision 2): `--scenario <name>` builds the
//! sim from `MatchSetup::scenario` (debug commands on) and feeds a
//! `ScenarioInput` (scripted stream + the player's commands); otherwise a
//! match with `LocalInput`: since M5b against the Standard bot
//! (`MatchSetup::vs_bot`), or, under `--spectate`, the Standard bot
//! against itself (`MatchSetup::bots`, `m5_first_match`'s setup), on a
//! fresh wall-clock seed unless `--seed` is given ([`match_seed`]). Every
//! windowed session records a replay (decision 3) to `--replay-dir` or the
//! platform data directory, from its first stepped tick (M5b).
//!
//! M5b states (`crate::menu`): a plain launch opens on the main menu
//! (`AppState::Menu`); `--scenario`, `--skip-menu` and `--spectate` start in
//! `AppState::Skirmish`. `--spectate --fast-forward-to <tick>` simulates up
//! to that tick in [`build_match`], before the window opens.
//!
//! Art pass: `AssetPlugin` reads models from [`Cli::resolve_assets_dir`]
//! (the repository's `assets/` under `cargo run`, not Bevy's default
//! `crates/game/assets`), `crate::models` loads them before the sim's
//! first tick and `crate::terrain_props` dresses forests, mountains and
//! water once they settle, `--window-size` sets the window,
//! `--grid` starts with the debug grid on, and `--draw-stats` (or a `dev`
//! build) adds the draw-call counter (`crate::draw_stats`) with the
//! frame-time lines. `EONMARK_WGPU_DISABLE` turns wgpu features off for the
//! counter's trace cross-check.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bevy::app::AppExit;
use bevy::asset::AssetMetaCheck;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::render::settings::WgpuSettings;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::{PresentMode, PrimaryWindow, WindowCloseRequested, WindowResolution};
use rules::Rules;
use sim::{MatchSetup, scenarios};

use crate::cli::{CameraStart, Cli};
use crate::frame_stats::FrameStatsPlugin;
use crate::hud::HudPlugin;
use crate::menu::{MenuPlugin, initial_state};
use crate::monitor_loss::MonitorLossPlugin;
use crate::orders::OrdersPlugin;
use crate::present::PresentPlugin;
use crate::selection::SelectionPlugin;
use crate::sim_driver::{
    InputSource, LocalInput, ScenarioInput, SimHandle, SimPlugin, new_replay_path, replay_dir,
};
use crate::{background, camera, ground, palette};

/// Default logical window size (`--window-size` overrides it).
pub const WINDOW_SIZE: (u32, u32) = (1280, 800);

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

/// The personality of the Standard bot a plain launch and `--spectate`
/// play (`data/ai/personalities.ron`).
pub const STANDARD_PERSONALITY: &str = "standard";

/// A fresh match seed from the wall clock (M5b owner decision 6): the low
/// 32 bits of the nanoseconds since the Unix epoch ([`seed_from_unix_nanos`]).
/// The game crate may read the clock; the sim only ever sees the seed,
/// which the replay header records.
pub fn fresh_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    seed_from_unix_nanos(nanos)
}

/// The seed [`fresh_seed`] takes from `nanos` since the Unix epoch: the low
/// 32 bits, so it stays short on the menu and in the replay's name.
pub fn seed_from_unix_nanos(nanos: u128) -> u64 {
    u64::try_from(nanos & u128::from(u32::MAX)).unwrap_or(0)
}

/// The seed of a plain launch or `--spectate`: `--seed` when given,
/// otherwise [`fresh_seed`].
pub fn match_seed(cli: &Cli) -> u64 {
    match_seed_with(cli, fresh_seed)
}

/// [`match_seed`] with the clock reading passed in: `--seed` when given,
/// otherwise `fresh()` (tests pass a fixed reading).
pub fn match_seed_with(cli: &Cli, fresh: impl FnOnce() -> u64) -> u64 {
    if cli.seed_explicit { cli.seed } else { fresh() }
}

/// The setup `--spectate` watches: the Standard bot in both slots
/// (`MatchSetup::bots`), the setup of the `m5_first_match` fixture when the
/// seed is 1.
pub fn spectate_setup(rules: &Rules, seed: u64) -> MatchSetup {
    MatchSetup::bots(
        rules,
        seed,
        (ai::BOT_STANDARD, STANDARD_PERSONALITY),
        (ai::BOT_STANDARD, STANDARD_PERSONALITY),
    )
}

/// The setup of a plain launch or `--skip-menu` (M6): the local player
/// against `--difficulty` (default `standard`) with `--personality`
/// (default `standard`; `random` resolved for the bot's seat at `seed`,
/// `ai::resolve_personality`), or `passive` with no personality.
///
/// # Errors
///
/// A personality for `passive`, or `random` with no personality in the
/// pool. Unknown ids are reported when the bots are built.
pub fn vs_bot_setup(cli: &Cli, rules: &Rules, seed: u64) -> Result<MatchSetup, String> {
    let bot = cli.difficulty.as_deref().unwrap_or(ai::BOT_STANDARD);
    let personality = match (bot, cli.personality.as_deref()) {
        (ai::BOT_PASSIVE, None) => String::new(),
        (ai::BOT_PASSIVE, Some(p)) => {
            return Err(format!("--personality {p}: the passive bot takes none"));
        }
        (_, None) => STANDARD_PERSONALITY.to_string(),
        (_, Some(ai::RANDOM_PERSONALITY)) => {
            ai::resolve_personality(rules, seed, sim::PlayerId(1))?
        }
        (_, Some(p)) => p.to_string(),
    };
    Ok(MatchSetup::vs_bot(rules, seed, bot, &personality))
}

/// The sim and the input source a windowed session starts with: a match
/// against the Standard bot (a plain launch, `--skip-menu`), the Standard
/// bot against itself (`--spectate`, fast-forwarded to
/// `--fast-forward-to` here, printing `fast_forward: tick=<n> wall_s=<s>`),
/// or a scenario with its scripted stream.
///
/// # Errors
///
/// An unknown scenario, or a fast-forward the outcome came before.
pub fn build_match(cli: &Cli, rules: Rules) -> Result<(SimHandle, Box<dyn InputSource>), String> {
    if cli.spectate {
        let setup = spectate_setup(&rules, match_seed(cli));
        let mut handle = SimHandle::try_from_setup(setup, rules)?;
        if let Some(target) = cli.fast_forward_to {
            let wall = handle.fast_forward(target)?;
            println!(
                "fast_forward: tick={} wall_s={:.2}",
                handle.tick(),
                wall.as_secs_f64()
            );
        }
        return Ok((handle, Box::new(LocalInput)));
    }
    match &cli.scenario {
        // M5b: a plain launch plays the Standard bot (`MatchSetup::vs_bot`);
        // M6: `--difficulty` and `--personality` choose it.
        None => {
            let seed = match_seed(cli);
            let setup = vs_bot_setup(cli, &rules, seed)?;
            Ok((
                SimHandle::try_from_setup(setup, rules)?,
                Box::new(LocalInput),
            ))
        }
        Some(name) => {
            let Some((mut setup, stream, _ticks)) = scenarios::by_name(name, &rules) else {
                return Err(format!(
                    "unknown scenario {name:?}; one of {}",
                    scenarios::NAMES.join(", ")
                ));
            };
            // `--seed` overrides the scenario's own seed only when given.
            if cli.seed_explicit {
                setup.seed = cli.seed;
            }
            Ok((
                SimHandle::from_setup(setup, rules),
                Box::new(ScenarioInput::new(stream)),
            ))
        }
    }
}

/// Where the camera starts for a scenario run without `--camera`: the
/// `unit_looks` lineup is framed at the 40 m acceptance zoom of
/// `docs/ART_STYLE.md` ("Readability at 40 m"), centred between its two
/// rows; `enemy_outpost` and `enemy_outpost_walkthrough` open on their
/// 200 vs 200 prologue east of the river at 55 m, which puts all 400 units
/// on screen between the HUD bars (ROADMAP M4b acceptance line 2 measures
/// that fight; the player pans home, Period jumps to an idle Yeoman);
/// `capture_seat` (M5a) opens at 40 m over the ring of P1's Seat, where
/// the Mangonels, the Skirmishers' posts and the annexation timer are;
/// `art_bench` (art pass) opens at 60 m over P0's Town and the fight east
/// of it ([`ART_BENCH_FRAMING`]). Every other scenario, and a skirmish,
/// starts over the local player's first units
/// (`present::focus_camera_on_first_units`).
pub fn default_framing(scenario: Option<&str>) -> Option<CameraStart> {
    match scenario? {
        "unit_looks" => Some(CameraStart {
            x: 20.0,
            y: 32.0,
            zoom: 40.0,
        }),
        "enemy_outpost" | "enemy_outpost_walkthrough" => Some(CameraStart {
            x: 106.0,
            y: 32.0,
            zoom: 55.0,
        }),
        "capture_seat" => Some(CameraStart {
            x: 100.0,
            y: 64.0,
            zoom: 40.0,
        }),
        "art_bench" => Some(ART_BENCH_FRAMING),
        _ => None,
    }
}

/// `art_bench`'s framing: P0's Town (24, 64) and the armies east of it
/// (x 34-59) at the camera's longest zoom.
pub const ART_BENCH_FRAMING: CameraStart = CameraStart {
    x: 40.0,
    y: 64.0,
    zoom: 60.0,
};

/// `true` for the scenarios that measure a 200 vs 200 fight and print the
/// frame-time lines with the fight window on exit: the `enemy_outpost` pair
/// (M4b, whose prologue is that fight) and `art_bench` (art pass).
pub fn prints_fight_lines(scenario: Option<&str>) -> bool {
    scenario.is_some_and(|s| s.starts_with("enemy_outpost") || s == "art_bench")
}

/// The wgpu settings of the session: the defaults, with the features named
/// by `EONMARK_WGPU_DISABLE` ([`crate::draw_stats::WGPU_DISABLE_ENV`])
/// turned off when it is set. `None` keeps Bevy's own `RenderPlugin`.
///
/// # Errors
///
/// An unknown feature name.
pub fn wgpu_settings(disable: Option<&str>) -> Result<Option<WgpuSettings>, String> {
    let Some(value) = disable.filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };
    let features = crate::draw_stats::parse_disabled_features(value)?;
    Ok(Some(WgpuSettings {
        disabled_features: Some(features),
        ..default()
    }))
}

/// `--exit-after-seconds`: wall-clock deadline for smoke tests.
#[derive(Resource)]
struct ExitAfter(Duration);

/// `--close-window-after-seconds`: the red-close-button proxy. After the
/// delay, write `WindowCloseRequested` for the primary window exactly as
/// winit does for a click on the button; `bevy_window::close_when_requested`
/// then despawns it and `exit_on_all_closed` writes `AppExit::Success`,
/// both in `Last`.
#[derive(Resource)]
struct CloseWindowAfter {
    after: Duration,
    sent: bool,
}

/// `--max-fps <n>`: minimum frame duration enforced by a sleep in `Last`
/// (`limit_frame_rate`). The window runs without vsync under the cap so the
/// display's refresh rate cannot quantise the frame period (a 33 ms sleep
/// plus a 60 Hz vsync would settle at 50 ms, not 33); the sim is unaffected
/// either way because `FixedUpdate` catches up.
#[derive(Resource, Debug, Clone, Copy)]
pub struct FrameLimiter {
    /// Target frames per second.
    pub max_fps: u32,
}

impl FrameLimiter {
    /// The frame period the limiter enforces.
    pub fn period(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.max_fps.max(1)))
    }
}

/// `limit_frame_rate` sleeps until this much before the deadline and spins
/// the rest: `thread::sleep` on macOS overshoots by up to about a
/// millisecond, which at 30 fps would cost 3 %.
const SPIN_MARGIN: Duration = Duration::from_micros(1200);

/// When the deadline for the next frame should be, given the last deadline
/// and the current time: the previous deadline plus one period (so sleep
/// jitter does not accumulate), unless the frame overran by more than a
/// period, in which case the clock resynchronises from `now`.
pub fn next_deadline(previous: Option<Instant>, now: Instant, period: Duration) -> Instant {
    match previous {
        Some(prev) if now <= prev + period => prev + period,
        _ => now + period,
    }
}

/// `--screenshot <path>`: capture the primary window about one second
/// before the exit deadline (or at the deadline when it is under a second),
/// so the asynchronous capture lands before the app exits.
#[derive(Resource, Debug, Clone)]
pub struct ScreenshotRequest {
    /// Output PNG path.
    pub path: std::path::PathBuf,
    /// When to capture, in wall-clock seconds since start.
    pub at_secs: f64,
    /// Set once the capture was requested.
    pub taken: bool,
}

/// Run the windowed game. Returns the exit status for `main`.
pub fn run(cli: &Cli) -> AppExit {
    let Some(rules) = load_rules(cli) else {
        return AppExit::error();
    };
    let units200_auto = cli.scenario.as_deref() == Some("units200_auto");
    // The `units200_auto` arrival proxy needs the scripted move's goal and
    // the spawn count before `rules` moves into the match.
    let arrival = units200_auto.then(|| {
        let (_, stream) = scenarios::units200_auto(&rules);
        (scenarios::EAST, spawn_total(&stream))
    });
    // The walkthrough's director beats come from the rules too (M4b);
    // `art_bench` selects every P0 unit once (art pass).
    let director_beats = match cli.scenario.as_deref() {
        Some("enemy_outpost_walkthrough") => Some(crate::director::walkthrough_beats(&rules)),
        Some("art_bench") => Some(crate::director::art_bench_beats()),
        _ => None,
    };
    let wgpu = match wgpu_settings(
        std::env::var(crate::draw_stats::WGPU_DISABLE_ENV)
            .ok()
            .as_deref(),
    ) {
        Ok(settings) => settings,
        Err(message) => {
            eprintln!("eonmark: {message}");
            return AppExit::from_code(2);
        }
    };
    let strings = match rules::strings::Strings::load(cli.resolve_data_dir(), "en") {
        Ok(s) => s,
        Err(err) => {
            eprintln!("eonmark: cannot load UI strings: {err}");
            return AppExit::error();
        }
    };
    let (handle, input) = match build_match(cli, rules) {
        Ok(pair) => pair,
        Err(message) => {
            eprintln!("eonmark: {message}");
            return AppExit::from_code(2);
        }
    };
    let replay_path = new_replay_path(&replay_dir(cli.replay_dir.as_deref()), handle.seed());
    let hud_click_check = cli.scenario.as_deref() == Some("hud_click");
    let present_mode = if cli.max_fps.is_some() {
        PresentMode::AutoNoVsync
    } else {
        PresentMode::AutoVsync
    };

    let background = cli.background_mode();
    let (width, height) = cli.window_size.unwrap_or(WINDOW_SIZE);
    let mut window = Window {
        // The title stays "Eonmark" (ROADMAP M0 acceptance); the seed,
        // tick and hash readout lives in the `dev` Sim panel.
        title: "Eonmark".into(),
        // Logical size: winit reads this as a LogicalSize at creation.
        resolution: WindowResolution::new(width, height),
        present_mode,
        ..default()
    };
    if background {
        background::configure_window(&mut window);
    }
    // Models load from the repository's (or the bundle's) assets/; shaders
    // are embedded and need no asset root. No `.meta` files are vendored.
    let assets = cli.resolve_assets_dir();
    let assets = std::path::absolute(&assets).unwrap_or(assets);

    let mut app = App::new();
    let plugins = DefaultPlugins
        .set(WindowPlugin {
            primary_window: Some(window),
            ..default()
        })
        .set(AssetPlugin {
            file_path: assets.to_string_lossy().into_owned(),
            meta_check: AssetMetaCheck::Never,
            ..default()
        });
    match wgpu {
        Some(settings) => app.add_plugins(plugins.set(RenderPlugin {
            render_creation: settings.into(),
            ..default()
        })),
        None => app.add_plugins(plugins),
    };
    // Logged only now: `LogPlugin` (in `DefaultPlugins`) installs the
    // tracing subscriber, and an event before it is dropped.
    // `scripts/render_smoke.sh` requires this line.
    info!("assets: {}", assets.display());
    // Art pass: Inter replaces Bevy's built-in FiraMono before the first
    // text layout (`crate::fonts`).
    app.add_plugins(crate::fonts::FontsPlugin {
        assets: assets.clone(),
    });
    // Display sleep or a screen lock must not end the session.
    app.add_plugins(MonitorLossPlugin);
    if background {
        app.add_plugins(background::BackgroundPlugin);
    }
    app.insert_resource(ClearColor(palette::SKY));
    app.insert_resource(crate::economy_hud::UiStrings(strings));
    // M5b: the main menu on a plain launch, the match at once otherwise.
    app.insert_state(initial_state(cli));
    app.add_plugins(MenuPlugin {
        new_game_after: cli.new_game_after_seconds.map(Duration::from_secs_f64),
    });
    if let Some(start) = cli
        .camera
        .or_else(|| default_framing(cli.scenario.as_deref()))
    {
        app.insert_resource(camera::StartFraming(start));
    }
    // Art pass: the debug grid is off unless asked for (G toggles it).
    app.insert_resource(ground::ShowGrid(cli.grid));
    app.add_plugins(
        SimPlugin::new(handle, input)
            .recording(Some(replay_path))
            .hash_every(cli.hash_every()),
    );
    app.add_plugins((
        camera::CameraPlugin,
        ground::GroundPlugin,
        crate::field::FieldPlugin,
        crate::render_watchdog::RenderWatchdogPlugin,
        PresentPlugin,
        crate::buildings::BuildingsPlugin,
        crate::health_bars::HealthBarsPlugin,
        crate::economy_hud::EconomyHudPlugin,
        crate::age_hud::AgeHudPlugin,
        crate::command_card::CommandCardPlugin,
        crate::placement::PlacementPlugin,
        SelectionPlugin,
        OrdersPlugin,
        HudPlugin { hud_click_check },
    ));
    // Art pass: the glTF models of visuals.ron (the sim waits for them,
    // `models::models_settled`) and the terrain props placed once they
    // settle. Only here, where DefaultPlugins provide the asset server.
    app.add_plugins((
        crate::models::ModelsPlugin,
        crate::terrain_props::TerrainPropsPlugin,
    ));
    // M5a: the attrition indicator, supply ring, annexation timer and
    // readout, and the outcome overlay over the frozen world. Art pass:
    // arrows, stones and melee lunges for every hit (presentation only).
    app.add_plugins((
        crate::markers::MarkersPlugin,
        crate::outcome::OutcomeOverlayPlugin,
        crate::combat_fx::CombatFxPlugin,
    ));

    // Art pass AP6 (owner decision 7): the minimap bottom-left over the
    // bottom bar; it prints `frame_stats: minimap ...` on exit.
    app.add_plugins(crate::minimap::MinimapPlugin);

    // Art pass AP10: every unit selectable by a real click (a driver
    // clicks the printed targets; `crate::click_check`).
    if cli.click_check {
        app.add_plugins(crate::click_check::ClickCheckPlugin);
    }
    // Art pass AP6: the minimap's input with real X input (a driver
    // performs the printed steps; `crate::minimap_check`).
    if cli.minimap_check {
        app.add_plugins(crate::minimap_check::MinimapCheckPlugin);
    }

    // Presentation only: frames and selects the walkthrough's beats, or
    // selects `art_bench`'s army.
    if let Some(beats) = director_beats {
        app.add_plugins(crate::director::DirectorPlugin { beats });
    }

    #[cfg(target_os = "macos")]
    app.add_plugins(crate::macos_menu::MacosMenuPlugin {
        quit_via_menu_after: cli.quit_via_menu_after_seconds.map(Duration::from_secs_f64),
    });
    #[cfg(not(target_os = "macos"))]
    if cli.quit_via_menu_after_seconds.is_some() {
        eprintln!("eonmark: --quit-via-menu-after-seconds is macOS-only; ignored");
    }

    #[cfg(feature = "dev")]
    app.add_plugins(crate::dev_tools::DevToolsPlugin);
    // M4b acceptance line 2: health bars and rings stay top-level entities;
    // prints `hierarchy_check: ...` on exit and exits 1 on a violation.
    #[cfg(feature = "dev")]
    app.add_plugins(crate::health_bars::BarHierarchyCheckPlugin);
    #[cfg(feature = "dev")]
    if units200_auto {
        app.add_plugins(crate::selection::SyntheticBoxSelectPlugin);
    }

    if let Some(max_fps) = cli.max_fps {
        app.insert_resource(FrameLimiter { max_fps })
            .add_systems(Last, limit_frame_rate);
    }

    // Frame-time lines on exit: the `units200_auto` owner proxy (with the
    // arrival count for its scripted move to EAST), the `enemy_outpost`
    // scenarios' 200 vs 200 prologue (M4b, with the fight window of
    // acceptance line 2) and `art_bench`'s fight (art pass), any
    // `--max-fps` run, whose achieved rate they document, and any
    // `--draw-stats` run.
    let fight = prints_fight_lines(cli.scenario.as_deref());
    let draw_stats = cli.draw_stats || cfg!(feature = "dev");
    if units200_auto || fight || cli.max_fps.is_some() || draw_stats {
        app.add_plugins(FrameStatsPlugin { arrival, fight });
    }
    // The draw-call counter (art pass): `--draw-stats` and every dev build.
    if draw_stats {
        app.add_plugins(crate::draw_stats::DrawStatsPlugin);
    }

    if let Some(secs) = cli.close_window_after_seconds {
        app.insert_resource(CloseWindowAfter {
            after: Duration::from_secs_f64(secs),
            sent: false,
        })
        .add_systems(Update, close_window_after);
    }

    if let Some(secs) = cli.exit_after_seconds {
        app.insert_resource(ExitAfter(Duration::from_secs_f64(secs)))
            .add_systems(Update, exit_after);
        if let Some(path) = &cli.screenshot {
            app.insert_resource(ScreenshotRequest {
                path: path.clone(),
                at_secs: (secs - 1.0).max(0.0),
                taken: false,
            })
            .add_systems(Update, take_screenshot);
        }
    } else if cli.screenshot.is_some() {
        eprintln!("eonmark: --screenshot needs --exit-after-seconds to know when to capture");
    }
    if !cli.screenshot_at.is_empty() {
        app.insert_resource(TickScreenshots(cli.screenshot_at.clone()))
            .add_systems(Update, take_tick_screenshots);
    }

    app.run()
}

fn exit_after(
    time: Res<Time<Real>>,
    deadline: Res<ExitAfter>,
    sim: NonSend<SimHandle>,
    mut exit: MessageWriter<AppExit>,
) {
    if time.elapsed() >= deadline.0 {
        info!(
            "exit-after-seconds reached: seed={} tick={} hash={:#018x}",
            sim.seed(),
            sim.tick(),
            sim.hash()
        );
        exit.write(AppExit::Success);
    }
}

/// Units a scenario stream spawns: the sum of its `DebugSpawn` counts.
pub fn spawn_total(stream: &scenarios::Stream) -> u32 {
    stream
        .iter()
        .flat_map(|(_, cmds)| cmds.iter())
        .map(|c| match &c.cmd {
            sim::Command::DebugSpawn { count, .. } => u32::from(*count),
            _ => 0,
        })
        .sum()
}

/// Request the primary window to close once `after` has elapsed, once.
fn close_window_after(
    time: Res<Time<Real>>,
    mut request: ResMut<CloseWindowAfter>,
    windows: Query<Entity, With<PrimaryWindow>>,
    mut close: MessageWriter<WindowCloseRequested>,
) {
    if request.sent || time.elapsed() < request.after {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    request.sent = true;
    println!(
        "close-window: requesting close after {:.2} s",
        time.elapsed().as_secs_f64()
    );
    close.write(WindowCloseRequested { window });
}

/// Sleep in `Last` so the frame takes at least `1 / max_fps` seconds: wait
/// until the deadline set by the previous frame (sleep, then spin the last
/// [`SPIN_MARGIN`]), then schedule the next deadline one period later.
fn limit_frame_rate(limiter: Res<FrameLimiter>, mut deadline: Local<Option<Instant>>) {
    let period = limiter.period();
    let now = Instant::now();
    if let Some(target) = *deadline
        && now < target
    {
        let remaining = target - now;
        if remaining > SPIN_MARGIN {
            std::thread::sleep(remaining - SPIN_MARGIN);
        }
        while Instant::now() < target {
            std::hint::spin_loop();
        }
    }
    *deadline = Some(next_deadline(*deadline, Instant::now(), period));
}

/// `--screenshot-at`: captures still to take, as `(tick, path)`.
#[derive(Resource, Debug, Default)]
pub(crate) struct TickScreenshots(Vec<(u32, std::path::PathBuf)>);

/// Capture the primary window for every `--screenshot-at` whose tick the
/// sim has reached (the PNG lands a frame or two later).
pub(crate) fn take_tick_screenshots(
    sim: NonSend<crate::sim_driver::SimHandle>,
    mut shots: ResMut<TickScreenshots>,
    mut commands: Commands,
) {
    if shots.0.is_empty() {
        return;
    }
    let tick = sim.tick();
    shots.0.retain(|(at, path)| {
        if tick < *at {
            return true;
        }
        info!(
            "screenshot: tick {tick} reached {at}, capturing to {}",
            path.display()
        );
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path.clone()));
        false
    });
}

/// Capture the primary window once `at_secs` has passed, exactly once
/// (`Screenshot::primary_window()` + `save_to_disk`; the PNG lands a frame
/// or two later).
fn take_screenshot(
    time: Res<Time<Real>>,
    mut request: ResMut<ScreenshotRequest>,
    mut commands: Commands,
) {
    if request.taken || time.elapsed().as_secs_f64() < request.at_secs {
        return;
    }
    request.taken = true;
    info!(
        "screenshot: capturing the primary window to {}",
        request.path.display()
    );
    // Overwrite semantics, and a stale file must not look like this run's
    // capture to anything waiting for the PNG (the `hud_click` check).
    if let Err(err) = std::fs::remove_file(&request.path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        warn!(
            "screenshot: cannot remove the old {}: {err}",
            request.path.display()
        );
    }
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(request.path.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units200_auto_spawns_200() {
        let rules =
            Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
                .unwrap();
        let (_, stream) = scenarios::units200_auto(&rules);
        assert_eq!(spawn_total(&stream), 200);
        assert_eq!(spawn_total(&Vec::new()), 0);
    }

    #[test]
    fn unit_looks_is_framed_on_its_lineup_at_40_m() {
        let rules =
            Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
                .unwrap();
        let framing = default_framing(Some("unit_looks")).expect("framed");
        assert_eq!(framing.zoom, 40.0, "the ART_STYLE acceptance distance");
        assert!(
            CameraStart::parse(&format!("{},{},{}", framing.x, framing.y, framing.zoom)).is_ok(),
            "a framing `--camera` would accept"
        );
        // Centred on the lineup: within a tile of the middle of its spawns.
        let (_, stream) = scenarios::unit_looks(&rules);
        let spawns: Vec<(f32, f32)> = stream
            .iter()
            .flat_map(|(_, cmds)| cmds.iter())
            .filter_map(|c| match &c.cmd {
                sim::Command::DebugSpawn { at, .. } => Some((
                    crate::present::fx_to_f32(at.x),
                    crate::present::fx_to_f32(at.y),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(spawns.len(), 2 * rules.units.len());
        let (min_x, max_x, min_y, max_y) = spawns.iter().fold(
            (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
            |(a, b, c, d), &(x, y)| (a.min(x), b.max(x), c.min(y), d.max(y)),
        );
        assert!(
            (framing.x - (min_x + max_x) / 2.0).abs() <= 1.0,
            "{min_x}..{max_x}"
        );
        assert!(
            (framing.y - (min_y + max_y) / 2.0).abs() <= 1.0,
            "{min_y}..{max_y}"
        );
        // Nothing else is framed by default but the enemy_outpost pair,
        // capture_seat (`capture_seat_is_framed_on_the_seats_ring`) and
        // art_bench (`art_bench_is_framed_on_the_town_and_the_fight`).
        for name in scenarios::NAMES.iter().filter(|n| {
            !matches!(
                **n,
                "unit_looks"
                    | "enemy_outpost"
                    | "enemy_outpost_walkthrough"
                    | "capture_seat"
                    | "art_bench"
            )
        }) {
            assert_eq!(default_framing(Some(name)), None, "{name}");
        }
        assert_eq!(default_framing(None), None);
    }

    /// The camera `camera.rs` spawns (40 degree vertical fov) at
    /// `framing`, looking at a 1280 x 800 window, and the part of that
    /// window between the HUD's bottom bar and the age strip under its top
    /// bar.
    fn framed_camera(framing: CameraStart) -> (Camera, GlobalTransform, Rect) {
        use crate::camera::RtsCamera;
        use crate::hud::{BOTTOM_BAR_HEIGHT_PX, TOP_BAR_HEIGHT_PX};
        let rts = RtsCamera {
            focus: Vec3::new(
                framing.x - crate::ground::HALF_EXTENT,
                0.0,
                framing.y - crate::ground::HALF_EXTENT,
            ),
            zoom: framing.zoom,
            zoom_target: framing.zoom,
        };
        let transform = GlobalTransform::from(
            Transform::from_translation(rts.eye()).looking_at(rts.focus, Vec3::Y),
        );
        let mut camera = Camera::default();
        camera.computed.target_info = Some(bevy::camera::RenderTargetInfo {
            physical_size: UVec2::new(WINDOW_SIZE.0, WINDOW_SIZE.1),
            scale_factor: 1.0,
        });
        let projection = Projection::Perspective(PerspectiveProjection {
            fov: 40.0_f32.to_radians(),
            near: 0.5,
            far: 400.0,
            aspect_ratio: WINDOW_SIZE.0 as f32 / WINDOW_SIZE.1 as f32,
            ..default()
        });
        camera.computed.clip_from_view = projection.get_clip_from_view();
        let visible = Rect::new(
            0.0,
            TOP_BAR_HEIGHT_PX + crate::age_hud::AGE_STRIP_HEIGHT_PX,
            WINDOW_SIZE.0 as f32,
            WINDOW_SIZE.1 as f32 - BOTTOM_BAR_HEIGHT_PX,
        );
        (camera, transform, visible)
    }

    /// Both `enemy_outpost` scenarios open on their prologue: at tick 3
    /// every one of the 400 battle units projects inside a 1280 x 800
    /// window between the HUD's bottom bar and the age strip under its top
    /// bar, through the RTS camera's own pose and lens at the default
    /// framing.
    #[test]
    fn enemy_outpost_is_framed_on_its_prologue_battle() {
        use crate::camera::{ZOOM_MAX, ZOOM_MIN};
        struct NoAi;
        impl sim::AiController for NoAi {
            fn think(
                &mut self,
                _: sim::PlayerId,
                _: &sim::SimView<'_>,
                _: &mut sim::AiMemory,
            ) -> Vec<sim::Command> {
                Vec::new()
            }
        }
        let rules =
            Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
                .unwrap();
        let framing = default_framing(Some("enemy_outpost")).expect("framed");
        assert_eq!(
            default_framing(Some("enemy_outpost_walkthrough")),
            Some(framing)
        );
        assert!((ZOOM_MIN..=ZOOM_MAX).contains(&framing.zoom));
        assert!(prints_fight_lines(Some("enemy_outpost_walkthrough")));
        assert!(!prints_fight_lines(Some("unit_looks")) && !prints_fight_lines(None));
        // Centred on the battle: within a tile of the middle of the groups.
        let centres: Vec<(i32, i32)> = scenarios::OUTPOST_BATTLE_P0_CENTRES
            .iter()
            .chain(&scenarios::OUTPOST_BATTLE_P1_CENTRES)
            .copied()
            .collect();
        let mid = |f: fn(&(i32, i32)) -> i32| {
            let (lo, hi) = (
                centres.iter().map(f).min().unwrap(),
                centres.iter().map(f).max().unwrap(),
            );
            (lo + hi) as f32 / 2.0
        };
        assert!((framing.x - mid(|c| c.0)).abs() <= 1.0, "{framing:?}");
        assert!((framing.y - mid(|c| c.1)).abs() <= 1.0, "{framing:?}");

        let (camera, transform, visible) = framed_camera(framing);
        for name in ["enemy_outpost", "enemy_outpost_walkthrough"] {
            let (setup, stream, _) = scenarios::by_name(name, &rules).unwrap();
            let mut sim = sim::Sim::new(setup, rules.clone(), Box::new(NoAi));
            for t in 0..3 {
                sim.step(scenarios::commands_at(&stream, t));
            }
            let view = sim.view();
            let battle: Vec<Vec3> = view
                .units()
                .filter(|u| u.id.0 > 10)
                .map(|u| crate::present::world_from_sim(u.pos, 0.0))
                .collect();
            assert_eq!(battle.len(), 400, "{name}");
            for p in battle {
                let at = camera.world_to_viewport(&transform, p).expect("in front");
                assert!(visible.contains(at), "{name}: {p} at {at} is off screen");
            }
        }
    }

    /// `capture_seat` opens at 40 m on its Seat from the west: at tick
    /// 300, when the Mangonels have stopped in range and the Skirmishers
    /// are told to walk to the ring, the Mangonels, every ring post and the
    /// whole Seat with its annexation timer's circle project inside the
    /// window between the HUD bars.
    #[test]
    fn capture_seat_is_framed_on_the_seats_ring() {
        struct NoAi;
        impl sim::AiController for NoAi {
            fn think(
                &mut self,
                _: sim::PlayerId,
                _: &sim::SimView<'_>,
                _: &mut sim::AiMemory,
            ) -> Vec<sim::Command> {
                Vec::new()
            }
        }
        let rules =
            Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
                .unwrap();
        let framing = default_framing(Some("capture_seat")).expect("framed");
        assert_eq!(framing.zoom, 40.0);
        let (camera, transform, visible) = framed_camera(framing);
        let (setup, stream, _) = scenarios::by_name("capture_seat", &rules).unwrap();
        let mut sim = sim::Sim::new(setup, rules.clone(), Box::new(NoAi));
        for t in 0..scenarios::CAPTURE_SEAT_MOVE_TICK {
            sim.step(scenarios::commands_at(&stream, t));
        }
        let view = sim.view();
        let on_screen = |p: Vec3, what: &str| {
            let at = camera.world_to_viewport(&transform, p).expect("in front");
            assert!(visible.contains(at), "{what}: {p} at {at} is off screen");
        };
        for id in 11..=14 {
            let u = view.unit(sim::UnitId(id)).expect("a Mangonel");
            on_screen(crate::present::world_from_sim(u.pos, 0.0), "Mangonel");
        }
        for tile in scenarios::CAPTURE_SEAT_RING {
            on_screen(
                crate::present::world_from_sim(view.map().center_of(tile), 0.0),
                "ring post",
            );
        }
        let seat = view.building(sim::BuildingId(2)).expect("P1's Seat");
        let (centre, radius) = crate::markers::annex_ring(&rules, seat);
        for (dx, dz) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            on_screen(centre + Vec3::new(dx, 0.0, dz) * radius, "timer circle");
        }
    }

    /// The fight lines: the `enemy_outpost` pair and `art_bench`, nothing
    /// else.
    #[test]
    fn prints_fight_lines_covers_art_bench() {
        for name in scenarios::NAMES {
            let want = matches!(
                name,
                "enemy_outpost" | "enemy_outpost_walkthrough" | "art_bench"
            );
            assert_eq!(prints_fight_lines(Some(name)), want, "{name}");
        }
        assert!(!prints_fight_lines(None));
    }

    /// `art_bench` opens at 60 m on P0's Town and the fight: at tick 40,
    /// when the director selects the army, P0's Seat and every armed unit
    /// of both armies project inside the window between the HUD bars.
    #[test]
    fn art_bench_is_framed_on_the_town_and_the_fight() {
        use crate::camera::{ZOOM_MAX, ZOOM_MIN};
        struct NoAi;
        impl sim::AiController for NoAi {
            fn think(
                &mut self,
                _: sim::PlayerId,
                _: &sim::SimView<'_>,
                _: &mut sim::AiMemory,
            ) -> Vec<sim::Command> {
                Vec::new()
            }
        }
        let rules =
            Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
                .unwrap();
        let framing = default_framing(Some("art_bench")).expect("framed");
        assert_eq!(framing, ART_BENCH_FRAMING);
        assert!((ZOOM_MIN..=ZOOM_MAX).contains(&framing.zoom));
        assert!(CameraStart::parse("40,64,60").is_ok_and(|c| c == framing));
        let (camera, transform, visible) = framed_camera(framing);
        let (setup, stream, _) = scenarios::by_name("art_bench", &rules).unwrap();
        let mut sim = sim::Sim::new(setup, rules.clone(), Box::new(NoAi));
        let is_armed = |kind: u16| rules.unit_kind(kind).is_some_and(|k| k.attack.is_some());
        // Every armed unit of both armies: the start units of armed kinds
        // (none while the Yeoman is unarmed) and each side's
        // `ART_BENCH_ARMY` rows of armed kinds, all read from the rules.
        let start_armed = sim.view().units().filter(|u| is_armed(u.kind.0)).count();
        let per_side: usize = scenarios::ART_BENCH_ARMY
            .iter()
            .filter(|(id, _)| {
                rules
                    .units
                    .iter()
                    .position(|k| k.id == *id)
                    .and_then(|i| u16::try_from(i).ok())
                    .is_some_and(is_armed)
            })
            .map(|&(_, n)| usize::from(n))
            .sum();
        assert!(per_side > 0, "art_bench fields armed units");
        for t in 0..crate::director::ART_BENCH_SELECT_TICK {
            sim.step(scenarios::commands_at(&stream, t));
        }
        let view = sim.view();
        let on_screen = |p: Vec3, what: &str| {
            let at = camera.world_to_viewport(&transform, p).expect("in front");
            assert!(visible.contains(at), "{what}: {p} at {at} is off screen");
        };
        let seat = view.building(sim::BuildingId(1)).expect("P0's Seat");
        on_screen(
            crate::present::world_from_sim(view.map().center_of(seat.tile), 0.0),
            "P0's Seat",
        );
        let mut armed = 0;
        for u in view.units() {
            if is_armed(u.kind.0) {
                armed += 1;
                on_screen(crate::present::world_from_sim(u.pos, 0.0), "armed unit");
            }
        }
        assert_eq!(
            armed,
            start_armed + 2 * per_side,
            "every armed unit of both armies"
        );
    }

    /// `EONMARK_WGPU_DISABLE`: unset or empty keeps Bevy's settings, a
    /// feature name disables it, a typo is an error.
    #[test]
    fn wgpu_disable_names_features() {
        assert!(wgpu_settings(None).unwrap().is_none());
        assert!(wgpu_settings(Some(" ")).unwrap().is_none());
        let settings = wgpu_settings(Some("indirect_first_instance"))
            .unwrap()
            .expect("settings");
        assert_eq!(
            settings.disabled_features,
            Some(bevy::render::settings::WgpuFeatures::INDIRECT_FIRST_INSTANCE)
        );
        assert!(wgpu_settings(Some("no_such_feature")).is_err());
    }

    fn launch(args: &[&str]) -> Cli {
        match Cli::parse(args.iter().map(|s| (*s).to_owned())) {
            Ok(crate::cli::Parsed::Run(cli)) => cli,
            other => panic!("{args:?}: {other:?}"),
        }
    }

    /// M5b acceptance 4's New Game: a plain launch opens on the main menu
    /// with a match already built for it, the local player (slot 0, human)
    /// against the Standard bot (slot 1, personality `standard`) on
    /// `plains_1v1` from the standard start, driven by local input, on a
    /// fresh seed unless `--seed` is given.
    #[test]
    fn a_plain_launch_builds_a_match_against_the_standard_bot_on_plains_1v1() {
        let rules =
            Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
                .unwrap();
        let cli = launch(&[]);
        assert_eq!(initial_state(&cli), crate::menu::AppState::Menu);
        let (handle, input) = build_match(&cli, rules.clone()).unwrap();
        let setup = handle.setup();
        assert_eq!(setup.map, "plains_1v1");
        assert_eq!(
            setup.players,
            vec![
                sim::PlayerSlot {
                    id: sim::PlayerId(0),
                    is_ai: false
                },
                sim::PlayerSlot {
                    id: sim::PlayerId(1),
                    is_ai: true
                },
            ]
        );
        assert_eq!(
            setup.ai_seats,
            vec![sim::AiSeat {
                player: sim::PlayerId(1),
                bot: "standard".into(),
                personality: "standard".into(),
            }]
        );
        assert_eq!(setup.start, sim::StartKind::Standard);
        assert!(!setup.debug_commands);
        assert_eq!(input.name(), "local");
        assert_eq!(
            *setup,
            MatchSetup::vs_bot(&rules, setup.seed, "standard", "standard"),
            "exactly the vs_bot setup"
        );
        // `--seed` overrides the fresh seed; `--skip-menu` builds the same
        // match without the menu.
        let (seeded, _) = build_match(&launch(&["--seed", "7"]), rules.clone()).unwrap();
        assert_eq!(seeded.seed(), 7);
        let skip = launch(&["--skip-menu", "--seed", "7"]);
        assert_eq!(initial_state(&skip), crate::menu::AppState::Skirmish);
        let (skipped, _) = build_match(&skip, rules.clone()).unwrap();
        assert_eq!(skipped.setup(), seeded.setup());
        // The fresh seed is the low 32 bits of the wall clock's nanoseconds.
        assert_eq!(seed_from_unix_nanos(0), 0);
        assert_eq!(seed_from_unix_nanos(0x1_2345_6789), 0x2345_6789);
        assert_eq!(
            seed_from_unix_nanos(1_792_000_000_123_456_789),
            1_792_000_000_123_456_789 & 0xffff_ffff
        );
        assert!(fresh_seed() <= u64::from(u32::MAX));
        // Owner decision 6: without `--seed` a plain launch and `--spectate`
        // take the clock's seed, and `--seed` (even `--seed 1`, the
        // default's value) overrides it.
        let clock = || 0xabcd_u64;
        assert_eq!(match_seed_with(&cli, clock), 0xabcd);
        assert_eq!(match_seed_with(&launch(&["--spectate"]), clock), 0xabcd);
        assert_eq!(match_seed_with(&launch(&["--skip-menu"]), clock), 0xabcd);
        assert_eq!(match_seed_with(&launch(&["--seed", "1"]), clock), 1);
        assert_eq!(match_seed_with(&launch(&["--seed", "7"]), clock), 7);
        // Through `build_match` and the real clock: two launches a whole
        // `SimHandle` build apart read different nanoseconds, so a launch
        // that fell back to `--seed`'s default would repeat it.
        for args in [&[][..], &["--spectate"]] {
            let (first, _) = build_match(&launch(args), rules.clone()).unwrap();
            let (second, _) = build_match(&launch(args), rules.clone()).unwrap();
            assert_ne!(
                first.seed(),
                second.seed(),
                "{args:?} draws a fresh seed per launch, not --seed's default {}",
                Cli::default().seed
            );
        }
    }

    /// M6: `--difficulty` and `--personality` choose the opponent of
    /// `--skip-menu` (and of a plain launch); an unknown id, a personality
    /// for `passive` and a `random` with an empty pool are errors.
    #[test]
    fn difficulty_and_personality_choose_the_opponent() {
        let rules =
            Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
                .unwrap();
        let (hard, _) = build_match(
            &launch(&["--skip-menu", "--seed", "5", "--difficulty", "hard"]),
            rules.clone(),
        )
        .unwrap();
        assert_eq!(
            *hard.setup(),
            MatchSetup::vs_bot(&rules, 5, "hard", "standard")
        );
        let (passive, _) = build_match(
            &launch(&["--seed", "5", "--difficulty", "passive"]),
            rules.clone(),
        )
        .unwrap();
        assert_eq!(passive.setup().ai_seats[0].bot, "passive");
        assert!(passive.setup().ai_seats[0].personality.is_empty());
        let err = |args: &[&str]| {
            build_match(&launch(args), rules.clone())
                .err()
                .unwrap_or_else(|| panic!("{args:?} builds"))
        };
        assert!(err(&["--difficulty", "nightmare"]).contains("unknown bot \"nightmare\""));
        assert!(err(&["--personality", "zerg"]).contains("no personality \"zerg\""));
        assert!(err(&["--difficulty", "passive", "--personality", "x"]).contains("takes none"));
        // The shipped pool (M6 slice 2: rush, boom and tower) resolves
        // `random` into the header; an empty pool is an error.
        let (random, _) = build_match(
            &launch(&["--seed", "5", "--personality", "random"]),
            rules.clone(),
        )
        .unwrap();
        let picked = random.setup().ai_seats[0].personality.clone();
        assert!(
            ["rush", "boom", "tower"].contains(&picked.as_str()),
            "{picked}"
        );
        let mut empty = rules.clone();
        for p in &mut empty.ai.personalities {
            p.random = false;
        }
        let e = build_match(&launch(&["--personality", "random"]), empty)
            .err()
            .expect("an empty pool is an error");
        assert!(e.contains("random: true"), "{e}");
    }

    /// `--scenario`, `--skip-menu` and `--spectate` start in `Skirmish`.
    /// `--spectate --seed 1` is the `m5_first_match` fixture's setup (its
    /// header, the informational git sha aside) with local input, and a
    /// scenario keeps its own seed unless `--seed` is given, even `--seed 1`.
    #[test]
    fn scenarios_skip_menu_and_spectate_start_in_skirmish() {
        use crate::menu::AppState;
        let rules =
            Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
                .unwrap();
        for args in [
            &["--scenario", "capture_seat"][..],
            &["--skip-menu"],
            &["--spectate"],
        ] {
            assert_eq!(initial_state(&launch(args)), AppState::Skirmish, "{args:?}");
        }
        let (spectated, input) =
            build_match(&launch(&["--spectate", "--seed", "1"]), rules.clone()).unwrap();
        assert_eq!(input.name(), "local");
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../sim/tests/fixtures/m5_first_match.eonreplay");
        let mut header = sim::replay::ReplayReader::open(&fixture).unwrap().setup;
        header.git_sha.clone_from(&spectated.setup().git_sha);
        assert_eq!(*spectated.setup(), header, "m5_first_match's setup");
        assert_eq!(*spectated.setup(), spectate_setup(&rules, 1));
        assert!(spectated.setup().players.iter().all(|p| p.is_ai));
        // A fast-forward runs before the window opens.
        let (forwarded, _) = build_match(
            &launch(&["--spectate", "--seed", "1", "--fast-forward-to", "40"]),
            rules.clone(),
        )
        .unwrap();
        assert_eq!(forwarded.tick(), 40);
        // A scenario's own seed stays unless --seed is explicit.
        let (own, _) =
            build_match(&launch(&["--scenario", "capture_seat"]), rules.clone()).unwrap();
        let (one, _) = build_match(
            &launch(&["--scenario", "capture_seat", "--seed", "1"]),
            rules.clone(),
        )
        .unwrap();
        assert_ne!(own.seed(), 1, "capture_seat has its own seed");
        assert_eq!(one.seed(), 1);
        assert!(build_match(&launch(&["--scenario", "nope"]), rules).is_err());
    }

    #[test]
    fn frame_limiter_period_and_deadlines() {
        let period = FrameLimiter { max_fps: 30 }.period();
        assert!((period.as_secs_f64() - 1.0 / 30.0).abs() < 1e-9);
        assert_eq!(FrameLimiter { max_fps: 0 }.period(), Duration::from_secs(1));
        let t0 = Instant::now();
        // First frame: one period from now.
        assert_eq!(next_deadline(None, t0, period), t0 + period);
        // On time (or early): advance from the previous deadline, not from now.
        let d1 = t0 + period;
        assert_eq!(
            next_deadline(Some(d1), d1 + Duration::from_millis(1), period),
            d1 + period
        );
        assert_eq!(next_deadline(Some(d1), d1, period), d1 + period);
        // Overran by more than a period: resynchronise from now.
        let late = d1 + period + Duration::from_millis(5);
        assert_eq!(next_deadline(Some(d1), late, period), late + period);
    }
}
