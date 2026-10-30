//! The build order: when the view shows a step as done (never when its
//! command is issued), the window of steps the bot works on, and what it
//! does for each (docs/design/ai.md rule 1).
//!
//! **The window** is the first `lookahead` steps that are not done and not
//! cooling down, in order; it stops at an unfinished `Wait`. Each window
//! step, in order, either issues its command (paid from the plan's
//! [`super::Ledger`]), saves up (its cost is reserved, so a later step or
//! production cannot spend it), waits (every trainer or host of the step
//! is busy or stands in a contested Town, a predecessor tech is still
//! queued, no worker is free, or the plan's `check_build` probes ran
//! out), or is blocked (any other rejection, no trainer or host built
//! yet, no site, which includes a `NearTown` building with no uncontested
//! own Town to go beside, or, for a `BorderEdge` building, enemy military
//! within [`site_danger_tiles`] of the site found, so a lost border
//! Watchtower is not placed again in front of the army that took it: a
//! border edge site faces the enemy outside the defence's radius, while
//! the other policies place nearer the bot's Towns): a blocked step is
//! cooled down for `blocked_retry_ds` in `BotMemory::blocked` and then
//! retried; it never becomes a permanent skip.
//!
//! **An `Advance`** is never cooled down: while the techs it needs are
//! still missing it waits and reserves nothing (the army and the steps
//! behind it spend the stock meanwhile), and once they are done it saves
//! up its cost until the stock fills, at the pace of its scarcest
//! resource ([`paced`]: each resource is held at the share of its cost
//! the scarcest one has reached, so the surplus of the others still pays
//! for the army), except while the bot defends (`BotMemory::defend`):
//! then it reserves nothing, so the stock goes to the army first. Plan
//! 4.1 has a step that cannot afford its cost reserve all of it; for an
//! `Advance` that held the Charter's 500 Grain and 300 Lumber for minutes
//! while its 300 Lore came in, and no army was trained meanwhile.
//!
//! **The `Military` latch** is set once every non-optional step before
//! the `Military` step is done, and never cleared.
//!
//! **The site keeper** gives one of the bot's construction sites with no
//! builder per plan a builder, with a `Build` on the site's own tile (a
//! free resume); a border building's site with enemy military in reach
//! gets none.
//!
//! **The remembered site** (`BotMemory::site`) is the last site chosen for
//! a `Build` step: tried first on the step's next plan, forgotten when a
//! check rejects it or the step leaves the window.

use super::ctx::Ctx;
use super::economy;
use super::facts::{Facts, kind_centre, tiles_sq, visible_enemies};
use super::memory::BotMemory;
use super::sites::{SiteSearch, near_town_candidates, search, site_for};
use rules::{BuildOrder, Placement, Rules, Step, UnitClass};
use sim::{BuildingId, BuildingKindId, Command, FxVec2, RejectReason, TechId, UnitId, UnitKindId};
use std::collections::BTreeSet;

/// Whether the view (through `facts`) shows `step` as done:
///
/// | Step | Done when |
/// | --- | --- |
/// | `Workers` | workers alive plus queued reach the count |
/// | `Build` | own buildings of the kind, sites included, reach the count |
/// | `Train` | units of the kind alive plus queued reach the count |
/// | `Research` | the tech is researched or queued |
/// | `Advance` | the age is reached |
/// | `Military` | always |
/// | `Wait` | the tick reaches the time |
///
/// An id the rules do not know (which `Rules::load` rules out) is never
/// done.
pub fn satisfied(step: &Step, facts: &Facts, rules: &Rules) -> bool {
    match step {
        Step::Workers { count, .. } => facts
            .worker_kind
            .is_some_and(|w| facts.alive_and_queued(w) >= *count),
        Step::Build { kind, count, .. } => rules
            .building_kind_by_id(kind)
            .is_some_and(|(i, _)| facts.owned(BuildingKindId(i)) >= *count),
        Step::Train { kind, count, .. } => {
            unit_kind(rules, kind).is_some_and(|k| facts.alive_and_queued(k) >= *count)
        }
        Step::Research { tech, .. } => rules
            .tech_by_id(tech)
            .is_some_and(|(i, _)| facts.researched_or_queued.contains(&TechId(i))),
        Step::Advance { age } => rules.age_index(age).is_some_and(|a| facts.age >= a),
        Step::Military => true,
        Step::Wait { until_ds } => facts.tick >= rules.ticks_from_ds(*until_ds),
    }
}

/// The index of the first step not done, skipping optional steps when
/// `required_only`; `None` when every (required) step is done.
pub fn first_unsatisfied(
    order: &BuildOrder,
    facts: &Facts,
    rules: &Rules,
    required_only: bool,
) -> Option<usize> {
    order
        .steps
        .iter()
        .position(|s| !(required_only && s.optional()) && !satisfied(s, facts, rules))
}

/// A unit kind's id by its string id.
fn unit_kind(rules: &Rules, id: &str) -> Option<UnitKindId> {
    let i = rules.units.iter().position(|u| u.id == id)?;
    Some(UnitKindId(u16::try_from(i).ok()?))
}

/// Whether step `i` is cooling down at `tick`.
pub fn cooled(mem: &BotMemory, i: usize, tick: u32) -> bool {
    mem.blocked
        .iter()
        .any(|(s, at)| usize::from(*s) == i && tick < *at)
}

/// The plan's window: the first `lookahead` steps that are not done and
/// not cooling down, in order, stopping at an unfinished `Wait`.
pub fn window(ctx: &Ctx<'_>, mem: &BotMemory) -> Vec<usize> {
    let lookahead = usize::try_from(ctx.knobs.lookahead).unwrap_or(1);
    window_of(ctx.order, &ctx.facts, ctx.view.rules(), mem, lookahead)
}

/// [`window`] from its inputs (`facts.tick` is the plan's tick).
pub fn window_of(
    order: &BuildOrder,
    facts: &Facts,
    rules: &Rules,
    mem: &BotMemory,
    lookahead: usize,
) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, step) in order.steps.iter().enumerate() {
        if out.len() >= lookahead {
            break;
        }
        if satisfied(step, facts, rules) || cooled(mem, i, facts.tick) {
            continue;
        }
        if matches!(step, Step::Wait { .. }) {
            break;
        }
        out.push(i);
    }
    out
}

/// Whether every non-optional step before the build order's first
/// `Military` step is done (`false` when it has none).
pub fn military_crossed(order: &BuildOrder, facts: &Facts, rules: &Rules) -> bool {
    let Some(m) = order.steps.iter().position(|s| matches!(s, Step::Military)) else {
        return false;
    };
    order.steps[..m]
        .iter()
        .all(|s| s.optional() || satisfied(s, facts, rules))
}

/// A short label for a step (`ai::describe`).
pub fn step_label(step: &Step) -> String {
    match step {
        Step::Workers { count, .. } => format!("workers {count}"),
        Step::Build { kind, count, .. } => format!("build {kind} {count}"),
        Step::Train { kind, count, .. } => format!("train {kind} {count}"),
        Step::Research { tech, .. } => format!("research {tech}"),
        Step::Advance { age } => format!("advance {age}"),
        Step::Military => "military".to_string(),
        Step::Wait { until_ds } => format!("wait {until_ds}"),
    }
}

/// What a window step did this plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
    /// Its command was issued.
    Issued,
    /// It saves up for this cost.
    Saving(Vec<i64>),
    /// It waits for something already under way.
    Waiting,
    /// It is blocked and cools down.
    Blocked,
}

/// The workers a `Build` may take, nearest first by the caller: idle,
/// unclaimed workers; when there are none, the gatherers of the resource
/// most over its share (ties to the lower resource index). Never a unit
/// that builds.
pub fn builder_pool(ctx: &Ctx<'_>, mem: &BotMemory) -> Vec<UnitId> {
    let idle: Vec<UnitId> = ctx
        .facts
        .idle_workers
        .iter()
        .copied()
        .filter(|u| !ctx.is_claimed(*u))
        .collect();
    if !idle.is_empty() {
        return idle;
    }
    let s = economy::shares(ctx, mem);
    let Some(r) = (0..s.current.len())
        .filter(|r| s.current[*r] > 0)
        .max_by_key(|r| {
            (
                i64::from(s.current[*r]) - i64::from(s.desired[*r]),
                core::cmp::Reverse(*r),
            )
        })
    else {
        return Vec::new();
    };
    ctx.facts.gatherers[r]
        .iter()
        .copied()
        .filter(|u| s.pool.contains(u))
        .collect()
}

/// Train one unit of `kind` at the free trainer with the shortest queue
/// (the caller's order breaks ties).
fn train(ctx: &mut Ctx<'_>, kind: UnitKindId, mut trainers: Vec<BuildingId>) -> StepOutcome {
    trainers.sort_by_key(|b| ctx.view.building(*b).map_or(usize::MAX, |b| b.queue.len()));
    let Some(b) = trainers.first().copied() else {
        return StepOutcome::Waiting;
    };
    match ctx.view.check_train(ctx.me, b, kind) {
        Ok(cost) if ctx.ledger.try_spend(&cost) => {
            ctx.push(Command::Train { building: b, kind });
            StepOutcome::Issued
        }
        Ok(cost) => StepOutcome::Saving(cost),
        Err(RejectReason::CannotAfford { .. }) => {
            StepOutcome::Saving(ctx.view.unit_cost(ctx.me, kind))
        }
        Err(RejectReason::QueueFull) => StepOutcome::Waiting,
        Err(_) => StepOutcome::Blocked,
    }
}

/// Free trainers of `kind` with the shortest queue first (Seat first for
/// the worker kind, then by id).
fn trainers_for(ctx: &Ctx<'_>, kind: UnitKindId) -> (bool, Vec<BuildingId>) {
    let rules = ctx.view.rules();
    let Some(trainer) = rules
        .unit_kind(kind.0)
        .and_then(|d| rules.building_kind_by_id(&d.trainer))
    else {
        return (false, Vec::new());
    };
    let kind_id = BuildingKindId(trainer.0);
    let all: Vec<BuildingId> = if kind_id.0 == rules.town_kind() {
        ctx.facts.towns.clone()
    } else {
        ctx.facts
            .complete
            .get(&kind_id)
            .cloned()
            .unwrap_or_default()
    };
    let max = usize::try_from(ctx.knobs.max_queue_per_building).unwrap_or(1);
    let free = all
        .iter()
        .copied()
        .filter(|b| !ctx.facts.contested.contains(b))
        .filter(|b| ctx.view.building(*b).is_some_and(|b| b.queue.len() < max))
        .collect();
    (!all.is_empty(), free)
}

/// Do step `i` of the window.
fn execute(ctx: &mut Ctx<'_>, mem: &mut BotMemory, i: usize) -> StepOutcome {
    let rules = ctx.view.rules();
    let step = ctx.order.steps[i].clone();
    match step {
        Step::Workers { .. } => {
            let Some(w) = ctx.facts.worker_kind else {
                return StepOutcome::Blocked;
            };
            match trainers_for(ctx, w) {
                (false, _) => StepOutcome::Blocked,
                (true, free) => train(ctx, w, free),
            }
        }
        Step::Train { kind, .. } => {
            let Some(k) = unit_kind(rules, &kind) else {
                return StepOutcome::Blocked;
            };
            match trainers_for(ctx, k) {
                (false, _) => StepOutcome::Blocked,
                (true, free) => train(ctx, k, free),
            }
        }
        Step::Research { tech, .. } => {
            let Some((t, def)) = rules.tech_by_id(&tech) else {
                return StepOutcome::Blocked;
            };
            let tech = TechId(t);
            let Some((host, _)) = rules.building_kind_by_id(&def.host) else {
                return StepOutcome::Blocked;
            };
            let all = ctx
                .facts
                .complete
                .get(&BuildingKindId(host))
                .cloned()
                .unwrap_or_default();
            if all.is_empty() {
                return StepOutcome::Blocked;
            }
            let max = usize::try_from(ctx.knobs.max_queue_per_building).unwrap_or(1);
            let mut free: Vec<BuildingId> = all
                .into_iter()
                .filter(|b| !ctx.facts.contested.contains(b))
                .filter(|b| ctx.view.building(*b).is_some_and(|b| b.queue.len() < max))
                .collect();
            free.sort_by_key(|b| ctx.view.building(*b).map_or(usize::MAX, |b| b.queue.len()));
            let Some(b) = free.first().copied() else {
                return StepOutcome::Waiting;
            };
            match ctx.view.check_research(ctx.me, b, tech) {
                Ok((cost, _)) if ctx.ledger.try_spend(&cost) => {
                    ctx.push(Command::Research { building: b, tech });
                    StepOutcome::Issued
                }
                Ok((cost, _)) => StepOutcome::Saving(cost),
                Err(RejectReason::CannotAfford { .. }) => {
                    StepOutcome::Saving(ctx.view.research_quote(ctx.me, tech).0)
                }
                Err(RejectReason::QueueFull) => StepOutcome::Waiting,
                Err(RejectReason::PrerequisiteMissing)
                    if rules
                        .tech_predecessor(t)
                        .is_some_and(|p| ctx.facts.researched_or_queued.contains(&TechId(p))) =>
                {
                    StepOutcome::Waiting
                }
                Err(_) => StepOutcome::Blocked,
            }
        }
        Step::Advance { .. } => {
            let next = rules.ages.get(usize::from(ctx.facts.age) + 1);
            let cost = next.map(|a| rules.amounts(&a.cost)).unwrap_or_default();
            match ctx.view.check_advance_age(ctx.me) {
                Ok(cost) if ctx.ledger.try_spend(&cost) => {
                    ctx.push(Command::AdvanceAge { player: ctx.me });
                    StepOutcome::Issued
                }
                Ok(cost) => StepOutcome::Saving(cost),
                Err(RejectReason::CannotAfford { .. }) => StepOutcome::Saving(cost),
                Err(_) => StepOutcome::Waiting,
            }
        }
        Step::Build { kind, at, .. } => {
            let Some((k, _)) = rules.building_kind_by_id(&kind) else {
                return StepOutcome::Blocked;
            };
            let k = BuildingKindId(k);
            let cost = match ctx.view.check_build_kind(ctx.me, k) {
                Ok(cost) => cost,
                Err(RejectReason::CannotAfford { .. }) => {
                    return StepOutcome::Saving(ctx.view.building_cost(ctx.me, k));
                }
                Err(_) => return StepOutcome::Blocked,
            };
            if !ctx.ledger.affords(&cost) {
                return StepOutcome::Saving(cost);
            }
            let pool = builder_pool(ctx, mem);
            let step_index = u16::try_from(i).expect("at most 64 steps");
            match site_for(ctx, mem, step_index, k, at, &pool) {
                // A border building is not placed in front of an enemy
                // army: the step cools down.
                SiteSearch::Found { tile, .. }
                    if at == Placement::BorderEdge
                        && enemy_military_near(ctx, kind_centre(rules, k, tile)) =>
                {
                    StepOutcome::Blocked
                }
                SiteSearch::Found { builder, tile } => {
                    if ctx.ledger.try_spend(&cost) {
                        ctx.claim(builder);
                        if ctx.push(Command::Build {
                            unit: builder,
                            kind: k,
                            tile,
                        }) && let Some(d) = rules.building_kind(k.0)
                        {
                            let (x, y) = (i32::from(tile.x), i32::from(tile.y));
                            let w = i32::try_from(d.width_tiles).expect("validated <= 8");
                            let h = i32::try_from(d.height_tiles).expect("validated <= 8");
                            ctx.planned.push((x, y, x + w - 1, y + h - 1));
                        }
                        StepOutcome::Issued
                    } else {
                        StepOutcome::Saving(cost)
                    }
                }
                SiteSearch::NoBuilder | SiteSearch::OutOfProbes => StepOutcome::Waiting,
                SiteSearch::NoSite => StepOutcome::Blocked,
            }
        }
        Step::Military | Step::Wait { .. } => StepOutcome::Waiting,
    }
}

/// Run the rule: latch `Military`, work the window, keep one abandoned
/// site going.
pub fn step(ctx: &mut Ctx<'_>, mem: &mut BotMemory) {
    let tick = ctx.tick();
    mem.blocked.retain(|(_, at)| tick < *at);
    if !mem.military && military_crossed(ctx.order, &ctx.facts, ctx.view.rules()) {
        mem.military = true;
    }
    let retry = ctx.ticks(ctx.knobs.blocked_retry_ds);
    let steps = window(ctx, mem);
    // A remembered site is only kept for a step still being worked on.
    if mem
        .site
        .is_some_and(|(s, _)| !steps.contains(&usize::from(s)))
    {
        mem.site = None;
    }
    for i in steps {
        if ctx.commands_left == 0 {
            break;
        }
        match execute(ctx, mem, i) {
            StepOutcome::Issued | StepOutcome::Waiting => {}
            // A defence outranks saving for an age: the `Advance` keeps
            // its place but leaves the stock to production.
            StepOutcome::Saving(_)
                if mem.defend.is_some() && matches!(ctx.order.steps[i], Step::Advance { .. }) => {}
            // An age saves at the pace of its scarcest resource.
            StepOutcome::Saving(cost) if matches!(ctx.order.steps[i], Step::Advance { .. }) => {
                let held = paced(&cost, &ctx.ledger);
                ctx.ledger.reserve(&held);
            }
            StepOutcome::Saving(cost) => ctx.ledger.reserve(&cost),
            StepOutcome::Blocked => {
                if !matches!(ctx.order.steps[i], Step::Advance { .. }) {
                    let s = u16::try_from(i).expect("at most 64 steps");
                    mem.blocked.push((s, tick.saturating_add(retry)));
                    if mem.site.is_some_and(|(step, _)| step == s) {
                        mem.site = None;
                    }
                }
            }
        }
    }
    reactive_tower(ctx, mem);
    site_keeper(ctx, mem);
}

/// The step index the reactive Watchtower's site search remembers its site
/// under (never a build-order step, so the next plan forgets it).
pub const REACTIVE_STEP: u16 = u16::MAX;

/// The reactive Watchtower (M6, `reactive_towers`): after a released
/// defence ([`super::defend`] sets `BotMemory::last_defended`), the bot
/// places one Watchtower `NearTown` around the defended Town with what
/// the plan's ledger can pay (it never saves up for it), up to
/// `reactive_towers` in the match. A Town lost or contested meanwhile, or
/// no site, drops the request.
pub fn reactive_tower(ctx: &mut Ctx<'_>, mem: &mut BotMemory) {
    let Some(town) = mem.last_defended else {
        return;
    };
    if u32::from(mem.extra_towers) >= ctx.knobs.reactive_towers
        || !ctx.facts.towns.contains(&town)
        || ctx.facts.contested.contains(&town)
    {
        mem.last_defended = None;
        return;
    }
    if ctx.commands_left == 0 || ctx.probes_left == 0 {
        return;
    }
    let rules = ctx.view.rules();
    let Some(k) = rules
        .buildings
        .iter()
        .position(|b| !b.town && b.border_radius_tiles.is_some() && b.attack.is_some())
    else {
        mem.last_defended = None;
        return;
    };
    let k = BuildingKindId(u16::try_from(k).expect("validated len"));
    let Ok(cost) = ctx.view.check_build_kind(ctx.me, k) else {
        return;
    };
    if !ctx.ledger.affords(&cost) {
        return;
    }
    let pool = builder_pool(ctx, mem);
    let candidates = near_town_candidates(ctx, k, town);
    match search(
        ctx,
        mem,
        REACTIVE_STEP,
        k,
        Placement::NearTown,
        &pool,
        candidates,
    ) {
        SiteSearch::Found { builder, tile } => {
            if ctx.ledger.try_spend(&cost) {
                ctx.claim(builder);
                if ctx.push(Command::Build {
                    unit: builder,
                    kind: k,
                    tile,
                }) {
                    ctx.planned.push((
                        i32::from(tile.x),
                        i32::from(tile.y),
                        i32::from(tile.x),
                        i32::from(tile.y),
                    ));
                    mem.extra_towers = mem.extra_towers.saturating_add(1);
                    mem.last_defended = None;
                }
            }
        }
        SiteSearch::NoSite => mem.last_defended = None,
        SiteSearch::NoBuilder | SiteSearch::OutOfProbes => {}
    }
}

/// What an `Advance` saving up for `cost` holds back: each resource at the
/// share of its cost that the scarcest resource has reached in `ledger`,
/// `cost_r × min_r(available_r / cost_r)`, at most `cost_r` (integer
/// cross-multiplication, rounded down). The scarce resource is held in
/// full and the others only in step with it, so their surplus still pays
/// for the army; every share only grows as stock comes in, and the whole
/// cost is held once every resource has reached it.
pub fn paced(cost: &[i64], ledger: &super::Ledger) -> Vec<i64> {
    // The scarcest share so far, as the fraction n / d (start: all of it).
    let (mut n, mut d) = (1i64, 1i64);
    for (r, c) in cost.iter().enumerate() {
        if *c > 0 {
            let a = ledger.available(r).min(*c);
            if a.saturating_mul(d) < n.saturating_mul(*c) {
                n = a;
                d = *c;
            }
        }
    }
    cost.iter()
        .map(|c| (c.saturating_mul(n) / d).min(*c))
        .collect()
}

/// How near enemy military may come to a site before a `Build` backs off
/// and the site keeper sends no builder to it: the farthest any unit kind
/// acquires or shoots (`acquire_range_tiles`, the longest weapon range
/// rounded up to whole tiles), plus `stage_margin_tiles`.
pub fn site_danger_tiles(ctx: &Ctx<'_>) -> u32 {
    let rules = ctx.view.rules();
    let weapon = rules
        .units
        .iter()
        .filter_map(|u| u.attack.as_ref())
        .map(|a| a.range_tiles_x100.div_ceil(100))
        .max()
        .unwrap_or(0);
    rules
        .acquire_range_tiles
        .max(weapon)
        .saturating_add(ctx.knobs.stage_margin_tiles)
}

/// Whether another player's armed unit (non-civilian, with an attack)
/// stands within [`site_danger_tiles`] of `centre`. It reads the enemy
/// through [`visible_enemies`], the fog seam.
pub fn enemy_military_near(ctx: &Ctx<'_>, centre: FxVec2) -> bool {
    let rules = ctx.view.rules();
    let r2 = tiles_sq(site_danger_tiles(ctx));
    visible_enemies(&ctx.view, ctx.me).any(|u| {
        u.pos.dist_sq_i64(centre) <= r2
            && rules
                .unit_kind(u.kind.0)
                .is_some_and(|k| k.class != UnitClass::Civilian && k.attack.is_some())
    })
}

/// One own site with no builder gets the nearest worker of the builder
/// pool, with a `Build` on its own tile (a free resume); the site of a
/// border building (a kind that projects a border, the Watchtower) with
/// enemy military in reach ([`enemy_military_near`]) gets none.
pub fn site_keeper(ctx: &mut Ctx<'_>, mem: &BotMemory) {
    if ctx.commands_left == 0 || ctx.probes_left == 0 {
        return;
    }
    let rules = ctx.view.rules();
    let held: BTreeSet<BuildingId> = ctx.facts.builders.keys().copied().collect();
    let border = |kind: BuildingKindId| {
        rules
            .building_kind(kind.0)
            .is_some_and(|d| d.border_radius_tiles.is_some())
    };
    let Some((kind, tile)) = ctx
        .facts
        .sites
        .iter()
        .filter(|s| !held.contains(s))
        .filter_map(|s| ctx.view.building(*s))
        .map(|b| (b.kind, b.tile))
        .find(|(kind, tile)| {
            !(border(*kind) && enemy_military_near(ctx, kind_centre(rules, *kind, *tile)))
        })
    else {
        return;
    };
    let centre = kind_centre(rules, kind, tile);
    let pool = builder_pool(ctx, mem);
    let Some(builder) = pool
        .iter()
        .filter_map(|id| ctx.view.unit(*id))
        .min_by_key(|u| (u.pos.dist_sq_i64(centre), u.id))
        .map(|u| u.id)
    else {
        return;
    };
    if ctx.check_build(builder, kind, tile).is_ok() {
        ctx.claim(builder);
        ctx.push(Command::Build {
            unit: builder,
            kind,
            tile,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{Arena, P0, P1, grant_all, place, standard_vs_passive, unit_kind};
    use sim::{Command, SimEvent, TargetId};

    fn order(steps: Vec<Step>) -> BuildOrder {
        BuildOrder {
            id: "test".into(),
            steps,
        }
    }

    fn build(kind: &str, count: u32, at: Placement) -> Step {
        Step::Build {
            kind: kind.into(),
            count,
            at,
            optional: false,
        }
    }

    /// The window is the first three steps not done; a site counts as a
    /// building and a queued unit as a unit.
    #[test]
    fn window_is_first_unsatisfied_steps_counting_sites_and_queued_entries() {
        let r = crate::testkit::rules();
        let mut a = Arena::frozen(standard_vs_passive(&r), Vec::new());
        let mem = BotMemory::default();
        assert_eq!(window(&a.ctx(P0), &mem), [0, 1, 2]);
        let croft = crate::testkit::building_kind(&r, "croft");
        let site = |x, y| Command::DebugPlaceBuilding {
            owner: P0,
            kind: croft,
            tile: sim::Tile::new(x, y),
            complete: false,
        };
        a.issue(P0, vec![site(18, 58), site(18, 70)]);
        assert_eq!(a.buildings_of(P0, "croft").len(), 2);
        assert_eq!(window(&a.ctx(P0), &mem), [0, 2, 3], "two croft sites");
        let seat = a.seat(P0);
        let yeoman = unit_kind(&r, "yeoman");
        let train = || Command::Train {
            building: seat,
            kind: yeoman,
        };
        a.issue(P0, vec![train(), train()]);
        assert_eq!(a.sim.view().building(seat).unwrap().queue.len(), 2);
        assert_eq!(
            window(&a.ctx(P0), &mem),
            [2, 3, 4],
            "five Yeomen and two queued make seven"
        );
    }

    /// A step the stock cannot pay for reserves its cost, so a later step
    /// cannot spend it; a step blocked for any other reason reserves
    /// nothing, cools down for `blocked_retry_ds` and is retried after.
    #[test]
    fn cannot_afford_reserves_other_rejections_cool_down_and_retry() {
        let r = crate::testkit::rules();
        let a = Arena::frozen(standard_vs_passive(&r), Vec::new());
        let o = order(vec![
            Step::Research {
                tech: "arms_1".into(),
                optional: true,
            },
            Step::Train {
                kind: "scribe".into(),
                count: 1,
                optional: true,
            },
            build("croft", 1, Placement::NearTown),
            build("muster_hall", 1, Placement::NearTown),
        ]);
        let mut ctx = a.ctx(P0);
        ctx.order = &o;
        let mut mem = BotMemory::default();
        assert_eq!(window(&ctx, &mem), [0, 1, 2]);
        step(&mut ctx, &mut mem);
        let tick = a.sim.tick();
        let retry = r.ticks_from_ds(ctx.knobs.blocked_retry_ds);
        assert_eq!(
            mem.blocked,
            [(0, tick + retry), (1, tick + retry)],
            "no Scriptorium hosts arms_1 or trains Scribes"
        );
        assert_eq!(ctx.ledger.reserved, [0, 0, 0, 0]);
        let cmds = ctx.into_commands();
        assert!(
            matches!(cmds[..], [Command::Build { .. }]),
            "the Croft only: {cmds:?}"
        );

        // The cooled steps leave the window; the Muster Hall enters it and
        // cannot be paid for once the Croft is: it saves up.
        let mut ctx = a.ctx(P0);
        ctx.order = &o;
        ctx.ledger.stock[1] = 120;
        assert_eq!(window(&ctx, &mem), [2, 3]);
        step(&mut ctx, &mut mem);
        assert_eq!(
            ctx.ledger.reserved,
            [0, 100, 0, 0],
            "the Muster Hall's 100 Lumber"
        );
        assert_eq!(ctx.ledger.available(1), 0);

        // With a Scriptorium and no Ore or Lore, the sim's own check says
        // CannotAfford: arms_1 saves up its quote, no cooldown.
        let a2 = Arena::frozen(
            standard_vs_passive(&r),
            vec![place(&r, P0, "scriptorium", 18, 58)],
        );
        let mut ctx = a2.ctx(P0);
        ctx.order = &o;
        let mut fresh = BotMemory::default();
        step(&mut ctx, &mut fresh);
        assert_eq!(ctx.ledger.reserved, [0, 0, 50, 75]);
        assert!(!fresh.blocked.iter().any(|(s, _)| *s == 0));

        // After the cooldown, with a Scriptorium and stock, both are done.
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            [
                vec![place(&r, P0, "scriptorium", 18, 58)],
                grant_all(&r, P0, 500),
            ]
            .concat(),
        );
        a.run(retry);
        let mut ctx = a.ctx(P0);
        ctx.order = &o;
        assert_eq!(window(&ctx, &mem), [0, 1, 2], "the cooldowns ran out");
        step(&mut ctx, &mut mem);
        assert!(mem.blocked.is_empty(), "{:?}", mem.blocked);
        let cmds = ctx.into_commands();
        assert!(cmds.iter().any(|c| matches!(c, Command::Research { .. })));
        assert!(cmds.iter().any(|c| matches!(c, Command::Train { .. })));
    }

    /// An `Advance` that cannot be made yet is never cooled down. While the
    /// techs it needs are missing it reserves nothing, so the steps behind
    /// it and production may spend the stock; once they are researched it
    /// saves up its cost (at its scarcest resource's pace, [`paced`]), and
    /// the Yeoman behind it saves up too.
    #[test]
    fn advance_is_never_cooled_down() {
        let r = crate::testkit::rules();
        let o = order(vec![
            Step::Advance {
                age: "masonry".into(),
            },
            Step::Workers {
                count: 9,
                split: [("grain".to_string(), 1)].into_iter().collect(),
            },
        ]);
        let yeoman_kind = unit_kind(&r, "yeoman");
        let a = Arena::frozen(standard_vs_passive(&r), Vec::new());
        let yeoman = a.sim.view().unit_cost(P0, yeoman_kind)[0];
        let mut mem = BotMemory::default();
        for _ in 0..3 {
            let mut ctx = a.ctx(P0);
            ctx.order = &o;
            step(&mut ctx, &mut mem);
            assert!(mem.blocked.is_empty(), "never cooled down");
            assert_eq!(
                ctx.ledger.reserved,
                [0, 0, 0, 0],
                "Masonry's techs are missing: nothing is reserved"
            );
            let cmds = ctx.into_commands();
            assert!(
                cmds.iter()
                    .any(|c| matches!(c, Command::Train { kind, .. } if *kind == yeoman_kind)),
                "the Yeoman spends the Grain: {cmds:?}"
            );
        }
        // The techs done and 100 Lore in stock: the 150 Grain is the
        // scarcest share of Masonry's 250 Grain and 100 Lore (60%), so all
        // of it and 60 Lore are held (`paced`).
        let a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                crate::testkit::research(&r, P0, "arms_1"),
                crate::testkit::research(&r, P0, "trade_1"),
                Command::DebugGrant {
                    owner: P0,
                    resource: 3,
                    amount: 100,
                },
            ],
        );
        assert!(matches!(
            a.sim.view().check_advance_age(P0),
            Err(RejectReason::CannotAfford { .. })
        ));
        for _ in 0..3 {
            let mut ctx = a.ctx(P0);
            ctx.order = &o;
            step(&mut ctx, &mut mem);
            assert!(mem.blocked.is_empty(), "never cooled down");
            assert_eq!(
                ctx.ledger.reserved,
                [150 + yeoman, 0, 0, 60],
                "the techs are done: Masonry's cost is saved at its scarcest resource's pace, and the Yeoman saves up behind it"
            );
            let cmds = ctx.into_commands();
            assert!(
                cmds.is_empty(),
                "the Yeoman cannot use the reserved Grain: {cmds:?}"
            );
        }
    }

    /// Once its techs are done an `Advance` saves up at the pace of its
    /// scarcest resource: every resource of its cost is held at the share
    /// the scarcest one has reached, so the plentiful ones still pay for
    /// the army while the scarce one builds up (the Charter's 300 Lore
    /// starved army production for minutes when its whole cost was held).
    /// Masonry costs 250 Grain and 100 Lore; the start holds 150 Grain.
    #[test]
    fn an_advance_saves_at_the_pace_of_its_scarcest_resource() {
        let r = crate::testkit::rules();
        let o = order(vec![Step::Advance {
            age: "masonry".into(),
        }]);
        let lore = |amount| Command::DebugGrant {
            owner: P0,
            resource: 3,
            amount,
        };
        for (granted, held, trains) in [
            // 40 of 100 Lore: 40% of the Grain is held, 50 Grain is free
            // and a 40-Grain Skirmisher trains.
            (40, [100, 0, 0, 40], true),
            // 120 Lore: Grain is the scarcest now (150 of 250, 60%), all
            // of it is held, and the army waits.
            (120, [150, 0, 0, 60], false),
        ] {
            let a = Arena::frozen(
                standard_vs_passive(&r),
                vec![
                    crate::testkit::research(&r, P0, "arms_1"),
                    crate::testkit::research(&r, P0, "trade_1"),
                    place(&r, P0, "muster_hall", 29, 66),
                    lore(granted),
                ],
            );
            assert!(matches!(
                a.sim.view().check_advance_age(P0),
                Err(RejectReason::CannotAfford { .. })
            ));
            let hall = a.buildings_of(P0, "muster_hall")[0];
            let mut mem = BotMemory {
                military: true,
                ..BotMemory::default()
            };
            let mut ctx = a.ctx(P0);
            ctx.order = &o;
            step(&mut ctx, &mut mem);
            assert_eq!(ctx.ledger.reserved, held, "{granted} Lore");
            crate::standard::army::produce(&mut ctx, &mut mem);
            let cmds = ctx.into_commands();
            assert_eq!(
                cmds.iter()
                    .any(|c| matches!(c, Command::Train { building, .. } if *building == hall)),
                trains,
                "{granted} Lore: {cmds:?}"
            );
        }
    }

    /// While the bot defends, an `Advance` that cannot be made yet keeps
    /// its place in the window but reserves nothing, so production can
    /// spend the stock; it is still never cooled down.
    #[test]
    fn an_advance_reserves_nothing_while_the_bot_defends() {
        let r = crate::testkit::rules();
        let a = Arena::frozen(standard_vs_passive(&r), Vec::new());
        let o = order(vec![Step::Advance {
            age: "masonry".into(),
        }]);
        let tick = a.sim.tick();
        let mut mem = BotMemory {
            defend: Some(crate::standard::memory::Defend {
                town: a.seat(P0),
                last_threat: tick,
                since: tick,
            }),
            ..BotMemory::default()
        };
        let mut ctx = a.ctx(P0);
        ctx.order = &o;
        assert_eq!(window(&ctx, &mem), [0]);
        step(&mut ctx, &mut mem);
        assert_eq!(ctx.ledger.reserved, [0, 0, 0, 0]);
        assert!(mem.blocked.is_empty(), "never cooled down");
        let mut ctx = a.ctx(P0);
        ctx.order = &o;
        assert_eq!(window(&ctx, &mem), [0], "it keeps its place");
    }

    /// The `Military` latch is set once the steps before it are done and
    /// stays set when a lost building reopens one of them.
    #[test]
    fn lost_building_reopens_its_step_without_unlatching_military() {
        let r = crate::testkit::rules();
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            vec![place(&r, P0, "croft", 18, 58)],
        );
        let o = order(vec![
            build("croft", 1, Placement::NearTown),
            Step::Military,
            Step::Wait { until_ds: 72_000 },
        ]);
        let mut mem = BotMemory::default();
        let mut ctx = a.ctx(P0);
        ctx.order = &o;
        assert!(window(&ctx, &mem).is_empty());
        step(&mut ctx, &mut mem);
        assert!(mem.military);
        let croft = a.buildings_of(P0, "croft")[0];
        a.issue(
            P0,
            vec![Command::DebugDamage {
                target: TargetId::Building(croft),
                amount: 10_000,
            }],
        );
        assert!(a.buildings_of(P0, "croft").is_empty());
        let mut ctx = a.ctx(P0);
        ctx.order = &o;
        assert_eq!(window(&ctx, &mem), [0], "the Croft step reopens");
        step(&mut ctx, &mut mem);
        assert!(mem.military, "the latch holds");
        assert!(
            ctx.into_commands()
                .iter()
                .any(|c| matches!(c, Command::Build { .. })),
            "and the Croft is rebuilt"
        );
    }

    /// A site with no builder gets the nearest worker with a `Build` on
    /// its own tile, which resumes it.
    #[test]
    fn site_keeper_resumes_an_abandoned_site() {
        let r = crate::testkit::rules();
        let croft = crate::testkit::building_kind(&r, "croft");
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            vec![Command::DebugPlaceBuilding {
                owner: P0,
                kind: croft,
                tile: sim::Tile::new(29, 66),
                complete: false,
            }],
        );
        let site = a.buildings_of(P0, "croft")[0];
        let o = order(vec![Step::Wait { until_ds: 72_000 }]);
        let mut mem = BotMemory::default();
        let mut ctx = a.ctx(P0);
        ctx.order = &o;
        step(&mut ctx, &mut mem);
        let cmds = ctx.into_commands();
        let [Command::Build { unit, kind, tile }] = cmds[..] else {
            panic!("one resume: {cmds:?}");
        };
        assert_eq!((kind, tile), (croft, sim::Tile::new(29, 66)));
        assert_eq!(unit, sim::UnitId(3), "the Yeoman nearest the site");
        a.issue(P0, cmds);
        assert_eq!(
            a.sim.view().unit(unit).unwrap().job,
            Some(sim::Job::Build { site }),
            "it builds the same site"
        );
        assert_eq!(a.buildings_of(P0, "croft"), [site], "no second site");
        let mut ctx = a.ctx(P0);
        ctx.order = &o;
        step(&mut ctx, &mut mem);
        assert!(ctx.into_commands().is_empty(), "the site has its builder");
        let _ = P1;
    }

    /// A border building's `Build` step (`BorderEdge`) backs off while
    /// enemy military stands within reach of its site (the border
    /// Watchtower was placed again and again in front of an enemy army,
    /// losing its Ore and its builders): the step is blocked and cools
    /// down, nothing is spent and the site is forgotten. An enemy Yeoman
    /// there is no threat, and with no enemy the same step builds at once.
    /// A `NearTown` building, beside a Town the defence covers, is placed
    /// with the same enemies beside its site.
    #[test]
    fn a_build_backs_off_from_enemy_military_at_its_site() {
        let r = crate::testkit::rules();
        let o = order(vec![build("watchtower", 1, Placement::BorderEdge)]);
        let arena = |enemy: Option<(&str, i32, i32)>| {
            let mut setup = grant_all(&r, P0, 200);
            if let Some((kind, x, y)) = enemy {
                setup.push(crate::testkit::spawn(&r, P1, kind, x, y, 3));
            }
            Arena::frozen(standard_vs_passive(&r), setup)
        };
        // With no enemy: the tower is placed at the border edge.
        let quiet = arena(None);
        let mut mem = BotMemory::default();
        let mut ctx = quiet.ctx(P0);
        ctx.order = &o;
        step(&mut ctx, &mut mem);
        let cmds = ctx.into_commands();
        let [Command::Build { tile, .. }] = cmds[..] else {
            panic!("the tower is placed: {cmds:?}");
        };
        let (x, y) = (i32::from(tile.x) + 3, i32::from(tile.y) + 3);
        // Enemy Skirmishers three tiles off the site: blocked.
        let raided = arena(Some(("skirmisher", x, y)));
        let mut mem = BotMemory::default();
        let mut ctx = raided.ctx(P0);
        ctx.order = &o;
        step(&mut ctx, &mut mem);
        let stock = ctx.ledger.stock.clone();
        assert!(
            ctx.into_commands().is_empty(),
            "no tower in front of an army"
        );
        assert_eq!(stock, [200 + 150, 200 + 150, 200, 200], "nothing spent");
        let tick = raided.sim.tick();
        let retry = raided.ctx(P0).ticks(raided.ctx(P0).knobs.blocked_retry_ds);
        assert_eq!(mem.blocked, [(0, tick + retry)], "cooled down");
        assert_eq!(mem.site, None, "the site is forgotten");
        // Enemy Yeomen there: placed all the same.
        let civilians = arena(Some(("yeoman", x, y)));
        let mut mem = BotMemory::default();
        let mut ctx = civilians.ctx(P0);
        ctx.order = &o;
        step(&mut ctx, &mut mem);
        assert!(
            matches!(ctx.into_commands()[..], [Command::Build { .. }]),
            "Yeomen are no threat"
        );
        // A Croft beside the Seat: placed though enemies stand by its site.
        let croft = order(vec![build("croft", 1, Placement::NearTown)]);
        let mut ctx = quiet.ctx(P0);
        ctx.order = &croft;
        step(&mut ctx, &mut BotMemory::default());
        let cmds = ctx.into_commands();
        let [Command::Build { tile, .. }] = cmds[..] else {
            panic!("the Croft is placed: {cmds:?}");
        };
        let near = arena(Some((
            "skirmisher",
            i32::from(tile.x) + 3,
            i32::from(tile.y) + 3,
        )));
        let mut ctx = near.ctx(P0);
        ctx.order = &croft;
        step(&mut ctx, &mut BotMemory::default());
        assert!(
            matches!(ctx.into_commands()[..], [Command::Build { .. }]),
            "a NearTown site is the defence's to keep"
        );
    }

    /// The site keeper sends no builder to a border building's site with
    /// enemy military in reach (the builders walked into the army one by
    /// one); a tower site with no enemy near gets one, and so does a Croft
    /// site with the same enemies beside it.
    #[test]
    fn site_keeper_sends_no_builder_into_an_enemy_army() {
        let r = crate::testkit::rules();
        let tower = crate::testkit::building_kind(&r, "watchtower");
        let croft = crate::testkit::building_kind(&r, "croft");
        let o = order(vec![Step::Wait { until_ds: 72_000 }]);
        for (kind, (x, y), raided, resumed) in [
            (tower, (40, 64), false, true),
            (tower, (40, 64), true, false),
            (croft, (29, 66), true, true),
        ] {
            let mut setup = vec![Command::DebugPlaceBuilding {
                owner: P0,
                kind,
                tile: sim::Tile::new(x, y),
                complete: false,
            }];
            if raided {
                setup.push(crate::testkit::spawn(
                    &r,
                    P1,
                    "skirmisher",
                    i32::from(x) + 4,
                    i32::from(y) + 3,
                    3,
                ));
            }
            let a = Arena::frozen(standard_vs_passive(&r), setup);
            let mut mem = BotMemory::default();
            let mut ctx = a.ctx(P0);
            ctx.order = &o;
            step(&mut ctx, &mut mem);
            let cmds = ctx.into_commands();
            assert_eq!(
                matches!(cmds[..], [Command::Build { kind: k, .. }] if k == kind),
                resumed,
                "{kind:?} raided {raided}: {cmds:?}"
            );
        }
    }

    /// The site chosen for a `Build` step is remembered while the step is
    /// worked on and forgotten once the building stands.
    #[test]
    fn a_remembered_site_is_forgotten_once_its_step_is_done() {
        let r = crate::testkit::rules();
        let mut a = Arena::frozen(standard_vs_passive(&r), Vec::new());
        let o = order(vec![
            build("croft", 1, Placement::NearTown),
            Step::Wait { until_ds: 72_000 },
        ]);
        let mut mem = BotMemory::default();
        let mut ctx = a.ctx(P0);
        ctx.order = &o;
        step(&mut ctx, &mut mem);
        let cmds = ctx.into_commands();
        let [Command::Build { tile, .. }] = cmds[..] else {
            panic!("{cmds:?}");
        };
        assert_eq!(mem.site, Some((0, tile)));
        a.issue(P0, cmds);
        let mut ctx = a.ctx(P0);
        ctx.order = &o;
        assert!(window(&ctx, &mem).is_empty(), "the site counts");
        step(&mut ctx, &mut mem);
        assert_eq!(mem.site, None);
        assert_eq!(mem, BotMemory::default(), "nothing left to remember");
    }

    /// Two `NearTown` steps worked in the same plan never pick footprints
    /// closer than `building_gap_tiles`: the second site keeps clear of
    /// the first, which is not placed yet, so neither `Build` is rejected
    /// when they apply.
    #[test]
    fn two_near_town_sites_in_one_plan_keep_their_gap() {
        let r = crate::testkit::rules();
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            [
                vec![place(&r, P0, "croft", 20, 60)],
                grant_all(&r, P0, 1000),
            ]
            .concat(),
        );
        let o = order(vec![
            build("muster_hall", 1, Placement::NearTown),
            build("scriptorium", 1, Placement::NearTown),
        ]);
        let mut ctx = a.ctx(P0);
        ctx.order = &o;
        let mut mem = BotMemory::default();
        step(&mut ctx, &mut mem);
        let gap = i32::try_from(ctx.knobs.building_gap_tiles).unwrap();
        let cmds = ctx.into_commands();
        let rects: Vec<(i32, i32, i32, i32)> = cmds
            .iter()
            .filter_map(|c| match c {
                Command::Build { kind, tile, .. } => {
                    let d = r.building_kind(kind.0).unwrap();
                    let (x, y) = (i32::from(tile.x), i32::from(tile.y));
                    let w = i32::try_from(d.width_tiles).unwrap();
                    let h = i32::try_from(d.height_tiles).unwrap();
                    Some((x, y, x + w - 1, y + h - 1))
                }
                _ => None,
            })
            .collect();
        assert_eq!(rects.len(), 2, "both steps build: {cmds:?}");
        let (p, q) = (rects[0], rects[1]);
        let apart = p.2 + gap < q.0 || q.2 + gap < p.0 || p.3 + gap < q.1 || q.3 + gap < p.1;
        assert!(apart, "footprints {p:?} and {q:?} within {gap} tiles");
        a.sim.drain_events();
        a.issue(P0, cmds);
        let rejected: Vec<SimEvent> = a
            .sim
            .drain_events()
            .into_iter()
            .filter(|e| matches!(e, SimEvent::CommandRejected { .. }))
            .collect();
        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!(a.buildings_of(P0, "muster_hall").len(), 1);
        assert_eq!(a.buildings_of(P0, "scriptorium").len(), 1);
    }

    /// At the first plan the builders the build order takes are never also
    /// sent to gather.
    #[test]
    fn claimed_builder_is_never_sent_to_gather() {
        let r = crate::testkit::rules();
        let a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                place(&r, P0, "croft", 18, 58),
                place(&r, P0, "lumber_yard", 16, 56),
            ],
        );
        assert_eq!(a.sim.view().idle_workers(P0).len(), 5);
        let (cmds, _) = a.plan(P0);
        let builders: Vec<sim::UnitId> = cmds
            .iter()
            .filter_map(|c| match c {
                Command::Build { unit, .. } => Some(*unit),
                _ => None,
            })
            .collect();
        let gatherers: Vec<sim::UnitId> = cmds
            .iter()
            .filter_map(|c| match c {
                Command::Gather { units, .. } => Some(units.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(builders.len(), 2, "a Croft and the Scriptorium: {cmds:?}");
        assert_eq!(gatherers.len(), 3, "the other three gather: {cmds:?}");
        assert!(builders.iter().all(|b| !gatherers.contains(b)));
    }
}
