//! The three ramping-cost rules (`docs/design/economy.md`, "Ramping
//! costs"), as pure functions of the loaded rules and a count. The sim
//! counts (`n`, `k`) from its state and calls these; the HUD and the bot
//! call the same functions through `SimView`, so a quote always equals the
//! deduction.
//!
//! Every cost is a dense `Vec<i64>` in `resources.ron` order, matching the
//! sim's `i64` stockpiles. Intermediate math is checked `u64`: an overflow
//! panics (`overflow-checks` is also on for this crate in release) instead
//! of wrapping, and every division floors.

use crate::{BuildingKind, Rules, UnitKind, UnitRamp};
use std::collections::BTreeMap;

/// Permille denominator: `1000` is a multiplier of 1.0.
pub const PERMILLE: u64 = 1000;

/// `floor(amount * permille / 1000)`, checked.
fn scale(amount: u64, permille: u64) -> u64 {
    amount
        .checked_mul(permille)
        .expect("cost * multiplier fits u64")
        / PERMILLE
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).expect("cost fits i64")
}

fn to_u64(v: i64) -> u64 {
    u64::try_from(v).expect("validated amounts are non-negative")
}

impl Rules {
    /// Index of a resource id in `resources.ron` order (the sim's stockpile
    /// index), if it exists.
    pub fn resource_index(&self, id: &str) -> Option<usize> {
        self.resources.resources.iter().position(|r| r.id == id)
    }

    /// A resource-id-to-amount map as a dense vector in `resources.ron`
    /// order. Keys are validated resource ids, so none is dropped.
    pub fn amounts(&self, map: &BTreeMap<String, u32>) -> Vec<i64> {
        let mut out = vec![0i64; self.resources.resources.len()];
        for (id, &v) in map {
            let i = self
                .resource_index(id)
                .expect("validated resource id in an amount map");
            out[i] = i64::from(v);
        }
        out
    }

    /// Building ramp multiplier in permille for the `n`-th copy (0-based):
    /// `1000 + building_ramp_step_permille * n`.
    pub fn building_mult_permille(&self, n: u32) -> u64 {
        u64::from(self.building_ramp_step_permille)
            .checked_mul(u64::from(n))
            .and_then(|v| v.checked_add(PERMILLE))
            .expect("building multiplier fits u64")
    }

    /// Cost of a building of `kind` when the player already owns `n` of that
    /// kind, sites included: `floor(base * (1000 + step * n) / 1000)` per
    /// resource.
    pub fn building_cost(&self, kind: &BuildingKind, n: u32) -> Vec<i64> {
        let mult = self.building_mult_permille(n);
        self.amounts(&kind.cost)
            .into_iter()
            .map(|base| to_i64(scale(to_u64(base), mult)))
            .collect()
    }

    /// Military ramp multiplier in permille at queue position `k` (1-based:
    /// `k = 1 + entries already queued at the player's buildings of the
    /// trainer kind`): `min(cap, 1000 + step * T(k - 1))` with the
    /// triangular number `T(m) = m * (m + 1) / 2`.
    ///
    /// # Panics
    ///
    /// If `k` is 0.
    pub fn military_mult_permille(&self, k: u32) -> u64 {
        assert!(k >= 1, "military queue position is 1-based");
        let m = u64::from(k - 1);
        let tri = m.checked_mul(m + 1).expect("T(k) fits u64") / 2;
        let mult = u64::from(self.military_ramp_step_permille)
            .checked_mul(tri)
            .and_then(|v| v.checked_add(PERMILLE))
            .expect("military multiplier fits u64");
        mult.min(u64::from(self.military_ramp_cap_permille))
    }

    /// Cost of training one `kind` when the player has `n` of that kind
    /// alive or queued, and `k` is its position in the trainer kind's
    /// shared queue (see [`Rules::military_mult_permille`]). `Flat` ignores
    /// both, `PerUnit` reads `n`, `Military` reads `k`.
    pub fn unit_cost(&self, kind: &UnitKind, n: u32, k: u32) -> Vec<i64> {
        let base = self.amounts(&kind.cost);
        match &kind.ramp {
            UnitRamp::Flat => base,
            UnitRamp::PerUnit(step) => base
                .into_iter()
                .zip(self.amounts(step))
                .map(|(b, s)| {
                    s.checked_mul(i64::from(n))
                        .and_then(|v| v.checked_add(b))
                        .expect("per-unit cost fits i64")
                })
                .collect(),
            UnitRamp::Military => {
                let mult = self.military_mult_permille(k);
                base.into_iter()
                    .map(|b| to_i64(scale(to_u64(b), mult)))
                    .collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Rules;
    use std::path::Path;

    fn rules() -> Rules {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap()
    }

    #[test]
    fn building_cost_ramp_multipliers() {
        let r = rules();
        let step = u64::from(r.building_ramp_step_permille);
        for n in 0..5u32 {
            assert_eq!(r.building_mult_permille(n), 1000 + step * u64::from(n));
        }
        if step == 200 {
            let want = [1000, 1200, 1400, 1600, 1800];
            for (n, w) in (0..5).zip(want) {
                assert_eq!(r.building_mult_permille(n), w);
            }
        }
        let (_, town) = r.building_kind_by_id("town").unwrap();
        let lumber = r.resource_index("lumber").unwrap();
        let base = i64::from(town.cost["lumber"]);
        let second = r.building_cost(town, 1);
        assert_eq!(
            second[lumber],
            base * (1000 + i64::from(r.building_ramp_step_permille)) / 1000
        );
        assert!(second[lumber] > r.building_cost(town, 0)[lumber]);
        assert_eq!(r.building_cost(town, 0)[lumber], base);
    }

    #[test]
    fn military_mult_steps_and_cap() {
        let r = rules();
        let step = u64::from(r.military_ramp_step_permille);
        let cap = u64::from(r.military_ramp_cap_permille);
        for k in 1..=10u32 {
            let m = u64::from(k - 1);
            assert_eq!(
                r.military_mult_permille(k),
                (1000 + step * m * (m + 1) / 2).min(cap)
            );
        }
        if (step, cap) == (100, 2250) {
            let got: Vec<u64> = (1..=6).map(|k| r.military_mult_permille(k)).collect();
            assert_eq!(got, [1000, 1100, 1300, 1600, 2000, 2250]);
        }
    }

    #[test]
    #[should_panic(expected = "1-based")]
    fn military_position_zero_panics() {
        rules().military_mult_permille(0);
    }

    #[test]
    fn unit_cost_per_unit_ramp() {
        let r = rules();
        let yeoman = r.unit_kind_by_id("yeoman").unwrap();
        let grain = r.resource_index("grain").unwrap();
        let base = i64::from(yeoman.cost["grain"]);
        let crate::UnitRamp::PerUnit(step) = &yeoman.ramp else {
            panic!("the Yeoman ramps per unit");
        };
        let step = i64::from(step["grain"]);
        for n in [0u32, 5, 6] {
            assert_eq!(r.unit_cost(yeoman, n, 1)[grain], base + step * i64::from(n));
        }
        if (base, step) == (20, 1) {
            assert_eq!(r.unit_cost(yeoman, 5, 1)[grain], 25);
            assert_eq!(r.unit_cost(yeoman, 6, 1)[grain], 26);
        }
    }

    #[test]
    fn unit_cost_military_ramp_reads_the_queue_position() {
        let r = rules();
        let sk = r.unit_kind_by_id("skirmisher").unwrap();
        let grain = r.resource_index("grain").unwrap();
        let base = i64::from(sk.cost["grain"]);
        for k in 1..=6u32 {
            let mult = i64::try_from(r.military_mult_permille(k)).unwrap();
            assert_eq!(r.unit_cost(sk, 99, k)[grain], base * mult / 1000);
        }
        let cap = i64::from(r.military_ramp_cap_permille);
        assert_eq!(r.unit_cost(sk, 0, 6)[grain], base * cap / 1000);
        let scribe = r.unit_kind_by_id("scribe").unwrap();
        assert_eq!(r.unit_cost(scribe, 7, 7), r.amounts(&scribe.cost));
    }

    #[test]
    fn costs_floor_not_round() {
        let mut r = rules();
        let (_, croft) = r.building_kind_by_id("croft").unwrap();
        let mut croft = croft.clone();
        croft.cost.insert("lumber".into(), 7);
        r.building_ramp_step_permille = 333;
        let lumber = r.resource_index("lumber").unwrap();
        // 7 * 1333 / 1000 = 9.331 -> 9; 7 * 1666 / 1000 = 11.662 -> 11.
        assert_eq!(r.building_cost(&croft, 1)[lumber], 9);
        assert_eq!(r.building_cost(&croft, 2)[lumber], 11);
    }

    #[test]
    #[should_panic(expected = "fits")]
    fn cost_overflow_panics_not_wraps() {
        let mut r = rules();
        r.building_ramp_step_permille = u32::MAX;
        let (_, town) = r.building_kind_by_id("town").unwrap();
        let town = town.clone();
        r.building_cost(&town, u32::MAX);
    }

    #[test]
    fn amounts_are_dense_in_resource_order() {
        let r = rules();
        let (_, town) = r.building_kind_by_id("town").unwrap();
        let v = r.amounts(&town.base_yield);
        assert_eq!(v.len(), r.resources.resources.len());
        for (i, res) in r.resources.resources.iter().enumerate() {
            assert_eq!(
                v[i],
                i64::from(town.base_yield.get(&res.id).copied().unwrap_or(0))
            );
        }
    }
}
