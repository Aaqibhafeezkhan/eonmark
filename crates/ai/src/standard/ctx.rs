//! [`Ctx`]: one plan's working state, and the [`Ledger`] that keeps the
//! plan's rules from spending the same stock twice.

use super::facts::Facts;
use super::sites::MapAnalysis;
use rules::{BuildOrder, Personality};
use sim::{BuildingKindId, Command, FxVec2, PlayerId, RejectReason, SimView, Tile, Unit, UnitId};
use std::collections::{BTreeMap, BTreeSet};

/// Where `u` is headed or holds: its order's goal, else its post's goal.
/// (A unit's order becomes a post on arrival, so "has an order goal near
/// X" alone is not a stable fact.)
pub fn goal_or_post(u: &Unit) -> Option<FxVec2> {
    u.order.as_ref().map(|o| o.goal).or(u.post.map(|p| p.goal))
}

/// Whether `u` is already heading for, or holding, a point within
/// `tolerance_tiles` of `goal` ("on station": it gets no new order).
pub fn on_station(u: &Unit, goal: FxVec2, tolerance_tiles: u32) -> bool {
    goal_or_post(u).is_some_and(|g| g.dist_sq_i64(goal) <= super::facts::tiles_sq(tolerance_tiles))
}

/// The stock one plan may still spend: what the stockpile holds, less
/// what was spent or reserved earlier in the same plan. All arithmetic
/// saturates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ledger {
    /// The stockpile at the plan's start, less what this plan spent.
    pub stock: Vec<i64>,
    /// What earlier steps of this plan are saving up for.
    pub reserved: Vec<i64>,
}

impl Ledger {
    /// A ledger over `stock` with nothing reserved.
    pub fn new(stock: Vec<i64>) -> Ledger {
        let reserved = vec![0; stock.len()];
        Ledger { stock, reserved }
    }

    /// What resource `r` can still pay for: stock less reservations, at
    /// least 0.
    pub fn available(&self, r: usize) -> i64 {
        let stock = self.stock.get(r).copied().unwrap_or(0);
        let reserved = self.reserved.get(r).copied().unwrap_or(0);
        stock.saturating_sub(reserved).max(0)
    }

    /// Whether `cost` fits what is available.
    pub fn affords(&self, cost: &[i64]) -> bool {
        cost.iter()
            .enumerate()
            .all(|(r, c)| *c <= self.available(r))
    }

    /// Spend `cost` if it fits what is available; `false` (and nothing
    /// spent) otherwise.
    pub fn try_spend(&mut self, cost: &[i64]) -> bool {
        if !self.affords(cost) {
            return false;
        }
        for (r, c) in cost.iter().enumerate() {
            if let Some(s) = self.stock.get_mut(r) {
                *s = s.saturating_sub(*c);
            }
        }
        true
    }

    /// Hold `cost` back from later spending in this plan (a step saving
    /// up).
    pub fn reserve(&mut self, cost: &[i64]) {
        for (r, c) in cost.iter().enumerate() {
            if let Some(v) = self.reserved.get_mut(r) {
                *v = v.saturating_add(*c);
            }
        }
    }
}

/// One plan's working state. The rules run in a fixed order over the same
/// `Ctx`: each reads the view and the facts, claims the units it uses so a
/// later rule leaves them alone, spends or reserves through the ledger,
/// and pushes commands within the plan's caps.
pub struct Ctx<'a> {
    /// The view of the tick being planned.
    pub view: SimView<'a>,
    /// The bot's player.
    pub me: PlayerId,
    /// The personality's knobs.
    pub knobs: &'a Personality,
    /// The personality's build order.
    pub order: &'a BuildOrder,
    /// The plan's facts.
    pub facts: Facts,
    /// The static map analysis of the bot's seat.
    pub map: &'a MapAnalysis,
    /// The plan's stock.
    pub ledger: Ledger,
    /// Units an earlier rule of this plan already gave an order.
    pub claimed: BTreeSet<UnitId>,
    /// `check_build` probes (a full-map search each) left in this plan.
    pub probes_left: u32,
    /// `check_build` probes made in this plan, every one counted by
    /// [`Ctx::check_build`] (the work-cap test reads it).
    pub probes_made: u32,
    /// Commands left in this plan.
    pub commands_left: u32,
    /// The footprints this plan's `Build`s place, as inclusive tile
    /// bounds `(x0, y0, x1, y1)`: the view does not show them yet, so a
    /// later site of the same plan keeps its gap from them
    /// ([`super::sites::fits`]).
    pub planned: Vec<(i32, i32, i32, i32)>,
    out: Vec<Command>,
    occupied: Option<BTreeSet<Tile>>,
    rally: Option<FxVec2>,
}

impl<'a> Ctx<'a> {
    /// A fresh plan context: the facts gathered, the ledger at the stock,
    /// nothing claimed, the caps from the knobs.
    ///
    /// # Panics
    ///
    /// If the knobs name a build order the rules do not have (`Rules::load`
    /// rules it out).
    pub fn new(
        view: SimView<'a>,
        me: PlayerId,
        knobs: &'a Personality,
        map: &'a MapAnalysis,
    ) -> Ctx<'a> {
        let order = view
            .rules()
            .ai
            .build_order(&knobs.build_order)
            .expect("Rules::load checks every personality's build order");
        let facts = Facts::gather(&view, me);
        let ledger = Ledger::new(facts.stock.clone());
        Ctx {
            view,
            me,
            knobs,
            order,
            facts,
            map,
            ledger,
            claimed: BTreeSet::new(),
            probes_left: knobs.site_checks_per_think,
            probes_made: 0,
            commands_left: knobs.max_commands_per_think,
            planned: Vec::new(),
            out: Vec::new(),
            occupied: None,
            rally: None,
        }
    }

    /// The plan's tick.
    pub fn tick(&self) -> u32 {
        self.view.tick
    }

    /// `ds` deciseconds in ticks.
    pub fn ticks(&self, ds: u32) -> u32 {
        self.view.rules().ticks_from_ds(ds)
    }

    /// The tiles any unit stands on, gathered once per plan on first use
    /// (a site with a unit on its footprint is rejected).
    pub fn occupied(&mut self) -> &BTreeSet<Tile> {
        let view = self.view;
        self.occupied
            .get_or_insert_with(|| view.units().map(|u| view.map().tile_of(u.pos)).collect())
    }

    /// The army's rally point: the centre of the first tile of the square
    /// spiral around the map analysis' rally seed that is passable and the
    /// bot's own land (the own start when no such tile is near), computed
    /// once per plan on first use.
    pub fn rally(&mut self) -> FxVec2 {
        if let Some(r) = self.rally {
            return r;
        }
        let map = self.view.map();
        let territory = self.view.territory();
        let seed = self.map.rally_seed;
        let reach = i32::try_from(self.knobs.rally_tiles_from_seat).unwrap_or(0);
        // The spiral yields whole rings in order and skips tiles off the
        // map, so this many tiles covers at most the rings up to `reach`.
        let tile = map
            .spiral(seed)
            .take(rally_search_tiles(reach))
            .find(|t| map.passable(*t) && territory.owner_at(*t) == Some(self.me))
            .unwrap_or(self.map.own_start);
        let r = map.center_of(tile);
        self.rally = Some(r);
        r
    }

    /// The view's `check_build` for the bot (a full-map search), counted:
    /// one probe off `probes_left` (the caller checks there is one left)
    /// and one onto `probes_made`. Every probe of the bot goes through
    /// here, so `probes_made` is the number of full-map searches the plan
    /// made.
    pub fn check_build(
        &mut self,
        builder: UnitId,
        kind: BuildingKindId,
        tile: Tile,
    ) -> Result<Vec<i64>, RejectReason> {
        self.probes_left = self.probes_left.saturating_sub(1);
        self.probes_made = self.probes_made.saturating_add(1);
        self.view.check_build(self.me, builder, kind, tile)
    }

    /// Issue `make(units)` as one command, unless `units` is empty (then
    /// nothing is issued). `false` when the plan's command cap is reached.
    pub fn order(&mut self, units: Vec<UnitId>, make: impl FnOnce(Vec<UnitId>) -> Command) -> bool {
        if units.is_empty() {
            return true;
        }
        self.push(make(units))
    }

    /// Issue one command per distinct point of `orders` (units grouped by
    /// the point they go to, in point order), within the cap.
    pub fn order_grouped(
        &mut self,
        orders: Vec<(UnitId, FxVec2)>,
        make: impl Fn(Vec<UnitId>, FxVec2) -> Command,
    ) {
        let mut by_point: BTreeMap<(i64, i64), (FxVec2, Vec<UnitId>)> = BTreeMap::new();
        for (u, p) in orders {
            by_point
                .entry((p.x.to_bits(), p.y.to_bits()))
                .or_insert_with(|| (p, Vec::new()))
                .1
                .push(u);
        }
        for (_, (p, mut units)) in by_point {
            units.sort();
            if !self.push(make(units, p)) {
                return;
            }
        }
    }

    /// Queue `cmd` if the plan has a command left; `false` when the cap is
    /// reached (and nothing was queued).
    pub fn push(&mut self, cmd: Command) -> bool {
        if self.commands_left == 0 {
            return false;
        }
        self.commands_left -= 1;
        self.out.push(cmd);
        true
    }

    /// Claim `unit` for the rule that is ordering it; `false` when an
    /// earlier rule of this plan already claimed it.
    pub fn claim(&mut self, unit: UnitId) -> bool {
        self.claimed.insert(unit)
    }

    /// Whether an earlier rule of this plan claimed `unit`.
    pub fn is_claimed(&self, unit: UnitId) -> bool {
        self.claimed.contains(&unit)
    }

    /// The commands this plan issues, in the order the rules pushed them.
    pub fn into_commands(self) -> Vec<Command> {
        self.out
    }
}

/// Tiles of the square spiral searched for the rally point: rings up to
/// `reach`.
fn rally_search_tiles(reach: i32) -> usize {
    let side = usize::try_from(2 * reach.max(0) + 1).expect("non-negative");
    side * side
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_spends_only_what_is_not_reserved_and_saturates() {
        let mut l = Ledger::new(vec![150, 150, 0, 0]);
        assert!(l.try_spend(&[30, 0, 0, 0]));
        assert_eq!(l.stock, [120, 150, 0, 0]);
        l.reserve(&[100, 0, 0, 0]);
        assert_eq!(l.available(0), 20);
        assert!(!l.try_spend(&[40, 0, 0, 0]), "the reservation holds");
        assert_eq!(l.stock, [120, 150, 0, 0], "a refused spend changes nothing");
        assert!(l.try_spend(&[20, 50, 0, 0]));
        assert_eq!(l.available(0), 0);
        l.reserve(&[i64::MAX, 0, 0, 0]);
        assert_eq!(l.available(0), 0, "saturates, never negative");
        assert!(!l.affords(&[0, 0, 1, 0]));
        assert!(l.affords(&[0, 0, 0, 0]));
        assert_eq!(l.available(9), 0, "an unknown resource has nothing");
    }

    /// Every full-map probe of the bot goes through [`Ctx::check_build`],
    /// so `probes_made` counts them all: no other source file of the crate
    /// calls a `check_build` but the plan context's.
    #[test]
    fn every_probe_goes_through_the_counter() {
        let files = [
            ("army.rs", include_str!("army.rs")),
            ("build_order.rs", include_str!("build_order.rs")),
            ("defend.rs", include_str!("defend.rs")),
            ("economy.rs", include_str!("economy.rs")),
            ("facts.rs", include_str!("facts.rs")),
            ("memory.rs", include_str!("memory.rs")),
            ("mod.rs", include_str!("mod.rs")),
            ("sites.rs", include_str!("sites.rs")),
            ("waves.rs", include_str!("waves.rs")),
            ("../harness.rs", include_str!("../harness.rs")),
            ("../lib.rs", include_str!("../lib.rs")),
            ("../seats.rs", include_str!("../seats.rs")),
        ];
        let call = "check_build(";
        let mut counted = 0;
        for (name, src) in files {
            for (at, _) in src.match_indices(call) {
                assert!(
                    src[..at].ends_with("ctx."),
                    "{name}: a check_build( not through Ctx::check_build: {:?}",
                    &src[at.saturating_sub(40)..at + call.len()]
                );
                counted += 1;
            }
        }
        assert!(
            counted >= 2,
            "the site probe and the site keeper: {counted}"
        );
    }
}
