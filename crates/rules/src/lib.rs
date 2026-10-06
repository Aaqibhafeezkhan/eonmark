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
//! | `maps/*.ron` | [`map::MapDef`] |
//!
//! Every error names the file and, when the data parsed, the field.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod map;
pub mod resources;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use map::{MapDef, Symmetry, Terrain, TilePos};
pub use resources::{Resource, Resources};

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

/// Placeholder vision radii in tiles. Fog of war arrives in M7; the sim
/// reads these from M3a for territory and target acquisition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vision {
    /// Radius revealed around a land unit.
    pub unit_tiles: u32,
    /// Radius revealed around an ordinary building.
    pub building_tiles: u32,
    /// Radius revealed around a Town.
    pub town_tiles: u32,
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
    /// Towns a player may own before any Statecraft research.
    pub town_limit_base: u32,
    /// A Supply Wain cancels attrition within this radius.
    pub supply_radius_tiles: u32,
    /// Interval between attrition ticks at Harrying I, in deciseconds.
    pub attrition_interval_ds: u32,
    /// Time a Town at 0 HP must stay uncontested before it flips, in deciseconds.
    pub annexation_ds: u32,
    /// Vision radii placeholder.
    pub vision: Vision,
    /// A* node expansions shared by all path requests per tick.
    pub path_budget_expansions: u32,
    /// Entries kept in the bounded path cache.
    pub path_cache_entries: u32,
    /// Contents of `rules/resources.ron`. Attached by [`Rules::load`].
    #[serde(skip_deserializing)]
    pub resources: Resources,
    /// Every map under `maps/`, keyed by name. Attached by [`Rules::load`].
    #[serde(skip_deserializing)]
    pub maps: BTreeMap<String, MapDef>,
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

        rules.maps = map::load_dir(&dir.join("maps"))?;
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
        Ok(rules)
    }

    fn validate_match_rules(&self, path: &Path) -> Result<(), Error> {
        let positive = |field: &str, v: u32| -> Result<(), Error> {
            if v == 0 {
                Err(Error::invalid(path, field, "must be > 0"))
            } else {
                Ok(())
            }
        };
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
        positive("town_limit_base", self.town_limit_base)?;
        positive("supply_radius_tiles", self.supply_radius_tiles)?;
        positive("attrition_interval_ds", self.attrition_interval_ds)?;
        positive("annexation_ds", self.annexation_ds)?;
        positive("vision.unit_tiles", self.vision.unit_tiles)?;
        positive("vision.building_tiles", self.vision.building_tiles)?;
        positive("vision.town_tiles", self.vision.town_tiles)?;
        positive("path_budget_expansions", self.path_budget_expansions)?;
        positive("path_cache_entries", self.path_cache_entries)?;
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

    /// Yield cap for a Trade level, clamped to the last table entry.
    pub fn yield_cap(&self, trade_level: u32) -> u32 {
        let idx = (trade_level as usize).min(self.yield_cap_table.len() - 1);
        self.yield_cap_table[idx]
    }

    /// Population cap at a given Arms level.
    pub fn pop_cap(&self, arms_level: u32) -> u32 {
        self.pop_cap_base + self.pop_cap_per_arms_level * arms_level
    }

    /// The map named `name`, if it was loaded.
    pub fn map(&self, name: &str) -> Option<&MapDef> {
        self.maps.get(name)
    }

    /// Content hash of the loaded rules (match rules, resources and maps);
    /// stored in replay headers so a changed RON file is reported as
    /// `RULES CHANGED` instead of a sim divergence.
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
        assert!(rules.map("plains_1v1").is_some());
        assert_eq!(rules.ticks_from_ds(32), 64);
        assert_eq!(rules.attrition_interval_ticks(), 64);
        assert_eq!(rules.annexation_ticks(), 1200);
        assert_eq!(rules.yield_cap(0), 70);
        assert_eq!(rules.yield_cap(99), 200);
        assert_eq!(rules.pop_cap(2), 75);
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
    }

    #[test]
    fn missing_default_map_is_reported() {
        let dir = std::env::temp_dir().join(format!("eonmark-rules-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("rules")).unwrap();
        std::fs::create_dir_all(dir.join("maps")).unwrap();
        std::fs::copy(
            data_dir().join("rules/rules.ron"),
            dir.join("rules/rules.ron"),
        )
        .unwrap();
        std::fs::copy(
            data_dir().join("rules/resources.ron"),
            dir.join("rules/resources.ron"),
        )
        .unwrap();
        let err = Rules::load(&dir).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("maps") || msg.contains("default_map"), "{msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
