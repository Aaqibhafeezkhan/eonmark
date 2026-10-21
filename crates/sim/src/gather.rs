//! Gather slots: the `Gather` command, stand tiles, worked-slot income and
//! idle workers seeking work on their own (`docs/design/economy.md`, "Gather
//! slots and rates" and "Idle Yeoman work seeking").
//!
//! A slot is taken by a unit holding [`Job::Gather`] on it; there is no other
//! record of occupancy, so nothing can fall out of sync. Slot `k` has a
//! stand tile for a given worker: the `(k mod n)`-th of the `n` ring tiles
//! around the footprint (clockwise from the north-west corner) that the
//! worker can walk to. A gathering building is complete, so its footprint
//! is blocked and "can walk to" is "in the worker's connected component".
//! A slot is worked, and earns its rate, while its unit is at work there
//! (`construction::at_work`): no order, and its post adjacent to the
//! footprint.

use crate::construction::{at_work, building_rect, centre_fx, order_move, ring};
use crate::economy::building_kind;
use crate::fx::FxVec2;
use crate::ids::{BuildingId, PlayerId, Tile, UnitId, UnitKindId};
use crate::map::{Map, NO_COMPONENT};
use crate::state::{Construction, Job, RejectReason, State};
use rules::Rules;
use std::collections::{BTreeMap, BTreeSet};

/// The unit kind that works `kind`'s slots, if it gathers.
fn worker_kind(rules: &Rules, node_kind: crate::ids::BuildingKindId) -> Option<UnitKindId> {
    let g = building_kind(rules, node_kind).gather.as_ref()?;
    let (i, _) = rules
        .units
        .iter()
        .enumerate()
        .find(|(_, u)| u.id == g.worker)
        .expect("validated gather worker");
    Some(UnitKindId(u16::try_from(i).expect("validated len")))
}

/// Every unit kind that works some building's slots (Yeoman, Scribe).
pub(crate) fn worker_kinds(rules: &Rules) -> BTreeSet<UnitKindId> {
    (0..rules.buildings.len())
        .filter_map(|k| {
            worker_kind(
                rules,
                crate::ids::BuildingKindId(u16::try_from(k).expect("validated len")),
            )
        })
        .collect()
}

/// The slots of `node` taken by a unit's [`Job::Gather`], working or walking.
pub(crate) fn taken_slots(state: &State, node: BuildingId) -> BTreeSet<u8> {
    state
        .units
        .values()
        .filter_map(|u| match u.job {
            Some(Job::Gather { node: n, slot }) if n == node => Some(slot),
            _ => None,
        })
        .collect()
}

/// `node`'s `(worked, taken)` slots in one pass over the units: taken by a
/// unit's gather job (walking or working), worked when that unit is at
/// work.
pub(crate) fn slot_counts(state: &State, rules: &Rules, node: BuildingId) -> (u8, u8) {
    let Some(b) = state.buildings.get(&node) else {
        return (0, 0);
    };
    let rect = building_rect(rules, &state.map, b);
    let (mut worked, mut taken) = (0u8, 0u8);
    for u in state.units.values() {
        if matches!(u.job, Some(Job::Gather { node: n, .. }) if n == node) {
            taken = taken.checked_add(1).expect("at most 255 slots");
            if at_work(u, &state.map, rect) {
                worked += 1;
            }
        }
    }
    (worked, taken)
}

/// The connected component a unit at `pos` walks in (its tile's, or the
/// nearest passable tile's when it somehow stands on a blocked one).
fn component_at(map: &Map, pos: FxVec2) -> u16 {
    map.nearest_passable(map.tile_of(pos), None)
        .map_or(NO_COMPONENT, |t| map.component_of(t))
}

/// The stand tile of slot `slot` of `node` for a worker at `pos`, or `None`
/// when no ring tile is reachable from there.
fn stand_tile(
    state: &State,
    rules: &Rules,
    node: BuildingId,
    slot: u8,
    pos: FxVec2,
) -> Option<Tile> {
    let b = &state.buildings[&node];
    let rect = building_rect(rules, &state.map, b);
    let map = &state.map;
    let comp = component_at(map, pos);
    let free: Vec<Tile> = ring(rect, map.width(), map.height())
        .into_iter()
        .filter(|t| map.passable(*t) && map.component_of(*t) == comp)
        .collect();
    (!free.is_empty()).then(|| free[usize::from(slot) % free.len()])
}

/// The lowest slot of `node` not in `taken`, if any is free.
fn lowest_free(slots: u8, taken: &BTreeSet<u8>) -> Option<u8> {
    (0..slots).find(|s| !taken.contains(s))
}

/// Give `unit` slot `slot` of `node` and send it to the slot's stand tile.
fn assign(state: &mut State, rules: &Rules, unit: UnitId, node: BuildingId, slot: u8, tick: u32) {
    let pos = state.units[&unit].pos;
    let stand =
        stand_tile(state, rules, node, slot, pos).expect("caller checked a reachable ring tile");
    let goal = state.map.center_of(stand);
    let u = state.units.get_mut(&unit).expect("unit exists");
    u.job = Some(Job::Gather { node, slot });
    u.seek_at = None;
    order_move(state, rules, unit, goal, tick);
}

/// What an accepted `Gather` does: these units take these slots.
pub(crate) type GatherPlan = Vec<(UnitId, u8)>;

/// Validate `Gather { units, node }` for `p` without changing anything.
/// Checks, first failure wins: the node is `p`'s (`UnknownBuilding`), is
/// complete (`UnderConstruction`) and gathers (`NotAGatherNode`); some
/// given unit is `p`'s, of the node's worker kind and able to walk to a
/// tile beside it (`NoValidUnits`); when every such unit already works the
/// node, the order is accepted and changes nothing; otherwise a slot is
/// free (`NoFreeSlot`). Candidates in id order that are not already on the
/// node take the lowest free slots; the rest are left as they are.
pub(crate) fn validate_gather(
    state: &State,
    rules: &Rules,
    p: PlayerId,
    units: &[UnitId],
    node: BuildingId,
) -> Result<GatherPlan, RejectReason> {
    let b = state
        .buildings
        .get(&node)
        .filter(|b| b.owner == p)
        .ok_or(RejectReason::UnknownBuilding)?;
    if b.construction != Construction::Complete {
        return Err(RejectReason::UnderConstruction);
    }
    let worker = worker_kind(rules, b.kind).ok_or(RejectReason::NotAGatherNode)?;
    // Deduplicate before the per-unit reach check, so a long repeated list
    // costs no more than the distinct units in it.
    let mine: BTreeSet<UnitId> = units
        .iter()
        .copied()
        .filter(|id| {
            state
                .units
                .get(id)
                .is_some_and(|u| u.owner == p && u.kind == worker)
        })
        .collect();
    let candidates: Vec<UnitId> = mine
        .into_iter()
        .filter(|id| stand_tile(state, rules, node, 0, state.units[id].pos).is_some())
        .collect();
    if candidates.is_empty() {
        return Err(RejectReason::NoValidUnits);
    }
    let newcomers: Vec<UnitId> = candidates
        .into_iter()
        .filter(|id| !matches!(state.units[id].job, Some(Job::Gather { node: n, .. }) if n == node))
        .collect();
    // Workers already on the node keep their slots: re-issuing the order
    // to them alone is accepted and changes nothing, full node or not.
    if newcomers.is_empty() {
        return Ok(Vec::new());
    }
    let mut taken = taken_slots(state, node);
    if lowest_free(b.slots, &taken).is_none() {
        return Err(RejectReason::NoFreeSlot);
    }
    let mut plan = Vec::new();
    for id in newcomers {
        let Some(slot) = lowest_free(b.slots, &taken) else {
            break;
        };
        taken.insert(slot);
        plan.push((id, slot));
    }
    Ok(plan)
}

/// Apply a validated `Gather`.
pub(crate) fn apply_gather(
    state: &mut State,
    rules: &Rules,
    node: BuildingId,
    plan: &GatherPlan,
    tick: u32,
) {
    for &(unit, slot) in plan {
        assign(state, rules, unit, node, slot, tick);
    }
}

/// Put `unit`, which just finished building `node` or was just trained
/// there, to work at `node` when it is the node's worker kind, can reach a
/// tile beside it and a slot is free; returns whether it was.
pub(crate) fn unit_takes_slot(
    state: &mut State,
    rules: &Rules,
    unit: UnitId,
    node: BuildingId,
    tick: u32,
) -> bool {
    let Some(u) = state.units.get(&unit) else {
        return false;
    };
    let b = &state.buildings[&node];
    if worker_kind(rules, b.kind) != Some(u.kind)
        || stand_tile(state, rules, node, 0, u.pos).is_none()
    {
        return false;
    }
    let Some(slot) = lowest_free(b.slots, &taken_slots(state, node)) else {
        return false;
    };
    assign(state, rules, unit, node, slot, tick);
    true
}

/// Step 6c, second half: idle workers look for work. A unit of a worker kind
/// with no job and no order is idle (holding a post is still idle). An idle
/// unit without a timer starts one (`idle_seek_delay`); when it runs out the
/// unit takes the lowest free slot of the nearest own complete building its
/// kind works (squared distance to the footprint centre, then
/// `BuildingId`), or retries after `idle_seek_retry`. A busy unit has no
/// timer. Units are visited in id order and see the slots taken earlier in
/// the same tick.
pub(crate) fn idle_seek_step(state: &mut State, rules: &Rules, tick: u32) {
    // One read-only pass: each worker's decision depends only on its own
    // fields, so it can be taken before any write. Assigning a slot below
    // changes only the assigned unit (and the path queue), never another
    // unit's decision.
    let workers = worker_kinds(rules);
    let mut clear = Vec::new();
    let mut start = Vec::new();
    let mut due = Vec::new();
    for u in state.units.values() {
        if !workers.contains(&u.kind) {
            continue;
        }
        if u.job.is_some() || u.order.is_some() {
            if u.seek_at.is_some() {
                clear.push(u.id);
            }
        } else {
            match u.seek_at {
                None => start.push(u.id),
                Some(at) if tick >= at => due.push(u.id),
                Some(_) => {}
            }
        }
    }
    for id in clear {
        state.units.get_mut(&id).expect("exists").seek_at = None;
    }
    for id in start {
        state.units.get_mut(&id).expect("exists").seek_at =
            Some(tick + rules.idle_seek_delay_ticks());
    }
    if due.is_empty() {
        return;
    }
    let mut taken: BTreeMap<BuildingId, BTreeSet<u8>> = BTreeMap::new();
    for u in state.units.values() {
        if let Some(Job::Gather { node, slot }) = u.job {
            taken.entry(node).or_default().insert(slot);
        }
    }
    let none = BTreeSet::new();
    for id in due {
        let u = &state.units[&id];
        let (owner, kind, pos) = (u.owner, u.kind, u.pos);
        let best = state
            .buildings
            .values()
            .filter(|b| {
                b.owner == owner
                    && b.construction == Construction::Complete
                    && worker_kind(rules, b.kind) == Some(kind)
                    && lowest_free(b.slots, taken.get(&b.id).unwrap_or(&none)).is_some()
                    && stand_tile(state, rules, b.id, 0, pos).is_some()
            })
            .min_by_key(|b| {
                let c = centre_fx(building_kind(rules, b.kind), b.tile);
                (pos.dist_sq_i64(c), b.id)
            })
            .map(|b| (b.id, b.slots));
        match best {
            Some((node, slots)) => {
                let t = taken.entry(node).or_default();
                let slot = lowest_free(slots, t).expect("filtered to a free slot");
                t.insert(slot);
                assign(state, rules, id, node, slot, tick);
            }
            None => {
                state.units.get_mut(&id).expect("exists").seek_at =
                    Some(tick + rules.idle_seek_retry_ticks());
            }
        }
    }
}

/// Raw income per resource from worked slots of `p`'s complete buildings.
pub(crate) fn worked_income(state: &State, rules: &Rules, p: PlayerId, raw: &mut [u32]) {
    for u in state.units.values() {
        let Some(Job::Gather { node, .. }) = u.job else {
            continue;
        };
        if u.owner != p {
            continue;
        }
        let Some(b) = state.buildings.get(&node) else {
            continue;
        };
        if b.owner != p || b.construction != Construction::Complete {
            continue;
        }
        if !at_work(u, &state.map, building_rect(rules, &state.map, b)) {
            continue;
        }
        let g = building_kind(rules, b.kind)
            .gather
            .as_ref()
            .expect("a gather job is only on a gatherer");
        let r = rules
            .resource_index(&g.resource)
            .expect("validated resource");
        raw[r] = raw[r]
            .checked_add(g.rate_per_worker)
            .expect("income rate fits u32");
    }
}

/// Whether `u`'s gather job can still be done: the node exists, is its
/// owner's, complete and worked by its kind, and the unit is walking there
/// or at work.
pub(crate) fn gather_job_valid(
    state: &State,
    rules: &Rules,
    unit: UnitId,
    node: BuildingId,
) -> bool {
    let u = &state.units[&unit];
    state.buildings.get(&node).is_some_and(|b| {
        b.owner == u.owner
            && b.construction == Construction::Complete
            && worker_kind(rules, b.kind) == Some(u.kind)
            && (u.order.is_some() || at_work(u, &state.map, building_rect(rules, &state.map, b)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowest_free_slot() {
        let taken: BTreeSet<u8> = [0, 2].into_iter().collect();
        assert_eq!(lowest_free(3, &taken), Some(1));
        assert_eq!(lowest_free(2, &taken), Some(1));
        let full: BTreeSet<u8> = [0, 1].into_iter().collect();
        assert_eq!(lowest_free(2, &full), None);
        assert_eq!(lowest_free(0, &BTreeSet::new()), None);
    }
}
