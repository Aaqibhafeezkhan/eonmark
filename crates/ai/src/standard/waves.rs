//! Attack waves with a Supply Wain attached (docs/design/ai.md rule 5; M5b
//! owner decision 4).
//!
//! A wave's roster lives in the memory (`BotMemory::wave`), so it is never
//! confused with the home army, and goes through these phases:
//!
//! - **Launch.** No wave is active, the defend latch does not block
//!   launches ([`launch_blocked`]), the bot has reached
//!   `wave_requires_age`, the tick has reached `next_wave_at` and
//!   `first_wave_not_before_ds`, at least [`size`] home armed units stand
//!   within `assemble_radius_tiles` of the rally with at least `min_siege`
//!   siege among them, and a Supply Wain is home. The roster is every
//!   assembled armed unit, siege first then by id, at most
//!   `max_wave_units`, plus one Wain: the armed units attack-move and the
//!   Wain moves to the stage point S ([`stage_point`]), and the next
//!   launch waits `wave_interval_ds`.
//! - **Stage.** Units not on station are sent to S again. The assault
//!   starts once `stage_ready_permille` of the roster's armed units and
//!   every roster siege unit stand within `stage_radius_tiles` of S, or
//!   `stage_max_ds` after the stage began. In that one plan every armed
//!   unit attack-moves to the target Town's centre and the Wain moves to
//!   the Wain point W ([`wain_point`]), so the observer's
//!   `first_attack_tick` counts the whole wave and its Wain together.
//! - **Assault** (the target has hit points). Siege within
//!   `siege_focus_tiles` of the target gets an explicit `Attack` on it;
//!   units not on station attack-move to it again; the Wain is moved back
//!   to W when it is more than `wain_leash_tiles` from it (W moves up as
//!   enemy shooters fall).
//! - **Ring hold** (the target at 0 HP, still the enemy's). The passable
//!   ring tiles of its footprint go, nearest first, to up to `ring_units`
//!   roster units that can annex (units already on the ring count), each
//!   with a single-unit `Move` (M5a's hold rule keeps them there); every
//!   enemy non-civilian unit within `annexation_defender_radius_tiles + 2`
//!   of the Town's centre gets an `Attack` from up to
//!   `clear_defenders_units` of the other roster units; the rest stay on
//!   station at the Town.
//! - **Continue.** Once the target is the bot's, a wave of at least
//!   `continue_min_units` armed units takes the enemy Town nearest the
//!   rally as its next target, with a fresh stage point and timer;
//!   otherwise it comes home.
//! - **End.** A wave with no unit that can annex, fewer than
//!   `recall_below_units` armed units, or older than `wave_max_ds` while
//!   no annexation of the bot's runs at the target comes home: its units
//!   attack-move (the Wain moves) to the rally and the wave ends.
//!
//! **All-in** (from `all_in_after_ds`): every home armed unit joins the
//! active wave each plan and is ordered like the rest of it; a wave ends
//! only when none of its armed units is left, and is never recalled
//! (`defend::should_recall`); a launch needs one home armed unit anywhere,
//! no siege, no Wain and no wait for `next_wave_at`.
//!
//! "On station" (no new order) is [`on_station`] with a tolerance that
//! grows with the group: a group order gives each unit its own tile of a
//! spiral around the point, and around a Town the spiral starts on its
//! ring. A unit on an explicit `Attack` whose target still exists is left
//! alone. The wave issues `AttackMove` into enemy land only at the assault
//! (and the all-in's joiners then), so the first such tick is some wave's
//! assault order (a wave that ends before its assault issues none).

use super::army::home_armed;
use super::ctx::{Ctx, goal_or_post, on_station};
use super::defend::{all_in, launch_blocked, recall_wave};
use super::facts::{kind_centre, kind_centre_tile, tiles_sq};
use super::memory::{BotMemory, Wave, WavePhase};
use rules::{Rules, UnitClass};
use sim::{Building, BuildingId, Command, Fx, FxVec2, Intent, TargetId, Tile, UnitId, UnitKindId};
use std::collections::BTreeSet;

/// A footprint as inclusive tile bounds `(x0, y0, x1, y1)`.
pub type Rect = (i32, i32, i32, i32);

/// The footprint of building `b`.
///
/// # Panics
///
/// If its kind is not in the rules.
pub fn footprint(rules: &Rules, b: &Building) -> Rect {
    let def = rules
        .building_kind(b.kind.0)
        .expect("a building's kind exists");
    let w = i32::try_from(def.width_tiles).expect("validated <= 8");
    let h = i32::try_from(def.height_tiles).expect("validated <= 8");
    let (x, y) = (i32::from(b.tile.x), i32::from(b.tile.y));
    (x, y, x + w - 1, y + h - 1)
}

/// Squared distance from `p` to the area of the footprint `r` (its tiles'
/// area, `[x0, x1 + 1] x [y0, y1 + 1]`), in [`FxVec2::dist_sq_i64`]'s
/// scale: what a shooter measures to a unit's centre.
pub fn dist_sq_to_rect(p: FxVec2, r: Rect) -> i64 {
    let x = p.x.clamp(Fx::from_int(r.0), Fx::from_int(r.2 + 1));
    let y = p.y.clamp(Fx::from_int(r.1), Fx::from_int(r.3 + 1));
    p.dist_sq_i64(FxVec2::new(x, y))
}

/// `v` hundredths of a tile, squared, in [`FxVec2::dist_sq_i64`]'s scale.
fn x100_sq(v: u32) -> i64 {
    ((i64::from(v) * i64::from(v)) << 32) / 10_000
}

/// Chebyshev distance from tile `t` to the footprint `r` (0 inside).
pub fn chebyshev(r: Rect, t: Tile) -> i32 {
    let (x, y) = (i32::from(t.x), i32::from(t.y));
    let dx = (r.0 - x).max(x - r.2).max(0);
    let dy = (r.1 - y).max(y - r.3).max(0);
    dx.max(dy)
}

/// The enemy's complete shooters with hit points left: footprint and
/// weapon range in hundredths of a tile.
pub fn shooters(ctx: &Ctx<'_>) -> Vec<(Rect, u32)> {
    let rules = ctx.view.rules();
    ctx.facts
        .enemy_shooters
        .iter()
        .filter_map(|id| {
            let b = ctx.view.building(*id)?;
            let a = rules.building_kind(b.kind.0)?.attack.as_ref()?;
            Some((footprint(rules, b), a.range_tiles_x100))
        })
        .collect()
}

/// Whether `p` lies more than `extra_tiles` beyond the range of every
/// shooter in `shooters`.
pub fn out_of_reach(shooters: &[(Rect, u32)], p: FxVec2, extra_tiles: u32) -> bool {
    shooters.iter().all(|(r, range)| {
        dist_sq_to_rect(p, *r) > x100_sq(range.saturating_add(extra_tiles.saturating_mul(100)))
    })
}

/// `num / den` rounded half away from zero (`den > 0`).
fn div_round(num: i32, den: i32) -> i32 {
    if num >= 0 {
        (2 * num + den) / (2 * den)
    } else {
        -((-2 * num + den) / (2 * den))
    }
}

/// The tiles of the integer segment from `a` to `b`, `a` first and `b`
/// last, one per step along the longer axis.
pub fn segment(a: Tile, b: Tile) -> Vec<Tile> {
    let (ax, ay) = (i32::from(a.x), i32::from(a.y));
    let (dx, dy) = (i32::from(b.x) - ax, i32::from(b.y) - ay);
    let n = dx.abs().max(dy.abs());
    if n == 0 {
        return vec![a];
    }
    (0..=n)
        .map(|k| {
            let x = ax + div_round(dx * k, n);
            let y = ay + div_round(dy * k, n);
            Tile::new(
                u16::try_from(x).expect("between two map tiles"),
                u16::try_from(y).expect("between two map tiles"),
            )
        })
        .collect()
}

/// How far a group order of `n` units spreads around its point: the
/// half-side of the square spiral that holds them, plus one.
pub fn spread(n: usize) -> u32 {
    u32::try_from(n.isqrt() / 2 + 1).unwrap_or(u32::MAX)
}

/// The armed units a wave needs to launch now: the `wave_sizes` row of
/// the tick, at most the home armed units plus the free pop (so a full pop
/// never deadlocks a launch), at least 1.
pub fn size(ctx: &Ctx<'_>, home: usize) -> u32 {
    let tick = ctx.tick();
    let table = ctx
        .knobs
        .wave_sizes
        .iter()
        .rev()
        .find(|w| ctx.ticks(w.from_ds) <= tick)
        .map(|w| w.armed)
        .unwrap_or(1);
    let (used, cap) = ctx.facts.pop;
    let home = u32::try_from(home).unwrap_or(u32::MAX);
    table
        .min(home.saturating_add(cap.saturating_sub(used)))
        .max(1)
}

/// Home armed units a launch or the all-in leaves at home: one while the
/// pop is full (nothing could replace it), else none.
pub fn home_guard(ctx: &Ctx<'_>) -> usize {
    let (used, cap) = ctx.facts.pop;
    usize::from(used >= cap)
}

/// The wave's target: the complete enemy Town nearest the rally, ties to
/// the lowest id (the one function influence maps replace in M6).
pub fn choose_target(ctx: &mut Ctx<'_>) -> Option<BuildingId> {
    let rally = ctx.rally();
    let rules = ctx.view.rules();
    let view = ctx.view;
    ctx.facts
        .enemy_towns
        .iter()
        .filter_map(|id| {
            let b = view.building(*id)?;
            Some((kind_centre(rules, b.kind, b.tile).dist_sq_i64(rally), *id))
        })
        .min()
        .map(|(_, id)| id)
}

/// The stage point for a wave of `n` units against `target`: walking the
/// segment from the target's centre tile toward the rally, the first tile
/// with no other player's land within `stage_margin_tiles + spread(n)`
/// (Chebyshev) and more than that beyond every enemy shooter's range,
/// snapped to a passable tile; the rally's tile when none is.
pub fn stage_point(ctx: &mut Ctx<'_>, target: BuildingId, n: usize) -> Tile {
    let rally = ctx.rally();
    let view = ctx.view;
    let map = view.map();
    let rally_tile = map.tile_of(rally);
    let Some(b) = view.building(target) else {
        return rally_tile;
    };
    let t = kind_centre_tile(view.rules(), b.kind, b.tile);
    let margin = ctx.knobs.stage_margin_tiles.saturating_add(spread(n));
    let m = i32::try_from(margin).unwrap_or(i32::MAX / 4);
    let shooters = shooters(ctx);
    let territory = view.territory();
    let me = ctx.me;
    let clear = |tile: Tile| {
        let foreign = (-m..=m).any(|dy| {
            (-m..=m).any(|dx| {
                map.offset(tile, dx, dy)
                    .is_some_and(|o| territory.owner_at(o).is_some_and(|p| p != me))
            })
        });
        !foreign && out_of_reach(&shooters, map.center_of(tile), margin)
    };
    let found = segment(t, rally_tile)
        .into_iter()
        .find(|x| clear(*x))
        .unwrap_or(rally_tile);
    map.nearest_passable(found, None).unwrap_or(found)
}

/// The Supply Wain's point W for an assault on `target` from `stage`: the
/// passable tile of the segment from the target's centre tile to the
/// stage point nearest the target that lies more than
/// `wain_standoff_tiles` beyond every enemy shooter's range (the stage
/// point when none is). While the target still shoots, W keeps out of its
/// range; at 0 HP it moves up.
pub fn wain_point(ctx: &Ctx<'_>, target: BuildingId, stage: Tile) -> FxVec2 {
    let view = ctx.view;
    let map = view.map();
    let Some(b) = view.building(target) else {
        return map.center_of(stage);
    };
    let t = kind_centre_tile(view.rules(), b.kind, b.tile);
    let shooters = shooters(ctx);
    let standoff = ctx.knobs.wain_standoff_tiles;
    segment(t, stage)
        .into_iter()
        .find(|x| map.passable(*x) && out_of_reach(&shooters, map.center_of(*x), standoff))
        .map_or_else(|| map.center_of(stage), |x| map.center_of(x))
}

/// Whether unit kind `k` is siege.
fn is_siege(rules: &Rules, k: UnitKindId) -> bool {
    rules
        .unit_kind(k.0)
        .is_some_and(|d| d.class == UnitClass::Siege)
}

/// Whether unit kind `k` annexes (`annexation_attackers` selects it).
fn annexes(rules: &Rules, k: UnitKindId) -> bool {
    rules
        .unit_kind(k.0)
        .is_some_and(|d| rules.annexation_attackers.iter().any(|s| s.selects(d)))
}

/// Whether `u` is on an explicit `Attack` whose target still stands.
fn attacking(ctx: &Ctx<'_>, u: UnitId) -> bool {
    let Some(unit) = ctx.view.unit(u) else {
        return false;
    };
    unit.intent == Intent::Attack
        && unit.target.is_some_and(|t| match t {
            TargetId::Unit(id) => ctx.view.unit(id).is_some(),
            TargetId::Building(id) => ctx.view.building_hp(id).is_some_and(|(hp, _)| hp > 0),
        })
}

/// Run the rule: launch a wave, or run the active one.
pub fn step(ctx: &mut Ctx<'_>, mem: &mut BotMemory) {
    if mem.wave.is_some() {
        run(ctx, mem);
    } else {
        launch(ctx, mem);
    }
}

/// Whether a launch is due but for the defence and the units: no wave is
/// out, the bot has reached `wave_requires_age`, and the tick has reached
/// `next_wave_at` and `first_wave_not_before_ds` (always, during the
/// all-in). Past its cap the defence sizes its guard by it
/// ([`super::defend`]), so the assembled units are left to the wave.
pub fn launch_due(ctx: &Ctx<'_>, mem: &BotMemory) -> bool {
    if mem.wave.is_some() {
        return false;
    }
    if let Some(age) = &ctx.knobs.wave_requires_age
        && ctx
            .view
            .rules()
            .age_index(age)
            .is_none_or(|a| ctx.facts.age < a)
    {
        return false;
    }
    let not_before = mem
        .next_wave_at
        .max(ctx.ticks(ctx.knobs.first_wave_not_before_ds));
    all_in(ctx) || ctx.tick() >= not_before
}

/// Launch a wave when every condition holds (see the module docs).
fn launch(ctx: &mut Ctx<'_>, mem: &mut BotMemory) {
    let tick = ctx.tick();
    let rules = ctx.view.rules();
    let all_in = all_in(ctx);
    if launch_blocked(ctx, mem) || ctx.commands_left < 2 || !launch_due(ctx, mem) {
        return;
    }
    let home = home_armed(ctx, mem);
    let rally = ctx.rally();
    let r2 = tiles_sq(ctx.knobs.assemble_radius_tiles);
    let view = ctx.view;
    let assembled: Vec<UnitId> = home
        .iter()
        .copied()
        .filter(|u| {
            all_in
                || view
                    .unit(*u)
                    .is_some_and(|x| x.pos.dist_sq_i64(rally) <= r2)
        })
        .collect();
    let keep = home_guard(ctx);
    let (need, min_siege) = if all_in {
        (1, 0)
    } else {
        (
            size(ctx, home.len().saturating_sub(keep)),
            ctx.knobs.min_siege,
        )
    };
    let siege = |u: &UnitId| view.unit(*u).is_some_and(|x| is_siege(rules, x.kind));
    let sieges = assembled.iter().filter(|u| siege(u)).count();
    let going = assembled.len().saturating_sub(keep);
    if u32::try_from(going).unwrap_or(u32::MAX) < need
        || u32::try_from(sieges).unwrap_or(u32::MAX) < min_siege
    {
        return;
    }
    let wain = ctx
        .facts
        .supply
        .iter()
        .copied()
        .find(|w| !ctx.is_claimed(*w));
    if wain.is_none() && !all_in {
        return;
    }
    let Some(target) = choose_target(ctx) else {
        return;
    };
    let mut roster = assembled;
    roster.sort_by_key(|u| (!siege(u), *u));
    roster.truncate(going.min(usize::try_from(ctx.knobs.max_wave_units).unwrap_or(usize::MAX)));
    roster.sort();
    let stage = stage_point(ctx, target, roster.len() + usize::from(wain.is_some()));
    let s = ctx.view.map().center_of(stage);
    for u in roster.iter().chain(wain.iter()) {
        ctx.claim(*u);
    }
    ctx.order(roster.clone(), |units| Command::AttackMove {
        units,
        target: s,
    });
    if let Some(w) = wain {
        ctx.push(Command::Move {
            units: vec![w],
            target: s,
            queue: false,
        });
    }
    mem.wave = Some(Wave {
        units: roster,
        wain,
        target,
        stage,
        phase: WavePhase::Stage,
        launched_at: tick,
        phase_at: tick,
    });
    mem.waves_launched = mem.waves_launched.saturating_add(1);
    mem.next_wave_at = tick.saturating_add(ctx.ticks(ctx.knobs.wave_interval_ds));
}

/// End the wave: its units come home ([`recall_wave`]). Waits for a plan
/// with room for the two commands, so no unit is left behind.
fn end(ctx: &mut Ctx<'_>, mem: &mut BotMemory, wave: Wave) {
    mem.wave = Some(wave);
    if ctx.commands_left >= 2 {
        recall_wave(ctx, mem);
    }
}

/// Run the active wave (see the module docs).
fn run(ctx: &mut Ctx<'_>, mem: &mut BotMemory) {
    let Some(mut wave) = mem.wave.clone() else {
        return;
    };
    let view = ctx.view;
    let rules = view.rules();
    let me = ctx.me;
    let tick = ctx.tick();
    let mine = |u: &UnitId| view.unit(*u).is_some_and(|x| x.owner == me);
    wave.units.retain(mine);
    if wave.wain.is_some_and(|w| !mine(&w)) {
        wave.wain = None;
    }
    let all_in = all_in(ctx);
    if all_in {
        let max = usize::try_from(ctx.knobs.max_wave_units).unwrap_or(usize::MAX);
        let room = max.saturating_sub(wave.units.len());
        let home = home_armed(ctx, mem);
        let free = home.len().saturating_sub(home_guard(ctx));
        let joiners: Vec<UnitId> = home.into_iter().take(room.min(free)).collect();
        wave.units.extend(joiners);
        wave.units.sort();
    }
    let annexers = wave
        .units
        .iter()
        .filter(|u| view.unit(**u).is_some_and(|x| annexes(rules, x.kind)))
        .count();
    let armed = u32::try_from(wave.units.len()).unwrap_or(u32::MAX);
    let Some(target) = view.building(wave.target) else {
        end(ctx, mem, wave);
        return;
    };
    if target.owner == me {
        if !all_in && armed < ctx.knobs.continue_min_units {
            end(ctx, mem, wave);
            return;
        }
        let Some(next) = choose_target(ctx) else {
            end(ctx, mem, wave);
            return;
        };
        wave.target = next;
        wave.stage = stage_point(ctx, next, wave.units.len() + 1);
        wave.phase = WavePhase::Stage;
        wave.launched_at = tick;
        wave.phase_at = tick;
        stage_upkeep(ctx, &wave);
        mem.wave = Some(wave);
        return;
    }
    let annexing = view
        .annexation(wave.target)
        .is_some_and(|a| a.attacker == me);
    let over = if all_in {
        wave.units.is_empty()
    } else {
        annexers == 0
            || armed < ctx.knobs.recall_below_units
            || (tick.saturating_sub(wave.launched_at) >= ctx.ticks(ctx.knobs.wave_max_ds)
                && !annexing)
    };
    if over {
        end(ctx, mem, wave);
        return;
    }
    match wave.phase {
        WavePhase::Stage => {
            if staged(ctx, &wave) && ctx.commands_left >= 3 {
                start_assault(ctx, &mut wave);
            } else {
                stage_upkeep(ctx, &wave);
            }
        }
        WavePhase::Assault | WavePhase::Hold => {
            if view.building_hp(wave.target).is_some_and(|(hp, _)| hp > 0) {
                wave.phase = WavePhase::Assault;
                assault_upkeep(ctx, &wave);
            } else {
                wave.phase = WavePhase::Hold;
                hold(ctx, &wave);
            }
        }
    }
    mem.wave = Some(wave);
}

/// Whether the stage is over: `stage_ready_permille` of the armed units
/// and every siege unit within `stage_radius_tiles` of the stage point, or
/// `stage_max_ds` since the stage began.
fn staged(ctx: &Ctx<'_>, wave: &Wave) -> bool {
    if ctx.tick().saturating_sub(wave.phase_at) >= ctx.ticks(ctx.knobs.stage_max_ds) {
        return true;
    }
    let view = ctx.view;
    let rules = view.rules();
    let s = view.map().center_of(wave.stage);
    let r2 = tiles_sq(ctx.knobs.stage_radius_tiles);
    let mut near = 0u64;
    for u in &wave.units {
        let Some(x) = view.unit(*u) else { continue };
        let here = x.pos.dist_sq_i64(s) <= r2;
        if here {
            near += 1;
        } else if is_siege(rules, x.kind) {
            return false;
        }
    }
    let total = u64::try_from(wave.units.len()).unwrap_or(u64::MAX);
    near * 1000 >= total * u64::from(ctx.knobs.stage_ready_permille)
}

/// The on-station tolerance of a wave of `n` units around a point.
fn station(ctx: &Ctx<'_>, n: usize) -> u32 {
    ctx.knobs.goal_tolerance_tiles.saturating_add(spread(n))
}

/// The on-station tolerance around the target Town's centre: a group's
/// spiral starts on the Town's ring, outside its footprint.
fn town_station(ctx: &Ctx<'_>, wave: &Wave) -> u32 {
    let half = ctx
        .view
        .building(wave.target)
        .map(|b| footprint(ctx.view.rules(), b))
        .map_or(0, |r| {
            u32::try_from((r.2 - r.0).max(r.3 - r.1) / 2 + 1).unwrap_or(0)
        });
    station(ctx, wave.units.len()).saturating_add(half)
}

/// Units of `units` not claimed, not on an explicit attack and not on
/// station within `tol` tiles of `goal`.
fn off_station(ctx: &Ctx<'_>, units: &[UnitId], goal: FxVec2, tol: u32) -> Vec<UnitId> {
    units
        .iter()
        .copied()
        .filter(|u| !ctx.is_claimed(*u) && !attacking(ctx, *u))
        .filter(|u| ctx.view.unit(*u).is_some_and(|x| !on_station(x, goal, tol)))
        .collect()
}

/// Keep the Wain at `at`: a `Move` when its goal is more than `leash`
/// tiles away.
fn keep_wain(ctx: &mut Ctx<'_>, wave: &Wave, at: FxVec2, leash: u32) {
    let Some(w) = wave.wain else { return };
    if ctx.is_claimed(w) || ctx.view.unit(w).is_none_or(|x| on_station(x, at, leash)) {
        return;
    }
    ctx.claim(w);
    ctx.push(Command::Move {
        units: vec![w],
        target: at,
        queue: false,
    });
}

/// Staging: units off station attack-move to the stage point again, the
/// Wain moves there.
fn stage_upkeep(ctx: &mut Ctx<'_>, wave: &Wave) {
    let s = ctx.view.map().center_of(wave.stage);
    let tol = station(ctx, wave.units.len());
    let moves = off_station(ctx, &wave.units, s, tol);
    for u in &moves {
        ctx.claim(*u);
    }
    ctx.order(moves, |units| Command::AttackMove { units, target: s });
    keep_wain(ctx, wave, s, tol);
}

/// The assault's first plan: every armed unit attack-moves to the target
/// Town (siege as its own group) and the Wain moves to W.
fn start_assault(ctx: &mut Ctx<'_>, wave: &mut Wave) {
    let view = ctx.view;
    let rules = view.rules();
    let Some(b) = view.building(wave.target) else {
        return;
    };
    let centre = kind_centre(rules, b.kind, b.tile);
    let (siege, other): (Vec<UnitId>, Vec<UnitId>) = wave
        .units
        .iter()
        .copied()
        .filter(|u| !ctx.is_claimed(*u))
        .partition(|u| view.unit(*u).is_some_and(|x| is_siege(rules, x.kind)));
    for u in siege.iter().chain(other.iter()) {
        ctx.claim(*u);
    }
    ctx.order(other, |units| Command::AttackMove {
        units,
        target: centre,
    });
    ctx.order(siege, |units| Command::AttackMove {
        units,
        target: centre,
    });
    let w = wain_point(ctx, wave.target, wave.stage);
    if let Some(wain) = wave.wain
        && !ctx.is_claimed(wain)
    {
        ctx.claim(wain);
        ctx.push(Command::Move {
            units: vec![wain],
            target: w,
            queue: false,
        });
    }
    wave.phase = WavePhase::Assault;
    wave.phase_at = ctx.tick();
}

/// The assault while the target has hit points: siege focus, units back
/// on station, the Wain on its leash.
fn assault_upkeep(ctx: &mut Ctx<'_>, wave: &Wave) {
    let view = ctx.view;
    let rules = view.rules();
    let Some(b) = view.building(wave.target) else {
        return;
    };
    let centre = kind_centre(rules, b.kind, b.tile);
    let focus_r2 = tiles_sq(ctx.knobs.siege_focus_tiles);
    let goal = TargetId::Building(wave.target);
    let focus: Vec<UnitId> = wave
        .units
        .iter()
        .copied()
        .filter(|u| !ctx.is_claimed(*u))
        .filter(|u| {
            view.unit(*u).is_some_and(|x| {
                is_siege(rules, x.kind)
                    && x.pos.dist_sq_i64(centre) <= focus_r2
                    && !(x.intent == Intent::Attack && x.target == Some(goal))
            })
        })
        .collect();
    if !focus.is_empty()
        && view
            .check_attack(ctx.me, &focus, goal)
            .is_ok_and(|v| !v.is_empty())
    {
        for u in &focus {
            ctx.claim(*u);
        }
        ctx.push(Command::Attack {
            units: focus,
            target: goal,
        });
    }
    let tol = town_station(ctx, wave);
    let moves = off_station(ctx, &wave.units, centre, tol);
    for u in &moves {
        ctx.claim(*u);
    }
    ctx.order(moves, |units| Command::AttackMove {
        units,
        target: centre,
    });
    let w = wain_point(ctx, wave.target, wave.stage);
    keep_wain(ctx, wave, w, ctx.knobs.wain_leash_tiles);
}

/// The ring hold while the target is at 0 HP and still the enemy's: ring
/// tiles to annexing units, defenders attacked, the rest on station.
fn hold(ctx: &mut Ctx<'_>, wave: &Wave) {
    let view = ctx.view;
    let rules = view.rules();
    let map = view.map();
    let Some(b) = view.building(wave.target) else {
        return;
    };
    let rect = footprint(rules, b);
    let centre = kind_centre(rules, b.kind, b.tile);
    let ring: Vec<Tile> = (rect.1 - 1..=rect.3 + 1)
        .flat_map(|y| (rect.0 - 1..=rect.2 + 1).map(move |x| (x, y)))
        .filter_map(|(x, y)| Some(Tile::new(u16::try_from(x).ok()?, u16::try_from(y).ok()?)))
        .filter(|t| map.passable(*t) && chebyshev(rect, *t) == 1)
        .collect();
    let ring_set: BTreeSet<Tile> = ring.iter().copied().collect();
    // A unit is on the ring when it stands on a ring tile or is heading
    // for (or holds) one.
    let ring_tile_of = |u: UnitId| -> Option<Tile> {
        let x = view.unit(u)?;
        let here = map.tile_of(x.pos);
        if ring_set.contains(&here) {
            return Some(here);
        }
        goal_or_post(x)
            .map(|g| map.tile_of(g))
            .filter(|t| ring_set.contains(t))
    };
    let mut on_ring: BTreeSet<UnitId> = BTreeSet::new();
    let mut taken: BTreeSet<Tile> = BTreeSet::new();
    for u in &wave.units {
        if view.unit(*u).is_some_and(|x| annexes(rules, x.kind))
            && let Some(t) = ring_tile_of(*u)
        {
            on_ring.insert(*u);
            taken.insert(t);
        }
    }
    let want = usize::try_from(ctx.knobs.ring_units).unwrap_or(usize::MAX);
    let mut free: Vec<Tile> = ring
        .iter()
        .copied()
        .filter(|t| !taken.contains(t))
        .collect();
    let mut candidates: Vec<UnitId> = wave
        .units
        .iter()
        .copied()
        .filter(|u| !on_ring.contains(u) && !ctx.is_claimed(*u))
        .filter(|u| view.unit(*u).is_some_and(|x| annexes(rules, x.kind)))
        .collect();
    while on_ring.len() < want && !free.is_empty() && !candidates.is_empty() {
        let Some((_, u, t)) = candidates
            .iter()
            .flat_map(|u| free.iter().map(move |t| (*u, *t)))
            .filter_map(|(u, t)| {
                let p = view.unit(u)?.pos;
                Some((p.dist_sq_i64(map.center_of(t)), u, t))
            })
            .min()
        else {
            break;
        };
        candidates.retain(|c| *c != u);
        free.retain(|f| *f != t);
        on_ring.insert(u);
        ctx.claim(u);
        if !ctx.push(Command::Move {
            units: vec![u],
            target: map.center_of(t),
            queue: false,
        }) {
            return;
        }
    }
    // Defenders: every enemy non-civilian unit near the Town's centre.
    let reach = tiles_sq(rules.annexation_defender_radius_tiles.saturating_add(2));
    let defenders: Vec<UnitId> = view
        .units()
        .filter(|x| x.owner != ctx.me && x.pos.dist_sq_i64(centre) <= reach)
        .filter(|x| {
            rules
                .unit_kind(x.kind.0)
                .is_some_and(|k| k.class != UnitClass::Civilian)
        })
        .map(|x| x.id)
        .collect();
    let per = usize::try_from(ctx.knobs.clear_defenders_units).unwrap_or(usize::MAX);
    let mut pool: Vec<UnitId> = wave
        .units
        .iter()
        .copied()
        .filter(|u| !on_ring.contains(u) && !ctx.is_claimed(*u))
        .collect();
    for d in defenders {
        let Some(dp) = view.unit(d).map(|x| x.pos) else {
            continue;
        };
        let already: Vec<UnitId> = pool
            .iter()
            .copied()
            .filter(|u| {
                view.unit(*u).is_some_and(|x| {
                    x.intent == Intent::Attack && x.target == Some(TargetId::Unit(d))
                })
            })
            .collect();
        pool.retain(|u| !already.contains(u));
        let mut near: Vec<(i64, UnitId)> = pool
            .iter()
            .filter_map(|u| Some((view.unit(*u)?.pos.dist_sq_i64(dp), *u)))
            .collect();
        near.sort();
        let send: Vec<UnitId> = near
            .into_iter()
            .take(per.saturating_sub(already.len()))
            .map(|(_, u)| u)
            .collect();
        if send.is_empty() {
            continue;
        }
        pool.retain(|u| !send.contains(u));
        if view
            .check_attack(ctx.me, &send, TargetId::Unit(d))
            .is_ok_and(|v| !v.is_empty())
        {
            for u in &send {
                ctx.claim(*u);
            }
            if !ctx.push(Command::Attack {
                units: send,
                target: TargetId::Unit(d),
            }) {
                return;
            }
        }
    }
    let rest: Vec<UnitId> = wave
        .units
        .iter()
        .copied()
        .filter(|u| !on_ring.contains(u))
        .collect();
    let tol = town_station(ctx, wave);
    let moves = off_station(ctx, &rest, centre, tol);
    for u in &moves {
        ctx.claim(*u);
    }
    ctx.order(moves, |units| Command::AttackMove {
        units,
        target: centre,
    });
    let w = wain_point(ctx, wave.target, wave.stage);
    keep_wain(ctx, wave, w, ctx.knobs.wain_leash_tiles);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standard::defend;
    use crate::testkit::{Arena, P0, P1, place, spawn, standard_vs_passive};
    use rules::Personality;

    /// P0's rally in these arenas: the rally seed (36,64) is passable own
    /// land.
    const RALLY: (i32, i32) = (36, 64);

    fn wave(units: Vec<UnitId>, wain: Option<UnitId>, target: BuildingId, stage: Tile) -> Wave {
        Wave {
            units,
            wain,
            target,
            stage,
            phase: WavePhase::Stage,
            launched_at: 0,
            phase_at: 0,
        }
    }

    /// The setup of a launch at P0's rally: Masonry (or not), `skirm`
    /// Skirmishers, `mang` Mangonels and `wains` Supply Wains.
    fn at_rally(r: &Rules, age: bool, skirm: u16, mang: u16, wains: u16) -> Vec<Command> {
        let (x, y) = RALLY;
        let mut v = Vec::new();
        if age {
            v.push(Command::DebugAdvanceAge { owner: P0 });
        }
        for (kind, n) in [
            ("skirmisher", skirm),
            ("mangonel", mang),
            ("supply_wain", wains),
        ] {
            if n > 0 {
                v.push(spawn(r, P0, kind, x, y, n));
            }
        }
        v
    }

    fn knobs(r: &Rules, edit: impl FnOnce(&mut Personality)) -> Personality {
        let mut k = r.ai.personality("standard").unwrap().clone();
        edit(&mut k);
        k
    }

    /// A launch needs the age, the size, the siege, a Wain and the timer;
    /// then every assembled armed unit attack-moves and the Wain moves to
    /// the stage point, which lies off the enemy's land.
    #[test]
    fn launch_needs_age_size_siege_wain_and_timer() {
        let r = crate::testkit::rules();
        let a = Arena::frozen(standard_vs_passive(&r), at_rally(&r, true, 7, 1, 1));
        let mut armed = a.units_of(P0, "skirmisher");
        armed.extend(a.units_of(P0, "mangonel"));
        armed.sort();
        let wain = a.units_of(P0, "supply_wain")[0];
        let mut ctx = a.ctx(P0);
        assert_eq!(size(&ctx, 8), 8, "the table's first row");
        let mut mem = BotMemory::default();
        step(&mut ctx, &mut mem);
        let tick = a.sim.tick();
        let w = mem.wave.clone().expect("a wave launches");
        assert_eq!(w.units, armed);
        assert_eq!(
            (w.wain, w.target, w.phase),
            (Some(wain), a.seat(P1), WavePhase::Stage)
        );
        assert_eq!((w.launched_at, w.phase_at), (tick, tick));
        assert_eq!(mem.waves_launched, 1);
        assert_eq!(
            mem.next_wave_at,
            tick + ctx.ticks(ctx.knobs.wave_interval_ds)
        );
        let view = a.sim.view();
        let s = view.map().center_of(w.stage);
        assert_eq!(
            ctx.into_commands(),
            [
                Command::AttackMove {
                    units: armed,
                    target: s
                },
                Command::Move {
                    units: vec![wain],
                    target: s,
                    queue: false
                }
            ]
        );
        assert_ne!(view.territory().owner_at(w.stage), Some(P1));

        // Each condition missing in turn: no launch.
        for (why, setup) in [
            ("no Masonry", at_rally(&r, false, 7, 1, 1)),
            ("7 armed of 8", at_rally(&r, true, 6, 1, 1)),
            ("no siege", at_rally(&r, true, 8, 0, 1)),
            ("no Wain", at_rally(&r, true, 7, 1, 0)),
        ] {
            let a = Arena::frozen(standard_vs_passive(&r), setup);
            let mut ctx = a.ctx(P0);
            let mut mem = BotMemory::default();
            step(&mut ctx, &mut mem);
            assert!(mem.wave.is_none(), "{why}");
            assert!(ctx.into_commands().is_empty(), "{why}");
        }
        let mut ctx = a.ctx(P0);
        let mut mem = BotMemory {
            next_wave_at: a.sim.tick() + 1,
            ..BotMemory::default()
        };
        step(&mut ctx, &mut mem);
        assert!(mem.wave.is_none(), "the wave interval");
        // Away from the rally, the units are not assembled.
        let (x, y) = RALLY;
        let far = Arena::frozen(
            standard_vs_passive(&r),
            [
                at_rally(&r, true, 0, 1, 1),
                vec![spawn(&r, P0, "skirmisher", x - 14, y, 7)],
            ]
            .concat(),
        );
        let mut ctx = far.ctx(P0);
        let mut mem = BotMemory::default();
        step(&mut ctx, &mut mem);
        assert!(mem.wave.is_none(), "not assembled");
    }

    /// The wave size is the table's row, at most the home armed units plus
    /// the free pop, never below 1; at a full pop one armed unit stays
    /// home.
    #[test]
    fn size_respects_free_pop() {
        let r = crate::testkit::rules();
        let (x, y) = RALLY;
        let a = Arena::frozen(
            standard_vs_passive(&r),
            vec![spawn(&r, P0, "skirmisher", x, y, 5)],
        );
        let ctx = a.ctx(P0);
        assert_eq!(ctx.facts.pop, (10, 25));
        assert_eq!((size(&ctx, 5), home_guard(&ctx)), (8, 0));
        // A full pop: 5 + 13 Yeomen, 2 Skirmishers, a Mangonel and a Wain.
        let full = Arena::frozen(
            standard_vs_passive(&r),
            [
                at_rally(&r, true, 2, 1, 1),
                vec![spawn(&r, P0, "yeoman", 20, 70, 13)],
            ]
            .concat(),
        );
        let mut ctx = full.ctx(P0);
        assert_eq!(ctx.facts.pop, (25, 25));
        assert_eq!((size(&ctx, 3), size(&ctx, 0)), (3, 1));
        assert_eq!(home_guard(&ctx), 1);
        let mut mem = BotMemory::default();
        step(&mut ctx, &mut mem);
        let w = mem.wave.expect("a wave of two launches");
        let mangonel = full.units_of(P0, "mangonel")[0];
        let skirm = full.units_of(P0, "skirmisher");
        let mut want = vec![mangonel, skirm[0]];
        want.sort();
        assert_eq!(w.units, want, "siege first, one Skirmisher stays home");
    }

    /// The assault waits for every siege unit at the stage point (or for
    /// `stage_max_ds`), then sends the armed units at the target's centre,
    /// siege as its own group, and the Wain to its point.
    #[test]
    fn stage_waits_for_siege() {
        let r = crate::testkit::rules();
        let probe = Arena::frozen(standard_vs_passive(&r), Vec::new());
        let target = probe.seat(P1);
        let stage = stage_point(&mut probe.ctx(P0), target, 12);
        let (sx, sy) = (i32::from(stage.x), i32::from(stage.y));
        let (rx, ry) = RALLY;
        let seat_centre = {
            let view = probe.sim.view();
            let b = view.building(target).unwrap();
            kind_centre(&r, b.kind, b.tile)
        };
        for (mangonel_at, timed_out, assault) in [
            ((rx, ry), false, false),
            ((rx, ry), true, true),
            ((sx, sy), false, true),
        ] {
            let mut a = Arena::frozen(
                standard_vs_passive(&r),
                vec![
                    spawn(&r, P0, "skirmisher", sx, sy, 8),
                    spawn(&r, P0, "mangonel", mangonel_at.0, mangonel_at.1, 1),
                    spawn(&r, P0, "supply_wain", sx, sy, 1),
                ],
            );
            let stage_max = a.ctx(P0).ticks(a.ctx(P0).knobs.stage_max_ds);
            a.run(stage_max);
            let skirm = a.units_of(P0, "skirmisher");
            let mangonel = a.units_of(P0, "mangonel")[0];
            let wain = a.units_of(P0, "supply_wain")[0];
            let mut units = skirm.clone();
            units.push(mangonel);
            units.sort();
            let tick = a.sim.tick();
            let mut w = wave(units, Some(wain), target, stage);
            w.launched_at = tick;
            w.phase_at = if timed_out { tick - stage_max } else { tick };
            let mut mem = BotMemory {
                wave: Some(w),
                ..BotMemory::default()
            };
            let mut ctx = a.ctx(P0);
            step(&mut ctx, &mut mem);
            let phase = mem.wave.as_ref().unwrap().phase;
            let w_point = wain_point(&ctx, target, stage);
            let cmds = ctx.into_commands();
            let at_town = |units: &[UnitId]| {
                cmds.contains(&Command::AttackMove {
                    units: units.to_vec(),
                    target: seat_centre,
                })
            };
            assert_eq!(phase == WavePhase::Assault, assault, "{cmds:?}");
            assert_eq!(at_town(&skirm), assault, "{cmds:?}");
            assert_eq!(at_town(&[mangonel]), assault, "{cmds:?}");
            assert_eq!(
                cmds.contains(&Command::Move {
                    units: vec![wain],
                    target: w_point,
                    queue: false
                }),
                assault,
                "{cmds:?}"
            );
            if !assault {
                let s = a.sim.view().map().center_of(stage);
                assert!(
                    cmds.iter().any(|c| matches!(c, Command::AttackMove { units, target } if units.contains(&mangonel) && *target == s)),
                    "the Mangonel is sent to the stage point: {cmds:?}"
                );
            }
        }
    }

    /// W keeps `wain_standoff_tiles` beyond the range of every enemy
    /// shooter (the target Town and a Watchtower), as near the target as
    /// that allows on the segment to the stage point; with both at 0 HP it
    /// moves up beside the Town.
    #[test]
    fn wain_point_is_out_of_every_shooters_range() {
        let r = crate::testkit::rules();
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            vec![place(&r, P1, "watchtower", 92, 66)],
        );
        let target = a.seat(P1);
        let tower = a.buildings_of(P1, "watchtower")[0];
        let mut ctx = a.ctx(P0);
        let stage = stage_point(&mut ctx, target, 10);
        let standoff = ctx.knobs.wain_standoff_tiles;
        let both = shooters(&ctx);
        assert_eq!(both.len(), 2, "the Seat and the tower");
        let w = wain_point(&ctx, target, stage);
        assert!(out_of_reach(&both, w, standoff));
        // The tile before W on the segment from the target is not.
        let view = a.sim.view();
        let b = view.building(target).unwrap();
        let line = segment(kind_centre_tile(&r, b.kind, b.tile), stage);
        let at = line
            .iter()
            .position(|t| view.map().center_of(*t) == w)
            .expect("W lies on the segment");
        assert!(!out_of_reach(
            &both,
            view.map().center_of(line[at - 1]),
            standoff
        ));
        // The tower alone pushes W back: without it W would be nearer.
        let seat_only: Vec<(Rect, u32)> = both
            .iter()
            .copied()
            .filter(|(rect, _)| *rect == footprint(&r, b))
            .collect();
        let nearer = line
            .iter()
            .position(|t| out_of_reach(&seat_only, view.map().center_of(*t), standoff))
            .unwrap();
        assert!(nearer < at, "{nearer} {at}");
        a.issue(
            P0,
            vec![
                Command::DebugDamage {
                    target: TargetId::Building(target),
                    amount: 5000,
                },
                Command::DebugDamage {
                    target: TargetId::Building(tower),
                    amount: 400,
                },
            ],
        );
        let ctx = a.ctx(P0);
        assert_eq!(shooters(&ctx).len(), 1, "the tower still shoots at 100 HP");
        a.issue(
            P0,
            vec![Command::DebugDamage {
                target: TargetId::Building(tower),
                amount: 400,
            }],
        );
        let ctx = a.ctx(P0);
        assert!(shooters(&ctx).is_empty());
        let w = wain_point(&ctx, target, stage);
        let view = a.sim.view();
        let rect = footprint(&r, view.building(target).unwrap());
        assert_eq!(chebyshev(rect, view.map().tile_of(w)), 1, "beside the Town");
    }

    /// At 0 HP the Town's ring tiles go to distinct annexing units, the
    /// nearest first, one `Move` each; units already on the ring count,
    /// and the annexation starts once they stand there.
    #[test]
    fn ring_hold_assigns_distinct_ring_tiles() {
        let r = crate::testkit::rules();
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            vec![spawn(&r, P0, "skirmisher", 96, 64, 6)],
        );
        let target = a.seat(P1);
        a.issue(
            P0,
            vec![Command::DebugDamage {
                target: TargetId::Building(target),
                amount: 5000,
            }],
        );
        let units = a.units_of(P0, "skirmisher");
        let mut w = wave(units.clone(), None, target, Tile::new(70, 64));
        w.phase = WavePhase::Assault;
        let mut mem = BotMemory {
            wave: Some(w),
            ..BotMemory::default()
        };
        a.set_mem(P0, &mem);
        let mut ctx = a.ctx(P0);
        step(&mut ctx, &mut mem);
        assert_eq!(mem.wave.as_ref().unwrap().phase, WavePhase::Hold);
        let cmds = ctx.into_commands();
        let view = a.sim.view();
        let rect = footprint(&r, view.building(target).unwrap());
        let moves: Vec<(UnitId, Tile)> = cmds
            .iter()
            .filter_map(|c| match c {
                Command::Move { units, target, .. } if units.len() == 1 => {
                    Some((units[0], view.map().tile_of(*target)))
                }
                _ => None,
            })
            .collect();
        assert_eq!(moves.len(), 4, "ring_units: {cmds:?}");
        let tiles: BTreeSet<Tile> = moves.iter().map(|(_, t)| *t).collect();
        let movers: BTreeSet<UnitId> = moves.iter().map(|(u, _)| *u).collect();
        assert_eq!((tiles.len(), movers.len()), (4, 4), "distinct");
        for t in &tiles {
            assert_eq!(chebyshev(rect, *t), 1);
            assert!(view.map().passable(*t));
        }
        // Run the plans: the ring holds and the annexation starts.
        for _ in 0..40 {
            a.act(P0);
            a.run(7);
        }
        let annex = a.sim.view().annexation(target).expect("annexing");
        assert_eq!(annex.attacker, P0);
        let (cmds, _) = a.plan(P0);
        assert!(
            cmds.iter()
                .all(|c| !matches!(c, Command::Move { units, .. } if units.len() == 1)),
            "the ring is held: {cmds:?}"
        );
    }

    /// Enemy military near a Town at 0 HP is attacked by the roster units
    /// not on the ring, up to `clear_defenders_units` each.
    #[test]
    fn clears_defenders_with_attack() {
        let r = crate::testkit::rules();
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            vec![spawn(&r, P0, "skirmisher", 95, 64, 10)],
        );
        let target = a.seat(P1);
        a.issue(
            P1,
            vec![
                Command::DebugDamage {
                    target: TargetId::Building(target),
                    amount: 5000,
                },
                spawn(&r, P1, "shieldbearer", 104, 70, 1),
                spawn(&r, P1, "shieldbearer", 104, 58, 1),
            ],
        );
        let defenders = a.units_of(P1, "shieldbearer");
        assert_eq!(defenders.len(), 2);
        let units = a.units_of(P0, "skirmisher");
        let mut w = wave(units, None, target, Tile::new(70, 64));
        w.phase = WavePhase::Hold;
        let mut mem = BotMemory {
            wave: Some(w),
            ..BotMemory::default()
        };
        let mut ctx = a.ctx(P0);
        step(&mut ctx, &mut mem);
        let cmds = ctx.into_commands();
        let ring: BTreeSet<UnitId> = cmds
            .iter()
            .filter_map(|c| match c {
                Command::Move { units, .. } if units.len() == 1 => Some(units[0]),
                _ => None,
            })
            .collect();
        assert_eq!(ring.len(), 4);
        let attacks: Vec<(TargetId, Vec<UnitId>)> = cmds
            .iter()
            .filter_map(|c| match c {
                Command::Attack { units, target } => Some((*target, units.clone())),
                _ => None,
            })
            .collect();
        let sizes: Vec<(TargetId, usize)> = attacks.iter().map(|(t, u)| (*t, u.len())).collect();
        assert_eq!(
            sizes,
            [
                (TargetId::Unit(defenders[0]), 4),
                (TargetId::Unit(defenders[1]), 2)
            ],
            "{cmds:?}"
        );
        for (_, units) in &attacks {
            assert!(units.iter().all(|u| !ring.contains(u)), "ring units hold");
        }
    }

    /// A wave ends (its units come home) with no unit that can annex, with
    /// fewer than `recall_below_units` armed units, or after
    /// `wave_max_ds`. The siege-only wave has `recall_below_units`
    /// Mangonels, so only the annexer rule can end it.
    #[test]
    fn wave_ends_without_annexers() {
        let r = crate::testkit::rules();
        let below = r.ai.personality("standard").unwrap().recall_below_units;
        assert!(3 >= below, "the siege-only case needs {below} Mangonels");
        let (x, y) = (80, 64);
        for (why, kind, n, age, ends) in [
            ("siege only", "mangonel", 3, 0, true),
            ("two armed", "skirmisher", 2, 0, true),
            ("three armed", "skirmisher", 3, 0, false),
            ("too old", "skirmisher", 3, 7200, true),
        ] {
            let mut a = Arena::frozen(
                standard_vs_passive(&r),
                vec![
                    Command::DebugAdvanceAge { owner: P0 },
                    spawn(&r, P0, kind, x, y, n),
                ],
            );
            a.run(age);
            let units = a.units_of(P0, kind);
            let tick = a.sim.tick();
            let mut w = wave(units.clone(), None, a.seat(P1), Tile::new(72, 64));
            w.launched_at = tick - age;
            w.phase_at = tick;
            let mut mem = BotMemory {
                wave: Some(w),
                ..BotMemory::default()
            };
            let mut ctx = a.ctx(P0);
            let rally = ctx.rally();
            step(&mut ctx, &mut mem);
            assert_eq!(mem.wave.is_none(), ends, "{why}");
            let home = ctx.into_commands().contains(&Command::AttackMove {
                units,
                target: rally,
            });
            assert_eq!(home, ends, "{why}");
        }
    }

    /// Once the target is the bot's, a wave of at least
    /// `continue_min_units` armed units retargets to the enemy Town nearest
    /// the rally with a fresh stage point; a smaller one comes home.
    #[test]
    fn retargets_after_capture() {
        let r = crate::testkit::rules();
        for (n, goes_on) in [(4, true), (3, false)] {
            let a = Arena::frozen(
                standard_vs_passive(&r),
                vec![
                    place(&r, P1, "town", 106, 81),
                    spawn(&r, P0, "skirmisher", 60, 64, n),
                ],
            );
            let units = a.units_of(P0, "skirmisher");
            // The wave's target is the bot's own Town now.
            let mut w = wave(units.clone(), None, a.seat(P0), Tile::new(40, 64));
            w.phase = WavePhase::Hold;
            let mut mem = BotMemory {
                wave: Some(w),
                ..BotMemory::default()
            };
            let mut ctx = a.ctx(P0);
            let rally = ctx.rally();
            let next = a.seat(P1);
            let stage = stage_point(&mut ctx, next, n as usize + 1);
            step(&mut ctx, &mut mem);
            let cmds = ctx.into_commands();
            if goes_on {
                let w = mem.wave.expect("goes on");
                assert_eq!(
                    (w.target, w.stage, w.phase),
                    (next, stage, WavePhase::Stage)
                );
                assert_eq!((w.launched_at, w.phase_at), (a.sim.tick(), a.sim.tick()));
                let s = a.sim.view().map().center_of(stage);
                assert!(
                    cmds.contains(&Command::AttackMove { units, target: s }),
                    "{cmds:?}"
                );
            } else {
                assert!(mem.wave.is_none());
                assert!(cmds.contains(&Command::AttackMove {
                    units,
                    target: rally
                }));
            }
        }
    }

    /// From `all_in_after_ds` every home armed unit joins the active wave
    /// and is sent with it, the wave goes on, and a threat at home recalls
    /// nothing.
    #[test]
    fn all_in_joins_every_home_unit_and_is_never_recalled() {
        let r = crate::testkit::rules();
        let (x, y) = RALLY;
        let a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                Command::DebugAdvanceAge { owner: P0 },
                spawn(&r, P0, "mangonel", 80, 64, 1),
                spawn(&r, P0, "skirmisher", x, y, 3),
            ],
        );
        let k = knobs(&r, |k| k.all_in_after_ds = 0);
        let siege = a.units_of(P0, "mangonel");
        let home = a.units_of(P0, "skirmisher");
        let target = a.seat(P1);
        let mut w = wave(siege.clone(), None, target, Tile::new(72, 64));
        w.phase = WavePhase::Stage;
        w.phase_at = a.sim.tick();
        let mut mem = BotMemory {
            wave: Some(w),
            ..BotMemory::default()
        };
        let mut ctx = a.ctx(P0);
        ctx.knobs = &k;
        step(&mut ctx, &mut mem);
        let w = mem.wave.clone().expect("the wave goes on");
        let mut all = siege.clone();
        all.extend(home.iter().copied());
        all.sort();
        assert_eq!(w.units, all, "the home units joined");
        let s = a.sim.view().map().center_of(w.stage);
        let cmds = ctx.into_commands();
        assert!(
            cmds.iter().any(|c| matches!(c, Command::AttackMove { units, target } if units.contains(&home[0]) && *target == s)),
            "{cmds:?}"
        );
        // A raider at home: no recall during the all-in.
        let raided = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                spawn(&r, P0, "skirmisher", 80, 64, 3),
                spawn(&r, P1, "skirmisher", 24, 51, 1),
            ],
        );
        let units = raided.units_of(P0, "skirmisher");
        let mut mem = BotMemory {
            wave: Some(wave(units, None, target, Tile::new(72, 64))),
            ..BotMemory::default()
        };
        let mut ctx = raided.ctx(P0);
        ctx.knobs = &k;
        defend::step(&mut ctx, &mut mem);
        assert!(mem.defend.is_some(), "the raid is seen");
        assert!(
            mem.wave.is_some(),
            "a staging wave stays out during the all-in"
        );
    }

    /// During the all-in a wave ends only when none of its armed units is
    /// left: two Mangonels alone, with no home unit to join them (no
    /// annexer and fewer than `recall_below_units`, which ends the same
    /// wave before the all-in), stay out.
    #[test]
    fn all_in_keeps_a_wave_of_siege_alone_out() {
        let r = crate::testkit::rules();
        let a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                Command::DebugAdvanceAge { owner: P0 },
                spawn(&r, P0, "mangonel", 80, 64, 2),
            ],
        );
        let siege = a.units_of(P0, "mangonel");
        let target = a.seat(P1);
        let standard = r.ai.personality("standard").unwrap().all_in_after_ds;
        for (all_in_after_ds, stays) in [(standard, false), (0, true)] {
            let k = knobs(&r, |k| k.all_in_after_ds = all_in_after_ds);
            let mut w = wave(siege.clone(), None, target, Tile::new(72, 64));
            w.phase_at = a.sim.tick();
            let mut mem = BotMemory {
                wave: Some(w),
                ..BotMemory::default()
            };
            let mut ctx = a.ctx(P0);
            ctx.knobs = &k;
            let rally = ctx.rally();
            step(&mut ctx, &mut mem);
            assert_eq!(mem.wave.is_some(), stays, "all-in from {all_in_after_ds}");
            let cmds = ctx.into_commands();
            let home = cmds
                .iter()
                .any(|c| matches!(c, Command::AttackMove { target, .. } if *target == rally));
            assert_eq!(home, !stays, "all-in from {all_in_after_ds}: {cmds:?}");
            if stays {
                assert_eq!(mem.wave.unwrap().units, siege, "no unit joined");
            }
        }
    }

    /// The all-in launches with one home armed unit anywhere: no size,
    /// siege or Wain, not assembled at the rally, and no wait for
    /// `next_wave_at` (the age gate still holds); before the all-in the
    /// same army launches nothing.
    #[test]
    fn all_in_launches_one_unit_without_siege_wain_or_timer() {
        let r = crate::testkit::rules();
        let (x, y) = RALLY;
        let a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                Command::DebugAdvanceAge { owner: P0 },
                spawn(&r, P0, "skirmisher", x - 14, y, 1),
            ],
        );
        let skirm = a.units_of(P0, "skirmisher");
        let standard = r.ai.personality("standard").unwrap().all_in_after_ds;
        for (all_in_after_ds, launches) in [(standard, false), (0, true)] {
            let k = knobs(&r, |k| k.all_in_after_ds = all_in_after_ds);
            let mut mem = BotMemory {
                next_wave_at: a.sim.tick() + 1000,
                ..BotMemory::default()
            };
            let mut ctx = a.ctx(P0);
            ctx.knobs = &k;
            step(&mut ctx, &mut mem);
            let cmds = ctx.into_commands();
            assert_eq!(
                mem.wave.is_some(),
                launches,
                "all-in from {all_in_after_ds}: {cmds:?}"
            );
            if launches {
                let w = mem.wave.unwrap();
                assert_eq!(
                    (&w.units, w.wain, w.phase),
                    (&skirm, None, WavePhase::Stage)
                );
                let s = a.sim.view().map().center_of(w.stage);
                assert_eq!(
                    cmds,
                    [Command::AttackMove {
                        units: skirm.clone(),
                        target: s
                    }]
                );
            } else {
                assert!(cmds.is_empty(), "{cmds:?}");
            }
        }
    }

    #[test]
    fn segments_and_rects() {
        let s = segment(Tile::new(10, 10), Tile::new(14, 12));
        assert_eq!(
            s,
            [
                Tile::new(10, 10),
                Tile::new(11, 11),
                Tile::new(12, 11),
                Tile::new(13, 12),
                Tile::new(14, 12)
            ]
        );
        assert_eq!(segment(Tile::new(3, 3), Tile::new(3, 3)), [Tile::new(3, 3)]);
        let back = segment(Tile::new(14, 12), Tile::new(10, 10));
        assert_eq!(back.first(), Some(&Tile::new(14, 12)));
        assert_eq!(back.last(), Some(&Tile::new(10, 10)));
        let r = (10, 10, 12, 12);
        assert_eq!(chebyshev(r, Tile::new(11, 11)), 0);
        assert_eq!(chebyshev(r, Tile::new(9, 13)), 1);
        assert_eq!(chebyshev(r, Tile::new(15, 11)), 3);
        assert_eq!(dist_sq_to_rect(FxVec2::from_ints(11, 11), r), 0);
        assert_eq!(dist_sq_to_rect(FxVec2::from_ints(16, 11), r), tiles_sq(3));
        assert_eq!((spread(1), spread(9), spread(24)), (1, 2, 3));
    }
}
