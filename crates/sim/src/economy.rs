//! Stockpiles, income and costs (`docs/design/economy.md`).
//!
//! Hashed state is only [`Economy`]: per player, a stockpile and an income
//! accumulator per resource, in `resources.ron` order. Rates, pop and the
//! ramp counters are computed from the units and buildings whenever they are
//! needed and never stored, so there is nothing to keep in sync.
//!
//! Income is exact (`docs/DETERMINISM.md`): every tick each resource's
//! credited rate (per 30 s) is added to an `i64` accumulator in units of
//! 1/P resource, where P = [`Rules::income_period_ticks`] (600 at 20 Hz);
//! whole resources move to the stockpile and the remainder stays. A rate `r`
//! therefore adds exactly `r` every P ticks from any phase.
//!
//! Raw income is the base yield of every complete building (a Town's Grain
//! and Lumber, a Scriptorium's Lore) plus the rate of every worked slot
//! (`gather`). The Yield Cap (`yield_cap_base` with the player's `YieldCap`
//! rows, `tech::yield_cap`) applies per resource to resources with
//! `yield_capped`; Lore is exempt and stops at its `stockpile_cap` instead.

use crate::construction::{at_work, building_rect};
use crate::gather;
use crate::ids::{BuildingKindId, PlayerId};
use crate::state::{Construction, Job, MatchSetup, SimEvent, State};
use rules::Rules;
use serde::{Deserialize, Serialize};

/// Every player's stockpiles and accumulators. Sub-hash `economy`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Economy {
    /// Indexed by `PlayerId.0`.
    pub players: Vec<PlayerEconomy>,
}

/// One player's stockpiles and income accumulators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerEconomy {
    /// Stockpile per resource, `resources.ron` order.
    pub stock: Vec<i64>,
    /// Income accumulator per resource in 1/P units, always in `0..P`.
    pub acc: Vec<i64>,
}

impl Economy {
    /// `players` empty stockpiles for `resources` resources.
    pub fn new(players: usize, resources: usize) -> Economy {
        Economy {
            players: (0..players)
                .map(|_| PlayerEconomy {
                    stock: vec![0; resources],
                    acc: vec![0; resources],
                })
                .collect(),
        }
    }

    /// One player's economy.
    ///
    /// # Panics
    ///
    /// If the player does not exist (`Sim::new` checks ids are `0..n`).
    pub fn player(&self, p: PlayerId) -> &PlayerEconomy {
        &self.players[usize::from(p.0)]
    }

    pub(crate) fn player_mut(&mut self, p: PlayerId) -> &mut PlayerEconomy {
        &mut self.players[usize::from(p.0)]
    }
}

/// One resource's income for a player this tick, per 30 s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateInfo {
    /// Production before the Yield Cap.
    pub raw: u32,
    /// The Yield Cap, when the resource is capped.
    pub cap: Option<u32>,
    /// What reaches the accumulator: `min(raw, cap)`, or `raw` when uncapped.
    /// `raw > credited` is the HUD's amber readout (M3b).
    pub credited: u32,
}

impl RateInfo {
    /// `true` while production of this resource is discarded at the Yield
    /// Cap (`raw > credited`): the HUD's amber readout (M3b) and
    /// `MatchStats`' cap seconds (M6) share this one rule.
    pub fn at_cap(&self) -> bool {
        self.raw > self.credited
    }
}

/// One bot seat's disclosed bonus schedule (M6, step 6d'): `bundle` is
/// added to the seat's stockpile on the step that completes tick `n` for
/// every `n > from_ticks` with `(n - from_ticks) % every_ticks == 0`.
/// Derived from the header and the rules ([`bonus_table`]), never hashed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BonusSeat {
    /// The bot seat.
    pub player: PlayerId,
    /// The match time the schedule counts from, in ticks.
    pub from_ticks: u32,
    /// The interval in ticks (at least 1).
    pub every_ticks: u32,
    /// The bundle per resource, `resources.ron` order.
    pub bundle: Vec<i64>,
}

/// The bonus schedule of a match (M6): one [`BonusSeat`] per AI seat, in
/// id order, whose `bot` names a difficulty level (`rules.ai`) that pays
/// (a non-zero interval and a non-empty bundle). Human slots and `passive`
/// seats get nothing. A pure function of the header and the
/// `rules_hash`-covered data, so a replay or a restore derives the same
/// table.
pub fn bonus_table(setup: &MatchSetup, rules: &Rules) -> Vec<BonusSeat> {
    setup
        .ai_seats
        .iter()
        .filter_map(|seat| {
            let level = rules.ai.difficulty(&seat.bot)?;
            if !level.pays() {
                return None;
            }
            Some(BonusSeat {
                player: seat.player,
                from_ticks: level.income_from_ticks(rules),
                every_ticks: level.income_interval_ticks(rules).max(1),
                bundle: level.bundle(rules),
            })
        })
        .collect()
}

/// Step 6d' (M6): pay every bonus seat whose schedule falls on the tick this
/// step completes (`tick + 1`), in id order, skipping a surrendered seat:
/// the bundle is added to the stockpile, then each resource is clamped to
/// its `stockpile_cap` (Lore). Emits `BonusIncome` with what was actually
/// credited. Integer arithmetic on the hashed stockpiles on a fixed tick
/// schedule, so replays re-derive it with nothing recorded.
pub(crate) fn bonus_step(
    state: &mut State,
    rules: &Rules,
    bonus: &[BonusSeat],
    tick: u32,
    events: &mut Vec<SimEvent>,
) {
    let paying: Vec<PlayerId> = state
        .players
        .iter()
        .filter(|p| !p.surrendered)
        .map(|p| p.id)
        .collect();
    pay_bonus(&mut state.economy, &paying, rules, bonus, tick, events);
}

/// Whether `seat`'s schedule pays on the step that completes tick
/// `completes`.
pub fn bonus_due(seat: &BonusSeat, completes: u32) -> bool {
    completes > seat.from_ticks
        && (completes - seat.from_ticks).is_multiple_of(seat.every_ticks.max(1))
}

/// [`bonus_step`] over a bare economy: `paying` lists the players that may
/// receive a bonus (those that have not surrendered).
fn pay_bonus(
    economy: &mut Economy,
    paying: &[PlayerId],
    rules: &Rules,
    bonus: &[BonusSeat],
    tick: u32,
    events: &mut Vec<SimEvent>,
) {
    let completes = tick.saturating_add(1);
    for seat in bonus {
        if !bonus_due(seat, completes) || !paying.contains(&seat.player) {
            continue;
        }
        let eco = economy.player_mut(seat.player);
        let before = eco.stock.clone();
        for (s, a) in eco.stock.iter_mut().zip(&seat.bundle) {
            *s = s.checked_add(*a).expect("stockpile fits i64");
        }
        clamp_to_caps(rules, &mut eco.stock);
        let amounts: Vec<i64> = eco
            .stock
            .iter()
            .zip(&before)
            .map(|(after, b)| after.saturating_sub(*b).max(0))
            .collect();
        events.push(SimEvent::BonusIncome {
            player: seat.player,
            amounts,
        });
    }
}

/// The player's income per resource this tick, `resources.ron` order.
pub(crate) fn rates(state: &State, rules: &Rules, p: PlayerId) -> Vec<RateInfo> {
    let n = rules.resources.resources.len();
    let mut raw = vec![0u32; n];
    for b in state.buildings.values() {
        if b.owner != p || b.construction != Construction::Complete {
            continue;
        }
        let kind = building_kind(rules, b.kind);
        for (id, &amount) in &kind.base_yield {
            let r = rules.resource_index(id).expect("validated resource id");
            raw[r] = raw[r].checked_add(amount).expect("income rate fits u32");
        }
    }
    gather::worked_income(state, rules, p, &mut raw);
    let cap = crate::tech::yield_cap(state, rules, p);
    rules
        .resources
        .resources
        .iter()
        .zip(raw)
        .map(|(res, raw)| {
            let cap = res.yield_capped.then_some(cap);
            RateInfo {
                raw,
                cap,
                credited: cap.map_or(raw, |c| raw.min(c)),
            }
        })
        .collect()
}

/// Step 6c: drop every job that can no longer be done, in unit id order,
/// then let idle workers seek work ([`gather::idle_seek_step`]). A build job
/// ends when its site is gone, is not the unit's owner's or is complete; a
/// gather job when its building is gone, not the owner's, not complete or
/// not worked by the unit's kind. Either ends when the unit has no order
/// and is not at work there (its path ended somewhere else: unreachable, or
/// the goal was corrected).
pub(crate) fn jobs_step(state: &mut State, rules: &Rules, tick: u32) {
    let mut dropped = Vec::new();
    for u in state.units.values() {
        let keep = match u.job {
            None => continue,
            Some(Job::Build { site }) => state.buildings.get(&site).is_some_and(|b| {
                b.owner == u.owner
                    && matches!(b.construction, Construction::Site { .. })
                    && (u.order.is_some()
                        || at_work(u, &state.map, building_rect(rules, &state.map, b)))
            }),
            Some(Job::Gather { node, .. }) => gather::gather_job_valid(state, rules, u.id, node),
        };
        if !keep {
            dropped.push(u.id);
        }
    }
    for id in dropped {
        if let Some(u) = state.units.get_mut(&id) {
            u.job = None;
        }
    }
    gather::idle_seek_step(state, rules, tick);
}

/// Step 6d: credit one tick of income to every player, in id order and
/// resource order, then clamp to each resource's `stockpile_cap`.
pub(crate) fn income_step(state: &mut State, rules: &Rules) {
    let period = i64::from(rules.income_period_ticks());
    let players: Vec<PlayerId> = state.players.iter().map(|p| p.id).collect();
    for p in players {
        let credited: Vec<u32> = rates(state, rules, p).iter().map(|r| r.credited).collect();
        let eco = state.economy.player_mut(p);
        for (r, c) in credited.into_iter().enumerate() {
            let acc = eco.acc[r]
                .checked_add(i64::from(c))
                .expect("accumulator fits i64");
            eco.stock[r] = eco.stock[r]
                .checked_add(acc / period)
                .expect("stockpile fits i64");
            eco.acc[r] = acc % period;
        }
        clamp_to_caps(rules, &mut eco.stock);
    }
}

/// Clamp every stockpile with a `stockpile_cap` to it.
pub(crate) fn clamp_to_caps(rules: &Rules, stock: &mut [i64]) {
    for (s, res) in stock.iter_mut().zip(&rules.resources.resources) {
        if let Some(cap) = res.stockpile_cap {
            *s = (*s).min(i64::from(cap));
        }
    }
}

/// The first resource (by index) the player cannot pay for, or `None` when
/// `cost` is affordable.
pub(crate) fn first_short(stock: &[i64], cost: &[i64]) -> Option<u8> {
    stock
        .iter()
        .zip(cost)
        .position(|(s, c)| s < c)
        .map(|i| u8::try_from(i).expect("resource count fits u8"))
}

/// Subtract `cost` from `stock`. The caller checked [`first_short`].
pub(crate) fn deduct(stock: &mut [i64], cost: &[i64]) {
    for (s, c) in stock.iter_mut().zip(cost) {
        *s = s.checked_sub(*c).expect("affordability checked");
    }
}

/// Add `amount` back to `stock`, clamped to each `stockpile_cap`.
pub(crate) fn refund(rules: &Rules, stock: &mut [i64], amount: &[i64]) {
    for (s, a) in stock.iter_mut().zip(amount) {
        *s = s.checked_add(*a).expect("stockpile fits i64");
    }
    clamp_to_caps(rules, stock);
}

/// The building kind behind a hashed `BuildingKindId`; every id in state was
/// validated against the rules when it was created.
pub(crate) fn building_kind(rules: &Rules, kind: BuildingKindId) -> &rules::BuildingKind {
    rules
        .building_kind(kind.0)
        .expect("building kind in state exists in the rules")
}

/// What a building of `kind` costs `p` now: the `n`-th copy, counting every
/// own building of the kind, sites and the Seat included.
pub(crate) fn building_cost_for(
    state: &State,
    rules: &Rules,
    p: PlayerId,
    kind: BuildingKindId,
) -> Vec<i64> {
    let n = state
        .buildings
        .values()
        .filter(|b| b.owner == p && b.kind == kind)
        .count();
    let n = u32::try_from(n).expect("building count fits u32");
    rules.building_cost(building_kind(rules, kind), n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn rules() -> Rules {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap()
    }

    /// M6: the HUD's amber rule and `MatchStats`' cap seconds are one
    /// function: production above the Yield Cap, `raw > credited`.
    #[test]
    fn rate_info_at_cap_matches_the_hud_rule() {
        let rate = |raw, cap: Option<u32>| RateInfo {
            raw,
            cap,
            credited: cap.map_or(raw, |c| raw.min(c)),
        };
        assert!(rate(80, Some(70)).at_cap());
        assert!(
            !rate(70, Some(70)).at_cap(),
            "exactly at the cap loses nothing"
        );
        assert!(!rate(10, Some(70)).at_cap());
        assert!(!rate(500, None).at_cap(), "uncapped Lore is never amber");
        for (raw, cap) in [
            (0, Some(0)),
            (1, Some(0)),
            (99, Some(100)),
            (101, Some(100)),
        ] {
            let r = rate(raw, cap);
            assert_eq!(r.at_cap(), r.raw > r.credited, "{r:?}");
        }
    }

    /// M6 step 6d': a hand-built schedule on a bare economy pays its
    /// bundle on the steps that complete `from + k x every` (k >= 1) and
    /// on no other, only to its seat, and reports what it paid.
    #[test]
    fn bonus_step_pays_on_schedule_from_income_from() {
        let rules = rules();
        let n = rules.resources.resources.len();
        let mut eco = Economy::new(2, n);
        let mut bundle = vec![0i64; n];
        bundle[0] = 20;
        bundle[1] = 7;
        let table = [BonusSeat {
            player: PlayerId(1),
            from_ticks: 100,
            every_ticks: 10,
            bundle: bundle.clone(),
        }];
        let mut events = Vec::new();
        let mut paid_at = Vec::new();
        for tick in 0..200u32 {
            let before = events.len();
            pay_bonus(
                &mut eco,
                &[PlayerId(0), PlayerId(1)],
                &rules,
                &table,
                tick,
                &mut events,
            );
            if events.len() > before {
                paid_at.push(tick + 1);
            }
        }
        assert_eq!(paid_at, (110..=200).step_by(10).collect::<Vec<u32>>());
        assert_eq!(eco.players[1].stock[0], 20 * 10);
        assert_eq!(eco.players[1].stock[1], 7 * 10);
        assert!(
            eco.players[0].stock.iter().all(|s| *s == 0),
            "seat 0 has no bonus"
        );
        assert!(
            eco.players[1].acc.iter().all(|a| *a == 0),
            "the bonus bypasses the accumulator"
        );
        assert!(events.iter().all(|e| *e
            == SimEvent::BonusIncome {
                player: PlayerId(1),
                amounts: bundle.clone()
            }));
        // A seat left out of `paying` (surrendered) gets nothing.
        let mut eco = Economy::new(2, n);
        pay_bonus(&mut eco, &[PlayerId(0)], &rules, &table, 109, &mut events);
        assert!(eco.players[1].stock.iter().all(|s| *s == 0));
        assert!(
            bonus_due(&table[0], 110) && !bonus_due(&table[0], 100) && !bonus_due(&table[0], 105)
        );
    }

    #[test]
    fn affordability_names_the_first_short_resource() {
        assert_eq!(first_short(&[10, 5, 0], &[10, 5, 0]), None);
        assert_eq!(first_short(&[10, 4, 0], &[11, 5, 0]), Some(0));
        assert_eq!(first_short(&[10, 4, 0], &[10, 5, 1]), Some(1));
        let mut s = vec![10, 5, 0];
        deduct(&mut s, &[3, 5, 0]);
        assert_eq!(s, [7, 0, 0]);
    }

    /// The accumulator adds exactly `rate` per period from any phase, for
    /// every rate the data can produce in M3a and every starting remainder.
    #[test]
    fn accumulator_adds_exactly_rate_per_period_from_any_phase() {
        let period = 600i64;
        for rate in [1i64, 7, 10, 70, 599, 600, 601, 700] {
            for phase in (0..period).step_by(37) {
                let (mut acc, mut stock) = (phase, 0i64);
                for _ in 0..period {
                    acc += rate;
                    stock += acc / period;
                    acc %= period;
                }
                assert_eq!(stock, rate, "rate {rate} phase {phase}");
                assert_eq!(acc, phase, "the remainder returns to its phase");
            }
        }
    }
}
