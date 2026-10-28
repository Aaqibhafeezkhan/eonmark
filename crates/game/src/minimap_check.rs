//! `--minimap-check` (art pass, acceptance AP6): the minimap's input
//! proven with real X input, as `--click-check` proves unit picking
//! (synthetic `PointerInput` fails on lavapipe, issue #33). Run on
//! `--scenario economy_walkthrough`. Once the sim reaches
//! [`MINIMAP_CHECK_TICK`] and the minimap is laid out, the game prints the
//! image node's rect and the first step; a driver (xdotool under Xvfb)
//! performs each step at the printed logical pixels, and after each the
//! game judges it, prints `minimap_check: pass <n> ...` and the next step:
//!
//! 1. `step 1 press px=<x>,<y>`: a left click on the south-east quarter;
//!    the camera's focus must move to that tile (by a minimap press).
//! 2. `step 2 drag from=<x>,<y> to=<x>,<y> tile=<tx>,<ty>`: a left drag; the focus must
//!    follow the pointer to the end tile (by `Pointer<Drag>` events: the
//!    press alone only reaches the start tile).
//! 3. `step 3 right_click px=<x>,<y>`: the check has selected the local
//!    player's units; exactly one `Move` of exactly those units to that
//!    tile's centre must be queued.
//! 4. `step 4 key=a left_click px=<x>,<y>`: the driver presses A (arming
//!    attack-move, `orders::order_mode_keys`), then left-clicks; exactly
//!    one `AttackMove` to that tile must be queued, the mode must be back
//!    to normal, and the camera must not have moved.
//!
//! Then it prints `minimap_check: ok` and exits 0, or at the first failed
//! step `minimap_check: FAIL <why>` and exits 1 (the `hud::verdict_line`
//! form); a run that ends before the last step prints `minimap_check:
//! FAIL stopped at step <n>`.

use bevy::app::AppExit;
use bevy::prelude::*;
use sim::{Command, UnitId};

use crate::camera::RtsCamera;
use crate::hud::WorldInputSet;
use crate::minimap::{Minimap, MinimapCanvas, MinimapStats, tile_centre};
use crate::orders::{OrderMode, own_selected_units};
use crate::present::world_from_sim;
use crate::selection::Selection;
use crate::sim_driver::{LOCAL_PLAYER, PendingCommands, SimHandle, SimSystems};

/// The sim tick at which the check starts.
pub const MINIMAP_CHECK_TICK: u32 = 40;
/// How close the camera's focus must come to a tile's centre, metres.
pub const FOCUS_TOLERANCE_M: f32 = 1.0;

/// Where the check is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Step {
    /// Waiting for the tick and the layout.
    #[default]
    Wait,
    /// Step 1, a left press.
    Press,
    /// Step 2, a left drag.
    Drag,
    /// Step 3, a right click.
    RightClick,
    /// Step 4, A then a left click.
    AttackMove,
    /// The verdict is out.
    Done,
}

impl Step {
    fn number(self) -> u8 {
        match self {
            Step::Wait | Step::Press => 1,
            Step::Drag => 2,
            Step::RightClick => 3,
            Step::AttackMove => 4,
            Step::Done => 5,
        }
    }
}

/// The step tiles (texels): the south-east quarter's centre, the drag's
/// end, the right click's and the attack-move click's targets, as shares
/// of the map's width and height.
pub const PRESS_AT: Vec2 = Vec2::new(0.75, 0.75);
/// The drag's end.
pub const DRAG_TO: Vec2 = Vec2::new(0.30, 0.45);
/// The right click.
pub const RIGHT_AT: Vec2 = Vec2::new(0.25, 0.60);
/// The attack-move click.
pub const ATTACK_AT: Vec2 = Vec2::new(0.35, 0.40);

/// State of the check.
#[derive(Resource, Debug, Default, Clone)]
pub struct MinimapCheck {
    /// The current step.
    pub step: Step,
    /// The minimap image's rect, logical px.
    pub rect: Option<Rect>,
    /// The tile the current step aims at.
    pub tile: (u16, u16),
    /// `MinimapStats::camera_moves` when the step began.
    pub moves_before: u32,
    /// `MinimapStats::drags` when the step began.
    pub drags_before: u32,
    /// The units selected for steps 3 and 4.
    pub units: Vec<UnitId>,
    /// Local commands queued since the step began.
    pub seen: Vec<Command>,
    /// The highest local seq already tallied.
    counted_through: Option<u32>,
    /// The first failure.
    pub failure: Option<String>,
}

impl MinimapCheck {
    /// Record the local commands waiting in `pending` not seen yet.
    pub fn tally(&mut self, pending: &PendingCommands) {
        for pc in pending.peek() {
            if pc.player != LOCAL_PLAYER || self.counted_through.is_some_and(|s| pc.seq <= s) {
                continue;
            }
            self.counted_through = Some(pc.seq);
            self.seen.push(pc.cmd.clone());
        }
    }

    /// The logical pixel at the centre of `tile` on the minimap.
    pub fn px(&self, tile: (u16, u16), size: (usize, usize)) -> Vec2 {
        let rect = self.rect.unwrap_or_default();
        let share = Vec2::new(
            (f32::from(tile.0) + 0.5) / size.0.max(1) as f32,
            (f32::from(tile.1) + 0.5) / size.1.max(1) as f32,
        );
        rect.min + share * rect.size()
    }
}

/// The tile at `share` of a `size` map.
pub fn tile_at(share: Vec2, size: (usize, usize)) -> (u16, u16) {
    let axis = |s: f32, n: usize| {
        u16::try_from(((s * n as f32) as usize).min(n.saturating_sub(1))).unwrap_or(0)
    };
    (axis(share.x, size.0), axis(share.y, size.1))
}

/// `px=<x>,<y> tile=<tx>,<ty>`, the pixel rounded.
fn at(p: Vec2, tile: (u16, u16)) -> String {
    format!(
        "px={},{} tile={},{}",
        p.x.round() as i64,
        p.y.round() as i64,
        tile.0,
        tile.1
    )
}

/// `minimap_check: ok`, or `minimap_check: FAIL <why>`.
pub fn verdict_line(failure: Option<&str>) -> String {
    match failure {
        None => "minimap_check: ok".to_string(),
        Some(why) => format!("minimap_check: FAIL {why}"),
    }
}

/// How far the camera's focus is from `tile`'s centre, metres (ground
/// plane).
fn focus_off(cameras: &Query<&RtsCamera>, tile: (u16, u16)) -> Option<(Vec3, f32)> {
    let want = world_from_sim(tile_centre(tile.0, tile.1), 0.0);
    cameras.single().ok().map(|c| {
        let d = Vec2::new(c.focus.x - want.x, c.focus.z - want.z).length();
        (c.focus, d)
    })
}

/// Prints the steps and judges them. Added by `app::run` under
/// `--minimap-check`.
pub struct MinimapCheckPlugin;

impl Plugin for MinimapCheckPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MinimapCheck>()
            .add_systems(FixedUpdate, tally_pending.before(SimSystems::Step))
            .add_systems(Update, run_minimap_check.after(WorldInputSet))
            .add_systems(Last, report_unfinished.after(bevy::window::ExitSystems));
    }
}

fn tally_pending(mut check: ResMut<MinimapCheck>, pending: Res<PendingCommands>) {
    check.tally(&pending);
}

/// The check's state machine (see the module docs).
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn run_minimap_check(
    sim: NonSend<SimHandle>,
    mut check: ResMut<MinimapCheck>,
    canvas: Option<Res<MinimapCanvas>>,
    stats: Res<MinimapStats>,
    nodes: Query<(&ComputedNode, &UiGlobalTransform), With<Minimap>>,
    cameras: Query<&RtsCamera>,
    buttons: Res<ButtonInput<MouseButton>>,
    mode: Res<OrderMode>,
    mut selection: ResMut<Selection>,
    pending: Res<PendingCommands>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(canvas) = canvas else {
        return;
    };
    let size = canvas.size;
    check.tally(&pending);
    let left = buttons.just_released(MouseButton::Left);
    let right = buttons.just_released(MouseButton::Right);
    let mut fail: Option<String> = None;
    match check.step {
        Step::Wait => {
            if sim.tick() < MINIMAP_CHECK_TICK {
                return;
            }
            let Ok((node, transform)) = nodes.single() else {
                return;
            };
            if node.size().min_element() <= 0.0 {
                return;
            }
            let scale = node.inverse_scale_factor;
            let centre = transform.translation * scale;
            let half = node.size() * scale / 2.0;
            let rect = Rect::from_center_half_size(centre, half);
            check.rect = Some(rect);
            println!(
                "minimap_check: rect left={} top={} width={} height={} texels={}x{}",
                rect.min.x.round(),
                rect.min.y.round(),
                rect.width().round(),
                rect.height().round(),
                size.0,
                size.1
            );
            check.tile = tile_at(PRESS_AT, size);
            check.moves_before = stats.camera_moves;
            let p = check.px(check.tile, size);
            println!("minimap_check: step 1 press {}", at(p, check.tile));
            check.step = Step::Press;
        }
        Step::Press if left => match focus_off(&cameras, check.tile) {
            _ if stats.camera_moves == check.moves_before => {
                fail = Some("step 1: the press did not move the camera".into());
            }
            Some((focus, d)) if d <= FOCUS_TOLERANCE_M && focus.x > 0.0 && focus.z > 0.0 => {
                println!("minimap_check: pass 1 focus={:.1},{:.1}", focus.x, focus.z);
                let from = check.tile;
                check.tile = tile_at(DRAG_TO, size);
                check.drags_before = stats.drags;
                let (a, b) = (check.px(from, size), check.px(check.tile, size));
                println!(
                    "minimap_check: step 2 drag from={},{} to={},{} tile={},{}",
                    a.x.round() as i64,
                    a.y.round() as i64,
                    b.x.round() as i64,
                    b.y.round() as i64,
                    check.tile.0,
                    check.tile.1
                );
                check.step = Step::Drag;
            }
            other => {
                fail = Some(format!(
                    "step 1: focus {:?} is not on tile {:?} in the south-east quarter",
                    other.map(|(f, _)| f),
                    check.tile
                ));
            }
        },
        Step::Drag if left => match focus_off(&cameras, check.tile) {
            _ if stats.drags == check.drags_before => {
                fail = Some("step 2: no drag reached the minimap".into());
            }
            Some((focus, d)) if d <= FOCUS_TOLERANCE_M => {
                println!(
                    "minimap_check: pass 2 focus={:.1},{:.1} drags={}",
                    focus.x,
                    focus.z,
                    stats.drags - check.drags_before
                );
                let own: Vec<UnitId> = sim
                    .view()
                    .units()
                    .filter(|u| u.owner == LOCAL_PLAYER)
                    .map(|u| u.id)
                    .collect();
                selection.set(own.iter().copied());
                check.units = own_selected_units(&sim.view(), &selection);
                check.tile = tile_at(RIGHT_AT, size);
                check.seen.clear();
                let p = check.px(check.tile, size);
                println!(
                    "minimap_check: step 3 right_click {} units={}",
                    at(p, check.tile),
                    check.units.len()
                );
                check.step = Step::RightClick;
            }
            other => {
                fail = Some(format!(
                    "step 2: focus {:?} did not follow the drag to tile {:?}",
                    other.map(|(f, _)| f),
                    check.tile
                ));
            }
        },
        Step::RightClick if right => {
            let want = Command::Move {
                units: check.units.clone(),
                target: tile_centre(check.tile.0, check.tile.1),
                queue: false,
            };
            if check.units.is_empty() {
                fail = Some("step 3: the local player has no units".into());
            } else if check.seen != [want] {
                fail = Some(format!(
                    "step 3: expected one Move of {} units to tile {:?}, saw {:?}",
                    check.units.len(),
                    check.tile,
                    check.seen
                ));
            } else {
                println!("minimap_check: pass 3 move units={}", check.units.len());
                check.tile = tile_at(ATTACK_AT, size);
                check.seen.clear();
                check.moves_before = stats.camera_moves;
                let p = check.px(check.tile, size);
                println!(
                    "minimap_check: step 4 key=a left_click {}",
                    at(p, check.tile)
                );
                check.step = Step::AttackMove;
            }
        }
        Step::AttackMove if left => {
            let target = tile_centre(check.tile.0, check.tile.1);
            let attack_moves = check
                .seen
                .iter()
                .filter(|c| matches!(c, Command::AttackMove { target: t, .. } if *t == target))
                .count();
            if check.seen.len() != 1 || attack_moves != 1 {
                fail = Some(format!(
                    "step 4: expected one AttackMove to tile {:?}, saw {:?} (mode {:?})",
                    check.tile, check.seen, *mode
                ));
            } else if *mode != OrderMode::Normal {
                fail = Some(format!("step 4: the mode stayed {:?}", *mode));
            } else if stats.camera_moves != check.moves_before {
                fail = Some("step 4: the attack-move click moved the camera".into());
            } else {
                println!("minimap_check: pass 4 attack_move");
                println!("{}", verdict_line(None));
                check.step = Step::Done;
                exit.write(AppExit::Success);
            }
        }
        _ => {}
    }
    if let Some(why) = fail {
        println!("{}", verdict_line(Some(&why)));
        check.failure = Some(why);
        check.step = Step::Done;
        exit.write(AppExit::error());
    }
}

/// On an exit before the verdict (`--exit-after-seconds`, a closed
/// window): say where the check stopped.
pub fn report_unfinished(mut exits: MessageReader<AppExit>, mut check: ResMut<MinimapCheck>) {
    if exits.is_empty() {
        return;
    }
    exits.clear();
    if check.step != Step::Done {
        let why = format!("stopped at step {}", check.step.number());
        println!("{}", verdict_line(Some(&why)));
        check.failure = Some(why);
        check.step = Step::Done;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::FxVec2;

    #[test]
    fn verdicts_and_step_pixels_are_exact() {
        assert_eq!(verdict_line(None), "minimap_check: ok");
        assert_eq!(
            verdict_line(Some("step 2: x")),
            "minimap_check: FAIL step 2: x"
        );
        let size = (128, 128);
        assert_eq!(tile_at(PRESS_AT, size), (96, 96));
        assert_eq!(tile_at(Vec2::new(1.0, 1.0), size), (127, 127), "clamped");
        let check = MinimapCheck {
            rect: Some(Rect::new(19.0, 499.0, 211.0, 691.0)),
            ..MinimapCheck::default()
        };
        // 192 px over 128 tiles: tile 96's centre is 96.5 * 1.5 px in.
        let p = check.px((96, 96), size);
        assert!((p.x - (19.0 + 144.75)).abs() < 1e-3, "{p}");
        assert!((p.y - (499.0 + 144.75)).abs() < 1e-3, "{p}");
        assert_eq!(at(p, (96, 96)), "px=164,644 tile=96,96");
    }

    #[test]
    fn tally_records_each_local_command_once() {
        let mut pending = PendingCommands::default();
        let mut check = MinimapCheck::default();
        let mv = Command::Move {
            units: vec![UnitId(1)],
            target: FxVec2::ZERO,
            queue: false,
        };
        pending.push_local(mv.clone());
        check.tally(&pending);
        check.tally(&pending);
        assert_eq!(check.seen, vec![mv.clone()]);
        pending.drain_sorted();
        check.tally(&pending);
        assert_eq!(check.seen, vec![mv]);
    }
}
