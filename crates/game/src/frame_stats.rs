//! Frame-time statistics and the `units200_auto` arrival proxy, printed on
//! exit so the M2 acceptance line "FPS >= 60, no periodic hitch at the 1 s
//! replay flush" and the `--max-fps` cap have numbers instead of eyeballs.
//!
//! `frame_ms mean=<f> p95=<f> max=<f> max_at_flush=<f>` is the frame
//! period in milliseconds over the run (`Time<Real>::delta`, the first frame
//! skipped). `max_at_flush` is the worst frame among the frame that sent a
//! recorder flush (every [`FLUSH_EVERY_TICKS`], see
//! [`DriverStats::flushes_sent`]) and the frame after it, where a blocking
//! write would show up. `frames=<n> seconds=<s> fps=<f>` is the plain mean
//! rate for the `--max-fps` check. `worst_ms=<ms>@<s>s/f<n> ...` lists the
//! [`WORST_FRAMES`] longest frames with the wall-clock second each ended at
//! and its `FrameCount`, so a one-off stall can be placed in time and
//! matched against the `debug` log (first ring and marker spawns). `window focused=<n>/<frames>
//! occluded=<n>/<frames> events=...` says how many frames ran with the
//! primary window focused and fully covered, and when that changed: an
//! unfocused window runs Bevy's `reactive_low_power` 60 Hz update mode
//! unless background mode switched to `continuous`, and a covered window is
//! not presented at all on macOS, so both change what the frame times
//! mean. `window logical=<w>x<h> physical=<w>x<h> scale=<f>` (art pass) is
//! the primary window's size as last seen, so a pasted run proves the size
//! it measured at (`--window-size 1280x720` on a Retina display reads
//! `physical=2560x1440 scale=2.0`). With the arrival proxy on,
//! `arrived=<n>/<total> by tick <t>` counts units whose move order has
//! completed and that hold a `Post` (the sim's own arrival state, what
//! `UnitArrived` reported): `n` at exit, `t` the tick the count first
//! reached `total` (or the exit tick when it never did). The next line,
//! `within_3_tiles=<k>/<total>`, is the `sim-cli bench` gate (units within
//! [`ARRIVED_TILES`] of the goal); 200 units of radius 0.35 cannot all pack
//! into that circle, so it is informational for a single-point goal.
//!
//! With the fight window on (the `enemy_outpost` scenarios, M4b acceptance
//! line 2, and `art_bench`, art pass; `app::prints_fight_lines`), `fight frames=<n> seconds=<s> mean=<ms> p95=<ms> max=<ms>
//! fps=<f> ticks=<a>..<b> on_screen_max=<n> on_screen_min=<n>/<alive>
//! units_at_first_hit=<p0>/<p1>` summarises only the frames of the 200 vs
//! 200 fight: a frame counts when a `Hit` landed within the last
//! [`FIGHT_HIT_WINDOW_TICKS`] ticks while both players still had at least
//! [`FIGHT_MIN_ARMED`] armed units alive (the `sim-cli bench --combat`
//! threshold), so start-up, the walk to contact and the mop-up are left
//! out. `ticks` are the first and last such tick. `on_screen_max` is the
//! most armed units (kinds with an attack) of either player projected
//! inside the world view between the HUD bars during a fight frame (by the
//! drag box's projection, [`crate::selection::units_in_rect`], not by
//! `ViewVisibility`, which the shadow cascades set for every unit), and
//! `on_screen_min` the fight frame that showed the smallest share of the
//! armed units then alive, as `<on screen>/<alive>`: equal numbers mean
//! every timed frame drew the whole battle, while a pan or zoom during the
//! fight shows as a lower first number though `on_screen_max` keeps its
//! best frame. `units_at_first_hit` is each player's armed units alive at
//! the first `Hit` of the match.

use std::time::Duration;

use bevy::app::AppExit;
use bevy::diagnostic::FrameCount;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowFocused, WindowOccluded};
use sim::{FxVec2, UnitId};

use crate::camera::RtsCamera;
use crate::hud::{BOTTOM_BAR_HEIGHT_PX, TOP_BAR_HEIGHT_PX};
use crate::present::world_from_sim;
use crate::selection::units_in_rect;
use crate::sim_driver::{
    DriverStats, FLUSH_EVERY_TICKS, SimEventMsg, SimHandle, SimSystems, sim_stepped,
};

/// The `within_3_tiles` line counts units this close to the goal, the same
/// gate `sim-cli bench` uses (docs/design/pathing.md). A measurement
/// threshold for the proxy line, not a gameplay number.
pub const ARRIVED_TILES: i64 = 3;

/// `ARRIVED_TILES` squared, in `FxVec2::dist_sq_i64` scale.
const ARRIVED_DIST_SQ: i64 = (ARRIVED_TILES * ARRIVED_TILES) << 32;

/// Prefix of every line `print_on_exit` writes to stdout, so scripts can
/// pick the frame-time report out of the log (`grep '^frame_stats:'`).
pub const STDOUT_PREFIX: &str = "frame_stats: ";

/// Frames flagged after each flush: the flush frame itself and the next.
const FLUSH_WINDOW_FRAMES: u8 = 2;

/// How many of the longest frames the `worst_ms` line lists.
pub const WORST_FRAMES: usize = 3;

/// Window focus / occlusion changes kept for the `window` line.
const MAX_WINDOW_EVENTS: usize = 12;

/// A frame belongs to the fight while the latest fighting tick is fewer
/// than this many ticks old (one second at 20 Hz). A measurement window
/// for the M4b proxy line, not a gameplay number.
pub const FIGHT_HIT_WINDOW_TICKS: u32 = 20;

/// A tick is a fighting tick when it has a `Hit` and both players keep at
/// least this many armed units alive: the `sim-cli bench --combat`
/// threshold (`docs/BUILD_TIMES.md`), so the window ends when one army is
/// down to stragglers.
pub const FIGHT_MIN_ARMED: u32 = 50;

/// The players the fight window counts: the two of the `enemy_outpost`
/// scenarios.
const FIGHT_PLAYERS: usize = 2;

/// Collected frame periods and flush attribution.
#[derive(Resource, Debug, Default)]
pub struct FrameStats {
    /// Frame periods in milliseconds, in order.
    pub samples_ms: Vec<f64>,
    /// Worst frame in a flush window.
    pub max_at_flush_ms: f64,
    /// `DriverStats::flushes_sent` last seen.
    flushes_seen: u32,
    /// Frames still to attribute to the latest flush.
    flush_window: u8,
    /// The longest frames as `(ms, wall-clock seconds at the frame's end,
    /// frame number)`, longest first, at most [`WORST_FRAMES`].
    pub worst: Vec<(f64, f64, u32)>,
    /// Frames sampled while the primary window had focus.
    pub focused_frames: u32,
    /// Frames sampled while the primary window was reported fully covered.
    pub occluded_frames: u32,
    /// The window is currently reported fully covered (`WindowOccluded`).
    occluded: bool,
    /// Focus and occlusion changes as `(label, seconds)`.
    window_events: Vec<(&'static str, f64)>,
    /// The exit lines were printed.
    printed: bool,
    /// The primary window's size as last sampled.
    pub window_size: Option<WindowSize>,
}

/// A window's size: logical pixels, physical pixels and the scale factor
/// between them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowSize {
    /// Logical width and height.
    pub logical: (f32, f32),
    /// Physical width and height.
    pub physical: (u32, u32),
    /// Physical pixels per logical pixel.
    pub scale: f32,
}

impl WindowSize {
    /// The size of `window`.
    pub fn of(window: &Window) -> Self {
        Self {
            logical: (window.width(), window.height()),
            physical: (window.physical_width(), window.physical_height()),
            scale: window.scale_factor(),
        }
    }
}

/// Mean, 95th percentile and maximum of the samples (milliseconds).
pub fn summarize(samples: &[f64]) -> (f64, f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let rank = ((sorted.len() as f64 * 0.95).ceil() as usize).clamp(1, sorted.len());
    (mean, sorted[rank - 1], sorted[sorted.len() - 1])
}

impl FrameStats {
    /// Record one frame period and attribute it to a flush window when due.
    /// `flushes_sent` is the driver's counter as of the frame that is
    /// running now; the period of that frame is the next sample, so a
    /// change opens the window after this one. `at_secs` (wall clock at
    /// the end of the frame) and `frame_no` place it on the `worst_ms` line.
    pub fn record(&mut self, frame: Duration, flushes_sent: u32, at_secs: f64, frame_no: u32) {
        let ms = frame.as_secs_f64() * 1000.0;
        self.samples_ms.push(ms);
        let slot = self.worst.partition_point(|(w, _, _)| *w >= ms);
        if slot < WORST_FRAMES {
            self.worst.insert(slot, (ms, at_secs, frame_no));
            self.worst.truncate(WORST_FRAMES);
        }
        if self.flush_window > 0 {
            self.flush_window -= 1;
            self.max_at_flush_ms = self.max_at_flush_ms.max(ms);
        }
        if flushes_sent != self.flushes_seen {
            self.flushes_seen = flushes_sent;
            self.flush_window = FLUSH_WINDOW_FRAMES;
        }
    }

    /// The `frame_ms ...` line.
    pub fn frame_line(&self) -> String {
        let (mean, p95, max) = summarize(&self.samples_ms);
        format!(
            "frame_ms mean={mean:.2} p95={p95:.2} max={max:.2} max_at_flush={:.2}",
            self.max_at_flush_ms
        )
    }

    /// Count this frame's window state.
    pub fn record_window(&mut self, focused: bool) {
        self.focused_frames += u32::from(focused);
        self.occluded_frames += u32::from(self.occluded);
    }

    /// Note a focus (`focused`) or occlusion (`occluded`) change at `secs`.
    pub fn window_event(&mut self, label: &'static str, secs: f64) {
        match label {
            "occluded" => self.occluded = true,
            "visible" => self.occluded = false,
            _ => {}
        }
        if self.window_events.len() < MAX_WINDOW_EVENTS {
            self.window_events.push((label, secs));
        }
    }

    /// The `worst_ms=...` line.
    pub fn worst_line(&self) -> String {
        let frames: Vec<String> = self
            .worst
            .iter()
            .map(|(ms, at, n)| format!("{ms:.2}@{at:.2}s/f{n}"))
            .collect();
        format!("worst_ms={}", frames.join(" "))
    }

    /// The `window ...` line.
    pub fn window_line(&self) -> String {
        let n = self.samples_ms.len();
        let mut events: Vec<String> = self
            .window_events
            .iter()
            .map(|(label, at)| format!("{label}@{at:.2}s"))
            .collect();
        if events.is_empty() {
            events.push("none".to_string());
        }
        format!(
            "window focused={}/{n} occluded={}/{n} events={}",
            self.focused_frames,
            self.occluded_frames,
            events.join(",")
        )
    }

    /// The `window logical=... physical=... scale=...` line.
    pub fn window_size_line(&self) -> String {
        match self.window_size {
            Some(s) => format!(
                "window logical={}x{} physical={}x{} scale={:.1}",
                s.logical.0.round(),
                s.logical.1.round(),
                s.physical.0,
                s.physical.1,
                s.scale
            ),
            None => "window logical=none physical=none scale=none".to_string(),
        }
    }

    /// `true` once the exit lines were printed.
    pub fn printed(&self) -> bool {
        self.printed
    }

    /// The `frames=... fps=...` line.
    pub fn rate_line(&self) -> String {
        let seconds = self.samples_ms.iter().sum::<f64>() / 1000.0;
        let fps = if seconds > 0.0 {
            self.samples_ms.len() as f64 / seconds
        } else {
            0.0
        };
        format!(
            "frames={} seconds={seconds:.2} fps={fps:.1}",
            self.samples_ms.len()
        )
    }
}

/// Arrival tracking for a scenario whose units all head for one goal.
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct ArrivalProxy {
    /// Where the scripted order sends every unit.
    pub goal: FxVec2,
    /// Units expected to arrive.
    pub total: u32,
    /// Units holding a `Post` (order completed) after the latest tick.
    pub arrived: u32,
    /// Units within [`ARRIVED_TILES`] of `goal` after the latest tick.
    pub within: u32,
    /// First tick at which `arrived == total`.
    pub all_arrived_at: Option<u32>,
}

impl ArrivalProxy {
    /// Track `total` units heading for `goal`.
    pub fn new(goal: FxVec2, total: u32) -> Self {
        Self {
            goal,
            total,
            arrived: 0,
            within: 0,
            all_arrived_at: None,
        }
    }

    /// Count after a tick from `(position, holds a post)` pairs; `tick` is
    /// the completed tick.
    pub fn observe(&mut self, units: impl Iterator<Item = (FxVec2, bool)>, tick: u32) {
        let (mut arrived, mut within) = (0u32, 0u32);
        for (pos, posted) in units {
            arrived += u32::from(posted);
            within += u32::from(pos.dist_sq_i64(self.goal) <= ARRIVED_DIST_SQ);
        }
        self.arrived = arrived;
        self.within = within;
        if self.arrived >= self.total && self.all_arrived_at.is_none() {
            self.all_arrived_at = Some(tick);
        }
    }

    /// The `arrived=...` line for an exit at `exit_tick`.
    pub fn line(&self, exit_tick: u32) -> String {
        format!(
            "arrived={}/{} by tick {}",
            self.arrived,
            self.total,
            self.all_arrived_at.unwrap_or(exit_tick)
        )
    }

    /// The `within_3_tiles=...` line.
    pub fn within_line(&self) -> String {
        format!(
            "within_{ARRIVED_TILES}_tiles={}/{}",
            self.within, self.total
        )
    }
}

/// The frames of a battle: what the `fight ...` line reports.
#[derive(Resource, Debug, Default, Clone, PartialEq)]
pub struct FightWindow {
    /// First and latest fighting tick: a completed tick with at least one
    /// `Hit` while both players kept [`FIGHT_MIN_ARMED`] armed units.
    pub ticks: Option<(u32, u32)>,
    /// Armed units alive per player after the first tick with any `Hit`.
    pub units_at_first_hit: Option<[u32; FIGHT_PLAYERS]>,
    /// Periods of the fight frames, milliseconds, in order.
    pub samples_ms: Vec<f64>,
    /// Most armed units projected inside the world view in one fight frame.
    pub on_screen_max: usize,
    /// The fight frame with the smallest share of the armed units alive
    /// projected inside the world view, as `(on screen, alive)`.
    pub on_screen_min: Option<(usize, usize)>,
}

impl FightWindow {
    /// Note a completed `tick` with `hits` `Hit` events, after which the
    /// players had `armed` armed units alive.
    pub fn observe_tick(&mut self, tick: u32, hits: u32, armed: [u32; FIGHT_PLAYERS]) {
        if hits == 0 {
            return;
        }
        self.units_at_first_hit.get_or_insert(armed);
        if armed.iter().all(|&n| n >= FIGHT_MIN_ARMED) {
            let first = self.ticks.map_or(tick, |(first, _)| first);
            self.ticks = Some((first, tick));
        }
    }

    /// `true` while the latest fighting tick is under
    /// [`FIGHT_HIT_WINDOW_TICKS`] ticks before `tick`, the sim's tick in
    /// the frame being measured.
    pub fn is_fight_frame(&self, tick: u32) -> bool {
        self.ticks
            .is_some_and(|(_, last)| tick.saturating_sub(last) < FIGHT_HIT_WINDOW_TICKS)
    }

    /// Record one fight frame: its period, and how many of the `alive`
    /// armed units were on screen in it.
    pub fn record_frame(&mut self, frame: Duration, on_screen: usize, alive: usize) {
        self.samples_ms.push(frame.as_secs_f64() * 1000.0);
        self.on_screen_max = self.on_screen_max.max(on_screen);
        // A smaller share `on_screen / alive`, compared without division; a
        // frame with no armed unit alive has nothing to show.
        if alive > 0
            && self
                .on_screen_min
                .is_none_or(|(seen, of)| on_screen * of < seen * alive)
        {
            self.on_screen_min = Some((on_screen, alive));
        }
    }

    /// The `fight ...` line.
    pub fn line(&self) -> String {
        let (mean, p95, max) = summarize(&self.samples_ms);
        // `fold` from +0.0: an empty `f64` `sum` is -0.0, printed "-0.00".
        let seconds = self.samples_ms.iter().fold(0.0, |a, b| a + b) / 1000.0;
        let fps = if seconds > 0.0 {
            self.samples_ms.len() as f64 / seconds
        } else {
            0.0
        };
        let ticks = self
            .ticks
            .map_or("none".to_string(), |(a, b)| format!("{a}..{b}"));
        let first_hit = self
            .units_at_first_hit
            .map_or("none".to_string(), |[p0, p1]| format!("{p0}/{p1}"));
        let on_screen_min = self
            .on_screen_min
            .map_or("none".to_string(), |(seen, alive)| {
                format!("{seen}/{alive}")
            });
        format!(
            "fight frames={} seconds={seconds:.2} mean={mean:.2} p95={p95:.2} max={max:.2} fps={fps:.1} ticks={ticks} on_screen_max={} on_screen_min={on_screen_min} units_at_first_hit={first_hit}",
            self.samples_ms.len(),
            self.on_screen_max
        )
    }
}

/// The part of a `size` logical window where the world shows: between the
/// top bar and the bottom bar.
pub fn world_view_rect(size: Vec2) -> Rect {
    Rect::new(
        0.0,
        TOP_BAR_HEIGHT_PX,
        size.x,
        (size.y - BOTTOM_BAR_HEIGHT_PX).max(TOP_BAR_HEIGHT_PX),
    )
}

/// How many of `positions` the camera projects inside the world view: the
/// drag box's projection test ([`units_in_rect`]) over
/// [`world_view_rect`] of the camera's logical viewport.
pub fn units_on_screen(
    camera: &Camera,
    camera_transform: &GlobalTransform,
    positions: impl IntoIterator<Item = (UnitId, Vec3)>,
) -> usize {
    let Some(size) = camera.logical_viewport_size() else {
        return 0;
    };
    units_in_rect(world_view_rect(size), camera, camera_transform, positions).len()
}

/// Collects frame periods and prints the summary lines when the app exits.
pub struct FrameStatsPlugin {
    /// Also count arrivals at this goal for this many units.
    pub arrival: Option<(FxVec2, u32)>,
    /// Also measure the fight window (the `enemy_outpost` scenarios).
    pub fight: bool,
}

impl Plugin for FrameStatsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrameStats>()
            .add_systems(Update, (window_events, sample_frame).chain())
            // After `bevy::window::ExitSystems`: the red close button's
            // `AppExit` is written there by `exit_on_all_closed`, in `Last`,
            // and the winit runner exits right after that frame, as for the
            // recorder (`sim_driver::finish_recorder_on_exit`).
            .add_systems(Last, print_on_exit.after(bevy::window::ExitSystems));
        if let Some((goal, total)) = self.arrival {
            app.insert_resource(ArrivalProxy::new(goal, total))
                .add_systems(FixedPostUpdate, observe_arrivals);
        }
        if self.fight {
            app.init_resource::<FightWindow>()
                .add_systems(
                    FixedPostUpdate,
                    observe_fight.after(SimSystems::Step).run_if(sim_stepped),
                )
                .add_systems(Update, sample_fight.after(sample_frame));
        }
    }
}

fn sample_frame(
    time: Res<Time<Real>>,
    frame: Res<FrameCount>,
    driver: Res<DriverStats>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut stats: ResMut<FrameStats>,
) {
    // The first frame's delta is zero (the clock has no previous update).
    if time.delta() > Duration::ZERO {
        stats.record(
            time.delta(),
            driver.flushes_sent,
            time.elapsed().as_secs_f64(),
            frame.0,
        );
        stats.record_window(windows.single().is_ok_and(|w| w.focused));
    }
    if let Ok(window) = windows.single() {
        stats.window_size = Some(WindowSize::of(window));
    }
}

/// Primary-window focus and occlusion changes, with their wall-clock time.
fn window_events(
    mut focus: MessageReader<WindowFocused>,
    mut occlusion: MessageReader<WindowOccluded>,
    primary: Query<(), With<PrimaryWindow>>,
    time: Res<Time<Real>>,
    mut stats: ResMut<FrameStats>,
) {
    let secs = time.elapsed().as_secs_f64();
    for msg in focus.read() {
        if primary.contains(msg.window) {
            stats.window_event(if msg.focused { "focused" } else { "unfocused" }, secs);
        }
    }
    for msg in occlusion.read() {
        if primary.contains(msg.window) {
            stats.window_event(if msg.occluded { "occluded" } else { "visible" }, secs);
        }
    }
}

fn observe_arrivals(sim: NonSend<SimHandle>, mut proxy: ResMut<ArrivalProxy>) {
    let view = sim.view();
    proxy.observe(view.units().map(|u| (u.pos, u.post.is_some())), sim.tick());
}

/// `true` for a unit of one of the fight's players whose kind has an
/// attack: the units the fight window counts.
fn fights(rules: &rules::Rules, unit: &sim::Unit) -> bool {
    usize::from(unit.owner.0) < FIGHT_PLAYERS
        && rules
            .unit_kind(unit.kind.0)
            .is_some_and(|k| k.attack.is_some())
}

/// After each stepped tick: count its `Hit` events and each player's armed
/// units (kinds with an attack) still alive.
fn observe_fight(
    sim: NonSend<SimHandle>,
    mut events: MessageReader<SimEventMsg>,
    mut fight: ResMut<FightWindow>,
) {
    let hits = events
        .read()
        .filter(|m| matches!(m.event, sim::SimEvent::Hit { .. }))
        .count();
    let rules = sim.rules();
    let mut armed = [0u32; FIGHT_PLAYERS];
    for u in sim.view().units().filter(|u| fights(rules, u)) {
        armed[usize::from(u.owner.0)] += 1;
    }
    fight.observe_tick(sim.tick(), u32::try_from(hits).unwrap_or(u32::MAX), armed);
}

/// In a fight frame, record its period and count the armed units alive and
/// those of them on screen.
fn sample_fight(
    time: Res<Time<Real>>,
    sim: NonSend<SimHandle>,
    cameras: Query<(&Camera, &GlobalTransform), With<RtsCamera>>,
    mut fight: ResMut<FightWindow>,
) {
    if time.delta() == Duration::ZERO || !fight.is_fight_frame(sim.tick()) {
        return;
    }
    let rules = sim.rules();
    let view = sim.view();
    let armed = || view.units().filter(|u| fights(rules, u));
    let on_screen = cameras.single().map_or(0, |(camera, transform)| {
        units_on_screen(
            camera,
            transform,
            armed().map(|u| (u.id, world_from_sim(u.pos, 0.0))),
        )
    });
    fight.record_frame(time.delta(), on_screen, armed().count());
}

/// Runs in `Last`, after `bevy::window::ExitSystems`: on the frame an
/// `AppExit` was written (Cmd-Q, `--exit-after-seconds`, the red close
/// button, a failed dev hierarchy check), print the summary lines.
pub(crate) fn print_on_exit(
    mut exits: MessageReader<AppExit>,
    mut stats: ResMut<FrameStats>,
    driver: Res<DriverStats>,
    arrival: Option<Res<ArrivalProxy>>,
    fight: Option<Res<FightWindow>>,
    sim: NonSend<SimHandle>,
) {
    if exits.is_empty() {
        return;
    }
    exits.clear();
    stats.printed = true;
    // Every line carries the `frame_stats: ` prefix that
    // `scripts/m2_checks.sh` (check 6) and docs/PLAYTEST.md grep for.
    println!("{STDOUT_PREFIX}{}", stats.frame_line());
    println!("{STDOUT_PREFIX}{}", stats.rate_line());
    println!("{STDOUT_PREFIX}{}", stats.worst_line());
    println!("{STDOUT_PREFIX}{}", stats.window_line());
    println!("{STDOUT_PREFIX}{}", stats.window_size_line());
    println!(
        "{STDOUT_PREFIX}ticks={} dropped_ticks={} stalled_ticks={} flushes={} (every {FLUSH_EVERY_TICKS} ticks)",
        sim.tick(),
        driver.dropped_ticks,
        driver.stalled_ticks,
        driver.flushes_sent
    );
    if let Some(arrival) = arrival {
        println!("{STDOUT_PREFIX}{}", arrival.line(sim.tick()));
        println!("{STDOUT_PREFIX}{}", arrival.within_line());
    }
    if let Some(fight) = fight {
        println!("{STDOUT_PREFIX}{}", fight.line());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_is_mean_p95_max() {
        let (mean, p95, max) = summarize(&[1.0, 2.0, 3.0, 4.0, 100.0]);
        assert!((mean - 22.0).abs() < 1e-9);
        assert_eq!(p95, 100.0);
        assert_eq!(max, 100.0);
        let twenty: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(summarize(&twenty).1, 19.0, "p95 of 1..=20 is the 19th");
        assert_eq!(summarize(&[]), (0.0, 0.0, 0.0));
    }

    #[test]
    fn flush_window_covers_the_flush_frame_and_the_next() {
        let mut s = FrameStats::default();
        let f = |ms: u64| Duration::from_millis(ms);
        s.record(f(10), 0, 0.01, 1); // quiet frame
        s.record(f(10), 1, 0.02, 2); // the flush was sent in the frame measured NEXT
        s.record(f(30), 1, 0.05, 3); // flush frame
        s.record(f(25), 1, 0.08, 4); // frame after
        s.record(f(90), 1, 0.17, 5); // outside the window
        assert_eq!(s.max_at_flush_ms, 30.0);
        assert_eq!(s.samples_ms.len(), 5);
        assert_eq!(
            s.frame_line(),
            "frame_ms mean=33.00 p95=90.00 max=90.00 max_at_flush=30.00"
        );
        assert_eq!(s.rate_line(), "frames=5 seconds=0.17 fps=30.3");
        assert_eq!(
            s.worst_line(),
            "worst_ms=90.00@0.17s/f5 30.00@0.05s/f3 25.00@0.08s/f4"
        );
    }

    /// The window size line reads the logical and physical sizes the
    /// window reports (a 1280 x 720 window at scale 2 is 2560 x 1440).
    #[test]
    fn window_size_line_names_logical_physical_and_scale() {
        let mut s = FrameStats::default();
        assert_eq!(
            s.window_size_line(),
            "window logical=none physical=none scale=none"
        );
        // As winit reports a 1280 x 720 logical window on a Retina display.
        let mut window = Window {
            resolution: bevy::window::WindowResolution::new(2560, 1440)
                .with_scale_factor_override(2.0),
            ..default()
        };
        s.window_size = Some(WindowSize::of(&window));
        assert_eq!(
            s.window_size_line(),
            "window logical=1280x720 physical=2560x1440 scale=2.0"
        );
        window.resolution = bevy::window::WindowResolution::new(1280, 800);
        s.window_size = Some(WindowSize::of(&window));
        assert_eq!(
            s.window_size_line(),
            "window logical=1280x800 physical=1280x800 scale=1.0"
        );
    }

    #[test]
    fn window_line_counts_focus_and_occlusion() {
        let mut s = FrameStats::default();
        assert_eq!(
            s.window_line(),
            "window focused=0/0 occluded=0/0 events=none"
        );
        let f = Duration::from_millis(8);
        s.record(f, 0, 0.1, 1);
        s.record_window(true);
        s.window_event("unfocused", 0.15);
        s.window_event("occluded", 0.2);
        s.record(f, 0, 0.2, 2);
        s.record_window(false);
        s.window_event("visible", 0.3);
        s.record(f, 0, 0.3, 3);
        s.record_window(false);
        assert_eq!(
            s.window_line(),
            "window focused=1/3 occluded=1/3 events=unfocused@0.15s,occluded@0.20s,visible@0.30s"
        );
    }

    /// The red close button: `bevy_window::exit_on_all_closed` writes
    /// `AppExit` in `Last`, inside `ExitSystems`, and the winit runner
    /// exits right after that frame, so the lines must print in that same
    /// frame. A `WindowPlugin` with no window stands in for the closed one.
    #[test]
    fn prints_when_the_window_closes() {
        use crate::sim_driver::{LocalInput, SimPlugin};
        let rules =
            rules::Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
                .expect("rules load");
        let handle = SimHandle::from_setup(sim::MatchSetup::scenario(&rules, 1), rules);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins((
            bevy::window::WindowPlugin {
                primary_window: None,
                ..default()
            },
            SimPlugin::new(handle, Box::new(LocalInput)),
            FrameStatsPlugin {
                arrival: None,
                fight: false,
            },
        ));
        app.update();
        assert_eq!(
            app.should_exit(),
            Some(AppExit::Success),
            "no window open: exit_on_all_closed exits"
        );
        assert!(app.world().resource::<FrameStats>().printed());
    }

    #[test]
    fn arrival_proxy_counts_and_remembers_the_first_full_tick() {
        let goal = FxVec2::from_ints(100, 64);
        let mut a = ArrivalProxy::new(goal, 2);
        let far = FxVec2::from_ints(10, 64);
        let near = FxVec2::from_ints(102, 65);
        // One unit still walking (far, no post), one that arrived.
        a.observe([(far, false), (near, true)].into_iter(), 5);
        assert_eq!((a.arrived, a.within), (1, 1));
        assert_eq!(a.all_arrived_at, None);
        assert_eq!(a.line(5), "arrived=1/2 by tick 5");
        assert_eq!(a.within_line(), "within_3_tiles=1/2");
        // Both hold a post; one was pushed out of the 3-tile circle.
        a.observe([(near, true), (far, true)].into_iter(), 9);
        assert_eq!((a.arrived, a.within), (2, 1));
        assert_eq!(a.all_arrived_at, Some(9));
        // A Stop clears the posts; the first full tick is remembered.
        a.observe([(far, false), (goal, false)].into_iter(), 12);
        assert_eq!(a.line(12), "arrived=0/2 by tick 9");
        assert_eq!(a.within_line(), "within_3_tiles=1/2");
    }

    #[test]
    fn fight_window_counts_only_frames_with_hits_and_both_armies() {
        let mut w = FightWindow::default();
        assert_eq!(
            w.line(),
            "fight frames=0 seconds=0.00 mean=0.00 p95=0.00 max=0.00 fps=0.0 ticks=none on_screen_max=0 on_screen_min=none units_at_first_hit=none"
        );
        let ms = |n: u64| Duration::from_millis(n);
        // `(tick, hits, armed)` after each tick, then a frame at that tick
        // of the given length with this many of the armed units alive on
        // screen; only fight frames are recorded, as `sample_fight` does.
        let frames: [(u32, u32, [u32; 2], u64, usize); 9] = [
            (100, 0, [200, 200], 10, 400), // start-up: no hit yet
            (150, 0, [200, 200], 11, 400), // walking to contact
            (151, 3, [200, 200], 20, 398), // first hit: the window opens
            (160, 0, [190, 195], 30, 385), // 9 ticks after a hit, all shown
            (170, 0, [185, 190], 40, 375), // 19 ticks after: still in
            (171, 0, [185, 190], 50, 999), // 20 ticks after: out
            (300, 2, [49, 150], 60, 999),  // P0 down to stragglers
            (310, 1, [52, 140], 70, 96),   // back in, zoomed on half of it
            (400, 5, [0, 120], 80, 999),   // mop-up: never counts
        ];
        for (tick, hits, armed, frame_ms, on_screen) in frames {
            w.observe_tick(tick, hits, armed);
            if w.is_fight_frame(tick) {
                let alive = usize::try_from(armed[0] + armed[1]).expect("small");
                w.record_frame(ms(frame_ms), on_screen, alive);
            }
        }
        assert_eq!(w.units_at_first_hit, Some([200, 200]));
        assert_eq!(w.ticks, Some((151, 310)));
        assert_eq!(w.samples_ms, vec![20.0, 30.0, 40.0, 70.0]);
        // The best frame still reads 398: only the smallest share (96 of
        // the 192 alive, not the smallest count) shows the zoomed frame.
        assert_eq!(w.on_screen_max, 398);
        assert_eq!(w.on_screen_min, Some((96, 192)));
        assert_eq!(
            w.line(),
            "fight frames=4 seconds=0.16 mean=40.00 p95=70.00 max=70.00 fps=25.0 ticks=151..310 on_screen_max=398 on_screen_min=96/192 units_at_first_hit=200/200"
        );
        // A share compares across frames: 375 of 375 shows more of the
        // battle than 398 of 400, and a frame with no armed unit alive has
        // nothing to show.
        let mut shares = FightWindow::default();
        shares.record_frame(ms(5), 375, 375);
        shares.record_frame(ms(5), 398, 400);
        shares.record_frame(ms(5), 0, 0);
        shares.record_frame(ms(5), 380, 380);
        assert_eq!(shares.on_screen_min, Some((398, 400)));
        let mut empty = FightWindow::default();
        empty.record_frame(ms(5), 0, 0);
        assert_eq!(empty.on_screen_min, None);
        // A hit while one army is under the threshold opens nothing.
        let mut lopsided = FightWindow::default();
        lopsided.observe_tick(5, 9, [FIGHT_MIN_ARMED - 1, 400]);
        assert!(!lopsided.is_fight_frame(5));
        assert_eq!(lopsided.ticks, None);
        assert_eq!(
            lopsided.units_at_first_hit,
            Some([FIGHT_MIN_ARMED - 1, 400])
        );
    }

    /// The RTS camera's pose and lens at `(x, y, zoom)` looking at a
    /// `WINDOW_SIZE` window, with the computed values a running camera has.
    fn camera_at(x: f32, y: f32, zoom: f32) -> (Camera, GlobalTransform) {
        let rts = RtsCamera {
            focus: Vec3::new(
                x - crate::ground::HALF_EXTENT,
                0.0,
                y - crate::ground::HALF_EXTENT,
            ),
            zoom,
            zoom_target: zoom,
        };
        let transform = GlobalTransform::from(
            Transform::from_translation(rts.eye()).looking_at(rts.focus, Vec3::Y),
        );
        let (w, h) = crate::app::WINDOW_SIZE;
        let mut camera = Camera::default();
        camera.computed.target_info = Some(bevy::camera::RenderTargetInfo {
            physical_size: UVec2::new(w, h),
            scale_factor: 1.0,
        });
        camera.computed.clip_from_view = Projection::Perspective(PerspectiveProjection {
            fov: 40.0_f32.to_radians(),
            near: 0.5,
            far: 400.0,
            aspect_ratio: w as f32 / h as f32,
            ..default()
        })
        .get_clip_from_view();
        (camera, transform)
    }

    #[test]
    fn units_on_screen_counts_what_projects_between_the_hud_bars() {
        assert_eq!(
            world_view_rect(Vec2::new(1280.0, 800.0)),
            Rect::new(0.0, TOP_BAR_HEIGHT_PX, 1280.0, 800.0 - BOTTOM_BAR_HEIGHT_PX)
        );
        let (camera, transform) = camera_at(64.0, 64.0, 40.0);
        let at = |x: i32, y: i32| world_from_sim(FxVec2::from_ints(x, y), 0.0);
        // The focus projects to the window centre; a tile 50 m off is out
        // of view; a point that projects into the bottom bar is not on
        // screen, though it is inside the window.
        let centre = camera.world_to_viewport(&transform, at(64, 64)).unwrap();
        assert!(
            (centre - Vec2::new(640.0, 400.0)).length() < 1.0,
            "{centre}"
        );
        let bottom_bar = (64..128)
            .map(|y| (y, camera.world_to_viewport(&transform, at(64, y)).unwrap()))
            .find(|(_, p)| p.y > 800.0 - BOTTOM_BAR_HEIGHT_PX && p.y < 800.0)
            .map(|(y, _)| y)
            .expect("a row under the bottom bar");
        let units = [
            (UnitId(1), at(64, 64)),
            (UnitId(2), at(66, 62)),
            (UnitId(3), at(64, 64 + 50)),
            (UnitId(4), at(64, bottom_bar)),
        ];
        assert_eq!(units_on_screen(&camera, &transform, units), 2);
        // No render target yet (the first frame): nothing is counted.
        assert_eq!(units_on_screen(&Camera::default(), &transform, units), 0);
    }

    /// The `<n>/<alive>` of the `on_screen_min=` field of a `fight` line.
    fn coverage(line: &str) -> (usize, usize) {
        let field = line
            .split(' ')
            .find_map(|f| f.strip_prefix("on_screen_min="))
            .unwrap_or_else(|| panic!("no on_screen_min= field: {line}"));
        let (seen, alive) = field
            .split_once('/')
            .unwrap_or_else(|| panic!("not <n>/<alive>: {line}"));
        (seen.parse().expect("n"), alive.parse().expect("alive"))
    }

    /// The `enemy_outpost` prologue through the real systems: the window
    /// opens at the first `Hit`, after the charge, with both armies whole
    /// and every armed unit of the battle on screen at the default framing;
    /// a camera move to a close-up during the fight shows in the line,
    /// though the most units on screen still reads the default framing's.
    #[test]
    fn the_outpost_prologue_opens_the_fight_window() {
        use crate::sim_driver::{ScenarioInput, SimPlugin};
        let rules =
            rules::Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
                .expect("rules load");
        let (setup, stream, _) =
            sim::scenarios::by_name("enemy_outpost", &rules).expect("scenario");
        let handle = SimHandle::from_setup(setup, rules);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                Duration::ZERO,
            ))
            .add_plugins((
                // The window messages `frame_stats` reads, and no exit.
                bevy::window::WindowPlugin {
                    primary_window: None,
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    ..default()
                },
                SimPlugin::new(handle, Box::new(ScenarioInput::new(stream))),
                FrameStatsPlugin {
                    arrival: None,
                    fight: true,
                },
            ));
        let framing = crate::app::default_framing(Some("enemy_outpost")).expect("framed");
        let (camera, transform) = camera_at(framing.x, framing.y, framing.zoom);
        let rts_camera = app
            .world_mut()
            .spawn((camera, transform, RtsCamera::default()))
            .id();
        app.update();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_millis(50),
        ));
        let mut opened_at = None;
        for _ in 0..400 {
            app.update();
            let tick = app.world().non_send::<SimHandle>().tick();
            let w = app.world().resource::<FightWindow>();
            if opened_at.is_none() && !w.samples_ms.is_empty() {
                opened_at = Some(tick);
            }
            if opened_at.is_some_and(|t| tick >= t + 10) {
                break;
            }
        }
        let opened_at = opened_at.expect("the fight window opened");
        let w = app.world().resource::<FightWindow>();
        println!("{}", w.line());
        let (first, _) = w.ticks.expect("fighting ticks");
        assert!(
            first > sim::scenarios::ENEMY_OUTPOST_T_CHARGE,
            "no hit before the charge: {first}"
        );
        assert_eq!(opened_at, first, "the frame of the first fighting tick");
        assert_eq!(w.units_at_first_hit, Some([200, 200]));
        assert!(w.samples_ms.iter().all(|&ms| (ms - 50.0).abs() < 1e-6));
        assert!(
            w.on_screen_max >= 350,
            "on screen at the default framing: {}",
            w.on_screen_max
        );
        let framed = w.line();
        // The director's `battle_close` beat, mid-fight: the 30 m close-up
        // shows only part of the battle from then on.
        let close = crate::director::BATTLE_CLOSE;
        let (camera, transform) = camera_at(close.x, close.y, close.zoom);
        app.world_mut()
            .entity_mut(rts_camera)
            .insert((camera, transform));
        let moved_at = app.world().non_send::<SimHandle>().tick();
        while app.world().non_send::<SimHandle>().tick() < moved_at + 10 {
            app.update();
        }
        let w = app.world().resource::<FightWindow>();
        let after = w.line();
        println!("{after}");
        assert!(w.on_screen_max >= 350, "the maximum cannot tell: {after}");
        let (seen, alive) = coverage(&after);
        assert!(seen < alive, "the close-up shows in the line: {after}");
        let (seen, alive) = coverage(&framed);
        assert_eq!(
            seen, alive,
            "every armed unit alive on screen in every frame at the default framing: {framed}"
        );
    }
}
