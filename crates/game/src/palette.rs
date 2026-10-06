//! The M0 subset of the Eonmark palette: matte, low-saturation colours.
//! The full contract (<= 12 base hues plus two team colours) lands with
//! `docs/ART_STYLE.md` in M7. Judge changes on a P3 display and an sRGB
//! screenshot; wgpu can oversaturate on wide-gamut surfaces.

use bevy::prelude::*;

/// Three close-valued matte greens for grass tiles. Index 0 is the base.
pub const GRASS: [Color; 3] = [
    Color::srgb(0.455, 0.600, 0.396),
    Color::srgb(0.435, 0.580, 0.376),
    Color::srgb(0.475, 0.618, 0.414),
];

/// Grid gizmo lines: a darker green at partial alpha.
pub const GRID_LINE: Color = Color::srgba(0.247, 0.361, 0.208, 0.45);

/// Clear colour seen past the ground edge: soft grey-blue.
pub const SKY: Color = Color::srgb(0.62, 0.70, 0.74);

/// Directional light colour: warm white.
pub const SUN: Color = Color::srgb(1.0, 0.957, 0.878);

/// Ambient fill colour: cool sky bounce.
pub const AMBIENT: Color = Color::srgb(0.78, 0.85, 0.92);
