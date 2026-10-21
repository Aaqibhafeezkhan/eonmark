//! The territory field: Towns and Watchtowers project integer disc kernels
//! into per-owner strength layers, and each tile belongs to the single
//! strongest owner (docs/design/territory.md).
//!
//! A source is `(owner, centre tile, R)`. It stamps
//! `K_R(dx, dy) = max(0, R - isqrt(dx² + dy²))` into its owner's `u16`
//! layer; the neutral pseudo-owner (a Town under annexation, M5a) has a
//! layer of its own. A tile's owner is a pure function of its layer values:
//!
//! ```text
//! best  = max over every layer (neutral included)
//! count = layers equal to best
//! owner = neutral if best == 0, count >= 2 or the neutral layer is the unique max
//!         otherwise the player of the unique max
//! ```
//!
//! An exact tie is neutral; nothing reads the previous owner (no hysteresis,
//! `IMPLEMENTER_NOTES` rule 7). Layer values are exact integer sums, so any
//! sequence of [`TerritoryField::add_source`], [`TerritoryField::remove_source`]
//! and [`TerritoryField::change_source`] leaves the field bit-identical to
//! [`TerritoryField::full_recompute`] over the same source set: the layers
//! hold the sum over the current sources whatever the history, every edit
//! re-evaluates the owner rule over the box its kernel touches, and every
//! other tile keeps both its layers and its owner.
//!
//! Layer 0 is the neutral layer and layer `p + 1` belongs to `PlayerId(p)`,
//! so a tile's owner code (0 neutral, otherwise player index + 1, the R
//! channel of the M3b field texture) is the index of the layer that won it.
//!
//! The field is hashed state with no derived member, so `restore()` has
//! nothing to rebuild. Kernels are computed per stamp rather than cached: a
//! stamp at the largest radius is a few thousand integer square roots. The
//! grids serialise as one byte string each, which keeps the per-tick state
//! hash cheap. Deserialising checks the field against a full recompute of its
//! sources, so a corrupt snapshot is a decode error, not a later panic.

use crate::ids::{BuildingId, PlayerId, Tile};
use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

/// Which layer a source stamps into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SourceOwner {
    /// A player's layer.
    Player(PlayerId),
    /// The neutral pseudo-owner (a Town whose annexation timer runs, M5a).
    /// Where it wins, the tile is neutral.
    Neutral,
}

/// A border source: a Town or a Watchtower.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// The layer the kernel goes into.
    pub owner: SourceOwner,
    /// Kernel centre (a building's footprint centre tile).
    pub centre: Tile,
    /// Kernel radius `R` in tiles: the kernel is positive where
    /// `dx² + dy² < R²`. Zero projects nothing.
    pub radius: u16,
}

/// Kernel value at offset `(dx, dy)` from a source of radius `radius`:
/// `max(0, R - isqrt(dx² + dy²))` with the floored integer square root.
pub fn kernel(radius: u16, dx: i32, dy: i32) -> u16 {
    let d2 = u64::from(dx.unsigned_abs()).pow(2) + u64::from(dy.unsigned_abs()).pow(2);
    let d = d2.isqrt();
    let r = u64::from(radius);
    if d < r {
        u16::try_from(r - d).expect("kernel value is below the radius")
    } else {
        0
    }
}

/// A `u16` per tile, serialised as one little-endian byte string.
#[derive(Debug, Clone, PartialEq, Eq)]
struct U16Grid(Vec<u16>);

impl Serialize for U16Grid {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let bytes: Vec<u8> = self.0.iter().flat_map(|v| v.to_le_bytes()).collect();
        s.serialize_bytes(&bytes)
    }
}

impl<'de> Deserialize<'de> for U16Grid {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let bytes = d.deserialize_bytes(BytesVisitor)?;
        if bytes.len() % 2 != 0 {
            return Err(de::Error::invalid_length(
                bytes.len(),
                &"an even byte count",
            ));
        }
        Ok(U16Grid(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| u16::from_le_bytes(c))
                .collect(),
        ))
    }
}

/// A `u8` per tile, serialised as one byte string.
#[derive(Debug, Clone, PartialEq, Eq)]
struct U8Grid(Vec<u8>);

impl Serialize for U8Grid {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for U8Grid {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(U8Grid(d.deserialize_bytes(BytesVisitor)?))
    }
}

/// Accepts a byte string in any of serde's representations.
struct BytesVisitor;

impl<'de> Visitor<'de> for BytesVisitor {
    type Value = Vec<u8>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a byte string")
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Vec<u8>, E> {
        Ok(v.to_vec())
    }

    fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<Vec<u8>, E> {
        Ok(v)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u8>, A::Error> {
        let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(b) = seq.next_element()? {
            out.push(b);
        }
        Ok(out)
    }
}

/// The territory field for one match. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawTerritoryField")]
pub struct TerritoryField {
    width: u16,
    height: u16,
    players: u8,
    /// Every source, iterated in `BuildingId` order.
    sources: BTreeMap<BuildingId, Source>,
    /// `players + 1` layers: index 0 is neutral, index `p + 1` is `PlayerId(p)`.
    layers: Vec<U16Grid>,
    /// Owner code per tile: 0 neutral, otherwise player index + 1.
    owner: U8Grid,
}

/// The serialised form, checked by `TryFrom` before it becomes a field.
#[derive(Deserialize)]
struct RawTerritoryField {
    width: u16,
    height: u16,
    players: u8,
    sources: BTreeMap<BuildingId, Source>,
    layers: Vec<U16Grid>,
    owner: U8Grid,
}

/// A serialised field that is not a consistent field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidTerritoryField(&'static str);

impl fmt::Display for InvalidTerritoryField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid territory field: {}", self.0)
    }
}

impl std::error::Error for InvalidTerritoryField {}

impl TryFrom<RawTerritoryField> for TerritoryField {
    type Error = InvalidTerritoryField;

    fn try_from(raw: RawTerritoryField) -> Result<Self, Self::Error> {
        let n = usize::from(raw.width) * usize::from(raw.height);
        if raw.players == u8::MAX {
            return Err(InvalidTerritoryField("too many players"));
        }
        if raw.layers.len() != usize::from(raw.players) + 1 {
            return Err(InvalidTerritoryField("layer count is not players + 1"));
        }
        if raw.layers.iter().any(|l| l.0.len() != n) || raw.owner.0.len() != n {
            return Err(InvalidTerritoryField("grid size does not match the field"));
        }
        let bad_source = raw.sources.values().any(|s| {
            s.centre.x >= raw.width
                || s.centre.y >= raw.height
                || matches!(s.owner, SourceOwner::Player(p) if p.0 >= raw.players)
        });
        if bad_source {
            return Err(InvalidTerritoryField(
                "a source lies outside the field or names no layer",
            ));
        }
        let field = TerritoryField {
            width: raw.width,
            height: raw.height,
            players: raw.players,
            sources: raw.sources,
            layers: raw.layers,
            owner: raw.owner,
        };
        let mut check = TerritoryField::new(field.width, field.height, field.players);
        for (&id, &src) in &field.sources {
            check.add_source(id, src);
        }
        if check != field {
            return Err(InvalidTerritoryField(
                "layers or owners disagree with the sources",
            ));
        }
        Ok(field)
    }
}

/// Inclusive tile rectangle `(x0, y0, x1, y1)`.
type Rect = (u16, u16, u16, u16);

impl TerritoryField {
    /// An empty field: every tile neutral, no sources.
    pub fn new(width: u16, height: u16, players: u8) -> TerritoryField {
        assert!(
            players < u8::MAX,
            "owner codes need player index + 1 to fit a u8"
        );
        let n = usize::from(width) * usize::from(height);
        TerritoryField {
            width,
            height,
            players,
            sources: BTreeMap::new(),
            layers: (0..=players).map(|_| U16Grid(vec![0; n])).collect(),
            owner: U8Grid(vec![0; n]),
        }
    }

    /// Field width in tiles.
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Field height in tiles.
    pub fn height(&self) -> u16 {
        self.height
    }

    /// Number of player layers (the neutral layer is extra).
    pub fn players(&self) -> u8 {
        self.players
    }

    /// Every source in `BuildingId` order.
    pub fn sources(&self) -> &BTreeMap<BuildingId, Source> {
        &self.sources
    }

    /// Add a source under a new id. Panics when the id is already present
    /// (ids are never reused) or the source names a player the field does
    /// not have.
    pub fn add_source(&mut self, id: BuildingId, src: Source) {
        assert!(
            !self.sources.contains_key(&id),
            "territory source {id:?} already present"
        );
        self.stamp(src, true);
        self.sources.insert(id, src);
        self.recompute_owner(self.kernel_rect(src));
    }

    /// Remove a source; returns it, or `None` when the id has none.
    pub fn remove_source(&mut self, id: BuildingId) -> Option<Source> {
        let src = self.sources.remove(&id)?;
        self.stamp(src, false);
        self.recompute_owner(self.kernel_rect(src));
        Some(src)
    }

    /// Replace an existing source (a tier change, a Statecraft level, an
    /// owner moving to or from the neutral layer); returns the old one.
    /// Panics when the id has no source.
    pub fn change_source(&mut self, id: BuildingId, src: Source) -> Source {
        let old = self
            .remove_source(id)
            .unwrap_or_else(|| panic!("territory source {id:?} not present"));
        self.add_source(id, src);
        old
    }

    /// Rebuild every layer and the owner grid from the source list, adding
    /// sources in `BuildingId` order. The result equals the incrementally
    /// maintained field bit for bit (see the module docs).
    pub fn full_recompute(&mut self) {
        for layer in &mut self.layers {
            layer.0.fill(0);
        }
        let sources: Vec<Source> = self.sources.values().copied().collect();
        for src in sources {
            self.stamp(src, true);
        }
        let whole =
            (self.width > 0 && self.height > 0).then(|| (0, 0, self.width - 1, self.height - 1));
        self.recompute_owner(whole);
    }

    /// A copy rebuilt with [`TerritoryField::full_recompute`].
    pub fn recomputed(&self) -> TerritoryField {
        let mut f = self.clone();
        f.full_recompute();
        f
    }

    /// The tile's owner, `None` when neutral. Panics outside the field.
    pub fn owner_at(&self, t: Tile) -> Option<PlayerId> {
        match self.owner_code(t) {
            0 => None,
            c => Some(PlayerId(c - 1)),
        }
    }

    /// The tile's owner code: 0 neutral, otherwise player index + 1 (the R
    /// channel of the M3b field texture). Panics outside the field.
    pub fn owner_code(&self, t: Tile) -> u8 {
        self.owner.0[self.idx(t)]
    }

    /// Owner codes for every tile, row-major.
    pub fn owner_grid(&self) -> &[u8] {
        &self.owner.0
    }

    /// `true` when the player owns the tile. Terrain and occupancy are
    /// checked separately by construction.
    pub fn is_buildable(&self, player: PlayerId, t: Tile) -> bool {
        self.in_bounds(t) && self.owner_at(t) == Some(player)
    }

    /// Tiles the player owns (the HUD and the 45-minute tiebreak).
    pub fn owned_tile_count(&self, player: PlayerId) -> u32 {
        let code = player.0 + 1;
        let n = self.owner.0.iter().filter(|&&c| c == code).count();
        u32::try_from(n).expect("tile count fits u32")
    }

    /// The layer value at a tile. Panics outside the field or for a player
    /// the field does not have.
    pub fn strength_at(&self, owner: SourceOwner, t: Tile) -> u16 {
        self.layers[self.layer_index(owner)].0[self.idx(t)]
    }

    /// One owner's layer, row-major (the G channel of the M3b field
    /// texture reads the owning layer's value). Panics for a player the
    /// field does not have.
    pub fn layer(&self, owner: SourceOwner) -> &[u16] {
        &self.layers[self.layer_index(owner)].0
    }

    fn in_bounds(&self, t: Tile) -> bool {
        t.x < self.width && t.y < self.height
    }

    fn idx(&self, t: Tile) -> usize {
        assert!(self.in_bounds(t), "tile ({}, {}) outside field", t.x, t.y);
        usize::from(t.y) * usize::from(self.width) + usize::from(t.x)
    }

    fn layer_index(&self, owner: SourceOwner) -> usize {
        match owner {
            SourceOwner::Player(p) => {
                assert!(p.0 < self.players, "player {p:?} has no layer");
                usize::from(p.0) + 1
            }
            SourceOwner::Neutral => 0,
        }
    }

    /// Tiles where the kernel can be positive (`|dx|, |dy| < R`), clipped
    /// to the field. `None` for a zero radius or a box entirely outside.
    /// Adding and removing a source go through this one function, so they
    /// touch exactly the same tiles.
    fn kernel_rect(&self, src: Source) -> Option<Rect> {
        if src.radius == 0 {
            return None;
        }
        let reach = i32::from(src.radius) - 1;
        let clip = |c: u16, size: u16| -> Option<(u16, u16)> {
            let lo = (i32::from(c) - reach).max(0);
            let hi = (i32::from(c) + reach).min(i32::from(size) - 1);
            (lo <= hi).then(|| {
                (
                    u16::try_from(lo).expect("clipped to the field"),
                    u16::try_from(hi).expect("clipped to the field"),
                )
            })
        };
        let (x0, x1) = clip(src.centre.x, self.width)?;
        let (y0, y1) = clip(src.centre.y, self.height)?;
        Some((x0, y0, x1, y1))
    }

    /// Add (`add == true`) or subtract the source's kernel from its layer.
    /// Checked arithmetic: an overflow or an underflow means the source set
    /// is inconsistent, which is a bug, so it panics instead of wrapping.
    fn stamp(&mut self, src: Source, add: bool) {
        let Some((x0, y0, x1, y1)) = self.kernel_rect(src) else {
            return;
        };
        let li = self.layer_index(src.owner);
        let width = usize::from(self.width);
        let layer = &mut self.layers[li].0;
        for y in y0..=y1 {
            let dy = i32::from(y) - i32::from(src.centre.y);
            for x in x0..=x1 {
                let k = kernel(src.radius, i32::from(x) - i32::from(src.centre.x), dy);
                if k == 0 {
                    continue;
                }
                let cell = &mut layer[usize::from(y) * width + usize::from(x)];
                *cell = if add {
                    cell.checked_add(k).expect("territory strength overflow")
                } else {
                    cell.checked_sub(k).expect("territory strength underflow")
                };
            }
        }
    }

    /// Re-evaluate the owner rule over a rectangle.
    fn recompute_owner(&mut self, rect: Option<Rect>) {
        let Some((x0, y0, x1, y1)) = rect else {
            return;
        };
        let width = usize::from(self.width);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let i = usize::from(y) * width + usize::from(x);
                let mut best = 0u16;
                let mut count = 0u32;
                let mut arg = 0usize;
                for (l, layer) in self.layers.iter().enumerate() {
                    let v = layer.0[i];
                    if v > best {
                        best = v;
                        count = 1;
                        arg = l;
                    } else if v == best {
                        count += 1;
                    }
                }
                // Layer 0 is neutral, so a unique neutral maximum also gives 0.
                self.owner.0[i] = if best == 0 || count >= 2 {
                    0
                } else {
                    u8::try_from(arg).expect("fewer than 255 layers")
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn town(p: u8, x: u16, y: u16, r: u16) -> Source {
        Source {
            owner: SourceOwner::Player(PlayerId(p)),
            centre: Tile::new(x, y),
            radius: r,
        }
    }

    /// Tiles with a positive kernel and the kernel's sum, by brute force
    /// over a box wider than any kernel.
    fn disc(r: u16) -> (u32, u32) {
        let mut tiles = 0;
        let mut sum = 0;
        for dy in -40..=40 {
            for dx in -40..=40 {
                let k = u32::from(kernel(r, dx, dy));
                if k > 0 {
                    tiles += 1;
                    sum += k;
                }
            }
        }
        (tiles, sum)
    }

    #[test]
    fn territory_kernel_is_the_floored_disc() {
        assert_eq!(kernel(20, 0, 0), 20);
        assert_eq!(kernel(20, 3, 4), 15);
        // isqrt(1 + 1) = 1: the diagonal neighbour is one step out.
        assert_eq!(kernel(2, 1, 1), 1);
        assert_eq!(kernel(2, 2, 0), 0);
        assert_eq!(kernel(0, 0, 0), 0);
        assert_eq!(kernel(24, -24, 0), 0);
        assert_eq!(kernel(24, -23, 0), 1);
        // Positive exactly where dx² + dy² < R².
        for r in [1u16, 2, 5, 20, 24, 27] {
            for dy in -30i32..=30 {
                for dx in -30i32..=30 {
                    let inside = dx * dx + dy * dy < i32::from(r) * i32::from(r);
                    assert_eq!(kernel(r, dx, dy) > 0, inside, "r={r} ({dx},{dy})");
                }
            }
        }
        // Tile counts and sums for the radii M3a uses (Watchtower 2, Town
        // tiers 20 and 24, the largest with Statecraft 27).
        assert_eq!(disc(2), (9, 10));
        assert_eq!(disc(20), (1245, 8840));
        assert_eq!(disc(24), (1789, 15160));
        assert_eq!(disc(27), (2285, 21495));
    }

    #[test]
    fn territory_single_source_owns_its_disc_and_removal_restores_empty() {
        let empty = TerritoryField::new(64, 64, 2);
        let mut f = empty.clone();
        f.add_source(BuildingId(1), town(0, 32, 32, 20));
        assert_eq!(f.owned_tile_count(PlayerId(0)), 1245);
        assert_eq!(f.owned_tile_count(PlayerId(1)), 0);
        assert_eq!(f.owner_at(Tile::new(32, 32)), Some(PlayerId(0)));
        assert_eq!(
            f.strength_at(SourceOwner::Player(PlayerId(0)), Tile::new(32, 32)),
            20
        );
        assert!(f.is_buildable(PlayerId(0), Tile::new(40, 32)));
        assert!(!f.is_buildable(PlayerId(1), Tile::new(40, 32)));
        assert!(
            !f.is_buildable(PlayerId(0), Tile::new(52, 32)),
            "d = 20 is outside"
        );
        assert!(
            !f.is_buildable(PlayerId(0), Tile::new(64, 0)),
            "off the field"
        );
        assert_eq!(f.remove_source(BuildingId(1)), Some(town(0, 32, 32, 20)));
        assert_eq!(f, empty);
        assert_eq!(f.remove_source(BuildingId(1)), None);
    }

    #[test]
    fn territory_kernels_clip_at_the_edges_and_corners() {
        let mut f = TerritoryField::new(64, 48, 2);
        f.add_source(BuildingId(1), town(0, 0, 0, 20));
        f.add_source(BuildingId(2), town(1, 63, 47, 24));
        f.add_source(BuildingId(3), town(0, 63, 0, 2));
        assert_eq!(f, f.recomputed());
        // A quarter disc at the origin corner: dx, dy in 0..20.
        let quarter = (0..20i32)
            .flat_map(|dy| (0..20i32).map(move |dx| (dx, dy)))
            .filter(|&(dx, dy)| kernel(20, dx, dy) > 0)
            .count();
        let p0_near_origin = (0..20u16)
            .flat_map(|y| (0..20u16).map(move |x| Tile::new(x, y)))
            .filter(|&t| f.owner_at(t) == Some(PlayerId(0)))
            .count();
        assert_eq!(p0_near_origin, quarter);
        for id in 1..=3 {
            f.remove_source(BuildingId(id));
        }
        assert_eq!(f, TerritoryField::new(64, 48, 2));
    }

    #[test]
    fn territory_exact_tie_is_neutral_without_hysteresis() {
        // Odd width: column 32 is exactly equidistant from x = 22 and x = 42.
        let mut f = TerritoryField::new(65, 65, 2);
        f.add_source(BuildingId(1), town(0, 22, 32, 20));
        f.add_source(BuildingId(2), town(1, 42, 32, 20));
        for y in 0..65 {
            let t = Tile::new(32, y);
            assert_eq!(f.owner_at(t), None, "midline tile {t:?}");
            if f.strength_at(SourceOwner::Player(PlayerId(0)), t) > 0 {
                assert_eq!(
                    f.strength_at(SourceOwner::Player(PlayerId(0)), t),
                    f.strength_at(SourceOwner::Player(PlayerId(1)), t)
                );
            }
        }
        assert_eq!(f.owner_at(Tile::new(31, 32)), Some(PlayerId(0)));
        assert_eq!(f.owner_at(Tile::new(33, 32)), Some(PlayerId(1)));
        // The order sources arrive in never matters: no incumbent keeps a tie.
        let mut g = TerritoryField::new(65, 65, 2);
        g.add_source(BuildingId(2), town(1, 42, 32, 20));
        g.add_source(BuildingId(1), town(0, 22, 32, 20));
        assert_eq!(f, g);
    }

    #[test]
    fn territory_neutral_layer_wins_as_neutral_and_ties_three_ways() {
        let mut f = TerritoryField::new(32, 32, 2);
        let mut annexing = town(0, 16, 16, 10);
        f.add_source(BuildingId(1), annexing);
        assert_eq!(f.owner_at(Tile::new(16, 16)), Some(PlayerId(0)));
        // The Town's kernel moves to the neutral layer while annexation runs.
        annexing.owner = SourceOwner::Neutral;
        f.change_source(BuildingId(1), annexing);
        assert_eq!(f.owner_at(Tile::new(16, 16)), None);
        assert_eq!(f.owned_tile_count(PlayerId(0)), 0);
        assert_eq!(f.strength_at(SourceOwner::Neutral, Tile::new(16, 16)), 10);
        // A stronger player kernel still wins over the neutral one.
        f.add_source(BuildingId(2), town(1, 16, 16, 12));
        assert_eq!(f.owner_at(Tile::new(16, 16)), Some(PlayerId(1)));
        // Equal to the neutral layer: neutral.
        f.change_source(BuildingId(2), town(1, 16, 16, 10));
        assert_eq!(f.owner_at(Tile::new(16, 16)), None);
        assert_eq!(f, f.recomputed());
    }

    #[test]
    fn territory_serialises_compactly_and_round_trips() {
        let mut f = TerritoryField::new(128, 128, 2);
        f.add_source(BuildingId(4), town(0, 24, 64, 20));
        f.add_source(BuildingId(9), town(1, 103, 64, 24));
        let bytes = postcard::to_allocvec(&f).unwrap();
        // Three u16 layers and one u8 owner grid as byte strings, plus the
        // sources and a few headers: no per-cell varints.
        let cells = 128 * 128;
        assert!(bytes.len() < cells * 7 + 256, "{} bytes", bytes.len());
        let back: TerritoryField = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, f);
        assert_eq!(postcard::to_allocvec(&back).unwrap(), bytes);
    }

    #[test]
    fn territory_deserialising_rejects_an_inconsistent_field() {
        let mut f = TerritoryField::new(32, 32, 2);
        f.add_source(BuildingId(1), town(0, 8, 8, 6));
        f.add_source(BuildingId(2), town(1, 24, 24, 6));
        let good = postcard::to_allocvec(&f).unwrap();
        assert_eq!(postcard::from_bytes::<TerritoryField>(&good).unwrap(), f);
        // Drop a source from the serialised map but keep its stamped layer:
        // the layers no longer equal a recompute of the sources.
        let mut g = f.clone();
        g.sources.remove(&BuildingId(2));
        let bad = postcard::to_allocvec(&g).unwrap();
        let err = postcard::from_bytes::<TerritoryField>(&bad).unwrap_err();
        assert!(
            format!("{err}").contains("territory") || matches!(err, postcard::Error::SerdeDeCustom),
            "{err}"
        );
        // A flipped owner byte is caught the same way.
        let mut h = f.clone();
        h.owner.0[0] = 2;
        assert!(
            postcard::from_bytes::<TerritoryField>(&postcard::to_allocvec(&h).unwrap()).is_err()
        );
        // The layer accessor exposes the neutral layer at index 0.
        assert!(f.layer(SourceOwner::Neutral).iter().all(|&v| v == 0));
        assert_eq!(f.layer(SourceOwner::Player(PlayerId(1)))[24 * 32 + 24], 6);
    }

    #[test]
    #[should_panic(expected = "already present")]
    fn territory_duplicate_source_id_panics() {
        let mut f = TerritoryField::new(16, 16, 2);
        f.add_source(BuildingId(1), town(0, 8, 8, 2));
        f.add_source(BuildingId(1), town(1, 4, 4, 2));
    }
}
