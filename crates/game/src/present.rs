//! Presentation of the sim (M2 decision 4): one entity per `UnitId`,
//! spawned and despawned by diffing the `SimView` after every tick, with a
//! per-unit [`Interp`] of the previous and current tick positions that the
//! render transform lerps by `Time<Fixed>::overstep_fraction()`. Yaw is
//! derived render-side from the unit's facing vector with `atan2` in f32.
//! Meshes come from `data/visuals.ron` (`rules::Visual`), one shared mesh
//! per kind and one material per `(kind, team)` so 200 units are a handful
//! of draw calls. Every look is built low-poly and flat-shaded by
//! [`body_mesh`] (M4b, `docs/ART_STYLE.md`), and [`look_top_y`] gives its
//! height analytically for whatever is drawn above a unit.
//!
//! Coordinates: one tile is one metre; sim `(x, y)` maps to world
//! `(x - HALF_EXTENT, y_up, y - HALF_EXTENT)` so the 128 x 128 ground mesh
//! built by `ground.rs` is centred on the origin. [`world_from_sim`] and
//! [`sim_from_world`] are the only two conversion points; orders and
//! selection use them too.
//!
//! When the local player's first units appear, the camera focus jumps once
//! to their centroid ([`focus_camera_on_first_units`]) so a scenario that
//! spawns at the west start is on screen from the first frame.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;
use bevy::transform::TransformSystems;
use rules::{Primitive, Rules, Visual};
use sim::{Fx, FxVec2, UnitId};

use crate::camera::RtsCamera;
use crate::ground::HALF_EXTENT;
use crate::palette;
use crate::sim_driver::{LOCAL_PLAYER, SimHandle, SimSystems, sim_stepped};

/// Height of a unit's pivot above the ground, in metres: capsules stand on
/// the ground, so the pivot is half the total height.
pub const UNIT_Y: f32 = 0.0;

/// Capsule length (cylinder part) as a multiple of the unit's radius, for
/// `Primitive(Capsule)` (a `Body` capsule carries its own length).
pub const CAPSULE_LENGTH_PER_RADIUS: f32 = 1.4;

/// Facets around a round look: the capsule's longitudes and the
/// cylinder's and cone's sides. Low enough to read as flat-shaded low poly
/// (`docs/ART_STYLE.md`), high enough that the ring of vertices includes
/// both ends of the x axis, so a lying capsule touches the ground.
pub const LOOK_SIDES: u32 = 10;

/// Capsule latitudes (the two half-spheres together). Bevy 0.19.1's
/// builder needs at least 4 (it computes `latitudes / 2 - 2`).
pub const CAPSULE_LATITUDES: u32 = 6;

/// Icosphere subdivisions of `Primitive(Sphere)`: 80 faces.
pub const SPHERE_SUBDIVISIONS: u32 = 1;

/// Vertex budget per look. Flat shading gives every triangle its own three
/// vertices, so this is three times the triangle budget.
pub const MAX_LOOK_VERTICES: usize = 600;

/// What [`body_mesh`] draws for a `Visual::Scene` until M7 loads glTF, and
/// for a kind with no visual.
pub const FALLBACK_VISUAL: Visual = Visual::Primitive(Primitive::Capsule);

/// `StandardMaterial::perceptual_roughness` for every unit (`docs/ART_STYLE.md`: matte).
pub const MATTE_ROUGHNESS: f32 = 0.95;
/// `StandardMaterial::reflectance` for every unit (`docs/ART_STYLE.md`).
pub const MATTE_REFLECTANCE: f32 = 0.2;

/// Marks a unit entity and names the sim unit it mirrors. Not `Reflect`:
/// `sim::UnitId` is engine-free and derives nothing from Bevy; the inspector
/// shows the id through `Name` instead.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnitRef(pub UnitId);

/// Previous and current tick poses of one unit, in world space. Updated by
/// [`sync_lifecycle`] after every tick; read by [`sync_transforms`] every
/// frame. On spawn `prev == curr` so a new unit does not slide in.
#[derive(Component, Debug, Clone, Copy, PartialEq, Reflect)]
#[reflect(Component)]
pub struct Interp {
    /// World position at the previous tick.
    pub prev: Vec3,
    /// World position at the current tick.
    pub curr: Vec3,
    /// Yaw (radians about +Y) at the previous tick.
    pub prev_yaw: f32,
    /// Yaw at the current tick.
    pub curr_yaw: f32,
}

impl Interp {
    /// Both poses at `pos`/`yaw` (a fresh spawn).
    pub fn at(pos: Vec3, yaw: f32) -> Self {
        Self {
            prev: pos,
            curr: pos,
            prev_yaw: yaw,
            curr_yaw: yaw,
        }
    }

    /// Shift `curr` into `prev` and set the new current pose.
    pub fn advance(&mut self, pos: Vec3, yaw: f32) {
        self.prev = self.curr;
        self.prev_yaw = self.curr_yaw;
        self.curr = pos;
        self.curr_yaw = yaw;
    }

    /// Interpolated position and yaw at `t` in `0..=1` (the overstep
    /// fraction). Yaw takes the short way round.
    pub fn sample(&self, t: f32) -> (Vec3, f32) {
        let t = t.clamp(0.0, 1.0);
        let pos = self.prev.lerp(self.curr, t);
        let mut dyaw = self.curr_yaw - self.prev_yaw;
        if dyaw > std::f32::consts::PI {
            dyaw -= std::f32::consts::TAU;
        } else if dyaw < -std::f32::consts::PI {
            dyaw += std::f32::consts::TAU;
        }
        (pos, self.prev_yaw + dyaw * t)
    }
}

/// The render-side mirror: which entity shows which sim unit. Rings, bars
/// and markers are keyed by `UnitId` elsewhere; this is the only map from
/// id to the unit's own entity.
#[derive(Resource, Default, Debug)]
pub struct UnitEntities(pub HashMap<UnitId, Entity>);

/// Shared mesh per unit kind and material per `(kind, team)`, created on
/// first use so draw calls stay low (one batch per combination), and the
/// height of each kind's look ([`look_top_y`]), cached with its mesh.
#[derive(Resource, Default)]
pub struct UnitVisuals {
    meshes: HashMap<u16, Handle<Mesh>>,
    top_y: HashMap<u16, f32>,
    materials: HashMap<(u16, u8), Handle<StandardMaterial>>,
}

impl UnitVisuals {
    /// The mesh for `kind`, built by [`body_mesh`] from its `Visual` and
    /// `radius_tiles_x100`. A `Visual::Scene` falls back to
    /// [`FALLBACK_VISUAL`] until M7. Also caches the look's height for
    /// [`UnitVisuals::top_y`].
    pub fn mesh(&mut self, kind: u16, rules: &Rules, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        if let Some(mesh) = self.meshes.get(&kind) {
            return mesh.clone();
        }
        let radius = rules
            .unit_kind(kind)
            .map_or(0.35, |k| k.radius_tiles_x100 as f32 / 100.0);
        let visual = rules.visual(kind).unwrap_or(&FALLBACK_VISUAL);
        self.top_y.insert(kind, look_top_y(visual, radius));
        let mesh = meshes.add(body_mesh(visual, radius));
        self.meshes.insert(kind, mesh.clone());
        mesh
    }

    /// Height above the ground of the top of `kind`'s look, in metres, once
    /// [`UnitVisuals::mesh`] has built it (every kind on screen has). The
    /// one place anything drawn above a unit takes its height from, so the
    /// art pass changes only [`look_top_y`].
    pub fn top_y(&self, kind: u16) -> Option<f32> {
        self.top_y.get(&kind).copied()
    }

    /// The matte team-coloured material for `(kind, team)`.
    pub fn material(
        &mut self,
        kind: u16,
        team: u8,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        self.materials
            .entry((kind, team))
            .or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: palette::team_color(team),
                    perceptual_roughness: MATTE_ROUGHNESS,
                    reflectance: MATTE_REFLECTANCE,
                    metallic: 0.0,
                    ..default()
                })
            })
            .clone()
    }

    /// Number of distinct meshes and materials created so far (dev panel
    /// readout, so only the `dev` build reads it; the unit test covers it).
    #[cfg_attr(not(feature = "dev"), allow(dead_code))]
    pub fn counts(&self) -> (usize, usize) {
        (self.meshes.len(), self.materials.len())
    }
}

/// Fixed-point to f32: exact for the magnitudes a 128-tile map produces.
pub fn fx_to_f32(v: Fx) -> f32 {
    v.0.to_num::<f32>()
}

/// f32 to fixed-point (rounding to the nearest representable value).
pub fn fx_from_f32(v: f32) -> Fx {
    Fx(sim::fx::Raw::from_num(v))
}

/// Sim position (tiles, y along the map's rows) to world metres with the
/// ground centred on the origin and `y_up` as height.
pub fn world_from_sim(pos: FxVec2, y_up: f32) -> Vec3 {
    Vec3::new(
        fx_to_f32(pos.x) - HALF_EXTENT,
        y_up,
        fx_to_f32(pos.y) - HALF_EXTENT,
    )
}

/// World metres back to a sim position (height ignored). The inverse of
/// [`world_from_sim`]; clamped to the map so a click past the edge still
/// resolves to a tile.
pub fn sim_from_world(p: Vec3) -> FxVec2 {
    let max = HALF_EXTENT * 2.0 - 0.001;
    FxVec2::new(
        fx_from_f32((p.x + HALF_EXTENT).clamp(0.0, max)),
        fx_from_f32((p.z + HALF_EXTENT).clamp(0.0, max)),
    )
}

/// Yaw about +Y so that a mesh facing +X at rest points along `facing`
/// (sim x maps to world x, sim y to world z): `atan2(-z, x)`.
pub fn yaw_from_facing(facing: FxVec2) -> f32 {
    let x = fx_to_f32(facing.x);
    let z = fx_to_f32(facing.y);
    if x == 0.0 && z == 0.0 {
        0.0
    } else {
        (-z).atan2(x)
    }
}

/// A unit's look resolved to one primitive shape and its size in metres:
/// what [`body_mesh`] builds and [`look_top_y`] measures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// The shape.
    pub shape: Primitive,
    /// Half the width (the kind's collision radius).
    pub radius: f32,
    /// Extent along the shape's own axis: the height when upright, the
    /// length along the unit's facing when `lying`. A sphere's is its
    /// diameter.
    pub length: f32,
    /// Lying on its side, long axis along the facing (+X at yaw 0).
    pub lying: bool,
}

impl Look {
    /// The look `visual` gives a unit of collision radius `radius`:
    /// `Primitive` sizes the shape from the radius (a capsule 3.4 radii
    /// tall, everything else 2), `Body` takes its own height, `Scene` is
    /// [`FALLBACK_VISUAL`] until M7.
    pub fn of(visual: &Visual, radius: f32) -> Look {
        let diameter = 2.0 * radius;
        match visual {
            Visual::Primitive(shape) => Look {
                shape: *shape,
                radius,
                length: match shape {
                    Primitive::Capsule => diameter + radius * CAPSULE_LENGTH_PER_RADIUS,
                    Primitive::Cuboid
                    | Primitive::Cylinder
                    | Primitive::Sphere
                    | Primitive::Cone => diameter,
                },
                lying: false,
            },
            Visual::Body {
                shape,
                height_tiles_x100,
                lying,
            } => Look {
                shape: *shape,
                radius,
                // The validator rejects a Body sphere; were one drawn, it
                // would still be a ball of the kind's radius.
                length: if *shape == Primitive::Sphere {
                    diameter
                } else {
                    *height_tiles_x100 as f32 / 100.0
                },
                lying: *lying,
            },
            Visual::Scene { .. } => Look::of(&FALLBACK_VISUAL, radius),
        }
    }

    /// Height of the top of the look above the ground: its length when
    /// upright, its width when lying.
    pub fn top_y(&self) -> f32 {
        if self.lying {
            2.0 * self.radius
        } else {
            self.length
        }
    }
}

/// Height above the ground of the top of `visual`'s look for a unit of
/// collision radius `radius`, in metres, computed from the visual rather
/// than measured on the mesh (`top_y_matches_the_mesh_aabb` holds them
/// together). Health bars and other markers above a unit read it through
/// [`UnitVisuals::top_y`].
pub fn look_top_y(visual: &Visual, radius: f32) -> f32 {
    Look::of(visual, radius).top_y()
}

/// The mesh for a unit look ([`Look::of`]), low-poly and flat-shaded
/// (`docs/ART_STYLE.md`), standing on the ground at the origin: every
/// shape is built centred with Bevy's low-resolution builders
/// ([`LOOK_SIDES`] facets, [`CAPSULE_LATITUDES`], an icosphere of
/// [`SPHERE_SUBDIVISIONS`]), a lying shape is turned so its long axis is
/// +X (the facing at yaw 0, see [`yaw_from_facing`]), and the result is
/// lifted so its bottom touches y = 0. Then every triangle gets its own
/// vertices and a face normal. The mesh keeps `RenderAssetUsages::default()`
/// (main and render world): picking ray-casts it on the CPU.
pub fn body_mesh(visual: &Visual, radius: f32) -> Mesh {
    let look = Look::of(visual, radius);
    let (r, length) = (look.radius, look.length);
    let mesh = match look.shape {
        Primitive::Capsule => Capsule3d::new(r, (length - 2.0 * r).max(0.0))
            .mesh()
            .longitudes(LOOK_SIDES)
            .latitudes(CAPSULE_LATITUDES)
            .build(),
        Primitive::Cuboid => Cuboid::new(2.0 * r, length, 2.0 * r).mesh().build(),
        Primitive::Cylinder => Cylinder::new(r, length)
            .mesh()
            .resolution(LOOK_SIDES)
            .build(),
        Primitive::Cone => Cone::new(r, length).mesh().resolution(LOOK_SIDES).build(),
        Primitive::Sphere => Sphere::new(r)
            .mesh()
            .ico(SPHERE_SUBDIVISIONS)
            .expect("an icosphere of 1 subdivision is far below Bevy's 80-subdivision limit"),
    };
    let mut mesh = if look.lying {
        mesh.rotated_by(Quat::from_rotation_z(-FRAC_PI_2))
            .translated_by(Vec3::Y * r)
    } else {
        mesh.translated_by(Vec3::Y * (length * 0.5))
    };
    mesh.duplicate_vertices();
    mesh.compute_flat_normals();
    mesh
}

/// Rotation that lays a gizmo circle (drawn in XY) flat on the ground.
pub fn flat_on_ground() -> Quat {
    Quat::from_rotation_x(-FRAC_PI_2)
}

/// Mirrors sim units into entities and interpolates their transforms.
pub struct PresentPlugin;

impl Plugin for PresentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UnitEntities>()
            .init_resource::<UnitVisuals>()
            .register_type::<Interp>()
            .add_systems(
                FixedUpdate,
                (sync_lifecycle, focus_camera_on_first_units)
                    .chain()
                    .after(SimSystems::Step)
                    .run_if(sim_stepped),
            )
            .add_systems(
                PostUpdate,
                sync_transforms.before(TransformSystems::Propagate),
            );
    }
}

/// After every tick: for each unit in `sim.view().units()`, update its
/// [`Interp`] (`advance` to the new pose) or spawn a new entity when the id
/// is not in [`UnitEntities`] (`Mesh3d`, `MeshMaterial3d`, `Transform`,
/// `UnitRef`, `Interp::at`, `Pickable::default()`, `Name`); despawn every
/// entity whose id is no longer in the view and drop it from the map. Runs
/// in `FixedUpdate` after [`SimSystems::Step`] so it sees every tick even
/// when several run in one frame, and only in iterations that stepped
/// ([`sim_stepped`]): a stalled or cap-dropped fixed step leaves `Interp`
/// alone so the render lerp finishes instead of snapping.
pub fn sync_lifecycle(
    sim: NonSend<SimHandle>,
    mut entities: ResMut<UnitEntities>,
    mut visuals: ResMut<UnitVisuals>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut interps: Query<&mut Interp>,
    mut commands: Commands,
) {
    let view = sim.view();
    let rules = sim.rules();
    for unit in view.units() {
        let pos = world_from_sim(unit.pos, UNIT_Y);
        let yaw = yaw_from_facing(unit.facing);
        match entities.0.get(&unit.id) {
            Some(entity) => {
                if let Ok(mut interp) = interps.get_mut(*entity) {
                    interp.advance(pos, yaw);
                }
            }
            None => {
                let mesh = visuals.mesh(unit.kind.0, rules, &mut meshes);
                let material = visuals.material(unit.kind.0, unit.owner.0, &mut materials);
                let entity = commands
                    .spawn((
                        Name::new(format!("Unit {}", unit.id.0)),
                        UnitRef(unit.id),
                        Interp::at(pos, yaw),
                        Mesh3d(mesh),
                        MeshMaterial3d(material),
                        Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw)),
                        Pickable::default(),
                    ))
                    .id();
                entities.0.insert(unit.id, entity);
            }
        }
    }
    if entities.0.len() > view.unit_count() {
        entities.0.retain(|id, entity| {
            if view.unit(*id).is_some() {
                true
            } else {
                commands.entity(*entity).despawn();
                false
            }
        });
    }
}

/// Every frame: `transform.translation, yaw = interp.sample(fixed.overstep_fraction())`,
/// `transform.rotation = Quat::from_rotation_y(yaw)`. Runs in `PostUpdate`
/// before transform propagation so the frame renders the interpolated pose.
pub fn sync_transforms(
    fixed: Res<Time<Fixed>>,
    mut units: Query<(&Interp, &mut Transform), With<UnitRef>>,
) {
    let t = fixed.overstep_fraction();
    for (interp, mut transform) in &mut units {
        let (pos, yaw) = interp.sample(t);
        transform.translation = pos;
        transform.rotation = Quat::from_rotation_y(yaw);
    }
}

/// Once per session, when the local player first has units, point the RTS
/// camera at their centroid (the start position). Later spawns never move
/// the camera; a skirmish with no units leaves it at the map centre.
pub fn focus_camera_on_first_units(
    sim: NonSend<SimHandle>,
    framing: Option<Res<crate::camera::StartFraming>>,
    mut cameras: Query<&mut RtsCamera>,
    mut done: Local<bool>,
) {
    if *done || framing.is_some() {
        return;
    }
    let view = sim.view();
    let (sum, n) = view
        .units()
        .filter(|u| u.owner == LOCAL_PLAYER)
        .map(|u| world_from_sim(u.pos, 0.0))
        .fold((Vec3::ZERO, 0u32), |(s, n), p| (s + p, n + 1));
    if n == 0 {
        return;
    }
    *done = true;
    let centroid = sum / n as f32;
    for mut cam in &mut cameras {
        cam.focus = Vec3::new(centroid.x, 0.0, centroid.z);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sim_world_round_trip_is_centred_on_the_origin() {
        let centre = FxVec2::from_ints(64, 64);
        assert_eq!(world_from_sim(centre, 0.0), Vec3::ZERO);
        let west = world_from_sim(sim::scenarios::WEST, 0.0);
        assert!(west.x < 0.0 && west.z == 0.0);
        let back = sim_from_world(west);
        assert_eq!(back, sim::scenarios::WEST);
        // Half-tile positions survive the round trip.
        let p = FxVec2::new(Fx::from_ratio(51, 2), Fx::from_ratio(7, 4));
        assert_eq!(sim_from_world(world_from_sim(p, 3.0)), p);
        // Clicks past the edge clamp into the map.
        let far = sim_from_world(Vec3::new(1e4, 0.0, -1e4));
        assert!(far.x < Fx::from_int(128) && far.y >= Fx::ZERO);
    }

    #[test]
    fn yaw_follows_the_facing_vector() {
        let eps = 1e-5;
        assert!(
            (yaw_from_facing(FxVec2::from_ints(1, 0))).abs() < eps,
            "+x is yaw 0"
        );
        assert!(
            (yaw_from_facing(FxVec2::from_ints(0, 1)) + FRAC_PI_2).abs() < eps,
            "+sim y (world +z) is -90 deg"
        );
        assert!((yaw_from_facing(FxVec2::from_ints(0, -1)) - FRAC_PI_2).abs() < eps);
        assert_eq!(yaw_from_facing(FxVec2::ZERO), 0.0);
        // Rotating a +X vector by the yaw gives the world-space direction.
        let yaw = yaw_from_facing(FxVec2::from_ints(1, 1));
        let dir = Quat::from_rotation_y(yaw) * Vec3::X;
        assert!((dir - Vec3::new(1.0, 0.0, 1.0).normalize()).length() < 1e-5);
    }

    #[test]
    fn interp_samples_between_poses_and_takes_the_short_way_round() {
        let mut i = Interp::at(Vec3::ZERO, 0.0);
        i.advance(Vec3::X * 2.0, 3.0);
        let (p, yaw) = i.sample(0.5);
        assert_eq!(p, Vec3::X);
        assert!((yaw - 1.5).abs() < 1e-6);
        let mut j = Interp::at(Vec3::ZERO, 3.0);
        j.advance(Vec3::ZERO, -3.0);
        let (_, yaw) = j.sample(0.5);
        // From 3.0 to -3.0 is 0.28 rad through pi, not 6 rad through zero.
        assert!(yaw.abs() > 3.0, "{yaw}");
        assert_eq!(i.sample(2.0).0, Vec3::X * 2.0, "clamped");
    }

    fn repo_rules() -> Rules {
        Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap()
    }

    /// `(name, visual, radius)` of every repository unit kind's look.
    fn repo_looks(rules: &Rules) -> Vec<(String, Visual, f32)> {
        (0u16..)
            .zip(&rules.units)
            .map(|(i, kind)| {
                let visual = rules.visual(i).expect("every kind has a visual").clone();
                (
                    kind.id.clone(),
                    visual,
                    kind.radius_tiles_x100 as f32 / 100.0,
                )
            })
            .collect()
    }

    /// Every repository look plus every shape as a `Primitive`, upright and
    /// lying `Body`s of each shape a `Body` may have, and a `Scene`.
    fn every_look(rules: &Rules) -> Vec<(String, Visual, f32)> {
        let mut looks = repo_looks(rules);
        for shape in [
            Primitive::Capsule,
            Primitive::Cuboid,
            Primitive::Cylinder,
            Primitive::Sphere,
            Primitive::Cone,
        ] {
            looks.push((
                format!("Primitive({shape:?})"),
                Visual::Primitive(shape),
                0.35,
            ));
            if shape == Primitive::Sphere {
                continue;
            }
            for lying in [false, true] {
                looks.push((
                    format!("Body({shape:?}, lying {lying})"),
                    Visual::Body {
                        shape,
                        height_tiles_x100: 150,
                        lying,
                    },
                    0.4,
                ));
            }
        }
        looks.push((
            "Scene".into(),
            Visual::Scene {
                path: "models/x.glb".into(),
                scale_x100: 100,
                y_offset_tiles_x100: 0,
                yaw_deg: 0,
            },
            0.35,
        ));
        looks
    }

    #[test]
    fn primitive_meshes_stand_on_the_ground() {
        use bevy::camera::primitives::MeshAabb;
        for (name, visual, radius) in every_look(&repo_rules()) {
            let mesh = body_mesh(&visual, radius);
            let aabb = mesh.compute_aabb().expect("positions");
            let min_y = aabb.min().y;
            assert!(min_y.abs() < 1e-4, "{name} bottom at {min_y}");
            assert!(aabb.max().y > 0.3, "{name}");
            // Centred on the unit's position in the ground plane.
            let c = aabb.center;
            assert!(c.x.abs() < 1e-4 && c.z.abs() < 1e-4, "{name} centre {c}");
        }
    }

    #[test]
    fn top_y_matches_the_mesh_aabb() {
        use bevy::camera::primitives::MeshAabb;
        let rules = repo_rules();
        for (name, visual, radius) in every_look(&rules) {
            let aabb = body_mesh(&visual, radius)
                .compute_aabb()
                .expect("positions");
            let top = look_top_y(&visual, radius);
            assert!(
                (aabb.max().y - top).abs() < 1e-4,
                "{name}: mesh top {} vs look_top_y {top}",
                aabb.max().y
            );
        }
        // The cache answers once the mesh is built, and only then.
        let mut meshes = Assets::<Mesh>::default();
        let mut v = UnitVisuals::default();
        for (i, (name, visual, radius)) in (0u16..).zip(repo_looks(&rules)) {
            assert_eq!(v.top_y(i), None, "{name}");
            v.mesh(i, &rules, &mut meshes);
            assert_eq!(v.top_y(i), Some(look_top_y(&visual, radius)), "{name}");
        }
        // The repository looks, as data/visuals.ron describes them.
        let tops: Vec<(String, f32)> = repo_looks(&rules)
            .into_iter()
            .map(|(name, visual, radius)| (name, look_top_y(&visual, radius)))
            .collect();
        let expected = [
            ("yeoman", 1.19),
            ("scribe", 0.7),
            ("skirmisher", 1.5),
            ("shieldbearer", 1.3),
            ("bowman", 1.9),
            ("outrider", 0.9),
            ("mangonel", 1.0),
            ("supply_wain", 0.6),
        ];
        assert_eq!(tops.len(), expected.len());
        for ((name, top), (want_name, want)) in tops.iter().zip(expected) {
            assert_eq!(name, want_name);
            assert!((top - want).abs() < 1e-5, "{name}: {top} vs {want}");
        }
    }

    #[test]
    fn looks_stay_under_600_vertices() {
        for (name, visual, radius) in every_look(&repo_rules()) {
            let mesh = body_mesh(&visual, radius);
            let n = mesh.count_vertices();
            assert!(n <= MAX_LOOK_VERTICES, "{name}: {n} vertices");
            // Flat-shaded: no shared vertices, one face normal per triangle.
            assert!(mesh.indices().is_none(), "{name} is still indexed");
            assert_eq!(n % 3, 0, "{name}");
            let Some(bevy::mesh::VertexAttributeValues::Float32x3(normals)) =
                mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
            else {
                panic!("{name} has float3 normals");
            };
            for tri in normals.as_chunks::<3>().0 {
                assert!(tri[0] == tri[1] && tri[1] == tri[2], "{name}: {tri:?}");
            }
            // Picking ray-casts the mesh on the CPU, so it stays in the main world.
            assert!(
                mesh.asset_usage
                    .contains(bevy::asset::RenderAssetUsages::MAIN_WORLD),
                "{name}"
            );
        }
    }

    #[test]
    fn every_unit_kind_has_its_own_silhouette() {
        use bevy::camera::primitives::MeshAabb;
        let rules = repo_rules();
        let looks: Vec<(String, Primitive, Vec3)> = repo_looks(&rules)
            .into_iter()
            .map(|(name, visual, radius)| {
                let aabb = body_mesh(&visual, radius)
                    .compute_aabb()
                    .expect("positions");
                let shape = Look::of(&visual, radius).shape;
                (name, shape, Vec3::from(aabb.half_extents) * 2.0)
            })
            .collect();
        assert_eq!(looks.len(), 8, "eight kinds in units.ron");
        for (i, (a, shape_a, size_a)) in looks.iter().enumerate() {
            for (b, shape_b, size_b) in &looks[i + 1..] {
                if shape_a != shape_b {
                    continue;
                }
                // The same shape must differ by at least 1.4x on some axis.
                let ratio = (*size_a / *size_b).max(*size_b / *size_a).max_element();
                assert!(
                    ratio >= 1.4,
                    "{a} and {b} are both {shape_a:?} and only {ratio:.2}x apart ({size_a} vs {size_b})"
                );
            }
        }
    }

    #[test]
    fn lying_bodies_point_along_facing() {
        use bevy::camera::primitives::MeshAabb;
        let rules = repo_rules();
        let lying: Vec<_> = every_look(&rules)
            .into_iter()
            .filter(|(_, visual, radius)| Look::of(visual, *radius).lying)
            .collect();
        assert!(
            lying.iter().any(|(name, ..)| name == "outrider"),
            "the Outrider lies"
        );
        for (name, visual, radius) in lying {
            let look = Look::of(&visual, radius);
            let mesh = body_mesh(&visual, radius);
            let size = Vec3::from(mesh.compute_aabb().expect("positions").half_extents) * 2.0;
            // At yaw 0 (facing +x) the long axis is world x.
            assert!((size.x - look.length).abs() < 1e-4, "{name}: {size}");
            assert!((size.y - 2.0 * radius).abs() < 1e-4, "{name}: {size}");
            assert!(size.z <= 2.0 * radius + 1e-4, "{name}: {size}");
            assert!(look.length > size.z, "{name} is longer than it is wide");
            // Turned by the yaw `sync_transforms` gives a unit facing +sim y
            // (world +z), the long axis follows: it is the world z extent.
            let yaw = yaw_from_facing(FxVec2::from_ints(0, 1));
            let turned = mesh.rotated_by(Quat::from_rotation_y(yaw));
            let size = Vec3::from(turned.compute_aabb().expect("positions").half_extents) * 2.0;
            assert!((size.z - look.length).abs() < 1e-4, "{name} turned: {size}");
            assert!(size.x <= 2.0 * radius + 1e-4, "{name} turned: {size}");
        }
    }

    #[test]
    fn a_stalled_fixed_step_leaves_interp_alone() {
        use crate::sim_driver::{InputSource, PendingCommands, ScenarioInput, SimPlugin};
        use sim::PlayerCommand;
        use std::time::Duration;

        /// `scripted_moves` until `until`, then stalls forever.
        struct StallAfter {
            inner: ScenarioInput,
            until: u32,
        }
        impl InputSource for StallAfter {
            fn commands_for(
                &mut self,
                tick: u32,
                local: &mut PendingCommands,
            ) -> Option<Vec<PlayerCommand>> {
                (tick < self.until)
                    .then(|| self.inner.commands_for(tick, local))
                    .flatten()
            }
            fn name(&self) -> &'static str {
                "stall-after"
            }
        }

        let rules =
            Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
                .unwrap();
        let (setup, stream) = sim::scenarios::scripted_moves(&rules);
        let handle = SimHandle::from_setup(setup, rules);
        let input = StallAfter {
            inner: ScenarioInput::new(stream),
            until: 25,
        };
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(Assets::<Mesh>::default())
            .insert_resource(Assets::<StandardMaterial>::default())
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                Duration::ZERO,
            ))
            .add_plugins((SimPlugin::new(handle, Box::new(input)), PresentPlugin));
        app.update();
        let step = |app: &mut App| {
            app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                Duration::from_millis(50),
            ));
            app.update();
        };
        for _ in 0..25 {
            step(&mut app);
        }
        assert_eq!(app.world().non_send::<SimHandle>().tick(), 25);
        let entity = app.world().resource::<UnitEntities>().0[&UnitId(1)];
        let before = *app.world().get::<Interp>(entity).unwrap();
        assert_ne!(before.prev, before.curr, "unit 1 is under way at tick 25");
        step(&mut app);
        step(&mut app);
        assert_eq!(app.world().non_send::<SimHandle>().tick(), 25, "stalled");
        let after = *app.world().get::<Interp>(entity).unwrap();
        assert_eq!(before, after, "a stalled step must not advance Interp");
    }

    #[test]
    fn visuals_are_shared_per_kind_and_team() {
        let rules = repo_rules();
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let mut v = UnitVisuals::default();
        let a = v.mesh(0, &rules, &mut meshes);
        let b = v.mesh(0, &rules, &mut meshes);
        assert_eq!(a, b);
        let m0 = v.material(0, 0, &mut materials);
        let m1 = v.material(0, 1, &mut materials);
        assert_ne!(m0, m1);
        assert_eq!(v.material(0, 0, &mut materials), m0);
        assert_eq!(v.counts(), (1, 2));
    }
}
