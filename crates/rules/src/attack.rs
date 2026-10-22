//! The attack block shared by `units.ron` and `buildings.ron`
//! (`docs/design/combat.md`): damage per hit, weapon range, cooldown and
//! which enemies an attacker prefers. A unit kind or building kind without
//! one does not attack. Range is in hundredths of a tile and is measured
//! edge to edge; the cooldown is in deciseconds and converted once by
//! [`crate::Rules::cooldown_ticks`].

use crate::Error;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Largest base damage per hit.
pub const MAX_DAMAGE: u32 = 10_000;
/// Largest weapon range, in hundredths of a tile (16 tiles).
pub const MAX_RANGE_TILES_X100: u32 = 1600;
/// Largest cooldown, in deciseconds (one minute).
pub const MAX_COOLDOWN_DS: u32 = 600;

/// Which enemies an attacker acquires first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum Prefer {
    /// Enemy units, then enemy buildings (the default).
    #[default]
    Units,
    /// Enemy buildings, then enemy units (the Mangonel).
    Buildings,
}

/// How a kind attacks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attack {
    /// Damage per hit before Arms rows and counters, in `1..=10000`.
    pub damage: u32,
    /// Weapon range edge to edge, in tiles times 100, in `1..=1600`.
    pub range_tiles_x100: u32,
    /// Time between hits, in deciseconds, in `1..=600`.
    pub cooldown_ds: u32,
    /// Which enemies to acquire first; [`Prefer::Units`] when absent.
    #[serde(default)]
    pub prefer: Prefer,
}

/// Check the ranges of an attack named `field` (`units[i].attack`).
pub(crate) fn validate(path: &Path, field: &str, a: &Attack) -> Result<(), Error> {
    for (name, v, hi) in [
        ("damage", a.damage, MAX_DAMAGE),
        ("range_tiles_x100", a.range_tiles_x100, MAX_RANGE_TILES_X100),
        ("cooldown_ds", a.cooldown_ds, MAX_COOLDOWN_DS),
    ] {
        if !(1..=hi).contains(&v) {
            return Err(Error::invalid(
                path,
                &format!("{field}.{name}"),
                format!("{v} must be in 1..={hi}"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attack_parses_with_default_preference_and_checks_ranges() {
        let a: Attack =
            ron::from_str("(damage: 10, range_tiles_x100: 25, cooldown_ds: 10)").unwrap();
        assert_eq!(a.prefer, Prefer::Units);
        validate(Path::new("u.ron"), "units[2].attack", &a).unwrap();
        let a: Attack = ron::from_str(
            "(damage: 30, range_tiles_x100: 800, cooldown_ds: 40, prefer: Buildings)",
        )
        .unwrap();
        assert_eq!(a.prefer, Prefer::Buildings);
        for (text, field) in [
            (
                "(damage: 0, range_tiles_x100: 25, cooldown_ds: 10)",
                "damage",
            ),
            (
                "(damage: 1, range_tiles_x100: 1601, cooldown_ds: 10)",
                "range_tiles_x100",
            ),
            (
                "(damage: 1, range_tiles_x100: 25, cooldown_ds: 0)",
                "cooldown_ds",
            ),
        ] {
            let a: Attack = ron::from_str(text).unwrap();
            let e = validate(Path::new("u.ron"), "units[2].attack", &a)
                .unwrap_err()
                .to_string();
            assert!(e.contains(&format!("units[2].attack.{field}")), "{e}");
        }
        let e = ron::from_str::<Attack>(
            "(damage: 1, range_tiles_x100: 25, cooldown_ds: 10, splash: 1)",
        )
        .unwrap_err();
        assert!(e.to_string().contains("splash"), "{e}");
    }
}
