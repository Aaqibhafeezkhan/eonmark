//! The main menu (M5b owner decision 5): a full-window panel over the
//! not-yet-started match with the title, the opponent and map line, the
//! match's seed, and the New Game and Quit buttons in the command card's
//! style. It is spawned on entering `AppState::Menu` with
//! `DespawnOnExit(AppState::Menu)`, so New Game removes it.
//!
//! Clicks go through `Pointer<Click>` observers, as the HUD's Stop button
//! does (`hud::on_stop_click`): `Interaction` only drives the buttons'
//! colours. New Game sets `NextState(AppState::Skirmish)`; Quit writes
//! `AppExit::Success`, and since nothing has stepped no replay is written.
//!
//! `--new-game-after-seconds <s>` ([`click_new_game_after`]) is the New
//! Game proxy for automated runs: after `s` seconds it moves a synthetic
//! mouse to the centre of the real button, presses and releases there
//! (`hud::SyntheticMouse`, the `hud_click` check's path through
//! `bevy_picking`), prints `new-game: clicked at (x, y) after <t> s` and
//! puts the real cursor back (`hud::RealCursor`).

use std::time::Duration;

use bevy::app::AppExit;
use bevy::camera::RenderTarget;
use bevy::picking::events::{Click, Pointer};
use bevy::picking::pointer::{PointerButton, PointerInput};
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowRef};
use rules::strings::fill;

use super::AppState;
use crate::command_card::{CARD_BUTTON_PX, CARD_GAP_PX};
use crate::economy_hud::UiStrings;
use crate::hud::{
    HUD_CLICK_STEP_FRAMES, HUD_FONT_PX, PADDING_PX, RealCursor, SyntheticMouse, node_centre_logical,
};
use crate::outcome::TITLE_FONT_PX;
use crate::palette;
use crate::sim_driver::SimHandle;

/// The menu's z index: above the HUD panels, the tooltip and the outcome
/// overlay's 50.
pub const MENU_Z: i32 = 60;

/// The menu's full-window root.
#[derive(Component, Debug, Clone, Copy)]
pub struct MainMenu;

/// One of the menu's three text lines, by index: 0 title, 1 opponent and
/// map, 2 seed.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuLine(pub usize);

/// The menu's buttons.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuButton {
    /// Start the match against the Standard bot.
    NewGame,
    /// Quit the game.
    Quit,
}

/// The menu's three lines for a match on `map` with `seed`: "Eonmark",
/// "Skirmish against the Standard bot on `plains_1v1`", "Seed 1234".
pub fn menu_texts(map: &str, seed: u64, strings: &UiStrings) -> [String; 3] {
    [
        strings.hud("menu.title").to_string(),
        fill(strings.hud("menu.opponent"), &[("name", map)]),
        fill(strings.hud("menu.seed"), &[("seed", &seed.to_string())]),
    ]
}

/// A button's label: "New Game" or "Quit".
pub fn button_label(button: MenuButton, strings: &UiStrings) -> String {
    match button {
        MenuButton::NewGame => strings.hud("menu.new_game"),
        MenuButton::Quit => strings.hud("menu.quit"),
    }
    .to_string()
}

fn text(value: &str, size: f32) -> impl Bundle {
    (
        Text::new(value),
        crate::fonts::hud_font(size),
        TextColor(palette::HUD_TEXT),
        Pickable::IGNORE,
    )
}

/// `OnEnter(AppState::Menu)`: spawn the menu, a full-window column (all
/// four edges at 0 px) on the `palette::OUTCOME_SCRIM` slate at
/// [`MENU_Z`], with the three lines of [`menu_texts`] for the match already
/// built (its map and seed) and the two buttons in a row, each
/// `command_card::CARD_BUTTON_PX` like a card button. The root keeps the
/// `bevy_ui` picking default, so it blocks clicks on the world and the HUD
/// under it.
pub fn spawn_main_menu(
    mut commands: Commands,
    sim: NonSend<SimHandle>,
    strings: Option<Res<UiStrings>>,
) {
    let Some(strings) = strings else {
        return;
    };
    let lines = menu_texts(&sim.setup().map, sim.seed(), &strings);
    commands
        .spawn((
            Name::new("Main menu"),
            MainMenu,
            DespawnOnExit(AppState::Menu),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: Val::Px(0.0),
                bottom: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(PADDING_PX),
                ..default()
            },
            BackgroundColor(palette::OUTCOME_SCRIM),
            GlobalZIndex(MENU_Z),
        ))
        .with_children(|menu| {
            for (i, line) in lines.iter().enumerate() {
                let size = if i == 0 { TITLE_FONT_PX } else { HUD_FONT_PX };
                menu.spawn((MenuLine(i), text(line, size)));
            }
            menu.spawn((
                Name::new("Menu buttons"),
                Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(PADDING_PX),
                    margin: UiRect::top(Val::Px(PADDING_PX + CARD_GAP_PX)),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|row| {
                for button in [MenuButton::NewGame, MenuButton::Quit] {
                    let label = button_label(button, &strings);
                    let mut entity = row.spawn((
                        Name::new(format!("Menu button {label}")),
                        button,
                        Button,
                        Node {
                            width: Val::Px(CARD_BUTTON_PX.x),
                            height: Val::Px(CARD_BUTTON_PX.y),
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::Center,
                            ..default()
                        },
                        BackgroundColor(palette::HUD_BUTTON),
                    ));
                    match button {
                        MenuButton::NewGame => entity.observe(on_new_game_click),
                        MenuButton::Quit => entity.observe(on_quit_click),
                    };
                    entity.with_children(|b| {
                        b.spawn(text(&label, HUD_FONT_PX));
                    });
                }
            });
        });
}

/// A primary click on New Game: enter [`AppState::Skirmish`]; the sim
/// starts stepping, and the recorder starts, in that frame's fixed update.
pub fn on_new_game_click(click: On<Pointer<Click>>, mut next: ResMut<NextState<AppState>>) {
    if click.event().event.button != PointerButton::Primary {
        return;
    }
    info!("menu: New Game");
    next.set(AppState::Skirmish);
}

/// A primary click on Quit: `AppExit::Success`. Nothing has been recorded.
pub fn on_quit_click(click: On<Pointer<Click>>, mut exit: MessageWriter<AppExit>) {
    if click.event().event.button != PointerButton::Primary {
        return;
    }
    println!("menu: quit");
    exit.write(AppExit::Success);
}

/// Hover and pressed colours for the menu's buttons (`palette::HUD_BUTTON*`).
pub fn menu_button_colors(
    mut buttons: Query<
        (&Interaction, &mut BackgroundColor),
        (Changed<Interaction>, With<MenuButton>),
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

/// The New Game click proxy's state (`--new-game-after-seconds`).
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct NewGameAfter {
    /// Wall time after start-up before the click begins.
    pub after: Duration,
    /// Frames since the click began (move, press, release).
    pub frame: u32,
    /// The release was written.
    pub done: bool,
}

impl NewGameAfter {
    /// A proxy that clicks once `after` has passed.
    pub fn new(after: Duration) -> Self {
        Self {
            after,
            frame: 0,
            done: false,
        }
    }
}

/// `--new-game-after-seconds`: once the delay has passed and the New Game
/// button is laid out, move the synthetic mouse to the button's centre,
/// press the primary button [`HUD_CLICK_STEP_FRAMES`] frames later and
/// release it as many frames after that (the hover map updates in the next
/// frame's `PreUpdate`, and `Pointer<Click>` fires the frame after the
/// release), with the window cursor at the pointer for each of those
/// frames and the real one put back in `PostUpdate`. Runs only in
/// `AppState::Menu`.
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn click_new_game_after(
    time: Res<Time<Real>>,
    mut proxy: ResMut<NewGameAfter>,
    mut pointer_inputs: MessageWriter<PointerInput>,
    mut buttons: ResMut<ButtonInput<MouseButton>>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    menu_buttons: Query<(&ComputedNode, &UiGlobalTransform, &MenuButton)>,
    mut real_cursor: ResMut<RealCursor>,
) {
    if proxy.done || time.elapsed() < proxy.after {
        return;
    }
    let Some((node, transform, _)) = menu_buttons
        .iter()
        .find(|(_, _, b)| **b == MenuButton::NewGame)
    else {
        return;
    };
    if node.size.length_squared() <= 0.0 {
        return;
    }
    let Ok((window_entity, mut window)) = windows.single_mut() else {
        return;
    };
    let Some(target) = RenderTarget::Window(WindowRef::Primary).normalize(Some(window_entity))
    else {
        return;
    };
    let at = node_centre_logical(node, transform);
    real_cursor.0 = Some(window.physical_cursor_position());
    let frame = proxy.frame;
    proxy.frame += 1;
    if frame > 0 {
        window.set_cursor_position(Some(at));
    }
    let mut mouse = SyntheticMouse {
        target,
        pointer_inputs: &mut pointer_inputs,
        buttons: &mut buttons,
        window: &mut window,
    };
    const STEP: u32 = HUD_CLICK_STEP_FRAMES;
    match frame {
        0 => mouse.move_from(Vec2::ZERO, at),
        STEP => mouse.press_at(at, PointerButton::Primary),
        f if f == 2 * STEP => {
            mouse.release_at(at, PointerButton::Primary);
            proxy.done = true;
            println!(
                "new-game: clicked at ({:.0}, {:.0}) after {:.2} s",
                at.x,
                at.y,
                time.elapsed().as_secs_f64()
            );
        }
        _ => {}
    }
}
