//! Schema for `data/rules/ages.ron`: the ages a player advances through
//! (`docs/design/tech-and-ages.md`, "Ages"). The list index is the age a
//! player is in; every player starts in `ages[0]`.
//!
//! An age is entered with `AdvanceAge` once the player has researched
//! `techs_required` techs and can pay `cost`. Its `unlocks` lists the unit
//! kinds, building kinds and techs that become available in it: every id
//! in `units.ron`, `buildings.ron` and `techs.ron` is listed by exactly one
//! age. The checks that need those files run in `Rules::load`.

use crate::Error;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Most ages a ruleset may have, so an age index fits a `u8`.
pub const MAX_AGES: usize = 255;

/// What an age makes available. Each list is empty when absent.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unlocks {
    /// Unit kind ids (`units.ron`).
    #[serde(default)]
    pub units: Vec<String>,
    /// Building kind ids (`buildings.ron`).
    #[serde(default)]
    pub buildings: Vec<String>,
    /// Tech ids (`techs.ron`).
    #[serde(default)]
    pub techs: Vec<String>,
}

/// One age. Index in [`Ages::ages`] is the age number (0 is the start).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Age {
    /// Stable identifier (`hearth`, `masonry`, `charter`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Researched techs needed to advance into this age (queued ones do not
    /// count); 0 for the first age.
    pub techs_required: u32,
    /// Cost of advancing into this age per resource id; empty for the
    /// first age and when absent. Zero entries are omitted.
    #[serde(default)]
    pub cost: BTreeMap<String, u32>,
    /// What becomes available in this age.
    #[serde(default)]
    pub unlocks: Unlocks,
}

/// Contents of `data/rules/ages.ron`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ages {
    /// Ages in order.
    pub ages: Vec<Age>,
}

impl Ages {
    /// Validate the list on its own: 1 to 255 ages, unique lowercase ids,
    /// a free first age, gates that never decrease from one age to the
    /// next (the tech count and every resource, a missing one counting as
    /// 0) and resource ids in costs. Every error names `path` and the field
    /// (`ages[i].<field>`).
    pub fn validate(&self, path: &Path, resource_ids: &[&str]) -> Result<(), Error> {
        if self.ages.is_empty() || self.ages.len() > MAX_AGES {
            return Err(Error::invalid(
                path,
                "ages",
                format!("must list 1..={MAX_AGES} ages (found {})", self.ages.len()),
            ));
        }
        for (i, a) in self.ages.iter().enumerate() {
            let field = |name: &str| format!("ages[{i}].{name}");
            if a.id.is_empty() || !a.id.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                return Err(Error::invalid(
                    path,
                    &field("id"),
                    format!(
                        "{:?} must be non-empty lowercase ascii with underscores",
                        a.id
                    ),
                ));
            }
            if self.ages[..i].iter().any(|o| o.id == a.id) {
                return Err(Error::invalid(
                    path,
                    &field("id"),
                    format!("duplicate id {:?}", a.id),
                ));
            }
            if a.name.is_empty() {
                return Err(Error::invalid(path, &field("name"), "must not be empty"));
            }
            crate::buildings::amounts(path, &field("cost"), &a.cost, resource_ids)?;
            let Some(prev) = i.checked_sub(1).map(|p| &self.ages[p]) else {
                if a.techs_required != 0 {
                    return Err(Error::invalid(
                        path,
                        &field("techs_required"),
                        "the first age is the start: must be 0",
                    ));
                }
                if !a.cost.is_empty() {
                    return Err(Error::invalid(
                        path,
                        &field("cost"),
                        "the first age is the start: must be empty",
                    ));
                }
                continue;
            };
            if a.techs_required < prev.techs_required {
                return Err(Error::invalid(
                    path,
                    &field("techs_required"),
                    format!(
                        "{} is below the previous age's {}; gates never decrease",
                        a.techs_required, prev.techs_required
                    ),
                ));
            }
            let ids: BTreeSet<&String> = a.cost.keys().chain(prev.cost.keys()).collect();
            for id in ids {
                let (now, before) = (
                    a.cost.get(id).copied().unwrap_or(0),
                    prev.cost.get(id).copied().unwrap_or(0),
                );
                if now < before {
                    return Err(Error::invalid(
                        path,
                        &format!("ages[{i}].cost[{id:?}]"),
                        format!("{now} is below the previous age's {before}; gates never decrease"),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RES: [&str; 4] = ["grain", "lumber", "ore", "lore"];

    fn age(id: &str, techs: u32, cost: &[(&str, u32)]) -> Age {
        Age {
            id: id.into(),
            name: id.into(),
            techs_required: techs,
            cost: cost.iter().map(|&(k, v)| (k.into(), v)).collect(),
            unlocks: Unlocks::default(),
        }
    }

    fn sample() -> Ages {
        Ages {
            ages: vec![
                age("hearth", 0, &[]),
                age("masonry", 2, &[("grain", 250), ("lore", 100)]),
                age(
                    "charter",
                    5,
                    &[("grain", 500), ("lumber", 300), ("lore", 300)],
                ),
            ],
        }
    }

    fn err(a: &Ages) -> String {
        a.validate(Path::new("a.ron"), &RES)
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn sample_is_valid() {
        sample().validate(Path::new("a.ron"), &RES).unwrap();
    }

    #[test]
    fn first_age_is_free() {
        let mut a = sample();
        a.ages[0].techs_required = 1;
        assert!(err(&a).contains("ages[0].techs_required"));
        let mut a = sample();
        a.ages[0].cost.insert("grain".into(), 1);
        assert!(err(&a).contains("ages[0].cost"));
    }

    #[test]
    fn gates_never_decrease() {
        let mut a = sample();
        a.ages[2].techs_required = 1;
        assert!(err(&a).contains("ages[2].techs_required"));
        let mut a = sample();
        a.ages[2].cost.remove("lore");
        assert!(err(&a).contains(r#"ages[2].cost["lore"]"#));
        let mut a = sample();
        a.ages[2].cost.insert("grain".into(), 249);
        assert!(err(&a).contains(r#"ages[2].cost["grain"]"#));
        // Equal gates are allowed.
        let mut a = sample();
        a.ages[2].techs_required = 2;
        a.validate(Path::new("a.ron"), &RES).unwrap();
    }

    #[test]
    fn ids_counts_and_costs_are_checked() {
        let mut a = sample();
        a.ages[1].id = "hearth".into();
        assert!(err(&a).contains("ages[1].id"));
        let mut a = sample();
        a.ages[1].cost.insert("gold".into(), 1);
        assert!(err(&a).contains(r#"ages[1].cost["gold"]"#));
        assert!(err(&Ages::default()).contains("ages"));
        let a = Ages {
            ages: (0..=MAX_AGES).map(|_| age("a", 0, &[])).collect(),
        };
        assert!(err(&a).contains("1..=255"));
    }

    #[test]
    fn unlocks_default_and_reject_unknown_fields() {
        let a: Ages = crate::parse_ron(
            Path::new("a.ron"),
            r#"(ages: [(id: "hearth", name: "Hearth", techs_required: 0, unlocks: (techs: ["x_1"]))])"#,
        )
        .unwrap();
        assert!(a.ages[0].cost.is_empty() && a.ages[0].unlocks.units.is_empty());
        assert_eq!(a.ages[0].unlocks.techs, ["x_1"]);
        let e = crate::parse_ron::<Ages>(
            Path::new("a.ron"),
            r#"(ages: [(id: "hearth", name: "Hearth", techs_required: 0, unlocks: (wonders: []))])"#,
        )
        .unwrap_err();
        assert!(e.to_string().contains("wonders"), "{e}");
    }
}
