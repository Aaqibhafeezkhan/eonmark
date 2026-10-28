//! A small glTF 2.0 writer: the bake's output (one node, one mesh, one
//! embedded PNG) and the synthetic models the tests build (node trees,
//! skins, animations, URI buffers and textures). The JSON is serialised
//! through `gltf::json`'s serde re-export with a fixed key order, and every
//! output is parsed and validated by `gltf` before it is returned, so the
//! same input always gives the same bytes and nothing invalid is written.

use serde::Serialize;
use std::borrow::Cow;
use std::collections::BTreeMap;

const FLOAT: u32 = 5126;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const NEAREST: u32 = 9728;

#[derive(Serialize)]
struct Root<'a> {
    asset: Asset<'a>,
    scene: usize,
    scenes: Vec<SceneJ>,
    nodes: &'a [Node],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    meshes: &'a [MeshJ],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    materials: &'a [MaterialJ],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    textures: &'a [TextureJ],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    samplers: &'a [SamplerJ],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    images: &'a [ImageJ],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    skins: &'a [SkinJ],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    animations: &'a [AnimationJ],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    accessors: &'a [AccessorJ],
    #[serde(rename = "bufferViews", skip_serializing_if = "<[_]>::is_empty")]
    buffer_views: &'a [ViewJ],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    buffers: Vec<BufferJ<'a>>,
}

#[derive(Serialize)]
struct Asset<'a> {
    version: &'static str,
    generator: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    extras: Option<&'a AssetExtras>,
}

/// What the bake records in `asset.extras` of its output.
#[derive(Clone, Debug, Default, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AssetExtras {
    /// The ATTRIBUTION.md Modifications text.
    pub modifications: String,
    /// The source rig's identity (a hash of its joint names), so `check`
    /// groups baked units of one rig for the 1.4x silhouette rule.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rig: String,
    /// The source file name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

#[derive(Serialize)]
struct SceneJ {
    nodes: Vec<usize>,
}

/// A node.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Node {
    /// Name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Mesh index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mesh: Option<usize>,
    /// Skin index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skin: Option<usize>,
    /// Child node indices.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<usize>,
    /// Translation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translation: Option<[f32; 3]>,
    /// Rotation `[x, y, z, w]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation: Option<[f32; 4]>,
    /// Scale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<[f32; 3]>,
}

#[derive(Serialize)]
struct MeshJ {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    primitives: Vec<PrimJ>,
}

/// One primitive: attribute accessors by semantic, indices and material.
#[derive(Clone, Debug, Default, Serialize)]
pub struct PrimJ {
    /// `POSITION`, `NORMAL`, `TEXCOORD_0`, `JOINTS_0`, `WEIGHTS_0`.
    pub attributes: BTreeMap<&'static str, usize>,
    /// Index accessor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indices: Option<usize>,
    /// Material index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material: Option<usize>,
    /// Topology (4 = triangles, the default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
}

#[derive(Serialize)]
struct MaterialJ {
    #[serde(rename = "pbrMetallicRoughness")]
    pbr: PbrJ,
}

#[derive(Serialize)]
struct PbrJ {
    #[serde(rename = "baseColorTexture", skip_serializing_if = "Option::is_none")]
    base_color_texture: Option<TexInfoJ>,
    #[serde(rename = "metallicFactor")]
    metallic_factor: f32,
    #[serde(rename = "roughnessFactor")]
    roughness_factor: f32,
}

#[derive(Serialize)]
struct TexInfoJ {
    index: usize,
}

#[derive(Serialize)]
struct TextureJ {
    sampler: usize,
    source: usize,
}

#[derive(Serialize)]
struct SamplerJ {
    #[serde(rename = "magFilter")]
    mag_filter: u32,
    #[serde(rename = "minFilter")]
    min_filter: u32,
}

#[derive(Serialize)]
struct ImageJ {
    #[serde(rename = "bufferView", skip_serializing_if = "Option::is_none")]
    buffer_view: Option<usize>,
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    mime_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uri: Option<String>,
}

#[derive(Serialize)]
struct SkinJ {
    joints: Vec<usize>,
    #[serde(
        rename = "inverseBindMatrices",
        skip_serializing_if = "Option::is_none"
    )]
    inverse_bind_matrices: Option<usize>,
}

#[derive(Serialize)]
struct AnimationJ {
    name: String,
    channels: Vec<ChannelJ>,
    samplers: Vec<AnimSamplerJ>,
}

#[derive(Serialize)]
struct ChannelJ {
    sampler: usize,
    target: TargetJ,
}

#[derive(Serialize)]
struct TargetJ {
    node: usize,
    path: &'static str,
}

#[derive(Serialize)]
struct AnimSamplerJ {
    input: usize,
    output: usize,
    interpolation: &'static str,
}

#[derive(Serialize)]
struct AccessorJ {
    #[serde(rename = "bufferView")]
    buffer_view: usize,
    #[serde(rename = "componentType")]
    component_type: u32,
    count: usize,
    #[serde(rename = "type")]
    type_: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<Vec<f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<Vec<f32>>,
}

#[derive(Serialize)]
struct ViewJ {
    buffer: usize,
    #[serde(rename = "byteOffset")]
    byte_offset: usize,
    #[serde(rename = "byteLength")]
    byte_length: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<u32>,
}

#[derive(Serialize)]
struct BufferJ<'a> {
    #[serde(rename = "byteLength")]
    byte_length: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    uri: Option<&'a str>,
}

/// One animation channel: target node, `translation` / `rotation` /
/// `scale`, input and output accessors and `LINEAR` / `STEP` /
/// `CUBICSPLINE`.
pub struct Channel {
    /// Target node.
    pub node: usize,
    /// Target path.
    pub path: &'static str,
    /// Key times accessor.
    pub input: usize,
    /// Key values accessor.
    pub output: usize,
    /// Interpolation name.
    pub interpolation: &'static str,
}

/// Builds a glTF document and its binary buffer.
#[derive(Default)]
pub struct Builder {
    bin: Vec<u8>,
    views: Vec<ViewJ>,
    accessors: Vec<AccessorJ>,
    meshes: Vec<MeshJ>,
    nodes: Vec<Node>,
    materials: Vec<MaterialJ>,
    textures: Vec<TextureJ>,
    samplers: Vec<SamplerJ>,
    images: Vec<ImageJ>,
    skins: Vec<SkinJ>,
    animations: Vec<AnimationJ>,
    roots: Vec<usize>,
}

fn f32_bytes(values: impl IntoIterator<Item = f32>) -> Vec<u8> {
    values.into_iter().flat_map(f32::to_le_bytes).collect()
}

impl Builder {
    /// An empty document.
    pub fn new() -> Builder {
        Builder::default()
    }

    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        self.views.push(ViewJ {
            buffer: 0,
            byte_offset: self.bin.len(),
            byte_length: bytes.len(),
            target,
        });
        self.bin.extend_from_slice(bytes);
        self.views.len() - 1
    }

    fn accessor(
        &mut self,
        bytes: &[u8],
        target: Option<u32>,
        component_type: u32,
        count: usize,
        type_: &'static str,
        min_max: Option<(Vec<f32>, Vec<f32>)>,
    ) -> usize {
        let buffer_view = self.view(bytes, target);
        let (min, max) = match min_max {
            Some((a, b)) => (Some(a), Some(b)),
            None => (None, None),
        };
        self.accessors.push(AccessorJ {
            buffer_view,
            component_type,
            count,
            type_,
            min,
            max,
        });
        self.accessors.len() - 1
    }

    /// A POSITION accessor, with the min and max glTF requires.
    pub fn positions(&mut self, p: &[[f32; 3]]) -> usize {
        let (lo, hi) = crate::model::bounds(p.iter()).unwrap_or(([0.0; 3], [0.0; 3]));
        let bytes = f32_bytes(p.iter().flatten().copied());
        self.accessor(
            &bytes,
            Some(ARRAY_BUFFER),
            FLOAT,
            p.len(),
            "VEC3",
            Some((lo.to_vec(), hi.to_vec())),
        )
    }

    /// A VEC3 float accessor (normals, translation keys).
    pub fn vec3(&mut self, v: &[[f32; 3]], vertex: bool) -> usize {
        let bytes = f32_bytes(v.iter().flatten().copied());
        let target = vertex.then_some(ARRAY_BUFFER);
        self.accessor(&bytes, target, FLOAT, v.len(), "VEC3", None)
    }

    /// A VEC2 float accessor (texture coordinates).
    pub fn vec2(&mut self, v: &[[f32; 2]]) -> usize {
        let bytes = f32_bytes(v.iter().flatten().copied());
        self.accessor(&bytes, Some(ARRAY_BUFFER), FLOAT, v.len(), "VEC2", None)
    }

    /// A VEC4 float accessor (weights, rotation keys).
    pub fn vec4(&mut self, v: &[[f32; 4]], vertex: bool) -> usize {
        let bytes = f32_bytes(v.iter().flatten().copied());
        let target = vertex.then_some(ARRAY_BUFFER);
        self.accessor(&bytes, target, FLOAT, v.len(), "VEC4", None)
    }

    /// A `JOINTS_0` accessor (unsigned short `VEC4`).
    pub fn joints(&mut self, j: &[[u16; 4]]) -> usize {
        let bytes: Vec<u8> = j.iter().flatten().flat_map(|x| x.to_le_bytes()).collect();
        self.accessor(
            &bytes,
            Some(ARRAY_BUFFER),
            UNSIGNED_SHORT,
            j.len(),
            "VEC4",
            None,
        )
    }

    /// A u32 index accessor.
    pub fn indices(&mut self, i: &[u32]) -> usize {
        let bytes: Vec<u8> = i.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.accessor(
            &bytes,
            Some(ELEMENT_ARRAY_BUFFER),
            UNSIGNED_INT,
            i.len(),
            "SCALAR",
            None,
        )
    }

    /// Animation key times, with the min and max glTF requires.
    pub fn times(&mut self, t: &[f32]) -> usize {
        let lo = t.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = t.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let bytes = f32_bytes(t.iter().copied());
        self.accessor(
            &bytes,
            None,
            FLOAT,
            t.len(),
            "SCALAR",
            Some((vec![lo], vec![hi])),
        )
    }

    /// Inverse bind matrices (column-major).
    pub fn mat4s(&mut self, m: &[[f32; 16]]) -> usize {
        let bytes = f32_bytes(m.iter().flatten().copied());
        self.accessor(&bytes, None, FLOAT, m.len(), "MAT4", None)
    }

    /// A nearest-sampled texture over PNG bytes stored in the buffer.
    pub fn png_texture(&mut self, png: &[u8]) -> usize {
        let view = self.view(png, None);
        self.images.push(ImageJ {
            buffer_view: Some(view),
            mime_type: Some("image/png"),
            uri: None,
        });
        self.texture()
    }

    /// A nearest-sampled texture over an image file named by `uri`.
    pub fn uri_texture(&mut self, uri: &str) -> usize {
        self.images.push(ImageJ {
            buffer_view: None,
            mime_type: None,
            uri: Some(uri.to_string()),
        });
        self.texture()
    }

    fn texture(&mut self) -> usize {
        if self.samplers.is_empty() {
            self.samplers.push(SamplerJ {
                mag_filter: NEAREST,
                min_filter: NEAREST,
            });
        }
        self.textures.push(TextureJ {
            sampler: 0,
            source: self.images.len() - 1,
        });
        self.textures.len() - 1
    }

    /// A matte material, textured when `texture` is given.
    pub fn material(&mut self, texture: Option<usize>) -> usize {
        self.materials.push(MaterialJ {
            pbr: PbrJ {
                base_color_texture: texture.map(|index| TexInfoJ { index }),
                metallic_factor: 0.0,
                roughness_factor: 1.0,
            },
        });
        self.materials.len() - 1
    }

    /// A triangle primitive from vertex data; `normals` and `uvs` may be
    /// left out.
    pub fn prim(
        &mut self,
        positions: &[[f32; 3]],
        normals: Option<&[[f32; 3]]>,
        uvs: Option<&[[f32; 2]]>,
        indices: &[u32],
        material: Option<usize>,
    ) -> PrimJ {
        let mut attributes = BTreeMap::new();
        attributes.insert("POSITION", self.positions(positions));
        if let Some(n) = normals {
            attributes.insert("NORMAL", self.vec3(n, true));
        }
        if let Some(uv) = uvs {
            attributes.insert("TEXCOORD_0", self.vec2(uv));
        }
        PrimJ {
            attributes,
            indices: Some(self.indices(indices)),
            material,
            mode: None,
        }
    }

    /// A mesh.
    pub fn mesh(&mut self, name: Option<&str>, primitives: Vec<PrimJ>) -> usize {
        self.meshes.push(MeshJ {
            name: name.map(str::to_string),
            primitives,
        });
        self.meshes.len() - 1
    }

    /// A node; returns its index.
    pub fn node(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    /// Make `node` a root of the scene.
    pub fn root(&mut self, node: usize) {
        self.roots.push(node);
    }

    /// A skin over `joints` with inverse bind matrices.
    pub fn skin(&mut self, joints: Vec<usize>, ibms: &[[f32; 16]]) -> usize {
        let acc = self.mat4s(ibms);
        self.skins.push(SkinJ {
            joints,
            inverse_bind_matrices: Some(acc),
        });
        self.skins.len() - 1
    }

    /// An animation clip.
    pub fn animation(&mut self, name: &str, channels: Vec<Channel>) {
        let mut out = AnimationJ {
            name: name.to_string(),
            channels: Vec::new(),
            samplers: Vec::new(),
        };
        for (i, c) in channels.into_iter().enumerate() {
            out.samplers.push(AnimSamplerJ {
                input: c.input,
                output: c.output,
                interpolation: c.interpolation,
            });
            out.channels.push(ChannelJ {
                sampler: i,
                target: TargetJ {
                    node: c.node,
                    path: c.path,
                },
            });
        }
        self.animations.push(out);
    }

    fn json(&self, extras: Option<&AssetExtras>, uri: Option<&str>) -> Result<Vec<u8>, String> {
        let root = Root {
            asset: Asset {
                version: "2.0",
                generator: "eonmark tools/inspect_gltf",
                extras,
            },
            scene: 0,
            scenes: vec![SceneJ {
                nodes: self.roots.clone(),
            }],
            nodes: &self.nodes,
            meshes: &self.meshes,
            materials: &self.materials,
            textures: &self.textures,
            samplers: &self.samplers,
            images: &self.images,
            skins: &self.skins,
            animations: &self.animations,
            accessors: &self.accessors,
            buffer_views: &self.views,
            buffers: if self.bin.is_empty() {
                Vec::new()
            } else {
                vec![BufferJ {
                    byte_length: self.bin.len(),
                    uri,
                }]
            },
        };
        gltf::json::serialize::to_vec(&root).map_err(|e| format!("json: {e}"))
    }

    /// The document as a GLB with the buffer embedded, validated by
    /// `gltf` before it is returned.
    pub fn glb(&self, extras: Option<&AssetExtras>) -> Result<Vec<u8>, String> {
        let json = self.json(extras, None)?;
        let glb = gltf::Glb {
            header: gltf::binary::Header {
                magic: *b"glTF",
                version: 2,
                length: 0,
            },
            json: Cow::Owned(json),
            bin: (!self.bin.is_empty()).then_some(Cow::Borrowed(self.bin.as_slice())),
        };
        let bytes = glb.to_vec().map_err(|e| format!("glb: {e}"))?;
        gltf::Gltf::from_slice(&bytes)
            .map_err(|e| format!("the written GLB does not validate: {e}"))?;
        Ok(bytes)
    }

    /// The document as `.gltf` JSON whose buffer is the file `bin_uri`,
    /// plus that file's bytes.
    pub fn gltf(&self, bin_uri: &str) -> Result<(Vec<u8>, Vec<u8>), String> {
        let json = self.json(None, Some(bin_uri))?;
        gltf::Gltf::from_slice(&json)
            .map_err(|e| format!("the written glTF does not validate: {e}"))?;
        Ok((json, self.bin.clone()))
    }
}
