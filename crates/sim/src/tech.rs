//! Research, ages and the player stats that Modifier rows change
//! (`docs/design/tech-and-ages.md`).
//!
//! Hashed state is only [`TechState`] (sub-hash `tech`): per player, the
//! age reached (an index into `ages.ron`) and the set of completed techs.
//! A tech being researched is an entry in its host's queue
//! ([`QueueItem::Research`], hashed in `buildings`) until step 6b
//! completes it.
//!
//! Every stat a Modifier row may change is resolved when it is needed and
//! never stored, so there is nothing to keep in sync: `stat` collects
//! the faction's rows (every player uses `factions[0]` until a match setup
//! chooses one in M9), then each researched tech's rows in `TechId` order
//! and file order, keeps the rows of the one stat and passes them to
//! [`rules::modifiers::resolve`] (the last `Set`, then every `Add`, then
//! each `Mul` floored). The wrappers clamp the result to the stat's range.
//! Unit hit points and damage (`unit_max_hp`, `unit_damage`) take the rows
//! whose `UnitHp` or `UnitDamage` selector reaches the kind; combat reads
//! them every tick, so Arms raises living units too.
//!
//! `Research` cost and time are fixed when the entry is queued (the Letters
//! rows researched by then apply; later ones speed only later research),
//! and the cost is deducted then; `Cancel` refunds exactly what the entry
//! paid. `AdvanceAge` is instant and moves one age per command. Completing
//! a tech or advancing an age re-derives every own complete Town
//! (`town::refresh_all`): Statecraft pushes the borders and the age may
//! open tier 2. Validation is pure, so a rejected command changes nothing
//! and draws no randomness; `SimView` calls the same functions.

use crate::economy::{self, building_kind, first_short};
use crate::ids::{BuildingId, BuildingKindId, PlayerId, TechId, UnitKindId};
use crate::state::{Construction, QueueEntry, QueueItem, RejectReason, SimEvent, State};
use crate::town;
use rules::modifiers::{self, StatPath};
use rules::{MAX_BORDER_RADIUS_TILES, Rules, UnitKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The faction every player uses until the match setup chooses one (M9).
pub const FACTION: usize = 0;

/// Every player's age and completed techs. Sub-hash `tech`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TechState {
    /// Indexed by `PlayerId.0`.
    pub players: Vec<PlayerTech>,
}

/// One player's age and completed techs.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PlayerTech {
    /// Index into `ages.ron`; every player starts in age 0.
    pub age: u8,
    /// Completed techs. Queued research is not here: it is an entry in its
    /// host building's queue until it completes.
    pub researched: BTreeSet<TechId>,
}

impl TechState {
    /// `players` players in the first age with nothing researched.
    pub fn new(players: usize) -> TechState {
        TechState {
            players: vec![PlayerTech::default(); players],
        }
    }

    /// One player's age and techs.
    ///
    /// # Panics
    ///
    /// If the player does not exist (`Sim::new` checks ids are `0..n`).
    pub fn player(&self, p: PlayerId) -> &PlayerTech {
        &self.players[usize::from(p.0)]
    }

    pub(crate) fn player_mut(&mut self, p: PlayerId) -> &mut PlayerTech {
        &mut self.players[usize::from(p.0)]
    }
}

/// Resolve stat `target` for `p` from `base`: the faction's rows, then
/// each researched tech's in `TechId` order and file order, filtered to
/// `target`, through [`modifiers::resolve`]. Unclamped; the wrappers below
/// clamp.
pub(crate) fn stat(state: &State, rules: &Rules, p: PlayerId, target: &StatPath, base: i64) -> i64 {
    stat_after(rules, &state.tech.player(p).researched, target, base)
}

/// [`stat`] for a player who has completed exactly `researched`.
fn stat_after(rules: &Rules, researched: &BTreeSet<TechId>, target: &StatPath, base: i64) -> i64 {
    modifiers::resolve(
        base,
        rules
            .modifier_rows(FACTION, researched.iter().map(|t| t.0))
            .filter(|m| m.target == *target),
    )
}

/// A non-negative resolved value as `u32`.
fn non_negative(v: i64) -> u32 {
    u32::try_from(v.max(0)).expect("data-check bounds every stat within u32")
}

/// `p`'s Yield Cap per capped resource per 30 s: `yield_cap_base` with the
/// `YieldCap` rows (Trade), at least 0.
pub(crate) fn yield_cap(state: &State, rules: &Rules, p: PlayerId) -> u32 {
    let base = i64::from(rules.yield_cap_base);
    non_negative(stat(state, rules, p, &StatPath::YieldCap, base))
}

/// `p`'s population cap: `pop_cap_base` with the `PopCap` rows (Arms), at
/// least 0.
pub(crate) fn pop_cap(state: &State, rules: &Rules, p: PlayerId) -> u32 {
    let base = i64::from(rules.pop_cap_base);
    non_negative(stat(state, rules, p, &StatPath::PopCap, base))
}

/// How many Towns `p` may own, sites and the Seat included:
/// `town_limit_base` with the `TownLimit` rows (Statecraft), at least 0.
pub(crate) fn town_limit(state: &State, rules: &Rules, p: PlayerId) -> u32 {
    let base = i64::from(rules.town_limit_base);
    non_negative(stat(state, rules, p, &StatPath::TownLimit, base))
}

/// The border radius of `p`'s Towns at `tier`: the tier's base radius with
/// the `TownBorderRadius` rows (Statecraft), clamped to
/// `1..=MAX_BORDER_RADIUS_TILES` (data-check keeps the worst case within).
pub(crate) fn town_radius(state: &State, rules: &Rules, p: PlayerId, tier: u8) -> u32 {
    let base = i64::from(rules.town_radius_base(tier));
    let v = stat(state, rules, p, &StatPath::TownBorderRadius, base);
    u32::try_from(v.clamp(1, i64::from(MAX_BORDER_RADIUS_TILES))).expect("clamped")
}

/// Resolve a unit stat of `kind` for `p` from `base`, in the order of
/// [`stat`]: the rows whose target is `UnitHp(sel)` (when `hp`) or
/// `UnitDamage(sel)` (otherwise) with a selector that reaches the kind.
/// Unclamped; the wrappers below clamp.
fn unit_stat(
    state: &State,
    rules: &Rules,
    p: PlayerId,
    kind: &UnitKind,
    hp: bool,
    base: u32,
) -> i64 {
    let researched = state.tech.player(p).researched.iter().map(|t| t.0);
    modifiers::resolve(
        i64::from(base),
        rules.modifier_rows(FACTION, researched).filter(|m| {
            let stat = if hp {
                matches!(m.target, StatPath::UnitHp(_))
            } else {
                matches!(m.target, StatPath::UnitDamage(_))
            };
            stat && m.target.matches(kind)
        }),
    )
}

/// The hit points of `p`'s units of `kind`: the kind's `hp` with the
/// `UnitHp` rows (Arms), at least 1. Resolved every time it is read, so a
/// tech completed mid-fight raises the hit points of living units too.
///
/// # Panics
///
/// If `kind` is not in the rules.
pub(crate) fn unit_max_hp(state: &State, rules: &Rules, p: PlayerId, kind: UnitKindId) -> i32 {
    let def = rules.unit_kind(kind.0).expect("validated unit kind");
    let v = unit_stat(state, rules, p, def, true, def.hp);
    i32::try_from(v.max(1)).expect("data-check bounds unit hp within MAX_HP")
}

/// The damage per hit of `p`'s units of `kind`, before counters and
/// variance: the kind's attack `damage` with the `UnitDamage` rows (Arms),
/// at least 0; `None` for a kind without an attack.
///
/// # Panics
///
/// If `kind` is not in the rules.
pub(crate) fn unit_damage(
    state: &State,
    rules: &Rules,
    p: PlayerId,
    kind: UnitKindId,
) -> Option<i32> {
    let def = rules.unit_kind(kind.0).expect("validated unit kind");
    let attack = def.attack.as_ref()?;
    let v = unit_stat(state, rules, p, def, false, attack.damage);
    Some(i32::try_from(v.max(0)).expect("data-check bounds unit damage within MAX_HP"))
}

/// What researching `tech` costs `p` now and how many ticks it takes: each
/// resource listed in the tech's cost with the `ResearchCost` rows
/// (Letters), at least 0 (unlisted resources cost 0), and its
/// `research_time_ds` with the `ResearchTime` rows, converted once with
/// `Rules::ticks_from_ds`, at least 1 tick.
///
/// # Panics
///
/// If `tech` is not in the rules.
pub(crate) fn research_quote(
    state: &State,
    rules: &Rules,
    p: PlayerId,
    tech: TechId,
) -> (Vec<i64>, u32) {
    research_quote_after(rules, &state.tech.player(p).researched, tech)
}

/// [`research_quote`] for a player who has completed exactly `researched`:
/// what a scripted scenario quotes ahead of time (the M4b `enemy_outpost`
/// grants and beats) without a running sim.
///
/// # Panics
///
/// If `tech` is not in the rules.
pub(crate) fn research_quote_after(
    rules: &Rules,
    researched: &BTreeSet<TechId>,
    tech: TechId,
) -> (Vec<i64>, u32) {
    let t = rules.tech(tech.0).expect("validated tech");
    let mut cost = vec![0i64; rules.resources.resources.len()];
    for (id, &amount) in &t.cost {
        let r = rules.resource_index(id).expect("validated resource id");
        let base = i64::from(amount);
        cost[r] = stat_after(rules, researched, &StatPath::ResearchCost, base).max(0);
    }
    let base = i64::from(t.research_time_ds);
    let ds = stat_after(rules, researched, &StatPath::ResearchTime, base).max(0);
    let ds = u32::try_from(ds).expect("data-check bounds research times");
    (cost, rules.ticks_from_ds(ds).max(1))
}

/// Whether `tech` is queued at one of `p`'s buildings.
pub(crate) fn in_progress(state: &State, p: PlayerId, tech: TechId) -> bool {
    state
        .buildings
        .values()
        .filter(|b| b.owner == p)
        .flat_map(|b| &b.queue)
        .any(|e| matches!(e.item, QueueItem::Research { tech: t, .. } if t == tech))
}

/// Whether `p` has reached the age that unlocks unit kind `kind`.
pub(crate) fn age_allows_unit(state: &State, rules: &Rules, p: PlayerId, kind: UnitKindId) -> bool {
    state.tech.player(p).age >= rules.age_of_unit(kind.0)
}

/// Whether `p` has reached the age that unlocks building kind `kind`.
pub(crate) fn age_allows_building(
    state: &State,
    rules: &Rules,
    p: PlayerId,
    kind: BuildingKindId,
) -> bool {
    state.tech.player(p).age >= rules.age_of_building(kind.0)
}

/// Whether `p` has researched the tech unit kind `kind` names in
/// `requires_tech` (true when it names none).
pub(crate) fn has_required_tech(
    state: &State,
    rules: &Rules,
    p: PlayerId,
    kind: UnitKindId,
) -> bool {
    let def = rules.unit_kind(kind.0).expect("validated unit kind");
    def.requires_tech.as_ref().is_none_or(|id| {
        let (t, _) = rules.tech_by_id(id).expect("validated requires_tech");
        state.tech.player(p).researched.contains(&TechId(t))
    })
}

/// Validate `Research { building, tech }` for `p` without changing
/// anything. Checks, first failure wins:
///
/// 1. the building is `p`'s (`UnknownBuilding`)
/// 2. and complete (`UnderConstruction`);
/// 3. the tech exists (`UnknownTech`);
/// 4. the building's kind is the tech's `host` (`CannotResearchHere`);
/// 5. the level below it in its line is researched (`PrerequisiteMissing`);
/// 6. `p` has reached the age that unlocks it (`AgeRequired`);
/// 7. it is not researched (`AlreadyResearched`)
/// 8. nor queued at any of `p`'s buildings (`ResearchInProgress`);
/// 9. the queue has room (`QueueFull`, `train_queue_max`);
/// 10. `p` can pay the quote (`CannotAfford`, first short resource).
///
/// Returns the quote: the cost and the time in ticks.
pub(crate) fn validate_research(
    state: &State,
    rules: &Rules,
    p: PlayerId,
    building: BuildingId,
    tech: TechId,
) -> Result<(Vec<i64>, u32), RejectReason> {
    let b = state
        .buildings
        .get(&building)
        .filter(|b| b.owner == p)
        .ok_or(RejectReason::UnknownBuilding)?;
    if b.construction != Construction::Complete {
        return Err(RejectReason::UnderConstruction);
    }
    let t = rules.tech(tech.0).ok_or(RejectReason::UnknownTech)?;
    if building_kind(rules, b.kind).id != t.host {
        return Err(RejectReason::CannotResearchHere);
    }
    let mine = state.tech.player(p);
    if let Some(prev) = rules.tech_predecessor(tech.0)
        && !mine.researched.contains(&TechId(prev))
    {
        return Err(RejectReason::PrerequisiteMissing);
    }
    if mine.age < rules.age_of_tech(tech.0) {
        return Err(RejectReason::AgeRequired);
    }
    if mine.researched.contains(&tech) {
        return Err(RejectReason::AlreadyResearched);
    }
    if in_progress(state, p, tech) {
        return Err(RejectReason::ResearchInProgress);
    }
    let max = usize::try_from(rules.train_queue_max).expect("validated <= 255");
    if b.queue.len() >= max {
        return Err(RejectReason::QueueFull);
    }
    let (cost, ticks) = research_quote(state, rules, p, tech);
    if let Some(resource) = first_short(&state.economy.player(p).stock, &cost) {
        return Err(RejectReason::CannotAfford { resource });
    }
    Ok((cost, ticks))
}

/// Apply a validated `Research`: deduct the quote and queue the entry with
/// its time fixed.
pub(crate) fn enqueue_research(
    state: &mut State,
    p: PlayerId,
    building: BuildingId,
    tech: TechId,
    cost: Vec<i64>,
    ticks: u32,
) {
    economy::deduct(&mut state.economy.player_mut(p).stock, &cost);
    state
        .buildings
        .get_mut(&building)
        .expect("validated building")
        .queue
        .push(QueueEntry {
            item: QueueItem::Research { tech, ticks },
            paid: cost,
            progress: 0,
        });
}

/// Validate `AdvanceAge { player: named }` from `p` without changing
/// anything. Checks, first failure wins: `named` is `p`
/// (`PlayerMismatch`); `p` is not in the last age (`FinalAge`); `p` has
/// completed the next age's `techs_required` techs, queued ones not
/// counting (`NotEnoughTechs`); `p` can pay its cost (`CannotAfford`,
/// first short resource). Returns the cost.
pub(crate) fn validate_advance(
    state: &State,
    rules: &Rules,
    p: PlayerId,
    named: PlayerId,
) -> Result<Vec<i64>, RejectReason> {
    if named != p {
        return Err(RejectReason::PlayerMismatch);
    }
    let mine = state.tech.player(p);
    let next = rules
        .ages
        .get(usize::from(mine.age) + 1)
        .ok_or(RejectReason::FinalAge)?;
    let done = u32::try_from(mine.researched.len()).expect("at most u16::MAX techs");
    if done < next.techs_required {
        return Err(RejectReason::NotEnoughTechs);
    }
    let cost = rules.amounts(&next.cost);
    if let Some(resource) = first_short(&state.economy.player(p).stock, &cost) {
        return Err(RejectReason::CannotAfford { resource });
    }
    Ok(cost)
}

/// Apply a validated `AdvanceAge`: deduct, enter the next age at once,
/// emit `AgeAdvanced`, then re-derive every own complete Town (the age may
/// open tier 2).
pub(crate) fn advance(
    state: &mut State,
    rules: &Rules,
    p: PlayerId,
    cost: &[i64],
    events: &mut Vec<SimEvent>,
) {
    economy::deduct(&mut state.economy.player_mut(p).stock, cost);
    let mine = state.tech.player_mut(p);
    mine.age += 1;
    events.push(SimEvent::AgeAdvanced {
        player: p,
        age: mine.age,
    });
    town::refresh_all(state, rules, p, events);
}

/// Validate `DebugResearch { owner, tech }` once the debug gate and the
/// owner have passed: the tech exists (`UnknownTech`), is not researched
/// (`AlreadyResearched`) and is not queued at any of the owner's buildings
/// (`ResearchInProgress`), so a tech never completes twice. Host, cost,
/// time, prerequisite and age are skipped.
pub(crate) fn validate_debug_research(
    state: &State,
    rules: &Rules,
    owner: PlayerId,
    tech: TechId,
) -> Result<(), RejectReason> {
    rules.tech(tech.0).ok_or(RejectReason::UnknownTech)?;
    if state.tech.player(owner).researched.contains(&tech) {
        return Err(RejectReason::AlreadyResearched);
    }
    if in_progress(state, owner, tech) {
        return Err(RejectReason::ResearchInProgress);
    }
    Ok(())
}

/// `p` completes `tech`: its rows apply from now on. Emits
/// `ResearchCompleted`, then re-derives every own complete Town
/// (Statecraft pushes the borders). Step 6b calls it when a research entry
/// finishes; `DebugResearch` calls it directly.
pub(crate) fn complete(
    state: &mut State,
    rules: &Rules,
    p: PlayerId,
    tech: TechId,
    events: &mut Vec<SimEvent>,
) {
    state.tech.player_mut(p).researched.insert(tech);
    events.push(SimEvent::ResearchCompleted { player: p, tech });
    town::refresh_all(state, rules, p, events);
}
