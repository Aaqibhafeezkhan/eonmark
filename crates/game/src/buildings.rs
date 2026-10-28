//! Presentation of buildings and construction sites (M3b): one entity per
//! `BuildingId`, mirrored from the `SimView` after every tick like units.
//! A building is drawn as its kind's `data/visuals.ron` entry
//! (`rules::Rules::building_visual`, art pass): `Primitive(Cuboid)` is the
//! M3b look, a matte team-coloured box covering its footprint, and a
//! `Scene` draws that same box (its only allowed fallback) until the art
//! pass loads models (slice 3). A site is the box in a lighter scaffold
//! colour that rises with its construction progress. One shared unit-cube
//! mesh is scaled per building ([`BuildingVisuals::mesh`]), with one
//! material per `(team, site)`, so every building is one of a handful of
//! draw calls. Buildings carry `Pickable` so they can be clicked (rule 16:
//! only units and buildings are pickable).

use std::collections::HashMap;

use bevy::prelude::*;
use rules::{Primitive, Rules, Visual};
use sim::{Building, BuildingId, Construction};

use crate::ground::HALF_EXTENT;
use crate::palette;
use crate::present::{MATTE_REFLECTANCE, MATTE_ROUGHNESS};
use crate::sim_driver::{SimHandle, SimSystems, sim_stepped};

/// Height of a complete building in metres: half a metre plus 0.4 m per
/// tile of its shorter footprint side, so a 3x3 Town stands taller than a
/// 2x2 Croft and a 1x1 Watchtower is a short post until the art pass.
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

/// One shared unit cube and one material per `(team, site)`.
#[derive(Resource, Default)]
pub struct BuildingVisuals {
    cube: Option<Handle<Mesh>>,
    materials: HashMap<(u8, bool), Handle<StandardMaterial>>,
}

impl BuildingVisuals {
    /// The mesh a building of a kind whose visual is `visual` is drawn
    /// with: the shared unit cube ([`BuildingVisuals::cube`]) for
    /// `Primitive(Cuboid)`, for a `Scene` (until models load, its fallback
    /// is the box) and for a kind with no visual.
    pub fn mesh(&mut self, visual: Option<&Visual>, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        match visual {
            None
            | Some(Visual::Primitive(Primitive::Cuboid))
            | Some(Visual::Scene { .. })
            | Some(Visual::Primitive(_))
            | Some(Visual::Body { .. }) => self.cube(meshes),
        }
    }

    /// The unit cube every building scales, its base on the ground.
    pub fn cube(&mut self, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        self.cube
            .get_or_insert_with(|| {
                let mut mesh = Mesh::from(Cuboid::new(1.0, 1.0, 1.0));
                mesh.translate_by(Vec3::Y * 0.5);
                meshes.add(mesh)
            })
            .clone()
    }

    /// The matte material for `team`, in scaffold colour for a site.
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

/// The world transform of `b`: its footprint's centre on the ground,
/// scaled to the footprint and to its height (a site by its progress).
pub fn building_transform(rules: &Rules, b: &Building) -> Transform {
    let kind = rules
        .building_kind(b.kind.0)
        .expect("a building's kind is in the rules");
    let (w, h) = (kind.width_tiles as f32, kind.height_tiles as f32);
    let full = building_height(kind.width_tiles, kind.height_tiles);
    let height = match b.construction {
        Construction::Complete => full,
        Construction::Site { progress } => {
            let total = rules.build_time_ticks(kind).max(1);
            let fraction = progress as f32 / total as f32;
            full * fraction.clamp(SITE_MIN_FRACTION, 1.0)
        }
    };
    let x = f32::from(b.tile.x) + w * 0.5 - HALF_EXTENT;
    let z = f32::from(b.tile.y) + h * 0.5 - HALF_EXTENT;
    Transform::from_translation(Vec3::new(x, 0.0, z)).with_scale(Vec3::new(w, height, h))
}

/// Mirrors sim buildings into entities.
pub struct BuildingsPlugin;

impl Plugin for BuildingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BuildingEntities>()
            .init_resource::<BuildingVisuals>()
            .add_systems(
                FixedUpdate,
                sync_buildings.after(SimSystems::Step).run_if(sim_stepped),
            );
    }
}

/// After every tick: spawn an entity for each new building, update the
/// transform and material of each known one (a site grows, then turns into
/// a complete building), and despawn entities whose building is gone.
pub fn sync_buildings(
    sim: NonSend<SimHandle>,
    mut entities: ResMut<BuildingEntities>,
    mut visuals: ResMut<BuildingVisuals>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut shown: Query<(&mut Transform, &mut MeshMaterial3d<StandardMaterial>), With<BuildingRef>>,
    mut commands: Commands,
) {
    let view = sim.view();
    let rules = sim.rules();
    let mut count = 0usize;
    for b in view.buildings() {
        count += 1;
        let transform = building_transform(rules, b);
        let site = matches!(b.construction, Construction::Site { .. });
        let material = visuals.material(b.owner.0, site, &mut materials);
        match entities.0.get(&b.id) {
            Some(entity) => {
                if let Ok((mut t, mut m)) = shown.get_mut(*entity) {
                    if *t != transform {
                        *t = transform;
                    }
                    if m.0 != material {
                        m.0 = material;
                    }
                }
            }
            None => {
                let mesh = visuals.mesh(rules.building_visual(b.kind.0), &mut meshes);
                let kind = rules
                    .building_kind(b.kind.0)
                    .map_or("Building", |k| &k.name);
                let entity = commands
                    .spawn((
                        Name::new(format!("{kind} {}", b.id.0)),
                        BuildingRef(b.id),
                        Mesh3d(mesh),
                        MeshMaterial3d(material),
                        transform,
                        Pickable::default(),
                    ))
                    .id();
                entities.0.insert(b.id, entity);
            }
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::{BuildingKindId, PlayerId, Tile};

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

    /// Art pass, slice 1: every kind's `visuals.ron` entry is read, and a
    /// `Primitive(Cuboid)` or a `Scene` (until models load) keeps the M3b
    /// box: the one shared unit cube, scaled to the footprint and height.
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
        assert_eq!(
            rules.building_visual(town),
            Some(&Visual::Primitive(Primitive::Cuboid))
        );
        assert_eq!(visuals.mesh(rules.building_visual(town), &mut meshes), cube);
        let scene = Visual::Scene {
            path: "models/castle.gltf".into(),
            scale_x100: 100,
            y_offset_tiles_x100: 0,
            yaw_deg: 0,
            nodes: Vec::new(),
            parts: Vec::new(),
            team: None,
            fallback: Some(Box::new(Visual::Primitive(Primitive::Cuboid))),
        };
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
    }

    #[test]
    fn sites_and_buildings_differ_in_colour_per_team() {
        assert_ne!(building_color(0, false), building_color(0, true));
        assert_ne!(building_color(0, false), building_color(1, false));
        assert_eq!(building_color(1, false), palette::team_color(1));
    }
}
