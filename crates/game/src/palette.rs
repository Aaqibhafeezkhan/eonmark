//! The M0-M2 subset of the Eonmark palette: matte, low-saturation colours.
//! The full contract (<= 12 base hues plus two team colours) lands with
//! `docs/ART_STYLE.md` in M7. Judge changes on a P3 display and an sRGB
//! screenshot; wgpu can oversaturate on wide-gamut surfaces.

use bevy::prelude::*;

/// Forest floor tiles (`docs/ART_STYLE.md`, #5C7A4A); impassable Lumber
/// sources.
pub const FOREST_FLOOR: Color = Color::srgb(0.361, 0.478, 0.290);

/// Mountain tiles (`docs/ART_STYLE.md` rock, #8C8578); impassable Ore
/// sources.
pub const ROCK: Color = Color::srgb(0.549, 0.522, 0.471);

/// Water tiles (`docs/ART_STYLE.md`, #5B7C8F); impassable.
pub const WATER: Color = Color::srgb(0.357, 0.486, 0.561);

/// Stone (`docs/ART_STYLE.md`, #B5AFA3): construction sites blend toward it.
pub const STONE: Color = Color::srgb(0.710, 0.686, 0.639);

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

/// The two team colours, indexed by `PlayerId.0`: slot 0 (the local player)
/// is a muted slate blue, slot 1 a muted brick red. Matte base colours for
/// unit materials; rings and markers use the brighter accents below.
pub const TEAM: [Color; 2] = [
    Color::srgb(0.329, 0.455, 0.627),
    Color::srgb(0.690, 0.345, 0.302),
];

/// Team colour for a player slot, falling back to neutral grey for a slot
/// the palette does not know (never happens in a two-player match).
pub fn team_color(slot: u8) -> Color {
    TEAM.get(usize::from(slot))
        .copied()
        .unwrap_or(Color::srgb(0.55, 0.55, 0.55))
}

/// Selection ring (retained gizmo circle under a selected unit): pale cream.
pub const SELECTION_RING: Color = Color::srgb(0.96, 0.93, 0.80);

/// Move marker ring at the click point, fading out: warm amber.
pub const MOVE_MARKER: Color = Color::srgba(0.90, 0.70, 0.35, 0.9);

/// Health bar track (#3A3A3F, M4b): the neutral dark grey the team-coloured
/// fill sits on. The fill is the owner's [`TEAM`] colour, never a
/// green-to-red ramp (team red is the [`INVALID`] hue).
pub const BAR_BACK: Color = Color::srgb(0.227, 0.227, 0.247);

/// Drag-box outline and fill (`bevy_ui` node drawn in screen space).
pub const DRAG_BOX_BORDER: Color = Color::srgba(0.96, 0.93, 0.80, 0.9);
/// Drag-box fill.
pub const DRAG_BOX_FILL: Color = Color::srgba(0.96, 0.93, 0.80, 0.12);

/// HUD panel background: matte dark slate at high alpha.
pub const HUD_PANEL: Color = Color::srgba(0.16, 0.18, 0.20, 0.92);
/// HUD button background.
pub const HUD_BUTTON: Color = Color::srgb(0.27, 0.30, 0.33);
/// HUD button background while hovered.
pub const HUD_BUTTON_HOVER: Color = Color::srgb(0.33, 0.37, 0.41);
/// HUD button background while pressed.
pub const HUD_BUTTON_PRESSED: Color = Color::srgb(0.21, 0.24, 0.27);
/// HUD text.
pub const HUD_TEXT: Color = Color::srgb(0.93, 0.92, 0.88);
/// Yield Cap amber (`docs/ART_STYLE.md`, #D29B3B): a rate readout while
/// production is discarded at the cap.
pub const HUD_AMBER: Color = Color::srgb(0.824, 0.608, 0.231);
/// Disabled HUD text and buttons (`docs/ART_STYLE.md`, #6B6B70).
pub const HUD_DISABLED: Color = Color::srgb(0.420, 0.420, 0.439);
/// Affordable or valid placement (`docs/ART_STYLE.md`, #6FA06A).
pub const VALID: Color = Color::srgb(0.435, 0.627, 0.416);
/// Unaffordable or invalid placement (`docs/ART_STYLE.md`, #B04A3F).
pub const INVALID: Color = Color::srgb(0.690, 0.290, 0.247);

/// Attrition indicator (M5a): the flat diamond under a unit bleeding on
/// enemy land, in the Yield Cap amber (#D29B3B), steady (nothing pulses,
/// `docs/ART_STYLE.md`). No new hue.
pub const ATTRITION: Color = HUD_AMBER;

/// Supply radius ring (M5a) around a selected Supply Wain: Timber
/// (#8B6B4A, `docs/ART_STYLE.md`, the Wain's own role colour).
pub const SUPPLY_RING: Color = Color::srgb(0.545, 0.420, 0.290);

/// Annexation timer track (M5a): the health bar's neutral dark grey
/// (#3A3A3F) at 80 % alpha, under the attacker's team-coloured progress
/// arc.
pub const ANNEX_TRACK: Color = Color::srgba(0.227, 0.227, 0.247, 0.8);

/// The outcome overlay's scrim (M5a): the HUD panel slate at 80 % alpha
/// over the frozen world.
pub const OUTCOME_SCRIM: Color = Color::srgba(0.16, 0.18, 0.20, 0.80);

/// Arrows in flight (art pass hit feedback, unlit): the dark neutral
/// (#3A3A3F, `docs/ART_STYLE.md`, "Neutral and fog"), a streak, not a glow.
pub const ARROW: Color = Color::srgb(0.227, 0.227, 0.247);
/// Mangonel stones in flight (unlit): Ore (#6E6A72, `docs/ART_STYLE.md`).
pub const STONE_SHOT: Color = Color::srgb(0.431, 0.416, 0.447);

/// The minimap's terrain layer (art pass): each tile's ground colour
/// scaled to this share of its value, so territory, buildings and units
/// stand out on it.
pub const MINIMAP_TERRAIN_SHADE: f32 = 0.70;
/// How far an owned tile on the minimap blends toward its owner's team
/// colour.
pub const MINIMAP_TERRITORY: f32 = 0.35;
/// How far an owned tile on the border (a 4-neighbour of another owner or
/// neutral land) blends toward its owner's team colour.
pub const MINIMAP_BORDER: f32 = 0.75;
/// How far a construction site on the minimap blends from its owner's
/// colour toward [`STONE`].
pub const MINIMAP_SITE: f32 = 0.5;
/// How far a unit's dot on the minimap is brightened from its team colour
/// toward white: 45%, so a dot stands out on its own team's tinted land
/// (the plan's 15% left P0's Yeomen nearly invisible on P0's territory in
/// the first lavapipe shot).
pub const MINIMAP_UNIT_BRIGHTEN: f32 = 0.45;
/// How far the camera's view trapezoid on the minimap blends toward
/// [`HUD_TEXT`] (it reads as the HUD text at 80%).
pub const MINIMAP_VIEW: f32 = 0.80;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_team_colours_and_a_neutral_fallback() {
        assert_eq!(team_color(0), TEAM[0]);
        assert_eq!(team_color(1), TEAM[1]);
        assert_ne!(team_color(0), team_color(1));
        assert_ne!(team_color(7), TEAM[0]);
        assert_ne!(team_color(7), TEAM[1]);
    }
}
