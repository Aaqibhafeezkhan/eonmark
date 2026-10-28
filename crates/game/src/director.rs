//! The director of `--scenario enemy_outpost_walkthrough` (M4b): a table
//! of presentation beats that move the camera and the selection when the
//! sim reaches their ticks, so a windowed run of the scripted line-1
//! sequence shows each step without a human at the controls (the research
//! card while Letters I runs, the Muster Hall's queue, the Advance button,
//! the Engine Yard's Mangonels, the siege of the Watchtower). Screenshots
//! taken with `--screenshot-at` at the ticks [`walkthrough_beats`] logs on
//! start-up are the owner proxy pictures in `docs/PLAYTEST.md`.
//!
//! Presentation only: the director writes `RtsCamera` and `Selection` and
//! never pushes a command, so the session's replay is the scenario's stream
//! and verifies like any other (`walkthrough_beats_change_only_the_view`).
//!
//! The art pass gives `--scenario art_bench` a one-beat director
//! ([`art_bench_beats`]): at [`ART_BENCH_SELECT_TICK`] it selects every unit
//! of the local player, so the draw-call counter measures 200 selection
//! rings in the fight.
//! Every beat tick derives from the scenario's own rules-derived beats
//! ([`sim::scenarios::enemy_outpost_beats`]) and pub constants, so a data
//! change that moves the walkthrough moves the director with it.

use bevy::prelude::*;
use rules::Rules;
use sim::scenarios::{
    ENEMY_OUTPOST_T_CHARGE, ENEMY_OUTPOST_TOWER_ID, enemy_outpost_beats, enemy_outpost_trained_ids,
};
use sim::{BuildingId, UnitId};

use crate::camera::RtsCamera;
use crate::cli::CameraStart;
use crate::ground::HALF_EXTENT;
use crate::selection::Selection;
use crate::sim_driver::{LOCAL_PLAYER, SimHandle};

/// The walkthrough's Scriptorium: P0 places it first, during the build
/// beat, after P1's tick-0 Watchtower (`BuildingId(4)` today).
pub const SCRIPTORIUM: BuildingId = BuildingId(ENEMY_OUTPOST_TOWER_ID.0 + 1);
/// The walkthrough's Muster Hall, placed right after the Scriptorium.
pub const MUSTER_HALL: BuildingId = BuildingId(ENEMY_OUTPOST_TOWER_ID.0 + 2);
/// The walkthrough's Engine Yard, placed during the age beat.
pub const ENGINE_YARD: BuildingId = BuildingId(ENEMY_OUTPOST_TOWER_ID.0 + 3);

/// Ticks the director looks ahead of a command beat, so the card shows the
/// state the command is issued from (Letters I enabled, Trade I enabled).
pub const LEAD_TICKS: u32 = 20;
/// Ticks the director waits after a command beat, so the building's queue
/// already holds what the command added.
pub const SETTLE_TICKS: u32 = 10;

/// Where the camera frames the prologue up close, at the bars' readable
/// distance (the default framing shows all 400 units at 55 m).
pub const BATTLE_CLOSE: CameraStart = CameraStart {
    x: 98.0,
    y: 32.0,
    zoom: 30.0,
};
/// P0's base at 40 m: the Seat, the Scriptorium, the Muster Hall and the
/// Engine Yard's site (the camera of the M4b S4 lavapipe checks).
pub const HOME: CameraStart = CameraStart {
    x: 30.0,
    y: 63.0,
    zoom: 40.0,
};
/// The staging point and the Watchtower across from it, at 35 m.
pub const SIEGE: CameraStart = CameraStart {
    x: 52.0,
    y: 63.0,
    zoom: 35.0,
};

/// What a beat does.
#[derive(Debug, Clone, PartialEq)]
pub enum BeatAction {
    /// Move the camera to `(x, y)` at `zoom` metres, with no smoothing.
    Focus(CameraStart),
    /// Select one building alone (its card and panel show).
    SelectBuilding(BuildingId),
    /// Select these units (their rings and bars show).
    SelectUnits(Vec<UnitId>),
    /// Select every unit the local player has at that tick.
    SelectAllLocal,
    /// Deselect everything.
    Clear,
}

/// One beat: from `tick` on, apply `actions` in order, once.
#[derive(Debug, Clone, PartialEq)]
pub struct Beat {
    /// Applied in the first frame whose sim tick is at least this.
    pub tick: u32,
    /// Short name for the start-up log and the screenshot table.
    pub name: &'static str,
    /// What the beat does, in order.
    pub actions: Vec<BeatAction>,
}

/// The walkthrough's beats, in tick order, from the rules:
///
/// | Beat | Tick | Does |
/// |---|---|---|
/// | `battle_close` | charge + 200 | frames the fight at 30 m |
/// | `research_card` | `research_1` - [`LEAD_TICKS`] | frames P0's base, selects the Scriptorium |
/// | `muster_queue` | `muster` + [`SETTLE_TICKS`] | selects the Muster Hall and its queue of five |
/// | `second_tech` | `research_2` - [`LEAD_TICKS`] | selects the Scriptorium again |
/// | `mangonel_queue` | `yard` + [`SETTLE_TICKS`] | selects the Engine Yard and its two Mangonels |
/// | `siege` | `siege` - [`LEAD_TICKS`] | frames the staging point and the tower, selects the Mangonels |
///
/// The Scriptorium stays selected from `second_tech` through the age
/// beat, so the Advance button and the "Researched:" line are on screen
/// together.
pub fn walkthrough_beats(rules: &Rules) -> Vec<Beat> {
    let b = enemy_outpost_beats(rules);
    let mangonels: Vec<UnitId> = enemy_outpost_trained_ids(rules)
        .into_iter()
        .rev()
        .take(sim::scenarios::ENEMY_OUTPOST_SIEGE_UNITS.len())
        .rev()
        .collect();
    let mut beats = vec![
        Beat {
            tick: ENEMY_OUTPOST_T_CHARGE + 200,
            name: "battle_close",
            actions: vec![BeatAction::Focus(BATTLE_CLOSE)],
        },
        Beat {
            tick: b.research_1 - LEAD_TICKS,
            name: "research_card",
            actions: vec![
                BeatAction::Focus(HOME),
                BeatAction::SelectBuilding(SCRIPTORIUM),
            ],
        },
        Beat {
            tick: b.muster + SETTLE_TICKS,
            name: "muster_queue",
            actions: vec![BeatAction::SelectBuilding(MUSTER_HALL)],
        },
        Beat {
            tick: b.research_2 - LEAD_TICKS,
            name: "second_tech",
            actions: vec![BeatAction::SelectBuilding(SCRIPTORIUM)],
        },
        Beat {
            tick: b.yard + SETTLE_TICKS,
            name: "mangonel_queue",
            actions: vec![BeatAction::SelectBuilding(ENGINE_YARD)],
        },
        Beat {
            tick: b.siege - LEAD_TICKS,
            name: "siege",
            actions: vec![BeatAction::Focus(SIEGE), BeatAction::SelectUnits(mangonels)],
        },
    ];
    beats.sort_by_key(|beat| beat.tick);
    beats
}

/// Tick at which `art_bench`'s director selects P0's whole army: after the
/// charge (tick 20, applied at 22), with both armies still whole.
pub const ART_BENCH_SELECT_TICK: u32 = 40;

/// `art_bench`'s one beat: select every P0 unit at
/// [`ART_BENCH_SELECT_TICK`], so the 200 selection rings are part of the
/// draw count (art pass, `docs/design/ui.md`).
pub fn art_bench_beats() -> Vec<Beat> {
    vec![Beat {
        tick: ART_BENCH_SELECT_TICK,
        name: "select_all",
        actions: vec![BeatAction::SelectAllLocal],
    }]
}

/// The beat table and how far it has run.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct Director {
    /// Every beat, in tick order.
    pub beats: Vec<Beat>,
    /// Index of the next beat to apply.
    pub next: usize,
}

impl Director {
    /// A director over `beats`, sorted by tick.
    pub fn new(mut beats: Vec<Beat>) -> Self {
        beats.sort_by_key(|beat| beat.tick);
        Self { beats, next: 0 }
    }

    /// The beats that are due at `tick` and not yet applied, marked
    /// applied.
    pub fn take_due(&mut self, tick: u32) -> &[Beat] {
        let start = self.next;
        while self.beats.get(self.next).is_some_and(|b| b.tick <= tick) {
            self.next += 1;
        }
        &self.beats[start..self.next]
    }
}

/// Runs the director: added by `app::run` for
/// `--scenario enemy_outpost_walkthrough` ([`walkthrough_beats`]) and
/// `--scenario art_bench` ([`art_bench_beats`]).
pub struct DirectorPlugin {
    /// The beats to run; `app::run` passes [`walkthrough_beats`].
    pub beats: Vec<Beat>,
}

impl Plugin for DirectorPlugin {
    fn build(&self, app: &mut App) {
        for beat in &self.beats {
            info!("director: beat {} at tick {}", beat.name, beat.tick);
        }
        app.insert_resource(Director::new(self.beats.clone()))
            .add_systems(Update, run_director);
    }
}

/// Apply every beat whose tick the sim has reached.
fn run_director(
    sim: NonSend<SimHandle>,
    mut director: ResMut<Director>,
    mut selection: ResMut<Selection>,
    mut cameras: Query<&mut RtsCamera>,
) {
    let tick = sim.tick();
    for beat in director.take_due(tick) {
        info!("director: {} at tick {tick}", beat.name);
        for action in &beat.actions {
            match action {
                BeatAction::Focus(at) => {
                    for mut cam in &mut cameras {
                        cam.focus = Vec3::new(at.x - HALF_EXTENT, 0.0, at.y - HALF_EXTENT);
                        cam.zoom = at.zoom;
                        cam.zoom_target = at.zoom;
                    }
                }
                BeatAction::SelectBuilding(id) => selection.set_building(*id),
                BeatAction::SelectUnits(ids) => selection.set(ids.iter().copied()),
                BeatAction::SelectAllLocal => {
                    let view = sim.view();
                    selection.set(
                        view.units()
                            .filter(|u| u.owner == LOCAL_PLAYER)
                            .map(|u| u.id),
                    );
                }
                BeatAction::Clear => selection.clear(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_driver::{PendingCommands, ScenarioInput, SimPlugin};
    use sim::scenarios::{self, OutpostBeats};
    use std::time::Duration;

    fn rules() -> Rules {
        Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
            .expect("rules load")
    }

    /// Every beat falls between the scenario's own beats so that what it
    /// shows exists: the Scriptorium's card before the first research is
    /// issued (and after the building is complete, which is at least the
    /// walk margin before `research_1`), the Muster Hall's queue after the
    /// Train commands applied, the Engine Yard after it was placed, the
    /// siege before the Attack; and the beats run in that order.
    #[test]
    fn walkthrough_beats_follow_the_scenarios_own_beats() {
        let rules = rules();
        let b: OutpostBeats = enemy_outpost_beats(&rules);
        let beats = walkthrough_beats(&rules);
        let names: Vec<&str> = beats.iter().map(|beat| beat.name).collect();
        assert_eq!(
            names,
            [
                "battle_close",
                "research_card",
                "muster_queue",
                "second_tech",
                "mangonel_queue",
                "siege"
            ]
        );
        assert!(beats.windows(2).all(|w| w[0].tick < w[1].tick));
        let tick = |name: &str| beats.iter().find(|beat| beat.name == name).unwrap().tick;
        let delay = rules.cmd_delay_ticks;
        // The battle close-up comes after the charge, before the base.
        assert!(tick("battle_close") > ENEMY_OUTPOST_T_CHARGE);
        assert!(tick("battle_close") < scenarios::ENEMY_OUTPOST_CLEAR_TICK);
        // The research card opens after the Scriptorium can be complete
        // (its build time from the build beat) and before Letters I.
        let scriptorium = rules
            .building_kind_by_id("scriptorium")
            .map(|(_, k)| rules.build_time_ticks(k))
            .unwrap();
        assert!(tick("research_card") >= b.build + delay + scriptorium);
        assert!(tick("research_card") < b.research_1);
        assert!(tick("muster_queue") > b.muster + delay);
        assert!(tick("second_tech") < b.research_2);
        assert!(tick("second_tech") > b.research_1 + delay);
        assert!(tick("mangonel_queue") > b.yard + delay);
        assert!(tick("siege") < b.siege);
        assert!(tick("siege") > b.yard);
        // The Mangonels the siege selects are the Engine Yard's two.
        let trained = enemy_outpost_trained_ids(&rules);
        let siege = beats.iter().find(|beat| beat.name == "siege").unwrap();
        assert!(siege.actions.contains(&BeatAction::SelectUnits(
            trained[trained.len() - 2..].to_vec()
        )));
        assert_eq!(
            (SCRIPTORIUM, MUSTER_HALL, ENGINE_YARD),
            (BuildingId(4), BuildingId(5), BuildingId(6))
        );
    }

    /// `art_bench`: at tick 40 the director selects all 200 of P0's units
    /// (none of P1's) and pushes no command; nothing is selected before.
    #[test]
    fn art_bench_director_selects_every_p0_unit_at_tick_40() {
        let rules = rules();
        let beats = art_bench_beats();
        assert_eq!(beats.len(), 1);
        assert_eq!(beats[0].tick, ART_BENCH_SELECT_TICK);
        // The charge is under way by then, and both armies are whole.
        assert!(ART_BENCH_SELECT_TICK > scenarios::ART_BENCH_CHARGE_TICK + rules.cmd_delay_ticks);
        let (setup, stream, _) = scenarios::by_name("art_bench", &rules).unwrap();
        let handle = SimHandle::from_setup(setup, rules);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                Duration::ZERO,
            ))
            .init_resource::<Selection>()
            .add_plugins((
                SimPlugin::new(handle, Box::new(ScenarioInput::new(stream))),
                DirectorPlugin { beats },
            ));
        app.world_mut().spawn(RtsCamera::default());
        app.update();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_millis(50),
        ));
        let tick = |app: &App| app.world().non_send::<SimHandle>().tick();
        while tick(&app) < ART_BENCH_SELECT_TICK {
            assert!(app.world().resource::<Selection>().is_empty(), "too early");
            app.update();
        }
        app.update();
        let selection = app.world().resource::<Selection>();
        assert_eq!(selection.units.len(), 200);
        let sim = app.world().non_send::<SimHandle>();
        let view = sim.view();
        assert!(
            selection
                .units
                .iter()
                .all(|id| view.unit(*id).is_some_and(|u| u.owner == LOCAL_PLAYER))
        );
        assert!(app.world().resource::<PendingCommands>().is_empty());
    }

    #[test]
    fn take_due_applies_each_beat_once_in_tick_order() {
        let beat = |tick, name| Beat {
            tick,
            name,
            actions: vec![BeatAction::Clear],
        };
        let mut d = Director::new(vec![beat(30, "c"), beat(10, "a"), beat(10, "b")]);
        assert!(d.take_due(9).is_empty());
        let due: Vec<&str> = d.take_due(10).iter().map(|b| b.name).collect();
        assert_eq!(due, ["a", "b"]);
        assert!(d.take_due(10).is_empty(), "never twice");
        // A frame that jumps past a beat still applies it.
        let due: Vec<&str> = d.take_due(57).iter().map(|b| b.name).collect();
        assert_eq!(due, ["c"]);
        assert!(d.take_due(u32::MAX).is_empty());
    }

    /// Through the real systems on a small match: the camera and the
    /// selection follow the beats at their ticks, and nothing is ever
    /// pushed to `PendingCommands` (the replay is the scenario's stream).
    #[test]
    fn walkthrough_beats_change_only_the_view() {
        let rules = rules();
        let setup = sim::MatchSetup::scenario(&rules, 3);
        let handle = SimHandle::from_setup(setup, rules);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                Duration::ZERO,
            ))
            .init_resource::<Selection>()
            .add_plugins((
                SimPlugin::new(handle, Box::new(ScenarioInput::new(Vec::new()))),
                DirectorPlugin {
                    beats: vec![
                        Beat {
                            tick: 4,
                            name: "focus",
                            actions: vec![
                                BeatAction::Focus(HOME),
                                BeatAction::SelectUnits(vec![UnitId(2), UnitId(1)]),
                            ],
                        },
                        Beat {
                            tick: 8,
                            name: "building",
                            actions: vec![BeatAction::SelectBuilding(SCRIPTORIUM)],
                        },
                        Beat {
                            tick: 12,
                            name: "clear",
                            actions: vec![BeatAction::Focus(SIEGE), BeatAction::Clear],
                        },
                    ],
                },
            ));
        app.world_mut().spawn(RtsCamera::default());
        app.update();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_millis(50),
        ));
        let focus = |at: CameraStart| Vec3::new(at.x - HALF_EXTENT, 0.0, at.y - HALF_EXTENT);
        let mut seen = Vec::new();
        for _ in 0..16 {
            app.update();
            let tick = app.world().non_send::<SimHandle>().tick();
            let cam = app
                .world_mut()
                .query::<&RtsCamera>()
                .single(app.world())
                .unwrap()
                .clone();
            let sel = app.world().resource::<Selection>();
            seen.push((tick, cam.focus, cam.zoom, sel.units.len(), sel.building));
            assert!(
                app.world().resource::<PendingCommands>().is_empty(),
                "the director pushed a command at tick {tick}"
            );
        }
        let at = |t: u32| *seen.iter().find(|s| s.0 == t).expect("tick seen");
        let start = RtsCamera::default();
        assert_eq!(at(3), (3, start.focus, start.zoom, 0, None));
        assert_eq!(at(4), (4, focus(HOME), HOME.zoom, 2, None));
        assert_eq!(at(7), (7, focus(HOME), HOME.zoom, 2, None));
        assert_eq!(at(8), (8, focus(HOME), HOME.zoom, 0, Some(SCRIPTORIUM)));
        assert_eq!(at(12), (12, focus(SIEGE), SIEGE.zoom, 0, None));
        assert_eq!(app.world().resource::<Director>().next, 3);
    }
}
