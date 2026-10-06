//! The flat ground: a 128 x 128 m grid of 1 m quads with flat normals and
//! per-tile vertex colours, plus a gizmo grid every 8 tiles. No heightmap
//! in v0.1; forest/rock/water tile colours arrive with the map data (M1).

use std::f32::consts::FRAC_PI_2;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::palette;

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

/// Build the ground mesh: four unique vertices per tile so normals and
/// colours are flat per tile. 65 536 vertices, 98 304 indices (u32).
pub fn build_ground_mesh() -> Mesh {
    let n = GROUND_SIZE_TILES;
    let tiles = (n * n) as usize;
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(tiles * 4);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(tiles * 4);
    let mut colors: Vec<[f32; 4]> = Vec::with_capacity(tiles * 4);
    let mut indices: Vec<u32> = Vec::with_capacity(tiles * 6);

    let greens: Vec<[f32; 4]> = palette::GRASS
        .iter()
        .map(|c| c.to_linear().to_f32_array())
        .collect();

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
            let color = greens[tile_palette_index(x, z)];
            colors.extend_from_slice(&[color; 4]);
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
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_indices(Indices::U32(indices))
}

fn spawn_ground(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.95,
        reflectance: 0.2,
        metallic: 0.0,
        ..default()
    });
    commands.spawn((
        Name::new("Ground"),
        Ground,
        Mesh3d(meshes.add(build_ground_mesh())),
        MeshMaterial3d(material),
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
        let mesh = build_ground_mesh();
        let tiles = (GROUND_SIZE_TILES * GROUND_SIZE_TILES) as usize;
        assert_eq!(mesh.count_vertices(), tiles * 4);
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("expected u32 indices");
        };
        assert_eq!(indices.len(), tiles * 6);
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
