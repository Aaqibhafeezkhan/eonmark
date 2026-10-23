//! Attrition and supply (`docs/design/attrition.md`, "M5a implementation
//! notes"): enemy units inside a player's borders lose hit points once that
//! player has researched Harrying, unless a friendly Supply Wain covers
//! them.
//!
//! Hashed state is one counter per unit, [`Unit::exposed_ticks`] (sub-hash
//! `units`): the number of consecutive steps the unit has been exposed,
//! saturating. Everything else is computed inside the step and never
//! stored, so a restore has nothing to rebuild: the per-player Harrying
//! levels, intervals and pulse sizes, and the `SupplyGrid` of every
//! player's Supply Wain tiles.
//!
//! Step 5b', after movement and its repath requests and before
//! `combat::resolve`, walks the units in `UnitId` order. A unit is exposed
//! when its kind is not `immune_to_attrition`, the tile it stands on is
//! owned by another player (`TerritoryField::owner_at`, so neutral land is
//! safe), that player's Harrying level is at least 1, and no Supply Wain of
//! the unit's owner stands within `supply_radius_tiles` of the tile
//! (`(dx² + dy²) <= r²`, tile to tile, inclusive). An exposed unit's
//! counter goes up by one; any other unit's counter drops to 0, so walking
//! in and out of cover never accumulates hidden damage (a reset, not a
//! pause). A pulse lands when the counter is a multiple of the owner's
//! interval: `damage_taken` grows by the pulse's hit points (clamped to
//! `i32`) and the step emits [`SimEvent::Attrition`]. The dead are removed
//! by `combat::reap` in 5c, which reads the hit points of the `Stats`
//! built at the start of step 5, through the common death path
//! (`UnitDied`, the path request cancelled); a unit that attrition kills
//! still lands its own hit in 5c, as with simultaneous hits.
//!
//! - Level: the `HarryingLevel` rows of the owner's researched techs and
//!   faction from base 0 (each Harrying tech adds 1), clamped to
//!   `0..=MAX_HARRYING_LEVEL`. 0 means no attrition.
//! - Interval at level `L >= 1`: `max(1, I1 >> (L - 1))` ticks with
//!   `I1 = Rules::attrition_interval_ticks()` (64 at 20 Hz), so 64 ticks at
//!   Harrying I and 32 at Harrying II. A level gained mid-exposure keeps the
//!   counter: the next pulse lands on the next multiple of the new interval,
//!   with no catch-up.
//! - Pulse: `attrition_age_gap_hp[min(gap, len - 1)]` with
//!   `gap = owner age - intruder age` floored at 0, so a more advanced
//!   intruder takes the gap-0 row; plus `harrying_bonus_hp` once the
//!   owner's age index is at least `harrying_bonus_age`'s. Integers only.
//!
//! When no player has a Harrying level the step only zeroes the counters
//! (every exposed unit's owner would be at level 0) and builds nothing. The
//! step draws nothing from the RNG.

use crate::combat::clamp_i32;
use crate::ids::{PlayerId, Tile};
use crate::map::Map;
use crate::state::{SimEvent, State, Unit};
use crate::tech;
use crate::territory::TerritoryField;
use rules::modifiers::StatPath;
use rules::{MAX_HARRYING_LEVEL, Rules};

/// What attrition does to a unit where it stands now: whose borders bite,
/// how often and how hard ([`crate::SimView::attrition`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exposure {
    /// The territory owner whose Harrying the unit is exposed to.
    pub owner: PlayerId,
    /// Ticks between pulses at the owner's Harrying level.
    pub interval_ticks: u32,
    /// Hit points each pulse takes.
    pub hp: i32,
}

/// Every player's Supply Wain tiles, in `UnitId` order, and the squared
/// supply radius. Built from the units whenever it is needed and never
/// stored, so it can never be stale and a restore has nothing to rebuild.
pub(crate) struct SupplyGrid {
    /// `[player]`: the tiles the player's supplying units stand on.
    wains: Vec<Vec<Tile>>,
    /// `supply_radius_tiles²` (at most 64², validated).
    r2: i64,
}

impl SupplyGrid {
    /// Collect the units whose kind `supplies`, by their current tile.
    pub(crate) fn new(state: &State, rules: &Rules) -> SupplyGrid {
        let mut wains = vec![Vec::new(); state.players.len()];
        for u in state.units.values() {
            let supplies = rules.unit_kind(u.kind.0).is_some_and(|k| k.supplies);
            if supplies && let Some(tiles) = wains.get_mut(usize::from(u.owner.0)) {
                tiles.push(state.map.tile_of(u.pos));
            }
        }
        let r = i64::from(rules.supply_radius_tiles);
        SupplyGrid { wains, r2: r * r }
    }

    /// `p`'s Supply Wain tiles in `UnitId` order (empty for a player the
    /// state does not have).
    pub(crate) fn wains(&self, p: PlayerId) -> &[Tile] {
        self.wains.get(usize::from(p.0)).map_or(&[], Vec::as_slice)
    }

    /// Whether one of `p`'s Supply Wains covers tile `t`:
    /// `dx² + dy² <= supply_radius_tiles²` from the Wain's tile, in `i64`.
    pub(crate) fn covers(&self, p: PlayerId, t: Tile) -> bool {
        self.wains(p).iter().any(|w| {
            let dx = i64::from(t.x) - i64::from(w.x);
            let dy = i64::from(t.y) - i64::from(w.y);
            dx * dx + dy * dy <= self.r2
        })
    }
}

/// `p`'s Harrying level: the `HarryingLevel` rows from base 0, clamped to
/// `0..=MAX_HARRYING_LEVEL` (data-check keeps the worst case within).
pub(crate) fn harrying_level(state: &State, rules: &Rules, p: PlayerId) -> u32 {
    let v = tech::stat(state, rules, p, &StatPath::HarryingLevel, 0);
    u32::try_from(v.clamp(0, i64::from(MAX_HARRYING_LEVEL))).expect("clamped")
}

/// Ticks between pulses at Harrying level `level`: `None` at level 0 (no
/// attrition), else `max(1, attrition_interval_ticks >> (level - 1))`.
pub(crate) fn interval_ticks(rules: &Rules, level: u32) -> Option<u32> {
    let shift = level.checked_sub(1)?;
    let halved = rules
        .attrition_interval_ticks()
        .checked_shr(shift)
        .unwrap_or(0);
    Some(halved.max(1))
}

/// Hit points one pulse takes from an intruder in age `intruder_age` on
/// land of an owner in age `owner_age`: the age-gap table's row for
/// `max(0, owner_age - intruder_age)` (the last row covers larger gaps),
/// plus `harrying_bonus_hp` once the owner has reached
/// `harrying_bonus_age`. At most 2000 (validated).
pub(crate) fn pulse_hp(rules: &Rules, owner_age: u8, intruder_age: u8) -> i32 {
    let gap = owner_age.saturating_sub(intruder_age);
    let mut hp = rules.attrition_pulse_hp(gap);
    if rules
        .harrying_bonus_age_index()
        .is_some_and(|bonus| owner_age >= bonus)
    {
        hp = hp.saturating_add(rules.harrying_bonus_hp);
    }
    i32::try_from(hp).expect("data-check bounds the pulse")
}

/// The pulse of an owner whose interval is `interval` (`None` with no
/// Harrying level) and whose age is `owner_age` against an intruder in
/// `intruder_age`: `(interval_ticks, hp)`.
fn pulse_of(
    rules: &Rules,
    interval: Option<u32>,
    owner_age: u8,
    intruder_age: u8,
) -> Option<(u32, i32)> {
    interval.map(|i| (i, pulse_hp(rules, owner_age, intruder_age)))
}

/// What a pulse on `owner`'s land does to `intruder`'s units now:
/// `(interval_ticks, hp)`, or `None` while `owner` has no Harrying level.
/// [`step`] builds its table with the same functions, through [`Pulses`].
///
/// # Panics
///
/// If either player does not exist.
pub(crate) fn pulse(
    state: &State,
    rules: &Rules,
    owner: PlayerId,
    intruder: PlayerId,
) -> Option<(u32, i32)> {
    let interval = interval_ticks(rules, harrying_level(state, rules, owner));
    let age = |p: PlayerId| state.tech.player(p).age;
    pulse_of(rules, interval, age(owner), age(intruder))
}

/// The step's tables: the pulse for every `(owner, intruder)` pair, which
/// kinds are immune, and the supply grid. Built once per step (and once
/// per [`crate::SimView::attrition`] call).
pub(crate) struct Pulses {
    /// Number of players.
    players: usize,
    /// `[owner * players + intruder]`.
    table: Vec<Option<(u32, i32)>>,
    /// `[kind]`: `immune_to_attrition`.
    immune: Vec<bool>,
    supply: SupplyGrid,
}

impl Pulses {
    /// The tables for the current state; `None` when no player has a
    /// Harrying level, so nobody can be exposed.
    pub(crate) fn new(state: &State, rules: &Rules) -> Option<Pulses> {
        // Each player's interval and age, resolved once.
        let per: Vec<(Option<u32>, u8)> = state
            .players
            .iter()
            .map(|p| {
                (
                    interval_ticks(rules, harrying_level(state, rules, p.id)),
                    state.tech.player(p.id).age,
                )
            })
            .collect();
        if per.iter().all(|(interval, _)| interval.is_none()) {
            return None;
        }
        let players = per.len();
        let mut table = Vec::with_capacity(players * players);
        for &(interval, owner_age) in &per {
            for &(_, intruder_age) in &per {
                table.push(pulse_of(rules, interval, owner_age, intruder_age));
            }
        }
        Some(Pulses {
            players,
            table,
            immune: rules.units.iter().map(|k| k.immune_to_attrition).collect(),
            supply: SupplyGrid::new(state, rules),
        })
    }

    /// Whether and how `u` is exposed where it stands: its kind is not
    /// immune, its tile belongs to another player whose Harrying level is
    /// at least 1, and none of its owner's Supply Wains covers the tile.
    pub(crate) fn exposure(
        &self,
        map: &Map,
        territory: &TerritoryField,
        u: &Unit,
    ) -> Option<Exposure> {
        if self
            .immune
            .get(usize::from(u.kind.0))
            .copied()
            .unwrap_or(true)
        {
            return None;
        }
        let tile = map.tile_of(u.pos);
        let owner = territory.owner_at(tile)?;
        if owner == u.owner {
            return None;
        }
        let (o, i) = (usize::from(owner.0), usize::from(u.owner.0));
        if o >= self.players || i >= self.players {
            return None;
        }
        let (interval_ticks, hp) = self.table[o * self.players + i]?;
        if self.supply.covers(u.owner, tile) {
            return None;
        }
        Some(Exposure {
            owner,
            interval_ticks,
            hp,
        })
    }
}

/// Step 5b': advance every unit's exposure counter in `UnitId` order and
/// land the pulses that fall due (see the module docs).
pub(crate) fn step(state: &mut State, rules: &Rules, events: &mut Vec<SimEvent>) {
    let Some(pulses) = Pulses::new(state, rules) else {
        // No player has a Harrying level: nobody is exposed.
        for u in state.units.values_mut() {
            u.exposed_ticks = 0;
        }
        return;
    };
    let State {
        units,
        map,
        territory,
        ..
    } = state;
    for u in units.values_mut() {
        let Some(e) = pulses.exposure(map, territory, u) else {
            u.exposed_ticks = 0;
            continue;
        };
        u.exposed_ticks = u.exposed_ticks.saturating_add(1);
        if u.exposed_ticks.is_multiple_of(e.interval_ticks) {
            u.damage_taken = clamp_i32(i64::from(u.damage_taken) + i64::from(e.hp));
            events.push(SimEvent::Attrition {
                unit: u.id,
                damage: e.hp,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn rules() -> Rules {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap()
    }

    /// The tiles a Wain on `w` covers on a `size` x `size` map.
    fn covered(grid: &SupplyGrid, p: PlayerId, size: u16) -> Vec<Tile> {
        (0..size)
            .flat_map(|y| (0..size).map(move |x| Tile::new(x, y)))
            .filter(|t| grid.covers(p, *t))
            .collect()
    }

    /// A supply disc of radius 14 holds 613 tiles (`dx² + dy² <= 196`),
    /// not the "about 617" the design draft estimated.
    #[test]
    fn supply_disc_has_613_tiles() {
        let grid = SupplyGrid {
            wains: vec![vec![Tile::new(40, 40)]],
            r2: 14 * 14,
        };
        assert_eq!(covered(&grid, PlayerId(0), 128).len(), 613);
        assert_eq!(rules().supply_radius_tiles, 14, "the shipped radius");
    }

    /// The grid covers exactly the union of the discs that the design's
    /// raster stamps (attrition.md, "SupplyGrid"): one byte per tile set
    /// for every tile within the radius of each Wain, clipped to the map,
    /// for Wains in the middle, on an edge and in a corner, overlapping.
    /// Another player's Wains cover nothing for this player.
    #[test]
    fn supply_grid_matches_a_stamped_disc() {
        let size = 128u16;
        let r = 14i32;
        let wains = vec![
            Tile::new(40, 40),
            Tile::new(50, 44),
            Tile::new(0, 70),
            Tile::new(127, 127),
        ];
        let grid = SupplyGrid {
            wains: vec![wains.clone(), vec![Tile::new(90, 20)]],
            r2: i64::from(r * r),
        };
        let mut stamped = vec![0u8; usize::from(size) * usize::from(size)];
        for w in &wains {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx * dx + dy * dy > r * r {
                        continue;
                    }
                    let (x, y) = (i32::from(w.x) + dx, i32::from(w.y) + dy);
                    if (0..i32::from(size)).contains(&x) && (0..i32::from(size)).contains(&y) {
                        let i = usize::try_from(y * i32::from(size) + x).unwrap();
                        stamped[i] = 1;
                    }
                }
            }
        }
        let want: Vec<Tile> = (0..size)
            .flat_map(|y| (0..size).map(move |x| Tile::new(x, y)))
            .filter(|t| stamped[usize::from(t.y) * usize::from(size) + usize::from(t.x)] == 1)
            .collect();
        assert_eq!(covered(&grid, PlayerId(0), size), want);
        assert!(!grid.covers(PlayerId(0), Tile::new(90, 20)));
        assert!(grid.covers(PlayerId(1), Tile::new(90, 20)));
        assert!(!grid.covers(PlayerId(1), Tile::new(40, 40)));
        assert!(
            !grid.covers(PlayerId(2), Tile::new(40, 40)),
            "no such player"
        );
    }

    /// 64 ticks at Harrying I, halved per level, never 0 at any level up
    /// to the maximum, whatever the base interval; `None` at level 0. The
    /// shipped 32 ds is set in memory, so a data-only edit of
    /// `attrition_interval_ds` (the worked example in
    /// `data/rules/README.md`) leaves this test alone; the rules crate's
    /// `loads_repo_data` pins the shipped value.
    #[test]
    fn interval_halves_per_level_and_never_reaches_zero() {
        let mut r = rules();
        r.attrition_interval_ds = 32;
        assert_eq!(r.attrition_interval_ticks(), 64);
        assert_eq!(interval_ticks(&r, 0), None);
        let got: Vec<u32> = (1..=MAX_HARRYING_LEVEL)
            .map(|l| interval_ticks(&r, l).unwrap())
            .collect();
        assert_eq!(got, [64, 32, 16, 8, 4, 2, 1, 1]);
        for ds in [1, 2, 3, 5, 32, 33] {
            r.attrition_interval_ds = ds;
            for l in 1..=MAX_HARRYING_LEVEL + 40 {
                let i = interval_ticks(&r, l).unwrap();
                assert!(i >= 1, "{ds} ds at level {l}");
                if l >= 2 {
                    assert!(i <= interval_ticks(&r, l - 1).unwrap());
                }
            }
        }
    }

    /// The gap is the owner's age minus the intruder's, floored at 0, and
    /// clamps to the table's last row; the bonus applies from its age on.
    #[test]
    fn age_gap_clamps_to_the_last_row() {
        let mut r = rules();
        assert_eq!(r.attrition_age_gap_hp, [1, 2]);
        assert_eq!(r.harrying_bonus_hp, 0);
        let table = |r: &Rules| {
            (0..3u8)
                .flat_map(|o| (0..3u8).map(move |i| (o, i)))
                .map(|(o, i)| pulse_hp(r, o, i))
                .collect::<Vec<i32>>()
        };
        // owner 0 vs 0, 1, 2; owner 1 vs ...; owner 2 vs ...
        assert_eq!(table(&r), [1, 1, 1, 2, 1, 1, 2, 2, 1]);
        r.harrying_bonus_hp = 3;
        assert_eq!(r.harrying_bonus_age_index(), Some(2));
        assert_eq!(table(&r), [1, 1, 1, 2, 1, 1, 5, 5, 4]);
        r.attrition_age_gap_hp = vec![1, 2, 7];
        r.harrying_bonus_age = None;
        assert_eq!(table(&r), [1, 1, 1, 2, 1, 1, 7, 2, 1]);
    }
}
