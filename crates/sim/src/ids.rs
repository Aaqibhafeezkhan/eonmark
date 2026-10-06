//! Identifiers. Every id is monotonic and never reused, so sorting by id is a
//! total order that is stable across snapshot and restore.

use serde::{Deserialize, Serialize};

/// A player slot in a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlayerId(pub u8);

/// A unit. Allocated by [`IdGen::unit`]; `0` is never allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UnitId(pub u32);

/// A building or resource node. Allocated by [`IdGen::building`]; `0` is never allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BuildingId(pub u32);

/// Index into the unit table of `units.ron` (M3a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UnitKindId(pub u16);

/// Index into the building table of `buildings.ron` (M3a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BuildingKindId(pub u16);

/// Index into the tech table of `techs.ron` (M4a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TechId(pub u16);

/// Something an attack can be aimed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TargetId {
    /// A unit.
    Unit(UnitId),
    /// A building.
    Building(BuildingId),
}

/// A tile coordinate; `x` grows east, `y` grows south, `(0, 0)` is north-west.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Tile {
    /// Column.
    pub x: u16,
    /// Row.
    pub y: u16,
}

impl Tile {
    /// Construct a tile coordinate.
    pub const fn new(x: u16, y: u16) -> Tile {
        Tile { x, y }
    }
}

/// Monotonic id allocator; part of the hashed state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdGen {
    next_unit: u32,
    next_building: u32,
}

impl Default for IdGen {
    fn default() -> Self {
        IdGen {
            next_unit: 1,
            next_building: 1,
        }
    }
}

impl IdGen {
    /// Fresh allocator; the first ids handed out are `UnitId(1)` and `BuildingId(1)`.
    pub fn new() -> IdGen {
        IdGen::default()
    }

    /// Allocate the next unit id. Panics after `u32::MAX - 1` allocations.
    pub fn unit(&mut self) -> UnitId {
        let id = UnitId(self.next_unit);
        self.next_unit = self
            .next_unit
            .checked_add(1)
            .expect("UnitId space exhausted");
        id
    }

    /// Allocate the next building id. Panics after `u32::MAX - 1` allocations.
    pub fn building(&mut self) -> BuildingId {
        let id = BuildingId(self.next_building);
        self.next_building = self
            .next_building
            .checked_add(1)
            .expect("BuildingId space exhausted");
        id
    }

    /// Ids handed out so far: `(units, buildings)`.
    pub fn allocated(&self) -> (u32, u32) {
        (self.next_unit - 1, self.next_building - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_monotonic_and_start_at_one() {
        let mut g = IdGen::new();
        assert_eq!(g.unit(), UnitId(1));
        assert_eq!(g.unit(), UnitId(2));
        assert_eq!(g.building(), BuildingId(1));
        assert_eq!(g.allocated(), (2, 1));
        let bytes = postcard::to_allocvec(&g).unwrap();
        let mut back: IdGen = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(
            back.unit(),
            UnitId(3),
            "restore keeps the counter, never reuses ids"
        );
    }
}
