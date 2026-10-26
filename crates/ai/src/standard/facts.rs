//! [`Facts`]: what one plan knows about the match, gathered in one pass over
//! the units and one over the buildings.

use rules::{Rules, UnitClass};
use sim::{
    BuildingId, BuildingKindId, Construction, Fx, FxVec2, Hold, Job, PlayerId, QueueItem, SimView,
    TechId, Tile, UnitId, UnitKindId,
};
use std::collections::{BTreeMap, BTreeSet};

/// The centre of a `w` x `h` footprint whose north-west tile is `nw`, in
/// world coordinates (a 3x3 footprint's centre is the middle of its middle
/// tile).
pub fn footprint_centre(nw: Tile, w: u32, h: u32) -> FxVec2 {
    let half = |n: u32| Fx::from_ratio(i32::try_from(n).expect("validated <= 8"), 2);
    FxVec2::new(
        Fx::from_int(i32::from(nw.x)) + half(w),
        Fx::from_int(i32::from(nw.y)) + half(h),
    )
}

/// The centre of building `kind` placed at `nw` (see [`footprint_centre`]).
///
/// # Panics
///
/// If `kind` is not in the rules.
pub fn kind_centre(rules: &Rules, kind: BuildingKindId, nw: Tile) -> FxVec2 {
    let def = rules
        .building_kind(kind.0)
        .expect("a building's kind exists");
    footprint_centre(nw, def.width_tiles, def.height_tiles)
}

/// The tile that holds the centre of building `kind` placed at `nw`: the
/// footprint's middle tile for odd sizes, the one south-east of the centre
/// for even ones.
pub fn kind_centre_tile(rules: &Rules, kind: BuildingKindId, nw: Tile) -> Tile {
    let def = rules
        .building_kind(kind.0)
        .expect("a building's kind exists");
    let half = |n: u32| u16::try_from(n / 2).expect("validated <= 8");
    Tile::new(nw.x + half(def.width_tiles), nw.y + half(def.height_tiles))
}

/// `r` tiles squared in the scale of [`FxVec2::dist_sq_i64`].
pub fn tiles_sq(r: u32) -> i64 {
    (i64::from(r) * i64::from(r)) << 32
}

/// A gathering building of the bot's: where it is and how many of its
/// slots a unit holds (walking or working).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Node {
    /// The building.
    pub id: BuildingId,
    /// Its footprint centre.
    pub centre: FxVec2,
    /// Its slots.
    pub slots: u8,
    /// Slots a unit's gather job holds.
    pub taken: u8,
}

impl Node {
    /// Slots no unit holds.
    pub fn free(&self) -> u8 {
        self.slots.saturating_sub(self.taken)
    }
}

/// Per unit kind, what a plan needs to sort units: whether the kind is
/// the worker kind, armed (non-civilian with an attack), a supplier.
#[derive(Debug, Clone, Copy, Default)]
struct KindFlags {
    worker: bool,
    armed: bool,
    supplies: bool,
    civilian: bool,
}

/// The facts one plan reads, gathered once per plan by [`Facts::gather`].
/// Every list is in id order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// The plan's tick.
    pub tick: u32,
    /// The bot's player.
    pub me: PlayerId,
    /// Its stockpile, `resources.ron` order.
    pub stock: Vec<i64>,
    /// The age it has reached.
    pub age: u8,
    /// Its pop: used (alive and queued) and cap.
    pub pop: (u32, u32),
    /// The worker kind (`rules::ai::worker_kind`).
    pub worker_kind: Option<UnitKindId>,
    /// Own units of the worker kind.
    pub workers: Vec<UnitId>,
    /// Own workers with no job and no order.
    pub idle_workers: Vec<UnitId>,
    /// Own units gathering, per resource index (`Job::Gather` on a node
    /// of that resource).
    pub gatherers: Vec<Vec<UnitId>>,
    /// Own units building, per site (`Job::Build`).
    pub builders: BTreeMap<BuildingId, Vec<UnitId>>,
    /// Own armed units: non-civilian kinds with an attack.
    pub armed: Vec<UnitId>,
    /// Own units of kinds that supply (the Supply Wain).
    pub supply: Vec<UnitId>,
    /// Own units alive per kind.
    pub alive: BTreeMap<UnitKindId, u32>,
    /// Own units queued per kind, at every own building.
    pub queued_units: BTreeMap<UnitKindId, u32>,
    /// Techs researched, or queued at an own building.
    pub researched_or_queued: BTreeSet<TechId>,
    /// Own buildings per kind, sites included.
    pub buildings: BTreeMap<BuildingKindId, u32>,
    /// Own buildings, sites included, per `(Town attributed to, kind)`.
    pub per_town: BTreeMap<(BuildingId, BuildingKindId), u32>,
    /// Own complete buildings the worker kind gathers at, per resource
    /// index, in id order.
    pub nodes: Vec<Vec<Node>>,
    /// Own complete Towns whose annexation timer runs (no training or
    /// research there).
    pub contested: BTreeSet<BuildingId>,
    /// The centre tiles of own buildings and sites of non-Town kinds that
    /// project a border (Watchtowers).
    pub towers: Vec<Tile>,
    /// The centres of own Towns, sites included.
    pub town_centres: Vec<FxVec2>,
    /// Own complete buildings per kind.
    pub complete: BTreeMap<BuildingKindId, Vec<BuildingId>>,
    /// Own construction sites.
    pub sites: Vec<BuildingId>,
    /// Own complete Towns: the Seat first, then by id.
    pub towns: Vec<BuildingId>,
    /// Complete Towns of the other players.
    pub enemy_towns: Vec<BuildingId>,
    /// Complete buildings of the other players that shoot and have hit
    /// points left.
    pub enemy_shooters: Vec<BuildingId>,
    /// Non-civilian units of the other players on own land.
    pub threats: Vec<UnitId>,
}

impl Facts {
    /// Gather the facts for `me` from the view: one pass over the units,
    /// one over the buildings.
    pub fn gather(view: &SimView<'_>, me: PlayerId) -> Facts {
        let rules: &Rules = view.rules();
        let worker = rules::ai::worker_kind(rules).map(|(i, _)| UnitKindId(i));
        let worker_id = rules::ai::worker_kind(rules).map(|(_, k)| k.id.as_str());
        let flags: Vec<KindFlags> = rules
            .units
            .iter()
            .enumerate()
            .map(|(i, k)| KindFlags {
                worker: worker.is_some_and(|w| usize::from(w.0) == i),
                armed: k.class != UnitClass::Civilian && k.attack.is_some(),
                supplies: k.supplies,
                civilian: k.class == UnitClass::Civilian,
            })
            .collect();
        let flag = |k: UnitKindId| flags.get(usize::from(k.0)).copied().unwrap_or_default();
        let resource_of = |node: BuildingId| -> Option<usize> {
            let b = view.building(node)?;
            let g = rules.building_kind(b.kind.0)?.gather.as_ref()?;
            rules
                .resources
                .resources
                .iter()
                .position(|r| r.id == g.resource)
        };
        let mut f = Facts {
            tick: view.tick,
            me,
            stock: view.economy(me).stock.clone(),
            age: view.age(me),
            pop: view.pop(me),
            worker_kind: worker,
            workers: Vec::new(),
            idle_workers: Vec::new(),
            gatherers: vec![Vec::new(); rules.resources.resources.len()],
            builders: BTreeMap::new(),
            armed: Vec::new(),
            supply: Vec::new(),
            alive: BTreeMap::new(),
            queued_units: BTreeMap::new(),
            researched_or_queued: view.researched(me).clone(),
            buildings: BTreeMap::new(),
            per_town: BTreeMap::new(),
            nodes: vec![Vec::new(); rules.resources.resources.len()],
            contested: BTreeSet::new(),
            towers: Vec::new(),
            town_centres: Vec::new(),
            complete: BTreeMap::new(),
            sites: Vec::new(),
            towns: Vec::new(),
            enemy_towns: Vec::new(),
            enemy_shooters: Vec::new(),
            threats: Vec::new(),
        };
        let territory = view.territory();
        let map = view.map();
        let mut taken: BTreeMap<BuildingId, u8> = BTreeMap::new();
        for u in view.units() {
            let k = flag(u.kind);
            if u.owner != me {
                if !k.civilian && territory.owner_at(map.tile_of(u.pos)) == Some(me) {
                    f.threats.push(u.id);
                }
                continue;
            }
            *f.alive.entry(u.kind).or_default() += 1;
            if k.worker {
                f.workers.push(u.id);
                if u.job.is_none() && u.order.is_none() {
                    f.idle_workers.push(u.id);
                }
            }
            match u.job {
                Some(Job::Gather { node, .. }) => {
                    let n = taken.entry(node).or_default();
                    *n = n.saturating_add(1);
                    if let Some(r) = resource_of(node) {
                        f.gatherers[r].push(u.id);
                    }
                }
                Some(Job::Build { site }) => f.builders.entry(site).or_default().push(u.id),
                None => {}
            }
            if k.armed {
                f.armed.push(u.id);
            }
            if k.supplies {
                f.supply.push(u.id);
            }
        }
        let town_kind = BuildingKindId(rules.town_kind());
        let mut seat = None;
        for b in view.buildings() {
            let complete = b.construction == Construction::Complete;
            if b.owner != me {
                if complete && b.kind == town_kind {
                    f.enemy_towns.push(b.id);
                }
                let shoots = rules
                    .building_kind(b.kind.0)
                    .is_some_and(|d| d.attack.is_some());
                if complete && shoots && b.hp > 0 {
                    f.enemy_shooters.push(b.id);
                }
                continue;
            }
            *f.buildings.entry(b.kind).or_default() += 1;
            if let Some(t) = b.town {
                *f.per_town.entry((t, b.kind)).or_default() += 1;
            }
            let def = rules.building_kind(b.kind.0);
            if b.kind == town_kind {
                f.town_centres.push(kind_centre(rules, b.kind, b.tile));
            } else if def.is_some_and(|d| d.border_radius_tiles.is_some()) {
                f.towers.push(kind_centre_tile(rules, b.kind, b.tile));
            }
            if !complete {
                f.sites.push(b.id);
                continue;
            }
            f.complete.entry(b.kind).or_default().push(b.id);
            if let Some(g) = def.and_then(|d| d.gather.as_ref())
                && worker_id.is_some_and(|w| w == g.worker)
                && let Some(r) = rules.resource_index(&g.resource)
            {
                f.nodes[r].push(Node {
                    id: b.id,
                    centre: kind_centre(rules, b.kind, b.tile),
                    slots: b.slots,
                    taken: taken.get(&b.id).copied().unwrap_or(0),
                });
            }
            if b.kind == town_kind {
                if b.town_state.is_some_and(|t| t.seat) {
                    seat = Some(b.id);
                } else {
                    f.towns.push(b.id);
                }
                if b.town_state.is_some_and(|t| t.hold != Hold::Held) {
                    f.contested.insert(b.id);
                }
            }
            for entry in &b.queue {
                match entry.item {
                    QueueItem::Unit(kind) => *f.queued_units.entry(kind).or_default() += 1,
                    QueueItem::Research { tech, .. } => {
                        f.researched_or_queued.insert(tech);
                    }
                }
            }
        }
        if let Some(seat) = seat {
            f.towns.insert(0, seat);
        }
        f
    }

    /// Units of `kind` alive plus queued.
    pub fn alive_and_queued(&self, kind: UnitKindId) -> u32 {
        self.alive.get(&kind).copied().unwrap_or(0)
            + self.queued_units.get(&kind).copied().unwrap_or(0)
    }

    /// Own buildings of `kind`, sites included.
    pub fn owned(&self, kind: BuildingKindId) -> u32 {
        self.buildings.get(&kind).copied().unwrap_or(0)
    }
}
