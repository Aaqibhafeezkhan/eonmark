//! The economy half of the HUD (M3b): the resource bar and population
//! readout on the top bar, and the message line above the bottom bar.
//!
//! The resource bar shows, per resource in `resources.ron` order, its name,
//! the local player's stockpile (`stock / cap` for a capped stockpile such
//! as Lore) and the income per 30 s from `SimView::income`. The rate turns
//! amber while production is discarded at the Yield Cap (`raw > credited`,
//! `docs/design/economy.md`). The pop readout is `SimView::pop`, at the
//! right end of the top bar; since M4b the age readout and the Advance
//! button hang under it on `age_hud`'s age strip, so the top bar's room
//! goes to the readouts. The top bar's texts never wrap: a bar too full
//! for the window runs off its right end instead of splitting every
//! readout onto a second line over the world.
//!
//! The message line shows the local player's last command rejection, in the
//! words of `data/rules/strings/en.ron`, or a short notice, for
//! [`MESSAGE_SECONDS`]: a building of theirs completed, and since M4b a
//! tech of theirs researched, their next age reached, a building destroyed
//! (another player's "destroyed", their own "lost") and their Town held at
//! 0 hit points, and since M5a an annexation of one of their Towns or by
//! them starting, stopping and ending ("Town annexed", "Town was lost")
//! ([`notice_text`]). Rejections and events arrive as
//! `SimEventMsg` from the sim driver, rejections `cmd_delay` ticks after
//! the command.
//! An `InvalidTarget` for a right-click `Attack` whose target died in those
//! ticks is not shown (`orders::IssuedAttacks`, M4b).
//! Every text and number comes from data or the sim; nothing is hard-coded
//! but the layout. [`HUD_KEYS`] lists every HUD string key the game reads
//! with the placeholders it fills, and a test holds `en.ron` to it.

use std::collections::BTreeMap;

use bevy::prelude::*;
use bevy::text::FontSize;
use rules::Rules;
use rules::strings::{Strings, fill};
use sim::{BuildingId, PlayerId, RejectReason, SimEvent};

use crate::hud::{BOTTOM_BAR_HEIGHT_PX, HUD_FONT_PX, HudRoot, PADDING_PX, TopBar, spawn_hud};
use crate::orders::{IssuedAttacks, target_gone};
use crate::palette;
use crate::sim_driver::{LOCAL_PLAYER, SimEventMsg, SimHandle};

/// Every key of the `hud` table in `data/rules/strings/en.ron` that the
/// game reads, with the placeholders the code fills into it. A value may
/// use only those (any other would reach the screen as `{name}`), and
/// each is one of `rules::strings::PLACEHOLDERS`
/// (`every_hud_key_the_game_reads_exists_with_whitelisted_placeholders`,
/// which also checks that every key the game's sources pass to
/// `UiStrings::hud` as a literal is listed here).
pub const HUD_KEYS: &[(&str, &[&str])] = &[
    // Top bar (M3b; M4b age readout and Advance button).
    ("pop", &["used", "cap"]),
    ("rate", &["rate"]),
    ("rate.capped", &["rate", "cap"]),
    ("stockpile.capped", &["count", "cap"]),
    ("age", &["name"]),
    ("button.advance", &["name"]),
    ("button.advance_techs", &["count", "cap"]),
    ("tooltip.techs", &["count", "cap"]),
    // Command card (M3b; M4b Attack-move and research).
    ("button.building", &["building"]),
    ("button.train", &["unit"]),
    ("button.cancel", &[]),
    ("button.attack_move", &[]),
    ("button.research", &["name"]),
    ("button.needs", &["name"]),
    ("button.researching", &["count"]),
    ("button.researched", &[]),
    ("tooltip.cost", &["cost"]),
    ("tooltip.place", &["building"]),
    ("tooltip.cancel_site", &[]),
    ("tooltip.attack_move", &[]),
    ("tooltip.research", &["name", "count"]),
    ("tooltip.requires", &["name"]),
    // Selection panel.
    ("info.units", &["name", "count"]),
    ("info.hp", &["count", "cap"]),
    ("info.site", &["count"]),
    ("info.seat", &["count"]),
    ("info.town", &["count"]),
    ("info.workers", &["count", "cap"]),
    ("info.researched", &["name"]),
    ("info.queue", &["name"]),
    // Message line notices.
    ("notice.completed", &["building"]),
    ("notice.researched", &["name"]),
    ("notice.age", &["name"]),
    ("notice.destroyed", &["building"]),
    ("notice.lost", &["building"]),
    ("notice.town_down", &["building"]),
    // M5a: annexation notices, the annexation readout and the outcome
    // overlay (`markers.rs`, `outcome.rs`).
    ("notice.annexation_started", &["building"]),
    ("notice.annexation_stopped", &[]),
    ("notice.annexed", &[]),
    ("notice.town_lost", &["building"]),
    ("annex.attacking", &["building", "time"]),
    ("annex.defending", &["building", "time"]),
    ("outcome.victory", &[]),
    ("outcome.defeat", &[]),
    ("outcome.draw", &[]),
    ("outcome.seat_captured", &[]),
    ("outcome.all_towns_lost", &[]),
    ("outcome.surrender", &[]),
    ("outcome.territory", &["count", "cap"]),
    ("outcome.time", &["time", "count"]),
    ("outcome.hash", &["hash"]),
    ("outcome.replay", &["name"]),
];

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
    /// The attacker of every annexation timer running as the message line
    /// last heard (`AnnexationStarted` adds, `AnnexationReset` and
    /// `TownAnnexed` remove): a reset carries only the Town, and its
    /// "Annexation stopped" is for the attacker too (M5a).
    pub annexing: BTreeMap<BuildingId, PlayerId>,
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
        RejectReason::TownContested => "town_contested",
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

/// Fills the top bar with one readout per resource and, at its right end,
/// the pop readout, each one line that never wraps, and adds the message
/// line above the bottom bar.
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
                        TextLayout::no_wrap(),
                    ));
                }
            });
            bar.spawn((
                Name::new("Pop"),
                PopReadout,
                hud_text(""),
                TextLayout::no_wrap(),
            ));
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

/// The message line's notice for `event`, if the local player gets one:
/// "Scriptorium complete" (their building), "Letters I researched" (their
/// tech), "Masonry Age reached" (their age), "Watchtower destroyed"
/// (another player's building) or "Muster Hall lost" (theirs), and
/// "Town has fallen" when their Town reaches 0 hit points. Names come from
/// the rules, by the event's ids (`BuildingDestroyed` carries the owner and
/// kind of the building it removed) or the building in the view.
///
/// Annexation (M5a), for a Town the local player owns or annexes: "Town is
/// being annexed" when a timer starts; "Annexation stopped" when it
/// resets, for the Town's owner or for the attacker `annexing` names (the
/// reset event carries only the Town); "Town annexed" for the player who
/// took it and "Town was lost" for the one who lost it. A flip the local
/// player only watches says nothing.
pub fn notice_text(
    view: &sim::SimView<'_>,
    rules: &Rules,
    strings: &UiStrings,
    event: &SimEvent,
    annexing: &BTreeMap<BuildingId, PlayerId>,
) -> Option<String> {
    let building_name = |kind: u16| {
        rules
            .building_kind(kind)
            .map_or("?", |k| k.name.as_str())
            .to_string()
    };
    let own_building = |id: sim::BuildingId| {
        view.building(id)
            .filter(|b| b.owner == LOCAL_PLAYER)
            .map(|b| building_name(b.kind.0))
    };
    match event {
        SimEvent::BuildingCompleted { building } => {
            let name = own_building(*building)?;
            Some(fill(
                strings.hud("notice.completed"),
                &[("building", &name)],
            ))
        }
        SimEvent::ResearchCompleted { player, tech } if *player == LOCAL_PLAYER => {
            let name = rules.tech(tech.0).map_or("?", |t| t.name.as_str());
            Some(fill(strings.hud("notice.researched"), &[("name", name)]))
        }
        SimEvent::AgeAdvanced { player, age } if *player == LOCAL_PLAYER => {
            let name = rules
                .ages
                .get(usize::from(*age))
                .map_or("?", |a| a.name.as_str());
            Some(fill(strings.hud("notice.age"), &[("name", name)]))
        }
        SimEvent::BuildingDestroyed { owner, kind, .. } => {
            let key = if *owner == LOCAL_PLAYER {
                "notice.lost"
            } else {
                "notice.destroyed"
            };
            Some(fill(
                strings.hud(key),
                &[("building", &building_name(kind.0))],
            ))
        }
        SimEvent::TownAtZeroHp { town } => {
            let name = own_building(*town)?;
            Some(fill(
                strings.hud("notice.town_down"),
                &[("building", &name)],
            ))
        }
        SimEvent::AnnexationStarted { town, attacker } => {
            let b = view.building(*town)?;
            (b.owner == LOCAL_PLAYER || *attacker == LOCAL_PLAYER).then(|| {
                fill(
                    strings.hud("notice.annexation_started"),
                    &[("building", &building_name(b.kind.0))],
                )
            })
        }
        SimEvent::AnnexationReset { town } => {
            let owned = view
                .building(*town)
                .is_some_and(|b| b.owner == LOCAL_PLAYER);
            let attacking = annexing.get(town) == Some(&LOCAL_PLAYER);
            (owned || attacking).then(|| strings.hud("notice.annexation_stopped").to_string())
        }
        SimEvent::TownAnnexed { town, from, to } => {
            if *to == LOCAL_PLAYER {
                Some(strings.hud("notice.annexed").to_string())
            } else if *from == LOCAL_PLAYER {
                let name = view
                    .building(*town)
                    .map_or("?".to_string(), |b| building_name(b.kind.0));
                Some(fill(
                    strings.hud("notice.town_lost"),
                    &[("building", &name)],
                ))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Turn the local player's rejections, and the events [`notice_text`]
/// names, into the message line's text; the latest wins. A rejection
/// [`IssuedAttacks::hides`] (an attack on a target that died before it
/// applied) is skipped.
pub fn collect_messages(
    mut events: MessageReader<SimEventMsg>,
    sim: NonSend<SimHandle>,
    strings: Option<Res<UiStrings>>,
    attacks: Option<Res<IssuedAttacks>>,
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
            SimEvent::CommandRejected {
                player,
                seq,
                reason,
            } if *player == LOCAL_PLAYER => {
                let view = sim.view();
                if attacks
                    .as_ref()
                    .is_some_and(|a| a.hides(*seq, reason, |t| target_gone(&view, t)))
                {
                    continue;
                }
                reject_text(&strings, rules, reason)
            }
            event => {
                let text = notice_text(&sim.view(), rules, &strings, event, &state.annexing);
                // Who annexes what, for a later reset's notice.
                match event {
                    SimEvent::AnnexationStarted { town, attacker } => {
                        state.annexing.insert(*town, *attacker);
                    }
                    SimEvent::AnnexationReset { town } | SimEvent::TownAnnexed { town, .. } => {
                        state.annexing.remove(town);
                    }
                    _ => {}
                }
                let Some(text) = text else {
                    continue;
                };
                text
            }
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
            RejectReason::TownContested,
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

    /// The message line drops an `InvalidTarget` for a remembered attack
    /// whose target is gone, and shows every other rejection.
    #[test]
    fn invalid_target_for_a_dead_target_is_not_shown() {
        use sim::{TargetId, UnitId};
        let rules = rules();
        let setup = sim::MatchSetup::scenario(&rules, 1);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SimEventMsg>()
            .insert_resource(strings())
            .init_resource::<MessageState>()
            .init_resource::<IssuedAttacks>()
            .add_systems(Update, collect_messages);
        app.world_mut()
            .insert_non_send(SimHandle::from_setup(setup, rules));
        {
            let mut attacks = app.world_mut().resource_mut::<IssuedAttacks>();
            // No unit exists in this empty match: both targets are gone.
            attacks.record(4, TargetId::Unit(UnitId(7)));
            attacks.record(5, TargetId::Unit(UnitId(8)));
        }
        let rejected = |seq, reason| SimEventMsg {
            tick: 3,
            event: SimEvent::CommandRejected {
                player: LOCAL_PLAYER,
                seq,
                reason,
            },
        };
        app.world_mut()
            .write_message(rejected(4, RejectReason::InvalidTarget));
        app.update();
        assert_eq!(app.world().resource::<MessageState>().text, "", "hidden");
        // Not a remembered attack: shown.
        app.world_mut()
            .write_message(rejected(6, RejectReason::InvalidTarget));
        app.update();
        assert_eq!(
            app.world().resource::<MessageState>().text,
            "Cannot attack that"
        );
        // A remembered attack rejected for another reason: shown.
        app.world_mut()
            .write_message(rejected(5, RejectReason::NoValidUnits));
        app.update();
        assert_eq!(
            app.world().resource::<MessageState>().text,
            "No selected unit can do that"
        );
    }

    /// Real events from a small match, each named with the view of its
    /// tick: P0's research and age, P0's Scriptorium placed complete, a P1
    /// Croft that P0's idle Mangonels destroy and the Scriptorium that P1's
    /// destroy; P1's own research and Croft get no notice. A Town held at
    /// 0 needs a long siege, so its event is written by hand: the local
    /// player's Seat gets the notice, the other player's does not. So is an
    /// age reached by P1, which has no two techs here: only the local
    /// player's age is named. The message line shows the latest.
    #[test]
    fn notices_name_research_age_and_destroyed_buildings() {
        use sim::{BuildingId, Command, PlayerCommand, PlayerId};
        let rules = rules();
        let s = strings();
        let mut setup = sim::MatchSetup::scenario(&rules, 1);
        setup.start = sim::StartKind::Standard;
        let mut sim = SimHandle::from_setup(setup, rules.clone());
        let p1 = PlayerId(1);
        let tech = |id| sim::TechId(rules.tech_by_id(id).unwrap().0);
        let res = |id| u8::try_from(rules.resource_index(id).unwrap()).unwrap();
        let bkind = |id| sim::BuildingKindId(rules.building_kind_by_id(id).unwrap().0);
        let mangonel = sim::UnitKindId(
            u16::try_from(rules.units.iter().position(|u| u.id == "mangonel").unwrap()).unwrap(),
        );
        let place = |owner, kind, x, y| Command::DebugPlaceBuilding {
            owner,
            kind: bkind(kind),
            tile: sim::Tile::new(x, y),
            complete: true,
        };
        let spawn = |owner, x, y| Command::DebugSpawn {
            owner,
            kind: mangonel,
            at: sim::FxVec2::from_ints(x, y),
            count: 3,
        };
        let batch = |p: PlayerId, first: u32, cmds: Vec<Command>| {
            (first..)
                .zip(cmds)
                .map(|(seq, c)| PlayerCommand::new(p, seq, c))
                .collect::<Vec<_>>()
        };
        let mut cmds = batch(
            LOCAL_PLAYER,
            0,
            vec![
                Command::DebugResearch {
                    owner: LOCAL_PLAYER,
                    tech: tech("letters_1"),
                },
                Command::DebugResearch {
                    owner: LOCAL_PLAYER,
                    tech: tech("trade_1"),
                },
                Command::DebugGrant {
                    owner: LOCAL_PLAYER,
                    resource: res("grain"),
                    amount: 250,
                },
                Command::DebugGrant {
                    owner: LOCAL_PLAYER,
                    resource: res("lore"),
                    amount: 100,
                },
                Command::AdvanceAge {
                    player: LOCAL_PLAYER,
                },
                // BuildingId(3), and P0's Mangonels 11..=13.
                place(LOCAL_PLAYER, "scriptorium", 34, 28),
                spawn(LOCAL_PLAYER, 10, 34),
            ],
        );
        cmds.extend(batch(
            p1,
            0,
            vec![
                Command::DebugResearch {
                    owner: p1,
                    tech: tech("arms_1"),
                },
                // BuildingId(4), and P1's Mangonels 14..=16.
                place(p1, "croft", 10, 28),
                spawn(p1, 34, 36),
            ],
        ));
        // Mangonels prefer buildings: idle, each trio destroys the enemy
        // building in its reach.
        let (mut events, mut notices) = (Vec::new(), Vec::new());
        sim.step(&cmds);
        for _ in 0..sim.setup().cmd_delay {
            sim.step(&[]);
        }
        for _ in 0..2000 {
            let view = sim.view();
            if view.building(BuildingId(3)).is_none() && view.building(BuildingId(4)).is_none() {
                break;
            }
            let stepped = sim.drain_events();
            let view = sim.view();
            notices.extend(
                stepped
                    .iter()
                    .filter_map(|e| notice_text(&view, &rules, &s, e, &BTreeMap::new())),
            );
            events.extend(stepped);
            sim.step(&[]);
        }
        let stepped = sim.drain_events();
        notices.extend(
            stepped
                .iter()
                .filter_map(|e| notice_text(&sim.view(), &rules, &s, e, &BTreeMap::new())),
        );
        events.extend(stepped);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, SimEvent::CommandRejected { .. })),
            "{events:?}"
        );
        assert_eq!(
            notices,
            [
                "Letters I researched",
                "Trade I researched",
                "Masonry Age reached",
                "Scriptorium complete",
                "Croft destroyed",
                "Scriptorium lost",
            ],
            "{events:?}"
        );
        let view = sim.view();
        // A Town held at 0: only the local player's.
        let down = |town| SimEvent::TownAtZeroHp {
            town: BuildingId(town),
        };
        assert_eq!(
            notice_text(&view, &rules, &s, &down(1), &BTreeMap::new()).as_deref(),
            Some("Town has fallen")
        );
        assert_eq!(
            notice_text(&view, &rules, &s, &down(2), &BTreeMap::new()),
            None
        );
        // An age reached: only the local player's.
        let aged = |player| SimEvent::AgeAdvanced { player, age: 1 };
        assert_eq!(
            notice_text(&view, &rules, &s, &aged(LOCAL_PLAYER), &BTreeMap::new()).as_deref(),
            Some("Masonry Age reached")
        );
        assert_eq!(
            notice_text(&view, &rules, &s, &aged(p1), &BTreeMap::new()),
            None
        );

        // The message line: the latest notice of a frame wins.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SimEventMsg>()
            .insert_resource(s)
            .init_resource::<MessageState>()
            .add_systems(Update, collect_messages);
        app.world_mut().insert_non_send(sim);
        for event in events {
            app.world_mut()
                .write_message(SimEventMsg { tick: 3, event });
        }
        app.update();
        assert_eq!(
            app.world().resource::<MessageState>().text,
            "Scriptorium lost"
        );
        app.world_mut().write_message(SimEventMsg {
            tick: 4,
            event: down(1),
        });
        app.update();
        assert_eq!(
            app.world().resource::<MessageState>().text,
            "Town has fallen"
        );
    }

    /// Annexation notices (M5a) go to the Town's owner and its attacker,
    /// and a reset, which names only the Town, reaches the attacker through
    /// the timers the message line heard start.
    #[test]
    fn annexation_notices_name_the_owner_and_the_attacker() {
        use sim::{BuildingId, PlayerId};
        let rules = rules();
        let s = strings();
        let mut setup = sim::MatchSetup::scenario(&rules, 1);
        setup.start = sim::StartKind::Standard;
        let sim = SimHandle::from_setup(setup, rules.clone());
        let view = sim.view();
        // The Seats: B1 is the local player's, B2 P1's; P2 is a bystander.
        let (mine, theirs) = (BuildingId(1), BuildingId(2));
        assert_eq!(view.building(mine).unwrap().owner, LOCAL_PLAYER);
        assert_eq!(view.building(theirs).unwrap().owner, PlayerId(1));
        let (p1, p2) = (PlayerId(1), PlayerId(2));
        let none = BTreeMap::new();
        let notice = |e: SimEvent, annexing: &BTreeMap<BuildingId, PlayerId>| {
            notice_text(&view, &rules, &s, &e, annexing)
        };
        let started = |town, attacker| SimEvent::AnnexationStarted { town, attacker };
        assert_eq!(
            notice(started(theirs, LOCAL_PLAYER), &none).as_deref(),
            Some("Town is being annexed"),
            "the attacker"
        );
        assert_eq!(
            notice(started(mine, p1), &none).as_deref(),
            Some("Town is being annexed"),
            "the owner"
        );
        assert_eq!(notice(started(theirs, p2), &none), None, "a bystander");
        let reset = |town| SimEvent::AnnexationReset { town };
        assert_eq!(
            notice(reset(mine), &none).as_deref(),
            Some("Annexation stopped")
        );
        let attacking: BTreeMap<_, _> = [(theirs, LOCAL_PLAYER)].into();
        assert_eq!(
            notice(reset(theirs), &attacking).as_deref(),
            Some("Annexation stopped")
        );
        assert_eq!(notice(reset(theirs), &none), None);
        assert_eq!(notice(reset(theirs), &[(theirs, p2)].into()), None);
        let flipped = |town, from, to| SimEvent::TownAnnexed { town, from, to };
        assert_eq!(
            notice(flipped(theirs, p1, LOCAL_PLAYER), &none).as_deref(),
            Some("Town annexed")
        );
        assert_eq!(
            notice(flipped(mine, LOCAL_PLAYER, p1), &none).as_deref(),
            Some("Town was lost")
        );
        assert_eq!(notice(flipped(theirs, p1, p2), &none), None);
        assert_eq!(
            notice(
                SimEvent::MatchDecided {
                    result: sim::MatchResult {
                        outcome: sim::Outcome::Draw,
                        reason: sim::OutcomeReason::Surrender,
                        tick: 3
                    }
                },
                &none
            ),
            None,
            "the overlay shows the outcome"
        );

        // The message line remembers who annexes what for a later reset.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SimEventMsg>()
            .insert_resource(s)
            .init_resource::<MessageState>()
            .add_systems(Update, collect_messages);
        app.world_mut().insert_non_send(sim);
        app.world_mut().write_message(SimEventMsg {
            tick: 3,
            event: started(theirs, LOCAL_PLAYER),
        });
        app.update();
        let state = app.world().resource::<MessageState>().clone();
        assert_eq!(state.text, "Town is being annexed");
        assert_eq!(state.annexing, [(theirs, LOCAL_PLAYER)].into());
        app.world_mut().write_message(SimEventMsg {
            tick: 4,
            event: reset(theirs),
        });
        app.update();
        let state = app.world().resource::<MessageState>().clone();
        assert_eq!(state.text, "Annexation stopped");
        assert!(state.annexing.is_empty());
        app.world_mut().write_message(SimEventMsg {
            tick: 5,
            event: started(theirs, LOCAL_PLAYER),
        });
        app.world_mut().write_message(SimEventMsg {
            tick: 1205,
            event: flipped(theirs, p1, LOCAL_PLAYER),
        });
        app.update();
        let state = app.world().resource::<MessageState>().clone();
        assert_eq!(state.text, "Town annexed");
        assert!(state.annexing.is_empty());
    }

    /// `HUD_KEYS` names every `hud` key the game reads: each exists in
    /// `en.ron`; its value uses only the placeholders the code fills, each
    /// of them on the strings file's whitelist; and every `hud("...")`
    /// literal in the game's sources is listed (keys read through a
    /// variable, the Seat's and a Town's, are listed by hand).
    #[test]
    fn every_hud_key_the_game_reads_exists_with_whitelisted_placeholders() {
        let s = strings();
        let mut seen = std::collections::BTreeSet::new();
        for (key, filled) in HUD_KEYS {
            assert!(seen.insert(*key), "{key} is listed twice");
            let value =
                s.0.hud(key)
                    .unwrap_or_else(|| panic!("en.ron has no hud[\"{key}\"]"));
            for name in *filled {
                assert!(
                    rules::strings::PLACEHOLDERS.contains(name),
                    "{key}: {{{name}}} is not a whitelisted placeholder"
                );
            }
            let mut rest = value;
            while let Some(open) = rest.find('{') {
                let close = rest[open..].find('}').expect("data-check balances braces");
                let name = &rest[open + 1..open + close];
                assert!(
                    filled.contains(&name),
                    "hud[\"{key}\"] = {value:?} uses {{{name}}}, which the game does not fill"
                );
                rest = &rest[open + close + 1..];
            }
        }
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut literals = 0;
        for entry in std::fs::read_dir(&src).expect("src") {
            let path = entry.expect("entry").path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read");
            // The game's code, not its tests.
            let code = text.split("\n#[cfg(test)]\nmod tests").next().unwrap_or("");
            for (i, _) in code.match_indices("hud(\"") {
                let rest = &code[i + 5..];
                let key = &rest[..rest.find('"').expect("closing quote")];
                literals += 1;
                assert!(
                    seen.contains(key),
                    "{}: hud(\"{key}\") is not in HUD_KEYS",
                    path.display()
                );
            }
        }
        assert!(literals > 30, "found {literals} hud(\"...\") reads");
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
