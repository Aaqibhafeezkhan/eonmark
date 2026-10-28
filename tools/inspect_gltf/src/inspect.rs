//! `inspect <file>`: scenes, the node tree with transforms, meshes and
//! primitives (vertices, triangles, attributes, texture), images, skins,
//! animations, the merged rest bounds and, for a skinned file, the rest
//! arm-span ratio (above 0.8 reads as a T-pose).

use crate::model::{Model, prims_bounds};
use std::fmt::Write as _;
use std::path::Path;

/// A rest pose whose horizontal extent is more than this share of its
/// height is reported as a T-pose.
pub const T_POSE_RATIO: f32 = 0.8;

fn fmt3(v: [f32; 3]) -> String {
    format!("({:.3}, {:.3}, {:.3})", v[0], v[1], v[2])
}

/// The report for `path`.
pub fn inspect(path: &Path) -> Result<String, String> {
    let model = Model::load(path)?;
    let doc = &model.doc;
    let mut out = String::new();
    let w = &mut out;
    let files: Vec<String> = model
        .files
        .iter()
        .skip(1)
        .map(|(p, b)| {
            format!(
                "{} {b} B",
                p.file_name().unwrap_or_default().to_string_lossy()
            )
        })
        .collect();
    let _ = writeln!(
        w,
        "file: {} ({} B; external: {})",
        path.display(),
        model.files[0].1,
        if files.is_empty() {
            "none".to_string()
        } else {
            files.join(", ")
        }
    );
    let asset = &doc.as_json().asset;
    let _ = writeln!(
        w,
        "asset: version {} generator {:?}",
        asset.version,
        asset.generator.as_deref().unwrap_or("")
    );
    if let Some(extras) = &asset.extras {
        let _ = writeln!(w, "asset extras: {}", extras.get());
    }
    let _ = writeln!(
        w,
        "scenes: {} (default {})",
        doc.scenes().count(),
        doc.default_scene()
            .map_or("none".to_string(), |s| s.index().to_string())
    );
    for s in doc.scenes() {
        let roots: Vec<String> = s.nodes().map(|n| n.index().to_string()).collect();
        let _ = writeln!(
            w,
            "  scene {} {:?}: roots [{}]",
            s.index(),
            s.name().unwrap_or(""),
            roots.join(", ")
        );
    }
    let nodes: Vec<gltf::Node> = doc.nodes().collect();
    let _ = writeln!(w, "nodes: {}", nodes.len());
    for (i, depth) in model.traversal() {
        let n = &nodes[i];
        let (t, r, s) = n.transform().decomposed();
        let _ = writeln!(
            w,
            "  {}{} {:?} mesh={} skin={} t={} r=({:.3}, {:.3}, {:.3}, {:.3}) s={}",
            "  ".repeat(depth),
            i,
            n.name().unwrap_or(""),
            n.mesh().map_or("-".to_string(), |m| m.index().to_string()),
            n.skin().map_or("-".to_string(), |s| s.index().to_string()),
            fmt3(t),
            r[0],
            r[1],
            r[2],
            r[3],
            fmt3(s)
        );
    }
    let _ = writeln!(w, "meshes: {}", doc.meshes().count());
    let mut total_tris = 0usize;
    let get = |b: gltf::Buffer| model.buffers.get(b.index()).map(Vec::as_slice);
    for m in doc.meshes() {
        let _ = writeln!(
            w,
            "  mesh {} {:?}: {} primitive(s)",
            m.index(),
            m.name().unwrap_or(""),
            m.primitives().count()
        );
        for p in m.primitives() {
            let reader = p.reader(get);
            let vertices = reader.read_positions().map_or(0, Iterator::count);
            let indices = reader.read_indices().map(|i| i.into_u32().count());
            let elems = indices.unwrap_or(vertices);
            let tris = if p.mode() == gltf::mesh::Mode::Triangles {
                elems / 3
            } else {
                0
            };
            total_tris += tris;
            let mut attrs: Vec<String> = p.attributes().map(|(s, _)| s.to_string()).collect();
            attrs.sort();
            let texture = p
                .material()
                .pbr_metallic_roughness()
                .base_color_texture()
                .map_or("none".to_string(), |t| {
                    format!(
                        "image {} ({})",
                        t.texture().source().index(),
                        model.image_name(t.texture().source().index())
                    )
                });
            let _ = writeln!(
                w,
                "    prim {}: mode={:?} indexed={} vertices={vertices} triangles={tris} attributes=[{}] material={} texture={texture}",
                p.index(),
                p.mode(),
                indices.is_some(),
                attrs.join(", "),
                p.material()
                    .index()
                    .map_or("default".to_string(), |i| i.to_string()),
            );
        }
    }
    let _ = writeln!(w, "images: {}", doc.images().count());
    for i in 0..doc.images().count() {
        let size = model.image(i).map_or_else(
            |e| format!("unreadable: {e}"),
            |img| format!("{}x{}", img.width, img.height),
        );
        let _ = writeln!(w, "  image {i}: {} {size}", model.image_name(i));
    }
    let _ = writeln!(w, "cameras: {}", doc.cameras().count());
    let exts: Vec<&str> = doc.extensions_used().collect();
    let _ = writeln!(
        w,
        "extensions used: {}",
        if exts.is_empty() {
            "none".to_string()
        } else {
            exts.join(", ")
        }
    );
    let _ = writeln!(w, "skins: {}", doc.skins().count());
    for s in doc.skins() {
        let names: Vec<String> = s
            .joints()
            .map(|j| j.name().unwrap_or("").to_string())
            .collect();
        let _ = writeln!(
            w,
            "  skin {} {:?}: {} joints: {}",
            s.index(),
            s.name().unwrap_or(""),
            names.len(),
            names.join(", ")
        );
    }
    let _ = writeln!(w, "animations: {}", doc.animations().count());
    for a in doc.animations() {
        let mut end = 0.0f32;
        let mut interps: Vec<String> = Vec::new();
        for c in a.channels() {
            if let Some(max) = c
                .sampler()
                .input()
                .max()
                .and_then(|v| v.as_array().and_then(|a| a.first().cloned()))
            {
                end = end.max(max.as_f64().unwrap_or(0.0) as f32);
            }
            let name = format!("{:?}", c.sampler().interpolation());
            if !interps.contains(&name) {
                interps.push(name);
            }
        }
        let _ = writeln!(
            w,
            "  anim {} {:?}: {:.3} s, {} channels, {}",
            a.index(),
            a.name().unwrap_or(""),
            end,
            a.channels().count(),
            interps.join("/")
        );
    }
    let prims = model.prims(&model.rest_locals(), &|_| true)?;
    let _ = writeln!(
        w,
        "triangles: {total_tris} (drawn by the scene: {})",
        prims.iter().map(|p| p.triangles()).sum::<usize>()
    );
    match prims_bounds(&prims) {
        Some((lo, hi)) => {
            let size = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
            let _ = writeln!(
                w,
                "bounds (rest): min {} max {} size {}",
                fmt3(lo),
                fmt3(hi),
                fmt3(size)
            );
            if doc.skins().count() > 0 && size[1] > 0.0 {
                let ratio = size[0].max(size[2]) / size[1];
                let _ = writeln!(
                    w,
                    "rest arm-span ratio: {ratio:.2} ({})",
                    if ratio > T_POSE_RATIO {
                        "T-pose"
                    } else {
                        "not a T-pose"
                    }
                );
            }
        }
        None => {
            let _ = writeln!(w, "bounds (rest): empty");
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{TempDir, quads_gltf, rig_glb};

    #[test]
    fn inspect_lists_nodes_meshes_skins_and_bounds() {
        let dir = TempDir::new("inspect");
        let rig = dir.path().join("rig.glb");
        std::fs::write(&rig, rig_glb("LINEAR", false)).unwrap();
        let report = inspect(&rig).unwrap();
        for want in [
            "nodes: 4",
            "  0 \"Root\" mesh=- skin=-",
            "    1 \"Upper\" mesh=- skin=- t=(0.000, 1.000, 0.000)",
            "      3 \"Sword\" mesh=1 skin=-",
            "  2 \"Body\" mesh=0 skin=0",
            "prim 0: mode=Triangles indexed=true vertices=4 triangles=2 attributes=[JOINTS_0, NORMAL, POSITION, TEXCOORD_0, WEIGHTS_0]",
            "image 0: bufferView 0 (embedded) 8x8",
            "skins: 1",
            "skin 0 \"\": 2 joints: Root, Upper",
            "anim 0 \"Bend\": 1.000 s, 1 channels, Linear",
            "triangles: 3 (drawn by the scene: 3)",
            // At rest the body strip spans 0..0.2 in x, 0..2 in y and
            // 0..0.3 in z (the sword stays inside it).
            "bounds (rest): min (0.000, 0.000, 0.000) max (0.200, 2.000, 0.300)",
            "rest arm-span ratio: 0.15 (not a T-pose)",
        ] {
            assert!(report.contains(want), "missing {want:?} in\n{report}");
        }
        let quads = quads_gltf(&dir, "flat", &[[1.5, 1.5]], 0.0);
        let report = inspect(&quads).unwrap();
        assert!(report.contains("external: flat.bin"), "{report}");
        assert!(report.contains("image 0: atlas.png 8x8"), "{report}");
        assert!(!report.contains("arm-span"), "{report}");
        std::fs::remove_file(dir.path().join("flat.bin")).unwrap();
        assert!(inspect(&quads).unwrap_err().contains("not found"));
    }
}
