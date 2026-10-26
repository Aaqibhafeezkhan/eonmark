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

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bevy::app::AppExit;
use bevy::prelude::*;
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

/// Default logical window size.
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
        // M5b: a plain launch plays the Standard bot (`MatchSetup::vs_bot`).
        None => Ok((
            SimHandle::vs_bot(
                rules,
                match_seed(cli),
                ai::BOT_STANDARD,
                STANDARD_PERSONALITY,
            ),
            Box::new(LocalInput),
        )),
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
/// the Mangonels, the Skirmishers' posts and the annexation timer are.
/// Every other scenario, and a skirmish, starts over the local player's
/// first units (`present::focus_camera_on_first_units`).
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
        _ => None,
    }
}

/// `true` for the scenarios whose prologue is the M4b 200 vs 200 fight:
/// they print the frame-time lines on exit.
pub fn is_enemy_outpost(scenario: Option<&str>) -> bool {
    scenario.is_some_and(|s| s.starts_with("enemy_outpost"))
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
    // The walkthrough's director beats come from the rules too (M4b).
    let director_beats = (cli.scenario.as_deref() == Some("enemy_outpost_walkthrough"))
        .then(|| crate::director::walkthrough_beats(&rules));
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
    let mut window = Window {
        // The title stays "Eonmark" (ROADMAP M0 acceptance); the seed,
        // tick and hash readout lives in the `dev` Sim panel.
        title: "Eonmark".into(),
        // Logical size: winit reads this as a LogicalSize at creation.
        resolution: WindowResolution::new(WINDOW_SIZE.0, WINDOW_SIZE.1),
        present_mode,
        ..default()
    };
    if background {
        background::configure_window(&mut window);
    }

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(window),
        ..default()
    }));
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
    // M5a: the attrition indicator, supply ring, annexation timer and
    // readout, and the outcome overlay over the frozen world.
    app.add_plugins((
        crate::markers::MarkersPlugin,
        crate::outcome::OutcomeOverlayPlugin,
    ));

    // Presentation only: frames and selects the walkthrough's beats.
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
    // acceptance line 2) and any `--max-fps` run, whose achieved rate they
    // document.
    let outpost = is_enemy_outpost(cli.scenario.as_deref());
    if units200_auto || outpost || cli.max_fps.is_some() {
        app.add_plugins(FrameStatsPlugin {
            arrival,
            fight: outpost,
        });
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
        // Nothing else is framed by default but the enemy_outpost pair and
        // capture_seat (`capture_seat_is_framed_on_the_seats_ring`).
        for name in scenarios::NAMES.iter().filter(|n| {
            !matches!(
                **n,
                "unit_looks" | "enemy_outpost" | "enemy_outpost_walkthrough" | "capture_seat"
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
        assert!(is_enemy_outpost(Some("enemy_outpost_walkthrough")));
        assert!(!is_enemy_outpost(Some("unit_looks")) && !is_enemy_outpost(None));
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
