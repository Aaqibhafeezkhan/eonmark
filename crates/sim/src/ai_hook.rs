//! The hook through which scripted opponents run *inside* [`crate::Sim::step`].
//!
//! Running the AI inside the step means bot-vs-bot replays and future
//! lockstep peers agree on every decision: the bot sees only the hashed
//! state, draws no randomness of its own, and its commands go through the
//! same delay queue as a human's.

use crate::combat::{self, UnitStats};
use crate::command::Command;
use crate::construction::{self, BuildPlan};
use crate::economy::{self, PlayerEconomy, RateInfo};
use crate::gather;
use crate::ids::{
    BuildingId, BuildingKindId, PlayerId, TargetId, TechId, Tile, UnitId, UnitKindId,
};
use crate::map::Map;
use crate::pathing::Pathing;
use crate::state::{Building, MatchSetup, Player, RejectReason, State, Unit};
use crate::tech;
use crate::territory::TerritoryField;
use crate::town;
use crate::training;
use rules::Rules;
use std::collections::BTreeSet;

/// Read-only view of the sim handed to AI controllers and the presenter.
/// Every economy accessor calls the same function `Sim::step` uses, so a
/// quote or a check here always matches what applying the command does.
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
    /// and what it would deduct.
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
/// Implementations must be deterministic functions of the view (and, later,
/// of randomness handed to them by the sim). `Send` is required so matches
/// can run on worker threads in `sim-cli play-bots`.
pub trait AiController: Send {
    /// Produce this tick's commands for `player` from a read-only view. The
    /// sim stamps them with the player's next sequence numbers and delays
    /// them by `cmd_delay_ticks` like any other command.
    fn think(&mut self, player: PlayerId, view: &SimView<'_>) -> Vec<Command>;
}

#[cfg(test)]
mod tests {
    use crate::command::{Command, PlayerCommand};
    use crate::fx::FxVec2;
    use crate::ids::{BuildingId, BuildingKindId, PlayerId, Tile, UnitKindId};
    use crate::state::{MatchSetup, Sim};
    use rules::Rules;
    use std::path::Path;

    struct NoAi;

    impl super::AiController for NoAi {
        fn think(&mut self, _player: PlayerId, _view: &super::SimView<'_>) -> Vec<Command> {
            Vec::new()
        }
    }

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
}
