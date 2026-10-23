//! The victory, defeat or draw overlay (M5a). When the sim decides the
//! match, `sim_driver::step_sim` finishes the replay, prints the `outcome:`
//! line and inserts [`MatchOver`]; [`show_outcome`] then fills and shows a
//! full-window panel over the frozen world with five lines: the result for
//! the local player (Victory, Defeat or Draw), the reason, the match time
//! and deciding tick, the final state hash and the replay's file name.
//! The hash is the one `sim-cli verify` prints for that replay, so the
//! owner can check it against the overlay (ROADMAP M5a acceptance 3).
//!
//! The overlay is minimal on purpose: the full game-over screen with its
//! statistics, Play Again and the `GameOver` state come with the menus
//! (M6, `docs/design/ui.md`). It keeps the `bevy_ui` picking default, so
//! it blocks every click on the world, the HUD's panels and the command
//! card under it; `hud::WorldInputSet` and the command card's systems stop
//! too (`run_if(not(resource_exists::<MatchOver>))`). The camera still
//! moves. Registered only by the windowed app, never by `--headless-run`.

use bevy::prelude::*;
use bevy::text::FontSize;
use rules::Rules;
use rules::strings::fill;
use sim::{Outcome, OutcomeReason, PlayerId, SimView};

use crate::economy_hud::UiStrings;
use crate::hud::{HUD_FONT_PX, PADDING_PX, spawn_hud};
use crate::markers::clock;
use crate::orders::OrderMode;
use crate::palette;
use crate::placement::PlacementGhost;
use crate::selection::DragBox;
use crate::sim_driver::{LOCAL_PLAYER, MatchOver, SimHandle};

/// The overlay's z index: above every HUD panel, the drag box (10) and the
/// tooltip.
pub const OUTCOME_Z: i32 = 50;
/// The title's font size (Victory, Defeat, Draw), logical px.
pub const TITLE_FONT_PX: f32 = 32.0;

/// The overlay's full-window root node, hidden until the match is decided.
#[derive(Component, Debug, Clone, Copy)]
pub struct OutcomeOverlay;

/// One of the overlay's five lines, by index: 0 title, 1 reason, 2 time,
/// 3 hash, 4 replay.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutcomeLine(pub usize);

/// The outcome overlay (windowed app only).
pub struct OutcomeOverlayPlugin;

impl Plugin for OutcomeOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_outcome_overlay.after(spawn_hud))
            .add_systems(
                Update,
                show_outcome
                    .run_if(resource_added::<MatchOver>)
                    // A `--screenshot-at` the deciding tick captures the
                    // frame that first shows the overlay.
                    .before(crate::app::take_tick_screenshots),
            );
    }
}

/// Spawns the hidden overlay: a full-window column (all four edges at
/// 0 px, so no percent units, `docs/design/ui.md`) on the
/// `palette::OUTCOME_SCRIM` scrim at [`OUTCOME_Z`], with five empty lines
/// centred in it.
pub fn spawn_outcome_overlay(mut commands: Commands) {
    commands
        .spawn((
            Name::new("Outcome overlay"),
            OutcomeOverlay,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: Val::Px(0.0),
                bottom: Val::Px(0.0),
                display: Display::None,
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(PADDING_PX),
                ..default()
            },
            BackgroundColor(palette::OUTCOME_SCRIM),
            GlobalZIndex(OUTCOME_Z),
        ))
        .with_children(|overlay| {
            for i in 0..5 {
                let size = if i == 0 { TITLE_FONT_PX } else { HUD_FONT_PX };
                overlay.spawn((
                    OutcomeLine(i),
                    Text::new(""),
                    TextFont {
                        font_size: FontSize::Px(size),
                        ..default()
                    },
                    TextColor(palette::HUD_TEXT),
                ));
            }
        });
}

/// The overlay's five lines for `over`, as `local` sees it:
///
/// 0. "Victory" when `local` won, "Defeat" when another player did, "Draw";
/// 1. the reason: "The Seat was captured", "Every Town was lost",
///    "Surrender", or "Territory at the time limit: 1248 tiles to 1245"
///    (the local player's owned tiles first, then the most any other
///    player owns);
/// 2. the match time and the deciding tick: "1:21, tick 1628";
/// 3. "Final hash 0x2c9a9ef52a5184d0", the hash `sim-cli verify` prints
///    for the replay;
/// 4. "Replay 20261023-120000-31.eonreplay", or empty when nothing was
///    recorded.
pub fn overlay_texts(
    over: &MatchOver,
    local: PlayerId,
    view: &SimView<'_>,
    rules: &Rules,
    strings: &UiStrings,
) -> [String; 5] {
    let title = match over.result.outcome {
        Outcome::Victory { player, .. } if player == local => strings.hud("outcome.victory"),
        Outcome::Victory { .. } => strings.hud("outcome.defeat"),
        Outcome::Draw => strings.hud("outcome.draw"),
    }
    .to_string();
    let reason = match over.result.reason {
        OutcomeReason::SeatCaptured => strings.hud("outcome.seat_captured").to_string(),
        OutcomeReason::AllTownsLost => strings.hud("outcome.all_towns_lost").to_string(),
        OutcomeReason::Surrender => strings.hud("outcome.surrender").to_string(),
        OutcomeReason::Territory => {
            let tiles = |p| view.territory().owned_tile_count(p);
            let others = view
                .players()
                .iter()
                .filter(|p| p.id != local)
                .map(|p| tiles(p.id))
                .max()
                .unwrap_or(0);
            fill(
                strings.hud("outcome.territory"),
                &[
                    ("count", &tiles(local).to_string()),
                    ("cap", &others.to_string()),
                ],
            )
        }
    };
    let tick = over.result.tick;
    let time = fill(
        strings.hud("outcome.time"),
        &[
            ("time", &clock(tick / rules.tick_rate_hz.max(1))),
            ("count", &tick.to_string()),
        ],
    );
    let hash = fill(
        strings.hud("outcome.hash"),
        &[("hash", &format!("{:#018x}", over.hash))],
    );
    let replay = over
        .replay
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|name| {
            fill(
                strings.hud("outcome.replay"),
                &[("name", &name.to_string_lossy())],
            )
        })
        .unwrap_or_default();
    [title, reason, time, hash, replay]
}

/// Once, when [`MatchOver`] appears: fill the five lines
/// ([`overlay_texts`]), show the overlay and leave any order mode (the
/// attack-move crosshair would otherwise stay over it). World input stops
/// with this frame, and with it the systems that would end a drag box or
/// hide the placement ghost, so a drag box held at the outcome is dropped
/// here (`selection::draw_drag_box` then hides its node) and the ghost is
/// hidden.
#[allow(clippy::too_many_arguments)]
pub fn show_outcome(
    over: Res<MatchOver>,
    sim: NonSend<SimHandle>,
    strings: Option<Res<UiStrings>>,
    mut overlay: Query<&mut Node, With<OutcomeOverlay>>,
    mut lines: Query<(&OutcomeLine, &mut Text)>,
    mode: Option<ResMut<OrderMode>>,
    drag: Option<ResMut<DragBox>>,
    mut ghosts: Query<&mut Visibility, With<PlacementGhost>>,
) {
    let Some(strings) = strings else {
        return;
    };
    let texts = overlay_texts(&over, LOCAL_PLAYER, &sim.view(), sim.rules(), &strings);
    for (line, mut text) in &mut lines {
        if let Some(want) = texts.get(line.0) {
            text.0.clone_from(want);
        }
    }
    for mut node in &mut overlay {
        node.display = Display::Flex;
    }
    if let Some(mut mode) = mode {
        *mode = OrderMode::Normal;
    }
    if let Some(mut drag) = drag {
        drag.0 = None;
    }
    for mut visibility in &mut ghosts {
        *visibility = Visibility::Hidden;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::{MatchResult, MatchSetup};
    use std::path::{Path, PathBuf};

    fn data() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    fn strings() -> UiStrings {
        UiStrings(rules::strings::Strings::load(data(), "en").unwrap())
    }

    fn over(outcome: Outcome, reason: OutcomeReason, tick: u32) -> MatchOver {
        MatchOver {
            result: MatchResult {
                outcome,
                reason,
                tick,
            },
            hash: 0x2c9a_9ef5_2a51_84d0,
            replay: Some(PathBuf::from("/r/20261023-120000-31.eonreplay")),
        }
    }

    #[test]
    fn overlay_texts_for_each_reason_and_side() {
        let rules = Rules::load(data()).unwrap();
        let s = strings();
        // The standard start gives each player a Seat that owns 1245 tiles
        // (m3a's `economy_standard_start_layout`); a complete P0 Watchtower
        // at (43, 64) adds 3 to P0 (m3a's two-Towns-and-a-Watchtower
        // golden), so the two counts differ and their order shows.
        let mut setup = MatchSetup::scenario(&rules, 1);
        setup.start = sim::StartKind::Standard;
        let delay = setup.cmd_delay;
        let mut sim = SimHandle::from_setup(setup, rules.clone());
        let (tower, tower_def) = rules.building_kind_by_id("watchtower").unwrap();
        let golden_radii =
            (rules.town_radius_tiles, tower_def.border_radius_tiles) == (20, Some(2));
        let (p0, p1) = (PlayerId(0), PlayerId(1));
        sim.step(&[sim::PlayerCommand::new(
            p0,
            0,
            sim::Command::DebugPlaceBuilding {
                owner: p0,
                kind: sim::BuildingKindId(tower),
                tile: sim::Tile::new(43, 64),
                complete: true,
            },
        )]);
        for _ in 0..delay {
            sim.step(&[]);
        }
        let view = sim.view();
        let tiles = |p| view.territory().owned_tile_count(PlayerId(p));
        assert!(tiles(0) > tiles(1), "{} : {}", tiles(0), tiles(1));
        if golden_radii {
            assert_eq!((tiles(0), tiles(1)), (1248, 1245));
        }
        let win = |player| Outcome::Victory {
            player,
            decisive: true,
        };
        let seat = over(win(p0), OutcomeReason::SeatCaptured, 1628);
        assert_eq!(
            overlay_texts(&seat, p0, &view, &rules, &s),
            [
                "Victory",
                "The Seat was captured",
                "1:21, tick 1628",
                "Final hash 0x2c9a9ef52a5184d0",
                "Replay 20261023-120000-31.eonreplay",
            ]
        );
        // The same result seen by the other player.
        assert_eq!(overlay_texts(&seat, p1, &view, &rules, &s)[0], "Defeat");
        let lost = over(win(p1), OutcomeReason::AllTownsLost, 2401);
        let texts = overlay_texts(&lost, p0, &view, &rules, &s);
        assert_eq!(texts[0], "Defeat");
        assert_eq!(texts[1], "Every Town was lost");
        assert_eq!(texts[2], "2:00, tick 2401");
        let quit = over(win(p0), OutcomeReason::Surrender, 3);
        assert_eq!(overlay_texts(&quit, p0, &view, &rules, &s)[1], "Surrender");
        let draw = over(Outcome::Draw, OutcomeReason::Surrender, 3);
        assert_eq!(overlay_texts(&draw, p0, &view, &rules, &s)[0], "Draw");
        // The tiebreak names both counts, the local player's first.
        let cap = over(
            Outcome::Victory {
                player: p0,
                decisive: false,
            },
            OutcomeReason::Territory,
            54000,
        );
        let line = |first: u32, second: u32| {
            format!("Territory at the time limit: {first} tiles to {second}")
        };
        let texts = overlay_texts(&cap, p0, &view, &rules, &s);
        assert_eq!(texts[0], "Victory");
        assert_eq!(texts[1], line(tiles(0), tiles(1)), "P0's count first");
        if golden_radii {
            assert_eq!(texts[1], "Territory at the time limit: 1248 tiles to 1245");
        }
        assert_eq!(texts[2], "45:00, tick 54000");
        let texts = overlay_texts(&cap, p1, &view, &rules, &s);
        assert_eq!(texts[0], "Defeat");
        assert_eq!(texts[1], line(tiles(1), tiles(0)), "P1's count first");
        if golden_radii {
            assert_eq!(texts[1], "Territory at the time limit: 1245 tiles to 1248");
        }
        // No recording: no replay line.
        let unrecorded = MatchOver {
            replay: None,
            ..seat.clone()
        };
        assert_eq!(overlay_texts(&unrecorded, p0, &view, &rules, &s)[4], "");
    }

    /// The overlay is hidden until `MatchOver` appears, then shows the
    /// five lines once; world input stops, an order mode is left, a drag
    /// box held at the outcome is dropped and its node hidden, a showing
    /// placement ghost is hidden, and a move marker placed just before the
    /// outcome still shrinks and expires (its timer is not world input).
    #[test]
    fn the_overlay_shows_once_the_match_is_over_and_input_stops() {
        use crate::hud::WorldInputSet;
        use crate::selection::{DragBoxNode, DragRect, MOVE_MARKER_SECS, MoveMarker};
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;
        let rules = Rules::load(data()).unwrap();
        let setup = MatchSetup::scenario(&rules, 1);
        #[derive(Resource, Default)]
        struct WorldInputRuns(u32);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                100,
            )))
            .insert_resource(strings())
            .init_resource::<OrderMode>()
            .init_resource::<DragBox>()
            .init_resource::<WorldInputRuns>()
            .add_systems(Startup, spawn_outcome_overlay)
            .add_systems(
                Update,
                (
                    show_outcome.run_if(resource_added::<MatchOver>),
                    (|mut runs: ResMut<WorldInputRuns>| runs.0 += 1).in_set(WorldInputSet),
                ),
            );
        crate::hud::gate_world_input(&mut app);
        crate::selection::add_world_input_feedback(&mut app);
        app.world_mut()
            .insert_non_send(SimHandle::from_setup(setup, rules));
        let drag_node = app
            .world_mut()
            .spawn((
                DragBoxNode,
                Node {
                    display: Display::None,
                    ..default()
                },
            ))
            .id();
        let ghost = app
            .world_mut()
            .spawn((PlacementGhost, Visibility::Inherited))
            .id();
        *app.world_mut().resource_mut::<OrderMode>() = OrderMode::AttackMove;
        app.update();
        let display = |app: &mut App| {
            let mut q = app
                .world_mut()
                .query_filtered::<&Node, With<OutcomeOverlay>>();
            q.single(app.world()).unwrap().display
        };
        assert_eq!(display(&mut app), Display::None);
        assert_eq!(app.world().resource::<WorldInputRuns>().0, 1);
        // The player is box-dragging and has just right-clicked a move.
        app.world_mut().resource_mut::<DragBox>().0 = Some(DragRect {
            start: Vec2::new(10.0, 10.0),
            current: Vec2::new(200.0, 150.0),
        });
        let now = app.world().resource::<Time>().elapsed_secs();
        let marker = app
            .world_mut()
            .spawn((
                MoveMarker {
                    expires_at: now + MOVE_MARKER_SECS,
                },
                Transform::default(),
            ))
            .id();
        app.update();
        let node_display = |app: &App| app.world().get::<Node>(drag_node).unwrap().display;
        assert_eq!(node_display(&app), Display::Flex, "the drag box shows");
        assert_eq!(app.world().resource::<WorldInputRuns>().0, 2);
        app.insert_resource(over(
            Outcome::Victory {
                player: LOCAL_PLAYER,
                decisive: true,
            },
            OutcomeReason::SeatCaptured,
            1628,
        ));
        app.update();
        assert_eq!(display(&mut app), Display::Flex);
        let mut q = app.world_mut().query::<(&OutcomeLine, &Text)>();
        let mut lines: Vec<(usize, String)> = q
            .iter(app.world())
            .map(|(l, t)| (l.0, t.0.clone()))
            .collect();
        lines.sort();
        assert_eq!(lines[0], (0, "Victory".to_string()));
        assert_eq!(lines[3], (3, "Final hash 0x2c9a9ef52a5184d0".to_string()));
        assert_eq!(*app.world().resource::<OrderMode>(), OrderMode::Normal);
        assert_eq!(
            app.world().resource::<DragBox>().0,
            None,
            "the drag box held at the outcome is dropped"
        );
        assert_eq!(
            app.world().get::<Visibility>(ghost),
            Some(&Visibility::Hidden),
            "the placement ghost is hidden"
        );
        app.update();
        assert_eq!(
            node_display(&app),
            Display::None,
            "the drag box node is hidden"
        );
        assert_eq!(
            app.world().resource::<WorldInputRuns>().0,
            2,
            "world input stopped at the outcome"
        );
        // The marker keeps shrinking and is gone once its time is up.
        for _ in 0..10 {
            app.update();
        }
        assert!(
            app.world().get_entity(marker).is_err(),
            "the move marker expired after the outcome"
        );
    }
}
