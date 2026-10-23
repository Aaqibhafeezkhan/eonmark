//! HUD skeleton (M2 decision 7): a top bar and a bottom bar in `bevy_ui`,
//! one placeholder Stop button, and the `PointerOverUi` resource that keeps
//! world clicks from falling through the HUD. Layout-only root nodes get
//! `Pickable::IGNORE`; panels keep the `bevy_ui` default (block lower), so a
//! click on a panel never reaches the ground or a unit.
//!
//! `PointerOverUi` is derived in `PreUpdate` after the picking hover map is
//! built: it is `true` when any entity hovered by the mouse pointer has a
//! `Node`. `orders.rs` and `selection.rs` read it in `Update` (their input
//! systems sit in [`WorldInputSet`]).
//!
//! The Stop button issues its order from a `Pointer<Click>` observer, not
//! from `Interaction::Pressed`: `bevy_ui`'s `Interaction` is driven by raw
//! `ButtonInput<MouseButton>` plus the window cursor, while the picking
//! pipeline (and the synthetic check below) goes through `PointerInput`; a
//! real click takes both paths, so only one may issue the command.
//! `Interaction` still drives the button's hover and pressed colours.
//!
//! `--scenario hud_click` turns on [`HudClickCheck`]: an automated check
//! that selects the spawned units, then writes synthetic
//! `bevy_picking::pointer::PointerInput` messages (move, press, release)
//! AND the matching `ButtonInput<MouseButton>` presses with the window
//! cursor moved to the same point for the frame (and put back in
//! `PostUpdate`, so `bevy_winit` never warps the real OS pointer; see
//! [`RealCursor`]). A positive control comes first: a right
//! click on open ground at the window centre must queue exactly one Move
//! (so the ground-order path is known to work). Then it clicks over the
//! empty bottom panel (left and right click) and over the Stop button, and
//! asserts that no Move was queued, the selection is unchanged and the
//! button issued a Stop. It prints `hud_click: ok` and exits 0, or
//! `hud_click: FAIL <why>` and exits 1 (`verdict_line`).

use std::collections::BTreeSet;

use bevy::app::AppExit;
use bevy::camera::{NormalizedRenderTarget, RenderTarget};
use bevy::picking::PickingSystems;
use bevy::picking::events::{Click, Pointer};
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::{Location, PointerAction, PointerButton, PointerId, PointerInput};
use bevy::prelude::*;
use bevy::text::FontSize;
use bevy::window::{PrimaryWindow, WindowRef};
use sim::{Command, UnitId, scenarios};

use crate::orders::{issue_stop, own_selected_units};
use crate::palette;
use crate::selection::Selection;
use crate::sim_driver::{LOCAL_PLAYER, MatchOver, PendingCommands, SimHandle, SimSystems};

/// Bottom bar height, logical px (authored in `Val::Px`: `UiScale` multiplies Px only).
pub const BOTTOM_BAR_HEIGHT_PX: f32 = 96.0;
/// Top bar height, logical px.
pub const TOP_BAR_HEIGHT_PX: f32 = 28.0;
/// Stop button size, logical px.
pub const BUTTON_SIZE_PX: Vec2 = Vec2::new(72.0, 28.0);
/// HUD padding, logical px.
pub const PADDING_PX: f32 = 8.0;
/// HUD text size, logical px.
pub const HUD_FONT_PX: f32 = 14.0;

/// Frames the `hud_click` check waits before its first synthetic input so
/// the HUD has laid out and the first tick has spawned the units.
pub const HUD_CLICK_SETTLE_FRAMES: u32 = 30;
/// Frames between two synthetic pointer steps: the hover map updates in
/// the next frame's `PreUpdate`, `Pointer<Click>` fires the frame after.
pub const HUD_CLICK_STEP_FRAMES: u32 = 3;
/// With `--screenshot`, how many frames after the capture was requested the
/// `hud_click` check waits for the PNG to land before printing its verdict
/// (and exiting) anyway.
pub const HUD_CLICK_SCREENSHOT_FRAMES: u32 = 120;

/// `true` while the mouse pointer is over any `bevy_ui` node. World input
/// (selection, orders, drag box) is ignored while set.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerOverUi(pub bool);

/// `Update` systems that turn raw mouse input into world actions (drag box,
/// ground orders). The `hud_click` check runs before this set so the
/// `ButtonInput` presses it fakes are seen in the same frame.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorldInputSet;

/// The full-window layout root (`Pickable::IGNORE`).
#[derive(Component, Debug, Clone, Copy)]
pub struct HudRoot;
/// The bottom panel (blocks picking).
#[derive(Component, Debug, Clone, Copy)]
pub struct BottomBar;
/// The top panel (blocks picking).
#[derive(Component, Debug, Clone, Copy)]
pub struct TopBar;
/// The placeholder Stop button on the bottom bar.
#[derive(Component, Debug, Clone, Copy)]
pub struct StopButton;

/// Stages of the automated `hud_click` check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudClickStage {
    /// Wait until the sim has spawned the units and the HUD has laid out.
    WaitForUnits,
    /// Select every unit (write `Selection` directly) and record the
    /// selection and the pending-command count.
    SelectAll,
    /// Positive control: synthetic move + right click on open ground at the
    /// window centre (not under the HUD); a few frames later exactly one
    /// Move must have been queued, proving the ground-order path works and
    /// the later "no Move" assertions are meaningful. The tally is reset
    /// before the HUD stages.
    ClickGround,
    /// Synthetic move + press over the bottom bar's empty area.
    PressPanel,
    /// Release over the bottom bar (then a right click there); a few frames
    /// later assert nothing changed.
    ReleasePanel,
    /// Synthetic move + press over the Stop button.
    PressButton,
    /// Release over the Stop button; a few frames later assert the
    /// selection is unchanged and a Stop was queued.
    ReleaseButton,
    /// Print the verdict and write `AppExit`.
    Verdict,
    /// Finished (the exit message is in flight).
    Done,
}

/// State of the automated `hud_click` check (inserted only under
/// `--scenario hud_click`).
#[derive(Resource, Debug, Clone)]
pub struct HudClickCheck {
    /// Current stage.
    pub stage: HudClickStage,
    /// Frames spent in the current stage (settling time between stages).
    pub frames_in_stage: u32,
    /// Selection captured in `SelectAll`.
    pub selection_before: BTreeSet<UnitId>,
    /// Moves (and `AttackMove`s) seen in `PendingCommands`.
    pub moves_seen: u32,
    /// Stops seen in `PendingCommands`.
    pub stops_seen: u32,
    /// Highest local `seq` already tallied, so a command is counted once
    /// however often the queue is inspected before a tick drains it.
    pub counted_through_seq: Option<u32>,
    /// Where the synthetic pointer is (logical px), for `Move` deltas.
    pub pointer_at: Vec2,
    /// `pointer_at` was set by a synthetic move: the window cursor follows
    /// it while the check runs.
    pub pointer_placed: bool,
    /// Failure reason, if any.
    pub failure: Option<String>,
}

impl Default for HudClickCheck {
    fn default() -> Self {
        Self {
            stage: HudClickStage::WaitForUnits,
            frames_in_stage: 0,
            selection_before: BTreeSet::new(),
            moves_seen: 0,
            stops_seen: 0,
            counted_through_seq: None,
            pointer_at: Vec2::ZERO,
            pointer_placed: false,
            failure: None,
        }
    }
}

impl HudClickCheck {
    /// Count local Moves and Stops waiting in `pending` that have not been
    /// tallied yet.
    pub fn tally(&mut self, pending: &PendingCommands) {
        for pc in pending.peek() {
            if pc.player != LOCAL_PLAYER || self.counted_through_seq.is_some_and(|s| pc.seq <= s) {
                continue;
            }
            self.counted_through_seq = Some(pc.seq);
            match &pc.cmd {
                Command::Move { .. } | Command::AttackMove { .. } => self.moves_seen += 1,
                Command::Stop { .. } => self.stops_seen += 1,
                _ => {}
            }
        }
    }

    fn advance(&mut self, stage: HudClickStage) {
        self.stage = stage;
        self.frames_in_stage = 0;
    }

    fn fail(&mut self, why: String) {
        if self.failure.is_none() {
            self.failure = Some(why);
        }
        self.advance(HudClickStage::Verdict);
    }

    /// After a synthetic click on `what`: no Move may have been queued and
    /// the selection must be unchanged. Fails the check and returns `false`
    /// otherwise.
    fn assert_world_untouched(&mut self, selection: &Selection, what: &str) -> bool {
        let why = if self.moves_seen > 0 {
            format!("{} Move(s) were queued by clicking {what}", self.moves_seen)
        } else if selection.units != self.selection_before {
            format!(
                "clicking {what} changed the selection: {} -> {} units",
                self.selection_before.len(),
                selection.units.len()
            )
        } else {
            return true;
        };
        self.fail(why);
        false
    }
}

/// The HUD skeleton and, optionally, the automated `hud_click` check.
pub struct HudPlugin {
    /// Run the `hud_click` automated check and exit with its verdict.
    pub hud_click_check: bool,
}

/// Stop [`WorldInputSet`] once the match is decided (M5a): from the
/// outcome on, no drag box, selection click, order or placement reaches
/// the frozen world (`outcome.rs` blocks picking under its overlay too).
pub fn gate_world_input(app: &mut App) {
    app.configure_sets(
        Update,
        WorldInputSet.run_if(not(resource_exists::<MatchOver>)),
    );
}

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        gate_world_input(app);
        app.init_resource::<PointerOverUi>()
            .add_systems(Startup, spawn_hud)
            .add_systems(
                PreUpdate,
                update_pointer_over_ui.after(PickingSystems::Hover),
            )
            .add_systems(Update, stop_button);
        if self.hud_click_check {
            app.init_resource::<HudClickCheck>()
                .add_systems(
                    FixedUpdate,
                    hud_click_tally_pending.before(SimSystems::Step),
                )
                .init_resource::<RealCursor>()
                .add_systems(Update, run_hud_click_check.before(WorldInputSet))
                .add_systems(PostUpdate, restore_real_cursor);
        }
    }
}

fn hud_text(text: &str) -> impl Bundle {
    (
        Text::new(text),
        TextFont {
            font_size: FontSize::Px(HUD_FONT_PX),
            ..default()
        },
        TextColor(palette::HUD_TEXT),
    )
}

/// Spawns the layout: a full-window root `Node` with `Pickable::IGNORE`
/// (`HudRoot`), an absolute-positioned bottom bar (`BottomBar`,
/// `BackgroundColor(palette::HUD_PANEL)`, height [`BOTTOM_BAR_HEIGHT_PX`])
/// holding one `Button` (`StopButton`, [`BUTTON_SIZE_PX`], text "Stop"),
/// and a top bar (`TopBar`, height [`TOP_BAR_HEIGHT_PX`]) that
/// `economy_hud` fills with the resource bar and pop readout. Panels and
/// the button keep `Pickable::default()`.
pub fn spawn_hud(mut commands: Commands) {
    commands
        .spawn((
            Name::new("HUD root"),
            HudRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                position_type: PositionType::Absolute,
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|root| {
            root.spawn((
                Name::new("Top bar"),
                TopBar,
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Px(TOP_BAR_HEIGHT_PX),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::SpaceBetween,
                    padding: UiRect::horizontal(Val::Px(PADDING_PX)),
                    ..default()
                },
                BackgroundColor(palette::HUD_PANEL),
            ));
            root.spawn((
                Name::new("Bottom bar"),
                BottomBar,
                Node {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(0.0),
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Px(BOTTOM_BAR_HEIGHT_PX),
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::FlexEnd,
                    padding: UiRect::all(Val::Px(PADDING_PX)),
                    ..default()
                },
                BackgroundColor(palette::HUD_PANEL),
            ))
            .with_children(|bar| {
                bar.spawn((
                    Name::new("Stop button"),
                    StopButton,
                    Button,
                    Node {
                        width: Val::Px(BUTTON_SIZE_PX.x),
                        height: Val::Px(BUTTON_SIZE_PX.y),
                        align_items: AlignItems::Center,
                        justify_content: JustifyContent::Center,
                        ..default()
                    },
                    BackgroundColor(palette::HUD_BUTTON),
                ))
                .observe(on_stop_click)
                .with_children(|button| {
                    button.spawn(hud_text("Stop"));
                });
            });
        });
}

/// `PointerOverUi = any hovered entity (mouse pointer) has a Node`.
pub fn update_pointer_over_ui(
    hover: Res<HoverMap>,
    nodes: Query<(), With<Node>>,
    mut over: ResMut<PointerOverUi>,
) {
    let hovering_ui = hover
        .get(&PointerId::Mouse)
        .is_some_and(|hits| hits.keys().any(|e| nodes.contains(*e)));
    if over.0 != hovering_ui {
        over.0 = hovering_ui;
    }
}

/// A primary `Pointer<Click>` on the Stop button issues `orders::issue_stop`
/// for the local player's selected units (observer attached by
/// [`spawn_hud`]).
fn on_stop_click(
    click: On<Pointer<Click>>,
    sim: NonSend<SimHandle>,
    selection: Res<Selection>,
    mut pending: ResMut<PendingCommands>,
) {
    if click.event().event.button != PointerButton::Primary {
        return;
    }
    let own = own_selected_units(&sim.view(), &selection);
    if let Some(seq) = issue_stop(&mut pending, &own) {
        info!("stop button: Stop seq {seq} for {} units", own.len());
    }
}

/// Hover and pressed colours for the Stop button (`palette::HUD_BUTTON*`).
pub fn stop_button(
    mut buttons: Query<
        (&Interaction, &mut BackgroundColor),
        (Changed<Interaction>, With<StopButton>),
    >,
) {
    for (interaction, mut background) in &mut buttons {
        background.0 = match interaction {
            Interaction::Pressed => palette::HUD_BUTTON_PRESSED,
            Interaction::Hovered => palette::HUD_BUTTON_HOVER,
            Interaction::None => palette::HUD_BUTTON,
        };
    }
}

/// Tally the local queue right before each tick drains it.
fn hud_click_tally_pending(mut check: ResMut<HudClickCheck>, pending: Res<PendingCommands>) {
    check.tally(&pending);
}

/// Centre of a laid-out UI node in logical window pixels.
fn node_centre_logical(node: &ComputedNode, transform: &UiGlobalTransform) -> Vec2 {
    transform.translation * node.inverse_scale_factor
}

/// The primary window's real cursor position (physical px, as
/// `Window::physical_cursor_position` reports it, which is what `bevy_winit`
/// compares), saved by the `hud_click` check before it puts the window
/// cursor at the synthetic pointer for one frame. The world-input systems
/// (`update_drag_box`, `issue_pointer_orders`) read
/// `Window::cursor_position`, so the check has to move it; but
/// `bevy_winit`'s `changed_windows` (in `Last`) warps the OS pointer whenever
/// the window's cursor differs from its cache, which made every `hud_click`
/// run jump the owner's mouse pointer to the game window.
/// [`restore_real_cursor`] writes the saved value back in `PostUpdate`, after
/// the world-input systems and before `Last`, so the cache never sees a
/// difference and nothing is warped.
#[derive(Resource, Debug, Default)]
pub struct RealCursor(Option<Option<Vec2>>);

/// `PostUpdate`: undo the `hud_click` check's per-frame cursor override
/// (see [`RealCursor`]).
pub fn restore_real_cursor(
    mut real: ResMut<RealCursor>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if let Some(position) = real.0.take()
        && let Ok(mut window) = windows.single_mut()
    {
        window.set_physical_cursor_position(position.map(|p| p.as_dvec2()));
    }
}

/// A synthetic mouse: the picking `PointerInput` stream plus the raw
/// `ButtonInput<MouseButton>` and window cursor the `bevy_ui` focus system
/// and the world-input systems read.
struct SyntheticMouse<'a, 'w> {
    target: NormalizedRenderTarget,
    pointer_inputs: &'a mut MessageWriter<'w, PointerInput>,
    buttons: &'a mut ButtonInput<MouseButton>,
    window: &'a mut Window,
}

impl SyntheticMouse<'_, '_> {
    fn write(&mut self, position: Vec2, action: PointerAction) {
        self.pointer_inputs.write(PointerInput::new(
            PointerId::Mouse,
            Location {
                target: self.target.clone(),
                position,
            },
            action,
        ));
    }

    fn move_to(&mut self, check: &mut HudClickCheck, position: Vec2) {
        let delta = position - check.pointer_at;
        check.pointer_at = position;
        check.pointer_placed = true;
        self.window.set_cursor_position(Some(position));
        self.write(position, PointerAction::Move { delta });
    }

    fn press(&mut self, check: &HudClickCheck, button: PointerButton) {
        self.buttons.press(mouse_button(button));
        self.write(check.pointer_at, PointerAction::Press(button));
    }

    fn release(&mut self, check: &HudClickCheck, button: PointerButton) {
        self.buttons.release(mouse_button(button));
        self.write(check.pointer_at, PointerAction::Release(button));
    }
}

fn mouse_button(button: PointerButton) -> MouseButton {
    match button {
        PointerButton::Primary => MouseButton::Left,
        PointerButton::Secondary => MouseButton::Right,
        PointerButton::Middle => MouseButton::Middle,
    }
}

/// The `hud_click` state machine (see the module docs and [`HudClickStage`]).
/// Positions come from the panels' `ComputedNode` + `UiGlobalTransform`;
/// each stage gives the hover map and `Pointer<Click>` a few frames between
/// steps ([`HUD_CLICK_STEP_FRAMES`]). Runs before [`WorldInputSet`].
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn run_hud_click_check(
    mut check: ResMut<HudClickCheck>,
    sim: NonSend<SimHandle>,
    mut selection: ResMut<Selection>,
    pending: Res<PendingCommands>,
    over_ui: Res<PointerOverUi>,
    mut pointer_inputs: MessageWriter<PointerInput>,
    mut buttons: ResMut<ButtonInput<MouseButton>>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    bars: Query<(&ComputedNode, &UiGlobalTransform), With<BottomBar>>,
    button: Query<(&ComputedNode, &UiGlobalTransform), With<StopButton>>,
    screenshot: Option<Res<crate::app::ScreenshotRequest>>,
    mut exit: MessageWriter<AppExit>,
    mut real_cursor: ResMut<RealCursor>,
) {
    use HudClickStage as S;
    if check.stage == S::Done {
        return;
    }
    let frame = check.frames_in_stage;
    check.frames_in_stage += 1;
    const STEP: u32 = HUD_CLICK_STEP_FRAMES;

    if check.stage == S::Verdict {
        // `--screenshot`: hold the verdict, and with it the exit, until the
        // capture was requested and its PNG is on disk (`take_screenshot`
        // removes a stale file first), so the owner proxy's picture shows
        // the HUD with the selection intact. Bounded so a failed save still
        // ends the check.
        if let Some(shot) = screenshot.as_deref() {
            if !shot.taken {
                check.frames_in_stage = 0;
                return;
            }
            if frame < HUD_CLICK_SCREENSHOT_FRAMES && !shot.path.exists() {
                return;
            }
        }
        check.tally(&pending);
        println!("{}", verdict_line(check.failure.as_deref()));
        exit.write(match check.failure {
            None => AppExit::Success,
            Some(_) => AppExit::error(),
        });
        check.advance(S::Done);
        return;
    }
    if sim.tick() > scenarios::HUD_CLICK_TICKS {
        let why = format!(
            "timed out in stage {:?} at tick {}",
            check.stage,
            sim.tick()
        );
        check.fail(why);
        return;
    }

    let Ok((window_entity, mut window)) = windows.single_mut() else {
        return;
    };
    // This frame only: the window cursor at the synthetic pointer for the
    // world-input systems; `restore_real_cursor` puts the real one back.
    real_cursor.0 = Some(window.physical_cursor_position());
    if check.pointer_placed {
        let at = check.pointer_at;
        window.set_cursor_position(Some(at));
    }
    let Some(target) = RenderTarget::Window(WindowRef::Primary).normalize(Some(window_entity))
    else {
        return;
    };
    let (Ok((bar_node, bar_tf)), Ok((button_node, button_tf))) = (bars.single(), button.single())
    else {
        return;
    };
    let bar_centre = node_centre_logical(bar_node, bar_tf);
    // Empty panel area: a quarter of the bar to the left of its centre,
    // well away from the Stop button at the right end.
    let panel_point = Vec2::new(
        bar_centre.x - bar_node.size.x * bar_node.inverse_scale_factor * 0.25,
        bar_centre.y,
    );
    let button_point = node_centre_logical(button_node, button_tf);
    // Open ground for the positive control: the window centre, between the
    // top and bottom bars (the camera is focused on the spawned units).
    let ground_point = Vec2::new(window.width(), window.height()) * 0.5;
    let mut mouse = SyntheticMouse {
        target,
        pointer_inputs: &mut pointer_inputs,
        buttons: &mut buttons,
        window: &mut window,
    };

    match check.stage {
        S::WaitForUnits => {
            let laid_out =
                bar_node.size.length_squared() > 0.0 && button_node.size.length_squared() > 0.0;
            if sim.view().unit_count() > 0 && laid_out && frame >= HUD_CLICK_SETTLE_FRAMES {
                check.advance(S::SelectAll);
            }
        }
        S::SelectAll => {
            selection.set(sim.view().units().map(|u| u.id));
            check.selection_before = selection.units.clone();
            check.tally(&pending);
            if check.selection_before.is_empty() {
                check.fail("no units to select".to_string());
            } else {
                info!(
                    "hud_click: selected {} units; panel point {panel_point}, button point {button_point}",
                    check.selection_before.len()
                );
                check.advance(S::ClickGround);
            }
        }
        S::ClickGround => match frame {
            0 => mouse.move_to(&mut check, ground_point),
            STEP => {
                if over_ui.0 {
                    check.fail("pointer over open ground is reported as over UI".to_string());
                } else {
                    mouse.press(&check, PointerButton::Secondary);
                }
            }
            f if f == 2 * STEP => mouse.release(&check, PointerButton::Secondary),
            f if f == 4 * STEP => {
                check.tally(&pending);
                if check.moves_seen != 1 {
                    let why = format!(
                        "a right click on open ground queued {} Move(s), expected 1",
                        check.moves_seen
                    );
                    check.fail(why);
                } else if selection.units != check.selection_before {
                    check.fail("a right click on open ground changed the selection".to_string());
                } else {
                    check.moves_seen = 0;
                    check.advance(S::PressPanel);
                }
            }
            _ => {}
        },
        S::PressPanel => match frame {
            0 => mouse.move_to(&mut check, panel_point),
            STEP => {
                if !over_ui.0 {
                    check
                        .fail("pointer over the bottom bar is not reported as over UI".to_string());
                } else {
                    mouse.press(&check, PointerButton::Primary);
                }
            }
            f if f == 2 * STEP => check.advance(S::ReleasePanel),
            _ => {}
        },
        S::ReleasePanel => match frame {
            0 => mouse.release(&check, PointerButton::Primary),
            STEP => mouse.press(&check, PointerButton::Secondary),
            f if f == 2 * STEP => mouse.release(&check, PointerButton::Secondary),
            f if f == 4 * STEP => {
                check.tally(&pending);
                if check.assert_world_untouched(&selection, "the empty bottom bar") {
                    check.advance(S::PressButton);
                }
            }
            _ => {}
        },
        S::PressButton => match frame {
            0 => mouse.move_to(&mut check, button_point),
            STEP => {
                if !over_ui.0 {
                    check.fail(
                        "pointer over the Stop button is not reported as over UI".to_string(),
                    );
                } else {
                    mouse.press(&check, PointerButton::Primary);
                }
            }
            f if f == 2 * STEP => check.advance(S::ReleaseButton),
            _ => {}
        },
        S::ReleaseButton => match frame {
            0 => mouse.release(&check, PointerButton::Primary),
            f if f == 4 * STEP => {
                check.tally(&pending);
                if !check.assert_world_untouched(&selection, "the Stop button") {
                    // failed; the verdict prints next frame
                } else if check.stops_seen == 0 {
                    check.fail("clicking the Stop button queued no Stop".to_string());
                } else {
                    check.advance(S::Verdict);
                }
            }
            _ => {}
        },
        S::Verdict | S::Done => {}
    }
}

/// Format the verdict line exactly as the scripts grep for it.
pub fn verdict_line(failure: Option<&str>) -> String {
    match failure {
        None => "hud_click: ok".to_string(),
        Some(why) => format!("hud_click: FAIL {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::FxVec2;

    #[test]
    fn verdict_lines_are_exact() {
        assert_eq!(verdict_line(None), "hud_click: ok");
        assert_eq!(
            verdict_line(Some("a Move was queued")),
            "hud_click: FAIL a Move was queued"
        );
    }

    /// The Stop button's observer orders only the local player's selected
    /// units: nothing for another player's units alone, the own ones of a
    /// mixed selection.
    #[test]
    fn the_stop_button_stops_only_own_units() {
        let rules =
            rules::Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
                .expect("rules load");
        let mut setup = sim::MatchSetup::scenario(&rules, 1);
        setup.start = sim::StartKind::Standard;
        // P0's Yeomen are 1..=5, P1's 6..=10.
        let mut handle = SimHandle::from_setup(setup, rules);
        for _ in 0..=handle.setup().cmd_delay {
            handle.step(&[]);
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<PendingCommands>()
            .init_resource::<Selection>();
        app.world_mut().insert_non_send(handle);
        let button = app
            .world_mut()
            .spawn(StopButton)
            .observe(on_stop_click)
            .id();
        app.update();
        let click = |app: &mut App| {
            app.world_mut().trigger(Pointer::new(
                PointerId::Mouse,
                Location {
                    target: NormalizedRenderTarget::None {
                        width: 1,
                        height: 1,
                    },
                    position: Vec2::ZERO,
                },
                Click {
                    button: PointerButton::Primary,
                    hit: bevy::picking::backend::HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
                    duration: std::time::Duration::ZERO,
                    count: 1,
                },
                button,
            ));
        };
        app.world_mut()
            .resource_mut::<Selection>()
            .set([UnitId(6), UnitId(7)]);
        click(&mut app);
        assert!(
            app.world().resource::<PendingCommands>().is_empty(),
            "another player's units are never ordered"
        );
        app.world_mut()
            .resource_mut::<Selection>()
            .set([UnitId(1), UnitId(6)]);
        click(&mut app);
        let cmds = app
            .world_mut()
            .resource_mut::<PendingCommands>()
            .drain_sorted();
        let cmds: Vec<&Command> = cmds.iter().map(|c| &c.cmd).collect();
        assert_eq!(
            cmds,
            [&Command::Stop {
                units: vec![UnitId(1)]
            }]
        );
    }

    #[test]
    fn tally_counts_each_local_command_once() {
        let mut pending = PendingCommands::default();
        let units = vec![UnitId(1)];
        pending.push_local(Command::Stop {
            units: units.clone(),
        });
        pending.push_local(Command::Move {
            units: units.clone(),
            target: FxVec2::ZERO,
            queue: false,
        });
        let mut check = HudClickCheck::default();
        check.tally(&pending);
        check.tally(&pending);
        assert_eq!((check.moves_seen, check.stops_seen), (1, 1));
        pending.push_local(Command::AttackMove {
            units,
            target: FxVec2::ZERO,
        });
        check.tally(&pending);
        assert_eq!((check.moves_seen, check.stops_seen), (2, 1));
        pending.drain_sorted();
        check.tally(&pending);
        assert_eq!((check.moves_seen, check.stops_seen), (2, 1));
    }
}
