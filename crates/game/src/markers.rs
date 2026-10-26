//! M5a markers on the ground: the attrition indicator under a unit that
//! is bleeding on enemy land, the supply radius ring around a selected
//! Supply Wain, the annexation timer around a contested Town, and the
//! annexation readout above the message line.
//!
//! - **Attrition indicator.** A flat diamond (a gizmo circle of 4 segments)
//!   in `palette::ATTRITION`, the Yield Cap amber, under every unit whose
//!   `exposed_ticks > 0`: the sim's own "bleeding" state, which does not
//!   flicker at a pulse (`docs/design/attrition.md`). One retained-gizmo
//!   entity per unit, keyed by `UnitId` and never a child of the unit
//!   (`docs/IMPLEMENTER_NOTES.md` rule 16), placed every frame at the
//!   unit's interpolated position, steady (nothing pulses,
//!   `docs/ART_STYLE.md`). Its half-diagonal is
//!   [`ATTRITION_MARKER_PER_UNIT_RADIUS`] unit radii, outside the selection
//!   ring, so its corners show around the unit's feet.
//! - **Supply ring.** A Timber circle (`palette::SUPPLY_RING`) of radius
//!   `supply_radius_tiles + 0.5` around each selected unit of a kind that
//!   `supplies`, centred on the tile the Wain stands on: the outer edge of
//!   the tiles step 5b' counts as covered (`dx² + dy² <= r²`, tile to
//!   tile). Also one retained gizmo per `UnitId`. The supply hatching in
//!   the field texture waits for M7 with fog (M5a owner decision 10).
//! - **Annexation timer.** For each Town under a running timer
//!   (`Hold::Contested`), an immediate-mode track circle in
//!   `palette::ANNEX_TRACK` just outside the units around the footprint
//!   and the attacker's team-coloured arc over it, filled clockwise from
//!   north by `(ticks + overstep) / total` ([`annex_fraction`]), both
//!   [`ANNEX_LINE_WIDTH_PX`] wide in their own gizmo group.
//! - **Annexation readout.** A line above the message line naming each
//!   running timer the local player is part of, with the time left until
//!   the flip: "Annexing Town: 0:42" or "Town is being annexed: 0:42"
//!   (`annex.attacking`, `annex.defending`). M4b's top bar has no room
//!   left, so the plan's fallback place is used.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, TAU};

use bevy::gizmos::config::GizmoLineConfig;
use bevy::prelude::*;
use bevy::text::FontSize;
use bevy::transform::TransformSystems;
use rules::Rules;
use rules::strings::fill;
use sim::{Building, PlayerId, SimView, UnitId, UnitKindId};

use crate::economy_hud::{MESSAGE_GAP_PX, UiStrings};
use crate::hud::{BOTTOM_BAR_HEIGHT_PX, HUD_FONT_PX, HudRoot, PADDING_PX, spawn_hud};
use crate::palette;
use crate::present::{Interp, UnitEntities, flat_on_ground, fx_to_f32, world_from_sim};
use crate::selection::{RING_LINE_WIDTH_PX, RING_Y, Selection, prune_dead};
use crate::sim_driver::{LOCAL_PLAYER, MatchOver, SimHandle};

/// The attrition diamond's half-diagonal as a multiple of the unit's
/// collision radius: outside the selection ring (1.4), so the diamond's
/// corners show around the unit's feet at the camera's pitch.
pub const ATTRITION_MARKER_PER_UNIT_RADIUS: f32 = 1.8;
/// Segments of the attrition marker's circle: 4 makes it a diamond.
pub const ATTRITION_MARKER_SIDES: u32 = 4;
/// Segments of the supply ring (a 14.5 m circle).
pub const SUPPLY_RING_RESOLUTION: u32 = 96;
/// Segments of the annexation track and arc.
pub const ANNEX_RESOLUTION: u32 = 64;
/// How far the annexation timer's circle lies outside the Town's
/// footprint, in metres: radius `max(w, h) / 2 + this`, 3.5 m for a 3 x 3
/// Town, just outside the units standing on the tiles around it (their
/// centres are 2 to 2.83 m from the Town's centre), which would hide a
/// circle drawn through them.
pub const ANNEX_RING_MARGIN: f32 = 2.0;
/// Line width of the annexation timer's track and arc, logical px: wider
/// than the 1 px of the default gizmo group (`ground.rs`), so the
/// progress reads at 40 m.
pub const ANNEX_LINE_WIDTH_PX: f32 = 3.0;
/// Gap between the annexation readout and the message line, logical px.
pub const ANNEX_READOUT_GAP_PX: f32 = 24.0;

/// The shared marker shapes (unit size; entities scale their `Transform`).
#[derive(Resource, Debug, Clone)]
pub struct MarkerAssets {
    /// A unit diamond laid flat, in `palette::ATTRITION`.
    pub attrition: Handle<GizmoAsset>,
    /// A unit circle laid flat, in `palette::SUPPLY_RING`.
    pub supply: Handle<GizmoAsset>,
}

/// The attrition diamond under a bleeding unit.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttritionMarker(pub UnitId);

/// The supply ring around a selected supplying unit.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupplyRing(pub UnitId);

/// Attrition marker entities by unit id.
#[derive(Resource, Default, Debug)]
pub struct AttritionMarkers(pub HashMap<UnitId, Entity>);

/// Supply ring entities by unit id.
#[derive(Resource, Default, Debug)]
pub struct SupplyRings(pub HashMap<UnitId, Entity>);

/// The annexation readout's text.
#[derive(Component, Debug, Clone, Copy)]
pub struct AnnexReadout;

/// The gizmo group the annexation timers draw in, with its own line width
/// ([`ANNEX_LINE_WIDTH_PX`]).
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct AnnexGizmos;

/// Attrition indicators, supply rings, annexation timers and the readout.
pub struct MarkersPlugin;

impl Plugin for MarkersPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AttritionMarkers>()
            .init_resource::<SupplyRings>()
            .insert_gizmo_config(
                AnnexGizmos,
                GizmoConfig {
                    line: GizmoLineConfig {
                        width: ANNEX_LINE_WIDTH_PX,
                        ..default()
                    },
                    ..default()
                },
            )
            .add_systems(
                Startup,
                (create_marker_assets, spawn_annex_readout.after(spawn_hud)),
            )
            .add_systems(Update, (draw_annexation_timers, update_annex_readout))
            .add_systems(
                PostUpdate,
                (sync_attrition_markers, sync_supply_rings)
                    .after(prune_dead)
                    .before(TransformSystems::Propagate),
            );
    }
}

/// Builds the attrition diamond and the supply circle, both of radius 1,
/// laid flat with `present::flat_on_ground()`.
pub fn create_marker_assets(mut commands: Commands, mut assets: ResMut<Assets<GizmoAsset>>) {
    commands.insert_resource(marker_assets(&mut assets));
}

/// The two shapes, added to `assets`.
pub fn marker_assets(assets: &mut Assets<GizmoAsset>) -> MarkerAssets {
    let flat = Isometry3d::new(Vec3::ZERO, flat_on_ground());
    let mut attrition = GizmoAsset::new();
    attrition
        .circle(flat, 1.0, palette::ATTRITION)
        .resolution(ATTRITION_MARKER_SIDES);
    let mut supply = GizmoAsset::new();
    supply
        .circle(flat, 1.0, palette::SUPPLY_RING)
        .resolution(SUPPLY_RING_RESOLUTION);
    MarkerAssets {
        attrition: assets.add(attrition),
        supply: assets.add(supply),
    }
}

fn line_config() -> GizmoLineConfig {
    GizmoLineConfig {
        width: RING_LINE_WIDTH_PX,
        ..default()
    }
}

/// `true` when units of `kind` carry supply (the Supply Wain).
pub fn supplies(rules: &Rules, kind: UnitKindId) -> bool {
    rules.unit_kind(kind.0).is_some_and(|k| k.supplies)
}

/// The supply ring's radius in metres: `supply_radius_tiles + 0.5`, the
/// outer edge of the tiles a Wain covers (14.5 with the shipped 14).
pub fn supply_ring_radius(rules: &Rules) -> f32 {
    rules.supply_radius_tiles as f32 + 0.5
}

/// How full the annexation arc is: `(ticks + overstep) / total`, clamped
/// to `0..=1` (a `total` of 0 is full).
pub fn annex_fraction(ticks: u32, total: u32, overstep: f32) -> f32 {
    if total == 0 {
        return 1.0;
    }
    ((ticks as f32 + overstep) / total as f32).clamp(0.0, 1.0)
}

/// Whole seconds left until the Town flips, rounded up: `total - ticks`
/// more steps at `tick_rate_hz` (the flip is in the step that brings
/// `ticks` to `total`, `sim::AnnexView`).
pub fn remaining_secs(ticks: u32, total: u32, tick_rate_hz: u32) -> u32 {
    total.saturating_sub(ticks).div_ceil(tick_rate_hz.max(1))
}

/// `m:ss` for a number of seconds: "1:21", "0:05", "45:00".
pub fn clock(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// The centre (at [`RING_Y`]) and radius of building `b`'s annexation
/// timer: the footprint's centre, and half its longer side plus
/// [`ANNEX_RING_MARGIN`].
pub fn annex_ring(rules: &Rules, b: &Building) -> (Vec3, f32) {
    let (w, h) = rules.building_kind(b.kind.0).map_or((1.0, 1.0), |k| {
        (k.width_tiles as f32, k.height_tiles as f32)
    });
    let centre = Vec3::new(
        f32::from(b.tile.x) + w * 0.5 - crate::ground::HALF_EXTENT,
        RING_Y,
        f32::from(b.tile.y) + h * 0.5 - crate::ground::HALF_EXTENT,
    );
    (centre, w.max(h) * 0.5 + ANNEX_RING_MARGIN)
}

/// One attrition diamond per unit with `exposed_ticks > 0`, at its
/// interpolated position (the pose its mesh is drawn at), scaled to
/// [`ATTRITION_MARKER_PER_UNIT_RADIUS`] times its radius; the diamond goes
/// when the exposure ends or the unit dies.
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn sync_attrition_markers(
    mut commands: Commands,
    sim: NonSend<SimHandle>,
    assets: Option<Res<MarkerAssets>>,
    mut markers: ResMut<AttritionMarkers>,
    entities: Res<UnitEntities>,
    interps: Query<&Interp>,
    mut transforms: Query<&mut Transform, With<AttritionMarker>>,
    fixed: Res<Time<Fixed>>,
) {
    let Some(assets) = assets else {
        return;
    };
    let view = sim.view();
    markers.0.retain(|id, marker| {
        if view.unit(*id).is_some_and(|u| u.exposed_ticks > 0) {
            true
        } else {
            commands.entity(*marker).despawn();
            false
        }
    });
    let t = fixed.overstep_fraction();
    for unit in view.units().filter(|u| u.exposed_ticks > 0) {
        let Some(interp) = entities.0.get(&unit.id).and_then(|e| interps.get(*e).ok()) else {
            continue;
        };
        let (pos, _) = interp.sample(t);
        let transform = Transform::from_translation(Vec3::new(pos.x, RING_Y, pos.z)).with_scale(
            Vec3::splat(fx_to_f32(unit.radius) * ATTRITION_MARKER_PER_UNIT_RADIUS),
        );
        match markers.0.get(&unit.id) {
            Some(marker) => {
                if let Ok(mut current) = transforms.get_mut(*marker) {
                    current.set_if_neq(transform);
                }
            }
            None => {
                let marker = commands
                    .spawn((
                        Name::new("Attrition marker"),
                        AttritionMarker(unit.id),
                        Gizmo {
                            handle: assets.attrition.clone(),
                            line_config: line_config(),
                            ..default()
                        },
                        transform,
                    ))
                    .id();
                markers.0.insert(unit.id, marker);
            }
        }
    }
}

/// One supply ring per selected unit of a kind that `supplies`, of radius
/// [`supply_ring_radius`], centred on the tile the unit stands on (the
/// centre of the sim's supply disc).
pub fn sync_supply_rings(
    mut commands: Commands,
    sim: NonSend<SimHandle>,
    selection: Res<Selection>,
    assets: Option<Res<MarkerAssets>>,
    mut rings: ResMut<SupplyRings>,
    mut transforms: Query<&mut Transform, With<SupplyRing>>,
) {
    let Some(assets) = assets else {
        return;
    };
    let view = sim.view();
    let rules = sim.rules();
    let wanted = |id: &UnitId| {
        selection.contains(*id) && view.unit(*id).is_some_and(|u| supplies(rules, u.kind))
    };
    rings.0.retain(|id, ring| {
        if wanted(id) {
            true
        } else {
            commands.entity(*ring).despawn();
            false
        }
    });
    let radius = supply_ring_radius(rules);
    for id in selection.units.iter().filter(|id| wanted(id)) {
        let Some(unit) = view.unit(*id) else {
            continue;
        };
        let centre = world_from_sim(view.map().center_of(view.map().tile_of(unit.pos)), RING_Y);
        let transform = Transform::from_translation(centre).with_scale(Vec3::splat(radius));
        match rings.0.get(id) {
            Some(ring) => {
                if let Ok(mut current) = transforms.get_mut(*ring) {
                    current.set_if_neq(transform);
                }
            }
            None => {
                let ring = commands
                    .spawn((
                        Name::new("Supply ring"),
                        SupplyRing(*id),
                        Gizmo {
                            handle: assets.supply.clone(),
                            line_config: line_config(),
                            ..default()
                        },
                        transform,
                    ))
                    .id();
                rings.0.insert(*id, ring);
            }
        }
    }
}

/// Every frame, for each Town under a running annexation timer: the track
/// circle and the attacker's arc ([`annex_ring`], [`annex_fraction`]),
/// clockwise from north. Once the match is decided the arc stops between
/// ticks (no overstep), as the frozen units do.
pub fn draw_annexation_timers(
    mut gizmos: Gizmos<AnnexGizmos>,
    sim: NonSend<SimHandle>,
    fixed: Res<Time<Fixed>>,
    over: Option<Res<MatchOver>>,
) {
    let view = sim.view();
    let rules = sim.rules();
    let overstep = if over.is_some() {
        0.0
    } else {
        fixed.overstep_fraction()
    };
    for b in view.buildings() {
        let Some(timer) = view.annexation(b.id) else {
            continue;
        };
        let (centre, radius) = annex_ring(rules, b);
        gizmos
            .circle(
                Isometry3d::new(centre, flat_on_ground()),
                radius,
                palette::ANNEX_TRACK,
            )
            .resolution(ANNEX_RESOLUTION);
        let frac = annex_fraction(timer.ticks, timer.total, overstep);
        if frac <= 0.0 {
            continue;
        }
        // `arc_3d` lies in the XZ plane, starts at +X and turns
        // counter-clockwise about +Y; turned a quarter it starts at -Z
        // (north, the top of the screen), and a negative angle runs
        // clockwise as seen from above.
        gizmos
            .arc_3d(
                -TAU * frac,
                radius,
                Isometry3d::new(centre, Quat::from_rotation_y(FRAC_PI_2)),
                palette::team_color(timer.attacker.0),
            )
            .resolution(ANNEX_RESOLUTION);
    }
}

/// The readout's text for `local` (the game passes `LOCAL_PLAYER`): one
/// entry per running timer `local` is part of, attacking ("Annexing Town:
/// 0:42") or defending ("Town is being annexed: 0:42"), in `BuildingId`
/// order; timers between other players are left out, and the text is
/// empty when there is none.
pub fn annex_readout_text(
    view: &SimView<'_>,
    rules: &Rules,
    strings: &UiStrings,
    local: PlayerId,
) -> String {
    let hz = rules.tick_rate_hz;
    let mut parts = Vec::new();
    for b in view.buildings() {
        let Some(timer) = view.annexation(b.id) else {
            continue;
        };
        let template = if timer.attacker == local {
            strings.hud("annex.attacking")
        } else if b.owner == local {
            strings.hud("annex.defending")
        } else {
            continue;
        };
        let name = rules
            .building_kind(b.kind.0)
            .map_or("?", |k| k.name.as_str());
        let time = clock(remaining_secs(timer.ticks, timer.total, hz));
        parts.push(fill(template, &[("building", name), ("time", &time)]));
    }
    parts.join("    ")
}

/// Adds the annexation readout line to the HUD root, centred above the
/// message line, ignored by picking.
pub fn spawn_annex_readout(mut commands: Commands, root: Query<Entity, With<HudRoot>>) {
    let Ok(root) = root.single() else {
        return;
    };
    commands.entity(root).with_children(|root| {
        root.spawn((
            Name::new("Annexation readout"),
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(BOTTOM_BAR_HEIGHT_PX + MESSAGE_GAP_PX + ANNEX_READOUT_GAP_PX),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                padding: UiRect::horizontal(Val::Px(PADDING_PX)),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|line| {
            line.spawn((
                AnnexReadout,
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(HUD_FONT_PX),
                    ..default()
                },
                TextColor(palette::HUD_TEXT),
                Pickable::IGNORE,
            ));
        });
    });
}

/// Every frame: the readout's text ([`annex_readout_text`]), written only
/// when it changed.
pub fn update_annex_readout(
    sim: NonSend<SimHandle>,
    strings: Option<Res<UiStrings>>,
    mut texts: Query<&mut Text, With<AnnexReadout>>,
) {
    let Some(strings) = strings else {
        return;
    };
    let want = annex_readout_text(&sim.view(), sim.rules(), &strings, LOCAL_PLAYER);
    for mut text in &mut texts {
        if text.0 != want {
            text.0 = want.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use std::path::Path;

    /// `player`'s units that are bleeding now (`exposed_ticks > 0`).
    fn bleeding(view: &SimView<'_>, player: PlayerId) -> Vec<UnitId> {
        view.units()
            .filter(|u| u.owner == player && u.exposed_ticks > 0)
            .map(|u| u.id)
            .collect()
    }

    fn rules() -> Rules {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap()
    }

    #[test]
    fn annexation_arc_fraction_and_remaining_time() {
        // Entry: an empty arc and the whole minute left at 20 Hz.
        assert_eq!(annex_fraction(0, 1200, 0.0), 0.0);
        assert_eq!(remaining_secs(0, 1200, 20), 60);
        assert_eq!(clock(remaining_secs(0, 1200, 20)), "1:00");
        // Halfway, between two ticks.
        assert!((annex_fraction(600, 1200, 0.5) - 600.5 / 1200.0).abs() < 1e-6);
        assert_eq!(remaining_secs(600, 1200, 20), 30);
        // One step before the flip: still a second on the clock, rounded up.
        assert_eq!(remaining_secs(1199, 1200, 20), 1);
        assert_eq!(clock(1), "0:01");
        // Clamped at both ends.
        assert_eq!(annex_fraction(1200, 1200, 0.9), 1.0);
        assert_eq!(annex_fraction(5000, 1200, 0.0), 1.0);
        assert_eq!(annex_fraction(0, 0, 0.0), 1.0);
        assert_eq!(remaining_secs(1300, 1200, 20), 0);
        // The overlay's match time: capture_seat decides at tick 1628.
        assert_eq!(clock(1628 / 20), "1:21");
        assert_eq!(clock(54000 / 20), "45:00");
        // The repository's timer is a minute (`annexation_ds: 600`).
        let rules = rules();
        assert_eq!(rules.annexation_ticks(), 1200);
        assert_eq!(
            clock(remaining_secs(
                0,
                rules.annexation_ticks(),
                rules.tick_rate_hz
            )),
            "1:00"
        );
    }

    /// The timer's circle runs outside every unit standing on a tile next
    /// to the Town (Chebyshev distance 1, where attackers stand), so they
    /// never hide it, and it is centred on the footprint.
    #[test]
    fn the_annexation_ring_lies_outside_the_attackers_posts() {
        let rules = rules();
        let (setup, stream, _) = sim::scenarios::by_name("capture_seat", &rules).unwrap();
        struct NoAi;
        impl sim::AiController for NoAi {
            fn think(
                &mut self,
                _: PlayerId,
                _: &SimView<'_>,
                _: &mut sim::AiMemory,
            ) -> Vec<sim::Command> {
                Vec::new()
            }
        }
        let mut sim = sim::Sim::new(setup, rules.clone(), Box::new(NoAi));
        sim.step(sim::scenarios::commands_at(&stream, 0));
        let view = sim.view();
        let seat = view.building(sim::BuildingId(2)).unwrap();
        let (centre, radius) = annex_ring(&rules, seat);
        assert_eq!(radius, 3.5);
        let unit_radius = rules
            .units
            .iter()
            .map(|k| k.radius_tiles_x100 as f32 / 100.0)
            .fold(0.0, f32::max);
        let (x0, y0) = (i32::from(seat.tile.x), i32::from(seat.tile.y));
        for x in x0 - 1..=x0 + 3 {
            for y in y0 - 1..=y0 + 3 {
                if (x0..x0 + 3).contains(&x) && (y0..y0 + 3).contains(&y) {
                    continue;
                }
                let post = world_from_sim(
                    view.map().center_of(sim::Tile::new(x as u16, y as u16)),
                    RING_Y,
                );
                assert!(
                    post.distance(centre) + unit_radius < radius,
                    "({x}, {y}) at {} m",
                    post.distance(centre)
                );
            }
        }
        let middle = world_from_sim(
            sim::FxVec2::new(
                sim::Fx::from_ratio(2 * x0 + 3, 2),
                sim::Fx::from_ratio(2 * y0 + 3, 2),
            ),
            RING_Y,
        );
        assert!(middle.distance(centre) < 1e-4, "{middle} vs {centre}");
    }

    #[test]
    fn supply_ring_only_for_supplying_kinds() {
        let rules = rules();
        let supplying: Vec<&str> = (0u16..)
            .zip(&rules.units)
            .filter(|(i, _)| supplies(&rules, UnitKindId(*i)))
            .map(|(_, k)| k.id.as_str())
            .collect();
        assert_eq!(supplying, ["supply_wain"]);
        assert!(!supplies(&rules, UnitKindId(999)), "an unknown kind");
        // The outer edge of the covered tiles: 14 + 0.5.
        assert_eq!(rules.supply_radius_tiles, 14);
        assert_eq!(supply_ring_radius(&rules), 14.5);
    }

    #[test]
    fn the_marker_shapes_are_a_flat_diamond_and_a_flat_circle() {
        let mut assets = Assets::<GizmoAsset>::default();
        let handles = marker_assets(&mut assets);
        let diamond = assets.get(&handles.attrition).unwrap();
        let ring = assets.get(&handles.supply).unwrap();
        // Four corners on the unit circle, flat on the ground: a diamond.
        assert!(diamond.list_positions.is_empty());
        let mut corners: Vec<Vec3> = Vec::new();
        for p in diamond.strip_positions.iter().filter(|p| p.is_finite()) {
            assert!(p.y.abs() < 1e-5, "flat on the ground: {p}");
            assert!((p.length() - 1.0).abs() < 1e-4, "unit radius: {p}");
            if !corners.iter().any(|c| c.distance(*p) < 1e-4) {
                corners.push(*p);
            }
        }
        assert_eq!(
            corners.len(),
            ATTRITION_MARKER_SIDES as usize,
            "{corners:?}"
        );
        let hue = LinearRgba::from(palette::ATTRITION);
        let drawn = |positions: &[Vec3], colors: &[LinearRgba], want: LinearRgba| {
            positions
                .iter()
                .zip(colors)
                .filter(|(p, _)| p.is_finite())
                .all(|(_, c)| *c == want)
        };
        assert!(drawn(&diamond.strip_positions, &diamond.strip_colors, hue));
        // The ring: a round circle of radius 1 in Timber.
        let points: Vec<Vec3> = ring
            .strip_positions
            .iter()
            .copied()
            .filter(|p| p.is_finite())
            .collect();
        assert!(points.len() >= SUPPLY_RING_RESOLUTION as usize);
        for p in &points {
            assert!(p.y.abs() < 1e-5 && (p.length() - 1.0).abs() < 1e-4, "{p}");
        }
        let timber = LinearRgba::from(palette::SUPPLY_RING);
        assert!(drawn(&ring.strip_positions, &ring.strip_colors, timber));
    }

    /// The markers follow the sim: a Bowman bleeding on P1's land in
    /// `capture_seat` gets a diamond from the step its exposure starts and
    /// loses it when the timer turns the land neutral; the Supply Wain gets
    /// its ring only while selected; the Seat's timer shows in the
    /// readout of the attacker.
    #[test]
    fn markers_follow_exposure_selection_and_the_timer() {
        use crate::sim_driver::{ScenarioInput, SimPlugin};
        use std::time::Duration;
        let rules = rules();
        let (setup, stream, _) = sim::scenarios::by_name("capture_seat", &rules).unwrap();
        let strings = UiStrings(
            rules::strings::Strings::load(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"),
                "en",
            )
            .unwrap(),
        );
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(Assets::<Mesh>::default())
            .insert_resource(Assets::<StandardMaterial>::default())
            .insert_resource(Assets::<GizmoAsset>::default())
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                Duration::ZERO,
            ))
            .insert_resource(strings.clone())
            .init_resource::<Selection>()
            .init_resource::<AttritionMarkers>()
            .init_resource::<SupplyRings>()
            .add_plugins((
                SimPlugin::new(
                    SimHandle::from_setup(setup, rules.clone()),
                    Box::new(ScenarioInput::new(stream)),
                ),
                crate::present::PresentPlugin,
            ))
            .add_systems(Startup, create_marker_assets)
            .add_systems(
                PostUpdate,
                (prune_dead, sync_attrition_markers, sync_supply_rings).chain(),
            );
        app.update();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_millis(50),
        ));
        let tick = |app: &App| app.world().non_send::<SimHandle>().tick();
        let run_to = |app: &mut App, t: u32| {
            while tick(app) < t {
                app.update();
            }
        };
        let bowman = UnitId(24);
        let wain = UnitId(23);
        let marked = |app: &mut App| -> Vec<UnitId> {
            let mut q = app.world_mut().query::<&AttritionMarker>();
            let mut ids: Vec<UnitId> = q.iter(app.world()).map(|m| m.0).collect();
            ids.sort();
            ids
        };
        run_to(&mut app, 10);
        {
            let sim = app.world().non_send::<SimHandle>();
            assert_eq!(bleeding(&sim.view(), LOCAL_PLAYER), vec![bowman]);
            assert_eq!(sim.view().unit(wain).unwrap().exposed_ticks, 0, "supplied");
        }
        assert_eq!(marked(&mut app), vec![bowman], "only the bleeding Bowman");
        let (marker_pos, bowman_pos) = {
            let world = app.world_mut();
            let mut q = world.query::<(&AttritionMarker, &Transform)>();
            let (_, t) = q.iter(world).next().unwrap();
            let entity = world.resource::<UnitEntities>().0[&bowman];
            (t.translation, world.get::<Interp>(entity).unwrap().curr)
        };
        assert!(
            (marker_pos.xz() - bowman_pos.xz()).length() < 0.6,
            "{marker_pos} under {bowman_pos}"
        );
        assert_eq!(marker_pos.y, RING_Y);

        // Select the Wain and a Skirmisher: one ring, the Wain's, of 14.5 m
        // around the centre of its tile.
        app.world_mut()
            .resource_mut::<Selection>()
            .set([wain, UnitId(15)]);
        app.update();
        let rings: Vec<(SupplyRing, Transform)> = {
            let world = app.world_mut();
            let mut q = world.query::<(&SupplyRing, &Transform)>();
            q.iter(world).map(|(r, t)| (*r, *t)).collect()
        };
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0].0, SupplyRing(wain));
        assert_eq!(rings[0].1.scale, Vec3::splat(14.5));
        let sim = app.world().non_send::<SimHandle>();
        let tile = sim.view().map().tile_of(sim.view().unit(wain).unwrap().pos);
        let c = rings[0].1.translation;
        assert_eq!(
            (
                c.x + crate::ground::HALF_EXTENT,
                c.z + crate::ground::HALF_EXTENT
            ),
            (f32::from(tile.x) + 0.5, f32::from(tile.y) + 0.5)
        );
        app.world_mut().resource_mut::<Selection>().clear();
        app.update();
        let mut q = app.world_mut().query::<&SupplyRing>();
        assert_eq!(q.iter(app.world()).count(), 0, "deselected");

        // No timer runs yet: the readout is empty for every player.
        {
            let sim = app.world().non_send::<SimHandle>();
            let view = sim.view();
            assert!(view.annexation(sim::BuildingId(2)).is_none());
            for p in [LOCAL_PLAYER, PlayerId(1)] {
                assert_eq!(annex_readout_text(&view, sim.rules(), &strings, p), "");
            }
        }

        // The timer starts when the Skirmishers reach the ring: the land
        // turns neutral, so the Bowman stops bleeding and loses its diamond,
        // and the attacker's readout counts down.
        for _ in 0..2000 {
            if app
                .world()
                .non_send::<SimHandle>()
                .view()
                .annexation(sim::BuildingId(2))
                .is_some()
            {
                break;
            }
            app.update();
        }
        app.update();
        let sim = app.world().non_send::<SimHandle>();
        let view = sim.view();
        let timer = view
            .annexation(sim::BuildingId(2))
            .expect("the Seat's timer runs");
        assert_eq!(timer.attacker, LOCAL_PLAYER);
        assert_eq!(view.unit(bowman).unwrap().exposed_ticks, 0);
        let readout = |p| annex_readout_text(&view, sim.rules(), &strings, p);
        assert_eq!(readout(LOCAL_PLAYER), "Annexing Town: 1:00", "{timer:?}");
        // The Seat's owner, P1, reads the defending text; a player who is
        // neither the attacker nor the owner sees nothing.
        assert_eq!(
            view.building(sim::BuildingId(2)).unwrap().owner,
            PlayerId(1)
        );
        assert_eq!(readout(PlayerId(1)), "Town is being annexed: 1:00");
        assert_eq!(readout(PlayerId(2)), "", "a bystander");
        // The HUD system writes the local player's text into the readout.
        let readout_entity = app.world_mut().spawn((AnnexReadout, Text::new(""))).id();
        app.world_mut()
            .run_system_once(update_annex_readout)
            .unwrap();
        assert_eq!(
            app.world().get::<Text>(readout_entity).unwrap().0,
            "Annexing Town: 1:00"
        );
        assert!(
            marked(&mut app).is_empty(),
            "no unit bleeds on neutral land"
        );
    }
}
