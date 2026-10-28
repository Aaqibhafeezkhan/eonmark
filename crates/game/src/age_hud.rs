//! The age readout and the Advance button (M4b), on the age strip: a
//! panel hanging under the right end of the top bar, below the pop readout.
//! The top bar keeps only the resource readouts and the pop readout, as in
//! M3b, so its room for capped rates is not spent on the age.
//!
//! The readout names the local player's age (`SimView::age`, the name from
//! `ages.ron`): "Hearth Age". The Advance button is shown in every age but
//! the last, greyed, not hidden, while the sim would reject `AdvanceAge`
//! (owner decision 7, ROADMAP M4b Notes; `ART_STYLE.md`: "greyed, not hidden"):
//! `SimView::check_advance_age` alone decides, the game copies none of its
//! checks. Its one line is the tech count while that is what blocks it
//! ("Advance: 1 / 2 techs"), else the next age and its cost (`Rules::amounts`
//! of the next age in `ages.ron`), "Advance to Masonry Age: 250 Grain 100
//! Lore", greyed while a resource is short as a card button is. The tooltip
//! shows under the strip: "Advance to Masonry Age: Costs 250 Grain 100
//! Lore" when enabled, else the rejection's message ("Not enough Grain"),
//! "n / m techs researched" and the cost. `n` is `SimView::age_tech_count`,
//! the techs that count toward an age, the number the sim compares with
//! the gate: since M5a's rules data the Watchtower's Harrying I and II are
//! side techs that count toward none. A click issues `AdvanceAge {
//! player: LOCAL_PLAYER }` from a `Pointer<Click>` observer, or puts the
//! tooltip on the message line when greyed. In the last age the button is
//! hidden (`Display::None`). The hover colours and the tooltip are the
//! command card's (`command_card::card_button_colors`,
//! `command_card::update_tooltip`), which follow any entity with a
//! [`CardState`].

use bevy::picking::events::{Click, Pointer};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use rules::Rules;
use rules::strings::fill;
use sim::{Command, RejectReason, SimView};

use crate::command_card::{CARD_FONT_PX, CardState, TooltipPlace, card_button_colors, cost_text};
use crate::economy_hud::{MessageState, UiStrings, reject_text, spawn_economy_hud};
use crate::hud::{HUD_FONT_PX, HudRoot, PADDING_PX, TOP_BAR_HEIGHT_PX};
use crate::palette;
use crate::sim_driver::{LOCAL_PLAYER, PendingCommands, SimHandle};

/// Height of the age strip under the top bar, logical px: the top bar's.
pub const AGE_STRIP_HEIGHT_PX: f32 = TOP_BAR_HEIGHT_PX;
/// Gap between the age readout and the Advance button, logical px.
pub const AGE_STRIP_GAP_PX: f32 = 12.0;
/// Advance button height, logical px (one line inside the 28 px strip).
pub const ADVANCE_BUTTON_HEIGHT_PX: f32 = 24.0;
/// Advance button horizontal padding, logical px.
pub const ADVANCE_PADDING_PX: f32 = 6.0;

/// The age strip: the panel under the right end of the top bar that holds
/// the age readout and the Advance button.
#[derive(Component, Debug, Clone, Copy)]
pub struct AgeStrip;

/// The age readout's text.
#[derive(Component, Debug, Clone, Copy)]
pub struct AgeReadout;

/// The Advance button.
#[derive(Component, Debug, Clone, Copy)]
pub struct AdvanceButton;

/// The Advance button's label.
#[derive(Component, Debug, Clone, Copy)]
pub struct AdvanceLabel;

/// What the Advance button shows now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvanceView {
    /// Shown; `false` in the last age.
    pub visible: bool,
    /// Its one line.
    pub label: String,
    /// Enabled (the sim would accept `AdvanceAge`) and the tooltip.
    pub state: CardState,
}

/// The readout's text: the local player's age, "Hearth Age".
pub fn age_text(view: &SimView<'_>, rules: &Rules, strings: &UiStrings) -> String {
    let age = &rules.ages[usize::from(view.age(LOCAL_PLAYER))];
    fill(strings.hud("age"), &[("name", &age.name)])
}

/// The Advance button now: enabled exactly when
/// `SimView::check_advance_age` accepts; labelled "Advance: 1 / 2 techs"
/// while the tech count is what the sim rejects (`NotEnoughTechs`), else
/// with the next age and its cost, "Advance to Masonry Age: 250 Grain 100
/// Lore"; hidden in the last age. The tooltip is "Advance to Masonry Age:
/// Costs 250 Grain 100 Lore" when enabled, else the rejection's message,
/// "n / m techs researched" and the cost.
pub fn advance_view(view: &SimView<'_>, rules: &Rules, strings: &UiStrings) -> AdvanceView {
    let verdict = view.check_advance_age(LOCAL_PLAYER);
    let Some(next) = rules.ages.get(usize::from(view.age(LOCAL_PLAYER)) + 1) else {
        return AdvanceView {
            visible: false,
            label: String::new(),
            state: CardState {
                enabled: false,
                tooltip: reject_text(strings, rules, &RejectReason::FinalAge),
            },
        };
    };
    let counts = [
        ("count", view.age_tech_count(LOCAL_PLAYER).to_string()),
        ("cap", next.techs_required.to_string()),
    ];
    let counts = counts.each_ref().map(|(k, v)| (*k, v.as_str()));
    let advance = fill(strings.hud("button.advance"), &[("name", &next.name)]);
    let cost = cost_text(rules, &rules.amounts(&next.cost));
    let label = match &verdict {
        Err(RejectReason::NotEnoughTechs) => fill(strings.hud("button.advance_techs"), &counts),
        _ => format!("{advance}: {cost}"),
    };
    let costs = fill(strings.hud("tooltip.cost"), &[("cost", &cost)]);
    let tooltip = match &verdict {
        Ok(_) => format!("{advance}: {costs}"),
        Err(reason) => format!(
            "{}. {}. {costs}",
            reject_text(strings, rules, reason),
            fill(strings.hud("tooltip.techs"), &counts)
        ),
    };
    AdvanceView {
        visible: true,
        label,
        state: CardState {
            enabled: verdict.is_ok(),
            tooltip,
        },
    }
}

/// The age readout and the Advance button.
pub struct AgeHudPlugin;

impl Plugin for AgeHudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_age_hud.after(spawn_economy_hud))
            .add_systems(Update, update_age_hud.before(card_button_colors));
    }
}

/// One line of HUD text that never wraps.
fn text(value: &str, size: f32) -> impl Bundle {
    (
        Text::new(value),
        crate::fonts::hud_font(size),
        TextColor(palette::HUD_TEXT),
        TextLayout::no_wrap(),
    )
}

/// Adds the age strip to the HUD root: a panel flush with the bottom of the
/// top bar at its right end, holding the age readout and then the Advance
/// button, its content aligned with the pop readout's right edge. It grows
/// leftward with the button's label and blocks world clicks, as the bars do.
pub fn spawn_age_hud(mut commands: Commands, root: Query<Entity, With<HudRoot>>) {
    let Ok(root) = root.single() else {
        return;
    };
    let strip = commands
        .spawn((
            Name::new("Age strip"),
            AgeStrip,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(TOP_BAR_HEIGHT_PX),
                right: Val::Px(0.0),
                height: Val::Px(AGE_STRIP_HEIGHT_PX),
                padding: UiRect::horizontal(Val::Px(PADDING_PX)),
                column_gap: Val::Px(AGE_STRIP_GAP_PX),
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(palette::HUD_PANEL),
        ))
        .id();
    let readout = commands
        .spawn((Name::new("Age"), AgeReadout, text("", HUD_FONT_PX)))
        .id();
    let button = commands
        .spawn((
            Name::new("Advance button"),
            AdvanceButton,
            CardState::default(),
            TooltipPlace::BelowAgeStrip,
            Button,
            Node {
                height: Val::Px(ADVANCE_BUTTON_HEIGHT_PX),
                padding: UiRect::horizontal(Val::Px(ADVANCE_PADDING_PX)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(palette::HUD_BUTTON),
        ))
        .observe(on_advance_click)
        .with_children(|button| {
            button.spawn((AdvanceLabel, text("", CARD_FONT_PX), Pickable::IGNORE));
        })
        .id();
    commands.entity(strip).add_children(&[readout, button]);
    commands.entity(root).add_children(&[strip]);
}

/// Every frame: the readout, and the button's label, state and visibility.
/// Writes only what changed.
pub fn update_age_hud(
    sim: NonSend<SimHandle>,
    strings: Option<Res<UiStrings>>,
    mut readout: Query<&mut Text, (With<AgeReadout>, Without<AdvanceLabel>)>,
    mut button: Query<(&mut CardState, &mut Node), With<AdvanceButton>>,
    mut label: Query<&mut Text, (With<AdvanceLabel>, Without<AgeReadout>)>,
) {
    let Some(strings) = strings else {
        return;
    };
    let view = sim.view();
    if view.player(LOCAL_PLAYER).is_none() {
        return;
    }
    let rules = sim.rules();
    let age = age_text(&view, rules, &strings);
    for mut t in &mut readout {
        if t.0 != age {
            t.0.clone_from(&age);
        }
    }
    let want = advance_view(&view, rules, &strings);
    let display = if want.visible {
        Display::Flex
    } else {
        Display::None
    };
    for (mut state, mut node) in &mut button {
        if *state != want.state {
            *state = want.state.clone();
        }
        if node.display != display {
            node.display = display;
        }
    }
    for mut t in &mut label {
        if t.0 != want.label {
            t.0.clone_from(&want.label);
        }
    }
}

/// A primary click on the Advance button: `AdvanceAge` for the local
/// player, or, when greyed, its tooltip (the rejection's message first) on
/// the message line, as a greyed card button does. Nothing while the local
/// slot is a bot (M5b, `--spectate`).
pub fn on_advance_click(
    click: On<Pointer<Click>>,
    buttons: Query<&CardState, With<AdvanceButton>>,
    mut pending: ResMut<PendingCommands>,
    mut message: ResMut<MessageState>,
    time: Res<Time>,
    seat: Option<Res<crate::sim_driver::LocalSeat>>,
) {
    if click.event().event.button != PointerButton::Primary
        || !crate::sim_driver::local_player_is_human(seat)
    {
        return;
    }
    let Ok(state) = buttons.get(click.event().entity) else {
        return;
    };
    if !state.enabled {
        message.text.clone_from(&state.tooltip);
        message.since = time.elapsed_secs_f64();
        return;
    }
    let seq = pending.push_local(Command::AdvanceAge {
        player: LOCAL_PLAYER,
    });
    info!("Advance: AdvanceAge seq {seq}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economy_hud::PopReadout;
    use crate::hud::spawn_hud;
    use bevy::text::FontSize;

    fn data_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    fn strings() -> UiStrings {
        UiStrings(rules::strings::Strings::load(data_dir(), "en").expect("strings load"))
    }

    /// A two-player match from the standard start (150 Grain, 150
    /// Lumber, no Ore or Lore).
    fn standard() -> SimHandle {
        let rules = Rules::load(data_dir()).expect("rules load");
        let mut setup = sim::MatchSetup::scenario(&rules, 1);
        setup.start = sim::StartKind::Standard;
        SimHandle::from_setup(setup, rules)
    }

    /// The HUD with the economy bar and the age readout, on a standard
    /// match, after Startup and one frame.
    fn hud_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<crate::sim_driver::SimEventMsg>()
            .insert_resource(strings())
            .init_resource::<PendingCommands>()
            .add_systems(Startup, spawn_hud)
            .add_plugins((crate::economy_hud::EconomyHudPlugin, AgeHudPlugin));
        app.world_mut().insert_non_send(standard());
        app.update();
        app
    }

    /// The next seq [`run`] stamps (the click's commands go through
    /// `PendingCommands` instead).
    #[derive(Resource, Default)]
    struct NextSeq(u32);

    /// P0 commands with their own seqs, stepped until they have applied;
    /// none may be rejected.
    fn run(app: &mut App, cmds: Vec<Command>) {
        let world = app.world_mut();
        let mut next = world.get_resource_or_insert_with(NextSeq::default);
        let first = 1000 + next.0;
        next.0 += u32::try_from(cmds.len()).unwrap();
        let mut sim = world.non_send_mut::<SimHandle>();
        let batch: Vec<_> = (first..)
            .zip(cmds)
            .map(|(seq, c)| sim::PlayerCommand::new(LOCAL_PLAYER, seq, c))
            .collect();
        sim.step(&batch);
        for _ in 0..sim.setup().cmd_delay {
            sim.step(&[]);
        }
        let events = sim.drain_events();
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, sim::SimEvent::CommandRejected { .. })),
            "{events:?}"
        );
        app.update();
    }

    fn tech(app: &App, id: &str) -> sim::TechId {
        let sim = app.world().non_send::<SimHandle>();
        sim::TechId(sim.rules().tech_by_id(id).unwrap().0)
    }

    fn grant(app: &App, id: &str, amount: u32) -> Command {
        let sim = app.world().non_send::<SimHandle>();
        Command::DebugGrant {
            owner: LOCAL_PLAYER,
            resource: u8::try_from(sim.rules().resource_index(id).unwrap()).unwrap(),
            amount,
        }
    }

    fn research(app: &App, id: &str) -> Command {
        Command::DebugResearch {
            owner: LOCAL_PLAYER,
            tech: tech(app, id),
        }
    }

    /// What the HUD shows: the readout, the label, the button's state and
    /// whether it is laid out.
    fn shown(app: &mut App) -> (String, String, CardState, bool) {
        let world = app.world_mut();
        let readout = world
            .query_filtered::<&Text, With<AgeReadout>>()
            .single(world)
            .unwrap()
            .0
            .clone();
        let label = world
            .query_filtered::<&Text, With<AdvanceLabel>>()
            .single(world)
            .unwrap()
            .0
            .clone();
        let (state, node) = world
            .query_filtered::<(&CardState, &Node), With<AdvanceButton>>()
            .single(world)
            .unwrap();
        (readout, label, state.clone(), node.display != Display::None)
    }

    /// The pure view, the sim's verdict and the HUD agree.
    fn check(app: &mut App) -> AdvanceView {
        let shown = shown(app);
        let sim = app.world().non_send::<SimHandle>();
        let view = sim.view();
        let want = advance_view(&view, sim.rules(), &strings());
        assert_eq!(
            want.state.enabled,
            view.check_advance_age(LOCAL_PLAYER).is_ok(),
            "{want:?}"
        );
        assert_eq!(
            !want.visible,
            view.check_advance_age(LOCAL_PLAYER) == Err(RejectReason::FinalAge)
        );
        assert_eq!(
            shown,
            (
                age_text(&view, sim.rules(), &strings()),
                want.label.clone(),
                want.state.clone(),
                want.visible
            )
        );
        want
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

    /// The readout and then the Advance button sit on the age strip, which
    /// hangs under the right end of the top bar, the button's tooltip
    /// below the strip; the top bar holds the resource readouts and the
    /// pop readout, as in M3b.
    #[test]
    fn the_age_strip_hangs_under_the_top_bars_right_end() {
        let mut app = hud_app();
        let world = app.world_mut();
        let (strip, node, row) = world
            .query_filtered::<(Entity, &Node, &Children), With<AgeStrip>>()
            .single(world)
            .unwrap();
        let (node, row) = (node.clone(), row.to_vec());
        assert_eq!(
            (node.position_type, node.top, node.right, node.height),
            (
                PositionType::Absolute,
                Val::Px(TOP_BAR_HEIGHT_PX),
                Val::Px(0.0),
                Val::Px(AGE_STRIP_HEIGHT_PX)
            )
        );
        assert!(
            world
                .get::<ChildOf>(strip)
                .is_some_and(|p| world.get::<HudRoot>(p.parent()).is_some()),
            "on the HUD root"
        );
        assert_eq!(row.len(), 2);
        assert!(world.get::<AgeReadout>(row[0]).is_some());
        assert!(world.get::<AdvanceButton>(row[1]).is_some());
        assert_eq!(
            world.get::<TooltipPlace>(row[1]),
            Some(&TooltipPlace::BelowAgeStrip)
        );
        let bar = world
            .query_filtered::<&Children, With<crate::hud::TopBar>>()
            .single(world)
            .unwrap()
            .to_vec();
        assert_eq!(bar.len(), 2, "the resource bar and the pop readout");
        assert!(world.get::<PopReadout>(bar[1]).is_some());
        let readouts = world.get::<Children>(bar[0]).unwrap();
        assert!(readouts.iter().all(|r| {
            world
                .get::<crate::economy_hud::ResourceReadout>(r)
                .is_some()
        }));
    }

    /// The width `e` lays out to, logical px, with `text_of` the text of a
    /// readout (else its current text): a text, the sum of the UI font's
    /// advances at its size (Inter since the art pass,
    /// `crate::fonts::measure`); a node, its children side by side with
    /// its column gap, plus its horizontal padding; nothing when hidden.
    fn laid_out_width(world: &World, e: Entity, text_of: &dyn Fn(Entity) -> Option<String>) -> f32 {
        let px = |v: Val| if let Val::Px(x) = v { x } else { 0.0 };
        let node = world.get::<Node>(e);
        if node.is_some_and(|n| n.display == Display::None) {
            return 0.0;
        }
        let padding = node.map_or(0.0, |n| px(n.padding.left) + px(n.padding.right));
        if let Some(text) = world.get::<Text>(e) {
            let Some(FontSize::Px(font)) = world.get::<TextFont>(e).map(|f| f.font_size) else {
                panic!("HUD text is sized in px");
            };
            let shown = text_of(e).unwrap_or_else(|| text.0.clone());
            return padding + crate::fonts::measure::text_width_px(&shown, font);
        }
        let children: Vec<Entity> = world
            .get::<Children>(e)
            .map(|c| c.to_vec())
            .unwrap_or_default();
        let gap = node.map_or(0.0, |n| px(n.column_gap));
        padding
            + children
                .iter()
                .map(|c| laid_out_width(world, *c, text_of))
                .sum::<f32>()
            + gap * children.len().saturating_sub(1) as f32
    }

    /// The top bar fits the default 1280 px window on one line with every
    /// stockpile at four digits (Lore at its 999 cap), two rates held at a
    /// Trade Yield Cap of 100 ("+100 / 30 s, capped at 100") and a pop of
    /// 100 / 100, whatever else it shows. (Before M4b the bar held only the
    /// resources and the pop; the age readout and the Advance button there
    /// left too little room for a capped rate.)
    #[test]
    fn the_top_bar_fits_four_digit_stocks_and_two_capped_rates() {
        use crate::economy_hud::{ResourceReadout, resource_text};
        let mut app = hud_app();
        let world = app.world_mut();
        let bar = world
            .query_filtered::<Entity, With<crate::hud::TopBar>>()
            .single(world)
            .unwrap();
        let s = strings();
        let rules = app.world().non_send::<SimHandle>().rules().clone();
        let text_of = |e: Entity| -> Option<String> {
            let world = app.world();
            if let Some(r) = world.get::<ResourceReadout>(e) {
                let def = &rules.resources.resources[r.0];
                let stock = def.stockpile_cap.map_or(1234, i64::from);
                let rate = sim::RateInfo {
                    raw: if r.0 < 2 { 150 } else { 100 },
                    cap: Some(100),
                    credited: 100,
                };
                return Some(resource_text(&s, &rules, r.0, stock, &rate));
            }
            world
                .get::<PopReadout>(e)
                .map(|_| fill(s.hud("pop"), &[("used", "100"), ("cap", "100")]))
        };
        let world = app.world();
        let shown: Vec<String> = world
            .get::<Children>(bar)
            .unwrap()
            .iter()
            .flat_map(|c| {
                let below: Vec<Entity> = world
                    .get::<Children>(c)
                    .map(|g| g.to_vec())
                    .unwrap_or_default();
                std::iter::once(c).chain(below)
            })
            .filter_map(text_of)
            .collect();
        assert_eq!(
            shown
                .iter()
                .filter(|t| t.ends_with(", capped at 100"))
                .count(),
            2,
            "{shown:?}"
        );
        let width = laid_out_width(world, bar, &text_of);
        assert!(
            width <= crate::app::WINDOW_SIZE.0 as f32,
            "the top bar needs {width} px: {shown:?}"
        );
    }

    /// Greyed with the tech count at the start; greyed with the cost on
    /// its face when only the stockpile is short, as a card button is;
    /// enabled with the cost once the sim accepts `AdvanceAge`; and greyed
    /// again for the Charter Age after a click advanced. Each state is
    /// `check_advance_age`'s. A greyed click sends nothing and shows the
    /// reason; an enabled one sends `AdvanceAge`.
    #[test]
    fn advance_button_follows_check_advance_age() {
        let mut app = hud_app();
        let button = app
            .world_mut()
            .query_filtered::<Entity, With<AdvanceButton>>()
            .single(app.world())
            .unwrap();
        let want = check(&mut app);
        assert_eq!(shown(&mut app).0.as_str(), "Hearth Age", "the start's age");
        assert_eq!(
            (want.label.as_str(), want.state.enabled),
            ("Advance: 0 / 2 techs", false)
        );
        assert_eq!(
            want.state.tooltip,
            "Research more techs first. 0 / 2 techs researched. Costs 250 Grain 100 Lore"
        );
        click(&mut app, button);
        assert!(app.world().resource::<PendingCommands>().is_empty());
        assert_eq!(
            app.world().resource::<MessageState>().text,
            want.state.tooltip
        );

        // Two techs, but 150 Grain of 250.
        let cmds = vec![research(&app, "letters_1"), research(&app, "trade_1")];
        run(&mut app, cmds);
        let want = check(&mut app);
        assert_eq!(
            (want.label.as_str(), want.state),
            (
                "Advance to Masonry Age: 250 Grain 100 Lore",
                CardState {
                    enabled: false,
                    tooltip: "Not enough Grain. 2 / 2 techs researched. Costs 250 Grain 100 Lore"
                        .into(),
                }
            )
        );

        let cmds = vec![grant(&app, "grain", 100), grant(&app, "lore", 100)];
        run(&mut app, cmds);
        let want = check(&mut app);
        assert_eq!(
            (want.label.as_str(), want.state),
            (
                "Advance to Masonry Age: 250 Grain 100 Lore",
                CardState {
                    enabled: true,
                    tooltip: "Advance to Masonry Age: Costs 250 Grain 100 Lore".into(),
                }
            )
        );
        click(&mut app, button);
        let sent = app
            .world_mut()
            .resource_mut::<PendingCommands>()
            .drain_sorted();
        assert_eq!(
            sent.iter().map(|c| &c.cmd).collect::<Vec<_>>(),
            [&Command::AdvanceAge {
                player: LOCAL_PLAYER
            }]
        );
        let cmds: Vec<Command> = sent.into_iter().map(|c| c.cmd).collect();
        run(&mut app, cmds);
        let want = check(&mut app);
        assert_eq!(shown(&mut app).0.as_str(), "Masonry Age");
        assert_eq!(
            (want.label.as_str(), want.state.enabled),
            ("Advance: 2 / 5 techs", false)
        );
        assert_eq!(
            want.state.tooltip,
            "Research more techs first. 2 / 5 techs researched. \
             Costs 500 Grain 300 Lumber 300 Lore"
        );
    }

    /// The tech count on the button and in its tooltip is the techs that
    /// count toward an age, which the sim compares with the gate (M5a):
    /// Letters I with both Harrying techs is "1 / 2", not 3, and the
    /// button stays on the count until a second Scriptorium tech.
    #[test]
    fn advance_tooltip_counts_only_age_techs() {
        let mut app = hud_app();
        let cmds = ["letters_1", "harrying_1", "harrying_2"]
            .iter()
            .map(|id| research(&app, id))
            .collect();
        run(&mut app, cmds);
        let sim = app.world().non_send::<SimHandle>();
        let (researched, counting) = (
            sim.view().researched(LOCAL_PLAYER).len(),
            sim.view().age_tech_count(LOCAL_PLAYER),
        );
        assert_eq!((researched, counting), (3, 1));
        let want = check(&mut app);
        assert_eq!(
            (want.label.as_str(), want.state),
            (
                "Advance: 1 / 2 techs",
                CardState {
                    enabled: false,
                    tooltip: "Research more techs first. 1 / 2 techs researched. \
                              Costs 250 Grain 100 Lore"
                        .into(),
                }
            )
        );
        let cmds = vec![research(&app, "trade_1")];
        run(&mut app, cmds);
        let want = check(&mut app);
        assert_eq!(
            (want.label.as_str(), want.state.tooltip.as_str()),
            (
                "Advance to Masonry Age: 250 Grain 100 Lore",
                "Not enough Grain. 2 / 2 techs researched. Costs 250 Grain 100 Lore"
            )
        );
    }

    /// In the last age the readout names it and the button is hidden, as
    /// the sim answers `FinalAge`.
    #[test]
    fn the_button_is_hidden_in_the_final_age() {
        let mut app = hud_app();
        let mut cmds: Vec<Command> = [
            "letters_1",
            "trade_1",
            "arms_1",
            "statecraft_1",
            "letters_2",
        ]
        .iter()
        .map(|id| research(&app, id))
        .collect();
        cmds.extend([
            grant(&app, "grain", 750),
            grant(&app, "lumber", 300),
            grant(&app, "lore", 400),
            Command::AdvanceAge {
                player: LOCAL_PLAYER,
            },
            Command::AdvanceAge {
                player: LOCAL_PLAYER,
            },
        ]);
        run(&mut app, cmds);
        let want = check(&mut app);
        let (readout, _, _, laid_out) = shown(&mut app);
        assert_eq!(readout, "Charter Age");
        assert!(!want.visible && !laid_out, "{want:?}");
        assert_eq!(
            app.world()
                .non_send::<SimHandle>()
                .view()
                .check_advance_age(LOCAL_PLAYER),
            Err(RejectReason::FinalAge)
        );
    }
}
