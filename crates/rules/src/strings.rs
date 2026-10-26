//! UI strings (`data/rules/strings/<lang>.ron`, M3b): the text the HUD
//! shows for each command rejection and for its own labels. Presentation
//! only: the sim never reads them, they are not part of [`crate::Rules`]
//! and so not part of `rules_hash`, and editing them needs no
//! `rules_version` bump or fixture re-record. `sim-cli data-check` still
//! loads and validates them.
//!
//! A value may hold placeholders in braces, filled by [`fill`]: `{resource}`,
//! `{unit}`, `{building}`, `{cost}`, `{used}`, `{cap}`, `{count}`,
//! `{rate}`, `{name}`, `{key}`, and from M5a `{time}` (a match time such as
//! "12:30") and `{hash}` (a state hash), and from M5b `{seed}` (a match
//! seed, on the main menu). Any other brace is an error, so a typo in a
//! placeholder fails `data-check` instead of reaching the screen.

use crate::{Error, load_ron};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Placeholders a string may contain.
pub const PLACEHOLDERS: [&str; 13] = [
    "resource", "unit", "building", "cost", "used", "cap", "count", "rate", "name", "key", "time",
    "hash", "seed",
];

/// The UI strings for one language.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Strings {
    /// Message-line text per `RejectReason`, keyed by the reason's name in
    /// snake case (`outside_borders`, `cannot_afford`, ...).
    pub reject: BTreeMap<String, String>,
    /// HUD labels and tooltips, keyed by a dotted name (`pop`,
    /// `button.train`, ...).
    pub hud: BTreeMap<String, String>,
}

impl Strings {
    /// The path of language `lang` under the data directory.
    pub fn path(data_dir: impl AsRef<Path>, lang: &str) -> PathBuf {
        data_dir
            .as_ref()
            .join("rules")
            .join("strings")
            .join(format!("{lang}.ron"))
    }

    /// Load and validate `rules/strings/<lang>.ron` under `data_dir`.
    pub fn load(data_dir: impl AsRef<Path>, lang: &str) -> Result<Strings, Error> {
        let path = Strings::path(data_dir, lang);
        let strings: Strings = load_ron(&path)?;
        strings.validate(&path)?;
        Ok(strings)
    }

    /// Every key is a non-empty run of lower-case letters, digits, `_` and
    /// `.`; every value is non-empty and uses only known placeholders.
    pub fn validate(&self, path: &Path) -> Result<(), Error> {
        for (table, entries) in [("reject", &self.reject), ("hud", &self.hud)] {
            for (key, value) in entries {
                let field = format!("{table}[\"{key}\"]");
                let key_ok = !key.is_empty()
                    && key.chars().all(|c| {
                        c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.'
                    });
                if !key_ok {
                    return Err(Error::invalid(
                        path,
                        &field,
                        "keys use lower-case letters, digits, '_' and '.'",
                    ));
                }
                if value.trim().is_empty() {
                    return Err(Error::invalid(path, &field, "must not be empty"));
                }
                check_placeholders(value).map_err(|m| Error::invalid(path, &field, m))?;
            }
        }
        Ok(())
    }

    /// The message for a rejection, by snake-case reason name.
    pub fn reject(&self, key: &str) -> Option<&str> {
        self.reject.get(key).map(String::as_str)
    }

    /// A HUD label or tooltip, by key.
    pub fn hud(&self, key: &str) -> Option<&str> {
        self.hud.get(key).map(String::as_str)
    }
}

/// Every `{...}` in `value` names a known placeholder; braces are balanced.
fn check_placeholders(value: &str) -> Result<(), String> {
    let mut rest = value;
    while let Some(open) = rest.find(['{', '}']) {
        if rest[open..].starts_with('}') {
            return Err("unmatched '}'".into());
        }
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            return Err("unmatched '{'".into());
        };
        let name = &after[..close];
        if !PLACEHOLDERS.contains(&name) {
            return Err(format!(
                "unknown placeholder {{{name}}}; known: {}",
                PLACEHOLDERS.join(", ")
            ));
        }
        rest = &after[close + 1..];
    }
    Ok(())
}

/// Replace each `{name}` in `template` with its value from `args`;
/// placeholders without a value are left as they are.
pub fn fill(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    #[test]
    fn english_strings_load_and_name_the_m3b_messages() {
        let s = Strings::load(data_dir(), "en").expect("en.ron loads");
        assert_eq!(s.reject("outside_borders"), Some("Outside your borders"));
        assert_eq!(s.reject("pop_cap_reached"), Some("Population cap reached"));
        assert_eq!(
            fill(s.reject("cannot_afford").unwrap(), &[("resource", "Ore")]),
            "Not enough Ore"
        );
    }

    #[test]
    fn placeholders_are_checked() {
        assert!(check_placeholders("Not enough {resource}").is_ok());
        assert!(check_placeholders("{used} / {cap}").is_ok());
        assert!(check_placeholders("plain").is_ok());
        assert!(check_placeholders("{time}, tick {count}").is_ok());
        assert!(check_placeholders("Final hash {hash}").is_ok());
        assert!(check_placeholders("Seed {seed}").is_ok());
        assert!(check_placeholders("Not enough {amount}").is_err());
        assert!(check_placeholders("open { brace").is_err());
        assert!(check_placeholders("close } brace").is_err());
    }

    #[test]
    fn a_bad_key_or_empty_value_names_the_field() {
        let path = Path::new("en.ron");
        let mut s = Strings {
            reject: BTreeMap::from([("Outside".to_string(), "x".to_string())]),
            hud: BTreeMap::new(),
        };
        let e = s.validate(path).unwrap_err().to_string();
        assert!(e.contains("reject[\"Outside\"]"), "{e}");
        s.reject = BTreeMap::from([("outside".to_string(), " ".to_string())]);
        let e = s.validate(path).unwrap_err().to_string();
        assert!(e.contains("must not be empty"), "{e}");
    }

    #[test]
    fn fill_replaces_known_placeholders_only() {
        assert_eq!(
            fill("{used} / {cap}", &[("used", "25"), ("cap", "25")]),
            "25 / 25"
        );
        assert_eq!(fill("Train {unit}", &[]), "Train {unit}");
    }
}
