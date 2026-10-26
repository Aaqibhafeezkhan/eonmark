//! Worker balancing (docs/design/ai.md rule 2): the active `Workers`
//! step's split decides how the gatherers are shared between the
//! resources, idle workers go where the deficit is largest, and a few
//! gatherers a plan move from the largest surplus to the largest deficit.
//!
//! The gatherers are the workers that are neither building a site nor
//! claimed by an earlier rule of the plan. Their share per resource comes
//! from the split weights by the largest-remainder method, each share
//! clamped to the slots of the bot's complete buildings of the resource
//! and, while the resource's income is at its Yield Cap, to the gatherers
//! it already has (no worker is added at the cap); what a clamp frees goes
//! to the other resources by weight. Anything left over (no slot anywhere)
//! is left to the sim's own idle seeking. Training workers is the
//! `Workers` step's own job (`build_order`), and the Easy and Hard income
//! cheat of M6 is the sim's, not this rule's.

use super::build_order;
use super::ctx::Ctx;
use super::memory::BotMemory;
use rules::Step;
use sim::{BuildingId, Command, UnitId};
use std::collections::{BTreeMap, BTreeSet};

/// Share `total` between the entries of `weights` by the largest-remainder
/// method: each gets `total × w / Σw` rounded down, then the units left go
/// one each to the largest remainders, ties to the lower index. All zero
/// when every weight is 0.
pub fn apportion(total: u32, weights: &[u32]) -> Vec<u32> {
    let sum: u64 = weights.iter().map(|w| u64::from(*w)).sum();
    if sum == 0 {
        return vec![0; weights.len()];
    }
    let t = u64::from(total);
    let mut out: Vec<u32> = weights
        .iter()
        .map(|w| u32::try_from(t * u64::from(*w) / sum).expect("at most total"))
        .collect();
    let given: u32 = out.iter().sum();
    let mut order: Vec<(u64, usize)> = weights
        .iter()
        .enumerate()
        .filter(|(_, w)| **w > 0)
        .map(|(i, w)| ((t * u64::from(*w)) % sum, i))
        .collect();
    order.sort_by_key(|(rem, i)| (core::cmp::Reverse(*rem), *i));
    for (_, i) in order
        .into_iter()
        .take(usize::try_from(total - given).expect("small"))
    {
        out[i] += 1;
    }
    out
}

/// Share `total` gatherers by `weights` with each share at most its
/// `limits` entry: [`apportion`], then every share over its limit is fixed
/// at the limit and the rest is shared again among the others, until no
/// share exceeds its limit. The result may sum to less than `total` when
/// the limits are too small.
pub fn targets(total: u32, weights: &[u32], limits: &[u32]) -> Vec<u32> {
    let n = weights.len();
    let mut fixed: Vec<Option<u32>> = (0..n).map(|r| (weights[r] == 0).then_some(0)).collect();
    let mut remaining = total;
    loop {
        let open: Vec<u32> = (0..n)
            .map(|r| if fixed[r].is_none() { weights[r] } else { 0 })
            .collect();
        if open.iter().all(|w| *w == 0) {
            break;
        }
        let share = apportion(remaining, &open);
        let mut clamped = false;
        for r in 0..n {
            if fixed[r].is_none() && share[r] > limits[r] {
                fixed[r] = Some(limits[r]);
                remaining = remaining.saturating_sub(limits[r]);
                clamped = true;
            }
        }
        if !clamped {
            for r in 0..n {
                if fixed[r].is_none() {
                    fixed[r] = Some(share[r]);
                }
            }
            break;
        }
    }
    fixed.into_iter().map(|v| v.unwrap_or(0)).collect()
}

/// The split weights per resource index of the active `Workers` step (the
/// latest at or before the window's first step; the first `Workers` step
/// when none is), all zero when the build order has none.
pub fn active_split(ctx: &Ctx<'_>, mem: &BotMemory) -> Vec<u32> {
    let rules = ctx.view.rules();
    let n = rules.resources.resources.len();
    let window = build_order::window(ctx, mem);
    let anchor = window.first().copied().unwrap_or(ctx.order.steps.len());
    let is_workers = |s: &&Step| matches!(s, Step::Workers { .. });
    let step = ctx.order.steps[..anchor.min(ctx.order.steps.len())]
        .iter()
        .rev()
        .find(is_workers)
        .or_else(|| ctx.order.steps.get(anchor).filter(is_workers))
        .or_else(|| ctx.order.steps.iter().find(is_workers));
    let mut w = vec![0; n];
    if let Some(Step::Workers { split, .. }) = step {
        for (id, weight) in split {
            if let Some(r) = rules.resource_index(id) {
                w[r] = *weight;
            }
        }
    }
    w
}

/// The economy's view of the gatherers this plan: who may be moved, how
/// many each resource has and should have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shares {
    /// Workers neither building nor claimed, in id order.
    pub pool: Vec<UnitId>,
    /// The pool's gatherers per resource index.
    pub current: Vec<u32>,
    /// What each resource should have.
    pub desired: Vec<u32>,
}

/// Compute [`Shares`] for the plan.
pub fn shares(ctx: &Ctx<'_>, mem: &BotMemory) -> Shares {
    let f = &ctx.facts;
    let building: BTreeSet<UnitId> = f.builders.values().flatten().copied().collect();
    let pool: Vec<UnitId> = f
        .workers
        .iter()
        .copied()
        .filter(|u| !building.contains(u) && !ctx.is_claimed(*u))
        .collect();
    let in_pool: BTreeSet<UnitId> = pool.iter().copied().collect();
    let current: Vec<u32> = f
        .gatherers
        .iter()
        .map(|g| u32::try_from(g.iter().filter(|u| in_pool.contains(u)).count()).expect("few"))
        .collect();
    let weights = active_split(ctx, mem);
    let income = ctx.view.income(ctx.me);
    let limits: Vec<u32> = (0..weights.len())
        .map(|r| {
            let slots: u32 = f.nodes[r].iter().map(|n| u32::from(n.slots)).sum();
            let at_cap = income
                .get(r)
                .is_some_and(|i| i.cap.is_some_and(|c| i.raw >= c));
            if at_cap { slots.min(current[r]) } else { slots }
        })
        .collect();
    let total = u32::try_from(pool.len()).expect("few workers");
    let desired = targets(total, &weights, &limits);
    Shares {
        pool,
        current,
        desired,
    }
}

/// The resource with the largest `desired − current` above 0 among those
/// with a free slot, ties to the lower index.
fn deficit(s: &Shares, free: &BTreeMap<BuildingId, (usize, u8)>) -> Option<usize> {
    (0..s.desired.len())
        .filter(|r| s.desired[*r] > s.current[*r])
        .filter(|r| free.values().any(|(res, n)| res == r && *n > 0))
        .max_by_key(|r| (s.desired[*r] - s.current[*r], core::cmp::Reverse(*r)))
}

/// Run the rule: idle workers to the deficit, then up to
/// `rebalance_per_think` moves from surplus to deficit. Every order is a
/// `Gather` checked with `check_gather` first, grouped by node.
pub fn step(ctx: &mut Ctx<'_>, mem: &mut BotMemory) {
    let mut s = shares(ctx, mem);
    let mut free: BTreeMap<BuildingId, (usize, u8)> = BTreeMap::new();
    for (r, nodes) in ctx.facts.nodes.iter().enumerate() {
        for n in nodes {
            free.insert(n.id, (r, n.free()));
        }
    }
    let mut orders: BTreeMap<BuildingId, Vec<UnitId>> = BTreeMap::new();
    let idle: Vec<UnitId> = ctx
        .facts
        .idle_workers
        .iter()
        .copied()
        .filter(|u| s.pool.contains(u))
        .collect();
    for u in idle {
        let Some(r) = deficit(&s, &free) else {
            break;
        };
        if let Some(node) = nearest_free(ctx, u, r, &free)
            && ctx
                .view
                .check_gather(ctx.me, &[u], node)
                .is_ok_and(|v| !v.is_empty())
        {
            take(&mut free, node);
            s.current[r] += 1;
            ctx.claim(u);
            orders.entry(node).or_default().push(u);
        }
    }
    let gatherers = ctx.facts.gatherers.clone();
    for _ in 0..ctx.knobs.rebalance_per_think {
        let Some(d) = deficit(&s, &free) else {
            break;
        };
        let Some(surplus) = (0..s.desired.len())
            .filter(|r| s.current[*r] > s.desired[*r])
            .max_by_key(|r| (s.current[*r] - s.desired[*r], core::cmp::Reverse(*r)))
        else {
            break;
        };
        let best = gatherers[surplus]
            .iter()
            .copied()
            .filter(|u| s.pool.contains(u) && !ctx.is_claimed(*u))
            .filter_map(|u| {
                let node = nearest_free(ctx, u, d, &free)?;
                let pos = ctx.view.unit(u)?.pos;
                let centre = ctx.facts.nodes[d].iter().find(|n| n.id == node)?.centre;
                Some((pos.dist_sq_i64(centre), u, node))
            })
            .min();
        let Some((_, u, node)) = best else {
            break;
        };
        ctx.claim(u);
        if ctx
            .view
            .check_gather(ctx.me, &[u], node)
            .is_ok_and(|v| !v.is_empty())
        {
            take(&mut free, node);
            s.current[surplus] -= 1;
            s.current[d] += 1;
            orders.entry(node).or_default().push(u);
        }
    }
    for (node, units) in orders {
        if !ctx.push(Command::Gather { units, node }) {
            break;
        }
    }
}

/// The node of resource `r` with a free slot nearest unit `u` (ties to the
/// lowest id).
fn nearest_free(
    ctx: &Ctx<'_>,
    u: UnitId,
    r: usize,
    free: &BTreeMap<BuildingId, (usize, u8)>,
) -> Option<BuildingId> {
    let pos = ctx.view.unit(u)?.pos;
    ctx.facts.nodes[r]
        .iter()
        .filter(|n| free.get(&n.id).is_some_and(|(_, k)| *k > 0))
        .min_by_key(|n| (pos.dist_sq_i64(n.centre), n.id))
        .map(|n| n.id)
}

fn take(free: &mut BTreeMap<BuildingId, (usize, u8)>, node: BuildingId) {
    if let Some((_, n)) = free.get_mut(&node) {
        *n = n.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{Arena, P0, place, spawn, standard_vs_passive};
    use rules::{BuildOrder, Step};

    #[test]
    fn apportions_split_largest_remainder() {
        assert_eq!(apportion(9, &[4, 5, 0, 0]), [4, 5, 0, 0]);
        assert_eq!(apportion(7, &[3, 4]), [3, 4]);
        assert_eq!(
            apportion(10, &[1, 1, 1]),
            [4, 3, 3],
            "ties to the lower index"
        );
        // 5 x (4, 5, 2) / 11 = 1.82, 2.27, 0.91: floors 1, 2, 0; the two
        // left go to the largest remainders, index 2 then index 0.
        assert_eq!(apportion(5, &[4, 5, 2]), [2, 2, 1]);
        assert_eq!(apportion(3, &[0, 0]), [0, 0]);
        assert_eq!(apportion(0, &[1, 2]), [0, 0]);
        // A clamped share frees its gatherers for the others, by weight.
        assert_eq!(targets(9, &[4, 5], &[3, 100]), [3, 6]);
        assert_eq!(targets(9, &[4, 5, 2], &[1, 2, 100]), [1, 2, 6]);
        assert_eq!(targets(9, &[4, 5], &[1, 2]), [1, 2], "the rest idles");
        assert_eq!(targets(9, &[0, 5], &[100, 100]), [0, 9]);
    }

    fn workers(split: &[(&str, u32)]) -> BuildOrder {
        BuildOrder {
            id: "test".into(),
            steps: vec![Step::Workers {
                count: 5,
                split: split.iter().map(|(r, w)| ((*r).to_string(), *w)).collect(),
            }],
        }
    }

    /// Five idle Yeomen with one Croft slot and a Lumber Yard: the split
    /// (1 Grain, 4 Lumber) sends one to the Croft and four to the yard, one
    /// `Gather` per node.
    #[test]
    fn idle_yeomen_gather_the_deficit() {
        let r = crate::testkit::rules();
        let a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                place(&r, P0, "croft", 29, 66),
                place(&r, P0, "lumber_yard", 16, 56),
            ],
        );
        let croft = a.buildings_of(P0, "croft")[0];
        let yard = a.buildings_of(P0, "lumber_yard")[0];
        let o = workers(&[("grain", 1), ("lumber", 4)]);
        let mut ctx = a.ctx(P0);
        ctx.order = &o;
        let mem = BotMemory::default();
        let s = shares(&ctx, &mem);
        assert_eq!(s.pool.len(), 5);
        assert_eq!(s.desired, [1, 4, 0, 0]);
        let mut mem = mem;
        step(&mut ctx, &mut mem);
        let cmds = ctx.into_commands();
        let mut to = BTreeMap::new();
        for c in &cmds {
            let Command::Gather { units, node } = c else {
                panic!("{c:?}");
            };
            to.insert(*node, units.len());
        }
        assert_eq!(to, BTreeMap::from([(croft, 1), (yard, 4)]), "{cmds:?}");
        assert_eq!(cmds.len(), 2);
    }

    /// A resource whose income is at its Yield Cap gets no more
    /// gatherers: its share is clamped to the gatherers it has, and below
    /// the cap the same idle Yeoman is sent.
    #[test]
    fn adds_no_gatherer_at_the_yield_cap() {
        let r = crate::testkit::rules();
        let mut a = Arena::frozen(
            standard_vs_passive(&r),
            vec![
                place(&r, P0, "lumber_yard", 16, 56),
                place(&r, P0, "lumber_yard", 11, 56),
                spawn(&r, P0, "yeoman", 20, 60, 1),
            ],
        );
        let yards = a.buildings_of(P0, "lumber_yard");
        let yeomen = a.units_of(P0, "yeoman");
        assert_eq!(yeomen.len(), 6);
        // Six Yeomen to the yards: 6 x 10 + the Town's 10 = 70 = the Yield
        // Cap.
        a.issue(
            P0,
            vec![
                Command::Gather {
                    units: yeomen[..4].to_vec(),
                    node: yards[0],
                },
                Command::Gather {
                    units: yeomen[4..].to_vec(),
                    node: yards[1],
                },
            ],
        );
        a.run(600);
        let lumber = a.sim.view().income(P0)[1];
        assert_eq!((lumber.raw, lumber.cap), (70, Some(70)));
        // A seventh Yeoman, idle (the sim's own seeking waits a while).
        a.issue(P0, vec![spawn(&r, P0, "yeoman", 20, 60, 1)]);
        assert_eq!(a.sim.view().idle_workers(P0).len(), 1);
        let o = workers(&[("lumber", 1)]);
        let mut ctx = a.ctx(P0);
        ctx.order = &o;
        let mut mem = BotMemory::default();
        let s = shares(&ctx, &mem);
        assert_eq!(s.current[1], 6);
        assert_eq!(s.desired[1], 6, "clamped to the six it has");
        step(&mut ctx, &mut mem);
        assert!(ctx.into_commands().is_empty(), "the seventh stays idle");

        // Below the cap the idle Yeoman goes.
        a.issue(
            P0,
            vec![Command::Stop {
                units: vec![yeomen[0]],
            }],
        );
        a.run(20);
        assert!(a.sim.view().income(P0)[1].raw < 70);
        let mut ctx = a.ctx(P0);
        ctx.order = &o;
        step(&mut ctx, &mut mem);
        let cmds = ctx.into_commands();
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::Gather { node, .. } if yards.contains(node))),
            "{cmds:?}"
        );
    }
}
