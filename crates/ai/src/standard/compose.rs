//! Counter-weighted composition (M6): `army::produce` weights the
//! personality's mix with it on every plan that trains.
//!
//! The bot weights its personality's `army_mix` by what the enemy shows.
//! With `s_e` the visible enemy armed share of kind `e` in per mille and
//! `c(k, e)` the counter table of `units.ron` (1000 when a pair is not
//! listed):
//!
//! ```text
//! adv(k)     = sum_e s_e x (c(k, e) - c(e, k)) / 1000
//! factor(k)  = clamp(1000 + weight x adv(k) / 1000, floor, cap)
//! weight'(k) = weight(k) x factor(k) / 1000
//! ```
//!
//! [`weighted_mix`] returns `weight(k) x factor(k)` itself, the same
//! proportions in thousandths of a weight, so a weight of 1 at the floor
//! (250) stays a quarter of a weight instead of rounding down to none.
//!
//! A kind of [`Counters::kinds`] (the Outrider, the only counter to
//! Mangonels, has no weight in the M5b mix) joins the mix with its listed
//! weight only while its factor reaches [`Counters::join_permille`] and it
//! is trainable now. With no visible enemy army every factor is 1000, so
//! the mix is the personality's own; a weight of 0 turns the rule off.
//! Integer arithmetic throughout, kinds in `UnitKindId` order, so ties go
//! to the lowest id.

use rules::{Defender, Rules};
use sim::UnitKindId;
use std::collections::BTreeMap;

/// The counter knobs (slice 2 reads them from the personality).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counters {
    /// How strongly counters bend the mix, per mille (0 turns it off).
    pub weight_permille: u32,
    /// The smallest factor, per mille.
    pub floor_permille: u32,
    /// The largest factor, per mille.
    pub cap_permille: u32,
    /// The factor at which a [`Counters::kinds`] entry joins the mix.
    pub join_permille: u32,
    /// Kinds outside the mix that join it, with their weight, while they
    /// counter what the enemy shows.
    pub kinds: Vec<(UnitKindId, u32)>,
}

/// The enemy's visible armed composition as shares in per mille of its
/// armed units, by kind (rounded down; empty with no enemy army).
pub fn shares(counts: &BTreeMap<UnitKindId, u32>) -> BTreeMap<UnitKindId, u32> {
    let total: u64 = counts.values().map(|n| u64::from(*n)).sum();
    if total == 0 {
        return BTreeMap::new();
    }
    counts
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(k, n)| {
            let s = u64::from(*n) * 1000 / total;
            (*k, u32::try_from(s).expect("at most 1000"))
        })
        .collect()
}

/// `c(a, d)`: the counter table's per mille for hits by kind `a` on kind
/// `d` (1000 when not listed or either kind is unknown).
fn counter(rules: &Rules, a: UnitKindId, d: UnitKindId) -> i64 {
    let (Some(ak), Some(dk)) = (rules.unit_kind(a.0), rules.unit_kind(d.0)) else {
        return 1000;
    };
    i64::from(rules.counter_permille(&ak.id, &Defender::Kind(dk.id.clone())))
}

/// `adv(k)`: how far kind `k` out-trades the visible enemy army, per mille.
pub fn advantage(rules: &Rules, k: UnitKindId, enemy: &BTreeMap<UnitKindId, u32>) -> i64 {
    enemy
        .iter()
        .map(|(e, s)| i64::from(*s) * (counter(rules, k, *e) - counter(rules, *e, k)) / 1000)
        .sum()
}

/// `factor(k)`, per mille, clamped to the floor and the cap.
pub fn factor(
    rules: &Rules,
    k: UnitKindId,
    enemy: &BTreeMap<UnitKindId, u32>,
    knobs: &Counters,
) -> u32 {
    let raw = 1000 + i64::from(knobs.weight_permille) * advantage(rules, k, enemy) / 1000;
    let lo = i64::from(knobs.floor_permille);
    let hi = i64::from(knobs.cap_permille).max(lo);
    u32::try_from(raw.clamp(lo, hi)).expect("clamped to u32 bounds")
}

/// The mix to train toward, in thousandths of a weight: every `mix`
/// entry's weight times its factor, then each [`Counters::kinds`] entry
/// not already in the mix whose factor reaches the join threshold and that
/// `trainable` accepts, with its listed weight times its factor. In
/// `UnitKindId` order; with every factor at 1000 it is the personality's
/// mix times 1000.
pub fn weighted_mix(
    rules: &Rules,
    mix: &[(UnitKindId, u32)],
    enemy: &BTreeMap<UnitKindId, u32>,
    knobs: &Counters,
    trainable: impl Fn(UnitKindId) -> bool,
) -> Vec<(UnitKindId, u32)> {
    let scale = |w: u32, f: u32| -> u32 { w.saturating_mul(f) };
    let mut out: BTreeMap<UnitKindId, u32> = BTreeMap::new();
    for (k, w) in mix {
        out.insert(*k, scale(*w, factor(rules, *k, enemy, knobs)));
    }
    for (k, w) in &knobs.kinds {
        if out.contains_key(k) {
            continue;
        }
        let f = factor(rules, *k, enemy, knobs);
        if f >= knobs.join_permille && trainable(*k) {
            out.insert(*k, scale(*w, f));
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::unit_kind;

    fn rules() -> Rules {
        crate::testkit::rules()
    }

    /// The M6 plan's shared starting values.
    fn knobs(r: &Rules) -> Counters {
        Counters {
            weight_permille: 750,
            floor_permille: 250,
            cap_permille: 4000,
            join_permille: 1500,
            kinds: vec![(unit_kind(r, "outrider"), 2)],
        }
    }

    /// The `standard` personality's mix, as kind ids.
    fn standard_mix(r: &Rules) -> Vec<(UnitKindId, u32)> {
        let p = r.ai.personality("standard").unwrap();
        p.army_mix
            .iter()
            .map(|m| (unit_kind(r, &m.kind), m.weight))
            .collect()
    }

    fn weight_of(mix: &[(UnitKindId, u32)], k: UnitKindId) -> u32 {
        mix.iter().find(|(x, _)| *x == k).map_or(0, |(_, w)| *w)
    }

    #[test]
    fn shares_are_per_mille_of_the_armed_units() {
        let r = rules();
        let (bow, skirm) = (unit_kind(&r, "bowman"), unit_kind(&r, "skirmisher"));
        assert!(shares(&BTreeMap::new()).is_empty());
        let s = shares(&BTreeMap::from([
            (bow, 3),
            (skirm, 1),
            (unit_kind(&r, "outrider"), 0),
        ]));
        assert_eq!(s, BTreeMap::from([(bow, 750), (skirm, 250)]));
    }

    /// M6: against an all-Bowman army the Skirmisher (2500 per mille on
    /// Bowmen) gains, the Shieldbearer (which Bowmen hit at 3000) drops to
    /// the floor, and the Skirmisher becomes the heaviest entry of the
    /// Standard mix.
    #[test]
    fn counter_weights_turn_bowmen_into_skirmishers() {
        let r = rules();
        let k = knobs(&r);
        let (bow, skirm, shield) = (
            unit_kind(&r, "bowman"),
            unit_kind(&r, "skirmisher"),
            unit_kind(&r, "shieldbearer"),
        );
        let enemy = shares(&BTreeMap::from([(bow, 10)]));
        assert_eq!(advantage(&r, skirm, &enemy), 1500);
        assert_eq!(factor(&r, skirm, &enemy, &k), 1000 + 750 * 1500 / 1000);
        assert_eq!(advantage(&r, bow, &enemy), 0);
        assert_eq!(factor(&r, bow, &enemy, &k), 1000);
        assert_eq!(advantage(&r, shield, &enemy), -2000);
        assert_eq!(factor(&r, shield, &enemy, &k), 250, "the floor");
        let base = standard_mix(&r);
        let mix = weighted_mix(&r, &base, &enemy, &k, |_| true);
        let heaviest = mix
            .iter()
            .max_by_key(|(id, w)| (*w, std::cmp::Reverse(*id)));
        assert_eq!(heaviest.map(|(id, _)| *id), Some(skirm), "{mix:?}");
        assert_eq!(weight_of(&mix, skirm), weight_of(&base, skirm) * 2125);
        assert_eq!(weight_of(&mix, bow), weight_of(&base, bow) * 1000);
        assert_eq!(
            weight_of(&mix, shield),
            weight_of(&base, shield) * 250,
            "a quarter of its weight, not rounded away"
        );
    }

    /// M6: the Outrider is not in the Standard mix; it joins with its
    /// listed weight against Mangonels (3000 per mille on them), only while
    /// it is trainable, and never against Shieldbearers, which counter it.
    #[test]
    fn counter_kinds_join_only_through_counters() {
        let r = rules();
        let k = knobs(&r);
        let (outrider, mangonel, shield) = (
            unit_kind(&r, "outrider"),
            unit_kind(&r, "mangonel"),
            unit_kind(&r, "shieldbearer"),
        );
        let base = standard_mix(&r);
        assert_eq!(weight_of(&base, outrider), 0, "not in the M5b mix");
        let siege = shares(&BTreeMap::from([(mangonel, 4)]));
        assert!(factor(&r, outrider, &siege, &k) >= k.join_permille);
        let joined = weighted_mix(&r, &base, &siege, &k, |_| true);
        assert_eq!(
            weight_of(&joined, outrider),
            2 * factor(&r, outrider, &siege, &k)
        );
        let untrainable = weighted_mix(&r, &base, &siege, &k, |x| x != outrider);
        assert_eq!(weight_of(&untrainable, outrider), 0, "not trainable now");
        let shields = shares(&BTreeMap::from([(shield, 4)]));
        assert!(factor(&r, outrider, &shields, &k) < k.join_permille);
        let not = weighted_mix(&r, &base, &shields, &k, |_| true);
        assert_eq!(weight_of(&not, outrider), 0);
        assert!(not.iter().all(|(id, _)| base.iter().any(|(b, _)| b == id)));
    }

    /// M6: a counter weight of 0, or no visible enemy army, keeps the
    /// personality's mix exactly.
    #[test]
    fn counter_weight_zero_keeps_the_mix() {
        let r = rules();
        let base = standard_mix(&r);
        let mut sorted: Vec<(UnitKindId, u32)> = base.iter().map(|(k, w)| (*k, w * 1000)).collect();
        sorted.sort_by_key(|(k, _)| *k);
        let off = Counters {
            weight_permille: 0,
            ..knobs(&r)
        };
        let bows = shares(&BTreeMap::from([(unit_kind(&r, "bowman"), 7)]));
        assert_eq!(weighted_mix(&r, &base, &bows, &off, |_| true), sorted);
        assert_eq!(
            weighted_mix(&r, &base, &BTreeMap::new(), &knobs(&r), |_| true),
            sorted,
            "no enemy army: every factor is 1000"
        );
    }
}
