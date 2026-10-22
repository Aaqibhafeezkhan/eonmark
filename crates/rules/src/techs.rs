//! Schema for `data/rules/techs.ron`: the techs a player researches
//! (`docs/design/tech-and-ages.md`). The list index is the sim's `TechId`.
//!
//! A tech belongs to a `line` at a `level`; level k of a line requires
//! level k-1 of the same line, which must come earlier in the file. Its
//! effects are [`Modifier`] rows. The cost, the time and the host building
//! are what a `Research` command pays and where it runs. The checks that
//! need `ages.ron` (every tech is unlocked by exactly one age, after its
//! host and its predecessor) and line continuity run in `Rules::load`, after
//! the `ages.ron` references resolve.

use crate::Error;
use crate::modifiers::{Modifier, validate_rows};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Largest `research_time_ds` (one hour).
pub const MAX_RESEARCH_TIME_DS: u32 = 36_000;

/// One tech. Index in [`Techs::techs`] is the `TechId`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tech {
    /// Stable identifier other data files use (`arms_1`, ...).
    pub id: String,
    /// Display name.
    pub name: String,
    /// The line the tech belongs to (`arms`, `statecraft`, ...).
    pub line: String,
    /// Level within the line, from 1.
    pub level: u8,
    /// Building kind id that researches it (`buildings.ron`).
    pub host: String,
    /// Cost per resource id before `ResearchCost` rows. Zero entries are
    /// omitted.
    pub cost: BTreeMap<String, u32>,
    /// Research time before `ResearchTime` rows, in deciseconds.
    pub research_time_ds: u32,
    /// What researching it changes.
    pub modifiers: Vec<Modifier>,
}

/// Contents of `data/rules/techs.ron`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Techs {
    /// Techs in `TechId` order, each line's levels in order.
    pub techs: Vec<Tech>,
}

impl Techs {
    /// Validate ids, lines and levels, the host, the cost, the time and the
    /// Modifier rows against the ids already loaded. Every error names
    /// `path` and the field (`techs[i].<field>`).
    pub fn validate(
        &self,
        path: &Path,
        resource_ids: &[&str],
        unit_ids: &[&str],
        building_ids: &[&str],
    ) -> Result<(), Error> {
        if self.techs.len() > usize::from(u16::MAX) {
            return Err(Error::invalid(
                path,
                "techs",
                "more techs than TechId (u16) can index",
            ));
        }
        for (i, t) in self.techs.iter().enumerate() {
            let field = |name: &str| format!("techs[{i}].{name}");
            let earlier = &self.techs[..i];
            for (name, v) in [("id", &t.id), ("line", &t.line)] {
                if v.is_empty()
                    || !v
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                {
                    return Err(Error::invalid(
                        path,
                        &field(name),
                        format!(
                            "{v:?} must be non-empty lowercase ascii with digits and underscores"
                        ),
                    ));
                }
            }
            if earlier.iter().any(|o| o.id == t.id) {
                return Err(Error::invalid(
                    path,
                    &field("id"),
                    format!("duplicate id {:?}", t.id),
                ));
            }
            if t.name.is_empty() {
                return Err(Error::invalid(path, &field("name"), "must not be empty"));
            }
            if t.level == 0 {
                return Err(Error::invalid(path, &field("level"), "must be >= 1"));
            }
            if let Some(o) = earlier
                .iter()
                .find(|o| o.line == t.line && o.level == t.level)
            {
                return Err(Error::invalid(
                    path,
                    &field("level"),
                    format!("{:?} level {} is already {:?}", t.line, t.level, o.id),
                ));
            }
            if !building_ids.contains(&t.host.as_str()) {
                return Err(Error::invalid(
                    path,
                    &field("host"),
                    format!("{:?} is not a building id in buildings.ron", t.host),
                ));
            }
            crate::buildings::amounts(path, &field("cost"), &t.cost, resource_ids)?;
            if !(1..=MAX_RESEARCH_TIME_DS).contains(&t.research_time_ds) {
                return Err(Error::invalid(
                    path,
                    &field("research_time_ds"),
                    format!(
                        "{} must be in 1..={MAX_RESEARCH_TIME_DS}",
                        t.research_time_ds
                    ),
                ));
            }
            validate_rows(path, &field("modifiers"), &t.modifiers, unit_ids)?;
        }
        Ok(())
    }
}

/// The check on line order, run by `Rules::load` once the `ages.ron`
/// references resolve (so a removed tech is reported where `ages.ron`
/// names it): level k > 1 of a line follows level k-1 of the same line
/// earlier in the file. Errors name `techs[i].level`.
pub(crate) fn validate_lines(path: &Path, techs: &[Tech]) -> Result<(), Error> {
    for (i, t) in techs.iter().enumerate() {
        if t.level > 1
            && !techs[..i]
                .iter()
                .any(|o| o.line == t.line && o.level + 1 == t.level)
        {
            return Err(Error::invalid(
                path,
                &format!("techs[{i}].level"),
                format!(
                    "{:?} is {:?} level {}; level {} of the line must come earlier in the file",
                    t.id,
                    t.line,
                    t.level,
                    t.level - 1
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modifiers::{ModOp, StatPath};

    const RES: [&str; 4] = ["grain", "lumber", "ore", "lore"];
    const UNITS: [&str; 2] = ["yeoman", "skirmisher"];
    const BUILDINGS: [&str; 2] = ["town", "scriptorium"];

    fn tech(id: &str, line: &str, level: u8) -> Tech {
        Tech {
            id: id.into(),
            name: id.into(),
            line: line.into(),
            level,
            host: "scriptorium".into(),
            cost: BTreeMap::from([("lore".into(), 60)]),
            research_time_ds: 250,
            modifiers: vec![Modifier {
                target: StatPath::ResearchCost,
                op: ModOp::Mul,
                value: 900,
            }],
        }
    }

    fn sample() -> Techs {
        Techs {
            techs: vec![
                tech("letters_1", "letters", 1),
                tech("letters_2", "letters", 2),
            ],
        }
    }

    fn err(t: &Techs) -> String {
        t.validate(Path::new("t.ron"), &RES, &UNITS, &BUILDINGS)
            .and_then(|()| validate_lines(Path::new("t.ron"), &t.techs))
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn sample_is_valid() {
        let t = sample();
        t.validate(Path::new("t.ron"), &RES, &UNITS, &BUILDINGS)
            .unwrap();
        validate_lines(Path::new("t.ron"), &t.techs).unwrap();
    }

    #[test]
    fn fields_are_checked_with_names() {
        let mut t = sample();
        t.techs[1].id = "letters_1".into();
        assert!(err(&t).contains("techs[1].id"));
        let mut t = sample();
        t.techs[0].id = "Letters".into();
        assert!(err(&t).contains("techs[0].id"));
        let mut t = sample();
        t.techs[0].level = 0;
        assert!(err(&t).contains("techs[0].level"));
        let mut t = sample();
        t.techs[1].level = 1;
        let e = err(&t);
        assert!(
            e.contains("techs[1].level") && e.contains("letters_1"),
            "{e}"
        );
        let mut t = sample();
        t.techs[0].host = "library".into();
        assert!(err(&t).contains("techs[0].host"));
        let mut t = sample();
        t.techs[0].cost.insert("gold".into(), 1);
        assert!(err(&t).contains(r#"techs[0].cost["gold"]"#));
        let mut t = sample();
        t.techs[0].research_time_ds = 0;
        assert!(err(&t).contains("techs[0].research_time_ds"));
        let mut t = sample();
        t.techs[1].modifiers[0].value = 0;
        assert!(err(&t).contains("techs[1].modifiers[0].value"));
    }

    #[test]
    fn each_level_needs_its_predecessor_earlier_in_the_file() {
        let mut t = sample();
        t.techs.swap(0, 1);
        let e = err(&t);
        assert!(e.contains("techs[0].level") && e.contains("level 1"), "{e}");
        let mut t = sample();
        t.techs.remove(0);
        assert!(err(&t).contains("techs[0].level"));
        let mut t = sample();
        t.techs[1].level = 3;
        assert!(err(&t).contains("techs[1].level"));
    }

    #[test]
    fn unknown_field_is_rejected_with_name() {
        let text = r#"(techs: [(id: "a_1", name: "A", line: "a", level: 1, host: "town", cost: {}, research_time_ds: 1, modifiers: [], icon: 3)])"#;
        let e = crate::parse_ron::<Techs>(Path::new("t.ron"), text).unwrap_err();
        assert!(e.to_string().contains("icon"), "{e}");
    }
}
