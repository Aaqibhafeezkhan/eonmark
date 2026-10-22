//! Unit selection (M2 decision 5): click, shift-add, double-click same
//! kind on screen, screen-space drag box, control groups, and the retained
//! gizmo rings and move markers.
//!
//! Picking rules (fixed on day one, `docs/design/ui.md`): `MeshPickingPlugin`
//! with `MeshPickingSettings { require_markers: true }`, `MeshPickingCamera`
//! on the RTS camera only, `Pickable::default()` only on unit entities
//! (`present::sync_lifecycle` adds it). The drag box never raycasts: it
//! projects every sim unit position with `Camera::world_to_viewport` and
//! tests the rectangle. Rings and markers are SEPARATE flat entities keyed
//! by `UnitId`, never children of unit entities, so unit transforms stay
//! flat and a despawned unit never drags a child with it.
//!
//! Input paths: a `Pointer<Click>` observer handles clicks that land on a
//! unit (the mesh backend decides what was hit); the drag box and the
//! click-on-empty-ground deselect read raw `ButtonInput<MouseButton>` and
//! the window cursor in [`WorldInputSet`], guarded by `PointerOverUi`. Box
//! selection takes the local player's units only.

use std::collections::{BTreeSet, HashMap};

use bevy::gizmos::config::GizmoLineConfig;
use bevy::picking::events::{Click, Pointer};
use bevy::picking::hover::HoverMap;
use bevy::picking::mesh_picking::{MeshPickingCamera, MeshPickingPlugin, MeshPickingSettings};
use bevy::picking::pointer::{PointerButton, PointerId};
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use bevy::window::PrimaryWindow;
use sim::{BuildingId, SimView, UnitId};

use crate::buildings::BuildingRef;
use crate::camera::RtsCamera;
use crate::hud::{PointerOverUi, WorldInputSet};
use crate::orders::OrderMode;
use crate::palette;
use crate::present::{
    Interp, UNIT_Y, UnitEntities, UnitRef, flat_on_ground, fx_to_f32, world_from_sim,
};
use crate::sim_driver::{LOCAL_PLAYER, SimHandle};

/// A left press that moves less than this (logical px) before release is a
/// click, not a drag box.
pub const DRAG_THRESHOLD_PX: f32 = 4.0;
/// Two clicks on the same unit within this many seconds are a double click.
pub const DOUBLE_CLICK_SECS: f32 = 0.3;
/// Selection ring radius as a multiple of the unit's collision radius.
pub const RING_RADIUS_PER_UNIT_RADIUS: f32 = 1.4;
/// Move marker radius, metres.
pub const MOVE_MARKER_RADIUS: f32 = 0.5;
/// Move marker lifetime, seconds (wall clock; purely cosmetic).
pub const MOVE_MARKER_SECS: f32 = 0.6;
/// Rings float this far above the ground to avoid z-fighting.
pub const RING_Y: f32 = 0.03;
/// Segments per ring circle.
pub const RING_RESOLUTION: u32 = 48;
/// Ring and marker line width, pixels (thin and calm, `docs/ART_STYLE.md`).
pub const RING_LINE_WIDTH_PX: f32 = 1.5;

/// The selected units or building, and the nine control groups.
/// Render-side state: the sim knows nothing about selection. Units and a
/// building are never selected together: selecting either clears the
/// other.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// Currently selected units, in id order.
    pub units: BTreeSet<UnitId>,
    /// The selected building (M3b), when no unit is selected.
    pub building: Option<BuildingId>,
    /// Control groups 1..=9 at indices 0..=8.
    pub groups: [BTreeSet<UnitId>; 9],
}

impl Selection {
    /// Replace the selection.
    pub fn set(&mut self, units: impl IntoIterator<Item = UnitId>) {
        self.units = units.into_iter().collect();
        self.building = None;
    }

    /// Select one building alone.
    pub fn set_building(&mut self, building: BuildingId) {
        self.units.clear();
        self.building = Some(building);
    }

    /// Deselect everything.
    pub fn clear(&mut self) {
        self.units.clear();
        self.building = None;
    }

    /// Shift-click: add if absent, remove if present.
    pub fn toggle(&mut self, unit: UnitId) {
        self.building = None;
        if !self.units.remove(&unit) {
            self.units.insert(unit);
        }
    }

    /// Add without removing.
    pub fn extend(&mut self, units: impl IntoIterator<Item = UnitId>) {
        self.building = None;
        self.units.extend(units);
    }

    /// `true` when `unit` is selected.
    pub fn contains(&self, unit: UnitId) -> bool {
        self.units.contains(&unit)
    }

    /// `true` when nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    /// Ctrl+`digit`: store the selection in group `digit` (1..=9). Ignores
    /// other digits.
    pub fn assign_group(&mut self, digit: u8) {
        if let Some(slot) = Self::slot(digit) {
            self.groups[slot] = self.units.clone();
        }
    }

    /// `digit`: recall group `digit` (1..=9) into the selection. Returns
    /// `false` and leaves the selection alone when the group is empty or
    /// the digit is out of range.
    pub fn recall_group(&mut self, digit: u8) -> bool {
        match Self::slot(digit) {
            Some(slot) if !self.groups[slot].is_empty() => {
                self.units = self.groups[slot].clone();
                self.building = None;
                true
            }
            _ => false,
        }
    }

    /// Drop a selected building that no longer exists.
    pub fn prune_building(&mut self, exists: impl Fn(BuildingId) -> bool) {
        if self.building.is_some_and(|b| !exists(b)) {
            self.building = None;
        }
    }

    /// Drop ids that no longer exist (dead or despawned units) from the
    /// selection and every group.
    pub fn prune(&mut self, alive: impl Fn(UnitId) -> bool) {
        self.units.retain(|u| alive(*u));
        for g in &mut self.groups {
            g.retain(|u| alive(*u));
        }
    }

    fn slot(digit: u8) -> Option<usize> {
        (1..=9).contains(&digit).then(|| usize::from(digit - 1))
    }

    /// The digit for a number-row key, if it is one.
    pub fn digit_of(key: KeyCode) -> Option<u8> {
        Some(match key {
            KeyCode::Digit1 => 1,
            KeyCode::Digit2 => 2,
            KeyCode::Digit3 => 3,
            KeyCode::Digit4 => 4,
            KeyCode::Digit5 => 5,
            KeyCode::Digit6 => 6,
            KeyCode::Digit7 => 7,
            KeyCode::Digit8 => 8,
            KeyCode::Digit9 => 9,
            _ => return None,
        })
    }
}

/// A left drag in progress, in logical window pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragRect {
    /// Where the button went down.
    pub start: Vec2,
    /// Where the cursor is now.
    pub current: Vec2,
}

impl DragRect {
    /// The normalised rectangle.
    pub fn rect(&self) -> Rect {
        Rect::from_corners(self.start, self.current)
    }

    /// `true` while the cursor has not moved past [`DRAG_THRESHOLD_PX`].
    pub fn is_click(&self) -> bool {
        self.start.distance(self.current) < DRAG_THRESHOLD_PX
    }
}

/// The current drag box, if the left button is held on the world.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq)]
pub struct DragBox(pub Option<DragRect>);

/// The `bevy_ui` node that draws the drag box (`Pickable::IGNORE`).
#[derive(Component, Debug, Clone, Copy)]
pub struct DragBoxNode;

/// Remembers the last click for double-click detection.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq)]
pub struct LastClick {
    /// Entity and wall-clock time of the previous primary click on a unit.
    pub previous: Option<(Entity, f32)>,
}

/// A retained-gizmo ring under a selected unit; one entity per selected
/// `UnitId`, tracked in [`SelectionRings`].
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionRing(pub UnitId);

/// Ring entities by unit id.
#[derive(Resource, Default, Debug)]
pub struct SelectionRings(pub HashMap<UnitId, Entity>);

/// A short-lived ring at an order's target point.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct MoveMarker {
    /// `Time::elapsed_secs()` after which the marker despawns.
    pub expires_at: f32,
}

/// The two `GizmoAsset`s shared by every ring and marker (one circle each,
/// unit radius; entities scale their `Transform`).
#[derive(Resource, Debug, Clone)]
pub struct RingAssets {
    /// Unit-radius circle in `palette::SELECTION_RING`.
    pub selection: Handle<GizmoAsset>,
    /// Unit-radius circle in `palette::MOVE_MARKER`.
    pub marker: Handle<GizmoAsset>,
}

/// Picking, selection input, rings and markers.
pub struct SelectionPlugin;

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MeshPickingPlugin)
            .insert_resource(MeshPickingSettings {
                require_markers: true,
                ..default()
            })
            .init_resource::<Selection>()
            .init_resource::<DragBox>()
            .init_resource::<LastClick>()
            .init_resource::<SelectionRings>()
            .add_systems(Startup, (create_ring_assets, spawn_drag_box_node))
            .add_systems(PostStartup, mark_picking_camera)
            .add_observer(on_unit_click)
            .add_observer(on_building_click)
            .add_systems(Update, draw_building_selection)
            .add_systems(
                Update,
                (
                    update_drag_box,
                    control_groups,
                    select_idle_builder,
                    clear_selection_on_escape,
                    expire_move_markers,
                )
                    .chain()
                    .before(crate::orders::order_mode_keys)
                    .in_set(WorldInputSet),
            )
            .add_systems(Update, draw_drag_box.after(WorldInputSet))
            // Before `FrameCount` advances, so the frame number matches the
            // one `frame_stats` records for this frame.
            .add_systems(
                Last,
                log_first_gizmo_spawns.before(bevy::diagnostic::update_frame_count),
            )
            .add_systems(
                PostUpdate,
                (prune_dead, sync_selection_rings)
                    .chain()
                    .before(TransformSystems::Propagate),
            );
    }
}

/// Adds `MeshPickingCamera` to the RTS camera. `camera.rs` spawns it with
/// `Commands` in `Startup`, so this runs in `PostStartup`, after that
/// schedule's commands were applied.
fn mark_picking_camera(
    mut commands: Commands,
    cameras: Query<Entity, (With<RtsCamera>, Without<MeshPickingCamera>)>,
) {
    for cam in &cameras {
        commands.entity(cam).insert(MeshPickingCamera);
    }
}

/// Builds the two shared ring assets: a unit circle laid flat
/// (`present::flat_on_ground()`), [`RING_RESOLUTION`] segments.
fn create_ring_assets(mut commands: Commands, mut gizmo_assets: ResMut<Assets<GizmoAsset>>) {
    let flat = Isometry3d::new(Vec3::ZERO, flat_on_ground());
    let mut selection = GizmoAsset::new();
    selection
        .circle(flat, 1.0, palette::SELECTION_RING)
        .resolution(RING_RESOLUTION);
    let mut marker = GizmoAsset::new();
    marker
        .circle(flat, 1.0, palette::MOVE_MARKER)
        .resolution(RING_RESOLUTION);
    commands.insert_resource(RingAssets {
        selection: gizmo_assets.add(selection),
        marker: gizmo_assets.add(marker),
    });
}

/// Spawns the hidden drag-box node; [`draw_drag_box`] shows and sizes it.
fn spawn_drag_box_node(mut commands: Commands) {
    commands.spawn((
        Name::new("Drag box"),
        DragBoxNode,
        Node {
            position_type: PositionType::Absolute,
            display: Display::None,
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(palette::DRAG_BOX_FILL),
        BorderColor::all(palette::DRAG_BOX_BORDER),
        GlobalZIndex(10),
        Pickable::IGNORE,
    ));
}

/// Shift held (either key).
fn shift_held(keys: &ButtonInput<KeyCode>) -> bool {
    keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight])
}

/// Primary click on a unit entity: plain click selects it alone; shift
/// toggles it; a second primary click on the same entity within
/// [`DOUBLE_CLICK_SECS`] selects every unit of the same kind whose
/// projected position is inside the camera's logical viewport rect.
/// Ignored while [`PointerOverUi`] is set, in attack-move mode, when the
/// release ends a box drag, or when the target has no [`UnitRef`] (the
/// event propagates to the window entity too).
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn on_unit_click(
    click: On<Pointer<Click>>,
    units: Query<&UnitRef>,
    mut selection: ResMut<Selection>,
    over_ui: Res<PointerOverUi>,
    keys: Res<ButtonInput<KeyCode>>,
    mut last: ResMut<LastClick>,
    time: Res<Time>,
    sim: NonSend<SimHandle>,
    camera: Query<(&Camera, &GlobalTransform), With<RtsCamera>>,
    drag: Res<DragBox>,
    mode: Res<OrderMode>,
) {
    let target = click.event().entity;
    let Ok(unit) = units.get(target) else {
        return;
    };
    if click.event().event.button != PointerButton::Primary
        || over_ui.0
        || *mode != OrderMode::Normal
        || drag.0.is_some_and(|d| !d.is_click())
    {
        return;
    }
    let shift = shift_held(&keys);
    let now = time.elapsed_secs();
    let double = last
        .previous
        .is_some_and(|(e, t)| e == target && now > t && now - t <= DOUBLE_CLICK_SECS);
    if !double {
        last.previous = Some((target, now));
        if shift {
            selection.toggle(unit.0);
        } else {
            selection.set([unit.0]);
        }
        return;
    }
    last.previous = None;
    let view = sim.view();
    let Some(kind) = view.unit(unit.0).map(|u| u.kind) else {
        return;
    };
    let Ok((cam, cam_tf)) = camera.single() else {
        return;
    };
    let Some(rect) = cam.logical_viewport_rect() else {
        return;
    };
    let same_kind = units_in_rect(
        rect,
        cam,
        cam_tf,
        view.units()
            .filter(|u| u.kind == kind && u.owner == LOCAL_PLAYER)
            .map(|u| (u.id, world_from_sim(u.pos, UNIT_Y))),
    );
    if shift {
        selection.extend(same_kind);
    } else {
        selection.set(same_kind);
    }
}

/// Primary click on one of the local player's buildings selects it alone
/// (`Selection::set_building`). The same gates as [`on_unit_click`]: not
/// over the HUD, `Normal` mode, not the end of a box drag. Another
/// player's building is not selectable yet.
pub fn on_building_click(
    click: On<Pointer<Click>>,
    buildings: Query<&BuildingRef>,
    mut selection: ResMut<Selection>,
    over_ui: Res<PointerOverUi>,
    sim: NonSend<SimHandle>,
    drag: Res<DragBox>,
    mode: Res<OrderMode>,
) {
    let Ok(building) = buildings.get(click.event().entity) else {
        return;
    };
    if click.event().event.button != PointerButton::Primary
        || over_ui.0
        || *mode != OrderMode::Normal
        || drag.0.is_some_and(|d| !d.is_click())
    {
        return;
    }
    let view = sim.view();
    if view
        .building(building.0)
        .is_some_and(|b| b.owner == LOCAL_PLAYER)
    {
        selection.set_building(building.0);
    }
}

/// Outline the selected building's footprint on the ground, a little
/// outside it, in the selection-ring colour.
pub fn draw_building_selection(
    selection: Res<Selection>,
    sim: NonSend<SimHandle>,
    mut gizmos: Gizmos,
) {
    let Some(id) = selection.building else {
        return;
    };
    let view = sim.view();
    let Some(b) = view.building(id) else {
        return;
    };
    let Some(kind) = sim.rules().building_kind(b.kind.0) else {
        return;
    };
    let (w, h) = (kind.width_tiles as f32, kind.height_tiles as f32);
    let centre = Vec3::new(
        f32::from(b.tile.x) + w * 0.5 - crate::ground::HALF_EXTENT,
        RING_Y,
        f32::from(b.tile.y) + h * 0.5 - crate::ground::HALF_EXTENT,
    );
    gizmos.rect(
        Isometry3d::new(centre, flat_on_ground()),
        Vec2::new(w + 0.6, h + 0.6),
        palette::SELECTION_RING,
    );
}

/// Left button: on press (not over UI, in `Normal` mode) start a
/// [`DragRect`] at the cursor; while held, update `current`; on release, if
/// `!is_click()`, select the units whose sim position projects inside the
/// rect (shift extends) via [`select_in_rect`]; a release that is still a
/// click on empty ground clears the selection unless shift is held. Units
/// clicked directly are handled by [`on_unit_click`], which fires on the
/// same release; the hover map tells the two apart.
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn update_drag_box(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    over_ui: Res<PointerOverUi>,
    mode: Res<OrderMode>,
    hover: Res<HoverMap>,
    units: Query<(), Or<(With<UnitRef>, With<BuildingRef>)>>,
    mut drag: ResMut<DragBox>,
    mut selection: ResMut<Selection>,
    sim: NonSend<SimHandle>,
    camera: Query<(&Camera, &GlobalTransform), With<RtsCamera>>,
) {
    let cursor = windows.single().ok().and_then(Window::cursor_position);
    if buttons.just_pressed(MouseButton::Left)
        && !over_ui.0
        && *mode == OrderMode::Normal
        && let Some(c) = cursor
    {
        drag.0 = Some(DragRect {
            start: c,
            current: c,
        });
    }
    if buttons.pressed(MouseButton::Left)
        && let (Some(d), Some(c)) = (drag.0.as_mut(), cursor)
    {
        d.current = c;
    }
    if !buttons.just_released(MouseButton::Left) {
        return;
    }
    let Some(d) = drag.0.take() else {
        return;
    };
    let shift = shift_held(&keys);
    if d.is_click() {
        let over_unit = hover
            .get(&PointerId::Mouse)
            .is_some_and(|hits| hits.keys().any(|e| units.contains(*e)));
        if !over_ui.0 && !over_unit && !shift {
            selection.clear();
        }
        return;
    }
    let Ok((cam, cam_tf)) = camera.single() else {
        return;
    };
    select_in_rect(d.rect(), shift, cam, cam_tf, &sim.view(), &mut selection);
}

/// Box selection: the local player's units whose positions project inside
/// `rect` replace the selection (or extend it). Returns how many matched.
pub fn select_in_rect(
    rect: Rect,
    extend: bool,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    view: &SimView<'_>,
    selection: &mut Selection,
) -> usize {
    let hits = units_in_rect(
        rect,
        camera,
        camera_transform,
        view.units()
            .filter(|u| u.owner == LOCAL_PLAYER)
            .map(|u| (u.id, world_from_sim(u.pos, UNIT_Y))),
    );
    let n = hits.len();
    if extend {
        selection.extend(hits);
    } else {
        selection.set(hits);
    }
    n
}

/// Shows the drag-box node over the current [`DragRect`] once it is past
/// the click threshold; hides it otherwise.
fn draw_drag_box(drag: Res<DragBox>, mut nodes: Query<&mut Node, With<DragBoxNode>>) {
    for mut node in &mut nodes {
        match drag.0.filter(|d| !d.is_click()) {
            Some(d) => {
                let r = d.rect();
                node.display = Display::Flex;
                node.left = Val::Px(r.min.x);
                node.top = Val::Px(r.min.y);
                node.width = Val::Px(r.width());
                node.height = Val::Px(r.height());
            }
            None => node.display = Display::None,
        }
    }
}

/// The ids whose world positions project inside `rect` (logical pixels).
/// Positions behind the camera (`world_to_viewport` errors) are skipped.
pub fn units_in_rect(
    rect: Rect,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    positions: impl IntoIterator<Item = (UnitId, Vec3)>,
) -> BTreeSet<UnitId> {
    positions
        .into_iter()
        .filter_map(|(id, pos)| {
            camera
                .world_to_viewport(camera_transform, pos)
                .ok()
                .filter(|p| rect.contains(*p))
                .map(|_| id)
        })
        .collect()
}

/// Ctrl+1..9 assigns, 1..9 recalls (`Selection::assign_group` /
/// `recall_group`). Cmd is not used: it is the menu modifier on macOS.
pub fn control_groups(keys: Res<ButtonInput<KeyCode>>, mut selection: ResMut<Selection>) {
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    for key in keys.get_just_pressed() {
        if let Some(digit) = Selection::digit_of(*key) {
            if ctrl {
                selection.assign_group(digit);
            } else {
                selection.recall_group(digit);
            }
        }
    }
}

/// The local player's next idle builder after `after` by id, wrapping to
/// the first: a unit of a builder kind with no job and no order (holding a
/// post counts as idle, as in the sim's idle seeking).
pub fn next_idle_builder(
    view: &SimView<'_>,
    rules: &rules::Rules,
    after: Option<UnitId>,
) -> Option<UnitId> {
    let idle: Vec<UnitId> = view
        .units()
        .filter(|u| {
            u.owner == LOCAL_PLAYER
                && u.job.is_none()
                && u.order.is_none()
                && rules.unit_kind(u.kind.0).is_some_and(|k| k.builder)
        })
        .map(|u| u.id)
        .collect();
    after
        .and_then(|a| idle.iter().find(|&&id| id > a))
        .or_else(|| idle.first())
        .copied()
}

/// Period selects the next idle builder (`next_idle_builder`, cycling from
/// the one selected last) and centres the camera on it.
pub fn select_idle_builder(
    keys: Res<ButtonInput<KeyCode>>,
    sim: NonSend<SimHandle>,
    mut selection: ResMut<Selection>,
    mut cameras: Query<&mut RtsCamera>,
    mut last: Local<Option<UnitId>>,
) {
    if !keys.just_pressed(KeyCode::Period) {
        return;
    }
    let view = sim.view();
    let Some(id) = next_idle_builder(&view, sim.rules(), *last) else {
        return;
    };
    *last = Some(id);
    selection.set([id]);
    if let Some(u) = view.unit(id) {
        let at = world_from_sim(u.pos, 0.0);
        for mut cam in &mut cameras {
            cam.focus = Vec3::new(at.x, 0.0, at.z);
        }
    }
}

/// Esc clears the selection (the pause menu takes Esc over from M6).
pub fn clear_selection_on_escape(
    keys: Res<ButtonInput<KeyCode>>,
    mode: Res<OrderMode>,
    mut selection: ResMut<Selection>,
) {
    // In attack-move or placement mode Esc only leaves the mode
    // (`orders::order_mode_keys`, which runs after this).
    if keys.just_pressed(KeyCode::Escape) && *mode == OrderMode::Normal {
        selection.clear();
    }
}

/// Drop ids that left the sim from the selection and groups.
pub fn prune_dead(sim: NonSend<SimHandle>, mut selection: ResMut<Selection>) {
    if selection.is_empty()
        && selection.building.is_none()
        && selection.groups.iter().all(BTreeSet::is_empty)
    {
        return;
    }
    let view = sim.view();
    selection.prune(|id| view.unit(id).is_some());
    selection.prune_building(|id| view.building(id).is_some());
}

/// One ring entity per selected unit: spawn `(SelectionRing(id), Gizmo {
/// handle: assets.selection, .. }, Transform)` for newly selected ids,
/// despawn rings whose id left the selection, and every frame place each
/// ring at the unit entity's interpolated position (`Interp::sample` with
/// the overstep fraction) at [`RING_Y`], scaled by the kind's radius times
/// [`RING_RADIUS_PER_UNIT_RADIUS`]. Rings are never children of units.
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn sync_selection_rings(
    mut commands: Commands,
    selection: Res<Selection>,
    mut rings: ResMut<SelectionRings>,
    assets: Option<Res<RingAssets>>,
    entities: Res<UnitEntities>,
    interps: Query<&Interp>,
    mut ring_transforms: Query<&mut Transform, With<SelectionRing>>,
    fixed: Res<Time<Fixed>>,
    sim: NonSend<SimHandle>,
) {
    let Some(assets) = assets else {
        return;
    };
    rings.0.retain(|id, ring| {
        if selection.contains(*id) {
            true
        } else {
            commands.entity(*ring).despawn();
            false
        }
    });
    let t = fixed.overstep_fraction();
    let view = sim.view();
    for id in &selection.units {
        let Some(unit) = view.unit(*id) else {
            continue; // prune_dead drops it this frame
        };
        let Some(unit_entity) = entities.0.get(id) else {
            continue;
        };
        let Ok(interp) = interps.get(*unit_entity) else {
            continue;
        };
        let (pos, _) = interp.sample(t);
        let radius = fx_to_f32(unit.radius) * RING_RADIUS_PER_UNIT_RADIUS;
        let transform = Transform::from_translation(Vec3::new(pos.x, RING_Y, pos.z))
            .with_scale(Vec3::splat(radius));
        match rings.0.get(id) {
            Some(ring) => {
                if let Ok(mut ring_transform) = ring_transforms.get_mut(*ring) {
                    *ring_transform = transform;
                }
            }
            None => {
                let ring = commands
                    .spawn((
                        Name::new("Selection ring"),
                        SelectionRing(*id),
                        Gizmo {
                            handle: assets.selection.clone(),
                            line_config: ring_line_config(),
                            ..default()
                        },
                        transform,
                    ))
                    .id();
                rings.0.insert(*id, ring);
            }
        }
    }
}

/// Thin lines for rings and markers.
fn ring_line_config() -> GizmoLineConfig {
    GizmoLineConfig {
        width: RING_LINE_WIDTH_PX,
        ..default()
    }
}

/// Spawn a move marker ring at `at` (world) that expires after
/// [`MOVE_MARKER_SECS`]. Called by `orders.rs` on a successful ground order.
pub fn spawn_move_marker(commands: &mut Commands, assets: &RingAssets, at: Vec3, now_secs: f32) {
    commands.spawn((
        Name::new("Move marker"),
        MoveMarker {
            expires_at: now_secs + MOVE_MARKER_SECS,
        },
        Gizmo {
            handle: assets.marker.clone(),
            line_config: ring_line_config(),
            ..default()
        },
        Transform::from_translation(Vec3::new(at.x, RING_Y, at.z))
            .with_scale(Vec3::splat(MOVE_MARKER_RADIUS)),
    ));
}

/// `debug` log of the frame and wall-clock time at which the first
/// selection ring and the first move marker appeared: the first draw of a
/// new kind of entity is where a synchronous pipeline compile would show,
/// so a long frame in `frame_stats`' `worst_ms` line can be matched
/// against these (`RUST_LOG=info,game=debug`).
fn log_first_gizmo_spawns(
    rings: Query<(), Added<SelectionRing>>,
    markers: Query<(), Added<MoveMarker>>,
    frame: Res<bevy::diagnostic::FrameCount>,
    time: Res<Time<Real>>,
    mut seen: Local<(bool, bool)>,
) {
    if !seen.0 && !rings.is_empty() {
        seen.0 = true;
        debug!(
            "first selection ring spawned: frame {} at {:.2} s",
            frame.0,
            time.elapsed_secs_f64()
        );
    }
    if !seen.1 && !markers.is_empty() {
        seen.1 = true;
        debug!(
            "first move marker spawned: frame {} at {:.2} s",
            frame.0,
            time.elapsed_secs_f64()
        );
    }
}

/// Shrinks and despawns move markers past their expiry.
pub fn expire_move_markers(
    mut commands: Commands,
    time: Res<Time>,
    mut markers: Query<(Entity, &MoveMarker, &mut Transform)>,
) {
    let now = time.elapsed_secs();
    for (entity, marker, mut transform) in &mut markers {
        if now >= marker.expires_at {
            commands.entity(entity).despawn();
        } else {
            let left = ((marker.expires_at - now) / MOVE_MARKER_SECS).clamp(0.0, 1.0);
            transform.scale = Vec3::splat(MOVE_MARKER_RADIUS * (0.4 + 0.6 * left));
        }
    }
}

/// Tick at which the dev-only synthetic box selection runs: after the
/// scenario's spawn during tick 0 and the camera's jump to the units.
#[cfg(feature = "dev")]
pub const SYNTHETIC_SELECT_TICK: u32 = 5;

/// Dev builds, `--scenario units200_auto`: once per run, at
/// [`SYNTHETIC_SELECT_TICK`], select everything through the drag-box code
/// path ([`select_in_rect`]) with a rectangle covering the whole viewport
/// and print `selection: <n>` (`selection: FAIL expected <e> got <n>` when
/// the count differs from the local player's unit count).
#[cfg(feature = "dev")]
pub struct SyntheticBoxSelectPlugin;

#[cfg(feature = "dev")]
impl Plugin for SyntheticBoxSelectPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, synthetic_box_select.in_set(WorldInputSet));
    }
}

#[cfg(feature = "dev")]
fn synthetic_box_select(
    mut done: Local<bool>,
    sim: NonSend<SimHandle>,
    camera: Query<(&Camera, &GlobalTransform), With<RtsCamera>>,
    mut selection: ResMut<Selection>,
) {
    if *done || sim.tick() < SYNTHETIC_SELECT_TICK {
        return;
    }
    let Ok((cam, cam_tf)) = camera.single() else {
        return;
    };
    let Some(rect) = cam.logical_viewport_rect() else {
        return;
    };
    *done = true;
    let view = sim.view();
    let n = select_in_rect(rect, false, cam, cam_tf, &view, &mut selection);
    let expected = view.units().filter(|u| u.owner == LOCAL_PLAYER).count();
    if n == expected {
        println!("selection: {n}");
    } else {
        println!("selection: FAIL expected {expected} got {n}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[u32]) -> Vec<UnitId> {
        v.iter().copied().map(UnitId).collect()
    }

    #[test]
    fn selection_set_toggle_and_groups() {
        let mut s = Selection::default();
        assert!(s.is_empty());
        s.set(ids(&[3, 1, 2]));
        assert_eq!(s.units.iter().copied().collect::<Vec<_>>(), ids(&[1, 2, 3]));
        s.toggle(UnitId(2));
        assert!(!s.contains(UnitId(2)));
        s.toggle(UnitId(9));
        assert!(s.contains(UnitId(9)));
        s.assign_group(1);
        s.clear();
        assert!(s.is_empty());
        assert!(s.recall_group(1));
        assert_eq!(s.units.iter().copied().collect::<Vec<_>>(), ids(&[1, 3, 9]));
        assert!(!s.recall_group(2), "empty group leaves the selection alone");
        assert_eq!(s.units.len(), 3);
        assert!(!s.recall_group(0));
        s.assign_group(10);
        assert!(s.groups.iter().filter(|g| !g.is_empty()).count() == 1);
        s.prune(|u| u.0 != 9);
        assert!(!s.contains(UnitId(9)));
        assert!(!s.groups[0].contains(&UnitId(9)));
        assert_eq!(Selection::digit_of(KeyCode::Digit7), Some(7));
        assert_eq!(Selection::digit_of(KeyCode::KeyA), None);
    }

    #[test]
    fn period_cycles_idle_builders_by_id_and_skips_busy_ones() {
        struct NoAi;
        impl sim::AiController for NoAi {
            fn think(&mut self, _: sim::PlayerId, _: &SimView<'_>) -> Vec<sim::Command> {
                Vec::new()
            }
        }
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let rules = rules::Rules::load(&data).expect("rules load");
        let mut setup = sim::MatchSetup::scenario(&rules, 1);
        setup.start = sim::StartKind::Standard;
        let mut sim = sim::Sim::new(setup, rules, Box::new(NoAi));
        // The standard start's Yeomen are UnitId(1..=5), all idle at tick 0.
        let next = |sim: &sim::Sim, after: Option<u32>| {
            next_idle_builder(&sim.view(), sim.rules(), after.map(UnitId)).map(|u| u.0)
        };
        assert_eq!(next(&sim, None), Some(1));
        assert_eq!(next(&sim, Some(1)), Some(2));
        assert_eq!(next(&sim, Some(5)), Some(1), "wraps");
        // Order Yeoman 1 to move: it is skipped while the order runs.
        let to = sim.view().unit(UnitId(1)).unwrap().pos;
        let mv = sim::PlayerCommand {
            player: LOCAL_PLAYER,
            seq: 0,
            cmd: sim::Command::Move {
                units: vec![UnitId(1)],
                target: sim::FxVec2::new(to.x + sim::Fx::from_int(20), to.y),
                queue: false,
            },
        };
        sim.step(&[mv]);
        for _ in 0..4 {
            sim.step(&[]);
        }
        assert!(sim.view().unit(UnitId(1)).unwrap().order.is_some());
        assert_eq!(next(&sim, None), Some(2));
        assert_eq!(next(&sim, Some(5)), Some(2), "wraps past the busy one");
    }

    /// Combat removes units and buildings from the sim between two frames:
    /// the presenter despawns their entities and `prune_dead` drops them
    /// from the selection and the control groups, a selected building
    /// included, without a panic.
    #[test]
    fn units_and_buildings_destroyed_in_combat_leave_the_world_and_the_selection() {
        use crate::buildings::{BuildingEntities, BuildingsPlugin};
        use crate::present::PresentPlugin;
        use crate::sim_driver::{ScenarioInput, SimPlugin};
        use sim::{Command, FxVec2, PlayerCommand, PlayerId, TargetId, Tile, UnitKindId};
        use std::time::Duration;

        let rules =
            rules::Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
                .expect("rules load");
        let kind = |id: &str| {
            UnitKindId(u16::try_from(rules.units.iter().position(|u| u.id == id).unwrap()).unwrap())
        };
        let (tower, _) = rules.building_kind_by_id("watchtower").unwrap();
        let p1 = PlayerId(1);
        // P0's Watchtower is BuildingId(1) and its Yeomen UnitId(1..=2);
        // P1's six Mangonels UnitId(3..=8) destroy the tower (6 x 120
        // against 500 HP), then kill the Yeomen (30 a hit against 40 HP).
        // The Mangonels are spawned in the tick their first Attack is
        // issued: idle armed units engage enemies in reach on their own
        // (M4a), and the scene must still stand at tick 4.
        let stream: sim::scenarios::Stream = vec![
            (
                0,
                vec![
                    PlayerCommand::new(
                        LOCAL_PLAYER,
                        0,
                        Command::DebugPlaceBuilding {
                            owner: LOCAL_PLAYER,
                            kind: sim::BuildingKindId(tower),
                            tile: Tile::new(20, 30),
                            complete: true,
                        },
                    ),
                    PlayerCommand::new(
                        LOCAL_PLAYER,
                        1,
                        Command::DebugSpawn {
                            owner: LOCAL_PLAYER,
                            kind: kind("yeoman"),
                            at: FxVec2::from_ints(18, 30),
                            count: 2,
                        },
                    ),
                ],
            ),
            (
                4,
                vec![
                    PlayerCommand::new(
                        p1,
                        0,
                        Command::DebugSpawn {
                            owner: p1,
                            kind: kind("mangonel"),
                            at: FxVec2::from_ints(24, 33),
                            count: 6,
                        },
                    ),
                    PlayerCommand::new(
                        p1,
                        1,
                        Command::Attack {
                            units: (3..=8).map(UnitId).collect(),
                            target: TargetId::Building(BuildingId(1)),
                        },
                    ),
                ],
            ),
            (
                8,
                vec![
                    PlayerCommand::new(
                        p1,
                        2,
                        Command::Attack {
                            units: (3..=5).map(UnitId).collect(),
                            target: TargetId::Unit(UnitId(1)),
                        },
                    ),
                    PlayerCommand::new(
                        p1,
                        3,
                        Command::Attack {
                            units: (6..=8).map(UnitId).collect(),
                            target: TargetId::Unit(UnitId(2)),
                        },
                    ),
                ],
            ),
        ];
        let setup = sim::MatchSetup::scenario(&rules, 3);
        let handle = SimHandle::from_setup(setup, rules);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(Assets::<Mesh>::default())
            .insert_resource(Assets::<StandardMaterial>::default())
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                Duration::ZERO,
            ))
            .add_plugins((
                SimPlugin::new(handle, Box::new(ScenarioInput::new(stream))),
                PresentPlugin,
                BuildingsPlugin,
            ))
            .init_resource::<Selection>()
            .add_systems(PostUpdate, prune_dead);
        app.update();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_millis(50),
        ));
        let mut entities = None;
        for _ in 0..200 {
            app.update();
            let tick = app.world().non_send::<SimHandle>().tick();
            if tick == 4 {
                // Everything spawned and shown: select the tower, and the
                // Yeomen in group 1.
                let units = app.world().resource::<UnitEntities>().0.clone();
                let buildings = app.world().resource::<BuildingEntities>().0.clone();
                entities = Some((
                    units[&UnitId(1)],
                    units[&UnitId(2)],
                    buildings[&BuildingId(1)],
                ));
                let mut selection = app.world_mut().resource_mut::<Selection>();
                selection.set_building(BuildingId(1));
                selection.groups[0] = [UnitId(1), UnitId(2)].into_iter().collect();
            }
        }
        let (u1, u2, b1) = entities.expect("the scene was shown at tick 4");
        let view_has = |app: &App| {
            let sim = app.world().non_send::<SimHandle>();
            let view = sim.view();
            (
                view.building(BuildingId(1)).is_some(),
                view.unit(UnitId(1)).is_some() || view.unit(UnitId(2)).is_some(),
            )
        };
        assert_eq!(view_has(&app), (false, false), "destroyed and killed");
        assert!(
            !app.world()
                .resource::<BuildingEntities>()
                .0
                .contains_key(&BuildingId(1))
        );
        let units = &app.world().resource::<UnitEntities>().0;
        assert!(!units.contains_key(&UnitId(1)) && !units.contains_key(&UnitId(2)));
        for e in [u1, u2, b1] {
            assert!(app.world().get_entity(e).is_err(), "{e:?} despawned");
        }
        let selection = app.world().resource::<Selection>();
        assert_eq!(selection.building, None);
        assert!(selection.groups[0].is_empty());
    }

    #[test]
    fn drag_rect_distinguishes_clicks() {
        let click = DragRect {
            start: Vec2::new(10.0, 10.0),
            current: Vec2::new(12.0, 11.0),
        };
        assert!(click.is_click());
        let drag = DragRect {
            start: Vec2::new(100.0, 50.0),
            current: Vec2::new(20.0, 90.0),
        };
        assert!(!drag.is_click());
        let r = drag.rect();
        assert_eq!(r.min, Vec2::new(20.0, 50.0));
        assert_eq!(r.max, Vec2::new(100.0, 90.0));
    }
}
