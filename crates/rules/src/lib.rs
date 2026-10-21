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
//! | `rules/units.ron` | [`Units`] |
//! | `rules/buildings.ron` | [`Buildings`] |
//! | `maps/*.ron` | [`map::MapDef`] |
//! | `visuals.ron` | [`Visuals`] (how each unit kind is drawn; validated here, read by the game) |
//!
//! Every error names the file and, when the data parsed, the field.
//!
//! The pure cost functions (building, Yeoman and military ramps) are
//! methods on [`Rules`] in the `costs` module, so the sim, the HUD and the
//! bot quote the same numbers.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod buildings;
pub mod costs;
pub mod map;
pub mod resources;
pub mod units;
pub mod visuals;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub use buildings::{BuildingKind, Buildings, GatherSpec, Slots};
pub use map::{MapDef, Symmetry, Terrain, TilePos};
pub use resources::{Resource, Resources};
pub use units::{UnitClass, UnitKind, UnitRamp, Units};
pub use visuals::{Primitive, Visual, Visuals};

/// The time unit every income rate is authored in: per 30 s, in
/// deciseconds. Not a tuning knob; [`Rules::income_period_ticks`] converts
/// it (600 ticks at 20 Hz).
pub const RATE_PERIOD_DS: u32 = 300;

/// Largest `_ds` value for the idle-seek fields (one hour).
pub const MAX_IDLE_SEEK_DS: u32 = 36_000;

/// Largest Town radius before Statecraft, so the radius plus the three
/// Statecraft levels stays within 64 tiles.
pub const MAX_TOWN_RADIUS_TILES: u32 = 60;

/// Largest starting stockpile entry.
pub const MAX_START_STOCK: u32 = 1_000_000;

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
    /// Per-resource income ceiling per 30 s, indexed by Trade level (0 = none).
    /// Lore is exempt (see `resources.ron`). Must be strictly increasing.
    pub yield_cap_table: Vec<u32>,
    /// Population cap before any Arms research.
    pub pop_cap_base: u32,
    /// Population cap added per Arms level.
    pub pop_cap_per_arms_level: u32,
    /// Border radius a Town projects when founded.
    pub town_radius_tiles: u32,
    /// Border radius once a Town hosts `town_growth_distinct_buildings` kinds.
    pub town_radius_grown_tiles: u32,
    /// Distinct building kinds inside a Town's radius needed for growth.
    pub town_growth_distinct_buildings: u32,
    /// Towns a player may own (sites included) before any Statecraft
    /// research; the Seat counts.
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
    /// A Supply Wain cancels attrition within this radius.
    pub supply_radius_tiles: u32,
    /// Interval between attrition ticks at Harrying I, in deciseconds.
    pub attrition_interval_ds: u32,
    /// Time a Town at 0 HP must stay uncontested before it flips, in deciseconds.
    pub annexation_ds: u32,
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
    /// Building kinds from `rules/buildings.ron` in `BuildingKindId` order.
    /// Attached by [`Rules::load`].
    #[serde(skip_deserializing)]
    pub buildings: Vec<BuildingKind>,
    /// Every map under `maps/`, keyed by name. Attached by [`Rules::load`].
    #[serde(skip_deserializing)]
    pub maps: BTreeMap<String, MapDef>,
    /// Contents of `visuals.ron`: a [`Visual`] per unit kind id. Attached by
    /// [`Rules::load`] and validated against `units`; read by the game crate's
    /// presenter, never by the sim.
    #[serde(skip_deserializing)]
    pub visuals: Visuals,
}

impl Rules {
    /// Load and validate every rules file under `dir` (the `data/` directory).
    pub fn load(dir: impl AsRef<Path>) -> Result<Rules, Error> {
        let dir = dir.as_ref();
        let rules_path = dir.join("rules").join("rules.ron");
        let mut rules: Rules = load_ron(&rules_path)?;
        rules.validate_match_rules(&rules_path)?;

        let resources_path = dir.join("rules").join("resources.ron");
        rules.resources = load_ron(&resources_path)?;
        rules
            .resources
            .validate(&resources_path, rules.yield_cap_table.len())?;

        let resource_ids: Vec<String> = rules
            .resources
            .resources
            .iter()
            .map(|r| r.id.clone())
            .collect();
        let resource_ids: Vec<&str> = resource_ids.iter().map(String::as_str).collect();

        let units_path = dir.join("rules").join("units.ron");
        let units: Units = load_ron(&units_path)?;
        units.validate(&units_path, &resource_ids)?;
        rules.units = units.units;
        let unit_ids: Vec<&str> = rules.units.iter().map(|u| u.id.as_str()).collect();

        let buildings_path = dir.join("rules").join("buildings.ron");
        let buildings: Buildings = load_ron(&buildings_path)?;
        buildings.validate(&buildings_path, &resource_ids, &unit_ids)?;
        rules.buildings = buildings.buildings;

        rules.cross_check(&rules_path, &resources_path, &units_path)?;

        let visuals_path = dir.join("visuals.ron");
        rules.visuals = load_ron(&visuals_path)?;
        rules.visuals.validate(&visuals_path, &rules.units)?;

        let maps_dir = dir.join("maps");
        rules.maps = map::load_dir(&maps_dir)?;
        if !rules.maps.contains_key(&rules.default_map) {
            return Err(Error::invalid(
                &rules_path,
                "default_map",
                format!(
                    "no file maps/{}.ron (found: {})",
                    rules.default_map,
                    rules.maps.keys().cloned().collect::<Vec<_>>().join(", ")
                ),
            ));
        }
        rules.check_start_footprints(&maps_dir)?;
        Ok(rules)
    }

    /// The checks that need more than one file, run once `rules.ron`,
    /// `resources.ron`, `units.ron` and `buildings.ron` are loaded. Each
    /// error names the file holding the side that has to change.
    fn cross_check(
        &self,
        rules_path: &Path,
        resources_path: &Path,
        units_path: &Path,
    ) -> Result<(), Error> {
        for (i, u) in self.units.iter().enumerate() {
            if self.building_kind_by_id(&u.trainer).is_none() {
                return Err(Error::invalid(
                    units_path,
                    &format!("units[{i}].trainer"),
                    format!("{:?} is not a building id in buildings.ron", u.trainer),
                ));
            }
        }
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
                    resources_path,
                    &format!("resources[{i}].gathered_from"),
                    format!(
                        "{declared:?} must equal the buildings.ron kinds that yield or gather {:?}: {actual:?}",
                        r.id
                    ),
                ));
            }
        }
        let non_town = self.buildings.iter().filter(|b| !b.town).count();
        if usize::try_from(self.town_growth_distinct_buildings).unwrap_or(usize::MAX) > non_town {
            return Err(Error::invalid(
                rules_path,
                "town_growth_distinct_buildings",
                format!("must be <= the {non_town} non-Town kinds in buildings.ron"),
            ));
        }
        self.check_standard_start(rules_path)
    }

    fn check_standard_start(&self, path: &Path) -> Result<(), Error> {
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

    /// Every map start must fit the Town footprint centred on it (the
    /// Seat): in bounds and all Grass. Errors name `starts[i]` in the map's
    /// file.
    fn check_start_footprints(&self, maps_dir: &Path) -> Result<(), Error> {
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

    fn validate_match_rules(&self, path: &Path) -> Result<(), Error> {
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
        if self.yield_cap_table.is_empty() {
            return Err(Error::invalid(
                path,
                "yield_cap_table",
                "must have at least one entry",
            ));
        }
        if !self.yield_cap_table.windows(2).all(|w| w[0] < w[1]) {
            return Err(Error::invalid(
                path,
                "yield_cap_table",
                "must be strictly increasing",
            ));
        }
        positive("pop_cap_base", self.pop_cap_base)?;
        positive("pop_cap_per_arms_level", self.pop_cap_per_arms_level)?;
        positive("town_radius_tiles", self.town_radius_tiles)?;
        if self.town_radius_grown_tiles < self.town_radius_tiles {
            return Err(Error::invalid(
                path,
                "town_radius_grown_tiles",
                "must be >= town_radius_tiles",
            ));
        }
        if self.town_radius_grown_tiles > MAX_TOWN_RADIUS_TILES {
            return Err(Error::invalid(
                path,
                "town_radius_grown_tiles",
                format!("must be <= {MAX_TOWN_RADIUS_TILES}"),
            ));
        }
        positive(
            "town_growth_distinct_buildings",
            self.town_growth_distinct_buildings,
        )?;
        positive("town_limit_base", self.town_limit_base)?;
        positive("supply_radius_tiles", self.supply_radius_tiles)?;
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
        positive("path_budget_expansions", self.path_budget_expansions)?;
        positive("path_cache_entries", self.path_cache_entries)?;
        positive("repath_interval_ds", self.repath_interval_ds)?;
        positive(
            "separation_push_moving_div",
            self.separation_push_moving_div,
        )?;
        positive("separation_push_idle_div", self.separation_push_idle_div)?;
        Ok(())
    }

    /// Convert a duration authored in deciseconds to whole ticks, rounding to
    /// nearest (half rounds up). This is the only seconds-to-ticks conversion
    /// in the codebase: `ticks = round(ds * tick_rate_hz / 10)`.
    pub fn ticks_from_ds(&self, ds: u32) -> u32 {
        let scaled = u64::from(ds) * u64::from(self.tick_rate_hz);
        u32::try_from((scaled + 5) / 10).expect("tick count fits in u32")
    }

    /// Attrition interval in ticks.
    pub fn attrition_interval_ticks(&self) -> u32 {
        self.ticks_from_ds(self.attrition_interval_ds)
    }

    /// Annexation timer in ticks.
    pub fn annexation_ticks(&self) -> u32 {
        self.ticks_from_ds(self.annexation_ds)
    }

    /// Ticks between periodic repaths while a unit is moving.
    pub fn repath_interval_ticks(&self) -> u32 {
        self.ticks_from_ds(self.repath_interval_ds)
    }

    /// Yield cap for a Trade level, clamped to the last table entry.
    pub fn yield_cap(&self, trade_level: u32) -> u32 {
        let idx = (trade_level as usize).min(self.yield_cap_table.len() - 1);
        self.yield_cap_table[idx]
    }

    /// Population cap at a given Arms level.
    pub fn pop_cap(&self, arms_level: u32) -> u32 {
        self.pop_cap_per_arms_level
            .checked_mul(arms_level)
            .and_then(|v| v.checked_add(self.pop_cap_base))
            .expect("pop cap fits u32")
    }

    /// Towns a player may own, sites included, at a Statecraft level.
    pub fn town_limit(&self, statecraft: u32) -> u32 {
        self.town_limit_base
            .checked_add(statecraft)
            .expect("town limit fits u32")
    }

    /// Border radius of a Town at `tier` (1 = founded, 2 = grown) plus one
    /// tile per Statecraft level of its owner.
    ///
    /// # Panics
    ///
    /// If `tier` is not 1 or 2.
    pub fn town_radius_for(&self, tier: u8, statecraft: u32) -> u32 {
        let base = match tier {
            1 => self.town_radius_tiles,
            2 => self.town_radius_grown_tiles,
            _ => panic!("Town tier {tier} is not 1 or 2"),
        };
        base.checked_add(statecraft).expect("radius fits u32")
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

    /// The visual for a unit kind index (the sim's `UnitKindId.0`), if the
    /// kind exists. `Rules::load` guarantees every kind has one.
    pub fn visual(&self, kind: u16) -> Option<&Visual> {
        self.unit_kind(kind).and_then(|k| self.visuals.get(&k.id))
    }

    /// Content hash of the loaded rules (match rules, resources, units,
    /// buildings, maps and visuals); stored in replay headers so a changed RON file is
    /// reported as `RULES CHANGED` instead of a sim divergence.
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
        assert_eq!(rules.units.len(), 3);
        assert_eq!(rules.unit_kind(0).unwrap().id, "yeoman");
        assert_eq!(rules.unit_kind(0).unwrap().speed_tiles_per_s_x100, 180);
        assert!(rules.unit_kind(3).is_none());
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
        assert!(rules.map("plains_1v1").is_some());
        assert_eq!(
            rules.visual(0),
            Some(&Visual::Primitive(visuals::Primitive::Capsule))
        );
        assert!(rules.visual(3).is_none());
        assert_eq!(rules.ticks_from_ds(32), 64);
        assert_eq!(rules.attrition_interval_ticks(), 64);
        assert_eq!(rules.annexation_ticks(), 1200);
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
        assert_eq!(rules.yield_cap(0), 70);
        assert_eq!(rules.yield_cap(99), 200);
        assert_eq!(rules.pop_cap(2), 75);
        assert_eq!(rules.town_limit(0), rules.town_limit_base);
        assert_eq!(rules.town_limit(3), rules.town_limit_base + 3);
        assert_eq!(rules.town_radius_for(1, 0), 20);
        assert_eq!(rules.town_radius_for(2, 3), 27);
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
    fn income_period_ticks_is_600_at_20hz() {
        let mut r = Rules::load(data_dir()).unwrap();
        assert_eq!(r.income_period_ticks(), 600);
        r.tick_rate_hz = 30;
        assert_eq!(r.income_period_ticks(), 900);
    }

    #[test]
    #[should_panic(expected = "tier 3")]
    fn town_radius_rejects_unknown_tiers() {
        Rules::load(data_dir()).unwrap().town_radius_for(3, 0);
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
        let mut e = Rules::load(data_dir()).unwrap();
        e.visuals.units.insert(
            "yeoman".into(),
            Visual::Primitive(visuals::Primitive::Sphere),
        );
        assert_ne!(
            a.rules_hash(),
            e.rules_hash(),
            "visuals.ron is part of the hash"
        );
        let mut f = Rules::load(data_dir()).unwrap();
        f.buildings[1].hp += 1;
        assert_ne!(
            a.rules_hash(),
            f.rules_hash(),
            "buildings.ron is part of the hash"
        );
    }

    #[test]
    fn missing_visual_names_file_and_kind() {
        let dir = copied_data_dir("visual-missing");
        std::fs::write(dir.join("visuals.ron"), "(units: {})").unwrap();
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
        r.yield_cap_table = vec![70, 70];
        let err = r.validate_match_rules(Path::new("x.ron")).unwrap_err();
        assert!(err.to_string().contains("yield_cap_table"), "{err}");
        let mut r = Rules::load(data_dir()).unwrap();
        r.town_radius_grown_tiles = 1;
        let err = r.validate_match_rules(Path::new("x.ron")).unwrap_err();
        assert!(err.to_string().contains("town_radius_grown_tiles"), "{err}");
        for (field, set) in [
            (
                "repath_interval_ds",
                (|r: &mut Rules| r.repath_interval_ds = 0) as fn(&mut Rules),
            ),
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
        let dir = copied_data_dir(name);
        let path = dir.join(file);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(from), "{file} no longer contains {from:?}");
        std::fs::write(&path, text.replacen(from, to, 1)).unwrap();
        let msg = Rules::load(&dir).unwrap_err().to_string();
        let _ = std::fs::remove_dir_all(&dir);
        msg
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
    fn rejects_zero_pop_cap_per_arms_level() {
        let dir = copied_data_dir("pop-cap-per-arms-zero");
        let path = dir.join("rules/rules.ron");
        let text = std::fs::read_to_string(&path).unwrap();
        let text = text.replacen(
            "pop_cap_per_arms_level: 25,",
            "pop_cap_per_arms_level: 0,",
            1,
        );
        std::fs::write(&path, text).unwrap();
        let err = Rules::load(&dir).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("rules.ron"), "{msg}");
        assert!(msg.contains("pop_cap_per_arms_level"), "{msg}");
        let _ = std::fs::remove_dir_all(&dir);
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

    #[test]
    fn town_radius_leaves_room_for_statecraft() {
        let msg = load_error_after_edit(
            "t-radius",
            "rules/rules.ron",
            "town_radius_grown_tiles: 24,",
            "town_radius_grown_tiles: 61,",
        );
        assert!(msg.contains("town_radius_grown_tiles"), "{msg}");
    }
}
