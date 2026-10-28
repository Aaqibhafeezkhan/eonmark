//! glTF models from `data/visuals.ron` (art pass, slices 3 and 4): every Scene's
//! files are loaded once through Bevy's asset server, each Scene is merged
//! into one placed mesh, and the sim waits until every model is ready or
//! has failed (the load gate, [`models_settled`], joined with `&&` into
//! `sim_driver::match_running`).
//!
//! - [`request_models`] (`Startup`) loads every distinct file of every Scene
//!   the game draws as a model (the units since slice 4, the buildings, the
//!   sites, the rubble, the Seat flag, the team banner and the terrain
//!   props) with `load_cameras`, `load_lights` and
//!   `load_animations` off, the source meshes kept in the main world only,
//!   and a nearest sampler forced on every texture (`override_sampler`).
//! - [`finish_models`] (`Update`) waits until every file of an entry is
//!   loaded with its dependencies, then [`merge_nodes`] walks each file's
//!   node tree from its roots (the nodes that are nobody's child),
//!   composes node transforms, keeps the nodes whose own name a `nodes`
//!   glob matches (`rules::visuals::keeps_node`), and appends `POSITION`,
//!   `NORMAL`, `TEXCOORD_0` and `u32` indices placed by the Scene's yaw,
//!   scale and offsets ([`scene_affine`]; parts by [`part_affine`] first),
//!   exactly as `tools/inspect_gltf check` places them. A missing
//!   attribute, a topology other than triangles, missing indices, two base
//!   textures (compared by content) or two base colours fail the entry. A
//!   failed load, a failed merge or more than [`MODEL_LOAD_TIMEOUT_SECS`]
//!   of real time marks the entry failed with one `error!`, and the game
//!   draws the fallback (units their Scene's `fallback` look, buildings
//!   and sites their M3b box; the rubble, flags, banners and props
//!   nothing), so `scripts/render_smoke.sh`
//!   fails on the ERROR line while a player still sees a look.
//! - Textures are deduplicated by content ([`content_hash`]): `KayKit` ships
//!   the same `hexagons_medieval.png` in every folder, and every model on
//!   it shares one neutral material and one material per team
//!   ([`ModelLibrary::material`]). A team material's texture is the source
//!   with the swatch rects repainted in `palette::team_color` and the
//!   rest desaturated ([`team_texture`]).
//!
//! Once every entry has settled the library logs
//! `models: settled ready=<n> failed=<m> ms=<t>` once. With no
//! [`ModelLibrary`] (headless runs, `cargo test -p game`) the gate counts
//! as settled and every look is its fallback.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

use bevy::asset::{LoadState, RecursiveDependencyLoadState, RenderAssetUsages};
use bevy::gltf::{Gltf, GltfLoaderSettings, GltfMaterial, GltfMesh, GltfNode};
use bevy::image::ImageSamplerDescriptor;
use bevy::math::{Affine3A, Mat3A, Vec3A};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use rules::visuals::keeps_node;
use rules::{Part, TeamSwatch, Visual, Visuals};

use crate::palette;
use crate::present::{MATTE_REFLECTANCE, MATTE_ROUGHNESS};
use crate::sim_driver::SimHandle;

/// Longest a model may take to load, seconds of real time, before it is
/// marked failed and drawn as its fallback (a stuck load must not freeze
/// the sim behind the gate).
pub const MODEL_LOAD_TIMEOUT_SECS: f64 = 10.0;

/// The mesh-defining part of a Scene: two Scenes with equal keys share one
/// merged mesh (their `team` and `fallback` do not change the mesh).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelKey {
    /// The main file.
    pub path: String,
    /// Its node globs.
    pub nodes: Vec<String>,
    /// Composited files: path, globs, offset, scale, yaw.
    pub parts: Vec<(String, Vec<String>, (i32, i32, i32), u32, i32)>,
    /// Uniform scale, hundredths.
    pub scale_x100: u32,
    /// Yaw, degrees.
    pub yaw_deg: i32,
    /// `(x, y, z)` offsets, hundredths of a tile.
    pub offset_x100: (i32, i32, i32),
}

impl ModelKey {
    /// The key of a `Visual::Scene`; `None` for anything else.
    pub fn of(visual: &Visual) -> Option<ModelKey> {
        let Visual::Scene {
            path,
            scale_x100,
            y_offset_tiles_x100,
            yaw_deg,
            x_offset_tiles_x100,
            z_offset_tiles_x100,
            nodes,
            parts,
            ..
        } = visual
        else {
            return None;
        };
        Some(ModelKey {
            path: path.clone(),
            nodes: nodes.clone(),
            parts: parts
                .iter()
                .map(|p| {
                    (
                        p.path.clone(),
                        p.nodes.clone(),
                        p.offset_x100,
                        p.scale_x100,
                        p.yaw_deg,
                    )
                })
                .collect(),
            scale_x100: *scale_x100,
            yaw_deg: *yaw_deg,
            offset_x100: (
                *x_offset_tiles_x100,
                *y_offset_tiles_x100,
                *z_offset_tiles_x100,
            ),
        })
    }

    /// Every file this key reads: the main one, then its parts'.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.path.as_str()).chain(self.parts.iter().map(|p| p.0.as_str()))
    }
}

/// A merged, placed model: one mesh in the Scene's placed space (yaw,
/// scale and offsets baked in), standing on the ground at the origin.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// The merged mesh (main and render world: units and buildings are
    /// picked by ray-casting it).
    pub mesh: Handle<Mesh>,
    /// Content hash of its base texture ([`content_hash`]).
    pub texture: u64,
    /// The glTF material's base colour factor.
    pub base_color: Color,
    /// Lower corner of its bounds, metres.
    pub min: Vec3,
    /// Upper corner of its bounds, metres.
    pub max: Vec3,
    /// The largest horizontal distance of a vertex from the origin, metres:
    /// the radius any yaw of the model stays within (terrain props).
    pub reach: f32,
    /// Triangles.
    pub triangles: u32,
}

impl Model {
    /// Height of its top above the ground, metres.
    pub fn top_y(&self) -> f32 {
        self.max.y
    }

    /// The size of its bounds, metres.
    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }
}

/// Where one entry of the library stands.
#[derive(Debug)]
pub enum ModelState {
    /// Waiting for its files (main file first).
    Loading(Vec<(String, Handle<Gltf>)>),
    /// Merged and placed.
    Ready(Model),
    /// Its load or merge failed or timed out; the fallback is drawn.
    Failed,
}

/// Material key: a texture's content hash and, for a team material, the
/// swatch key and the team slot.
type MaterialKey = (u64, Option<(String, u8)>);

/// Every model the game draws, by [`ModelKey`], with the textures and
/// materials they share. Inserted by [`ModelsPlugin`] (only in the
/// windowed game, where `DefaultPlugins` provide the asset server).
#[derive(Resource, Default)]
pub struct ModelLibrary {
    entries: BTreeMap<ModelKey, ModelState>,
    /// One source image per texture content hash (its CPU data is kept for
    /// the team repaint).
    images: BTreeMap<u64, Handle<Image>>,
    /// The team swatches of `visuals.ron`.
    swatches: BTreeMap<String, TeamSwatch>,
    /// `visuals.ron`'s `neutral_desaturate_permille`: how far a neutral
    /// material's texture is desaturated.
    neutral_desaturate_permille: u32,
    materials: BTreeMap<MaterialKey, Handle<StandardMaterial>>,
    /// Real time the requests went out, seconds.
    started: Option<f64>,
    settled_logged: bool,
    /// `error!` lines logged for failed entries (one per entry).
    errors_logged: u32,
}

impl ModelLibrary {
    /// A library for `visuals`' swatches with no entries.
    pub fn new(visuals: &Visuals) -> Self {
        Self {
            swatches: visuals.team_swatches.clone(),
            neutral_desaturate_permille: visuals.neutral_desaturate_permille,
            ..Self::default()
        }
    }

    /// The ready model of `visual` (a Scene), if it loaded.
    pub fn model(&self, visual: &Visual) -> Option<&Model> {
        match self.entries.get(&ModelKey::of(visual)?) {
            Some(ModelState::Ready(model)) => Some(model),
            _ => None,
        }
    }

    /// The state of `visual`'s entry.
    pub fn state(&self, visual: &Visual) -> Option<&ModelState> {
        self.entries.get(&ModelKey::of(visual)?)
    }

    /// No entry is still loading.
    pub fn is_settled(&self) -> bool {
        !self
            .entries
            .values()
            .any(|s| matches!(s, ModelState::Loading(_)))
    }

    /// `(ready, failed)` entries.
    pub fn counts(&self) -> (usize, usize) {
        let ready = self
            .entries
            .values()
            .filter(|s| matches!(s, ModelState::Ready(_)))
            .count();
        let failed = self
            .entries
            .values()
            .filter(|s| matches!(s, ModelState::Failed))
            .count();
        (ready, failed)
    }

    /// `error!` lines logged so far for failed entries.
    pub fn errors_logged(&self) -> u32 {
        self.errors_logged
    }

    /// Put `model` in as ready under `key` (tests, and [`finish_models`]),
    /// with `image` as the source of its texture.
    pub fn insert_ready(&mut self, key: ModelKey, model: Model, image: Handle<Image>) {
        self.images.entry(model.texture).or_insert(image);
        self.entries.insert(key, ModelState::Ready(model));
    }

    /// Start loading `key` with the files' handles (tests use weak or
    /// default handles that never load).
    pub fn insert_loading(&mut self, key: ModelKey, gltfs: Vec<(String, Handle<Gltf>)>) {
        self.entries.insert(key, ModelState::Loading(gltfs));
    }

    /// Mark `key` failed with `reason`, logging one `error!` the first
    /// time (a failed entry stays failed).
    pub fn fail(&mut self, key: &ModelKey, reason: &str) {
        if let Some(state) = self.entries.get_mut(key)
            && !matches!(state, ModelState::Failed)
        {
            *state = ModelState::Failed;
            self.errors_logged += 1;
            error!("model {}: {reason}; drawing the fallback", key.path);
        }
    }

    /// The material for `model`: neutral when `team` is `None` (the texture
    /// as shipped, or desaturated by `neutral_desaturate_permille`), else
    /// the texture repainted for `(swatch key, team slot)`. Created once per texture content and team, so every
    /// model on one texture shares it. A swatch the library does not know
    /// or a texture without CPU data gives the neutral material, with one
    /// `error!`.
    pub fn material(
        &mut self,
        model: &Model,
        team: Option<(&str, u8)>,
        images: &mut Assets<Image>,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        let key: MaterialKey = (model.texture, team.map(|(k, t)| (k.to_string(), t)));
        if let Some(handle) = self.materials.get(&key) {
            return handle.clone();
        }
        let source = self.images.get(&model.texture).cloned();
        let texture = match (team, &source) {
            (Some((swatch_key, slot)), Some(src)) => {
                let repainted = self.swatches.get(swatch_key).and_then(|swatch| {
                    images
                        .get(src)
                        .and_then(|img| team_texture(img, swatch, palette::team_color(slot)))
                });
                match repainted {
                    Some(img) => Some(images.add(img)),
                    None => {
                        self.errors_logged += 1;
                        error!(
                            "team texture {swatch_key:?} for team {slot}: no swatch or no CPU texture data; drawing the texture as shipped"
                        );
                        source.clone()
                    }
                }
            }
            (None, Some(src)) if self.neutral_desaturate_permille > 0 => {
                match images
                    .get(src)
                    .and_then(|img| neutral_texture(img, self.neutral_desaturate_permille))
                {
                    Some(img) => Some(images.add(img)),
                    None => source.clone(),
                }
            }
            _ => source.clone(),
        };
        let handle = materials.add(StandardMaterial {
            base_color: model.base_color,
            base_color_texture: texture,
            perceptual_roughness: MATTE_ROUGHNESS,
            reflectance: MATTE_REFLECTANCE,
            metallic: 0.0,
            ..default()
        });
        self.materials.insert(key, handle.clone());
        handle
    }

    /// Distinct materials created so far.
    pub fn material_count(&self) -> usize {
        self.materials.len()
    }
}

/// Run condition: models have settled (every entry is ready or failed), or
/// there is no [`ModelLibrary`] at all (headless runs and tests). Joined
/// with `&&` into `sim_driver::match_running`, so the sim does not step and no
/// building or unit is spawned before its model can be drawn.
pub fn models_settled(lib: Option<Res<ModelLibrary>>) -> bool {
    lib.is_none_or(|l| l.is_settled())
}

/// Every Scene the game draws as a model, in file order: units (art
/// slice 4), buildings, sites, rubble, Seat flag, banner, terrain props.
pub fn drawn_scenes(visuals: &Visuals) -> Vec<&Visual> {
    let mut out: Vec<&Visual> = visuals.units.values().collect();
    out.extend(visuals.buildings.values());
    out.extend(visuals.sites.iter());
    out.extend(visuals.rubble.as_ref().map(|r| &r.visual));
    out.extend(visuals.seat_flag.as_ref());
    out.extend(visuals.banner.as_ref());
    for decor in visuals.terrain.values() {
        out.extend(decor.looks.iter().map(|l| &l.visual));
    }
    out.retain(|v| matches!(v, Visual::Scene { .. }));
    out
}

/// Loads, merges and gates models. Added only by `app::run`.
pub struct ModelsPlugin;

impl Plugin for ModelsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, request_models)
            .add_systems(Update, finish_models);
    }
}

/// `Startup`: a [`ModelLibrary`] with one loading entry per distinct
/// [`ModelKey`] of [`drawn_scenes`], each file requested once.
pub fn request_models(
    sim: NonSend<SimHandle>,
    server: Res<AssetServer>,
    time: Res<Time<Real>>,
    mut commands: Commands,
) {
    let visuals = &sim.rules().visuals;
    let mut lib = ModelLibrary::new(visuals);
    lib.started = Some(time.elapsed_secs_f64());
    let mut handles: BTreeMap<String, Handle<Gltf>> = BTreeMap::new();
    for visual in drawn_scenes(visuals) {
        let Some(key) = ModelKey::of(visual) else {
            continue;
        };
        if lib.entries.contains_key(&key) {
            continue;
        }
        let gltfs = key
            .files()
            .map(|file| {
                let handle = handles
                    .entry(file.to_string())
                    .or_insert_with(|| {
                        server
                            .load_builder()
                            .with_settings(|s: &mut GltfLoaderSettings| {
                                s.load_meshes = RenderAssetUsages::MAIN_WORLD;
                                s.load_cameras = false;
                                s.load_lights = false;
                                s.load_animations = false;
                                s.override_sampler = true;
                                s.default_sampler = Some(ImageSamplerDescriptor::nearest());
                            })
                            .load(file.to_string())
                    })
                    .clone();
                (file.to_string(), handle)
            })
            .collect();
        lib.insert_loading(key, gltfs);
    }
    info!(
        "models: requested {} model(s) from {} file(s)",
        lib.entries.len(),
        handles.len()
    );
    commands.insert_resource(lib);
}

/// What waiting on an entry's files gives.
#[derive(Debug, PartialEq)]
pub enum Wait {
    /// Every file is loaded with its dependencies.
    Loaded,
    /// Still loading, within the timeout.
    Pending,
    /// A file failed, or the timeout passed: the reason.
    Failed(String),
}

/// The wait on an entry whose files are in `states` (path, its own load
/// state, its dependencies' recursive state) after `elapsed` seconds.
pub fn wait_on(states: &[(String, LoadState, RecursiveDependencyLoadState)], elapsed: f64) -> Wait {
    for (path, own, deps) in states {
        if let LoadState::Failed(e) = own {
            return Wait::Failed(format!("{path} failed to load: {e}"));
        }
        if let RecursiveDependencyLoadState::Failed(e) = deps {
            return Wait::Failed(format!("a dependency of {path} failed to load: {e}"));
        }
    }
    let loaded = states.iter().all(|(_, own, deps)| {
        matches!(own, LoadState::Loaded) && matches!(deps, RecursiveDependencyLoadState::Loaded)
    });
    if loaded {
        Wait::Loaded
    } else if elapsed > MODEL_LOAD_TIMEOUT_SECS {
        Wait::Failed(format!(
            "not loaded after {MODEL_LOAD_TIMEOUT_SECS} s of real time"
        ))
    } else {
        Wait::Pending
    }
}

/// The glTF assets [`finish_models`] reads.
#[derive(bevy::ecs::system::SystemParam)]
pub struct GltfAssets<'w> {
    gltfs: Res<'w, Assets<Gltf>>,
    nodes: Res<'w, Assets<GltfNode>>,
    meshes: Res<'w, Assets<GltfMesh>>,
    materials: Res<'w, Assets<GltfMaterial>>,
}

/// `Update`: settle every loading entry whose files are loaded (merge and
/// place it), failed or past the timeout, then log the settled line once.
pub fn finish_models(
    server: Option<Res<AssetServer>>,
    time: Res<Time<Real>>,
    gltf: Option<GltfAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    images: Res<Assets<Image>>,
    lib: Option<ResMut<ModelLibrary>>,
) {
    let Some(mut lib) = lib else {
        return;
    };
    let now = time.elapsed_secs_f64();
    let started = *lib.started.get_or_insert(now);
    let elapsed = now - started;
    let loading: Vec<ModelKey> = lib
        .entries
        .iter()
        .filter(|(_, s)| matches!(s, ModelState::Loading(_)))
        .map(|(k, _)| k.clone())
        .collect();
    for key in loading {
        let Some(ModelState::Loading(files)) = lib.entries.get(&key) else {
            continue;
        };
        let states: Vec<(String, LoadState, RecursiveDependencyLoadState)> = files
            .iter()
            .map(|(path, h)| match &server {
                Some(server) => (
                    path.clone(),
                    server.load_state(h),
                    server.recursive_dependency_load_state(h),
                ),
                None => (
                    path.clone(),
                    LoadState::NotLoaded,
                    RecursiveDependencyLoadState::NotLoaded,
                ),
            })
            .collect();
        match wait_on(&states, elapsed) {
            Wait::Pending => {}
            Wait::Failed(reason) => lib.fail(&key, &reason),
            Wait::Loaded => {
                let Some(assets) = gltf.as_ref() else {
                    lib.fail(&key, "no glTF assets");
                    continue;
                };
                let handles: Vec<Handle<Gltf>> = files.iter().map(|(_, h)| h.clone()).collect();
                match build_model(&key, &handles, assets, &mut meshes, &images) {
                    Ok((model, image)) => lib.insert_ready(key, model, image),
                    Err(e) => lib.fail(&key, &e.to_string()),
                }
            }
        }
    }
    if !lib.settled_logged && lib.is_settled() {
        lib.settled_logged = true;
        let (ready, failed) = lib.counts();
        info!(
            "models: settled ready={ready} failed={failed} ms={:.0}",
            elapsed * 1000.0
        );
    }
}

/// Why a merge failed.
#[derive(Debug, Clone, PartialEq)]
pub enum MergeError {
    /// A file's asset is gone.
    Missing(String),
    /// A primitive is not a triangle list.
    Topology(String),
    /// A primitive lacks an attribute or has it in an unexpected format.
    Attribute(String),
    /// A primitive has no indices.
    NotIndexed(String),
    /// Two base textures (by content), or a primitive without one.
    Texture(String),
    /// Two base colours.
    BaseColor(String),
    /// No node matched the globs.
    Empty(String),
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeError::Missing(s) => write!(f, "{s} is not loaded"),
            MergeError::Topology(s) => write!(f, "{s} is not a triangle list"),
            MergeError::NotIndexed(s) => write!(f, "{s} is not indexed"),
            MergeError::Attribute(s)
            | MergeError::Texture(s)
            | MergeError::BaseColor(s)
            | MergeError::Empty(s) => write!(f, "{s}"),
        }
    }
}

/// One glTF node as [`merge_nodes`] reads it.
#[derive(Clone, Debug)]
pub struct SourceNode {
    /// The node's name (Bevy names an unnamed node `GltfNode<index>`).
    pub name: String,
    /// Its local transform.
    pub local: Affine3A,
    /// Indices of its children in the same list.
    pub children: Vec<usize>,
    /// Its mesh's primitives.
    pub prims: Vec<SourcePrim>,
}

/// One primitive of a [`SourceNode`].
#[derive(Clone, Debug)]
pub struct SourcePrim {
    /// The primitive's mesh.
    pub mesh: Handle<Mesh>,
    /// Its base-colour texture.
    pub texture: Option<Handle<Image>>,
    /// Its base-colour factor.
    pub base_color: Color,
}

/// The merged geometry of a model before it becomes a mesh.
#[derive(Clone, Debug, Default)]
pub struct Geometry {
    /// Placed positions.
    pub positions: Vec<[f32; 3]>,
    /// Placed, renormalised normals.
    pub normals: Vec<[f32; 3]>,
    /// Texture coordinates.
    pub uvs: Vec<[f32; 2]>,
    /// Triangle indices.
    pub indices: Vec<u32>,
    /// Every base texture the kept primitives use.
    pub textures: Vec<Handle<Image>>,
    /// The base colour of the first kept primitive.
    pub base_color: Option<Color>,
}

/// `T(x, y, z) * S(scale) * R(yaw)`: a Scene's placement (the order
/// `tools/inspect_gltf check` uses), with `offset_x100` in hundredths of a
/// tile.
pub fn scene_affine(scale_x100: u32, yaw_deg: i32, offset_x100: (i32, i32, i32)) -> Affine3A {
    let (x, y, z) = offset_x100;
    Affine3A::from_translation(Vec3::new(x as f32, y as f32, z as f32) / 100.0)
        * Affine3A::from_scale(Vec3::splat(scale_x100 as f32 / 100.0))
        * Affine3A::from_rotation_y((yaw_deg as f32).to_radians())
}

/// `T(offset) * R(yaw) * S(scale)`: a part in its Scene's model space.
pub fn part_affine(part: &(String, Vec<String>, (i32, i32, i32), u32, i32)) -> Affine3A {
    let (x, y, z) = part.2;
    Affine3A::from_translation(Vec3::new(x as f32, y as f32, z as f32) / 100.0)
        * Affine3A::from_rotation_y((part.4 as f32).to_radians())
        * Affine3A::from_scale(Vec3::splat(part.3 as f32 / 100.0))
}

/// The [`part_affine`] of a `rules::Part`.
pub fn rules_part_affine(part: &Part) -> Affine3A {
    part_affine(&(
        part.path.clone(),
        part.nodes.clone(),
        part.offset_x100,
        part.scale_x100,
        part.yaw_deg,
    ))
}

/// Append the primitives of the nodes of `nodes` whose own name `globs`
/// keeps to `out`, placed by `placement * world`, where `world` composes
/// the node transforms from the roots (nodes that are nobody's child).
pub fn merge_nodes(
    file: &str,
    nodes: &[SourceNode],
    meshes: &Assets<Mesh>,
    globs: &[String],
    placement: Affine3A,
    out: &mut Geometry,
) -> Result<(), MergeError> {
    let mut is_child = vec![false; nodes.len()];
    for n in nodes {
        for &c in &n.children {
            if let Some(slot) = is_child.get_mut(c) {
                *slot = true;
            }
        }
    }
    let mut stack: Vec<(usize, Affine3A)> = (0..nodes.len())
        .rev()
        .filter(|&i| !is_child[i])
        .map(|i| (i, Affine3A::IDENTITY))
        .collect();
    let mut seen = vec![false; nodes.len()];
    let mut kept = 0usize;
    while let Some((i, parent)) = stack.pop() {
        if std::mem::replace(&mut seen[i], true) {
            continue;
        }
        let node = &nodes[i];
        let world = parent * node.local;
        for &c in node.children.iter().rev() {
            if c < nodes.len() {
                stack.push((c, world));
            }
        }
        if node.prims.is_empty() || !keeps_node(globs, &node.name) {
            continue;
        }
        let m = placement * world;
        let normal_matrix: Mat3A = m.matrix3.inverse().transpose();
        for (pi, prim) in node.prims.iter().enumerate() {
            let what = format!("{file} node {:?} primitive {pi}", node.name);
            let mesh = meshes
                .get(&prim.mesh)
                .ok_or_else(|| MergeError::Missing(what.clone()))?;
            if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
                return Err(MergeError::Topology(what));
            }
            let attr = |id, name: &str| {
                mesh.try_attribute_option(id)
                    .ok()
                    .flatten()
                    .ok_or_else(|| MergeError::Attribute(format!("{what} has no {name}")))
            };
            let VertexAttributeValues::Float32x3(positions) =
                attr(Mesh::ATTRIBUTE_POSITION, "POSITION")?
            else {
                return Err(MergeError::Attribute(format!(
                    "{what}: POSITION is not Float32x3"
                )));
            };
            let VertexAttributeValues::Float32x3(normals) = attr(Mesh::ATTRIBUTE_NORMAL, "NORMAL")?
            else {
                return Err(MergeError::Attribute(format!(
                    "{what}: NORMAL is not Float32x3"
                )));
            };
            let VertexAttributeValues::Float32x2(uvs) = attr(Mesh::ATTRIBUTE_UV_0, "TEXCOORD_0")?
            else {
                return Err(MergeError::Attribute(format!(
                    "{what}: TEXCOORD_0 is not Float32x2"
                )));
            };
            if normals.len() != positions.len() || uvs.len() != positions.len() {
                return Err(MergeError::Attribute(format!(
                    "{what}: attribute lengths differ"
                )));
            }
            let indices = mesh
                .try_indices_option()
                .ok()
                .flatten()
                .ok_or_else(|| MergeError::NotIndexed(what.clone()))?;
            let Some(texture) = &prim.texture else {
                return Err(MergeError::Texture(format!(
                    "{what} has no base-colour texture"
                )));
            };
            match out.base_color {
                None => out.base_color = Some(prim.base_color),
                Some(c) if c != prim.base_color => {
                    return Err(MergeError::BaseColor(format!(
                        "{what} has another base colour than the model's first primitive"
                    )));
                }
                Some(_) => {}
            }
            if !out.textures.contains(texture) {
                out.textures.push(texture.clone());
            }
            let base = u32::try_from(out.positions.len())
                .map_err(|_| MergeError::Attribute(format!("{what}: too many vertices")))?;
            out.positions.extend(
                positions
                    .iter()
                    .map(|&p| m.transform_point3(Vec3::from(p)).to_array()),
            );
            out.normals.extend(normals.iter().map(|&n| {
                (normal_matrix * Vec3A::from(n))
                    .normalize_or(Vec3A::Y)
                    .to_array()
            }));
            out.uvs.extend_from_slice(uvs);
            for ix in indices.iter() {
                let ix = u32::try_from(ix)
                    .ok()
                    .filter(|&ix| (ix as usize) < positions.len())
                    .ok_or_else(|| {
                        MergeError::Attribute(format!("{what}: an index is out of range"))
                    })?;
                out.indices.push(base + ix);
            }
            kept += 1;
        }
    }
    if kept == 0 {
        return Err(MergeError::Empty(format!(
            "{file}: no mesh node matches nodes {globs:?}"
        )));
    }
    Ok(())
}

/// The [`SourceNode`]s of a loaded glTF file.
pub fn source_nodes(
    file: &str,
    gltf: &Gltf,
    assets: &GltfAssets,
) -> Result<Vec<SourceNode>, MergeError> {
    let index: BTreeMap<AssetId<GltfNode>, usize> = gltf
        .nodes
        .iter()
        .enumerate()
        .map(|(i, h)| (h.id(), i))
        .collect();
    gltf.nodes
        .iter()
        .map(|h| {
            let node = assets
                .nodes
                .get(h)
                .ok_or_else(|| MergeError::Missing(format!("{file} node")))?;
            let mut prims = Vec::new();
            if let Some(mesh) = &node.mesh {
                let mesh = assets
                    .meshes
                    .get(mesh)
                    .ok_or_else(|| MergeError::Missing(format!("{file} mesh")))?;
                for p in &mesh.primitives {
                    let material = p.material.as_ref().and_then(|m| assets.materials.get(m));
                    prims.push(SourcePrim {
                        mesh: p.mesh.clone(),
                        texture: material.and_then(|m| m.base_color_texture.clone()),
                        base_color: material.map_or(Color::WHITE, |m| m.base_color),
                    });
                }
            }
            Ok(SourceNode {
                name: node.name.clone(),
                local: node.transform.compute_affine(),
                children: node
                    .children
                    .iter()
                    .filter_map(|c| index.get(&c.id()).copied())
                    .collect(),
                prims,
            })
        })
        .collect()
}

/// Merge every file of `key` (handles in [`ModelKey::files`] order) into
/// one placed mesh in `meshes`, and return the model with the handle of
/// its one base texture.
fn build_model(
    key: &ModelKey,
    handles: &[Handle<Gltf>],
    assets: &GltfAssets,
    meshes: &mut Assets<Mesh>,
    images: &Assets<Image>,
) -> Result<(Model, Handle<Image>), MergeError> {
    let scene = scene_affine(key.scale_x100, key.yaw_deg, key.offset_x100);
    let mut geometry = Geometry::default();
    let sources = std::iter::once((key.path.as_str(), &key.nodes, Affine3A::IDENTITY)).chain(
        key.parts
            .iter()
            .map(|p| (p.0.as_str(), &p.1, part_affine(p))),
    );
    for ((file, globs, local), handle) in sources.zip(handles) {
        let gltf = assets
            .gltfs
            .get(handle)
            .ok_or_else(|| MergeError::Missing(file.to_string()))?;
        let nodes = source_nodes(file, gltf, assets)?;
        merge_nodes(file, &nodes, meshes, globs, scene * local, &mut geometry)?;
    }
    let (texture, image) = one_texture(&key.path, &geometry.textures, images)?;
    let model = finish_geometry(geometry, texture, meshes);
    Ok((model, image))
}

/// The content hash of the one base texture `textures` share, and one of
/// their handles; two different contents fail.
pub fn one_texture(
    file: &str,
    textures: &[Handle<Image>],
    images: &Assets<Image>,
) -> Result<(u64, Handle<Image>), MergeError> {
    let mut found: Option<(u64, Handle<Image>)> = None;
    for handle in textures {
        let image = images
            .get(handle)
            .ok_or_else(|| MergeError::Texture(format!("{file}: a base texture is not loaded")))?;
        let hash = content_hash(image);
        match &found {
            None => found = Some((hash, handle.clone())),
            Some((h, _)) if *h != hash => {
                return Err(MergeError::Texture(format!(
                    "{file}: two base textures; one base texture per model"
                )));
            }
            Some(_) => {}
        }
    }
    found.ok_or_else(|| MergeError::Texture(format!("{file}: no base texture")))
}

/// Turn merged `geometry` into a [`Model`] whose mesh is added to
/// `meshes` (main and render world, for picking).
pub fn finish_geometry(geometry: Geometry, texture: u64, meshes: &mut Assets<Mesh>) -> Model {
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &geometry.positions {
        min = min.min(Vec3::from(*p));
        max = max.max(Vec3::from(*p));
    }
    if geometry.positions.is_empty() {
        (min, max) = (Vec3::ZERO, Vec3::ZERO);
    }
    let reach = geometry
        .positions
        .iter()
        .map(|p| Vec2::new(p[0], p[2]).length())
        .fold(0.0f32, f32::max);
    let triangles = u32::try_from(geometry.indices.len() / 3).unwrap_or(u32::MAX);
    let mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, geometry.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, geometry.normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, geometry.uvs)
    .with_inserted_indices(Indices::U32(geometry.indices));
    Model {
        mesh: meshes.add(mesh),
        texture,
        base_color: geometry.base_color.unwrap_or(Color::WHITE),
        min,
        max,
        reach,
        triangles,
    }
}

/// A hash of an image's size, format and texels (std's `DefaultHasher`,
/// `SipHash` with fixed keys, so equal content always hashes alike): the
/// key textures are deduplicated by.
pub fn content_hash(image: &Image) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    image.width().hash(&mut h);
    image.height().hash(&mut h);
    format!("{:?}", image.texture_descriptor.format).hash(&mut h);
    image.data.as_deref().unwrap_or(&[]).hash(&mut h);
    h.finish()
}

/// sRGB-encoded channel to linear.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear channel to sRGB-encoded.
fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Relative luminance of a linear colour.
fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// `src` repainted for a team (art pass, `docs/ART_STYLE.md`): inside the
/// swatch rects every texel becomes `team` scaled by its own luminance
/// over the mean luminance of its rect, which keeps the pack's gradient
/// shading; outside them every texel moves `desaturate_permille` of the
/// way toward its own grey, so team colour stays the strongest hue. Alpha
/// is kept. `None` for an image without CPU data or not 8-bit RGBA.
pub fn team_texture(src: &Image, swatch: &TeamSwatch, team: Color) -> Option<Image> {
    let srgb = match src.texture_descriptor.format {
        TextureFormat::Rgba8UnormSrgb => true,
        TextureFormat::Rgba8Unorm => false,
        _ => return None,
    };
    let data = src.data.as_ref()?;
    let (w, h) = (src.width(), src.height());
    if data.len() != (w as usize) * (h as usize) * 4 {
        return None;
    }
    let decode = |b: u8| {
        let c = f32::from(b) / 255.0;
        if srgb { srgb_to_linear(c) } else { c }
    };
    let encode = |c: f32| {
        let c = if srgb {
            linear_to_srgb(c)
        } else {
            c.clamp(0.0, 1.0)
        };
        (c * 255.0).round() as u8
    };
    let texel = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        [decode(data[i]), decode(data[i + 1]), decode(data[i + 2])]
    };
    let team = team.to_linear();
    let team = [team.red, team.green, team.blue];
    let mut out = data.clone();
    // Which rect (index) each texel belongs to, the first that holds it.
    let mut owner: Vec<u16> = vec![u16::MAX; (w as usize) * (h as usize)];
    let mut means = Vec::new();
    for (ri, &(rx, ry, rw, rh)) in swatch.rects.iter().enumerate() {
        let (x1, y1) = ((rx + rw).min(w), (ry + rh).min(h));
        let (mut sum, mut n) = (0.0f64, 0u32);
        for y in ry.min(h)..y1 {
            for x in rx.min(w)..x1 {
                sum += f64::from(luminance(texel(x, y)));
                n += 1;
                let slot = &mut owner[(y * w + x) as usize];
                if *slot == u16::MAX {
                    *slot = u16::try_from(ri).unwrap_or(u16::MAX - 1);
                }
            }
        }
        means.push(if n > 0 {
            (sum / f64::from(n)) as f32
        } else {
            0.0
        });
    }
    let desaturate = swatch.desaturate_permille.min(1000) as f32 / 1000.0;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let c = texel(x, y);
            let l = luminance(c);
            let painted = match owner[(y * w + x) as usize] {
                u16::MAX => [
                    c[0] + (l - c[0]) * desaturate,
                    c[1] + (l - c[1]) * desaturate,
                    c[2] + (l - c[2]) * desaturate,
                ],
                ri => {
                    let mean = means.get(usize::from(ri)).copied().unwrap_or(0.0);
                    let k = if mean > 0.0 { l / mean } else { 1.0 };
                    [team[0] * k, team[1] * k, team[2] * k]
                }
            };
            out[i] = encode(painted[0]);
            out[i + 1] = encode(painted[1]);
            out[i + 2] = encode(painted[2]);
        }
    }
    let mut image = src.clone();
    image.data = Some(out);
    Some(image)
}

/// `src` desaturated toward grey by `permille` (keeping each texel's
/// luminance), as a team texture desaturates the texels outside its rects;
/// `None` for a texture that is not 8-bit RGBA with CPU data.
pub fn neutral_texture(src: &Image, permille: u32) -> Option<Image> {
    let swatch = TeamSwatch {
        rects: Vec::new(),
        desaturate_permille: permille,
    };
    team_texture(src, &swatch, Color::WHITE)
}

/// Every Scene file `visuals` names that the game loads, distinct (for
/// logs and tests).
pub fn drawn_files(visuals: &Visuals) -> BTreeSet<String> {
    drawn_scenes(visuals)
        .into_iter()
        .filter_map(ModelKey::of)
        .flat_map(|k| k.files().map(str::to_string).collect::<Vec<_>>())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::image::ImageSampler;
    use bevy::render::render_resource::{Extent3d, TextureDimension};

    /// A triangle-list mesh with the three attributes and `indices`.
    fn tri_mesh(positions: Vec<[f32; 3]>, indices: Option<Vec<u32>>) -> Mesh {
        let n = positions.len();
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; n])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.5, 0.5]; n]);
        if let Some(ix) = indices {
            mesh.insert_indices(Indices::U32(ix));
        }
        mesh
    }

    fn rgba(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Image {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&f(x, y));
            }
        }
        let mut image = Image::new(
            Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        image.sampler = ImageSampler::nearest();
        image
    }

    fn node(
        name: &str,
        local: Affine3A,
        children: Vec<usize>,
        prims: Vec<SourcePrim>,
    ) -> SourceNode {
        SourceNode {
            name: name.into(),
            local,
            children,
            prims,
        }
    }

    fn prim(mesh: &Handle<Mesh>, tex: &Handle<Image>) -> SourcePrim {
        SourcePrim {
            mesh: mesh.clone(),
            texture: Some(tex.clone()),
            base_color: Color::WHITE,
        }
    }

    /// A parent at x = 2 with a child at y = 1 (both one triangle) and an
    /// unrelated root: the child lands at (2, 1, 0) composed with its
    /// parent, and a glob keeps only the nodes it names (own name only).
    #[test]
    fn merge_applies_node_transforms_and_filters() {
        let mut meshes = Assets::<Mesh>::default();
        let mut images = Assets::<Image>::default();
        let tex = images.add(rgba(2, 2, |_, _| [255, 255, 255, 255]));
        let tri = meshes.add(tri_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            Some(vec![0, 1, 2]),
        ));
        let nodes = vec![
            node(
                "tower_base",
                Affine3A::from_translation(Vec3::X * 2.0),
                vec![1],
                vec![prim(&tri, &tex)],
            ),
            node(
                "tower_top",
                Affine3A::from_translation(Vec3::Y),
                vec![],
                vec![prim(&tri, &tex)],
            ),
            node("other", Affine3A::IDENTITY, vec![], vec![prim(&tri, &tex)]),
        ];
        let mut all = Geometry::default();
        merge_nodes("f", &nodes, &meshes, &[], Affine3A::IDENTITY, &mut all).unwrap();
        assert_eq!(all.indices.len(), 9, "three nodes, one triangle each");
        assert!(
            all.positions.contains(&[2.0, 1.0, 0.0]),
            "{:?}",
            all.positions
        );
        assert_eq!(all.indices[3..6], [3, 4, 5], "indices are rebased");
        let mut top = Geometry::default();
        merge_nodes(
            "f",
            &nodes,
            &meshes,
            &["*_top".to_string()],
            Affine3A::IDENTITY,
            &mut top,
        )
        .unwrap();
        assert_eq!(
            top.positions,
            vec![[2.0, 1.0, 0.0], [3.0, 1.0, 0.0], [2.0, 2.0, 0.0]],
            "the kept child keeps its parent's transform; the parent's own mesh is dropped"
        );
        let err = merge_nodes(
            "f",
            &nodes,
            &meshes,
            &["nothing".to_string()],
            Affine3A::IDENTITY,
            &mut Geometry::default(),
        )
        .unwrap_err();
        assert!(matches!(err, MergeError::Empty(_)), "{err}");
    }

    #[test]
    fn merge_rejects_mismatched_attribute_sets() {
        let mut meshes = Assets::<Mesh>::default();
        let mut images = Assets::<Image>::default();
        let tex = images.add(rgba(2, 2, |_, _| [255; 4]));
        let positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mut no_normal = tri_mesh(positions.clone(), Some(vec![0, 1, 2]));
        no_normal.remove_attribute(Mesh::ATTRIBUTE_NORMAL);
        let mut no_uv = tri_mesh(positions.clone(), Some(vec![0, 1, 2]));
        no_uv.remove_attribute(Mesh::ATTRIBUTE_UV_0);
        let unindexed = tri_mesh(positions.clone(), None);
        let lines = Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
            .with_inserted_indices(Indices::U32(vec![0, 1]));
        for (mesh, want) in [
            (no_normal, "has no NORMAL"),
            (no_uv, "has no TEXCOORD_0"),
            (unindexed, "is not indexed"),
            (lines, "is not a triangle list"),
        ] {
            let h = meshes.add(mesh);
            let nodes = vec![node("n", Affine3A::IDENTITY, vec![], vec![prim(&h, &tex)])];
            let err = merge_nodes(
                "f",
                &nodes,
                &meshes,
                &[],
                Affine3A::IDENTITY,
                &mut Geometry::default(),
            )
            .unwrap_err()
            .to_string();
            assert!(err.contains(want), "{want}: {err}");
        }
        // A primitive without a base texture fails too.
        let ok = meshes.add(tri_mesh(
            vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            Some(vec![0, 1, 2]),
        ));
        let bare = SourcePrim {
            texture: None,
            ..prim(&ok, &tex)
        };
        let err = merge_nodes(
            "f",
            &[node("n", Affine3A::IDENTITY, vec![], vec![bare])],
            &meshes,
            &[],
            Affine3A::IDENTITY,
            &mut Geometry::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("no base-colour texture"), "{err}");
    }

    /// Two base textures with different content fail; two handles of the
    /// same content (`KayKit`'s per-folder atlas copies) are one texture.
    #[test]
    fn merge_rejects_two_textures() {
        let mut images = Assets::<Image>::default();
        let a = images.add(rgba(2, 2, |_, _| [10, 20, 30, 255]));
        let a2 = images.add(rgba(2, 2, |_, _| [10, 20, 30, 255]));
        let b = images.add(rgba(2, 2, |_, _| [200, 20, 30, 255]));
        let (hash, _) = one_texture("f", &[a.clone(), a2.clone()], &images).unwrap();
        assert_eq!(hash, content_hash(images.get(&a).unwrap()));
        let err = one_texture("f", &[a, a2, b], &images).unwrap_err();
        assert!(err.to_string().contains("two base textures"), "{err}");
        assert!(one_texture("f", &[], &images).is_err());
        // Two base colours fail the merge.
        let mut meshes = Assets::<Mesh>::default();
        let tri = meshes.add(tri_mesh(
            vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            Some(vec![0, 1, 2]),
        ));
        let tex = images.add(rgba(2, 2, |_, _| [255; 4]));
        let red = SourcePrim {
            base_color: Color::srgb(1.0, 0.0, 0.0),
            ..prim(&tri, &tex)
        };
        let err = merge_nodes(
            "f",
            &[node(
                "n",
                Affine3A::IDENTITY,
                vec![],
                vec![prim(&tri, &tex), red],
            )],
            &meshes,
            &[],
            Affine3A::IDENTITY,
            &mut Geometry::default(),
        )
        .unwrap_err();
        assert!(matches!(err, MergeError::BaseColor(_)), "{err}");
    }

    /// The Scene placement turns by the yaw, scales, then moves by the
    /// offsets (the order `inspect_gltf check` uses); a part is moved,
    /// turned and scaled in the Scene's model space first.
    #[test]
    fn place_turns_scales_and_lifts() {
        let m = scene_affine(200, 90, (10, 5, -30));
        // +X turned 90° about +Y is -Z, doubled, then offset.
        let p = m.transform_point3(Vec3::X);
        assert!((p - Vec3::new(0.1, 0.05, -2.3)).length() < 1e-5, "{p}");
        let part = part_affine(&("p".into(), vec![], (100, 0, 0), 50, 180));
        let q = (m * part).transform_point3(Vec3::X);
        // Part: halve, turn 180° (+X to -X), move +1 m on x: (0.5, 0, 0);
        // then the Scene: turn to (0, 0, -0.5), double, offset.
        assert!((q - Vec3::new(0.1, 0.05, -1.3)).length() < 1e-5, "{q}");
        assert_eq!(
            rules_part_affine(&Part {
                path: "p".into(),
                nodes: vec![],
                offset_x100: (100, 0, 0),
                scale_x100: 50,
                yaw_deg: 180,
            }),
            part
        );
        // Merged normals turn with the yaw and stay unit length.
        let mut meshes = Assets::<Mesh>::default();
        let mut images = Assets::<Image>::default();
        let tex = images.add(rgba(2, 2, |_, _| [255; 4]));
        let tri = meshes.add(tri_mesh(
            vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            Some(vec![0, 1, 2]),
        ));
        let mut g = Geometry::default();
        merge_nodes(
            "f",
            &[node(
                "n",
                Affine3A::IDENTITY,
                vec![],
                vec![prim(&tri, &tex)],
            )],
            &meshes,
            &[],
            m,
            &mut g,
        )
        .unwrap();
        let n = Vec3::from(g.normals[0]);
        // +Z turned 90° about +Y is +X.
        assert!((n - Vec3::X).length() < 1e-5, "{n}");
    }

    #[test]
    fn top_y_and_size_come_from_the_merged_aabb() {
        let mut meshes = Assets::<Mesh>::default();
        let g = Geometry {
            positions: vec![[-1.0, 0.0, -0.5], [1.0, 2.5, 0.5], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 1.0, 0.0]; 3],
            uvs: vec![[0.0, 0.0]; 3],
            indices: vec![0, 1, 2],
            textures: vec![],
            base_color: None,
        };
        let model = finish_geometry(g, 7, &mut meshes);
        assert_eq!(model.top_y(), 2.5);
        assert_eq!(model.size(), Vec3::new(2.0, 2.5, 1.0));
        assert!(
            (model.reach - Vec2::new(1.0, 0.5).length()).abs() < 1e-6,
            "the farthest vertex"
        );
        assert_eq!((model.triangles, model.texture), (1, 7));
        assert_eq!(model.base_color, Color::WHITE);
        let mesh = meshes.get(&model.mesh).unwrap();
        assert_eq!(mesh.count_vertices(), 3);
        assert!(
            mesh.asset_usage.contains(RenderAssetUsages::MAIN_WORLD),
            "picking ray-casts the model on the CPU"
        );
    }

    /// Inside the rect every texel takes the team colour scaled by its
    /// lightness over the rect's mean (a darker texel stays darker);
    /// outside, texels move toward grey by the permille; alpha is kept.
    #[test]
    fn team_texture_repaints_rects_keeps_lightness_and_desaturates_the_rest() {
        // A 4x2 texture: the left 2x2 is the team rect, a light and a dark
        // blue column; the right half is saturated green.
        let src = rgba(4, 2, |x, _| match x {
            0 => [120, 160, 230, 255],
            1 => [40, 60, 110, 200],
            _ => [30, 200, 40, 255],
        });
        let swatch = TeamSwatch {
            rects: vec![(0, 0, 2, 2)],
            desaturate_permille: 500,
        };
        let team = palette::team_color(1);
        let out = team_texture(&src, &swatch, team).unwrap();
        let px = |img: &Image, x: u32| {
            let i = (x * 4) as usize;
            let d = img.data.as_ref().unwrap();
            [d[i], d[i + 1], d[i + 2], d[i + 3]]
        };
        let (light, dark) = (px(&out, 0), px(&out, 1));
        // Red team: red dominates both repainted texels.
        assert!(
            light[0] > light[2] && dark[0] > dark[2],
            "{light:?} {dark:?}"
        );
        assert!(light[0] > dark[0], "lightness kept: {light:?} vs {dark:?}");
        assert_eq!(dark[3], 200, "alpha kept");
        // The rect's mean luminance maps to the team colour itself: the
        // two texels straddle it.
        let t = team.to_srgba();
        let want = (t.red * 255.0).round() as u8;
        assert!(
            dark[0] < want && want < light[0],
            "{dark:?} {want} {light:?}"
        );
        // Outside: half way to grey, so green weakens but leads.
        let g = px(&out, 2);
        assert!(g[1] < 200 && g[1] > g[0] && g[0] > 30, "{g:?}");
        assert_eq!(px(&out, 3), g);
        // No desaturation leaves the outside as shipped.
        let none = TeamSwatch {
            desaturate_permille: 0,
            ..swatch.clone()
        };
        assert_eq!(
            px(&team_texture(&src, &none, team).unwrap(), 2),
            [30, 200, 40, 255]
        );
        // Not 8-bit RGBA, or no CPU data: no repaint.
        let mut odd = src.clone();
        odd.texture_descriptor.format = TextureFormat::R8Unorm;
        assert!(team_texture(&odd, &swatch, team).is_none());
        let mut empty = src;
        empty.data = None;
        assert!(team_texture(&empty, &swatch, team).is_none());
    }

    /// `neutral_desaturate_permille` (visuals.ron): a neutral material
    /// (terrain props, sites, rubble) draws its texture desaturated toward
    /// grey, keeping each texel's luminance, so a prop's colour never
    /// outshouts the team colours; at 0 it draws the texture as shipped.
    /// A team material is untouched by it.
    #[test]
    fn neutral_models_take_the_neutral_desaturation() {
        let saturation = |p: [u8; 4]| {
            let (mx, mn) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
            f32::from(mx - mn) / f32::from(mx.max(1))
        };
        let green = [30, 170, 70, 255];
        let run = |permille: u32| {
            let mut images = Assets::<Image>::default();
            let mut materials = Assets::<StandardMaterial>::default();
            let mut meshes = Assets::<Mesh>::default();
            let src = images.add(rgba(2, 2, |_, _| green));
            let hash = content_hash(images.get(&src).unwrap());
            let visuals = Visuals {
                neutral_desaturate_permille: permille,
                team_swatches: [(
                    "hex".to_string(),
                    TeamSwatch {
                        rects: vec![(0, 0, 1, 1)],
                        desaturate_permille: 0,
                    },
                )]
                .into(),
                ..Visuals::default()
            };
            let mut lib = ModelLibrary::new(&visuals);
            let model = Model {
                mesh: meshes.add(tri_mesh(vec![[0.0; 3]; 3], Some(vec![0, 1, 2]))),
                texture: hash,
                base_color: Color::WHITE,
                min: Vec3::ZERO,
                max: Vec3::ONE,
                reach: 1.0,
                triangles: 1,
            };
            let key = ModelKey {
                path: "tree.gltf".into(),
                nodes: vec![],
                parts: vec![],
                scale_x100: 100,
                yaw_deg: 0,
                offset_x100: (0, 0, 0),
            };
            lib.insert_ready(key, model.clone(), src.clone());
            let neutral = lib.material(&model, None, &mut images, &mut materials);
            let team = lib.material(&model, Some(("hex", 0)), &mut images, &mut materials);
            let texel = |m: &Handle<StandardMaterial>, x: u32| {
                let tex = materials
                    .get(m)
                    .unwrap()
                    .base_color_texture
                    .clone()
                    .unwrap();
                let d = images.get(&tex).unwrap().data.clone().unwrap();
                let i = (x * 4) as usize;
                ([d[i], d[i + 1], d[i + 2], d[i + 3]], tex == src)
            };
            (texel(&neutral, 1), texel(&team, 1))
        };
        let ((shipped, same), (team0, _)) = run(0);
        assert_eq!(shipped, green, "at 0, as shipped");
        assert!(same, "the source texture itself");
        let ((half, same), (team500, _)) = run(500);
        assert!(!same, "a desaturated copy");
        assert!(
            saturation(half) < saturation(green) * 0.75 && half[1] > half[0],
            "{half:?}: duller, still green"
        );
        assert_eq!(team500, team0, "a team material ignores it");
        let ((grey, _), _) = run(1000);
        assert!(saturation(grey) < 0.02, "{grey:?}: grey at 1000");
    }

    /// Models on equal texture content share one neutral and one material
    /// per team, whatever handle their image came from.
    #[test]
    fn textures_are_deduplicated_by_content() {
        let mut images = Assets::<Image>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let mut meshes = Assets::<Mesh>::default();
        let a = images.add(rgba(4, 4, |x, _| [x as u8 * 50, 90, 200, 255]));
        let a2 = images.add(rgba(4, 4, |x, _| [x as u8 * 50, 90, 200, 255]));
        let hash = content_hash(images.get(&a).unwrap());
        assert_eq!(hash, content_hash(images.get(&a2).unwrap()));
        let visuals = Visuals {
            team_swatches: [(
                "hex".to_string(),
                TeamSwatch {
                    rects: vec![(0, 0, 2, 2)],
                    desaturate_permille: 0,
                },
            )]
            .into(),
            ..Visuals::default()
        };
        let mut lib = ModelLibrary::new(&visuals);
        let model = |meshes: &mut Assets<Mesh>| Model {
            mesh: meshes.add(tri_mesh(vec![[0.0; 3]; 3], Some(vec![0, 1, 2]))),
            texture: hash,
            base_color: Color::WHITE,
            min: Vec3::ZERO,
            max: Vec3::ONE,
            reach: 1.0,
            triangles: 1,
        };
        let (m1, m2) = (model(&mut meshes), model(&mut meshes));
        let key = |p: &str| ModelKey {
            path: p.into(),
            nodes: vec![],
            parts: vec![],
            scale_x100: 100,
            yaw_deg: 0,
            offset_x100: (0, 0, 0),
        };
        lib.insert_ready(key("a.gltf"), m1.clone(), a);
        lib.insert_ready(key("b.gltf"), m2.clone(), a2);
        let n1 = lib.material(&m1, None, &mut images, &mut materials);
        let n2 = lib.material(&m2, None, &mut images, &mut materials);
        assert_eq!(n1, n2, "one neutral material per texture content");
        let t0 = lib.material(&m1, Some(("hex", 0)), &mut images, &mut materials);
        assert_eq!(
            t0,
            lib.material(&m2, Some(("hex", 0)), &mut images, &mut materials)
        );
        let t1 = lib.material(&m1, Some(("hex", 1)), &mut images, &mut materials);
        assert!(t0 != t1 && t0 != n1);
        assert_eq!(lib.material_count(), 3);
        let mat = materials.get(&t1).unwrap();
        assert_eq!(
            (mat.perceptual_roughness, mat.reflectance, mat.metallic),
            (MATTE_ROUGHNESS, MATTE_REFLECTANCE, 0.0),
            "matte, as every look"
        );
        assert_ne!(
            mat.base_color_texture,
            materials.get(&n1).unwrap().base_color_texture
        );
        assert_eq!(lib.errors_logged(), 0);
        // An unknown swatch draws the shipped texture and logs once.
        let x = lib.material(&m1, Some(("nope", 0)), &mut images, &mut materials);
        assert_eq!(
            materials.get(&x).unwrap().base_color_texture,
            materials.get(&n1).unwrap().base_color_texture
        );
        assert_eq!(lib.errors_logged(), 1);
    }

    #[test]
    fn wait_on_loads_fails_and_times_out() {
        use std::sync::Arc;
        let loaded = (
            "a.gltf".to_string(),
            LoadState::Loaded,
            RecursiveDependencyLoadState::Loaded,
        );
        let pending = (
            "b.gltf".to_string(),
            LoadState::Loading,
            RecursiveDependencyLoadState::Loading,
        );
        assert_eq!(wait_on(std::slice::from_ref(&loaded), 0.0), Wait::Loaded);
        assert_eq!(
            wait_on(&[loaded.clone(), pending.clone()], 1.0),
            Wait::Pending
        );
        assert!(matches!(
            wait_on(&[loaded.clone(), pending], MODEL_LOAD_TIMEOUT_SECS + 0.1),
            Wait::Failed(r) if r.contains("not loaded after")
        ));
        let err = Arc::new(bevy::asset::AssetLoadError::MissingAssetLoader {
            asset_type_id: None,
            asset_path: "c.gltf".into(),
        });
        let failed = (
            "c.gltf".to_string(),
            LoadState::Failed(err),
            RecursiveDependencyLoadState::Loading,
        );
        assert!(matches!(
            wait_on(&[loaded, failed], 0.0),
            Wait::Failed(r) if r.starts_with("c.gltf failed to load")
        ));
    }

    /// A model whose files never load settles as failed after the
    /// timeout, logs one error however long the game runs, and the gate
    /// opens; until then the gate is closed.
    #[test]
    fn a_failed_load_settles_with_the_fallback_and_logs_once() {
        use bevy::time::TimeUpdateStrategy;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_secs(4),
            ))
            .insert_resource(Assets::<Mesh>::default())
            .insert_resource(Assets::<Image>::default())
            .add_systems(Update, finish_models);
        let key = ModelKey::of(&Visual::Scene {
            path: "models/missing.gltf".into(),
            scale_x100: 100,
            y_offset_tiles_x100: 0,
            yaw_deg: 0,
            x_offset_tiles_x100: 0,
            z_offset_tiles_x100: 0,
            nodes: vec![],
            parts: vec![],
            team: None,
            fallback: None,
            banner_at: None,
            seat_flag_at: None,
        })
        .unwrap();
        let mut lib = ModelLibrary::default();
        lib.insert_loading(
            key.clone(),
            vec![("models/missing.gltf".into(), Handle::default())],
        );
        app.insert_resource(lib);
        let settled = |app: &mut App| {
            app.world_mut()
                .run_system_once(models_settled)
                .expect("the condition runs")
        };
        app.update();
        assert!(!settled(&mut app), "loading: the gate is closed");
        for _ in 0..6 {
            app.update();
        }
        assert!(settled(&mut app), "past the timeout the entry has failed");
        let lib = app.world().resource::<ModelLibrary>();
        assert!(matches!(lib.entries.get(&key), Some(ModelState::Failed)));
        assert_eq!(lib.counts(), (0, 1));
        assert_eq!(lib.errors_logged(), 1, "one error for the entry");
        assert!(lib.settled_logged);
    }

    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn no_asset_server_is_settled() {
        let mut world = World::new();
        assert!(world.run_system_once(models_settled).unwrap());
        world.insert_resource(ModelLibrary::default());
        assert!(
            world.run_system_once(models_settled).unwrap(),
            "nothing to load"
        );
    }

    /// The files the game loads: units' (since art slice 4), buildings',
    /// sites', the rubble's, the flags' and the props'. On the live file
    /// every path `Visuals::scene_paths` lists (what data-check checks) is
    /// loaded; and with a distinct model for each use, each one's key is
    /// requested, so a use dropped from `drawn_scenes` would draw nothing
    /// (a site as the box, no rubble, no banner, no Seat flag) without an
    /// error.
    #[test]
    fn drawn_scenes_cover_every_scene() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let mut rules = rules::Rules::load(&data).expect("data loads");
        let unit = Visual::Scene {
            path: "models/unit.glb".into(),
            scale_x100: 100,
            y_offset_tiles_x100: 0,
            yaw_deg: 0,
            x_offset_tiles_x100: 0,
            z_offset_tiles_x100: 0,
            nodes: vec![],
            parts: vec![],
            team: None,
            fallback: None,
            banner_at: None,
            seat_flag_at: None,
        };
        rules.visuals.units.insert("yeoman".into(), unit);
        let files = drawn_files(&rules.visuals);
        assert!(files.contains("models/unit.glb"), "{files:?}");
        // A unit that is not a Scene loads nothing.
        rules.visuals.units.insert(
            "yeoman".into(),
            Visual::Primitive(rules::Primitive::Capsule),
        );
        assert!(
            !drawn_files(&rules.visuals).contains("models/unit.glb"),
            "a Primitive look is no file"
        );
        for v in rules
            .visuals
            .buildings
            .values()
            .chain(rules.visuals.units.values())
        {
            if let Visual::Scene { path, .. } = v {
                assert!(files.contains(path), "{path}");
            }
        }
        for decor in rules.visuals.terrain.values() {
            for look in &decor.looks {
                if let Visual::Scene { path, .. } = &look.visual {
                    assert!(files.contains(path), "{path}");
                }
            }
        }
        let live = rules::Rules::load(&data).expect("data loads");
        let files = drawn_files(&live.visuals);
        for (field, path) in live.visuals.scene_paths() {
            assert!(files.contains(path), "{field}: {path} is never loaded");
        }

        // One model per use, none shared: each use's own key is requested.
        let scene = |path: &str| Visual::Scene {
            path: path.into(),
            scale_x100: 100,
            y_offset_tiles_x100: 0,
            yaw_deg: 0,
            x_offset_tiles_x100: 0,
            z_offset_tiles_x100: 0,
            nodes: vec![],
            parts: vec![],
            team: None,
            fallback: None,
            banner_at: None,
            seat_flag_at: None,
        };
        let mut v = live.visuals.clone();
        v.units.insert("yeoman".into(), scene("m/unit.glb"));
        v.buildings.insert("croft".into(), scene("m/house.gltf"));
        v.sites = vec![scene("m/site_a.gltf"), scene("m/site_b.gltf")];
        v.rubble = Some(rules::Rubble {
            visual: scene("m/rubble.gltf"),
            ds: 300,
        });
        v.seat_flag = Some(scene("m/seat_flag.gltf"));
        v.banner = Some(scene("m/banner.gltf"));
        let look = rules::DecorLook {
            visual: scene("m/tree.gltf"),
            at: rules::DecorAt::Any,
            weight: 1,
            shadow: true,
        };
        v.terrain.insert(
            "forest".into(),
            rules::TerrainDecor {
                interior_permille: 1000,
                edge_permille: 1000,
                jitter_x100: 0,
                scale_jitter_pct: 0,
                looks: vec![look],
            },
        );
        let keys: BTreeSet<ModelKey> = drawn_scenes(&v)
            .into_iter()
            .filter_map(ModelKey::of)
            .collect();
        for path in [
            "m/unit.glb",
            "m/house.gltf",
            "m/site_a.gltf",
            "m/site_b.gltf",
            "m/rubble.gltf",
            "m/seat_flag.gltf",
            "m/banner.gltf",
            "m/tree.gltf",
        ] {
            let key = ModelKey::of(&scene(path)).unwrap();
            assert!(keys.contains(&key), "{path} is never requested");
        }
    }
}
