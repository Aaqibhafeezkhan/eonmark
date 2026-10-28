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
//! Since art slice 4 a kind whose look is a `Visual::Scene` with a ready
//! model (`crate::models`) draws that merged model, with its texture
//! repainted for the owner's team ([`UnitVisuals::mesh_with`],
//! [`UnitVisuals::material_with`]); a kind whose model failed, and every
//! kind in headless runs and tests, draws the Scene's `fallback`. Units of
//! a kind and team share one mesh and one material, so 400 units stay a
//! few dozen draws. A Scene with a `banner_at` (the Mangonel, owner
//! decision 12 of the art pass) carries the team banner as a top-level
//! [`UnitBanner`] entity that follows it every frame, never a child (rule
//! 16). Walking units bob ([`Gait`]): civilians, infantry and cavalry lift
//! by up to [`GAIT_LIFT_M`] once per [`GAIT_STRIDE_M`] of ground covered,
//! siege and support units roll; the bob is presentation only, read from
//! the interpolated pose and never fed back to the sim.
//!
//! When the local player's first units appear, the camera focus jumps once
//! to their centroid ([`focus_camera_on_first_units`]) so a scenario that
//! spawns at the west start is on screen from the first frame.
//!
//! At the outcome (M5a) the live sim stops stepping, but the fixed clock's
//! overstep keeps cycling, so a unit still lerping between two poses would
//! swing between them forever: [`settle_at_outcome`] sets every `Interp`'s
//! previous pose to its current one once, in the fixed step that decided
//! the match, after [`sync_lifecycle`] advanced it.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI};

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use rules::{Primitive, Rules, UnitClass, Visual};
use sim::{Fx, FxVec2, UnitId};

use crate::buildings::{DrawAssets, mount_transform};
use crate::camera::RtsCamera;
use crate::ground::HALF_EXTENT;
use crate::models::{Model, ModelLibrary};
use crate::palette;
use crate::sim_driver::{LOCAL_PLAYER, MatchOver, SimHandle, SimSystems, sim_stepped};

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

/// What [`body_mesh`] draws for a `Visual::Scene` without a `fallback`
/// whose model is not ready (headless runs, tests, a failed load), and for
/// a kind with no visual.
pub const FALLBACK_VISUAL: Visual = Visual::Primitive(Primitive::Capsule);

/// Height of the walk bob at mid-step, metres (art pass plan 4.3).
pub const GAIT_LIFT_M: f32 = 0.05;
/// Ground a walking unit covers per step, metres: the bob lifts once per
/// stride.
pub const GAIT_STRIDE_M: f32 = 0.6;
/// How fast a unit that stopped finishes the step it was in, metres of
/// stride per second, so it settles on the ground instead of freezing
/// mid-air (and nothing bobs at rest or after the outcome).
pub const GAIT_SETTLE_M_PER_S: f32 = 1.2;

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

    /// Hold the current pose: `prev = curr` (and the yaw), so every
    /// [`Interp::sample`] returns it. The frozen game at the outcome (M5a).
    pub fn settle(&mut self) {
        self.prev = self.curr;
        self.prev_yaw = self.curr_yaw;
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

/// The walk bob of a civilian, infantry or cavalry unit ([`has_gait`]):
/// the step phase advances by the ground the unit's interpolated pose
/// covers each frame, and [`Gait::step`] gives the lift. Presentation
/// only: it reads the render pose and never feeds the sim.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Gait {
    /// Distance into the current stride, metres, in `0..GAIT_STRIDE_M`.
    pub phase: f32,
    /// The pose the last frame drew (height ignored).
    pub last: Vec3,
}

impl Gait {
    /// A unit standing at `pos`.
    pub fn at(pos: Vec3) -> Self {
        Self {
            phase: 0.0,
            last: pos,
        }
    }

    /// Advance to this frame's pose `pos`, `dt` seconds after the last,
    /// and return the lift in metres: [`gait_lift`] of the phase after
    /// adding the horizontal distance moved, or, when the unit did not
    /// move, after finishing its step at [`GAIT_SETTLE_M_PER_S`] (a unit
    /// at rest ends on the ground).
    pub fn step(&mut self, pos: Vec3, dt: f32) -> f32 {
        let moved = Vec2::new(pos.x - self.last.x, pos.z - self.last.z).length();
        self.last = pos;
        if moved > 1e-5 {
            self.phase = (self.phase + moved).rem_euclid(GAIT_STRIDE_M);
        } else if self.phase > 0.0 {
            let next = self.phase + GAIT_SETTLE_M_PER_S * dt.max(0.0);
            self.phase = if next >= GAIT_STRIDE_M { 0.0 } else { next };
        }
        gait_lift(self.phase)
    }
}

/// The lift at `phase` metres into a stride: `GAIT_LIFT_M * |sin|`, zero
/// at each footfall (phase 0) and highest mid-step.
pub fn gait_lift(phase: f32) -> f32 {
    GAIT_LIFT_M * (phase * PI / GAIT_STRIDE_M).sin().abs()
}

/// Whether units of `class` walk (and bob): civilians, infantry and
/// cavalry do; siege engines and the supply wain roll.
pub fn has_gait(class: UnitClass) -> bool {
    matches!(
        class,
        UnitClass::Civilian | UnitClass::Infantry | UnitClass::Cavalry
    )
}

/// A unit's team banner (owner decision 12 of the art pass: the Mangonel
/// carries one): a top-level entity, never the unit's child (rule 16),
/// placed every frame at the unit's pose times `local`, the Scene's
/// `banner_at` mount in the unit's own axes ([`sync_transforms`]).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct UnitBanner {
    /// The unit carrying it.
    pub unit: UnitId,
    /// The mount, in the unit's frame (+X its facing).
    pub local: Transform,
}

/// On a unit entity: the [`UnitBanner`] entity it carries, despawned with
/// it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarriesBanner(pub Entity);

/// The render-side mirror: which entity shows which sim unit. Rings, bars
/// and markers are keyed by `UnitId` elsewhere; this is the only map from
/// id to the unit's own entity.
#[derive(Resource, Default, Debug)]
pub struct UnitEntities(pub HashMap<UnitId, Entity>);

/// Shared mesh per unit kind and material per `(kind, team)`, created on
/// first use so draw calls stay low (one batch per combination), and the
/// height of each kind's look ([`look_top_y`], or its model's top), cached
/// with its mesh.
#[derive(Resource, Default)]
pub struct UnitVisuals {
    meshes: HashMap<u16, Handle<Mesh>>,
    top_y: HashMap<u16, f32>,
    /// The ready model each kind is drawn as (art slice 4); a kind drawn as
    /// its fallback look has none.
    models: HashMap<u16, Model>,
    materials: HashMap<(u16, u8), Handle<StandardMaterial>>,
}

impl UnitVisuals {
    /// The mesh for `kind` with no model library: its fallback look
    /// ([`UnitVisuals::mesh_with`] with `None`).
    pub fn mesh(&mut self, kind: u16, rules: &Rules, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        self.mesh_with(kind, rules, meshes, None)
    }

    /// The mesh for `kind`: the merged model of its `Visual::Scene` when
    /// `models` holds it ready (art slice 4: scale, yaw and offsets are
    /// baked in, so the unit's transform stays translation and facing),
    /// else [`body_mesh`] of its look (a Scene's `fallback`, else
    /// [`FALLBACK_VISUAL`]) sized from `radius_tiles_x100`. Also caches the
    /// look's height (the higher of the model's top and the top of the team
    /// banner the kind carries, or [`look_top_y`]) for
    /// [`UnitVisuals::top_y`]. Cached on first use; the load gate
    /// (`models::models_settled`) makes that first use come after every
    /// model has settled, so no kind caches its fallback while its model
    /// is still loading.
    pub fn mesh_with(
        &mut self,
        kind: u16,
        rules: &Rules,
        meshes: &mut Assets<Mesh>,
        models: Option<&ModelLibrary>,
    ) -> Handle<Mesh> {
        if let Some(mesh) = self.meshes.get(&kind) {
            return mesh.clone();
        }
        if let Some(model) = rules
            .visual(kind)
            .and_then(|visual| models.and_then(|lib| lib.model(visual)))
        {
            // A carried banner counts (the Mangonel's stands far above its
            // catapult): the bar rides over it, so the cloth between the
            // camera and the unit never covers the bar.
            let banner_top = match rules.visual(kind) {
                Some(Visual::Scene {
                    banner_at: Some(mount),
                    ..
                }) => rules
                    .visuals
                    .banner
                    .as_ref()
                    .and_then(|b| models.and_then(|lib| lib.model(b)))
                    .map(|b| {
                        mount.offset_x100.1 as f32 / 100.0
                            + b.top_y() * mount.scale_x100 as f32 / 100.0
                    }),
                _ => None,
            };
            self.top_y.insert(
                kind,
                banner_top.map_or(model.top_y(), |t| t.max(model.top_y())),
            );
            self.meshes.insert(kind, model.mesh.clone());
            self.models.insert(kind, model.clone());
            return model.mesh.clone();
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
    /// [`UnitVisuals::mesh`] has built it (every kind on screen has): the
    /// model's top, or its carried banner's when that stands higher, since
    /// art slice 4, else [`look_top_y`] of the fallback look. The one place
    /// anything drawn above a unit takes its height from.
    pub fn top_y(&self, kind: u16) -> Option<f32> {
        self.top_y.get(&kind).copied()
    }

    /// The matte team-coloured material for `(kind, team)` with no model
    /// library ([`UnitVisuals::material_with`] with `None`).
    pub fn material(
        &mut self,
        kind: u16,
        team: u8,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        self.material_with(kind, team, None, materials, None, None)
    }

    /// The material for `(kind, team)`: for a kind drawn as its model
    /// ([`UnitVisuals::mesh_with`] first), the library's material for the
    /// model's texture repainted with `swatch` (its Scene's `team`) for
    /// `team`, shared with every model on that texture; otherwise the matte
    /// team colour. Cached per `(kind, team)`.
    pub fn material_with(
        &mut self,
        kind: u16,
        team: u8,
        swatch: Option<&str>,
        materials: &mut Assets<StandardMaterial>,
        models: Option<&mut ModelLibrary>,
        images: Option<&mut Assets<Image>>,
    ) -> Handle<StandardMaterial> {
        if let Some(handle) = self.materials.get(&(kind, team)) {
            return handle.clone();
        }
        let handle = match (self.models.get(&kind), models, images) {
            (Some(model), Some(lib), Some(images)) => {
                lib.material(model, swatch.map(|k| (k, team)), images, materials)
            }
            _ => materials.add(StandardMaterial {
                base_color: palette::team_color(team),
                perceptual_roughness: MATTE_ROUGHNESS,
                reflectance: MATTE_REFLECTANCE,
                metallic: 0.0,
                ..default()
            }),
        };
        self.materials.insert((kind, team), handle.clone());
        handle
    }

    /// Whether `kind` is drawn as its model (after
    /// [`UnitVisuals::mesh_with`] found it ready).
    pub fn is_model(&self, kind: u16) -> bool {
        self.models.contains_key(&kind)
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
    /// tall, everything else 2), `Body` takes its own height, and `Scene`
    /// is its `fallback` (a `Primitive` or a `Body`, `rules::Visuals`
    /// checks), else [`FALLBACK_VISUAL`]: what a Scene kind draws when its
    /// model is not ready (art slice 4 draws ready models instead,
    /// [`UnitVisuals::mesh_with`]).
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
            Visual::Scene { fallback, .. } => {
                Look::of(fallback.as_deref().unwrap_or(&FALLBACK_VISUAL), radius)
            }
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
                FixedUpdate,
                settle_at_outcome
                    .after(sync_lifecycle)
                    .run_if(resource_added::<MatchOver>),
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
/// `UnitRef`, `Interp::at`, `Pickable::default()`, `Name`, plus a [`Gait`]
/// for a walking kind and a [`UnitBanner`] entity for a kind whose Scene
/// names a `banner_at`); despawn every entity whose id is no longer in the
/// view, with its banner, and drop it from the map. Runs in `FixedUpdate`
/// after [`SimSystems::Step`] so it sees every tick even when several run
/// in one frame, and only in iterations that stepped ([`sim_stepped`]): a
/// stalled or cap-dropped fixed step leaves `Interp` alone so the render
/// lerp finishes instead of snapping.
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn sync_lifecycle(
    sim: NonSend<SimHandle>,
    mut entities: ResMut<UnitEntities>,
    mut visuals: ResMut<UnitVisuals>,
    mut assets: DrawAssets,
    mut interps: Query<&mut Interp>,
    carriers: Query<&CarriesBanner>,
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
                let kind = unit.kind.0;
                let (mesh, material) =
                    unit_look(&mut visuals, &mut assets, rules, kind, unit.owner.0);
                let transform =
                    Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw));
                let mut entity = commands.spawn((
                    Name::new(format!("Unit {}", unit.id.0)),
                    UnitRef(unit.id),
                    Interp::at(pos, yaw),
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    transform,
                    Pickable::default(),
                ));
                if rules.unit_kind(kind).is_some_and(|k| has_gait(k.class)) {
                    entity.insert(Gait::at(pos));
                }
                let id = entity.id();
                if let Some((mesh, material, local)) =
                    unit_banner(&visuals, &mut assets, rules, kind, unit.owner.0)
                {
                    let banner = commands
                        .spawn((
                            Name::new(format!("Banner of unit {}", unit.id.0)),
                            UnitBanner {
                                unit: unit.id,
                                local,
                            },
                            Mesh3d(mesh),
                            MeshMaterial3d(material),
                            transform.mul_transform(local),
                            NotShadowCaster,
                        ))
                        .id();
                    commands.entity(id).insert(CarriesBanner(banner));
                }
                entities.0.insert(unit.id, id);
            }
        }
    }
    if entities.0.len() > view.unit_count() {
        entities.0.retain(|id, entity| {
            if view.unit(*id).is_some() {
                true
            } else {
                if let Ok(banner) = carriers.get(*entity) {
                    commands.entity(banner.0).despawn();
                }
                commands.entity(*entity).despawn();
                false
            }
        });
    }
}

/// The shared mesh and material a unit of `kind` owned by `team` is drawn
/// with: its kind's ready model in the team's repaint of the model's
/// texture, else its fallback look in the matte team colour
/// ([`UnitVisuals::mesh_with`], [`UnitVisuals::material_with`]).
pub fn unit_look(
    visuals: &mut UnitVisuals,
    assets: &mut DrawAssets,
    rules: &Rules,
    kind: u16,
    team: u8,
) -> (Handle<Mesh>, Handle<StandardMaterial>) {
    let mesh = visuals.mesh_with(kind, rules, &mut assets.meshes, assets.models.as_deref());
    let swatch = match rules.visual(kind) {
        Some(Visual::Scene {
            team: Some(key), ..
        }) => Some(key.as_str()),
        _ => None,
    };
    let material = visuals.material_with(
        kind,
        team,
        swatch,
        &mut assets.materials,
        assets.models.as_deref_mut(),
        assets.images.as_deref_mut(),
    );
    (mesh, material)
}

/// The team banner a unit of `kind` owned by `team` carries, when its kind
/// is drawn as its model and its Scene names a `banner_at` and the banner
/// model is ready: the banner's mesh (shared with every building's
/// banner), its team material and the mount in the unit's frame.
pub fn unit_banner(
    visuals: &UnitVisuals,
    assets: &mut DrawAssets,
    rules: &Rules,
    kind: u16,
    team: u8,
) -> Option<(Handle<Mesh>, Handle<StandardMaterial>, Transform)> {
    if !visuals.is_model(kind) {
        return None;
    }
    let Some(Visual::Scene {
        banner_at: Some(mount),
        ..
    }) = rules.visual(kind)
    else {
        return None;
    };
    let banner = rules.visuals.banner.as_ref()?;
    let model = assets.models.as_deref()?.model(banner)?.clone();
    let swatch = match banner {
        Visual::Scene {
            team: Some(key), ..
        } => Some((key.as_str(), team)),
        _ => None,
    };
    let lib = assets.models.as_deref_mut()?;
    let images = assets.images.as_deref_mut()?;
    let material = lib.material(&model, swatch, images, &mut assets.materials);
    Some((model.mesh, material, mount_transform(Vec3::ZERO, mount)))
}

/// Once, in the fixed step that decided the match ([`MatchOver`] was just
/// inserted by `step_sim`), after [`sync_lifecycle`] advanced every unit to
/// the deciding tick: [`Interp::settle`] on every unit, so the frozen game
/// shows each unit still at its final pose instead of swinging between its
/// last two (the live sim never steps again, while the overstep fraction
/// keeps cycling). Health bars and rings sample the same `Interp`.
pub fn settle_at_outcome(mut interps: Query<&mut Interp>) {
    for mut interp in &mut interps {
        interp.settle();
    }
}

/// Every frame: `transform.translation, yaw = interp.sample(fixed.overstep_fraction())`,
/// lifted by the unit's [`Gait`] when it has one, and
/// `transform.rotation = Quat::from_rotation_y(yaw)`; a unit's
/// [`UnitBanner`] follows at the unit's transform times its mount. Runs in
/// `PostUpdate` before transform propagation so the frame renders the
/// interpolated pose.
pub fn sync_transforms(
    fixed: Res<Time<Fixed>>,
    time: Res<Time>,
    mut units: Query<
        (
            &Interp,
            &mut Transform,
            Option<&mut Gait>,
            Option<&CarriesBanner>,
        ),
        With<UnitRef>,
    >,
    mut banners: Query<(&UnitBanner, &mut Transform), Without<UnitRef>>,
) {
    let t = fixed.overstep_fraction();
    let dt = time.delta_secs();
    for (interp, mut transform, gait, banner) in &mut units {
        let (pos, yaw) = interp.sample(t);
        let lift = gait.map_or(0.0, |mut g| g.step(pos, dt));
        transform.translation = pos + Vec3::Y * lift;
        transform.rotation = Quat::from_rotation_y(yaw);
        if let Some(banner) = banner
            && let Ok((mount, mut placed)) = banners.get_mut(banner.0)
        {
            *placed = transform.mul_transform(mount.local);
        }
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

    /// The M4b unit looks, frozen as `data/visuals.ron` had them when M4b
    /// signed them off, with each kind's collision radius in metres: the
    /// look tests below measure these, not the live file, so an art-pass
    /// edit of `visuals.ron` never fails a Rust test (art-pass decision
    /// 17). `rules::Visuals::validate` keeps the live file's 1.4x rule.
    const M4B_LOOKS: [(&str, Visual, f32); 8] = [
        ("yeoman", Visual::Primitive(Primitive::Capsule), 0.35),
        ("scribe", Visual::Primitive(Primitive::Cylinder), 0.35),
        (
            "skirmisher",
            Visual::Body {
                shape: Primitive::Cone,
                height_tiles_x100: 150,
                lying: false,
            },
            0.35,
        ),
        (
            "shieldbearer",
            Visual::Body {
                shape: Primitive::Cuboid,
                height_tiles_x100: 130,
                lying: false,
            },
            0.40,
        ),
        (
            "bowman",
            Visual::Body {
                shape: Primitive::Cylinder,
                height_tiles_x100: 190,
                lying: false,
            },
            0.35,
        ),
        (
            "outrider",
            Visual::Body {
                shape: Primitive::Capsule,
                height_tiles_x100: 180,
                lying: true,
            },
            0.45,
        ),
        ("mangonel", Visual::Primitive(Primitive::Sphere), 0.50),
        (
            "supply_wain",
            Visual::Body {
                shape: Primitive::Cuboid,
                height_tiles_x100: 60,
                lying: false,
            },
            0.50,
        ),
    ];

    /// `(name, visual, radius)` of the frozen [`M4B_LOOKS`].
    fn m4b_looks() -> Vec<(String, Visual, f32)> {
        M4B_LOOKS
            .iter()
            .map(|(name, visual, radius)| ((*name).to_string(), visual.clone(), *radius))
            .collect()
    }

    /// A Scene look with `fallback`, as the art pass writes them.
    fn scene(fallback: Option<Visual>) -> Visual {
        Visual::Scene {
            path: "models/x.glb".into(),
            scale_x100: 100,
            y_offset_tiles_x100: 0,
            yaw_deg: 0,
            x_offset_tiles_x100: 0,
            z_offset_tiles_x100: 0,
            nodes: Vec::new(),
            parts: Vec::new(),
            team: None,
            fallback: fallback.map(Box::new),
            banner_at: None,
            seat_flag_at: None,
        }
    }

    /// `(name, visual, radius)` of every repository unit kind's look, as
    /// the live `data/visuals.ron` has them (only for checks that hold for
    /// any valid file).
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

    /// Every M4b look plus every shape as a `Primitive`, upright and lying
    /// `Body`s of each shape a `Body` may have, and a `Scene` with and
    /// without a fallback.
    fn every_look() -> Vec<(String, Visual, f32)> {
        let mut looks = m4b_looks();
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
        looks.push(("Scene".into(), scene(None), 0.35));
        looks.push((
            "Scene with a lying fallback".into(),
            scene(Some(Visual::Body {
                shape: Primitive::Cylinder,
                height_tiles_x100: 120,
                lying: true,
            })),
            0.4,
        ));
        looks
    }

    #[test]
    fn primitive_meshes_stand_on_the_ground() {
        use bevy::camera::primitives::MeshAabb;
        for (name, visual, radius) in every_look() {
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
        for (name, visual, radius) in every_look() {
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
        // The M4b looks, as data/visuals.ron described them.
        let tops: Vec<(String, f32)> = m4b_looks()
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
        for (name, visual, radius) in every_look() {
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
        let looks: Vec<(String, Primitive, Vec3)> = m4b_looks()
            .into_iter()
            .map(|(name, visual, radius)| {
                let aabb = body_mesh(&visual, radius)
                    .compute_aabb()
                    .expect("positions");
                let shape = Look::of(&visual, radius).shape;
                (name, shape, Vec3::from(aabb.half_extents) * 2.0)
            })
            .collect();
        assert_eq!(looks.len(), 8, "the eight M4b kinds");
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
        let lying: Vec<_> = every_look()
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

    /// Until models load, a Scene unit draws its fallback (a `Primitive`
    /// or a `Body`), and without one the presenter's capsule: the look, the
    /// mesh and the cached height all follow it.
    #[test]
    fn a_scene_draws_its_fallback() {
        use bevy::camera::primitives::MeshAabb;
        let body = Visual::Body {
            shape: Primitive::Cuboid,
            height_tiles_x100: 130,
            lying: false,
        };
        let with = scene(Some(body.clone()));
        assert_eq!(Look::of(&with, 0.4), Look::of(&body, 0.4));
        assert_eq!(look_top_y(&with, 0.4), 1.3);
        assert_eq!(
            Look::of(&scene(None), 0.35),
            Look::of(&FALLBACK_VISUAL, 0.35)
        );
        let aabb = |v: &Visual| body_mesh(v, 0.4).compute_aabb().expect("positions");
        assert_eq!(aabb(&with).half_extents, aabb(&body).half_extents);
        // Through the cache: a rules whose Yeoman is a Scene with the
        // Shieldbearer's M4b block as its fallback.
        let mut rules = repo_rules();
        rules.visuals.units.insert("yeoman".into(), with);
        let mut meshes = Assets::<Mesh>::default();
        let mut v = UnitVisuals::default();
        v.mesh(0, &rules, &mut meshes);
        let radius = rules.units[0].radius_tiles_x100 as f32 / 100.0;
        assert_eq!(v.top_y(0), Some(look_top_y(&body, radius)));
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

    /// An 8x8 texture, the team rect at (0, 0, 4, 4).
    fn atlas() -> Image {
        use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
        Image::new(
            Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![200; 8 * 8 * 4],
            TextureFormat::Rgba8UnormSrgb,
            bevy::asset::RenderAssetUsages::default(),
        )
    }

    /// `visual` (a Scene) put in `lib` as a ready model `top` m tall.
    fn ready(
        lib: &mut ModelLibrary,
        visual: &Visual,
        top: f32,
        meshes: &mut Assets<Mesh>,
        images: &mut Assets<Image>,
    ) -> Model {
        use crate::models::{Geometry, ModelKey, content_hash, finish_geometry};
        let image = atlas();
        let hash = content_hash(&image);
        let handle = images.add(image);
        let model = finish_geometry(
            Geometry {
                positions: vec![[-0.3, 0.0, -0.3], [0.3, top, 0.3], [0.0, 0.0, 0.3]],
                normals: vec![[0.0, 1.0, 0.0]; 3],
                uvs: vec![[0.1, 0.1]; 3],
                indices: vec![0, 1, 2],
                textures: vec![],
                base_color: None,
            },
            hash,
            meshes,
        );
        lib.insert_ready(ModelKey::of(visual).unwrap(), model.clone(), handle);
        model
    }

    /// A team Scene at `path` with `fallback` and an optional banner mount.
    fn team_scene(path: &str, fallback: Visual, banner_at: Option<rules::Mount>) -> Visual {
        match scene(Some(fallback)) {
            Visual::Scene {
                scale_x100,
                y_offset_tiles_x100,
                yaw_deg,
                x_offset_tiles_x100,
                z_offset_tiles_x100,
                nodes,
                parts,
                fallback,
                seat_flag_at,
                ..
            } => Visual::Scene {
                path: path.into(),
                scale_x100,
                y_offset_tiles_x100,
                yaw_deg,
                x_offset_tiles_x100,
                z_offset_tiles_x100,
                nodes,
                parts,
                team: Some("adv".into()),
                fallback,
                banner_at,
                seat_flag_at,
            },
            _ => unreachable!(),
        }
    }

    /// The repository rules with every unit kind its M4b look (no test
    /// reads the live looks), then the Yeoman and the Mangonel as team
    /// Scenes (the Mangonel with a banner mount), the Scribe as a Scene
    /// whose model never loads, the team banner model, a swatch, and a
    /// library with the Yeoman's, the Mangonel's and the banner's models
    /// ready.
    struct UnitFixture {
        rules: Rules,
        lib: ModelLibrary,
        meshes: Assets<Mesh>,
        images: Assets<Image>,
        yeoman: Model,
        mangonel: Model,
        banner: Model,
        mount: rules::Mount,
    }

    fn unit_fixture() -> UnitFixture {
        let mut rules = repo_rules();
        for (name, visual, _) in m4b_looks() {
            rules.visuals.units.insert(name, visual);
        }
        rules.visuals.team_swatches.insert(
            "adv".into(),
            rules::TeamSwatch {
                rects: vec![(0, 0, 4, 4)],
                desaturate_permille: 0,
            },
        );
        let mount = rules::Mount {
            offset_x100: (-40, 0, 40),
            scale_x100: 300,
            yaw_deg: 0,
        };
        let yeoman = team_scene("m/yeoman.glb", Visual::Primitive(Primitive::Capsule), None);
        let mangonel = team_scene(
            "m/catapult.gltf",
            Visual::Primitive(Primitive::Sphere),
            Some(mount.clone()),
        );
        let scribe = team_scene("m/scribe.glb", Visual::Primitive(Primitive::Cylinder), None);
        let banner = team_scene("m/flag.gltf", Visual::Primitive(Primitive::Cuboid), None);
        let banner = match banner {
            Visual::Scene { .. } => {
                let mut b = banner;
                if let Visual::Scene { fallback, .. } = &mut b {
                    *fallback = None;
                }
                b
            }
            other => other,
        };
        rules.visuals.units.insert("yeoman".into(), yeoman.clone());
        rules
            .visuals
            .units
            .insert("mangonel".into(), mangonel.clone());
        rules.visuals.units.insert("scribe".into(), scribe);
        rules.visuals.banner = Some(banner.clone());
        let (mut meshes, mut images) = (Assets::default(), Assets::default());
        let mut lib = ModelLibrary::new(&rules.visuals);
        let yeoman = ready(&mut lib, &yeoman, 1.3, &mut meshes, &mut images);
        let mangonel = ready(&mut lib, &mangonel, 0.8, &mut meshes, &mut images);
        let banner = ready(&mut lib, &banner, 0.28, &mut meshes, &mut images);
        UnitFixture {
            rules,
            lib,
            meshes,
            images,
            yeoman,
            mangonel,
            banner,
            mount,
        }
    }

    /// An app that runs [`PresentPlugin`] on `art_bench` (195 units a
    /// side of every kind, the `AttackMove` at tick 20) with the fixture's
    /// rules and ready models, one tick per update after the first.
    fn unit_app(f: UnitFixture) -> App {
        use crate::sim_driver::{ScenarioInput, SimPlugin};
        use bevy::time::TimeUpdateStrategy;
        let (setup, stream, _) =
            sim::scenarios::by_name("art_bench", &f.rules).expect("a scenario");
        let handle = SimHandle::from_setup(setup, f.rules);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(
                std::time::Duration::ZERO,
            ))
            .insert_resource(f.meshes)
            .insert_resource(f.images)
            .insert_resource(Assets::<StandardMaterial>::default())
            .insert_resource(f.lib)
            .add_plugins(SimPlugin::new(handle, Box::new(ScenarioInput::new(stream))))
            .add_plugins(PresentPlugin);
        app.update();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_millis(50),
        ));
        app
    }

    fn run_ticks(app: &mut App, n: u32) {
        let target = app.world().non_send::<SimHandle>().tick() + n;
        for _ in 0..1000 {
            if app.world().non_send::<SimHandle>().tick() >= target {
                return;
            }
            app.update();
        }
        panic!("tick {target} not reached");
    }

    /// `(kind id, owner, entity)` of every unit on the map.
    fn unit_entities(app: &App) -> Vec<(String, u8, Entity)> {
        let sim = app.world().non_send::<SimHandle>();
        let view = sim.view();
        let rules = sim.rules();
        let map = &app.world().resource::<UnitEntities>().0;
        view.units()
            .map(|u| {
                (
                    rules.unit_kind(u.kind.0).unwrap().id.clone(),
                    u.owner.0,
                    map[&u.id],
                )
            })
            .collect()
    }

    /// Art slice 4: a kind whose model is ready draws it, every unit of a
    /// kind shares that one mesh, and every unit of a kind and team one
    /// material, the model's texture repainted for the team (the
    /// library's, shared with every model on that texture); the cached
    /// top is the model's. A kind whose model never loaded draws its
    /// fallback look in the plain team colour. The Mangonel carries the
    /// team banner as a top-level entity that follows it as it moves and
    /// turns, and goes when it dies.
    #[test]
    fn units_of_a_kind_and_team_share_the_model_mesh_and_material() {
        let f = unit_fixture();
        let (yeoman_mesh, mangonel_mesh) = (f.yeoman.mesh.clone(), f.mangonel.mesh.clone());
        let banner_mesh = f.banner.mesh.clone();
        let mount = f.mount.clone();
        let mut app = unit_app(f);
        run_ticks(&mut app, 8);
        let units = unit_entities(&app);
        assert_eq!(units.len(), 400, "both armies stand");
        let mut q = app.world_mut().query::<(Entity, &UnitBanner)>();
        let banners: Vec<(Entity, UnitBanner)> =
            q.iter(app.world()).map(|(e, b)| (e, *b)).collect();
        let mesh_of = |e: Entity| app.world().get::<Mesh3d>(e).unwrap().0.clone();
        let material_of = |e: Entity| {
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(e)
                .unwrap()
                .0
                .clone()
        };
        let of = |kind: &str, team: u8| -> Vec<Entity> {
            units
                .iter()
                .filter(|(k, t, _)| k == kind && *t == team)
                .map(|(_, _, e)| *e)
                .collect()
        };
        let materials = app.world().resource::<Assets<StandardMaterial>>();
        for kind in ["yeoman", "mangonel"] {
            let want = if kind == "yeoman" {
                &yeoman_mesh
            } else {
                &mangonel_mesh
            };
            let mut team_materials = Vec::new();
            for team in [0, 1] {
                let es = of(kind, team);
                assert!(es.len() > 1, "{kind} team {team}: {}", es.len());
                let m = material_of(es[0]);
                for e in &es {
                    assert_eq!(&mesh_of(*e), want, "{kind}: the model mesh");
                    assert_eq!(material_of(*e), m, "{kind} team {team}: one material");
                }
                let mat = materials.get(&m).unwrap();
                assert!(
                    mat.base_color_texture.is_some(),
                    "{kind}: the team-painted texture"
                );
                team_materials.push(m);
            }
            assert_ne!(team_materials[0], team_materials[1], "{kind}");
        }
        // Both models are on one texture: one team material per team.
        assert_eq!(
            material_of(of("yeoman", 0)[0]),
            material_of(of("mangonel", 0)[0])
        );
        // The Scribe's model never loaded: its fallback cylinder in the
        // plain team colour.
        let scribe = of("scribe", 0)[0];
        assert_ne!(mesh_of(scribe), yeoman_mesh);
        let scribe_mat = materials.get(&material_of(scribe)).unwrap();
        assert!(scribe_mat.base_color_texture.is_none());
        assert_eq!(scribe_mat.base_color, palette::team_color(0));
        let visuals = app.world().resource::<UnitVisuals>();
        assert_eq!(visuals.top_y(0), Some(1.3), "the Yeoman's model top");
        assert!(visuals.is_model(0) && !visuals.is_model(1));
        assert_eq!(visuals.top_y(1), Some(0.7), "the Scribe's fallback top");
        // Banners: one per Mangonel, none on anyone else, top level, the
        // shared banner mesh in the team's material, at the unit's pose
        // times the mount.
        let mangonels = of("mangonel", 0).len() + of("mangonel", 1).len();
        assert_eq!(banners.len(), mangonels);
        for (kind, team, e) in &units {
            let carries = app.world().get::<CarriesBanner>(*e);
            assert_eq!(carries.is_some(), kind == "mangonel", "{kind}");
            let Some(carries) = carries else { continue };
            let banner = carries.0;
            let id = app.world().get::<UnitRef>(*e).unwrap().0;
            assert!(
                banners
                    .iter()
                    .any(|(b, ub)| *b == banner && ub.unit == id && ub.local.scale.x == 3.0),
                "its banner names it and the mount"
            );
            assert!(app.world().get::<ChildOf>(banner).is_none());
            assert!(app.world().get::<Pickable>(banner).is_none());
            assert_eq!(mesh_of(banner), banner_mesh);
            assert_eq!(
                material_of(banner),
                material_of(of("yeoman", *team)[0]),
                "the banner is on the same texture: the team material"
            );
            let unit_tf = *app.world().get::<Transform>(*e).unwrap();
            let banner_tf = *app.world().get::<Transform>(banner).unwrap();
            let want = unit_tf.mul_transform(crate::buildings::mount_transform(Vec3::ZERO, &mount));
            assert!(
                (banner_tf.translation - want.translation).length() < 1e-4
                    && banner_tf.rotation.abs_diff_eq(want.rotation, 1e-5)
                    && banner_tf.scale == want.scale,
                "{banner_tf:?} vs {want:?}"
            );
        }

        // Past the charge (tick 20) P1's Mangonels turn west and every
        // engine has moved: each banner still stands at its unit's pose
        // times the mount, turned with it.
        run_ticks(&mut app, 30);
        let carriers: Vec<(Entity, Entity)> = {
            let mut q = app.world_mut().query::<(Entity, &CarriesBanner)>();
            q.iter(app.world()).map(|(e, c)| (e, c.0)).collect()
        };
        assert_eq!(carriers.len(), mangonels);
        let mut turned = 0;
        for (e, banner) in &carriers {
            let unit_tf = *app.world().get::<Transform>(*e).unwrap();
            let banner_tf = *app.world().get::<Transform>(*banner).unwrap();
            let want = unit_tf.mul_transform(crate::buildings::mount_transform(Vec3::ZERO, &mount));
            assert!(
                (banner_tf.translation - want.translation).length() < 1e-4
                    && banner_tf.rotation.abs_diff_eq(want.rotation, 1e-5),
                "{banner_tf:?} vs {want:?}"
            );
            if unit_tf.rotation.angle_between(Quat::IDENTITY) > 1.0 {
                turned += 1;
            }
        }
        assert!(turned > 0, "some Mangonel faces away from +X");

        // A Mangonel that dies takes its banner with it.
        let (e, banner) = carriers[0];
        let id = app.world().get::<UnitRef>(e).unwrap().0;
        app.world_mut()
            .resource_mut::<crate::sim_driver::PendingCommands>()
            .push_local(sim::Command::DebugDamage {
                target: sim::TargetId::Unit(id),
                amount: u32::MAX,
            });
        run_ticks(&mut app, 4);
        assert!(
            !app.world().resource::<UnitEntities>().0.contains_key(&id),
            "the Mangonel is gone"
        );
        assert!(app.world().get_entity(e).is_err());
        assert!(app.world().get_entity(banner).is_err(), "so is its banner");
        let mut q = app.world_mut().query::<&UnitBanner>();
        assert_eq!(q.iter(app.world()).count(), mangonels - 1);
    }

    /// The walk bob: civilians, infantry and cavalry carry a [`Gait`],
    /// siege engines and the Wain do not; a walking unit lifts off the
    /// ground and the rest stay on it, and a unit that stops ends its step
    /// on the ground.
    #[test]
    fn gait_bobs_only_moving_infantry_cavalry_and_civilians() {
        // The step itself: mid-stride is the full lift, a footfall none.
        let mut g = Gait::at(Vec3::ZERO);
        assert_eq!(g.step(Vec3::ZERO, 0.1), 0.0, "at rest from the start");
        let lift = g.step(Vec3::new(GAIT_STRIDE_M / 2.0, 0.0, 0.0), 0.016);
        assert!((lift - GAIT_LIFT_M).abs() < 1e-5, "{lift}");
        assert_eq!(
            g.step(Vec3::new(GAIT_STRIDE_M / 2.0, 7.0, 0.0), 0.0),
            lift,
            "height does not count as walking"
        );
        // Stopped mid-step: it settles within one stride's settle time and
        // stays down.
        let mut settled = false;
        for _ in 0..60 {
            if g.step(Vec3::new(GAIT_STRIDE_M / 2.0, 0.0, 0.0), 0.016) == 0.0 {
                settled = true;
                break;
            }
        }
        assert!(settled, "{g:?}");
        assert_eq!(g.step(Vec3::new(GAIT_STRIDE_M / 2.0, 0.0, 0.0), 1.0), 0.0);
        assert!(gait_lift(GAIT_STRIDE_M) < 1e-6);
        assert!(has_gait(UnitClass::Civilian) && has_gait(UnitClass::Infantry));
        assert!(has_gait(UnitClass::Cavalry));
        assert!(!has_gait(UnitClass::Siege) && !has_gait(UnitClass::Support));

        // In the game: art_bench's armies attack-move at tick 20 (spawned
        // during tick 0, they stand from the command delay on).
        let mut app = unit_app(unit_fixture());
        run_ticks(&mut app, 8);
        let rules = app.world().non_send::<SimHandle>().rules().clone();
        let units = unit_entities(&app);
        assert_eq!(units.len(), 400, "both armies stand");
        for (kind, _, e) in &units {
            let class = rules.units.iter().find(|k| &k.id == kind).unwrap().class;
            assert_eq!(
                app.world().get::<Gait>(*e).is_some(),
                has_gait(class),
                "{kind}"
            );
        }
        let mut lifted = std::collections::BTreeSet::new();
        let walkers = ["skirmisher", "shieldbearer", "bowman", "outrider"];
        for _ in 0..200 {
            if walkers.iter().all(|k| lifted.contains(*k)) {
                break;
            }
            app.update();
            for (kind, _, e) in &units {
                let Some(tf) = app.world().get::<Transform>(*e) else {
                    continue;
                };
                let interp = app.world().get::<Interp>(*e).unwrap();
                if tf.translation.y > 1e-4 {
                    assert!(
                        app.world().get::<Gait>(*e).is_some(),
                        "{kind} rolls, never bobs"
                    );
                    assert!(tf.translation.y <= GAIT_LIFT_M + 1e-5);
                    assert!(interp.curr.y == 0.0, "the bob is not in the pose");
                    lifted.insert(kind.clone());
                }
            }
        }
        for kind in walkers {
            assert!(
                lifted.contains(kind),
                "{kind} walked without a bob: {lifted:?}"
            );
        }
    }
}
