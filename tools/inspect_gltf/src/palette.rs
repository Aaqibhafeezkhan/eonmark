//! `palette <file> [--grid 16]`: the base texture's cell grid, with each
//! used cell's texel rect, mean colour and the faces and nodes whose UV
//! centroid falls in it, to pick a character's team rects by eye.

use crate::model::Model;
use crate::uv::Rect;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

/// One used cell.
#[derive(Debug)]
pub struct Cell {
    /// Column and row in the grid.
    pub col: u32,
    /// Row.
    pub row: u32,
    /// Its texel rect.
    pub rect: Rect,
    /// Mean colour `#rrggbb`.
    pub hex: String,
    /// Faces whose UV centroid is in it.
    pub faces: usize,
    /// The nodes of those faces.
    pub nodes: BTreeSet<String>,
}

/// The used cells of `path`'s texture on a `grid` x `grid` grid.
pub fn cells(path: &Path, grid: u32) -> Result<(u32, u32, Vec<Cell>), String> {
    if grid == 0 {
        return Err("--grid must be at least 1".into());
    }
    let model = Model::load(path)?;
    let prims = model.prims(&model.rest_locals(), &|_| true)?;
    let image = prims
        .iter()
        .find_map(|p| p.image)
        .ok_or("no primitive has a base-colour texture")?;
    let tex = model.image(image)?;
    let (cw, ch) = (
        tex.width.div_ceil(grid).max(1),
        tex.height.div_ceil(grid).max(1),
    );
    let mut used: BTreeMap<(u32, u32), (usize, BTreeSet<String>)> = BTreeMap::new();
    for p in prims.iter().filter(|p| p.image == Some(image)) {
        let Some(uv) = &p.uvs else { continue };
        for t in 0..p.triangles() {
            let c = p.tri(t);
            let centroid = [
                (uv[c[0]][0] + uv[c[1]][0] + uv[c[2]][0]) / 3.0,
                (uv[c[0]][1] + uv[c[1]][1] + uv[c[2]][1]) / 3.0,
            ];
            let (x, y) = tex.texel_at(centroid);
            let e = used.entry((y / ch, x / cw)).or_default();
            e.0 += 1;
            e.1.insert(p.node_name.clone());
        }
    }
    let out = used
        .into_iter()
        .map(|((row, col), (faces, nodes))| {
            let rect = Rect {
                x: col * cw,
                y: row * ch,
                w: cw.min(tex.width - col * cw),
                h: ch.min(tex.height - row * ch),
            };
            Cell {
                col,
                row,
                hex: crate::image::hex(tex.mean_rgb(rect.x, rect.y, rect.w, rect.h)),
                rect,
                faces,
                nodes,
            }
        })
        .collect();
    Ok((tex.width, tex.height, out))
}

/// The printed report.
pub fn palette(path: &Path, grid: u32) -> Result<String, String> {
    let (w, h, cells) = cells(path, grid)?;
    let mut out = format!(
        "palette: {} texture {w}x{h}, grid {grid}x{grid}\n",
        path.display()
    );
    for c in &cells {
        let nodes: Vec<&str> = c.nodes.iter().map(String::as_str).collect();
        let _ = writeln!(
            out,
            "  cell ({}, {}) rect {} {} faces={} nodes={}",
            c.col,
            c.row,
            c.rect,
            c.hex,
            c.faces,
            nodes.join(",")
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{TempDir, quads_gltf};

    #[test]
    fn palette_reports_cells_and_users() {
        let dir = TempDir::new("palette");
        let file = quads_gltf(&dir, "two", &[[1.5, 1.5], [6.5, 5.5], [1.5, 1.5]], 0.0);
        let report = palette(&file, 2).unwrap();
        assert!(report.contains("texture 8x8, grid 2x2"), "{report}");
        // Cell (0, 0) has quads 0 and 2 (4 faces); cell (1, 1) quad 1.
        assert!(report.contains("cell (0, 0) rect (0, 0, 4, 4)"), "{report}");
        assert!(report.contains("faces=4 nodes=quad0,quad2"), "{report}");
        assert!(report.contains("cell (1, 1) rect (4, 4, 4, 4)"), "{report}");
        assert!(report.contains("faces=2 nodes=quad1"), "{report}");
        let (_, _, cells) = cells(&file, 2).unwrap();
        assert_eq!(cells.len(), 2);
        // The test atlas paints the top-left quarter red.
        assert_eq!(cells[0].hex, "#c83c32");
        assert!(palette(&file, 0).is_err());
    }
}
