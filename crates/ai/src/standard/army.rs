//! Army production and the rally (docs/design/ai.md rules 4 and 5).
//!
//! Production runs once the build order's `Military` step is latched, or
//! while the bot defends (emergency production before the latch). Supply
//! Wains come first: while the bot has fewer than `wains_per_wave` alive
//! or queued, one is trained (or saved up for) at a complete trainer of
//! the supply kind. Then every complete, uncontested trainer with fewer
//! than `max_queue_per_building` entries trains one army-mix kind. Kind
//! `k`'s deficit over a set of mix kinds is
//! `weight_k × (N + 1) − have_k × W`, `have_k` its units alive or queued
//! outside the attack wave, `N` and `W` the sums of `have` and of the
//! weights over the set. Two sets are used:
//!
//! - **the share base**: the mix kinds the bot can field at all now, those
//!   with a complete, uncontested trainer, free or busy, that accepts the
//!   kind and whose cost the plan's unreserved stock pays. A kind whose
//!   deficit over the base is negative never trains, so an Engine Yard
//!   never fills up with Mangonels while the Muster Hall is busy;
//! - **the trainable mix**: the base kinds trainable this plan (a free
//!   trainer accepts the kind and the stock pays for it). A trainer
//!   trains the kind it can train with the largest positive deficit over
//!   the trainable mix (ties to the lowest kind id), so a busy or
//!   short-of-stock trainer's kinds do not hold the free ones back, and a
//!   Mangonel short of Lumber does not stall the Muster Hall.
//!
//! A kind the pop cap or the unreserved stock does not allow is skipped.
//!
//! The rally is the first passable own tile of the spiral around the map
//! analysis' rally seed ([`Ctx::rally`]); every trainer of a non-civilian
//! kind whose rally differs gets `SetRally` (`check_rally` first).

use super::compose::{Counters, shares, weighted_mix};
use super::ctx::{Ctx, jitter, salt};
use super::memory::BotMemory;
use super::waves::wain_needed;
use rules::UnitClass;
use sim::{BuildingId, BuildingKindId, Command, UnitId, UnitKindId};
use std::collections::BTreeSet;

/// A unit kind's index as a `UnitKindId`, by id.
fn unit_kind_id(ctx: &Ctx<'_>, id: &str) -> Option<UnitKindId> {
    let i = ctx.view.rules().units.iter().position(|u| u.id == id)?;
    Some(UnitKindId(u16::try_from(i).ok()?))
}

/// The own complete, uncontested buildings of the kind that trains
/// `kind`, with fewer than `max_queue_per_building` entries, in id order.
pub fn free_trainers(ctx: &Ctx<'_>, kind: UnitKindId) -> Vec<BuildingId> {
    let rules = ctx.view.rules();
    let Some(def) = rules.unit_kind(kind.0) else {
        return Vec::new();
    };
    let Some((trainer, _)) = rules.building_kind_by_id(&def.trainer) else {
        return Vec::new();
    };
    let max = usize::try_from(ctx.knobs.max_queue_per_building).unwrap_or(1);
    ctx.facts
        .complete
        .get(&BuildingKindId(trainer))
        .into_iter()
        .flatten()
        .copied()
        .filter(|b| !ctx.facts.contested.contains(b))
        .filter(|b| ctx.view.building(*b).is_some_and(|b| b.queue.len() < max))
        .collect()
}

/// Whether the bot can train `kind` this plan: a free trainer of it (not
/// `used` by the Wain) accepts it (`check_train`) and the plan's
/// unreserved stock pays for it.
fn trainable_now(ctx: &Ctx<'_>, kind: UnitKindId, used: &BTreeSet<BuildingId>) -> bool {
    free_trainers(ctx, kind)
        .into_iter()
        .filter(|b| !used.contains(b))
        .any(|b| {
            ctx.view
                .check_train(ctx.me, b, kind)
                .is_ok_and(|cost| ctx.ledger.affords(&cost))
        })
}

/// Whether `kind` is in the share base: an own complete, uncontested
/// trainer of it, free or busy, accepts it (a full sim queue counts as
/// accepting) and the plan's unreserved stock pays for it.
fn in_share_base(ctx: &Ctx<'_>, kind: UnitKindId) -> bool {
    let rules = ctx.view.rules();
    let Some(trainer) = rules
        .unit_kind(kind.0)
        .and_then(|d| rules.building_kind_by_id(&d.trainer))
    else {
        return false;
    };
    ctx.facts
        .complete
        .get(&BuildingKindId(trainer.0))
        .into_iter()
        .flatten()
        .filter(|b| !ctx.facts.contested.contains(b))
        .any(|b| match ctx.view.check_train(ctx.me, *b, kind) {
            Ok(cost) => ctx.ledger.affords(&cost),
            Err(sim::RejectReason::QueueFull) => {
                ctx.ledger.affords(&ctx.view.unit_cost(ctx.me, kind))
            }
            Err(_) => false,
        })
}

/// Kind `i`'s deficit over the kinds of `set` (indices into `mix` and
/// `have`): `weight_i × (N + 1) − have_i × W`.
fn deficit(mix: &[(UnitKindId, u32)], have: &[u32], set: &[usize], i: usize) -> i64 {
    let n: i64 = set.iter().map(|j| i64::from(have[*j])).sum();
    let w: i64 = set.iter().map(|j| i64::from(mix[*j].1)).sum();
    i64::from(mix[i].1) * (n + 1) - i64::from(have[i]) * w
}

/// Units of `kind` alive or queued outside the active wave.
fn home_and_queued(ctx: &Ctx<'_>, mem: &BotMemory, kind: UnitKindId) -> u32 {
    let in_wave: u32 = mem.wave.as_ref().map_or(0, |w| {
        let n = w
            .units
            .iter()
            .chain(w.wain.iter())
            .filter(|id| ctx.view.unit(**id).is_some_and(|u| u.kind == kind))
            .count();
        u32::try_from(n).expect("roster is small")
    });
    ctx.facts.alive_and_queued(kind).saturating_sub(in_wave)
}

/// Whether production runs this plan: the `Military` latch, or a defence.
pub fn producing(mem: &BotMemory) -> bool {
    mem.military || mem.defend.is_some()
}

/// Whether kind `k` is armed siege.
fn is_siege(ctx: &Ctx<'_>, k: UnitKindId) -> bool {
    ctx.view
        .rules()
        .unit_kind(k.0)
        .is_some_and(|d| d.class == UnitClass::Siege)
}

/// Emergency production before a difficulty's deadline raid (M6): from
/// `by_ds - prep_ds` until the raid launches (and while no wave has),
/// the bot trains non-siege armed kinds of its mix toward the raid's
/// `min_armed`, before the `Military` latch too. `true` while that many
/// are not yet alive or queued.
pub fn emergency(ctx: &Ctx<'_>, mem: &BotMemory) -> bool {
    let Some(raid) = ctx.first_raid else {
        return false;
    };
    if mem.raid_used
        || mem.waves_launched > 0
        || ctx.tick() < ctx.ticks(raid.by_ds.saturating_sub(raid.prep_ds))
    {
        return false;
    }
    raiders_alive_and_queued(ctx) < raid.min_armed
}

/// Non-siege armed units alive or queued.
fn raiders_alive_and_queued(ctx: &Ctx<'_>) -> u32 {
    let rules = ctx.view.rules();
    rules
        .units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.class != UnitClass::Civilian && u.class != UnitClass::Siege)
        .filter(|(_, u)| u.attack.is_some())
        .map(|(i, _)| {
            ctx.facts
                .alive_and_queued(UnitKindId(u16::try_from(i).expect("few")))
        })
        .sum()
}

/// The personality's counter knobs as [`Counters`].
fn counters(ctx: &Ctx<'_>) -> Counters {
    Counters {
        weight_permille: ctx.knobs.counter_weight_permille,
        floor_permille: ctx.knobs.counter_floor_permille,
        cap_permille: ctx.knobs.counter_cap_permille,
        join_permille: ctx.knobs.counter_join_permille,
        kinds: ctx
            .knobs
            .counter_kinds
            .iter()
            .filter_map(|m| Some((unit_kind_id(ctx, &m.kind)?, m.weight)))
            .collect(),
    }
}

/// Train the plan's army: Wains first, then the mix by deficit. M6: the
/// mix is weighted by counters against the visible enemy army
/// ([`super::compose`]) and jittered by `noise_mix_permille` per wave;
/// Wains are trained only while the Wain rule needs them; and before the
/// latch, emergency production ([`emergency`]) trains the non-siege kinds
/// of the mix toward the deadline raid's `min_armed`.
pub fn produce(ctx: &mut Ctx<'_>, mem: &mut BotMemory) {
    let urgent = !producing(mem) && emergency(ctx, mem);
    if !producing(mem) && !urgent {
        return;
    }
    let mut used: BTreeSet<BuildingId> = BTreeSet::new();
    if !urgent
        && wain_needed(ctx)
        && let Some(wain) = unit_kind_id(ctx, &ctx.knobs.supply_kind)
        && ctx.facts.alive_and_queued(wain) < ctx.knobs.wains_per_wave
        && let Some(b) = free_trainers(ctx, wain).first().copied()
    {
        match ctx.view.check_train(ctx.me, b, wain) {
            Ok(cost) if ctx.ledger.try_spend(&cost) => {
                if ctx.push(Command::Train {
                    building: b,
                    kind: wain,
                }) {
                    used.insert(b);
                }
            }
            Ok(cost) => ctx.ledger.reserve(&cost),
            Err(sim::RejectReason::CannotAfford { .. }) => {
                let cost = ctx.view.unit_cost(ctx.me, wain);
                ctx.ledger.reserve(&cost);
            }
            Err(_) => {}
        }
    }
    let personal: Vec<(UnitKindId, u32)> = ctx
        .knobs
        .army_mix
        .iter()
        .filter_map(|m| Some((unit_kind_id(ctx, &m.kind)?, m.weight)))
        .collect();
    let enemy = shares(&ctx.facts.enemy.armed);
    let knobs = counters(ctx);
    let weighted = weighted_mix(ctx.view.rules(), &personal, &enemy, &knobs, |k| {
        in_share_base(ctx, k)
    });
    let waves = u64::from(mem.waves_launched);
    let mix: Vec<(UnitKindId, u32)> = weighted
        .into_iter()
        .filter(|(k, _)| !urgent || !is_siege(ctx, *k))
        .filter(|(k, _)| in_share_base(ctx, *k))
        .map(|(k, w)| {
            let noise = ctx.noise(salt::MIX + (u64::from(k.0) << 16) + waves);
            (k, jitter(w, noise, ctx.knobs.noise_mix_permille).max(1))
        })
        .collect();
    let base: Vec<usize> = (0..mix.len()).collect();
    let now: Vec<usize> = base
        .iter()
        .copied()
        .filter(|i| trainable_now(ctx, mix[*i].0, &used))
        .collect();
    if now.is_empty() {
        return;
    }
    let mut have: Vec<u32> = mix
        .iter()
        .map(|(k, _)| home_and_queued(ctx, mem, *k))
        .collect();
    let mut trainers: BTreeSet<BuildingId> = BTreeSet::new();
    for i in &now {
        trainers.extend(free_trainers(ctx, mix[*i].0));
    }
    let mut raiders = raiders_alive_and_queued(ctx);
    let raid_min = ctx.first_raid.map_or(0, |r| r.min_armed);
    for b in trainers {
        if used.contains(&b) || ctx.commands_left == 0 || (urgent && raiders >= raid_min) {
            continue;
        }
        let mut order: Vec<(i64, UnitKindId, usize)> = now
            .iter()
            .map(|i| (deficit(&mix, &have, &now, *i), mix[*i].0, *i))
            .filter(|(d, _, _)| *d > 0)
            .filter(|(_, _, i)| deficit(&mix, &have, &base, *i) >= 0)
            .collect();
        order.sort_by_key(|(d, k, _)| (core::cmp::Reverse(*d), *k));
        for (_, k, i) in order {
            let Ok(cost) = ctx.view.check_train(ctx.me, b, k) else {
                continue;
            };
            if !ctx.ledger.try_spend(&cost) {
                continue;
            }
            if ctx.push(Command::Train {
                building: b,
                kind: k,
            }) {
                have[i] += 1;
                raiders = raiders.saturating_add(1);
            }
            break;
        }
    }
}

/// Keep every own trainer of a non-civilian kind rallied to the army's
/// rally point.
pub fn rally_upkeep(ctx: &mut Ctx<'_>) {
    let rally = ctx.rally();
    let rules = ctx.view.rules();
    let kinds: BTreeSet<BuildingKindId> = rules
        .units
        .iter()
        .filter(|u| u.class != UnitClass::Civilian)
        .filter_map(|u| rules.building_kind_by_id(&u.trainer))
        .map(|(i, _)| BuildingKindId(i))
        .collect();
    let stale: Vec<BuildingId> = kinds
        .iter()
        .flat_map(|k| ctx.facts.complete.get(k).into_iter().flatten().copied())
        .filter(|b| {
            ctx.view
                .building(*b)
                .is_some_and(|b| b.rally != Some(rally))
        })
        .filter(|b| ctx.view.check_rally(ctx.me, *b, rally).is_ok())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    for b in stale {
        if !ctx.push(Command::SetRally {
            building: b,
            target: rally,
        }) {
            return;
        }
    }
}

/// Own armed units outside the active wave and not claimed: the home
/// army.
pub fn home_armed(ctx: &Ctx<'_>, mem: &BotMemory) -> Vec<UnitId> {
    let roster: BTreeSet<UnitId> = mem
        .wave
        .as_ref()
        .map(|w| w.units.iter().copied().collect())
        .unwrap_or_default();
    ctx.facts
        .armed
        .iter()
        .copied()
        .filter(|u| !roster.contains(u) && !ctx.is_claimed(*u))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{Arena, P0, grant_all, place, spawn, standard_vs_passive, unit_kind};
    use sim::Command;

    fn trains(cmds: &[Command]) -> Vec<(BuildingId, UnitKindId)> {
        cmds.iter()
            .filter_map(|c| match c {
                Command::Train { building, kind } => Some((*building, *kind)),
                _ => None,
            })
            .collect()
    }

    /// With an Engine Yard and no Wain, the Wain is trained first there,
    /// and the Muster Hall trains the mix.
    #[test]
    fn wains_first() {
        let r = crate::testkit::rules();
        let setup = [
            vec![
                Command::DebugAdvanceAge { owner: P0 },
                place(&r, P0, "engine_yard", 18, 58),
                place(&r, P0, "muster_hall", 29, 66),
            ],
            grant_all(&r, P0, 1000),
        ]
        .concat();
        let a = Arena::frozen(standard_vs_passive(&r), setup);
        let ey = a.buildings_of(P0, "engine_yard")[0];
        let mh = a.buildings_of(P0, "muster_hall")[0];
        let mut ctx = a.ctx(P0);
        let mut mem = BotMemory {
            military: true,
            ..BotMemory::default()
        };
        produce(&mut ctx, &mut mem);
        let t = trains(&ctx.into_commands());
        assert_eq!(t[0], (ey, unit_kind(&r, "supply_wain")), "{t:?}");
        assert_eq!(t.iter().filter(|(b, _)| *b == ey).count(), 1);
        assert_eq!(t.iter().filter(|(b, _)| *b == mh).count(), 1);
    }

    /// A kind at or above its share of the army is never trained: ten
    /// Skirmishers make the Muster Hall train a Bowman; five Mangonels
    /// among nine units leave the Engine Yard idle, also while the Muster
    /// Hall is busy training.
    #[test]
    fn no_training_at_negative_deficit() {
        let r = crate::testkit::rules();
        let setup = [
            vec![
                place(&r, P0, "muster_hall", 29, 66),
                spawn(&r, P0, "skirmisher", 36, 60, 10),
            ],
            grant_all(&r, P0, 1000),
        ]
        .concat();
        let a = Arena::frozen(standard_vs_passive(&r), setup);
        let mh = a.buildings_of(P0, "muster_hall")[0];
        let mut ctx = a.ctx(P0);
        let mut mem = BotMemory {
            military: true,
            ..BotMemory::default()
        };
        produce(&mut ctx, &mut mem);
        assert_eq!(
            trains(&ctx.into_commands()),
            [(mh, unit_kind(&r, "bowman"))]
        );

        let setup = [
            vec![
                Command::DebugAdvanceAge { owner: P0 },
                place(&r, P0, "engine_yard", 18, 58),
                place(&r, P0, "muster_hall", 29, 66),
                spawn(&r, P0, "mangonel", 36, 60, 5),
                spawn(&r, P0, "skirmisher", 36, 56, 3),
                spawn(&r, P0, "supply_wain", 36, 70, 1),
                crate::testkit::research(&r, P0, "arms_1"), // pop 25 -> 50
            ],
            grant_all(&r, P0, 1000),
        ]
        .concat();
        let mut a = Arena::frozen(standard_vs_passive(&r), setup);
        let ey = a.buildings_of(P0, "engine_yard")[0];
        let mh = a.buildings_of(P0, "muster_hall")[0];
        let mut ctx = a.ctx(P0);
        produce(&mut ctx, &mut mem);
        let t = trains(&ctx.into_commands());
        assert!(t.iter().all(|(b, _)| *b != ey), "{t:?}");
        assert_eq!(t.len(), 1, "the Muster Hall still trains: {t:?}");

        // The same army while the Muster Hall trains (its queue of one is
        // full, as it is most of the time): its kinds still count toward
        // the shares, so the free Engine Yard adds no sixth Mangonel, plan
        // after plan.
        a.issue(
            P0,
            vec![Command::Train {
                building: mh,
                kind: unit_kind(&r, "skirmisher"),
            }],
        );
        assert_eq!(a.sim.view().building(mh).unwrap().queue.len(), 1);
        for round in 0..4 {
            let mut ctx = a.ctx(P0);
            produce(&mut ctx, &mut mem);
            let cmds = ctx.into_commands();
            assert!(
                trains(&cmds).iter().all(|(b, _)| *b != ey),
                "round {round}: five Mangonels of nine armed units, yet {cmds:?}"
            );
            a.issue(P0, cmds);
        }
        assert_eq!(a.units_of(P0, "mangonel").len(), 5);
    }

    /// Production waits for the `Military` latch or a defence.
    #[test]
    fn no_production_before_the_latch_without_a_threat() {
        let r = crate::testkit::rules();
        let setup = [
            vec![place(&r, P0, "muster_hall", 29, 66)],
            grant_all(&r, P0, 1000),
        ]
        .concat();
        let a = Arena::frozen(standard_vs_passive(&r), setup);
        let mut ctx = a.ctx(P0);
        let mut mem = BotMemory::default();
        produce(&mut ctx, &mut mem);
        assert!(ctx.into_commands().is_empty());
    }

    /// Every trainer of the army is rallied once to a passable own tile
    /// toward the enemy; a rally already there is left alone.
    #[test]
    fn rally_is_owned_and_set_once() {
        let r = crate::testkit::rules();
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            vec![place(&r, P0, "muster_hall", 29, 66)],
        );
        let mh = a.buildings_of(P0, "muster_hall")[0];
        let mut ctx = a.ctx(P0);
        rally_upkeep(&mut ctx);
        let cmds = ctx.into_commands();
        let [Command::SetRally { building, target }] = cmds[..] else {
            panic!("{cmds:?}");
        };
        assert_eq!(building, mh);
        let view = a.sim.view();
        let tile = view.map().tile_of(target);
        assert_eq!(tile, a.map(P0).rally_seed, "the seed itself is free");
        assert!(view.map().passable(tile));
        assert_eq!(view.territory().owner_at(tile), Some(P0));
        a.issue(P0, cmds);
        let mut ctx = a.ctx(P0);
        rally_upkeep(&mut ctx);
        assert!(ctx.into_commands().is_empty(), "set once");
    }

    /// M6: from `by_ds - prep_ds` of a deadline raid, before the
    /// `Military` latch, the bot trains non-siege armed kinds of its mix
    /// toward `min_armed`; not without a deadline raid, not once
    /// `min_armed` stand, and never a Mangonel or a Wain.
    #[test]
    fn emergency_production_trains_raiders_before_the_military_latch() {
        let r = crate::testkit::rules();
        let raid = rules::FirstRaid {
            by_ds: 600,
            prep_ds: 600,
            min_armed: 6,
            max_armed: 10,
        };
        let setup = |extra: u16| {
            let mut v = vec![
                Command::DebugAdvanceAge { owner: P0 },
                place(&r, P0, "muster_hall", 29, 66),
                place(&r, P0, "engine_yard", 29, 58),
            ];
            v.extend(grant_all(&r, P0, 1000));
            if extra > 0 {
                v.push(crate::testkit::spawn(&r, P0, "skirmisher", 36, 64, extra));
            }
            Arena::frozen(standard_vs_passive(&r), v)
        };
        let a = setup(0);
        let mh = a.buildings_of(P0, "muster_hall")[0];
        let mut ctx = a.ctx(P0);
        ctx.first_raid = Some(raid);
        let mut mem = BotMemory::default();
        assert!(emergency(&ctx, &mem));
        produce(&mut ctx, &mut mem);
        let cmds = ctx.into_commands();
        assert!(!cmds.is_empty(), "trains before the latch");
        for c in &cmds {
            let Command::Train { building, kind } = c else {
                panic!("{c:?}");
            };
            assert_eq!(*building, mh, "at the Muster Hall, not the Engine Yard");
            let def = r.unit_kind(kind.0).unwrap();
            assert!(
                def.class != UnitClass::Siege && def.attack.is_some(),
                "{}",
                def.id
            );
        }
        // No deadline raid: no production before the latch.
        let mut ctx = a.ctx(P0);
        ctx.first_raid = None;
        produce(&mut ctx, &mut BotMemory::default());
        assert!(ctx.into_commands().is_empty(), "no raid, no latch");
        // Six armed already: nothing more.
        let six = setup(6);
        let mut ctx = six.ctx(P0);
        ctx.first_raid = Some(raid);
        let mem = BotMemory::default();
        assert!(!emergency(&ctx, &mem));
        // The raid launched: back to the latch.
        let mut ctx = a.ctx(P0);
        ctx.first_raid = Some(raid);
        let mem = BotMemory {
            raid_used: true,
            ..BotMemory::default()
        };
        assert!(!emergency(&ctx, &mem));
    }
}
