//! The economy half of the HUD (M3b): the resource bar and population
//! readout on the top bar, and the message line above the bottom bar.
//!
//! The resource bar shows, per resource in `resources.ron` order, its name,
//! the local player's stockpile (`stock / cap` for a capped stockpile such
//! as Lore) and the income per 30 s from `SimView::income`. The rate turns
//! amber while production is discarded at the Yield Cap (`raw > credited`,
//! `docs/design/economy.md`). The pop readout is `SimView::pop`.
//!
//! The message line shows the local player's last command rejection, in the
//! words of `data/rules/strings/en.ron`, or a short notice (a building of
//! theirs completed), for [`MESSAGE_SECONDS`]. Rejections arrive as
//! `SimEventMsg` from the sim driver, `cmd_delay` ticks after the command.
//! Every text and number comes from data or the sim; nothing is hard-coded
//! but the layout.

use bevy::prelude::*;
use bevy::text::FontSize;
use rules::Rules;
use rules::strings::{Strings, fill};
use sim::{RejectReason, SimEvent};

use crate::hud::{BOTTOM_BAR_HEIGHT_PX, HUD_FONT_PX, HudRoot, PADDING_PX, TopBar, spawn_hud};
use crate::palette;
use crate::sim_driver::{LOCAL_PLAYER, SimEventMsg, SimHandle};

/// How long the message line shows a message, in seconds.
pub const MESSAGE_SECONDS: f64 = 4.0;
/// Gap between the message line and the bottom bar, logical px.
pub const MESSAGE_GAP_PX: f32 = 8.0;
/// Gap between two resource readouts, logical px.
pub const READOUT_GAP_PX: f32 = 24.0;

/// The UI strings the HUD shows, loaded from `data/rules/strings/en.ron`.
#[derive(Resource, Debug, Clone)]
pub struct UiStrings(pub Strings);

impl UiStrings {
    /// A HUD string by key, or the key itself when it is missing (a
    /// missing key is a data bug the tests catch; the HUD still shows
    /// something readable).
    pub fn hud<'a>(&'a self, key: &'a str) -> &'a str {
        self.0.hud(key).unwrap_or(key)
    }
}

/// One resource readout on the top bar: the index into `resources.ron`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceReadout(pub usize);

/// The pop readout on the top bar.
#[derive(Component, Debug, Clone, Copy)]
pub struct PopReadout;

/// The text of the message line.
#[derive(Component, Debug, Clone, Copy)]
pub struct MessageLine;

/// What the message line shows and since when (`Time::elapsed_secs_f64`).
#[derive(Resource, Debug, Default, Clone, PartialEq)]
pub struct MessageState {
    /// The message; empty when nothing is shown.
    pub text: String,
    /// When it was set.
    pub since: f64,
}

/// The snake-case key of `reason` in the `reject` table of the strings
/// file. Exhaustive, so a new `RejectReason` fails to compile until it has
/// a key; `every_reject_reason_has_a_message` checks the file has them all.
pub fn reject_key(reason: &RejectReason) -> &'static str {
    match reason {
        RejectReason::UnknownPlayer => "unknown_player",
        RejectReason::Surrendered => "surrendered",
        RejectReason::NoValidUnits => "no_valid_units",
        RejectReason::DebugCommandsDisabled => "debug_commands_disabled",
        RejectReason::UnknownUnitKind => "unknown_unit_kind",
        RejectReason::Unreachable => "unreachable",
        RejectReason::OutsideBorders => "outside_borders",
        RejectReason::CannotAfford { .. } => "cannot_afford",
        RejectReason::TownLimitReached => "town_limit_reached",
        RejectReason::PerTownLimitReached => "per_town_limit_reached",
        RejectReason::UnknownBuildingKind => "unknown_building_kind",
        RejectReason::UnknownBuilding => "unknown_building",
        RejectReason::UnknownResource => "unknown_resource",
        RejectReason::NotABuilder => "not_a_builder",
        RejectReason::SiteOutOfBounds => "site_out_of_bounds",
        RejectReason::SiteBlocked => "site_blocked",
        RejectReason::SiteOccupied => "site_occupied",
        RejectReason::NoAdjacentResource => "no_adjacent_resource",
        RejectReason::InvalidSlot => "invalid_slot",
        RejectReason::UnderConstruction => "under_construction",
        RejectReason::NotAGatherNode => "not_a_gather_node",
        RejectReason::PopCapReached => "pop_cap_reached",
        RejectReason::CannotTrainHere => "cannot_train_here",
        RejectReason::QueueFull => "queue_full",
        RejectReason::NoFreeSlot => "no_free_slot",
        RejectReason::UnknownTech => "unknown_tech",
        RejectReason::CannotResearchHere => "cannot_research_here",
        RejectReason::PrerequisiteMissing => "prerequisite_missing",
        RejectReason::AgeRequired => "age_required",
        RejectReason::AlreadyResearched => "already_researched",
        RejectReason::ResearchInProgress => "research_in_progress",
        RejectReason::NotEnoughTechs => "not_enough_techs",
        RejectReason::FinalAge => "final_age",
        RejectReason::PlayerMismatch => "player_mismatch",
        RejectReason::InvalidTarget => "invalid_target",
    }
}

/// The message-line text for `reason`, placeholders filled from the rules
/// (`CannotAfford` names the resource).
pub fn reject_text(strings: &UiStrings, rules: &Rules, reason: &RejectReason) -> String {
    let key = reject_key(reason);
    let template = strings.0.reject(key).unwrap_or(key);
    match reason {
        RejectReason::CannotAfford { resource } => {
            let name = rules
                .resources
                .resources
                .get(usize::from(*resource))
                .map_or("?", |r| r.name.as_str());
            fill(template, &[("resource", name)])
        }
        _ => template.to_string(),
    }
}

/// The readout text for resource `i`: name, stockpile and rate.
pub fn resource_text(
    strings: &UiStrings,
    rules: &Rules,
    i: usize,
    stock: i64,
    rate: &sim::RateInfo,
) -> String {
    let def = &rules.resources.resources[i];
    let stock = match def.stockpile_cap {
        Some(cap) => fill(
            strings.hud("stockpile.capped"),
            &[("count", &stock.to_string()), ("cap", &cap.to_string())],
        ),
        None => stock.to_string(),
    };
    let rate_text = match rate.cap {
        Some(cap) if rate.raw > rate.credited => fill(
            strings.hud("rate.capped"),
            &[
                ("rate", &rate.credited.to_string()),
                ("cap", &cap.to_string()),
            ],
        ),
        _ => fill(strings.hud("rate"), &[("rate", &rate.credited.to_string())]),
    };
    format!("{} {stock}  {rate_text}", def.name)
}

/// `true` while production of this resource is discarded at the Yield Cap.
pub fn at_cap(rate: &sim::RateInfo) -> bool {
    rate.raw > rate.credited
}

/// The resource bar, pop readout and message line.
pub struct EconomyHudPlugin;

impl Plugin for EconomyHudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MessageState>()
            .add_systems(Startup, spawn_economy_hud.after(spawn_hud))
            .add_systems(
                Update,
                (update_resource_bar, collect_messages, update_message_line).chain(),
            );
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

/// Fills the top bar with one readout per resource and the pop readout,
/// and adds the message line above the bottom bar.
pub fn spawn_economy_hud(
    mut commands: Commands,
    sim: Option<NonSend<SimHandle>>,
    top: Query<Entity, With<TopBar>>,
    root: Query<Entity, With<HudRoot>>,
) {
    let count = sim
        .as_ref()
        .map_or(0, |s| s.rules().resources.resources.len());
    if let Ok(bar) = top.single() {
        commands.entity(bar).with_children(|bar| {
            bar.spawn((
                Name::new("Resource bar"),
                Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(READOUT_GAP_PX),
                    align_items: AlignItems::Center,
                    ..default()
                },
            ))
            .with_children(|row| {
                for i in 0..count {
                    row.spawn((
                        Name::new(format!("Resource {i}")),
                        ResourceReadout(i),
                        hud_text(""),
                    ));
                }
            });
            bar.spawn((Name::new("Pop"), PopReadout, hud_text("")));
        });
    }
    if let Ok(root) = root.single() {
        commands.entity(root).with_children(|root| {
            root.spawn((
                Name::new("Message line"),
                Node {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(BOTTOM_BAR_HEIGHT_PX + MESSAGE_GAP_PX),
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    justify_content: JustifyContent::Center,
                    padding: UiRect::horizontal(Val::Px(PADDING_PX)),
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|line| {
                line.spawn((MessageLine, hud_text(""), Pickable::IGNORE));
            });
        });
    }
}

/// Every frame: stockpiles, rates (amber at the cap) and pop for the local
/// player. A text is only written when it changed, so layout stays idle.
pub fn update_resource_bar(
    sim: NonSend<SimHandle>,
    strings: Option<Res<UiStrings>>,
    mut readouts: Query<(&ResourceReadout, &mut Text, &mut TextColor), Without<PopReadout>>,
    mut pop: Query<&mut Text, With<PopReadout>>,
) {
    let Some(strings) = strings else {
        return;
    };
    let view = sim.view();
    if view.player(LOCAL_PLAYER).is_none() {
        return;
    }
    let rules = sim.rules();
    let stock = &view.economy(LOCAL_PLAYER).stock;
    let rates = view.income(LOCAL_PLAYER);
    for (readout, mut text, mut color) in &mut readouts {
        let i = readout.0;
        let (Some(&amount), Some(rate)) = (stock.get(i), rates.get(i)) else {
            continue;
        };
        let want = resource_text(&strings, rules, i, amount, rate);
        if text.0 != want {
            text.0 = want;
        }
        let tint = if at_cap(rate) {
            palette::HUD_AMBER
        } else {
            palette::HUD_TEXT
        };
        if color.0 != tint {
            color.0 = tint;
        }
    }
    let (used, cap) = view.pop(LOCAL_PLAYER);
    let want = fill(
        strings.hud("pop"),
        &[("used", &used.to_string()), ("cap", &cap.to_string())],
    );
    for mut text in &mut pop {
        if text.0 != want {
            text.0 = want.clone();
        }
    }
}

/// Turn the local player's rejections, and completions of their buildings,
/// into the message line's text.
pub fn collect_messages(
    mut events: MessageReader<SimEventMsg>,
    sim: NonSend<SimHandle>,
    strings: Option<Res<UiStrings>>,
    time: Res<Time>,
    mut state: ResMut<MessageState>,
) {
    let Some(strings) = strings else {
        events.clear();
        return;
    };
    let rules = sim.rules();
    for msg in events.read() {
        let text = match &msg.event {
            SimEvent::CommandRejected { player, reason, .. } if *player == LOCAL_PLAYER => {
                reject_text(&strings, rules, reason)
            }
            SimEvent::BuildingCompleted { building } => {
                let view = sim.view();
                let Some(b) = view.building(*building).filter(|b| b.owner == LOCAL_PLAYER) else {
                    continue;
                };
                let name = rules
                    .building_kind(b.kind.0)
                    .map_or("?", |k| k.name.as_str());
                fill(strings.hud("notice.completed"), &[("building", name)])
            }
            _ => continue,
        };
        state.text = text;
        state.since = time.elapsed_secs_f64();
    }
    if !state.text.is_empty() && time.elapsed_secs_f64() - state.since > MESSAGE_SECONDS {
        state.text.clear();
    }
}

/// Mirror [`MessageState`] into the message line's text.
pub fn update_message_line(
    state: Res<MessageState>,
    mut line: Query<&mut Text, With<MessageLine>>,
) {
    if !state.is_changed() {
        return;
    }
    for mut text in &mut line {
        if text.0 != state.text {
            text.0 = state.text.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    fn strings() -> UiStrings {
        UiStrings(Strings::load(data_dir(), "en").expect("en.ron loads"))
    }

    fn rules() -> Rules {
        Rules::load(data_dir()).expect("rules load")
    }

    /// Every variant, so a reason without a message fails here.
    fn all_reasons() -> Vec<RejectReason> {
        vec![
            RejectReason::UnknownPlayer,
            RejectReason::Surrendered,
            RejectReason::NoValidUnits,
            RejectReason::DebugCommandsDisabled,
            RejectReason::UnknownUnitKind,
            RejectReason::Unreachable,
            RejectReason::OutsideBorders,
            RejectReason::CannotAfford { resource: 0 },
            RejectReason::TownLimitReached,
            RejectReason::PerTownLimitReached,
            RejectReason::UnknownBuildingKind,
            RejectReason::UnknownBuilding,
            RejectReason::UnknownResource,
            RejectReason::NotABuilder,
            RejectReason::SiteOutOfBounds,
            RejectReason::SiteBlocked,
            RejectReason::SiteOccupied,
            RejectReason::NoAdjacentResource,
            RejectReason::InvalidSlot,
            RejectReason::UnderConstruction,
            RejectReason::NotAGatherNode,
            RejectReason::PopCapReached,
            RejectReason::CannotTrainHere,
            RejectReason::QueueFull,
            RejectReason::NoFreeSlot,
            RejectReason::UnknownTech,
            RejectReason::CannotResearchHere,
            RejectReason::PrerequisiteMissing,
            RejectReason::AgeRequired,
            RejectReason::AlreadyResearched,
            RejectReason::ResearchInProgress,
            RejectReason::NotEnoughTechs,
            RejectReason::FinalAge,
            RejectReason::PlayerMismatch,
            RejectReason::InvalidTarget,
        ]
    }

    #[test]
    fn every_reject_reason_has_a_message() {
        let s = strings();
        for reason in all_reasons() {
            let key = reject_key(&reason);
            assert!(s.0.reject(key).is_some(), "en.ron has no reject[\"{key}\"]");
        }
        assert_eq!(
            s.0.reject.len(),
            all_reasons().len(),
            "no stale reject keys"
        );
    }

    #[test]
    fn rejections_read_as_the_docs_say() {
        let (s, r) = (strings(), rules());
        assert_eq!(
            reject_text(&s, &r, &RejectReason::OutsideBorders),
            "Outside your borders"
        );
        assert_eq!(
            reject_text(&s, &r, &RejectReason::PopCapReached),
            "Population cap reached"
        );
        let ore = r
            .resources
            .resources
            .iter()
            .position(|x| x.id == "ore")
            .unwrap();
        let reason = RejectReason::CannotAfford {
            resource: u8::try_from(ore).unwrap(),
        };
        assert_eq!(reject_text(&s, &r, &reason), "Not enough Ore");
    }

    struct NoAi;

    impl sim::AiController for NoAi {
        fn think(&mut self, _: sim::PlayerId, _: &sim::SimView<'_>) -> Vec<sim::Command> {
            Vec::new()
        }
    }

    #[test]
    fn the_walkthroughs_grain_readout_turns_amber_at_its_beat_and_not_before() {
        use sim::scenarios::{ECONOMY_WALKTHROUGH_AMBER_TICK, commands_at, economy_walkthrough};
        let s = strings();
        let rules = rules();
        let grain = rules
            .resources
            .resources
            .iter()
            .position(|x| x.id == "grain")
            .unwrap();
        let (setup, stream) = economy_walkthrough(&rules);
        let mut sim = sim::Sim::new(setup, rules, Box::new(NoAi));
        let mut first_amber = None;
        for t in 0..ECONOMY_WALKTHROUGH_AMBER_TICK {
            sim.step(commands_at(&stream, t));
            if first_amber.is_none() && at_cap(&sim.view().income(LOCAL_PLAYER)[grain]) {
                first_amber = Some(t);
            }
        }
        let first = first_amber.expect("Grain reaches the Yield Cap by the amber beat");
        assert!(
            first + 100 > ECONOMY_WALKTHROUGH_AMBER_TICK,
            "amber only near its beat, not from tick {first}"
        );
        let view = sim.view();
        let rate = &view.income(LOCAL_PLAYER)[grain];
        let text = resource_text(
            &s,
            sim.rules(),
            grain,
            view.economy(LOCAL_PLAYER).stock[grain],
            rate,
        );
        assert!(text.ends_with(", capped at 70"), "{text}");
    }

    #[test]
    fn readouts_show_stock_rate_and_the_lore_cap() {
        let (s, r) = (strings(), rules());
        let grain = r
            .resources
            .resources
            .iter()
            .position(|x| x.id == "grain")
            .unwrap();
        let rate = sim::RateInfo {
            raw: 20,
            cap: Some(70),
            credited: 20,
        };
        assert_eq!(
            resource_text(&s, &r, grain, 150, &rate),
            "Grain 150  +20 / 30 s"
        );
        assert!(!at_cap(&rate));
        let capped = sim::RateInfo {
            raw: 120,
            cap: Some(70),
            credited: 70,
        };
        assert_eq!(
            resource_text(&s, &r, grain, 150, &capped),
            "Grain 150  +70 / 30 s, capped at 70"
        );
        assert!(at_cap(&capped));
        let lore = r
            .resources
            .resources
            .iter()
            .position(|x| x.id == "lore")
            .unwrap();
        let uncapped = sim::RateInfo {
            raw: 10,
            cap: None,
            credited: 10,
        };
        assert_eq!(
            resource_text(&s, &r, lore, 12, &uncapped),
            "Lore 12 / 999  +10 / 30 s"
        );
    }
}
