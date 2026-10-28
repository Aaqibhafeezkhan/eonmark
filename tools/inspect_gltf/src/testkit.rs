//! Synthetic models for the tests, written with [`crate::writer`]: an 8x8
//! atlas of four 4x4 colour blocks, rows of quads, boxes and a two-joint
//! rig with a clip.

use crate::image::Rgba;
use crate::math::Mat4;
use crate::writer::{Builder, Channel, Node, PrimJ};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The repository root.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A scratch directory removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    /// A new empty directory named after `tag`.
    pub fn new(tag: &str) -> TempDir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "inspect_gltf-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }

    /// Its path.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The 8x8 test atlas: red top left, blue top right, green bottom left,
/// cream bottom right; `detail` paints texel (0, 0) yellow.
pub fn atlas(detail: bool) -> Rgba {
    let mut img = Rgba::filled(8, 8, [0; 4]);
    for y in 0..8 {
        for x in 0..8 {
            let c = match (x < 4, y < 4) {
                (true, true) => [200, 60, 50, 255],
                (false, true) => [50, 90, 200, 255],
                (true, false) => [60, 160, 60, 255],
                (false, false) => [220, 220, 200, 255],
            };
            img.set(x, y, c);
        }
    }
    if detail {
        img.set(0, 0, [250, 220, 0, 255]);
    }
    img
}

/// The UV of texel coordinates `(x, y)` in the 8x8 atlas.
fn uv(t: [f32; 2]) -> [f32; 2] {
    [t[0] / 8.0, t[1] / 8.0]
}

/// `<name>.gltf` + `<name>.bin` + `atlas.png` in `dir`: one quad node
/// `quad<i>` per entry, side by side along +X, every corner sampling the
/// atlas at texel coordinates `texels[i]`; `lift` raises the geometry.
pub fn quads_gltf(dir: &TempDir, name: &str, texels: &[[f32; 2]], lift: f32) -> PathBuf {
    std::fs::write(
        dir.path().join("atlas.png"),
        atlas(false).encode_png().unwrap(),
    )
    .unwrap();
    let mut b = Builder::new();
    let tex = b.uri_texture("atlas.png");
    let mat = b.material(Some(tex));
    let y = lift;
    for (i, t) in texels.iter().enumerate() {
        let x = i as f32 * 2.0;
        let p = [
            [x, y, 0.0],
            [x + 1.0, y, 0.0],
            [x + 1.0, y + 1.0, 0.0],
            [x, y + 1.0, 0.0],
        ];
        let n = [[0.0, 0.0, 1.0]; 4];
        let uvs = [uv(*t); 4];
        let prim = b.prim(&p, Some(&n), Some(&uvs), &[0, 1, 2, 0, 2, 3], Some(mat));
        let mesh = b.mesh(None, vec![prim]);
        let node = b.node(Node {
            name: Some(format!("quad{i}")),
            mesh: Some(mesh),
            ..Node::default()
        });
        b.root(node);
    }
    let (json, bin) = b.gltf(&format!("{name}.bin")).unwrap();
    std::fs::write(dir.path().join(format!("{name}.bin")), bin).unwrap();
    let path = dir.path().join(format!("{name}.gltf"));
    std::fs::write(&path, json).unwrap();
    path
}

/// `<name>.gltf` + `.bin` + `atlas.png` in `dir`: a box of `size`
/// centred on x/z with its base at `base_y`. Its four sides sample texels
/// 5..7 x 1..2 (inside the blue block 4..8 x 0..4) when `team`, else the
/// green block; top and bottom sample the green block.
pub fn box_gltf(dir: &Path, name: &str, size: [f32; 3], base_y: f32, team: bool) -> PathBuf {
    box_gltf_with(dir, name, size, base_y, team, "atlas.png")
}

/// [`box_gltf`] with the test atlas written as `texture` (a file name in
/// `dir`), so a test can give it a name such as a shipped atlas's.
pub fn box_gltf_with(
    dir: &Path,
    name: &str,
    size: [f32; 3],
    base_y: f32,
    team: bool,
    texture: &str,
) -> PathBuf {
    std::fs::write(dir.join(texture), atlas(false).encode_png().unwrap()).unwrap();
    let [w, h, d] = size;
    let (x0, x1, y0, y1, z0, z1) = (-w / 2.0, w / 2.0, base_y, base_y + h, -d / 2.0, d / 2.0);
    // Each face: four corners and a normal.
    let faces: [([[f32; 3]; 4], [f32; 3], bool); 6] = [
        (
            [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
            [1.0, 0.0, 0.0],
            true,
        ),
        (
            [[x0, y0, z1], [x0, y1, z1], [x0, y1, z0], [x0, y0, z0]],
            [-1.0, 0.0, 0.0],
            true,
        ),
        (
            [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
            [0.0, 0.0, 1.0],
            true,
        ),
        (
            [[x1, y0, z0], [x0, y0, z0], [x0, y1, z0], [x1, y1, z0]],
            [0.0, 0.0, -1.0],
            true,
        ),
        (
            [[x0, y1, z1], [x1, y1, z1], [x1, y1, z0], [x0, y1, z0]],
            [0.0, 1.0, 0.0],
            false,
        ),
        (
            [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
            [0.0, -1.0, 0.0],
            false,
        ),
    ];
    let (mut p, mut n, mut t, mut ix) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (corners, normal, side) in faces {
        let base = p.len() as u32;
        let texels: [[f32; 2]; 4] = if side && team {
            [[5.2, 1.2], [6.8, 1.2], [6.8, 1.8], [5.2, 1.8]]
        } else {
            [[1.2, 5.2], [2.8, 5.2], [2.8, 5.8], [1.2, 5.8]]
        };
        for (c, tx) in corners.iter().zip(texels) {
            p.push(*c);
            n.push(normal);
            t.push(uv(tx));
        }
        ix.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let mut b = Builder::new();
    let tex = b.uri_texture(texture);
    let mat = b.material(Some(tex));
    let prim = b.prim(&p, Some(&n), Some(&t), &ix, Some(mat));
    let mesh = b.mesh(Some(name), vec![prim]);
    let node = b.node(Node {
        name: Some(name.to_string()),
        mesh: Some(mesh),
        ..Node::default()
    });
    b.root(node);
    let (json, bin) = b.gltf(&format!("{name}.bin")).unwrap();
    std::fs::write(dir.join(format!("{name}.bin")), bin).unwrap();
    let path = dir.join(format!("{name}.gltf"));
    std::fs::write(&path, json).unwrap();
    path
}

/// A GLB with a two-joint rig: `Root` at the origin and `Upper` at
/// (0, 1, 0); a skinned `Body` strip with vertices (0, 0, 0) on `Root`,
/// (0, 1.5, 0) half on each and (0, 2, 0) on `Upper`, plus (0.2, 0, 0.3)
/// on `Root`; an unskinned one-triangle `Sword` under `Upper`; and a clip
/// `Bend` turning `Upper` 90 degrees about +Z from 0 s to 1 s with
/// `interpolation`. The texture is the test atlas (`detail` as in
/// [`atlas`]); `Body` samples texel (0, 0), `Sword` texel (5, 1).
pub fn rig_glb(interpolation: &'static str, detail: bool) -> Vec<u8> {
    let mut b = Builder::new();
    let tex = b.png_texture(&atlas(detail).encode_png().unwrap());
    let mat = b.material(Some(tex));
    let body_p = [
        [0.0, 0.0, 0.0],
        [0.0, 1.5, 0.0],
        [0.0, 2.0, 0.0],
        [0.2, 0.0, 0.3],
    ];
    let body_n = [[0.0, 0.0, 1.0]; 4];
    let body_uv = [uv([0.5, 0.5]); 4];
    let mut body: PrimJ = b.prim(
        &body_p,
        Some(&body_n),
        Some(&body_uv),
        &[0, 3, 1, 1, 3, 2],
        Some(mat),
    );
    let joints = b.joints(&[[0, 0, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0], [0, 0, 0, 0]]);
    let weights = b.vec4(
        &[
            [1.0, 0.0, 0.0, 0.0],
            [0.5, 0.5, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
        ],
        true,
    );
    body.attributes.insert("JOINTS_0", joints);
    body.attributes.insert("WEIGHTS_0", weights);
    let body_mesh = b.mesh(Some("Body"), vec![body]);
    let sword_p = [[0.0, 0.0, 0.0], [0.1, 0.0, 0.0], [0.0, 0.5, 0.0]];
    let sword = b.prim(
        &sword_p,
        Some(&[[0.0, 0.0, 1.0]; 3]),
        Some(&[uv([5.5, 1.5]); 3]),
        &[0, 1, 2],
        Some(mat),
    );
    let sword_mesh = b.mesh(Some("Sword"), vec![sword]);
    // Nodes: 0 Root, 1 Upper, 2 Body, 3 Sword.
    b.node(Node {
        name: Some("Root".into()),
        children: vec![1],
        ..Node::default()
    });
    b.node(Node {
        name: Some("Upper".into()),
        translation: Some([0.0, 1.0, 0.0]),
        children: vec![3],
        ..Node::default()
    });
    b.node(Node {
        name: Some("Body".into()),
        mesh: Some(body_mesh),
        skin: Some(0),
        ..Node::default()
    });
    b.node(Node {
        name: Some("Sword".into()),
        mesh: Some(sword_mesh),
        translation: Some([0.0, 0.5, 0.0]),
        ..Node::default()
    });
    b.root(0);
    b.root(2);
    let ibm = [
        Mat4::IDENTITY.to_cols_array(),
        Mat4::translation([0.0, -1.0, 0.0]).to_cols_array(),
    ];
    b.skin(vec![0, 1], &ibm);
    let half = std::f32::consts::FRAC_1_SQRT_2;
    let input = b.times(&[0.0, 1.0]);
    let keys = [[0.0, 0.0, 0.0, 1.0], [0.0, 0.0, half, half]];
    let output = if interpolation == "CUBICSPLINE" {
        let zero = [0.0; 4];
        b.vec4(&[zero, keys[0], zero, zero, keys[1], zero], false)
    } else {
        b.vec4(&keys, false)
    };
    b.animation(
        "Bend",
        vec![Channel {
            node: 1,
            path: "rotation",
            input,
            output,
            interpolation,
        }],
    );
    b.glb(None).unwrap()
}
