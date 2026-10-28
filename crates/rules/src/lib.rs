//! Rules: the schema, loader and validator for everything under `data/`.
//!
//! This crate is engine-free and deterministic. Every number is authored in
//! RON as an integer (deciseconds for time, tiles for distance) and converted
//! to ticks exactly once here, by [`Rules::ticks_from_ds`]. Floats never
//! appear in this crate; `clippy.toml` bans them.
//!
//! Layout of the data directory (see `data/README.md`):
//!
//! | File | Schema |
//! |------|--------|
//! | `rules/rules.ron` | [`Rules`] (the fields that are not `skip_deserializing`) |
//! | `rules/resources.ron` | [`Resources`] |
//! | `rules/units.ron` | [`Units`] (unit kinds and the counter table) |
//! | `rules/buildings.ron` | [`Buildings`] |
//! | `rules/techs.ron` | [`Techs`] |
//! | `rules/ages.ron` | [`Ages`] |
//! | `rules/factions.ron` | [`Factions`] |
//! | `maps/*.ron` | [`map::MapDef`] |
//! | `visuals.ron` | [`Visuals`] (how each unit and building kind is drawn; validated here, read by the game, not in [`Rules::rules_hash`]: ADR 0003) |
//! | `ai/personalities.ron`, `ai/build_orders/*.ron` | [`ai::AiRules`] (the bot's knobs and build orders, M5b; validated here, read by `crates/ai`) |
//!
//! Every error names the file and, when the data parsed, the field.
//!
//! The pure cost functions (building, Yeoman and military ramps) are
//! methods on [`Rules`] in the `costs` module, so the sim, the HUD and the
//! bot quote the same numbers. Tech and faction effects are [`Modifier`]
//! rows, resolved by [`modifiers::resolve`].
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod ages;
pub mod ai;
pub mod attack;
pub mod buildings;
pub mod costs;
pub mod factions;
pub mod map;
pub mod modifiers;
pub mod resources;
pub mod strings;
pub mod techs;
pub mod units;
pub mod visuals;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub use ages::{Age, Ages, Unlocks};
pub use ai::{AiRules, BuildOrder, MixEntry, Personality, Placement, RecallWave, Step, WaveSize};
pub use attack::{Attack, Prefer};
pub use buildings::{BuildingKind, Buildings, GatherSpec, Slots};
pub use factions::{Faction, Factions};
pub use map::{MapDef, Symmetry, Terrain, TilePos};
pub use modifiers::{ModOp, Modifier, StatPath, UnitSel};
pub use resources::{Resource, Resources};
pub use techs::{Tech, Techs};
pub use units::{Counter, Defender, UnitClass, UnitKind, UnitRamp, Units};
pub use visuals::{
    DecorAt, DecorLook, Part, Primitive, Rubble, TeamSwatch, TerrainDecor, Visual, Visuals,
};

/// The time unit every income rate is authored in: per 30 s, in
/// deciseconds. Not a tuning knob; [`Rules::income_period_ticks`] converts
/// it (600 ticks at 20 Hz).
pub const RATE_PERIOD_DS: u32 = 300;

/// Largest `_ds` value for the idle-seek fields (one hour).
pub const MAX_IDLE_SEEK_DS: u32 = 36_000;

/// Largest border radius a Town can reach: its grown radius plus every
/// `TownBorderRadius` row (the Statecraft techs and the faction's) must
/// stay within it in the worst case.
pub const MAX_BORDER_RADIUS_TILES: u32 = 64;

/// Largest `damage_variance_permille` (+/- 50%).
pub const MAX_DAMAGE_VARIANCE_PERMILLE: u32 = 500;

/// Largest `acquire_range_tiles` and `chase_regoal_tiles`.
pub const MAX_COMBAT_TILES: u32 = 16;

/// Largest starting stockpile entry.
pub const MAX_START_STOCK: u32 = 1_000_000;

/// Largest `supply_radius_tiles`, so the sim's squared radius stays far
/// inside `i32` with overflow checks on.
pub const MAX_SUPPLY_RADIUS_TILES: u32 = 64;

/// Largest Harrying level the `HarryingLevel` rows may reach in the worst
/// case (M5a). The sim clamps the resolved level to `0..=8` and halves
/// the attrition interval once per level above 1.
pub const MAX_HARRYING_LEVEL: u32 = 8;

/// Largest entry of `attrition_age_gap_hp` and largest `harrying_bonus_hp`,
/// in hit points per pulse.
pub const MAX_ATTRITION_HP: u32 = 1000;

/// Largest `annexation_adjacent_tiles`.
pub const MAX_ANNEXATION_ADJACENT_TILES: u32 = 4;

/// Errors produced while loading or validating rules data.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file could not be read.
    #[error("{path}: {source}")]
    Io {
        /// File that failed.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// A file did not parse against the schema.
    #[error("{path}: {message}")]
    Parse {
        /// File that failed.
        path: PathBuf,
        /// Parser message, including the offending field when known.
        message: String,
    },
    /// Data parsed but violated a cross-reference or range rule.
    #[error("{path}: {field}: {message}")]
    Invalid {
        /// File that failed.
        path: PathBuf,
        /// Field that failed.
        field: String,
        /// Human-readable reason.
        message: String,
    },
}

impl Error {
    fn invalid(path: &Path, field: &str, message: impl Into<String>) -> Self {
        Error::Invalid {
            path: path.to_path_buf(),
            field: field.to_string(),
            message: message.into(),
        }
    }
}

/// Read `path` and parse it as RON into `T`, naming the file in any error.
pub(crate) fn load_ron<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, Error> {
    let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse_ron(path, &text)
}

/// Parse `text` as RON into `T`, naming `path` in any error.
pub(crate) fn parse_ron<T: for<'de> Deserialize<'de>>(path: &Path, text: &str) -> Result<T, Error> {
    ron::from_str(text).map_err(|e| Error::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
}

/// What each player has at match start under the standard start (the sim's
/// `StartKind::Standard`): the Seat (one Town of the `town: true` kind,
/// centred on the map start) plus these units and stockpiles.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandardStart {
    /// Units per unit kind id, spawned in `UnitKindId` order.
    pub units: BTreeMap<String, u32>,
    /// Starting stockpile per resource id; lists every resource, zeros
    /// included.
    pub stockpile: BTreeMap<String, u32>,
}

/// Top-level match rules from `data/rules/rules.ron`, plus the other data
/// files that `Rules::load` attaches after parsing.
///
/// All time fields end in `_ds` and are deciseconds (tenths of a second);
/// all distance fields end in `_tiles`. Convert with [`Rules::ticks_from_ds`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    /// Human-facing label bumped whenever golden replays are regenerated.
    pub rules_version: u32,
    /// Simulation ticks per second.
    pub tick_rate_hz: u32,
    /// Commands issued during tick N apply at tick N + `cmd_delay_ticks`.
    pub cmd_delay_ticks: u32,
    /// Map loaded by `MatchSetup::skirmish`; must exist under `data/maps/`.
    pub default_map: String,
    /// Per-resource income ceiling per 30 s before any `YieldCap` row (the
    /// Trade techs set it to 100, 150 and 200). Lore is exempt (see
    /// `resources.ron`).
    pub yield_cap_base: u32,
    /// Population cap before any `PopCap` row (each Arms level adds 25).
    pub pop_cap_base: u32,
    /// Border radius a Town projects when founded.
    pub town_radius_tiles: u32,
    /// Border radius once a Town hosts `town_growth_distinct_buildings` kinds.
    pub town_radius_grown_tiles: u32,
    /// Distinct building kinds inside a Town's radius needed for growth.
    pub town_growth_distinct_buildings: u32,
    /// The age (an `ages.ron` id) a Town's owner must have reached before
    /// the Town grows to tier 2; `None` for no age gate. Read by the sim
    /// from the M4a tech slice.
    pub town_growth_age: Option<String>,
    /// Towns a player may own (sites included) before any `TownLimit` row
    /// (each Statecraft level adds 1); the Seat counts.
    pub town_limit_base: u32,
    /// Building ramp: the `n`-th copy of a kind (0-based, sites and the Seat
    /// included) costs `base * (1000 + step * n) / 1000`.
    pub building_ramp_step_permille: u32,
    /// Military ramp step: `mult = 1000 + step * T(k - 1)` permille, with
    /// `k = 1 + entries queued at the player's buildings of the trainer
    /// kind` and the triangular number `T`.
    pub military_ramp_step_permille: u32,
    /// Military ramp ceiling in permille (`2250` is 2.25x base).
    pub military_ramp_cap_permille: u32,
    /// Most entries a building's training queue holds.
    pub train_queue_max: u32,
    /// An idle worker seeks work on its own after this long, in deciseconds.
    pub idle_seek_delay_ds: u32,
    /// An idle worker that found no free slot retries after this long, in
    /// deciseconds.
    pub idle_seek_retry_ds: u32,
    /// What each player starts with under the standard start.
    pub standard_start: StandardStart,
    /// Random spread of every hit in permille, `0..=500`: a hit is scaled
    /// by a draw in `1000 - v ..= 1000 + v`. 0 draws nothing.
    pub damage_variance_permille: u32,
    /// Units acquire enemies within this many tiles, or within their weapon
    /// range when that is longer.
    pub acquire_range_tiles: u32,
    /// An explicit chase re-goals its path once the target's tile is more
    /// than this many tiles (Chebyshev) from the path's goal.
    pub chase_regoal_tiles: u32,
    /// A Supply Wain cancels attrition within this radius.
    pub supply_radius_tiles: u32,
    /// Interval between attrition ticks at Harrying I, in deciseconds.
    pub attrition_interval_ds: u32,
    /// Time a Town at 0 HP must stay uncontested before it flips, in deciseconds.
    pub annexation_ds: u32,
    /// Hit points one attrition pulse takes, indexed by the age gap
    /// `max(0, territory owner's age - intruder's age)`; the last entry
    /// covers every larger gap. Non-empty, at least 1 and non-decreasing.
    pub attrition_age_gap_hp: Vec<u32>,
    /// The late-game pressure knob: once the territory owner has reached
    /// this age (an `ages.ron` id), each pulse takes `harrying_bonus_hp`
    /// more; `None` for no bonus.
    pub harrying_bonus_age: Option<String>,
    /// Extra hit points per pulse from `harrying_bonus_age` on (0 until the
    /// M6 fun gate tunes it).
    pub harrying_bonus_hp: u32,
    /// An attacker counts toward annexing a Town within this many tiles
    /// (Chebyshev, tile to footprint) of its footprint.
    pub annexation_adjacent_tiles: u32,
    /// A defender within this many tiles of a Town's centre (centre to
    /// centre, inclusive) blocks or resets its annexation.
    pub annexation_defender_radius_tiles: u32,
    /// The unit kinds whose units annex an enemy Town (infantry and
    /// cavalry); every selector must reach a kind and none a civilian.
    pub annexation_attackers: Vec<UnitSel>,
    /// The unit kinds whose units defend their own Town (every
    /// non-civilian kind).
    pub annexation_defenders: Vec<UnitSel>,
    /// A flipped Town's hit points, in permille of its kind's `hp`.
    pub annexation_town_hp_permille: u32,
    /// The match ends at this time with the territory tiebreak (45 min), in
    /// deciseconds.
    pub match_cap_ds: u32,
    /// The late-game Yield Cap decay starts at this match time, in
    /// deciseconds.
    pub yield_cap_decay_after_ds: u32,
    /// The decay lowers the Yield Cap by `yield_cap_decay_step` once per
    /// this many deciseconds from `yield_cap_decay_after_ds` on.
    pub yield_cap_decay_every_ds: u32,
    /// How much each decay step lowers the Yield Cap; 0 turns the knob off.
    pub yield_cap_decay_step: u32,
    /// A* node expansions shared by all path requests per tick.
    pub path_budget_expansions: u32,
    /// Entries kept in the bounded path cache.
    pub path_cache_entries: u32,
    /// Deciseconds between periodic repaths while a unit is moving
    /// (`30` = 60 ticks at 20 Hz). Convert with [`Rules::repath_interval_ticks`].
    pub repath_interval_ds: u32,
    /// The separation push on a moving unit is capped at `speed / this`, so
    /// the steering direction keeps at least that share of the weight.
    pub separation_push_moving_div: u32,
    /// The separation push on a unit without an order is capped at
    /// `speed / this`: parked units yield slowly to a passing crowd.
    pub separation_push_idle_div: u32,
    /// Contents of `rules/resources.ron`. Attached by [`Rules::load`].
    #[serde(skip_deserializing)]
    pub resources: Resources,
    /// Unit kinds from `rules/units.ron` in `UnitKindId` order. Attached by
    /// [`Rules::load`].
    #[serde(skip_deserializing)]
    pub units: Vec<UnitKind>,
    /// The counter table from `rules/units.ron`. Attached by
    /// [`Rules::load`]; read it through [`Rules::counter_permille`].
    #[serde(skip_deserializing)]
    pub counters: Vec<Counter>,
    /// Building kinds from `rules/buildings.ron` in `BuildingKindId` order.
    /// Attached by [`Rules::load`].
    #[serde(skip_deserializing)]
    pub buildings: Vec<BuildingKind>,
    /// Techs from `rules/techs.ron` in `TechId` order. Attached by
    /// [`Rules::load`].
    #[serde(skip_deserializing)]
    pub techs: Vec<Tech>,
    /// Ages from `rules/ages.ron` in order. Attached by [`Rules::load`].
    #[serde(skip_deserializing)]
    pub ages: Vec<Age>,
    /// Factions from `rules/factions.ron`; every player uses `factions[0]`
    /// until a match setup chooses one (M9). Attached by [`Rules::load`].
    #[serde(skip_deserializing)]
    pub factions: Vec<Faction>,
    /// Every map under `maps/`, keyed by name. Attached by [`Rules::load`].
    #[serde(skip_deserializing)]
    pub maps: BTreeMap<String, MapDef>,
    /// Contents of `visuals.ron`: a [`Visual`] per unit and building kind id
    /// and the art pass's tables. Attached by [`Rules::load`] and validated
    /// against `units` and `buildings`; read by the game crate's
    /// presenter, never by the sim. `#[serde(skip)]`, not only
    /// `skip_deserializing`, so it is the one attached file left out of
    /// [`Rules::rules_hash`] (ADR 0003, `docs/adr/0003-visuals-outside-rules-hash.md`):
    /// a look edit needs no `rules_version` bump and no re-record, and a
    /// bad or missing entry still fails `Rules::load` and `data-check`.
    #[serde(skip)]
    pub visuals: Visuals,
    /// Everything under `ai/` (M5b): the bot personalities and build
    /// orders. Attached by [`Rules::load`] and validated against the rest
    /// of the rules; read by `crates/ai`, never by the sim, and in
    /// `rules_hash` like every attached file but `visuals`.
    #[serde(skip_deserializing)]
    pub ai: AiRules,
}

/// The files `Rules::load` read, so cross-file errors name the right one.
struct Paths {
    rules: PathBuf,
    resources: PathBuf,
    units: PathBuf,
    buildings: PathBuf,
    techs: PathBuf,
    ages: PathBuf,
}

/// Which list of an age's `unlocks` a check reads.
#[derive(Clone, Copy)]
enum Unlock {
    Units,
    Buildings,
    Techs,
}

impl Unlock {
    fn list(self, u: &Unlocks) -> &[String] {
        match self {
            Unlock::Units => &u.units,
            Unlock::Buildings => &u.buildings,
            Unlock::Techs => &u.techs,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Unlock::Units => "units",
            Unlock::Buildings => "buildings",
            Unlock::Techs => "techs",
        }
    }
}

/// The result type of the cross-file checks.
type Check = Result<(), Error>;

impl Rules {
    /// Load and validate every rules file under `dir` (the `data/` directory).
    ///
    /// Each file is parsed and checked on its own first, in the order
    /// `rules.ron`, `resources.ron`, `units.ron`, `buildings.ron`,
    /// `techs.ron`, `ages.ron`, `factions.ron`; then one pass checks what
    /// spans files, resolving the `ages.ron` references before anything
    /// else; then `visuals.ron` and the maps load, and last the bot data
    /// under `ai/` (M5b), checked against everything before it.
    pub fn load(dir: impl AsRef<Path>) -> Result<Rules, Error> {
        let dir = dir.as_ref();
        let file = |name: &str| dir.join("rules").join(name);
        let paths = Paths {
            rules: file("rules.ron"),
            resources: file("resources.ron"),
            units: file("units.ron"),
            buildings: file("buildings.ron"),
            techs: file("techs.ron"),
            ages: file("ages.ron"),
        };
        let factions_path = file("factions.ron");
        let mut rules: Rules = load_ron(&paths.rules)?;
        rules.validate_match_rules(&paths.rules)?;

        rules.resources = load_ron(&paths.resources)?;
        rules.resources.validate(&paths.resources)?;
        let resource_ids: Vec<String> = rules
            .resources
            .resources
            .iter()
            .map(|r| r.id.clone())
            .collect();
        let resource_ids: Vec<&str> = resource_ids.iter().map(String::as_str).collect();

        let units: Units = load_ron(&paths.units)?;
        units.validate(&paths.units, &resource_ids)?;
        rules.units = units.units;
        rules.counters = units.counters;
        let unit_ids: Vec<&str> = rules.units.iter().map(|u| u.id.as_str()).collect();

        let buildings: Buildings = load_ron(&paths.buildings)?;
        buildings.validate(&paths.buildings, &resource_ids, &unit_ids)?;
        rules.buildings = buildings.buildings;
        let building_ids: Vec<&str> = rules.buildings.iter().map(|b| b.id.as_str()).collect();

        let techs: Techs = load_ron(&paths.techs)?;
        techs.validate(&paths.techs, &resource_ids, &unit_ids, &building_ids)?;
        let ages: Ages = load_ron(&paths.ages)?;
        ages.validate(&paths.ages, &resource_ids)?;
        let factions: Factions = load_ron(&factions_path)?;
        factions.validate(&factions_path, &unit_ids)?;
        rules.techs = techs.techs;
        rules.ages = ages.ages;
        rules.factions = factions.factions;

        rules.cross_check(&paths)?;

        let visuals_path = dir.join("visuals.ron");
        rules.visuals = load_ron(&visuals_path)?;
        rules
            .visuals
            .validate(&visuals_path, &rules.units, &rules.buildings)?;

        let maps_dir = dir.join("maps");
        rules.maps = map::load_dir(&maps_dir)?;
        if !rules.maps.contains_key(&rules.default_map) {
            return Err(Error::invalid(
                &paths.rules,
                "default_map",
                format!(
                    "no file maps/{}.ron (found: {})",
                    rules.default_map,
                    rules.maps.keys().cloned().collect::<Vec<_>>().join(", ")
                ),
            ));
        }
        rules.check_start_footprints(&maps_dir)?;
        rules.ai = ai::load(dir, &rules)?;
        Ok(rules)
    }

    /// The checks that need more than one file, run once every file under
    /// `rules/` has passed its own checks. Each error names the file holding
    /// the side that has to change. The `ages.ron` references resolve first,
    /// so a tech removed from `techs.ron` is reported where `ages.ron` names
    /// it, before the line order of the remaining techs is checked.
    fn cross_check(&self, p: &Paths) -> Check {
        self.check_age_references(&p.ages)?;
        techs::validate_lines(&p.techs, &self.techs)?;
        self.check_unlocked_once(&p.ages)?;
        self.check_unit_references(&p.units)?;
        self.check_unlock_order(&p.ages)?;
        self.check_age_gates_reachable(&p.ages)?;
        if let Some(age) = &self.town_growth_age
            && self.age_index(age).is_none()
        {
            return Err(Error::invalid(
                &p.rules,
                "town_growth_age",
                format!("{age:?} is not an age id in ages.ron"),
            ));
        }
        if let Some(age) = &self.harrying_bonus_age
            && self.age_index(age).is_none()
        {
            return Err(Error::invalid(
                &p.rules,
                "harrying_bonus_age",
                format!("{age:?} is not an age id in ages.ron"),
            ));
        }
        self.check_annexation_sets(&p.rules)?;
        self.check_gathered_from(&p.resources)?;
        let non_town = self.buildings.iter().filter(|b| !b.town).count();
        if usize::try_from(self.town_growth_distinct_buildings).unwrap_or(usize::MAX) > non_town {
            return Err(Error::invalid(
                &p.rules,
                "town_growth_distinct_buildings",
                format!("must be <= the {non_town} non-Town kinds in buildings.ron"),
            ));
        }
        self.check_standard_start(&p.rules)?;
        self.check_costs_fit_caps(&p.techs, &p.ages)?;
        self.check_cooldowns(&p.units, &p.buildings)?;
        self.check_worst_cases(p)?;
        self.check_min_hits(&p.units, &p.buildings)?;
        self.check_research_times(&p.techs)
    }

    /// Where `id` is listed in `kind` lists: `(age, position)`.
    fn unlock_position(&self, kind: Unlock, id: &str) -> Option<(usize, usize)> {
        self.ages.iter().enumerate().find_map(|(i, a)| {
            kind.list(&a.unlocks)
                .iter()
                .position(|x| x == id)
                .map(|j| (i, j))
        })
    }

    /// Every id an age unlocks names a unit kind, building kind or tech.
    fn check_age_references(&self, path: &Path) -> Check {
        for (i, a) in self.ages.iter().enumerate() {
            for kind in [Unlock::Units, Unlock::Buildings, Unlock::Techs] {
                for (j, id) in kind.list(&a.unlocks).iter().enumerate() {
                    let (known, file) = match kind {
                        Unlock::Units => (self.unit_kind_by_id(id).is_some(), "units.ron"),
                        Unlock::Buildings => {
                            (self.building_kind_by_id(id).is_some(), "buildings.ron")
                        }
                        Unlock::Techs => (self.tech_by_id(id).is_some(), "techs.ron"),
                    };
                    if !known {
                        return Err(Error::invalid(
                            path,
                            &format!("ages[{i}].unlocks.{}[{j}]", kind.name()),
                            format!("{id:?} is not an id in {file}"),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Every unit kind, building kind and tech is unlocked by exactly one
    /// age.
    fn check_unlocked_once(&self, path: &Path) -> Check {
        for kind in [Unlock::Units, Unlock::Buildings, Unlock::Techs] {
            let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
            for (i, a) in self.ages.iter().enumerate() {
                for (j, id) in kind.list(&a.unlocks).iter().enumerate() {
                    if let Some(first) = seen.insert(id.as_str(), i) {
                        return Err(Error::invalid(
                            path,
                            &format!("ages[{i}].unlocks.{}[{j}]", kind.name()),
                            format!("{id:?} is already unlocked by ages[{first}]"),
                        ));
                    }
                }
            }
            let ids: Vec<&str> = match kind {
                Unlock::Units => self.units.iter().map(|u| u.id.as_str()).collect(),
                Unlock::Buildings => self.buildings.iter().map(|b| b.id.as_str()).collect(),
                Unlock::Techs => self.techs.iter().map(|t| t.id.as_str()).collect(),
            };
            if let Some(id) = ids.into_iter().find(|id| !seen.contains_key(id)) {
                return Err(Error::invalid(
                    path,
                    &format!("ages[*].unlocks.{}", kind.name()),
                    format!("{id:?} is unlocked by no age; list it in exactly one"),
                ));
            }
        }
        Ok(())
    }

    /// Every unit kind's `trainer` names a building kind and its
    /// `requires_tech`, if any, a tech.
    fn check_unit_references(&self, path: &Path) -> Check {
        for (i, u) in self.units.iter().enumerate() {
            if self.building_kind_by_id(&u.trainer).is_none() {
                return Err(Error::invalid(
                    path,
                    &format!("units[{i}].trainer"),
                    format!("{:?} is not a building id in buildings.ron", u.trainer),
                ));
            }
            if let Some(t) = &u.requires_tech
                && self.tech_by_id(t).is_none()
            {
                return Err(Error::invalid(
                    path,
                    &format!("units[{i}].requires_tech"),
                    format!("{t:?} is not a tech id in techs.ron"),
                ));
            }
        }
        Ok(())
    }

    /// Nothing is unlocked before what it needs: a tech no earlier than its
    /// host and its predecessor, a unit kind no earlier than its trainer
    /// and its `requires_tech`.
    fn check_unlock_order(&self, path: &Path) -> Check {
        let age_of = |kind: Unlock, id: &str| {
            self.unlock_position(kind, id)
                .expect("every id is unlocked once")
                .0
        };
        let fail = |kind: Unlock, id: &str, needs: &str, other: Unlock, other_id: &str| {
            let (i, j) = self.unlock_position(kind, id).expect("unlocked once");
            Error::invalid(
                path,
                &format!("ages[{i}].unlocks.{}[{j}]", kind.name()),
                format!(
                    "{id:?} is unlocked in {:?}, before its {needs} {other_id:?} (unlocked in {:?})",
                    self.ages[i].id,
                    self.ages[age_of(other, other_id)].id
                ),
            )
        };
        for (i, t) in self.techs.iter().enumerate() {
            let at = age_of(Unlock::Techs, &t.id);
            if age_of(Unlock::Buildings, &t.host) > at {
                return Err(fail(
                    Unlock::Techs,
                    &t.id,
                    "host",
                    Unlock::Buildings,
                    &t.host,
                ));
            }
            let index = u16::try_from(i).expect("validated tech count");
            if let Some(prev) = self.tech_predecessor(index) {
                let prev = &self.techs[usize::from(prev)].id;
                if age_of(Unlock::Techs, prev) > at {
                    return Err(fail(
                        Unlock::Techs,
                        &t.id,
                        "predecessor",
                        Unlock::Techs,
                        prev,
                    ));
                }
            }
        }
        for u in &self.units {
            let at = age_of(Unlock::Units, &u.id);
            if age_of(Unlock::Buildings, &u.trainer) > at {
                return Err(fail(
                    Unlock::Units,
                    &u.id,
                    "trainer",
                    Unlock::Buildings,
                    &u.trainer,
                ));
            }
            if let Some(t) = &u.requires_tech
                && age_of(Unlock::Techs, t) > at
            {
                return Err(fail(
                    Unlock::Units,
                    &u.id,
                    "requires_tech",
                    Unlock::Techs,
                    t,
                ));
            }
        }
        Ok(())
    }

    /// An age's `techs_required` can be met with the techs the ages before
    /// it unlock, counting only those that count toward an age (M5a: not
    /// the Harrying side techs). Since those ages unlock at most
    /// [`Rules::counting_techs`], no gate can ask for more.
    fn check_age_gates_reachable(&self, path: &Path) -> Check {
        let mut available = 0usize;
        for (i, a) in self.ages.iter().enumerate() {
            if usize::try_from(a.techs_required).unwrap_or(usize::MAX) > available {
                return Err(Error::invalid(
                    path,
                    &format!("ages[{i}].techs_required"),
                    format!(
                        "{} techs can never be researched: the ages before it unlock {available} that count toward an age",
                        a.techs_required
                    ),
                ));
            }
            available += a
                .unlocks
                .techs
                .iter()
                .filter(|id| {
                    self.tech_by_id(id)
                        .is_some_and(|(_, t)| t.counts_toward_age)
                })
                .count();
        }
        Ok(())
    }

    /// The annexation selectors (M5a), naming `rules.ron`: every `Kind`
    /// names a unit kind in `units.ron`; each list reaches at least one
    /// kind; no attacker selector reaches a `Civilian` kind.
    fn check_annexation_sets(&self, path: &Path) -> Check {
        for (field, sels) in [
            ("annexation_attackers", &self.annexation_attackers),
            ("annexation_defenders", &self.annexation_defenders),
        ] {
            for (j, sel) in sels.iter().enumerate() {
                if let UnitSel::Kind(id) = sel
                    && self.unit_kind_by_id(id).is_none()
                {
                    return Err(Error::invalid(
                        path,
                        &format!("{field}[{j}]"),
                        format!("{id:?} is not a unit id in units.ron"),
                    ));
                }
            }
            if !self.units.iter().any(|u| sels.iter().any(|s| s.selects(u))) {
                return Err(Error::invalid(
                    path,
                    field,
                    "selects no unit kind in units.ron",
                ));
            }
        }
        for (j, sel) in self.annexation_attackers.iter().enumerate() {
            if let Some(u) = self
                .units
                .iter()
                .find(|u| u.class == UnitClass::Civilian && sel.selects(u))
            {
                return Err(Error::invalid(
                    path,
                    &format!("annexation_attackers[{j}]"),
                    format!(
                        "selects {:?}, a Civilian kind; civilians never annex a Town",
                        u.id
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Each resource's `gathered_from` equals the building kinds that yield
    /// or gather it.
    fn check_gathered_from(&self, path: &Path) -> Check {
        for (i, r) in self.resources.resources.iter().enumerate() {
            let declared: BTreeSet<&str> = r.gathered_from.iter().map(String::as_str).collect();
            let actual: BTreeSet<&str> = self
                .buildings
                .iter()
                .filter(|b| {
                    b.base_yield.contains_key(&r.id)
                        || b.gather.as_ref().is_some_and(|g| g.resource == r.id)
                })
                .map(|b| b.id.as_str())
                .collect();
            if declared != actual {
                return Err(Error::invalid(
                    path,
                    &format!("resources[{i}].gathered_from"),
                    format!(
                        "{declared:?} must equal the buildings.ron kinds that yield or gather {:?}: {actual:?}",
                        r.id
                    ),
                ));
            }
        }
        Ok(())
    }

    fn check_standard_start(&self, path: &Path) -> Check {
        let start = &self.standard_start;
        let mut pop: u64 = 0;
        for (id, &count) in &start.units {
            let field = format!("standard_start.units[{id:?}]");
            let Some(kind) = self.unit_kind_by_id(id) else {
                return Err(Error::invalid(path, &field, "not a unit id in units.ron"));
            };
            if !(1..=255).contains(&count) {
                return Err(Error::invalid(
                    path,
                    &field,
                    format!("{count} must be in 1..=255"),
                ));
            }
            pop += u64::from(kind.pop) * u64::from(count);
        }
        if pop > u64::from(self.pop_cap_base) {
            return Err(Error::invalid(
                path,
                "standard_start.units",
                format!("take {pop} pop, over pop_cap_base {}", self.pop_cap_base),
            ));
        }
        for r in &self.resources.resources {
            let field = format!("standard_start.stockpile[{:?}]", r.id);
            let Some(&v) = start.stockpile.get(&r.id) else {
                return Err(Error::invalid(
                    path,
                    &field,
                    "missing: list every resource, zeros included",
                ));
            };
            let cap = r
                .stockpile_cap
                .unwrap_or(MAX_START_STOCK)
                .min(MAX_START_STOCK);
            if v > cap {
                return Err(Error::invalid(
                    path,
                    &field,
                    format!("{v} must be <= {cap}"),
                ));
            }
        }
        if let Some(id) = start
            .stockpile
            .keys()
            .find(|id| self.resource_index(id).is_none())
        {
            return Err(Error::invalid(
                path,
                &format!("standard_start.stockpile[{id:?}]"),
                "not a resource id in resources.ron",
            ));
        }
        Ok(())
    }

    /// Every tech and age cost fits the stockpile cap of each capped
    /// resource (999 Lore), so it can be paid at all.
    fn check_costs_fit_caps(&self, techs_path: &Path, ages_path: &Path) -> Check {
        let lists = self
            .techs
            .iter()
            .enumerate()
            .map(|(i, t)| (techs_path, format!("techs[{i}].cost"), &t.cost))
            .chain(
                self.ages
                    .iter()
                    .enumerate()
                    .map(|(i, a)| (ages_path, format!("ages[{i}].cost"), &a.cost)),
            );
        for (path, field, cost) in lists {
            for (id, &v) in cost {
                let cap = self.resources.get(id).and_then(|r| r.stockpile_cap);
                if let Some(cap) = cap
                    && v > cap
                {
                    return Err(Error::invalid(
                        path,
                        &format!("{field}[{id:?}]"),
                        format!("{v} is over the {id} stockpile cap {cap}; it could never be paid"),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Every attack's cooldown lasts at least one tick.
    fn check_cooldowns(&self, units_path: &Path, buildings_path: &Path) -> Check {
        let attacks = self
            .units
            .iter()
            .enumerate()
            .filter_map(|(i, u)| {
                u.attack
                    .as_ref()
                    .map(|a| (units_path, format!("units[{i}]"), a))
            })
            .chain(self.buildings.iter().enumerate().filter_map(|(i, b)| {
                b.attack
                    .as_ref()
                    .map(|a| (buildings_path, format!("buildings[{i}]"), a))
            }));
        for (path, field, a) in attacks {
            if self.cooldown_ticks(a) == 0 {
                return Err(Error::invalid(
                    path,
                    &format!("{field}.attack.cooldown_ds"),
                    format!(
                        "{} ds is 0 ticks at {} Hz; it must last at least one tick",
                        a.cooldown_ds, self.tick_rate_hz
                    ),
                ));
            }
        }
        Ok(())
    }

    /// The smallest possible hit of every attacker on every defender (each
    /// unit kind and the buildings) does at least 1 damage, under every
    /// faction's rows with any set of researched techs (a lower bound,
    /// `modifiers::lower_bound`) and the lowest variance draw:
    /// `floor(floor(damage * permille / 1000) * (1000 - v) / 1000) >= 1`.
    fn check_min_hits(&self, units_path: &Path, buildings_path: &Path) -> Check {
        let v = i64::from(self.damage_variance_permille);
        let low = |raw: i64, permille: u32| raw * i64::from(permille) / 1000 * (1000 - v) / 1000;
        let defenders: Vec<Defender> = self
            .units
            .iter()
            .map(|u| Defender::Kind(u.id.clone()))
            .chain([Defender::Buildings])
            .collect();
        for (f, faction) in self.factions.iter().enumerate() {
            for (i, u) in self.units.iter().enumerate() {
                let Some(a) = &u.attack else { continue };
                let damage_row = |m: &&Modifier| {
                    matches!(m.target, StatPath::UnitDamage(_)) && m.target.matches(u)
                };
                let raw = modifiers::lower_bound(
                    i64::from(a.damage),
                    faction.modifiers.iter().filter(damage_row),
                    self.techs
                        .iter()
                        .flat_map(|t| t.modifiers.iter())
                        .filter(damage_row),
                )
                .max(0);
                for d in &defenders {
                    let permille = self.counter_permille(&u.id, d);
                    let hit = low(raw, permille);
                    if hit >= 1 {
                        continue;
                    }
                    let field = self
                        .counters
                        .iter()
                        .position(|c| c.attacker == u.id && &c.defender == d)
                        .map_or(format!("units[{i}].attack.damage"), |j| {
                            format!("counters[{j}].permille")
                        });
                    return Err(Error::invalid(
                        units_path,
                        &field,
                        format!(
                            "{:?} hits {d:?} for {hit} (damage at least {raw} under factions[{f}] {:?} with any research, counter {permille}, variance {v}); every hit must do at least 1",
                            u.id, faction.id
                        ),
                    ));
                }
            }
        }
        for (i, b) in self.buildings.iter().enumerate() {
            let Some(a) = &b.attack else { continue };
            let hit = low(i64::from(a.damage), 1000);
            if hit < 1 {
                return Err(Error::invalid(
                    buildings_path,
                    &format!("buildings[{i}].attack.damage"),
                    format!("hits for {hit} at variance {v}; every hit must do at least 1"),
                ));
            }
        }
        Ok(())
    }

    /// Faction `f`'s rows, then every tech's in file order, that `keep`
    /// accepts: every row that could ever apply to a stat.
    fn all_rows<'a>(
        &'a self,
        f: &'a Faction,
        keep: impl Fn(&StatPath) -> bool + Clone + 'a,
    ) -> impl Iterator<Item = &'a Modifier> + Clone + 'a {
        f.modifiers
            .iter()
            .chain(self.techs.iter().flat_map(|t| t.modifiers.iter()))
            .filter(move |m| keep(&m.target))
    }

    /// Every stat stays within its type whatever is researched, in the worst
    /// case `(|base| + max |Set| + sum |Add|)` times each `Mul` above 1000,
    /// so the sim's checked math cannot overflow. The Town radius must stay
    /// within [`MAX_BORDER_RADIUS_TILES`].
    fn check_worst_cases(&self, p: &Paths) -> Check {
        let u32_max = u64::from(u32::MAX);
        let hp_max = u64::from(buildings::MAX_HP);
        for (f, faction) in self.factions.iter().enumerate() {
            let fits = |path: &Path,
                        field: &str,
                        base: u32,
                        stat: &str,
                        bound: Option<u128>,
                        max: u64|
             -> Check {
                if bound.is_some_and(|b| b <= u128::from(max)) {
                    return Ok(());
                }
                Err(Error::invalid(
                    path,
                    field,
                    format!(
                        "{base} with every {stat} row of techs.ron and factions[{f}] {:?} can reach {}; must be <= {max}",
                        faction.id,
                        bound.map_or_else(|| "more than u128".to_string(), |b| b.to_string())
                    ),
                ))
            };
            let player = |stat: StatPath, base: u32| {
                modifiers::worst_case(i64::from(base), self.all_rows(faction, move |t| *t == stat))
            };
            for (field, base, stat, name) in [
                (
                    "yield_cap_base",
                    self.yield_cap_base,
                    StatPath::YieldCap,
                    "YieldCap",
                ),
                (
                    "pop_cap_base",
                    self.pop_cap_base,
                    StatPath::PopCap,
                    "PopCap",
                ),
                (
                    "town_limit_base",
                    self.town_limit_base,
                    StatPath::TownLimit,
                    "TownLimit",
                ),
            ] {
                fits(&p.rules, field, base, name, player(stat, base), u32_max)?;
            }
            let radius = self.town_radius_tiles.max(self.town_radius_grown_tiles);
            fits(
                &p.rules,
                "town_radius_grown_tiles",
                radius,
                "TownBorderRadius",
                player(StatPath::TownBorderRadius, radius),
                u64::from(MAX_BORDER_RADIUS_TILES),
            )?;
            // The Harrying level (M5a): at most MAX_HARRYING_LEVEL, and the
            // attrition interval halved once per level above 1 stays at
            // least one tick at the highest level reachable.
            let harrying = player(StatPath::HarryingLevel, 0);
            fits(
                &p.techs,
                "techs[*].modifiers",
                0,
                "HarryingLevel",
                harrying,
                u64::from(MAX_HARRYING_LEVEL),
            )?;
            let level = harrying
                .and_then(|w| u32::try_from(w).ok())
                .expect("bounded by MAX_HARRYING_LEVEL above");
            let interval = self.attrition_interval_ticks();
            if level >= 1 && interval.checked_shr(level - 1).unwrap_or(0) == 0 {
                return Err(Error::invalid(
                    &p.rules,
                    "attrition_interval_ds",
                    format!(
                        "{} ds is {interval} ticks at {} Hz, which halved for Harrying level {level} (the worst case of the HarryingLevel rows of techs.ron and factions[{f}] {:?}) is 0 ticks; it must stay at least one tick",
                        self.attrition_interval_ds, self.tick_rate_hz, faction.id
                    ),
                ));
            }
            for (i, t) in self.techs.iter().enumerate() {
                for (id, &amount) in &t.cost {
                    fits(
                        &p.techs,
                        &format!("techs[{i}].cost[{id:?}]"),
                        amount,
                        "ResearchCost",
                        player(StatPath::ResearchCost, amount),
                        u32_max,
                    )?;
                }
                fits(
                    &p.techs,
                    &format!("techs[{i}].research_time_ds"),
                    t.research_time_ds,
                    "ResearchTime",
                    player(StatPath::ResearchTime, t.research_time_ds),
                    u64::from(u32::MAX / 100),
                )?;
            }
            for (i, u) in self.units.iter().enumerate() {
                let unit = |hp: bool, base: u32| {
                    modifiers::worst_case(
                        i64::from(base),
                        self.all_rows(faction, move |t| {
                            let stat = if hp {
                                matches!(t, StatPath::UnitHp(_))
                            } else {
                                matches!(t, StatPath::UnitDamage(_))
                            };
                            stat && t.matches(u)
                        }),
                    )
                };
                fits(
                    &p.units,
                    &format!("units[{i}].hp"),
                    u.hp,
                    "UnitHp",
                    unit(true, u.hp),
                    hp_max,
                )?;
                if let Some(a) = &u.attack {
                    fits(
                        &p.units,
                        &format!("units[{i}].attack.damage"),
                        a.damage,
                        "UnitDamage",
                        unit(false, a.damage),
                        hp_max,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Every research lasts at least one tick with every `ResearchTime` row
    /// of the techs and the faction applied.
    fn check_research_times(&self, path: &Path) -> Check {
        for (f, faction) in self.factions.iter().enumerate() {
            for (i, t) in self.techs.iter().enumerate() {
                let rows = self.all_rows(faction, |s| *s == StatPath::ResearchTime);
                let ds = modifiers::resolve(i64::from(t.research_time_ds), rows).max(0);
                let ds = u32::try_from(ds).expect("bounded by check_worst_cases");
                let ticks = self.ticks_from_ds(ds);
                if ticks == 0 {
                    return Err(Error::invalid(
                        path,
                        &format!("techs[{i}].research_time_ds"),
                        format!(
                            "{} ds falls to {ds} ds, 0 ticks, with every ResearchTime row of techs.ron and factions[{f}] {:?}; it must last at least one tick",
                            t.research_time_ds, faction.id
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Every map start must fit the Town footprint centred on it (the
    /// Seat): in bounds and all Grass. Errors name `starts[i]` in the map's
    /// file.
    fn check_start_footprints(&self, maps_dir: &Path) -> Check {
        let town = &self.buildings[usize::from(self.town_kind())];
        let (w, h) = (town.width_tiles, town.height_tiles);
        for (name, map) in &self.maps {
            let path = maps_dir.join(format!("{name}.ron"));
            for (i, s) in map.starts.iter().enumerate() {
                let x0 = i64::from(s.x) - i64::from((w - 1) / 2);
                let y0 = i64::from(s.y) - i64::from((h - 1) / 2);
                let fits = x0 >= 0
                    && y0 >= 0
                    && x0 + i64::from(w) <= i64::from(map.width)
                    && y0 + i64::from(h) <= i64::from(map.height);
                let grass = fits
                    && (y0..y0 + i64::from(h)).all(|y| {
                        (x0..x0 + i64::from(w)).all(|x| {
                            let (x, y) = (
                                u16::try_from(x).expect("in bounds"),
                                u16::try_from(y).expect("in bounds"),
                            );
                            map.tile(x, y) == Terrain::Grass
                        })
                    });
                if !grass {
                    return Err(Error::invalid(
                        &path,
                        &format!("starts[{i}]"),
                        format!(
                            "the {w}x{h} {} footprint centred on ({}, {}) must be in bounds and all grass",
                            town.id, s.x, s.y
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_match_rules(&self, path: &Path) -> Check {
        let positive = |field: &str, v: u32| -> Result<(), Error> {
            if v == 0 {
                Err(Error::invalid(path, field, "must be > 0"))
            } else {
                Ok(())
            }
        };
        positive("rules_version", self.rules_version)?;
        positive("tick_rate_hz", self.tick_rate_hz)?;
        if self.tick_rate_hz > 1000 {
            return Err(Error::invalid(path, "tick_rate_hz", "must be <= 1000"));
        }
        positive("cmd_delay_ticks", self.cmd_delay_ticks)?;
        if self.default_map.is_empty() {
            return Err(Error::invalid(path, "default_map", "must not be empty"));
        }
        positive("yield_cap_base", self.yield_cap_base)?;
        positive("pop_cap_base", self.pop_cap_base)?;
        positive("town_radius_tiles", self.town_radius_tiles)?;
        if self.town_radius_grown_tiles < self.town_radius_tiles {
            return Err(Error::invalid(
                path,
                "town_radius_grown_tiles",
                "must be >= town_radius_tiles",
            ));
        }
        positive(
            "town_growth_distinct_buildings",
            self.town_growth_distinct_buildings,
        )?;
        if self.town_growth_age.as_deref() == Some("") {
            return Err(Error::invalid(
                path,
                "town_growth_age",
                "must not be empty; use None for no age gate",
            ));
        }
        positive("town_limit_base", self.town_limit_base)?;
        positive("attrition_interval_ds", self.attrition_interval_ds)?;
        positive("annexation_ds", self.annexation_ds)?;
        let in_range = |field: &str, v: u32, lo: u32, hi: u32| -> Result<(), Error> {
            if (lo..=hi).contains(&v) {
                Ok(())
            } else {
                Err(Error::invalid(
                    path,
                    field,
                    format!("{v} must be in {lo}..={hi}"),
                ))
            }
        };
        in_range(
            "building_ramp_step_permille",
            self.building_ramp_step_permille,
            0,
            10_000,
        )?;
        in_range(
            "military_ramp_step_permille",
            self.military_ramp_step_permille,
            0,
            10_000,
        )?;
        in_range(
            "military_ramp_cap_permille",
            self.military_ramp_cap_permille,
            1000,
            100_000,
        )?;
        in_range("train_queue_max", self.train_queue_max, 1, 255)?;
        in_range(
            "idle_seek_delay_ds",
            self.idle_seek_delay_ds,
            1,
            MAX_IDLE_SEEK_DS,
        )?;
        in_range(
            "idle_seek_retry_ds",
            self.idle_seek_retry_ds,
            1,
            MAX_IDLE_SEEK_DS,
        )?;
        in_range(
            "damage_variance_permille",
            self.damage_variance_permille,
            0,
            MAX_DAMAGE_VARIANCE_PERMILLE,
        )?;
        in_range(
            "acquire_range_tiles",
            self.acquire_range_tiles,
            1,
            MAX_COMBAT_TILES,
        )?;
        in_range(
            "chase_regoal_tiles",
            self.chase_regoal_tiles,
            1,
            MAX_COMBAT_TILES,
        )?;
        positive("path_budget_expansions", self.path_budget_expansions)?;
        positive("path_cache_entries", self.path_cache_entries)?;
        positive("repath_interval_ds", self.repath_interval_ds)?;
        positive(
            "separation_push_moving_div",
            self.separation_push_moving_div,
        )?;
        positive("separation_push_idle_div", self.separation_push_idle_div)?;
        self.validate_m5a_rules(path)
    }

    /// The attrition, annexation and match-end fields of `rules.ron` (M5a)
    /// that need no other file: ranges, the age-gap table's shape, and
    /// every timer converting to a tick count that fits `u32` (at least one
    /// tick where the sim divides or counts by it).
    fn validate_m5a_rules(&self, path: &Path) -> Check {
        let in_range = |field: &str, v: u32, lo: u32, hi: u32| -> Check {
            if (lo..=hi).contains(&v) {
                Ok(())
            } else {
                Err(Error::invalid(
                    path,
                    field,
                    format!("{v} must be in {lo}..={hi}"),
                ))
            }
        };
        in_range(
            "supply_radius_tiles",
            self.supply_radius_tiles,
            1,
            MAX_SUPPLY_RADIUS_TILES,
        )?;
        let gaps = &self.attrition_age_gap_hp;
        if gaps.is_empty() {
            return Err(Error::invalid(
                path,
                "attrition_age_gap_hp",
                "must list the HP per pulse at age gap 0 at least",
            ));
        }
        for (i, &hp) in gaps.iter().enumerate() {
            let field = format!("attrition_age_gap_hp[{i}]");
            in_range(&field, hp, 1, MAX_ATTRITION_HP)?;
            if i > 0 && hp < gaps[i - 1] {
                return Err(Error::invalid(
                    path,
                    &field,
                    format!(
                        "{hp} is less than {} at gap {}; HP per pulse never decreases as the age gap grows",
                        gaps[i - 1],
                        i - 1
                    ),
                ));
            }
        }
        if self.harrying_bonus_age.as_deref() == Some("") {
            return Err(Error::invalid(
                path,
                "harrying_bonus_age",
                "must not be empty; use None for no bonus",
            ));
        }
        in_range(
            "harrying_bonus_hp",
            self.harrying_bonus_hp,
            0,
            MAX_ATTRITION_HP,
        )?;
        in_range(
            "annexation_adjacent_tiles",
            self.annexation_adjacent_tiles,
            1,
            MAX_ANNEXATION_ADJACENT_TILES,
        )?;
        in_range(
            "annexation_defender_radius_tiles",
            self.annexation_defender_radius_tiles,
            1,
            MAX_COMBAT_TILES,
        )?;
        in_range(
            "annexation_town_hp_permille",
            self.annexation_town_hp_permille,
            0,
            1000,
        )?;
        for (field, sels) in [
            ("annexation_attackers", &self.annexation_attackers),
            ("annexation_defenders", &self.annexation_defenders),
        ] {
            if sels.is_empty() {
                return Err(Error::invalid(
                    path,
                    field,
                    "must list at least one selector (Military, Class(..) or Kind(\"<unit id>\"))",
                ));
            }
        }
        let ticks = |field: &str, ds: u32| -> Result<u32, Error> {
            self.ticks_from_ds_checked(ds).ok_or_else(|| {
                Error::invalid(
                    path,
                    field,
                    format!(
                        "{ds} ds is more ticks than u32 holds at {} Hz",
                        self.tick_rate_hz
                    ),
                )
            })
        };
        let at_least_one_tick = |field: &str, ds: u32| -> Result<u32, Error> {
            let t = ticks(field, ds)?;
            if t == 0 {
                return Err(Error::invalid(
                    path,
                    field,
                    format!(
                        "{ds} ds is 0 ticks at {} Hz; it must last at least one tick",
                        self.tick_rate_hz
                    ),
                ));
            }
            Ok(t)
        };
        ticks("attrition_interval_ds", self.attrition_interval_ds)?;
        let annexation = at_least_one_tick("annexation_ds", self.annexation_ds)?;
        let cap = ticks("match_cap_ds", self.match_cap_ds)?;
        if cap <= annexation {
            return Err(Error::invalid(
                path,
                "match_cap_ds",
                format!(
                    "{} ds ({cap} ticks) must be longer than annexation_ds {} ({annexation} ticks)",
                    self.match_cap_ds, self.annexation_ds
                ),
            ));
        }
        ticks("yield_cap_decay_after_ds", self.yield_cap_decay_after_ds)?;
        at_least_one_tick("yield_cap_decay_every_ds", self.yield_cap_decay_every_ds)?;
        if self.yield_cap_decay_step > self.yield_cap_base {
            return Err(Error::invalid(
                path,
                "yield_cap_decay_step",
                format!(
                    "{} must be <= yield_cap_base {}",
                    self.yield_cap_decay_step, self.yield_cap_base
                ),
            ));
        }
        Ok(())
    }

    /// Convert a duration authored in deciseconds to whole ticks, rounding to
    /// nearest (half rounds up). This is the only seconds-to-ticks conversion
    /// in the codebase: `ticks = round(ds * tick_rate_hz / 10)`.
    pub fn ticks_from_ds(&self, ds: u32) -> u32 {
        self.ticks_from_ds_checked(ds)
            .expect("tick count fits in u32")
    }

    /// [`Rules::ticks_from_ds`], or `None` when the tick count does not fit
    /// `u32`; the validator uses it on timers that have no tighter bound.
    fn ticks_from_ds_checked(&self, ds: u32) -> Option<u32> {
        let scaled = u64::from(ds) * u64::from(self.tick_rate_hz);
        u32::try_from((scaled + 5) / 10).ok()
    }

    /// Attrition interval in ticks.
    pub fn attrition_interval_ticks(&self) -> u32 {
        self.ticks_from_ds(self.attrition_interval_ds)
    }

    /// Annexation timer in ticks.
    pub fn annexation_ticks(&self) -> u32 {
        self.ticks_from_ds(self.annexation_ds)
    }

    /// Hit points one attrition pulse takes at age gap `gap` (the territory
    /// owner's age minus the intruder's, at least 0) before the late
    /// bonus: `attrition_age_gap_hp[min(gap, len - 1)]`, so the last entry
    /// covers every larger gap.
    pub fn attrition_pulse_hp(&self, gap: u8) -> u32 {
        let last = self.attrition_age_gap_hp.len().saturating_sub(1);
        self.attrition_age_gap_hp
            .get(usize::from(gap).min(last))
            .copied()
            .expect("validated non-empty")
    }

    /// The index of `harrying_bonus_age` in `ages.ron`, or `None` for no
    /// bonus. `Rules::load` checks that the id names an age.
    pub fn harrying_bonus_age_index(&self) -> Option<u8> {
        self.harrying_bonus_age
            .as_deref()
            .and_then(|a| self.age_index(a))
    }

    /// The match cap in ticks (54000 at 20 Hz): the step that reaches it
    /// decides the match by territory.
    pub fn match_cap_ticks(&self) -> u32 {
        self.ticks_from_ds(self.match_cap_ds)
    }

    /// The radius within which a defender blocks a Town's annexation, in
    /// hundredths of a tile, as [`Rules::acquire_reach`] (600 for 6 tiles).
    pub fn annexation_defender_radius(&self) -> u32 {
        self.annexation_defender_radius_tiles * 100
    }

    /// The Yield Cap decay's start and step period in ticks:
    /// `(yield_cap_decay_after_ds, yield_cap_decay_every_ds)` converted. The
    /// period is at least one tick in loaded data.
    pub fn yield_cap_decay_ticks(&self) -> (u32, u32) {
        (
            self.ticks_from_ds(self.yield_cap_decay_after_ds),
            self.ticks_from_ds(self.yield_cap_decay_every_ds),
        )
    }

    /// How many techs count toward the ages' `techs_required`: every tech
    /// except the side techs with `counts_toward_age: false` (Harrying).
    pub fn counting_techs(&self) -> usize {
        self.techs.iter().filter(|t| t.counts_toward_age).count()
    }

    /// Ticks between periodic repaths while a unit is moving.
    pub fn repath_interval_ticks(&self) -> u32 {
        self.ticks_from_ds(self.repath_interval_ds)
    }

    /// Border radius of a Town at `tier` (1 = founded, 2 = grown) before any
    /// `TownBorderRadius` row.
    ///
    /// # Panics
    ///
    /// If `tier` is not 1 or 2.
    pub fn town_radius_base(&self, tier: u8) -> u32 {
        match tier {
            1 => self.town_radius_tiles,
            2 => self.town_radius_grown_tiles,
            _ => panic!("Town tier {tier} is not 1 or 2"),
        }
    }

    /// Ticks per income period: rates are per 30 s ([`RATE_PERIOD_DS`]),
    /// so a rate `r` adds exactly `r` to the stockpile every this many
    /// ticks.
    pub fn income_period_ticks(&self) -> u32 {
        self.ticks_from_ds(RATE_PERIOD_DS)
    }

    /// Construction time of `kind` in ticks.
    pub fn build_time_ticks(&self, kind: &BuildingKind) -> u32 {
        self.ticks_from_ds(kind.build_time_ds)
    }

    /// Training time of `kind` in ticks.
    pub fn train_time_ticks(&self, kind: &UnitKind) -> u32 {
        self.ticks_from_ds(kind.train_time_ds)
    }

    /// Cooldown of an attack in ticks; at least 1 for loaded data.
    pub fn cooldown_ticks(&self, attack: &Attack) -> u32 {
        self.ticks_from_ds(attack.cooldown_ds)
    }

    /// How far an attacker looks for enemies, in hundredths of a tile:
    /// `acquire_range_tiles` or its weapon range, whichever is longer.
    pub fn acquire_reach(&self, attack: &Attack) -> u32 {
        (self.acquire_range_tiles * 100).max(attack.range_tiles_x100)
    }

    /// The counter multiplier in permille for hits by the unit kind
    /// `attacker` (an id) on `defender`: the listed row, else 1000.
    pub fn counter_permille(&self, attacker: &str, defender: &Defender) -> u32 {
        self.counters
            .iter()
            .find(|c| c.attacker == attacker && &c.defender == defender)
            .map_or(1000, |c| c.permille)
    }

    /// Ticks an idle worker waits before it seeks work.
    pub fn idle_seek_delay_ticks(&self) -> u32 {
        self.ticks_from_ds(self.idle_seek_delay_ds)
    }

    /// Ticks between an idle worker's failed seek and its next one.
    pub fn idle_seek_retry_ticks(&self) -> u32 {
        self.ticks_from_ds(self.idle_seek_retry_ds)
    }

    /// The map named `name`, if it was loaded.
    pub fn map(&self, name: &str) -> Option<&MapDef> {
        self.maps.get(name)
    }

    /// The unit kind at index `kind` (the sim's `UnitKindId.0`), if it exists.
    pub fn unit_kind(&self, kind: u16) -> Option<&UnitKind> {
        self.units.get(usize::from(kind))
    }

    /// The unit kind with the given id string, if it exists.
    pub fn unit_kind_by_id(&self, id: &str) -> Option<&UnitKind> {
        self.units.iter().find(|u| u.id == id)
    }

    /// The building kind at index `kind` (the sim's `BuildingKindId.0`), if
    /// it exists.
    pub fn building_kind(&self, kind: u16) -> Option<&BuildingKind> {
        self.buildings.get(usize::from(kind))
    }

    /// The building kind with the given id string and its index, if it
    /// exists.
    pub fn building_kind_by_id(&self, id: &str) -> Option<(u16, &BuildingKind)> {
        self.buildings
            .iter()
            .position(|b| b.id == id)
            .map(|i| (u16::try_from(i).expect("validated len"), &self.buildings[i]))
    }

    /// Index of the one Town kind (`town: true`). `Rules::load` guarantees
    /// exactly one.
    pub fn town_kind(&self) -> u16 {
        let i = self
            .buildings
            .iter()
            .position(|b| b.town)
            .expect("buildings.ron has a town kind");
        u16::try_from(i).expect("validated len")
    }

    /// The tech at index `tech` (the sim's `TechId.0`), if it exists.
    pub fn tech(&self, tech: u16) -> Option<&Tech> {
        self.techs.get(usize::from(tech))
    }

    /// The tech with the given id string and its index, if it exists.
    pub fn tech_by_id(&self, id: &str) -> Option<(u16, &Tech)> {
        self.techs
            .iter()
            .position(|t| t.id == id)
            .map(|i| (u16::try_from(i).expect("validated len"), &self.techs[i]))
    }

    /// The tech one level below `tech` in its line, if `tech` is above
    /// level 1 (and exists).
    pub fn tech_predecessor(&self, tech: u16) -> Option<u16> {
        let t = self.tech(tech)?;
        self.techs
            .iter()
            .position(|o| o.line == t.line && o.level + 1 == t.level)
            .map(|i| u16::try_from(i).expect("validated len"))
    }

    /// The index of the age with the given id, if it exists.
    pub fn age_index(&self, id: &str) -> Option<u8> {
        self.ages
            .iter()
            .position(|a| a.id == id)
            .map(|i| u8::try_from(i).expect("at most 255 ages"))
    }

    fn unlocked_in(&self, kind: Unlock, id: &str) -> u8 {
        let (i, _) = self
            .unlock_position(kind, id)
            .unwrap_or_else(|| panic!("{id:?} is unlocked by no age"));
        u8::try_from(i).expect("at most 255 ages")
    }

    /// The age that unlocks unit kind `kind` (the sim's `UnitKindId.0`).
    ///
    /// # Panics
    ///
    /// If the kind does not exist; `Rules::load` guarantees every kind is
    /// unlocked by exactly one age.
    pub fn age_of_unit(&self, kind: u16) -> u8 {
        let id = &self.unit_kind(kind).expect("unit kind exists").id;
        self.unlocked_in(Unlock::Units, id)
    }

    /// The age that unlocks building kind `kind` (the sim's
    /// `BuildingKindId.0`).
    ///
    /// # Panics
    ///
    /// If the kind does not exist.
    pub fn age_of_building(&self, kind: u16) -> u8 {
        let id = &self.building_kind(kind).expect("building kind exists").id;
        self.unlocked_in(Unlock::Buildings, id)
    }

    /// The age that unlocks tech `tech` (the sim's `TechId.0`).
    ///
    /// # Panics
    ///
    /// If the tech does not exist.
    pub fn age_of_tech(&self, tech: u16) -> u8 {
        let id = &self.tech(tech).expect("tech exists").id;
        self.unlocked_in(Unlock::Techs, id)
    }

    /// Every Modifier row that applies to a player of faction `faction`
    /// with the techs `researched`, in collection order: the faction's rows,
    /// then each researched tech's rows in file order. Pass `researched` in
    /// ascending `TechId` order (a `BTreeSet` iterates that way); an index
    /// with no tech is skipped. Filter the result to one stat and pass it to
    /// [`modifiers::resolve`].
    ///
    /// # Panics
    ///
    /// If `faction` is not an index into `factions`.
    pub fn modifier_rows<'a, I>(
        &'a self,
        faction: usize,
        researched: I,
    ) -> impl Iterator<Item = &'a Modifier> + Clone + 'a
    where
        I: IntoIterator<Item = u16>,
        I::IntoIter: Clone + 'a,
    {
        self.factions[faction].modifiers.iter().chain(
            researched
                .into_iter()
                .filter_map(|t| self.tech(t))
                .flat_map(|t| t.modifiers.iter()),
        )
    }

    /// The visual for a unit kind index (the sim's `UnitKindId.0`), if the
    /// kind exists. `Rules::load` guarantees every kind has one.
    pub fn visual(&self, kind: u16) -> Option<&Visual> {
        self.unit_kind(kind).and_then(|k| self.visuals.get(&k.id))
    }

    /// The visual for a building kind index (the sim's `BuildingKindId.0`),
    /// if the kind exists (art pass). `Rules::load` guarantees every kind
    /// has one: `Primitive(Cuboid)` or a `Scene`.
    pub fn building_visual(&self, kind: u16) -> Option<&Visual> {
        self.building_kind(kind)
            .and_then(|k| self.visuals.building(&k.id))
    }

    /// Content hash of the loaded rules (match rules, resources, units and
    /// their counter table, buildings, techs, ages, factions, maps and the
    /// bot data under `ai/`); stored in replay headers so a changed RON file
    /// is reported as `RULES CHANGED` instead of a sim divergence.
    /// `visuals.ron` is loaded and validated but not hashed (ADR 0003: the
    /// sim never reads it), like the UI strings, which are not in `Rules`.
    pub fn rules_hash(&self) -> u64 {
        let bytes = postcard::to_allocvec(self).expect("Rules serialises");
        xxhash_rust::xxh3::xxh3_64(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    #[test]
    fn loads_repo_data() {
        let rules = Rules::load(data_dir()).expect("data/ loads");
        assert_eq!(rules.tick_rate_hz, 20);
        assert_eq!(rules.resources.resources.len(), 4);
        assert_eq!(rules.units.len(), 8);
        assert_eq!(rules.unit_kind(0).unwrap().id, "yeoman");
        assert_eq!(rules.unit_kind(0).unwrap().speed_tiles_per_s_x100, 180);
        let ids: Vec<&str> = rules.units.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "yeoman",
                "scribe",
                "skirmisher",
                "shieldbearer",
                "bowman",
                "outrider",
                "mangonel",
                "supply_wain"
            ]
        );
        assert!(rules.unit_kind(8).is_none());
        assert!(rules.unit_kind_by_id("yeoman").is_some());
        assert_eq!(rules.buildings.len(), 8);
        assert_eq!(rules.town_kind(), 0);
        assert_eq!(rules.building_kind(0).unwrap().id, "town");
        assert!(rules.building_kind(8).is_none());
        assert_eq!(
            rules
                .building_kind_by_id("watchtower")
                .map(|(i, b)| (i, b.border_radius_tiles)),
            Some((6, Some(2)))
        );
        assert_eq!(rules.ages.len(), 3);
        // The 12 Scriptorium techs count toward the ages; Harrying I and II
        // (M5a) are side techs.
        assert_eq!(rules.techs.len(), 14);
        assert_eq!(rules.counting_techs(), 12);
        assert_eq!(rules.factions.len(), 1);
        assert!(rules.factions[0].modifiers.is_empty());
        assert_eq!(rules.yield_cap_base, 70);
        assert_eq!(rules.pop_cap_base, 25);
        assert!(rules.map("plains_1v1").is_some());
        assert_eq!(
            rules.visual(0),
            Some(&Visual::Primitive(visuals::Primitive::Capsule))
        );
        assert_eq!(
            rules.visual(3),
            Some(&Visual::Body {
                shape: visuals::Primitive::Cuboid,
                height_tiles_x100: 130,
                lying: false,
            })
        );
        assert_eq!(
            rules.visual(5),
            Some(&Visual::Body {
                shape: visuals::Primitive::Capsule,
                height_tiles_x100: 180,
                lying: true,
            })
        );
        assert!(rules.visual(8).is_none());
        assert_eq!(rules.ticks_from_ds(32), 64);
        assert_eq!(rules.attrition_interval_ticks(), 64);
        assert_eq!(rules.annexation_ticks(), 1200);
        // M5a: the attrition, annexation and match-end knobs.
        assert_eq!(rules.supply_radius_tiles, 14);
        assert_eq!(rules.attrition_age_gap_hp, [1, 2]);
        assert_eq!(
            (0..=3)
                .map(|g| rules.attrition_pulse_hp(g))
                .collect::<Vec<_>>(),
            [1, 2, 2, 2],
            "the last row covers every larger gap"
        );
        assert_eq!(rules.harrying_bonus_age_index(), Some(2));
        assert_eq!(rules.harrying_bonus_hp, 0);
        assert_eq!(
            (
                rules.annexation_adjacent_tiles,
                rules.annexation_defender_radius_tiles,
                rules.annexation_defender_radius(),
                rules.annexation_town_hp_permille,
            ),
            (1, 6, 600, 250)
        );
        assert_eq!(
            rules.annexation_attackers,
            [
                UnitSel::Class(UnitClass::Infantry),
                UnitSel::Class(UnitClass::Cavalry)
            ]
        );
        assert_eq!(rules.annexation_defenders, [UnitSel::Military]);
        assert_eq!(rules.match_cap_ticks(), 54_000);
        assert_eq!(rules.yield_cap_decay_ticks(), (36_000, 6000));
        assert_eq!(rules.yield_cap_decay_step, 0, "off until M6");
        let flags = |id: &str| {
            let u = rules.unit_kind_by_id(id).unwrap();
            (u.immune_to_attrition, u.supplies)
        };
        assert_eq!(
            ["yeoman", "scribe", "skirmisher", "supply_wain"].map(flags),
            [(false, false), (true, false), (false, false), (true, true)]
        );
        let stays: Vec<&str> = rules
            .buildings
            .iter()
            .filter(|b| b.stays_on_annexation)
            .map(|b| b.id.as_str())
            .collect();
        assert_eq!(stays, ["watchtower"]);
        assert_eq!(rules.repath_interval_ticks(), 60);
        assert_eq!(rules.separation_push_moving_div, 2);
        assert_eq!(rules.separation_push_idle_div, 8);
        assert_eq!(
            rules
                .unit_kind(0)
                .unwrap()
                .arrive_slowdown_radius_tiles_x100,
            50
        );
        assert_eq!(
            rules.unit_kind(0).unwrap().return_to_post_radius_tiles_x100,
            100
        );
        assert_eq!(rules.town_radius_base(1), 20);
        assert_eq!(rules.town_radius_base(2), 24);
        assert_eq!(rules.idle_seek_delay_ticks(), 100);
        assert_eq!(rules.idle_seek_retry_ticks(), 100);
        let (_, town) = rules.building_kind_by_id("town").unwrap();
        assert_eq!(rules.build_time_ticks(town), 1200);
        assert_eq!(rules.train_time_ticks(rules.unit_kind(0).unwrap()), 240);
        assert_eq!(
            rules.amounts(&rules.standard_start.stockpile),
            [150, 150, 0, 0]
        );
    }

    #[test]
    fn tech_and_age_accessors_read_the_repo_data() {
        let r = Rules::load(data_dir()).unwrap();
        let techs: Vec<&str> = r.techs.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            techs,
            [
                "arms_1",
                "arms_2",
                "arms_3",
                "statecraft_1",
                "statecraft_2",
                "statecraft_3",
                "trade_1",
                "trade_2",
                "trade_3",
                "letters_1",
                "letters_2",
                "letters_3",
                "harrying_1",
                "harrying_2"
            ]
        );
        // Every tech that counts toward an age is a Scriptorium tech; the
        // two side techs are the Watchtower's Harrying line (M5a).
        assert!(
            r.techs
                .iter()
                .filter(|t| t.counts_toward_age)
                .all(|t| t.host == "scriptorium")
        );
        let side: Vec<(&str, &str, u8)> = r
            .techs
            .iter()
            .filter(|t| !t.counts_toward_age)
            .map(|t| (t.id.as_str(), t.host.as_str(), t.level))
            .collect();
        assert_eq!(
            side,
            [
                ("harrying_1", "watchtower", 1),
                ("harrying_2", "watchtower", 2)
            ]
        );
        assert_eq!(r.tech(11).unwrap().id, "letters_3");
        assert_eq!(r.tech(12).unwrap().id, "harrying_1");
        assert_eq!(r.tech(13).unwrap().id, "harrying_2");
        assert!(r.tech(14).is_none());
        assert_eq!(r.tech_predecessor(13), Some(12));
        assert_eq!(r.ages[2].unlocks.techs[3], "letters_3");
        assert_eq!(r.tech_by_id("trade_2").map(|(i, _)| i), Some(7));
        assert!(r.tech_by_id("trade_4").is_none());
        assert_eq!(r.tech_predecessor(0), None);
        assert_eq!(r.tech_predecessor(1), Some(0));
        assert_eq!(r.tech_predecessor(11), Some(10));
        assert_eq!(r.age_index("hearth"), Some(0));
        assert_eq!(r.age_index("masonry"), Some(1));
        assert_eq!(r.age_index("powder"), None);
        assert_eq!(r.town_growth_age.as_deref(), Some("masonry"));
        let (age, unit, building) = (
            |id: &str| r.age_of_tech(r.tech_by_id(id).unwrap().0),
            |id: &str| {
                let i = r.units.iter().position(|u| u.id == id).unwrap();
                r.age_of_unit(u16::try_from(i).unwrap())
            },
            |id: &str| r.age_of_building(r.building_kind_by_id(id).unwrap().0),
        );
        assert_eq!((age("arms_1"), age("letters_2"), age("trade_3")), (0, 1, 2));
        assert_eq!((age("harrying_1"), age("harrying_2")), (1, 2));
        assert_eq!(
            (unit("bowman"), unit("outrider"), unit("supply_wain")),
            (0, 1, 1)
        );
        assert_eq!((building("watchtower"), building("engine_yard")), (0, 1));
        let gates: Vec<(u32, Vec<i64>)> = r
            .ages
            .iter()
            .map(|a| (a.techs_required, r.amounts(&a.cost)))
            .collect();
        assert_eq!(
            gates,
            [
                (0, vec![0, 0, 0, 0]),
                (2, vec![250, 0, 0, 100]),
                (5, vec![500, 300, 0, 300])
            ]
        );
        // Collection order: the faction's rows, then each researched tech in
        // the order given, each with its rows in file order.
        let rows: Vec<&StatPath> = r.modifier_rows(0, [3u16, 6]).map(|m| &m.target).collect();
        assert_eq!(
            rows,
            [
                &StatPath::TownLimit,
                &StatPath::TownBorderRadius,
                &StatPath::YieldCap
            ]
        );
        let cap = modifiers::resolve(
            i64::from(r.yield_cap_base),
            r.modifier_rows(0, [6u16, 7])
                .filter(|m| m.target == StatPath::YieldCap),
        );
        assert_eq!(cap, 150);
    }

    #[test]
    fn combat_accessors_read_the_repo_data() {
        let r = Rules::load(data_dir()).unwrap();
        let kind = |id: &str| Defender::Kind(id.into());
        assert_eq!(r.counter_permille("skirmisher", &kind("bowman")), 2500);
        assert_eq!(r.counter_permille("bowman", &kind("shieldbearer")), 3000);
        assert_eq!(r.counter_permille("mangonel", &Defender::Buildings), 4000);
        assert_eq!(r.counter_permille("bowman", &Defender::Buildings), 250);
        assert_eq!(r.counter_permille("bowman", &kind("bowman")), 1000);
        let attack = |id: &str| r.unit_kind_by_id(id).unwrap().attack.clone();
        assert!(attack("yeoman").is_none() && attack("supply_wain").is_none());
        let bowman = attack("bowman").unwrap();
        assert_eq!(r.cooldown_ticks(&bowman), 30);
        assert_eq!(r.acquire_reach(&bowman), 600);
        assert_eq!(r.acquire_reach(&attack("skirmisher").unwrap()), 600);
        let mangonel = attack("mangonel").unwrap();
        assert_eq!(
            (r.acquire_reach(&mangonel), mangonel.prefer),
            (800, Prefer::Buildings)
        );
        let shooter = |id: &str| r.building_kind_by_id(id).unwrap().1.attack.clone();
        assert_eq!(shooter("town"), shooter("watchtower"));
        assert_eq!(
            shooter("watchtower").map(|a| (a.damage, a.range_tiles_x100, a.cooldown_ds)),
            Some((15, 700, 15))
        );
        assert!(shooter("croft").is_none());
        assert_eq!(
            (
                r.damage_variance_permille,
                r.acquire_range_tiles,
                r.chase_regoal_tiles
            ),
            (0, 6, 2)
        );
        assert!(r.units.iter().all(|u| u.requires_tech.is_none()));
    }

    #[test]
    fn income_period_ticks_is_600_at_20hz() {
        let mut r = Rules::load(data_dir()).unwrap();
        assert_eq!(r.income_period_ticks(), 600);
        r.tick_rate_hz = 30;
        assert_eq!(r.income_period_ticks(), 900);
    }

    #[test]
    #[should_panic(expected = "tier 3")]
    fn town_radius_rejects_unknown_tiers() {
        Rules::load(data_dir()).unwrap().town_radius_base(3);
    }

    #[test]
    fn rules_hash_is_stable_and_covers_attached_files() {
        let a = Rules::load(data_dir()).unwrap();
        let mut b = Rules::load(data_dir()).unwrap();
        assert_eq!(a.rules_hash(), b.rules_hash());
        b.resources.resources[0].name.push('x');
        assert_ne!(
            a.rules_hash(),
            b.rules_hash(),
            "resources.ron is part of the hash"
        );
        let mut c = Rules::load(data_dir()).unwrap();
        c.maps.clear();
        assert_ne!(a.rules_hash(), c.rules_hash(), "maps are part of the hash");
        let mut d = Rules::load(data_dir()).unwrap();
        d.units[0].speed_tiles_per_s_x100 += 1;
        assert_ne!(
            a.rules_hash(),
            d.rules_hash(),
            "units.ron is part of the hash"
        );
        let mut f = Rules::load(data_dir()).unwrap();
        f.buildings[1].hp += 1;
        assert_ne!(
            a.rules_hash(),
            f.rules_hash(),
            "buildings.ron is part of the hash"
        );
    }

    /// ADR 0003: `visuals.ron` is attached and validated by `load` but left
    /// out of `rules_hash` (`#[serde(skip)]`), so a look edit needs no
    /// `rules_version` bump and no re-record. Checked in memory and through
    /// an edited copy of the file, which must load with the edit in place.
    #[test]
    fn rules_hash_ignores_visuals_ron() {
        let a = Rules::load(data_dir()).unwrap();
        let mut e = Rules::load(data_dir()).unwrap();
        e.visuals.units.insert(
            "yeoman".into(),
            Visual::Primitive(visuals::Primitive::Sphere),
        );
        assert_ne!(a.visuals, e.visuals);
        assert_eq!(
            a.rules_hash(),
            e.rules_hash(),
            "visuals.ron is not part of the hash"
        );
        let mut cleared = Rules::load(data_dir()).unwrap();
        cleared.visuals = Visuals::default();
        assert_eq!(a.rules_hash(), cleared.rules_hash());

        let edited = loads_after_edits(
            "visuals-edit",
            &[(
                "visuals.ron",
                r#""skirmisher": Body(shape: Cone, height_tiles_x100: 150)"#,
                r#""skirmisher": Body(shape: Cone, height_tiles_x100: 160)"#,
            )],
        );
        assert_eq!(
            edited.visuals.units.get("skirmisher"),
            Some(&Visual::Body {
                shape: visuals::Primitive::Cone,
                height_tiles_x100: 160,
                lying: false,
            }),
            "the edited file is loaded"
        );
        assert_eq!(
            a.rules_hash(),
            edited.rules_hash(),
            "a visuals.ron edit keeps rules_hash"
        );
    }

    /// The other half of ADR 0003: leaving the hash does not mean leaving
    /// validation. A bad entry still fails `load`, naming the file and field.
    #[test]
    fn bad_visual_still_fails_to_load_naming_file_and_field() {
        let msg = load_error_after_edit(
            "visual-bad-height",
            "visuals.ron",
            r#""skirmisher": Body(shape: Cone, height_tiles_x100: 150)"#,
            r#""skirmisher": Body(shape: Cone, height_tiles_x100: 500)"#,
        );
        assert!(msg.contains("visuals.ron"), "{msg}");
        assert!(
            msg.contains(r#"units["skirmisher"].height_tiles_x100"#),
            "{msg}"
        );
    }

    #[test]
    fn rules_hash_covers_techs_ages_factions() {
        let a = Rules::load(data_dir()).unwrap();
        let mut b = Rules::load(data_dir()).unwrap();
        b.techs[0].research_time_ds += 1;
        assert_ne!(
            a.rules_hash(),
            b.rules_hash(),
            "techs.ron is part of the hash"
        );
        let mut c = Rules::load(data_dir()).unwrap();
        c.ages[1].techs_required += 1;
        assert_ne!(
            a.rules_hash(),
            c.rules_hash(),
            "ages.ron is part of the hash"
        );
        let mut d = Rules::load(data_dir()).unwrap();
        d.factions[0].modifiers.push(Modifier {
            target: StatPath::YieldCap,
            op: ModOp::Mul,
            value: 1250,
        });
        assert_ne!(
            a.rules_hash(),
            d.rules_hash(),
            "factions.ron is part of the hash"
        );
        let mut e = Rules::load(data_dir()).unwrap();
        e.counters[0].permille += 1;
        assert_ne!(
            a.rules_hash(),
            e.rules_hash(),
            "the counter table is part of the hash"
        );
        let mut f = Rules::load(data_dir()).unwrap();
        f.town_growth_age = None;
        assert_ne!(
            a.rules_hash(),
            f.rules_hash(),
            "rules.ron is part of the hash"
        );
    }

    #[test]
    fn missing_visual_names_file_and_kind() {
        let dir = copied_data_dir("visual-missing");
        std::fs::write(dir.join("visuals.ron"), "(units: {}, buildings: {})").unwrap();
        let err = Rules::load(&dir).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("visuals.ron"), "{msg}");
        assert!(msg.contains(r#"units["yeoman"]"#), "{msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_zero_unit_speed_naming_file_and_field() {
        let dir = copied_data_dir("unit-speed-zero");
        let path = dir.join("rules/units.ron");
        let text = std::fs::read_to_string(&path).unwrap();
        let text = text.replacen(
            "speed_tiles_per_s_x100: 180,",
            "speed_tiles_per_s_x100: 0,",
            1,
        );
        std::fs::write(&path, text).unwrap();
        let err = Rules::load(&dir).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("units.ron"), "{msg}");
        assert!(msg.contains("units[0].speed_tiles_per_s_x100"), "{msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_field_names_file_and_field() {
        let text = std::fs::read_to_string(data_dir().join("rules/rules.ron")).unwrap();
        let text = text.replacen("tick_rate_hz:", "tick_rate_hertz: 1,\n  tick_rate_hz:", 1);
        let path = Path::new("data/rules/rules.ron");
        let err = parse_ron::<Rules>(path, &text).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("data/rules/rules.ron"), "{msg}");
        assert!(msg.contains("tick_rate_hertz"), "{msg}");
    }

    #[test]
    fn ticks_from_ds_rounds_to_nearest() {
        let mut r = Rules::load(data_dir()).unwrap();
        r.tick_rate_hz = 20;
        assert_eq!(r.ticks_from_ds(0), 0);
        assert_eq!(r.ticks_from_ds(1), 2);
        assert_eq!(r.ticks_from_ds(300), 600);
        r.tick_rate_hz = 3;
        // 0.5 s at 3 Hz = 1.5 ticks -> 2
        assert_eq!(r.ticks_from_ds(5), 2);
        // 0.4 s at 3 Hz = 1.2 ticks -> 1
        assert_eq!(r.ticks_from_ds(4), 1);
    }

    #[test]
    fn validator_rejects_bad_ranges() {
        let mut r = Rules::load(data_dir()).unwrap();
        r.town_radius_grown_tiles = 1;
        let err = r.validate_match_rules(Path::new("x.ron")).unwrap_err();
        assert!(err.to_string().contains("town_radius_grown_tiles"), "{err}");
        for (field, set) in [
            (
                "yield_cap_base",
                (|r: &mut Rules| r.yield_cap_base = 0) as fn(&mut Rules),
            ),
            ("damage_variance_permille", |r| {
                r.damage_variance_permille = MAX_DAMAGE_VARIANCE_PERMILLE + 1
            }),
            ("acquire_range_tiles", |r| r.acquire_range_tiles = 0),
            ("acquire_range_tiles", |r| {
                r.acquire_range_tiles = MAX_COMBAT_TILES + 1
            }),
            ("chase_regoal_tiles", |r| r.chase_regoal_tiles = 0),
            ("chase_regoal_tiles", |r| {
                r.chase_regoal_tiles = MAX_COMBAT_TILES + 1
            }),
            ("town_growth_age", |r| {
                r.town_growth_age = Some(String::new())
            }),
            ("repath_interval_ds", |r| r.repath_interval_ds = 0),
            ("separation_push_moving_div", |r| {
                r.separation_push_moving_div = 0
            }),
            ("separation_push_idle_div", |r| {
                r.separation_push_idle_div = 0
            }),
        ] {
            let mut r = Rules::load(data_dir()).unwrap();
            set(&mut r);
            let err = r.validate_match_rules(Path::new("x.ron")).unwrap_err();
            assert!(err.to_string().contains(field), "{err}");
        }
    }

    fn copy_dir(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if src_path.is_dir() {
                copy_dir(&src_path, &dst_path);
            } else {
                std::fs::copy(src_path, dst_path).unwrap();
            }
        }
    }

    fn copied_data_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "eonmark-rules-test-{}-{}",
            name,
            std::process::id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        copy_dir(&data_dir(), &dir);
        dir
    }

    /// Replace the first `from` in the copied `file` with `to`, asserting
    /// that it was there, then load and return the error message.
    fn load_error_after_edit(name: &str, file: &str, from: &str, to: &str) -> String {
        load_error_after_edits(name, &[(file, from, to)])
    }

    /// `load_error_after_edit` with several `(file, from, to)` edits.
    fn load_error_after_edits(name: &str, edits: &[(&str, &str, &str)]) -> String {
        let dir = copied_data_dir(name);
        for (file, from, to) in edits {
            let path = dir.join(file);
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.contains(from), "{file} no longer contains {from:?}");
            std::fs::write(&path, text.replacen(from, to, 1)).unwrap();
        }
        let msg = Rules::load(&dir).unwrap_err().to_string();
        let _ = std::fs::remove_dir_all(&dir);
        msg
    }

    /// Apply text edits to a copy of `data/` and load it, expecting success.
    fn loads_after_edits(name: &str, edits: &[(&str, &str, &str)]) -> Rules {
        let dir = copied_data_dir(name);
        for (file, from, to) in edits {
            let path = dir.join(file);
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.contains(from), "{file} no longer contains {from:?}");
            std::fs::write(&path, text.replacen(from, to, 1)).unwrap();
        }
        let rules = Rules::load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        rules.unwrap()
    }

    #[test]
    fn rejects_zero_rules_version() {
        let dir = copied_data_dir("rules-version-zero");
        let path = dir.join("rules/rules.ron");
        let text = std::fs::read_to_string(&path).unwrap();
        // Read the current line so a version bump never touches this test.
        let line = text
            .lines()
            .find(|l| l.trim_start().starts_with("rules_version:"))
            .expect("rules.ron has a rules_version line")
            .trim()
            .to_string();
        let text = text.replacen(&line, "rules_version: 0,", 1);
        assert!(
            text.contains("rules_version: 0,"),
            "repo rules_version moved"
        );
        std::fs::write(&path, text).unwrap();
        let err = Rules::load(&dir).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("rules.ron"), "{msg}");
        assert!(msg.contains("rules_version"), "{msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_zero_yield_cap_base_and_the_removed_cap_fields() {
        let msg = load_error_after_edit(
            "yield-cap-zero",
            "rules/rules.ron",
            "yield_cap_base: 70,",
            "yield_cap_base: 0,",
        );
        assert!(
            msg.contains("rules.ron") && msg.contains("yield_cap_base"),
            "{msg}"
        );
        // The M3a cap fields are gone: the Trade and Arms rows in techs.ron
        // are the only source, so an old file fails naming the field.
        for old in [
            "yield_cap_table: [70, 100, 150, 200],",
            "pop_cap_per_arms_level: 25,",
        ] {
            let msg = load_error_after_edit(
                "cap-field-removed",
                "rules/rules.ron",
                "pop_cap_base: 25,",
                &format!("pop_cap_base: 25,\n  {old}"),
            );
            let name = old.split(':').next().unwrap();
            assert!(msg.contains("rules.ron") && msg.contains(name), "{msg}");
        }
    }

    #[test]
    fn missing_default_map_is_reported() {
        let dir = copied_data_dir("default-map-missing");
        for entry in std::fs::read_dir(dir.join("maps")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "ron") {
                std::fs::remove_file(path).unwrap();
            }
        }
        let err = Rules::load(&dir).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("default_map"), "{msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn buildings_reject_unknown_resource_in_cost() {
        let msg = load_error_after_edit(
            "b-cost",
            "rules/buildings.ron",
            r#"cost: {"lumber": 30}"#,
            r#"cost: {"timber": 30}"#,
        );
        assert!(
            msg.contains("buildings.ron") && msg.contains(r#"buildings[1].cost["timber"]"#),
            "{msg}"
        );
    }

    #[test]
    fn buildings_reject_zero_build_time() {
        let msg = load_error_after_edit(
            "b-time",
            "rules/buildings.ron",
            "build_time_ds: 150,",
            "build_time_ds: 0,",
        );
        assert!(
            msg.contains("buildings.ron") && msg.contains("buildings[1].build_time_ds"),
            "{msg}"
        );
    }

    #[test]
    fn buildings_reject_even_footprint_for_a_border_source() {
        let msg = load_error_after_edit(
            "b-even",
            "rules/buildings.ron",
            "width_tiles: 1,",
            "width_tiles: 2,",
        );
        assert!(
            msg.contains("buildings[6].width_tiles") && msg.contains("odd"),
            "{msg}"
        );
    }

    #[test]
    fn buildings_reject_two_town_kinds() {
        let msg = load_error_after_edit(
            "b-towns",
            "rules/buildings.ron",
            "      id: \"scriptorium\",\n      name: \"Scriptorium\",",
            "      id: \"scriptorium\",\n      name: \"Scriptorium\",\n      town: true,",
        );
        assert!(msg.contains("exactly one kind"), "{msg}");
    }

    #[test]
    fn buildings_reject_grass_adjacent_slots() {
        let msg = load_error_after_edit(
            "b-grass",
            "rules/buildings.ron",
            "Adjacent(terrain: Forest, max",
            "Adjacent(terrain: Grass, max",
        );
        assert!(msg.contains("buildings[2].gather.slots.terrain"), "{msg}");
    }

    #[test]
    fn buildings_reject_fixed_zero_slots() {
        let msg = load_error_after_edit(
            "b-slots",
            "rules/buildings.ron",
            "slots: Fixed(1)",
            "slots: Fixed(0)",
        );
        assert!(msg.contains("buildings[1].gather.slots"), "{msg}");
    }

    #[test]
    fn buildings_reject_float_literal() {
        let msg = load_error_after_edit("b-float", "rules/buildings.ron", "hp: 300,", "hp: 300.5,");
        assert!(msg.contains("buildings.ron") && msg.contains(':'), "{msg}");
    }

    #[test]
    fn gathered_from_must_match_building_yields_both_ways() {
        let msg = load_error_after_edit(
            "g-extra",
            "rules/resources.ron",
            r#"gathered_from: ["ore_pit"]"#,
            r#"gathered_from: ["ore_pit", "watchtower"]"#,
        );
        assert!(
            msg.contains("resources.ron") && msg.contains("resources[2].gathered_from"),
            "{msg}"
        );
        let msg = load_error_after_edit(
            "g-missing",
            "rules/buildings.ron",
            r#"base_yield: {"grain": 10, "lumber": 10}"#,
            r#"base_yield: {"grain": 10, "lumber": 10, "ore": 1}"#,
        );
        assert!(msg.contains("resources[2].gathered_from"), "{msg}");
    }

    #[test]
    fn unit_trainer_must_name_a_building() {
        let msg = load_error_after_edit(
            "u-trainer",
            "rules/units.ron",
            r#"trainer: "muster_hall""#,
            r#"trainer: "barracks""#,
        );
        assert!(
            msg.contains("units.ron") && msg.contains("units[2].trainer"),
            "{msg}"
        );
    }

    #[test]
    fn military_ramp_only_for_non_civilians() {
        let msg = load_error_after_edit(
            "u-ramp",
            "rules/units.ron",
            "class: Infantry,",
            "class: Civilian,",
        );
        assert!(msg.contains("units[2].ramp"), "{msg}");
    }

    #[test]
    fn standard_start_lists_every_resource_and_known_units() {
        let msg = load_error_after_edit("s-res", "rules/rules.ron", r#", "lore": 0}"#, "}");
        assert!(
            msg.contains("rules.ron") && msg.contains(r#"standard_start.stockpile["lore"]"#),
            "{msg}"
        );
        let msg = load_error_after_edit(
            "s-unit",
            "rules/rules.ron",
            r#"units: {"yeoman": 5}"#,
            r#"units: {"peasant": 5}"#,
        );
        assert!(msg.contains(r#"standard_start.units["peasant"]"#), "{msg}");
        let msg = load_error_after_edit(
            "s-pop",
            "rules/rules.ron",
            r#"units: {"yeoman": 5}"#,
            r#"units: {"yeoman": 26}"#,
        );
        assert!(
            msg.contains("standard_start.units") && msg.contains("pop_cap_base"),
            "{msg}"
        );
        let msg = load_error_after_edit(
            "s-lore",
            "rules/rules.ron",
            r#""lore": 0}"#,
            r#""lore": 1000}"#,
        );
        assert!(msg.contains(r#"standard_start.stockpile["lore"]"#), "{msg}");
    }

    #[test]
    fn map_starts_fit_the_town_footprint() {
        // A start on the west edge leaves the 3x3 Seat half off the map.
        let msg = load_error_after_edit(
            "m-start",
            "maps/plains_1v1.ron",
            "starts: [(x: 24, y: 64), (x: 103, y: 64)]",
            "starts: [(x: 0, y: 64), (x: 127, y: 64)]",
        );
        assert!(
            msg.contains("plains_1v1.ron") && msg.contains("starts[0]"),
            "{msg}"
        );
    }

    #[test]
    fn town_growth_threshold_fits_the_kind_count() {
        let msg = load_error_after_edit(
            "t-growth",
            "rules/rules.ron",
            "town_growth_distinct_buildings: 5,",
            "town_growth_distinct_buildings: 8,",
        );
        assert!(msg.contains("town_growth_distinct_buildings"), "{msg}");
    }

    #[test]
    fn ramp_permille_ranges() {
        let msg = load_error_after_edit(
            "r-cap",
            "rules/rules.ron",
            "military_ramp_cap_permille: 2250,",
            "military_ramp_cap_permille: 999,",
        );
        assert!(msg.contains("military_ramp_cap_permille"), "{msg}");
        let msg = load_error_after_edit(
            "r-step",
            "rules/rules.ron",
            "building_ramp_step_permille: 200,",
            "building_ramp_step_permille: 10001,",
        );
        assert!(msg.contains("building_ramp_step_permille"), "{msg}");
        let msg = load_error_after_edit(
            "r-queue",
            "rules/rules.ron",
            "train_queue_max: 5,",
            "train_queue_max: 0,",
        );
        assert!(msg.contains("train_queue_max"), "{msg}");
    }

    #[test]
    fn idle_seek_ds_bounds() {
        let msg = load_error_after_edit(
            "i-delay",
            "rules/rules.ron",
            "idle_seek_delay_ds: 50,",
            "idle_seek_delay_ds: 0,",
        );
        assert!(msg.contains("idle_seek_delay_ds"), "{msg}");
        let msg = load_error_after_edit(
            "i-retry",
            "rules/rules.ron",
            "idle_seek_retry_ds: 50,",
            "idle_seek_retry_ds: 36001,",
        );
        assert!(msg.contains("idle_seek_retry_ds"), "{msg}");
    }

    /// Parse `file` of the repo data as `T`, apply `edit`, write it back
    /// serialised into a copy of `data/` and load that copy: the error.
    fn load_error_with<T>(name: &str, file: &str, edit: impl FnOnce(&mut T)) -> String
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let dir = copied_data_dir(name);
        let path = dir.join(file);
        let mut value: T = load_ron(&path).unwrap();
        edit(&mut value);
        let text = ron::ser::to_string_pretty(&value, ron::ser::PrettyConfig::default()).unwrap();
        std::fs::write(&path, text).unwrap();
        let msg = Rules::load(&dir).unwrap_err().to_string();
        let _ = std::fs::remove_dir_all(&dir);
        msg
    }

    /// `load_error_with`, expecting the copy to load.
    fn loads_with<T>(name: &str, file: &str, edit: impl FnOnce(&mut T)) -> Rules
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let dir = copied_data_dir(name);
        let path = dir.join(file);
        let mut value: T = load_ron(&path).unwrap();
        edit(&mut value);
        let text = ron::ser::to_string_pretty(&value, ron::ser::PrettyConfig::default()).unwrap();
        std::fs::write(&path, text).unwrap();
        let rules = Rules::load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        rules.unwrap()
    }

    #[test]
    fn serialised_copies_of_every_file_load_unchanged() {
        let a = Rules::load(data_dir()).unwrap();
        let b = loads_with::<Techs>("ser-techs", "rules/techs.ron", |_| {});
        let c = loads_with::<Ages>("ser-ages", "rules/ages.ron", |_| {});
        let d = loads_with::<Factions>("ser-factions", "rules/factions.ron", |_| {});
        let e = loads_with::<Units>("ser-units", "rules/units.ron", |_| {});
        for r in [b, c, d, e] {
            assert_eq!(a.rules_hash(), r.rules_hash());
        }
    }

    #[test]
    fn town_radius_worst_case_fits() {
        // 61 plus the three Statecraft rows is exactly 64.
        let msg = load_error_after_edit(
            "t-radius-62",
            "rules/rules.ron",
            "town_radius_grown_tiles: 24,",
            "town_radius_grown_tiles: 62,",
        );
        assert!(
            msg.contains("rules.ron")
                && msg.contains("town_radius_grown_tiles")
                && msg.contains("can reach 65")
                && msg.contains("<= 64"),
            "{msg}"
        );
        let r = loads_after_edits(
            "t-radius-61",
            &[(
                "rules/rules.ron",
                "town_radius_grown_tiles: 24,",
                "town_radius_grown_tiles: 61,",
            )],
        );
        assert_eq!(r.town_radius_base(2), 61);
        // A faction row counts too.
        let msg = load_error_with::<Factions>("t-radius-faction", "rules/factions.ron", |f| {
            f.factions[0].modifiers.push(Modifier {
                target: StatPath::TownBorderRadius,
                op: ModOp::Add,
                value: 38,
            });
        });
        assert!(
            msg.contains("town_radius_grown_tiles") && msg.contains("freeholders"),
            "{msg}"
        );
    }

    #[test]
    fn worst_case_bounds_cover_every_stat() {
        let msg = load_error_with::<Factions>("w-pop", "rules/factions.ron", |f| {
            for _ in 0..2 {
                f.factions[0].modifiers.push(Modifier {
                    target: StatPath::PopCap,
                    op: ModOp::Mul,
                    value: modifiers::MAX_MUL_PERMILLE,
                });
            }
            f.factions[0].modifiers.push(Modifier {
                target: StatPath::PopCap,
                op: ModOp::Set,
                value: modifiers::MAX_ADD_SET,
            });
        });
        assert!(
            msg.contains("rules.ron") && msg.contains("pop_cap_base"),
            "{msg}"
        );
        let msg = load_error_with::<Techs>("w-hp", "rules/techs.ron", |t| {
            for _ in 0..2 {
                t.techs[0].modifiers.push(Modifier {
                    target: StatPath::UnitHp(UnitSel::Kind("supply_wain".into())),
                    op: ModOp::Mul,
                    value: modifiers::MAX_MUL_PERMILLE,
                });
            }
        });
        assert!(
            msg.contains("units.ron") && msg.contains("units[7].hp"),
            "{msg}"
        );
    }

    #[test]
    fn techs_reject_a_float_mul_naming_the_file() {
        let msg = load_error_after_edit("t-float", "rules/techs.ron", "value: 900", "value: 1.25");
        assert!(msg.contains("rules/techs.ron"), "{msg}");
        // The parse error carries the span, `line:col`.
        let span = msg.split("techs.ron: ").nth(1).unwrap_or("");
        assert!(
            span.split(':')
                .next()
                .is_some_and(|l| l.parse::<u32>().is_ok()),
            "{msg}"
        );
    }

    #[test]
    fn techs_reject_non_positive_mul() {
        for bad in ["0", "-900"] {
            let msg = load_error_after_edit(
                "t-mul",
                "rules/techs.ron",
                "value: 900",
                &format!("value: {bad}"),
            );
            assert!(
                msg.contains("rules/techs.ron") && msg.contains("techs[9].modifiers[0].value"),
                "{msg}"
            );
        }
    }

    #[test]
    fn techs_reject_unknown_stat_path_naming_the_file() {
        let msg = load_error_after_edit(
            "t-stat",
            "rules/techs.ron",
            "target: ResearchCost",
            "target: ResearchSpeed",
        );
        assert!(
            msg.contains("rules/techs.ron") && msg.contains("StatPath"),
            "{msg}"
        );
        let msg = load_error_after_edit(
            "t-sel",
            "rules/techs.ron",
            "target: UnitHp(Military)",
            "target: UnitHp(Soldiers)",
        );
        assert!(
            msg.contains("rules/techs.ron") && msg.contains("UnitSel"),
            "{msg}"
        );
        let msg = load_error_after_edit(
            "t-kind",
            "rules/techs.ron",
            "target: UnitHp(Military)",
            r#"target: UnitHp(Kind("dragon"))"#,
        );
        assert!(
            msg.contains("rules/techs.ron") && msg.contains("techs[0].modifiers[1].target"),
            "{msg}"
        );
    }

    #[test]
    fn techs_need_each_level_after_its_predecessor() {
        let msg = load_error_with::<Techs>("t-order", "rules/techs.ron", |t| {
            t.techs.swap(0, 1);
        });
        assert!(
            msg.contains("rules/techs.ron") && msg.contains("techs[0].level"),
            "{msg}"
        );
        let msg = load_error_after_edit("t-level", "rules/techs.ron", "level: 3,", "level: 4,");
        assert!(msg.contains("techs[2].level"), "{msg}");
        let msg = load_error_after_edit("t-dup", "rules/techs.ron", "level: 3,", "level: 2,");
        assert!(
            msg.contains("techs[2].level") && msg.contains("arms_2"),
            "{msg}"
        );
        let msg = load_error_after_edit(
            "t-host",
            "rules/techs.ron",
            r#"host: "scriptorium""#,
            r#"host: "library""#,
        );
        assert!(msg.contains("techs[0].host"), "{msg}");
    }

    #[test]
    fn removing_any_referenced_tech_names_ages_ron_and_field() {
        for (id, field) in [
            ("arms_1", "ages[0].unlocks.techs[0]"),
            ("statecraft_2", "ages[1].unlocks.techs[1]"),
            ("letters_3", "ages[2].unlocks.techs[3]"),
        ] {
            let msg = load_error_with::<Techs>("t-remove", "rules/techs.ron", |t| {
                t.techs.retain(|t| t.id != id);
            });
            assert!(
                msg.contains("rules/ages.ron") && msg.contains(field) && msg.contains(id),
                "{id}: {msg}"
            );
        }
    }

    #[test]
    fn ages_gates_must_be_monotonic() {
        let msg = load_error_after_edit(
            "a-techs",
            "rules/ages.ron",
            "techs_required: 5,",
            "techs_required: 1,",
        );
        assert!(
            msg.contains("rules/ages.ron") && msg.contains("ages[2].techs_required"),
            "{msg}"
        );
        let msg = load_error_after_edit(
            "a-cost",
            "rules/ages.ron",
            r#"cost: {"grain": 500, "lumber": 300, "lore": 300}"#,
            r#"cost: {"grain": 500, "lumber": 300, "lore": 99}"#,
        );
        assert!(msg.contains(r#"ages[2].cost["lore"]"#), "{msg}");
        // A gate the ages before it cannot meet.
        let msg = load_error_after_edit(
            "a-reach",
            "rules/ages.ron",
            "techs_required: 2,",
            "techs_required: 5,",
        );
        assert!(
            msg.contains("ages[1].techs_required") && msg.contains("unlock 4"),
            "{msg}"
        );
    }

    #[test]
    fn ages_first_age_is_free() {
        let msg = load_error_after_edit(
            "a-first",
            "rules/ages.ron",
            "techs_required: 0,",
            "techs_required: 1,",
        );
        assert!(
            msg.contains("rules/ages.ron") && msg.contains("ages[0].techs_required"),
            "{msg}"
        );
        let msg = load_error_with::<Ages>("a-first-cost", "rules/ages.ron", |a| {
            a.ages[0].cost.insert("grain".into(), 1);
        });
        assert!(msg.contains("ages[0].cost"), "{msg}");
    }

    #[test]
    fn ages_unlock_every_id_exactly_once() {
        let msg = load_error_after_edit(
            "a-twice",
            "rules/ages.ron",
            r#"techs: ["arms_2","#,
            r#"techs: ["arms_1", "arms_2","#,
        );
        assert!(
            msg.contains("rules/ages.ron")
                && msg.contains("ages[1].unlocks.techs[0]")
                && msg.contains("ages[0]"),
            "{msg}"
        );
        let msg = load_error_after_edit("a-missing", "rules/ages.ron", r#", "bowman"]"#, "]");
        assert!(
            msg.contains("rules/ages.ron")
                && msg.contains("unlocks.units")
                && msg.contains("bowman"),
            "{msg}"
        );
        let msg = load_error_after_edit(
            "a-unknown",
            "rules/ages.ron",
            r#"buildings: ["engine_yard"]"#,
            r#"buildings: ["engine_yard", "harbour"]"#,
        );
        assert!(msg.contains("ages[1].unlocks.buildings[1]"), "{msg}");
        let msg = load_error_after_edit(
            "a-unknown-unit",
            "rules/ages.ron",
            r#"units: ["outrider","#,
            r#"units: ["knight", "outrider","#,
        );
        assert!(msg.contains("ages[1].unlocks.units[0]"), "{msg}");
    }

    #[test]
    fn unlock_order_respects_trainer_host_and_predecessor() {
        // A unit before its trainer.
        let msg = load_error_with::<Ages>("o-trainer", "rules/ages.ron", |a| {
            a.ages[0].unlocks.buildings.retain(|b| b != "muster_hall");
            a.ages[1].unlocks.buildings.push("muster_hall".into());
        });
        assert!(
            msg.contains("rules/ages.ron")
                && msg.contains("ages[0].unlocks.units[2]")
                && msg.contains("trainer"),
            "{msg}"
        );
        // A tech before its host.
        let msg = load_error_with::<Ages>("o-host", "rules/ages.ron", |a| {
            a.ages[0].unlocks.buildings.retain(|b| b != "scriptorium");
            a.ages[1].unlocks.buildings.push("scriptorium".into());
        });
        assert!(
            msg.contains("ages[0].unlocks.techs[0]") && msg.contains("host"),
            "{msg}"
        );
        // A level before the level below it.
        let msg = load_error_with::<Ages>("o-pred", "rules/ages.ron", |a| {
            a.ages[0].unlocks.techs[0] = "arms_2".into();
            a.ages[1].unlocks.techs[0] = "arms_1".into();
        });
        assert!(
            msg.contains("ages[0].unlocks.techs[0]") && msg.contains("predecessor"),
            "{msg}"
        );
        // A unit before its requires_tech, and one naming no tech.
        let msg = load_error_with::<Units>("o-requires", "rules/units.ron", |u| {
            u.units[5].requires_tech = Some("arms_3".into());
        });
        assert!(
            msg.contains("ages[1].unlocks.units[0]") && msg.contains("requires_tech"),
            "{msg}"
        );
        let msg = load_error_with::<Units>("o-requires-none", "rules/units.ron", |u| {
            u.units[5].requires_tech = Some("arms_9".into());
        });
        assert!(
            msg.contains("rules/units.ron") && msg.contains("units[5].requires_tech"),
            "{msg}"
        );
        // The same age is fine: Arms I gates the Outrider by data alone.
        let r = loads_with::<Units>("o-requires-ok", "rules/units.ron", |u| {
            u.units[5].requires_tech = Some("arms_1".into());
        });
        assert_eq!(r.units[5].requires_tech.as_deref(), Some("arms_1"));
        // town_growth_age names an age.
        let msg = load_error_after_edit(
            "o-growth",
            "rules/rules.ron",
            r#"town_growth_age: Some("masonry"),"#,
            r#"town_growth_age: Some("bronze"),"#,
        );
        assert!(
            msg.contains("rules.ron") && msg.contains("town_growth_age"),
            "{msg}"
        );
    }

    #[test]
    fn costs_fit_capped_stockpiles() {
        let msg = load_error_after_edit(
            "c-tech",
            "rules/techs.ron",
            r#"cost: {"ore": 150, "lore": 250}"#,
            r#"cost: {"ore": 150, "lore": 1000}"#,
        );
        assert!(
            msg.contains("rules/techs.ron") && msg.contains(r#"techs[2].cost["lore"]"#),
            "{msg}"
        );
        let msg = load_error_after_edit(
            "c-age",
            "rules/ages.ron",
            r#""lumber": 300, "lore": 300}"#,
            r#""lumber": 300, "lore": 1000}"#,
        );
        assert!(
            msg.contains("rules/ages.ron") && msg.contains(r#"ages[2].cost["lore"]"#),
            "{msg}"
        );
    }

    #[test]
    fn every_hit_does_at_least_one_damage() {
        // Skirmisher on buildings: 10 * 250 / 1000 = 2, at variance 500 -> 1.
        let r = loads_after_edits(
            "h-ok",
            &[(
                "rules/rules.ron",
                "damage_variance_permille: 0,",
                "damage_variance_permille: 500,",
            )],
        );
        assert_eq!(r.damage_variance_permille, 500);
        // A counter row that floors a hit to 0 is named.
        let msg = load_error_with::<Units>("h-counter", "rules/units.ron", |u| {
            u.counters[7].permille = 99;
        });
        assert!(
            msg.contains("rules/units.ron") && msg.contains("counters[7].permille"),
            "{msg}"
        );
        // So is a faction row that shrinks the damage: 10 -> 1, and 1 * 250
        // / 1000 floors to 0 on buildings.
        let msg = load_error_with::<Factions>("h-faction", "rules/factions.ron", |f| {
            f.factions[0].modifiers.push(Modifier {
                target: StatPath::UnitDamage(UnitSel::Military),
                op: ModOp::Mul,
                value: 100,
            });
        });
        assert!(
            msg.contains("units.ron")
                && msg.contains("counters[7].permille")
                && msg.contains("freeholders"),
            "{msg}"
        );
        // So is a tech row that shrinks it once researched (the Arms rows
        // raise damage and never count against the bound).
        let msg = load_error_with::<Techs>("h-tech", "rules/techs.ron", |t| {
            t.techs[0].modifiers.push(Modifier {
                target: StatPath::UnitDamage(UnitSel::Military),
                op: ModOp::Mul,
                value: 100,
            });
        });
        assert!(
            msg.contains("units.ron")
                && msg.contains("counters[7].permille")
                && msg.contains("with any research"),
            "{msg}"
        );
        // And a shooter whose hit vanishes at the lowest variance draw:
        // 1 * 999 / 1000 = 0.
        let msg = load_error_after_edits(
            "h-town",
            &[
                (
                    "rules/rules.ron",
                    "damage_variance_permille: 0,",
                    "damage_variance_permille: 1,",
                ),
                (
                    "rules/buildings.ron",
                    "damage: 15, range_tiles_x100: 700",
                    "damage: 1, range_tiles_x100: 700",
                ),
            ],
        );
        assert!(
            msg.contains("rules/buildings.ron") && msg.contains("buildings[0].attack.damage"),
            "{msg}"
        );
    }

    #[test]
    fn cooldowns_and_research_last_at_least_one_tick() {
        let msg = load_error_after_edit(
            "k-research",
            "rules/techs.ron",
            "research_time_ds: 250,",
            "research_time_ds: 1,",
        );
        assert!(
            msg.contains("rules/techs.ron") && msg.contains("techs[9].research_time_ds"),
            "{msg}"
        );
        // At 4 Hz one decisecond is 0.4 ticks, which rounds to 0.
        let msg = load_error_after_edits(
            "k-cooldown",
            &[
                ("rules/rules.ron", "tick_rate_hz: 20,", "tick_rate_hz: 4,"),
                ("rules/units.ron", "cooldown_ds: 10", "cooldown_ds: 1"),
            ],
        );
        assert!(
            msg.contains("rules/units.ron") && msg.contains("units[2].attack.cooldown_ds"),
            "{msg}"
        );
    }

    #[test]
    fn counters_and_attacks_are_checked_in_the_repo_files() {
        let msg = load_error_after_edit(
            "u-counter",
            "rules/units.ron",
            r#"(attacker: "skirmisher", defender: Kind("bowman")"#,
            r#"(attacker: "scribe", defender: Kind("bowman")"#,
        );
        assert!(
            msg.contains("rules/units.ron") && msg.contains("counters[0].attacker"),
            "{msg}"
        );
        let msg = load_error_after_edit(
            "u-radius",
            "rules/units.ron",
            "radius_tiles_x100: 45,",
            "radius_tiles_x100: 51,",
        );
        assert!(msg.contains("units[5].radius_tiles_x100"), "{msg}");
        let msg = load_error_after_edit("u-hp", "rules/units.ron", "hp: 46,", "hp: 0,");
        assert!(msg.contains("units[4].hp"), "{msg}");
        let msg = load_error_after_edit(
            "b-prefer",
            "rules/buildings.ron",
            "cooldown_ds: 15)),",
            "cooldown_ds: 15, prefer: Buildings)),",
        );
        assert!(msg.contains("buildings[0].attack.prefer"), "{msg}");
    }

    /// The M5a fields of `rules.ron` that need no other file are range
    /// checked, each error naming the field; the boundaries load.
    #[test]
    fn m5a_match_rules_are_range_checked() {
        let base = Rules::load(data_dir()).unwrap();
        let check = |set: &dyn Fn(&mut Rules)| {
            let mut r = base.clone();
            set(&mut r);
            r.validate_match_rules(Path::new("x.ron"))
                .map_err(|e| e.to_string())
        };
        let cases: [(&str, &str, &dyn Fn(&mut Rules)); 22] = [
            ("supply_radius_tiles", "1..=64", &|r| {
                r.supply_radius_tiles = 0
            }),
            ("supply_radius_tiles", "1..=64", &|r| {
                r.supply_radius_tiles = MAX_SUPPLY_RADIUS_TILES + 1
            }),
            ("attrition_age_gap_hp", "gap 0", &|r| {
                r.attrition_age_gap_hp.clear()
            }),
            ("attrition_age_gap_hp[0]", "1..=1000", &|r| {
                r.attrition_age_gap_hp = vec![0, 2]
            }),
            ("attrition_age_gap_hp[1]", "never decreases", &|r| {
                r.attrition_age_gap_hp = vec![2, 1]
            }),
            ("attrition_age_gap_hp[2]", "1..=1000", &|r| {
                r.attrition_age_gap_hp = vec![1, 2, MAX_ATTRITION_HP + 1]
            }),
            ("harrying_bonus_age", "None", &|r| {
                r.harrying_bonus_age = Some(String::new())
            }),
            ("harrying_bonus_hp", "0..=1000", &|r| {
                r.harrying_bonus_hp = MAX_ATTRITION_HP + 1
            }),
            ("annexation_adjacent_tiles", "1..=4", &|r| {
                r.annexation_adjacent_tiles = 0
            }),
            ("annexation_adjacent_tiles", "1..=4", &|r| {
                r.annexation_adjacent_tiles = 5
            }),
            ("annexation_defender_radius_tiles", "1..=16", &|r| {
                r.annexation_defender_radius_tiles = 0
            }),
            ("annexation_defender_radius_tiles", "1..=16", &|r| {
                r.annexation_defender_radius_tiles = MAX_COMBAT_TILES + 1
            }),
            ("annexation_town_hp_permille", "0..=1000", &|r| {
                r.annexation_town_hp_permille = 1001
            }),
            ("annexation_attackers", "at least one", &|r| {
                r.annexation_attackers.clear()
            }),
            ("annexation_defenders", "at least one", &|r| {
                r.annexation_defenders.clear()
            }),
            // At 4 Hz one decisecond is 0.4 ticks, which rounds to 0.
            ("annexation_ds", "0 ticks", &|r| {
                r.tick_rate_hz = 4;
                r.annexation_ds = 1;
            }),
            ("match_cap_ds", "longer than annexation_ds", &|r| {
                r.match_cap_ds = r.annexation_ds
            }),
            ("match_cap_ds", "more ticks than u32", &|r| {
                r.tick_rate_hz = 1000;
                r.match_cap_ds = u32::MAX;
            }),
            ("attrition_interval_ds", "more ticks than u32", &|r| {
                r.tick_rate_hz = 1000;
                r.attrition_interval_ds = u32::MAX;
            }),
            ("yield_cap_decay_after_ds", "more ticks than u32", &|r| {
                r.tick_rate_hz = 1000;
                r.yield_cap_decay_after_ds = u32::MAX;
            }),
            ("yield_cap_decay_every_ds", "0 ticks", &|r| {
                r.yield_cap_decay_every_ds = 0
            }),
            ("yield_cap_decay_step", "yield_cap_base 70", &|r| {
                r.yield_cap_decay_step = r.yield_cap_base + 1
            }),
        ];
        for (field, words, set) in cases {
            let err = check(set).unwrap_err();
            assert!(
                err.contains(&format!("x.ron: {field}: ")) && err.contains(words),
                "{field}: {err}"
            );
        }
        check(&|r| {
            r.supply_radius_tiles = MAX_SUPPLY_RADIUS_TILES;
            r.attrition_age_gap_hp = vec![1, 1, MAX_ATTRITION_HP];
            r.harrying_bonus_hp = MAX_ATTRITION_HP;
            r.annexation_adjacent_tiles = MAX_ANNEXATION_ADJACENT_TILES;
            r.annexation_defender_radius_tiles = MAX_COMBAT_TILES;
            r.annexation_town_hp_permille = 1000;
            r.match_cap_ds = r.annexation_ds + 1;
            r.yield_cap_decay_after_ds = 0;
            r.yield_cap_decay_every_ds = 1;
            r.yield_cap_decay_step = r.yield_cap_base;
        })
        .unwrap();
        check(&|r| {
            r.harrying_bonus_age = None;
            r.annexation_town_hp_permille = 0;
        })
        .unwrap();
    }

    /// The M5a checks that need the other files name `rules.ron` (or
    /// `buildings.ron` for the Town's flag) and the field.
    #[test]
    fn m5a_cross_file_checks_name_the_file_and_field() {
        let msg = load_error_after_edit(
            "m5a-bonus-age",
            "rules/rules.ron",
            r#"harrying_bonus_age: Some("charter"),"#,
            r#"harrying_bonus_age: Some("bronze"),"#,
        );
        assert!(
            msg.contains("rules/rules.ron: harrying_bonus_age: ") && msg.contains("bronze"),
            "{msg}"
        );
        let attackers = "annexation_attackers: [Class(Infantry), Class(Cavalry)],";
        let msg = load_error_after_edit(
            "m5a-civilian-kind",
            "rules/rules.ron",
            attackers,
            r#"annexation_attackers: [Class(Infantry), Kind("yeoman")],"#,
        );
        assert!(
            msg.contains("rules/rules.ron: annexation_attackers[1]: ")
                && msg.contains("\"yeoman\"")
                && msg.contains("Civilian"),
            "{msg}"
        );
        let msg = load_error_after_edit(
            "m5a-civilian-class",
            "rules/rules.ron",
            attackers,
            "annexation_attackers: [Class(Civilian)],",
        );
        assert!(
            msg.contains("annexation_attackers[0]: ") && msg.contains("Civilian"),
            "{msg}"
        );
        let msg = load_error_after_edit(
            "m5a-unknown-defender",
            "rules/rules.ron",
            "annexation_defenders: [Military],",
            r#"annexation_defenders: [Military, Kind("dragon")],"#,
        );
        assert!(
            msg.contains("rules/rules.ron: annexation_defenders[1]: ") && msg.contains("dragon"),
            "{msg}"
        );
        // Military never reaches a Civilian kind, so it may annex.
        let r = loads_after_edits(
            "m5a-military-attackers",
            &[(
                "rules/rules.ron",
                attackers,
                "annexation_attackers: [Military],",
            )],
        );
        assert_eq!(r.annexation_attackers, [UnitSel::Military]);
        // A selector list that reaches no kind (no Cavalry kind left).
        let mut r = Rules::load(data_dir()).unwrap();
        r.units.retain(|u| u.class != UnitClass::Cavalry);
        r.annexation_attackers = vec![UnitSel::Class(UnitClass::Cavalry)];
        let msg = r
            .check_annexation_sets(Path::new("x.ron"))
            .unwrap_err()
            .to_string();
        assert!(
            msg.contains("x.ron: annexation_attackers: ") && msg.contains("selects no unit kind"),
            "{msg}"
        );
        // The Town is what changes hands; it cannot stay.
        let msg = load_error_after_edit(
            "m5a-town-stays",
            "rules/buildings.ron",
            "town: true,",
            "town: true,\n      stays_on_annexation: true,",
        );
        assert!(
            msg.contains("rules/buildings.ron: buildings[0].stays_on_annexation: "),
            "{msg}"
        );
    }

    /// Harrying I and II count toward no age gate (M5a): the Charter Age's
    /// reachable count is the 8 Scriptorium techs of the two ages before
    /// it, not 9 with Harrying I. Marking Harrying I as counting makes 9
    /// reachable, which shows the flag is what is read.
    #[test]
    fn harrying_techs_do_not_count_toward_age_gates() {
        let r = Rules::load(data_dir()).unwrap();
        assert_eq!((r.techs.len(), r.counting_techs()), (14, 12));
        let nine = ("rules/ages.ron", "techs_required: 5,", "techs_required: 9,");
        let msg = load_error_after_edits("h-gate-9", &[nine]);
        assert!(
            msg.contains("rules/ages.ron: ages[2].techs_required: ") && msg.contains("unlock 8"),
            "{msg}"
        );
        // The shipped build order researches 6 counting techs before its
        // Charter Advance, so a 9-tech gate makes that Advance unreachable
        // (M5b's build-order check); this copy ends the order before it.
        let charter = (
            "ai/build_orders/standard.ron",
            "    Advance(age: \"charter\"),                    // 6 counting techs before it\n    Research(tech: \"arms_3\"),\n",
            "",
        );
        let r = loads_after_edits(
            "h-gate-9-counting",
            &[
                nine,
                (
                    "rules/techs.ron",
                    "      counts_toward_age: false,",
                    "      counts_toward_age: true,",
                ),
                charter,
            ],
        );
        assert_eq!(r.ages[2].techs_required, 9);
        assert_eq!(r.counting_techs(), 13);
        // Each Harrying tech adds one level.
        let level = |researched: &[u16]| {
            modifiers::resolve(
                0,
                r.modifier_rows(0, researched.iter().copied())
                    .filter(|m| m.target == StatPath::HarryingLevel),
            )
        };
        assert_eq!(
            (level(&[]), level(&[0, 9]), level(&[12]), level(&[12, 13])),
            (0, 0, 1, 2)
        );
    }

    /// The `HarryingLevel` rows of the techs and the faction may reach at
    /// most level 8, and the attrition interval halved per level above 1
    /// must stay at least one tick at the highest reachable level: 64
    /// ticks allow level 7 (one tick) but not level 8.
    #[test]
    fn harrying_level_worst_case_bounds_the_level_and_the_interval() {
        // Everything here is read from the repository data, so a data-only
        // edit of `attrition_interval_ds` (the worked example in
        // data/rules/README.md) moves the numbers instead of failing the
        // test: the shipped Harrying rows' level, the interval in ticks and
        // the first level at which it halves to 0.
        let repo = Rules::load(data_dir()).unwrap();
        let shipped: i32 = repo
            .techs
            .iter()
            .flat_map(|t| &t.modifiers)
            .chain(&repo.factions[0].modifiers)
            .filter(|m| m.target == StatPath::HarryingLevel && m.op == ModOp::Add)
            .map(|m| m.value)
            .sum();
        let interval = repo.attrition_interval_ticks();
        // The first level whose halving `interval >> (level - 1)` is 0: one
        // more than the interval's bit length.
        let zero_at = 32 - interval.leading_zeros() + 1;
        let max = i32::try_from(MAX_HARRYING_LEVEL).unwrap();
        let harrying = |value: i32| {
            move |f: &mut Factions| {
                f.factions[0].modifiers.push(Modifier {
                    target: StatPath::HarryingLevel,
                    op: ModOp::Add,
                    value,
                });
            }
        };
        let msg = load_error_with::<Factions>(
            "h-level-9",
            "rules/factions.ron",
            harrying(max + 1 - shipped),
        );
        assert!(
            msg.contains("rules/techs.ron")
                && msg.contains("HarryingLevel")
                && msg.contains(&format!("can reach {}", max + 1))
                && msg.contains(&format!("<= {max}"))
                && msg.contains("freeholders"),
            "{msg}"
        );
        let zero_at = i32::try_from(zero_at).unwrap();
        if zero_at <= max {
            let msg = load_error_with::<Factions>(
                "h-level-8",
                "rules/factions.ron",
                harrying(zero_at - shipped),
            );
            assert!(
                msg.contains("rules/rules.ron: attrition_interval_ds: ")
                    && msg.contains(&format!("{interval} ticks"))
                    && msg.contains(&format!("level {zero_at}")),
                "{msg}"
            );
        }
        let top = zero_at.min(max + 1) - 1;
        let r = loads_with::<Factions>("h-level-7", "rules/factions.ron", harrying(top - shipped));
        let shift = u32::try_from(top - 1).unwrap();
        assert!(r.attrition_interval_ticks().checked_shr(shift) >= Some(1));
    }
}
