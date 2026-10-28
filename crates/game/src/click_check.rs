//! `--click-check` (art pass, acceptance AP10): every unit kind of both
//! teams must be selectable by a real click on its model, because picking
//! ray-casts the merged model mesh (rule 16) and a thin model could slip
//! between the pixels a player clicks.
//!
//! Run on `--scenario unit_looks` (one unit of every kind per team, nothing
//! moves, framed at 40 m). Once the sim reaches [`CLICK_CHECK_TICK`] the
//! game prints one line per unit, in id order ([`target_line`]):
//!
//! ```text
//! click_check: target <i> unit=<id> kind=<kind id> team=<slot> px=<x>,<y>
//! ```
//!
//! `px` is where a player aims: the logical window pixel at the centroid
//! of the unit's mesh vertices as projected through the RTS camera (the
//! body, where most of a figure's vertices are), moved to the nearest whole
//! pixel within [`AIM_SEARCH_PX`] whose ray hits the unit before anything
//! else pickable ([`aim_pixel`]; `moved=<px>` says how far, `moved=none`
//! that no pixel near the centroid shows the unit). That keeps a target
//! off the gaps of a silhouette (between a Skirmisher's body and the axes
//! held out in front of it, where the centre of its bounds falls) without
//! leaving the unit, so the check tests the real input path: X events,
//! winit, Bevy's mesh picking of the merged model (rule 16) and the
//! selection. A driver clicks each target in order with real X input (xdotool under
//! Xvfb, the `scripts/m4b_outpost_drive.sh` precedent; synthetic
//! `PointerInput` fails on lavapipe, issue #33). After every primary
//! release, once the selection systems have run, the game compares the
//! selection with the next target ([`judge`]): exactly that unit prints
//! `click_check: selected <i> ...`, anything else `click_check: miss <i>
//! ... got=<ids>`. After the last target it prints [`verdict_line`],
//! `click_check: ok <n>/<n>`, or `click_check: FAIL ...` naming every
//! kind and team missed, and exits 0 or 1.

use bevy::mesh::VertexAttributeValues;
use bevy::picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings};
use bevy::prelude::*;
use sim::UnitId;

use crate::camera::RtsCamera;
use crate::hud::WorldInputSet;
use crate::present::UnitEntities;
use crate::selection::Selection;
use crate::sim_driver::SimHandle;

/// The sim tick at which the targets are printed: the lineup has stood
/// still for three seconds and every model has long settled.
pub const CLICK_CHECK_TICK: u32 = 60;

/// How far from the centroid pixel [`aim_pixel`] looks for a pixel that
/// shows the unit, logical pixels (a 40 m unit is about 20 px tall).
pub const AIM_SEARCH_PX: i32 = 8;

/// One unit to click.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// The unit.
    pub unit: UnitId,
    /// Its kind's id (`units.ron`).
    pub kind: String,
    /// Its owner's slot.
    pub team: u8,
    /// The logical window pixel to click.
    pub px: Vec2,
    /// How far [`aim_pixel`] moved it off the centroid, whole pixels;
    /// `None` when no pixel within [`AIM_SEARCH_PX`] shows the unit.
    pub moved: Option<u32>,
}

/// State of the check: the printed targets, the next one to be clicked and
/// the ones missed.
#[derive(Resource, Default, Debug)]
pub struct ClickCheck {
    /// Every target, in unit id order; empty until printed.
    pub targets: Vec<Target>,
    /// Index of the target the next click is judged against.
    pub next: usize,
    /// Indices of the targets a click missed.
    pub missed: Vec<usize>,
    /// The verdict has been printed.
    pub done: bool,
}

/// What one primary click did to the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Judged {
    /// The selection is exactly the target unit.
    Selected,
    /// Anything else (nothing, another unit, several).
    Missed,
}

/// `click_check: target <i> unit=<id> kind=<kind> team=<t> px=<x>,<y>
/// moved=<d|none>`, with the pixel rounded to whole logical pixels.
pub fn target_line(i: usize, t: &Target) -> String {
    format!(
        "click_check: target {i} unit={} kind={} team={} px={},{} moved={}",
        t.unit.0,
        t.kind,
        t.team,
        t.px.x.round() as i64,
        t.px.y.round() as i64,
        t.moved.map_or("none".to_string(), |d| d.to_string())
    )
}

/// The whole pixels around `centre` in the order [`aim_pixel`] tries
/// them: `centre` rounded first, then rings of growing Chebyshev radius up
/// to `radius`, each ring sorted by distance from the centre (then row,
/// then column, so the order is fixed).
pub fn aim_candidates(centre: Vec2, radius: i32) -> Vec<IVec2> {
    let c = IVec2::new(centre.x.round() as i32, centre.y.round() as i32);
    let mut out = vec![c];
    for r in 1..=radius {
        let mut ring: Vec<IVec2> = (-r..=r)
            .flat_map(|dy| (-r..=r).map(move |dx| IVec2::new(dx, dy)))
            .filter(|d| d.x.abs() == r || d.y.abs() == r)
            .collect();
        ring.sort_by_key(|d| (d.x * d.x + d.y * d.y, d.y, d.x));
        out.extend(ring.into_iter().map(|d| c + d));
    }
    out
}

/// Where to click `unit`: the first of [`aim_candidates`] around
/// `centroid` whose ray, cast from the camera through it, hits `unit`
/// before any other entity `pickable` accepts; with how far that is from
/// the centroid. `None` when none within [`AIM_SEARCH_PX`] does.
pub fn aim_pixel(
    centroid: Vec2,
    unit: Entity,
    pickable: &dyn Fn(Entity) -> bool,
    cam: &Camera,
    cam_tf: &GlobalTransform,
    ray_cast: &mut MeshRayCast,
) -> Option<(Vec2, u32)> {
    let mut settings = MeshRayCastSettings::default().always_early_exit();
    settings.filter = pickable;
    let c = IVec2::new(centroid.x.round() as i32, centroid.y.round() as i32);
    aim_candidates(centroid, AIM_SEARCH_PX)
        .into_iter()
        .find(|p| {
            cam.viewport_to_world(cam_tf, p.as_vec2()).is_ok_and(|ray| {
                ray_cast.cast_ray(ray, &settings).first().map(|h| h.0) == Some(unit)
            })
        })
        .map(|p| {
            let d = (p - c).as_vec2().length().round() as u32;
            (p.as_vec2(), d)
        })
}

/// The centroid of `mesh`'s vertices placed by `tf` and projected through
/// the camera, logical pixels; `None` for a mesh without float3 positions
/// or with none in front of the camera.
pub fn projected_centroid(
    mesh: &Mesh,
    tf: &GlobalTransform,
    cam: &Camera,
    cam_tf: &GlobalTransform,
) -> Option<Vec2> {
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return None;
    };
    let (sum, n) = positions
        .iter()
        .filter_map(|p| {
            cam.world_to_viewport(cam_tf, tf.transform_point(Vec3::from(*p)))
                .ok()
        })
        .fold((Vec2::ZERO, 0u32), |(s, n), p| (s + p, n + 1));
    (n > 0).then(|| sum / n as f32)
}

/// Compare `selection` with target `t`.
pub fn judge(selection: &Selection, t: &Target) -> Judged {
    if selection.building.is_none()
        && selection.units.len() == 1
        && selection.units.contains(&t.unit)
    {
        Judged::Selected
    } else {
        Judged::Missed
    }
}

/// `click_check: ok <n>/<n>` when nothing was missed, else
/// `click_check: FAIL <hits>/<n> selected; missed <kind> team <t>, ...`
/// (the `hud::verdict_line` form).
pub fn verdict_line(targets: &[Target], missed: &[usize]) -> String {
    let n = targets.len();
    if missed.is_empty() {
        return format!("click_check: ok {n}/{n}");
    }
    let names: Vec<String> = missed
        .iter()
        .filter_map(|&i| targets.get(i))
        .map(|t| format!("{} team {}", t.kind, t.team))
        .collect();
    format!(
        "click_check: FAIL {}/{n} selected; missed {}",
        n - missed.len(),
        names.join(", ")
    )
}

/// Prints the targets and judges the clicks. Added by `app::run` under
/// `--click-check`.
pub struct ClickCheckPlugin;

impl Plugin for ClickCheckPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClickCheck>().add_systems(
            Update,
            (print_targets, judge_clicks.after(WorldInputSet)).chain(),
        );
    }
}

/// Once the sim has reached [`CLICK_CHECK_TICK`]: aim at every unit
/// ([`projected_centroid`], [`aim_pixel`] against the pickable entities)
/// and print the targets.
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn print_targets(
    sim: NonSend<SimHandle>,
    mut check: ResMut<ClickCheck>,
    entities: Res<UnitEntities>,
    units: Query<(&GlobalTransform, &Mesh3d)>,
    pickable: Query<(), With<Pickable>>,
    meshes: Res<Assets<Mesh>>,
    camera: Query<(&Camera, &GlobalTransform), With<RtsCamera>>,
    mut ray_cast: MeshRayCast,
) {
    if !check.targets.is_empty() || check.done || sim.tick() < CLICK_CHECK_TICK {
        return;
    }
    let Ok((cam, cam_tf)) = camera.single() else {
        return;
    };
    let view = sim.view();
    let rules = sim.rules();
    let is_pickable = |e: Entity| pickable.contains(e);
    let mut targets = Vec::new();
    for u in view.units() {
        let (Some(entity), Some(kind)) = (entities.0.get(&u.id), rules.unit_kind(u.kind.0)) else {
            continue;
        };
        let Ok((tf, mesh)) = units.get(*entity) else {
            continue;
        };
        let Some(centroid) = meshes
            .get(&mesh.0)
            .and_then(|m| projected_centroid(m, tf, cam, cam_tf))
        else {
            continue;
        };
        let aimed = aim_pixel(centroid, *entity, &is_pickable, cam, cam_tf, &mut ray_cast);
        targets.push(Target {
            unit: u.id,
            kind: kind.id.clone(),
            team: u.owner.0,
            px: aimed.map_or(centroid, |(p, _)| p),
            moved: aimed.map(|(_, d)| d),
        });
    }
    if targets.is_empty() {
        return;
    }
    for (i, t) in targets.iter().enumerate() {
        println!("{}", target_line(i, t));
    }
    check.targets = targets;
}

/// After the selection systems: on every primary release once the targets
/// are out, judge the selection against the next target and print the
/// result; after the last, print the verdict and exit (1 on a miss).
pub fn judge_clicks(
    buttons: Res<ButtonInput<MouseButton>>,
    selection: Res<Selection>,
    mut check: ResMut<ClickCheck>,
    mut exit: MessageWriter<AppExit>,
) {
    if check.done || check.targets.is_empty() || !buttons.just_released(MouseButton::Left) {
        return;
    }
    let i = check.next;
    let Some(t) = check.targets.get(i).cloned() else {
        return;
    };
    let what = format!("{i} unit={} kind={} team={}", t.unit.0, t.kind, t.team);
    match judge(&selection, &t) {
        Judged::Selected => println!("click_check: selected {what}"),
        Judged::Missed => {
            let got: Vec<String> = selection.units.iter().map(|u| u.0.to_string()).collect();
            println!("click_check: miss {what} got=[{}]", got.join(","));
            check.missed.push(i);
        }
    }
    check.next += 1;
    if check.next == check.targets.len() {
        check.done = true;
        let line = verdict_line(&check.targets, &check.missed);
        println!("{line}");
        exit.write(if check.missed.is_empty() {
            AppExit::Success
        } else {
            AppExit::error()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(id: u32, kind: &str, team: u8) -> Target {
        Target {
            unit: UnitId(id),
            kind: kind.into(),
            team,
            px: Vec2::new(100.4, 200.6),
            moved: Some(3),
        }
    }

    #[test]
    fn lines_and_verdicts_are_exact() {
        let a = target(1, "yeoman", 0);
        let b = target(9, "yeoman", 1);
        assert_eq!(
            target_line(0, &a),
            "click_check: target 0 unit=1 kind=yeoman team=0 px=100,201 moved=3"
        );
        let unseen = Target {
            moved: None,
            ..a.clone()
        };
        assert!(target_line(0, &unseen).ends_with(" moved=none"));
        let both = [a, b];
        assert_eq!(verdict_line(&both, &[]), "click_check: ok 2/2");
        assert_eq!(
            verdict_line(&both, &[1]),
            "click_check: FAIL 1/2 selected; missed yeoman team 1"
        );
    }

    /// The centre first, then every pixel of each ring outward, nearest
    /// first, each once.
    #[test]
    fn aim_candidates_spiral_out_from_the_centre() {
        let c = aim_candidates(Vec2::new(10.4, 19.6), 2);
        assert_eq!(c[0], IVec2::new(10, 20));
        assert_eq!(c.len(), 25, "a 5x5 square");
        let unique: std::collections::BTreeSet<(i32, i32)> = c.iter().map(|p| (p.x, p.y)).collect();
        assert_eq!(unique.len(), 25);
        // The four edge neighbours come before the diagonals, the first
        // ring before the second.
        let d2 = |p: &IVec2| (p.x - 10).pow(2) + (p.y - 20).pow(2);
        assert!(c[1..5].iter().all(|p| d2(p) == 1), "{c:?}");
        assert!(c[5..9].iter().all(|p| d2(p) == 2), "{c:?}");
        assert!(c[9..].iter().all(|p| d2(p) >= 4), "{c:?}");
    }

    #[test]
    fn only_the_target_alone_counts_as_selected() {
        let t = target(3, "skirmisher", 0);
        let mut s = Selection::default();
        assert_eq!(judge(&s, &t), Judged::Missed, "nothing selected");
        s.set([UnitId(3)]);
        assert_eq!(judge(&s, &t), Judged::Selected);
        s.set([UnitId(3), UnitId(4)]);
        assert_eq!(judge(&s, &t), Judged::Missed, "not alone");
        s.set([UnitId(4)]);
        assert_eq!(judge(&s, &t), Judged::Missed, "another unit");
    }

    /// The judge runs on primary releases only, in target order, and the
    /// last one prints the verdict and exits 1 when a click missed.
    #[test]
    fn clicks_are_judged_in_order_and_a_miss_exits_1() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<Selection>()
            .add_message::<AppExit>()
            .insert_resource(ClickCheck {
                targets: vec![target(1, "yeoman", 0), target(9, "yeoman", 1)],
                ..ClickCheck::default()
            })
            .add_systems(Update, judge_clicks);
        let click = |app: &mut App, selected: &[u32]| {
            app.world_mut()
                .resource_mut::<Selection>()
                .set(selected.iter().map(|&u| UnitId(u)));
            let mut buttons = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
            buttons.press(MouseButton::Left);
            buttons.release(MouseButton::Left);
            app.update();
            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .clear();
        };
        app.update();
        assert_eq!(app.world().resource::<ClickCheck>().next, 0, "no click yet");
        click(&mut app, &[1]);
        assert_eq!(app.world().resource::<ClickCheck>().next, 1);
        assert!(app.world().resource::<ClickCheck>().missed.is_empty());
        click(&mut app, &[]);
        let check = app.world().resource::<ClickCheck>();
        assert!(check.done);
        assert_eq!(check.missed, vec![1]);
        let exits: Vec<AppExit> = app
            .world_mut()
            .resource_mut::<Messages<AppExit>>()
            .drain()
            .collect();
        assert_eq!(exits, vec![AppExit::error()]);
    }
}
