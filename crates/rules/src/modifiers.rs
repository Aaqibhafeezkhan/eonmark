//! The integer `Modifier` model every tech and faction effect is written in
//! (`docs/design/tech-and-ages.md`, "Modifier model").
//!
//! A row is `(target: StatPath, op: Set | Add | Mul, value: i32)`. `Set` and
//! `Add` values are plain amounts in the stat's own unit; `Mul` values are
//! permille (`1250` is x1.25). [`resolve`] applies the rows of one stat: the
//! last `Set` in collection order replaces the base, then every `Add` is
//! summed, then each `Mul` row is applied in order and floored,
//! `v = (v * m).div_euclid(1000)`. Collection order is the faction's rows,
//! then each researched tech in `TechId` order with its rows in file order
//! ([`crate::Rules::modifier_rows`]).
//!
//! `StatPath` and `UnitSel` are closed enums: an unknown name is a RON parse
//! error naming the file, the span and the enum.

use crate::{Error, UnitClass, UnitKind};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Permille denominator: a `Mul` of `1000` leaves the value unchanged.
pub const PERMILLE: i64 = 1000;

/// Largest `Mul` value (x100).
pub const MAX_MUL_PERMILLE: i32 = 100_000;

/// Largest magnitude of an `Add` or `Set` value.
pub const MAX_ADD_SET: i32 = 1_000_000;

/// Which unit kinds a unit stat row reaches.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum UnitSel {
    /// Every kind whose class is not `Civilian`.
    Military,
    /// Every kind of this class.
    Class(UnitClass),
    /// The one kind with this `units.ron` id.
    Kind(String),
}

impl UnitSel {
    /// Whether this selector reaches `kind`.
    pub fn selects(&self, kind: &UnitKind) -> bool {
        match self {
            UnitSel::Military => kind.class != UnitClass::Civilian,
            UnitSel::Class(c) => kind.class == *c,
            UnitSel::Kind(id) => kind.id == *id,
        }
    }
}

/// The stats a `Modifier` may change. Player-wide stats resolve once per
/// player; unit stats resolve per player and unit kind.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum StatPath {
    /// Income ceiling per yield-capped resource per 30 s; base
    /// `yield_cap_base` in `rules.ron`.
    YieldCap,
    /// Population cap; base `pop_cap_base`.
    PopCap,
    /// Towns a player may own, sites included; base `town_limit_base`.
    TownLimit,
    /// Border radius of every own Town, in tiles; base `town_radius_tiles`
    /// or `town_radius_grown_tiles` by tier.
    TownBorderRadius,
    /// Every resource of a research cost.
    ResearchCost,
    /// Research time, in deciseconds before the tick conversion.
    ResearchTime,
    /// Hit points of the selected unit kinds.
    UnitHp(UnitSel),
    /// Attack damage of the selected unit kinds.
    UnitDamage(UnitSel),
}

impl StatPath {
    /// Whether a row with this target changes a stat of a unit of `kind`:
    /// for `UnitHp` and `UnitDamage`, whether the selector reaches `kind`;
    /// always `false` for the player-wide stats.
    pub fn matches(&self, kind: &UnitKind) -> bool {
        match self {
            StatPath::UnitHp(sel) | StatPath::UnitDamage(sel) => sel.selects(kind),
            StatPath::YieldCap
            | StatPath::PopCap
            | StatPath::TownLimit
            | StatPath::TownBorderRadius
            | StatPath::ResearchCost
            | StatPath::ResearchTime => false,
        }
    }
}

/// How a row combines with the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ModOp {
    /// Replace the base (the last `Set` in collection order wins).
    Set,
    /// Add the value after any `Set`.
    Add,
    /// Multiply by the value in permille after every `Add`, flooring.
    Mul,
}

/// One effect row of a tech or a faction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modifier {
    /// The stat the row changes.
    pub target: StatPath,
    /// How it combines.
    pub op: ModOp,
    /// The amount, or the permille multiplier for `Mul`.
    pub value: i32,
}

/// Resolve one stat: `base`, then the last `Set` among `rows`, then the sum
/// of the `Add` rows, then each `Mul` row in order as
/// `v = (v * m).div_euclid(1000)`. `rows` are the rows of this stat only,
/// in collection order; the caller filters them. The math is checked: an
/// overflow panics (the validator bounds every shipped stat so it cannot).
///
/// # Panics
///
/// If an intermediate value leaves `i64`.
pub fn resolve<'a, I>(base: i64, rows: I) -> i64
where
    I: IntoIterator<Item = &'a Modifier>,
    I::IntoIter: Clone,
{
    let rows = rows.into_iter();
    let mut v = rows
        .clone()
        .filter(|m| m.op == ModOp::Set)
        .last()
        .map_or(base, |m| i64::from(m.value));
    for m in rows.clone().filter(|m| m.op == ModOp::Add) {
        v = v
            .checked_add(i64::from(m.value))
            .expect("modifier Add fits i64");
    }
    for m in rows.filter(|m| m.op == ModOp::Mul) {
        v = v
            .checked_mul(i64::from(m.value))
            .expect("modifier Mul fits i64")
            .div_euclid(PERMILLE);
    }
    v
}

/// An upper bound on `|resolve(base, s)|` for every subset `s` of `rows`
/// in any order: `|base| + max |Set| + sum |Add|`, then for each `Mul` row
/// `ceil(v * max(m, 1000) / 1000)`. `None` when the bound itself overflows.
pub(crate) fn worst_case<'a>(
    base: i64,
    rows: impl Iterator<Item = &'a Modifier> + Clone,
) -> Option<u128> {
    let abs = |v: i32| u128::from(v.unsigned_abs());
    let set = rows
        .clone()
        .filter(|m| m.op == ModOp::Set)
        .map(|m| abs(m.value))
        .max()
        .unwrap_or(0);
    let add = rows
        .clone()
        .filter(|m| m.op == ModOp::Add)
        .try_fold(0u128, |acc, m| acc.checked_add(abs(m.value)))?;
    let mut v = u128::from(base.unsigned_abs())
        .checked_add(set)?
        .checked_add(add)?;
    for m in rows.filter(|m| m.op == ModOp::Mul) {
        let f = abs(m.value.max(1000));
        v = v.checked_mul(f)?.div_ceil(1000);
    }
    Some(v)
}

/// A lower bound on `resolve(base, faction ++ s)` for every subset `s` of
/// `techs` kept in order (the faction's rows always apply): the lowest of
/// the faction's start (its last `Set`, else `base`) and every tech `Set`,
/// plus every faction `Add` and every negative tech `Add`, then every
/// faction `Mul` and every tech `Mul` below 1000, in order. Sound because
/// `floor(v * m / 1000)` never decreases as `v` grows and a `Mul` of 1000 or
/// more never lowers a value of 0 or more; a start at or below 0 is
/// returned as it is (the caller only asks whether the bound is positive).
pub(crate) fn lower_bound<'a>(
    base: i64,
    faction: impl Iterator<Item = &'a Modifier> + Clone,
    techs: impl Iterator<Item = &'a Modifier> + Clone,
) -> i64 {
    let start = faction
        .clone()
        .filter(|m| m.op == ModOp::Set)
        .last()
        .map_or(base, |m| i64::from(m.value));
    let start = techs
        .clone()
        .filter(|m| m.op == ModOp::Set)
        .map(|m| i64::from(m.value))
        .fold(start, i64::min);
    let adds: i64 = faction
        .clone()
        .filter(|m| m.op == ModOp::Add)
        .chain(techs.clone().filter(|m| m.op == ModOp::Add && m.value < 0))
        .map(|m| i64::from(m.value))
        .sum();
    let mut v = start + adds;
    if v <= 0 {
        return v;
    }
    for m in faction
        .filter(|m| m.op == ModOp::Mul)
        .chain(techs.filter(|m| m.op == ModOp::Mul && m.value < 1000))
    {
        v = (v * i64::from(m.value)).div_euclid(PERMILLE);
    }
    v
}

/// Check a list of rows, `rows[j]` named `{field}[j]`: `Mul` in
/// `1..=100000`, `Add` and `Set` within `±1000000`, and a `UnitSel::Kind`
/// names a kind in `unit_ids`.
pub(crate) fn validate_rows(
    path: &Path,
    field: &str,
    rows: &[Modifier],
    unit_ids: &[&str],
) -> Result<(), Error> {
    for (j, m) in rows.iter().enumerate() {
        let name = |part: &str| format!("{field}[{j}].{part}");
        match m.op {
            ModOp::Mul => {
                if !(1..=MAX_MUL_PERMILLE).contains(&m.value) {
                    return Err(Error::invalid(
                        path,
                        &name("value"),
                        format!(
                            "Mul {} must be in 1..={MAX_MUL_PERMILLE} (permille, 1000 = x1.0)",
                            m.value
                        ),
                    ));
                }
            }
            ModOp::Add | ModOp::Set => {
                if !(-MAX_ADD_SET..=MAX_ADD_SET).contains(&m.value) {
                    return Err(Error::invalid(
                        path,
                        &name("value"),
                        format!("{:?} {} must be within ±{MAX_ADD_SET}", m.op, m.value),
                    ));
                }
            }
        }
        if let StatPath::UnitHp(UnitSel::Kind(id)) | StatPath::UnitDamage(UnitSel::Kind(id)) =
            &m.target
            && !unit_ids.contains(&id.as_str())
        {
            return Err(Error::invalid(
                path,
                &name("target"),
                format!("{id:?} is not a unit id in units.ron"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(target: StatPath, op: ModOp, value: i32) -> Modifier {
        Modifier { target, op, value }
    }

    fn mul(v: i32) -> Modifier {
        row(StatPath::YieldCap, ModOp::Mul, v)
    }

    fn add(v: i32) -> Modifier {
        row(StatPath::YieldCap, ModOp::Add, v)
    }

    fn set(v: i32) -> Modifier {
        row(StatPath::YieldCap, ModOp::Set, v)
    }

    #[test]
    fn resolve_applies_set_then_add_then_mul_floored() {
        assert_eq!(resolve(70, &[mul(1250)]), 87);
        assert_eq!(resolve(70, &[set(100), mul(1250)]), 125);
        assert_eq!(resolve(25, &[add(25), add(25)]), 75);
        assert_eq!(resolve(100, &[mul(900)]), 90);
        assert_eq!(resolve(100, &[mul(900), mul(900)]), 81);
        assert_eq!(resolve(100, &[mul(900), mul(900), mul(900)]), 72);
        // A floor after each row (150 -> 135 -> 121 -> 108), not one floor
        // over the product (150 * 0.729 = 109.35 -> 109).
        assert_eq!(resolve(150, &[mul(900), mul(900), mul(900)]), 108);
        assert_eq!(resolve(2, &[add(1)]), 3);
        assert_eq!(resolve(70, &[add(10), mul(1250)]), 100);
        assert_eq!(resolve(70, &[set(100), add(10)]), 110);
        assert_eq!(resolve(100, &[mul(1100), mul(1100)]), 121);
        // Set applies first whatever the row order; the last Set wins.
        assert_eq!(resolve(70, &[mul(1250), set(100)]), 125);
        assert_eq!(resolve(70, &[add(10), set(100)]), 110);
        assert_eq!(resolve(70, &[set(100), set(150), set(200)]), 200);
        // Floor, not truncation toward zero: -1 * 1500 / 1000 = -1.5 -> -2.
        assert_eq!(resolve(1, &[add(-2), mul(1500)]), -2);
        assert_eq!(resolve(42, &[]), 42);
    }

    #[test]
    #[should_panic(expected = "fits i64")]
    fn resolve_overflow_panics_not_wraps() {
        let rows = vec![mul(MAX_MUL_PERMILLE); 8];
        resolve(i64::MAX / 2, &rows);
    }

    #[test]
    fn worst_case_bounds_every_subset() {
        let rows = [set(100), add(-10), add(5), mul(1100), mul(900)];
        let bound = worst_case(70, rows.iter()).unwrap();
        // |70| + 100 + 15 = 185, * 1.1 -> 204 (ceil 203.5), * max(0.9, 1) -> 204.
        assert_eq!(bound, 204);
        for mask in 0u32..32 {
            let subset: Vec<&Modifier> = (0..5)
                .filter(|i| mask & (1 << i) != 0)
                .map(|i| &rows[i])
                .collect();
            let v = resolve(70, subset.iter().copied());
            assert!(u128::from(v.unsigned_abs()) <= bound, "{mask:b}: {v}");
        }
        assert!(worst_case(1, std::iter::repeat_n(&mul(MAX_MUL_PERMILLE), 40)).is_none());
    }

    #[test]
    fn lower_bound_holds_for_every_subset_of_tech_rows() {
        let faction = [add(-2), mul(1200)];
        let techs = [mul(1100), set(40), add(-5), add(3), mul(900), mul(500)];
        let bound = lower_bound(30, faction.iter(), techs.iter());
        // min(30, 40) - 2 - 5 = 23, * 1.2 -> 27, * 0.9 -> 24, * 0.5 -> 12.
        assert_eq!(bound, 12);
        for mask in 0u32..64 {
            let rows: Vec<&Modifier> = faction
                .iter()
                .chain((0..6).filter(|i| mask & (1 << i) != 0).map(|i| &techs[i]))
                .collect();
            let v = resolve(30, rows.iter().copied());
            assert!(v >= bound, "{mask:b}: {v} < {bound}");
        }
    }

    #[test]
    fn selectors_reach_their_kinds() {
        let rules =
            crate::Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap();
        let yeoman = rules.unit_kind_by_id("yeoman").unwrap();
        let bowman = rules.unit_kind_by_id("bowman").unwrap();
        let wain = rules.unit_kind_by_id("supply_wain").unwrap();
        assert!(!UnitSel::Military.selects(yeoman));
        assert!(UnitSel::Military.selects(bowman));
        assert!(UnitSel::Military.selects(wain));
        assert!(UnitSel::Class(UnitClass::Infantry).selects(bowman));
        assert!(!UnitSel::Class(UnitClass::Infantry).selects(wain));
        assert!(UnitSel::Kind("bowman".into()).selects(bowman));
        assert!(!UnitSel::Kind("bowman".into()).selects(yeoman));
        assert!(StatPath::UnitHp(UnitSel::Military).matches(bowman));
        assert!(!StatPath::UnitDamage(UnitSel::Military).matches(yeoman));
        assert!(!StatPath::PopCap.matches(bowman));
    }

    #[test]
    fn rows_are_range_checked_with_field_names() {
        let p = Path::new("t.ron");
        let ids = ["yeoman"];
        let err = |rows: &[Modifier]| {
            validate_rows(p, "techs[0].modifiers", rows, &ids)
                .unwrap_err()
                .to_string()
        };
        validate_rows(
            p,
            "x",
            &[mul(1), mul(MAX_MUL_PERMILLE), add(-MAX_ADD_SET)],
            &ids,
        )
        .unwrap();
        assert!(err(&[add(1), mul(0)]).contains("techs[0].modifiers[1].value"));
        assert!(err(&[mul(-900)]).contains("techs[0].modifiers[0].value"));
        assert!(err(&[mul(MAX_MUL_PERMILLE + 1)]).contains("modifiers[0].value"));
        assert!(err(&[set(MAX_ADD_SET + 1)]).contains("modifiers[0].value"));
        let e = err(&[row(
            StatPath::UnitHp(UnitSel::Kind("dragon".into())),
            ModOp::Add,
            1,
        )]);
        assert!(
            e.contains("modifiers[0].target") && e.contains("dragon"),
            "{e}"
        );
    }

    #[test]
    fn rows_parse_from_ron_and_reject_unknown_names() {
        let m: Modifier =
            ron::from_str("(target: UnitDamage(Class(Cavalry)), op: Mul, value: 1100)").unwrap();
        assert_eq!(
            m,
            row(
                StatPath::UnitDamage(UnitSel::Class(UnitClass::Cavalry)),
                ModOp::Mul,
                1100
            )
        );
        let m: Modifier =
            ron::from_str("Modifier(target: YieldCap, op: Mul, value: 1250)").unwrap();
        assert_eq!(m, mul(1250));
        let e = ron::from_str::<Modifier>("(target: GoldCap, op: Add, value: 1)").unwrap_err();
        assert!(e.to_string().contains("StatPath"), "{e}");
        let e =
            ron::from_str::<Modifier>("(target: UnitHp(Heroes), op: Add, value: 1)").unwrap_err();
        assert!(e.to_string().contains("UnitSel"), "{e}");
        assert!(ron::from_str::<Modifier>("(target: YieldCap, op: Mul, value: 1.25)").is_err());
        let e = ron::from_str::<Modifier>("(target: YieldCap, op: Mul, value: 1, scale: 2)")
            .unwrap_err();
        assert!(e.to_string().contains("scale"), "{e}");
    }
}
