//! Retreat when losing (M6, docs/design/ai.md rule 7).
//!
//! Each plan, an active wave in its `Stage` or `Assault` phase (a raid is
//! always in `Assault`) is weighed against the enemy army around it:
//!
//! - `own`: `sum(hp) x sum(dps_x10)` of the wave's living armed units (the
//!   Lanchester strength of [`super::influence`], their current hit
//!   points and resolved damage);
//! - `enemy`: the enemy-unit layer's strength over the block of
//!   `influence_radius_cells` around the cell of the wave's centroid
//!   (Towns and Watchtowers are left out, so a wave never flees from a
//!   building it is taking down);
//! - `lost`: `1000 - 1000 x alive / launched_units`, per mille.
//!
//! It retreats when `enemy > 0`, `own x 1000 < enemy x
//! retreat_ratio_permille` and `lost >= retreat_loss_permille`, except
//! during the all-in, in the `Hold` phase (the target is at 0 HP), within
//! `retreat_commit_ds` of the assault order (a raid's launch), and while
//! the target Town is at or below `retreat_commit_town_hp_permille` of its
//! hit points. A retreat orders one grouped plain `Move` of the wave's
//! armed units and its Wain to the rally, which disengages (a `Move` never
//! engages, `sim::combat`); arrivals hold the rally as guards and
//! re-acquire. The wave ends (`mem.wave = None`), `retreats` counts it and
//! the next launch waits at least `retreat_cooldown_ds`.

use super::ctx::Ctx;
use super::defend::all_in;
use super::influence::dps_x10;
use super::memory::{BotMemory, WaveKind, WavePhase};
use super::waves::committed;
use sim::{Command, Fx, FxVec2, UnitId};

/// The verdict of [`assess`] on the active wave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assessment {
    /// The wave's own strength.
    pub own: i64,
    /// The enemy units' strength around the wave's centroid.
    pub enemy: i64,
    /// Per mille of the launched armed units lost.
    pub lost: u32,
}

/// The strength of the wave and of the enemy around it, `None` when the
/// rule does not apply (no wave, a `Hold`, the all-in, the commit window,
/// a target at or below the commit share, or no living armed unit).
pub fn assess(ctx: &mut Ctx<'_>, mem: &BotMemory) -> Option<Assessment> {
    let wave = mem.wave.as_ref()?;
    if ctx.knobs.retreat_ratio_permille == 0 || wave.phase == WavePhase::Hold || all_in(ctx) {
        return None;
    }
    let tick = ctx.tick();
    if wave.phase == WavePhase::Assault
        && tick.saturating_sub(wave.phase_at) < ctx.ticks(ctx.knobs.retreat_commit_ds)
    {
        return None;
    }
    // M6 owner decision 12: a siege committed to a target that is losing
    // hit points stays.
    if committed(ctx, wave) {
        return None;
    }
    if wave.kind == WaveKind::Siege
        && ctx.view.building_hp(wave.target).is_some_and(|(hp, max)| {
            i64::from(hp.max(0)) * 1000
                <= i64::from(ctx.knobs.retreat_commit_town_hp_permille) * i64::from(max.max(1))
        })
    {
        return None;
    }
    let view = ctx.view;
    let me = ctx.me;
    let rules = view.rules();
    let (mut hp, mut dps, mut alive) = (0i64, 0i64, 0u32);
    let (mut sx, mut sy) = (0i64, 0i64);
    let units: Vec<UnitId> = wave.units.clone();
    for id in units {
        let Some(u) = view.unit(id).filter(|u| u.owner == me) else {
            continue;
        };
        let s = ctx.unit_stats(me, u.kind);
        let cooldown = rules
            .unit_kind(u.kind.0)
            .and_then(|k| k.attack.as_ref())
            .map_or(0, |a| a.cooldown_ds);
        hp += i64::from(s.max_hp.saturating_sub(u.damage_taken).max(0));
        dps += i64::from(s.attack.map_or(0, |a| dps_x10(a.damage, cooldown)));
        sx += i64::from(u.pos.x.floor_to_int());
        sy += i64::from(u.pos.y.floor_to_int());
        alive += 1;
    }
    if alive == 0 {
        return None;
    }
    let n = i64::from(alive);
    let centroid = FxVec2::new(
        Fx::from_int(i32::try_from(sx / n).unwrap_or(0)),
        Fx::from_int(i32::try_from(sy / n).unwrap_or(0)),
    );
    let launched = u32::from(wave.launched_units).max(1);
    let lost = 1000u32.saturating_sub(alive.saturating_mul(1000) / launched);
    let radius = ctx.knobs.influence_radius_cells;
    let inf = ctx.influence();
    let enemy = inf.strength(&inf.enemy_units, inf.cell_at(centroid), radius);
    Some(Assessment {
        own: hp.saturating_mul(dps),
        enemy,
        lost,
    })
}

/// Whether `a` calls for a retreat under the knobs.
pub fn losing(ctx: &Ctx<'_>, a: &Assessment) -> bool {
    a.enemy > 0
        && i128::from(a.own) * 1000
            < i128::from(a.enemy) * i128::from(ctx.knobs.retreat_ratio_permille)
        && a.lost >= ctx.knobs.retreat_loss_permille
}

/// Run the rule.
pub fn step(ctx: &mut Ctx<'_>, mem: &mut BotMemory) {
    if ctx.commands_left == 0 {
        return;
    }
    let Some(a) = assess(ctx, mem) else {
        return;
    };
    if !losing(ctx, &a) {
        return;
    }
    let Some(wave) = mem.wave.take() else {
        return;
    };
    let tick = ctx.tick();
    let rally = ctx.rally();
    let view = ctx.view;
    let me = ctx.me;
    let units: Vec<UnitId> = wave
        .units
        .iter()
        .chain(wave.wain.iter())
        .copied()
        .filter(|id| view.unit(*id).is_some_and(|u| u.owner == me))
        .filter(|id| ctx.claim(*id))
        .collect();
    ctx.order(units, |units| Command::Move {
        units,
        target: rally,
        queue: false,
    });
    mem.retreats = mem.retreats.saturating_add(1);
    mem.last_retreat_at = tick;
    mem.next_wave_at = mem
        .next_wave_at
        .max(tick.saturating_add(ctx.ticks(ctx.knobs.retreat_cooldown_ds)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standard::memory::Wave;
    use crate::testkit::{Arena, P0, P1, place, spawn, standard_vs_passive};
    use sim::{BuildingId, Intent, Tile};

    /// A wave of `units` in `phase`, launched with `launched` armed units
    /// at tick 0, against `target`.
    fn wave(units: Vec<UnitId>, launched: u16, phase: WavePhase, target: BuildingId) -> Wave {
        Wave {
            units,
            wain: None,
            target,
            stage: Tile::new(60, 64),
            phase,
            launched_at: 0,
            phase_at: 0,
            kind: WaveKind::Siege,
            launched_units: launched,
            last_contact_at: 0,
            target_hp: 0,
            progress_at: 0,
        }
    }

    /// Four of P0's Skirmishers at (60,64), `enemy` of P1's beside them.
    fn fight(enemy: u16) -> Arena {
        let r = crate::testkit::rules();
        let mut cmds = vec![spawn(&r, P0, "skirmisher", 60, 64, 4)];
        if enemy > 0 {
            cmds.push(spawn(&r, P1, "skirmisher", 62, 64, enemy));
        }
        Arena::frozen(standard_vs_passive(&r), cmds)
    }

    /// M6: a staging wave outnumbered three to one that has lost half of
    /// what it launched with comes home with one plain `Move` (the rally),
    /// counts a retreat and pushes the next launch out by the cooldown; one
    /// that has lost only a fifth, or that is not outnumbered, goes on.
    #[test]
    fn retreat_when_outnumbered_and_down_30_percent() {
        let a = fight(12);
        let units = a.units_of(P0, "skirmisher");
        let target = a.seat(P1);
        let mut ctx = a.base_ctx(P0);
        let rally = ctx.rally();
        let mut mem = BotMemory {
            wave: Some(wave(units.clone(), 8, WavePhase::Stage, target)),
            next_wave_at: 5,
            ..BotMemory::default()
        };
        let verdict = assess(&mut ctx, &mem).expect("the rule applies");
        assert_eq!(verdict.lost, 500, "4 of 8 left");
        assert!(verdict.enemy > verdict.own * 2, "{verdict:?}");
        assert!(losing(&ctx, &verdict));
        step(&mut ctx, &mut mem);
        let tick = a.sim.tick();
        assert!(mem.wave.is_none());
        assert_eq!((mem.retreats, mem.last_retreat_at), (1, tick));
        assert_eq!(
            mem.next_wave_at,
            tick + ctx.ticks(ctx.knobs.retreat_cooldown_ds)
        );
        assert_eq!(
            ctx.into_commands(),
            [Command::Move {
                units,
                target: rally,
                queue: false
            }]
        );

        // Down only a fifth: no retreat yet.
        let units = a.units_of(P0, "skirmisher");
        let mut ctx = a.base_ctx(P0);
        let mut mem = BotMemory {
            wave: Some(wave(units.clone(), 5, WavePhase::Stage, target)),
            ..BotMemory::default()
        };
        step(&mut ctx, &mut mem);
        assert!(mem.wave.is_some() && mem.retreats == 0);
        assert!(ctx.into_commands().is_empty());

        // Not outnumbered: no retreat, however many were lost.
        let even = fight(3);
        let units = even.units_of(P0, "skirmisher");
        let mut ctx = even.base_ctx(P0);
        let mut mem = BotMemory {
            wave: Some(wave(units, 8, WavePhase::Stage, even.seat(P1))),
            ..BotMemory::default()
        };
        step(&mut ctx, &mut mem);
        assert!(mem.wave.is_some() && mem.retreats == 0);
    }

    /// M6: a retreating wave in contact disengages: after the retreat no
    /// unit of it attack-moves or attacks, and it heads for the rally.
    #[test]
    fn a_retreating_wave_in_contact_moves_away() {
        let mut a = fight(12);
        let units = a.units_of(P0, "skirmisher");
        let enemy = a.units_of(P1, "skirmisher");
        let target = a.seat(P1);
        let centroid = |a: &Arena, ids: &[UnitId]| -> i64 {
            let view = a.sim.view();
            let xs: Vec<i64> = ids
                .iter()
                .filter_map(|u| view.unit(*u))
                .map(|u| i64::from(u.pos.x.floor_to_int()))
                .collect();
            xs.iter().sum::<i64>() / i64::try_from(xs.len().max(1)).unwrap()
        };
        let gap0 = centroid(&a, &enemy) - centroid(&a, &units);
        let x0 = centroid(&a, &units);
        let mut mem = BotMemory {
            wave: Some(wave(units.clone(), 8, WavePhase::Stage, target)),
            ..BotMemory::default()
        };
        let cmds = {
            let mut ctx = a.base_ctx(P0);
            step(&mut ctx, &mut mem);
            ctx.into_commands()
        };
        assert_eq!(mem.retreats, 1);
        a.issue(P0, cmds);
        a.run(80);
        let view = a.sim.view();
        let alive: Vec<UnitId> = units
            .iter()
            .copied()
            .filter(|u| view.unit(*u).is_some())
            .collect();
        assert!(!alive.is_empty(), "some got away");
        for u in &alive {
            let x = view.unit(*u).unwrap();
            assert!(
                !matches!(x.intent, Intent::AttackMove { .. } | Intent::Attack),
                "{u:?} still fights: {:?}",
                x.intent
            );
        }
        // The rally lies west, at x 36: the wave heads there instead of
        // standing its ground (the frozen enemy chases it, so the gap
        // itself need not grow).
        let x1 = centroid(&a, &alive);
        assert!(x1 + 5 <= x0, "the wave moves home: x {x0} -> {x1}");
        assert!(gap0 > 0);
    }

    /// M6: no retreat in the `Hold` phase, within `retreat_commit_ds` of
    /// the assault order, during the all-in, or while the target Town is at
    /// or below `retreat_commit_town_hp_permille`; a ratio of 0 turns the
    /// rule off.
    #[test]
    fn no_retreat_in_commit_window_hold_all_in_or_on_a_weak_target() {
        let r = crate::testkit::rules();
        let a = fight(12);
        let units = a.units_of(P0, "skirmisher");
        let target = a.seat(P1);
        let tick = a.sim.tick();
        let check = |mem: &BotMemory, knobs: Option<&rules::Personality>, why: &str| {
            let mut ctx = match knobs {
                Some(k) => crate::standard::Ctx::new(a.sim.view(), P0, k, a.map(P0)),
                None => a.base_ctx(P0),
            };
            let mut mem = mem.clone();
            step(&mut ctx, &mut mem);
            assert!(mem.wave.is_some() && mem.retreats == 0, "{why}");
        };
        let hold = BotMemory {
            wave: Some(wave(units.clone(), 8, WavePhase::Hold, target)),
            ..BotMemory::default()
        };
        check(&hold, None, "hold");
        let mut fresh = wave(units.clone(), 8, WavePhase::Assault, target);
        fresh.phase_at = tick;
        let fresh = BotMemory {
            wave: Some(fresh),
            ..BotMemory::default()
        };
        check(&fresh, None, "inside the commit window");
        let base = r.ai.personality("standard").unwrap().clone();
        let mut all_in = base.clone();
        all_in.all_in_after_ds = 0;
        let staging = BotMemory {
            wave: Some(wave(units.clone(), 8, WavePhase::Stage, target)),
            ..BotMemory::default()
        };
        check(&staging, Some(&all_in), "all-in");
        let mut off = base.clone();
        off.retreat_ratio_permille = 0;
        check(&staging, Some(&off), "ratio 0");
        // The target Town at 25%: committed. The wave is staging, not
        // assaulting, so the commit window (Assault only, and the arena is
        // only a few ticks old) cannot be what holds it: the Town-HP clause
        // must turn the rule off on its own.
        let mut weak = fight(12);
        weak.issue(
            P0,
            vec![Command::DebugDamage {
                target: sim::TargetId::Building(target),
                amount: 1500,
            }],
        );
        let mut ctx = weak.base_ctx(P0);
        let mut mem = BotMemory {
            wave: Some(wave(
                weak.units_of(P0, "skirmisher"),
                8,
                WavePhase::Stage,
                target,
            )),
            ..BotMemory::default()
        };
        assert!(assess(&mut ctx, &mem).is_none(), "weak target: rule off");
        step(&mut ctx, &mut mem);
        assert!(mem.wave.is_some() && mem.retreats == 0, "weak target");
        // The same staging wave against the sound Town retreats (the
        // paired control).
        let mut ctx = a.base_ctx(P0);
        let mut mem = staging.clone();
        step(&mut ctx, &mut mem);
        assert_eq!(mem.retreats, 1, "control");
        let _ = place;
    }

    /// M6: after a retreat the next launch waits for the cooldown: a full
    /// wave at the rally does not launch until `next_wave_at`.
    #[test]
    fn retreat_cooldown_blocks_the_next_launch() {
        let r = crate::testkit::rules();
        let mut a = fight(12);
        let units = a.units_of(P0, "skirmisher");
        let target = a.seat(P1);
        let mut mem = BotMemory {
            wave: Some(wave(units, 8, WavePhase::Stage, target)),
            ..BotMemory::default()
        };
        {
            let mut ctx = a.base_ctx(P0);
            step(&mut ctx, &mut mem);
        }
        assert_eq!(mem.retreats, 1);
        let until = mem.next_wave_at;
        // A launchable wave at the rally: Masonry, 9 Skirmishers, a
        // Mangonel and a Wain.
        a.issue(
            P0,
            vec![
                Command::DebugAdvanceAge { owner: P0 },
                spawn(&r, P0, "skirmisher", 36, 64, 9),
                spawn(&r, P0, "mangonel", 36, 64, 1),
                spawn(&r, P0, "supply_wain", 36, 64, 1),
            ],
        );
        assert!(a.sim.tick() < until);
        let mut k = r.ai.personality("standard").unwrap().clone();
        k.noise_first_wave_ds = 0;
        k.noise_wave_units = 0;
        let mut ctx = crate::standard::Ctx::new(a.sim.view(), P0, &k, a.map(P0));
        let mut m = mem.clone();
        crate::standard::waves::step(&mut ctx, &mut m);
        assert!(m.wave.is_none(), "cooling down");
        let mut ctx = crate::standard::Ctx::new(a.sim.view(), P0, &k, a.map(P0));
        let mut m = BotMemory {
            next_wave_at: a.sim.tick(),
            waves_launched: 1,
            ..mem
        };
        crate::standard::waves::step(&mut ctx, &mut m);
        assert!(m.wave.is_some(), "the control launches");
    }
}
