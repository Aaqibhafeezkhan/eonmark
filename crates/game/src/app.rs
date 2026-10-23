//! The windowed Bevy app: window, plugins, the sim driver with its input
//! source and recorder, and the `--exit-after-seconds` / `--max-fps` /
//! `--screenshot` smoke-test helpers.
//!
//! Which input source runs (M2 decision 2): `--scenario <name>` builds the
//! sim from `MatchSetup::scenario` (debug commands on) and feeds a
//! `ScenarioInput` (scripted stream + the player's commands); otherwise a
//! skirmish with `LocalInput`. Every windowed session records a replay
//! (decision 3) to `--replay-dir` or the platform data directory.

use std::time::{Duration, Instant};

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::{PresentMode, PrimaryWindow, WindowCloseRequested, WindowResolution};
use rules::Rules;
use sim::scenarios;

use crate::cli::{CameraStart, Cli};
use crate::frame_stats::FrameStatsPlugin;
use crate::hud::HudPlugin;
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

/// The sim and the input source a windowed session starts with.
pub fn build_match(cli: &Cli, rules: Rules) -> Result<(SimHandle, Box<dyn InputSource>), String> {
    match &cli.scenario {
        None => Ok((SimHandle::skirmish(rules, cli.seed), Box::new(LocalInput))),
        Some(name) => {
            let Some((mut setup, stream, _ticks)) = scenarios::by_name(name, &rules) else {
                return Err(format!(
                    "unknown scenario {name:?}; one of {}",
                    scenarios::NAMES.join(", ")
                ));
            };
            // `--seed` overrides the scenario's own seed only when given
            // explicitly (the default 1 would otherwise clobber it).
            if cli.seed != Cli::default().seed {
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
/// that fight; the player pans home, Period jumps to an idle Yeoman).
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
struct TickScreenshots(Vec<(u32, std::path::PathBuf)>);

/// Capture the primary window for every `--screenshot-at` whose tick the
/// sim has reached (the PNG lands a frame or two later).
fn take_tick_screenshots(
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
        // Nothing else is framed by default but the enemy_outpost pair.
        for name in scenarios::NAMES.iter().filter(|n| {
            !matches!(
                **n,
                "unit_looks" | "enemy_outpost" | "enemy_outpost_walkthrough"
            )
        }) {
            assert_eq!(default_framing(Some(name)), None, "{name}");
        }
        assert_eq!(default_framing(None), None);
    }

    /// Both `enemy_outpost` scenarios open on their prologue: at tick 3
    /// every one of the 400 battle units projects inside a 1280 x 800
    /// window between the HUD's bottom bar and the age strip under its top
    /// bar, through the RTS camera's own pose and lens at the default
    /// framing.
    #[test]
    fn enemy_outpost_is_framed_on_its_prologue_battle() {
        use crate::camera::{RtsCamera, ZOOM_MAX, ZOOM_MIN};
        use crate::hud::{BOTTOM_BAR_HEIGHT_PX, TOP_BAR_HEIGHT_PX};
        struct NoAi;
        impl sim::AiController for NoAi {
            fn think(&mut self, _: sim::PlayerId, _: &sim::SimView<'_>) -> Vec<sim::Command> {
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

        // The camera `camera.rs` spawns (40 degree vertical fov) at this
        // framing, looking at a 1280 x 800 window.
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
