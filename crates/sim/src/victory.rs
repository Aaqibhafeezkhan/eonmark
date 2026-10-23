//! The match outcome (`docs/design/towns.md`, "Victory and defeat" and "M5a
//! implementation notes"): step 6e decides it once and latches it in
//! `State::outcome` (sub-hash `meta`).
//!
//! Step 6e runs last in the step, while no outcome is set, and checks in
//! this order; the first condition that holds decides:
//!
//! 1. Surrender: when a player has surrendered and exactly one player has
//!    not, that player wins (`Victory { decisive: true }`, reason
//!    `Surrender`); when every player has surrendered it is a `Draw`. With
//!    more than one player left the match goes on.
//! 2. Seat captured: this step's flips of a Seat (step 5d), by distinct new
//!    owners: one gives that player `Victory { decisive: true }`, two or
//!    more a `Draw` (reason `SeatCaptured`).
//! 3. All Towns lost: the old owners of this step's flips that own no
//!    complete Town afterwards (sites do not count). When that leaves one
//!    player who has not surrendered, that player wins
//!    (`Victory { decisive: true }`, `AllTownsLost`); when it leaves none,
//!    a `Draw`. Only a flip can take a player's last Town (a complete Town
//!    is never destroyed), so the check runs at the flip, not every step.
//! 4. Territory: in the step that reaches `Rules::match_cap_ticks()` (the
//!    step at tick 53999 ends at 54000), the player who has not surrendered
//!    with the most owned tiles wins (`Victory { decisive: false }`,
//!    `Territory`); an exact tie for the most is a `Draw`.
//!
//! The result's tick is `Sim::tick()` after the deciding step, and
//! `MatchDecided` is emitted once. The sim keeps stepping afterwards and
//! the result never changes; only the game freezes. Nothing draws from the
//! RNG.

use crate::annexation::Flip;
use crate::ids::PlayerId;
use crate::state::{MatchResult, Outcome, OutcomeReason, SimEvent, State};
use rules::Rules;
use std::collections::BTreeSet;

/// Step 6e: decide the outcome when a condition holds (see the module
/// docs). `tick` is the step's tick; `flips` are step 5d's.
pub(crate) fn step(
    state: &mut State,
    rules: &Rules,
    tick: u32,
    flips: &[Flip],
    events: &mut Vec<SimEvent>,
) {
    if state.outcome.is_some() {
        return;
    }
    let Some((outcome, reason)) = decide(state, rules, tick, flips) else {
        return;
    };
    let result = MatchResult {
        outcome,
        reason,
        tick: tick.checked_add(1).expect("tick fits u32"),
    };
    state.outcome = Some(result);
    events.push(SimEvent::MatchDecided { result });
}

/// The winner when exactly one player is in `left`, a draw when none is,
/// and nothing (the match goes on) when several are.
fn last_one_standing(left: &[PlayerId]) -> Option<Outcome> {
    match left {
        [] => Some(Outcome::Draw),
        [p] => Some(Outcome::Victory {
            player: *p,
            decisive: true,
        }),
        _ => None,
    }
}

/// Which condition, if any, decides the match in this step.
fn decide(
    state: &State,
    rules: &Rules,
    tick: u32,
    flips: &[Flip],
) -> Option<(Outcome, OutcomeReason)> {
    let standing: Vec<PlayerId> = state
        .players
        .iter()
        .filter(|p| !p.surrendered)
        .map(|p| p.id)
        .collect();
    if standing.len() < state.players.len()
        && let Some(outcome) = last_one_standing(&standing)
    {
        return Some((outcome, OutcomeReason::Surrender));
    }
    let capturers: BTreeSet<PlayerId> = flips.iter().filter(|f| f.seat).map(|f| f.to).collect();
    match capturers.len() {
        0 => {}
        1 => {
            let player = *capturers.first().expect("one");
            let outcome = Outcome::Victory {
                player,
                decisive: true,
            };
            return Some((outcome, OutcomeReason::SeatCaptured));
        }
        _ => return Some((Outcome::Draw, OutcomeReason::SeatCaptured)),
    }
    let losers: BTreeSet<PlayerId> = flips
        .iter()
        .map(|f| f.from)
        .filter(|&p| {
            !state
                .buildings
                .values()
                .any(|b| b.owner == p && b.town_state.is_some())
        })
        .collect();
    if !losers.is_empty() {
        let left: Vec<PlayerId> = standing
            .iter()
            .copied()
            .filter(|p| !losers.contains(p))
            .collect();
        if let Some(outcome) = last_one_standing(&left) {
            return Some((outcome, OutcomeReason::AllTownsLost));
        }
    }
    if tick.saturating_add(1) >= rules.match_cap_ticks() {
        let mut best: Vec<PlayerId> = Vec::new();
        let mut most = 0;
        for &p in &standing {
            let n = state.territory.owned_tile_count(p);
            if best.is_empty() || n > most {
                best = vec![p];
                most = n;
            } else if n == most {
                best.push(p);
            }
        }
        let outcome = match best[..] {
            [p] => Outcome::Victory {
                player: p,
                decisive: false,
            },
            _ => Outcome::Draw,
        };
        return Some((outcome, OutcomeReason::Territory));
    }
    None
}
