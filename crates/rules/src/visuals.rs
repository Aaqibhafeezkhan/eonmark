//! Schema for `data/visuals.ron`: how each unit and building kind is drawn,
//! and (from the art pass) the construction sites, the rubble, the Seat
//! flag, the terrain props and the team-colour swatches of glTF models.
//!
//! The simulation never reads this file; it is loaded by `Rules::load` so
//! that it is validated with everything else (every unit and building kind
//! must have a visual, so `sim-cli data-check` fails on a bad or missing
//! entry). It is not part of `rules_hash` (ADR 0003,
//! `docs/adr/0003-visuals-outside-rules-hash.md`; `Rules::visuals` is
//! `#[serde(skip)]`): changing a look needs no `rules_version` bump and no
//! re-record, and replays never record which looks were loaded. Nothing the
//! sim reads may ever go in here. The game crate maps a [`Visual`] to a
//! Bevy mesh and material in `present.rs` (units) and `buildings.rs`.
//!
//! M2 shipped primitives only; M4b adds [`Visual::Body`], one primitive
//! with its own height (or length, lying along the unit's facing), so each
//! kind has its own silhouette. [`Visual::Scene`] is a glTF model under
//! `assets/`; the art pass (`docs/ROADMAP.md`, "Art pass (pre-stream)")
//! gives it `nodes`, `parts`, a `team` swatch and a `fallback`, and adds
//! the `buildings`, `sites`, `rubble`, `seat_flag`, `terrain` and
//! `team_swatches` tables. Until its models load (art slice 3) the game
//! draws a Scene's fallback. Every number is an integer: fields ending in
//! `_x100` are hundredths, `_permille` thousandths, `_pct` percent, `_ds`
//! deciseconds and `yaw_deg` whole degrees, following `docs/DATA_FORMAT.md`.

use crate::{BuildingKind, Error, UnitKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// A built-in mesh shape. Its width comes from the unit kind's
/// `radius_tiles_x100`; a [`Visual::Primitive`] also takes its height from
/// the radius, a [`Visual::Body`] carries its own. For a building only
/// `Cuboid` is valid: the M3b box covering the footprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Primitive {
    /// Upright capsule (the M2 look for every unit): as a `Primitive`, its
    /// straight part is 1.4 radii long, so it stands 3.4 radii tall.
    Capsule,
    /// Axis-aligned box two radii wide and deep; as a `Primitive`, two
    /// radii tall. As a building or site, the footprint box.
    Cuboid,
    /// Upright cylinder; as a `Primitive`, two radii tall.
    Cylinder,
    /// Sphere of the kind's radius. Only as a `Primitive`: a `Body` is
    /// never a sphere, since a sphere has no height of its own.
    Sphere,
    /// Upright cone on its base (M4b); as a `Primitive`, two radii tall.
    Cone,
}

/// How a kind is drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Visual {
    /// One shared primitive mesh sized from the kind's radius, tinted with
    /// the owner's team colour. For a building, `Primitive(Cuboid)` is the
    /// M3b box over its footprint.
    Primitive(Primitive),
    /// One shared primitive mesh with its own size (M4b): `shape` is two
    /// radii wide and `height_tiles_x100` tall, or, when `lying`, that long
    /// along the unit's facing and two radii tall. Tinted with the owner's
    /// team colour. Units only.
    Body {
        /// The shape; never `Sphere`.
        shape: Primitive,
        /// Height in hundredths of a tile (a tile is a metre), or the
        /// length along the facing when `lying`. In
        /// [`MIN_BODY_HEIGHT_X100`]`..=`[`MAX_BODY_HEIGHT_X100`]; a
        /// `Capsule` is at least two radii (its two half-spheres).
        height_tiles_x100: u32,
        /// Lay the shape on its side with its long axis along the unit's
        /// facing. Only a `Capsule` or a `Cylinder` can lie. Defaults to
        /// `false`.
        #[serde(default)]
        lying: bool,
    },
    /// A glTF model under the assets directory, merged into one mesh at
    /// load (art pass). Rest facing: a unit faces +X after `yaw_deg`;
    /// buildings, sites and rubble put their front toward +Z (toward the
    /// camera); props have no facing.
    Scene {
        /// Path relative to the assets directory, ending in `.glb` or
        /// `.gltf`, with no `..` and no leading `/` or `\`
        /// ([`check_asset_path`]).
        path: String,
        /// Uniform scale in hundredths (`100` is 1.0), 1..=[`MAX_SCALE_X100`].
        scale_x100: u32,
        /// Vertical offset in tiles, hundredths (`-5` sinks the model 5 cm).
        y_offset_tiles_x100: i32,
        /// Extra yaw applied to the model for its rest facing, degrees,
        /// within -360..=360.
        yaw_deg: i32,
        /// Node-name globs (`*` matches any run of characters) of the nodes
        /// to keep; empty keeps every node. At most [`MAX_NODE_GLOBS`],
        /// none empty.
        #[serde(default)]
        nodes: Vec<String>,
        /// Further files composited into the same mesh (a loaded cart, a
        /// pennant), at most [`MAX_PARTS`].
        #[serde(default)]
        parts: Vec<Part>,
        /// The `team_swatches` key whose texel rects are repainted in the
        /// owner's team colour; `None` draws the model as shipped (neutral).
        #[serde(default)]
        team: Option<String>,
        /// What the game draws when the model is missing or broken, and
        /// until models load: for a unit a `Primitive` or a `Body` (never a
        /// `Scene`; `None` draws the presenter's capsule), for a building
        /// or a site `Primitive(Cuboid)` or `None` (the M3b box either
        /// way). Props, the rubble and the Seat flag take none: they are
        /// skipped when their model fails.
        #[serde(default)]
        fallback: Option<Box<Visual>>,
    },
}

/// One more glTF file composited into a [`Visual::Scene`]'s mesh, placed
/// relative to the scene's own origin before the scene's scale and yaw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    /// Path relative to the assets directory, as for `Scene::path`.
    pub path: String,
    /// Node-name globs to keep from this file, as for `Scene::nodes`.
    #[serde(default)]
    pub nodes: Vec<String>,
    /// Offset `(x, y, z)` in hundredths of a tile.
    #[serde(default)]
    pub offset_x100: (i32, i32, i32),
    /// Uniform scale in hundredths, 1..=[`MAX_SCALE_X100`]; defaults to 100.
    #[serde(default = "default_scale_x100")]
    pub scale_x100: u32,
    /// Yaw in degrees, within -360..=360.
    #[serde(default)]
    pub yaw_deg: i32,
}

fn default_scale_x100() -> u32 {
    100
}

/// The texel rects of a model's base texture that are repainted in the
/// owner's team colour (art pass, `docs/ROADMAP.md`), and how much the rest
/// of the texture is desaturated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamSwatch {
    /// Texel rects `(x, y, w, h)` of the base texture, at least one; sides
    /// 1..=[`MAX_TEXTURE_SIDE`] and inside a texture of that size.
    pub rects: Vec<(u32, u32, u32, u32)>,
    /// How far texels outside the rects are desaturated, 0..=1000 permille.
    #[serde(default)]
    pub desaturate_permille: u32,
}

/// The wreck left where a building was destroyed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rubble {
    /// A neutral [`Visual::Scene`] without a fallback.
    pub visual: Visual,
    /// How long it stays, deciseconds, 1..=[`MAX_RUBBLE_DS`].
    pub ds: u32,
}

/// Where on a dressed terrain a [`DecorLook`] may stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecorAt {
    /// Only on a tile whose 8 neighbours are the same terrain.
    Interior,
    /// Only on a tile with a neighbour of another terrain.
    Edge,
    /// On either.
    Any,
}

/// One prop look of a [`TerrainDecor`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecorLook {
    /// A neutral [`Visual::Scene`] without a fallback.
    pub visual: Visual,
    /// Which tiles may take it.
    pub at: DecorAt,
    /// Relative weight among the looks a tile may take, 1..=[`MAX_DECOR_WEIGHT`].
    pub weight: u32,
    /// Casts shadows; defaults to `true`.
    #[serde(default = "default_true")]
    pub shadow: bool,
}

fn default_true() -> bool {
    true
}

/// Props dressing one impassable terrain (art pass).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerrainDecor {
    /// Share of interior tiles that get a prop, 0..=1000 permille.
    pub interior_permille: u32,
    /// Share of edge tiles that get a prop, 0..=1000 permille.
    pub edge_permille: u32,
    /// Largest jitter from the tile centre, hundredths of a tile,
    /// 0..=[`MAX_DECOR_JITTER_X100`].
    pub jitter_x100: u32,
    /// Largest scale jitter, percent, 0..=[`MAX_DECOR_SCALE_JITTER_PCT`].
    pub scale_jitter_pct: u32,
    /// The looks, 1..=[`MAX_DECOR_LOOKS`].
    pub looks: Vec<DecorLook>,
}

/// Contents of `data/visuals.ron`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Visuals {
    /// Visual per unit kind, keyed by the kind's `id` from `units.ron`.
    pub units: BTreeMap<String, Visual>,
    /// Visual per building kind, keyed by the kind's `id` from
    /// `buildings.ron` (art pass): `Primitive(Cuboid)` (the M3b box) or a
    /// `Scene`.
    pub buildings: BTreeMap<String, Visual>,
    /// Team-colour swatches by key, named by a Scene's `team`.
    #[serde(default)]
    pub team_swatches: BTreeMap<String, TeamSwatch>,
    /// Construction-site looks, one per equal share of the build progress
    /// (1..=[`MAX_SITE_STAGES`]): `Primitive(Cuboid)` (the M3b rising box)
    /// or a neutral Scene. Defaults to `[Primitive(Cuboid)]`.
    #[serde(default = "default_sites")]
    pub sites: Vec<Visual>,
    /// The wreck of a destroyed building; `None` leaves nothing.
    #[serde(default)]
    pub rubble: Option<Rubble>,
    /// The flag on every Seat (a Scene); `None` draws none.
    #[serde(default)]
    pub seat_flag: Option<Visual>,
    /// Props per impassable terrain, keyed `forest`, `mountain` or
    /// `water` ([`DECOR_TERRAINS`]); a terrain without an entry is bare.
    #[serde(default)]
    pub terrain: BTreeMap<String, TerrainDecor>,
}

fn default_sites() -> Vec<Visual> {
    vec![Visual::Primitive(Primitive::Cuboid)]
}

impl Default for Visuals {
    fn default() -> Self {
        Self {
            units: BTreeMap::new(),
            buildings: BTreeMap::new(),
            team_swatches: BTreeMap::new(),
            sites: default_sites(),
            rubble: None,
            seat_flag: None,
            terrain: BTreeMap::new(),
        }
    }
}

/// Upper bound on `scale_x100` (a 100x model is a data error).
pub const MAX_SCALE_X100: u32 = 10_000;

/// Lower bound on a [`Visual::Body`]'s `height_tiles_x100` (20 cm).
pub const MIN_BODY_HEIGHT_X100: u32 = 20;

/// Upper bound on a [`Visual::Body`]'s `height_tiles_x100` (4 m, a unit
/// four tiles tall or long is a data error).
pub const MAX_BODY_HEIGHT_X100: u32 = 400;

/// Most `parts` a Scene composites.
pub const MAX_PARTS: usize = 8;

/// Most `nodes` globs a Scene or a part lists.
pub const MAX_NODE_GLOBS: usize = 16;

/// Largest texture side a team-swatch rect may reach, texels.
pub const MAX_TEXTURE_SIDE: u32 = 4096;

/// Most construction-site stages.
pub const MAX_SITE_STAGES: usize = 4;

/// Longest rubble life, deciseconds (two minutes).
pub const MAX_RUBBLE_DS: u32 = 1200;

/// The terrains that may carry props. Grass is buildable land: a prop there
/// would hide buildings.
pub const DECOR_TERRAINS: [&str; 3] = ["forest", "mountain", "water"];

/// Most looks per dressed terrain.
pub const MAX_DECOR_LOOKS: usize = 8;

/// Largest prop weight.
pub const MAX_DECOR_WEIGHT: u32 = 100;

/// Largest prop jitter, hundredths of a tile.
pub const MAX_DECOR_JITTER_X100: u32 = 45;

/// Largest prop scale jitter, percent.
pub const MAX_DECOR_SCALE_JITTER_PCT: u32 = 50;

/// Two unit looks of the same shape must differ by at least this factor on
/// some axis (`docs/ART_STYLE.md`, "one silhouette per kind"), as
/// [`SILHOUETTE_RATIO_NUM`]` / `[`SILHOUETTE_RATIO_DEN`] in integer maths.
pub const SILHOUETTE_RATIO_NUM: u64 = 14;
/// Denominator of the 1.4x silhouette rule.
pub const SILHOUETTE_RATIO_DEN: u64 = 10;

/// `field` (`units["yeoman"]`) with `.name` appended, or as it is when
/// `name` is empty.
fn sub(field: &str, name: &str) -> String {
    if name.is_empty() {
        field.to_string()
    } else {
        format!("{field}.{name}")
    }
}

/// Check an asset path: relative, ending in `.glb` or `.gltf`, with no
/// `..` and no leading `/` or `\`.
pub fn check_asset_path(path: &str) -> Result<(), String> {
    if !(path.ends_with(".glb") || path.ends_with(".gltf")) {
        return Err(format!("{path:?} must end in .glb or .gltf"));
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(format!(
            "{path:?} must be relative to the assets directory (no leading / or \\)"
        ));
    }
    if path.contains("..") {
        return Err(format!(
            "{path:?} must stay inside the assets directory (no ..)"
        ));
    }
    Ok(())
}

/// Who a Scene is drawn for, which decides its `team` and `fallback`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SceneUse {
    /// A unit: any team; the fallback is a Primitive or a Body.
    Unit,
    /// A building or a site: the fallback is absent or `Primitive(Cuboid)`.
    /// `neutral` (a site) takes no team.
    Footprint { neutral: bool },
    /// A prop, the rubble or the Seat flag: no fallback; `neutral` (props,
    /// rubble) takes no team.
    Decor { neutral: bool },
}

impl Visuals {
    /// Validate against the loaded unit and building kinds: every kind has
    /// a visual and every key names a kind; every `Body` and `Scene` is well
    /// formed (the Scene rules of `docs/DATA_FORMAT.md`: paths, scale, yaw,
    /// `nodes`, `parts`, `team` and `fallback`); buildings are boxes or
    /// Scenes; `sites`, `rubble`, `seat_flag`, `terrain` and
    /// `team_swatches` are in range; and two unit looks of the same shape
    /// differ by 1.4x on some axis. Errors name `path` and the field
    /// (`units["yeoman"]`, `buildings["town"].fallback` or
    /// `terrain["forest"].looks[0].weight`).
    pub fn validate(
        &self,
        path: &Path,
        units: &[UnitKind],
        buildings: &[BuildingKind],
    ) -> Result<(), Error> {
        for kind in units {
            if !self.units.contains_key(&kind.id) {
                return Err(Error::invalid(
                    path,
                    &format!("units[{:?}]", kind.id),
                    "missing: every unit kind in units.ron needs a visual",
                ));
            }
        }
        for (id, visual) in &self.units {
            let field = format!("units[{id:?}]");
            let Some(kind) = units.iter().find(|k| &k.id == id) else {
                return Err(Error::invalid(
                    path,
                    &field,
                    "no unit kind with this id in units.ron",
                ));
            };
            self.check_unit_look(path, &field, visual, kind)?;
        }
        for kind in buildings {
            if !self.buildings.contains_key(&kind.id) {
                return Err(Error::invalid(
                    path,
                    &format!("buildings[{:?}]", kind.id),
                    "missing: every building kind in buildings.ron needs a visual",
                ));
            }
        }
        for (id, visual) in &self.buildings {
            let field = format!("buildings[{id:?}]");
            if !buildings.iter().any(|k| &k.id == id) {
                return Err(Error::invalid(
                    path,
                    &field,
                    "no building kind with this id in buildings.ron",
                ));
            }
            self.check_footprint_look(path, &field, visual, false)?;
        }
        if self.sites.is_empty() || self.sites.len() > MAX_SITE_STAGES {
            return Err(Error::invalid(
                path,
                "sites",
                format!("must list 1..={MAX_SITE_STAGES} stages"),
            ));
        }
        for (i, visual) in self.sites.iter().enumerate() {
            self.check_footprint_look(path, &format!("sites[{i}]"), visual, true)?;
        }
        if let Some(rubble) = &self.rubble {
            if !(1..=MAX_RUBBLE_DS).contains(&rubble.ds) {
                return Err(Error::invalid(
                    path,
                    "rubble.ds",
                    format!("must be in 1..={MAX_RUBBLE_DS}"),
                ));
            }
            self.check_decor_scene(path, "rubble.visual", &rubble.visual, true)?;
        }
        if let Some(flag) = &self.seat_flag {
            self.check_decor_scene(path, "seat_flag", flag, false)?;
        }
        for (key, decor) in &self.terrain {
            self.check_terrain(path, key, decor)?;
        }
        for (key, swatch) in &self.team_swatches {
            check_swatch(path, key, swatch)?;
        }
        self.check_silhouettes(path, units)
    }

    /// A unit's look: a `Primitive`, a well-formed `Body`, or a `Scene`
    /// whose fallback is a `Primitive` or a `Body`.
    fn check_unit_look(
        &self,
        path: &Path,
        field: &str,
        visual: &Visual,
        kind: &UnitKind,
    ) -> Result<(), Error> {
        match visual {
            Visual::Primitive(_) => Ok(()),
            Visual::Body {
                shape,
                height_tiles_x100,
                lying,
            } => check_body(path, field, *shape, *height_tiles_x100, *lying, kind),
            Visual::Scene { fallback, .. } => {
                self.check_scene(path, field, visual, SceneUse::Unit)?;
                if let Some(fallback) = fallback {
                    let field = sub(field, "fallback");
                    if matches!(**fallback, Visual::Scene { .. }) {
                        return Err(Error::invalid(
                            path,
                            &field,
                            "a unit's fallback is a Primitive or a Body, never a Scene",
                        ));
                    }
                    self.check_unit_look(path, &field, fallback, kind)?;
                }
                Ok(())
            }
        }
    }

    /// A building's or a site's look: `Primitive(Cuboid)` or a `Scene`
    /// (neutral for a site) whose fallback is absent or `Primitive(Cuboid)`.
    fn check_footprint_look(
        &self,
        path: &Path,
        field: &str,
        visual: &Visual,
        site: bool,
    ) -> Result<(), Error> {
        match visual {
            Visual::Primitive(Primitive::Cuboid) => Ok(()),
            Visual::Scene { .. } => {
                self.check_scene(path, field, visual, SceneUse::Footprint { neutral: site })
            }
            _ => Err(Error::invalid(
                path,
                field,
                format!(
                    "a {} is Primitive(Cuboid) (the footprint box) or a Scene",
                    if site { "site" } else { "building" }
                ),
            )),
        }
    }

    /// A prop, the rubble or the Seat flag: a Scene without a fallback,
    /// neutral when `neutral`.
    fn check_decor_scene(
        &self,
        path: &Path,
        field: &str,
        visual: &Visual,
        neutral: bool,
    ) -> Result<(), Error> {
        if !matches!(visual, Visual::Scene { .. }) {
            return Err(Error::invalid(path, field, "must be a Scene"));
        }
        self.check_scene(path, field, visual, SceneUse::Decor { neutral })
    }

    /// The rules every Scene follows, plus its `team` and `fallback` for
    /// `usage`. A non-Scene `visual` passes.
    fn check_scene(
        &self,
        path: &Path,
        field: &str,
        visual: &Visual,
        usage: SceneUse,
    ) -> Result<(), Error> {
        let Visual::Scene {
            path: scene,
            scale_x100,
            yaw_deg,
            nodes,
            parts,
            team,
            fallback,
            ..
        } = visual
        else {
            return Ok(());
        };
        check_asset_path(scene).map_err(|m| Error::invalid(path, &sub(field, "path"), m))?;
        check_scale(path, &sub(field, "scale_x100"), *scale_x100)?;
        check_yaw(path, &sub(field, "yaw_deg"), *yaw_deg)?;
        check_nodes(path, &sub(field, "nodes"), nodes)?;
        if parts.len() > MAX_PARTS {
            return Err(Error::invalid(
                path,
                &sub(field, "parts"),
                format!("at most {MAX_PARTS} parts"),
            ));
        }
        for (i, part) in parts.iter().enumerate() {
            let field = sub(field, &format!("parts[{i}]"));
            check_asset_path(&part.path)
                .map_err(|m| Error::invalid(path, &sub(&field, "path"), m))?;
            check_nodes(path, &sub(&field, "nodes"), &part.nodes)?;
            check_scale(path, &sub(&field, "scale_x100"), part.scale_x100)?;
            check_yaw(path, &sub(&field, "yaw_deg"), part.yaw_deg)?;
        }
        if let Some(key) = team {
            let neutral = matches!(
                usage,
                SceneUse::Footprint { neutral: true } | SceneUse::Decor { neutral: true }
            );
            if neutral {
                return Err(Error::invalid(
                    path,
                    &sub(field, "team"),
                    "must be None: sites, rubble and props are neutral",
                ));
            }
            if !self.team_swatches.contains_key(key) {
                return Err(Error::invalid(
                    path,
                    &sub(field, "team"),
                    format!("{key:?} is not a key of team_swatches"),
                ));
            }
        }
        match (usage, fallback.as_deref()) {
            (_, None)
            | (SceneUse::Unit, Some(_))
            | (SceneUse::Footprint { .. }, Some(Visual::Primitive(Primitive::Cuboid))) => Ok(()),
            (SceneUse::Footprint { .. }, Some(_)) => Err(Error::invalid(
                path,
                &sub(field, "fallback"),
                "a building's or a site's fallback is Primitive(Cuboid) (the footprint box) or absent",
            )),
            (SceneUse::Decor { .. }, Some(_)) => Err(Error::invalid(
                path,
                &sub(field, "fallback"),
                "props, the rubble and the Seat flag are skipped when their model fails; give no fallback",
            )),
        }
    }

    /// One `terrain` entry: a dressable terrain, densities, jitters and
    /// 1..=8 neutral Scene looks with their weights.
    fn check_terrain(&self, path: &Path, key: &str, decor: &TerrainDecor) -> Result<(), Error> {
        let field = format!("terrain[{key:?}]");
        if key == "grass" {
            return Err(Error::invalid(
                path,
                &field,
                "props on buildable land would hide buildings; only forest, mountain and water take props",
            ));
        }
        if !DECOR_TERRAINS.contains(&key) {
            return Err(Error::invalid(
                path,
                &field,
                format!(
                    "not a terrain with props; one of {}",
                    DECOR_TERRAINS.join(", ")
                ),
            ));
        }
        for (name, value, max) in [
            ("interior_permille", decor.interior_permille, 1000),
            ("edge_permille", decor.edge_permille, 1000),
            ("jitter_x100", decor.jitter_x100, MAX_DECOR_JITTER_X100),
            (
                "scale_jitter_pct",
                decor.scale_jitter_pct,
                MAX_DECOR_SCALE_JITTER_PCT,
            ),
        ] {
            if value > max {
                return Err(Error::invalid(
                    path,
                    &sub(&field, name),
                    format!("must be at most {max}"),
                ));
            }
        }
        if decor.looks.is_empty() || decor.looks.len() > MAX_DECOR_LOOKS {
            return Err(Error::invalid(
                path,
                &sub(&field, "looks"),
                format!("must list 1..={MAX_DECOR_LOOKS} looks"),
            ));
        }
        for (i, look) in decor.looks.iter().enumerate() {
            let field = sub(&field, &format!("looks[{i}]"));
            if !(1..=MAX_DECOR_WEIGHT).contains(&look.weight) {
                return Err(Error::invalid(
                    path,
                    &sub(&field, "weight"),
                    format!("must be in 1..={MAX_DECOR_WEIGHT}"),
                ));
            }
            self.check_decor_scene(path, &sub(&field, "visual"), &look.visual, true)?;
        }
        Ok(())
    }

    /// The 1.4x rule (`docs/ART_STYLE.md`): two unit kinds whose looks are
    /// the same shape must differ by at least 1.4x in width, height or
    /// depth, so each kind keeps its own silhouette. Measured in integer
    /// maths on the nominal sizes of the `Primitive` and `Body` looks, a
    /// `Scene` standing in by its fallback; a Scene without one is left to
    /// the model check. The error names the later kind of the pair.
    fn check_silhouettes(&self, path: &Path, units: &[UnitKind]) -> Result<(), Error> {
        let looks: Vec<(&str, Primitive, [u64; 3])> = units
            .iter()
            .filter_map(|kind| {
                let visual = self.units.get(&kind.id)?;
                let (shape, size) = nominal_size_x1000(visual, kind.radius_tiles_x100)?;
                Some((kind.id.as_str(), shape, size))
            })
            .collect();
        for (i, (a, shape_a, size_a)) in looks.iter().enumerate() {
            for (b, shape_b, size_b) in &looks[i + 1..] {
                if shape_a != shape_b {
                    continue;
                }
                let apart = size_a.iter().zip(size_b).any(|(&x, &y)| {
                    let (lo, hi) = (x.min(y), x.max(y));
                    hi * SILHOUETTE_RATIO_DEN >= lo * SILHOUETTE_RATIO_NUM
                });
                if !apart {
                    let m = |s: &[u64; 3]| {
                        format!(
                            "{}x{}x{} cm",
                            s[0].div_ceil(10),
                            s[1].div_ceil(10),
                            s[2].div_ceil(10)
                        )
                    };
                    return Err(Error::invalid(
                        path,
                        &format!("units[{b:?}]"),
                        format!(
                            "the same shape ({shape_a:?}) as units[{a:?}] and less than 1.4x apart on every axis ({} vs {}); each kind needs its own silhouette (docs/ART_STYLE.md)",
                            m(size_b),
                            m(size_a)
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Every Scene path in the file with the field that names it
    /// (`buildings["town"].path`, `units["supply_wain"].parts[0].path`), in
    /// file order: units, buildings, sites, rubble, Seat flag, terrain.
    /// `sim-cli data-check` checks that each exists under the assets
    /// directory ([`Visuals::check_files`]).
    pub fn scene_paths(&self) -> Vec<(String, &str)> {
        let mut out = Vec::new();
        for (id, v) in &self.units {
            push_scene_paths(&mut out, &format!("units[{id:?}]"), v);
        }
        for (id, v) in &self.buildings {
            push_scene_paths(&mut out, &format!("buildings[{id:?}]"), v);
        }
        for (i, v) in self.sites.iter().enumerate() {
            push_scene_paths(&mut out, &format!("sites[{i}]"), v);
        }
        if let Some(rubble) = &self.rubble {
            push_scene_paths(&mut out, "rubble.visual", &rubble.visual);
        }
        if let Some(flag) = &self.seat_flag {
            push_scene_paths(&mut out, "seat_flag", flag);
        }
        for (key, decor) in &self.terrain {
            for (i, look) in decor.looks.iter().enumerate() {
                push_scene_paths(
                    &mut out,
                    &format!("terrain[{key:?}].looks[{i}].visual"),
                    &look.visual,
                );
            }
        }
        out
    }

    /// Check that every Scene path ([`Visuals::scene_paths`]) is a file
    /// under `assets`, and return how many distinct files the Scenes name.
    /// The error names `path` (the `visuals.ron` file), the field and the
    /// missing path.
    pub fn check_files(&self, path: &Path, assets: &Path) -> Result<usize, Error> {
        let mut files = BTreeSet::new();
        for (field, scene) in self.scene_paths() {
            if !assets.join(scene).is_file() {
                return Err(Error::invalid(
                    path,
                    &field,
                    format!("{scene:?} is not a file under {}", assets.display()),
                ));
            }
            files.insert(scene);
        }
        Ok(files.len())
    }

    /// The visual for a unit kind id, if listed.
    pub fn get(&self, id: &str) -> Option<&Visual> {
        self.units.get(id)
    }

    /// The visual for a building kind id, if listed.
    pub fn building(&self, id: &str) -> Option<&Visual> {
        self.buildings.get(id)
    }
}

/// Append the Scene paths of `visual` (its own, its parts', its
/// fallback's) under `field`.
fn push_scene_paths<'a>(out: &mut Vec<(String, &'a str)>, field: &str, visual: &'a Visual) {
    if let Visual::Scene {
        path,
        parts,
        fallback,
        ..
    } = visual
    {
        out.push((sub(field, "path"), path.as_str()));
        for (i, part) in parts.iter().enumerate() {
            out.push((sub(field, &format!("parts[{i}].path")), part.path.as_str()));
        }
        if let Some(fallback) = fallback {
            push_scene_paths(out, &sub(field, "fallback"), fallback);
        }
    }
}

/// The shape and nominal size `(x, y, z)` in thousandths of a tile of a
/// unit look for a kind of radius `radius_x100` (hundredths): a
/// `Primitive` is two radii wide and deep, a Capsule 3.4 radii tall and the
/// rest two; a `Body` is two radii wide and its own height tall, or that
/// long along +X and two radii tall when lying; a `Scene` measures its
/// fallback; `None` for a Scene without one.
fn nominal_size_x1000(visual: &Visual, radius_x100: u32) -> Option<(Primitive, [u64; 3])> {
    let r = u64::from(radius_x100);
    let diameter = 20 * r;
    match visual {
        Visual::Primitive(shape) => {
            let tall = if *shape == Primitive::Capsule {
                34 * r
            } else {
                diameter
            };
            Some((*shape, [diameter, tall, diameter]))
        }
        Visual::Body {
            shape,
            height_tiles_x100,
            lying,
        } => {
            let length = if *shape == Primitive::Sphere {
                diameter
            } else {
                10 * u64::from(*height_tiles_x100)
            };
            Some((
                *shape,
                if *lying {
                    [length, diameter, diameter]
                } else {
                    [diameter, length, diameter]
                },
            ))
        }
        Visual::Scene { fallback, .. } => nominal_size_x1000(fallback.as_deref()?, radius_x100),
    }
}

/// A unit's `Body`: not a sphere, only a Capsule or a Cylinder lies, the
/// height in range and a Capsule at least two radii.
fn check_body(
    path: &Path,
    field: &str,
    shape: Primitive,
    height_tiles_x100: u32,
    lying: bool,
    kind: &UnitKind,
) -> Result<(), Error> {
    if shape == Primitive::Sphere {
        return Err(Error::invalid(
            path,
            &sub(field, "shape"),
            "a Body cannot be a Sphere (it has no height of its own); use Primitive(Sphere)",
        ));
    }
    if lying && !matches!(shape, Primitive::Capsule | Primitive::Cylinder) {
        return Err(Error::invalid(
            path,
            &sub(field, "lying"),
            format!("only a Capsule or a Cylinder can lie, not a {shape:?}"),
        ));
    }
    if !(MIN_BODY_HEIGHT_X100..=MAX_BODY_HEIGHT_X100).contains(&height_tiles_x100) {
        return Err(Error::invalid(
            path,
            &sub(field, "height_tiles_x100"),
            format!("must be in {MIN_BODY_HEIGHT_X100}..={MAX_BODY_HEIGHT_X100}"),
        ));
    }
    let diameter = kind.radius_tiles_x100.saturating_mul(2);
    if shape == Primitive::Capsule && height_tiles_x100 < diameter {
        return Err(Error::invalid(
            path,
            &sub(field, "height_tiles_x100"),
            format!("a Capsule must be at least twice the kind's radius_tiles_x100 ({diameter})"),
        ));
    }
    Ok(())
}

fn check_scale(path: &Path, field: &str, scale_x100: u32) -> Result<(), Error> {
    if scale_x100 == 0 || scale_x100 > MAX_SCALE_X100 {
        return Err(Error::invalid(
            path,
            field,
            format!("must be in 1..={MAX_SCALE_X100}"),
        ));
    }
    Ok(())
}

fn check_yaw(path: &Path, field: &str, yaw_deg: i32) -> Result<(), Error> {
    if !(-360..=360).contains(&yaw_deg) {
        return Err(Error::invalid(path, field, "must be within -360..=360"));
    }
    Ok(())
}

fn check_nodes(path: &Path, field: &str, nodes: &[String]) -> Result<(), Error> {
    if nodes.len() > MAX_NODE_GLOBS {
        return Err(Error::invalid(
            path,
            field,
            format!("at most {MAX_NODE_GLOBS} globs"),
        ));
    }
    if let Some(i) = nodes.iter().position(|g| g.trim().is_empty()) {
        return Err(Error::invalid(
            path,
            &format!("{field}[{i}]"),
            "must not be empty",
        ));
    }
    Ok(())
}

/// One `team_swatches` entry: at least one rect, sides in range and inside
/// a [`MAX_TEXTURE_SIDE`] texture, and `desaturate_permille` at most 1000.
fn check_swatch(path: &Path, key: &str, swatch: &TeamSwatch) -> Result<(), Error> {
    let field = format!("team_swatches[{key:?}]");
    if swatch.rects.is_empty() {
        return Err(Error::invalid(
            path,
            &sub(&field, "rects"),
            "must list at least one texel rect",
        ));
    }
    for (i, &(x, y, w, h)) in swatch.rects.iter().enumerate() {
        let inside = |at: u32, side: u32| {
            (1..=MAX_TEXTURE_SIDE).contains(&side)
                && at
                    .checked_add(side)
                    .is_some_and(|end| end <= MAX_TEXTURE_SIDE)
        };
        if !inside(x, w) || !inside(y, h) {
            return Err(Error::invalid(
                path,
                &sub(&field, &format!("rects[{i}]")),
                format!(
                    "({x}, {y}, {w}, {h}): sides must be in 1..={MAX_TEXTURE_SIDE} and the rect inside a {MAX_TEXTURE_SIDE} x {MAX_TEXTURE_SIDE} texture"
                ),
            ));
        }
    }
    if swatch.desaturate_permille > 1000 {
        return Err(Error::invalid(
            path,
            &sub(&field, "desaturate_permille"),
            "must be at most 1000",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds() -> Vec<UnitKind> {
        vec![UnitKind {
            id: "yeoman".into(),
            name: "Yeoman".into(),
            speed_tiles_per_s_x100: 180,
            radius_tiles_x100: 35,
            arrive_radius_tiles_x100: 25,
            arrive_slowdown_radius_tiles_x100: 50,
            waypoint_radius_tiles_x100: 50,
            separation_tiles_x100: 10,
            return_to_post_radius_tiles_x100: 100,
            class: crate::UnitClass::Civilian,
            hp: 40,
            attack: None,
            cost: BTreeMap::new(),
            ramp: crate::UnitRamp::Flat,
            pop: 1,
            train_time_ds: 120,
            trainer: "town".into(),
            requires_tech: None,
            builder: true,
            vision_tiles: 8,
            immune_to_attrition: false,
            supplies: false,
        }]
    }

    fn parse(text: &str) -> Result<Visuals, Error> {
        crate::parse_ron::<Visuals>(Path::new("v.ron"), text)
    }

    #[test]
    fn primitive_parses_and_validates() {
        let v = parse(r#"(buildings: {}, units: { "yeoman": Primitive(Capsule) })"#).unwrap();
        v.validate(Path::new("v.ron"), &kinds(), &[]).unwrap();
        assert_eq!(
            v.get("yeoman"),
            Some(&Visual::Primitive(Primitive::Capsule))
        );
        assert!(v.get("scribe").is_none());
    }

    #[test]
    fn scene_parses_and_validates() {
        let v = parse(
            r#"(buildings: {}, units: { "yeoman": Scene(path: "models/yeoman.glb", scale_x100: 100, y_offset_tiles_x100: -5, yaw_deg: 90) })"#,
        )
        .unwrap();
        v.validate(Path::new("v.ron"), &kinds(), &[]).unwrap();
    }

    #[test]
    fn missing_kind_names_file_and_field() {
        let v = parse("(buildings: {}, units: {})").unwrap();
        let err = v
            .validate(Path::new("v.ron"), &kinds(), &[])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("v.ron") && err.contains(r#"units["yeoman"]"#),
            "{err}"
        );
    }

    #[test]
    fn unknown_kind_is_rejected() {
        let v = parse(r#"(buildings: {}, units: { "yeoman": Primitive(Capsule), "dragon": Primitive(Sphere) })"#)
            .unwrap();
        let err = v
            .validate(Path::new("v.ron"), &kinds(), &[])
            .unwrap_err()
            .to_string();
        assert!(err.contains(r#"units["dragon"]"#), "{err}");
    }

    #[test]
    fn bad_scene_fields_are_rejected_with_field() {
        let bad = [
            (
                r#"(buildings: {}, units: { "yeoman": Scene(path: "models/yeoman.obj", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0) })"#,
                "path",
            ),
            (
                r#"(buildings: {}, units: { "yeoman": Scene(path: "a.glb", scale_x100: 0, y_offset_tiles_x100: 0, yaw_deg: 0) })"#,
                "scale_x100",
            ),
            (
                r#"(buildings: {}, units: { "yeoman": Scene(path: "a.glb", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 400) })"#,
                "yaw_deg",
            ),
        ];
        for (text, field) in bad {
            let v = parse(text).unwrap();
            let err = v
                .validate(Path::new("v.ron"), &kinds(), &[])
                .unwrap_err()
                .to_string();
            assert!(
                err.contains(&format!(r#"units["yeoman"].{field}"#)),
                "{err}"
            );
        }
    }

    #[test]
    fn scene_rejects_unknown_fields() {
        let err = parse(
            r#"(buildings: {}, units: { "yeoman": Scene(path: "a.glb", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0, tint: 3) })"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("tint"), "{err}");
    }

    #[test]
    fn unknown_field_and_float_are_parse_errors() {
        assert!(
            parse(r#"(buildings: {}, units: { "yeoman": Primitive(Capsule) }, extra: 1)"#).is_err()
        );
        assert!(
            parse(
                r#"(buildings: {}, units: { "yeoman": Scene(path: "a.glb", scale_x100: 1.5, y_offset_tiles_x100: 0, yaw_deg: 0) })"#
            )
            .is_err()
        );
    }

    #[test]
    fn body_parses_and_validates() {
        let ok = [
            (
                r#"(buildings: {}, units: { "yeoman": Body(shape: Cone, height_tiles_x100: 150) })"#,
                Visual::Body {
                    shape: Primitive::Cone,
                    height_tiles_x100: 150,
                    lying: false,
                },
            ),
            (
                r#"(buildings: {}, units: { "yeoman": Body(shape: Capsule, height_tiles_x100: 180, lying: true) })"#,
                Visual::Body {
                    shape: Primitive::Capsule,
                    height_tiles_x100: 180,
                    lying: true,
                },
            ),
            (
                r#"(buildings: {}, units: { "yeoman": Body(shape: Cylinder, height_tiles_x100: 20, lying: true) })"#,
                Visual::Body {
                    shape: Primitive::Cylinder,
                    height_tiles_x100: MIN_BODY_HEIGHT_X100,
                    lying: true,
                },
            ),
            (
                // A Capsule exactly two radii long (35 x 2) is the shortest one.
                r#"(buildings: {}, units: { "yeoman": Body(shape: Capsule, height_tiles_x100: 70) })"#,
                Visual::Body {
                    shape: Primitive::Capsule,
                    height_tiles_x100: 70,
                    lying: false,
                },
            ),
            (
                r#"(buildings: {}, units: { "yeoman": Body(shape: Cuboid, height_tiles_x100: 400, lying: false) })"#,
                Visual::Body {
                    shape: Primitive::Cuboid,
                    height_tiles_x100: MAX_BODY_HEIGHT_X100,
                    lying: false,
                },
            ),
            (
                r#"(buildings: {}, units: { "yeoman": Primitive(Cone) })"#,
                Visual::Primitive(Primitive::Cone),
            ),
        ];
        for (text, want) in ok {
            let v = parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            v.validate(Path::new("v.ron"), &kinds(), &[])
                .unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(v.get("yeoman"), Some(&want), "{text}");
        }
    }

    #[test]
    fn bad_body_fields_are_rejected_with_field() {
        let bad = [
            (
                r#"(buildings: {}, units: { "yeoman": Body(shape: Sphere, height_tiles_x100: 100) })"#,
                "shape",
            ),
            (
                r#"(buildings: {}, units: { "yeoman": Body(shape: Cone, height_tiles_x100: 150, lying: true) })"#,
                "lying",
            ),
            (
                r#"(buildings: {}, units: { "yeoman": Body(shape: Cuboid, height_tiles_x100: 60, lying: true) })"#,
                "lying",
            ),
            (
                r#"(buildings: {}, units: { "yeoman": Body(shape: Cuboid, height_tiles_x100: 19) })"#,
                "height_tiles_x100",
            ),
            (
                r#"(buildings: {}, units: { "yeoman": Body(shape: Cylinder, height_tiles_x100: 401) })"#,
                "height_tiles_x100",
            ),
            (
                // Shorter than its two half-spheres (radius 35).
                r#"(buildings: {}, units: { "yeoman": Body(shape: Capsule, height_tiles_x100: 69, lying: true) })"#,
                "height_tiles_x100",
            ),
        ];
        for (text, field) in bad {
            let v = parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            let err = v
                .validate(Path::new("v.ron"), &kinds(), &[])
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("v.ron") && err.contains(&format!(r#"units["yeoman"].{field}"#)),
                "{text}: {err}"
            );
        }
    }

    #[test]
    fn unknown_body_shape_is_a_parse_error_with_line_and_column() {
        let err =
            parse("(buildings: {}, units: {\n    \"yeoman\": Body(shape: Pyramid, height_tiles_x100: 100),\n})")
                .unwrap_err()
                .to_string();
        // ron 0.12 names the span of the variant: line 2, columns 27-34.
        assert!(err.starts_with("v.ron: 2:27"), "{err}");
        assert!(err.contains("Pyramid") && err.contains("Cone"), "{err}");
    }

    #[test]
    fn body_rejects_unknown_and_missing_fields_and_floats() {
        for text in [
            r#"(buildings: {}, units: { "yeoman": Body(shape: Cone, height_tiles_x100: 150, width_tiles_x100: 50) })"#,
            r#"(buildings: {}, units: { "yeoman": Body(shape: Cone) })"#,
            r#"(buildings: {}, units: { "yeoman": Body(shape: Cone, height_tiles_x100: 1.5) })"#,
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }

    /// A building kind `id` with a 2x2 footprint.
    fn building(id: &str) -> BuildingKind {
        BuildingKind {
            id: id.into(),
            name: id.into(),
            town: false,
            width_tiles: 2,
            height_tiles: 2,
            cost: BTreeMap::new(),
            build_time_ds: 100,
            hp: 100,
            vision_tiles: 4,
            base_yield: BTreeMap::new(),
            gather: None,
            max_per_town: None,
            border_radius_tiles: None,
            attack: None,
            stays_on_annexation: false,
        }
    }

    fn town() -> Vec<BuildingKind> {
        vec![building("town")]
    }

    /// The unit kind `id` of radius `radius_x100`, otherwise the Yeoman.
    fn unit(id: &str, radius_x100: u32) -> UnitKind {
        UnitKind {
            id: id.into(),
            radius_tiles_x100: radius_x100,
            ..kinds().remove(0)
        }
    }

    /// `units` and `town` entries plus `rest` (more top-level fields), as
    /// a visuals.ron text.
    fn file(units: &str, town: &str, rest: &str) -> String {
        format!(r#"(units: {{ {units} }}, buildings: {{ "town": {town} }}, {rest})"#)
    }

    /// Parse and validate `text` against the Yeoman and the Town.
    fn check(text: &str) -> Result<(), String> {
        let v = parse(text).map_err(|e| format!("parse: {e}"))?;
        v.validate(Path::new("v.ron"), &kinds(), &town())
            .map_err(|e| e.to_string())
    }

    /// `text` parses but fails validation naming `v.ron` and `field`.
    fn rejects(text: &str, field: &str) -> String {
        let err = check(text).expect_err(text);
        assert!(!err.starts_with("parse"), "{text}: {err}");
        assert!(
            err.starts_with(&format!("v.ron: {field}: ")),
            "{text}: wanted the field {field}, got {err}"
        );
        err
    }

    const YEOMAN: &str = r#""yeoman": Primitive(Capsule)"#;
    const CUBOID: &str = "Primitive(Cuboid)";
    const SWATCH: &str =
        r#"team_swatches: { "hex": (rects: [(0, 0, 8, 8)], desaturate_permille: 200) }"#;

    /// The shape of art-pass section 4.1: every new field and table parses
    /// and validates, and the accessors find the entries.
    #[test]
    fn the_art_pass_schema_parses_and_validates() {
        let text = r#"(
            team_swatches: {
                "hex": (rects: [(0, 0, 8, 8), (8, 8, 4, 2)], desaturate_permille: 200),
                "adv": (rects: [(0, 0, 4096, 4096)], desaturate_permille: 250),
            },
            units: {
                "yeoman": Scene(path: "models/a/yeoman.glb", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0, team: Some("hex"), fallback: Some(Body(shape: Capsule, height_tiles_x100: 120))),
            },
            buildings: {
                "town": Scene(path: "models/hex/castle.gltf", nodes: ["*castle*", "base"], parts: [(path: "models/hex/flag.gltf", offset_x100: (-45, 40, 0), scale_x100: 140), (path: "models/hex/sack.gltf")], scale_x100: 150, y_offset_tiles_x100: 1, yaw_deg: 0, team: Some("hex"), fallback: Some(Primitive(Cuboid))),
            },
            sites: [Scene(path: "models/hex/stage_A.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0), Primitive(Cuboid)],
            rubble: Some((visual: Scene(path: "models/hex/destroyed.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0), ds: 300)),
            seat_flag: Some(Scene(path: "models/hex/flag.gltf", scale_x100: 120, y_offset_tiles_x100: 0, yaw_deg: 0, team: Some("hex"))),
            terrain: {
                "forest": (interior_permille: 1000, edge_permille: 600, jitter_x100: 30, scale_jitter_pct: 10,
                           looks: [(visual: Scene(path: "models/hex/trees.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0), at: Interior, weight: 2)]),
                "water": (interior_permille: 60, edge_permille: 60, jitter_x100: 0, scale_jitter_pct: 0,
                          looks: [(visual: Scene(path: "models/hex/lily.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0), at: Any, weight: 1, shadow: false)]),
            },
        )"#;
        let v = parse(text).unwrap();
        v.validate(Path::new("v.ron"), &kinds(), &town()).unwrap();
        let Some(Visual::Scene {
            nodes,
            parts,
            team,
            fallback,
            ..
        }) = v.building("town")
        else {
            panic!("the town is a Scene");
        };
        assert_eq!(nodes, &["*castle*", "base"]);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].offset_x100, (-45, 40, 0));
        assert_eq!((parts[0].scale_x100, parts[0].yaw_deg), (140, 0));
        assert_eq!(
            (parts[1].offset_x100, parts[1].scale_x100),
            ((0, 0, 0), 100),
            "part defaults"
        );
        assert_eq!(team.as_deref(), Some("hex"));
        assert_eq!(
            fallback.as_deref(),
            Some(&Visual::Primitive(Primitive::Cuboid))
        );
        assert_eq!(v.sites.len(), 2);
        assert_eq!(v.rubble.as_ref().map(|r| r.ds), Some(300));
        let water = &v.terrain["water"].looks[0];
        assert_eq!(
            (water.at, water.weight, water.shadow),
            (DecorAt::Any, 1, false)
        );
        assert!(
            v.terrain["forest"].looks[0].shadow,
            "shadow defaults to true"
        );
        // The optional tables default: one box site, no rubble, flag,
        // terrain or swatches.
        let bare = parse(&file(YEOMAN, CUBOID, "")).unwrap();
        assert_eq!(bare.sites, vec![Visual::Primitive(Primitive::Cuboid)]);
        assert_eq!(bare.sites, Visuals::default().sites);
        assert!(bare.rubble.is_none() && bare.seat_flag.is_none());
        assert!(bare.terrain.is_empty() && bare.team_swatches.is_empty());
        bare.validate(Path::new("v.ron"), &kinds(), &town())
            .unwrap();
        // `buildings` is required.
        assert!(parse(r#"(units: { "yeoman": Primitive(Capsule) })"#).is_err());
    }

    #[test]
    fn visuals_need_every_building_kind() {
        let v = parse(&file(YEOMAN, CUBOID, "")).unwrap();
        let err = v
            .validate(
                Path::new("v.ron"),
                &kinds(),
                &[building("town"), building("croft")],
            )
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with(r#"v.ron: buildings["croft"]: missing"#),
            "{err}"
        );
        assert!(err.contains("every building kind"), "{err}");
    }

    #[test]
    fn unknown_building_key_is_rejected() {
        let text = format!(
            r#"(units: {{ {YEOMAN} }}, buildings: {{ "town": {CUBOID}, "castle": {CUBOID} }})"#
        );
        let err = rejects(&text, r#"buildings["castle"]"#);
        assert!(err.contains("no building kind"), "{err}");
    }

    #[test]
    fn building_visual_is_a_cuboid_or_a_scene() {
        for town in [
            "Primitive(Capsule)",
            "Primitive(Sphere)",
            "Body(shape: Cuboid, height_tiles_x100: 100)",
        ] {
            let err = rejects(&file(YEOMAN, town, ""), r#"buildings["town"]"#);
            assert!(err.contains("Primitive(Cuboid)"), "{err}");
        }
        // A Scene's fallback is absent or the box.
        check(&file(
            YEOMAN,
            r#"Scene(path: "m/castle.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0)"#,
            "",
        ))
        .unwrap();
        check(&file(
            YEOMAN,
            r#"Scene(path: "m/castle.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0, fallback: Some(Primitive(Cuboid)))"#,
            "",
        ))
        .unwrap();
        for fallback in [
            "Primitive(Cone)",
            "Body(shape: Cuboid, height_tiles_x100: 100)",
            r#"Scene(path: "m/b.glb", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0)"#,
        ] {
            rejects(
                &file(
                    YEOMAN,
                    &format!(
                        r#"Scene(path: "m/castle.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0, fallback: Some({fallback}))"#
                    ),
                    "",
                ),
                r#"buildings["town"].fallback"#,
            );
        }
        // A building Scene follows every Scene rule, under its own field.
        rejects(
            &file(
                YEOMAN,
                r#"Scene(path: "m/castle.gltf", scale_x100: 0, y_offset_tiles_x100: 0, yaw_deg: 0)"#,
                "",
            ),
            r#"buildings["town"].scale_x100"#,
        );
    }

    #[test]
    fn scene_fallback_is_a_primitive_or_body() {
        let unit = |fallback: &str| {
            format!(
                r#""yeoman": Scene(path: "m/y.glb", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0, fallback: Some({fallback}))"#
            )
        };
        for ok in [
            "Primitive(Capsule)",
            "Primitive(Sphere)",
            "Body(shape: Capsule, height_tiles_x100: 180, lying: true)",
        ] {
            check(&file(&unit(ok), CUBOID, "")).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        let err = rejects(
            &file(
                &unit(
                    r#"Scene(path: "m/z.glb", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0)"#,
                ),
                CUBOID,
                "",
            ),
            r#"units["yeoman"].fallback"#,
        );
        assert!(err.contains("never a Scene"), "{err}");
        // The fallback Body follows the Body rules, under its own field.
        rejects(
            &file(
                &unit("Body(shape: Cone, height_tiles_x100: 500)"),
                CUBOID,
                "",
            ),
            r#"units["yeoman"].fallback.height_tiles_x100"#,
        );
        rejects(
            &file(
                &unit("Body(shape: Sphere, height_tiles_x100: 100)"),
                CUBOID,
                "",
            ),
            r#"units["yeoman"].fallback.shape"#,
        );
    }

    #[test]
    fn scene_paths_stay_inside_assets() {
        let scene = |path: &str| {
            format!(
                r#""yeoman": Scene(path: {path:?}, scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0)"#
            )
        };
        for ok in ["models/a.glb", "models/kaykit/b.gltf", "x.glb"] {
            check(&file(&scene(ok), CUBOID, "")).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for (bad, why) in [
            ("../secret.glb", "no .."),
            ("models/../../x.glb", "no .."),
            ("/abs/x.glb", "leading /"),
            ("\\abs\\x.glb", "leading /"),
            ("models/x.obj", ".glb or .gltf"),
            ("", ".glb or .gltf"),
        ] {
            let err = rejects(&file(&scene(bad), CUBOID, ""), r#"units["yeoman"].path"#);
            assert!(err.contains(why), "{bad}: {err}");
        }
        // Parts follow the same rules and have at most 8 entries; nodes at
        // most 16 non-empty globs.
        let with = |extra: &str| {
            format!(
                r#""yeoman": Scene(path: "m/y.glb", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0, {extra})"#
            )
        };
        rejects(
            &file(
                &with(r#"parts: [(path: "m/ok.glb"), (path: "../x.glb")]"#),
                CUBOID,
                "",
            ),
            r#"units["yeoman"].parts[1].path"#,
        );
        rejects(
            &file(
                &with(r#"parts: [(path: "m/ok.glb", scale_x100: 0)]"#),
                CUBOID,
                "",
            ),
            r#"units["yeoman"].parts[0].scale_x100"#,
        );
        rejects(
            &file(
                &with(r#"parts: [(path: "m/ok.glb", yaw_deg: 361)]"#),
                CUBOID,
                "",
            ),
            r#"units["yeoman"].parts[0].yaw_deg"#,
        );
        rejects(
            &file(
                &with(r#"parts: [(path: "m/ok.glb", nodes: ["a", " "])]"#),
                CUBOID,
                "",
            ),
            r#"units["yeoman"].parts[0].nodes[1]"#,
        );
        let parts = |n: usize| format!("parts: [{}]", vec![r#"(path: "m/p.glb")"#; n].join(", "));
        check(&file(&with(&parts(MAX_PARTS)), CUBOID, "")).unwrap();
        rejects(
            &file(&with(&parts(MAX_PARTS + 1)), CUBOID, ""),
            r#"units["yeoman"].parts"#,
        );
        let nodes = |n: usize| format!("nodes: [{}]", vec![r#""n*""#; n].join(", "));
        check(&file(&with(&nodes(MAX_NODE_GLOBS)), CUBOID, "")).unwrap();
        rejects(
            &file(&with(&nodes(MAX_NODE_GLOBS + 1)), CUBOID, ""),
            r#"units["yeoman"].nodes"#,
        );
        rejects(
            &file(&with(r#"nodes: ["a", ""]"#), CUBOID, ""),
            r#"units["yeoman"].nodes[1]"#,
        );
    }

    #[test]
    fn team_names_a_swatch_and_rects_are_sane() {
        let unit = |team: &str| {
            format!(
                r#""yeoman": Scene(path: "m/y.glb", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0, team: Some({team:?}))"#
            )
        };
        check(&file(&unit("hex"), CUBOID, SWATCH)).unwrap();
        let err = rejects(
            &file(&unit("adv"), CUBOID, SWATCH),
            r#"units["yeoman"].team"#,
        );
        assert!(err.contains("not a key of team_swatches"), "{err}");
        let swatch = |rects: &str, desaturate: u32| {
            format!(
                r#"team_swatches: {{ "hex": (rects: [{rects}], desaturate_permille: {desaturate}) }}"#
            )
        };
        check(&file(YEOMAN, CUBOID, &swatch("(4095, 0, 1, 4096)", 1000))).unwrap();
        for (rects, desaturate, field) in [
            ("", 0, r#"team_swatches["hex"].rects"#),
            ("(0, 0, 0, 8)", 0, r#"team_swatches["hex"].rects[0]"#),
            (
                "(0, 0, 8, 8), (0, 0, 8, 4097)",
                0,
                r#"team_swatches["hex"].rects[1]"#,
            ),
            ("(4090, 0, 8, 8)", 0, r#"team_swatches["hex"].rects[0]"#),
            (
                "(4294967295, 0, 8, 8)",
                0,
                r#"team_swatches["hex"].rects[0]"#,
            ),
            (
                "(0, 0, 8, 8)",
                1001,
                r#"team_swatches["hex"].desaturate_permille"#,
            ),
        ] {
            rejects(&file(YEOMAN, CUBOID, &swatch(rects, desaturate)), field);
        }
        // Sites, rubble and props are neutral; a building or the flag may
        // take a team.
        let scene_team = r#"Scene(path: "m/s.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0, team: Some("hex"))"#;
        check(&file(YEOMAN, scene_team, SWATCH)).unwrap();
        check(&file(
            YEOMAN,
            CUBOID,
            &format!("{SWATCH}, seat_flag: Some({scene_team})"),
        ))
        .unwrap();
        let err = rejects(
            &file(YEOMAN, CUBOID, &format!("{SWATCH}, sites: [{scene_team}]")),
            "sites[0].team",
        );
        assert!(err.contains("neutral"), "{err}");
        rejects(
            &file(
                YEOMAN,
                CUBOID,
                &format!("{SWATCH}, rubble: Some((visual: {scene_team}, ds: 300))"),
            ),
            "rubble.visual.team",
        );
    }

    #[test]
    fn sites_rubble_and_flag_ranges() {
        let scene =
            r#"Scene(path: "m/s.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0)"#;
        let sites = |n: usize| format!("sites: [{}]", vec![scene; n].join(", "));
        for n in 1..=MAX_SITE_STAGES {
            check(&file(YEOMAN, CUBOID, &sites(n))).unwrap();
        }
        rejects(&file(YEOMAN, CUBOID, &sites(0)), "sites");
        rejects(&file(YEOMAN, CUBOID, &sites(MAX_SITE_STAGES + 1)), "sites");
        rejects(
            &file(YEOMAN, CUBOID, "sites: [Primitive(Capsule)]"),
            "sites[0]",
        );
        rejects(
            &file(
                YEOMAN,
                CUBOID,
                &format!("sites: [{CUBOID}, {}]", scene.replace("100,", "0,")),
            ),
            "sites[1].scale_x100",
        );
        let rubble = |visual: &str, ds: u32| format!("rubble: Some((visual: {visual}, ds: {ds}))");
        check(&file(YEOMAN, CUBOID, &rubble(scene, 1))).unwrap();
        check(&file(YEOMAN, CUBOID, &rubble(scene, MAX_RUBBLE_DS))).unwrap();
        rejects(&file(YEOMAN, CUBOID, &rubble(scene, 0)), "rubble.ds");
        rejects(
            &file(YEOMAN, CUBOID, &rubble(scene, MAX_RUBBLE_DS + 1)),
            "rubble.ds",
        );
        rejects(&file(YEOMAN, CUBOID, &rubble(CUBOID, 300)), "rubble.visual");
        let with_fallback = scene.replace(")", ", fallback: Some(Primitive(Cuboid)))");
        let err = rejects(
            &file(YEOMAN, CUBOID, &rubble(&with_fallback, 300)),
            "rubble.visual.fallback",
        );
        assert!(err.contains("skipped"), "{err}");
        check(&file(YEOMAN, CUBOID, &format!("seat_flag: Some({scene})"))).unwrap();
        let err = rejects(
            &file(YEOMAN, CUBOID, &format!("seat_flag: Some({CUBOID})")),
            "seat_flag",
        );
        assert!(err.contains("must be a Scene"), "{err}");
        rejects(
            &file(YEOMAN, CUBOID, &format!("seat_flag: Some({with_fallback})")),
            "seat_flag.fallback",
        );
    }

    #[test]
    fn terrain_rejects_grass_and_bad_densities() {
        let look = r#"(visual: Scene(path: "m/tree.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0), at: Edge, weight: 1)"#;
        let decor = |key: &str, numbers: [u32; 4], looks: &str| {
            format!(
                r#"terrain: {{ {key:?}: (interior_permille: {}, edge_permille: {}, jitter_x100: {}, scale_jitter_pct: {}, looks: [{looks}]) }}"#,
                numbers[0], numbers[1], numbers[2], numbers[3]
            )
        };
        let ok = [
            1000,
            1000,
            MAX_DECOR_JITTER_X100,
            MAX_DECOR_SCALE_JITTER_PCT,
        ];
        for key in DECOR_TERRAINS {
            check(&file(YEOMAN, CUBOID, &decor(key, ok, look))).unwrap();
        }
        let err = rejects(
            &file(YEOMAN, CUBOID, &decor("grass", ok, look)),
            r#"terrain["grass"]"#,
        );
        assert!(
            err.contains("props on buildable land would hide buildings"),
            "{err}"
        );
        let err = rejects(
            &file(YEOMAN, CUBOID, &decor("swamp", ok, look)),
            r#"terrain["swamp"]"#,
        );
        assert!(err.contains("forest, mountain, water"), "{err}");
        for (i, name) in [
            "interior_permille",
            "edge_permille",
            "jitter_x100",
            "scale_jitter_pct",
        ]
        .into_iter()
        .enumerate()
        {
            let mut numbers = ok;
            numbers[i] += 1;
            rejects(
                &file(YEOMAN, CUBOID, &decor("forest", numbers, look)),
                &format!(r#"terrain["forest"].{name}"#),
            );
        }
        rejects(
            &file(YEOMAN, CUBOID, &decor("forest", ok, "")),
            r#"terrain["forest"].looks"#,
        );
        let looks = |n: usize| vec![look; n].join(", ");
        check(&file(
            YEOMAN,
            CUBOID,
            &decor("forest", ok, &looks(MAX_DECOR_LOOKS)),
        ))
        .unwrap();
        rejects(
            &file(
                YEOMAN,
                CUBOID,
                &decor("forest", ok, &looks(MAX_DECOR_LOOKS + 1)),
            ),
            r#"terrain["forest"].looks"#,
        );
        for weight in [0, MAX_DECOR_WEIGHT + 1] {
            rejects(
                &file(
                    YEOMAN,
                    CUBOID,
                    &decor(
                        "forest",
                        ok,
                        &look.replace("weight: 1", &format!("weight: {weight}")),
                    ),
                ),
                r#"terrain["forest"].looks[0].weight"#,
            );
        }
        rejects(
            &file(
                YEOMAN,
                CUBOID,
                &decor(
                    "forest",
                    ok,
                    r#"(visual: Primitive(Cone), at: Any, weight: 1)"#,
                ),
            ),
            r#"terrain["forest"].looks[0].visual"#,
        );
        rejects(
            &file(
                YEOMAN,
                CUBOID,
                &format!(
                    "{SWATCH}, {}",
                    decor(
                        "forest",
                        ok,
                        &look.replace("yaw_deg: 0)", r#"yaw_deg: 0, team: Some("hex"))"#)
                    )
                ),
            ),
            r#"terrain["forest"].looks[0].visual.team"#,
        );
        assert!(
            parse(&file(
                YEOMAN,
                CUBOID,
                &decor("forest", ok, &look.replace("Edge", "Coast"))
            ))
            .is_err(),
            "at is Interior, Edge or Any"
        );
    }

    /// The 1.4x rule over the live `data/visuals.ron`, and a clash: two
    /// Cylinders (the Scribe's 0.7 m drum and a Bowman post) pass at
    /// exactly 1.4x apart and fail just under it, naming the later kind.
    #[test]
    fn same_shape_looks_differ_by_1_4x_in_the_live_file() {
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let rules = crate::Rules::load(&data).expect("the repository data loads");
        let path = data.join("visuals.ron");
        rules
            .visuals
            .validate(&path, &rules.units, &rules.buildings)
            .unwrap();
        let units = [unit("scribe", 35), unit("bowman", 35)];
        let clash = |bowman: &str| {
            parse(&format!(
                r#"(units: {{ "scribe": Primitive(Cylinder), "bowman": {bowman} }}, buildings: {{}})"#
            ))
            .unwrap()
            .validate(Path::new("v.ron"), &units, &[])
        };
        // 0.7 m wide, 0.7 m tall against 0.98 m tall: exactly 1.4x.
        clash("Body(shape: Cylinder, height_tiles_x100: 98)").unwrap();
        let err = clash("Body(shape: Cylinder, height_tiles_x100: 97)")
            .unwrap_err()
            .to_string();
        assert!(err.starts_with(r#"v.ron: units["bowman"]: "#), "{err}");
        assert!(
            err.contains(r#"same shape (Cylinder) as units["scribe"]"#) && err.contains("1.4x"),
            "{err}"
        );
        // Lying, the length counts along the facing (x): still too close.
        assert!(clash("Body(shape: Cylinder, height_tiles_x100: 97, lying: true)").is_err());
        // The same Body upright and lying are two silhouettes.
        parse(
            r#"(units: { "scribe": Body(shape: Cylinder, height_tiles_x100: 140), "bowman": Body(shape: Cylinder, height_tiles_x100: 140, lying: true) }, buildings: {})"#,
        )
        .unwrap()
        .validate(Path::new("v.ron"), &units, &[])
        .unwrap();
        // Another shape never clashes.
        clash("Body(shape: Cone, height_tiles_x100: 70)").unwrap();
        // A Scene stands in by its fallback, and without one is skipped.
        let scene = |fallback: &str| {
            format!(
                r#"Scene(path: "m/b.glb", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0{fallback})"#
            )
        };
        assert!(clash(&scene(", fallback: Some(Primitive(Cylinder))")).is_err());
        clash(&scene("")).unwrap();
        // A Capsule is 3.4 radii tall: a 35-radius capsule (1.19 m) against
        // a capsule Body 1.67 m tall is 1.403x apart; 1.66 m is not.
        let capsules = [unit("scribe", 35), unit("bowman", 35)];
        let capsule = |h: u32| {
            parse(&format!(
                r#"(units: {{ "scribe": Primitive(Capsule), "bowman": Body(shape: Capsule, height_tiles_x100: {h}) }}, buildings: {{}})"#
            ))
            .unwrap()
            .validate(Path::new("v.ron"), &capsules, &[])
        };
        capsule(167).unwrap();
        assert!(capsule(166).is_err());
    }

    #[test]
    fn scene_paths_lists_parts() {
        let v = parse(&file(
            r#""yeoman": Scene(path: "m/y.glb", parts: [(path: "m/p0.gltf"), (path: "m/p1.glb")], scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0, fallback: Some(Primitive(Capsule)))"#,
            r#"Scene(path: "m/castle.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0)"#,
            r#"sites: [Primitive(Cuboid), Scene(path: "m/stage.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0)],
               rubble: Some((visual: Scene(path: "m/rubble.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0), ds: 300)),
               seat_flag: Some(Scene(path: "m/flag.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0)),
               terrain: { "water": (interior_permille: 60, edge_permille: 60, jitter_x100: 0, scale_jitter_pct: 0,
                   looks: [(visual: Scene(path: "m/lily.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0), at: Any, weight: 1, shadow: false)]) }"#,
        ))
        .unwrap();
        v.validate(Path::new("v.ron"), &kinds(), &town()).unwrap();
        let paths: Vec<(String, &str)> = v.scene_paths();
        let want = [
            (r#"units["yeoman"].path"#, "m/y.glb"),
            (r#"units["yeoman"].parts[0].path"#, "m/p0.gltf"),
            (r#"units["yeoman"].parts[1].path"#, "m/p1.glb"),
            (r#"buildings["town"].path"#, "m/castle.gltf"),
            ("sites[1].path", "m/stage.gltf"),
            ("rubble.visual.path", "m/rubble.gltf"),
            ("seat_flag.path", "m/flag.gltf"),
            (r#"terrain["water"].looks[0].visual.path"#, "m/lily.gltf"),
        ];
        assert_eq!(
            paths
                .iter()
                .map(|(f, p)| (f.as_str(), *p))
                .collect::<Vec<_>>(),
            want
        );
        // Primitive-only files have none.
        let bare = parse(&file(YEOMAN, CUBOID, "")).unwrap();
        assert!(bare.scene_paths().is_empty());
    }

    /// `check_files` counts the distinct files and names the field and
    /// path of the first one missing under the assets directory.
    #[test]
    fn check_files_counts_models_and_names_a_missing_one() {
        let assets = std::env::temp_dir().join(format!("visuals-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&assets);
        std::fs::create_dir_all(assets.join("m")).unwrap();
        for f in ["m/y.glb", "m/castle.gltf"] {
            std::fs::write(assets.join(f), b"glTF").unwrap();
        }
        let v = parse(&file(
            r#""yeoman": Scene(path: "m/y.glb", parts: [(path: "m/castle.gltf")], scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0)"#,
            r#"Scene(path: "m/castle.gltf", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0)"#,
            "",
        ))
        .unwrap();
        assert_eq!(v.check_files(Path::new("v.ron"), &assets).unwrap(), 2);
        let bare = parse(&file(YEOMAN, CUBOID, "")).unwrap();
        assert_eq!(
            bare.check_files(Path::new("v.ron"), &assets.join("absent"))
                .unwrap(),
            0,
            "no Scene, nothing to find, even without an assets directory"
        );
        std::fs::remove_file(assets.join("m/castle.gltf")).unwrap();
        let err = v
            .check_files(Path::new("v.ron"), &assets)
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with(
                r#"v.ron: units["yeoman"].parts[0].path: "m/castle.gltf" is not a file under "#
            ),
            "{err}"
        );
        // A directory is not a model.
        std::fs::create_dir_all(assets.join("m/castle.gltf")).unwrap();
        assert!(v.check_files(Path::new("v.ron"), &assets).is_err());
        let _ = std::fs::remove_dir_all(&assets);
    }
}
