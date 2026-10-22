//! The command card and selection panel on the bottom bar (M3b).
//!
//! The card's buttons follow the selection:
//! - local units of a builder kind: one Build button per building kind,
//!   with its ramped cost (`SimView::building_cost`); a click enters
//!   [`OrderMode::Place`];
//! - one of the local player's complete buildings: one Train button per
//!   unit kind it trains (the kind's `trainer`), with the ramped cost
//!   (`SimView::unit_cost`); a click issues `Train`;
//! - one of their construction sites: a Cancel button (`Cancel` slot 0,
//!   full refund).
//!
//! A button the sim would reject is greyed, not hidden, and its tooltip is
//! the rejection's message (`SimView::check_train`, or for Build
//! `SimView::check_build_kind`: the age, the town limit and affordability,
//! since the site checks need a tile); clicking it shows the message
//! instead of sending the command. An enabled button's
//! tooltip shows what it does and costs. Text comes from the rules and
//! `data/rules/strings/en.ron`; the HUD hard-codes no number.
//!
//! The selection panel at the left of the bar names the selection: unit
//! kinds and counts, or the building with its state (construction
//! progress, Town tier, workers, training queue).
//!
//! Buttons issue commands from a `Pointer<Click>` observer, like the Stop
//! button, never from `Interaction::Pressed` (a click would fire twice);
//! `Interaction` drives the hover colour and the tooltip.

use bevy::picking::events::{Click, Pointer};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::text::FontSize;
use rules::Rules;
use rules::strings::fill;
use sim::{
    BuildingId, BuildingKindId, Command, Construction, QueueItem, RejectReason, SimView, UnitKindId,
};

use crate::economy_hud::{MessageState, UiStrings, reject_text};
use crate::hud::{BOTTOM_BAR_HEIGHT_PX, BottomBar, HUD_FONT_PX, HudRoot, PADDING_PX, spawn_hud};
use crate::orders::OrderMode;
use crate::palette;
use crate::selection::Selection;
use crate::sim_driver::{LOCAL_PLAYER, PendingCommands, SimHandle};

/// Card button size, logical px.
pub const CARD_BUTTON_PX: Vec2 = Vec2::new(112.0, 36.0);
/// Gap between card buttons, logical px.
pub const CARD_GAP_PX: f32 = 4.0;
/// Buttons per card row.
pub const CARD_COLUMNS: usize = 4;
/// Card button label size, logical px.
pub const CARD_FONT_PX: f32 = 12.0;
/// Width of the selection panel on the left of the bar, logical px.
pub const INFO_WIDTH_PX: f32 = 300.0;

/// What a card button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardAction {
    /// Enter placement for a building kind.
    Build(BuildingKindId),
    /// Queue a unit at a building.
    Train(BuildingId, UnitKindId),
    /// Cancel a construction site (slot 0).
    CancelSite(BuildingId),
}

/// A card button and what it does.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardButton(pub CardAction);

/// Whether a card button is usable now, and its tooltip.
#[derive(Component, Debug, Clone, Default, PartialEq, Eq)]
pub struct CardState {
    /// The sim would accept the action now.
    pub enabled: bool,
    /// Tooltip: the rejection's message when disabled, else what it costs.
    pub tooltip: String,
}

/// The cost line of a card button.
#[derive(Component, Debug, Clone, Copy)]
pub struct CardCost;

/// The node holding the card's buttons.
#[derive(Component, Debug, Clone, Copy)]
pub struct CardGrid;

/// The selection panel's text.
#[derive(Component, Debug, Clone, Copy)]
pub struct SelectionInfo;

/// The tooltip's text.
#[derive(Component, Debug, Clone, Copy)]
pub struct Tooltip;

/// The tooltip's panel, shown while a card button is hovered.
#[derive(Component, Debug, Clone, Copy)]
pub struct TooltipPanel;

/// What the card currently shows; buttons are rebuilt when it changes.
#[derive(Resource, Debug, Default, Clone, PartialEq, Eq)]
pub struct CardContent(pub Vec<CardAction>);

/// The command card, the selection panel and the tooltip.
pub struct CommandCardPlugin;

impl Plugin for CommandCardPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CardContent>()
            .add_systems(Startup, spawn_card.after(spawn_hud))
            .add_systems(
                Update,
                (
                    rebuild_card,
                    update_card_states,
                    card_button_colors,
                    update_tooltip,
                    update_selection_info,
                )
                    .chain(),
            );
    }
}

fn text(value: &str, size: f32) -> impl Bundle {
    (
        Text::new(value),
        TextFont {
            font_size: FontSize::Px(size),
            ..default()
        },
        TextColor(palette::HUD_TEXT),
    )
}

/// Adds the selection panel and the card grid to the bottom bar (before
/// the Stop button) and the hidden tooltip to the HUD root.
pub fn spawn_card(
    mut commands: Commands,
    bar: Query<Entity, With<BottomBar>>,
    root: Query<Entity, With<HudRoot>>,
) {
    if let Ok(bar) = bar.single() {
        let info = commands
            .spawn((
                Name::new("Selection panel"),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(PADDING_PX),
                    top: Val::Px(PADDING_PX),
                    width: Val::Px(INFO_WIDTH_PX),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|panel| {
                panel.spawn((SelectionInfo, text("", HUD_FONT_PX), Pickable::IGNORE));
            })
            .id();
        let width =
            CARD_COLUMNS as f32 * CARD_BUTTON_PX.x + (CARD_COLUMNS - 1) as f32 * CARD_GAP_PX;
        let grid = commands
            .spawn((
                Name::new("Command card"),
                CardGrid,
                Node {
                    width: Val::Px(width),
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    justify_content: JustifyContent::FlexEnd,
                    align_content: AlignContent::Center,
                    column_gap: Val::Px(CARD_GAP_PX),
                    row_gap: Val::Px(CARD_GAP_PX),
                    margin: UiRect::right(Val::Px(PADDING_PX)),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .id();
        // The grid goes before the Stop button, which stays at the right
        // end; the panel is absolutely positioned, so its place is moot.
        commands.entity(bar).insert_children(0, &[grid]);
        commands.entity(bar).add_children(&[info]);
    }
    if let Ok(root) = root.single() {
        commands.entity(root).with_children(|root| {
            root.spawn((
                Name::new("Tooltip"),
                TooltipPanel,
                Node {
                    position_type: PositionType::Absolute,
                    right: Val::Px(PADDING_PX),
                    bottom: Val::Px(BOTTOM_BAR_HEIGHT_PX + PADDING_PX),
                    padding: UiRect::all(Val::Px(6.0)),
                    display: Display::None,
                    ..default()
                },
                BackgroundColor(palette::HUD_PANEL),
                GlobalZIndex(20),
                Pickable::IGNORE,
            ))
            .with_children(|tip| {
                tip.spawn((Tooltip, text("", HUD_FONT_PX), Pickable::IGNORE));
            });
        });
    }
}

/// The actions the card offers for `selection`.
pub fn card_actions(view: &SimView<'_>, rules: &Rules, selection: &Selection) -> Vec<CardAction> {
    if let Some(id) = selection.building {
        let Some(b) = view.building(id).filter(|b| b.owner == LOCAL_PLAYER) else {
            return Vec::new();
        };
        return match b.construction {
            Construction::Site { .. } => vec![CardAction::CancelSite(id)],
            Construction::Complete => {
                let Some(kind) = rules.building_kind(b.kind.0) else {
                    return Vec::new();
                };
                rules
                    .units
                    .iter()
                    .enumerate()
                    .filter(|(_, u)| u.trainer == kind.id)
                    .map(|(i, _)| {
                        CardAction::Train(id, UnitKindId(u16::try_from(i).expect("kinds fit u16")))
                    })
                    .collect()
            }
        };
    }
    let builders = selection
        .units
        .iter()
        .filter_map(|u| view.unit(*u))
        .any(|u| u.owner == LOCAL_PLAYER && rules.unit_kind(u.kind.0).is_some_and(|k| k.builder));
    if !builders {
        return Vec::new();
    }
    (0..rules.buildings.len())
        .map(|i| CardAction::Build(BuildingKindId(u16::try_from(i).expect("kinds fit u16"))))
        .collect()
}

/// A cost vector as text: "100 Lumber", "40 Grain 10 Ore"; "free" for none.
pub fn cost_text(rules: &Rules, cost: &[i64]) -> String {
    let parts: Vec<String> = cost
        .iter()
        .zip(&rules.resources.resources)
        .filter(|(n, _)| **n > 0)
        .map(|(n, r)| format!("{n} {}", r.name))
        .collect();
    if parts.is_empty() {
        "free".to_string()
    } else {
        parts.join(" ")
    }
}

/// The label, cost and verdict of `action` now.
pub fn evaluate(
    view: &SimView<'_>,
    rules: &Rules,
    action: CardAction,
) -> (String, Vec<i64>, Result<(), RejectReason>) {
    match action {
        CardAction::Build(kind) => {
            let def = rules
                .building_kind(kind.0)
                .expect("card kinds come from the rules");
            let cost = view.building_cost(LOCAL_PLAYER, kind);
            let verdict = view.check_build_kind(LOCAL_PLAYER, kind).map(|_| ());
            (def.name.clone(), cost, verdict)
        }
        CardAction::Train(building, kind) => {
            let def = rules
                .unit_kind(kind.0)
                .expect("card kinds come from the rules");
            let cost = view.unit_cost(LOCAL_PLAYER, kind);
            let verdict = view.check_train(LOCAL_PLAYER, building, kind).map(|_| ());
            (def.name.clone(), cost, verdict)
        }
        CardAction::CancelSite(_) => (String::new(), Vec::new(), Ok(())),
    }
}

/// Rebuild the card's buttons when the actions on offer change.
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn rebuild_card(
    sim: NonSend<SimHandle>,
    selection: Res<Selection>,
    strings: Option<Res<UiStrings>>,
    mut content: ResMut<CardContent>,
    grid: Query<Entity, With<CardGrid>>,
    buttons: Query<Entity, With<CardButton>>,
    mut commands: Commands,
) {
    let Some(strings) = strings else {
        return;
    };
    let view = sim.view();
    let actions = card_actions(&view, sim.rules(), &selection);
    if actions == content.0 {
        return;
    }
    content.0.clone_from(&actions);
    for b in &buttons {
        commands.entity(b).despawn();
    }
    let Ok(grid) = grid.single() else {
        return;
    };
    let rules = sim.rules();
    commands.entity(grid).with_children(|grid| {
        for action in actions {
            let label = match action {
                CardAction::Build(kind) => fill(
                    strings.hud("button.building"),
                    &[("building", &rules.buildings[usize::from(kind.0)].name)],
                ),
                CardAction::Train(_, kind) => fill(
                    strings.hud("button.train"),
                    &[("unit", &rules.units[usize::from(kind.0)].name)],
                ),
                CardAction::CancelSite(_) => strings.hud("button.cancel").to_string(),
            };
            grid.spawn((
                Name::new(format!("Card {label}")),
                CardButton(action),
                CardState::default(),
                Button,
                Node {
                    width: Val::Px(CARD_BUTTON_PX.x),
                    height: Val::Px(CARD_BUTTON_PX.y),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                BackgroundColor(palette::HUD_BUTTON),
            ))
            .observe(on_card_click)
            .with_children(|button| {
                button.spawn((text(&label, CARD_FONT_PX), Pickable::IGNORE));
                button.spawn((CardCost, text("", CARD_FONT_PX), Pickable::IGNORE));
            });
        }
    });
}

/// A button's state now (enabled, and the tooltip: the rejection's message
/// when disabled, else what it does and costs) and its cost line.
pub fn card_state(
    view: &SimView<'_>,
    rules: &Rules,
    strings: &UiStrings,
    action: CardAction,
) -> (CardState, String) {
    let (name, cost, verdict) = evaluate(view, rules, action);
    let cost_line = match action {
        CardAction::CancelSite(_) => String::new(),
        _ => cost_text(rules, &cost),
    };
    let tooltip = match (&verdict, action) {
        (Err(reason), _) => reject_text(strings, rules, reason),
        (Ok(()), CardAction::Build(_)) => format!(
            "{}. {}",
            fill(strings.hud("tooltip.place"), &[("building", &name)]),
            fill(strings.hud("tooltip.cost"), &[("cost", &cost_line)])
        ),
        (Ok(()), CardAction::Train(..)) => format!(
            "{}: {}",
            fill(strings.hud("button.train"), &[("unit", &name)]),
            fill(strings.hud("tooltip.cost"), &[("cost", &cost_line)])
        ),
        (Ok(()), CardAction::CancelSite(_)) => strings.hud("tooltip.cancel_site").to_string(),
    };
    let state = CardState {
        enabled: verdict.is_ok(),
        tooltip,
    };
    (state, cost_line)
}

/// Every frame: each button's verdict, tooltip and cost line.
pub fn update_card_states(
    sim: NonSend<SimHandle>,
    strings: Option<Res<UiStrings>>,
    mut buttons: Query<(&CardButton, &mut CardState, &Children)>,
    mut costs: Query<&mut Text, With<CardCost>>,
) {
    let Some(strings) = strings else {
        return;
    };
    let view = sim.view();
    if view.player(LOCAL_PLAYER).is_none() {
        return;
    }
    let rules = sim.rules();
    for (button, mut state, children) in &mut buttons {
        let (want, cost_line) = card_state(&view, rules, &strings, button.0);
        if *state != want {
            *state = want;
        }
        for child in children {
            if let Ok(mut t) = costs.get_mut(*child)
                && t.0 != cost_line
            {
                t.0.clone_from(&cost_line);
            }
        }
    }
}

/// Hover and pressed colours; a disabled button stays greyed.
pub fn card_button_colors(
    mut buttons: Query<
        (&CardState, &Interaction, &mut BackgroundColor, &Children),
        With<CardButton>,
    >,
    mut texts: Query<&mut TextColor>,
) {
    for (state, interaction, mut background, children) in &mut buttons {
        let (bg, fg) = if !state.enabled {
            (palette::HUD_BUTTON_PRESSED, palette::HUD_DISABLED)
        } else {
            let bg = match interaction {
                Interaction::Pressed => palette::HUD_BUTTON_PRESSED,
                Interaction::Hovered => palette::HUD_BUTTON_HOVER,
                Interaction::None => palette::HUD_BUTTON,
            };
            (bg, palette::HUD_TEXT)
        };
        if background.0 != bg {
            background.0 = bg;
        }
        for child in children {
            if let Ok(mut color) = texts.get_mut(*child)
                && color.0 != fg
            {
                color.0 = fg;
            }
        }
    }
}

/// Show the hovered card button's tooltip above the bottom bar.
pub fn update_tooltip(
    buttons: Query<(&Interaction, &CardState), With<CardButton>>,
    mut tip: Query<&mut Text, With<Tooltip>>,
    mut panel: Query<&mut Node, With<TooltipPanel>>,
) {
    let hovered = buttons
        .iter()
        .find(|(i, _)| **i != Interaction::None)
        .map(|(_, s)| s.tooltip.clone());
    let display = if hovered.is_some() {
        Display::Flex
    } else {
        Display::None
    };
    for mut n in &mut panel {
        if n.display != display {
            n.display = display;
        }
    }
    if let Some(text) = hovered {
        for mut t in &mut tip {
            if t.0 != text {
                t.0.clone_from(&text);
            }
        }
    }
}

/// A primary click on a card button: run its action, or show why not.
#[allow(clippy::too_many_arguments)] // Bevy observer: each parameter is one resource or query
pub fn on_card_click(
    click: On<Pointer<Click>>,
    buttons: Query<(&CardButton, &CardState)>,
    mut mode: ResMut<OrderMode>,
    mut pending: ResMut<PendingCommands>,
    mut message: ResMut<MessageState>,
    time: Res<Time>,
) {
    if click.event().event.button != PointerButton::Primary {
        return;
    }
    let Ok((button, state)) = buttons.get(click.event().entity) else {
        return;
    };
    if !state.enabled {
        message.text.clone_from(&state.tooltip);
        message.since = time.elapsed_secs_f64();
        return;
    }
    match button.0 {
        CardAction::Build(kind) => *mode = OrderMode::Place(kind),
        CardAction::Train(building, kind) => {
            let seq = pending.push_local(Command::Train { building, kind });
            info!("Train seq {seq}");
        }
        CardAction::CancelSite(building) => {
            let seq = pending.push_local(Command::Cancel { building, slot: 0 });
            info!("Cancel site seq {seq}");
        }
    }
}

/// The selection panel's text for `selection`.
pub fn selection_text(
    view: &SimView<'_>,
    rules: &Rules,
    strings: &UiStrings,
    selection: &Selection,
) -> String {
    if let Some(id) = selection.building {
        let Some(b) = view.building(id) else {
            return String::new();
        };
        let Some(kind) = rules.building_kind(b.kind.0) else {
            return String::new();
        };
        let mut lines = vec![kind.name.clone()];
        match b.construction {
            Construction::Site { progress } => {
                let total = rules.build_time_ticks(kind).max(1);
                lines.push(fill(
                    strings.hud("info.site"),
                    &[("count", &(progress * 100 / total).to_string())],
                ));
            }
            Construction::Complete => {
                if let Some(t) = b.town_state {
                    let key = if t.seat { "info.seat" } else { "info.town" };
                    lines.push(fill(strings.hud(key), &[("count", &t.tier.to_string())]));
                }
                if let Some((worked, _, slots)) = view.worked_slots(id)
                    && slots > 0
                {
                    lines.push(fill(
                        strings.hud("info.workers"),
                        &[("count", &worked.to_string()), ("cap", &slots.to_string())],
                    ));
                }
                if !b.queue.is_empty() {
                    let items: Vec<String> = b
                        .queue
                        .iter()
                        .enumerate()
                        .map(|(i, e)| {
                            let (name, total) = match e.item {
                                QueueItem::Unit(k) => {
                                    let unit = rules
                                        .unit_kind(k.0)
                                        .expect("queued kinds are in the rules");
                                    (unit.name.as_str(), rules.train_time_ticks(unit))
                                }
                                QueueItem::Research { tech, ticks } => {
                                    let tech =
                                        rules.tech(tech.0).expect("queued techs are in the rules");
                                    (tech.name.as_str(), ticks)
                                }
                            };
                            if i == 0 {
                                format!("{name} {}%", e.progress * 100 / total.max(1))
                            } else {
                                name.to_string()
                            }
                        })
                        .collect();
                    lines.push(fill(
                        strings.hud("info.queue"),
                        &[("name", &items.join(", "))],
                    ));
                }
            }
        }
        return lines.join("\n");
    }
    let mut counts: std::collections::BTreeMap<u16, u32> = std::collections::BTreeMap::new();
    for u in selection.units.iter().filter_map(|id| view.unit(*id)) {
        *counts.entry(u.kind.0).or_default() += 1;
    }
    counts
        .iter()
        .filter_map(|(k, n)| {
            rules.unit_kind(*k).map(|u| {
                fill(
                    strings.hud("info.units"),
                    &[("name", &u.name), ("count", &n.to_string())],
                )
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every frame: the selection panel names the selection.
pub fn update_selection_info(
    sim: NonSend<SimHandle>,
    strings: Option<Res<UiStrings>>,
    selection: Res<Selection>,
    mut info: Query<&mut Text, With<SelectionInfo>>,
) {
    let Some(strings) = strings else {
        return;
    };
    let view = sim.view();
    let want = selection_text(&view, sim.rules(), &strings, &selection);
    for mut t in &mut info {
        if t.0 != want {
            t.0.clone_from(&want);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
            .expect("rules load")
    }

    struct NoAi;

    impl sim::AiController for NoAi {
        fn think(&mut self, _: sim::PlayerId, _: &SimView<'_>) -> Vec<Command> {
            Vec::new()
        }
    }

    /// A two-player match from the standard start: P0's Seat is
    /// `BuildingId(1)`, its Yeomen `UnitId(1..=5)`.
    fn standard() -> sim::Sim {
        let rules = rules();
        let mut setup = sim::MatchSetup::scenario(&rules, 1);
        setup.start = sim::StartKind::Standard;
        sim::Sim::new(setup, rules, Box::new(NoAi))
    }

    fn strings() -> UiStrings {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        UiStrings(rules::strings::Strings::load(data, "en").expect("strings load"))
    }

    fn kind(rules: &Rules, id: &str) -> BuildingKindId {
        BuildingKindId(
            u16::try_from(rules.buildings.iter().position(|b| b.id == id).unwrap()).unwrap(),
        )
    }

    #[test]
    fn builders_get_a_build_button_per_kind_with_its_ramped_cost() {
        let sim = standard();
        let (view, rules) = (sim.view(), sim.rules());
        let mut selection = Selection::default();
        selection.set((1..=5).map(sim::UnitId));
        let actions = card_actions(&view, rules, &selection);
        assert_eq!(actions.len(), rules.buildings.len());
        // The second Town is ramped (100 -> 120 Lumber) and affordable from
        // the 150 starting Lumber; a Watchtower needs Ore, which is 0.
        let (name, cost, verdict) = evaluate(&view, rules, CardAction::Build(kind(rules, "town")));
        assert_eq!(
            (name.as_str(), cost_text(rules, &cost), verdict),
            ("Town", "120 Lumber".into(), Ok(()))
        );
        let ore = u8::try_from(
            rules
                .resources
                .resources
                .iter()
                .position(|r| r.id == "ore")
                .unwrap(),
        )
        .unwrap();
        let (_, _, verdict) = evaluate(&view, rules, CardAction::Build(kind(rules, "watchtower")));
        assert_eq!(verdict, Err(RejectReason::CannotAfford { resource: ore }));
    }

    #[test]
    fn the_seat_trains_yeomen_and_names_itself() {
        let sim = standard();
        let (view, rules) = (sim.view(), sim.rules());
        let mut selection = Selection::default();
        selection.set_building(BuildingId(1));
        let actions = card_actions(&view, rules, &selection);
        let yeoman = UnitKindId(
            u16::try_from(rules.units.iter().position(|u| u.id == "yeoman").unwrap()).unwrap(),
        );
        assert_eq!(actions, [CardAction::Train(BuildingId(1), yeoman)]);
        let (_, cost, verdict) = evaluate(&view, rules, actions[0]);
        assert_eq!(
            cost_text(rules, &cost),
            "25 Grain",
            "the start Yeomen count (D11)"
        );
        assert_eq!(verdict, Ok(()));
        let text = selection_text(&view, rules, &strings(), &selection);
        assert_eq!(text, "Town\nSeat, tier 1");
        selection.set((1..=5).map(sim::UnitId));
        assert_eq!(
            selection_text(&view, rules, &strings(), &selection),
            "Yeoman x5"
        );
    }

    #[test]
    fn a_queued_research_shows_its_tech_name_and_progress() {
        let mut sim = standard();
        let rules = sim.rules().clone();
        let lore = u8::try_from(rules.resource_index("lore").unwrap()).unwrap();
        let letters = sim::TechId(rules.tech_by_id("letters_1").unwrap().0);
        let p = LOCAL_PLAYER;
        let cmds = [
            Command::DebugPlaceBuilding {
                owner: p,
                kind: kind(&rules, "scriptorium"),
                tile: sim::Tile::new(28, 67),
                complete: true,
            },
            Command::DebugGrant {
                owner: p,
                resource: lore,
                amount: 100,
            },
            Command::Research {
                building: BuildingId(3),
                tech: letters,
            },
        ];
        let cmds: Vec<_> = (0..)
            .zip(cmds)
            .map(|(seq, c)| sim::PlayerCommand::new(p, seq, c))
            .collect();
        sim.step(&cmds);
        let (_, ticks) = sim.view().research_quote(p, letters);
        // Applied during tick 2 (the command delay), which also gives the
        // head its first tick; then half the research time.
        for _ in 0..2 + ticks / 2 - 1 {
            sim.step(&[]);
        }
        let mut selection = Selection::default();
        selection.set_building(BuildingId(3));
        let text = selection_text(&sim.view(), &rules, &strings(), &selection);
        assert_eq!(text.lines().last(), Some("Queue: Letters I 50%"), "{text}");
    }

    #[test]
    fn at_the_walkthroughs_pop_cap_train_is_disabled_and_says_why() {
        use sim::scenarios::{ECONOMY_WALKTHROUGH_POPCAP_TICK, commands_at, economy_walkthrough};
        let rules = rules();
        let (setup, stream) = economy_walkthrough(&rules);
        let mut sim = sim::Sim::new(setup, rules, Box::new(NoAi));
        for t in 0..ECONOMY_WALKTHROUGH_POPCAP_TICK {
            sim.step(commands_at(&stream, t));
        }
        let (view, rules) = (sim.view(), sim.rules());
        assert_eq!(view.pop(LOCAL_PLAYER), (25, 25));
        let mut selection = Selection::default();
        selection.set_building(BuildingId(1));
        let actions = card_actions(&view, rules, &selection);
        assert!(
            matches!(actions.as_slice(), [CardAction::Train(BuildingId(1), _)]),
            "{actions:?}"
        );
        let (state, cost) = card_state(&view, rules, &strings(), actions[0]);
        assert_eq!(
            state,
            CardState {
                enabled: false,
                tooltip: "Population cap reached".into(),
            }
        );
        assert!(cost.ends_with("Grain"), "the cost still shows: {cost}");
    }

    #[test]
    fn another_players_building_offers_nothing() {
        let sim = standard();
        let mut selection = Selection::default();
        selection.set_building(BuildingId(2));
        assert!(card_actions(&sim.view(), sim.rules(), &selection).is_empty());
    }

    #[test]
    fn costs_read_as_amounts_and_names() {
        let r = rules();
        assert_eq!(cost_text(&r, &[0, 100, 0, 0]), "100 Lumber");
        assert_eq!(cost_text(&r, &[40, 0, 10, 0]), "40 Grain 10 Ore");
        assert_eq!(cost_text(&r, &[0, 0, 0, 0]), "free");
    }

    /// On a tile where every site check passes, a Build button's verdict
    /// is the sim's `check_build`: enabled, the first short resource, or
    /// the age.
    #[test]
    fn the_build_verdict_is_the_sims_on_a_valid_site() {
        let sim = standard();
        let (view, rules) = (sim.view(), sim.rules());
        let tile = sim::Tile::new(19, 67);
        let mut verdicts = Vec::new();
        for id in ["town", "scriptorium", "watchtower", "engine_yard"] {
            let k = kind(rules, id);
            let (_, _, verdict) = evaluate(&view, rules, CardAction::Build(k));
            let placed = view
                .check_build(LOCAL_PLAYER, sim::UnitId(1), k, tile)
                .map(|_| ());
            assert_eq!(verdict, placed, "{id}");
            verdicts.push(verdict);
        }
        let ore = u8::try_from(rules.resource_index("ore").unwrap()).unwrap();
        assert_eq!(
            verdicts,
            [
                Ok(()),
                Ok(()),
                Err(RejectReason::CannotAfford { resource: ore }),
                Err(RejectReason::AgeRequired),
            ]
        );
    }

    /// The Engine Yard needs the Masonry Age: in the Hearth Age its button
    /// is greyed with the age message although the starting Lumber pays
    /// for it, and it is enabled once the player has advanced.
    #[test]
    fn the_engine_yard_build_button_waits_for_the_masonry_age() {
        let mut sim = standard();
        let rules = sim.rules().clone();
        let p = LOCAL_PLAYER;
        let yard = CardAction::Build(kind(&rules, "engine_yard"));
        let (_, cost, verdict) = evaluate(&sim.view(), &rules, yard);
        assert_eq!(cost_text(&rules, &cost), "150 Lumber");
        assert!(
            sim.view()
                .economy(p)
                .stock
                .iter()
                .zip(&cost)
                .all(|(s, c)| s >= c),
            "affordable from the start"
        );
        assert_eq!(verdict, Err(RejectReason::AgeRequired));
        let (state, _) = card_state(&sim.view(), &rules, &strings(), yard);
        assert_eq!(
            state,
            CardState {
                enabled: false,
                tooltip: reject_text(&strings(), &rules, &RejectReason::AgeRequired),
            }
        );
        assert_eq!(state.tooltip, "Not available in this age");
        let tech = |id: &str| sim::TechId(rules.tech_by_id(id).unwrap().0);
        let res = |id: &str| u8::try_from(rules.resource_index(id).unwrap()).unwrap();
        let cmds = [
            Command::DebugResearch {
                owner: p,
                tech: tech("letters_1"),
            },
            Command::DebugResearch {
                owner: p,
                tech: tech("trade_1"),
            },
            Command::DebugGrant {
                owner: p,
                resource: res("grain"),
                amount: 250,
            },
            Command::DebugGrant {
                owner: p,
                resource: res("lore"),
                amount: 100,
            },
            Command::AdvanceAge { player: p },
        ];
        let cmds: Vec<_> = (0..)
            .zip(cmds)
            .map(|(seq, c)| sim::PlayerCommand::new(p, seq, c))
            .collect();
        sim.step(&cmds);
        for _ in 0..sim.setup().cmd_delay {
            sim.step(&[]);
        }
        assert_eq!(sim.view().age(p), 1, "the Masonry Age");
        let (_, cost, verdict) = evaluate(&sim.view(), &rules, yard);
        assert_eq!(
            (cost_text(&rules, &cost), verdict),
            ("150 Lumber".into(), Ok(()))
        );
        let (state, _) = card_state(&sim.view(), &rules, &strings(), yard);
        assert!(state.enabled, "{state:?}");
    }
}
