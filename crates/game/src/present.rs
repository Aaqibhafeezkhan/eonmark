//! Presentation of the sim (M2 decision 4): one entity per `UnitId`,
//! spawned and despawned by diffing the `SimView` after every tick, with a
//! per-unit [`Interp`] of the previous and current tick positions that the
//! render transform lerps by `Time<Fixed>::overstep_fraction()`. Yaw is
//! derived render-side from the unit's facing vector with `atan2` in f32.
//! Meshes come from `data/visuals.ron` (`rules::Visual`), one shared mesh
//! per kind and one material per `(kind, team)` so 200 units are a handful
//! of draw calls.
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

/// Capsule length (cylinder part) as a multiple of the unit's radius.
pub const CAPSULE_LENGTH_PER_RADIUS: f32 = 1.4;

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
/// first use so draw calls stay low (one batch per combination).
#[derive(Resource, Default)]
pub struct UnitVisuals {
    meshes: HashMap<u16, Handle<Mesh>>,
    materials: HashMap<(u16, u8), Handle<StandardMaterial>>,
}

impl UnitVisuals {
    /// The mesh for `kind`, built from its `Visual` and `radius_tiles_x100`.
    /// A `Visual::Scene` falls back to a capsule until M7.
    pub fn mesh(&mut self, kind: u16, rules: &Rules, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        self.meshes
            .entry(kind)
            .or_insert_with(|| {
                let radius = rules
                    .unit_kind(kind)
                    .map_or(0.35, |k| k.radius_tiles_x100 as f32 / 100.0);
                let primitive = match rules.visual(kind) {
                    Some(Visual::Primitive(p)) => *p,
                    Some(Visual::Scene { .. }) | None => Primitive::Capsule,
                };
                meshes.add(primitive_mesh(primitive, radius))
            })
            .clone()
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

/// The mesh for a primitive sized from the unit's collision radius: the
/// capsule and cylinder stand on the ground (their origin is lifted so the
/// bottom touches y = 0), the cuboid is a radius-wide box of twice the
/// radius in height, the sphere sits on the ground.
pub fn primitive_mesh(primitive: Primitive, radius: f32) -> Mesh {
    let lift = |mut mesh: Mesh, dy: f32| {
        mesh.translate_by(Vec3::Y * dy);
        mesh
    };
    match primitive {
        Primitive::Capsule => {
            let length = radius * CAPSULE_LENGTH_PER_RADIUS;
            lift(
                Mesh::from(Capsule3d::new(radius, length)),
                radius + length * 0.5,
            )
        }
        Primitive::Cuboid => lift(
            Mesh::from(Cuboid::new(radius * 2.0, radius * 2.0, radius * 2.0)),
            radius,
        ),
        Primitive::Cylinder => lift(Mesh::from(Cylinder::new(radius, radius * 2.0)), radius),
        Primitive::Sphere => lift(Mesh::from(Sphere::new(radius)), radius),
    }
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

    #[test]
    fn primitive_meshes_stand_on_the_ground() {
        use bevy::camera::primitives::MeshAabb;
        for p in [
            Primitive::Capsule,
            Primitive::Cuboid,
            Primitive::Cylinder,
            Primitive::Sphere,
        ] {
            let mesh = primitive_mesh(p, 0.35);
            let aabb = mesh.compute_aabb().expect("positions");
            let min_y = aabb.min().y;
            assert!(min_y.abs() < 1e-4, "{p:?} bottom at {min_y}");
            assert!(aabb.max().y > 0.3, "{p:?}");
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
        let rules =
            Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
                .unwrap();
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
