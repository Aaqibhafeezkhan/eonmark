//! The flat ground: a 128 x 128 m grid of 1 m quads with flat normals and
//! per-tile vertex colours, plus a gizmo grid every 8 tiles. No heightmap
//! in v0.1. Tiles take their colour from the match's map: grass in three
//! close greens, forest floor, rock and water (`docs/ART_STYLE.md`).
//!
//! Territory (M3b) is not in the mesh: the ground's material is
//! [`GroundMaterial`], whose shader lays the tint and the soft border line
//! over these colours from the field texture (`crate::field`).

use std::f32::consts::FRAC_PI_2;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use rules::{MapDef, Terrain};

use crate::field::{GroundMaterial, ground_material};
use crate::palette;
use crate::sim_driver::SimHandle;

/// Ground edge length in tiles (and metres).
pub const GROUND_SIZE_TILES: u32 = 128;
/// Edge length of one tile in metres.
pub const TILE_SIZE: f32 = 1.0;
/// Gizmo grid spacing in tiles.
pub const GRID_SPACING_TILES: u32 = 8;
/// Half the ground edge in metres; the camera focus is clamped to this.
pub const HALF_EXTENT: f32 = GROUND_SIZE_TILES as f32 * TILE_SIZE * 0.5;

/// Spawns the ground mesh and draws the grid.
pub struct GroundPlugin;

impl Plugin for GroundPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (spawn_ground, configure_gizmos))
            .add_systems(Update, draw_grid);
    }
}

/// Marker for the ground entity.
#[derive(Component)]
pub struct Ground;

/// Deterministic tile colour choice. Not sim state: purely cosmetic, so it
/// may use floats and need not match across machines.
fn tile_palette_index(x: u32, z: u32) -> usize {
    let mut h = x.wrapping_mul(0x9E37_79B1) ^ z.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    // Mostly the base green; each variant appears on 1/8 of tiles.
    match h & 7 {
        0..=5 => 0,
        6 => 1,
        _ => 2,
    }
}

/// The colour of tile `(x, z)`: its terrain from `map`, all grass without a
/// map. Grass varies between three close greens.
fn tile_color(map: Option<&MapDef>, x: u32, z: u32) -> Color {
    let terrain = map
        .filter(|m| x < u32::from(m.width) && z < u32::from(m.height))
        .map_or(Terrain::Grass, |m| {
            m.tile(
                u16::try_from(x).expect("map width fits u16"),
                u16::try_from(z).expect("map height fits u16"),
            )
        });
    match terrain {
        Terrain::Grass => palette::GRASS[tile_palette_index(x, z)],
        Terrain::Forest => palette::FOREST_FLOOR,
        Terrain::Mountain => palette::ROCK,
        Terrain::Water => palette::WATER,
    }
}

/// Each ground tile's colour, row-major.
pub fn base_colors(map: Option<&MapDef>) -> Vec<Color> {
    let n = GROUND_SIZE_TILES;
    (0..n)
        .flat_map(|z| (0..n).map(move |x| (x, z)))
        .map(|(x, z)| tile_color(map, x, z))
        .collect()
}

/// Per-vertex colours for the ground mesh from per-tile colours (four
/// vertices per tile, in mesh order).
fn vertex_colors(tiles: &[Color]) -> Vec<[f32; 4]> {
    tiles
        .iter()
        .flat_map(|c| [c.to_linear().to_f32_array(); 4])
        .collect()
}

/// Build the ground mesh: four unique vertices per tile so normals and
/// colours are flat per tile. 65 536 vertices, 98 304 indices (u32). Tile
/// colours follow `map`'s terrain (sim tile `(x, y)` is world tile
/// `(x, z)`); without a map every tile is grass. The terrain never changes
/// and the ground is not a picking target, so the mesh lives in the render
/// world only.
pub fn build_ground_mesh(map: Option<&MapDef>) -> Mesh {
    let n = GROUND_SIZE_TILES;
    let tiles = (n * n) as usize;
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(tiles * 4);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(tiles * 4);
    let mut indices: Vec<u32> = Vec::with_capacity(tiles * 6);

    for z in 0..n {
        for x in 0..n {
            let x0 = x as f32 * TILE_SIZE - HALF_EXTENT;
            let z0 = z as f32 * TILE_SIZE - HALF_EXTENT;
            let x1 = x0 + TILE_SIZE;
            let z1 = z0 + TILE_SIZE;
            let base = positions.len() as u32;
            positions.extend_from_slice(&[
                [x0, 0.0, z0],
                [x1, 0.0, z0],
                [x1, 0.0, z1],
                [x0, 0.0, z1],
            ]);
            normals.extend_from_slice(&[[0.0, 1.0, 0.0]; 4]);
            // Counter-clockwise seen from +Y.
            indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
        }
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, vertex_colors(&base_colors(map)))
    .with_inserted_indices(Indices::U32(indices))
}

fn spawn_ground(
    mut commands: Commands,
    sim: Option<NonSend<SimHandle>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<GroundMaterial>>,
) {
    let map = sim.as_ref().and_then(|s| s.rules().map(&s.setup().map));
    let mesh = meshes.add(build_ground_mesh(map));
    let view = sim.as_ref().map(|s| s.view());
    let base = StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.95,
        reflectance: 0.2,
        metallic: 0.0,
        ..default()
    };
    let (material, texture) =
        ground_material(base, view.as_ref().map(|v| v.territory()), &mut images);
    commands.insert_resource(texture);
    commands.spawn((
        Name::new("Ground"),
        Ground,
        Mesh3d(mesh),
        MeshMaterial3d(materials.add(material)),
        Transform::IDENTITY,
    ));
}

fn configure_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<DefaultGizmoConfigGroup>();
    config.line.width = 1.0;
}

fn draw_grid(mut gizmos: Gizmos) {
    let cells = GROUND_SIZE_TILES / GRID_SPACING_TILES;
    let spacing = GRID_SPACING_TILES as f32 * TILE_SIZE;
    // `grid` draws in the XY plane; rotate it onto XZ and lift it 2 cm to
    // avoid z-fighting with the ground.
    gizmos.grid(
        Isometry3d::new(Vec3::Y * 0.02, Quat::from_rotation_x(-FRAC_PI_2)),
        UVec2::splat(cells),
        Vec2::splat(spacing),
        palette::GRID_LINE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_has_four_vertices_and_six_indices_per_tile() {
        let mesh = build_ground_mesh(None);
        let tiles = (GROUND_SIZE_TILES * GROUND_SIZE_TILES) as usize;
        assert_eq!(mesh.count_vertices(), tiles * 4);
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("expected u32 indices");
        };
        assert_eq!(indices.len(), tiles * 6);
    }

    #[test]
    fn tiles_take_their_terrain_colour_from_the_map() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let rules = rules::Rules::load(&data).expect("data loads");
        let map = rules.map("plains_1v1").expect("default map");
        let mut seen = [false; 4];
        for z in 0..u32::from(map.height) {
            for x in 0..u32::from(map.width) {
                let terrain = map.tile(x as u16, z as u16);
                let want = match terrain {
                    Terrain::Grass => None,
                    Terrain::Forest => Some(palette::FOREST_FLOOR),
                    Terrain::Mountain => Some(palette::ROCK),
                    Terrain::Water => Some(palette::WATER),
                };
                seen[terrain as usize] = true;
                let got = tile_color(Some(map), x, z);
                match want {
                    Some(c) => assert_eq!(got, c, "({x}, {z})"),
                    None => assert!(palette::GRASS.contains(&got), "({x}, {z})"),
                }
            }
        }
        assert_eq!(seen, [true; 4], "plains_1v1 has every terrain");
    }

    #[test]
    fn palette_index_is_in_range_and_stable() {
        for z in 0..GROUND_SIZE_TILES {
            for x in 0..GROUND_SIZE_TILES {
                let i = tile_palette_index(x, z);
                assert!(i < palette::GRASS.len());
                assert_eq!(i, tile_palette_index(x, z));
            }
        }
    }
}
