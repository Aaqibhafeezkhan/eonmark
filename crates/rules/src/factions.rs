//! Schema for `data/rules/factions.ron`: the factions and their Modifier
//! rows (`docs/design/tech-and-ages.md`). A faction's rows apply to its
//! players from the start, before any tech's. v0.1 ships one faction,
//! Freeholders, with no rows; until M9 adds a faction choice to the match
//! setup every player uses `factions[0]`.

use crate::Error;
use crate::modifiers::{Modifier, validate_rows};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Most factions a ruleset may have, so a faction index fits a `u8`.
pub const MAX_FACTIONS: usize = 255;

/// One faction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Faction {
    /// Stable identifier (`freeholders`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Rows that apply to the faction's players from the start; empty when
    /// absent.
    #[serde(default)]
    pub modifiers: Vec<Modifier>,
}

/// Contents of `data/rules/factions.ron`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Factions {
    /// Factions; the first is the default.
    pub factions: Vec<Faction>,
}

impl Factions {
    /// Validate ids, names and Modifier rows (`unit_ids` from `units.ron`).
    /// Every error names `path` and the field (`factions[i].<field>`).
    pub fn validate(&self, path: &Path, unit_ids: &[&str]) -> Result<(), Error> {
        if self.factions.is_empty() || self.factions.len() > MAX_FACTIONS {
            return Err(Error::invalid(
                path,
                "factions",
                format!(
                    "must list 1..={MAX_FACTIONS} factions (found {})",
                    self.factions.len()
                ),
            ));
        }
        for (i, f) in self.factions.iter().enumerate() {
            let field = |name: &str| format!("factions[{i}].{name}");
            if f.id.is_empty() || !f.id.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                return Err(Error::invalid(
                    path,
                    &field("id"),
                    format!(
                        "{:?} must be non-empty lowercase ascii with underscores",
                        f.id
                    ),
                ));
            }
            if self.factions[..i].iter().any(|o| o.id == f.id) {
                return Err(Error::invalid(
                    path,
                    &field("id"),
                    format!("duplicate id {:?}", f.id),
                ));
            }
            if f.name.is_empty() {
                return Err(Error::invalid(path, &field("name"), "must not be empty"));
            }
            validate_rows(path, &field("modifiers"), &f.modifiers, unit_ids)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modifiers::{ModOp, StatPath};

    fn sample() -> Factions {
        crate::parse_ron(
            Path::new("f.ron"),
            r#"(factions: [(id: "freeholders", name: "Freeholders", modifiers: []), (id: "tidewater", name: "Tidewater League", modifiers: [(target: YieldCap, op: Mul, value: 1250), (target: TownBorderRadius, op: Add, value: -1)])])"#,
        )
        .unwrap()
    }

    #[test]
    fn sample_is_valid() {
        let f = sample();
        f.validate(Path::new("f.ron"), &["yeoman"]).unwrap();
        assert_eq!(
            f.factions[1].modifiers[0],
            Modifier {
                target: StatPath::YieldCap,
                op: ModOp::Mul,
                value: 1250
            }
        );
    }

    #[test]
    fn fields_are_checked_with_names() {
        let err = |f: &Factions| {
            f.validate(Path::new("f.ron"), &["yeoman"])
                .unwrap_err()
                .to_string()
        };
        let mut f = sample();
        f.factions[1].id = "freeholders".into();
        assert!(err(&f).contains("factions[1].id"));
        let mut f = sample();
        f.factions[1].modifiers[0].value = 0;
        assert!(err(&f).contains("factions[1].modifiers[0].value"));
        let mut f = sample();
        f.factions.clear();
        assert!(err(&f).contains("factions"));
        let mut f = sample();
        f.factions[0].name.clear();
        assert!(err(&f).contains("factions[0].name"));
    }
}
