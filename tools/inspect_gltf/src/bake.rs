//! `bake <bake.ron> [--only <kind>]`: turn a rigged, animated character
//! into the static model the game draws. For each row of `bake.ron`: pose
//! the skeleton at one moment of a clip (STEP and LINEAR keys; CUBICSPLINE
//! fails), skin the mesh with linear blend skinning, keep the named nodes,
//! merge them into one primitive, put the base centre at the origin and
//! turn it to face +X, embed the base texture (a nearest downsample only
//! when every face keeps its colour), and write a GLB with no skin and no
//! animation. The ATTRIBUTION.md Modifications text goes into
//! `asset.extras.modifications`. The same inputs on the same OS give
//! byte-identical output.

use crate::glob::keeps;
use crate::image::{FNV_OFFSET, Rgba, fnv1a, hex};
use crate::math::{Mat4, Vec3};
use crate::model::{Model, bounds};
use crate::writer::{AssetExtras, Builder, Node};
use serde::Deserialize;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// `tools/assets/bake.ron`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BakeFile {
    /// One row per baked model.
    pub models: Vec<BakeModel>,
}

/// One baked model.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BakeModel {
    /// Output path under the assets directory; its file stem is the kind
    /// `--only` selects.
    pub out: String,
    /// Source file name in the download cache.
    pub src: String,
    /// The base texture's file name in the cache when the source's own
    /// image URI does not resolve; `None` uses the source's image.
    #[serde(default)]
    pub texture: Option<String>,
    /// Node-name globs to keep (`crate::glob`); at least one.
    pub keep: Vec<String>,
    /// The moment of a clip to pose at.
    pub pose: Pose,
    /// Largest texture side; a larger texture is downsampled by a power of
    /// two when no face changes colour.
    pub max_texture: u32,
    /// Turn applied after centring so the model faces +X; glTF characters
    /// face +Z, which 90 turns into +X.
    #[serde(default = "default_yaw")]
    pub yaw_deg: i32,
    /// The sha256 of the committed output (64 lowercase hex digits), the
    /// pin `scripts/check_assets.sh` holds the file to (rule 5, as
    /// `sources.tsv` pins the vendored files): a re-bake that changes a
    /// byte must be re-pinned deliberately. Empty only while a new row is
    /// being pinned (`check_assets.sh` then prints the hash to paste); the
    /// repository's rows all carry one.
    #[serde(default)]
    pub sha256: String,
}

/// How far, per 8-bit channel, a face's colour (the texel at its UV
/// centroid) may move when the texture is downsampled. KayKit's character
/// atlases are palette cells with a faint gradient, so an exact match is
/// too strict (the Knight's face 2 moves from `#757e83` to `#747d82`), while
/// a face landing in another cell moves by far more.
pub const DOWNSAMPLE_TOLERANCE: u8 = 8;

fn default_yaw() -> i32 {
    90
}

/// A pose: clip name and time in milliseconds.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pose {
    /// Animation name.
    pub clip: String,
    /// Time into the clip, milliseconds.
    pub at_ms: u32,
}

impl BakeModel {
    /// The kind name (`out`'s file stem).
    pub fn kind(&self) -> String {
        Path::new(&self.out)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// A baked model.
pub struct Baked {
    /// The GLB bytes.
    pub glb: Vec<u8>,
    /// The Modifications text written into `asset.extras`.
    pub modifications: String,
    /// Triangle count.
    pub triangles: usize,
    /// Embedded texture size.
    pub texture: (u32, u32),
    /// Morph-target weight channels of the clip that were not applied.
    pub skipped_weight_channels: usize,
}

/// Parse a `bake.ron`.
pub fn parse(text: &str) -> Result<BakeFile, String> {
    let file: BakeFile = ron::from_str(text).map_err(|e| format!("bake.ron: {e}"))?;
    for m in &file.models {
        if m.keep.is_empty() {
            return Err(format!("bake.ron: {}: keep lists no node", m.out));
        }
        rules::visuals::check_asset_path(&m.out).map_err(|e| format!("bake.ron: out {e}"))?;
        if !m.out.ends_with(".glb") {
            return Err(format!("bake.ron: {} must end in .glb", m.out));
        }
        crate::model::check_uri(&m.src)?;
        if let Some(t) = &m.texture {
            crate::model::check_uri(t)?;
        }
        let pin = &m.sha256;
        if !pin.is_empty()
            && !(pin.len() == 64
                && pin
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
        {
            return Err(format!(
                "bake.ron: {}: sha256 must be 64 lowercase hex digits",
                m.out
            ));
        }
        if m.max_texture == 0 {
            return Err(format!(
                "bake.ron: {}: max_texture must be at least 1",
                m.out
            ));
        }
    }
    Ok(file)
}

fn quote_list(items: &[String]) -> String {
    let q: Vec<String> = items.iter().map(|s| format!("`{s}`")).collect();
    match q.len() {
        0 => String::new(),
        1 => q[0].clone(),
        n => format!("{} and {}", q[..n - 1].join(", "), q[n - 1]),
    }
}

/// The smallest power of two that brings `side` to at most `max`.
fn downsample_factor(side: u32, max: u32) -> u32 {
    let mut f = 1;
    while side.div_ceil(f) > max {
        f *= 2;
    }
    f
}

/// Bake one model from the files in `cache`.
pub fn bake_model(m: &BakeModel, cache: &Path) -> Result<Baked, String> {
    let src = cache.join(&m.src);
    let model = Model::load_with(&src, &[cache.to_path_buf()])?;
    let (locals, skipped) = model.posed_locals(&m.pose.clip, m.pose.at_ms)?;
    let prims = model.prims(&locals, &|name| keeps(&m.keep, name))?;
    if prims.is_empty() {
        return Err(format!("{}: no mesh node matches keep {:?}", m.src, m.keep));
    }
    // One texture for every kept primitive.
    let mut image = None;
    for p in &prims {
        if !p.triangles_mode {
            return Err(format!(
                "{}: node {:?} is {}, not triangles",
                m.src, p.node_name, p.mode
            ));
        }
        if p.normals.is_none() || p.uvs.is_none() {
            return Err(format!(
                "{}: node {:?} lacks NORMAL or TEXCOORD_0",
                m.src, p.node_name
            ));
        }
        match (image, p.image) {
            (_, None) => {
                return Err(format!(
                    "{}: node {:?} has no base-colour texture",
                    m.src, p.node_name
                ));
            }
            (None, Some(i)) => image = Some(i),
            (Some(a), Some(b)) if a != b => {
                return Err(format!(
                    "{}: the kept nodes use two textures (images {a} and {b})",
                    m.src
                ));
            }
            _ => {}
        }
    }
    let (texture, texture_name) = match &m.texture {
        Some(name) => {
            let bytes = std::fs::read(cache.join(name)).map_err(|e| format!("{name}: {e}"))?;
            (
                Rgba::decode_png(&bytes).map_err(|e| format!("{name}: {e}"))?,
                name.clone(),
            )
        }
        None => {
            let i = image.unwrap_or(0);
            (model.image(i)?, model.image_name(i))
        }
    };

    // Merge.
    let mut positions: Vec<Vec3> = Vec::new();
    let mut normals: Vec<Vec3> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for p in &prims {
        let base = u32::try_from(positions.len()).map_err(|_| "too many vertices")?;
        positions.extend_from_slice(&p.positions);
        normals.extend_from_slice(p.normals.as_deref().unwrap_or_default());
        uvs.extend_from_slice(p.uvs.as_deref().unwrap_or_default());
        indices.extend(p.indices.iter().map(|i| i + base));
    }

    // Base centre to the origin, then face +X.
    let (lo, hi) = bounds(positions.iter()).ok_or("no vertices")?;
    let centre = Mat4::translation([-(lo[0] + hi[0]) / 2.0, -lo[1], -(lo[2] + hi[2]) / 2.0]);
    let place = Mat4::yaw_deg(m.yaw_deg).mul(&centre);
    let normal3 = Mat4::yaw_deg(m.yaw_deg)
        .normal_matrix()
        .ok_or("singular yaw")?;
    let positions: Vec<Vec3> = positions.iter().map(|&p| place.point(p)).collect();
    let normals: Vec<Vec3> = normals
        .iter()
        .map(|&n| crate::math::apply_normal(&normal3, n))
        .collect();

    // Texture, downsampled only when no face changes colour.
    let side = texture.width.max(texture.height);
    let factor = downsample_factor(side, m.max_texture);
    let mut downsampled = false;
    let texture = if factor > 1 {
        let small = texture.downsample_nearest(factor);
        for (t, tri) in indices.chunks(3).enumerate() {
            let c = [
                uvs[tri[0] as usize],
                uvs[tri[1] as usize],
                uvs[tri[2] as usize],
            ];
            let centroid = [
                (c[0][0] + c[1][0] + c[2][0]) / 3.0,
                (c[0][1] + c[1][1] + c[2][1]) / 3.0,
            ];
            let (a, b) = (texture.sample(centroid), small.sample(centroid));
            let delta = (0..4).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
            if delta > DOWNSAMPLE_TOLERANCE {
                return Err(format!(
                    "{}: downsampling {}x{} by {factor} changes face {t} from {} to {} (more than {DOWNSAMPLE_TOLERANCE}/255 on a channel); raise max_texture",
                    m.out,
                    texture.width,
                    texture.height,
                    hex([a[0], a[1], a[2]]),
                    hex([b[0], b[1], b[2]])
                ));
            }
        }
        downsampled = true;
        small
    } else {
        texture
    };

    let mut modifications = format!(
        "baked by `tools/inspect_gltf bake` (tools/assets/bake.ron, `{}`): posed at clip `{}` t={} ms; skin and animations removed; nodes other than {} dropped; base centre at the origin, facing +X; `{}` embedded",
        m.kind(),
        m.pose.clip,
        m.pose.at_ms,
        quote_list(&m.keep),
        texture_name
    );
    if downsampled {
        let _ = write!(
            modifications,
            " (downsampled to {}x{} nearest; every face keeps its colour within {DOWNSAMPLE_TOLERANCE}/255)",
            texture.width, texture.height
        );
    }
    let rig = rig_key(&model);
    let mut b = Builder::new();
    let png = texture.encode_png()?;
    let tex = b.png_texture(&png);
    let mat = b.material(Some(tex));
    let prim = b.prim(&positions, Some(&normals), Some(&uvs), &indices, Some(mat));
    let kind = m.kind();
    let mesh = b.mesh(Some(&kind), vec![prim]);
    let node = b.node(Node {
        name: Some(kind),
        mesh: Some(mesh),
        ..Node::default()
    });
    b.root(node);
    let glb = b.glb(Some(&AssetExtras {
        modifications: modifications.clone(),
        rig,
        source: m.src.clone(),
    }))?;
    Ok(Baked {
        glb,
        modifications,
        triangles: indices.len() / 3,
        texture: (texture.width, texture.height),
        skipped_weight_channels: skipped,
    })
}

/// A model's rig identity for the silhouette rule: the baked
/// `asset.extras.rig` when present, else a hash of its first skin's joint
/// names, else empty (the caller uses the file).
pub fn rig_key(model: &Model) -> String {
    if let Some(extras) = read_extras(model)
        && !extras.rig.is_empty()
    {
        return extras.rig;
    }
    match model.doc.skins().next() {
        Some(skin) => {
            let mut h = FNV_OFFSET;
            for j in skin.joints() {
                h = fnv1a(h, j.name().unwrap_or("").as_bytes());
                h = fnv1a(h, b"\0");
            }
            format!("{h:016x}")
        }
        None => String::new(),
    }
}

/// The bake's `asset.extras`, if the file has them.
pub fn read_extras(model: &Model) -> Option<AssetExtras> {
    let raw = model.doc.as_json().asset.extras.as_ref()?;
    gltf::json::deserialize::from_str(raw.get()).ok()
}

/// Bake every row of `bake_ron` (or only `only`), reading sources from
/// `cache` and writing under `assets`. Returns one line per model.
pub fn bake(
    bake_ron: &Path,
    cache: &Path,
    assets: &Path,
    only: Option<&str>,
) -> Result<String, String> {
    let text =
        std::fs::read_to_string(bake_ron).map_err(|e| format!("{}: {e}", bake_ron.display()))?;
    let file = parse(&text)?;
    let mut out = String::new();
    let mut n = 0;
    for m in &file.models {
        if only.is_some_and(|k| k != m.kind()) {
            continue;
        }
        let baked = bake_model(m, cache)?;
        let dest: PathBuf = assets.join(&m.out);
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&dest, &baked.glb).map_err(|e| format!("{}: {e}", dest.display()))?;
        let _ = writeln!(
            out,
            "bake: {} tris={} bytes={} texture={}x{}{}",
            m.out,
            baked.triangles,
            baked.glb.len(),
            baked.texture.0,
            baked.texture.1,
            if baked.skipped_weight_channels > 0 {
                format!(
                    " (skipped {} morph-weight channel(s))",
                    baked.skipped_weight_channels
                )
            } else {
                String::new()
            }
        );
        n += 1;
    }
    if let Some(k) = only
        && n == 0
    {
        return Err(format!("bake.ron has no model {k:?}"));
    }
    let _ = writeln!(out, "bake: {n} model(s)");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{TempDir, rig_glb};

    fn row(keep: &[&str], clip: &str, at_ms: u32, max_texture: u32) -> BakeModel {
        BakeModel {
            out: "models/test/fighter.glb".into(),
            src: "rig.glb".into(),
            texture: None,
            keep: keep.iter().map(|s| (*s).to_string()).collect(),
            pose: Pose {
                clip: clip.into(),
                at_ms,
            },
            max_texture,
            yaw_deg: 0,
            sha256: String::new(),
        }
    }

    #[test]
    fn lbs_two_joint_bend_matches_hand_math() {
        let dir = TempDir::new("lbs");
        std::fs::write(dir.path().join("rig.glb"), rig_glb("LINEAR", false)).unwrap();
        let model = Model::load(&dir.path().join("rig.glb")).unwrap();
        let (locals, _) = model.posed_locals("Bend", 1000).unwrap();
        let prims = model.prims(&locals, &|n| n == "Body").unwrap();
        let p = &prims[0].positions;
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5);
        // Vertex 0 (0, 0, 0) on the root joint stays; vertex 2 (0, 2, 0)
        // on the upper joint, bent 90 degrees about +Z at (0, 1, 0), goes
        // to (-1, 1, 0); vertex 1 (0, 1.5, 0), half on each, blends to
        // (-0.25, 1.25, 0).
        assert!(close(p[0], [0.0, 0.0, 0.0]), "{:?}", p[0]);
        assert!(close(p[2], [-1.0, 1.0, 0.0]), "{:?}", p[2]);
        assert!(close(p[1], [-0.25, 1.25, 0.0]), "{:?}", p[1]);
        // Halfway through the clip: 45 degrees.
        let (half, _) = model.posed_locals("Bend", 500).unwrap();
        let p = model.prims(&half, &|n| n == "Body").unwrap()[0]
            .positions
            .clone();
        let s = std::f32::consts::FRAC_1_SQRT_2;
        assert!(close(p[2], [-s, 1.0 + s, 0.0]), "{:?}", p[2]);
        // STEP holds the first key until the second.
        std::fs::write(dir.path().join("step.glb"), rig_glb("STEP", false)).unwrap();
        let step = Model::load(&dir.path().join("step.glb")).unwrap();
        let (held, _) = step.posed_locals("Bend", 999).unwrap();
        let p = step.prims(&held, &|n| n == "Body").unwrap()[0]
            .positions
            .clone();
        assert!(close(p[2], [0.0, 2.0, 0.0]), "{:?}", p[2]);
        // CUBICSPLINE fails loudly, and an unknown clip names the clips.
        std::fs::write(dir.path().join("cubic.glb"), rig_glb("CUBICSPLINE", false)).unwrap();
        let cubic = Model::load(&dir.path().join("cubic.glb")).unwrap();
        let err = cubic.posed_locals("Bend", 500).err().unwrap();
        assert!(err.contains("CUBICSPLINE"), "{err}");
        let err = model.posed_locals("Walk", 0).err().unwrap();
        assert!(err.contains("\"Bend\""), "{err}");
    }

    #[test]
    fn bake_keeps_only_named_nodes_and_reparses_without_skin_or_animation() {
        let dir = TempDir::new("bake_keep");
        std::fs::write(dir.path().join("rig.glb"), rig_glb("LINEAR", false)).unwrap();
        let body_only = bake_model(&row(&["Body"], "Bend", 1000, 256), dir.path()).unwrap();
        let both = bake_model(&row(&["Bo*", "Sword"], "Bend", 1000, 256), dir.path()).unwrap();
        assert_eq!(body_only.triangles, 2);
        assert_eq!(both.triangles, 3);
        std::fs::write(dir.path().join("out.glb"), &both.glb).unwrap();
        let out = Model::load(&dir.path().join("out.glb")).unwrap();
        assert_eq!(out.doc.skins().count(), 0);
        assert_eq!(out.doc.animations().count(), 0);
        assert_eq!(out.doc.nodes().count(), 1);
        assert_eq!(out.doc.meshes().next().unwrap().primitives().count(), 1);
        let prims = out.prims(&out.rest_locals(), &|_| true).unwrap();
        let (lo, hi) = crate::model::prims_bounds(&prims).unwrap();
        // Base centre at the origin.
        assert!(lo[1].abs() < 1e-6, "{lo:?}");
        assert!(
            (lo[0] + hi[0]).abs() < 1e-6 && (lo[2] + hi[2]).abs() < 1e-6,
            "{lo:?} {hi:?}"
        );
        // POSITION carries min and max.
        let acc = out
            .doc
            .meshes()
            .next()
            .unwrap()
            .primitives()
            .next()
            .unwrap()
            .get(&gltf::Semantic::Positions)
            .unwrap();
        assert!(acc.min().is_some() && acc.max().is_some());
        let extras = read_extras(&out).unwrap();
        assert_eq!(extras.modifications, both.modifications);
        assert!(
            both.modifications
                .contains("nodes other than `Bo*` and `Sword` dropped"),
            "{}",
            both.modifications
        );
        assert_eq!(
            extras.rig,
            rig_key(&Model::load(&dir.path().join("rig.glb")).unwrap())
        );
        assert!(!extras.rig.is_empty());
        // Nothing kept is an error.
        let err = bake_model(&row(&["Nope"], "Bend", 0, 256), dir.path())
            .err()
            .unwrap();
        assert!(err.contains("no mesh node matches"), "{err}");
    }

    #[test]
    fn downsample_fails_when_a_face_changes_cell() {
        let dir = TempDir::new("downsample");
        // The test atlas is 8x8 in four 4x4 colour blocks: halving it keeps
        // every face's colour.
        std::fs::write(dir.path().join("rig.glb"), rig_glb("LINEAR", false)).unwrap();
        let ok = bake_model(&row(&["Body", "Sword"], "Bend", 0, 4), dir.path()).unwrap();
        assert_eq!(ok.texture, (4, 4));
        assert!(
            ok.modifications.contains("downsampled to 4x4 nearest"),
            "{}",
            ok.modifications
        );
        // With a one-texel detail under a face, it would change colour.
        std::fs::write(dir.path().join("rig.glb"), rig_glb("LINEAR", true)).unwrap();
        let err = bake_model(&row(&["Body", "Sword"], "Bend", 0, 4), dir.path())
            .err()
            .unwrap();
        assert!(err.contains("changes face"), "{err}");
        // Without a downsample it bakes.
        assert_eq!(
            bake_model(&row(&["Body", "Sword"], "Bend", 0, 8), dir.path())
                .unwrap()
                .texture,
            (8, 8)
        );
    }

    #[test]
    fn bake_twice_is_byte_identical() {
        let dir = TempDir::new("bake_twice");
        std::fs::write(dir.path().join("rig.glb"), rig_glb("LINEAR", false)).unwrap();
        let bake_ron = dir.path().join("bake.ron");
        std::fs::write(
            &bake_ron,
            r#"(models: [(out: "models/test/fighter.glb", src: "rig.glb", keep: ["Body", "Sword"], pose: (clip: "Bend", at_ms: 400), max_texture: 4)])"#,
        )
        .unwrap();
        let assets = dir.path().join("assets");
        let first = bake(&bake_ron, dir.path(), &assets, None).unwrap();
        let a = std::fs::read(assets.join("models/test/fighter.glb")).unwrap();
        let second = bake(&bake_ron, dir.path(), &assets, Some("fighter")).unwrap();
        let b = std::fs::read(assets.join("models/test/fighter.glb")).unwrap();
        assert_eq!(a, b);
        assert_eq!(first, second);
        assert!(
            first.contains("bake: models/test/fighter.glb tris=3"),
            "{first}"
        );
        assert!(bake(&bake_ron, dir.path(), &assets, Some("nobody")).is_err());
        // The default yaw turns +Z into +X.
        let file = parse(&std::fs::read_to_string(&bake_ron).unwrap()).unwrap();
        assert_eq!(file.models[0].yaw_deg, 90);
        assert!(parse(r#"(models: [(out: "../x.glb", src: "a.glb", keep: ["a"], pose: (clip: "c", at_ms: 0), max_texture: 4)])"#).is_err());
        // A pin is 64 lowercase hex digits.
        let pinned = |pin: &str| {
            parse(&format!(
                r#"(models: [(out: "m/x.glb", src: "a.glb", keep: ["a"], pose: (clip: "c", at_ms: 0), max_texture: 4, sha256: "{pin}")])"#
            ))
        };
        assert_eq!(
            pinned(&"0f".repeat(32)).unwrap().models[0].sha256,
            "0f".repeat(32)
        );
        assert_eq!(pinned("").unwrap().models[0].sha256, "", "not pinned yet");
        assert!(pinned(&"0F".repeat(32)).is_err());
        assert!(pinned("abc").is_err());
        assert!(parse("(models: [])").unwrap().models.is_empty());
    }
}
