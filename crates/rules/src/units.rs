//! Schema for `data/rules/units.ron`: the unit kinds, their movement stats
//! and how they are trained.
//!
//! `yeoman` stays at index 0 (scenarios and tests spawn `UnitKindId(0)`).
//! M3a adds the class, cost, cost ramp, pop, training time, trainer
//! building, builder flag and vision radius; combat stats arrive in M4a.
//! Every value is an integer; fields ending in `_x100` are scaled by 100 so
//! that `180` means `1.80`. The sim converts them to fixed-point exactly
//! once at spawn time. `trainer` is checked against `buildings.ron` in
//! `Rules::load`, which loads this file first.

use crate::Error;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// What a unit kind is for. Civilians gather and build; the other classes
/// are military and share the military cost ramp of their trainer.
/// Infantry and cavalry also start annexation (M5a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum UnitClass {
    /// Yeoman, Scribe.
    Civilian,
    /// Skirmisher, Shieldbearer, Bowman.
    Infantry,
    /// Outrider.
    Cavalry,
    /// Mangonel.
    Siege,
    /// Supply Wain.
    Support,
}

/// How a unit kind's cost grows with what the player already has.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum UnitRamp {
    /// Always the base cost.
    #[default]
    Flat,
    /// Base cost plus these amounts per unit of this kind alive or queued
    /// (the Yeoman's +1 Grain).
    PerUnit(BTreeMap<String, u32>),
    /// The base cost times the triangular military multiplier of the
    /// trainer kind's shared queue counter (`military_ramp_*` in
    /// `rules.ron`). Only for non-civilian classes.
    Military,
}

/// One unit kind. Index in [`Units::units`] is the `UnitKindId`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitKind {
    /// Stable identifier other data files use (`yeoman`, ...).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Movement speed in tiles per second, times 100 (`180` is 1.8 tiles/s).
    pub speed_tiles_per_s_x100: u32,
    /// Collision radius in tiles, times 100 (`35` is 0.35 tiles).
    pub radius_tiles_x100: u32,
    /// A unit has arrived when it is within this radius of its final
    /// waypoint, in tiles times 100.
    pub arrive_radius_tiles_x100: u32,
    /// Arrival steering slows the unit linearly inside this radius around its
    /// final waypoint, in tiles times 100. At least `arrive_radius_tiles_x100`.
    pub arrive_slowdown_radius_tiles_x100: u32,
    /// A non-final waypoint counts as reached within this radius, in tiles
    /// times 100 (`50` is half a tile).
    pub waypoint_radius_tiles_x100: u32,
    /// Extra margin added to the sum of two radii before separation pushes
    /// neighbours apart, in tiles times 100.
    pub separation_tiles_x100: u32,
    /// An arrived unit that is pushed farther than this from its post (the
    /// tile centre it arrived at) walks straight back to it, in tiles times
    /// 100 (`100` is one tile). At least `arrive_radius_tiles_x100`; `100`
    /// when the field is absent.
    #[serde(default = "default_return_to_post_radius_tiles_x100")]
    pub return_to_post_radius_tiles_x100: u32,
    /// What the kind is for; non-civilians may use [`UnitRamp::Military`].
    pub class: UnitClass,
    /// Base training cost per resource id. Zero entries are omitted.
    pub cost: BTreeMap<String, u32>,
    /// How the cost grows; [`UnitRamp::Flat`] when absent.
    #[serde(default)]
    pub ramp: UnitRamp,
    /// Population the unit takes from the cap, charged when it is queued.
    pub pop: u32,
    /// Training time at the trainer, in deciseconds.
    pub train_time_ds: u32,
    /// Building kind id that trains this kind (`buildings.ron`).
    pub trainer: String,
    /// Whether the kind can construct buildings (`Build`).
    #[serde(default)]
    pub builder: bool,
    /// Vision radius in tiles (fog of war, M7).
    pub vision_tiles: u32,
}

/// Default for [`UnitKind::return_to_post_radius_tiles_x100`]: one tile.
fn default_return_to_post_radius_tiles_x100() -> u32 {
    100
}

/// Contents of `data/rules/units.ron`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Units {
    /// Unit kinds in `UnitKindId` order.
    pub units: Vec<UnitKind>,
}

/// Upper bound on every `_x100` field so the sim's fixed-point conversion
/// (`value / (100 * tick_rate_hz)`) never leaves `i32` range.
pub const MAX_X100: u32 = 100_000;
/// Largest `pop` of one unit.
pub const MAX_POP: u32 = 100;
/// Largest `train_time_ds` (one hour).
pub const MAX_TRAIN_TIME_DS: u32 = 36_000;

impl Units {
    /// Validate ids, ranges and resource ids in costs (`resource_ids` from
    /// `resources.ron`). Every error names `path` and the field. The
    /// trainer is checked by `Rules::load` once `buildings.ron` is loaded.
    pub fn validate(&self, path: &Path, resource_ids: &[&str]) -> Result<(), Error> {
        if self.units.is_empty() {
            return Err(Error::invalid(
                path,
                "units",
                "must list at least one unit kind",
            ));
        }
        if self.units.len() > usize::from(u16::MAX) {
            return Err(Error::invalid(
                path,
                "units",
                "more kinds than UnitKindId (u16) can index",
            ));
        }
        for (i, u) in self.units.iter().enumerate() {
            let field = |name: &str| format!("units[{i}].{name}");
            if u.id.is_empty() || !u.id.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                return Err(Error::invalid(
                    path,
                    &field("id"),
                    format!(
                        "{:?} must be non-empty lowercase ascii with underscores",
                        u.id
                    ),
                ));
            }
            if self.units[..i].iter().any(|o| o.id == u.id) {
                return Err(Error::invalid(
                    path,
                    &field("id"),
                    format!("duplicate id {:?}", u.id),
                ));
            }
            if u.name.is_empty() {
                return Err(Error::invalid(path, &field("name"), "must not be empty"));
            }
            let positive = |name: &str, v: u32| -> Result<(), Error> {
                if v == 0 {
                    return Err(Error::invalid(path, &field(name), "must be > 0"));
                }
                if v > MAX_X100 {
                    return Err(Error::invalid(
                        path,
                        &field(name),
                        format!("must be <= {MAX_X100}"),
                    ));
                }
                Ok(())
            };
            positive("speed_tiles_per_s_x100", u.speed_tiles_per_s_x100)?;
            positive("radius_tiles_x100", u.radius_tiles_x100)?;
            positive("waypoint_radius_tiles_x100", u.waypoint_radius_tiles_x100)?;
            let bounded = |name: &str, v: u32| -> Result<(), Error> {
                if v > MAX_X100 {
                    return Err(Error::invalid(
                        path,
                        &field(name),
                        format!("must be <= {MAX_X100}"),
                    ));
                }
                Ok(())
            };
            bounded("arrive_radius_tiles_x100", u.arrive_radius_tiles_x100)?;
            bounded(
                "arrive_slowdown_radius_tiles_x100",
                u.arrive_slowdown_radius_tiles_x100,
            )?;
            if u.arrive_slowdown_radius_tiles_x100 < u.arrive_radius_tiles_x100 {
                return Err(Error::invalid(
                    path,
                    &field("arrive_slowdown_radius_tiles_x100"),
                    "must be >= arrive_radius_tiles_x100",
                ));
            }
            bounded("separation_tiles_x100", u.separation_tiles_x100)?;
            positive(
                "return_to_post_radius_tiles_x100",
                u.return_to_post_radius_tiles_x100,
            )?;
            if u.return_to_post_radius_tiles_x100 < u.arrive_radius_tiles_x100 {
                return Err(Error::invalid(
                    path,
                    &field("return_to_post_radius_tiles_x100"),
                    "must be >= arrive_radius_tiles_x100",
                ));
            }
            validate_training(path, i, u, resource_ids)?;
        }
        Ok(())
    }

    /// Look up a kind by id string.
    pub fn get(&self, id: &str) -> Option<&UnitKind> {
        self.units.iter().find(|u| u.id == id)
    }

    /// Look up a kind by index (`UnitKindId.0`).
    pub fn kind(&self, index: u16) -> Option<&UnitKind> {
        self.units.get(usize::from(index))
    }
}

/// The M3a training fields of `units[i]`.
fn validate_training(
    path: &Path,
    i: usize,
    u: &UnitKind,
    resource_ids: &[&str],
) -> Result<(), Error> {
    let field = |name: &str| format!("units[{i}].{name}");
    let in_range = |name: &str, v: u32, lo: u32, hi: u32| -> Result<(), Error> {
        if (lo..=hi).contains(&v) {
            Ok(())
        } else {
            Err(Error::invalid(
                path,
                &field(name),
                format!("{v} must be in {lo}..={hi}"),
            ))
        }
    };
    crate::buildings::amounts(path, &field("cost"), &u.cost, resource_ids)?;
    match &u.ramp {
        UnitRamp::Flat => {}
        UnitRamp::PerUnit(step) => {
            crate::buildings::amounts(path, &field("ramp"), step, resource_ids)?;
        }
        UnitRamp::Military => {
            if u.class == UnitClass::Civilian {
                return Err(Error::invalid(
                    path,
                    &field("ramp"),
                    "Military is only for non-civilian classes",
                ));
            }
        }
    }
    in_range("pop", u.pop, 0, MAX_POP)?;
    in_range("train_time_ds", u.train_time_ds, 1, MAX_TRAIN_TIME_DS)?;
    in_range(
        "vision_tiles",
        u.vision_tiles,
        1,
        crate::buildings::MAX_RADIUS_TILES,
    )?;
    if u.trainer.is_empty() {
        return Err(Error::invalid(path, &field("trainer"), "must not be empty"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yeoman() -> UnitKind {
        UnitKind {
            id: "yeoman".into(),
            name: "Yeoman".into(),
            speed_tiles_per_s_x100: 180,
            radius_tiles_x100: 35,
            arrive_radius_tiles_x100: 25,
            arrive_slowdown_radius_tiles_x100: 50,
            waypoint_radius_tiles_x100: 50,
            separation_tiles_x100: 10,
            return_to_post_radius_tiles_x100: 100,
            class: UnitClass::Civilian,
            cost: BTreeMap::from([("grain".into(), 20)]),
            ramp: UnitRamp::PerUnit(BTreeMap::from([("grain".into(), 1)])),
            pop: 1,
            train_time_ds: 120,
            trainer: "town".into(),
            builder: true,
            vision_tiles: 8,
        }
    }

    const RES: [&str; 4] = ["grain", "lumber", "ore", "lore"];

    /// The movement and training fields every RON sample below shares.
    const FIELDS: &str = r#"id: "yeoman", name: "Yeoman", speed_tiles_per_s_x100: 180, radius_tiles_x100: 35, arrive_radius_tiles_x100: 25, arrive_slowdown_radius_tiles_x100: 50, waypoint_radius_tiles_x100: 50, separation_tiles_x100: 10, class: Civilian, cost: {"grain": 20}, pop: 1, train_time_ds: 120, trainer: "town", vision_tiles: 8"#;

    fn sample() -> Units {
        Units {
            units: vec![yeoman()],
        }
    }

    #[test]
    fn sample_is_valid() {
        sample().validate(Path::new("u.ron"), &RES).unwrap();
        assert_eq!(sample().kind(0).unwrap().id, "yeoman");
        assert!(sample().kind(1).is_none());
        assert!(sample().get("yeoman").is_some());
    }

    #[test]
    fn zero_speed_and_radius_are_rejected_with_field() {
        let mut u = sample();
        u.units[0].speed_tiles_per_s_x100 = 0;
        let err = u
            .validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("u.ron") && err.contains("units[0].speed_tiles_per_s_x100"),
            "{err}"
        );
        let mut u = sample();
        u.units[0].radius_tiles_x100 = 0;
        let err = u
            .validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string();
        assert!(err.contains("units[0].radius_tiles_x100"), "{err}");
        let mut u = sample();
        u.units[0].separation_tiles_x100 = MAX_X100 + 1;
        let err = u
            .validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string();
        assert!(err.contains("units[0].separation_tiles_x100"), "{err}");
        let mut u = sample();
        u.units[0].waypoint_radius_tiles_x100 = 0;
        let err = u
            .validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string();
        assert!(err.contains("units[0].waypoint_radius_tiles_x100"), "{err}");
        let mut u = sample();
        u.units[0].arrive_slowdown_radius_tiles_x100 = 24;
        let err = u
            .validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("units[0].arrive_slowdown_radius_tiles_x100"),
            "{err}"
        );
        let mut u = sample();
        u.units[0].return_to_post_radius_tiles_x100 = 0;
        let err = u
            .validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("units[0].return_to_post_radius_tiles_x100"),
            "{err}"
        );
        let mut u = sample();
        u.units[0].return_to_post_radius_tiles_x100 = 24;
        let err = u
            .validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("units[0].return_to_post_radius_tiles_x100")
                && err.contains(">= arrive_radius_tiles_x100"),
            "{err}"
        );
    }

    #[test]
    fn optional_fields_default() {
        let text = format!("(units: [({FIELDS})])");
        let u = crate::parse_ron::<Units>(Path::new("u.ron"), &text).unwrap();
        assert_eq!(u.units[0].return_to_post_radius_tiles_x100, 100);
        assert_eq!(u.units[0].ramp, UnitRamp::Flat);
        assert!(!u.units[0].builder);
        u.validate(Path::new("u.ron"), &RES).unwrap();
    }

    #[test]
    fn ramps_parse_in_every_shape() {
        for (ramp, want) in [
            ("Flat", UnitRamp::Flat),
            (
                r#"PerUnit({"grain": 1})"#,
                UnitRamp::PerUnit(BTreeMap::from([("grain".into(), 1)])),
            ),
            ("Military", UnitRamp::Military),
        ] {
            let text = format!("(units: [({FIELDS}, ramp: {ramp})])");
            let u = crate::parse_ron::<Units>(Path::new("u.ron"), &text).unwrap();
            assert_eq!(u.units[0].ramp, want);
        }
    }

    #[test]
    fn training_fields_are_checked_with_field_names() {
        let mut u = sample();
        u.units[0].ramp = UnitRamp::Military;
        let err = u
            .validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("units[0].ramp") && err.contains("non-civilian"),
            "{err}"
        );
        let mut u = sample();
        u.units[0].class = UnitClass::Infantry;
        u.units[0].ramp = UnitRamp::Military;
        u.validate(Path::new("u.ron"), &RES).unwrap();
        let mut u = sample();
        u.units[0].cost.insert("gold".into(), 1);
        let err = u
            .validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string();
        assert!(err.contains(r#"units[0].cost["gold"]"#), "{err}");
        let mut u = sample();
        u.units[0].ramp = UnitRamp::PerUnit(BTreeMap::from([("grain".into(), 0)]));
        let err = u
            .validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string();
        assert!(err.contains(r#"units[0].ramp["grain"]"#), "{err}");
        let mut u = sample();
        u.units[0].train_time_ds = 0;
        let err = u
            .validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string();
        assert!(err.contains("units[0].train_time_ds"), "{err}");
        let mut u = sample();
        u.units[0].pop = MAX_POP + 1;
        let err = u
            .validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string();
        assert!(err.contains("units[0].pop"), "{err}");
        let mut u = sample();
        u.units[0].vision_tiles = 0;
        let err = u
            .validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string();
        assert!(err.contains("units[0].vision_tiles"), "{err}");
    }

    #[test]
    fn duplicate_and_empty_ids_are_rejected() {
        let mut u = sample();
        u.units.push(yeoman());
        let err = u
            .validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string();
        assert!(err.contains("units[1].id"), "{err}");
        let mut u = sample();
        u.units[0].id = "Yeoman".into();
        let err = u
            .validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string();
        assert!(err.contains("units[0].id"), "{err}");
        let u = Units::default();
        let err = u
            .validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string();
        assert!(err.contains("units"), "{err}");
    }

    #[test]
    fn unknown_field_is_rejected_with_name() {
        let text = format!("(units: [({FIELDS}, hp: 3)])");
        let err = crate::parse_ron::<Units>(Path::new("u.ron"), &text).unwrap_err();
        assert!(err.to_string().contains("hp"), "{err}");
    }
}
