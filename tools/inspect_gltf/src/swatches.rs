//! `swatches <blue> <red> [<blue> <red> ...]`: KayKit ships each building
//! in team colours that differ only in texture coordinates over one atlas.
//! For each pair this asserts identical geometry (positions after node
//! transforms within [`POSITION_TOLERANCE`], counting the vertices that
//! differ by float noise, and indices exactly), finds the faces whose texture
//! coordinates differ (the team faces), and prints the texel rects of the
//! blue file's texture they sample. Over all pairs it prints the union of
//! the rects for `visuals.ron`'s `team_swatches` and checks that no
//! non-team face of any pair samples a texel of them.

use crate::image::Rgba;
use crate::math::triangle_area;
use crate::model::{Model, Prim};
use crate::uv::{Mask, Rect, face_rect, merge_overlapping, ron_list};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The result of one blue/red pair.
pub struct Pair {
    /// The blue file.
    pub blue: PathBuf,
    /// The blue file's primitives.
    prims: Vec<Prim>,
    /// Per primitive, per triangle: a team face.
    team: Vec<Vec<bool>>,
    /// The texture.
    texture: Rgba,
    /// This pair's team rects.
    pub rects: Vec<Rect>,
    /// Share of the surface on team faces, permille.
    pub team_permille: u32,
    /// Vertices whose positions differ by float noise (at most
    /// [`POSITION_TOLERANCE`]) and the largest such difference.
    pub noise: (usize, f32),
}

/// Two variants count as the same geometry when every position agrees to
/// within this many metres: KayKit's exporter leaves last-bit differences
/// between colour variants of a few models (the lumbermill's top differs by
/// 1e-8 m in some vertices).
pub const POSITION_TOLERANCE: f32 = 1e-5;

/// The whole run.
pub struct Report {
    /// The text printed.
    pub text: String,
    /// The union of every pair's rects.
    pub union: Vec<Rect>,
    /// Non-team faces sampling a union rect, over all pairs.
    pub shared: usize,
}

fn one_texture(model: &Model, prims: &[Prim]) -> Result<Rgba, String> {
    let mut image = None;
    for p in prims {
        match (p.image, image) {
            (None, _) => {
                return Err(format!(
                    "{}: node {:?} has no base-colour texture",
                    model.path.display(),
                    p.node_name
                ));
            }
            (Some(i), None) => image = Some(i),
            (Some(i), Some(j)) if i != j => {
                return Err(format!(
                    "{}: primitives use two textures ({j} and {i})",
                    model.path.display()
                ));
            }
            _ => {}
        }
    }
    model.image(image.ok_or_else(|| format!("{}: no primitives", model.path.display()))?)
}

/// Compare one blue/red pair.
pub fn pair(blue: &Path, red: &Path) -> Result<Pair, String> {
    let mb = Model::load(blue)?;
    let mr = Model::load(red)?;
    let pb = mb.prims(&mb.rest_locals(), &|_| true)?;
    let pr = mr.prims(&mr.rest_locals(), &|_| true)?;
    if pb.len() != pr.len() {
        return Err(format!(
            "geometry differs: {} has {} primitives, {} has {}",
            blue.display(),
            pb.len(),
            red.display(),
            pr.len()
        ));
    }
    let texture = one_texture(&mb, &pb)?;
    let red_texture = one_texture(&mr, &pr)?;
    if red_texture.content_hash() != texture.content_hash() {
        return Err(format!(
            "{} and {} use different textures; the team colour is not a UV difference over one atlas",
            blue.display(),
            red.display()
        ));
    }
    let mut mask = Mask::new(texture.width, texture.height);
    let mut team = Vec::new();
    let (mut team_area, mut area) = (0.0f64, 0.0f64);
    let (mut noisy, mut max_noise) = (0usize, 0.0f32);
    for (a, b) in pb.iter().zip(&pr) {
        let gap = |v: usize| {
            (0..3)
                .map(|c| (a.positions[v][c] - b.positions[v][c]).abs())
                .fold(0.0f32, f32::max)
        };
        let n = a.positions.len().min(b.positions.len());
        let first_moved = (0..n).find(|&v| gap(v) > POSITION_TOLERANCE);
        for v in 0..n {
            let g = gap(v);
            if g > 0.0 {
                noisy += 1;
                max_noise = max_noise.max(g);
            }
        }
        if a.positions.len() != b.positions.len() || first_moved.is_some() || a.indices != b.indices
        {
            let detail = match first_moved {
                _ if a.positions.len() != b.positions.len() => {
                    format!("{} vs {} vertices", a.positions.len(), b.positions.len())
                }
                Some(v) => format!("vertex {v} at {:?} vs {:?}", a.positions[v], b.positions[v]),
                None => format!(
                    "indices differ ({} vs {})",
                    a.indices.len(),
                    b.indices.len()
                ),
            };
            return Err(format!(
                "geometry differs in node {:?} mesh {} primitive {}: {detail}",
                a.node_name, a.mesh, a.primitive
            ));
        }
        let (Some(ua), Some(ub)) = (&a.uvs, &b.uvs) else {
            return Err(format!("node {:?} has no TEXCOORD_0", a.node_name));
        };
        let mut faces = Vec::with_capacity(a.triangles());
        for t in 0..a.triangles() {
            let c = a.tri(t);
            let differs = c.iter().any(|&v| {
                ua[v][0].to_bits() != ub[v][0].to_bits() || ua[v][1].to_bits() != ub[v][1].to_bits()
            });
            let s = f64::from(triangle_area(
                a.positions[c[0]],
                a.positions[c[1]],
                a.positions[c[2]],
            ));
            area += s;
            if differs {
                team_area += s;
                mask.add(&face_rect(
                    [ua[c[0]], ua[c[1]], ua[c[2]]],
                    texture.width,
                    texture.height,
                ));
            }
            faces.push(differs);
        }
        team.push(faces);
    }
    let team_permille = if area > 0.0 {
        (1000.0 * team_area / area).round() as u32
    } else {
        0
    };
    Ok(Pair {
        blue: blue.to_path_buf(),
        rects: mask.rects(),
        prims: pb,
        team,
        texture,
        team_permille,
        noise: (noisy, max_noise),
    })
}

/// The non-team faces of `pair` that sample a texel of `rects`, as
/// `(node, triangle, rect)`.
fn shared_faces(pair: &Pair, rects: &[Rect]) -> Vec<(String, usize, Rect)> {
    let team = Mask::from_rects(pair.texture.width, pair.texture.height, rects);
    let mut out = Vec::new();
    for (p, faces) in pair.prims.iter().zip(&pair.team) {
        let Some(uv) = &p.uvs else { continue };
        for (t, &is_team) in faces.iter().enumerate() {
            if is_team {
                continue;
            }
            let c = p.tri(t);
            let r = face_rect(
                [uv[c[0]], uv[c[1]], uv[c[2]]],
                pair.texture.width,
                pair.texture.height,
            );
            if team.cover(&r) != crate::uv::Cover::Outside {
                out.push((p.node_name.clone(), t, r));
            }
        }
    }
    out
}

/// Run over `pairs` and build the report.
pub fn swatches(pairs: &[(PathBuf, PathBuf)]) -> Result<Report, String> {
    let mut text = String::new();
    let mut done = Vec::new();
    for (blue, red) in pairs {
        let p = pair(blue, red)?;
        let tris: usize = p.prims.iter().map(Prim::triangles).sum();
        let team_faces: usize = p.team.iter().flatten().filter(|&&t| t).count();
        let _ = writeln!(
            text,
            "swatches: {} vs {}\n  geometry: identical{} ({} primitives, {tris} triangles)\n  texture: {}x{}, identical in both files\n  team faces: {team_faces} of {tris} (area {}‰)\n  team rects: {}",
            blue.display(),
            red.display(),
            if p.noise.0 > 0 {
                format!(
                    " within {POSITION_TOLERANCE} m ({} vertices differ by float noise, at most {:e} m)",
                    p.noise.0, p.noise.1
                )
            } else {
                String::new()
            },
            p.prims.len(),
            p.texture.width,
            p.texture.height,
            p.team_permille,
            ron_list(&p.rects)
        );
        let own = shared_faces(&p, &p.rects);
        let _ = writeln!(text, "  non-team faces in rects: {}", own.len());
        for (node, t, r) in own.iter().take(5) {
            let _ = writeln!(text, "    node {node:?} triangle {t} samples {r}");
        }
        done.push(p);
    }
    let union = if done
        .windows(2)
        .all(|w| w[0].texture.content_hash() == w[1].texture.content_hash())
    {
        merge_overlapping(done.iter().flat_map(|p| p.rects.iter().copied()).collect())
    } else {
        return Err("the pairs use different textures; run them separately".into());
    };
    let mut shared = 0;
    for p in &done {
        let faces = shared_faces(p, &union);
        shared += faces.len();
        for (node, t, r) in faces.iter().take(5) {
            let _ = writeln!(
                text,
                "  {}: non-team node {node:?} triangle {t} samples {r} of the union",
                p.blue.display()
            );
        }
    }
    let _ = writeln!(
        text,
        "union over {} pair(s): rects: {}\nnon-team faces in rects: {shared}",
        done.len(),
        ron_list(&union)
    );
    Ok(Report {
        text,
        union,
        shared,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{TempDir, quads_gltf};

    #[test]
    fn swatches_find_team_faces_and_reject_shared_rects() {
        let dir = TempDir::new("swatches");
        // Two quads; the second is the team quad: blue samples texel
        // (6, 1), red samples (6, 5). The first quad samples (1, 1).
        let blue = quads_gltf(&dir, "house_blue", &[[1.5, 1.5], [6.5, 1.5]], 0.0);
        let red = quads_gltf(&dir, "house_red", &[[1.5, 1.5], [6.5, 5.5]], 0.0);
        let report = swatches(&[(blue.clone(), red.clone())]).unwrap();
        assert_eq!(
            report.union,
            vec![Rect {
                x: 6,
                y: 1,
                w: 1,
                h: 1
            }],
            "{}",
            report.text
        );
        assert_eq!(report.shared, 0);
        assert!(
            report.text.contains("team faces: 2 of 4 (area 500‰)"),
            "{}",
            report.text
        );
        assert!(
            report.text.contains("non-team faces in rects: 0"),
            "{}",
            report.text
        );

        // A third, non-team quad sampling the team texel is reported.
        let blue3 = quads_gltf(&dir, "hut_blue", &[[6.5, 1.5], [1.5, 1.5], [6.5, 1.5]], 0.0);
        let red3 = quads_gltf(&dir, "hut_red", &[[6.5, 1.5], [1.5, 1.5], [6.5, 5.5]], 0.0);
        let report = swatches(&[(blue3, red3)]).unwrap();
        assert_eq!(report.shared, 2, "{}", report.text);
        assert!(
            report.text.contains("non-team faces in rects: 2"),
            "{}",
            report.text
        );

        // Float noise below the tolerance is the same geometry.
        let noisy = quads_gltf(&dir, "house_noisy", &[[1.5, 1.5], [6.5, 5.5]], 1e-7);
        let report = swatches(&[(blue.clone(), noisy)]).unwrap();
        assert!(
            report
                .text
                .contains("geometry: identical within 0.00001 m (8 vertices differ by float noise"),
            "{}",
            report.text
        );
        // Different geometry is refused.
        let moved = quads_gltf(&dir, "house_moved", &[[1.5, 1.5], [6.5, 5.5]], 0.5);
        let err = swatches(&[(blue, moved)]).err().unwrap();
        assert!(err.contains("geometry differs"), "{err}");
    }
}
