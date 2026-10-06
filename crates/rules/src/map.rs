//! Schema and loader for `data/maps/*.ron`.
//!
//! A map is a rectangle of terrain characters plus start positions. The file
//! stores one string per row so it can be read and diffed as a picture:
//!
//! | Char | Terrain | Passable | Resource |
//! |------|---------|----------|----------|
//! | `.`  | Grass   | yes      | none (buildable) |
//! | `f`  | Forest  | no       | Lumber (Lumber Yard beside it) |
//! | `m`  | Mountain| no       | Ore (Ore Pit beside it) |
//! | `~`  | Water   | no       | none |
//!
//! Row 0 is the north edge; column 0 is the west edge. `x` grows east and
//! `y` grows south, matching the sim's tile coordinates.

use crate::Error;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Terrain of one tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Terrain {
    /// Open ground; passable and buildable.
    Grass,
    /// Lumber source; impassable.
    Forest,
    /// Ore source; impassable.
    Mountain,
    /// Impassable; no resource.
    Water,
}

impl Terrain {
    /// Parse one map character.
    pub fn from_char(c: char) -> Option<Terrain> {
        match c {
            '.' => Some(Terrain::Grass),
            'f' => Some(Terrain::Forest),
            'm' => Some(Terrain::Mountain),
            '~' => Some(Terrain::Water),
            _ => None,
        }
    }

    /// The character used in map files.
    pub fn to_char(self) -> char {
        match self {
            Terrain::Grass => '.',
            Terrain::Forest => 'f',
            Terrain::Mountain => 'm',
            Terrain::Water => '~',
        }
    }

    /// Whether land units can enter the tile.
    pub fn passable(self) -> bool {
        matches!(self, Terrain::Grass)
    }
}

/// A tile coordinate on the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TilePos {
    /// Column, 0 at the west edge.
    pub x: u16,
    /// Row, 0 at the north edge.
    pub y: u16,
}

/// How the two halves of the map relate. Checked by the validator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Symmetry {
    /// Tile `(x, y)` equals tile `(width - 1 - x, y)`: mirrored across the
    /// vertical midline. The midline is between columns `width/2 - 1` and
    /// `width/2`.
    MirrorX,
    /// Tile `(x, y)` equals tile `(width - 1 - x, height - 1 - y)`.
    Rotate180,
}

impl Symmetry {
    /// The tile that must match `(x, y)` on a map of this size.
    pub fn partner(self, x: u16, y: u16, width: u16, height: u16) -> (u16, u16) {
        match self {
            Symmetry::MirrorX => (width - 1 - x, y),
            Symmetry::Rotate180 => (width - 1 - x, height - 1 - y),
        }
    }
}

/// On-disk schema. Converted to [`MapDef`] after validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MapFile {
    name: String,
    width: u16,
    height: u16,
    symmetry: Symmetry,
    starts: Vec<TilePos>,
    rows: Vec<String>,
}

/// A validated map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapDef {
    /// Map name; equals the file stem.
    pub name: String,
    /// Columns.
    pub width: u16,
    /// Rows.
    pub height: u16,
    /// Declared symmetry (verified).
    pub symmetry: Symmetry,
    /// Player start tiles in player order. Exactly two in v0.1.
    pub starts: Vec<TilePos>,
    /// Row-major terrain, `height * width` entries.
    tiles: Vec<Terrain>,
}

/// Number of player starts every map must have (2-player skirmish only).
pub const REQUIRED_STARTS: usize = 2;

impl MapDef {
    /// Load and validate one map file.
    pub fn load(path: &Path) -> Result<MapDef, Error> {
        let file: MapFile = crate::load_ron(path)?;
        MapDef::from_file(path, file)
    }

    /// Parse and validate map text; `path` is used only in error messages.
    pub fn parse(path: &Path, text: &str) -> Result<MapDef, Error> {
        let file: MapFile = crate::parse_ron(path, text)?;
        MapDef::from_file(path, file)
    }

    fn from_file(path: &Path, file: MapFile) -> Result<MapDef, Error> {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if !stem.is_empty() && file.name != stem {
            return Err(Error::invalid(
                path,
                "name",
                format!("{:?} must equal the file stem {stem:?}", file.name),
            ));
        }
        if file.width < 8 || file.height < 8 {
            return Err(Error::invalid(
                path,
                "width",
                "width and height must be >= 8",
            ));
        }
        if !file.width.is_multiple_of(2) {
            return Err(Error::invalid(
                path,
                "width",
                "must be even so the midline is between tiles",
            ));
        }
        if file.rows.len() != usize::from(file.height) {
            return Err(Error::invalid(
                path,
                "rows",
                format!("expected {} rows, found {}", file.height, file.rows.len()),
            ));
        }
        let mut tiles = Vec::with_capacity(usize::from(file.width) * usize::from(file.height));
        for (y, row) in file.rows.iter().enumerate() {
            let chars: Vec<char> = row.chars().collect();
            if chars.len() != usize::from(file.width) {
                return Err(Error::invalid(
                    path,
                    &format!("rows[{y}]"),
                    format!("expected {} characters, found {}", file.width, chars.len()),
                ));
            }
            for (x, c) in chars.iter().enumerate() {
                let Some(t) = Terrain::from_char(*c) else {
                    return Err(Error::invalid(
                        path,
                        &format!("rows[{y}]"),
                        format!("unknown terrain character {c:?} at column {x}"),
                    ));
                };
                tiles.push(t);
            }
        }
        let map = MapDef {
            name: file.name,
            width: file.width,
            height: file.height,
            symmetry: file.symmetry,
            starts: file.starts,
            tiles,
        };
        map.validate_starts(path)?;
        map.validate_symmetry(path)?;
        Ok(map)
    }

    fn validate_starts(&self, path: &Path) -> Result<(), Error> {
        if self.starts.len() != REQUIRED_STARTS {
            return Err(Error::invalid(
                path,
                "starts",
                format!(
                    "expected exactly {REQUIRED_STARTS} starts, found {}",
                    self.starts.len()
                ),
            ));
        }
        for (i, s) in self.starts.iter().enumerate() {
            let field = format!("starts[{i}]");
            if s.x >= self.width || s.y >= self.height {
                return Err(Error::invalid(
                    path,
                    &field,
                    format!(
                        "({}, {}) is outside the {}x{} map",
                        s.x, s.y, self.width, self.height
                    ),
                ));
            }
            if !self.tile(s.x, s.y).passable() {
                return Err(Error::invalid(path, &field, "must be on Grass"));
            }
            if self.starts[..i].contains(s) {
                return Err(Error::invalid(path, &field, "duplicates an earlier start"));
            }
        }
        let a = self.starts[0];
        let b = self.starts[1];
        let (px, py) = self.symmetry.partner(a.x, a.y, self.width, self.height);
        if (px, py) != (b.x, b.y) {
            return Err(Error::invalid(
                path,
                "starts[1]",
                format!(
                    "({}, {}) is not the {:?} partner of starts[0]; expected ({px}, {py})",
                    b.x, b.y, self.symmetry
                ),
            ));
        }
        Ok(())
    }

    fn validate_symmetry(&self, path: &Path) -> Result<(), Error> {
        for y in 0..self.height {
            for x in 0..self.width {
                let (px, py) = self.symmetry.partner(x, y, self.width, self.height);
                let a = self.tile(x, y);
                let b = self.tile(px, py);
                if a != b {
                    return Err(Error::invalid(
                        path,
                        &format!("rows[{y}]"),
                        format!(
                            "{:?} symmetry broken: column {x} is {:?} but its partner ({px}, {py}) is {:?}",
                            self.symmetry,
                            a.to_char(),
                            b.to_char()
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Terrain at `(x, y)`. Panics when out of range.
    pub fn tile(&self, x: u16, y: u16) -> Terrain {
        assert!(
            x < self.width && y < self.height,
            "tile ({x}, {y}) outside map"
        );
        self.tiles[usize::from(y) * usize::from(self.width) + usize::from(x)]
    }

    /// Row-major terrain slice, `height * width` entries.
    pub fn tiles(&self) -> &[Terrain] {
        &self.tiles
    }

    /// Count of tiles with the given terrain.
    pub fn count(&self, terrain: Terrain) -> usize {
        self.tiles.iter().filter(|t| **t == terrain).count()
    }
}

/// Load every `*.ron` file under `dir`, keyed by map name, in sorted order.
pub fn load_dir(dir: &Path) -> Result<BTreeMap<String, MapDef>, Error> {
    let entries = std::fs::read_dir(dir).map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "ron"))
        .collect();
    paths.sort();
    let mut maps = BTreeMap::new();
    for path in paths {
        let map = MapDef::load(&path)?;
        maps.insert(map.name.clone(), map);
    }
    Ok(maps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small(rows: &[&str], starts: &str) -> String {
        let rows: Vec<String> = rows.iter().map(|r| format!("    {r:?},")).collect();
        format!(
            "(\n  name: \"t\",\n  width: 8,\n  height: 8,\n  symmetry: MirrorX,\n  starts: [{starts}],\n  rows: [\n{}\n  ],\n)",
            rows.join("\n")
        )
    }

    const OK_ROWS: [&str; 8] = [
        "........", ".f....f.", "........", "..m..m..", "........", "...~~...", "........",
        "........",
    ];

    #[test]
    fn valid_small_map_loads() {
        let text = small(&OK_ROWS, "(x: 1, y: 4), (x: 6, y: 4)");
        let m = MapDef::parse(Path::new("t.ron"), &text).unwrap();
        assert_eq!(m.count(Terrain::Forest), 2);
        assert_eq!(m.tile(2, 3), Terrain::Mountain);
        assert_eq!(m.tiles().len(), 64);
    }

    #[test]
    fn broken_symmetry_names_row() {
        let mut rows = OK_ROWS;
        rows[2] = ".....f..";
        let text = small(&rows, "(x: 1, y: 4), (x: 6, y: 4)");
        let err = MapDef::parse(Path::new("t.ron"), &text)
            .unwrap_err()
            .to_string();
        assert!(err.contains("t.ron") && err.contains("rows[2]"), "{err}");
    }

    #[test]
    fn start_count_and_partner_are_checked() {
        let err = MapDef::parse(Path::new("t.ron"), &small(&OK_ROWS, "(x: 1, y: 4)"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("starts"), "{err}");
        let err = MapDef::parse(
            Path::new("t.ron"),
            &small(&OK_ROWS, "(x: 1, y: 4), (x: 6, y: 5)"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("starts[1]"), "{err}");
        let err = MapDef::parse(
            Path::new("t.ron"),
            &small(&OK_ROWS, "(x: 3, y: 5), (x: 4, y: 5)"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("Grass"), "{err}");
    }

    #[test]
    fn wrong_dimensions_are_reported() {
        let mut rows = OK_ROWS.to_vec();
        rows[5] = "...~~..";
        let text = small(&rows, "(x: 1, y: 4), (x: 6, y: 4)");
        let err = MapDef::parse(Path::new("t.ron"), &text)
            .unwrap_err()
            .to_string();
        assert!(err.contains("rows[5]"), "{err}");
        rows.pop();
        let text = small(&rows, "(x: 1, y: 4), (x: 6, y: 4)");
        let err = MapDef::parse(Path::new("t.ron"), &text)
            .unwrap_err()
            .to_string();
        assert!(err.contains("expected 8 rows"), "{err}");
    }

    #[test]
    fn name_must_match_file_stem() {
        let text = small(&OK_ROWS, "(x: 1, y: 4), (x: 6, y: 4)");
        let err = MapDef::parse(Path::new("other.ron"), &text)
            .unwrap_err()
            .to_string();
        assert!(err.contains("name"), "{err}");
    }
}
