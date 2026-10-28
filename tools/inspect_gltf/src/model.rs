//! Loading a glTF or GLB file without gltf's `import` feature: the GLB blob
//! or the `.bin` and image files its relative URIs name, node transforms,
//! the scene hierarchy, primitives in world space (skinned with linear
//! blend skinning when the node has a skin), and animation sampling.
//!
//! Downloaded files are data: URIs must be plain relative paths (no
//! scheme, no `..`, no absolute path, no percent-encoding), so a file can
//! only name files beside it.

use crate::image::Rgba;
use crate::math::{Mat4, Quat, Vec3, apply_normal, lerp3, slerp};
use gltf::animation::util::ReadOutputs;
use gltf::animation::{Interpolation, Property};
use std::path::{Path, PathBuf};

/// A loaded glTF document with its buffers and images.
pub struct Model {
    /// The file it was loaded from.
    pub path: PathBuf,
    /// The parsed and validated document.
    pub doc: gltf::Document,
    /// Buffer data by buffer index.
    pub buffers: Vec<Vec<u8>>,
    /// Encoded image bytes by image index, or why they are missing.
    pub images: Vec<Result<Vec<u8>, String>>,
    /// The file itself and every external file it names, with sizes.
    pub files: Vec<(PathBuf, u64)>,
}

/// Check that a URI names a file beside the model.
pub fn check_uri(uri: &str) -> Result<&str, String> {
    if uri.is_empty()
        || uri.contains(':')
        || uri.contains("..")
        || uri.contains('%')
        || uri.contains('\\')
        || uri.starts_with('/')
    {
        return Err(format!(
            "URI {uri:?} is not a plain relative path (data: URIs, schemes, .., absolute paths and percent-encoding are not supported)"
        ));
    }
    Ok(uri)
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

impl Model {
    /// Load `path`, resolving relative URIs in its directory, then in each
    /// of `extra_dirs` in order.
    pub fn load_with(path: &Path, extra_dirs: &[PathBuf]) -> Result<Model, String> {
        let bytes = read(path)?;
        let gltf =
            gltf::Gltf::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let resolve = |uri: &str| -> Result<PathBuf, String> {
            let uri = check_uri(uri)?;
            std::iter::once(&dir)
                .chain(extra_dirs)
                .map(|d| d.join(uri))
                .find(|p| p.is_file())
                .ok_or_else(|| format!("{}: {uri:?} not found beside it", path.display()))
        };
        let mut files = vec![(path.to_path_buf(), bytes.len() as u64)];
        let mut buffers = Vec::new();
        for buffer in gltf.document.buffers() {
            let data = match buffer.source() {
                gltf::buffer::Source::Bin => gltf.blob.clone().ok_or_else(|| {
                    format!(
                        "{}: buffer {} has no GLB blob",
                        path.display(),
                        buffer.index()
                    )
                })?,
                gltf::buffer::Source::Uri(uri) => {
                    let p = resolve(uri)?;
                    let data = read(&p)?;
                    files.push((p, data.len() as u64));
                    data
                }
            };
            if data.len() < buffer.length() {
                return Err(format!(
                    "{}: buffer {} holds {} bytes, the file declares {}",
                    path.display(),
                    buffer.index(),
                    data.len(),
                    buffer.length()
                ));
            }
            buffers.push(data);
        }
        let mut images = Vec::new();
        for image in gltf.document.images() {
            let data = match image.source() {
                gltf::image::Source::View { view, .. } => {
                    let buf = &buffers[view.buffer().index()];
                    let end = view.offset() + view.length();
                    buf.get(view.offset()..end)
                        .map(<[u8]>::to_vec)
                        .ok_or_else(|| format!("image {} view out of range", image.index()))
                }
                gltf::image::Source::Uri { uri, .. } => match resolve(uri) {
                    Ok(p) => {
                        let data = read(&p);
                        if let Ok(d) = &data {
                            files.push((p, d.len() as u64));
                        }
                        data
                    }
                    Err(e) => Err(e),
                },
            };
            images.push(data);
        }
        Ok(Model {
            path: path.to_path_buf(),
            doc: gltf.document,
            buffers,
            images,
            files,
        })
    }

    /// Load `path`, resolving URIs beside it.
    pub fn load(path: &Path) -> Result<Model, String> {
        Model::load_with(path, &[])
    }

    /// Total bytes of the file and the files it names.
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|(_, b)| b).sum()
    }

    /// Decode image `index`.
    pub fn image(&self, index: usize) -> Result<Rgba, String> {
        let bytes = self
            .images
            .get(index)
            .ok_or_else(|| format!("no image {index}"))?
            .as_ref()
            .map_err(Clone::clone)?;
        Rgba::decode_png(bytes).map_err(|e| format!("{} image {index}: {e}", self.path.display()))
    }

    /// The display name of image `index`: its URI, or for an image stored
    /// in a buffer its name (else `bufferView n`) and `(embedded)`.
    pub fn image_name(&self, index: usize) -> String {
        let Some(image) = self.doc.images().nth(index) else {
            return format!("image {index}");
        };
        match image.source() {
            gltf::image::Source::Uri { uri, .. } => uri.to_string(),
            gltf::image::Source::View { view, .. } => match image.name() {
                Some(name) if !name.is_empty() => format!("{name} (embedded)"),
                _ => format!("bufferView {} (embedded)", view.index()),
            },
        }
    }

    /// Each node's local transform as authored.
    pub fn rest_locals(&self) -> Vec<Mat4> {
        self.doc
            .nodes()
            .map(|n| Mat4(n.transform().matrix()))
            .collect()
    }

    /// Each node's parent.
    pub fn parents(&self) -> Vec<Option<usize>> {
        let mut parents = vec![None; self.doc.nodes().count()];
        for n in self.doc.nodes() {
            for c in n.children() {
                parents[c.index()] = Some(n.index());
            }
        }
        parents
    }

    /// The root nodes drawn: the default scene's, else the first scene's,
    /// else every node without a parent.
    pub fn roots(&self) -> Vec<usize> {
        if let Some(scene) = self
            .doc
            .default_scene()
            .or_else(|| self.doc.scenes().next())
        {
            return scene.nodes().map(|n| n.index()).collect();
        }
        let parents = self.parents();
        (0..parents.len())
            .filter(|&i| parents[i].is_none())
            .collect()
    }

    /// World matrices from `locals`, in traversal order from the roots;
    /// `None` for nodes the scene does not reach.
    pub fn worlds(&self, locals: &[Mat4]) -> Result<Vec<Option<Mat4>>, String> {
        let nodes: Vec<gltf::Node> = self.doc.nodes().collect();
        let mut worlds = vec![None; nodes.len()];
        let mut stack: Vec<(usize, Mat4)> = self
            .roots()
            .into_iter()
            .rev()
            .map(|r| (r, Mat4::IDENTITY))
            .collect();
        while let Some((i, parent)) = stack.pop() {
            if worlds[i].is_some() {
                return Err(format!(
                    "{}: node {i} is reached twice (a cycle or a shared child)",
                    self.path.display()
                ));
            }
            let w = parent.mul(&locals[i]);
            worlds[i] = Some(w);
            for c in nodes[i].children().collect::<Vec<_>>().into_iter().rev() {
                stack.push((c.index(), w));
            }
        }
        Ok(worlds)
    }

    /// The order nodes are visited from the roots (depth first).
    pub fn traversal(&self) -> Vec<(usize, usize)> {
        let nodes: Vec<gltf::Node> = self.doc.nodes().collect();
        let mut out = Vec::new();
        let mut seen = vec![false; nodes.len()];
        let mut stack: Vec<(usize, usize)> =
            self.roots().into_iter().rev().map(|r| (r, 0)).collect();
        while let Some((i, depth)) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            out.push((i, depth));
            for c in nodes[i].children().collect::<Vec<_>>().into_iter().rev() {
                stack.push((c.index(), depth + 1));
            }
        }
        out
    }

    /// Every primitive of every mesh node the scene reaches whose node
    /// `keep` accepts, in traversal order, placed in world space with
    /// `locals` (skinned by the node's skin when it has one).
    pub fn prims(&self, locals: &[Mat4], keep: &dyn Fn(&str) -> bool) -> Result<Vec<Prim>, String> {
        let worlds = self.worlds(locals)?;
        let nodes: Vec<gltf::Node> = self.doc.nodes().collect();
        let get = |b: gltf::Buffer| self.buffers.get(b.index()).map(Vec::as_slice);
        let mut out = Vec::new();
        for (i, _) in self.traversal() {
            let node = &nodes[i];
            let Some(mesh) = node.mesh() else { continue };
            let name = node.name().unwrap_or("").to_string();
            if !keep(&name) {
                continue;
            }
            let world = worlds[i].unwrap_or(Mat4::IDENTITY);
            let joint_mats: Option<Vec<Mat4>> = match node.skin() {
                Some(skin) => {
                    let ibms: Vec<Mat4> = match skin.reader(get).read_inverse_bind_matrices() {
                        Some(iter) => iter.map(Mat4).collect(),
                        None => vec![Mat4::IDENTITY; skin.joints().count()],
                    };
                    let mut mats = Vec::new();
                    for (j, joint) in skin.joints().enumerate() {
                        let w = worlds[joint.index()].unwrap_or(Mat4::IDENTITY);
                        let ibm = ibms.get(j).copied().unwrap_or(Mat4::IDENTITY);
                        mats.push(w.mul(&ibm));
                    }
                    Some(mats)
                }
                None => None,
            };
            for p in mesh.primitives() {
                let reader = p.reader(get);
                let positions: Vec<Vec3> = reader
                    .read_positions()
                    .ok_or_else(|| {
                        format!(
                            "{}: node {name:?} mesh {} primitive {} has no POSITION",
                            self.path.display(),
                            mesh.index(),
                            p.index()
                        )
                    })?
                    .collect();
                let normals: Option<Vec<Vec3>> = reader.read_normals().map(Iterator::collect);
                let uvs: Option<Vec<[f32; 2]>> =
                    reader.read_tex_coords(0).map(|t| t.into_f32().collect());
                let (indices, indexed) = match reader.read_indices() {
                    Some(ix) => (ix.into_u32().collect::<Vec<u32>>(), true),
                    None => ((0..positions.len() as u32).collect(), false),
                };
                if let Some(&bad) = indices.iter().find(|&&ix| ix as usize >= positions.len()) {
                    return Err(format!(
                        "{}: node {name:?} index {bad} out of range ({} vertices)",
                        self.path.display(),
                        positions.len()
                    ));
                }
                let skin = match (&joint_mats, reader.read_joints(0), reader.read_weights(0)) {
                    (Some(mats), Some(j), Some(w)) => Some((
                        mats,
                        j.into_u16().collect::<Vec<[u16; 4]>>(),
                        w.into_f32().collect::<Vec<[f32; 4]>>(),
                    )),
                    _ => None,
                };
                let (positions, normals) = match &skin {
                    Some((mats, joints, weights)) => {
                        skin_vertices(mats, joints, weights, &positions, normals.as_deref())?
                    }
                    None => {
                        let n3 = world.normal_matrix();
                        (
                            positions.iter().map(|&v| world.point(v)).collect(),
                            normals.map(|ns| match n3 {
                                Some(n3) => ns.iter().map(|&n| apply_normal(&n3, n)).collect(),
                                None => ns,
                            }),
                        )
                    }
                };
                let base = p.material().pbr_metallic_roughness().base_color_texture();
                out.push(Prim {
                    node: i,
                    node_name: name.clone(),
                    mesh: mesh.index(),
                    primitive: p.index(),
                    triangles_mode: p.mode() == gltf::mesh::Mode::Triangles,
                    mode: format!("{:?}", p.mode()),
                    indexed,
                    positions,
                    normals,
                    uvs,
                    indices,
                    image: base.as_ref().map(|t| t.texture().source().index()),
                    tex_coord: base.as_ref().map_or(0, gltf::texture::Info::tex_coord),
                    skinned: skin.is_some(),
                });
            }
        }
        Ok(out)
    }

    /// Node locals with the clip called `clip` sampled at `at_ms`
    /// (STEP and LINEAR; CUBICSPLINE is refused). Morph-target weight
    /// channels are skipped; the count is returned.
    pub fn posed_locals(&self, clip: &str, at_ms: u32) -> Result<(Vec<Mat4>, usize), String> {
        let Some(anim) = self.doc.animations().find(|a| a.name() == Some(clip)) else {
            let names: Vec<String> = self
                .doc
                .animations()
                .map(|a| format!("{:?}", a.name().unwrap_or("")))
                .collect();
            return Err(format!(
                "{}: no animation {clip:?} (it has {})",
                self.path.display(),
                if names.is_empty() {
                    "none".to_string()
                } else {
                    names.join(", ")
                }
            ));
        };
        let mut trs: Vec<(Vec3, Quat, Vec3)> = self
            .doc
            .nodes()
            .map(|n| n.transform().decomposed())
            .collect();
        let t = at_ms as f32 / 1000.0;
        let get = |b: gltf::Buffer| self.buffers.get(b.index()).map(Vec::as_slice);
        let mut skipped = 0;
        for channel in anim.channels() {
            let node = channel.target().node().index();
            let interp = channel.sampler().interpolation();
            if interp == Interpolation::CubicSpline {
                return Err(format!(
                    "{}: clip {clip:?} channel on node {node} is CUBICSPLINE, which the bake does not sample",
                    self.path.display()
                ));
            }
            let reader = channel.reader(get);
            let inputs: Vec<f32> = reader
                .read_inputs()
                .ok_or("animation input missing")?
                .collect();
            let outputs = reader.read_outputs().ok_or("animation output missing")?;
            match (channel.target().property(), outputs) {
                (Property::Translation, ReadOutputs::Translations(v)) => {
                    let keys: Vec<Vec3> = v.collect();
                    trs[node].0 = sample(&inputs, &keys, t, interp, lerp3)?;
                }
                (Property::Scale, ReadOutputs::Scales(v)) => {
                    let keys: Vec<Vec3> = v.collect();
                    trs[node].2 = sample(&inputs, &keys, t, interp, lerp3)?;
                }
                (Property::Rotation, ReadOutputs::Rotations(v)) => {
                    let keys: Vec<Quat> = v.into_f32().collect();
                    trs[node].1 = sample(&inputs, &keys, t, interp, slerp)?;
                }
                _ => skipped += 1,
            }
        }
        Ok((
            trs.into_iter()
                .map(|(t, r, s)| Mat4::from_trs(t, r, s))
                .collect(),
            skipped,
        ))
    }
}

/// Sample a keyed channel at `t` seconds: before the first key its value,
/// after the last the last's, STEP holds the earlier key, LINEAR blends
/// with `mix`.
pub fn sample<T: Copy>(
    inputs: &[f32],
    keys: &[T],
    t: f32,
    interp: Interpolation,
    mix: fn(T, T, f32) -> T,
) -> Result<T, String> {
    if inputs.is_empty() || inputs.len() != keys.len() {
        return Err(format!(
            "animation has {} times and {} values",
            inputs.len(),
            keys.len()
        ));
    }
    if t <= inputs[0] {
        return Ok(keys[0]);
    }
    let last = inputs.len() - 1;
    if t >= inputs[last] {
        return Ok(keys[last]);
    }
    let k = inputs
        .windows(2)
        .position(|w| w[0] <= t && t < w[1])
        .unwrap_or(last);
    match interp {
        Interpolation::Step => Ok(keys[k]),
        _ => {
            let span = inputs[k + 1] - inputs[k];
            let f = if span > 0.0 {
                (t - inputs[k]) / span
            } else {
                0.0
            };
            Ok(mix(keys[k], keys[k + 1], f))
        }
    }
}

/// Linear blend skinning: each vertex is the weighted sum of its joints'
/// matrices applied to it (weights renormalised to sum 1); normals use
/// each joint's normal matrix and are renormalised.
fn skin_vertices(
    mats: &[Mat4],
    joints: &[[u16; 4]],
    weights: &[[f32; 4]],
    positions: &[Vec3],
    normals: Option<&[Vec3]>,
) -> Result<(Vec<Vec3>, Option<Vec<Vec3>>), String> {
    if joints.len() != positions.len() || weights.len() != positions.len() {
        return Err("JOINTS_0 or WEIGHTS_0 count differs from POSITION".into());
    }
    let normal_mats: Vec<Option<[Vec3; 3]>> = mats.iter().map(Mat4::normal_matrix).collect();
    let mut out_p = Vec::with_capacity(positions.len());
    let mut out_n = normals.map(|n| Vec::with_capacity(n.len()));
    for (v, &p) in positions.iter().enumerate() {
        let total: f32 = weights[v].iter().sum();
        let norm = if total > 0.0 { 1.0 / total } else { 0.0 };
        let mut acc = [0.0f32; 3];
        let mut acc_n = [0.0f32; 3];
        for k in 0..4 {
            let w = weights[v][k] * norm;
            if w == 0.0 {
                continue;
            }
            let j = joints[v][k] as usize;
            let m = mats
                .get(j)
                .ok_or_else(|| format!("joint index {j} out of range ({} joints)", mats.len()))?;
            let q = m.point(p);
            for c in 0..3 {
                acc[c] += w * q[c];
            }
            if let (Some(ns), Some(n3)) = (normals, normal_mats[j]) {
                let n = apply_normal(&n3, ns[v]);
                for c in 0..3 {
                    acc_n[c] += w * n[c];
                }
            }
        }
        out_p.push(acc);
        if let Some(on) = out_n.as_mut() {
            on.push(crate::math::normalize(acc_n));
        }
    }
    Ok((out_p, out_n))
}

/// One primitive in world space.
#[derive(Clone, Debug)]
pub struct Prim {
    /// Node index.
    pub node: usize,
    /// Node name (empty when unnamed).
    pub node_name: String,
    /// Mesh index.
    pub mesh: usize,
    /// Primitive index within the mesh.
    pub primitive: usize,
    /// Whether the topology is a triangle list.
    pub triangles_mode: bool,
    /// The topology's name.
    pub mode: String,
    /// Whether the file gives indices (else `indices` is `0..n`).
    pub indexed: bool,
    /// Positions in world space.
    pub positions: Vec<Vec3>,
    /// Normals in world space, if the primitive has them.
    pub normals: Option<Vec<Vec3>>,
    /// `TEXCOORD_0`, if present.
    pub uvs: Option<Vec<[f32; 2]>>,
    /// Triangle-list indices.
    pub indices: Vec<u32>,
    /// The base-colour texture's image index.
    pub image: Option<usize>,
    /// Which `TEXCOORD_n` the base-colour texture reads.
    pub tex_coord: u32,
    /// Whether it was skinned.
    pub skinned: bool,
}

impl Prim {
    /// Triangle count (0 for other topologies).
    pub fn triangles(&self) -> usize {
        if self.triangles_mode {
            self.indices.len() / 3
        } else {
            0
        }
    }

    /// The `i`th triangle's corner indices.
    pub fn tri(&self, i: usize) -> [usize; 3] {
        [
            self.indices[3 * i] as usize,
            self.indices[3 * i + 1] as usize,
            self.indices[3 * i + 2] as usize,
        ]
    }
}

/// Axis-aligned bounds of a set of points: `(min, max)`.
pub fn bounds<'a>(points: impl IntoIterator<Item = &'a Vec3>) -> Option<(Vec3, Vec3)> {
    let mut it = points.into_iter();
    let first = *it.next()?;
    Some(it.fold((first, first), |(lo, hi), p| {
        (
            [lo[0].min(p[0]), lo[1].min(p[1]), lo[2].min(p[2])],
            [hi[0].max(p[0]), hi[1].max(p[1]), hi[2].max(p[2])],
        )
    }))
}

/// Bounds over every primitive.
pub fn prims_bounds(prims: &[Prim]) -> Option<(Vec3, Vec3)> {
    bounds(prims.iter().flat_map(|p| p.positions.iter()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_must_be_plain_relative_paths() {
        assert!(check_uri("castle.bin").is_ok());
        assert!(check_uri("textures/a.png").is_ok());
        for bad in [
            "data:application/octet-stream;base64,AAAA",
            "../secret.bin",
            "/etc/passwd",
            "https://example.com/x.bin",
            "a%20b.bin",
            "a\\b.bin",
            "",
        ] {
            assert!(check_uri(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn animation_sampling_linear_and_step() {
        let times = [0.0, 1.0, 2.0];
        let keys = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 4.0, 0.0]];
        let at = |t: f32, i: Interpolation| sample(&times, &keys, t, i, lerp3).unwrap();
        assert_eq!(at(0.25, Interpolation::Linear), [0.5, 0.0, 0.0]);
        assert_eq!(at(1.5, Interpolation::Linear), [2.0, 2.0, 0.0]);
        assert_eq!(at(0.75, Interpolation::Step), [0.0, 0.0, 0.0]);
        assert_eq!(at(1.0, Interpolation::Step), [2.0, 0.0, 0.0]);
        assert_eq!(
            at(-1.0, Interpolation::Linear),
            keys[0],
            "before the first key"
        );
        assert_eq!(
            at(9.0, Interpolation::Linear),
            keys[2],
            "after the last key"
        );
        assert!(sample(&times, &keys[..2], 0.5, Interpolation::Linear, lerp3).is_err());
    }
}
