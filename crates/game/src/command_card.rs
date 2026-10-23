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
//!   full refund);
//! - local units with no builder among them and at least one armed unit
//!   (M4b): an Attack-move button, which enters [`OrderMode::AttackMove`]
//!   as the A key does;
//! - one of the local player's complete buildings that hosts techs (the
//!   Scriptorium, M4b research panel): after its Train buttons, one
//!   Research button per tech line, showing the line's next level
//!   ([`research_actions`]; owner decision 8, ROADMAP M4b Notes); a click
//!   issues `Research`.
//!
//! A button the sim would reject is greyed, not hidden, and its tooltip is
//! the rejection's message (`SimView::check_train`, `SimView::check_research`,
//! or for Build `SimView::check_build_kind`: the age, the town limit and
//! affordability, since the site checks need a tile); clicking it shows the
//! message instead of sending the command. An enabled button's tooltip
//! shows what it does and costs. A Research button's second line is its
//! cost, or what blocks it ("Masonry Age", "Researching 40%",
//! "Researched"), and its tooltip names the level before it and the age it
//! needs ("Requires Arms I, Masonry Age"). Text comes from the rules and
//! `data/rules/strings/en.ron`; the HUD hard-codes no number.
//!
//! The selection panel at the left of the bar names the selection: unit
//! kinds and counts, or the building with its state (construction
//! progress, Town tier, workers, the techs researched there, the training
//! and research queue). One selected unit of either player, or the
//! selected own building, also shows its hit points (M4b); another
//! player's building is not selectable yet (`selection::on_building_click`),
//! so its hit points show only on its bar once it is damaged. The panel
//! takes the room left of the card, up to [`INFO_WIDTH_PX`]: each line a
//! Scriptorium shows fits one row of that, so its five lines (name, hit
//! points, workers, researched, queue) fit the bar and the queue line stays
//! on screen.
//!
//! Buttons issue commands from a `Pointer<Click>` observer, like the Stop
//! button, never from `Interaction::Pressed` (a click would fire twice);
//! `Interaction` drives the hover colour and the tooltip. The colours and
//! the tooltip follow any entity with a [`CardState`] and an `Interaction`,
//! so the Advance button on the age strip under the top bar (`age_hud`)
//! shares them; its tooltip shows under the strip ([`TooltipPlace`]).

use bevy::picking::events::{Click, Pointer};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::text::FontSize;
use rules::Rules;
use rules::strings::fill;
use sim::{
    BuildingId, BuildingKindId, Command, Construction, PlayerId, QueueItem, RejectReason, SimView,
    TechId, UnitKindId,
};

use crate::age_hud::AGE_STRIP_HEIGHT_PX;
use crate::economy_hud::{MessageState, UiStrings, reject_text};
use crate::hud::{
    BOTTOM_BAR_HEIGHT_PX, BottomBar, HUD_FONT_PX, HudRoot, PADDING_PX, TOP_BAR_HEIGHT_PX, spawn_hud,
};
use crate::orders::OrderMode;
use crate::palette;
use crate::selection::Selection;
use crate::sim_driver::{LOCAL_PLAYER, PendingCommands, SimHandle};

/// Card button size, logical px. Every line a button can show fits one
/// line of it at [`CARD_FONT_PX`] (7.2 px a character): the longest are
/// "Needs Statecraft II" (19 characters; [`research_actions`] never offers
/// a level whose level before is missing, but the blocker exists) and the
/// Bowman at the military ramp's cap, "45 Grain 90 Lumber" (18).
pub const CARD_BUTTON_PX: Vec2 = Vec2::new(140.0, 36.0);
/// Gap between card buttons, logical px.
pub const CARD_GAP_PX: f32 = 4.0;
/// Buttons per card row.
pub const CARD_COLUMNS: usize = 4;
/// Card button label size, logical px.
pub const CARD_FONT_PX: f32 = 12.0;
/// Width of the selection panel on the left of the bar, logical px, when
/// the window leaves that much room before the card (the default 1280 px
/// window does). Every line a Scriptorium shows fits one row of it, the
/// longest being its queue of the four level III researches and a Scribe,
/// "Queue: Statecraft III 99%, Letters III, Trade III, Arms III, Scribe"
/// (67 characters of 8.4 px at the 14 px HUD font): its five lines then
/// fit the 88 px between the bar's top padding and the window's bottom.
pub const INFO_WIDTH_PX: f32 = 580.0;

/// What a card button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardAction {
    /// Enter placement for a building kind.
    Build(BuildingKindId),
    /// Queue a unit at a building.
    Train(BuildingId, UnitKindId),
    /// Cancel a construction site (slot 0).
    CancelSite(BuildingId),
    /// Enter attack-move mode for the selected units (M4b).
    AttackMove,
    /// Research a tech at a building (M4b).
    Research(BuildingId, TechId),
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

/// Where the tooltip of a button with a [`CardState`] shows: above the
/// bottom bar (the default, for the command card) or under the age strip
/// that hangs under the top bar (the Advance button, M4b). Both sit at the
/// right edge.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TooltipPlace {
    /// Above the bottom bar.
    #[default]
    AboveBottomBar,
    /// Under the age strip, which hangs under the top bar.
    BelowAgeStrip,
}

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
                    .chain()
                    // The card freezes with the world at the outcome (M5a);
                    // the overlay blocks its clicks.
                    .run_if(not(resource_exists::<crate::sim_driver::MatchOver>)),
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
        // The panel fills the room left of the card from the bar's top
        // padding down, so the card and the Stop button stay at the right
        // end; its text is at most INFO_WIDTH_PX wide, and a window too
        // narrow for that narrows it (the text wraps) instead of laying it
        // over the card. (Not an auto margin: taffy 0.10.1 still applies the
        // bar's `FlexEnd` after giving the free space to the margin.)
        let info = commands
            .spawn((
                Name::new("Selection panel"),
                Node {
                    flex_grow: 1.0,
                    flex_basis: Val::Px(0.0),
                    min_width: Val::Px(0.0),
                    align_self: AlignSelf::FlexStart,
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|panel| {
                panel.spawn((
                    SelectionInfo,
                    text("", HUD_FONT_PX),
                    Node {
                        max_width: Val::Px(INFO_WIDTH_PX),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
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
                    margin: UiRect::horizontal(Val::Px(PADDING_PX)),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .id();
        // The panel, then the grid, then the Stop button at the right end.
        commands.entity(bar).insert_children(0, &[info, grid]);
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
                let mut actions: Vec<CardAction> = rules
                    .units
                    .iter()
                    .enumerate()
                    .filter(|(_, u)| u.trainer == kind.id)
                    .map(|(i, _)| {
                        CardAction::Train(id, UnitKindId(u16::try_from(i).expect("kinds fit u16")))
                    })
                    .collect();
                actions.extend(research_actions(view, rules, id));
                actions
            }
        };
    }
    let own: Vec<_> = selection
        .units
        .iter()
        .filter_map(|u| view.unit(*u))
        .filter(|u| u.owner == LOCAL_PLAYER)
        .filter_map(|u| rules.unit_kind(u.kind.0))
        .collect();
    if own.iter().any(|k| k.builder) {
        return (0..rules.buildings.len())
            .map(|i| CardAction::Build(BuildingKindId(u16::try_from(i).expect("kinds fit u16"))))
            .collect();
    }
    if own.iter().any(|k| k.attack.is_some()) {
        return vec![CardAction::AttackMove];
    }
    Vec::new()
}

/// The tech lines a building kind hosts, in `techs.ron` order of each
/// line's first tech, with each line's techs by level: `(TechId, level)`.
fn hosted_lines<'a>(rules: &'a Rules, host: &str) -> Vec<(&'a str, Vec<TechId>)> {
    let mut lines: Vec<(&str, Vec<(u8, TechId)>)> = Vec::new();
    for (i, t) in rules
        .techs
        .iter()
        .enumerate()
        .filter(|(_, t)| t.host == host)
    {
        let id = TechId(u16::try_from(i).expect("techs fit u16"));
        match lines.iter_mut().find(|(line, _)| *line == t.line) {
            Some((_, levels)) => levels.push((t.level, id)),
            None => lines.push((t.line.as_str(), vec![(t.level, id)])),
        }
    }
    lines
        .into_iter()
        .map(|(line, mut levels)| {
            levels.sort_unstable();
            (line, levels.into_iter().map(|(_, id)| id).collect())
        })
        .collect()
}

/// The Research buttons of building `id` (M4b research panel, owner
/// decision 8): for each tech line its kind hosts, in `techs.ron` order,
/// the lowest level its owner has not researched (a level being researched
/// is not researched yet, so it keeps its button, greyed while in
/// progress), or the line's top level once every level is done (greyed,
/// "Researched"). Empty for a building that hosts no tech or does not
/// exist. Whether a button is enabled is the sim's `check_research`.
pub fn research_actions(view: &SimView<'_>, rules: &Rules, id: BuildingId) -> Vec<CardAction> {
    let Some(b) = view.building(id) else {
        return Vec::new();
    };
    let Some(kind) = rules.building_kind(b.kind.0) else {
        return Vec::new();
    };
    let researched = view.researched(b.owner);
    hosted_lines(rules, &kind.id)
        .into_iter()
        .filter_map(|(_, levels)| {
            levels
                .iter()
                .find(|t| !researched.contains(t))
                .or(levels.last())
                .map(|t| CardAction::Research(id, *t))
        })
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
        CardAction::Research(building, tech) => {
            let def = rules.tech(tech.0).expect("card techs come from the rules");
            let (cost, _) = view.research_quote(LOCAL_PLAYER, tech);
            let verdict = view
                .check_research(LOCAL_PLAYER, building, tech)
                .map(|_| ());
            (def.name.clone(), cost, verdict)
        }
        CardAction::CancelSite(_) | CardAction::AttackMove => (String::new(), Vec::new(), Ok(())),
    }
}

/// The command a click on an enabled button issues; `None` for the
/// buttons that enter an order mode instead (Build: placement;
/// Attack-move).
pub fn card_command(action: CardAction) -> Option<Command> {
    match action {
        CardAction::Train(building, kind) => Some(Command::Train { building, kind }),
        CardAction::CancelSite(building) => Some(Command::Cancel { building, slot: 0 }),
        CardAction::Research(building, tech) => Some(Command::Research { building, tech }),
        CardAction::Build(_) | CardAction::AttackMove => None,
    }
}

/// A button's first line: the building, unit or tech name, or what it does.
pub fn card_label(rules: &Rules, strings: &UiStrings, action: CardAction) -> String {
    match action {
        CardAction::Build(kind) => fill(
            strings.hud("button.building"),
            &[("building", &rules.buildings[usize::from(kind.0)].name)],
        ),
        CardAction::Train(_, kind) => fill(
            strings.hud("button.train"),
            &[("unit", &rules.units[usize::from(kind.0)].name)],
        ),
        CardAction::CancelSite(_) => strings.hud("button.cancel").to_string(),
        CardAction::AttackMove => strings.hud("button.attack_move").to_string(),
        CardAction::Research(_, tech) => fill(
            strings.hud("button.research"),
            &[("name", &rules.techs[usize::from(tech.0)].name)],
        ),
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
            let label = card_label(rules, &strings, action);
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

/// What the research of `tech` by player `p` has done so far, in percent of
/// its queued time: the entry at any of `p`'s buildings (only a queue's
/// head progresses, so a queued entry behind another reads 0).
fn research_percent(view: &SimView<'_>, p: PlayerId, tech: TechId) -> Option<u32> {
    view.buildings()
        .filter(|b| b.owner == p)
        .flat_map(|b| &b.queue)
        .find_map(|e| match e.item {
            QueueItem::Research { tech: t, ticks } if t == tech => {
                Some(e.progress * 100 / ticks.max(1))
            }
            _ => None,
        })
}

/// What a research of `tech` needs: the level before it in its line and
/// the age that unlocks it, as "Requires Arms I, Masonry Age"; `None` for
/// a first level of the first age.
pub fn requires_text(rules: &Rules, strings: &UiStrings, tech: TechId) -> Option<String> {
    let mut needs: Vec<&str> = Vec::new();
    if let Some(prev) = rules.tech_predecessor(tech.0).and_then(|t| rules.tech(t)) {
        needs.push(&prev.name);
    }
    let age = usize::from(rules.age_of_tech(tech.0));
    if age > 0 {
        needs.push(&rules.ages[age].name);
    }
    (!needs.is_empty()).then(|| {
        fill(
            strings.hud("tooltip.requires"),
            &[("name", &needs.join(", "))],
        )
    })
}

/// A Research button's second line and tooltip from the sim's verdict.
/// Enabled: the cost, and "Research Arms I, 30 s. Costs ..." with the
/// quoted time. Greyed: the cost when only the stockpile is short (the
/// tooltip names the resource), else what blocks it (the age that unlocks
/// it, "Needs" the level before, "Researching n%", "Researched"); the
/// tooltip is the rejection's message plus what the level requires.
fn research_lines(
    view: &SimView<'_>,
    rules: &Rules,
    strings: &UiStrings,
    tech: TechId,
    cost_line: String,
    verdict: &Result<(), RejectReason>,
) -> (String, String) {
    let name = &rules
        .tech(tech.0)
        .expect("card techs come from the rules")
        .name;
    let Err(reason) = verdict else {
        let (_, ticks) = view.research_quote(LOCAL_PLAYER, tech);
        let hz = rules.tick_rate_hz.max(1);
        let secs = (ticks + hz / 2) / hz;
        let tooltip = format!(
            "{}. {}",
            fill(
                strings.hud("tooltip.research"),
                &[("name", name), ("count", &secs.to_string())]
            ),
            fill(strings.hud("tooltip.cost"), &[("cost", &cost_line)])
        );
        return (cost_line, tooltip);
    };
    let line = match reason {
        RejectReason::CannotAfford { .. } => cost_line,
        RejectReason::AgeRequired => rules.ages[usize::from(rules.age_of_tech(tech.0))]
            .name
            .clone(),
        RejectReason::PrerequisiteMissing => {
            let prev = rules
                .tech_predecessor(tech.0)
                .and_then(|t| rules.tech(t))
                .map_or("?", |t| t.name.as_str());
            fill(strings.hud("button.needs"), &[("name", prev)])
        }
        RejectReason::ResearchInProgress => {
            let pct = research_percent(view, LOCAL_PLAYER, tech).unwrap_or(0);
            fill(
                strings.hud("button.researching"),
                &[("count", &pct.to_string())],
            )
        }
        RejectReason::AlreadyResearched => strings.hud("button.researched").to_string(),
        other => reject_text(strings, rules, other),
    };
    let message = reject_text(strings, rules, reason);
    let tooltip = match (reason, requires_text(rules, strings, tech)) {
        (RejectReason::AlreadyResearched | RejectReason::ResearchInProgress, _) | (_, None) => {
            message
        }
        (_, Some(requires)) => format!("{message}. {requires}"),
    };
    (line, tooltip)
}

/// A button's state now (enabled, and the tooltip: the rejection's message
/// when disabled, else what it does and costs) and its second line: the
/// cost, or for a greyed Research button what blocks it.
pub fn card_state(
    view: &SimView<'_>,
    rules: &Rules,
    strings: &UiStrings,
    action: CardAction,
) -> (CardState, String) {
    let (name, cost, verdict) = evaluate(view, rules, action);
    let cost_line = match action {
        CardAction::CancelSite(_) | CardAction::AttackMove => String::new(),
        _ => cost_text(rules, &cost),
    };
    let (line, tooltip) = match (&verdict, action) {
        (_, CardAction::Research(_, tech)) => {
            research_lines(view, rules, strings, tech, cost_line, &verdict)
        }
        (Err(reason), _) => (cost_line, reject_text(strings, rules, reason)),
        (Ok(()), CardAction::Build(_)) => {
            let tooltip = format!(
                "{}. {}",
                fill(strings.hud("tooltip.place"), &[("building", &name)]),
                fill(strings.hud("tooltip.cost"), &[("cost", &cost_line)])
            );
            (cost_line, tooltip)
        }
        (Ok(()), CardAction::Train(..)) => {
            let tooltip = format!(
                "{}: {}",
                fill(strings.hud("button.train"), &[("unit", &name)]),
                fill(strings.hud("tooltip.cost"), &[("cost", &cost_line)])
            );
            (cost_line, tooltip)
        }
        (Ok(()), CardAction::CancelSite(_)) => {
            (cost_line, strings.hud("tooltip.cancel_site").to_string())
        }
        (Ok(()), CardAction::AttackMove) => {
            (cost_line, strings.hud("tooltip.attack_move").to_string())
        }
    };
    let state = CardState {
        enabled: verdict.is_ok(),
        tooltip,
    };
    (state, line)
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

/// Hover and pressed colours of every button with a [`CardState`] (the
/// card's and the Advance button); a disabled button stays greyed.
pub fn card_button_colors(
    mut buttons: Query<(&CardState, &Interaction, &mut BackgroundColor, &Children)>,
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

/// The tooltip panel's vertical offsets for `place`: `(top, bottom)`.
pub fn tooltip_offsets(place: TooltipPlace) -> (Val, Val) {
    match place {
        TooltipPlace::AboveBottomBar => (Val::Auto, Val::Px(BOTTOM_BAR_HEIGHT_PX + PADDING_PX)),
        TooltipPlace::BelowAgeStrip => (
            Val::Px(TOP_BAR_HEIGHT_PX + AGE_STRIP_HEIGHT_PX + PADDING_PX),
            Val::Auto,
        ),
    }
}

/// Show the hovered button's tooltip: any entity with a [`CardState`], at
/// its [`TooltipPlace`] (above the bottom bar unless it says otherwise).
pub fn update_tooltip(
    buttons: Query<(&Interaction, &CardState, Option<&TooltipPlace>)>,
    mut tip: Query<&mut Text, With<Tooltip>>,
    mut panel: Query<&mut Node, With<TooltipPanel>>,
) {
    let hovered = buttons
        .iter()
        .find(|(i, _, _)| **i != Interaction::None)
        .map(|(_, s, place)| (s.tooltip.clone(), place.copied().unwrap_or_default()));
    let display = if hovered.is_some() {
        Display::Flex
    } else {
        Display::None
    };
    for mut n in &mut panel {
        if n.display != display {
            n.display = display;
        }
        if let Some((_, place)) = &hovered {
            let (top, bottom) = tooltip_offsets(*place);
            if n.top != top || n.bottom != bottom {
                n.top = top;
                n.bottom = bottom;
            }
        }
    }
    if let Some((text, _)) = hovered {
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
        CardAction::AttackMove => *mode = OrderMode::AttackMove,
        action => {
            if let Some(cmd) = card_command(action) {
                let seq = pending.push_local(cmd);
                info!("card {action:?}: seq {seq}");
            }
        }
    }
}

/// The selection panel's hit-point line, "HP {count} / {cap}", from the
/// sim's `(hp, max_hp)`.
pub fn hp_text(strings: &UiStrings, (hp, max): (i32, i32)) -> String {
    fill(
        strings.hud("info.hp"),
        &[("count", &hp.to_string()), ("cap", &max.to_string())],
    )
}

/// The selection panel's line for a building of kind `host` that hosts
/// techs (M4b): "Researched: Trade I, Letters I", the top level `p` has
/// completed in each line it hosts, in `techs.ron` order; `None` when it
/// hosts none or nothing is researched yet.
pub fn researched_text(
    view: &SimView<'_>,
    rules: &Rules,
    strings: &UiStrings,
    p: PlayerId,
    host: &str,
) -> Option<String> {
    let researched = view.researched(p);
    let names: Vec<&str> = hosted_lines(rules, host)
        .iter()
        .filter_map(|(_, levels)| levels.iter().rev().find(|t| researched.contains(t)))
        .filter_map(|t| rules.tech(t.0).map(|t| t.name.as_str()))
        .collect();
    (!names.is_empty()).then(|| {
        fill(
            strings.hud("info.researched"),
            &[("name", &names.join(", "))],
        )
    })
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
        if let Some(hp) = view.building_hp(id) {
            lines.push(hp_text(strings, hp));
        }
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
                if let Some(line) = researched_text(view, rules, strings, b.owner, &kind.id) {
                    lines.push(line);
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
    let mut lines: Vec<String> = counts
        .iter()
        .filter_map(|(k, n)| {
            rules.unit_kind(*k).map(|u| {
                fill(
                    strings.hud("info.units"),
                    &[("name", &u.name), ("count", &n.to_string())],
                )
            })
        })
        .collect();
    // One unit, the local player's or not: its hit points.
    if let [id] = selection.units.iter().copied().collect::<Vec<_>>()[..]
        && let Some(hp) = view.unit_hp(id)
    {
        lines.push(hp_text(strings, hp));
    }
    lines.join("\n")
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
        assert_eq!(text, "Town\nHP 2000 / 2000\nSeat, tier 1");
        selection.set((1..=5).map(sim::UnitId));
        assert_eq!(
            selection_text(&view, rules, &strings(), &selection),
            "Yeoman x5"
        );
    }

    /// One selected unit shows the sim's hit points, another player's unit
    /// included; several units do not. (The own Seat's line is checked in
    /// `the_seat_trains_yeomen_and_names_itself`; another player's building
    /// cannot be selected.)
    #[test]
    fn one_unit_shows_its_hp_enemy_included() {
        let mut sim = standard();
        let rules = sim.rules().clone();
        let p1 = sim::PlayerId(1);
        let sk = UnitKindId(
            u16::try_from(
                rules
                    .units
                    .iter()
                    .position(|u| u.id == "skirmisher")
                    .unwrap(),
            )
            .unwrap(),
        );
        let arms = sim::TechId(rules.tech_by_id("arms_1").unwrap().0);
        // P1 researches Arms I and spawns a Skirmisher (UnitId 11) next to
        // P0's Yeoman 1, then attacks it once the spawn has applied.
        let cmds = [
            sim::PlayerCommand::new(
                p1,
                0,
                Command::DebugResearch {
                    owner: p1,
                    tech: arms,
                },
            ),
            sim::PlayerCommand::new(
                p1,
                1,
                Command::DebugSpawn {
                    owner: p1,
                    kind: sk,
                    at: sim.view().unit(sim::UnitId(1)).unwrap().pos,
                    count: 1,
                },
            ),
        ];
        sim.step(&cmds);
        for _ in 0..sim.setup().cmd_delay {
            sim.step(&[]);
        }
        let attack = sim::PlayerCommand::new(
            p1,
            2,
            Command::Attack {
                units: vec![sim::UnitId(11)],
                target: sim::TargetId::Unit(sim::UnitId(1)),
            },
        );
        sim.step(&[attack]);
        let mut hit = false;
        for _ in 0..200 {
            sim.step(&[]);
            if sim
                .view()
                .unit(sim::UnitId(1))
                .is_some_and(|u| u.damage_taken > 0)
            {
                hit = true;
                break;
            }
        }
        assert!(hit, "the Skirmisher hit Yeoman 1");
        let (view, s) = (sim.view(), strings());
        let mut selection = Selection::default();
        selection.set([sim::UnitId(1)]);
        let (hp, max) = view.unit_hp(sim::UnitId(1)).unwrap();
        assert!(hp < max);
        assert_eq!(
            selection_text(&view, &rules, &s, &selection),
            format!("Yeoman x1\nHP {hp} / {max}")
        );
        // The enemy Skirmisher (the Seat shoots it): read-only, with its
        // Arms maximum.
        selection.set([sim::UnitId(11)]);
        let (hp, max) = view.unit_hp(sim::UnitId(11)).unwrap();
        assert_eq!(max, view.unit_stats(p1, sk).max_hp);
        assert!(max > 80, "Arms I raised it: {max}");
        assert_eq!(
            selection_text(&view, &rules, &s, &selection),
            format!("Skirmisher x1\nHP {hp} / {max}")
        );
        assert!(
            card_actions(&view, &rules, &selection).is_empty(),
            "no orders for it"
        );
        // Two units: counts only.
        selection.set([sim::UnitId(1), sim::UnitId(2)]);
        assert_eq!(selection_text(&view, &rules, &s, &selection), "Yeoman x2");
    }

    /// The Attack-move button: for own units with no builder and at least
    /// one armed unit; never for builders (their card is full of Build
    /// buttons), unarmed units or an enemy selection. A click enters
    /// attack-move mode, as A does.
    #[test]
    fn attack_move_button_only_for_armed_selections() {
        let mut sim = standard();
        let rules = sim.rules().clone();
        let p1 = sim::PlayerId(1);
        let kind_of = |id: &str| {
            UnitKindId(u16::try_from(rules.units.iter().position(|u| u.id == id).unwrap()).unwrap())
        };
        let spawn = |owner, seq, kind, x| {
            sim::PlayerCommand::new(
                owner,
                seq,
                Command::DebugSpawn {
                    owner,
                    kind,
                    at: sim::FxVec2::from_ints(x, 40),
                    count: 1,
                },
            )
        };
        // P0: a Skirmisher (11) and a Scribe (12); P1: a Bowman (13).
        let cmds = [
            spawn(LOCAL_PLAYER, 0, kind_of("skirmisher"), 20),
            spawn(LOCAL_PLAYER, 1, kind_of("scribe"), 24),
            spawn(p1, 0, kind_of("bowman"), 60),
        ];
        sim.step(&cmds);
        for _ in 0..sim.setup().cmd_delay {
            sim.step(&[]);
        }
        let view = sim.view();
        let actions = |ids: &[u32]| {
            let mut selection = Selection::default();
            selection.set(ids.iter().copied().map(sim::UnitId));
            card_actions(&view, &rules, &selection)
        };
        assert_eq!(actions(&[11]), [CardAction::AttackMove]);
        assert_eq!(
            actions(&[11, 12]),
            [CardAction::AttackMove],
            "one armed unit is enough"
        );
        assert_eq!(
            actions(&[11, 13]),
            [CardAction::AttackMove],
            "the enemy is ignored"
        );
        assert!(actions(&[12]).is_empty(), "a Scribe has no attack");
        assert!(actions(&[13]).is_empty(), "an enemy selection has no card");
        assert!(actions(&[]).is_empty());
        let builders = actions(&[1, 11]);
        assert_eq!(builders.len(), rules.buildings.len());
        assert!(builders.iter().all(|a| matches!(a, CardAction::Build(_))));
        // Enabled, no cost, the strings' label and tooltip.
        let (state, cost) = card_state(&view, &rules, &strings(), CardAction::AttackMove);
        assert!(state.enabled);
        assert_eq!(cost, "");
        // The key exists (`UiStrings::hud` falls back to the key itself, so
        // the comparison alone would pass without it), and the arm reads it.
        assert!(strings().0.hud("tooltip.attack_move").is_some());
        assert_eq!(state.tooltip, strings().hud("tooltip.attack_move"));
        assert!(strings().0.hud("button.attack_move").is_some());

        // The click: an app with the card's click observer.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<OrderMode>()
            .init_resource::<PendingCommands>()
            .init_resource::<MessageState>();
        let button = app
            .world_mut()
            .spawn((
                CardButton(CardAction::AttackMove),
                CardState {
                    enabled: true,
                    tooltip: String::new(),
                },
            ))
            .observe(on_card_click)
            .id();
        app.update();
        app.world_mut().trigger(Pointer::new(
            bevy::picking::pointer::PointerId::Mouse,
            bevy::picking::pointer::Location {
                target: bevy::camera::NormalizedRenderTarget::None {
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
        assert_eq!(*app.world().resource::<OrderMode>(), OrderMode::AttackMove);
        assert!(
            app.world().resource::<PendingCommands>().is_empty(),
            "no command"
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

    /// A P0 command driver: `run` stamps a batch with P0's next seqs,
    /// steps once with it and `cmd_delay` more ticks (so it has applied)
    /// and returns the events of those ticks.
    struct Lab {
        sim: sim::Sim,
        seq: u32,
    }

    impl Lab {
        fn new(sim: sim::Sim) -> Lab {
            Lab { sim, seq: 0 }
        }

        fn run(&mut self, cmds: impl IntoIterator<Item = Command>) -> Vec<sim::SimEvent> {
            let batch: Vec<_> = cmds
                .into_iter()
                .map(|c| {
                    self.seq += 1;
                    sim::PlayerCommand::new(LOCAL_PLAYER, self.seq - 1, c)
                })
                .collect();
            self.sim.step(&batch);
            for _ in 0..self.sim.setup().cmd_delay {
                self.sim.step(&[]);
            }
            self.sim.drain_events()
        }

        /// A copy of the match (`Sim::restore` of a snapshot).
        fn fork(&self) -> Lab {
            let mut sim = sim::Sim::new(
                self.sim.setup().clone(),
                self.sim.rules().clone(),
                Box::new(NoAi),
            );
            sim.restore(&self.sim.snapshot()).expect("restore");
            Lab { sim, seq: self.seq }
        }
    }

    fn tech(rules: &Rules, id: &str) -> TechId {
        TechId(rules.tech_by_id(id).unwrap().0)
    }

    fn res(rules: &Rules, id: &str) -> u8 {
        u8::try_from(rules.resource_index(id).unwrap()).unwrap()
    }

    fn unit(rules: &Rules, id: &str) -> UnitKindId {
        UnitKindId(u16::try_from(rules.units.iter().position(|u| u.id == id).unwrap()).unwrap())
    }

    fn grant(resource: u8, amount: u32) -> Command {
        Command::DebugGrant {
            owner: LOCAL_PLAYER,
            resource,
            amount,
        }
    }

    fn debug_research(tech: TechId) -> Command {
        Command::DebugResearch {
            owner: LOCAL_PLAYER,
            tech,
        }
    }

    /// Grants of exactly the Masonry and Charter costs and both
    /// `AdvanceAge`s, for a player with five techs researched: a line's
    /// top level is unlocked in the last age, so a finished line is only
    /// reachable there.
    fn to_charter(rules: &Rules) -> Vec<Command> {
        let mut cost = vec![0i64; rules.resources.resources.len()];
        for age in &rules.ages[1..] {
            for (c, a) in cost.iter_mut().zip(rules.amounts(&age.cost)) {
                *c += a;
            }
        }
        cost.iter()
            .enumerate()
            .filter(|(_, c)| **c > 0)
            .map(|(r, c)| grant(u8::try_from(r).unwrap(), u32::try_from(*c).unwrap()))
            .chain((1..rules.ages.len()).map(|_| Command::AdvanceAge {
                player: LOCAL_PLAYER,
            }))
            .collect()
    }

    fn rejections(events: &[sim::SimEvent]) -> Vec<RejectReason> {
        events
            .iter()
            .filter_map(|e| match e {
                sim::SimEvent::CommandRejected { player, reason, .. }
                    if *player == LOCAL_PLAYER =>
                {
                    Some(*reason)
                }
                _ => None,
            })
            .collect()
    }

    /// A complete P0 building of kind `id` at (28, 67).
    fn place(rules: &Rules, id: &str) -> Command {
        Command::DebugPlaceBuilding {
            owner: LOCAL_PLAYER,
            kind: kind(rules, id),
            tile: sim::Tile::new(28, 67),
            complete: true,
        }
    }

    /// The standard start plus a complete P0 Scriptorium, `BuildingId(3)`
    /// (the Seats are 1 and 2).
    fn with_scriptorium() -> Lab {
        let mut lab = Lab::new(standard());
        let cmd = place(lab.sim.rules(), "scriptorium");
        assert!(rejections(&lab.run([cmd])).is_empty());
        lab
    }

    fn selected(b: BuildingId) -> Selection {
        let mut selection = Selection::default();
        selection.set_building(b);
        selection
    }

    /// A primary click on `button`, as the picking pipeline sends it.
    fn click(app: &mut App, button: Entity) {
        app.world_mut().trigger(Pointer::new(
            bevy::picking::pointer::PointerId::Mouse,
            bevy::picking::pointer::Location {
                target: bevy::camera::NormalizedRenderTarget::None {
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
    }

    /// The Scriptorium's card: Train Scribe, then one Research button per
    /// tech line in `techs.ron` order, each the line's next level; the
    /// Arms button moves on to Arms II once Arms I is done. Greyed for the
    /// start's empty Ore and Lore with the cost on the second line, then
    /// enabled with its cost and quoted time.
    #[test]
    fn scriptorium_offers_scribe_and_the_next_level_of_each_line() {
        let mut lab = with_scriptorium();
        let rules = lab.sim.rules().clone();
        let s = strings();
        let b = BuildingId(3);
        let r = |id| CardAction::Research(b, tech(&rules, id));
        let scribe = CardAction::Train(b, unit(&rules, "scribe"));
        let first = [r("arms_1"), r("statecraft_1"), r("trade_1"), r("letters_1")];
        assert_eq!(
            card_actions(&lab.sim.view(), &rules, &selected(b)),
            [&[scribe][..], &first].concat()
        );
        let labels: Vec<String> = first.iter().map(|a| card_label(&rules, &s, *a)).collect();
        assert_eq!(labels, ["Arms I", "Statecraft I", "Trade I", "Letters I"]);
        let (ore, lore) = (res(&rules, "ore"), res(&rules, "lore"));
        let view = lab.sim.view();
        for (action, cost, short) in [
            (first[0], "50 Ore 75 Lore", ore),
            (first[1], "80 Lore", lore),
            (first[2], "70 Lore", lore),
            (first[3], "60 Lore", lore),
        ] {
            let CardAction::Research(_, t) = action else {
                unreachable!()
            };
            let reason = RejectReason::CannotAfford { resource: short };
            assert_eq!(
                view.check_research(LOCAL_PLAYER, b, t).map(|_| ()),
                Err(reason),
                "{action:?}"
            );
            let (state, line) = card_state(&view, &rules, &s, action);
            assert_eq!(
                (state.enabled, line.as_str(), state.tooltip),
                (false, cost, reject_text(&s, &rules, &reason)),
                "{action:?}: a first level of the first age requires nothing more"
            );
        }
        lab.run([grant(ore, 200), grant(lore, 400)]);
        let view = lab.sim.view();
        for action in first {
            let CardAction::Research(_, t) = action else {
                unreachable!()
            };
            let (cost, _) = view.research_quote(LOCAL_PLAYER, t);
            let (state, line) = card_state(&view, &rules, &s, action);
            assert!(view.check_research(LOCAL_PLAYER, b, t).is_ok());
            assert!(state.enabled, "{action:?}: {state:?}");
            assert_eq!(line, cost_text(&rules, &cost));
        }
        // 250 ds is 500 ticks, 25 s at 20 Hz.
        let (state, line) = card_state(&view, &rules, &s, first[3]);
        assert_eq!(
            (line.as_str(), state.tooltip.as_str()),
            ("60 Lore", "Research Letters I, 25 s. Costs 60 Lore")
        );
        lab.run([debug_research(tech(&rules, "arms_1"))]);
        assert_eq!(
            card_actions(&lab.sim.view(), &rules, &selected(b)),
            [
                scribe,
                r("arms_2"),
                r("statecraft_1"),
                r("trade_1"),
                r("letters_1")
            ]
        );
    }

    /// Each Research button's verdict is what the sim does with the
    /// command it sends: on a copy of the match the `Research` is rejected
    /// with exactly the button's reason, or accepted and queued when the
    /// button is enabled. An Empty start keeps the stockpiles still between
    /// the check and the apply `cmd_delay` ticks later, but for the
    /// Scriptorium's own Lore, so the exact-stock case is short of Ore.
    #[test]
    fn research_verdicts_match_the_sim() {
        let rules = rules();
        let s = strings();
        let mut base = Lab::new(sim::Sim::new(
            sim::MatchSetup::scenario(&rules, 1),
            rules.clone(),
            Box::new(NoAi),
        ));
        assert!(rejections(&base.run([place(&rules, "scriptorium")])).is_empty());
        let b = BuildingId(1);
        let t = |id| tech(&rules, id);
        let (grain, ore, lore) = (
            res(&rules, "grain"),
            res(&rules, "ore"),
            res(&rules, "lore"),
        );
        let scribe = unit(&rules, "scribe");
        let case = |cmds: Vec<Command>| {
            let mut lab = base.fork();
            assert!(rejections(&lab.run(cmds)).is_empty());
            lab
        };
        let cases = [
            ("exact", case(vec![grant(ore, 50), grant(lore, 80)])),
            ("poor", case(vec![grant(ore, 49), grant(lore, 80)])),
            (
                "busy",
                case(vec![
                    grant(ore, 50),
                    grant(lore, 400),
                    debug_research(t("letters_1")),
                    Command::Research {
                        building: b,
                        tech: t("trade_1"),
                    },
                ]),
            ),
            (
                "done",
                case(
                    [
                        "statecraft_1",
                        "statecraft_2",
                        "statecraft_3",
                        "trade_1",
                        "letters_1",
                    ]
                    .map(|id| debug_research(t(id)))
                    .into_iter()
                    .chain(to_charter(&rules))
                    .collect(),
                ),
            ),
            (
                "full",
                case(
                    [grant(grain, 300), grant(ore, 50), grant(lore, 400)]
                        .into_iter()
                        .chain((0..5).map(|_| Command::Train {
                            building: b,
                            kind: scribe,
                        }))
                        .collect(),
                ),
            ),
        ];
        let mut table = Vec::new();
        for (name, lab) in &cases {
            let view = lab.sim.view();
            for action in card_actions(&view, &rules, &selected(b)) {
                let CardAction::Research(_, tech) = action else {
                    continue;
                };
                let (_, _, verdict) = evaluate(&view, &rules, action);
                let (state, _) = card_state(&view, &rules, &s, action);
                assert_eq!(state.enabled, verdict.is_ok(), "{name} {tech:?}");
                let mut fork = lab.fork();
                let events = fork.run([card_command(action).expect("a Research command")]);
                match &verdict {
                    Err(reason) => {
                        assert_eq!(rejections(&events), [*reason], "{name} {tech:?}");
                    }
                    Ok(()) => {
                        assert!(rejections(&events).is_empty(), "{name} {tech:?}");
                        let queue = &fork.sim.view().building(b).unwrap().queue;
                        assert!(
                            queue.iter().any(|e| matches!(
                                e.item,
                                QueueItem::Research { tech: q, .. } if q == tech
                            )),
                            "{name} {tech:?} queued"
                        );
                    }
                }
                table.push((*name, rules.tech(tech.0).unwrap().id.clone(), verdict));
            }
        }
        let ok = |name, id: &str| (name, id.to_string(), Ok(()));
        let no = |name, id: &str, reason| (name, id.to_string(), Err(reason));
        let short = |resource| RejectReason::CannotAfford { resource };
        assert_eq!(
            table,
            [
                ok("exact", "arms_1"),
                ok("exact", "statecraft_1"),
                ok("exact", "trade_1"),
                ok("exact", "letters_1"),
                no("poor", "arms_1", short(ore)),
                ok("poor", "statecraft_1"),
                ok("poor", "trade_1"),
                ok("poor", "letters_1"),
                ok("busy", "arms_1"),
                ok("busy", "statecraft_1"),
                no("busy", "trade_1", RejectReason::ResearchInProgress),
                no("busy", "letters_2", RejectReason::AgeRequired),
                no("done", "arms_1", short(ore)),
                no("done", "statecraft_3", RejectReason::AlreadyResearched),
                no("done", "trade_2", short(lore)),
                no("done", "letters_2", short(lore)),
                no("full", "arms_1", RejectReason::QueueFull),
                no("full", "statecraft_1", RejectReason::QueueFull),
                no("full", "trade_1", RejectReason::QueueFull),
                no("full", "letters_1", RejectReason::QueueFull),
            ]
        );
    }

    /// A locked level names what it needs: on the button the age (or the
    /// missing level), in the tooltip the rejection plus the level before
    /// it and its age. A level in progress shows how far it is.
    #[test]
    fn locked_levels_name_their_prerequisite_and_age() {
        let mut lab = with_scriptorium();
        let rules = lab.sim.rules().clone();
        let s = strings();
        let b = BuildingId(3);
        lab.run([debug_research(tech(&rules, "arms_1"))]);
        let view = lab.sim.view();
        let arms_2 = CardAction::Research(b, tech(&rules, "arms_2"));
        assert!(card_actions(&view, &rules, &selected(b)).contains(&arms_2));
        assert_eq!(
            view.check_research(LOCAL_PLAYER, b, tech(&rules, "arms_2"))
                .map(|_| ()),
            Err(RejectReason::AgeRequired)
        );
        let (state, line) = card_state(&view, &rules, &s, arms_2);
        assert_eq!(
            (state.enabled, line.as_str(), state.tooltip.as_str()),
            (
                false,
                "Masonry Age",
                "Not available in this age. Requires Arms I, Masonry Age"
            )
        );
        // Two levels ahead (the card never offers it): the sim reports the
        // missing level before the age, and so does the button.
        let arms_3 = CardAction::Research(b, tech(&rules, "arms_3"));
        assert_eq!(
            view.check_research(LOCAL_PLAYER, b, tech(&rules, "arms_3"))
                .map(|_| ()),
            Err(RejectReason::PrerequisiteMissing)
        );
        let (state, line) = card_state(&view, &rules, &s, arms_3);
        assert_eq!(
            (state.enabled, line.as_str(), state.tooltip.as_str()),
            (
                false,
                "Needs Arms II",
                "Needs a tech you have not researched. Requires Arms II, Charter Age"
            )
        );
        // In progress, half way: the button counts, the queue line agrees.
        let letters = CardAction::Research(b, tech(&rules, "letters_1"));
        lab.run([
            grant(res(&rules, "lore"), 100),
            card_command(letters).unwrap(),
        ]);
        let Some(QueueItem::Research { ticks, .. }) = lab
            .sim
            .view()
            .building(b)
            .and_then(|b| b.queue.first())
            .map(|e| e.item)
        else {
            panic!("Letters I is queued");
        };
        // The step that applied it was its first; half the time is left.
        for _ in 0..ticks / 2 - 1 {
            lab.sim.step(&[]);
        }
        let view = lab.sim.view();
        assert_eq!(
            view.check_research(LOCAL_PLAYER, b, tech(&rules, "letters_1"))
                .map(|_| ()),
            Err(RejectReason::ResearchInProgress)
        );
        let (state, line) = card_state(&view, &rules, &s, letters);
        assert_eq!(
            (state.enabled, line.as_str(), state.tooltip.as_str()),
            (false, "Researching 50%", "Already being researched")
        );
        let text = selection_text(&view, &rules, &s, &selected(b));
        assert_eq!(text.lines().last(), Some("Queue: Letters I 50%"), "{text}");
    }

    /// A Watchtower hosts the Harrying line (M5a rules data), so its card
    /// offers it as the Scriptorium's card offers its lines: Harrying I
    /// greyed with "Masonry Age" in the Hearth Age, then enabled with its
    /// quoted cost and time; in progress while researched; then Harrying
    /// II greyed with "Charter Age", enabled in the Charter Age, and
    /// "Researched" once done. Every verdict is the sim's
    /// `check_research`, and the panel names the level done.
    #[test]
    fn watchtower_card_offers_harrying_from_masonry() {
        let mut lab = Lab::new(standard());
        let rules = lab.sim.rules().clone();
        let s = strings();
        let t = |id| tech(&rules, id);
        assert!(rejections(&lab.run([place(&rules, "watchtower")])).is_empty());
        let b = BuildingId(3);
        let h1 = CardAction::Research(b, t("harrying_1"));
        let h2 = CardAction::Research(b, t("harrying_2"));
        // The card offers exactly `action`; its label, second line and
        // state, enabled exactly when the sim accepts it.
        let shown = |lab: &Lab, action: CardAction| {
            let view = lab.sim.view();
            assert_eq!(card_actions(&view, &rules, &selected(b)), [action]);
            let CardAction::Research(_, tech) = action else {
                unreachable!("a Research button")
            };
            let (state, line) = card_state(&view, &rules, &s, action);
            assert_eq!(
                state.enabled,
                view.check_research(LOCAL_PLAYER, b, tech).is_ok(),
                "{state:?}"
            );
            (card_label(&rules, &s, action), line, state)
        };
        let quote = |lab: &Lab, action: CardAction| {
            let CardAction::Research(_, tech) = action else {
                unreachable!("a Research button")
            };
            lab.sim.view().research_quote(LOCAL_PLAYER, tech)
        };
        let grants = |cost: &[i64]| -> Vec<Command> {
            cost.iter()
                .enumerate()
                .filter(|(_, c)| **c > 0)
                .map(|(r, c)| grant(u8::try_from(r).unwrap(), u32::try_from(*c).unwrap()))
                .collect()
        };

        // The Hearth Age: Harrying I waits for Masonry.
        let (label, line, state) = shown(&lab, h1);
        assert_eq!(
            (label.as_str(), line.as_str(), state.tooltip.as_str()),
            (
                "Harrying I",
                "Masonry Age",
                "Not available in this age. Requires Masonry Age"
            )
        );
        assert!(!state.enabled);

        // The Masonry Age (two Scriptorium techs, no Letters, so the quote
        // is the base cost), with exactly Harrying I's cost in Ore and Lore.
        let mut cmds = vec![debug_research(t("arms_1")), debug_research(t("trade_1"))];
        cmds.extend(grants(&rules.amounts(&rules.ages[1].cost)));
        cmds.push(Command::AdvanceAge {
            player: LOCAL_PLAYER,
        });
        assert!(rejections(&lab.run(cmds)).is_empty());
        let (cost, ticks) = quote(&lab, h1);
        assert!(rejections(&lab.run(grants(&cost))).is_empty());
        let (label, line, state) = shown(&lab, h1);
        assert_eq!(
            (label.as_str(), line.as_str(), state.enabled),
            ("Harrying I", "75 Ore 100 Lore", true)
        );
        assert_eq!(
            state.tooltip,
            "Research Harrying I, 40 s. Costs 75 Ore 100 Lore"
        );
        assert_eq!(ticks, 800);

        // A click's command researches it; the button shows the progress.
        let cmd = card_command(h1).expect("Research issues a command");
        assert!(rejections(&lab.run([cmd])).is_empty());
        let (_, line, state) = shown(&lab, h1);
        assert!(line.starts_with("Researching "), "{line}");
        assert!(!state.enabled);
        for _ in 0..ticks {
            lab.sim.step(&[]);
        }
        let events = lab.sim.drain_events();
        assert!(events.contains(&sim::SimEvent::ResearchCompleted {
            player: LOCAL_PLAYER,
            tech: t("harrying_1"),
        }));
        let (label, line, state) = shown(&lab, h2);
        assert_eq!(
            (label.as_str(), line.as_str(), state.tooltip.as_str()),
            (
                "Harrying II",
                "Charter Age",
                "Not available in this age. Requires Harrying I, Charter Age"
            )
        );
        let view = lab.sim.view();
        let text = selection_text(&view, &rules, &s, &selected(b));
        assert_eq!(
            text.lines().last(),
            Some("Researched: Harrying I"),
            "{text}"
        );
        assert_fits_the_panel(&text);

        // The Charter Age: five Scriptorium techs (Harrying counts toward no
        // age), the age's cost and Harrying II's quote.
        let mut cmds: Vec<Command> = ["statecraft_1", "letters_1", "letters_2"]
            .iter()
            .map(|id| debug_research(t(id)))
            .collect();
        cmds.extend(grants(&rules.amounts(&rules.ages[2].cost)));
        cmds.push(Command::AdvanceAge {
            player: LOCAL_PLAYER,
        });
        assert!(rejections(&lab.run(cmds)).is_empty());
        assert_eq!(lab.sim.view().age(LOCAL_PLAYER), 2);
        let (cost, _) = quote(&lab, h2);
        assert!(rejections(&lab.run(grants(&cost))).is_empty());
        let (_, line, state) = shown(&lab, h2);
        assert_eq!((line, state.enabled), (cost_text(&rules, &cost), true));
        assert!(rejections(&lab.run([debug_research(t("harrying_2"))])).is_empty());
        let (_, line, state) = shown(&lab, h2);
        assert_eq!(
            (line.as_str(), state.enabled, state.tooltip.as_str()),
            ("Researched", false, "Already researched")
        );
    }

    /// A line with every level researched keeps its button, the top level
    /// greyed as "Researched" (in the Charter Age, where the top level is
    /// unlocked; the sim checks the age before "already researched"); the
    /// panel lists the top level done in each line, before the queue line.
    #[test]
    fn finished_line_shows_level_three_greyed() {
        let mut lab = with_scriptorium();
        let rules = lab.sim.rules().clone();
        let s = strings();
        let b = BuildingId(3);
        let t = |id| tech(&rules, id);
        let events = lab.run(
            ["arms_1", "arms_2", "arms_3", "trade_1", "letters_1"]
                .map(|id| debug_research(t(id)))
                .into_iter()
                .chain(to_charter(&rules)),
        );
        assert!(rejections(&events).is_empty(), "{events:?}");
        let view = lab.sim.view();
        assert_eq!(view.age(LOCAL_PLAYER), 2, "the Charter Age");
        let arms_3 = CardAction::Research(b, t("arms_3"));
        assert_eq!(card_actions(&view, &rules, &selected(b))[1], arms_3);
        assert_eq!(
            view.check_research(LOCAL_PLAYER, b, t("arms_3"))
                .map(|_| ()),
            Err(RejectReason::AlreadyResearched)
        );
        let (state, line) = card_state(&view, &rules, &s, arms_3);
        assert_eq!(
            (state.enabled, line.as_str(), state.tooltip.as_str()),
            (false, "Researched", "Already researched")
        );
        assert_eq!(
            selection_text(&view, &rules, &s, &selected(b)),
            "Scriptorium\nHP 600 / 600\nWorkers 0 / 3\nResearched: Arms III, Trade I, Letters I"
        );
        let events = lab.run([
            grant(res(&rules, "lore"), 200),
            Command::Research {
                building: b,
                tech: t("trade_2"),
            },
        ]);
        assert!(rejections(&events).is_empty(), "{events:?}");
        let text = selection_text(&lab.sim.view(), &rules, &s, &selected(b));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[lines.len() - 2],
            "Researched: Arms III, Trade I, Letters I"
        );
        assert!(
            lines[lines.len() - 1].starts_with("Queue: Trade II "),
            "{text}"
        );
    }

    /// Bevy 0.19.1's default font, the only one the game uses until M7
    /// (`bevy_text-0.19.1/src/FiraMono-subset.ttf`), is monospace: every
    /// glyph advances 600 of its 1000 units per em.
    const GLYPH_ADVANCE_EM: f32 = 0.6;
    /// Bevy 0.19.1's default line height, `LineHeight::RelativeToFont(1.2)`.
    const LINE_HEIGHT_EM: f32 = 1.2;

    /// How many characters of `font_px` text fit on one line `width_px` wide.
    fn chars_per_line(width_px: f32, font_px: f32) -> usize {
        (width_px / (GLYPH_ADVANCE_EM * font_px)).floor() as usize
    }

    /// How many rows of HUD text fit in the selection panel: from the
    /// bottom bar's top padding to the bottom of the window.
    fn panel_rows() -> usize {
        ((BOTTOM_BAR_HEIGHT_PX - PADDING_PX) / (LINE_HEIGHT_EM * HUD_FONT_PX)).floor() as usize
    }

    /// The selection panel's text fits the bottom bar: every line on one
    /// row of `INFO_WIDTH_PX`, and the lines within the rows from the bar's
    /// top padding to the bottom of the window.
    fn assert_fits_the_panel(text: &str) {
        let (rows, width) = (panel_rows(), chars_per_line(INFO_WIDTH_PX, HUD_FONT_PX));
        assert!(
            text.lines().count() <= rows,
            "{} lines, {rows} fit:\n{text}",
            text.lines().count()
        );
        for line in text.lines() {
            assert!(
                line.chars().count() <= width,
                "{} characters, {width} fit on a row: {line}",
                line.chars().count()
            );
        }
    }

    /// A Scriptorium's panel at its longest fits the bottom bar, its queue
    /// line included: with every tech line at level III, and with every
    /// line at level II and a full queue of the four level III researches
    /// (the longest name first, at two-digit progress) and a Scribe. The
    /// panel, the card and the Stop button fit the default window side by
    /// side.
    #[test]
    fn the_scriptoriums_longest_panel_fits_the_bottom_bar() {
        assert_eq!(panel_rows(), 5);
        let card = CARD_COLUMNS as f32 * CARD_BUTTON_PX.x + (CARD_COLUMNS - 1) as f32 * CARD_GAP_PX;
        // The bar's padding, the panel, the card with its margins, the
        // Stop button and the bar's padding.
        let row: f32 = [
            PADDING_PX,
            INFO_WIDTH_PX,
            PADDING_PX,
            card,
            PADDING_PX,
            crate::hud::BUTTON_SIZE_PX.x,
            PADDING_PX,
        ]
        .iter()
        .sum();
        assert!(
            row <= crate::app::WINDOW_SIZE.0 as f32,
            "the bottom bar needs {row} px"
        );
        let rules = rules();
        let s = strings();
        let b = BuildingId(3);
        let t = |id: &str| tech(&rules, id);
        let scribe = || Command::Train {
            building: b,
            kind: unit(&rules, "scribe"),
        };
        let levels = |top: u8| {
            rules
                .techs
                .iter()
                .filter(|x| x.level <= top)
                .map(|x| debug_research(t(&x.id)))
                .collect::<Vec<_>>()
        };

        // Every line at level III, five Scribes queued.
        let mut lab = with_scriptorium();
        let events = lab.run(levels(3).into_iter().chain(to_charter(&rules)));
        assert!(rejections(&events).is_empty(), "{events:?}");
        let events = lab.run(
            [grant(res(&rules, "grain"), 150)]
                .into_iter()
                .chain((0..5).map(|_| scribe())),
        );
        assert!(rejections(&events).is_empty(), "{events:?}");
        let text = selection_text(&lab.sim.view(), &rules, &s, &selected(b));
        assert_eq!(
            text.lines().nth(3),
            Some("Researched: Arms III, Statecraft III, Trade III, Letters III"),
            "{text}"
        );
        assert_fits_the_panel(&text);

        // Every line at level II, the four level III researches and a
        // Scribe queued, the head past 10%.
        let mut lab = with_scriptorium();
        let events = lab.run(levels(2).into_iter().chain(to_charter(&rules)));
        assert!(rejections(&events).is_empty(), "{events:?}");
        let research = |id| Command::Research {
            building: b,
            tech: t(id),
        };
        let events = lab.run([
            grant(res(&rules, "lore"), 800),
            grant(res(&rules, "ore"), 150),
            grant(res(&rules, "grain"), 30),
            research("statecraft_3"),
            research("letters_3"),
            research("trade_3"),
            research("arms_3"),
            scribe(),
        ]);
        assert!(rejections(&events).is_empty(), "{events:?}");
        let Some(QueueItem::Research { ticks, .. }) = lab
            .sim
            .view()
            .building(b)
            .and_then(|b| b.queue.first())
            .map(|e| e.item)
        else {
            panic!("Statecraft III leads the queue");
        };
        for _ in 0..ticks / 8 {
            lab.sim.step(&[]);
        }
        let view = lab.sim.view();
        let pct = research_percent(&view, LOCAL_PLAYER, t("statecraft_3")).unwrap();
        assert!((10..100).contains(&pct), "{pct}");
        let text = selection_text(&view, &rules, &s, &selected(b));
        assert_eq!(
            text.lines().skip(3).collect::<Vec<_>>(),
            [
                "Researched: Arms II, Statecraft II, Trade II, Letters II".to_string(),
                format!("Queue: Statecraft III {pct}%, Letters III, Trade III, Arms III, Scribe"),
            ],
            "{text}"
        );
        assert_fits_the_panel(&text);
    }

    /// Every line a card button can show fits one line of the button:
    /// every name on the first line; on the second every cost at its
    /// highest (research at full price, before any Letters discount; each
    /// military unit at the ramp's cap, five entries queued at its trainer;
    /// every single-resource cost up to five digits, which covers the
    /// building and Yeoman ramps) and every blocker a Research button shows.
    #[test]
    fn every_card_line_fits_its_button() {
        let width = chars_per_line(CARD_BUTTON_PX.x, CARD_FONT_PX);
        let mut lab = Lab::new(standard());
        let rules = lab.sim.rules().clone();
        let s = strings();
        let id = |i: usize| u16::try_from(i).unwrap();
        let b = BuildingId(3);
        let mut lines: Vec<String> = Vec::new();
        let mut actions = vec![CardAction::CancelSite(b), CardAction::AttackMove];
        actions
            .extend((0..rules.buildings.len()).map(|i| CardAction::Build(BuildingKindId(id(i)))));
        actions.extend((0..rules.units.len()).map(|i| CardAction::Train(b, UnitKindId(id(i)))));
        actions.extend((0..rules.techs.len()).map(|i| CardAction::Research(b, TechId(id(i)))));
        lines.extend(actions.iter().map(|a| card_label(&rules, &s, *a)));
        let view = lab.sim.view();
        for i in 0..rules.techs.len() {
            let (cost, _) = view.research_quote(LOCAL_PLAYER, TechId(id(i)));
            lines.push(cost_text(&rules, &cost));
        }
        for i in 0..rules.buildings.len() {
            lines.push(cost_text(
                &rules,
                &view.building_cost(LOCAL_PLAYER, BuildingKindId(id(i))),
            ));
        }
        for r in 0..rules.resources.resources.len() {
            let mut cost = vec![0; rules.resources.resources.len()];
            cost[r] = 99_999;
            lines.push(cost_text(&rules, &cost));
        }
        lines.extend(rules.ages[1..].iter().map(|a| a.name.clone()));
        lines.extend((0..rules.techs.len()).filter_map(|i| {
            let prev = rules.tech(rules.tech_predecessor(id(i))?)?;
            Some(fill(s.hud("button.needs"), &[("name", &prev.name)]))
        }));
        lines.push(fill(s.hud("button.researching"), &[("count", "99")]));
        lines.push(s.hud("button.researched").to_string());
        lines.push(reject_text(&s, &rules, &RejectReason::QueueFull));

        // Five entries queued at the Muster Hall (3) and the Engine Yard
        // (4) put every military unit at the ramp's cap.
        let t = |id: &str| tech(&rules, id);
        let building = |which, x| Command::DebugPlaceBuilding {
            owner: LOCAL_PLAYER,
            kind: kind(&rules, which),
            tile: sim::Tile::new(x, 67),
            complete: true,
        };
        let events = lab.run([
            building("muster_hall", 28),
            building("engine_yard", 19),
            debug_research(t("letters_1")),
            debug_research(t("trade_1")),
            debug_research(t("arms_1")),
            grant(res(&rules, "grain"), 650),
            grant(res(&rules, "lumber"), 600),
            grant(res(&rules, "lore"), 100),
            Command::AdvanceAge {
                player: LOCAL_PLAYER,
            },
        ]);
        assert!(rejections(&events).is_empty(), "{events:?}");
        let train = |building, kind| Command::Train {
            building: BuildingId(building),
            kind: unit(&rules, kind),
        };
        let events = lab.run(
            (0..5)
                .map(|_| train(3, "skirmisher"))
                .chain((0..5).map(|_| train(4, "supply_wain"))),
        );
        assert!(rejections(&events).is_empty(), "{events:?}");
        let view = lab.sim.view();
        let capped: Vec<String> = (0..rules.units.len())
            .map(|i| cost_text(&rules, &view.unit_cost(LOCAL_PLAYER, UnitKindId(id(i)))))
            .collect();
        assert!(
            capped.contains(&"45 Grain 90 Lumber".to_string()),
            "the Bowman at the cap: {capped:?}"
        );
        lines.extend(capped);

        for line in &lines {
            assert!(
                line.chars().count() <= width,
                "{} characters, {width} fit on a button: {line}",
                line.chars().count()
            );
        }
    }

    /// The command each button sends, and the click: an enabled Research
    /// button queues its `Research`; a greyed one sends nothing and puts
    /// its tooltip on the message line.
    #[test]
    fn card_command_maps_every_action() {
        let (b, k, t) = (BuildingId(4), UnitKindId(1), TechId(2));
        assert_eq!(
            card_command(CardAction::Train(b, k)),
            Some(Command::Train {
                building: b,
                kind: k
            })
        );
        assert_eq!(
            card_command(CardAction::CancelSite(b)),
            Some(Command::Cancel {
                building: b,
                slot: 0
            })
        );
        assert_eq!(
            card_command(CardAction::Research(b, t)),
            Some(Command::Research {
                building: b,
                tech: t
            })
        );
        assert_eq!(card_command(CardAction::Build(BuildingKindId(0))), None);
        assert_eq!(card_command(CardAction::AttackMove), None);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<OrderMode>()
            .init_resource::<PendingCommands>()
            .init_resource::<MessageState>();
        let enabled = app
            .world_mut()
            .spawn((
                CardButton(CardAction::Research(b, t)),
                CardState {
                    enabled: true,
                    tooltip: String::new(),
                },
            ))
            .observe(on_card_click)
            .id();
        let greyed = app
            .world_mut()
            .spawn((
                CardButton(CardAction::Research(b, TechId(3))),
                CardState {
                    enabled: false,
                    tooltip: "Not available in this age".into(),
                },
            ))
            .observe(on_card_click)
            .id();
        app.update();
        click(&mut app, enabled);
        click(&mut app, greyed);
        let sent = app
            .world_mut()
            .resource_mut::<PendingCommands>()
            .drain_sorted();
        let sent: Vec<&Command> = sent.iter().map(|c| &c.cmd).collect();
        assert_eq!(
            sent,
            [&Command::Research {
                building: b,
                tech: t
            }]
        );
        assert_eq!(
            app.world().resource::<MessageState>().text,
            "Not available in this age"
        );
        assert_eq!(*app.world().resource::<OrderMode>(), OrderMode::default());
    }

    /// The Engine Yard offers the Mangonel and the Supply Wain; the
    /// Mangonel is greyed with the age message in the Hearth Age (its cost
    /// still shows) and enabled once the player has advanced, as the sim's
    /// `check_train` says, and the sim queues what the button sends.
    #[test]
    fn the_engine_yard_trains_mangonels_in_the_masonry_age() {
        let mut lab = Lab::new(standard());
        let rules = lab.sim.rules().clone();
        let s = strings();
        assert!(rejections(&lab.run([place(&rules, "engine_yard")])).is_empty());
        let b = BuildingId(3);
        let mangonel = CardAction::Train(b, unit(&rules, "mangonel"));
        let wain = CardAction::Train(b, unit(&rules, "supply_wain"));
        let view = lab.sim.view();
        assert_eq!(card_actions(&view, &rules, &selected(b)), [mangonel, wain]);
        assert_eq!(
            view.check_train(LOCAL_PLAYER, b, unit(&rules, "mangonel"))
                .map(|_| ()),
            Err(RejectReason::AgeRequired)
        );
        let (state, line) = card_state(&view, &rules, &s, mangonel);
        assert_eq!(
            (state.enabled, line.as_str(), state.tooltip.as_str()),
            (false, "160 Lumber", "Not available in this age")
        );
        let t = |id| tech(&rules, id);
        let events = lab.run([
            debug_research(t("letters_1")),
            debug_research(t("trade_1")),
            grant(res(&rules, "grain"), 250),
            grant(res(&rules, "lore"), 100),
            grant(res(&rules, "lumber"), 100),
            Command::AdvanceAge {
                player: LOCAL_PLAYER,
            },
        ]);
        assert!(rejections(&events).is_empty(), "{events:?}");
        let view = lab.sim.view();
        assert_eq!(view.age(LOCAL_PLAYER), 1, "the Masonry Age");
        assert!(
            view.check_train(LOCAL_PLAYER, b, unit(&rules, "mangonel"))
                .is_ok()
        );
        let (state, line) = card_state(&view, &rules, &s, mangonel);
        assert_eq!(
            (state.enabled, line.as_str(), state.tooltip.as_str()),
            (true, "160 Lumber", "Mangonel: Costs 160 Lumber")
        );
        let events = lab.run([card_command(mangonel).unwrap()]);
        assert!(rejections(&events).is_empty(), "{events:?}");
        let queue = &lab.sim.view().building(b).unwrap().queue;
        assert_eq!(
            queue.iter().map(|e| e.item).collect::<Vec<_>>(),
            [QueueItem::Unit(unit(&rules, "mangonel"))]
        );
    }

    /// A disabled button is greyed and an enabled one follows the
    /// pointer, card button or not: the Advance button carries a
    /// `CardState` but no `CardButton` (owner decision 7: greyed, not
    /// hidden).
    #[test]
    fn button_colours_follow_any_card_state() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_systems(Update, card_button_colors);
        let button = app
            .world_mut()
            .spawn((
                CardState {
                    enabled: false,
                    tooltip: String::new(),
                },
                Interaction::Hovered,
                BackgroundColor(palette::HUD_BUTTON),
            ))
            .with_children(|b| {
                b.spawn(TextColor(palette::HUD_TEXT));
            })
            .id();
        let read = |app: &App| {
            let world = app.world();
            let label = world.get::<Children>(button).unwrap()[0];
            (
                world.get::<BackgroundColor>(button).unwrap().0,
                world.get::<TextColor>(label).unwrap().0,
            )
        };
        app.update();
        assert_eq!(
            read(&app),
            (palette::HUD_BUTTON_PRESSED, palette::HUD_DISABLED),
            "greyed"
        );
        app.world_mut()
            .get_mut::<CardState>(button)
            .unwrap()
            .enabled = true;
        app.update();
        assert_eq!(
            read(&app),
            (palette::HUD_BUTTON_HOVER, palette::HUD_TEXT),
            "hovered"
        );
        app.world_mut().entity_mut(button).insert(Interaction::None);
        app.update();
        assert_eq!(read(&app), (palette::HUD_BUTTON, palette::HUD_TEXT));
    }

    /// The tooltip panel sits above the bottom bar for the card's buttons
    /// and under the age strip for a button placed there.
    #[test]
    fn the_tooltip_follows_the_hovered_buttons_place() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_systems(Update, update_tooltip);
        let panel = app
            .world_mut()
            .spawn((
                TooltipPanel,
                Node {
                    display: Display::None,
                    ..default()
                },
            ))
            .id();
        let tip = app.world_mut().spawn((Tooltip, Text::new(""))).id();
        let card = app
            .world_mut()
            .spawn((
                CardState {
                    enabled: true,
                    tooltip: "card".into(),
                },
                Interaction::Hovered,
            ))
            .id();
        app.update();
        let read = |app: &App| {
            let n = app.world().get::<Node>(panel).unwrap();
            let t = app.world().get::<Text>(tip).unwrap();
            (n.display, n.top, n.bottom, t.0.clone())
        };
        let (top_below, bottom_below) = tooltip_offsets(TooltipPlace::BelowAgeStrip);
        let (top_above, bottom_above) = tooltip_offsets(TooltipPlace::AboveBottomBar);
        assert_eq!(
            read(&app),
            (Display::Flex, top_above, bottom_above, "card".into())
        );
        app.world_mut().entity_mut(card).insert(Interaction::None);
        app.world_mut().spawn((
            CardState {
                enabled: false,
                tooltip: "top".into(),
            },
            Interaction::Hovered,
            TooltipPlace::BelowAgeStrip,
        ));
        app.update();
        assert_eq!(
            read(&app),
            (Display::Flex, top_below, bottom_below, "top".into())
        );
        assert_eq!(
            top_below,
            Val::Px(TOP_BAR_HEIGHT_PX + AGE_STRIP_HEIGHT_PX + PADDING_PX)
        );
    }
}
