//! [`MatchStats`] (M6): what a match is summed up by, for the play-bots
//! CSV, `sim-cli match-log` and the game-over screen.
//!
//! Engine-free, unhashed and not part of the sim's state: the stats are a
//! pure function of the view after every step and the events that step
//! emitted, so the harness, the game (`SimHandle::drain_events`) and
//! `match-log` (a replay's re-simulation) all derive the same numbers. The
//! sim never reads them. Rates, cap time and army size are sampled once a
//! second of match time (`tick % tick_rate_hz == 0`); everything else is
//! read from events, or from the view on every observed tick (Town hit
//! points, the first unit on enemy land), so observing a step twice at the
//! same tick changes nothing.

use crate::ai_hook::SimView;
use crate::ids::{BuildingId, BuildingKindId, PlayerId, TargetId, UnitKindId};
use crate::state::{Construction, MatchSetup, SimEvent};
use rules::{Rules, UnitClass};
use std::collections::{BTreeMap, BTreeSet};

/// One player's statistics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayerStats {
    /// Units that entered play by kind (`UnitSpawned`: training, and
    /// `DebugSpawn` in scenarios); the start units are not counted.
    pub units_trained: BTreeMap<UnitKindId, u32>,
    /// Units that died by kind (`UnitDied`).
    pub units_lost: BTreeMap<UnitKindId, u32>,
    /// Towns this player annexed (`TownAnnexed` to it).
    pub towns_taken: u32,
    /// Towns annexed from this player (`TownAnnexed` from it).
    pub towns_lost: u32,
    /// `(age index, tick)` of every `AgeAdvanced`, in order.
    pub ages: Vec<(u8, u32)>,
    /// Bonus resources received per resource (`BonusIncome`),
    /// `resources.ron` order.
    pub bonus: Vec<i64>,
    /// Credited income per resource in 1/P resource units (P the income
    /// period), summed over the once-a-second samples.
    income_acc: Vec<i64>,
    /// Samples (seconds) in which any resource sat at its Yield Cap
    /// (`RateInfo::at_cap`, the HUD's amber readout).
    pub cap_seconds: u32,
    /// The most armed, non-civilian units the player had at a sample.
    pub peak_army: u32,
    /// The first observed tick at which one of the player's armed units
    /// stood on another player's land.
    pub first_on_land_tick: Option<u32>,
    /// The lowest hit points, per mille, of any complete Town the player
    /// started with or founded, while it owned it; 0 once one was annexed
    /// from it. Towns it annexed never count. 1000 when untouched.
    pub min_own_town_hp_permille: u32,
}

impl PlayerStats {
    /// Units that died, every kind.
    pub fn units_lost_total(&self) -> u32 {
        self.units_lost.values().sum()
    }

    /// Units that entered play, every kind.
    pub fn units_trained_total(&self) -> u32 {
        self.units_trained.values().sum()
    }

    /// Bonus resources received, every resource.
    pub fn bonus_total(&self) -> i64 {
        self.bonus.iter().sum()
    }
}

/// The statistics of one match (M6). Feed it with [`MatchStats::observe`]
/// after every step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchStats {
    /// Ticks per sample: one second of match time.
    sample_every: u32,
    /// The income period P (`Rules::income_period_ticks`).
    period: i64,
    town_kind: BuildingKindId,
    /// Per unit kind: armed (an attack) and not civilian.
    armed: Vec<bool>,
    players: Vec<PlayerStats>,
    /// Per player: the Towns it received by annexation (never its own).
    annexed_into: Vec<BTreeSet<BuildingId>>,
    first_contact_tick: Option<u32>,
    last_contact_tick: Option<u32>,
    max_gap_ticks: u32,
    last_tick: u32,
    last_sample_tick: Option<u32>,
    decided_tick: Option<u32>,
}

impl MatchStats {
    /// Empty statistics for a match of `setup` under `rules`.
    pub fn new(setup: &MatchSetup, rules: &Rules) -> MatchStats {
        let n = setup.players.len();
        let resources = rules.resources.resources.len();
        MatchStats {
            sample_every: rules.tick_rate_hz.max(1),
            period: i64::from(rules.income_period_ticks().max(1)),
            town_kind: BuildingKindId(rules.town_kind()),
            armed: rules
                .units
                .iter()
                .map(|u| u.attack.is_some() && u.class != UnitClass::Civilian)
                .collect(),
            players: (0..n)
                .map(|_| PlayerStats {
                    bonus: vec![0; resources],
                    income_acc: vec![0; resources],
                    min_own_town_hp_permille: 1000,
                    ..PlayerStats::default()
                })
                .collect(),
            annexed_into: vec![BTreeSet::new(); n],
            first_contact_tick: None,
            last_contact_tick: None,
            max_gap_ticks: 0,
            last_tick: 0,
            last_sample_tick: None,
            decided_tick: None,
        }
    }

    /// One player's statistics.
    ///
    /// # Panics
    ///
    /// If the player is not in the match.
    pub fn player(&self, p: PlayerId) -> &PlayerStats {
        &self.players[usize::from(p.0)]
    }

    /// Every player's statistics, in id order.
    pub fn players(&self) -> &[PlayerStats] {
        &self.players
    }

    /// Whether the unit kind counts toward the army (armed, not civilian).
    fn is_armed(&self, kind: UnitKindId) -> bool {
        self.armed
            .get(usize::from(kind.0))
            .copied()
            .unwrap_or(false)
    }

    fn stats_mut(&mut self, p: PlayerId) -> Option<&mut PlayerStats> {
        self.players.get_mut(usize::from(p.0))
    }

    /// Record one step: `view` is the sim after it, `events` what it
    /// emitted. Steps after the deciding one are ignored.
    pub fn observe(&mut self, view: &SimView<'_>, events: &[SimEvent]) {
        if self.decided_tick.is_some() {
            return;
        }
        let tick = view.tick;
        for e in events {
            self.event(view, events, e, tick);
        }
        self.every_tick(view, tick);
        if tick > 0 && tick.is_multiple_of(self.sample_every) && self.last_sample_tick != Some(tick)
        {
            self.sample(view);
            self.last_sample_tick = Some(tick);
        }
        self.last_tick = tick;
    }

    fn event(&mut self, view: &SimView<'_>, events: &[SimEvent], e: &SimEvent, tick: u32) {
        match e {
            SimEvent::UnitSpawned { unit } => {
                if let Some(u) = view.unit(*unit) {
                    let (owner, kind) = (u.owner, u.kind);
                    if let Some(s) = self.stats_mut(owner) {
                        *s.units_trained.entry(kind).or_default() += 1;
                    }
                }
            }
            SimEvent::UnitDied { owner, kind, .. } => {
                if let Some(s) = self.stats_mut(*owner) {
                    *s.units_lost.entry(*kind).or_default() += 1;
                }
            }
            SimEvent::TownAnnexed { town, from, to } => {
                let own = self
                    .annexed_into
                    .get(usize::from(from.0))
                    .is_some_and(|set| !set.contains(town));
                if let Some(s) = self.stats_mut(*from) {
                    s.towns_lost += 1;
                    if own {
                        s.min_own_town_hp_permille = 0;
                    }
                }
                if let Some(s) = self.stats_mut(*to) {
                    s.towns_taken += 1;
                }
                if let Some(set) = self.annexed_into.get_mut(usize::from(from.0)) {
                    set.remove(town);
                }
                if let Some(set) = self.annexed_into.get_mut(usize::from(to.0)) {
                    set.insert(*town);
                }
            }
            SimEvent::AgeAdvanced { player, age } => {
                if let Some(s) = self.stats_mut(*player) {
                    s.ages.push((*age, tick));
                }
            }
            SimEvent::BonusIncome { player, amounts } => {
                if let Some(s) = self.stats_mut(*player) {
                    for (total, a) in s.bonus.iter_mut().zip(amounts) {
                        *total = total.saturating_add(*a);
                    }
                }
            }
            SimEvent::Hit {
                attacker, target, ..
            } => {
                let a = owner_of(view, events, *attacker);
                let t = owner_of(view, events, *target);
                let cross = !matches!((a, t), (Some(a), Some(t)) if a == t);
                if cross {
                    self.contact(tick);
                }
            }
            SimEvent::MatchDecided { result } => {
                self.decided_tick = Some(result.tick);
            }
            _ => {}
        }
    }

    /// A cross-player hit at `tick`: the first contact, and the quiet span
    /// since the last one.
    fn contact(&mut self, tick: u32) {
        if self.first_contact_tick.is_none() {
            self.first_contact_tick = Some(tick);
        }
        if let Some(last) = self.last_contact_tick {
            self.max_gap_ticks = self.max_gap_ticks.max(tick.saturating_sub(last));
        }
        self.last_contact_tick = Some(tick);
    }

    /// The reads of every observed tick: own Town hit points and the first
    /// armed unit on another player's land.
    fn every_tick(&mut self, view: &SimView<'_>, tick: u32) {
        for b in view.buildings() {
            if b.kind != self.town_kind || b.construction != Construction::Complete {
                continue;
            }
            let annexed = self
                .annexed_into
                .get(usize::from(b.owner.0))
                .is_none_or(|set| set.contains(&b.id));
            if annexed {
                continue;
            }
            let Some((hp, max)) = view.building_hp(b.id) else {
                continue;
            };
            let permille = i64::from(hp.max(0)) * 1000 / i64::from(max.max(1));
            let permille = u32::try_from(permille.clamp(0, 1000)).unwrap_or(0);
            if let Some(s) = self.stats_mut(b.owner) {
                s.min_own_town_hp_permille = s.min_own_town_hp_permille.min(permille);
            }
        }
        if self.players.iter().all(|s| s.first_on_land_tick.is_some()) {
            return;
        }
        let territory = view.territory();
        let map = view.map();
        let mut found: Vec<PlayerId> = Vec::new();
        for u in view.units() {
            if !self.is_armed(u.kind)
                || self
                    .players
                    .get(usize::from(u.owner.0))
                    .is_none_or(|s| s.first_on_land_tick.is_some())
                || found.contains(&u.owner)
            {
                continue;
            }
            let land = territory.owner_at(map.tile_of(u.pos));
            if land.is_some_and(|q| q != u.owner) {
                found.push(u.owner);
            }
        }
        for p in found {
            if let Some(s) = self.stats_mut(p) {
                s.first_on_land_tick = Some(tick);
            }
        }
    }

    /// The once-a-second sample: credited income, cap time and army size.
    fn sample(&mut self, view: &SimView<'_>) {
        let every = i64::from(self.sample_every);
        let mut army = vec![0u32; self.players.len()];
        for u in view.units() {
            if self.is_armed(u.kind)
                && let Some(n) = army.get_mut(usize::from(u.owner.0))
            {
                *n += 1;
            }
        }
        for (i, s) in self.players.iter_mut().enumerate() {
            let p = PlayerId(u8::try_from(i).expect("at most 255 players"));
            let rates = view.income(p);
            for (acc, r) in s.income_acc.iter_mut().zip(&rates) {
                *acc = acc.saturating_add(i64::from(r.credited).saturating_mul(every));
            }
            if rates.iter().any(|r| r.at_cap()) {
                s.cap_seconds += 1;
            }
            s.peak_army = s.peak_army.max(army[i]);
        }
    }

    /// Total credited income (gathering and base yields, not the bonus) in
    /// whole resources, every resource, from the samples.
    pub fn income_total(&self, p: PlayerId) -> i64 {
        self.player(p).income_acc.iter().sum::<i64>() / self.period
    }

    /// The bonus's share of everything the player received,
    /// `bonus / (bonus + income)` in per mille (0 with no income at all).
    pub fn bonus_share_permille(&self, p: PlayerId) -> u32 {
        let bonus = self.player(p).bonus_total().max(0);
        let total = bonus.saturating_add(self.income_total(p).max(0));
        if total == 0 {
            return 0;
        }
        u32::try_from(bonus.saturating_mul(1000) / total).unwrap_or(1000)
    }

    /// The first tick a hit landed between two players' entities.
    pub fn first_contact_tick(&self) -> Option<u32> {
        self.first_contact_tick
    }

    /// The longest span with no cross-player hit after the first contact,
    /// up to the outcome (or the last observed tick); `None` before any
    /// contact.
    pub fn max_quiet_ticks(&self) -> Option<u32> {
        let last = self.last_contact_tick?;
        let end = self.decided_tick.unwrap_or(self.last_tick);
        Some(self.max_gap_ticks.max(end.saturating_sub(last)))
    }

    /// The deciding tick, once a `MatchDecided` was observed.
    pub fn decided_tick(&self) -> Option<u32> {
        self.decided_tick
    }
}

/// The owner of a hit's attacker or target: from the view after the step,
/// or, for an entity removed in that step, from its `UnitDied` or
/// `BuildingDestroyed` event.
fn owner_of(view: &SimView<'_>, events: &[SimEvent], id: TargetId) -> Option<PlayerId> {
    match id {
        TargetId::Unit(u) => view.unit(u).map(|x| x.owner).or_else(|| {
            events.iter().find_map(|e| match e {
                SimEvent::UnitDied { unit, owner, .. } if *unit == u => Some(*owner),
                _ => None,
            })
        }),
        TargetId::Building(b) => view.building(b).map(|x| x.owner).or_else(|| {
            events.iter().find_map(|e| match e {
                SimEvent::BuildingDestroyed {
                    building, owner, ..
                } if *building == b => Some(*owner),
                _ => None,
            })
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_hook::NoAi;
    use crate::command::{Command, PlayerCommand};
    use crate::fx::FxVec2;
    use crate::ids::{Tile, UnitId};
    use crate::state::{MatchResult, Outcome, OutcomeReason, Sim, StartKind};
    use std::path::Path;

    const P0: PlayerId = PlayerId(0);
    const P1: PlayerId = PlayerId(1);

    fn rules() -> Rules {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap()
    }

    fn unit_kind(rules: &Rules, id: &str) -> UnitKindId {
        UnitKindId(u16::try_from(rules.units.iter().position(|u| u.id == id).unwrap()).unwrap())
    }

    fn step(sim: &mut Sim, stats: &mut MatchStats, cmds: &[PlayerCommand]) {
        sim.step(cmds);
        let events = sim.drain_events();
        stats.observe(&sim.view(), &events);
    }

    /// M6: deaths by owner and kind, annexations both ways, ages, the bonus
    /// and cross-player contact come from events; cap seconds from the
    /// once-a-second samples (a Yield Cap of 0 puts every capped resource
    /// at the cap from the first second). Observing a tick twice changes
    /// nothing, and nothing is counted after the deciding step.
    #[test]
    fn match_stats_count_deaths_annexations_ages_and_cap_seconds() {
        let mut r = rules();
        r.yield_cap_base = 0;
        let mut setup = MatchSetup::scenario(&r, 1);
        setup.start = StartKind::Standard;
        let mut sim = Sim::new(setup.clone(), r.clone(), Box::new(NoAi));
        let mut stats = MatchStats::new(&setup, &r);
        for _ in 0..40 {
            step(&mut sim, &mut stats, &[]);
        }
        stats.observe(&sim.view(), &[]);
        for p in [P0, P1] {
            assert_eq!(stats.player(p).cap_seconds, 2, "samples at ticks 20 and 40");
            assert_eq!(stats.player(p).peak_army, 0, "Yeomen are civilians");
            assert_eq!(stats.player(p).units_trained_total(), 0, "start units");
        }
        let yeoman = unit_kind(&r, "yeoman");
        let skirmisher = unit_kind(&r, "skirmisher");
        let n = r.resources.resources.len();
        let mut bonus = vec![0i64; n];
        bonus[0] = 20;
        bonus[2] = 10;
        let seat0 = TargetId::Building(BuildingId(1));
        let synthetic = [
            SimEvent::UnitDied {
                unit: UnitId(900),
                owner: P1,
                kind: yeoman,
            },
            SimEvent::UnitDied {
                unit: UnitId(901),
                owner: P1,
                kind: yeoman,
            },
            SimEvent::UnitDied {
                unit: UnitId(902),
                owner: P0,
                kind: skirmisher,
            },
            SimEvent::TownAnnexed {
                town: BuildingId(2),
                from: P1,
                to: P0,
            },
            SimEvent::AgeAdvanced { player: P0, age: 1 },
            SimEvent::BonusIncome {
                player: P0,
                amounts: bonus.clone(),
            },
            // P0's own Yeomen: not contact.
            SimEvent::Hit {
                attacker: TargetId::Unit(UnitId(1)),
                target: TargetId::Unit(UnitId(2)),
                damage: 1,
            },
        ];
        stats.observe(&sim.view(), &synthetic);
        assert_eq!(stats.first_contact_tick(), None);
        assert_eq!(stats.max_quiet_ticks(), None);
        // A P1 Yeoman (UnitId(6)) hits P0's Seat at tick 40.
        let hit = SimEvent::Hit {
            attacker: TargetId::Unit(UnitId(6)),
            target: seat0,
            damage: 1,
        };
        stats.observe(&sim.view(), std::slice::from_ref(&hit));
        assert_eq!(stats.first_contact_tick(), Some(40));
        for _ in 0..30 {
            step(&mut sim, &mut stats, &[]);
        }
        assert_eq!(stats.max_quiet_ticks(), Some(30));
        stats.observe(&sim.view(), std::slice::from_ref(&hit));
        for _ in 0..10 {
            step(&mut sim, &mut stats, &[]);
        }
        assert_eq!(stats.max_quiet_ticks(), Some(30), "30 ticks, then 10");
        let p0 = stats.player(P0);
        let p1 = stats.player(P1);
        assert_eq!(p1.units_lost.get(&yeoman), Some(&2));
        assert_eq!((p0.units_lost_total(), p1.units_lost_total()), (1, 2));
        assert_eq!((p0.towns_taken, p0.towns_lost), (1, 0));
        assert_eq!((p1.towns_taken, p1.towns_lost), (0, 1));
        assert_eq!(p0.ages, [(1, 40)]);
        assert_eq!(p0.bonus, bonus);
        assert_eq!(p0.bonus_total(), 30);
        assert_eq!(p1.bonus_total(), 0);
        assert_eq!(p1.min_own_town_hp_permille, 0, "its Seat was annexed");
        assert_eq!(p0.min_own_town_hp_permille, 1000);
        assert_eq!(p0.cap_seconds, 4);
        let income = stats.income_total(P0);
        assert_eq!(
            stats.bonus_share_permille(P0),
            u32::try_from(30 * 1000 / (30 + income)).unwrap()
        );
        // The deciding step closes the books.
        let decided = SimEvent::MatchDecided {
            result: MatchResult {
                outcome: Outcome::Victory {
                    player: P0,
                    decisive: true,
                },
                reason: OutcomeReason::SeatCaptured,
                tick: 80,
            },
        };
        stats.observe(&sim.view(), &[decided]);
        let before = stats.clone();
        for _ in 0..25 {
            step(&mut sim, &mut stats, &[]);
        }
        stats.observe(&sim.view(), &synthetic);
        assert_eq!(stats, before);
        assert_eq!(stats.decided_tick(), Some(80));
        assert_eq!(stats.max_quiet_ticks(), Some(30));
    }

    /// M6: a side's lowest Town counts only the Towns it started with or
    /// founded. P0's own Town at 60% is its lowest even after it annexes
    /// one of P1's Towns, which comes back at 250 per mille; P1's lowest is
    /// 0 once its Town is annexed. A real annexation, with a 10-tick timer.
    #[test]
    fn match_stats_keep_town_hp_by_original_owner() {
        let mut r = rules();
        r.annexation_ds = 5;
        let town = BuildingKindId(r.building_kind_by_id("town").unwrap().0);
        let town_hp = r.building_kind(town.0).unwrap().hp;
        let skirmisher = unit_kind(&r, "skirmisher");
        let setup = MatchSetup::scenario(&r, 1);
        let mut sim = Sim::new(setup.clone(), r.clone(), Box::new(NoAi));
        let mut stats = MatchStats::new(&setup, &r);
        let mut seq = 0;
        let mut cmd = |c: Command| {
            seq += 1;
            PlayerCommand::new(P0, seq, c)
        };
        let place = |owner, x, y| Command::DebugPlaceBuilding {
            owner,
            kind: town,
            tile: Tile::new(x, y),
            complete: true,
        };
        // P1's second Town (BuildingId(3)) keeps the match undecided after
        // the annexation, so the samples go on.
        let mut tick0 = vec![
            cmd(place(P0, 59, 30)),
            cmd(place(P1, 20, 30)),
            cmd(place(P1, 40, 30)),
        ];
        for x in 20..=22 {
            tick0.push(cmd(Command::DebugSpawn {
                owner: P0,
                kind: skirmisher,
                at: FxVec2::from_ints(x, 29),
                count: 1,
            }));
        }
        step(&mut sim, &mut stats, &tick0);
        let damage = |b: u32, amount: u32| Command::DebugDamage {
            target: TargetId::Building(BuildingId(b)),
            amount,
        };
        let tick1 = [cmd(damage(2, u32::MAX)), cmd(damage(1, town_hp * 2 / 5))];
        step(&mut sim, &mut stats, &tick1);
        let mut annexed_at = None;
        for _ in 0..80 {
            sim.step(&[]);
            let events = sim.drain_events();
            if events.iter().any(|e| {
                matches!(
                    e,
                    SimEvent::TownAnnexed {
                        from: P1,
                        to: P0,
                        ..
                    }
                )
            }) {
                annexed_at = Some(sim.tick());
            }
            stats.observe(&sim.view(), &events);
        }
        assert!(annexed_at.is_some(), "P0 annexed P1's Town");
        let flipped = sim.view().building(BuildingId(2)).unwrap();
        assert_eq!(flipped.owner, P0);
        let (hp, max) = sim.view().building_hp(BuildingId(2)).unwrap();
        assert_eq!(i64::from(hp) * 1000 / i64::from(max), 250);
        assert_eq!(stats.player(P0).min_own_town_hp_permille, 600);
        assert_eq!(stats.player(P1).min_own_town_hp_permille, 0);
        assert_eq!(stats.player(P0).towns_taken, 1);
        assert_eq!(stats.player(P1).towns_lost, 1);
        assert_eq!(stats.player(P0).units_trained.get(&skirmisher), Some(&3));
        assert_eq!(stats.player(P0).peak_army, 3);
        assert!(
            stats.player(P0).first_on_land_tick.is_some(),
            "the Skirmishers stand on P1's land"
        );
        assert_eq!(stats.player(P1).first_on_land_tick, None);
        assert_eq!(sim.view().result(), None, "P1 keeps its second Town");
    }
}
