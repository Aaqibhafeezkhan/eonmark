//! Terrain dressing (art pass, slice 3): trees on forest, mountains and
//! rocks on mountain, plants on water, from `visuals.ron`'s `terrain`
//! table, spawned once when models settle.
//!
//! [`decor_placements`] is a pure function of the map, the table and each
//! look's placed size ([`LookSize`], measured on the merged model). For
//! every Forest, Mountain or Water tile with a table entry it:
//!
//! 1. measures its room ([`room`]): how many rings of tiles around it are
//!    all the same terrain (off the map counts as alike, up to
//!    [`MAX_ROOM`]); an edge tile has room 0, an interior tile (all 8
//!    neighbours alike) at least 1;
//! 2. keeps it when a tile hash `% 1000` is under its class's density;
//! 3. picks a weighted look among those allowed `at` its class and that
//!    fit: only looks whose footprint (their reach, [`LookSize`], times
//!    the largest scale jitter) stays within the square of same-terrain
//!    tiles around the tile, `room + 0.5` m from its centre, so nothing
//!    spills onto passable land (on an edge tile: inside the tile itself);
//!    and on a tile whose north neighbour is passable only looks at most
//!    [`TALL_PROP_MAX_M`] tall, because the camera looks north and a tall
//!    prop would hide the units behind it;
//! 4. takes a jitter (±`jitter_x100`, clamped so the footprint stays
//!    within that square), a yaw of 60° × (hash % 6) and a scale
//!    (±`scale_jitter_pct`) from further hashes. Big mountains therefore
//!    stand only deep inside a range, and smaller looks fill its rim.
//!
//! The hashes are integer mixes of the tile and a salt, as the ground's
//! tile colours are (`ground.rs`): cosmetic, so the floats after them need
//! not match across machines. Props are top-level entities with the
//! look's neutral material, never `Pickable`, and without a shadow when
//! the look says `shadow: false`. Each look is one mesh and every look on
//! the `KayKit` atlas shares one material, so a look is one draw bin per
//! pass however many props it has.

use std::collections::BTreeMap;

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use rules::{DecorAt, MapDef, Terrain, TerrainDecor};

use crate::ground::HALF_EXTENT;
use crate::models::ModelLibrary;
use crate::sim_driver::SimHandle;

/// Tallest prop allowed on a tile whose north neighbour is passable,
/// metres (a unit is 1.2-1.9 m tall).
pub const TALL_PROP_MAX_M: f32 = 1.6;

/// The most rings of same-terrain tiles [`room`] counts.
pub const MAX_ROOM: u16 = 3;

/// How many rings of tiles around `(x, y)` are all `map`'s terrain at
/// `(x, y)` (off the map counts as alike), up to [`MAX_ROOM`]: 0 on an
/// edge tile, at least 1 on an interior one.
pub fn room(map: &MapDef, x: u16, y: u16) -> u16 {
    let t = map.tile(x, y);
    let alike = |nx: i32, ny: i32| match (u16::try_from(nx), u16::try_from(ny)) {
        (Ok(nx), Ok(ny)) if nx < map.width && ny < map.height => map.tile(nx, ny) == t,
        _ => true,
    };
    let (x, y) = (i32::from(x), i32::from(y));
    for r in 1..=i32::from(MAX_ROOM) {
        let ring = (-r..=r).all(|d| {
            alike(x + d, y - r) && alike(x + d, y + r) && alike(x - r, y + d) && alike(x + r, y + d)
        });
        if !ring {
            return u16::try_from(r - 1).unwrap_or(0);
        }
    }
    MAX_ROOM
}

/// The placed size of one look, metres: the largest horizontal distance of
/// its bounds' corners from its origin (so any yaw stays within it) and the
/// height of its top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookSize {
    /// Horizontal reach from the origin.
    pub reach: f32,
    /// Height of the top above the ground.
    pub height: f32,
}

/// One prop to spawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    /// The `terrain` key.
    pub terrain: &'static str,
    /// The index of the look in its entry's `looks`.
    pub look: usize,
    /// The map tile.
    pub tile: (u16, u16),
    /// Whether the tile is interior.
    pub interior: bool,
    /// Rings of same-terrain tiles around it ([`room`]).
    pub room: u16,
    /// World position on the ground.
    pub pos: Vec3,
    /// Yaw, radians.
    pub yaw: f32,
    /// Uniform scale.
    pub scale: f32,
}

/// The `terrain` key of a terrain that may carry props.
pub fn terrain_key(t: Terrain) -> Option<&'static str> {
    match t {
        Terrain::Forest => Some("forest"),
        Terrain::Mountain => Some("mountain"),
        Terrain::Water => Some("water"),
        Terrain::Grass => None,
    }
}

/// An integer mix of a tile and a salt (cosmetic, deterministic).
fn mix(x: u16, y: u16, salt: u32) -> u32 {
    let mut h = u32::from(x).wrapping_mul(0x9E37_79B1)
        ^ u32::from(y).wrapping_mul(0x85EB_CA77)
        ^ salt.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h
}

/// `-1.0..=1.0` from a hash.
fn unit(h: u32) -> f32 {
    (h % 2001) as f32 / 1000.0 - 1.0
}

/// Every prop of `map` dressed by `terrain`, in tile order (row by row);
/// `size(key, look)` is a look's placed size, `None` for a look whose model
/// is not ready (skipped).
pub fn decor_placements(
    map: &MapDef,
    terrain: &BTreeMap<String, TerrainDecor>,
    size: &dyn Fn(&str, usize) -> Option<LookSize>,
) -> Vec<Placement> {
    let mut out = Vec::new();
    for y in 0..map.height {
        for x in 0..map.width {
            let t = map.tile(x, y);
            let Some(key) = terrain_key(t) else {
                continue;
            };
            let Some(decor) = terrain.get(key) else {
                continue;
            };
            let room = room(map, x, y);
            let interior = room > 0;
            // The half side of the same-terrain square around the tile.
            let square = f32::from(room) + 0.5;
            let density = if interior {
                decor.interior_permille
            } else {
                decor.edge_permille
            };
            if mix(x, y, 1) % 1000 >= density {
                continue;
            }
            let north_passable = y > 0 && map.tile(x, y - 1).passable();
            let pct = decor.scale_jitter_pct;
            let max_scale = 1.0 + pct as f32 / 100.0;
            let fits: Vec<(usize, u32, LookSize)> = decor
                .looks
                .iter()
                .enumerate()
                .filter(|(_, l)| match l.at {
                    DecorAt::Interior => interior,
                    DecorAt::Edge => !interior,
                    DecorAt::Any => true,
                })
                .filter_map(|(i, l)| size(key, i).map(|s| (i, l.weight, s)))
                .filter(|(_, _, s)| !north_passable || s.height * max_scale <= TALL_PROP_MAX_M)
                .filter(|(_, _, s)| s.reach * max_scale <= square)
                .collect();
            let total: u32 = fits.iter().map(|f| f.1).sum();
            if total == 0 {
                continue;
            }
            let mut pick = mix(x, y, 2) % total;
            let Some(&(look, _, look_size)) = fits.iter().find(|f| {
                if pick < f.1 {
                    true
                } else {
                    pick -= f.1;
                    false
                }
            }) else {
                continue;
            };
            let span = 2 * pct + 1;
            let scale = 1.0 + ((mix(x, y, 3) % span) as f32 - pct as f32) / 100.0;
            let reach = decor.jitter_x100 as f32 / 100.0;
            let limit = (square - look_size.reach * scale).max(0.0);
            let jx = (unit(mix(x, y, 4)) * reach).clamp(-limit, limit);
            let jz = (unit(mix(x, y, 5)) * reach).clamp(-limit, limit);
            out.push(Placement {
                terrain: key,
                look,
                tile: (x, y),
                interior,
                room,
                pos: Vec3::new(
                    f32::from(x) + 0.5 + jx - HALF_EXTENT,
                    0.0,
                    f32::from(y) + 0.5 + jz - HALF_EXTENT,
                ),
                yaw: (60.0 * (mix(x, y, 6) % 6) as f32).to_radians(),
                scale,
            });
        }
    }
    out
}

/// Marks a terrain prop.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prop;

/// Spawns the props once models settle. Added only by `app::run`, with
/// `models::ModelsPlugin`.
pub struct TerrainPropsPlugin;

impl Plugin for TerrainPropsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, spawn_props);
    }
}

/// Once, the first frame the model library has settled: every placement
/// as a top-level entity with its look's model and neutral material.
pub fn spawn_props(
    sim: NonSend<SimHandle>,
    lib: Option<ResMut<ModelLibrary>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut done: Local<bool>,
    mut commands: Commands,
) {
    if *done {
        return;
    }
    let Some(mut lib) = lib else {
        *done = true;
        return;
    };
    if !lib.is_settled() {
        return;
    }
    *done = true;
    let rules = sim.rules();
    let Some(map) = rules.map(&sim.setup().map) else {
        return;
    };
    let table = &rules.visuals.terrain;
    let size = |key: &str, look: usize| {
        let model = lib.model(&table.get(key)?.looks.get(look)?.visual)?;
        Some(LookSize {
            reach: model.reach,
            height: model.max.y,
        })
    };
    let placements = decor_placements(map, table, &size);
    let mut tiles = std::collections::BTreeSet::new();
    for p in &placements {
        let Some(look) = table.get(p.terrain).and_then(|d| d.looks.get(p.look)) else {
            continue;
        };
        let Some(model) = lib.model(&look.visual).cloned() else {
            continue;
        };
        let material = lib.material(&model, None, &mut images, &mut materials);
        let mut e = commands.spawn((
            Name::new(format!("Prop {}", p.terrain)),
            Prop,
            Mesh3d(model.mesh),
            MeshMaterial3d(material),
            Transform::from_translation(p.pos)
                .with_rotation(Quat::from_rotation_y(p.yaw))
                .with_scale(Vec3::splat(p.scale)),
        ));
        if !look.shadow {
            e.insert(NotShadowCaster);
        }
        tiles.insert(p.tile);
    }
    info!(
        "terrain_props: {} prop(s) on {} tile(s)",
        placements.len(),
        tiles.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use rules::{DecorLook, Rules, Visual};

    fn rules() -> Rules {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        Rules::load(&data).expect("data loads")
    }

    fn look(path: &str, at: DecorAt, weight: u32) -> DecorLook {
        DecorLook {
            visual: Visual::Scene {
                path: path.into(),
                scale_x100: 100,
                y_offset_tiles_x100: 0,
                yaw_deg: 0,
                x_offset_tiles_x100: 0,
                z_offset_tiles_x100: 0,
                nodes: vec![],
                parts: vec![],
                team: None,
                fallback: None,
                banner_at: None,
                seat_flag_at: None,
            },
            at,
            weight,
            shadow: true,
        }
    }

    /// Forest: a tall wide cluster for interiors and a short single tree
    /// for edges; mountain: a tall peak anywhere and a pebble anywhere.
    fn table(interior: u32, edge: u32) -> BTreeMap<String, TerrainDecor> {
        let decor = |looks| TerrainDecor {
            interior_permille: interior,
            edge_permille: edge,
            jitter_x100: 30,
            scale_jitter_pct: 10,
            looks,
        };
        BTreeMap::from([
            (
                "forest".to_string(),
                decor(vec![
                    look("cluster.gltf", DecorAt::Interior, 2),
                    look("single.gltf", DecorAt::Edge, 1),
                ]),
            ),
            (
                "mountain".to_string(),
                decor(vec![
                    look("peak.gltf", DecorAt::Any, 1),
                    look("pebble.gltf", DecorAt::Any, 1),
                ]),
            ),
        ])
    }

    fn sizes(key: &str, look: usize) -> Option<LookSize> {
        Some(match (key, look) {
            ("forest", 0) => LookSize {
                reach: 1.2,
                height: 2.2,
            },
            ("forest", 1) => LookSize {
                reach: 0.3,
                height: 1.2,
            },
            ("mountain", 0) => LookSize {
                reach: 1.0,
                height: 3.0,
            },
            _ => LookSize {
                reach: 0.15,
                height: 0.1,
            },
        })
    }

    fn map(rules: &Rules) -> &MapDef {
        rules.maps.get("plains_1v1").expect("plains_1v1")
    }

    #[test]
    fn placements_are_deterministic_and_only_on_their_terrain() {
        let rules = rules();
        let map = map(&rules);
        let t = table(1000, 600);
        let a = decor_placements(map, &t, &sizes);
        let b = decor_placements(map, &t, &sizes);
        assert_eq!(a, b, "the same props every run");
        assert!(!a.is_empty());
        for p in &a {
            let terrain = map.tile(p.tile.0, p.tile.1);
            assert_eq!(terrain_key(terrain), Some(p.terrain), "{p:?}");
            assert_ne!(terrain, Terrain::Grass);
            assert!(p.terrain != "water", "water has no entry in this table");
            let (cx, cz) = (
                f32::from(p.tile.0) + 0.5 - HALF_EXTENT,
                f32::from(p.tile.1) + 0.5 - HALF_EXTENT,
            );
            assert!((p.pos.x - cx).abs() <= 0.3 + 1e-5 && (p.pos.z - cz).abs() <= 0.3 + 1e-5);
            assert!((0.9 - 1e-5..=1.1 + 1e-5).contains(&p.scale), "{p:?}");
            let sixths = p.yaw / 60f32.to_radians();
            assert!(
                (sixths - sixths.round()).abs() < 1e-4 && sixths < 5.5,
                "{p:?}"
            );
        }
        // Both classes and both terrains appear; interior tiles take only
        // looks allowed there.
        assert!(a.iter().any(|p| p.interior) && a.iter().any(|p| !p.interior));
        assert!(a.iter().any(|p| p.terrain == "mountain"));
        assert!(
            a.iter()
                .filter(|p| p.terrain == "forest")
                .all(|p| (p.look == 0) == p.interior),
            "the cluster only inside, the single tree only on edges"
        );
    }

    #[test]
    fn edge_props_stay_inside_their_tile() {
        let rules = rules();
        let map = map(&rules);
        let placements = decor_placements(map, &table(1000, 1000), &sizes);
        let mut edges = 0;
        for p in placements.iter().filter(|p| !p.interior) {
            edges += 1;
            let s = sizes(p.terrain, p.look).unwrap();
            let (cx, cz) = (
                f32::from(p.tile.0) + 0.5 - HALF_EXTENT,
                f32::from(p.tile.1) + 0.5 - HALF_EXTENT,
            );
            let r = s.reach * p.scale;
            assert!(
                (p.pos.x - cx).abs() + r <= 0.5 + 1e-5 && (p.pos.z - cz).abs() + r <= 0.5 + 1e-5,
                "{p:?} reaches out of its tile"
            );
            assert!(s.reach * 1.1 <= 0.5, "only looks that fit: {p:?}");
        }
        assert!(edges > 100, "{edges} edge props");
    }

    /// Every prop's footprint (a circle of its reach times its scale
    /// around its position) lies on tiles of its own terrain, so no tree
    /// or mountain spills onto passable land; a look too big for a tile's
    /// room goes only where there is room for it.
    #[test]
    fn props_stay_on_their_terrain_and_big_looks_need_room() {
        let rules = rules();
        let map = map(&rules);
        let placements = decor_placements(map, &table(1000, 1000), &sizes);
        for p in &placements {
            let r = sizes(p.terrain, p.look).unwrap().reach * p.scale;
            let t = map.tile(p.tile.0, p.tile.1);
            let lo = |c: f32| (c - r + HALF_EXTENT + 1e-4).floor() as i32;
            let hi = |c: f32| (c + r + HALF_EXTENT - 1e-4).floor() as i32;
            for ty in lo(p.pos.z)..=hi(p.pos.z) {
                for tx in lo(p.pos.x)..=hi(p.pos.x) {
                    let (Ok(tx), Ok(ty)) = (u16::try_from(tx), u16::try_from(ty)) else {
                        continue;
                    };
                    if tx < map.width && ty < map.height {
                        assert_eq!(map.tile(tx, ty), t, "{p:?} covers ({tx}, {ty})");
                    }
                }
            }
            assert!(r <= f32::from(p.room) + 0.5 + 1e-5, "{p:?}");
        }
        // The 1.0 m-reach peak needs a room of 1 at least (1.1 m at the
        // largest scale); the forest cluster (1.2 m) a room of 1 too.
        assert!(
            placements
                .iter()
                .filter(|p| p.terrain == "mountain" && p.look == 0)
                .all(|p| p.room >= 1)
        );
        assert!(placements.iter().any(|p| p.room >= 2), "deep tiles exist");
        // room: an edge tile 0; plains_1v1's forest blob (10-18, 48-56) has
        // its centre three rings deep at most.
        let edge = placements.iter().find(|p| !p.interior).unwrap();
        assert_eq!(room(map, edge.tile.0, edge.tile.1), 0);
        assert_eq!(map.tile(14, 52), Terrain::Forest);
        assert!(room(map, 14, 52) >= 2, "{}", room(map, 14, 52));
    }

    #[test]
    fn tall_props_never_stand_south_of_passable_land() {
        let rules = rules();
        let map = map(&rules);
        let placements = decor_placements(map, &table(1000, 1000), &sizes);
        let mut beside = 0;
        for p in &placements {
            let s = sizes(p.terrain, p.look).unwrap();
            let (x, y) = p.tile;
            if y > 0 && map.tile(x, y - 1).passable() {
                beside += 1;
                assert!(
                    s.height * p.scale <= TALL_PROP_MAX_M,
                    "{p:?}: {} m south of passable land",
                    s.height * p.scale
                );
            }
        }
        assert!(beside > 50, "{beside} props south of passable tiles");
        // Tall props do stand where nothing passable lies north.
        assert!(
            placements
                .iter()
                .any(|p| sizes(p.terrain, p.look).unwrap().height > TALL_PROP_MAX_M)
        );
    }

    #[test]
    fn densities_scale_counts() {
        let rules = rules();
        let map = map(&rules);
        let count = |interior, edge| decor_placements(map, &table(interior, edge), &sizes).len();
        assert_eq!(count(0, 0), 0);
        let full = count(1000, 1000);
        let half = count(500, 500);
        let tenth = count(100, 100);
        assert!(
            full > half && half > tenth && tenth > 0,
            "{full} {half} {tenth}"
        );
        let ratio = half as f32 / full as f32;
        assert!((0.4..0.6).contains(&ratio), "{half} of {full}");
        // Edge density alone leaves the interior bare.
        let edge_only = decor_placements(map, &table(0, 1000), &sizes);
        assert!(edge_only.iter().all(|p| !p.interior));
        // A look whose model is not ready is skipped.
        let none = decor_placements(map, &table(1000, 1000), &|_, _| None);
        assert!(none.is_empty());
    }

    /// The spawn system: nothing while a look's model is still loading;
    /// once the library settles, every placement once (a later frame adds
    /// nothing), each a top-level [`Prop`] that is never pickable, casting
    /// a shadow unless its look says `shadow: false` (the water plants).
    #[test]
    fn spawn_props_once_unpickable_with_per_look_shadows() {
        use crate::models::{Geometry, ModelKey, content_hash, finish_geometry};
        use bevy::asset::RenderAssetUsages;
        use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
        let mut rules = rules();
        let tree = look("m/tree.gltf", DecorAt::Any, 1);
        let mut lily = look("m/lily.gltf", DecorAt::Any, 1);
        lily.shadow = false;
        let decor = |l: DecorLook| TerrainDecor {
            interior_permille: 1000,
            edge_permille: 1000,
            jitter_x100: 0,
            scale_jitter_pct: 0,
            looks: vec![l],
        };
        rules.visuals.terrain = BTreeMap::from([
            ("forest".to_string(), decor(tree.clone())),
            ("water".to_string(), decor(lily.clone())),
        ]);
        let (mut meshes, mut images) = (Assets::<Mesh>::default(), Assets::<Image>::default());
        let model = |meshes: &mut Assets<Mesh>, images: &mut Assets<Image>| {
            let image = Image::new(
                Extent3d {
                    width: 4,
                    height: 4,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                vec![120; 4 * 4 * 4],
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::default(),
            );
            let hash = content_hash(&image);
            let m = finish_geometry(
                Geometry {
                    positions: vec![[-0.2, 0.0, -0.2], [0.2, 0.3, 0.2], [0.0, 0.0, 0.2]],
                    normals: vec![[0.0, 1.0, 0.0]; 3],
                    uvs: vec![[0.1, 0.1]; 3],
                    indices: vec![0, 1, 2],
                    textures: vec![],
                    base_color: None,
                },
                hash,
                meshes,
            );
            (m, images.add(image))
        };
        let mut lib = ModelLibrary::new(&rules.visuals);
        let (tree_model, tree_image) = model(&mut meshes, &mut images);
        lib.insert_ready(ModelKey::of(&tree.visual).unwrap(), tree_model, tree_image);
        let lily_key = ModelKey::of(&lily.visual).unwrap();
        lib.insert_loading(lily_key.clone(), Vec::new());
        let setup = sim::MatchSetup::scenario(&rules, 7);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(Assets::<StandardMaterial>::default())
            .insert_resource(lib)
            .add_systems(Update, spawn_props);
        app.world_mut()
            .insert_non_send(SimHandle::from_setup(setup, rules));
        let props = |app: &mut App| {
            let mut q = app.world_mut().query_filtered::<(
                &Name,
                Has<NotShadowCaster>,
                Has<Pickable>,
                Has<ChildOf>,
            ), With<Prop>>();
            q.iter(app.world())
                .map(|(n, ns, p, c)| (n.as_str().to_string(), ns, p, c))
                .collect::<Vec<_>>()
        };
        app.insert_resource(meshes).insert_resource(images);
        app.update();
        assert!(props(&mut app).is_empty(), "nothing while a model loads");
        let (lily_model, lily_image) = {
            let world = app.world_mut();
            let mut meshes = world.remove_resource::<Assets<Mesh>>().unwrap();
            let mut images = world.remove_resource::<Assets<Image>>().unwrap();
            let out = model(&mut meshes, &mut images);
            world.insert_resource(meshes);
            world.insert_resource(images);
            out
        };
        app.world_mut()
            .resource_mut::<ModelLibrary>()
            .insert_ready(lily_key, lily_model, lily_image);
        app.update();
        let first = props(&mut app);
        let (forest, water): (Vec<_>, Vec<_>) =
            first.iter().partition(|(n, ..)| n == "Prop forest");
        assert!(!forest.is_empty(), "forest props");
        assert!(
            !water.is_empty() && water.iter().all(|(n, ..)| n == "Prop water"),
            "water props"
        );
        assert!(forest.iter().all(|(_, ns, ..)| !ns), "trees cast shadows");
        assert!(water.iter().all(|(_, ns, ..)| *ns), "water plants do not");
        assert!(
            first
                .iter()
                .all(|(_, _, pickable, child)| !pickable && !child),
            "top level, never pickable"
        );
        app.update();
        assert_eq!(props(&mut app).len(), first.len(), "spawned once");
    }
}
