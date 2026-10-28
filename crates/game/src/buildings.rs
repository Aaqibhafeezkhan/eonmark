//! Presentation of buildings and construction sites (M3b; art pass slice
//! 3): one entity per `BuildingId`, mirrored from the `SimView` after every
//! tick like units.
//!
//! A building is drawn as its kind's `data/visuals.ron` entry
//! (`rules::Rules::building_visual`):
//!
//! - `Primitive(Cuboid)` is the M3b look, a matte team-coloured box over
//!   its footprint ([`BuildingVisuals`]: one shared unit cube scaled per
//!   building, one material per `(team, site)`).
//! - A `Scene` draws its merged model (`crate::models`) at the footprint's
//!   centre with the owner's team material (one per texture and team, so
//!   every building on the `KayKit` atlas shares two), and the box when the
//!   model failed or there is no model library (headless, tests).
//! - A site draws `sites[min(progress * n / total, n - 1)]`
//!   ([`site_stage`]): a neutral construction-stage model scaled uniformly
//!   by the footprint side (footprints are square), or the M3b box in a
//!   lighter scaffold colour rising with its progress.
//!
//! Art pass extras, each a top-level entity (never a child), drawn only
//! when its model is ready:
//!
//! - the team banner (owner decision 12 of the art pass): every complete
//!   building drawn as its model whose Scene names a `banner_at` carries
//!   `visuals.banner` there ([`mount_transform`]); the M3b box of a failed
//!   model is team-coloured all over and carries none. All banners share
//!   one mesh and one material per team (the same team material as the
//!   buildings on that texture), so they add about two draw bins whatever
//!   the building count; they cast no shadow.
//! - the Seat flag ([`SeatFlag`]): on every complete Seat, at its Scene's
//!   `seat_flag_at` while the Seat is drawn as its model, else on the
//!   ground at the footprint's north-east corner; on the keep in the
//!   shipped data, above the Town's own banner, so a Seat reads apart from
//!   any other Town.
//! - rubble ([`Rubble`]): where a building or site was destroyed, for
//!   `rubble.ds` of real time, removed early when a new building occupies
//!   its footprint. [`spawn_rubble`] reads the destroyed entity's place
//!   before [`sync_buildings`] despawns it.
//!
//! Every material follows the owner, so an annexation recolours the model,
//! its banner and its flag together. Buildings carry `Pickable` so they can
//! be clicked (rule 16: only units and buildings are pickable); a model is
//! picked by ray-casting its merged mesh. [`building_top_y`] is the one
//! height bars and attack markers take a building's top from.

use std::collections::HashMap;

use bevy::ecs::system::SystemParam;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use rules::{Mount, Primitive, Rules, Visual};
use sim::{Building, BuildingId, Construction, SimEvent};

use crate::ground::HALF_EXTENT;
use crate::models::{Model, ModelLibrary};
use crate::palette;
use crate::present::{MATTE_REFLECTANCE, MATTE_ROUGHNESS};
use crate::sim_driver::{SimEventMsg, SimHandle, SimSystems, sim_stepped};

/// Height of a complete building in metres: half a metre plus 0.4 m per
/// tile of its shorter footprint side, so a 3x3 Town stands taller than a
/// 2x2 Croft and a 1x1 Watchtower is a short post (the M3b box).
pub fn building_height(width_tiles: u32, height_tiles: u32) -> f32 {
    0.5 + 0.4 * width_tiles.min(height_tiles) as f32
}

/// Lowest drawn height of a site, as a fraction of the complete height, so
/// a site that has not started is still visible on the ground.
pub const SITE_MIN_FRACTION: f32 = 0.08;

/// How far a site's colour is blended from the team colour toward stone.
pub const SITE_BLEND: f32 = 0.55;

/// Marks a building entity and names the sim building it mirrors.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BuildingRef(pub BuildingId);

/// The render-side mirror: which entity shows which sim building.
#[derive(Resource, Default, Debug)]
pub struct BuildingEntities(pub HashMap<BuildingId, Entity>);

/// A building's team banner (owner decision 12 of the art pass).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Banner(pub BuildingId);

/// The flag on a Seat.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeatFlag(pub BuildingId);

/// The wreck of a destroyed building or site.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Rubble {
    /// `Time<Real>` seconds at which it goes.
    pub until: f64,
    /// Its footprint: north-west tile and size in tiles.
    pub tile: (u16, u16),
    /// Width and height in tiles.
    pub size: (u16, u16),
}

impl Rubble {
    /// Whether its footprint overlaps the footprint at `tile` of `size`.
    pub fn overlaps(&self, tile: (u16, u16), size: (u16, u16)) -> bool {
        let (ax, ay, aw, ah) = (
            u32::from(self.tile.0),
            u32::from(self.tile.1),
            u32::from(self.size.0),
            u32::from(self.size.1),
        );
        let (bx, by, bw, bh) = (
            u32::from(tile.0),
            u32::from(tile.1),
            u32::from(size.0),
            u32::from(size.1),
        );
        ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah
    }
}

/// The banner and Seat flag entities each building carries.
#[derive(Resource, Default, Debug)]
pub struct BuildingExtras(pub HashMap<BuildingId, Extras>);

/// One building's extra entities.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Extras {
    /// Its team banner.
    pub banner: Option<Entity>,
    /// Its Seat flag.
    pub flag: Option<Entity>,
}

/// One shared unit cube and one material per `(team, site)`.
#[derive(Resource, Default)]
pub struct BuildingVisuals {
    cube: Option<Handle<Mesh>>,
    materials: HashMap<(u8, bool), Handle<StandardMaterial>>,
}

impl BuildingVisuals {
    /// The mesh a building of a kind whose visual is `visual` is drawn
    /// with when it is a box: the shared unit cube
    /// ([`BuildingVisuals::cube`]) for `Primitive(Cuboid)`, for a `Scene`
    /// whose model is not ready (its fallback is the box) and for a kind
    /// with no visual.
    pub fn mesh(&mut self, visual: Option<&Visual>, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        match visual {
            None
            | Some(Visual::Primitive(Primitive::Cuboid))
            | Some(Visual::Scene { .. })
            | Some(Visual::Primitive(_))
            | Some(Visual::Body { .. }) => self.cube(meshes),
        }
    }

    /// The unit cube every box building scales, its base on the ground.
    pub fn cube(&mut self, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        self.cube
            .get_or_insert_with(|| {
                let mut mesh = Mesh::from(Cuboid::new(1.0, 1.0, 1.0));
                mesh.translate_by(Vec3::Y * 0.5);
                meshes.add(mesh)
            })
            .clone()
    }

    /// The matte box material for `team`, in scaffold colour for a site.
    pub fn material(
        &mut self,
        team: u8,
        site: bool,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        self.materials
            .entry((team, site))
            .or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: building_color(team, site),
                    perceptual_roughness: MATTE_ROUGHNESS,
                    reflectance: MATTE_REFLECTANCE,
                    metallic: 0.0,
                    ..default()
                })
            })
            .clone()
    }
}

/// A complete building is its team colour; a site is that colour blended
/// toward stone ([`SITE_BLEND`]).
pub fn building_color(team: u8, site: bool) -> Color {
    let team = palette::team_color(team);
    if site {
        team.mix(&palette::STONE, SITE_BLEND)
    } else {
        team
    }
}

/// The centre of `b`'s footprint on the ground, world metres.
pub fn footprint_centre(rules: &Rules, b: &Building) -> Vec3 {
    let kind = rules
        .building_kind(b.kind.0)
        .expect("a building's kind is in the rules");
    let x = f32::from(b.tile.x) + kind.width_tiles as f32 * 0.5 - HALF_EXTENT;
    let z = f32::from(b.tile.y) + kind.height_tiles as f32 * 0.5 - HALF_EXTENT;
    Vec3::new(x, 0.0, z)
}

/// The box transform of `b` (the M3b look): its footprint's centre on the
/// ground, scaled to the footprint and to its height (a site by its
/// progress).
pub fn building_transform(rules: &Rules, b: &Building) -> Transform {
    let kind = rules
        .building_kind(b.kind.0)
        .expect("a building's kind is in the rules");
    let (w, h) = (kind.width_tiles as f32, kind.height_tiles as f32);
    let full = building_height(kind.width_tiles, kind.height_tiles);
    let height = match b.construction {
        Construction::Complete => full,
        Construction::Site { .. } => full * site_fraction(rules, b).clamp(SITE_MIN_FRACTION, 1.0),
    };
    Transform::from_translation(footprint_centre(rules, b)).with_scale(Vec3::new(w, height, h))
}

/// How far a site is built, 0..=1 (1 for a complete building).
pub fn site_fraction(rules: &Rules, b: &Building) -> f32 {
    match b.construction {
        Construction::Complete => 1.0,
        Construction::Site { progress } => {
            let total = rules
                .building_kind(b.kind.0)
                .map_or(1, |k| rules.build_time_ticks(k).max(1));
            progress as f32 / total as f32
        }
    }
}

/// Which of `stages` site looks a site at `progress` of `total` ticks
/// draws: `min(progress * stages / total, stages - 1)`, so each look
/// covers an equal share of the build.
pub fn site_stage(progress: u32, total: u32, stages: usize) -> usize {
    let stages = stages.max(1);
    let n = u64::try_from(stages).unwrap_or(u64::MAX);
    let at = u64::from(progress).saturating_mul(n) / u64::from(total.max(1));
    usize::try_from(at).unwrap_or(usize::MAX).min(stages - 1)
}

/// The visual `b` is drawn with now: its kind's for a complete building,
/// the current site stage's for a site.
pub fn current_visual<'a>(rules: &'a Rules, b: &Building) -> Option<&'a Visual> {
    match b.construction {
        Construction::Complete => rules.building_visual(b.kind.0),
        Construction::Site { progress } => {
            let sites = &rules.visuals.sites;
            let total = rules
                .building_kind(b.kind.0)
                .map_or(1, |k| rules.build_time_ticks(k));
            sites.get(site_stage(progress, total, sites.len()))
        }
    }
}

/// A site model's uniform scale: the footprint's side (the shorter one,
/// should a footprint ever not be square).
pub fn site_scale(rules: &Rules, b: &Building) -> f32 {
    rules
        .building_kind(b.kind.0)
        .map_or(1.0, |k| k.width_tiles.min(k.height_tiles) as f32)
}

/// The ready model `b` is drawn as, if any, with its entity scale.
fn ready_model<'a>(
    rules: &Rules,
    b: &Building,
    models: Option<&'a ModelLibrary>,
) -> Option<(&'a Model, f32)> {
    let model = models?.model(current_visual(rules, b)?)?;
    let scale = match b.construction {
        Construction::Complete => 1.0,
        Construction::Site { .. } => site_scale(rules, b),
    };
    Some((model, scale))
}

/// Height above the ground of the top of `b` as drawn, metres: its model's
/// top (a site's stage model times its footprint scale), or the M3b box's
/// height (a site's times its progress). Health bars ride on it and the
/// attack marker sits on it (`orders::target_point`).
pub fn building_top_y(rules: &Rules, b: &Building, models: Option<&ModelLibrary>) -> f32 {
    match ready_model(rules, b, models) {
        Some((model, scale)) => model.top_y() * scale,
        None => building_transform(rules, b).scale.y,
    }
}

/// `T(centre + offset) * R(yaw) * S(scale)`: where a banner or the Seat
/// flag stands on a building whose footprint centre is `centre` (the
/// placement `tools/inspect_gltf check` measures).
pub fn mount_transform(centre: Vec3, mount: &Mount) -> Transform {
    let (x, y, z) = mount.offset_x100;
    Transform::from_translation(centre + Vec3::new(x as f32, y as f32, z as f32) / 100.0)
        .with_rotation(Quat::from_rotation_y((mount.yaw_deg as f32).to_radians()))
        .with_scale(Vec3::splat(mount.scale_x100 as f32 / 100.0))
}

/// How a building, a banner or a flag is drawn this tick.
#[derive(Debug, Clone, PartialEq)]
pub struct Drawn {
    /// Its mesh.
    pub mesh: Handle<Mesh>,
    /// Its material.
    pub material: Handle<StandardMaterial>,
    /// Its world transform.
    pub transform: Transform,
}

/// The asset stores a building is drawn from.
#[derive(SystemParam)]
pub struct DrawAssets<'w> {
    /// Meshes.
    pub meshes: ResMut<'w, Assets<Mesh>>,
    /// Materials.
    pub materials: ResMut<'w, Assets<StandardMaterial>>,
    /// Images (team textures are made from them); absent in tests.
    pub images: Option<ResMut<'w, Assets<Image>>>,
    /// The models; absent in headless runs and tests.
    pub models: Option<ResMut<'w, ModelLibrary>>,
}

impl DrawAssets<'_> {
    /// The material for `model` for `team` (a `(swatch key, slot)`), or
    /// neutral; `None` without images or a library.
    fn model_material(
        &mut self,
        model: &Model,
        team: Option<(&str, u8)>,
    ) -> Option<Handle<StandardMaterial>> {
        let images = self.images.as_mut()?;
        let lib = self.models.as_mut()?;
        Some(lib.material(model, team, images, &mut self.materials))
    }

    /// The swatch key of a Scene `visual`, if it names one.
    fn swatch(visual: Option<&Visual>) -> Option<&str> {
        match visual {
            Some(Visual::Scene { team, .. }) => team.as_deref(),
            _ => None,
        }
    }

    /// How `b` is drawn: its ready model (complete: owner's team material,
    /// scale 1; site: neutral, scaled by the footprint side), else the M3b
    /// box.
    pub fn building(&mut self, rules: &Rules, b: &Building, boxes: &mut BuildingVisuals) -> Drawn {
        let centre = footprint_centre(rules, b);
        let site = matches!(b.construction, Construction::Site { .. });
        let visual = current_visual(rules, b);
        let ready = ready_model(rules, b, self.models.as_deref()).map(|(m, s)| (m.clone(), s));
        if let Some((model, scale)) = ready {
            let team = if site {
                None
            } else {
                Self::swatch(visual).map(|k| (k, b.owner.0))
            };
            if let Some(material) = self.model_material(&model, team) {
                return Drawn {
                    mesh: model.mesh.clone(),
                    material,
                    transform: Transform::from_translation(centre).with_scale(Vec3::splat(scale)),
                };
            }
        }
        Drawn {
            mesh: boxes.mesh(rules.building_visual(b.kind.0), &mut self.meshes),
            material: boxes.material(b.owner.0, site, &mut self.materials),
            transform: building_transform(rules, b),
        }
    }

    /// The team banner of `b`: a complete building drawn as its model (the
    /// mounts are measured on the model; the M3b box, team-coloured all
    /// over, carries none) whose Scene names a `banner_at`, with the banner
    /// model ready.
    pub fn banner(&mut self, rules: &Rules, b: &Building) -> Option<Drawn> {
        if !matches!(b.construction, Construction::Complete) {
            return None;
        }
        ready_model(rules, b, self.models.as_deref())?;
        let Some(Visual::Scene {
            banner_at: Some(mount),
            ..
        }) = rules.building_visual(b.kind.0)
        else {
            return None;
        };
        let banner = rules.visuals.banner.as_ref()?;
        let model = self.models.as_deref()?.model(banner)?.clone();
        let team = Self::swatch(Some(banner)).map(|k| (k, b.owner.0));
        Some(Drawn {
            mesh: model.mesh.clone(),
            material: self.model_material(&model, team)?,
            transform: mount_transform(footprint_centre(rules, b), mount),
        })
    }

    /// The Seat flag of `b`: a complete Seat, with the flag model ready; at
    /// its Scene's `seat_flag_at` while the building is drawn as that
    /// model, else (no mount, or the M3b box of a failed model, which has
    /// no keep to stand on) on the ground at the footprint's north-east
    /// corner. It is the only thing that tells a Seat's box from another
    /// Town's, so the box keeps it.
    pub fn seat_flag(&mut self, rules: &Rules, b: &Building) -> Option<Drawn> {
        if !matches!(b.construction, Construction::Complete)
            || !b.town_state.as_ref().is_some_and(|t| t.seat)
        {
            return None;
        }
        let flag = rules.visuals.seat_flag.as_ref()?;
        let model = self.models.as_deref()?.model(flag)?.clone();
        let centre = footprint_centre(rules, b);
        let on_model = ready_model(rules, b, self.models.as_deref()).is_some();
        let transform = match rules.building_visual(b.kind.0) {
            Some(Visual::Scene {
                seat_flag_at: Some(mount),
                ..
            }) if on_model => mount_transform(centre, mount),
            _ => {
                let kind = rules.building_kind(b.kind.0)?;
                let corner = Vec3::new(
                    kind.width_tiles as f32 * 0.5,
                    0.0,
                    -(kind.height_tiles as f32) * 0.5,
                );
                Transform::from_translation(centre + corner)
            }
        };
        let team = Self::swatch(Some(flag)).map(|k| (k, b.owner.0));
        Some(Drawn {
            mesh: model.mesh.clone(),
            material: self.model_material(&model, team)?,
            transform,
        })
    }
}

/// Mirrors sim buildings into entities.
pub struct BuildingsPlugin;

impl Plugin for BuildingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BuildingEntities>()
            .init_resource::<BuildingVisuals>()
            .init_resource::<BuildingExtras>()
            .add_systems(
                FixedUpdate,
                (spawn_rubble, sync_buildings)
                    .chain()
                    .after(SimSystems::Step)
                    .run_if(sim_stepped),
            )
            .add_systems(Update, expire_rubble);
    }
}

/// After every tick: spawn an entity for each new building (removing any
/// rubble on its footprint), update the mesh, material and transform of
/// each known one (a site moves through its stages, then becomes its
/// model; an annexation recolours it), keep its banner and Seat flag in
/// step, and despawn the entities of buildings that are gone.
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn sync_buildings(
    sim: NonSend<SimHandle>,
    mut entities: ResMut<BuildingEntities>,
    mut extras: ResMut<BuildingExtras>,
    mut boxes: ResMut<BuildingVisuals>,
    mut assets: DrawAssets,
    mut shown: Shown,
    rubble: Query<(Entity, &Rubble)>,
    mut commands: Commands,
) {
    let view = sim.view();
    let rules = sim.rules();
    let mut count = 0usize;
    for b in view.buildings() {
        count += 1;
        let drawn = assets.building(rules, b, &mut boxes);
        match entities.0.get(&b.id) {
            Some(entity) => update_drawn(&mut shown, *entity, &drawn),
            None => {
                let kind = rules.building_kind(b.kind.0);
                if let Some(kind) = kind {
                    let size = (
                        u16::try_from(kind.width_tiles).unwrap_or(u16::MAX),
                        u16::try_from(kind.height_tiles).unwrap_or(u16::MAX),
                    );
                    for (e, r) in &rubble {
                        if r.overlaps((b.tile.x, b.tile.y), size) {
                            commands.entity(e).despawn();
                        }
                    }
                }
                let name = kind.map_or("Building", |k| &k.name);
                let entity = commands
                    .spawn((
                        Name::new(format!("{name} {}", b.id.0)),
                        BuildingRef(b.id),
                        Mesh3d(drawn.mesh),
                        MeshMaterial3d(drawn.material),
                        drawn.transform,
                        Pickable::default(),
                    ))
                    .id();
                entities.0.insert(b.id, entity);
            }
        }
        let want_banner = assets.banner(rules, b);
        let want_flag = assets.seat_flag(rules, b);
        let slot = extras.0.entry(b.id).or_default();
        reconcile(
            &mut commands,
            &mut shown,
            &mut slot.banner,
            want_banner,
            || (Name::new(format!("Banner {}", b.id.0)), Banner(b.id)),
        );
        reconcile(&mut commands, &mut shown, &mut slot.flag, want_flag, || {
            (Name::new(format!("Seat flag {}", b.id.0)), SeatFlag(b.id))
        });
    }
    if entities.0.len() > count {
        entities.0.retain(|id, entity| {
            if view.building(*id).is_some() {
                true
            } else {
                commands.entity(*entity).despawn();
                false
            }
        });
    }
    if extras.0.len() > count {
        extras.0.retain(|id, slot| {
            if view.building(*id).is_some() {
                true
            } else {
                for e in [slot.banner, slot.flag].into_iter().flatten() {
                    commands.entity(e).despawn();
                }
                false
            }
        });
    }
}

/// The drawn parts of buildings, banners and flags.
type Shown<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut Mesh3d,
        &'static mut MeshMaterial3d<StandardMaterial>,
    ),
    Without<Rubble>,
>;

/// Write `drawn` to `entity`, touching only what changed.
fn update_drawn(shown: &mut Shown<'_, '_>, entity: Entity, drawn: &Drawn) {
    if let Ok((mut t, mut mesh, mut material)) = shown.get_mut(entity) {
        if *t != drawn.transform {
            *t = drawn.transform;
        }
        if mesh.0 != drawn.mesh {
            mesh.0 = drawn.mesh.clone();
        }
        if material.0 != drawn.material {
            material.0 = drawn.material.clone();
        }
    }
}

/// Bring one extra entity (a banner or a flag) in line with `want`:
/// spawn it (top level, no shadow, not pickable), update it, or despawn it.
fn reconcile<B: Bundle>(
    commands: &mut Commands,
    shown: &mut Shown<'_, '_>,
    slot: &mut Option<Entity>,
    want: Option<Drawn>,
    bundle: impl FnOnce() -> B,
) {
    match (want, *slot) {
        (Some(drawn), Some(e)) => update_drawn(shown, e, &drawn),
        (Some(drawn), None) => {
            *slot = Some(
                commands
                    .spawn((
                        bundle(),
                        Mesh3d(drawn.mesh),
                        MeshMaterial3d(drawn.material),
                        drawn.transform,
                        NotShadowCaster,
                    ))
                    .id(),
            );
        }
        (None, Some(e)) => {
            commands.entity(e).despawn();
            *slot = None;
        }
        (None, None) => {}
    }
}

/// After a tick, before [`sync_buildings`] despawns them: rubble where a
/// building or site was destroyed (`SimEvent::BuildingDestroyed`), at its
/// entity's footprint centre and scaled by its footprint side, for
/// `rubble.ds` of real time. Nothing without a ready rubble model.
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn spawn_rubble(
    sim: NonSend<SimHandle>,
    mut events: MessageReader<SimEventMsg>,
    entities: Res<BuildingEntities>,
    transforms: Query<&Transform, With<BuildingRef>>,
    mut assets: DrawAssets,
    time: Option<Res<Time<Real>>>,
    mut commands: Commands,
) {
    let rules = sim.rules();
    let destroyed: Vec<(BuildingId, sim::BuildingKindId)> = events
        .read()
        .filter_map(|m| match m.event {
            SimEvent::BuildingDestroyed { building, kind, .. } => Some((building, kind)),
            _ => None,
        })
        .collect();
    if destroyed.is_empty() {
        return;
    }
    let Some(rubble) = rules.visuals.rubble.as_ref() else {
        return;
    };
    let Some(model) = assets
        .models
        .as_deref()
        .and_then(|m| m.model(&rubble.visual))
        .cloned()
    else {
        return;
    };
    let Some(material) = assets.model_material(&model, None) else {
        return;
    };
    let now = time.map_or(0.0, |t| t.elapsed_secs_f64());
    for (building, kind) in destroyed {
        let (Some(entity), Some(kind)) = (entities.0.get(&building), rules.building_kind(kind.0))
        else {
            continue;
        };
        let Ok(t) = transforms.get(*entity) else {
            continue;
        };
        // The footprint's north-west tile from its centre.
        let nw = |centre: f32, side: u32| {
            let tile = (centre + HALF_EXTENT - side as f32 * 0.5).round();
            u16::try_from(tile as i64).unwrap_or(0)
        };
        let side = kind.width_tiles.min(kind.height_tiles) as f32;
        commands.spawn((
            Name::new(format!("Rubble {}", building.0)),
            Rubble {
                until: now + f64::from(rubble.ds) / 10.0,
                tile: (
                    nw(t.translation.x, kind.width_tiles),
                    nw(t.translation.z, kind.height_tiles),
                ),
                size: (
                    u16::try_from(kind.width_tiles).unwrap_or(u16::MAX),
                    u16::try_from(kind.height_tiles).unwrap_or(u16::MAX),
                ),
            },
            Mesh3d(model.mesh.clone()),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(Vec3::new(t.translation.x, 0.0, t.translation.z))
                .with_scale(Vec3::splat(side)),
        ));
    }
}

/// Every frame: despawn rubble whose time is up.
pub fn expire_rubble(
    time: Res<Time<Real>>,
    rubble: Query<(Entity, &Rubble)>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs_f64();
    for (e, r) in &rubble {
        if now >= r.until {
            commands.entity(e).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Geometry, ModelKey, content_hash, finish_geometry};
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    use sim::{BuildingKindId, Hold, PlayerId, Tile, TownState};

    fn seat_state() -> Option<TownState> {
        Some(TownState {
            seat: true,
            tier: 1,
            hold: Hold::Held,
        })
    }

    fn rules() -> Rules {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        Rules::load(&data).expect("data loads")
    }

    fn town(rules: &Rules) -> u16 {
        rules.town_kind()
    }

    fn building(kind: u16, construction: Construction) -> Building {
        Building {
            id: BuildingId(1),
            owner: PlayerId(0),
            kind: BuildingKindId(kind),
            tile: Tile::new(23, 63),
            hp: 1,
            construction,
            paid: Vec::new(),
            town: None,
            town_state: None,
            slots: 0,
            queue: Vec::new(),
            rally: None,
            ready_at: 0,
        }
    }

    fn scene(path: &str) -> Visual {
        Visual::Scene {
            path: path.into(),
            scale_x100: 100,
            y_offset_tiles_x100: 0,
            yaw_deg: 0,
            x_offset_tiles_x100: 0,
            z_offset_tiles_x100: 0,
            nodes: Vec::new(),
            parts: Vec::new(),
            team: None,
            fallback: Some(Box::new(Visual::Primitive(Primitive::Cuboid))),
            banner_at: None,
            seat_flag_at: None,
        }
    }

    fn with_team(v: Visual, key: &str) -> Visual {
        match v {
            Visual::Scene {
                path,
                scale_x100,
                y_offset_tiles_x100,
                yaw_deg,
                x_offset_tiles_x100,
                z_offset_tiles_x100,
                nodes,
                parts,
                fallback,
                banner_at,
                seat_flag_at,
                ..
            } => Visual::Scene {
                path,
                scale_x100,
                y_offset_tiles_x100,
                yaw_deg,
                x_offset_tiles_x100,
                z_offset_tiles_x100,
                nodes,
                parts,
                team: Some(key.into()),
                fallback,
                banner_at,
                seat_flag_at,
            },
            other => other,
        }
    }

    #[test]
    fn a_complete_town_covers_its_footprint() {
        let rules = rules();
        let t = building_transform(&rules, &building(town(&rules), Construction::Complete));
        // Footprint (23..=25, 63..=65): centre (24.5, 64.5) in tiles.
        assert_eq!(t.translation, Vec3::new(24.5 - 64.0, 0.0, 64.5 - 64.0));
        assert_eq!(t.scale, Vec3::new(3.0, building_height(3, 3), 3.0));
    }

    #[test]
    fn a_site_rises_with_progress_from_a_visible_floor() {
        let rules = rules();
        let kind = town(&rules);
        let total = rules.build_time_ticks(rules.building_kind(kind).unwrap());
        let full = building_height(3, 3);
        let at = |progress| {
            building_transform(&rules, &building(kind, Construction::Site { progress }))
                .scale
                .y
        };
        assert_eq!(at(0), full * SITE_MIN_FRACTION);
        assert!((at(total / 2) - full * 0.5).abs() < 1e-4);
        assert_eq!(at(total), full);
    }

    /// Art pass: every kind's `visuals.ron` entry is read, and a
    /// `Primitive(Cuboid)`, or a `Scene` whose model is not loaded, keeps
    /// the M3b box: the one shared unit cube, scaled to the footprint and
    /// height. (Since slice 3 the test sets the Town's visual itself
    /// instead of reading it from `visuals.ron`, whose Towns are models:
    /// a look edit must never fail a Rust test, art-pass decision 17.)
    #[test]
    fn cuboid_visual_keeps_the_m3b_box() {
        use bevy::camera::primitives::MeshAabb;
        let mut rules = rules();
        for i in 0..rules.buildings.len() {
            let kind = u16::try_from(i).unwrap();
            assert!(rules.building_visual(kind).is_some(), "kind {kind}");
        }
        let mut meshes = Assets::<Mesh>::default();
        let mut visuals = BuildingVisuals::default();
        let cube = visuals.cube(&mut meshes);
        let town = town(&rules);
        rules
            .visuals
            .buildings
            .insert("town".into(), Visual::Primitive(Primitive::Cuboid));
        assert_eq!(
            rules.building_visual(town),
            Some(&Visual::Primitive(Primitive::Cuboid))
        );
        assert_eq!(visuals.mesh(rules.building_visual(town), &mut meshes), cube);
        let scene = scene("models/castle.gltf");
        rules.visuals.buildings.insert("town".into(), scene.clone());
        assert_eq!(rules.building_visual(town), Some(&scene));
        assert_eq!(visuals.mesh(rules.building_visual(town), &mut meshes), cube);
        assert_eq!(visuals.mesh(None, &mut meshes), cube);
        assert_eq!(meshes.len(), 1, "one shared cube");
        // The cube stands on the ground, one metre a side, so the
        // transform's scale is the drawn size.
        let aabb = meshes.get(&cube).unwrap().compute_aabb().unwrap();
        assert_eq!(aabb.min().y, 0.0);
        assert_eq!(Vec3::from(aabb.half_extents) * 2.0, Vec3::ONE);
        let t = building_transform(&rules, &building(town, Construction::Complete));
        assert_eq!(t.scale, Vec3::new(3.0, building_height(3, 3), 3.0));
        // Without a model library the Scene draws that box too.
        assert_eq!(
            building_top_y(&rules, &building(town, Construction::Complete), None),
            building_height(3, 3)
        );
    }

    #[test]
    fn sites_and_buildings_differ_in_colour_per_team() {
        assert_ne!(building_color(0, false), building_color(0, true));
        assert_ne!(building_color(0, false), building_color(1, false));
        assert_eq!(building_color(1, false), palette::team_color(1));
    }

    /// An 8x8 white texture with a team rect at (0, 0, 4, 4).
    fn atlas() -> Image {
        Image::new(
            Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![200; 8 * 8 * 4],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        )
    }

    /// A ready model `top` m tall on the shared test atlas, as a library
    /// entry for `visual`.
    fn ready(
        lib: &mut ModelLibrary,
        visual: &Visual,
        top: f32,
        meshes: &mut Assets<Mesh>,
        images: &mut Assets<Image>,
    ) -> Model {
        let image = atlas();
        let hash = content_hash(&image);
        let handle = images.add(image);
        let model = finish_geometry(
            Geometry {
                positions: vec![[-0.5, 0.0, -0.5], [0.5, top, 0.5], [0.0, 0.0, 0.5]],
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

    /// A rules set whose Town and Croft are models with a banner, plus
    /// three site stages, rubble, the Seat flag and the banner model, and
    /// a library with every one of them ready.
    struct Fixture {
        rules: Rules,
        lib: ModelLibrary,
        meshes: Assets<Mesh>,
        images: Assets<Image>,
        materials: Assets<StandardMaterial>,
        town: Model,
        stages: Vec<Model>,
        banner: Model,
        flag: Model,
        rubble: Model,
    }

    fn fixture() -> Fixture {
        let mut rules = rules();
        // Every kind the box first, so no test reads the live looks
        // (art-pass decision 17: a look edit never fails a Rust test).
        for visual in rules.visuals.buildings.values_mut() {
            *visual = Visual::Primitive(Primitive::Cuboid);
        }
        rules.visuals.team_swatches.insert(
            "hex".into(),
            rules::TeamSwatch {
                rects: vec![(0, 0, 4, 4)],
                desaturate_permille: 0,
            },
        );
        let mount = |x, y, z, scale| Mount {
            offset_x100: (x, y, z),
            scale_x100: scale,
            yaw_deg: 90,
        };
        let castle = match with_team(scene("m/castle.gltf"), "hex") {
            Visual::Scene {
                path,
                scale_x100,
                y_offset_tiles_x100,
                yaw_deg,
                x_offset_tiles_x100,
                z_offset_tiles_x100,
                nodes,
                parts,
                team,
                fallback,
                ..
            } => Visual::Scene {
                path,
                scale_x100,
                y_offset_tiles_x100,
                yaw_deg,
                x_offset_tiles_x100,
                z_offset_tiles_x100,
                nodes,
                parts,
                team,
                fallback,
                banner_at: Some(mount(100, 20, 120, 300)),
                seat_flag_at: Some(mount(0, 400, -50, 600)),
            },
            _ => unreachable!(),
        };
        rules
            .visuals
            .buildings
            .insert("town".into(), castle.clone());
        rules.visuals.sites = vec![
            scene("m/stage_A.gltf"),
            scene("m/stage_B.gltf"),
            scene("m/stage_C.gltf"),
        ];
        rules.visuals.rubble = Some(rules::Rubble {
            visual: scene("m/rubble.gltf"),
            ds: 300,
        });
        let flag = with_team(scene("m/flag.gltf"), "hex");
        rules.visuals.banner = Some(flag.clone());
        rules.visuals.seat_flag = Some(with_team(scene("m/seat_flag.gltf"), "hex"));
        let (mut meshes, mut images) = (Assets::default(), Assets::default());
        let mut lib = ModelLibrary::new(&rules.visuals);
        let town = ready(&mut lib, &castle, 4.0, &mut meshes, &mut images);
        let stages = rules
            .visuals
            .sites
            .clone()
            .iter()
            .enumerate()
            .map(|(i, v)| ready(&mut lib, v, 0.3 * (i + 1) as f32, &mut meshes, &mut images))
            .collect();
        let banner = ready(&mut lib, &flag, 0.28, &mut meshes, &mut images);
        let seat = rules.visuals.seat_flag.clone().unwrap();
        let flag = ready(&mut lib, &seat, 0.28, &mut meshes, &mut images);
        let rubble_v = rules.visuals.rubble.clone().unwrap().visual;
        let rubble = ready(&mut lib, &rubble_v, 0.9, &mut meshes, &mut images);
        Fixture {
            rules,
            lib,
            meshes,
            images,
            materials: Assets::default(),
            town,
            stages,
            banner,
            flag,
            rubble,
        }
    }

    /// `v` (a Scene) with these mounts.
    fn with_mounts(v: Visual, banner: Option<Mount>, seat_flag: Option<Mount>) -> Visual {
        match v {
            Visual::Scene {
                path,
                scale_x100,
                y_offset_tiles_x100,
                yaw_deg,
                x_offset_tiles_x100,
                z_offset_tiles_x100,
                nodes,
                parts,
                team,
                fallback,
                ..
            } => Visual::Scene {
                path,
                scale_x100,
                y_offset_tiles_x100,
                yaw_deg,
                x_offset_tiles_x100,
                z_offset_tiles_x100,
                nodes,
                parts,
                team,
                fallback,
                banner_at: banner,
                seat_flag_at: seat_flag,
            },
            other => other,
        }
    }

    /// The fixture with its Croft drawn as a model too (the castle's mesh,
    /// so it is ready) carrying a banner.
    fn croft_with_banner(f: &mut Fixture) {
        let croft = with_mounts(
            with_team(scene("m/castle.gltf"), "hex"),
            Some(Mount {
                offset_x100: (50, 0, 50),
                scale_x100: 200,
                yaw_deg: 0,
            }),
            None,
        );
        f.rules.visuals.buildings.insert("croft".into(), croft);
    }

    /// An app that runs [`BuildingsPlugin`] on the scenario `name` with
    /// the fixture's rules and ready models, one tick per update after the
    /// first.
    fn models_app(f: Fixture, name: &str) -> App {
        let (setup, stream, _) = sim::scenarios::by_name(name, &f.rules).expect("a scenario");
        models_app_with(f, setup, stream)
    }

    /// [`models_app`] on a hand-written match.
    fn models_app_with(f: Fixture, setup: sim::MatchSetup, stream: sim::scenarios::Stream) -> App {
        use crate::sim_driver::{ScenarioInput, SimPlugin};
        use bevy::time::TimeUpdateStrategy;
        let handle = SimHandle::from_setup(setup, f.rules);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(
                std::time::Duration::ZERO,
            ))
            .insert_resource(f.meshes)
            .insert_resource(f.images)
            .insert_resource(f.materials)
            .insert_resource(f.lib)
            .add_plugins(SimPlugin::new(handle, Box::new(ScenarioInput::new(stream))))
            .add_plugins(BuildingsPlugin);
        app.update();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_millis(50),
        ));
        app
    }

    /// Update until the sim has stepped `n` more ticks (the scenario's
    /// tick-0 commands apply after the command delay).
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

    fn entity_of(app: &App, id: BuildingId) -> Entity {
        *app.world()
            .resource::<BuildingEntities>()
            .0
            .get(&id)
            .expect("the building has an entity")
    }

    #[test]
    fn a_complete_building_draws_its_team_model() {
        let f = fixture();
        let town_mesh = f.town.mesh.clone();
        let mut app = models_app(f, "art_bench");
        run_ticks(&mut app, 4);
        let seats: Vec<(BuildingId, PlayerId)> = {
            let sim = app.world().non_send::<SimHandle>();
            let view = sim.view();
            view.buildings()
                .filter(|b| b.town_state.as_ref().is_some_and(|t| t.seat))
                .map(|b| (b.id, b.owner))
                .collect()
        };
        assert_eq!(seats.len(), 2, "both Seats");
        let mut materials = Vec::new();
        for (id, _) in &seats {
            let e = entity_of(&app, *id);
            let world = app.world();
            assert_eq!(
                world.get::<Mesh3d>(e).unwrap().0,
                town_mesh,
                "the castle model"
            );
            let t = world.get::<Transform>(e).unwrap();
            assert_eq!(t.scale, Vec3::ONE, "a complete model is drawn at scale 1");
            assert_eq!(t.translation.y, 0.0);
            assert!(world.get::<Pickable>(e).is_some());
            materials.push(
                world
                    .get::<MeshMaterial3d<StandardMaterial>>(e)
                    .unwrap()
                    .0
                    .clone(),
            );
        }
        assert_ne!(materials[0], materials[1], "one team material each");
        let mats = app.world().resource::<Assets<StandardMaterial>>();
        assert!(
            mats.get(&materials[0])
                .unwrap()
                .base_color_texture
                .is_some()
        );
        // A kind whose visual is the box (the fixture's Crofts are not
        // models) keeps the shared cube.
        let croft = {
            let sim = app.world().non_send::<SimHandle>();
            let rules = sim.rules();
            sim.view()
                .buildings()
                .find(|b| {
                    rules
                        .building_kind(b.kind.0)
                        .is_some_and(|k| k.id == "croft")
                })
                .map(|b| b.id)
                .expect("a Croft")
        };
        let cube = app
            .world_mut()
            .resource_mut::<BuildingVisuals>()
            .cube
            .clone();
        let e = entity_of(&app, croft);
        assert_eq!(Some(app.world().get::<Mesh3d>(e).unwrap().0.clone()), cube);
    }

    #[test]
    fn site_stage_follows_progress_thirds() {
        assert_eq!(site_stage(0, 300, 3), 0);
        assert_eq!(site_stage(99, 300, 3), 0);
        assert_eq!(site_stage(100, 300, 3), 1);
        assert_eq!(site_stage(199, 300, 3), 1);
        assert_eq!(site_stage(200, 300, 3), 2);
        assert_eq!(site_stage(300, 300, 3), 2, "never past the last stage");
        assert_eq!(site_stage(5, 300, 1), 0);
        assert_eq!(site_stage(5, 0, 3), 2, "a zero build time is complete");
        assert_eq!(site_stage(u32::MAX, 1, 4), 3, "no overflow");
        // On the fixture: a third-built site draws stage B, scaled by its
        // footprint side, in the neutral material.
        let mut f = fixture();
        let kind = town(&f.rules);
        let total = f
            .rules
            .build_time_ticks(f.rules.building_kind(kind).unwrap());
        let mut boxes = BuildingVisuals::default();
        let mut images = std::mem::take(&mut f.images);
        let mut materials = Assets::<StandardMaterial>::default();
        let stage_b = f.stages[1].mesh.clone();
        let mut world = World::new();
        world.insert_resource(std::mem::take(&mut f.meshes));
        world.insert_resource(std::mem::take(&mut materials));
        world.insert_resource(std::mem::take(&mut images));
        world.insert_resource(std::mem::take(&mut f.lib));
        let rules = f.rules;
        let drawn = world
            .run_system_once(move |mut assets: DrawAssets| {
                let b = building(
                    kind,
                    Construction::Site {
                        progress: total / 3 + 1,
                    },
                );
                let site = assets.building(&rules, &b, &mut boxes);
                let done =
                    assets.building(&rules, &building(kind, Construction::Complete), &mut boxes);
                (
                    site,
                    done,
                    building_top_y(&rules, &b, assets.models.as_deref()),
                )
            })
            .unwrap();
        let (site, done, top) = drawn;
        assert_eq!(site.mesh, stage_b);
        assert_eq!(
            site.transform.scale,
            Vec3::splat(3.0),
            "scaled by the 3-tile side"
        );
        assert_ne!(
            site.material, done.material,
            "sites are neutral, buildings team"
        );
        assert!(
            (top - 0.6 * 3.0).abs() < 1e-5,
            "stage B's top times the side: {top}"
        );
    }

    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn an_annexed_building_takes_the_new_owners_material() {
        let mut f = fixture();
        let kind = town(&f.rules);
        let mut boxes = BuildingVisuals::default();
        let mut world = World::new();
        world.insert_resource(std::mem::take(&mut f.meshes));
        world.insert_resource(Assets::<StandardMaterial>::default());
        world.insert_resource(std::mem::take(&mut f.images));
        world.insert_resource(std::mem::take(&mut f.lib));
        let rules = f.rules;
        let (a, b, banner_a, banner_b, flag) = world
            .run_system_once(move |mut assets: DrawAssets| {
                let mut seat = building(kind, Construction::Complete);
                seat.town_state = seat_state();
                let a = assets.building(&rules, &seat, &mut boxes);
                let banner_a = assets.banner(&rules, &seat).unwrap();
                seat.owner = PlayerId(1);
                let b = assets.building(&rules, &seat, &mut boxes);
                let banner_b = assets.banner(&rules, &seat).unwrap();
                let flag = assets.seat_flag(&rules, &seat).unwrap();
                (a, b, banner_a, banner_b, flag)
            })
            .unwrap();
        assert_eq!(a.mesh, b.mesh, "the same model");
        assert_ne!(a.material, b.material, "the new owner's material");
        assert_eq!(banner_a.mesh, banner_b.mesh, "one shared banner mesh");
        assert_eq!(
            banner_b.material, b.material,
            "the banner shares the building's team material"
        );
        assert_ne!(banner_a.material, banner_b.material);
        assert_eq!(flag.material, b.material, "so does the Seat flag");
    }

    #[test]
    fn building_top_y_follows_the_model() {
        let mut f = fixture();
        let kind = town(&f.rules);
        let complete = building(kind, Construction::Complete);
        assert_eq!(building_top_y(&f.rules, &complete, Some(&f.lib)), 4.0);
        assert_eq!(
            building_top_y(&f.rules, &complete, None),
            building_height(3, 3),
            "no library: the box"
        );
        // A failed model draws the box, so the top is the box's.
        let castle = f.rules.building_visual(kind).unwrap().clone();
        f.lib.fail(&ModelKey::of(&castle).unwrap(), "test");
        assert_eq!(
            building_top_y(&f.rules, &complete, Some(&f.lib)),
            building_height(3, 3)
        );
        assert!(f.lib.errors_logged() == 1);
    }

    #[test]
    fn banners_and_the_seat_flag_stand_at_their_mounts() {
        let mut f = fixture();
        let kind = town(&f.rules);
        let (banner_mesh, flag_mesh) = (f.banner.mesh.clone(), f.flag.mesh.clone());
        let mut world = World::new();
        world.insert_resource(std::mem::take(&mut f.meshes));
        world.insert_resource(Assets::<StandardMaterial>::default());
        world.insert_resource(std::mem::take(&mut f.images));
        world.insert_resource(std::mem::take(&mut f.lib));
        let rules = f.rules;
        let centre = footprint_centre(&rules, &building(kind, Construction::Complete));
        let out = world
            .run_system_once(move |mut assets: DrawAssets| {
                let mut b = building(kind, Construction::Complete);
                let plain = (assets.banner(&rules, &b), assets.seat_flag(&rules, &b));
                b.town_state = seat_state();
                let seat = (assets.banner(&rules, &b), assets.seat_flag(&rules, &b));
                let site = building(kind, Construction::Site { progress: 1 });
                let on_site = (
                    assets.banner(&rules, &site),
                    assets.seat_flag(&rules, &site),
                );
                (plain, seat, on_site)
            })
            .unwrap();
        let ((banner, no_flag), (banner2, flag), (site_banner, site_flag)) = out;
        let banner = banner.expect("a complete Town carries its banner");
        assert!(no_flag.is_none(), "only a Seat flies the Seat flag");
        assert_eq!(banner.mesh, banner_mesh);
        assert_eq!(
            banner.transform.translation,
            centre + Vec3::new(1.0, 0.2, 1.2),
            "at banner_at"
        );
        assert_eq!(banner.transform.scale, Vec3::splat(3.0));
        assert!(banner2.is_some());
        let flag = flag.expect("a Seat flies the flag");
        assert_eq!(flag.mesh, flag_mesh);
        assert_eq!(
            flag.transform.translation,
            centre + Vec3::new(0.0, 4.0, -0.5)
        );
        assert!(
            flag.transform.scale.x > banner.transform.scale.x,
            "the Seat flag is larger than a banner"
        );
        assert!(
            site_banner.is_none() && site_flag.is_none(),
            "not on a site"
        );
        let r = mount_transform(
            Vec3::ZERO,
            &Mount {
                offset_x100: (0, 0, 0),
                scale_x100: 100,
                yaw_deg: 90,
            },
        );
        assert!(
            (r.rotation * Vec3::X - Vec3::NEG_Z).length() < 1e-5,
            "yaw turns about +Y"
        );
    }

    /// A building whose model failed draws the M3b box, which is
    /// team-coloured all over: it carries no banner, and a Seat flies its
    /// flag on the ground at the footprint's north-east corner, not at the
    /// `seat_flag_at` mount measured on the model (the keep the box lacks).
    #[test]
    fn a_box_building_carries_no_banner_and_flies_the_flag_at_its_corner() {
        let mut f = fixture();
        let kind = town(&f.rules);
        let castle = f.rules.building_visual(kind).unwrap().clone();
        f.lib.fail(&ModelKey::of(&castle).unwrap(), "test");
        let mut world = World::new();
        world.insert_resource(std::mem::take(&mut f.meshes));
        world.insert_resource(Assets::<StandardMaterial>::default());
        world.insert_resource(std::mem::take(&mut f.images));
        world.insert_resource(std::mem::take(&mut f.lib));
        let rules = f.rules;
        let centre = footprint_centre(&rules, &building(kind, Construction::Complete));
        let (drawn, banner, flag) = world
            .run_system_once(move |mut assets: DrawAssets| {
                let mut boxes = BuildingVisuals::default();
                let mut seat = building(kind, Construction::Complete);
                seat.town_state = seat_state();
                let drawn = assets.building(&rules, &seat, &mut boxes);
                let cube = boxes.cube(&mut assets.meshes);
                (
                    (drawn, cube),
                    assets.banner(&rules, &seat),
                    assets.seat_flag(&rules, &seat),
                )
            })
            .unwrap();
        let (drawn, cube) = drawn;
        assert_eq!(drawn.mesh, cube, "the failed model draws the box");
        assert!(banner.is_none(), "a box carries no banner");
        let flag = flag.expect("a Seat still flies its flag");
        assert_eq!(
            flag.transform.translation,
            centre + Vec3::new(1.5, 0.0, -1.5),
            "on the ground at the north-east corner, not on the missing keep"
        );
    }

    /// The material `e` is drawn with.
    fn material_of(app: &App, e: Entity) -> AssetId<StandardMaterial> {
        app.world()
            .get::<MeshMaterial3d<StandardMaterial>>(e)
            .expect("a drawn entity")
            .0
            .id()
    }

    /// The banner and Seat flag entities of `id`.
    fn extras_of(app: &App, id: BuildingId) -> Extras {
        app.world()
            .resource::<BuildingExtras>()
            .0
            .get(&id)
            .copied()
            .unwrap_or_default()
    }

    /// In `art_bench` both Seats fly the flag, every complete building
    /// whose Scene names a `banner_at` carries one banner, all banners share
    /// one mesh, no extra is a child, and banners cast no shadow. In
    /// `capture_seat`, when P0 annexes P1's Seat, [`sync_buildings`]
    /// recolours the Seat, its banner and its flag together in P0's
    /// material on the same three entities, and the Seat keeps its flag.
    #[test]
    fn seat_flag_follows_the_seat_and_its_owner() {
        let mut f = fixture();
        croft_with_banner(&mut f);
        let mut app = models_app(f, "art_bench");
        run_ticks(&mut app, 4);
        let (seats, towns, crofts) = {
            let sim = app.world().non_send::<SimHandle>();
            let rules = sim.rules();
            let view = sim.view();
            let complete = |id: &str| {
                view.buildings()
                    .filter(|b| {
                        matches!(b.construction, Construction::Complete)
                            && rules.building_kind(b.kind.0).is_some_and(|k| k.id == id)
                    })
                    .count()
            };
            (
                view.buildings()
                    .filter(|b| b.town_state.as_ref().is_some_and(|t| t.seat))
                    .count(),
                complete("town"),
                complete("croft"),
            )
        };
        assert_eq!(seats, 2);
        let mut q = app.world_mut().query::<(&SeatFlag, Option<&ChildOf>)>();
        let flags: Vec<_> = q.iter(app.world()).collect();
        assert_eq!(flags.len(), seats, "one flag per Seat");
        assert!(flags.iter().all(|(_, p)| p.is_none()), "top level");
        let mut q = app
            .world_mut()
            .query::<(&Banner, &Mesh3d, Option<&ChildOf>, Has<NotShadowCaster>)>();
        let banners: Vec<_> = q.iter(app.world()).collect();
        assert_eq!(
            banners.len(),
            towns + crofts,
            "one per complete Town and Croft"
        );
        let mesh = &banners[0].1.0;
        assert!(
            banners
                .iter()
                .all(|(_, m, p, ns)| &m.0 == mesh && p.is_none() && *ns)
        );
        let mut q = app.world_mut().query::<&MeshMaterial3d<StandardMaterial>>();
        let mut mats: Vec<_> = q.iter(app.world()).map(|m| m.0.id()).collect();
        mats.sort();
        mats.dedup();
        assert!(
            mats.len() <= 6,
            "boxes, sites and two team materials: {}",
            mats.len()
        );

        // The owner flip: P0 annexes P1's Seat in `capture_seat`.
        let mut app = models_app(fixture(), "capture_seat");
        run_ticks(&mut app, 2);
        let (p0_seat, p1_seat) = (BuildingId(1), BuildingId(2));
        let owner = |app: &App| {
            let sim = app.world().non_send::<SimHandle>();
            sim.view().building(p1_seat).map(|b| b.owner)
        };
        let drawn = |app: &App| {
            let x = extras_of(app, p1_seat);
            (
                entity_of(app, p1_seat),
                x.banner.expect("the Seat carries its banner"),
                x.flag.expect("the Seat flies its flag"),
            )
        };
        assert_eq!(owner(&app), Some(PlayerId(1)));
        let (seat, banner, flag) = drawn(&app);
        let p0 = material_of(&app, entity_of(&app, p0_seat));
        let p1 = material_of(&app, seat);
        assert_ne!(p0, p1, "one team material each");
        assert_eq!(material_of(&app, banner), p1);
        assert_eq!(material_of(&app, flag), p1);
        for _ in 0..3000 {
            if owner(&app) == Some(PlayerId(0)) {
                break;
            }
            app.update();
        }
        assert_eq!(owner(&app), Some(PlayerId(0)), "P0 annexed P1's Seat");
        assert_eq!(drawn(&app), (seat, banner, flag), "on the same entities");
        for (what, e) in [("Seat", seat), ("banner", banner), ("Seat flag", flag)] {
            assert_eq!(material_of(&app, e), p0, "the {what} takes P0's material");
        }
        let sim = app.world().non_send::<SimHandle>();
        let still_seat = sim
            .view()
            .building(p1_seat)
            .and_then(|b| b.town_state.as_ref())
            .is_some_and(|t| t.seat);
        assert!(still_seat, "an annexed Seat stays a Seat");
    }

    /// [`sync_buildings`] on a site that completes: while a site it draws
    /// the construction stages A, B and C in turn (neutral, scaled by the
    /// footprint side, no banner), then on the same entity its kind's model
    /// at scale 1 in the owner's team material, and its banner appears.
    /// Should that model fail, the next tick draws the box on the same
    /// entity, its banner goes, and a Seat on the same model moves its
    /// flag to the footprint's north-east corner.
    #[test]
    fn a_site_becomes_its_model_and_raises_its_banner() {
        let mut f = fixture();
        croft_with_banner(&mut f);
        let stages: Vec<Handle<Mesh>> = f.stages.iter().map(|m| m.mesh.clone()).collect();
        let (model_mesh, banner_mesh) = (f.town.mesh.clone(), f.banner.mesh.clone());
        let castle = ModelKey::of(f.rules.building_visual(town(&f.rules)).unwrap()).unwrap();
        let croft = BuildingKindId(f.rules.building_kind_by_id("croft").unwrap().0);
        let mut setup = sim::MatchSetup::scenario(&f.rules, 7);
        setup.start = sim::StartKind::Standard;
        let (p0, tile) = (PlayerId(0), Tile::new(15, 69));
        let command = |seq, c| vec![sim::PlayerCommand::new(p0, seq, c)];
        let stream = vec![
            (
                0,
                command(
                    0,
                    sim::Command::DebugPlaceBuilding {
                        owner: p0,
                        kind: croft,
                        tile,
                        complete: false,
                    },
                ),
            ),
            (
                2,
                command(
                    1,
                    sim::Command::Build {
                        unit: sim::UnitId(1),
                        kind: croft,
                        tile,
                    },
                ),
            ),
        ];
        let mut app = models_app_with(f, setup, stream);
        let (seat, site) = (BuildingId(1), BuildingId(3));
        run_ticks(&mut app, 3);
        let e = entity_of(&app, site);
        let complete = |app: &App| {
            let sim = app.world().non_send::<SimHandle>();
            sim.view()
                .building(site)
                .is_some_and(|b| matches!(b.construction, Construction::Complete))
        };
        let (mut seen, mut site_material) = (Vec::new(), None);
        for _ in 0..2000 {
            if complete(&app) {
                break;
            }
            let world = app.world();
            let mesh = &world.get::<Mesh3d>(e).unwrap().0;
            let stage = stages
                .iter()
                .position(|s| s == mesh)
                .expect("a site draws a construction stage");
            if seen.last() != Some(&stage) {
                seen.push(stage);
            }
            assert_eq!(
                world.get::<Transform>(e).unwrap().scale,
                Vec3::splat(2.0),
                "scaled by the 2-tile side"
            );
            assert_eq!(extras_of(&app, site).banner, None, "no banner on a site");
            site_material = Some(material_of(&app, e));
            app.update();
        }
        assert!(complete(&app), "the Croft was built");
        assert_eq!(seen, vec![0, 1, 2], "stages A, B and C in turn");
        assert_eq!(entity_of(&app, site), e, "the same entity");
        let world = app.world();
        assert_eq!(world.get::<Mesh3d>(e).unwrap().0, model_mesh, "its model");
        assert_eq!(world.get::<Transform>(e).unwrap().scale, Vec3::ONE);
        let team = material_of(&app, entity_of(&app, seat));
        assert_eq!(material_of(&app, e), team, "P0's team material");
        assert_ne!(Some(team), site_material, "sites are neutral");
        let banner = extras_of(&app, site).banner.expect("the banner is raised");
        assert_eq!(world.get::<Mesh3d>(banner).unwrap().0, banner_mesh);
        assert_eq!(material_of(&app, banner), team);

        // The model fails: the box on the same entity, and no banner.
        app.world_mut()
            .resource_mut::<ModelLibrary>()
            .fail(&castle, "test");
        run_ticks(&mut app, 1);
        let cube = app
            .world()
            .resource::<BuildingVisuals>()
            .cube
            .clone()
            .expect("the box mesh");
        let (box_transform, flag_at) = {
            let sim = app.world().non_send::<SimHandle>();
            let view = sim.view();
            let corner = footprint_centre(sim.rules(), view.building(seat).unwrap())
                + Vec3::new(1.5, 0.0, -1.5);
            (
                building_transform(sim.rules(), view.building(site).unwrap()),
                corner,
            )
        };
        assert_eq!(entity_of(&app, site), e, "still the same entity");
        assert_eq!(app.world().get::<Mesh3d>(e).unwrap().0, cube, "the box");
        assert_eq!(*app.world().get::<Transform>(e).unwrap(), box_transform);
        assert!(app.world().get_entity(banner).is_err(), "its banner went");
        assert_eq!(extras_of(&app, site).banner, None);
        assert_eq!(extras_of(&app, seat).banner, None, "the Seat's too");
        let flag = extras_of(&app, seat).flag.expect("the Seat keeps its flag");
        assert_eq!(
            app.world().get::<Transform>(flag).unwrap().translation,
            flag_at,
            "at the box's north-east corner"
        );
    }

    #[test]
    fn rubble_spawns_at_the_footprint_and_expires_or_yields() {
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;
        let r = Rubble {
            until: 30.0,
            tile: (10, 10),
            size: (2, 2),
        };
        assert!(r.overlaps((11, 11), (1, 1)));
        assert!(r.overlaps((9, 9), (2, 2)));
        assert!(!r.overlaps((12, 10), (1, 1)), "east of it");
        assert!(!r.overlaps((8, 10), (2, 2)), "west of it, touching");
        // A destroyed P0 Croft in art_bench leaves rubble on its
        // footprint, scaled by its side; the Croft's entity and its
        // banner go.
        let mut f = fixture();
        croft_with_banner(&mut f);
        let rubble_mesh = f.rubble.mesh.clone();
        let croft_kind = BuildingKindId(f.rules.building_kind_by_id("croft").unwrap().0);
        let mut app = models_app(f, "art_bench");
        run_ticks(&mut app, 4);
        let crofts: Vec<(BuildingId, Tile)> = {
            let sim = app.world().non_send::<SimHandle>();
            let view = sim.view();
            view.buildings()
                .filter(|b| {
                    b.owner == PlayerId(0)
                        && matches!(b.construction, Construction::Complete)
                        && b.kind == croft_kind
                })
                .map(|b| (b.id, b.tile))
                .collect()
        };
        assert!(crofts.len() >= 2, "two complete P0 Crofts: {crofts:?}");
        let destroy = |app: &mut App, id: BuildingId| {
            app.world_mut()
                .resource_mut::<crate::sim_driver::PendingCommands>()
                .push_local(sim::Command::DebugDamage {
                    target: sim::TargetId::Building(id),
                    amount: u32::MAX,
                });
            run_ticks(app, 4);
        };
        let rubble = |app: &mut App| -> Vec<(Rubble, Handle<Mesh>, Transform)> {
            let mut q = app.world_mut().query::<(&Rubble, &Mesh3d, &Transform)>();
            q.iter(app.world())
                .map(|(r, m, t)| (*r, m.0.clone(), *t))
                .collect()
        };
        let target = crofts[0];
        let before = entity_of(&app, target.0);
        let banner = extras_of(&app, target.0)
            .banner
            .expect("the Croft carries its banner");
        destroy(&mut app, target.0);
        assert!(
            app.world().get_entity(before).is_err(),
            "the Croft's entity is gone"
        );
        assert!(app.world().get_entity(banner).is_err(), "so is its banner");
        let found = rubble(&mut app);
        assert_eq!(found.len(), 1);
        let (r, mesh, t) = &found[0];
        assert_eq!(*mesh, rubble_mesh);
        assert_eq!(r.tile, (target.1.x, target.1.y));
        assert_eq!(r.size, (2, 2));
        assert_eq!(t.scale, Vec3::splat(2.0));
        let now = app.world().resource::<Time<Real>>().elapsed_secs_f64();
        assert!((r.until - now - 30.0).abs() < 1.0, "rubble.ds 300 is 30 s");

        // It yields: a Croft placed on the footprint before the 30 s are
        // up removes it.
        app.world_mut()
            .resource_mut::<crate::sim_driver::PendingCommands>()
            .push_local(sim::Command::DebugPlaceBuilding {
                owner: PlayerId(0),
                kind: croft_kind,
                tile: target.1,
                complete: true,
            });
        run_ticks(&mut app, 4);
        let new = {
            let sim = app.world().non_send::<SimHandle>();
            sim.view()
                .buildings()
                .find(|b| b.tile == target.1 && b.id != target.0)
                .map(|b| b.id)
                .expect("a new Croft on the old footprint")
        };
        assert!(
            app.world()
                .resource::<BuildingEntities>()
                .0
                .contains_key(&new)
        );
        assert!(rubble(&mut app).is_empty(), "the new Croft removed it");

        // It expires: the second Croft's rubble stays until 30 s of real
        // time have passed, then goes.
        destroy(&mut app, crofts[1].0);
        let found = rubble(&mut app);
        assert_eq!(found.len(), 1);
        let until = found[0].0.until;
        let now = app.world().resource::<Time<Real>>().elapsed_secs_f64();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            until - now - 0.5,
        )));
        app.update();
        assert_eq!(rubble(&mut app).len(), 1, "half a second before its time");
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(1)));
        app.update();
        assert!(rubble(&mut app).is_empty(), "gone once its 30 s are up");
    }
}
