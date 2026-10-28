//! `check <data> <assets>`: every Scene in `visuals.ron` (its parts and
//! node filters resolved, placed with its scale, yaw and offset as the game
//! places it) against the art pass's budgets and import rules
//! (`docs/ART_STYLE.md`): file size, triangles per use, texture size,
//! indexed triangles with `POSITION`, `NORMAL` and `TEXCOORD_0` and one base
//! texture, no cameras, lights, skins or animations, ground contact, team
//! area against `team_swatches`, the reserved-rect rule and the 1.4x
//! silhouette rule within a rig group (units sharing a skeleton, or a
//! file, and a base texture: variants of one character, see
//! `same_rig_group`). It prints one `ok` line per Scene
//! with triangles, bytes, top, size, team share and the facing it finds
//! ([`facing`]), plus a suggested `scale_x100` for a building's footprint
//! or a unit's look height.
//!
//! Owner decision 12 of the art pass: a building's team share is measured
//! on the model plus its team banner (`banner_at`, the `banner` model placed
//! at its data scale), against the same floors, and the line prints the
//! banner's height (and on a Town the Seat flag's). Every mount on a
//! building keeps its banner or flag inside the footprint in plan view, so
//! no cloth hangs over a neighbour's tiles, into the terrain beside it or
//! over the units walking there (a building's own rule, not a unit's: a
//! unit's banner moves with it). On a Town the Seat flag flies wholly
//! above the Town's own banner, so a Seat reads apart from any other Town
//! by its place. A banner taller than its building is only a note: its
//! size is the owner's to judge at the 40 m sign-off.

use crate::bake::rig_key;
use crate::image::Rgba;
use crate::math::{Mat4, Vec3, cross, sub, triangle_area};
use crate::model::{Model, bounds};
use crate::uv::{Cover, Mask, Rect, face_rect};
use rules::visuals::{DecorAt, Mount, Part, Primitive, Visual, Visuals};
use rules::{BuildingKind, UnitKind};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Textures shipped by a pack that may exceed the 256-texel rule because
/// every model of the pack shares them (owner decision 5 of the art pass:
/// KayKit Hexagon's palette atlas is vendored as shipped).
pub const SHIPPED_ATLASES: &[&str] = &["hexagons_medieval.png"];

/// Lowest and highest allowed base of a placed model, metres.
pub const GROUND_Y: (f32, f32) = (-0.02, 0.05);
/// Largest x/z distance of a placed model's centre from its origin, metres.
pub const CENTRE_TOLERANCE: f32 = 0.10;
/// The same for terrain props, which are jittered anyway.
pub const PROP_CENTRE_TOLERANCE: f32 = 0.5;
/// How far a banner or the Seat flag may reach past its building's
/// footprint edge, metres (rounding; the rule is that it stays inside).
pub const FOOTPRINT_TOLERANCE: f32 = 0.01;
/// Share of a footprint side a building model should fill.
pub const FOOTPRINT_FILL: f32 = 0.95;

/// The limits `check` applies (`docs/ART_STYLE.md`, "Budgets").
#[derive(Clone, Debug)]
pub struct Budgets {
    /// Bytes per file.
    pub file_bytes: u64,
    /// Texture side in texels (except [`SHIPPED_ATLASES`]).
    pub texture_side: u32,
    /// A unit above this many triangles prints a note.
    pub unit_triangles_target: usize,
    /// Unit triangle cap.
    pub unit_triangles: usize,
    /// Building triangle cap.
    pub building_triangles: usize,
    /// Construction-site and rubble cap.
    pub site_triangles: usize,
    /// Seat-flag cap.
    pub flag_triangles: usize,
    /// Cap for a prop that stands only on interior tiles.
    pub interior_prop_triangles: usize,
    /// Cap for a prop that may stand on edge tiles.
    pub edge_prop_triangles: usize,
    /// Least team share of a unit, permille.
    pub unit_team_permille: u32,
    /// Least team share of the Supply Wain (only its pennant), permille.
    pub wain_team_permille: u32,
    /// Least team share of the Mangonel (catapult plus banner), permille.
    pub mangonel_team_permille: u32,
    /// Least team share of a building, permille.
    pub building_team_permille: u32,
    /// Least team share of the Seat flag, permille.
    pub flag_team_permille: u32,
}

impl Default for Budgets {
    /// The caps set in art slice 2 from the measured sources
    /// (`inspect_gltf inspect` on every chosen file; `docs/ART_STYLE.md`,
    /// "Budgets"), each a little above the heaviest file it covers:
    ///
    /// - units: the KayKit Adventurers bodies with their kept items are
    ///   4,005 (hooded Rogue) to 5,325 (Barbarian with two axes) triangles,
    ///   and no lighter CC0 character is reachable, so the plan's
    ///   provisional 2,000 / 3,000 became 5,000 / 5,500; the Mangonel's
    ///   catapult nodes are 1,326 and the Wain's parts 668;
    /// - buildings: the castle is the heaviest at 5,659 (under the plan's
    ///   12,000, so it stays);
    /// - sites and rubble: `building_stage_C` 1,218;
    /// - the Seat flag: 48;
    /// - interior props: `trees_B_large` 1,540 (under the plan's 2,000, so
    ///   the large trees stay for interiors);
    /// - edge props: `tree_single_B` 220.
    ///
    /// The team floors are the plan's (units 200‰, the Wain 80‰, buildings
    /// 100‰); the Seat flag's 300‰ sits under its measured 457‰. The
    /// Mangonel's own floor is 120‰ (owner decision 15 of the art pass,
    /// 2026-10-28: its banner shrinks to scale 450, about 123‰, "no crowd
    /// clipping"; centred it still reaches 0.10 m past the 0.5 m radius,
    /// which `check` notes, and a banner inside the radius would miss the
    /// floor; every other unit keeps 200‰).
    fn default() -> Self {
        Budgets {
            file_bytes: 2 * 1024 * 1024,
            texture_side: 256,
            unit_triangles_target: 5_000,
            unit_triangles: 5_500,
            building_triangles: 6_000,
            site_triangles: 1_500,
            flag_triangles: 100,
            interior_prop_triangles: 1_600,
            edge_prop_triangles: 300,
            unit_team_permille: 200,
            wain_team_permille: 80,
            mangonel_team_permille: 120,
            building_team_permille: 100,
            flag_team_permille: 300,
        }
    }
}

/// What a Scene is drawn for.
#[derive(Clone, Copy)]
enum Use<'a> {
    Unit(&'a UnitKind),
    Building(&'a BuildingKind),
    Site,
    Rubble,
    Flag,
    /// The team banner model (owner decision 12 of the art pass).
    Banner,
    Prop(DecorAt),
}

struct SceneRef<'a> {
    path: &'a str,
    scale_x100: u32,
    offset_x100: (i32, i32, i32),
    yaw_deg: i32,
    nodes: &'a [String],
    parts: &'a [Part],
    team: Option<&'a str>,
    fallback: Option<&'a Visual>,
    banner_at: Option<&'a Mount>,
    seat_flag_at: Option<&'a Mount>,
}

fn scene_ref(v: &Visual) -> Option<SceneRef<'_>> {
    match v {
        Visual::Scene {
            path,
            scale_x100,
            y_offset_tiles_x100,
            yaw_deg,
            x_offset_tiles_x100,
            z_offset_tiles_x100,
            nodes,
            parts,
            team,
            fallback,
            banner_at,
            seat_flag_at,
        } => Some(SceneRef {
            path,
            scale_x100: *scale_x100,
            offset_x100: (
                *x_offset_tiles_x100,
                *y_offset_tiles_x100,
                *z_offset_tiles_x100,
            ),
            yaw_deg: *yaw_deg,
            nodes,
            parts,
            team: team.as_deref(),
            fallback: fallback.as_deref(),
            banner_at: banner_at.as_ref(),
            seat_flag_at: seat_flag_at.as_ref(),
        }),
        _ => None,
    }
}

/// The placed, merged model of one Scene.
struct Placed {
    tris: Vec<([Vec3; 3], [[f32; 2]; 3])>,
    texture: Option<(Rgba, String, u64)>,
    files: BTreeMap<PathBuf, u64>,
    rig: String,
    problems: Vec<String>,
}

/// `T(x, lift, z) * S(scale) * R(yaw)`: the game's placement of a Scene
/// (`crates/game/src/models.rs`, `scene_transform`).
fn scene_matrix(s: &SceneRef) -> Mat4 {
    let (x, y, z) = s.offset_x100;
    Mat4::translation([x as f32 / 100.0, y as f32 / 100.0, z as f32 / 100.0])
        .mul(&Mat4::scale(s.scale_x100 as f32 / 100.0))
        .mul(&Mat4::yaw_deg(s.yaw_deg))
}

/// `T(offset) * R(yaw) * S(scale)`: a banner or the Seat flag on its
/// building, from the footprint centre on the ground (the game's
/// `buildings::mount_transform`).
fn mount_matrix(m: &Mount) -> Mat4 {
    let (x, y, z) = m.offset_x100;
    Mat4::translation([x as f32 / 100.0, y as f32 / 100.0, z as f32 / 100.0])
        .mul(&Mat4::yaw_deg(m.yaw_deg))
        .mul(&Mat4::scale(m.scale_x100 as f32 / 100.0))
}

/// `T(offset) * R(yaw) * S(scale)`: a part in its scene's model space.
fn part_matrix(p: &Part) -> Mat4 {
    let o = p.offset_x100;
    Mat4::translation([o.0 as f32 / 100.0, o.1 as f32 / 100.0, o.2 as f32 / 100.0])
        .mul(&Mat4::yaw_deg(p.yaw_deg))
        .mul(&Mat4::scale(p.scale_x100 as f32 / 100.0))
}

/// Load and merge the Scene `s`, placed by `outer * scene_matrix(s)`.
fn place(s: &SceneRef, assets: &Path, budgets: &Budgets, outer: &Mat4) -> Placed {
    let mut placed = Placed {
        tris: Vec::new(),
        texture: None,
        files: BTreeMap::new(),
        rig: String::new(),
        problems: Vec::new(),
    };
    let scene = outer.mul(&scene_matrix(s));
    let mut sources: Vec<(&str, &[String], Mat4)> = vec![(s.path, s.nodes, Mat4::IDENTITY)];
    for p in s.parts {
        sources.push((&p.path, &p.nodes, part_matrix(p)));
    }
    for (i, (path, nodes, local)) in sources.into_iter().enumerate() {
        let file = assets.join(path);
        let model = match Model::load(&file) {
            Ok(m) => m,
            Err(e) => {
                placed.problems.push(e);
                continue;
            }
        };
        if i == 0 {
            placed.rig = rig_key(&model);
            if placed.rig.is_empty() {
                placed.rig = path.to_string();
            }
        }
        for (f, bytes) in &model.files {
            if *bytes > budgets.file_bytes {
                placed.problems.push(format!(
                    "{} is {bytes} bytes; the limit is {}",
                    f.display(),
                    budgets.file_bytes
                ));
            }
            placed.files.insert(f.clone(), *bytes);
        }
        if model.doc.cameras().count() > 0 {
            placed.problems.push(format!("{path} has cameras"));
        }
        if model
            .doc
            .extensions_used()
            .any(|e| e == "KHR_lights_punctual")
        {
            placed.problems.push(format!("{path} has lights"));
        }
        if model.doc.skins().count() > 0 || model.doc.animations().count() > 0 {
            placed.problems.push(format!(
                "{path} has skins or animations; bake it first (tools/assets/bake.ron)"
            ));
        }
        let prims = match model.prims(&model.rest_locals(), &|n| crate::glob::keeps(nodes, n)) {
            Ok(p) => p,
            Err(e) => {
                placed.problems.push(e);
                continue;
            }
        };
        if prims.is_empty() {
            placed
                .problems
                .push(format!("{path}: no mesh node matches nodes {nodes:?}"));
        }
        let m = scene.mul(&local);
        for p in &prims {
            let what = format!("{path} node {:?}", p.node_name);
            if !p.triangles_mode {
                placed
                    .problems
                    .push(format!("{what} is {}, not triangles", p.mode));
                continue;
            }
            if !p.indexed {
                placed.problems.push(format!("{what} is not indexed"));
            }
            if p.normals.is_none() {
                placed.problems.push(format!("{what} has no NORMAL"));
            }
            let Some(uvs) = &p.uvs else {
                placed.problems.push(format!("{what} has no TEXCOORD_0"));
                continue;
            };
            if p.tex_coord != 0 {
                placed.problems.push(format!(
                    "{what}'s base texture reads TEXCOORD_{}",
                    p.tex_coord
                ));
            }
            let Some(image) = p.image else {
                placed
                    .problems
                    .push(format!("{what} has no base-colour texture"));
                continue;
            };
            match model.image(image) {
                Ok(tex) => {
                    let hash = tex.content_hash();
                    match &placed.texture {
                        None => placed.texture = Some((tex, model.image_name(image), hash)),
                        Some((_, name, h)) if *h != hash => {
                            placed.problems.push(format!(
                                "{what} uses {} but the model already uses {name}; one base texture per model",
                                model.image_name(image)
                            ));
                        }
                        Some(_) => {}
                    }
                }
                Err(e) => placed.problems.push(e),
            }
            for t in 0..p.triangles() {
                let c = p.tri(t);
                placed.tris.push((
                    [
                        m.point(p.positions[c[0]]),
                        m.point(p.positions[c[1]]),
                        m.point(p.positions[c[2]]),
                    ],
                    [uvs[c[0]], uvs[c[1]], uvs[c[2]]],
                ));
            }
        }
    }
    placed
}

/// The horizontal direction a placed model shows the most surface to, and
/// that direction's share of all horizontally facing surface, in percent:
/// each triangle's outward normal (from its winding, weighted by its area)
/// adds its +X, -X, +Z and -Z parts to four separate totals, and the
/// largest wins (ties in the order +Z, -Z, +X, -X). Separate totals,
/// because over a closed mesh the signed normals sum to about zero.
///
/// It is a hint for the rest-facing rules (`docs/ART_STYLE.md`, import
/// rules: units face +X after `yaw_deg`, buildings, sites and rubble face
/// +Z toward the camera, props have no facing): a model whose front is its
/// largest side reads its front here, a share near 25% means no side
/// dominates (a symmetric house), and the 40 m screenshots decide. `None`
/// for a model with no horizontally facing surface.
pub fn facing(tris: &[([Vec3; 3], [[f32; 2]; 3])]) -> Option<(&'static str, u32)> {
    const NAMES: [&str; 4] = ["+Z", "-Z", "+X", "-X"];
    let mut area = [0.0f64; 4];
    for (p, _) in tris {
        let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        area[0] += f64::from(n[2].max(0.0));
        area[1] += f64::from((-n[2]).max(0.0));
        area[2] += f64::from(n[0].max(0.0));
        area[3] += f64::from((-n[0]).max(0.0));
    }
    let total: f64 = area.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let best = (1..4).fold(0, |b, i| if area[i] > area[b] { i } else { b });
    Some((NAMES[best], (100.0 * area[best] / total).round() as u32))
}

/// The team and whole surface of a placed model's faces against the
/// swatch `key`, and how many faces straddle a rect edge.
struct Surface {
    team: f64,
    straddles: usize,
}

/// Measure `faces` (texel rect and area of each triangle) of a model whose
/// base texture is `tex` against `team_swatches[key]`: an unknown key or a
/// rect outside the texture is a problem.
fn swatch_surface(
    faces: &[(Rect, f32)],
    tex: &Rgba,
    key: &str,
    visuals: &Visuals,
) -> Result<Surface, String> {
    let Some(swatch) = visuals.team_swatches.get(key) else {
        return Err(format!("team {key:?} is not a key of team_swatches"));
    };
    let rects: Vec<Rect> = swatch
        .rects
        .iter()
        .map(|&(x, y, w, h)| Rect { x, y, w, h })
        .collect();
    if let Some(r) = rects
        .iter()
        .find(|r| r.x + r.w > tex.width || r.y + r.h > tex.height)
    {
        return Err(format!(
            "team_swatches[{key:?}] rect {r} lies outside the {}x{} texture",
            tex.width, tex.height
        ));
    }
    let mask = Mask::from_rects(tex.width, tex.height, &rects);
    let (mut team, mut straddles) = (0.0f64, 0);
    for (r, a) in faces {
        match mask.cover(r) {
            Cover::Inside => team += f64::from(*a),
            Cover::Straddles => straddles += 1,
            Cover::Outside => {}
        }
    }
    Ok(Surface { team, straddles })
}

/// How far placed triangles (metres from a building's footprint centre)
/// reach past the edge of `kind`'s footprint in plan view, if they do.
fn past_footprint(tris: &[([Vec3; 3], [[f32; 2]; 3])], kind: &BuildingKind) -> Option<f32> {
    let (lo, hi) = bounds(tris.iter().flat_map(|(p, _)| p.iter()))?;
    let (hx, hz) = (
        kind.width_tiles as f32 / 2.0,
        kind.height_tiles as f32 / 2.0,
    );
    let past = (-hx - lo[0])
        .max(hi[0] - hx)
        .max(-hz - lo[2])
        .max(hi[2] - hz);
    (past > FOOTPRINT_TOLERANCE).then_some(past)
}

/// Height of placed triangles (top minus base), metres; 0 for none.
fn height(tris: &[([Vec3; 3], [[f32; 2]; 3])]) -> f32 {
    bounds(tris.iter().flat_map(|(p, _)| p.iter())).map_or(0.0, |(lo, hi)| hi[1] - lo[1])
}

/// The nominal height of a unit's Primitive or Body look, metres.
fn look_height(v: &Visual, kind: &UnitKind) -> Option<f32> {
    let r = kind.radius_tiles_x100 as f32 / 100.0;
    match v {
        Visual::Primitive(Primitive::Capsule) => Some(3.4 * r),
        Visual::Primitive(_) => Some(2.0 * r),
        Visual::Body {
            height_tiles_x100,
            lying,
            ..
        } => Some(if *lying {
            2.0 * r
        } else {
            *height_tiles_x100 as f32 / 100.0
        }),
        Visual::Scene { .. } => None,
    }
}

/// The result of a check.
#[derive(Debug, Default)]
pub struct CheckReport {
    /// One line per Scene (`ok …` or `FAIL …`) and notes.
    pub lines: Vec<String>,
    /// Every problem found.
    pub problems: Vec<String>,
    /// Scenes checked.
    pub scenes: usize,
}

impl CheckReport {
    /// The printed report, ending in the summary line.
    pub fn text(&self) -> String {
        let mut out = self.lines.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        if self.problems.is_empty() {
            out.push_str(&format!("check: ok {} scene(s)\n", self.scenes));
        } else {
            out.push_str(&format!(
                "check: FAIL {} problem(s) in {} scene(s)\n",
                self.problems.len(),
                self.scenes
            ));
        }
        out
    }
}

struct Measured<'a> {
    field: String,
    path: &'a str,
    usage: Use<'a>,
    team: Option<&'a str>,
    size: Vec3,
    rig: String,
    texture_hash: Option<u64>,
    faces: Vec<(Rect, f32)>,
}

/// Check every Scene of `visuals` against the files under `assets`.
pub fn check_visuals(
    visuals: &Visuals,
    units: &[UnitKind],
    buildings: &[BuildingKind],
    assets: &Path,
    budgets: &Budgets,
) -> CheckReport {
    let mut entries: Vec<(String, Use, &Visual)> = Vec::new();
    for (id, v) in &visuals.units {
        if let Some(kind) = units.iter().find(|k| &k.id == id) {
            entries.push((format!("units[{id:?}]"), Use::Unit(kind), v));
        }
    }
    for (id, v) in &visuals.buildings {
        if let Some(kind) = buildings.iter().find(|k| &k.id == id) {
            entries.push((format!("buildings[{id:?}]"), Use::Building(kind), v));
        }
    }
    for (i, v) in visuals.sites.iter().enumerate() {
        entries.push((format!("sites[{i}]"), Use::Site, v));
    }
    if let Some(r) = &visuals.rubble {
        entries.push(("rubble.visual".into(), Use::Rubble, &r.visual));
    }
    if let Some(f) = &visuals.seat_flag {
        entries.push(("seat_flag".into(), Use::Flag, f));
    }
    if let Some(b) = &visuals.banner {
        entries.push(("banner".into(), Use::Banner, b));
    }
    for (key, decor) in &visuals.terrain {
        for (i, look) in decor.looks.iter().enumerate() {
            entries.push((
                format!("terrain[{key:?}].looks[{i}].visual"),
                Use::Prop(look.at),
                &look.visual,
            ));
        }
    }

    let mut report = CheckReport::default();
    let mut measured: Vec<Measured> = Vec::new();
    let mut all_files: BTreeSet<PathBuf> = BTreeSet::new();
    for (field, usage, visual) in entries {
        let Some(s) = scene_ref(visual) else { continue };
        report.scenes += 1;
        let placed = place(&s, assets, budgets, &Mat4::IDENTITY);
        let mut problems = placed.problems.clone();
        let mut notes = Vec::new();
        all_files.extend(placed.files.keys().cloned());
        let tris = placed.tris.len();
        let cap = match usage {
            Use::Unit(_) => budgets.unit_triangles,
            Use::Building(_) => budgets.building_triangles,
            Use::Site | Use::Rubble => budgets.site_triangles,
            Use::Flag | Use::Banner => budgets.flag_triangles,
            Use::Prop(DecorAt::Interior) => budgets.interior_prop_triangles,
            Use::Prop(_) => budgets.edge_prop_triangles,
        };
        if tris > cap {
            problems.push(format!("{tris} triangles; the cap for this use is {cap}"));
        } else if matches!(usage, Use::Unit(_)) && tris > budgets.unit_triangles_target {
            notes.push(format!(
                "{tris} triangles is over the unit target {}",
                budgets.unit_triangles_target
            ));
        }
        let (lo, hi) =
            bounds(placed.tris.iter().flat_map(|(p, _)| p.iter())).unwrap_or(([0.0; 3], [0.0; 3]));
        let size = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
        if tris > 0 {
            if lo[1] < GROUND_Y.0 || lo[1] > GROUND_Y.1 {
                problems.push(format!(
                    "its base is at y={:.3} m, outside [{}, {}]; set y_offset_tiles_x100",
                    lo[1], GROUND_Y.0, GROUND_Y.1
                ));
            }
            let tol = if matches!(usage, Use::Prop(_)) {
                PROP_CENTRE_TOLERANCE
            } else {
                CENTRE_TOLERANCE
            };
            let (cx, cz) = ((lo[0] + hi[0]) / 2.0, (lo[2] + hi[2]) / 2.0);
            if cx.abs() > tol || cz.abs() > tol {
                problems.push(format!(
                    "its x/z centre is ({cx:.3}, {cz:.3}) m, more than {tol} m from the origin"
                ));
            }
        }
        let mut texture_desc = "none".to_string();
        let mut faces = Vec::new();
        let mut model_team: Option<f64> = None;
        if let Some((tex, name, _)) = &placed.texture {
            texture_desc = format!("{}x{}", tex.width, tex.height);
            let base = Path::new(name)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            if tex.width.max(tex.height) > budgets.texture_side {
                if SHIPPED_ATLASES.contains(&base.as_str()) {
                    notes.push(format!("{name} is {texture_desc}, over {} texels, shipped as is (owner decision 5)", budgets.texture_side));
                } else {
                    problems.push(format!(
                        "{name} is {texture_desc}; the limit is {} texels a side",
                        budgets.texture_side
                    ));
                }
            }
            for (p, uv) in &placed.tris {
                faces.push((
                    face_rect(*uv, tex.width, tex.height),
                    triangle_area(p[0], p[1], p[2]),
                ));
            }
            if let Some(key) = s.team {
                match swatch_surface(&faces, tex, key, visuals) {
                    Err(p) => problems.push(p),
                    Ok(surface) => {
                        if surface.straddles > 0 {
                            problems.push(format!(
                                "{} face(s) straddle a team rect edge, so the repaint would split them (reserved-rect rule)",
                                surface.straddles
                            ));
                        }
                        model_team = Some(surface.team);
                    }
                }
            }
        }
        let model_area: f64 = faces.iter().map(|(_, a)| f64::from(*a)).sum();

        // Owner decision 12: the team banner (and on a Seat the Seat flag)
        // at the building's mounts, measured at their data scale.
        let mut mounts_desc = String::new();
        let mut banner_team: Option<(f64, f64)> = None;
        let mut banner_top = None;
        if let (Use::Building(_) | Use::Unit(_), Some(mount)) = (usage, s.banner_at) {
            match visuals.banner.as_ref().and_then(scene_ref) {
                None => problems.push(
                    "banner_at names where the banner stands, but there is no banner model".into(),
                ),
                Some(b) => {
                    let bp = place(&b, assets, budgets, &mount_matrix(mount));
                    problems.extend(bp.problems.iter().map(|p| format!("banner: {p}")));
                    all_files.extend(bp.files.keys().cloned());
                    let h = height(&bp.tris);
                    banner_top = bounds(bp.tris.iter().flat_map(|(p, _)| p.iter())).map(|b| b.1[1]);
                    mounts_desc.push_str(&format!(" banner={h:.2}m"));
                    if let Use::Unit(kind) = usage {
                        // A unit's banner moves with it, so no footprint
                        // holds it; how far it reaches from the unit's
                        // centre in plan view (any facing) says whether
                        // packed units' banners cross (owner decision 15).
                        let reach = bp
                            .tris
                            .iter()
                            .flat_map(|(p, _)| p.iter())
                            .map(|v| v[0].hypot(v[2]))
                            .fold(0.0f32, f32::max);
                        let radius = kind.radius_tiles_x100 as f32 / 100.0;
                        mounts_desc.push_str(&format!(" banner_reach={reach:.2}m"));
                        if reach > radius + FOOTPRINT_TOLERANCE {
                            notes.push(format!(
                                "the banner reaches {reach:.2} m from the unit's centre, {:.2} m past its {radius:.2} m radius, so packed units' banners cross (a size for the owner to judge)",
                                reach - radius
                            ));
                        }
                    }
                    if let Use::Building(kind) = usage {
                        if let Some(past) = past_footprint(&bp.tris, kind) {
                            problems.push(format!(
                                "the banner reaches {past:.2} m past the {}x{} footprint's edge, over a neighbour's tiles (a building or prop beside it would be pierced, units there covered); keep banner_at inside the footprint",
                                kind.width_tiles, kind.height_tiles
                            ));
                        }
                        if tris > 0 && h > size[1] {
                            notes.push(format!(
                                "the banner stands {h:.2} m tall, over the building's {:.2} m (a size for the owner to judge at the 40 m sign-off)",
                                size[1]
                            ));
                        }
                    }
                    let area: f64 = bp
                        .tris
                        .iter()
                        .map(|(p, _)| f64::from(triangle_area(p[0], p[1], p[2])))
                        .sum();
                    let mut team = 0.0;
                    if let (Some(key), Some((tex, _, _))) = (b.team, &bp.texture) {
                        let bfaces: Vec<(Rect, f32)> = bp
                            .tris
                            .iter()
                            .map(|(p, uv)| {
                                (
                                    face_rect(*uv, tex.width, tex.height),
                                    triangle_area(p[0], p[1], p[2]),
                                )
                            })
                            .collect();
                        match swatch_surface(&bfaces, tex, key, visuals) {
                            Err(p) => problems.push(format!("banner: {p}")),
                            Ok(surface) => {
                                if surface.straddles > 0 {
                                    problems.push(format!(
                                        "banner: {} face(s) straddle a team rect edge (reserved-rect rule)",
                                        surface.straddles
                                    ));
                                }
                                team = surface.team;
                            }
                        }
                    }
                    banner_team = Some((team, area));
                }
            }
        }
        if let (Use::Building(kind), Some(mount)) = (usage, s.seat_flag_at)
            && kind.town
        {
            match visuals.seat_flag.as_ref().and_then(scene_ref) {
                None => problems.push(
                    "seat_flag_at names where the Seat flag stands, but there is no seat_flag model"
                        .into(),
                ),
                Some(f) => {
                    let fp = place(&f, assets, budgets, &mount_matrix(mount));
                    problems.extend(fp.problems.iter().map(|p| format!("seat flag: {p}")));
                    let h = height(&fp.tris);
                    mounts_desc.push_str(&format!(" seat_flag={h:.2}m"));
                    if let Some(past) = past_footprint(&fp.tris, kind) {
                        problems.push(format!(
                            "the Seat flag reaches {past:.2} m past the {}x{} footprint's edge; keep seat_flag_at inside the footprint",
                            kind.width_tiles, kind.height_tiles
                        ));
                    }
                    let foot = bounds(fp.tris.iter().flat_map(|(p, _)| p.iter())).map(|b| b.0[1]);
                    if let (Some(foot), Some(top)) = (foot, banner_top)
                        && foot <= top
                    {
                        problems.push(format!(
                            "the Seat flag's foot at {foot:.2} m stands under the top of the Town's banner at {top:.2} m; it must fly wholly above that banner, so a Seat reads apart from any other Town"
                        ));
                    }
                }
            }
        }
        let mut model_team_desc = String::new();
        let team_permille = match (model_team, banner_team) {
            (None, None) => None,
            (model, banner) => {
                let (bt, ba) = banner.unwrap_or((0.0, 0.0));
                let total = model_area + ba;
                let share = |team: f64, area: f64| {
                    if area > 0.0 {
                        (1000.0 * team / area).round() as u32
                    } else {
                        0
                    }
                };
                if banner.is_some() {
                    model_team_desc =
                        format!(" model_team={}‰", share(model.unwrap_or(0.0), model_area));
                }
                Some(share(model.unwrap_or(0.0) + bt, total))
            }
        };
        if let Some(permille) = team_permille {
            let floor = match usage {
                Use::Unit(k) if k.id == "supply_wain" => budgets.wain_team_permille,
                Use::Unit(k) if k.id == "mangonel" => budgets.mangonel_team_permille,
                Use::Unit(_) => budgets.unit_team_permille,
                Use::Building(_) => budgets.building_team_permille,
                Use::Flag | Use::Banner => budgets.flag_team_permille,
                _ => 0,
            };
            let with = if banner_team.is_some() {
                " (model plus banner)"
            } else {
                ""
            };
            if permille < floor {
                problems.push(format!(
                    "team area {permille}‰{with} is under the {floor}‰ floor"
                ));
            }
        }
        let mut suggest = String::new();
        let unscale = 100.0 / s.scale_x100.max(1) as f32;
        match usage {
            Use::Building(k) => {
                let side = k.width_tiles.min(k.height_tiles) as f32 * FOOTPRINT_FILL;
                let extent = size[0].max(size[2]) * unscale;
                if extent > 0.0 {
                    suggest = format!(
                        " suggest scale_x100={} ({side:.2} m footprint)",
                        (100.0 * side / extent).round()
                    );
                }
            }
            Use::Site | Use::Rubble => {
                let extent = size[0].max(size[2]) * unscale;
                if extent > 0.0 {
                    suggest = format!(
                        " suggest scale_x100={} ({FOOTPRINT_FILL} m per footprint tile)",
                        (100.0 * FOOTPRINT_FILL / extent).round()
                    );
                }
            }
            Use::Unit(k) => {
                if let Some(h) = s.fallback.and_then(|f| look_height(f, k))
                    && size[1] > 0.0
                {
                    suggest = format!(
                        " suggest scale_x100={} (fallback height {h:.2} m)",
                        (100.0 * h / (size[1] * unscale)).round()
                    );
                }
            }
            _ => {}
        }
        let bytes: u64 = placed.files.values().sum();
        let team_desc = team_permille.map_or("-".to_string(), |p| format!("{p}‰"));
        // Props have no facing (they are turned by the tile hash).
        let facing_desc = match (usage, facing(&placed.tris)) {
            (Use::Prop(_), _) | (_, None) => "-".to_string(),
            (_, Some((dir, share))) => format!("{dir}({share}%)"),
        };
        let measures = format!(
            "tris={tris} bytes={bytes} top={:.2} size={:.2}x{:.2}x{:.2} team={team_desc}{model_team_desc}{mounts_desc} facing={facing_desc} texture={texture_desc}{suggest}",
            hi[1], size[0], size[1], size[2]
        );
        if problems.is_empty() {
            report
                .lines
                .push(format!("ok {field} {} {measures}", s.path));
        } else {
            for p in &problems {
                report.lines.push(format!("FAIL {field} {}: {p}", s.path));
                report.problems.push(format!("{field}: {p}"));
            }
            if tris > 0 {
                report.lines.push(format!("  measured {field}: {measures}"));
            }
        }
        for n in notes {
            report.lines.push(format!("  note {field}: {n}"));
        }
        measured.push(Measured {
            field,
            path: s.path,
            usage,
            team: s.team,
            size,
            rig: placed.rig,
            texture_hash: placed.texture.as_ref().map(|t| t.2),
            faces,
        });
    }

    // Reserved rects: a neutral model must not sample the team rects of a
    // swatch that team models on the same texture use, or it would show a
    // team-looking colour.
    let mut team_rects: BTreeMap<u64, Vec<(String, Rect)>> = BTreeMap::new();
    for m in &measured {
        if let (Some(key), Some(hash)) = (m.team, m.texture_hash)
            && let Some(swatch) = visuals.team_swatches.get(key)
        {
            let list = team_rects.entry(hash).or_default();
            for &(x, y, w, h) in &swatch.rects {
                let r = Rect { x, y, w, h };
                if !list.iter().any(|(k, q)| k == key && *q == r) {
                    list.push((key.to_string(), r));
                }
            }
        }
    }
    for m in &measured {
        let (None, Some(hash)) = (m.team, m.texture_hash) else {
            continue;
        };
        let Some(rects) = team_rects.get(&hash) else {
            continue;
        };
        let hits = m
            .faces
            .iter()
            .filter(|(r, _)| rects.iter().any(|(_, q)| q.intersects(r)))
            .count();
        if hits > 0 {
            let msg = format!(
                "{hits} face(s) of this neutral model sample team rects of the same texture (reserved-rect rule)"
            );
            report
                .lines
                .push(format!("FAIL {} {}: {msg}", m.field, m.path));
            report.problems.push(format!("{}: {msg}", m.field));
        }
    }

    // The 1.4x silhouette rule over placed unit bounds within a rig group.
    for (i, a) in measured.iter().enumerate() {
        for b in &measured[i + 1..] {
            let (Use::Unit(_), Use::Unit(_)) = (a.usage, b.usage) else {
                continue;
            };
            if !same_rig_group(a, b) {
                continue;
            }
            let apart = (0..3).any(|c| {
                let (lo, hi) = (a.size[c].min(b.size[c]), a.size[c].max(b.size[c]));
                hi >= lo * 1.4
            });
            if !apart {
                let msg = format!(
                    "same rig as {} and less than 1.4x apart on every axis ({:.2}x{:.2}x{:.2} vs {:.2}x{:.2}x{:.2} m); each kind needs its own silhouette",
                    a.field, b.size[0], b.size[1], b.size[2], a.size[0], a.size[1], a.size[2]
                );
                report
                    .lines
                    .push(format!("FAIL {} {}: {msg}", b.field, b.path));
                report.problems.push(format!("{}: {msg}", b.field));
            }
        }
    }
    report
        .lines
        .push(format!("files: {} distinct", all_files.len()));
    report
}

/// Whether two measured units are variants of one character, whose
/// silhouettes the 1.4x rule compares: the same rig (the baked
/// `asset.extras.rig`, the hash of a skin's joint names, else the file)
/// and the same base texture. Art slice 4 added the texture: every `KayKit`
/// Adventurers character is skinned to one 41-joint skeleton, so the rig
/// alone would hold the Knight, the Mage, the Barbarian and both Rogues in
/// one group, while their bodies, heads and clothes differ; each one's
/// atlas (`knight_texture`, `mage_texture`, ...) is what its variants
/// share (the hooded Rogue embeds the Rogue's bytes, so the Yeoman and the
/// Bowman are compared, as are the Shieldbearer and the Outrider).
fn same_rig_group(a: &Measured, b: &Measured) -> bool {
    a.rig == b.rig && a.texture_hash == b.texture_hash
}

/// Load the rules under `data` and check `visuals.ron` against `assets`.
pub fn check_dirs(data: &Path, assets: &Path) -> Result<CheckReport, String> {
    let rules = rules::Rules::load(data).map_err(|e| e.to_string())?;
    Ok(check_visuals(
        &rules.visuals,
        &rules.units,
        &rules.buildings,
        assets,
        &Budgets::default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{TempDir, box_gltf, repo_root};

    #[test]
    fn check_flags_budgets_ground_contact_team_area_and_silhouettes() {
        let dir = TempDir::new("check");
        let assets = dir.path().join("assets");
        std::fs::create_dir_all(assets.join("m")).unwrap();
        // Boxes: size, base y, and whether the four sides sample the team
        // texels (4..8, 0..4 of the test atlas) or a neutral one.
        box_gltf(&assets.join("m"), "unit", [0.5, 1.5, 0.5], 0.0, true);
        box_gltf(&assets.join("m"), "floating", [0.5, 1.5, 0.5], 0.5, true);
        box_gltf(&assets.join("m"), "house", [2.0, 1.5, 2.0], 0.0, true);
        box_gltf(&assets.join("m"), "rock", [0.5, 0.4, 0.5], 0.0, false);
        let rules = rules::Rules::load(repo_root().join("data")).unwrap();
        let ron = |units: &str, buildings: &str| {
            format!(
                r#"(units: {{ {units} }}, buildings: {{ {buildings} }},
                   team_swatches: {{ "t": (rects: [(4, 0, 4, 4)], desaturate_permille: 0) }},
                   terrain: {{ "mountain": (interior_permille: 1000, edge_permille: 0, jitter_x100: 0, scale_jitter_pct: 0,
                      looks: [(visual: Scene(path: "m/rock.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0), at: Edge, weight: 1)]) }})"#
            )
        };
        let unit = |id: &str, file: &str, scale: u32| {
            format!(
                r#""{id}": Scene(path: "m/{file}.gltf", scale_x100: {scale}, y_offset_tiles_x100: 0, yaw_deg: 0, team: Some("t"), fallback: Some(Primitive(Capsule))),"#
            )
        };
        let house = r#""croft": Scene(path: "m/house.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0, team: Some("t")),"#;
        let run = |text: String, budgets: &Budgets| {
            let v: Visuals = ron::from_str(&text).unwrap();
            check_visuals(&v, &rules.units, &rules.buildings, &assets, budgets)
        };
        let budgets = Budgets::default();

        // Two kinds on one rig 1.5x apart in height, a house and a rock: ok.
        let good = run(
            ron(
                &(unit("yeoman", "unit", 100) + &unit("scribe", "unit", 150)),
                house,
            ),
            &budgets,
        );
        assert!(good.problems.is_empty(), "{}", good.text());
        assert_eq!(good.scenes, 4);
        let text = good.text();
        assert!(
            text.contains("ok units[\"yeoman\"] m/unit.gltf tris=12"),
            "{text}"
        );
        // The four sides sample the team rect, the top and bottom do not:
        // 3.0 of 3.5 m² on the unit, 12 of 20 m² on the house.
        assert!(text.contains("team=857‰"), "{text}");
        assert!(text.contains("team=600‰"), "{text}");
        // The 2 m house fills FOOTPRINT_FILL of the Croft's footprint side,
        // read from the rules, so a data-only footprint change does not
        // fail this test.
        let croft = rules.buildings.iter().find(|k| k.id == "croft").unwrap();
        let side = croft.width_tiles.min(croft.height_tiles) as f32 * FOOTPRINT_FILL;
        let want = format!(
            "suggest scale_x100={} ({side:.2} m footprint)",
            (100.0 * side / 2.0).round()
        );
        assert!(text.contains(&want), "{want}\n{text}");
        // The yeoman's capsule fallback is 3.4 of its radii tall.
        let yeoman = rules.units.iter().find(|k| k.id == "yeoman").unwrap();
        let height = 3.4 * yeoman.radius_tiles_x100 as f32 / 100.0;
        let want = format!("(fallback height {height:.2} m)");
        assert!(text.contains(&want), "{want}\n{text}");
        assert!(text.ends_with("check: ok 4 scene(s)\n"), "{text}");

        // Same rig, same size: the silhouette rule fires.
        let clash = run(
            ron(
                &(unit("yeoman", "unit", 100) + &unit("scribe", "unit", 110)),
                house,
            ),
            &budgets,
        );
        assert!(
            clash.text().contains("FAIL units[\"yeoman\"]")
                || clash.text().contains("FAIL units[\"scribe\"]"),
            "{}",
            clash.text()
        );
        assert!(
            clash.text().contains("less than 1.4x apart"),
            "{}",
            clash.text()
        );

        // Floating off the ground.
        let float = run(ron(&unit("yeoman", "floating", 100), house), &budgets);
        assert!(
            float.text().contains("its base is at y=0.500 m"),
            "{}",
            float.text()
        );

        // Over a triangle cap, and under the team floor.
        let tight = Budgets {
            edge_prop_triangles: 6,
            building_team_permille: 700,
            ..Budgets::default()
        };
        let over = run(ron(&unit("yeoman", "unit", 100), house), &tight);
        let text = over.text();
        assert!(
            text.contains("12 triangles; the cap for this use is 6"),
            "{text}"
        );
        assert!(
            text.contains("team area 600‰ is under the 700‰ floor"),
            "{text}"
        );
        assert!(text.contains("check: FAIL 2 problem(s)"), "{text}");

        // A rect that cuts faces in half breaks the reserved-rect rule.
        let split = run(
            ron(&unit("yeoman", "unit", 100), house).replace("(4, 0, 4, 4)", "(4, 0, 2, 4)"),
            &budgets,
        );
        assert!(
            split.text().contains("straddle a team rect edge"),
            "{}",
            split.text()
        );

        // A neutral model sampling team rects of its texture fails too.
        box_gltf(&assets.join("m"), "rock", [0.5, 0.4, 0.5], 0.0, true);
        let reserved = run(ron(&unit("yeoman", "unit", 100), house), &budgets);
        assert!(
            reserved
                .text()
                .contains("this neutral model sample team rects"),
            "{}",
            reserved.text()
        );

        // A missing file is a problem, not a panic.
        let missing = run(ron(&unit("yeoman", "nope", 100), ""), &budgets);
        assert!(
            missing
                .text()
                .contains("FAIL units[\"yeoman\"] m/nope.gltf"),
            "{}",
            missing.text()
        );
    }

    /// A one-triangle `<name>.gltf` in `dir` for the import rules: with or
    /// without `NORMAL` and indices, and one primitive per texture in
    /// `textures` (files in `dir`), each a triangle on the ground.
    fn tri_gltf(dir: &Path, name: &str, normals: bool, indexed: bool, textures: &[&str]) {
        use crate::writer::{Builder, Node};
        let mut b = Builder::new();
        let mut prims = Vec::new();
        for (i, tex) in textures.iter().enumerate() {
            let x = i as f32 * 0.1;
            let p = [[x, 0.0, 0.0], [x + 0.2, 0.0, 0.0], [x, 0.3, 0.0]];
            let n = [[0.0, 0.0, 1.0]; 3];
            let uv = [[0.15, 0.65]; 3];
            let t = b.uri_texture(tex);
            let m = b.material(Some(t));
            let mut prim = b.prim(
                &p,
                normals.then_some(&n[..]),
                Some(&uv),
                &[0, 1, 2],
                Some(m),
            );
            if !indexed {
                prim.indices = None;
            }
            prims.push(prim);
        }
        let mesh = b.mesh(Some(name), prims);
        let node = b.node(Node {
            name: Some(name.to_string()),
            mesh: Some(mesh),
            ..Node::default()
        });
        b.root(node);
        let (json, bin) = b.gltf(&format!("{name}.bin")).unwrap();
        std::fs::write(dir.join(format!("{name}.bin")), bin).unwrap();
        std::fs::write(dir.join(format!("{name}.gltf")), json).unwrap();
    }

    /// Copy `<from>.gltf` to `<to>.gltf` (sharing its `.bin`), with `field`
    /// (a JSON member such as `"cameras": [...]`) added at the top level.
    fn with_field(dir: &Path, from: &str, to: &str, field: &str) {
        let json = std::fs::read_to_string(dir.join(format!("{from}.gltf"))).unwrap();
        let json = json.replacen('{', &format!("{{{field},"), 1);
        std::fs::write(dir.join(format!("{to}.gltf")), json).unwrap();
    }

    /// One negative case per rule the first test does not reach: the x/z
    /// centre for buildings (both axes) against the looser prop tolerance,
    /// the 256-texel rule and its shipped-atlas exemption, the per-file
    /// cap, cameras, lights, skins and animations, a primitive that is not
    /// indexed or has no `NORMAL`, two base textures, a team key that is
    /// not in `team_swatches`, a swatch rect outside the texture, and the
    /// silhouette rule comparing only models of one rig.
    #[test]
    fn check_flags_centre_texture_file_and_import_rules_and_rig_groups() {
        let dir = TempDir::new("check-rules");
        let assets = dir.path().join("assets");
        let m = assets.join("m");
        std::fs::create_dir_all(&m).unwrap();
        box_gltf(&m, "rock", [0.5, 0.4, 0.5], 0.0, false);
        box_gltf(&m, "unit", [0.5, 1.5, 0.5], 0.0, true);
        box_gltf(&m, "unit2", [0.5, 1.5, 0.5], 0.0, true);
        crate::testkit::box_gltf_with(
            &m,
            "hex",
            [2.0, 1.5, 2.0],
            0.0,
            false,
            "hexagons_medieval.png",
        );
        std::fs::write(
            m.join("atlas2.png"),
            crate::testkit::atlas(true).encode_png().unwrap(),
        )
        .unwrap();
        tri_gltf(&m, "no_normal", false, true, &["atlas.png"]);
        tri_gltf(&m, "unindexed", true, false, &["atlas.png"]);
        tri_gltf(&m, "two_tex", true, true, &["atlas.png", "atlas2.png"]);
        tri_gltf(&m, "tri", true, true, &["atlas.png"]);
        with_field(
            &m,
            "tri",
            "camera",
            r#""cameras": [{"type": "perspective", "perspective": {"yfov": 0.8, "znear": 0.1}}]"#,
        );
        with_field(
            &m,
            "tri",
            "light",
            r#""extensionsUsed": ["KHR_lights_punctual"]"#,
        );
        std::fs::write(m.join("rig.glb"), crate::testkit::rig_glb("LINEAR", false)).unwrap();
        let rules = rules::Rules::load(repo_root().join("data")).unwrap();
        let scene = |path: &str, extra: &str| {
            format!(
                r#"Scene(path: "m/{path}", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0{extra})"#
            )
        };
        // A Visuals with `units`, one Scene as the Croft and one as the
        // only (edge) mountain prop.
        let ron = |units: &str, croft: &str, prop: &str, rect: &str| {
            format!(
                r#"(units: {{ {units} }}, buildings: {{ "croft": {croft} }},
                   team_swatches: {{ "t": (rects: [{rect}], desaturate_permille: 0) }},
                   terrain: {{ "mountain": (interior_permille: 1000, edge_permille: 0, jitter_x100: 0, scale_jitter_pct: 0,
                      looks: [(visual: {prop}, at: Edge, weight: 1)]) }})"#
            )
        };
        let run = |text: String, budgets: &Budgets| {
            let v: Visuals = ron::from_str(&text).unwrap();
            check_visuals(&v, &rules.units, &rules.buildings, &assets, budgets).text()
        };
        let budgets = Budgets::default();
        let rock = scene("rock.gltf", "");

        // The x/z centre: the rock with a copy of itself 0.6 m along x (or
        // z) is centred 0.3 m off. A building fails; a prop, jittered
        // anyway, passes under its 0.5 m tolerance.
        for (axis, offset, centre) in [
            ("x", "(60, 0, 0)", "(0.300, 0.000)"),
            ("z", "(0, 0, 60)", "(0.000, 0.300)"),
        ] {
            let off = scene(
                "rock.gltf",
                &format!(r#", parts: [(path: "m/rock.gltf", offset_x100: {offset})]"#),
            );
            let text = run(ron("", &off, &off, "(4, 0, 4, 4)"), &budgets);
            let want = format!(
                "FAIL buildings[\"croft\"] m/rock.gltf: its x/z centre is {centre} m, more than {CENTRE_TOLERANCE} m"
            );
            assert!(text.contains(&want), "{axis}: {want}\n{text}");
            assert!(
                text.contains("ok terrain[\"mountain\"].looks[0].visual m/rock.gltf"),
                "{axis}: {text}"
            );
            assert!(text.contains("check: FAIL 1 problem(s)"), "{axis}: {text}");
        }

        // Textures over the side limit fail unless they are a shipped atlas.
        let small = Budgets {
            texture_side: 4,
            ..Budgets::default()
        };
        let text = run(ron("", &rock, &rock, "(4, 0, 4, 4)"), &small);
        assert!(text.contains("FAIL buildings[\"croft\"] m/rock.gltf: atlas.png is 8x8; the limit is 4 texels a side"), "{text}");
        let text = run(
            ron("", &scene("hex.gltf", ""), &rock, "(4, 0, 4, 4)"),
            &small,
        );
        assert!(
            text.contains("ok buildings[\"croft\"] m/hex.gltf"),
            "{text}"
        );
        assert!(
            text.contains("hexagons_medieval.png is 8x8, over 4 texels, shipped as is"),
            "{text}"
        );

        // A file over the per-file cap.
        let tiny = Budgets {
            file_bytes: 16,
            ..Budgets::default()
        };
        let text = run(ron("", &rock, &rock, "(4, 0, 4, 4)"), &tiny);
        assert!(text.contains("bytes; the limit is 16"), "{text}");

        // Import rules, each on its own model as the Croft.
        for (file, want) in [
            ("camera.gltf", "m/camera.gltf has cameras"),
            ("light.gltf", "m/light.gltf has lights"),
            ("rig.glb", "m/rig.glb has skins or animations"),
            (
                "no_normal.gltf",
                "m/no_normal.gltf node \"no_normal\" has no NORMAL",
            ),
            (
                "unindexed.gltf",
                "m/unindexed.gltf node \"unindexed\" is not indexed",
            ),
            (
                "two_tex.gltf",
                "uses atlas2.png but the model already uses atlas.png; one base texture per model",
            ),
        ] {
            let text = run(ron("", &scene(file, ""), &rock, "(4, 0, 4, 4)"), &budgets);
            assert!(text.contains(want), "{file}: {want}\n{text}");
            assert!(text.contains("check: FAIL"), "{file}: {text}");
        }
        // The camera and light copies differ from `tri` only by that field.
        let text = run(
            ron("", &scene("tri.gltf", ""), &rock, "(4, 0, 4, 4)"),
            &budgets,
        );
        assert!(
            !text.contains("has cameras") && !text.contains("has lights"),
            "{text}"
        );

        // A team key that is not in team_swatches, and a rect outside the
        // 8x8 texture.
        let unit = |id: &str, file: &str, team: &str| {
            format!(
                r#""{id}": {},"#,
                scene(
                    file,
                    &format!(r#", team: Some("{team}"), fallback: Some(Primitive(Capsule))"#)
                )
            )
        };
        let text = run(
            ron(
                &unit("yeoman", "unit.gltf", "nope"),
                &rock,
                &rock,
                "(4, 0, 4, 4)",
            ),
            &budgets,
        );
        assert!(
            text.contains("team \"nope\" is not a key of team_swatches"),
            "{text}"
        );
        let text = run(
            ron(
                &unit("yeoman", "unit.gltf", "t"),
                &rock,
                &rock,
                "(4, 0, 8, 4)",
            ),
            &budgets,
        );
        assert!(
            text.contains("team_swatches[\"t\"] rect (4, 0, 8, 4) lies outside the 8x8 texture"),
            "{text}"
        );

        // Two kinds of the same size on different rigs are never compared;
        // on one rig they clash.
        let two = unit("yeoman", "unit.gltf", "t") + &unit("scribe", "unit2.gltf", "t");
        let text = run(ron(&two, &rock, &rock, "(4, 0, 4, 4)"), &budgets);
        assert!(!text.contains("less than 1.4x apart"), "{text}");
        assert!(text.ends_with("check: ok 4 scene(s)\n"), "{text}");
        let one = unit("yeoman", "unit.gltf", "t") + &unit("scribe", "unit.gltf", "t");
        let text = run(ron(&one, &rock, &rock, "(4, 0, 4, 4)"), &budgets);
        assert!(text.contains("less than 1.4x apart"), "{text}");
    }

    /// `facing` names the side with the most outward surface and its
    /// share; `check` prints it for every Scene but props, after the yaw.
    #[test]
    fn check_prints_the_facing_it_finds() {
        let dir = TempDir::new("check-facing");
        // One quad facing +Z (from its winding) and a closed box.
        crate::testkit::quads_gltf(&dir, "wall", &[[1.5, 5.5]], 0.0);
        box_gltf(dir.path(), "cube", [1.0, 1.0, 1.0], 0.0, false);
        let rules = rules::Rules::load(repo_root().join("data")).unwrap();
        let run = |croft: &str| {
            let text = format!(
                r#"(units: {{}}, buildings: {{ "croft": {croft} }},
                   terrain: {{ "mountain": (interior_permille: 1000, edge_permille: 0, jitter_x100: 0, scale_jitter_pct: 0,
                      looks: [(visual: Scene(path: "wall.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0), at: Interior, weight: 1)]) }})"#
            );
            let v: Visuals = ron::from_str(&text).unwrap();
            check_visuals(
                &v,
                &rules.units,
                &rules.buildings,
                dir.path(),
                &Budgets::default(),
            )
            .text()
        };
        let wall = |yaw: i32| {
            format!(
                r#"Scene(path: "wall.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: {yaw})"#
            )
        };
        for (yaw, want) in [
            (0, "+Z(100%)"),
            (90, "+X(100%)"),
            (180, "-Z(100%)"),
            (270, "-X(100%)"),
        ] {
            let text = run(&wall(yaw));
            let line = text
                .lines()
                .find(|l| l.contains("buildings[\"croft\"]") && l.contains("tris="))
                .unwrap_or_else(|| panic!("{text}"));
            assert!(
                line.contains(&format!(" facing={want} ")),
                "yaw {yaw}: {line}"
            );
            // The prop has no facing.
            assert!(text.contains("facing=-"), "{text}");
        }
        // A cube shows every side alike: the tie goes to +Z at 25%.
        let text =
            run(r#"Scene(path: "cube.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0)"#);
        assert!(
            text.contains("ok buildings[\"croft\"] cube.gltf") && text.contains(" facing=+Z(25%) "),
            "{text}"
        );
        assert_eq!(facing(&[]), None);
    }

    /// Owner decision 12 of the art pass: a building's team share is
    /// measured on the model plus its banner at the mount's scale, against
    /// the same floor for every kind; a banner or Seat flag that reaches
    /// past the footprint fails, as does a Seat flag whose foot is not
    /// above the Town's banner; a banner taller than its building is a
    /// note; the banner model gets its own line with its facing; and the
    /// Scene's x/z offsets centre an off-centre model.
    #[test]
    fn check_measures_model_plus_banner_and_mounts() {
        let dir = TempDir::new("check-banner");
        let m = dir.path().join("m");
        std::fs::create_dir_all(&m).unwrap();
        // A neutral 2 x 1.5 x 2 m barn (20 m², none on team texels) and a
        // 0.1 x 0.5 x 1 m flag whose four sides sample the team rect.
        box_gltf(&m, "barn", [2.0, 1.5, 2.0], 0.0, false);
        box_gltf(&m, "flag", [0.1, 0.5, 1.0], 0.0, true);
        let rules = rules::Rules::load(repo_root().join("data")).unwrap();
        let flag = r#"Scene(path: "m/flag.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 270, team: Some("t"))"#;
        let run = |town: &str, sites: &str| {
            let text = format!(
                r#"(units: {{}}, buildings: {{ "town": {town} }},
                   team_swatches: {{ "t": (rects: [(4, 0, 4, 4)], desaturate_permille: 0) }},
                   banner: Some({flag}), seat_flag: Some({flag}), sites: [{sites}])"#
            );
            let v: Visuals = ron::from_str(&text).unwrap();
            check_visuals(
                &v,
                &rules.units,
                &rules.buildings,
                dir.path(),
                &Budgets::default(),
            )
            .text()
        };
        // The flag is 1 m long and 0.1 m deep after its 270° yaw; the
        // Town's footprint is 3 x 3 m.
        let town_at = |banner: u32, at: (i32, i32), seat_flag: u32, seat_flag_y: i32| {
            format!(
                r#"Scene(path: "m/barn.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0, team: Some("t"), banner_at: Some((offset_x100: ({}, 0, {}), scale_x100: {banner}, yaw_deg: 30)), seat_flag_at: Some((offset_x100: (0, {seat_flag_y}, 0), scale_x100: {seat_flag})))"#,
                at.0, at.1
            )
        };
        let town = |banner: u32, seat_flag: u32| town_at(banner, (80, 80), seat_flag, 150);
        let site = r#"Primitive(Cuboid)"#;

        // At 1.5x the flag adds 2.475 of 2.925 m² on team texels:
        // 2.475 / 22.925 = 108‰, over the 100‰ floor; the model alone is 0‰.
        let text = run(&town(150, 250), site);
        let line = text
            .lines()
            .find(|l| l.starts_with("ok buildings[\"town\"]"))
            .unwrap_or_else(|| panic!("{text}"));
        assert!(
            line.contains(" team=108‰ model_team=0‰ banner=0.75m seat_flag=1.25m "),
            "{line}"
        );
        assert!(text.ends_with("check: ok 3 scene(s)\n"), "{text}");
        // The banner model is checked like the Seat flag, with its facing:
        // its broad sides turned toward the camera (+Z) by the 270° yaw.
        let banner = text
            .lines()
            .find(|l| l.starts_with("ok banner m/flag.gltf"))
            .unwrap_or_else(|| panic!("{text}"));
        assert!(
            banner.contains(" team=846‰ ") && banner.contains(" facing=+Z("),
            "{banner}"
        );

        // At its own size the banner is too small: 1.1 / 21.3 = 52‰.
        let text = run(&town(100, 250), site);
        assert!(
            text.contains(
                "FAIL buildings[\"town\"] m/barn.gltf: team area 52‰ (model plus banner) is under the 100‰ floor"
            ),
            "{text}"
        );
        // At 4x, 1 m east of the centre, the turned banner reaches 1.13 m
        // past the footprint's east edge: it fails however much team
        // colour it adds. Its height over the building's is only a note.
        let text = run(&town(400, 600), site);
        assert!(
            text.contains(
                "FAIL buildings[\"town\"] m/barn.gltf: the banner reaches 1.13 m past the 3x3 footprint's edge"
            ),
            "{text}"
        );
        assert!(!text.contains("under the 100‰ floor"), "{text}");
        assert!(
            text.contains(
                "note buildings[\"town\"]: the banner stands 2.00 m tall, over the building's 1.50 m"
            ),
            "{text}"
        );
        // Centred, a 1.60 m banner stays inside and passes, with the note.
        let text = run(&town_at(320, (0, 0), 250, 250), site);
        assert!(text.contains("ok buildings[\"town\"]"), "{text}");
        assert!(
            text.contains("the banner stands 1.60 m tall, over the building's 1.50 m"),
            "{text}"
        );
        assert!(text.ends_with("check: ok 3 scene(s)\n"), "{text}");
        // So does a Seat flag 4 m long over a 3 m footprint.
        let text = run(&town(150, 400), site);
        assert!(
            text.contains(
                "FAIL buildings[\"town\"] m/barn.gltf: the Seat flag reaches 0.50 m past the 3x3 footprint's edge"
            ),
            "{text}"
        );
        // A Seat flag standing on the ground beside the banner does not
        // read apart from it: its foot must be above the banner's top.
        let text = run(&town_at(150, (80, 80), 160, 0), site);
        assert!(
            text.contains(
                "the Seat flag's foot at 0.00 m stands under the top of the Town's banner at 0.75 m"
            ),
            "{text}"
        );

        // The x/z offsets: the barn with a copy of itself 0.6 m along +X
        // is centred 0.3 m off; x_offset_tiles_x100: -30 centres it.
        let pair = |offset: &str| {
            format!(
                r#"Scene(path: "m/barn.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0, parts: [(path: "m/barn.gltf", offset_x100: (60, 0, 0))]{offset})"#
            )
        };
        let text = run(&town(150, 250), &pair(""));
        assert!(
            text.contains("FAIL sites[0] m/barn.gltf: its x/z centre is (0.300, 0.000) m"),
            "{text}"
        );
        let text = run(&town(150, 250), &pair(", x_offset_tiles_x100: -30"));
        assert!(text.contains("ok sites[0] m/barn.gltf"), "{text}");
        let text = run(
            &town(150, 250),
            &pair(", x_offset_tiles_x100: -30, z_offset_tiles_x100: 25"),
        );
        assert!(
            text.contains("FAIL sites[0] m/barn.gltf: its x/z centre is (0.000, 0.250) m"),
            "{text}"
        );
    }

    /// Art slice 4 (owner decision 12): a unit's `banner_at` plants the
    /// team banner on it too, and its team share is measured on model plus
    /// banner against the unit floor: 200‰ for every unit but the Mangonel,
    /// whose own floor is 120‰ (owner decision 15, 2026-10-28). A unit's
    /// banner may stand taller than the unit (a standard on a pole over a
    /// siege engine), unlike a building's, and the line prints how far it
    /// reaches from the unit's centre, with a note past the unit's radius.
    #[test]
    fn check_measures_a_unit_banner() {
        let dir = TempDir::new("check-unit-banner");
        let m = dir.path().join("m");
        std::fs::create_dir_all(&m).unwrap();
        box_gltf(&m, "cart", [2.0, 1.5, 2.0], 0.0, false);
        box_gltf(&m, "flag", [0.1, 0.5, 1.0], 0.0, true);
        let rules = rules::Rules::load(repo_root().join("data")).unwrap();
        let run = |kind: &str, banner: u32| {
            let text = format!(
                r#"(units: {{ "{kind}": Scene(path: "m/cart.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0, team: Some("t"), banner_at: Some((offset_x100: (-80, 0, 80), scale_x100: {banner}))) }},
                   buildings: {{}},
                   team_swatches: {{ "t": (rects: [(4, 0, 4, 4)], desaturate_permille: 0) }},
                   banner: Some(Scene(path: "m/flag.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 270, team: Some("t"))))"#
            );
            let v: Visuals = ron::from_str(&text).unwrap();
            check_visuals(
                &v,
                &rules.units,
                &rules.buildings,
                dir.path(),
                &Budgets::default(),
            )
            .text()
        };
        // At 1.5x the banner lifts the neutral cart to 108‰: under even the
        // Mangonel's 120‰.
        let text = run("mangonel", 150);
        assert!(
            text.contains(
                "FAIL units[\"mangonel\"] m/cart.gltf: team area 108‰ (model plus banner) is under the 120‰ floor"
            ),
            "{text}"
        );
        // At 1.9x it gives 161‰ (3.97 of 24.69 m² on team texels): over the
        // Mangonel's floor, under every other unit's 200‰.
        let text = run("mangonel", 190);
        assert!(
            text.lines()
                .any(|l| l.starts_with("ok units[\"mangonel\"]") && l.contains(" team=161‰ ")),
            "{text}"
        );
        let text = run("skirmisher", 190);
        assert!(
            text.contains(
                "FAIL units[\"skirmisher\"] m/cart.gltf: team area 161‰ (model plus banner) is under the 200‰ floor"
            ),
            "{text}"
        );
        // At 4x it stands 2.00 m over the 1.5 m cart and passes: 17.6 of
        // 40.8 m² on team texels. Turned by yaw 270 the 1.0 m deep flag lies
        // along x, so its far corner at (-0.8 - 2.0, 0.8 + 0.2) is 2.97 m
        // from the centre, 2.47 m past the Mangonel's 0.5 m radius.
        let text = run("mangonel", 400);
        let line = text
            .lines()
            .find(|l| l.starts_with("ok units[\"mangonel\"]"))
            .unwrap_or_else(|| panic!("{text}"));
        assert!(
            line.contains(" team=431‰ model_team=0‰ banner=2.00m banner_reach=2.97m "),
            "{line}"
        );
        assert!(
            text.contains("note units[\"mangonel\"]: the banner reaches 2.97 m from the unit's centre, 2.47 m past its 0.50 m radius"),
            "{text}"
        );
        assert!(!text.contains("taller than"), "{text}");
    }

    /// Art slice 4: the silhouette rule compares units of one rig only when
    /// they also share a base texture. Two bakes of one skeleton with
    /// different textures (two characters on a shared rig) are never
    /// compared; with the same texture they clash.
    #[test]
    fn silhouettes_compare_one_rig_on_one_texture() {
        let dir = TempDir::new("check-rig-texture");
        let cache = dir.path().join("cache");
        let assets = dir.path().join("assets");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(
            cache.join("rig.glb"),
            crate::testkit::rig_glb("LINEAR", false),
        )
        .unwrap();
        std::fs::write(
            cache.join("other.png"),
            crate::testkit::atlas(true).encode_png().unwrap(),
        )
        .unwrap();
        let bake_ron = dir.path().join("bake.ron");
        std::fs::write(
            &bake_ron,
            r#"(models: [
                (out: "m/a.glb", src: "rig.glb", keep: ["Body"], pose: (clip: "Bend", at_ms: 0), max_texture: 8),
                (out: "m/b.glb", src: "rig.glb", keep: ["Body"], pose: (clip: "Bend", at_ms: 0), max_texture: 8),
                (out: "m/c.glb", src: "rig.glb", texture: Some("other.png"), keep: ["Body"], pose: (clip: "Bend", at_ms: 0), max_texture: 8),
            ])"#,
        )
        .unwrap();
        crate::bake::bake(&bake_ron, &cache, &assets, None).unwrap();
        let rig = |f: &str| crate::bake::rig_key(&Model::load(&assets.join(f)).unwrap());
        assert_eq!(rig("m/a.glb"), rig("m/c.glb"), "one skeleton");
        let rules = rules::Rules::load(repo_root().join("data")).unwrap();
        let unit = |kind: &str, file: &str| {
            format!(
                r#""{kind}": Scene(path: "m/{file}", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0),"#
            )
        };
        let run = |units: String| {
            let v: Visuals =
                ron::from_str(&format!("(units: {{ {units} }}, buildings: {{}})")).unwrap();
            check_visuals(
                &v,
                &rules.units,
                &rules.buildings,
                &assets,
                &Budgets::default(),
            )
            .text()
        };
        let text = run(unit("yeoman", "a.glb") + &unit("scribe", "c.glb"));
        assert!(!text.contains("less than 1.4x apart"), "{text}");
        let text = run(unit("yeoman", "a.glb") + &unit("scribe", "b.glb"));
        assert!(
            text.contains(r#"FAIL units["yeoman"] m/a.glb: same rig as units["scribe"] and less than 1.4x apart"#),
            "{text}"
        );
    }
}
