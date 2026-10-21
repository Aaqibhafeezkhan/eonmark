//! Build placement (M3b): while [`OrderMode::Place`] holds a building kind,
//! a translucent ghost of its footprint follows the cursor, green where the
//! sim would accept the `Build` and red where it would not, and a left
//! click on the ground issues the `Build`.
//!
//! The verdict is `SimView::check_build`, the same pure function the sim
//! runs when the command arrives, so the ghost never disagrees with the
//! sim. It is evaluated only when the tile, the kind, the builder or the
//! tick changes, because the check includes a reachability search. While
//! the ghost is red the message line shows the reason ("Outside your
//! borders"); a click on a red ghost sends nothing. The builder is the
//! selected unit of a builder kind nearest the site.

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use rules::Rules;
use sim::{BuildingKindId, Command, FxVec2, RejectReason, SimView, Tile, UnitId};

use crate::buildings::building_height;
use crate::camera::RtsCamera;
use crate::economy_hud::{MessageState, UiStrings, reject_text};
use crate::ground::HALF_EXTENT;
use crate::hud::{PointerOverUi, WorldInputSet};
use crate::orders::{OrderMode, ground_hit, order_mode_keys};
use crate::palette;
use crate::present::sim_from_world;
use crate::selection::Selection;
use crate::sim_driver::{LOCAL_PLAYER, PendingCommands, SimHandle};

/// Alpha of the placement ghost.
pub const GHOST_ALPHA: f32 = 0.5;
/// Height of the ghost as a fraction of the complete building's.
pub const GHOST_HEIGHT_FRACTION: f32 = 0.6;
/// Height of the ghost's ground outline, metres (above the grid gizmo).
pub const GHOST_OUTLINE_Y: f32 = 0.04;

/// The ghost entity.
#[derive(Component, Debug, Clone, Copy)]
pub struct PlacementGhost;

/// The ghost's two materials and the last verdict, keyed by what it was
/// computed for.
#[derive(Resource, Debug)]
pub struct Ghost {
    valid: Handle<StandardMaterial>,
    invalid: Handle<StandardMaterial>,
    /// `(kind, nw tile, builder, tick)` and the verdict for them.
    last: Option<(PlacementKey, Result<Vec<i64>, RejectReason>)>,
}

/// What a placement verdict depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacementKey {
    /// Building kind.
    pub kind: BuildingKindId,
    /// North-west footprint tile.
    pub tile: Tile,
    /// Builder, if any selected unit can build.
    pub builder: Option<UnitId>,
    /// Sim tick the verdict was computed at.
    pub tick: u32,
}

/// The ghost, the verdict and the placing click.
pub struct PlacementPlugin;

impl Plugin for PlacementPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_ghost).add_systems(
            Update,
            update_placement
                .after(order_mode_keys)
                .in_set(WorldInputSet),
        );
    }
}

fn ghost_material(color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: color.with_alpha(GHOST_ALPHA),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    }
}

fn spawn_ghost(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let valid = materials.add(ghost_material(palette::VALID));
    let invalid = materials.add(ghost_material(palette::INVALID));
    let mut cube = Mesh::from(Cuboid::new(1.0, 1.0, 1.0));
    cube.translate_by(Vec3::Y * 0.5);
    commands.spawn((
        Name::new("Placement ghost"),
        PlacementGhost,
        Mesh3d(meshes.add(cube)),
        MeshMaterial3d(valid.clone()),
        Transform::IDENTITY,
        Visibility::Hidden,
        NotShadowCaster,
    ));
    commands.insert_resource(Ghost {
        valid,
        invalid,
        last: None,
    });
}

/// The north-west tile of a `w` x `h` footprint centred under `cursor`,
/// kept on a map of `map_w` x `map_h` tiles.
pub fn footprint_nw(cursor: Tile, w: u32, h: u32, map_w: u16, map_h: u16) -> Tile {
    let half = |size: u32| u16::try_from(size.saturating_sub(1) / 2).unwrap_or(0);
    let fit = |c: u16, size: u32, max: u16| -> u16 {
        let size = u16::try_from(size).unwrap_or(max);
        c.saturating_sub(half(u32::from(size)))
            .min(max.saturating_sub(size))
    };
    Tile::new(fit(cursor.x, w, map_w), fit(cursor.y, h, map_h))
}

/// The selected local unit of a builder kind nearest the footprint centre,
/// preferring one that is not already building (the sim keeps one builder
/// per site, so taking a busy one would leave its site idle), ties by id.
pub fn nearest_builder(
    view: &SimView<'_>,
    rules: &Rules,
    selection: &Selection,
    centre: FxVec2,
) -> Option<UnitId> {
    selection
        .units
        .iter()
        .filter_map(|id| view.unit(*id))
        .filter(|u| u.owner == LOCAL_PLAYER && rules.unit_kind(u.kind.0).is_some_and(|k| k.builder))
        .min_by_key(|u| {
            let busy = matches!(u.job, Some(sim::Job::Build { .. }));
            (busy, u.pos.dist_sq_i64(centre), u.id)
        })
        .map(|u| u.id)
}

/// In `Place` mode: move the ghost to the footprint under the cursor and
/// colour it by `check_build`; on a left click (not over the HUD) issue the
/// `Build` when it would be accepted, and leave the mode unless Shift is
/// held. Outside `Place` mode, or over the HUD, the ghost is hidden.
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn update_placement(
    sim: NonSend<SimHandle>,
    mut mode: ResMut<OrderMode>,
    selection: Res<Selection>,
    over_ui: Res<PointerOverUi>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<RtsCamera>>,
    ghost: Option<ResMut<Ghost>>,
    mut ghosts: Query<
        (
            &mut Transform,
            &mut Visibility,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        With<PlacementGhost>,
    >,
    mut pending: ResMut<PendingCommands>,
    strings: Option<Res<UiStrings>>,
    mut message: ResMut<MessageState>,
    time: Res<Time>,
    mut gizmos: Gizmos,
) {
    let Some(mut ghost) = ghost else {
        return;
    };
    let Ok((mut transform, mut visibility, mut material)) = ghosts.single_mut() else {
        return;
    };
    let OrderMode::Place(kind) = *mode else {
        *visibility = Visibility::Hidden;
        ghost.last = None;
        return;
    };
    let rules = sim.rules();
    let view = sim.view();
    let Some(def) = rules.building_kind(kind.0) else {
        *mode = OrderMode::Normal;
        return;
    };
    let cursor = windows.single().ok().and_then(Window::cursor_position);
    let hit = cursor.and_then(|c| {
        let (cam, cam_tf) = camera.single().ok()?;
        ground_hit(cam, cam_tf, c)
    });
    let Some(hit) = hit.filter(|_| !over_ui.0) else {
        *visibility = Visibility::Hidden;
        return;
    };
    let map = view.map();
    let under = map.tile_of(sim_from_world(hit));
    let nw = footprint_nw(
        under,
        def.width_tiles,
        def.height_tiles,
        map.width(),
        map.height(),
    );
    let centre = sim::construction::centre_fx(def, nw);
    let builder = nearest_builder(&view, rules, &selection, centre);
    let key = PlacementKey {
        kind,
        tile: nw,
        builder,
        tick: view.tick,
    };
    if ghost.last.as_ref().is_none_or(|(k, _)| *k != key) {
        let verdict = match builder {
            Some(unit) => view.check_build(LOCAL_PLAYER, unit, kind, nw),
            None => Err(RejectReason::NoValidUnits),
        };
        ghost.last = Some((key, verdict));
    }
    let verdict = ghost
        .last
        .as_ref()
        .map(|(_, v)| v.clone())
        .expect("just set");

    let (w, h) = (def.width_tiles as f32, def.height_tiles as f32);
    let height = building_height(def.width_tiles, def.height_tiles) * GHOST_HEIGHT_FRACTION;
    *transform = Transform::from_translation(Vec3::new(
        f32::from(nw.x) + w * 0.5 - HALF_EXTENT,
        0.0,
        f32::from(nw.y) + h * 0.5 - HALF_EXTENT,
    ))
    .with_scale(Vec3::new(w, height, h));
    *visibility = Visibility::Visible;
    // The footprint outlined on the ground too, so a red ghost reads even
    // where it stands inside a taller building.
    gizmos.rect(
        Isometry3d::new(
            Vec3::new(
                transform.translation.x,
                GHOST_OUTLINE_Y,
                transform.translation.z,
            ),
            crate::present::flat_on_ground(),
        ),
        Vec2::new(w, h),
        if verdict.is_ok() {
            palette::VALID
        } else {
            palette::INVALID
        },
    );
    let wanted = if verdict.is_ok() {
        &ghost.valid
    } else {
        &ghost.invalid
    };
    if material.0 != *wanted {
        material.0 = wanted.clone();
    }
    if let (Err(reason), Some(strings)) = (&verdict, strings.as_deref()) {
        let text = reject_text(strings, rules, reason);
        if message.text != text {
            message.text = text;
        }
        message.since = time.elapsed_secs_f64();
    }

    if !buttons.just_released(MouseButton::Left) {
        return;
    }
    if let (Ok(_), Some(unit)) = (&verdict, builder) {
        let seq = pending.push_local(Command::Build {
            unit,
            kind,
            tile: nw,
        });
        info!("Build {} at ({}, {}) seq {seq}", def.name, nw.x, nw.y);
        let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        if !shift {
            *mode = OrderMode::Normal;
        }
        ghost.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_footprint_is_centred_under_the_cursor_and_kept_on_the_map() {
        assert_eq!(
            footprint_nw(Tile::new(10, 10), 3, 3, 128, 128),
            Tile::new(9, 9)
        );
        assert_eq!(
            footprint_nw(Tile::new(10, 10), 2, 2, 128, 128),
            Tile::new(10, 10)
        );
        assert_eq!(
            footprint_nw(Tile::new(10, 10), 1, 1, 128, 128),
            Tile::new(10, 10)
        );
        assert_eq!(
            footprint_nw(Tile::new(0, 0), 3, 3, 128, 128),
            Tile::new(0, 0)
        );
        assert_eq!(
            footprint_nw(Tile::new(127, 127), 3, 3, 128, 128),
            Tile::new(125, 125)
        );
    }
}
