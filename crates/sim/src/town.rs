//! Towns (`docs/design/towns.md`): the standard start, the town count and
//! limit, attribution of other buildings to a Town, and the tier and border
//! radius a Town derives from its buildings, its owner's age and the
//! owner's Statecraft rows.

use crate::construction::{self, centre_fx};
use crate::economy::building_kind;
use crate::ids::{BuildingId, PlayerId, Tile, UnitKindId};
use crate::state::{Construction, SimEvent, State, TownState, Unit};
use crate::tech;
use crate::territory::{Source, SourceOwner};
use rules::{MapDef, Rules};
use std::collections::BTreeSet;

/// Town-kind buildings `p` owns, sites included (the town limit counts
/// both).
pub(crate) fn town_count(state: &State, rules: &Rules, p: PlayerId) -> u32 {
    let town = rules.town_kind();
    let n = state
        .buildings
        .values()
        .filter(|b| b.owner == p && b.kind.0 == town)
        .count();
    u32::try_from(n).expect("count fits u32")
}

/// The Town a new building centred at `centre` belongs to: the nearest own
/// complete Town by squared distance between footprint centres, ties by
/// `BuildingId`. `None` when `p` has no complete Town.
pub(crate) fn attribute(
    state: &State,
    rules: &Rules,
    p: PlayerId,
    centre: crate::fx::FxVec2,
) -> Option<BuildingId> {
    let town = rules.town_kind();
    state
        .buildings
        .values()
        .filter(|b| b.owner == p && b.kind.0 == town && b.construction == Construction::Complete)
        .min_by_key(|b| {
            let c = centre_fx(building_kind(rules, b.kind), b.tile);
            (c.dist_sq_i64(centre), b.id)
        })
        .map(|b| b.id)
}

/// Re-derive complete Town `town`'s tier and border radius. The tier is 2
/// when the owner's complete buildings attributed to it cover at least
/// `town_growth_distinct_buildings` distinct non-Town kinds (sites do not
/// count) and the owner has reached `town_growth_age` (when set), else 1.
/// The radius is the tier's base radius with the owner's
/// `TownBorderRadius` rows ([`tech::town_radius`]). A tier change emits
/// `TownTierChanged`; a radius change, which a tier change or a Statecraft
/// level makes, moves the Town's border source with
/// `TerritoryField::change_source` and emits `TerritoryChanged`. Runs when a
/// building attributed to the Town completes and, through [`refresh_all`],
/// when the owner completes a tech or advances an age. Does nothing for a
/// site or a building that is not a Town.
pub(crate) fn refresh(
    state: &mut State,
    rules: &Rules,
    town: BuildingId,
    events: &mut Vec<SimEvent>,
) {
    let Some(t) = state.buildings.get(&town) else {
        return;
    };
    let Some(ts) = t.town_state else {
        return;
    };
    let owner = t.owner;
    let town_kind = rules.town_kind();
    let kinds: BTreeSet<u16> = state
        .buildings
        .values()
        .filter(|b| {
            b.owner == owner
                && b.town == Some(town)
                && b.kind.0 != town_kind
                && b.construction == Construction::Complete
        })
        .map(|b| b.kind.0)
        .collect();
    let threshold = usize::try_from(rules.town_growth_distinct_buildings).expect("small");
    let age_reached = rules.town_growth_age.as_ref().is_none_or(|id| {
        state.tech.player(owner).age >= rules.age_index(id).expect("validated age id")
    });
    let tier = if kinds.len() >= threshold && age_reached {
        2
    } else {
        1
    };
    if tier != ts.tier {
        state.buildings.get_mut(&town).expect("exists").town_state = Some(TownState { tier, ..ts });
        events.push(SimEvent::TownTierChanged { town, tier });
    }
    let radius =
        u16::try_from(tech::town_radius(state, rules, owner, tier)).expect("radius clamped to 64");
    let mut source = state.territory.sources()[&town];
    if source.radius != radius {
        source.radius = radius;
        state.territory.change_source(town, source);
        events.push(SimEvent::TerritoryChanged);
    }
}

/// [`refresh`] every complete Town `p` owns, in `BuildingId` order.
pub(crate) fn refresh_all(
    state: &mut State,
    rules: &Rules,
    p: PlayerId,
    events: &mut Vec<SimEvent>,
) {
    let towns: Vec<BuildingId> = state
        .buildings
        .values()
        .filter(|b| b.owner == p && b.town_state.is_some())
        .map(|b| b.id)
        .collect();
    for town in towns {
        refresh(state, rules, town, events);
    }
}

/// The standard start (`rules.ron` `standard_start`): first every player's
/// Seat, in id order, a complete tier-1 Town centred on the player's map
/// start, projecting its border; then, per player in id order, the start
/// units on the nearest free passable tiles around the start, in
/// `UnitKindId` order, and the stockpile.
/// On a map whose starts are `(24, 64)` and `(103, 64)` that gives the
/// Seats `BuildingId(1)` and `BuildingId(2)`, P0's Yeomen `UnitId(1..=5)`
/// and P1's `UnitId(6..=10)`.
pub(crate) fn apply_standard_start(state: &mut State, rules: &Rules, def: &MapDef, hz: u32) {
    let town_kind = rules.town_kind();
    let kind = building_kind(rules, crate::ids::BuildingKindId(town_kind));
    let (w, h) = (
        u16::try_from(kind.width_tiles).expect("validated <= 8"),
        u16::try_from(kind.height_tiles).expect("validated <= 8"),
    );
    let players: Vec<PlayerId> = state.players.iter().map(|p| p.id).collect();
    // Every Seat first, so no unit can spawn where a later Seat stands.
    for &p in &players {
        let start = def.starts[usize::from(p.0)];
        let nw = Tile::new(start.x - (w - 1) / 2, start.y - (h - 1) / 2);
        let seat = construction::place(
            state,
            rules,
            p,
            crate::ids::BuildingKindId(town_kind),
            nw,
            vec![0; rules.resources.resources.len()],
            0,
            None,
        );
        let b = state.buildings.get_mut(&seat).expect("just placed");
        b.construction = Construction::Complete;
        b.town_state = Some(TownState {
            seat: true,
            tier: 1,
        });
        let radius = tech::town_radius(state, rules, p, 1);
        state.territory.add_source(
            seat,
            Source {
                owner: SourceOwner::Player(p),
                centre: Tile::new(start.x, start.y),
                radius: u16::try_from(radius).expect("validated radius fits u16"),
            },
        );
    }
    // Then each player's units, kind by kind, on free passable tiles of the
    // spiral around the start (one unit per tile, across kinds and players).
    let mut taken: BTreeSet<Tile> = BTreeSet::new();
    for &p in &players {
        let start = def.starts[usize::from(p.0)];
        for (k, unit_kind) in rules.units.iter().enumerate() {
            let Some(&count) = rules.standard_start.units.get(&unit_kind.id) else {
                continue;
            };
            let kind_id = UnitKindId(u16::try_from(k).expect("validated len"));
            let tiles: Vec<Tile> = state
                .map
                .spiral(Tile::new(start.x, start.y))
                .filter(|t| state.map.passable(*t) && !taken.contains(t))
                .take(usize::try_from(count).expect("validated <= 255"))
                .collect();
            for tile in tiles {
                taken.insert(tile);
                let id = state.ids.unit();
                let u = Unit::spawn(id, p, kind_id, tile, unit_kind, &state.map, hz);
                state.units.insert(id, u);
            }
        }
        state.economy.player_mut(p).stock = rules.amounts(&rules.standard_start.stockpile);
    }
}
