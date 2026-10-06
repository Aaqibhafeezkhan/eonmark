//! Schema for `data/rules/resources.ron`: the four stockpiled resources.

use crate::Error;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One stockpiled resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    /// Stable identifier used by other data files (`grain`, `lumber`, ...).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Building ids that produce this resource. Cross-checked against
    /// `buildings.ron` from M3a; until then only non-emptiness is checked.
    pub gathered_from: Vec<String>,
    /// One-sentence description of what the resource pays for.
    pub role: String,
    /// `true` if income flatlines at the Yield Cap in `rules.ron`.
    pub yield_capped: bool,
    /// Hard stockpile ceiling. Required when `yield_capped` is `false`.
    pub stockpile_cap: Option<u32>,
}

/// Contents of `data/rules/resources.ron`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    /// Resources in stockpile display order.
    pub resources: Vec<Resource>,
}

impl Resources {
    /// Validate ids, caps and references. `yield_cap_levels` is the length of
    /// the Yield Cap table in `rules.ron`, passed so a later schema can check
    /// per-resource overrides against it.
    pub fn validate(&self, path: &Path, yield_cap_levels: usize) -> Result<(), Error> {
        if self.resources.is_empty() {
            return Err(Error::invalid(
                path,
                "resources",
                "must list at least one resource",
            ));
        }
        if yield_cap_levels == 0 {
            return Err(Error::invalid(
                path,
                "resources",
                "rules.ron yield_cap_table is empty",
            ));
        }
        for (i, r) in self.resources.iter().enumerate() {
            let field = |name: &str| format!("resources[{i}].{name}");
            if r.id.is_empty() || !r.id.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                return Err(Error::invalid(
                    path,
                    &field("id"),
                    format!(
                        "{:?} must be non-empty lowercase ascii with underscores",
                        r.id
                    ),
                ));
            }
            if self.resources[..i].iter().any(|o| o.id == r.id) {
                return Err(Error::invalid(
                    path,
                    &field("id"),
                    format!("duplicate id {:?}", r.id),
                ));
            }
            if r.name.is_empty() {
                return Err(Error::invalid(path, &field("name"), "must not be empty"));
            }
            if r.gathered_from.is_empty() {
                return Err(Error::invalid(
                    path,
                    &field("gathered_from"),
                    "must name at least one building",
                ));
            }
            if r.role.is_empty() {
                return Err(Error::invalid(path, &field("role"), "must not be empty"));
            }
            match (r.yield_capped, r.stockpile_cap) {
                (false, None) => {
                    return Err(Error::invalid(
                        path,
                        &field("stockpile_cap"),
                        "required when yield_capped is false",
                    ));
                }
                (_, Some(0)) => {
                    return Err(Error::invalid(path, &field("stockpile_cap"), "must be > 0"));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Look up a resource by id.
    pub fn get(&self, id: &str) -> Option<&Resource> {
        self.resources.iter().find(|r| r.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Resources {
        Resources {
            resources: vec![
                Resource {
                    id: "grain".into(),
                    name: "Grain".into(),
                    gathered_from: vec!["town".into()],
                    role: "food".into(),
                    yield_capped: true,
                    stockpile_cap: None,
                },
                Resource {
                    id: "lore".into(),
                    name: "Lore".into(),
                    gathered_from: vec!["scriptorium".into()],
                    role: "research".into(),
                    yield_capped: false,
                    stockpile_cap: Some(999),
                },
            ],
        }
    }

    #[test]
    fn sample_is_valid() {
        sample().validate(Path::new("r.ron"), 4).unwrap();
    }

    #[test]
    fn uncapped_resource_needs_stockpile_cap() {
        let mut r = sample();
        r.resources[1].stockpile_cap = None;
        let err = r.validate(Path::new("r.ron"), 4).unwrap_err().to_string();
        assert!(
            err.contains("r.ron") && err.contains("resources[1].stockpile_cap"),
            "{err}"
        );
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let mut r = sample();
        r.resources[1].id = "grain".into();
        let err = r.validate(Path::new("r.ron"), 4).unwrap_err().to_string();
        assert!(err.contains("resources[1].id"), "{err}");
    }

    #[test]
    fn unknown_field_is_rejected_with_name() {
        let text = r#"(resources: [(id: "grain", name: "Grain", gathered_from: ["town"], role: "x", yield_capped: true, stockpile_cap: None, colour: 3)])"#;
        let err = crate::parse_ron::<Resources>(Path::new("r.ron"), text).unwrap_err();
        assert!(err.to_string().contains("colour"), "{err}");
    }
}
