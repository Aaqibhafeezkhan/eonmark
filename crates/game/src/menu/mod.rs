//! The app's states and the main menu (M5b; `docs/design/ui.md`, "Menus").
//!
//! [`AppState`] has two states in M5b: `Menu`, the minimal main menu a
//! plain launch opens on (M5b owner decision 5: a title, the opponent and
//! map line, the seed, New Game and Quit), and `Skirmish`, the match.
//! `app::run` inserts the initial state explicitly ([`initial_state`]):
//! `Menu` for a plain launch, `Skirmish` for `--scenario`, `--skip-menu`
//! and `--spectate`. `--headless-run` has no states at all. The pause menu,
//! the game-over screen with Play Again and the difficulty and seed
//! selectors are M6, full menus M7.
//!
//! The match itself is built at launch (`app::build_match`), before any
//! state: while the menu is open the sim does not step
//! (`sim_driver::match_running`), world input is off, and the replay
//! recorder has not started (`sim_driver::PendingRecording`), so quitting
//! from the menu writes no replay. New Game only switches the state; M6's
//! Play Again will rebuild the match on entering `Skirmish` instead.
//!
//! On entering `Skirmish` the game prints one line naming the match
//! ([`match_line`]): `match: new game vs standard personality=standard
//! seed=<s> map=plains_1v1` against the bot, `match: spectate ...` with
//! both slots bots.

pub mod main_menu;

use std::time::Duration;

use bevy::prelude::*;
use sim::MatchSetup;

use crate::cli::Cli;
use crate::hud::{RealCursor, restore_real_cursor};
use crate::sim_driver::{LOCAL_PLAYER, SimHandle};

/// The app's states (M5b): the main menu, then the match.
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    /// The main menu (a plain launch): the sim waits.
    #[default]
    Menu,
    /// The match runs.
    Skirmish,
}

/// The state a windowed launch starts in: [`AppState::Menu`] for a plain
/// launch, [`AppState::Skirmish`] when `--scenario`, `--skip-menu` or
/// `--spectate` skips the menu.
pub fn initial_state(cli: &Cli) -> AppState {
    if cli.starts_in_menu() {
        AppState::Menu
    } else {
        AppState::Skirmish
    }
}

/// The line printed on entering [`AppState::Skirmish`]: for a match of the
/// local (human) player against a bot, `match: new game vs <bot>
/// personality=<id> seed=<s> map=<map>`; for a match of bots only
/// (`--spectate`), `match: spectate <bot> personality=<id> vs <bot>
/// personality=<id> seed=<s> map=<map>`. `None` for a scenario (no bot).
pub fn match_line(setup: &MatchSetup) -> Option<String> {
    let seat = |s: &sim::AiSeat| format!("{} personality={}", s.bot, s.personality);
    let tail = format!("seed={} map={}", setup.seed, setup.map);
    let local_is_ai = setup
        .players
        .iter()
        .any(|p| p.id == LOCAL_PLAYER && p.is_ai);
    match (local_is_ai, setup.ai_seats.as_slice()) {
        (_, []) => None,
        (false, [bot, ..]) => Some(format!("match: new game vs {} {tail}", seat(bot))),
        (true, seats) => {
            let bots: Vec<String> = seats.iter().map(seat).collect();
            Some(format!("match: spectate {} {tail}", bots.join(" vs ")))
        }
    }
}

/// Print [`match_line`] once the match starts (`OnEnter(Skirmish)`).
fn announce_match(sim: NonSend<SimHandle>) {
    if let Some(line) = match_line(sim.setup()) {
        println!("{line}");
    }
}

/// The main menu and the `Skirmish` announcement. `app::run` adds it and
/// inserts the initial [`AppState`]; `--new-game-after-seconds` sets
/// `new_game_after`, the New Game click proxy (`main_menu::click_new_game_after`).
pub struct MenuPlugin {
    /// `--new-game-after-seconds`: click the real New Game button after
    /// this long.
    pub new_game_after: Option<Duration>,
}

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Menu), main_menu::spawn_main_menu)
            .add_systems(
                Update,
                main_menu::menu_button_colors.run_if(in_state(AppState::Menu)),
            )
            .add_systems(OnEnter(AppState::Skirmish), announce_match);
        if let Some(after) = self.new_game_after {
            app.insert_resource(main_menu::NewGameAfter::new(after))
                .init_resource::<RealCursor>()
                .add_systems(
                    Update,
                    main_menu::click_new_game_after.run_if(in_state(AppState::Menu)),
                )
                .add_systems(PostUpdate, restore_real_cursor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::main_menu::{MainMenu, MenuButton, MenuLine};
    use super::*;
    use crate::economy_hud::UiStrings;
    use crate::hud::WorldInputSet;
    use crate::sim_driver::{
        DriverStats, LocalInput, PendingRecording, ReplayRecorder, SimPlugin, SimStepped,
    };
    use bevy::app::AppExit;
    use bevy::camera::NormalizedRenderTarget;
    use bevy::picking::backend::HitData;
    use bevy::picking::events::{Click, Pointer};
    use bevy::picking::pointer::{Location, PointerButton, PointerId};
    use bevy::time::TimeUpdateStrategy;
    use rules::Rules;
    use std::path::{Path, PathBuf};

    fn data() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eonmark-menu-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[derive(Resource, Default)]
    struct WorldInputRuns(u32);

    /// A plain launch as `app::run` builds it, on `MinimalPlugins` with
    /// states and a manual clock: the match against the Standard bot on
    /// seed 1234, recording to `path`, the main menu open. After the first
    /// update (start-up, which spawns the menu) each update is 50 ms, one
    /// tick once the match runs. A counter system in
    /// `hud::WorldInputSet` shows whether world input runs.
    fn menu_app(path: &Path) -> App {
        let rules = Rules::load(data()).unwrap();
        let strings = rules::strings::Strings::load(data(), "en").unwrap();
        let handle = SimHandle::vs_bot(rules, 1234, ai::BOT_STANDARD, "standard");
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO))
            .insert_resource(UiStrings(strings))
            .init_resource::<WorldInputRuns>()
            .add_plugins(
                SimPlugin::new(handle, Box::new(LocalInput)).recording(Some(path.to_path_buf())),
            )
            .insert_state(AppState::Menu)
            .add_plugins(MenuPlugin {
                new_game_after: None,
            })
            .add_systems(
                Update,
                (|mut runs: ResMut<WorldInputRuns>| runs.0 += 1).in_set(WorldInputSet),
            );
        crate::hud::gate_world_input(&mut app);
        app.update();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            50,
        )));
        app
    }

    fn tick(app: &App) -> u32 {
        app.world().non_send::<SimHandle>().tick()
    }

    fn state(app: &App) -> AppState {
        *app.world().resource::<State<AppState>>().get()
    }

    fn menus(app: &mut App) -> usize {
        let mut q = app.world_mut().query_filtered::<(), With<MainMenu>>();
        q.iter(app.world()).count()
    }

    /// A primary `Pointer<Click>` on the menu's `button`, as the picking
    /// pipeline triggers it.
    fn click(app: &mut App, button: MenuButton) {
        let mut q = app.world_mut().query::<(Entity, &MenuButton)>();
        let entity = q
            .iter(app.world())
            .find(|(_, b)| **b == button)
            .map(|(e, _)| e)
            .expect("the button exists");
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
                hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
                duration: Duration::ZERO,
                count: 1,
            },
            entity,
        ));
    }

    /// While the main menu is open the sim does not step (one second of
    /// frames), world input is off, the recorder has not started, and a
    /// quit then leaves no replay at all (the lazy recorder, M5b). The menu
    /// shows the opponent, the map and the match's seed.
    #[test]
    fn the_sim_does_not_step_and_no_replay_is_written_while_the_menu_is_open() {
        let path = temp("menu_open.eonreplay");
        let _ = std::fs::remove_file(&path);
        let mut app = menu_app(&path);
        for _ in 0..20 {
            app.update();
        }
        assert_eq!(state(&app), AppState::Menu);
        assert_eq!(tick(&app), 0, "the sim waits for New Game");
        assert!(!app.world().resource::<SimStepped>().0);
        assert_eq!(app.world().resource::<DriverStats>().ticks_last_frame, 0);
        assert_eq!(app.world().resource::<WorldInputRuns>().0, 0, "no input");
        assert_eq!(menus(&mut app), 1);
        let mut q = app.world_mut().query::<(&MenuLine, &Text)>();
        let mut lines: Vec<(usize, String)> = q
            .iter(app.world())
            .map(|(l, t)| (l.0, t.0.clone()))
            .collect();
        lines.sort();
        let lines: Vec<String> = lines.into_iter().map(|(_, t)| t).collect();
        assert_eq!(
            lines,
            [
                "Eonmark",
                "Skirmish against the Standard bot on plains_1v1",
                "Seed 1234"
            ]
        );
        assert!(app.world().resource::<ReplayRecorder>().0.is_none());
        assert_eq!(
            app.world().resource::<PendingRecording>().0.as_deref(),
            Some(path.as_path())
        );
        assert!(!path.exists(), "no replay while the menu is open");
        app.world_mut().write_message(AppExit::Success);
        app.update();
        assert_eq!(tick(&app), 0);
        assert!(!path.exists(), "a quit from the menu writes no replay");
    }

    /// New Game (a picking click on the button entity) enters `Skirmish`:
    /// the menu goes, world input comes on, the sim steps from that frame,
    /// one tick for its 50 ms and not a burst for the second spent on the
    /// menu, and the recorder starts at the first stepped tick. The replay
    /// it writes verifies with the Standard bot rebuilt from its header.
    #[test]
    fn new_game_click_enters_skirmish_and_the_sim_starts_stepping() {
        let path = temp("new_game.eonreplay");
        let _ = std::fs::remove_file(&path);
        let mut app = menu_app(&path);
        for _ in 0..20 {
            app.update();
        }
        assert_eq!(tick(&app), 0);
        click(&mut app, MenuButton::NewGame);
        app.update();
        assert_eq!(state(&app), AppState::Skirmish);
        assert_eq!(menus(&mut app), 0, "DespawnOnExit removed the menu");
        assert_eq!(tick(&app), 1, "one tick for one frame, no burst");
        assert!(app.world().resource::<ReplayRecorder>().0.is_some());
        assert!(app.world().resource::<PendingRecording>().0.is_none());
        assert!(path.exists(), "the recorder started with the first tick");
        let runs = app.world().resource::<WorldInputRuns>().0;
        assert!(runs > 0, "world input runs in the match");
        for _ in 0..40 {
            app.update();
        }
        assert_eq!(tick(&app), 41);
        app.world_mut().write_message(AppExit::Success);
        app.update();
        let (ticks, hash) = {
            let sim = app.world().non_send::<SimHandle>();
            (sim.tick(), sim.hash())
        };
        let header_bots = Box::new(|setup: &MatchSetup, rules: &Rules| {
            ai::controller_for(setup, rules).map_err(|e| e.to_string())
        });
        assert_eq!(
            sim::replay::verify_with(&path, &data(), header_bots).unwrap(),
            sim::replay::VerifyOutcome::Ok {
                final_hash: hash,
                ticks
            }
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Quit writes `AppExit::Success` and, with nothing stepped, no replay.
    #[test]
    fn quit_writes_app_exit() {
        let path = temp("quit.eonreplay");
        let _ = std::fs::remove_file(&path);
        let mut app = menu_app(&path);
        app.update();
        assert_eq!(app.should_exit(), None);
        click(&mut app, MenuButton::Quit);
        assert_eq!(app.should_exit(), Some(AppExit::Success));
        app.update();
        assert_eq!(state(&app), AppState::Menu);
        assert_eq!(tick(&app), 0);
        assert!(!path.exists());
    }

    /// The line printed on entering `Skirmish` names the bot, its
    /// personality, the seed and the map; a spectated match names both
    /// bots; a scenario prints nothing.
    #[test]
    fn match_lines_name_the_bots_the_seed_and_the_map() {
        let rules = Rules::load(data()).unwrap();
        assert_eq!(
            match_line(&MatchSetup::vs_bot(&rules, 7, "standard", "standard")).as_deref(),
            Some("match: new game vs standard personality=standard seed=7 map=plains_1v1")
        );
        assert_eq!(
            match_line(&MatchSetup::bots(
                &rules,
                1,
                ("standard", "standard"),
                ("standard", "standard")
            ))
            .as_deref(),
            Some(
                "match: spectate standard personality=standard vs standard personality=standard seed=1 map=plains_1v1"
            )
        );
        assert_eq!(match_line(&MatchSetup::scenario(&rules, 1)), None);
    }
}
