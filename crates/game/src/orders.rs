//! Orders (M2 decision 6): a right click on the ground becomes
//! `Command::Move { units: selection, target, queue: shift }`, S is Stop,
//! A then left click is the `AttackMove` placeholder (the sim applies it like
//! Move until M4a). The ground hit is one ray-plane intersection with
//! y = 0 (`Camera::viewport_to_world`, then solve for t): never a mesh
//! raycast. World clicks are ignored while the pointer is over the HUD
//! (`PointerOverUi`).
//!
//! Every order goes through `PendingCommands::push_local`, which stamps the
//! seq; nothing here touches the sim.
//!
//! A right click is contextual (M3b): with units selected, on one of the
//! player's construction sites it is `Build` on that site (the nearest
//! selected builder takes it over), on one of their buildings that has
//! gather slots it is `Gather`; with a building selected it is `SetRally`
//! at the ground point; otherwise it is `Move`.
//! The sim validates every one of them, so a wrong guess comes back as a
//! rejection on the message line, never as a silent change.
//!
//! [`OrderMode::Place`] is the build-placement mode the command card enters;
//! `placement.rs` owns its left click, a right click or Esc cancels it.
//!
//! Keys: S = Stop, A = attack-move mode (when something is selected), Esc
//! or a right click cancels the mode. The camera also pans on A and S
//! (`camera.rs`, WASD): the keybinding table in `docs/PLAYTEST.md` is the
//! place to settle that, not this file.

use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::PointerId;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sim::{BuildingId, BuildingKindId, Command, FxVec2};

use crate::buildings::BuildingRef;
use crate::camera::RtsCamera;
use crate::hud::{PointerOverUi, WorldInputSet};
use crate::present::sim_from_world;
use crate::selection::{RingAssets, Selection, spawn_move_marker};
use crate::sim_driver::{LOCAL_PLAYER, PendingCommands, SimHandle};

/// What the next left click on the ground means.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderMode {
    /// Left click selects, right click moves.
    #[default]
    Normal,
    /// After A: the next left click on the ground is an `AttackMove`; Esc or
    /// a right click cancels back to `Normal`.
    AttackMove,
    /// Placing a building of this kind: a ghost follows the cursor and a
    /// left click on the ground issues `Build` (`placement.rs`); Esc or a
    /// right click cancels back to `Normal`.
    Place(BuildingKindId),
}

/// Where the cursor ray meets the ground plane y = 0, in world metres, or
/// `None` when the ray misses (looking at the sky) or the camera has no
/// viewport yet.
pub fn ground_hit(
    camera: &Camera,
    camera_transform: &GlobalTransform,
    cursor: Vec2,
) -> Option<Vec3> {
    let ray = camera.viewport_to_world(camera_transform, cursor).ok()?;
    let t = ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Dir3::Y))?;
    Some(ray.get_point(t))
}

/// Queue a Move for the selection; returns the seq, or `None` when nothing
/// is selected (no command is queued).
pub fn issue_move(
    pending: &mut PendingCommands,
    selection: &Selection,
    target: FxVec2,
    queue: bool,
) -> Option<u32> {
    if selection.is_empty() {
        return None;
    }
    Some(pending.push_local(Command::Move {
        units: selection.units.iter().copied().collect(),
        target,
        queue,
    }))
}

/// Queue a Stop for the selection.
pub fn issue_stop(pending: &mut PendingCommands, selection: &Selection) -> Option<u32> {
    if selection.is_empty() {
        return None;
    }
    Some(pending.push_local(Command::Stop {
        units: selection.units.iter().copied().collect(),
    }))
}

/// Queue an `AttackMove` for the selection (applied like `Move` until M4a).
pub fn issue_attack_move(
    pending: &mut PendingCommands,
    selection: &Selection,
    target: FxVec2,
) -> Option<u32> {
    if selection.is_empty() {
        return None;
    }
    Some(pending.push_local(Command::AttackMove {
        units: selection.units.iter().copied().collect(),
        target,
    }))
}

/// Queue a Gather for the selected units onto `node`.
pub fn issue_gather(
    pending: &mut PendingCommands,
    selection: &Selection,
    node: BuildingId,
) -> Option<u32> {
    if selection.is_empty() {
        return None;
    }
    Some(pending.push_local(Command::Gather {
        units: selection.units.iter().copied().collect(),
        node,
    }))
}

/// Queue a `SetRally` for the selected building.
pub fn issue_rally(
    pending: &mut PendingCommands,
    selection: &Selection,
    target: FxVec2,
) -> Option<u32> {
    let building = selection.building?;
    Some(pending.push_local(Command::SetRally { building, target }))
}

/// Right click, S and A handling.
pub struct OrdersPlugin;

impl Plugin for OrdersPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OrderMode>().add_systems(
            Update,
            (order_mode_keys, issue_pointer_orders)
                .chain()
                .in_set(WorldInputSet),
        );
    }
}

/// A enters [`OrderMode::AttackMove`] when something is selected; Esc
/// returns to `Normal` (a right click does too, in
/// [`issue_pointer_orders`]); S issues a Stop (`issue_stop`).
pub fn order_mode_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut mode: ResMut<OrderMode>,
    selection: Res<Selection>,
    mut pending: ResMut<PendingCommands>,
) {
    if keys.just_pressed(KeyCode::Escape) && *mode != OrderMode::Normal {
        *mode = OrderMode::Normal;
    }
    if keys.just_pressed(KeyCode::KeyA) && !selection.is_empty() {
        *mode = OrderMode::AttackMove;
    }
    if keys.just_pressed(KeyCode::KeyS)
        && let Some(seq) = issue_stop(&mut pending, &selection)
    {
        info!("Stop seq {seq} for {} units", selection.units.len());
    }
}

/// On right-button release (Normal) or left-button release (`AttackMove`),
/// not over UI: `ground_hit` at the cursor, `sim_from_world`, then
/// `issue_move(.., queue = shift held)` or `issue_attack_move`; on success
/// `selection::spawn_move_marker` at the hit and the mode returns to
/// `Normal`. A right click in `AttackMove` mode only cancels the mode.
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn issue_pointer_orders(
    sim: NonSend<SimHandle>,
    hover: Res<HoverMap>,
    buildings: Query<&BuildingRef>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<RtsCamera>>,
    over_ui: Res<PointerOverUi>,
    mut mode: ResMut<OrderMode>,
    selection: Res<Selection>,
    mut pending: ResMut<PendingCommands>,
    ring_assets: Option<Res<RingAssets>>,
    time: Res<Time>,
    mut commands: Commands,
) {
    let right = buttons.just_released(MouseButton::Right);
    let left = buttons.just_released(MouseButton::Left);
    if !(right || left) {
        return;
    }
    if matches!(*mode, OrderMode::AttackMove | OrderMode::Place(_)) && right {
        *mode = OrderMode::Normal;
        return;
    }
    if over_ui.0 {
        return;
    }
    let Some(cursor) = windows.single().ok().and_then(Window::cursor_position) else {
        return;
    };
    let Ok((cam, cam_tf)) = camera.single() else {
        return;
    };
    let Some(hit) = ground_hit(cam, cam_tf, cursor) else {
        return;
    };
    let target = sim_from_world(hit);
    let issued = match *mode {
        OrderMode::Normal if right && selection.building.is_some() => {
            issue_rally(&mut pending, &selection, target)
        }
        OrderMode::Normal if right => match context_target(&sim, &hover, &buildings) {
            Some(Context::Site(site)) => issue_resume(&sim, &mut pending, &selection, site),
            Some(Context::Gather(node)) => issue_gather(&mut pending, &selection, node),
            None => {
                let queue = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
                issue_move(&mut pending, &selection, target, queue)
            }
        },
        OrderMode::AttackMove if left => {
            *mode = OrderMode::Normal;
            issue_attack_move(&mut pending, &selection, target)
        }
        _ => None,
    };
    if issued.is_some()
        && let Some(assets) = ring_assets.as_deref()
    {
        spawn_move_marker(&mut commands, assets, hit, time.elapsed_secs());
    }
}

/// What a right click on one of the local player's buildings means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    /// A construction site: a selected builder takes it over.
    Site(BuildingId),
    /// A complete building with gather slots: the units work it.
    Gather(BuildingId),
}

/// The local player's building under the mouse pointer, when a right click
/// on it means something other than a move.
fn context_target(
    sim: &SimHandle,
    hover: &HoverMap,
    buildings: &Query<&BuildingRef>,
) -> Option<Context> {
    let view = sim.view();
    let rules = sim.rules();
    hover
        .get(&PointerId::Mouse)?
        .keys()
        .filter_map(|e| buildings.get(*e).ok())
        .filter_map(|r| view.building(r.0))
        .filter(|b| b.owner == LOCAL_PLAYER)
        .find_map(|b| match b.construction {
            sim::Construction::Site { .. } => Some(Context::Site(b.id)),
            sim::Construction::Complete => rules
                .building_kind(b.kind.0)
                .is_some_and(|k| k.gather.is_some())
                .then_some(Context::Gather(b.id)),
        })
}

/// Queue a `Build` that hands `site` to the selected builder nearest it
/// (a resume: no cost). `None` when no selected unit can build.
fn issue_resume(
    sim: &SimHandle,
    pending: &mut PendingCommands,
    selection: &Selection,
    site: BuildingId,
) -> Option<u32> {
    let view = sim.view();
    let rules = sim.rules();
    let b = view.building(site)?;
    let kind = rules.building_kind(b.kind.0)?;
    let centre = sim::construction::centre_fx(kind, b.tile);
    let unit = crate::placement::nearest_builder(&view, rules, selection, centre)?;
    Some(pending.push_local(Command::Build {
        unit,
        kind: b.kind,
        tile: b.tile,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::UnitId;

    fn selection(n: u32) -> Selection {
        let mut s = Selection::default();
        s.set((1..=n).map(UnitId));
        s
    }

    #[test]
    fn orders_go_through_pending_commands_with_the_selection() {
        let mut pending = PendingCommands::default();
        let empty = Selection::default();
        assert_eq!(issue_move(&mut pending, &empty, FxVec2::ZERO, false), None);
        assert!(pending.is_empty());
        let sel = selection(3);
        let target = FxVec2::from_ints(10, 20);
        assert_eq!(issue_move(&mut pending, &sel, target, true), Some(0));
        assert_eq!(issue_stop(&mut pending, &sel), Some(1));
        assert_eq!(issue_attack_move(&mut pending, &sel, target), Some(2));
        let cmds = pending.drain_sorted();
        assert_eq!(cmds.len(), 3);
        assert_eq!(
            cmds[0].cmd,
            Command::Move {
                units: vec![UnitId(1), UnitId(2), UnitId(3)],
                target,
                queue: true
            }
        );
        assert!(matches!(cmds[1].cmd, Command::Stop { .. }));
        assert!(matches!(cmds[2].cmd, Command::AttackMove { .. }));
    }

    #[test]
    fn ground_hit_solves_the_y0_plane() {
        // A camera 10 m up looking straight down at the origin with a
        // known viewport: the viewport centre hits the origin, a corner
        // misses nothing but lands away from it; a camera looking up misses.
        let mut camera = Camera::default();
        // Fake the computed viewport so viewport_to_world has a size.
        camera.computed.target_info = Some(bevy::camera::RenderTargetInfo {
            physical_size: UVec2::new(800, 600),
            scale_factor: 1.0,
        });
        let projection = Projection::Perspective(PerspectiveProjection::default());
        camera.computed.clip_from_view = projection.get_clip_from_view();
        let down = GlobalTransform::from(
            Transform::from_xyz(0.0, 10.0, 0.0).looking_at(Vec3::ZERO, Vec3::NEG_Z),
        );
        let hit = ground_hit(&camera, &down, Vec2::new(400.0, 300.0)).expect("centre hits");
        assert!(hit.length() < 1e-3, "{hit}");
        assert!(hit.y.abs() < 1e-4);
        let corner = ground_hit(&camera, &down, Vec2::new(0.0, 0.0)).expect("corner hits");
        assert!(corner.length() > 1.0);
        assert!(corner.y.abs() < 1e-3);
        let up = GlobalTransform::from(
            Transform::from_xyz(0.0, 10.0, 0.0).looking_at(Vec3::new(0.0, 20.0, 0.0), Vec3::Z),
        );
        assert_eq!(ground_hit(&camera, &up, Vec2::new(400.0, 300.0)), None);
    }
}
