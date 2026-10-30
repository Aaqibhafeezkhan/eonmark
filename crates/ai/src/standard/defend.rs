//! Defence (docs/design/ai.md rule 6 as decided for M5b, owner decision 7).
//!
//! A threat is an enemy non-civilian unit on the bot's own land within
//! `defend_radius_tiles` of the centre of one of its complete Towns. The
//! engaged Town is the one with the most threatening pop, the Seat first
//! in a tie, then the lowest id. While a threat is seen the defend latch
//! (`BotMemory::defend`) is set and refreshed, and the home guard (every
//! home armed unit, claimed here, until the cap below) attack-moves to
//! the threatening unit nearest each guard (so a parked raider is killed,
//! not just watched); enemy siege in reach also gets an explicit `Attack`
//! from up to [`SIEGE_GUARDS`] guards. Production runs while the latch is
//! set (`army::producing`). A wave is called home while it is staging and
//! the latch still blocks launches, or when a threatened Town (any Town
//! with a threat, the engaged one or not) is contested or at or below
//! `recall_wave_town_hp_permille` of its hit points (`recall_wave:
//! WhileStaging`; `Always` recalls any wave), never once the all-in time
//! is reached. After `defend_release_ds` with no threat the latch is
//! released and the guard attack-moves back to the rally.
//!
//! **The cap** (M5b owner decision 7: a defence blocks launches for at
//! most 60 s). Waves do not launch while the latch is set, for at most
//! `defend_max_block_ds` ([`launch_blocked`]). Past that, while a launch
//! is due ([`super::waves::launch_due`]), the guard is sized to the
//! threat: the home armed units nearest it, siege last, until their pop
//! reaches `defend_guard_pop_ratio` times the threats' pop (at least one
//! unit). The rest away from the rally attack-move back to it, and the
//! rest at the rally get no order and stay unclaimed, so the wave
//! launches with them in the same plan; a wave launched so is not called
//! home while it stages for the same defence. A raider kept on own land
//! therefore holds launches back for `defend_max_block_ds` at most,
//! however long it stays.

use super::army::home_armed;
use super::ctx::{Ctx, on_station};
use super::facts::{kind_centre, tiles_sq, visible_enemies};
use super::memory::{BotMemory, Defend, WavePhase};
use super::waves::launch_due;
use rules::{RecallWave, UnitClass};
use sim::{BuildingId, Command, FxVec2, Intent, TargetId, UnitId};
use std::collections::{BTreeMap, BTreeSet};

/// Guards sent at each enemy siege unit with an explicit `Attack`.
pub const SIEGE_GUARDS: usize = 4;

/// The threats to each own complete Town, Seat first then by id: the
/// enemy non-civilian units on own land within `defend_radius_tiles` of
/// its centre, in id order, and their pop. Around a contested Town, whose
/// border has gone neutral while the enemy annexes it, every enemy
/// non-civilian unit in the radius counts, own land or not.
pub fn threats(ctx: &Ctx<'_>) -> Vec<(BuildingId, Vec<UnitId>, u32)> {
    let rules = ctx.view.rules();
    let view = ctx.view;
    let r2 = tiles_sq(ctx.knobs.defend_radius_tiles);
    let military = |u: &sim::Unit| {
        rules
            .unit_kind(u.kind.0)
            .is_some_and(|k| k.class != UnitClass::Civilian)
    };
    // M6: with `defend_economy`, the centres of own worked gathering
    // buildings, by the Town they are attributed to.
    let e2 = tiles_sq(ctx.knobs.defend_economy_tiles);
    let mut worked: BTreeMap<BuildingId, Vec<FxVec2>> = BTreeMap::new();
    if ctx.knobs.defend_economy {
        for node in ctx.facts.nodes.iter().flatten().filter(|n| n.taken > 0) {
            if let Some(t) = view.building(node.id).and_then(|b| b.town) {
                worked.entry(t).or_default().push(node.centre);
            }
        }
    }
    ctx.facts
        .towns
        .iter()
        .filter_map(|t| {
            let b = view.building(*t)?;
            let c = kind_centre(rules, b.kind, b.tile);
            let nodes = worked.get(t).map_or(&[][..], Vec::as_slice);
            let near_node = |u: &sim::Unit| nodes.iter().any(|n| u.pos.dist_sq_i64(*n) <= e2);
            let near = |u: &&sim::Unit| u.pos.dist_sq_i64(c) <= r2;
            let units: Vec<&sim::Unit> = if ctx.facts.contested.contains(t) {
                visible_enemies(&view, ctx.me)
                    .filter(|u| military(u))
                    .filter(|u| near(u) || near_node(u))
                    .collect()
            } else {
                let mut on_land: Vec<&sim::Unit> = ctx
                    .facts
                    .threats
                    .iter()
                    .filter_map(|id| view.unit(*id))
                    .filter(near)
                    .collect();
                if !nodes.is_empty() {
                    // Raiders at a worked building count, own land or not.
                    on_land.extend(
                        visible_enemies(&view, ctx.me)
                            .filter(|u| military(u) && near_node(u))
                            .filter(|u| {
                                !(u.pos.dist_sq_i64(c) <= r2 && ctx.facts.threats.contains(&u.id))
                            }),
                    );
                    on_land.sort_by_key(|u| u.id);
                }
                on_land
            };
            let pop = units.iter().fold(0u32, |p, u| {
                p.saturating_add(rules.unit_kind(u.kind.0).map_or(1, |k| k.pop))
            });
            Some((*t, units.into_iter().map(|u| u.id).collect(), pop))
        })
        .collect()
}

/// The engaged Town: the most threatening pop, ties to the earlier Town
/// in [`threats`]' order (the Seat first).
pub fn engaged(threats: &[(BuildingId, Vec<UnitId>, u32)]) -> Option<BuildingId> {
    let mut best: Option<(BuildingId, u32)> = None;
    for (t, _, pop) in threats {
        if *pop > 0 && best.is_none_or(|(_, p)| *pop > p) {
            best = Some((*t, *pop));
        }
    }
    best.map(|(t, _)| t)
}

/// Whether the defend latch blocks wave launches now: it is set and has
/// been for less than `defend_max_block_ds`.
pub fn launch_blocked(ctx: &Ctx<'_>, mem: &BotMemory) -> bool {
    let max = ctx.ticks(ctx.knobs.defend_max_block_ds);
    mem.defend
        .is_some_and(|d| ctx.tick().saturating_sub(d.since) < max)
}

/// Whether the all-in time is reached (no wave is recalled from then on).
pub fn all_in(ctx: &Ctx<'_>) -> bool {
    ctx.tick() >= ctx.ticks(ctx.knobs.all_in_after_ds)
}

/// Whether the active wave is called home to defend `town`.
pub fn should_recall(ctx: &Ctx<'_>, mem: &BotMemory, town: BuildingId) -> bool {
    let Some(wave) = &mem.wave else {
        return false;
    };
    if all_in(ctx) {
        return false;
    }
    match ctx.knobs.recall_wave {
        RecallWave::Always => true,
        RecallWave::WhileStaging => {
            let contested = ctx
                .view
                .annexation(town)
                .is_some_and(|a| a.attacker != ctx.me);
            let low = ctx.view.building_hp(town).is_some_and(|(hp, max)| {
                i64::from(hp.max(0)) * 1000
                    <= i64::from(ctx.knobs.recall_wave_town_hp_permille) * i64::from(max.max(1))
            });
            // Past its cap the defence holds the army home no longer: a
            // wave launched through it is not called back while staging.
            (wave.phase == WavePhase::Stage && launch_blocked(ctx, mem)) || contested || low
        }
    }
}

/// Call the active wave home: its living units attack-move to the rally
/// and the wave ends.
pub fn recall_wave(ctx: &mut Ctx<'_>, mem: &mut BotMemory) {
    let Some(wave) = mem.wave.take() else {
        return;
    };
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
    let (armed, other): (Vec<UnitId>, Vec<UnitId>) = units
        .into_iter()
        .partition(|id| ctx.facts.armed.contains(id));
    ctx.order(armed, |units| Command::AttackMove {
        units,
        target: rally,
    });
    ctx.order(other, |units| Command::Move {
        units,
        target: rally,
        queue: false,
    });
}

/// Whether `u` is attacking `target` on an explicit order.
fn attacking(ctx: &Ctx<'_>, u: UnitId, target: TargetId) -> bool {
    ctx.view
        .unit(u)
        .is_some_and(|u| u.intent == Intent::Attack && u.target == Some(target))
}

/// Run the rule.
pub fn step(ctx: &mut Ctx<'_>, mem: &mut BotMemory) {
    let tick = ctx.tick();
    let threats = threats(ctx);
    let Some(town) = engaged(&threats) else {
        release(ctx, mem);
        return;
    };
    mem.defend = Some(Defend {
        town,
        last_threat: tick,
        since: mem.defend.map_or(tick, |d| d.since),
    });
    // Any threatened Town can call the wave home, not only the engaged
    // one: a second Town being annexed may draw fewer raiders than the Seat.
    if threats
        .iter()
        .any(|(t, _, pop)| *pop > 0 && should_recall(ctx, mem, *t))
    {
        recall_wave(ctx, mem);
    }
    let rules = ctx.view.rules();
    let enemies: BTreeSet<UnitId> = threats
        .iter()
        .flat_map(|(_, u, _)| u.iter().copied())
        .collect();
    let home = home_armed(ctx, mem);
    let (guard, back) = if launch_blocked(ctx, mem) || !launch_due(ctx, mem) {
        (home, Vec::new())
    } else {
        sized_guard(ctx, home, &enemies)
    };
    for g in guard.iter().chain(back.iter()) {
        ctx.claim(*g);
    }
    let view = ctx.view;
    let pos = |id: UnitId| view.unit(id).map(|u| u.pos);
    // Siege first: each enemy siege unit gets its nearest free guards.
    // M6: with `defend_wains_first`, enemy Supply Wains among the threats
    // come before it, so the attacker's attrition cover goes first.
    let mut busy: BTreeSet<UnitId> = BTreeSet::new();
    let mut attacks: BTreeMap<UnitId, Vec<UnitId>> = BTreeMap::new();
    let kind_of = |e: UnitId| ctx.view.unit(e).and_then(|u| rules.unit_kind(u.kind.0));
    let wains_first = ctx.knobs.defend_wains_first;
    let mut marked: Vec<UnitId> = enemies
        .iter()
        .copied()
        .filter(|e| wains_first && kind_of(*e).is_some_and(|k| k.supplies))
        .collect();
    marked.extend(
        enemies
            .iter()
            .copied()
            .filter(|e| kind_of(*e).is_some_and(|k| k.class == UnitClass::Siege)),
    );
    for e in &marked {
        let Some(ep) = pos(*e) else { continue };
        let mut near: Vec<(i64, UnitId)> = guard
            .iter()
            .filter(|g| !busy.contains(g))
            .filter_map(|g| Some((pos(*g)?.dist_sq_i64(ep), *g)))
            .collect();
        near.sort();
        for (_, g) in near.into_iter().take(SIEGE_GUARDS) {
            busy.insert(g);
            if !attacking(ctx, g, TargetId::Unit(*e)) {
                attacks.entry(*e).or_default().push(g);
            }
        }
    }
    for (e, units) in attacks {
        if ctx
            .view
            .check_attack(ctx.me, &units, TargetId::Unit(e))
            .is_ok_and(|v| !v.is_empty())
            && !ctx.push(Command::Attack {
                units,
                target: TargetId::Unit(e),
            })
        {
            return;
        }
    }
    // The rest attack-move to their nearest threat unless on station.
    let regoal = ctx.knobs.defend_regoal_tiles;
    let mut moves: Vec<(UnitId, FxVec2)> = Vec::new();
    for g in guard.iter().filter(|g| !busy.contains(g)) {
        let Some(u) = ctx.view.unit(*g) else { continue };
        let Some((_, _, ep)) = enemies
            .iter()
            .filter_map(|e| Some((pos(*e)?.dist_sq_i64(u.pos), *e, pos(*e)?)))
            .min()
        else {
            continue;
        };
        // On an explicit Attack against one of the threats: on station.
        let fighting = u.intent == Intent::Attack
            && u.target
                .is_some_and(|t| enemies.iter().any(|x| t == TargetId::Unit(*x)));
        if fighting || on_station(u, ep, regoal) {
            continue;
        }
        moves.push((*g, ep));
    }
    ctx.order_grouped(moves, |units, target| Command::AttackMove { units, target });
    let rally = ctx.rally();
    ctx.order(back, |units| Command::AttackMove {
        units,
        target: rally,
    });
}

/// The guard past the cap while a launch is due: the home armed units
/// nearest a threat, siege last (ties to the lowest id), until their pop
/// reaches `defend_guard_pop_ratio` times the threats' pop, at least one
/// unit.
/// Returns it and the rest that are away from the rally and not already
/// heading there (they go back to it). The rest at the rally get no order
/// and stay unclaimed, so the wave can take them in the same plan.
fn sized_guard(
    ctx: &mut Ctx<'_>,
    home: Vec<UnitId>,
    enemies: &BTreeSet<UnitId>,
) -> (Vec<UnitId>, Vec<UnitId>) {
    let view = ctx.view;
    let rules = view.rules();
    let pop = |id: UnitId| {
        view.unit(id)
            .and_then(|u| rules.unit_kind(u.kind.0))
            .map_or(1, |k| k.pop)
    };
    let threat = enemies.iter().fold(0u32, |p, e| p.saturating_add(pop(*e)));
    let want = threat.saturating_mul(ctx.knobs.defend_guard_pop_ratio);
    // Siege last: slow, and no match for the raiders a guard chases.
    let mut near: Vec<(bool, i64, UnitId)> = home
        .iter()
        .filter_map(|g| {
            let u = view.unit(*g)?;
            let siege = rules
                .unit_kind(u.kind.0)
                .is_some_and(|k| k.class == UnitClass::Siege);
            let d = enemies
                .iter()
                .filter_map(|e| Some(view.unit(*e)?.pos.dist_sq_i64(u.pos)))
                .min()?;
            Some((siege, d, *g))
        })
        .collect();
    near.sort();
    let mut guard = Vec::new();
    let mut held = 0u32;
    let mut rest = Vec::new();
    for (_, _, g) in near {
        if guard.is_empty() || held < want {
            held = held.saturating_add(pop(g));
            guard.push(g);
        } else {
            rest.push(g);
        }
    }
    guard.sort();
    let rally = ctx.rally();
    let assembled = tiles_sq(ctx.knobs.assemble_radius_tiles);
    let tol = ctx.knobs.goal_tolerance_tiles;
    let back = rest
        .into_iter()
        .filter(|g| {
            view.unit(*g)
                .is_some_and(|u| u.pos.dist_sq_i64(rally) > assembled && !on_station(u, rally, tol))
        })
        .collect();
    (guard, back)
}

/// No threat this plan: release the latch once `defend_release_ds` has
/// passed since the last threat, sending the guard back to the rally.
fn release(ctx: &mut Ctx<'_>, mem: &mut BotMemory) {
    let Some(d) = mem.defend else {
        return;
    };
    if ctx.tick().saturating_sub(d.last_threat) < ctx.ticks(ctx.knobs.defend_release_ds) {
        return;
    }
    mem.defend = None;
    mem.defends = mem.defends.saturating_add(1);
    if u32::from(mem.extra_towers) < ctx.knobs.reactive_towers {
        mem.last_defended = Some(d.town);
    }
    let rally = ctx.rally();
    let tol = ctx.knobs.goal_tolerance_tiles;
    let back: Vec<UnitId> = home_armed(ctx, mem)
        .into_iter()
        .filter(|g| {
            ctx.view
                .unit(*g)
                .is_some_and(|u| !on_station(u, rally, tol))
        })
        .collect();
    for g in &back {
        ctx.claim(*g);
    }
    ctx.order(back, |units| Command::AttackMove {
        units,
        target: rally,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standard::memory::Wave;
    use crate::testkit::{Arena, P0, P1, grant_all, orders_of, place, spawn, standard_vs_passive};
    use sim::Tile;

    /// Each guard attack-moves to the threatening unit nearest it, one
    /// command per threat, and the latch names the Seat.
    #[test]
    fn home_guard_attack_moves_to_the_nearest_threat() {
        let r = crate::testkit::rules();
        let a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                spawn(&r, P0, "skirmisher", 30, 56, 2),
                spawn(&r, P0, "skirmisher", 30, 72, 2),
                spawn(&r, P1, "skirmisher", 26, 52, 1),
                spawn(&r, P1, "skirmisher", 26, 76, 1),
            ],
        );
        let guards = a.units_of(P0, "skirmisher");
        let raiders = a.units_of(P1, "skirmisher");
        let view = a.sim.view();
        let at = |id| view.unit(id).unwrap().pos;
        let mut ctx = a.ctx(P0);
        let mut mem = BotMemory::default();
        step(&mut ctx, &mut mem);
        let seat = a.seat(P0);
        assert_eq!(
            mem.defend,
            Some(Defend {
                town: seat,
                last_threat: a.sim.tick(),
                since: a.sim.tick()
            })
        );
        let cmds = ctx.into_commands();
        assert_eq!(cmds.len(), 2, "{cmds:?}");
        for c in &cmds {
            let Command::AttackMove { units, target } = c else {
                panic!("{c:?}");
            };
            let north = *target == at(raiders[0]);
            assert!(north || *target == at(raiders[1]), "{c:?}");
            let want = if north { &guards[..2] } else { &guards[2..] };
            assert_eq!(units, want, "{c:?}");
        }
        assert!(launch_blocked(&a.ctx(P0), &mem));
    }

    /// A raider parked on own land is hunted down by the guard (not just
    /// watched), the latch is released after a quiet spell, and launches
    /// are no longer blocked.
    #[test]
    fn a_parked_raider_is_killed_and_waves_resume() {
        let r = crate::testkit::rules();
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                spawn(&r, P0, "skirmisher", 36, 64, 4),
                spawn(&r, P1, "skirmisher", 24, 51, 1),
            ],
        );
        let raider = a.units_of(P1, "skirmisher")[0];
        let mut dead_at = None;
        for _ in 0..200 {
            a.act(P0);
            a.run(7);
            if dead_at.is_none() && a.sim.view().unit(raider).is_none() {
                dead_at = Some(a.sim.tick());
            }
            if dead_at.is_some() && a.mem(P0).defend.is_none() {
                break;
            }
        }
        let dead_at = dead_at.expect("the raider dies");
        assert!(dead_at < 1500, "killed at {dead_at}");
        let mem = a.mem(P0);
        assert_eq!(mem.defend, None, "released");
        assert_eq!(mem.defends, 1);
        assert!(!launch_blocked(&a.ctx(P0), &mem));
        assert_eq!(a.units_of(P0, "skirmisher").len(), 4, "no guard lost");
    }

    /// The arena of the launch-cap tests: P0 in the Masonry Age at tick
    /// 1400 with 12 Skirmishers, a Mangonel and a Supply Wain at its rally
    /// (36,64), and one P1 Outrider (pop 2) on P0's land at (24,51).
    fn raided_rally(r: &rules::Rules) -> Arena {
        let mut a = Arena::frozen(
            standard_vs_passive(r),
            vec![Command::DebugAdvanceAge { owner: P0 }],
        );
        a.run(1400);
        a.issue(
            P0,
            vec![
                spawn(r, P0, "skirmisher", 36, 64, 12),
                spawn(r, P0, "mangonel", 36, 64, 1),
                spawn(r, P0, "supply_wain", 36, 64, 1),
                spawn(r, P1, "outrider", 24, 51, 1),
            ],
        );
        a
    }

    /// A defence blocks launches for at most `defend_max_block_ds` (M5b
    /// owner decision 7), though the raider stays on own land: past the
    /// cap a guard still attack-moves to it, the units assembled at the
    /// rally are left to the wave and the wave launches in the same plan.
    /// Inside the cap the whole home army guards and nothing launches.
    #[test]
    fn a_defence_past_its_cap_lets_the_assembled_wave_launch() {
        let r = crate::testkit::rules();
        for (latch_age, launches) in [(100, false), (1300, true)] {
            let a = raided_rally(&r);
            let tick = a.sim.tick();
            let raider = a.units_of(P1, "outrider")[0];
            let raider_at = a.sim.view().unit(raider).unwrap().pos;
            let mut armed = a.units_of(P0, "skirmisher");
            armed.extend(a.units_of(P0, "mangonel"));
            armed.sort();
            let mut mem = BotMemory {
                military: true,
                defend: Some(Defend {
                    town: a.seat(P0),
                    last_threat: tick - 10,
                    since: tick - latch_age,
                }),
                // M6: past the first wave's noise wait (its own test is
                // in waves.rs).
                next_wave_at: 1,
                ..BotMemory::default()
            };
            let mut ctx = a.ctx(P0);
            assert_eq!(launch_blocked(&ctx, &mem), !launches);
            crate::standard::run_rules(&mut ctx, &mut mem);
            let cmds = ctx.into_commands();
            let guard: Vec<UnitId> = cmds
                .iter()
                .filter_map(|c| match c {
                    Command::AttackMove { units, target } if *target == raider_at => {
                        Some(units.clone())
                    }
                    _ => None,
                })
                .flatten()
                .collect();
            assert!(
                mem.defend.is_some(),
                "{latch_age}: the raider is still seen"
            );
            assert_eq!(
                mem.waves_launched,
                u16::from(launches),
                "{latch_age}: {cmds:?}"
            );
            if launches {
                let w = mem.wave.expect("a wave");
                assert!(w.units.len() >= 8, "{:?}", w.units);
                assert!(w.units.contains(&a.units_of(P0, "mangonel")[0]));
                assert!(
                    !guard.is_empty(),
                    "a guard still goes at the raider: {cmds:?}"
                );
                assert!(
                    guard.iter().all(|g| !w.units.contains(g)),
                    "the guard stays out of the wave"
                );
                assert!(
                    guard.len() >= 2,
                    "Skirmishers outweighing the raider's pop 2 guard: {guard:?}"
                );
            } else {
                assert_eq!(guard, armed, "the whole home army guards: {cmds:?}");
            }
        }
    }

    /// Past the cap, with a launch due, the guard is sized to the threat:
    /// the home armed units nearest the raider until their pop reaches
    /// `defend_guard_pop_ratio` times its pop; units away from the rally go
    /// back to it, and those at the rally get no order and stay unclaimed.
    #[test]
    fn past_the_cap_the_guard_is_sized_to_the_threat() {
        let r = crate::testkit::rules();
        let mut a = raided_rally(&r);
        a.issue(P0, vec![spawn(&r, P0, "skirmisher", 36, 80, 3)]);
        let tick = a.sim.tick();
        let raider = a.units_of(P1, "outrider")[0];
        let view = a.sim.view();
        let raider_at = view.unit(raider).unwrap().pos;
        let skirm = a.units_of(P0, "skirmisher");
        let (at_rally, away) = skirm.split_at(12);
        let mut by_distance: Vec<(i64, UnitId)> = at_rally
            .iter()
            .map(|u| (view.unit(*u).unwrap().pos.dist_sq_i64(raider_at), *u))
            .collect();
        by_distance.sort();
        let mut mem = BotMemory {
            military: true,
            defend: Some(Defend {
                town: a.seat(P0),
                last_threat: tick - 10,
                since: tick - 1300,
            }),
            ..BotMemory::default()
        };
        let mut ctx = a.ctx(P0);
        let rally = ctx.rally();
        // The Outrider's pop is 2; Skirmishers are pop 1 each.
        let want = usize::try_from(2 * ctx.knobs.defend_guard_pop_ratio).unwrap();
        let mut guard: Vec<UnitId> = by_distance.iter().take(want).map(|(_, u)| *u).collect();
        guard.sort();
        step(&mut ctx, &mut mem);
        let rest: Vec<UnitId> = at_rally
            .iter()
            .chain(a.units_of(P0, "mangonel").iter())
            .copied()
            .filter(|u| !guard.contains(u))
            .collect();
        assert!(rest.iter().all(|u| !ctx.is_claimed(*u)), "left to the wave");
        assert_eq!(
            ctx.into_commands(),
            [
                Command::AttackMove {
                    units: guard,
                    target: raider_at
                },
                Command::AttackMove {
                    units: away.to_vec(),
                    target: rally
                }
            ]
        );
    }

    /// A wave launched past the cap stages on while the same old defence
    /// goes on (else the cap would not hold: the next plan would call it
    /// home); a defence inside the cap still recalls a staging wave.
    #[test]
    fn a_staging_wave_is_recalled_only_inside_the_cap() {
        let r = crate::testkit::rules();
        for (latch_age, recalled) in [(100, true), (1300, false)] {
            let mut a = Arena::frozen(
                standard_vs_passive(&r),
                vec![Command::DebugAdvanceAge { owner: P0 }],
            );
            a.run(1400);
            a.issue(
                P0,
                vec![
                    spawn(&r, P0, "skirmisher", 60, 64, 8),
                    spawn(&r, P1, "outrider", 24, 51, 1),
                ],
            );
            let tick = a.sim.tick();
            let roster = a.units_of(P0, "skirmisher");
            let mut w = wave(roster, a.seat(P1), WavePhase::Stage);
            w.launched_at = tick - 50;
            w.phase_at = tick - 50;
            let mut mem = BotMemory {
                wave: Some(w),
                defend: Some(Defend {
                    town: a.seat(P0),
                    last_threat: tick - 10,
                    since: tick - latch_age,
                }),
                ..BotMemory::default()
            };
            let mut ctx = a.ctx(P0);
            step(&mut ctx, &mut mem);
            assert_eq!(mem.wave.is_none(), recalled, "latch {latch_age} ticks old");
        }
    }

    fn wave(units: Vec<UnitId>, target: BuildingId, phase: WavePhase) -> Wave {
        let launched_units = u16::try_from(units.len()).unwrap();
        Wave {
            units,
            wain: None,
            target,
            stage: Tile::new(80, 64),
            phase,
            launched_at: 0,
            phase_at: 0,
            kind: crate::standard::memory::WaveKind::Siege,
            launched_units,
            last_contact_at: 0,
            target_hp: 0,
            progress_at: 0,
        }
    }

    /// A threat calls a staging wave home; an assaulting wave goes on
    /// unless the threatened Town is at or below half its hit points.
    #[test]
    fn staging_wave_recalled_assaulting_wave_continues_unless_contested() {
        let r = crate::testkit::rules();
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                spawn(&r, P0, "skirmisher", 60, 64, 3),
                spawn(&r, P1, "skirmisher", 24, 51, 1),
            ],
        );
        let roster = a.units_of(P0, "skirmisher");
        let enemy_seat = a.seat(P1);
        for (phase, recalled) in [(WavePhase::Stage, true), (WavePhase::Assault, false)] {
            let mut mem = BotMemory {
                wave: Some(wave(roster.clone(), enemy_seat, phase)),
                ..BotMemory::default()
            };
            let mut ctx = a.ctx(P0);
            let rally = ctx.rally();
            step(&mut ctx, &mut mem);
            assert_eq!(mem.wave.is_none(), recalled, "{phase:?}");
            let cmds = ctx.into_commands();
            let home = cmds.iter().any(|c| {
                matches!(c, Command::AttackMove { units, target } if *units == roster && *target == rally)
            });
            assert_eq!(home, recalled, "{phase:?}: {cmds:?}");
            assert!(
                orders_of(&cmds, roster[0])
                    .iter()
                    .all(|c| matches!(c, Command::AttackMove { target, .. } if *target == rally)),
                "a wave unit is never part of the home guard"
            );
        }
        let seat = a.seat(P0);
        a.issue(
            P0,
            vec![Command::DebugDamage {
                target: TargetId::Building(seat),
                amount: 1200,
            }],
        );
        assert_eq!(a.sim.view().building_hp(seat), Some((800, 2000)));
        let mut mem = BotMemory {
            wave: Some(wave(roster.clone(), enemy_seat, WavePhase::Assault)),
            ..BotMemory::default()
        };
        let mut ctx = a.ctx(P0);
        step(&mut ctx, &mut mem);
        assert!(mem.wave.is_none(), "the Seat at 40% calls the wave home");
    }

    /// The recall looks at every threatened Town, not only the engaged
    /// one: with more raiders at the Seat than at a second Town that is
    /// being annexed at 0 HP, the Seat stays the engaged Town and the
    /// assaulting wave still comes home for the contested one.
    #[test]
    fn a_contested_second_town_recalls_the_wave_though_the_seat_is_engaged() {
        let r = crate::testkit::rules();
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                place(&r, P0, "town", 18, 81),
                spawn(&r, P0, "skirmisher", 60, 64, 3),
            ],
        );
        let seat = a.seat(P0);
        let town2 = *a
            .buildings_of(P0, "town")
            .iter()
            .find(|t| **t != seat)
            .unwrap();
        a.issue(
            P0,
            vec![
                Command::DebugDamage {
                    target: TargetId::Building(town2),
                    amount: 5000,
                },
                spawn(&r, P1, "skirmisher", 17, 83, 2),
            ],
        );
        a.run(5);
        a.issue(P0, vec![spawn(&r, P1, "skirmisher", 24, 54, 4)]);
        let view = a.sim.view();
        assert_eq!(view.annexation(town2).map(|x| x.attacker), Some(P1));
        assert_eq!(view.building_hp(seat), Some((2000, 2000)));
        let roster = a.units_of(P0, "skirmisher");
        let mut mem = BotMemory {
            wave: Some(wave(roster, a.seat(P1), WavePhase::Assault)),
            ..BotMemory::default()
        };
        let mut ctx = a.ctx(P0);
        let threats = threats(&ctx);
        let pops: Vec<(BuildingId, u32)> = threats.iter().map(|(t, _, p)| (*t, *p)).collect();
        assert_eq!(pops, [(seat, 4), (town2, 2)]);
        step(&mut ctx, &mut mem);
        assert_eq!(
            mem.defend.map(|d| d.town),
            Some(seat),
            "the Seat is engaged"
        );
        assert!(mem.wave.is_none(), "the contested Town calls the wave home");
    }

    /// With no threat for `defend_release_ds` the latch is released and
    /// the guard attack-moves back to the rally; before that it holds.
    #[test]
    fn release_after_quiet_returns_with_attack_move() {
        let r = crate::testkit::rules();
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            vec![spawn(&r, P0, "skirmisher", 24, 50, 2)],
        );
        a.run(400);
        let tick = a.sim.tick();
        let seat = a.seat(P0);
        let guards = a.units_of(P0, "skirmisher");
        let latch = |quiet: u32| BotMemory {
            defend: Some(Defend {
                town: seat,
                last_threat: tick - quiet,
                since: tick - 400,
            }),
            ..BotMemory::default()
        };
        let mut mem = latch(100);
        let mut ctx = a.ctx(P0);
        step(&mut ctx, &mut mem);
        assert!(mem.defend.is_some(), "100 ticks are not quiet enough");
        assert!(ctx.into_commands().is_empty());
        let mut mem = latch(300);
        let mut ctx = a.ctx(P0);
        let rally = ctx.rally();
        step(&mut ctx, &mut mem);
        assert_eq!((mem.defend, mem.defends), (None, 1));
        assert_eq!(
            ctx.into_commands(),
            [Command::AttackMove {
                units: guards,
                target: rally
            }]
        );
    }

    /// A threat starts production before the build order's `Military`
    /// latch.
    #[test]
    fn emergency_production_before_the_military_latch() {
        let r = crate::testkit::rules();
        let setup = [
            vec![
                place(&r, P0, "muster_hall", 29, 66),
                spawn(&r, P1, "skirmisher", 24, 51, 1),
            ],
            grant_all(&r, P0, 500),
        ]
        .concat();
        let a = Arena::frozen(standard_vs_passive(&r), setup);
        let mh = a.buildings_of(P0, "muster_hall")[0];
        let (cmds, memory) = a.plan(P0);
        let mem = BotMemory::decode(memory.bytes());
        assert!(!mem.military && mem.defend.is_some());
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::Train { building, .. } if *building == mh)),
            "{cmds:?}"
        );
        // Without the raider, the same plan trains no army.
        let quiet = Arena::frozen(
            standard_vs_passive(&r),
            [
                vec![place(&r, P0, "muster_hall", 29, 66)],
                grant_all(&r, P0, 500),
            ]
            .concat(),
        );
        let mh = quiet.buildings_of(P0, "muster_hall")[0];
        let (cmds, _) = quiet.plan(P0);
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::Train { building, .. } if *building == mh)),
            "{cmds:?}"
        );
    }

    /// A Town being annexed has a neutral border, so its attackers stand on
    /// no one's land: they are threats all the same, the guard attack-moves
    /// to them, and an assaulting wave is called home.
    #[test]
    fn a_contested_town_is_defended_though_its_land_went_neutral() {
        let r = crate::testkit::rules();
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                spawn(&r, P0, "skirmisher", 44, 64, 2),
                spawn(&r, P0, "skirmisher", 60, 64, 2),
            ],
        );
        let seat = a.seat(P0);
        a.issue(
            P0,
            vec![
                Command::DebugDamage {
                    target: TargetId::Building(seat),
                    amount: 5000,
                },
                spawn(&r, P1, "skirmisher", 22, 64, 1),
            ],
        );
        a.run(5);
        let view = a.sim.view();
        let attacker = a.units_of(P1, "skirmisher")[0];
        let annex = view.annexation(seat).expect("the Seat is contested");
        assert_eq!(annex.attacker, P1);
        let at = view.unit(attacker).unwrap().pos;
        assert_eq!(view.territory().owner_at(view.map().tile_of(at)), None);
        let all = a.units_of(P0, "skirmisher");
        let (guard, roster) = all.split_at(2);
        let mut mem = BotMemory {
            wave: Some(wave(roster.to_vec(), a.seat(P1), WavePhase::Assault)),
            ..BotMemory::default()
        };
        let mut ctx = a.ctx(P0);
        assert!(ctx.facts.threats.is_empty(), "not on own land");
        step(&mut ctx, &mut mem);
        assert_eq!(mem.defend.map(|d| d.town), Some(seat));
        assert!(mem.wave.is_none(), "the assaulting wave comes home");
        let cmds = ctx.into_commands();
        assert!(
            cmds.contains(&Command::AttackMove {
                units: guard.to_vec(),
                target: at
            }),
            "{cmds:?}"
        );
    }

    /// M6: with `defend_economy`, enemy military within
    /// `defend_economy_tiles` of an own worked gathering building is a
    /// threat to the Town it belongs to, though it stands outside
    /// `defend_radius_tiles` of the Town; without it, it is not.
    #[test]
    fn defend_economy_answers_a_raid_on_a_lumber_yard() {
        let r = crate::testkit::rules();
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                place(&r, P0, "croft", 24, 82),
                spawn(&r, P0, "yeoman", 24, 80, 1),
            ],
        );
        let croft = a.buildings_of(P0, "croft")[0];
        let yeoman = *a.units_of(P0, "yeoman").last().unwrap();
        a.issue(
            P0,
            vec![Command::Gather {
                units: vec![yeoman],
                node: croft,
            }],
        );
        a.issue(P0, vec![spawn(&r, P1, "skirmisher", 26, 88, 2)]);
        let raiders = a.units_of(P1, "skirmisher");
        let seat = a.seat(P0);
        let ctx = a.ctx(P0);
        assert!(ctx.knobs.defend_economy);
        let centre = a
            .sim
            .view()
            .building(seat)
            .map(|b| kind_centre(&r, b.kind, b.tile))
            .unwrap();
        for u in &raiders {
            let p = a.sim.view().unit(*u).unwrap().pos;
            assert!(p.dist_sq_i64(centre) > tiles_sq(ctx.knobs.defend_radius_tiles));
        }
        let found = threats(&ctx);
        assert_eq!(found, vec![(seat, raiders.clone(), 2)]);
        assert_eq!(engaged(&found), Some(seat));
        let mut off = ctx.knobs.clone();
        off.defend_economy = false;
        let ctx = crate::standard::Ctx::new(a.sim.view(), P0, &off, a.map(P0));
        assert_eq!(engaged(&threats(&ctx)), None, "without the rule");
    }

    /// M6: with `defend_wains_first` the guards nearest an enemy Supply
    /// Wain among the threats attack it explicitly; without it the Wain
    /// gets no explicit attack.
    #[test]
    fn tower_guards_attack_wains_first() {
        let r = crate::testkit::rules();
        let a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                spawn(&r, P0, "skirmisher", 30, 64, 6),
                spawn(&r, P1, "supply_wain", 33, 58, 1),
                spawn(&r, P1, "skirmisher", 33, 70, 2),
            ],
        );
        let wain = a.units_of(P1, "supply_wain")[0];
        let tower = r.ai.personality("tower").unwrap();
        assert!(tower.defend_wains_first);
        let mut k = r.ai.personality("standard").unwrap().clone();
        k.defend_wains_first = true;
        let mut ctx = crate::standard::Ctx::new(a.sim.view(), P0, &k, a.map(P0));
        let mut mem = BotMemory::default();
        step(&mut ctx, &mut mem);
        let cmds = ctx.into_commands();
        let on_wain: Vec<&Command> = cmds
            .iter()
            .filter(
                |c| matches!(c, Command::Attack { target, .. } if *target == TargetId::Unit(wain)),
            )
            .collect();
        assert_eq!(on_wain.len(), 1, "{cmds:?}");
        let Command::Attack { units, .. } = on_wain[0] else {
            unreachable!()
        };
        assert_eq!(units.len(), SIEGE_GUARDS);
        k.defend_wains_first = false;
        let mut ctx = crate::standard::Ctx::new(a.sim.view(), P0, &k, a.map(P0));
        let mut mem = BotMemory::default();
        step(&mut ctx, &mut mem);
        assert!(
            !ctx.into_commands().iter().any(
                |c| matches!(c, Command::Attack { target, .. } if *target == TargetId::Unit(wain))
            ),
            "without the rule"
        );
    }

    /// M6: a released defence with `reactive_towers` left marks the
    /// defended Town, and the plan's build-order rule then places one
    /// Watchtower near it (counting it); a personality with none places
    /// nothing.
    #[test]
    fn reactive_tower_after_a_defence() {
        let r = crate::testkit::rules();
        let a = Arena::frozen(standard_vs_passive(&r), grant_all(&r, P0, 200));
        let seat = a.seat(P0);
        let tick = a.sim.tick();
        let mut k = r.ai.personality("standard").unwrap().clone();
        k.reactive_towers = 1;
        let released = BotMemory {
            defend: Some(Defend {
                town: seat,
                last_threat: 0,
                since: 0,
            }),
            ..BotMemory::default()
        };
        assert!(tick >= a.sim.rules().ticks_from_ds(k.defend_release_ds) || tick > 0);
        let mut a = a;
        a.run(400);
        let mut ctx = crate::standard::Ctx::new(a.sim.view(), P0, &k, a.map(P0));
        let mut mem = released.clone();
        step(&mut ctx, &mut mem);
        assert_eq!((mem.defend, mem.last_defended), (None, Some(seat)));
        crate::standard::build_order::reactive_tower(&mut ctx, &mut mem);
        let tower = sim::BuildingKindId(r.building_kind_by_id("watchtower").unwrap().0);
        let cmds = ctx.into_commands();
        let built: Vec<Tile> = cmds
            .iter()
            .filter_map(|c| match c {
                Command::Build { kind, tile, .. } if *kind == tower => Some(*tile),
                _ => None,
            })
            .collect();
        assert_eq!(built.len(), 1, "{cmds:?}");
        let c = a
            .sim
            .view()
            .building(seat)
            .map(|b| kind_centre(&r, b.kind, b.tile))
            .unwrap();
        let at = a.sim.view().map().center_of(built[0]);
        assert!(at.dist_sq_i64(c) <= tiles_sq(k.near_town_max_tiles + 1));
        assert_eq!((mem.extra_towers, mem.last_defended), (1, None));
        // Once the allowance is spent, a release marks nothing.
        let mut ctx = crate::standard::Ctx::new(a.sim.view(), P0, &k, a.map(P0));
        let mut mem = BotMemory {
            extra_towers: 1,
            ..released.clone()
        };
        step(&mut ctx, &mut mem);
        assert_eq!(mem.last_defended, None);
        // The standard personality has none.
        let mut ctx = a.ctx(P0);
        let mut mem = released;
        step(&mut ctx, &mut mem);
        assert_eq!(mem.last_defended, None);
    }
}
