//! Territory on the ground (M3b): the ground's material is
//! [`GroundMaterial`], the `StandardMaterial` extended with [`FieldExt`],
//! whose fragment shader (`field.wgsl`, embedded in the binary) tints owned
//! land [`TERRITORY_TINT`] toward its owner's team colour and draws a soft
//! line about one tile wide, peaking at [`BORDER_TINT`], wherever ownership
//! changes (`docs/ART_STYLE.md`, "Territory and fog").
//!
//! The shader reads a field texture with one RGBA8 texel per sim tile: R
//! the owner code (0 neutral, else player index + 1), G the owning layer's
//! strength (saturating at 255), B fog (255, all visible, until M7), A
//! supply (0 until M7: M5a owner decision 10 defers the supply hatching to
//! M7, with fog). After every stepped tick the texture is rewritten
//! when the field's border sources differ from the ones it shows; the
//! sources determine every layer and owner, so nothing else can change it.
//! The GPU texture is updated in place (`GpuImage::prepare_asset` reuses a
//! texture of the same size), so the material's bind group stays valid.

use std::collections::BTreeMap;

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::image::ImageSampler;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;
use sim::{BuildingId, PlayerId, Source, SourceOwner, TerritoryField};

use crate::ground::HALF_EXTENT;
use crate::palette;
use crate::sim_driver::{SimHandle, SimSystems, sim_stepped};

/// How far owned land is blended toward its team colour
/// (`docs/ART_STYLE.md`: about 12%).
pub const TERRITORY_TINT: f32 = 0.12;
/// How far the centre of the border line is blended toward its team
/// colour; the line fades to nothing a half tile to either side.
pub const BORDER_TINT: f32 = 0.45;
/// Player slots the shader has team colours for; higher slots draw in the
/// last one (the palette's neutral grey).
pub const MAX_TEAMS: usize = 8;
/// Asset path of the embedded fragment shader.
const SHADER_PATH: &str = "embedded://game/field.wgsl";

/// The ground's material.
pub type GroundMaterial = ExtendedMaterial<StandardMaterial, FieldExt>;

/// The field texture and the shader's parameters. Bindings start at 100,
/// clear of the `StandardMaterial`'s.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct FieldExt {
    /// One texel per sim tile (see the module docs).
    #[texture(100)]
    pub field: Handle<Image>,
    /// Team colours, field placement and the two blend strengths.
    #[uniform(101)]
    pub params: FieldParams,
}

impl MaterialExtension for FieldExt {
    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }
}

/// `FieldParams` in `field.wgsl`.
#[derive(Clone, Copy, Debug, ShaderType, Reflect)]
pub struct FieldParams {
    /// Linear RGB per player slot; alpha unused.
    pub team: [Vec4; MAX_TEAMS],
    /// Field width and height in tiles.
    pub size: Vec2,
    /// World XZ of the field's north-west corner.
    pub origin: Vec2,
    /// [`TERRITORY_TINT`].
    pub tint: f32,
    /// [`BORDER_TINT`].
    pub border: f32,
}

impl FieldParams {
    /// Parameters for a `width` x `height` field laid on the ground from
    /// its north-west corner, as the ground mesh lays sim tiles.
    pub fn new(width: u32, height: u32) -> FieldParams {
        FieldParams {
            team: std::array::from_fn(|slot| {
                let slot = u8::try_from(slot).expect("MAX_TEAMS fits u8");
                palette::team_color(slot).to_linear().to_vec4()
            }),
            size: Vec2::new(width as f32, height as f32),
            origin: Vec2::splat(-HALF_EXTENT),
            tint: TERRITORY_TINT,
            border: BORDER_TINT,
        }
    }
}

/// The field texture and the border sources it currently shows.
#[derive(Resource, Debug)]
pub struct FieldTexture {
    image: Handle<Image>,
    shown: BTreeMap<BuildingId, Source>,
}

/// Registers the shader and the material, and keeps the field texture in
/// step with the sim.
pub struct FieldPlugin;

impl Plugin for FieldPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "field.wgsl");
        app.add_plugins(MaterialPlugin::<GroundMaterial>::default())
            .add_systems(
                FixedUpdate,
                sync_field.after(SimSystems::Step).run_if(sim_stepped),
            );
    }
}

/// The texture's bytes for `field`: four per tile, row-major.
pub fn field_texels(field: &TerritoryField) -> Vec<u8> {
    let owners = field.owner_grid();
    let mut texels = Vec::with_capacity(owners.len() * 4);
    for (i, &code) in owners.iter().enumerate() {
        let strength = match code {
            0 => 0,
            c => field.layer(SourceOwner::Player(PlayerId(c - 1)))[i],
        };
        texels.extend_from_slice(&[code, u8::try_from(strength).unwrap_or(u8::MAX), u8::MAX, 0]);
    }
    texels
}

/// A `width` x `height` RGBA8 field texture holding `texels`. Linear
/// (`Rgba8Unorm`), so owner codes read back exactly; kept in the main world
/// too so [`sync_field`] can rewrite it.
pub fn field_image(width: u32, height: u32, texels: Vec<u8>) -> Image {
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        texels,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::nearest();
    image
}

/// The ground material over `base` showing `field`, and the resource that
/// keeps it current. Without a field (no sim) the texture is one neutral
/// texel.
pub fn ground_material(
    base: StandardMaterial,
    field: Option<&TerritoryField>,
    images: &mut Assets<Image>,
) -> (GroundMaterial, FieldTexture) {
    let (width, height, texels, shown) = match field {
        Some(f) => (
            u32::from(f.width()),
            u32::from(f.height()),
            field_texels(f),
            f.sources().clone(),
        ),
        None => (1, 1, vec![0, 0, u8::MAX, 0], BTreeMap::new()),
    };
    let image = images.add(field_image(width, height, texels));
    let material = ExtendedMaterial {
        base,
        extension: FieldExt {
            field: image.clone(),
            params: FieldParams::new(width, height),
        },
    };
    (material, FieldTexture { image, shown })
}

/// After every tick: when the field's sources differ from the ones the
/// texture shows, rewrite the texture.
pub fn sync_field(
    sim: NonSend<SimHandle>,
    texture: Option<ResMut<FieldTexture>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut texture) = texture else {
        return;
    };
    let view = sim.view();
    let field = view.territory();
    if *field.sources() == texture.shown {
        return;
    }
    if let Some(mut image) = images.get_mut(&texture.image) {
        image.data = Some(field_texels(field));
    }
    texture.shown = field.sources().clone();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CPU mirror of `territory` in `field.wgsl`, line by line: the tint
    /// (team colour, coverage) and the border line (colour, strength) at
    /// tile-space point `p` over a row-major owner grid.
    fn territory_at(owners: &[u8], width: u32, height: u32, p: Vec2) -> (Vec4, Vec4) {
        let params = FieldParams::new(width, height);
        let owner_code = |tile: IVec2| -> u32 {
            let last = IVec2::new(width as i32, height as i32) - IVec2::ONE;
            let t = tile.clamp(IVec2::ZERO, last);
            u32::from(owners[(t.y * width as i32 + t.x) as usize])
        };
        let team_color =
            |code: u32| params.team[((code - 1) as usize).min(MAX_TEAMS - 1)].truncate();
        let (mut tint, mut line) = (Vec4::ZERO, Vec4::ZERO);
        if p.cmplt(Vec2::ZERO).any() || p.cmpge(params.size).any() {
            return (tint, line);
        }
        let q = p - Vec2::splat(0.5);
        let nw = q.floor().as_ivec2();
        let f = q - q.floor();
        let s = f * f * (Vec2::splat(3.0) - 2.0 * f);
        let codes = [
            owner_code(nw),
            owner_code(nw + IVec2::new(1, 0)),
            owner_code(nw + IVec2::new(0, 1)),
            owner_code(nw + IVec2::new(1, 1)),
        ];
        let weights = [
            (1.0 - s.x) * (1.0 - s.y),
            s.x * (1.0 - s.y),
            (1.0 - s.x) * s.y,
            s.x * s.y,
        ];
        let (mut tint_rgb, mut tint_cover) = (Vec3::ZERO, 0.0);
        let (mut line_rgb, mut line_sum, mut line_max) = (Vec3::ZERO, 0.0, 0.0_f32);
        for i in 0..4 {
            let code = codes[i];
            if code == 0 || codes[..i].contains(&code) {
                continue;
            }
            let m: f32 = (i..4)
                .filter(|&j| codes[j] == code)
                .map(|j| weights[j])
                .sum();
            let color = team_color(code);
            tint_rgb += m * color;
            tint_cover += m;
            let t = 1.0 - (2.0 * m - 1.0).abs();
            let l = t * t * (3.0 - 2.0 * t);
            line_rgb += l * m * color;
            line_sum += l * m;
            line_max = line_max.max(l);
        }
        if tint_cover > 0.0 {
            tint = (tint_rgb / tint_cover).extend(tint_cover);
        }
        if line_sum > 0.0 {
            line = (line_rgb / line_sum).extend(line_max);
        }
        (tint, line)
    }

    const N: u32 = 32;

    /// Player 0 owns the block x 4..12, y 4..12 of a 32 x 32 field; player
    /// 1 owns x 12..16 on the same rows; the rest is neutral.
    fn two_owners() -> Vec<u8> {
        let mut owners = vec![0u8; (N * N) as usize];
        for y in 4..12 {
            for x in 4..16 {
                owners[(y * N + x) as usize] = if x < 12 { 1 } else { 2 };
            }
        }
        owners
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn owned_land_is_tinted_and_neutral_land_only_beside_it() {
        let owners = two_owners();
        let blue = palette::team_color(0).to_linear().to_vec4().truncate();
        let (tint, line) = territory_at(&owners, N, N, Vec2::new(8.5, 8.5));
        assert!(close(tint.w, 1.0), "full cover inside: {tint}");
        assert!(tint.truncate().abs_diff_eq(blue, 1e-5), "{tint}");
        assert!(close(line.w, 0.0), "no line at a tile centre inside");
        let (tint, line) = territory_at(&owners, N, N, Vec2::new(24.5, 24.5));
        assert_eq!((tint, line), (Vec4::ZERO, Vec4::ZERO), "neutral");
        // Neutral tile (3, 8) beside player 0's west edge: untinted at its
        // centre, the owner's tint fading in under the line toward the edge
        // (docs/ART_STYLE.md, "Neutral or tied tiles").
        let centre = territory_at(&owners, N, N, Vec2::new(3.5, 8.5)).0;
        assert!(close(centre.w, 0.0), "{centre}");
        let near = territory_at(&owners, N, N, Vec2::new(3.75, 8.5)).0;
        assert!(near.w > 0.0 && near.w < 0.5, "fades in: {near}");
    }

    #[test]
    fn the_border_line_peaks_on_the_edge_and_fades_within_half_a_tile() {
        let owners = two_owners();
        // Player 0's west edge, x = 4, crossed along y = 8.5.
        let at = |x: f32| territory_at(&owners, N, N, Vec2::new(x, 8.5)).1.w;
        assert!(close(at(4.0), 1.0), "peak on the edge: {}", at(4.0));
        assert!(
            close(at(3.5), 0.0) && close(at(4.5), 0.0),
            "zero at centres"
        );
        assert!(at(3.75) > 0.1 && at(3.75) < 0.9, "soft: {}", at(3.75));
        assert!(close(at(3.75), at(4.25)), "symmetric across the edge");
        let mut last = at(4.0);
        for step in 1..=10 {
            let v = at(4.0 + step as f32 * 0.05);
            assert!(v <= last + 1e-6, "falls off monotonically");
            last = v;
        }
    }

    #[test]
    fn a_shared_border_is_one_line_in_both_team_colours() {
        let owners = two_owners();
        let blue = palette::team_color(0).to_linear().to_vec4().truncate();
        let red = palette::team_color(1).to_linear().to_vec4().truncate();
        let (_, line) = territory_at(&owners, N, N, Vec2::new(12.0, 8.5));
        assert!(close(line.w, 1.0));
        assert!(
            line.truncate().abs_diff_eq((blue + red) * 0.5, 1e-5),
            "an even mix on the edge: {line}"
        );
        let (_, west) = territory_at(&owners, N, N, Vec2::new(11.8, 8.5));
        assert!(
            west.truncate().distance(blue) < west.truncate().distance(red),
            "each side leans to its owner"
        );
    }

    #[test]
    fn corners_are_rounded_and_the_map_edge_is_no_border() {
        let owners = two_owners();
        // Diagonally past player 0's north-west corner tile (4, 4) the line
        // is weaker than on the straight edge: the isoline cuts the corner.
        let corner = territory_at(&owners, N, N, Vec2::new(4.0, 4.0)).1.w;
        assert!(corner > 0.0 && corner < 1.0, "{corner}");
        // A field owned edge to edge has no line anywhere along its rim.
        let all = vec![1u8; (N * N) as usize];
        for p in [
            Vec2::new(0.1, 0.1),
            Vec2::new(16.0, 0.2),
            Vec2::new(31.9, 31.9),
        ] {
            assert!(close(territory_at(&all, N, N, p).1.w, 0.0), "{p}");
        }
        // Outside the field nothing is drawn.
        assert_eq!(
            territory_at(&all, N, N, Vec2::new(-0.5, 3.0)),
            (Vec4::ZERO, Vec4::ZERO)
        );
    }

    #[test]
    fn texels_carry_owner_strength_fog_and_supply() {
        let mut field = TerritoryField::new(8, 8, 2);
        field.add_source(
            BuildingId(1),
            Source {
                owner: SourceOwner::Player(PlayerId(1)),
                centre: sim::Tile::new(2, 2),
                radius: 3,
            },
        );
        let texels = field_texels(&field);
        assert_eq!(texels.len(), 8 * 8 * 4);
        let at = |x: usize, y: usize| &texels[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4];
        assert_eq!(at(2, 2), [2, 3, 255, 0], "player 1, kernel peak 3");
        assert_eq!(at(7, 7), [0, 0, 255, 0], "neutral");
    }

    #[test]
    fn params_lay_the_field_on_the_ground_and_fill_every_team_slot() {
        let p = FieldParams::new(128, 128);
        assert_eq!(p.size, Vec2::splat(128.0));
        assert_eq!(p.origin, Vec2::splat(-HALF_EXTENT));
        assert_eq!(p.team[0], palette::team_color(0).to_linear().to_vec4());
        assert_eq!(p.team[1], palette::team_color(1).to_linear().to_vec4());
        assert_eq!(p.team[7], palette::team_color(7).to_linear().to_vec4());
    }
}
