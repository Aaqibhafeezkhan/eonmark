//! Annexation (`docs/design/towns.md`, "Annexation state machine" and "M5a
//! implementation notes"): a complete Town at 0 HP with one enemy owner's
//! infantry or cavalry beside it and no defender near changes hands after
//! `annexation_ds` (60 s = 1200 ticks).
//!
//! Hashed state is one field per Town, [`TownState::hold`] (sub-hash
//! `buildings`), and the Town's border source, which stamps the neutral
//! layer while the timer runs (sub-hash `territory`). Nothing else carries
//! over between steps: the attacker and defender sets and the flips are
//! computed inside step 5d and never stored, so a restore has nothing to
//! rebuild.
//!
//! Step 5d runs after `combat::resolve`, on the post-movement, post-reap
//! positions, and walks every complete Town at 0 HP in `BuildingId` order
//! (none, the usual case, returns at once: a Town is contested only at 0
//! HP, and only a flip restores hit points). For each it makes one pass
//! over the units:
//!
//! - an attacker is a unit of another player whose kind
//!   `annexation_attackers` selects (infantry and cavalry) standing on a
//!   tile within `annexation_adjacent_tiles` (1) of the footprint
//!   (Chebyshev);
//! - a defender is a unit of the Town's owner whose kind
//!   `annexation_defenders` selects (every non-Civilian kind, Mangonels and
//!   Supply Wains included) within `annexation_defender_radius_tiles` (6.0)
//!   of the footprint's centre, centre to centre, inclusive.
//!
//! The Town is eligible when exactly one player owns attackers and there is
//! no defender (`eligible`). Transitions:
//!
//! - `Held` and eligible: the timer starts (`Contested { attacker, ticks: 0
//!   }`), the Town's border source moves to the neutral layer
//!   (`AnnexationStarted`, `TerritoryChanged`).
//! - `Contested` and eligible for the same attacker: `ticks += 1`; at
//!   `Rules::annexation_ticks()` the Town flips, so the flip lands in the
//!   step 1200 steps after the entry step.
//! - `Contested` and not eligible: the timer resets to `Held` and the
//!   source returns to the owner's layer (`AnnexationReset`,
//!   `TerritoryChanged`). A different sole attacker is a reset followed by
//!   a new entry in the same step.
//!
//! A flip (`flip`) transfers the Town and every building and site of the
//! old owner attributed to it, except the kinds with `stays_on_annexation`
//! (the Watchtower, sites included), which stay and are attributed again to
//! the old owner's nearest uncontested Town. Queues are refunded to the old
//! owner like `Cancel`, sites lose what was paid for them, rallies are
//! cleared, a transferred building with its own border projects for its new
//! owner, and the old owner's builders and workers there are idled. The
//! Town returns `Held` with `annexation_town_hp_permille` of its kind's hit
//! points (500 of 2000), keeps its seat flag, and projects for its new
//! owner. The town limit does not apply to a flip. Step 6e
//! (`victory::step`) reads the flips. Nothing draws from the RNG.

use crate::combat::reach_sq;
use crate::construction::{building_rect, centre_fx};
use crate::economy::building_kind;
use crate::fx::FxVec2;
use crate::ids::{BuildingId, PlayerId};
use crate::state::{Building, Construction, Hold, Job, SimEvent, State, TownState, fx_from_x100};
use crate::territory::{Source, SourceOwner};
use crate::town;
use crate::training;
use rules::Rules;
use std::collections::BTreeSet;

/// A running annexation timer as `SimView::annexation` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnexView {
    /// The player whose units are adjacent.
    pub attacker: PlayerId,
    /// Eligible steps since the entry step (0 after it).
    pub ticks: u32,
    /// `Rules::annexation_ticks()`: the Town flips in the step that brings
    /// `ticks` to it, so `total - ticks` more steps flip it.
    pub total: u32,
}

/// A Town that changed hands this step, for step 6e.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Flip {
    /// The Town.
    pub town: BuildingId,
    /// Its old owner.
    pub from: PlayerId,
    /// Its new owner, the attacker.
    pub to: PlayerId,
    /// Whether it is a Seat.
    pub seat: bool,
}

/// Whether `b` is a Town whose annexation timer runs.
pub(crate) fn is_contested(b: &Building) -> bool {
    matches!(
        b.town_state,
        Some(TownState {
            hold: Hold::Contested { .. },
            ..
        })
    )
}

/// The attacker that may annex a Town at 0 HP: the one player in
/// `attackers` when there is exactly one and nobody `defended` it, else
/// `None`. Two or more attacking owners hold the Town as it is.
pub(crate) fn eligible(attackers: &BTreeSet<PlayerId>, defended: bool) -> Option<PlayerId> {
    match (attackers.len(), defended) {
        (1, false) => attackers.first().copied(),
        _ => None,
    }
}

/// Step 5d: advance every Town at 0 HP (see the module docs). Returns the
/// flips, in `BuildingId` order.
pub(crate) fn step(state: &mut State, rules: &Rules, events: &mut Vec<SimEvent>) -> Vec<Flip> {
    let towns: Vec<BuildingId> = state
        .buildings
        .values()
        .filter(|b| {
            b.town_state
                .is_some_and(|ts| b.hp == 0 || ts.hold != Hold::Held)
        })
        .map(|b| b.id)
        .collect();
    if towns.is_empty() {
        return Vec::new();
    }
    let attacking: Vec<bool> = rules
        .units
        .iter()
        .map(|k| rules.annexation_attackers.iter().any(|s| s.selects(k)))
        .collect();
    let defending: Vec<bool> = rules
        .units
        .iter()
        .map(|k| rules.annexation_defenders.iter().any(|s| s.selects(k)))
        .collect();
    let adjacent = u16::try_from(rules.annexation_adjacent_tiles).expect("validated <= 4");
    let radius = reach_sq(fx_from_x100(rules.annexation_defender_radius()));
    let total = rules.annexation_ticks();
    let mut flips = Vec::new();
    for id in towns {
        let b = &state.buildings[&id];
        let owner = b.owner;
        let rect = building_rect(rules, &state.map, b);
        let centre = centre_fx(building_kind(rules, b.kind), b.tile);
        let hold = b.town_state.expect("a Town").hold;
        let mut attackers = BTreeSet::new();
        let mut defended = false;
        if b.hp == 0 {
            for u in state.units.values() {
                let k = usize::from(u.kind.0);
                if u.owner == owner {
                    defended |= defending[k] && u.pos.dist_sq_i64(centre) <= radius;
                } else if attacking[k] && rect.chebyshev(state.map.tile_of(u.pos)) <= adjacent {
                    attackers.insert(u.owner);
                }
            }
        }
        let now = if b.hp == 0 {
            eligible(&attackers, defended)
        } else {
            None
        };
        let ticks = match (hold, now) {
            (Hold::Contested { attacker, ticks }, Some(a)) if a == attacker => {
                ticks.saturating_add(1)
            }
            (Hold::Held, None) => continue,
            (Hold::Contested { .. }, _) => {
                set_hold(state, id, Hold::Held, SourceOwner::Player(owner));
                events.push(SimEvent::AnnexationReset { town: id });
                events.push(SimEvent::TerritoryChanged);
                let Some(a) = now else { continue };
                enter(state, id, a, events);
                0
            }
            (Hold::Held, Some(a)) => {
                enter(state, id, a, events);
                0
            }
        };
        let attacker = now.expect("a running timer has its attacker");
        if ticks >= total {
            flips.push(flip(state, rules, id, attacker, events));
        } else {
            set_hold(
                state,
                id,
                Hold::Contested { attacker, ticks },
                SourceOwner::Neutral,
            );
        }
    }
    flips
}

/// Start `town`'s timer for `attacker`: `Contested { ticks: 0 }` and its
/// border source on the neutral layer.
fn enter(state: &mut State, town: BuildingId, attacker: PlayerId, events: &mut Vec<SimEvent>) {
    set_hold(
        state,
        town,
        Hold::Contested { attacker, ticks: 0 },
        SourceOwner::Neutral,
    );
    events.push(SimEvent::AnnexationStarted { town, attacker });
    events.push(SimEvent::TerritoryChanged);
}

/// Set `town`'s hold and move its border source to `layer` when it is on
/// another one (centre and radius unchanged).
fn set_hold(state: &mut State, town: BuildingId, hold: Hold, layer: SourceOwner) {
    let b = state.buildings.get_mut(&town).expect("a Town");
    let ts = b.town_state.as_mut().expect("a Town");
    ts.hold = hold;
    let mut source = state.territory.sources()[&town];
    if source.owner != layer {
        source.owner = layer;
        state.territory.change_source(town, source);
    }
}

/// Transfer `town` to `to` (see the module docs), in this order: the Town
/// and the buildings and sites of its owner attributed to it, in
/// `BuildingId` order, are split into those that change hands and those
/// whose kind `stays_on_annexation`; each one that changes hands has its
/// queue refunded to the old owner (`TrainingCancelled` per entry), a
/// site's `paid` zeroed with no refund, its rally cleared and its owner
/// set, and a complete one with its own border source (a kind with
/// `border_radius_tiles` that does not stay) has that source moved to
/// `to`'s layer, centre and radius unchanged; every unit of another player
/// whose `Build` or `Gather` job names
/// one of them goes idle; the Town is `Held` again at
/// `annexation_town_hp_permille` of its kind's hit points (floored, in
/// `i64`) and its source moves to `to`'s layer before `town::refresh`
/// re-derives its tier and radius for `to`; each staying building is
/// attributed again with `town::attribute` for the old owner (`None` when
/// it has no uncontested complete Town left), and the old owner's Towns
/// that gained one are refreshed in `BuildingId` order. Emits
/// `TownAnnexed` and `TerritoryChanged`.
fn flip(
    state: &mut State,
    rules: &Rules,
    town: BuildingId,
    to: PlayerId,
    events: &mut Vec<SimEvent>,
) -> Flip {
    let from = state.buildings[&town].owner;
    let mut moving = vec![town];
    let mut staying = Vec::new();
    for b in state.buildings.values() {
        if b.owner == from && b.town == Some(town) {
            if building_kind(rules, b.kind).stays_on_annexation {
                staying.push(b.id);
            } else {
                moving.push(b.id);
            }
        }
    }
    moving.sort_unstable();
    for &id in &moving {
        training::refund_queue(state, rules, id, events);
        let b = state.buildings.get_mut(&id).expect("listed");
        if matches!(b.construction, Construction::Site { .. }) {
            b.paid.iter_mut().for_each(|v| *v = 0);
        }
        b.rally = None;
        b.owner = to;
        // A complete building with its own border (a kind with
        // `border_radius_tiles` that does not stay) projects for its new
        // owner; the Town's source moves below, a site has none.
        if id != town
            && let Some(&source) = state.territory.sources().get(&id)
        {
            let source = Source {
                owner: SourceOwner::Player(to),
                ..source
            };
            state.territory.change_source(id, source);
        }
    }
    for u in state.units.values_mut() {
        if u.owner == to {
            continue;
        }
        let named = match u.job {
            Some(Job::Build { site }) => Some(site),
            Some(Job::Gather { node, .. }) => Some(node),
            None => None,
        };
        if named.is_some_and(|b| moving.binary_search(&b).is_ok()) {
            u.job = None;
        }
    }
    let b = state.buildings.get_mut(&town).expect("the Town");
    let max = i64::from(building_kind(rules, b.kind).hp);
    let hp = max * i64::from(rules.annexation_town_hp_permille) / 1000;
    b.hp = i32::try_from(hp).expect("at most the kind's hit points");
    let ts = b.town_state.as_mut().expect("a Town");
    ts.hold = Hold::Held;
    let seat = ts.seat;
    let mut source = state.territory.sources()[&town];
    source.owner = SourceOwner::Player(to);
    state.territory.change_source(town, source);
    town::refresh(state, rules, town, events);
    let mut regained = BTreeSet::new();
    for id in staying {
        let b = &state.buildings[&id];
        let centre: FxVec2 = centre_fx(building_kind(rules, b.kind), b.tile);
        let to_town = town::attribute(state, rules, from, centre);
        state.buildings.get_mut(&id).expect("listed").town = to_town;
        regained.extend(to_town);
    }
    for t in regained {
        town::refresh(state, rules, t, events);
    }
    events.push(SimEvent::TownAnnexed { town, from, to });
    events.push(SimEvent::TerritoryChanged);
    Flip {
        town,
        from,
        to,
        seat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Town at 0 HP is annexable by exactly one attacking owner with no
    /// defender near: none, two or more owners, or any defender, hold it.
    #[test]
    fn eligible_needs_exactly_one_attacker_owner_and_no_defender() {
        let set = |ps: &[u8]| ps.iter().map(|&p| PlayerId(p)).collect::<BTreeSet<_>>();
        assert_eq!(eligible(&set(&[]), false), None);
        assert_eq!(eligible(&set(&[]), true), None);
        assert_eq!(eligible(&set(&[1]), false), Some(PlayerId(1)));
        assert_eq!(eligible(&set(&[0]), false), Some(PlayerId(0)));
        assert_eq!(eligible(&set(&[1]), true), None);
        assert_eq!(eligible(&set(&[0, 2]), false), None);
        assert_eq!(eligible(&set(&[0, 2]), true), None);
        assert_eq!(eligible(&set(&[0, 1, 2]), false), None);
    }
}
