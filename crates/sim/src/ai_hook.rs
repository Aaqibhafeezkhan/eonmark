//! The hook through which scripted opponents run *inside* [`crate::Sim::step`].
//!
//! Running the AI inside the step means bot-vs-bot replays and future
//! lockstep peers agree on every decision: the bot sees only the hashed
//! state and its own hashed memory ([`AiMemory`], M5b), draws no randomness
//! of its own, and its commands go through the same delay queue as a
//! human's.

use crate::annexation::AnnexView;
use crate::attrition::{self, Exposure, Pulses, SupplyGrid};
use crate::combat::{self, UnitStats};
use crate::command::Command;
use crate::construction::{self, BuildPlan};
use crate::economy::{self, PlayerEconomy, RateInfo};
use crate::fx::FxVec2;
use crate::gather;
use crate::ids::{
    BuildingId, BuildingKindId, PlayerId, TargetId, TechId, Tile, UnitId, UnitKindId,
};
use crate::map::Map;
use crate::pathing::Pathing;
use crate::state::{
    Building, Hold, MatchResult, MatchSetup, Outcome, Player, RejectReason, State, Unit,
};
use crate::tech;
use crate::territory::TerritoryField;
use crate::town;
use crate::training;
use rules::Rules;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The size a bot's memory is designed to stay under, in bytes (M5b). Not
/// enforced by the sim: a bot's tests check it, since the memory is hashed
/// and snapshotted with the state every tick.
pub const AI_MEMORY_GUIDE_BYTES: usize = 4096;

/// A bot's memory between plans (M5b): opaque bytes the bot encodes and
/// decodes itself, stored in [`Player::ai_memory`] and hashed in the
/// `meta` sub-hash, so a snapshot carries it and a restore reproduces the
/// bot's next decision exactly. During [`AiController::think`] each bot
/// gets only its own memory; every bot's memory reads empty in the view.
/// It holds only what cannot be re-derived from the view (wave rosters,
/// timers, latches); everything else is recomputed every plan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiMemory(Vec<u8>);

impl AiMemory {
    /// The stored bytes (empty until the bot first writes).
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    /// Replace the stored bytes.
    pub fn set(&mut self, bytes: Vec<u8>) {
        self.0 = bytes;
    }

    /// Whether nothing is stored.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Read-only view of the sim handed to AI controllers and the presenter.
/// Every economy accessor calls the same function `Sim::step` uses, so a
/// quote or a check here always matches what applying the command does.
/// It is a handful of references, so it is `Copy`.
#[derive(Clone, Copy)]
pub struct SimView<'a> {
    /// Current tick.
    pub tick: u32,
    state: &'a State,
    rules: &'a Rules,
    setup: &'a MatchSetup,
}

impl<'a> SimView<'a> {
    pub(crate) fn new(state: &'a State, rules: &'a Rules, setup: &'a MatchSetup) -> SimView<'a> {
        SimView {
            tick: state.tick,
            state,
            rules,
            setup,
        }
    }

    /// The match setup.
    pub fn setup(&self) -> &'a MatchSetup {
        self.setup
    }

    /// A player's stockpiles and accumulators.
    ///
    /// # Panics
    ///
    /// If the player does not exist.
    pub fn economy(&self, p: PlayerId) -> &'a PlayerEconomy {
        self.state.economy.player(p)
    }

    /// A player's income this tick per resource, `resources.ron` order,
    /// with the player's resolved Yield Cap.
    pub fn income(&self, p: PlayerId) -> Vec<RateInfo> {
        economy::rates(self.state, self.rules, p)
    }

    /// The territory field.
    pub fn territory(&self) -> &'a TerritoryField {
        &self.state.territory
    }

    /// One building or site, if it exists.
    pub fn building(&self, id: BuildingId) -> Option<&'a Building> {
        self.state.buildings.get(&id)
    }

    /// A gathering building's slots: `(worked, taken, slots)`. Taken slots
    /// have a unit assigned (walking or working); worked ones earn income.
    /// `None` when the building does not exist.
    pub fn worked_slots(&self, id: BuildingId) -> Option<(u8, u8, u8)> {
        let b = self.state.buildings.get(&id)?;
        let (worked, taken) = gather::slot_counts(self.state, self.rules, id);
        Some((worked, taken, b.slots))
    }

    /// `p`'s idle workers in id order: units of a kind that works some
    /// building's slots, with no job and no order.
    pub fn idle_workers(&self, p: PlayerId) -> Vec<UnitId> {
        let workers = gather::worker_kinds(self.rules);
        self.state
            .units
            .values()
            .filter(|u| {
                u.owner == p && workers.contains(&u.kind) && u.job.is_none() && u.order.is_none()
            })
            .map(|u| u.id)
            .collect()
    }

    /// `p`'s pop: `(used, cap)`, used counting units alive and queued, the
    /// cap with `p`'s `PopCap` rows.
    pub fn pop(&self, p: PlayerId) -> (u32, u32) {
        (
            training::pop_used(self.state, self.rules, p),
            tech::pop_cap(self.state, self.rules, p),
        )
    }

    /// What one more unit of `kind` costs `p` now (the ramped cost `Train`
    /// would deduct), `resources.ron` order.
    ///
    /// # Panics
    ///
    /// If `kind` is not in the rules.
    pub fn unit_cost(&self, p: PlayerId, kind: UnitKindId) -> Vec<i64> {
        training::unit_cost_for(self.state, self.rules, p, kind)
    }

    /// Whether `Train { building, kind }` from `p` would be accepted now,
    /// and what it would deduct (`TownContested` at a Town whose
    /// annexation timer runs, M5a).
    pub fn check_train(
        &self,
        p: PlayerId,
        building: BuildingId,
        kind: UnitKindId,
    ) -> Result<Vec<i64>, RejectReason> {
        self.check_player(p)?;
        training::validate_train(self.state, self.rules, p, building, kind)
    }

    /// The checks `Sim::apply` makes on the issuing player before any
    /// command's own: it exists (`UnknownPlayer`) and has not surrendered
    /// (`Surrendered`).
    fn check_player(&self, p: PlayerId) -> Result<(), RejectReason> {
        match self.player(p) {
            None => Err(RejectReason::UnknownPlayer),
            Some(player) if player.surrendered => Err(RejectReason::Surrendered),
            Some(_) => Ok(()),
        }
    }

    /// Town-kind buildings `p` owns, sites included, and the town limit
    /// with `p`'s `TownLimit` rows.
    pub fn towns(&self, p: PlayerId) -> (u32, u32) {
        (
            town::town_count(self.state, self.rules, p),
            tech::town_limit(self.state, self.rules, p),
        )
    }

    /// The age `p` has reached (index into `ages.ron`).
    ///
    /// # Panics
    ///
    /// If the player does not exist.
    pub fn age(&self, p: PlayerId) -> u8 {
        self.state.tech.player(p).age
    }

    /// The techs `p` has completed (queued research is in its host's
    /// queue).
    ///
    /// # Panics
    ///
    /// If the player does not exist.
    pub fn researched(&self, p: PlayerId) -> &'a BTreeSet<TechId> {
        &self.state.tech.player(p).researched
    }

    /// How many of `p`'s completed techs count toward the next age's
    /// `techs_required` (M5a): `researched(p)` without the side techs whose
    /// `counts_toward_age` is false (Harrying I and II), the number
    /// `AdvanceAge` compares with the gate.
    ///
    /// # Panics
    ///
    /// If the player does not exist.
    pub fn age_tech_count(&self, p: PlayerId) -> u32 {
        tech::age_tech_count(self.state, self.rules, p)
    }

    /// What researching `tech` costs `p` now and how many ticks it takes
    /// (what `Research` would deduct and queue), `resources.ron` order.
    ///
    /// # Panics
    ///
    /// If `tech` is not in the rules.
    pub fn research_quote(&self, p: PlayerId, tech: TechId) -> (Vec<i64>, u32) {
        tech::research_quote(self.state, self.rules, p, tech)
    }

    /// Whether `Research { building, tech }` from `p` would be accepted
    /// now, and its quote: the cost and the time in ticks.
    pub fn check_research(
        &self,
        p: PlayerId,
        building: BuildingId,
        tech: TechId,
    ) -> Result<(Vec<i64>, u32), RejectReason> {
        self.check_player(p)?;
        tech::validate_research(self.state, self.rules, p, building, tech)
    }

    /// Whether `AdvanceAge { player: p }` from `p` would be accepted now,
    /// and what it would deduct.
    pub fn check_advance_age(&self, p: PlayerId) -> Result<Vec<i64>, RejectReason> {
        self.check_player(p)?;
        tech::validate_advance(self.state, self.rules, p, p)
    }

    /// What a building of `kind` costs `p` now (the ramped cost `Build`
    /// would deduct), `resources.ron` order.
    ///
    /// # Panics
    ///
    /// If `kind` is not in the rules.
    pub fn building_cost(&self, p: PlayerId, kind: BuildingKindId) -> Vec<i64> {
        economy::building_cost_for(self.state, self.rules, p, kind)
    }

    /// Whether `Build { unit, kind, tile }` from `p` would be accepted now,
    /// and what it would deduct (zeros when it resumes an own site).
    pub fn check_build(
        &self,
        p: PlayerId,
        unit: UnitId,
        kind: BuildingKindId,
        tile: Tile,
    ) -> Result<Vec<i64>, RejectReason> {
        self.check_player(p)?;
        let def = self
            .rules
            .map(&self.setup.map)
            .expect("setup map is in the rules");
        match construction::validate_build(self.state, self.rules, def, p, unit, kind, tile)? {
            BuildPlan::Resume { .. } => Ok(vec![0; self.rules.resources.resources.len()]),
            BuildPlan::New { cost, .. } => Ok(cost),
        }
    }

    /// Whether `p` could place a new building of `kind` now, before a
    /// builder and a tile are chosen, and what it would deduct: the checks
    /// of `check_build` that need neither, in its order (`UnknownPlayer`,
    /// `Surrendered`, `UnknownBuildingKind`, `AgeRequired`,
    /// `TownLimitReached`, `CannotAfford`). `check_build` on a tile can
    /// still reject the site.
    pub fn check_build_kind(
        &self,
        p: PlayerId,
        kind: BuildingKindId,
    ) -> Result<Vec<i64>, RejectReason> {
        self.check_player(p)?;
        construction::validate_build_kind(self.state, self.rules, p, kind)
    }

    /// `p`'s combat stats for unit kind `kind` now: hit points with the
    /// `UnitHp` rows and, for a kind that attacks, damage with the
    /// `UnitDamage` rows, range, reach and cooldown (Arms raises hit points
    /// and damage).
    ///
    /// # Panics
    ///
    /// If the player or `kind` does not exist.
    pub fn unit_stats(&self, p: PlayerId, kind: UnitKindId) -> UnitStats {
        combat::unit_stats(self.state, self.rules, p, kind)
    }

    /// A unit's hit points now and its maximum, `(hp, max_hp)`; `None`
    /// when the unit does not exist (or has died).
    pub fn unit_hp(&self, id: UnitId) -> Option<(i32, i32)> {
        let u = self.state.units.get(&id)?;
        let max = combat::unit_stats(self.state, self.rules, u.owner, u.kind).max_hp;
        Some((max - u.damage_taken, max))
    }

    /// A building's or site's hit points now and its maximum, the kind's
    /// `hp` (no Modifier row touches buildings), `(hp, max_hp)`; `None`
    /// when the building does not exist (or was destroyed). A complete
    /// Town that reached 0 stays, at 0.
    pub fn building_hp(&self, id: BuildingId) -> Option<(i32, i32)> {
        let b = self.state.buildings.get(&id)?;
        let def = self
            .rules
            .building_kind(b.kind.0)
            .expect("a building's kind is in the rules");
        let max = i32::try_from(def.hp).expect("data-check bounds building hp");
        Some((b.hp, max))
    }

    /// `p`'s Harrying level (M5a): the `HarryingLevel` rows from base 0,
    /// one per Harrying tech researched, clamped to `0..=8`. Enemy units
    /// on `p`'s land are exposed to attrition from level 1.
    ///
    /// # Panics
    ///
    /// If the player does not exist.
    pub fn harrying_level(&self, p: PlayerId) -> u32 {
        attrition::harrying_level(self.state, self.rules, p)
    }

    /// The radius within which a Supply Wain covers its owner's units, in
    /// tiles (`supply_radius_tiles`, tile to tile, inclusive).
    pub fn supply_radius_tiles(&self) -> u32 {
        self.rules.supply_radius_tiles
    }

    /// The tiles `p`'s Supply Wains (the kinds that `supplies`) stand on,
    /// in `UnitId` order: the centres of `p`'s supply discs.
    pub fn supply_wains(&self, p: PlayerId) -> Vec<Tile> {
        SupplyGrid::new(self.state, self.rules).wains(p).to_vec()
    }

    /// Whether one of `p`'s Supply Wains covers `tile` now, as step 5b'
    /// decides it (`dx² + dy² <= supply_radius_tiles²`).
    pub fn supplied(&self, p: PlayerId, tile: Tile) -> bool {
        SupplyGrid::new(self.state, self.rules).covers(p, tile)
    }

    /// What a pulse on `owner`'s land does to `intruder`'s units now:
    /// `(interval_ticks, hp)` from `owner`'s Harrying level and both ages
    /// (the step's own function), or `None` while `owner` has no Harrying
    /// level.
    ///
    /// # Panics
    ///
    /// If either player does not exist.
    pub fn attrition_pulse(&self, owner: PlayerId, intruder: PlayerId) -> Option<(u32, i32)> {
        attrition::pulse(self.state, self.rules, owner, intruder)
    }

    /// Whether unit `id` is exposed to attrition where it stands now, and
    /// to whose borders, how often and how hard: what step 5b' would apply
    /// if the unit did not move. `None` when it is not exposed (immune,
    /// on neutral or own land, the owner has no Harrying level, or an own
    /// Supply Wain covers it) or does not exist. Builds the step's tables
    /// on every call, so it is meant for single units; the unit's
    /// `exposed_ticks` says whether it is bleeding.
    pub fn attrition(&self, id: UnitId) -> Option<Exposure> {
        let u = self.state.units.get(&id)?;
        Pulses::new(self.state, self.rules)?.exposure(&self.state.map, &self.state.territory, u)
    }

    /// Town `town`'s running annexation timer (M5a): who annexes it, how
    /// many eligible steps have passed since the entry step and how many
    /// flip it. `None` when the building is not a Town, does not exist, or
    /// is held.
    pub fn annexation(&self, town: BuildingId) -> Option<AnnexView> {
        match self.state.buildings.get(&town)?.town_state?.hold {
            Hold::Held => None,
            Hold::Contested { attacker, ticks } => Some(AnnexView {
                attacker,
                ticks,
                total: self.rules.annexation_ticks(),
            }),
        }
    }

    /// How the match ended, once step 6e decided it (M5a); `None` while it
    /// runs.
    pub fn outcome(&self) -> Option<Outcome> {
        self.state.outcome.map(|r| r.outcome)
    }

    /// The match result (outcome, reason and deciding tick), once decided.
    pub fn result(&self) -> Option<MatchResult> {
        self.state.outcome
    }

    /// Whether `Attack { units, target }` from `p` would be accepted now,
    /// and which units would attack (`p`'s listed units of a kind that
    /// attacks, in id order).
    pub fn check_attack(
        &self,
        p: PlayerId,
        units: &[UnitId],
        target: TargetId,
    ) -> Result<Vec<UnitId>, RejectReason> {
        self.check_player(p)?;
        combat::validate_attack(self.state, self.rules, p, units, target)
    }

    /// Whether `Gather { units, node }` from `p` would be accepted now, and
    /// which of the units would take a slot (in id order; empty when every
    /// valid unit already works the node, which is accepted and changes
    /// nothing). M5b.
    pub fn check_gather(
        &self,
        p: PlayerId,
        units: &[UnitId],
        node: BuildingId,
    ) -> Result<Vec<UnitId>, RejectReason> {
        self.check_player(p)?;
        gather::validate_gather(self.state, self.rules, p, units, node)
            .map(|plan| plan.into_iter().map(|(unit, _)| unit).collect())
    }

    /// Whether `SetRally { building, target }` from `p` would be accepted
    /// now (M5b). Any point is accepted for an own building.
    pub fn check_rally(
        &self,
        p: PlayerId,
        building: BuildingId,
        _target: FxVec2,
    ) -> Result<(), RejectReason> {
        self.check_player(p)?;
        training::validate_rally(self.state, p, building)
    }

    /// `p`'s bot memory bytes (M5b), for the dev panel and the bot
    /// harness; empty for a human slot, a bot that never wrote and an
    /// unknown player. During [`AiController::think`] it is empty for
    /// every bot: each bot receives its own memory as an argument.
    pub fn ai_memory(&self, p: PlayerId) -> &'a [u8] {
        self.player(p).map_or(&[], |pl| pl.ai_memory.bytes())
    }

    /// The rules this match runs under.
    pub fn rules(&self) -> &'a Rules {
        self.rules
    }

    /// Match seed (informational; the bot must not reseed anything from it).
    pub fn seed(&self) -> u64 {
        self.state.seed
    }

    /// All players in slot order.
    pub fn players(&self) -> &'a [Player] {
        &self.state.players
    }

    /// One player, if the slot exists.
    pub fn player(&self, id: PlayerId) -> Option<&'a Player> {
        self.state.players.iter().find(|p| p.id == id)
    }

    /// Units in id order.
    pub fn units(&self) -> impl Iterator<Item = &'a Unit> + 'a {
        self.state.units.values()
    }

    /// One unit, if it exists.
    pub fn unit(&self, id: UnitId) -> Option<&'a Unit> {
        self.state.units.get(&id)
    }

    /// Number of units.
    pub fn unit_count(&self) -> usize {
        self.state.units.len()
    }

    /// The tile map (cost grid, generation, components).
    pub fn map(&self) -> &'a Map {
        &self.state.map
    }

    /// The pathing subsystem (queue, active search, cache statistics).
    pub fn pathing(&self) -> &'a Pathing {
        &self.state.pathing
    }

    /// Buildings in id order.
    pub fn buildings(&self) -> impl Iterator<Item = &'a Building> + 'a {
        self.state.buildings.values()
    }

    /// Commands applied so far (M0 placeholder statistic).
    pub fn commands_applied(&self) -> u64 {
        self.state.commands_applied
    }
}

/// A scripted opponent that runs *inside* [`crate::Sim::step`] so bot-vs-bot
/// replays and future lockstep peers agree on every decision.
///
/// Implementations must be deterministic functions of the view and of the
/// player's own [`AiMemory`] (and, later, of randomness handed to them by
/// the sim): any other state a controller keeps must be a pure function of
/// immutable inputs (the rules, the setup), so a controller rebuilt
/// mid-match, or a sim restored from a snapshot, issues the same commands.
/// `Send` is required so matches can run on worker threads in
/// `sim-cli play-bots`.
pub trait AiController: Send {
    /// Produce this tick's commands for `player` from a read-only view and
    /// the player's own memory, which the bot may rewrite (it is hashed in
    /// `meta`). The sim takes every bot's memory out of the state before it
    /// builds the view, so `view.players()` shows no bot's memory. The sim
    /// stamps the commands with the player's next sequence numbers and
    /// delays them by `cmd_delay_ticks` like any other command.
    fn think(
        &mut self,
        player: PlayerId,
        view: &SimView<'_>,
        memory: &mut AiMemory,
    ) -> Vec<Command>;
}

/// A controller that never acts and never writes memory (M5b): for new
/// code that needs a controller for a setup without AI slots, or one whose
/// AI slots should stay idle. `ai::Passive` is the same bot as reached
/// through a header's `passive` seat.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoAi;

impl AiController for NoAi {
    fn think(
        &mut self,
        _player: PlayerId,
        _view: &SimView<'_>,
        _memory: &mut AiMemory,
    ) -> Vec<Command> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::command::{Command, PlayerCommand};
    use crate::fx::FxVec2;
    use crate::ids::{BuildingId, BuildingKindId, PlayerId, Tile, UnitId, UnitKindId};
    use crate::state::{MatchSetup, Sim};
    use rules::Rules;
    use std::path::Path;

    use super::NoAi;

    /// `building_hp` reads the building's hit points and its kind's `hp`:
    /// full on placement, a site included, lower once a guard chips at it,
    /// and `None` for an id that names no building.
    #[test]
    fn building_hp_is_hp_and_the_kinds_maximum() {
        let rules = Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap();
        let kind = |id: &str| BuildingKindId(rules.building_kind_by_id(id).unwrap().0);
        let (tower, croft) = (kind("watchtower"), kind("croft"));
        let skirmisher = UnitKindId(
            u16::try_from(
                rules
                    .units
                    .iter()
                    .position(|u| u.id == "skirmisher")
                    .unwrap(),
            )
            .unwrap(),
        );
        let place = |seq, owner: u8, kind, x, y, complete| {
            PlayerCommand::new(
                PlayerId(owner),
                seq,
                Command::DebugPlaceBuilding {
                    owner: PlayerId(owner),
                    kind,
                    tile: Tile::new(x, y),
                    complete,
                },
            )
        };
        let setup = MatchSetup::scenario(&rules, 1);
        let delay = setup.cmd_delay;
        let mut sim = Sim::new(setup, rules.clone(), Box::new(NoAi));
        sim.step(&[
            place(0, 1, tower, 20, 20, true),
            place(1, 1, croft, 40, 40, false),
            // A guard beside the tower: with no enemy unit in reach it
            // acquires the tower and chips at it (and the tower shoots back).
            PlayerCommand::new(
                PlayerId(0),
                0,
                Command::DebugSpawn {
                    owner: PlayerId(0),
                    kind: skirmisher,
                    at: FxVec2::from_ints(22, 20),
                    count: 1,
                },
            ),
        ]);
        for _ in 0..delay {
            sim.step(&[]);
        }
        let view = sim.view();
        let max = |k: BuildingKindId| i32::try_from(rules.building_kind(k.0).unwrap().hp).unwrap();
        assert_eq!(
            view.building_hp(BuildingId(1)),
            Some((max(tower), max(tower)))
        );
        assert_eq!(
            view.building_hp(BuildingId(2)),
            Some((max(croft), max(croft)))
        );
        assert_eq!(view.building_hp(BuildingId(3)), None);
        for _ in 0..60 {
            sim.step(&[]);
        }
        let view = sim.view();
        let (hp, cap) = view.building_hp(BuildingId(1)).unwrap();
        assert_eq!(cap, max(tower));
        assert_eq!(hp, view.building(BuildingId(1)).unwrap().hp);
        assert!(hp < cap, "the guard hit the tower: {hp} / {cap}");
    }

    /// M5b: `check_gather` and `check_rally` answer as `Gather` and
    /// `SetRally` would be applied: who takes a slot, and every rejection
    /// the issuing player's checks or the validator make.
    #[test]
    fn check_gather_and_check_rally_answer_as_the_commands_apply() {
        use crate::state::{RejectReason, SimEvent, StartKind};
        let rules = Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap();
        let croft = BuildingKindId(rules.building_kind_by_id("croft").unwrap().0);
        let mut setup = MatchSetup::scenario(&rules, 3);
        setup.start = StartKind::Standard;
        let delay = setup.cmd_delay;
        let mut sim = Sim::new(setup, rules, Box::new(NoAi));
        // The Seats are BuildingId(1) and (2); P0's Yeomen UnitId(1..=5).
        sim.step(&[PlayerCommand::new(
            PlayerId(0),
            0,
            Command::DebugPlaceBuilding {
                owner: PlayerId(0),
                kind: croft,
                tile: Tile::new(28, 60),
                complete: true,
            },
        )]);
        for _ in 0..delay {
            sim.step(&[]);
        }
        let node = BuildingId(3);
        let p0 = PlayerId(0);
        let units = [UnitId(1), UnitId(2), UnitId(6)];
        let view = sim.view();
        assert_eq!(view.building(node).unwrap().kind, croft);
        let slots = view.building(node).unwrap().slots;
        assert_eq!(slots, 1, "a Croft has one Grain slot");
        assert_eq!(view.check_gather(p0, &units, node), Ok(vec![UnitId(1)]));
        assert_eq!(
            view.check_gather(p0, &[UnitId(6)], node),
            Err(RejectReason::NoValidUnits),
            "P1's Yeoman"
        );
        assert_eq!(
            view.check_gather(p0, &units, BuildingId(2)),
            Err(RejectReason::UnknownBuilding),
            "P1's Seat"
        );
        assert_eq!(
            view.check_gather(PlayerId(7), &units, node),
            Err(RejectReason::UnknownPlayer)
        );
        let target = FxVec2::from_ints(30, 64);
        assert_eq!(view.check_rally(p0, BuildingId(1), target), Ok(()));
        assert_eq!(
            view.check_rally(p0, BuildingId(2), target),
            Err(RejectReason::UnknownBuilding)
        );
        // Applying them agrees: the Gather takes the slot for Yeoman 1,
        // the SetRally on P1's Seat is rejected for the same reason.
        sim.drain_events();
        sim.step(&[
            PlayerCommand::new(
                p0,
                1,
                Command::Gather {
                    units: units.to_vec(),
                    node,
                },
            ),
            PlayerCommand::new(
                p0,
                2,
                Command::SetRally {
                    building: BuildingId(2),
                    target,
                },
            ),
        ]);
        for _ in 0..delay {
            sim.step(&[]);
        }
        let rejected: Vec<_> = sim
            .drain_events()
            .into_iter()
            .filter_map(|e| match e {
                SimEvent::CommandRejected { seq, reason, .. } => Some((seq, reason)),
                _ => None,
            })
            .collect();
        assert_eq!(rejected, [(2, RejectReason::UnknownBuilding)]);
        let view = sim.view();
        assert!(matches!(
            view.unit(UnitId(1)).unwrap().job,
            Some(crate::state::Job::Gather { node: n, .. }) if n == node
        ));
        assert_eq!(
            view.check_gather(p0, &[UnitId(1)], node),
            Ok(vec![]),
            "already on it"
        );
        assert_eq!(
            view.check_gather(p0, &[UnitId(2)], node),
            Err(RejectReason::NoFreeSlot)
        );
        // A surrendered player is refused before any check.
        sim.step(&[PlayerCommand::new(p0, 3, Command::Surrender)]);
        for _ in 0..delay {
            sim.step(&[]);
        }
        assert_eq!(
            sim.view().check_rally(p0, BuildingId(1), target),
            Err(RejectReason::Surrendered)
        );
    }
}
