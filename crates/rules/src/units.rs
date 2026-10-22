//! Schema for `data/rules/units.ron`: the unit kinds, their movement stats
//! and how they are trained.
//!
//! `yeoman` stays at index 0 (scenarios and tests spawn `UnitKindId(0)`).
//! M3a adds the class, cost, cost ramp, pop, training time, trainer
//! building, builder flag and vision radius; M4a adds hit points, the
//! attack, the optional `requires_tech` and the top-level counter table.
//! Every value is an integer; fields ending in `_x100` are scaled by 100 so
//! that `180` means `1.80`. The sim converts them to fixed-point exactly
//! once at spawn time. `trainer` and `requires_tech` are checked against
//! `buildings.ron` and `techs.ron` in `Rules::load`, which loads this file
//! first.

use crate::Error;
use crate::attack::Attack;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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
    /// Hit points before Arms rows, in `1..=MAX_HP`.
    pub hp: u32,
    /// How the kind attacks; `None` (the default) for a kind that never
    /// attacks.
    #[serde(default)]
    pub attack: Option<Attack>,
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
    /// A tech id (`techs.ron`) the owner must have researched before
    /// training the kind, on top of the age that unlocks it; `None` (the
    /// default) when the age alone gates it.
    #[serde(default)]
    pub requires_tech: Option<String>,
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

/// The defender side of a counter row.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Defender {
    /// Units of the kind with this id.
    Kind(String),
    /// Every building kind, sites included.
    Buildings,
}

/// One row of the counter table: hits by `attacker` on `defender` are
/// scaled by `permille`. A pair that is not listed is 1000 (x1.0).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counter {
    /// Unit kind id of the attacker; the kind must have an attack.
    pub attacker: String,
    /// What it hits.
    pub defender: Defender,
    /// Damage multiplier in permille, in `1..=10000`.
    pub permille: u32,
}

/// Contents of `data/rules/units.ron`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Units {
    /// Unit kinds in `UnitKindId` order.
    pub units: Vec<UnitKind>,
    /// The counter table; empty when absent.
    #[serde(default)]
    pub counters: Vec<Counter>,
}

/// Upper bound on every `_x100` field so the sim's fixed-point conversion
/// (`value / (100 * tick_rate_hz)`) never leaves `i32` range.
pub const MAX_X100: u32 = 100_000;
/// Largest `pop` of one unit.
pub const MAX_POP: u32 = 100;
/// Largest `train_time_ds` (one hour).
pub const MAX_TRAIN_TIME_DS: u32 = 36_000;
/// Largest `radius_tiles_x100`: movement's neighbour query radius grows with
/// the largest radius of any kind, so this bounds its cost.
pub const MAX_UNIT_RADIUS_X100: u32 = 50;
/// Largest counter multiplier in permille (x10).
pub const MAX_COUNTER_PERMILLE: u32 = 10_000;

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
            if u.radius_tiles_x100 > MAX_UNIT_RADIUS_X100 {
                return Err(Error::invalid(
                    path,
                    &field("radius_tiles_x100"),
                    format!("{} must be <= {MAX_UNIT_RADIUS_X100}", u.radius_tiles_x100),
                ));
            }
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
            validate_combat(path, i, u)?;
        }
        self.validate_counters(path)
    }

    /// The counter table: the attacker is a kind with an attack, a `Kind`
    /// defender is a kind, each pair is listed once and `permille` is in
    /// `1..=10000`. Errors name `counters[j].<field>`.
    fn validate_counters(&self, path: &Path) -> Result<(), Error> {
        let mut pairs = BTreeSet::new();
        for (j, c) in self.counters.iter().enumerate() {
            let field = |name: &str| format!("counters[{j}].{name}");
            let Some(attacker) = self.get(&c.attacker) else {
                return Err(Error::invalid(
                    path,
                    &field("attacker"),
                    format!("{:?} is not a unit id", c.attacker),
                ));
            };
            if attacker.attack.is_none() {
                return Err(Error::invalid(
                    path,
                    &field("attacker"),
                    format!("{:?} has no attack", c.attacker),
                ));
            }
            if let Defender::Kind(id) = &c.defender
                && self.get(id).is_none()
            {
                return Err(Error::invalid(
                    path,
                    &field("defender"),
                    format!("{id:?} is not a unit id"),
                ));
            }
            if !pairs.insert((c.attacker.as_str(), &c.defender)) {
                return Err(Error::invalid(
                    path,
                    &field("defender"),
                    format!("{:?} against {:?} is listed twice", c.attacker, c.defender),
                ));
            }
            if !(1..=MAX_COUNTER_PERMILLE).contains(&c.permille) {
                return Err(Error::invalid(
                    path,
                    &field("permille"),
                    format!("{} must be in 1..={MAX_COUNTER_PERMILLE}", c.permille),
                ));
            }
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

/// The M4a combat fields of `units[i]`: `hp` in `1..=MAX_HP`, the attack's
/// ranges, a non-empty `requires_tech`. The tech itself is resolved by
/// `Rules::load` once `techs.ron` is loaded.
fn validate_combat(path: &Path, i: usize, u: &UnitKind) -> Result<(), Error> {
    let field = |name: &str| format!("units[{i}].{name}");
    if !(1..=crate::buildings::MAX_HP).contains(&u.hp) {
        return Err(Error::invalid(
            path,
            &field("hp"),
            format!("{} must be in 1..={}", u.hp, crate::buildings::MAX_HP),
        ));
    }
    if let Some(a) = &u.attack {
        crate::attack::validate(path, &field("attack"), a)?;
    }
    if u.requires_tech.as_deref() == Some("") {
        return Err(Error::invalid(
            path,
            &field("requires_tech"),
            "must not be empty; omit it or use None",
        ));
    }
    Ok(())
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
            hp: 40,
            attack: None,
            cost: BTreeMap::from([("grain".into(), 20)]),
            ramp: UnitRamp::PerUnit(BTreeMap::from([("grain".into(), 1)])),
            pop: 1,
            train_time_ds: 120,
            trainer: "town".into(),
            requires_tech: None,
            builder: true,
            vision_tiles: 8,
        }
    }

    fn skirmisher() -> UnitKind {
        UnitKind {
            id: "skirmisher".into(),
            name: "Skirmisher".into(),
            class: UnitClass::Infantry,
            hp: 80,
            attack: Some(Attack {
                damage: 10,
                range_tiles_x100: 25,
                cooldown_ds: 10,
                prefer: crate::Prefer::Units,
            }),
            cost: BTreeMap::from([("grain".into(), 40)]),
            ramp: UnitRamp::Military,
            trainer: "muster_hall".into(),
            builder: false,
            ..yeoman()
        }
    }

    const RES: [&str; 4] = ["grain", "lumber", "ore", "lore"];

    /// The movement and training fields every RON sample below shares.
    const FIELDS: &str = r#"id: "yeoman", name: "Yeoman", speed_tiles_per_s_x100: 180, radius_tiles_x100: 35, arrive_radius_tiles_x100: 25, arrive_slowdown_radius_tiles_x100: 50, waypoint_radius_tiles_x100: 50, separation_tiles_x100: 10, class: Civilian, hp: 40, cost: {"grain": 20}, pop: 1, train_time_ds: 120, trainer: "town", vision_tiles: 8"#;

    fn sample() -> Units {
        Units {
            units: vec![yeoman()],
            counters: Vec::new(),
        }
    }

    /// A Yeoman and a Skirmisher with a counter row against the Yeoman.
    fn armed() -> Units {
        Units {
            units: vec![yeoman(), skirmisher()],
            counters: vec![Counter {
                attacker: "skirmisher".into(),
                defender: Defender::Kind("yeoman".into()),
                permille: 1500,
            }],
        }
    }

    fn err(u: &Units) -> String {
        u.validate(Path::new("u.ron"), &RES)
            .unwrap_err()
            .to_string()
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
        assert_eq!(u.units[0].attack, None);
        assert_eq!(u.units[0].requires_tech, None);
        assert!(u.counters.is_empty());
        u.validate(Path::new("u.ron"), &RES).unwrap();
    }

    #[test]
    fn hp_is_required() {
        let text = format!("(units: [({})])", FIELDS.replace("hp: 40, ", ""));
        let e = crate::parse_ron::<Units>(Path::new("u.ron"), &text).unwrap_err();
        assert!(e.to_string().contains("hp"), "{e}");
    }

    #[test]
    fn combat_fields_are_checked_with_field_names() {
        armed().validate(Path::new("u.ron"), &RES).unwrap();
        let mut u = armed();
        u.units[0].hp = 0;
        assert!(err(&u).contains("units[0].hp"));
        let mut u = armed();
        u.units[0].hp = crate::buildings::MAX_HP + 1;
        assert!(err(&u).contains("units[0].hp"));
        let mut u = armed();
        u.units[1].attack.as_mut().unwrap().range_tiles_x100 = 0;
        assert!(err(&u).contains("units[1].attack.range_tiles_x100"));
        let mut u = armed();
        u.units[1].radius_tiles_x100 = MAX_UNIT_RADIUS_X100 + 1;
        assert!(err(&u).contains("units[1].radius_tiles_x100"));
        let mut u = armed();
        u.units[1].requires_tech = Some(String::new());
        assert!(err(&u).contains("units[1].requires_tech"));
    }

    #[test]
    fn counter_rows_are_checked_with_field_names() {
        let mut u = armed();
        u.counters[0].attacker = "yeoman".into();
        let e = err(&u);
        assert!(
            e.contains("counters[0].attacker") && e.contains("no attack"),
            "{e}"
        );
        let mut u = armed();
        u.counters[0].attacker = "dragon".into();
        assert!(err(&u).contains("counters[0].attacker"));
        let mut u = armed();
        u.counters[0].defender = Defender::Kind("dragon".into());
        assert!(err(&u).contains("counters[0].defender"));
        let mut u = armed();
        u.counters[0].permille = 0;
        assert!(err(&u).contains("counters[0].permille"));
        let mut u = armed();
        u.counters[0].permille = MAX_COUNTER_PERMILLE + 1;
        assert!(err(&u).contains("counters[0].permille"));
        let mut u = armed();
        u.counters.push(u.counters[0].clone());
        let e = err(&u);
        assert!(
            e.contains("counters[1].defender") && e.contains("twice"),
            "{e}"
        );
        let mut u = armed();
        u.counters.push(Counter {
            attacker: "skirmisher".into(),
            defender: Defender::Buildings,
            permille: 250,
        });
        u.validate(Path::new("u.ron"), &RES).unwrap();
        let text = format!(
            r#"(units: [({FIELDS})], counters: [(attacker: "yeoman", defender: Buildings, permille: 250)])"#
        );
        let parsed = crate::parse_ron::<Units>(Path::new("u.ron"), &text).unwrap();
        assert_eq!(parsed.counters[0].defender, Defender::Buildings);
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
        let text = format!("(units: [({FIELDS}, armour: 3)])");
        let err = crate::parse_ron::<Units>(Path::new("u.ron"), &text).unwrap_err();
        assert!(err.to_string().contains("armour"), "{err}");
    }
}
