//! The minimap (art pass, owner decision 7): bottom-left above the bottom
//! bar, [`MINIMAP_PX`] logical px square, north up, one texel per map tile.
//! It shows the terrain, territory and its borders, every building (a site
//! blended toward stone), an outline around every Town (amber, steady,
//! while it is being annexed), every unit (no fog until M7, which suits
//! `--spectate`), the local selection and the camera's view trapezoid. No
//! pings, nothing blinks.
//!
//! **Placement.** A frame node ([`MinimapFrame`], a child of `HudRoot`) at
//! `left: 16 px` (outside the camera's 12 px edge-scroll band) and
//! `bottom: BOTTOM_BAR_HEIGHT_PX + PADDING_PX`, with a
//! [`MINIMAP_BORDER_PX`] border in `palette::HUD_PANEL`, holds the
//! [`Minimap`] `ImageNode`. Both keep the default `Pickable`, so the
//! minimap blocks world clicks and `hud::PointerOverUi` is true over it.
//! The centred message line and annexation readout stay where they are;
//! no text of theirs reaches it (`minimap_overlaps_no_hud_text`).
//!
//! **Painting.** The image is `Rgba8UnormSrgb`, one texel per tile, sampled
//! nearest. Three layers ([`MinimapCanvas`]):
//!
//! 1. terrain, painted once from `ground::base_colors`, darkened to
//!    [`palette::MINIMAP_TERRAIN_SHADE`];
//! 2. territory over it, rebuilt in `FixedUpdate` right after the sim step
//!    whenever the field's border sources change (the trigger and the step
//!    `field::sync_field` uses), so a new border shows in the frame of the
//!    tick that made it: owned tiles blend [`palette::MINIMAP_TERRITORY`]
//!    toward the owner's team colour, owned tiles with a 4-neighbour of
//!    another owner [`palette::MINIMAP_BORDER`];
//! 3. everything that moves, over a copy of the first two
//!    ([`paint_dynamic`]): buildings, Town outlines, unit dots
//!    ([`UNIT_DOT_TEXELS`] square, the team colour brightened), the
//!    selection in `palette::HUD_TEXT`, and the view trapezoid: the four
//!    corners of the world view between the HUD bars cast to the ground
//!    with `orders::ground_hit` ([`view_quad`]), drawn as clipped 1-texel
//!    lines.
//!
//! [`paint_minimap`] repaints at most once a frame, and only when the tick,
//! the camera's focus or zoom, the window size, the selection or the
//! territory changed ([`PaintKey`]); each repaint is timed and the exit
//! line `frame_stats: minimap repaints=<n> paint_us mean=<f> p95=<f>
//! max=<f>` reports them.
//!
//! **Input** (observers on the [`Minimap`] entity; a UI hit position runs
//! from `(-0.5, -0.5)` at the node's top left to `(0.5, 0.5)`):
//!
//! - a primary press (not in attack-move mode) moves the camera's focus to
//!   the tile's centre, keeping the zoom; a primary drag does the same from
//!   the pointer's location mapped through the node's transform, since
//!   `Pointer<Drag>` carries no hit;
//! - a primary click in attack-move mode issues `AttackMove` to the tile
//!   for the own selected units and disarms; the camera stays;
//! - a secondary click in the normal mode issues [`ground_order`]:
//!   `Move` (shift queues) for the own selected units, or `SetRally` with an
//!   own building selected.
//!
//! Orders go through `PendingCommands::push_local` (the sim validates them)
//! and leave the usual move or attack marker at the tile. They are issued
//! only while the local player is human and the match is not over: these
//! observers run outside `hud::WorldInputSet`, so they check both
//! themselves. Moving the camera is a view action, so the press and drag
//! handlers do not check either and it works while spectating. After the
//! outcome it does not: the outcome overlay (`outcome::OUTCOME_Z`, a
//! full-window node with the default `Pickable`) covers the minimap, so a
//! press there lands on the overlay and never reaches these observers (a
//! drag started on the minimap before the outcome and held through it
//! still moves the camera, since `Pointer<Drag>` follows the pressed
//! entity).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use bevy::app::AppExit;
use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::picking::events::{Click, Drag, Pointer, Press};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::PrimaryWindow;
use rules::Rules;
use sim::{BuildingId, Command, Fx, FxVec2, SimView, Source, UnitId};

use crate::camera::RtsCamera;
use crate::frame_stats::{STDOUT_PREFIX, summarize};
use crate::hud::{BOTTOM_BAR_HEIGHT_PX, HudRoot, PADDING_PX, TOP_BAR_HEIGHT_PX, spawn_hud};
use crate::orders::{OrderMode, ground_hit, ground_order};
use crate::palette;
use crate::present::world_from_sim;
use crate::selection::{RingAssets, Selection, spawn_attack_marker, spawn_move_marker};
use crate::sim_driver::{
    LocalSeat, MatchOver, PendingCommands, SimHandle, SimSystems, local_player_is_human,
    sim_stepped,
};

/// The minimap's side, logical px (owner decision 7): 1.5 px per tile on a
/// 128-tile map at scale 1, 3 physical px per tile on the Retina stream.
pub const MINIMAP_PX: f32 = 192.0;
/// The frame's distance from the window's left edge, logical px: past the
/// camera's 12 px edge-scroll band (`camera::EDGE_SCROLL_MARGIN_PX`), so
/// the pointer over the minimap never scrolls the view.
pub const MINIMAP_LEFT_PX: f32 = 16.0;
/// The frame's distance from the window's bottom edge, logical px: on the
/// bottom bar, with the HUD's padding.
pub const MINIMAP_BOTTOM_PX: f32 = BOTTOM_BAR_HEIGHT_PX + PADDING_PX;
/// The frame's border, logical px.
pub const MINIMAP_BORDER_PX: f32 = 3.0;
/// The frame's outer side, logical px.
pub const MINIMAP_FRAME_PX: f32 = MINIMAP_PX + 2.0 * MINIMAP_BORDER_PX;
/// A unit's dot, texels a side.
pub const UNIT_DOT_TEXELS: i32 = 2;

/// The frame around the minimap (a child of `HudRoot`).
#[derive(Component, Debug, Clone, Copy)]
pub struct MinimapFrame;

/// The minimap's `ImageNode`.
#[derive(Component, Debug, Clone, Copy)]
pub struct Minimap;

/// One sRGB texel.
pub type Texel = [u8; 4];

/// The minimap image and its cached layers.
#[derive(Resource, Debug)]
pub struct MinimapCanvas {
    /// The image the [`Minimap`] node shows.
    pub image: Handle<Image>,
    /// Map width and height in tiles (texels).
    pub size: (usize, usize),
    /// Terrain, darkened (painted once).
    pub terrain: Vec<Texel>,
    /// Terrain plus territory and borders.
    pub territory: Vec<Texel>,
    /// The border sources [`MinimapCanvas::territory`] shows.
    shown: Option<BTreeMap<BuildingId, Source>>,
    /// Bumped whenever the territory layer is rebuilt.
    pub territory_generation: u32,
    /// What the image shows now; `None` before the first repaint.
    last: Option<PaintKey>,
}

/// Repaint and input counters, for the exit line and `--minimap-check`.
#[derive(Resource, Debug, Default, Clone)]
pub struct MinimapStats {
    /// Repaints so far.
    pub repaints: u32,
    /// Each repaint's paint time, microseconds.
    pub paint_us: Vec<f64>,
    /// Camera moves by a minimap press or drag.
    pub camera_moves: u32,
    /// `Pointer<Drag>` events handled.
    pub drags: u32,
    /// The exit line was printed.
    printed: bool,
}

impl MinimapStats {
    /// `minimap repaints=<n> paint_us mean=<f> p95=<f> max=<f>`.
    pub fn line(&self) -> String {
        let (mean, p95, max) = summarize(&self.paint_us);
        format!(
            "minimap repaints={} paint_us mean={mean:.1} p95={p95:.1} max={max:.1}",
            self.repaints
        )
    }
}

/// Everything a repaint depends on besides the sim tick's state: when none
/// of it changed, the image is current.
#[derive(Debug, Clone, PartialEq)]
pub struct PaintKey {
    /// The sim tick.
    pub tick: u32,
    /// The camera's focus and zoom.
    pub camera: Option<(Vec3, f32)>,
    /// The window's logical size.
    pub window: Option<Vec2>,
    /// The selected units and building.
    pub units: BTreeSet<UnitId>,
    /// The selected building.
    pub building: Option<BuildingId>,
    /// [`MinimapCanvas::territory_generation`].
    pub territory: u32,
}

/// Spawns the minimap, keeps it painted and handles its input. Added by
/// `app::run`.
pub struct MinimapPlugin;

impl Plugin for MinimapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MinimapStats>()
            .add_systems(Startup, spawn_minimap.after(spawn_hud))
            .add_systems(
                FixedUpdate,
                sync_territory_layer
                    .after(SimSystems::Step)
                    .run_if(sim_stepped),
            )
            .add_systems(
                PostUpdate,
                paint_minimap.after(bevy::transform::TransformSystems::Propagate),
            )
            .add_systems(Last, print_minimap_line.after(bevy::window::ExitSystems));
    }
}

/// An sRGB texel of `c`.
pub fn texel(c: Color) -> Texel {
    c.to_srgba().to_u8_array()
}

/// `a` blended `t` of the way toward `b`, per channel (alpha from `a`).
pub fn mix(a: Texel, b: Texel, t: f32) -> Texel {
    let ch = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    [ch(a[0], b[0]), ch(a[1], b[1]), ch(a[2], b[2]), a[3]]
}

/// The terrain layer of a `width` x `height` map: each tile's ground
/// colour (`ground::base_colors`, a 128 x 128 grid) darkened to
/// [`palette::MINIMAP_TERRAIN_SHADE`].
pub fn terrain_layer(map: Option<&rules::MapDef>, width: usize, height: usize) -> Vec<Texel> {
    let colors = crate::ground::base_colors(map);
    let n = crate::ground::GROUND_SIZE_TILES as usize;
    let black = [0, 0, 0, 255];
    (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            let c = colors.get(y * n + x).copied().unwrap_or(palette::GRASS[0]);
            mix(texel(c), black, 1.0 - palette::MINIMAP_TERRAIN_SHADE)
        })
        .collect()
}

/// `terrain` with the territory of `owners` (the field's owner codes, 0
/// neutral, else player index + 1) over it: an owned tile blends
/// [`palette::MINIMAP_TERRITORY`] toward its owner's team colour, an owned
/// tile with a 4-neighbour of another owner (or neutral land)
/// [`palette::MINIMAP_BORDER`].
pub fn territory_layer(terrain: &[Texel], owners: &[u8], width: usize) -> Vec<Texel> {
    let height = owners.len().checked_div(width).unwrap_or(0);
    let mut out = terrain.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            let code = owners[i];
            if code == 0 || i >= out.len() {
                continue;
            }
            let team = texel(palette::team_color(code - 1));
            let differs = |nx: isize, ny: isize| {
                nx >= 0
                    && ny >= 0
                    && (nx as usize) < width
                    && (ny as usize) < height
                    && owners[ny as usize * width + nx as usize] != code
            };
            let (xi, yi) = (x as isize, y as isize);
            let border = differs(xi - 1, yi)
                || differs(xi + 1, yi)
                || differs(xi, yi - 1)
                || differs(xi, yi + 1);
            let t = if border {
                palette::MINIMAP_BORDER
            } else {
                palette::MINIMAP_TERRITORY
            };
            out[i] = mix(out[i], team, t);
        }
    }
    out
}

/// A texel buffer being painted, with clipped writes.
struct Paint<'a> {
    px: &'a mut [Texel],
    width: usize,
    height: usize,
}

impl Paint<'_> {
    fn set(&mut self, x: i32, y: i32, c: Texel) {
        self.blend(x, y, c, 1.0);
    }

    /// Blend texel `(x, y)` `t` of the way toward `c` (skipped off the
    /// image).
    fn blend(&mut self, x: i32, y: i32, c: Texel, t: f32) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            let i = y as usize * self.width + x as usize;
            self.px[i] = if t >= 1.0 { c } else { mix(self.px[i], c, t) };
        }
    }

    /// A 1-texel line from `a` to `b` (texel coordinates) blended `t`
    /// toward `c`, clipped to the image (Bresenham over the floored end
    /// points; ends far outside the image are first clamped to a few
    /// image sizes away, which keeps the visible part and bounds the
    /// steps).
    fn line(&mut self, a: Vec2, b: Vec2, c: Texel, t: f32) {
        let limit = 4.0 * (self.width + self.height) as f32;
        let clamp = |p: Vec2| p.clamp(Vec2::splat(-limit), Vec2::splat(limit));
        let (a, b) = (clamp(a), clamp(b));
        let (mut x0, mut y0) = (a.x.floor() as i32, a.y.floor() as i32);
        let (x1, y1) = (b.x.floor() as i32, b.y.floor() as i32);
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let mut err = dx + dy;
        loop {
            self.blend(x0, y0, c, t);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    /// The 1-texel ring just outside the `w` x `h` rect at `(x, y)`.
    fn outline(&mut self, x: i32, y: i32, w: i32, h: i32, c: Texel) {
        for i in x - 1..=x + w {
            self.set(i, y - 1, c);
            self.set(i, y + h, c);
        }
        for j in y..y + h {
            self.set(x - 1, j, c);
            self.set(x + w, j, c);
        }
    }
}

/// The dynamic layer over a copy of `territory`: complete buildings fill
/// their footprint in their owner's team colour, sites the same blended
/// [`palette::MINIMAP_SITE`] toward stone; units as
/// [`UNIT_DOT_TEXELS`]-square dots in their team colour brightened
/// [`palette::MINIMAP_UNIT_BRIGHTEN`], the selected ones (and the selected
/// building) in `palette::HUD_TEXT`; over them a ring of `palette::HUD_TEXT`
/// just outside every Town, `palette::ATTRITION` amber while it is being
/// annexed; then `quad`, the view trapezoid in texel coordinates, as
/// 1-texel lines.
pub fn paint_dynamic(
    territory: &[Texel],
    width: usize,
    view: &SimView<'_>,
    rules: &Rules,
    selection: &Selection,
    quad: Option<[Vec2; 4]>,
) -> Vec<Texel> {
    let mut px = territory.to_vec();
    let height = px.len().checked_div(width).unwrap_or(0);
    let mut paint = Paint {
        px: &mut px,
        width,
        height,
    };
    let selected = texel(palette::HUD_TEXT);
    for b in view.buildings() {
        let Some(kind) = rules.building_kind(b.kind.0) else {
            continue;
        };
        let team = texel(palette::team_color(b.owner.0));
        let fill = match b.construction {
            sim::Construction::Complete => team,
            sim::Construction::Site { .. } => {
                mix(team, texel(palette::STONE), palette::MINIMAP_SITE)
            }
        };
        let fill = if selection.building == Some(b.id) {
            selected
        } else {
            fill
        };
        let (x, y) = (i32::from(b.tile.x), i32::from(b.tile.y));
        let (w, h) = (kind.width_tiles as i32, kind.height_tiles as i32);
        for j in y..y + h {
            for i in x..x + w {
                paint.set(i, j, fill);
            }
        }
    }
    for u in view.units() {
        let c = if selection.units.contains(&u.id) {
            selected
        } else {
            mix(
                texel(palette::team_color(u.owner.0)),
                [255, 255, 255, 255],
                palette::MINIMAP_UNIT_BRIGHTEN,
            )
        };
        let (x0, y0) = dot_origin(u.pos);
        for j in y0..y0 + UNIT_DOT_TEXELS {
            for i in x0..x0 + UNIT_DOT_TEXELS {
                paint.set(i, j, c);
            }
        }
    }
    // Town outlines over the unit dots: the attackers that start an
    // annexation stand next to the Town, where the ring is.
    for b in view.buildings() {
        let Some(kind) = rules.building_kind(b.kind.0).filter(|k| k.town) else {
            continue;
        };
        let ring = if view.annexation(b.id).is_some() {
            texel(palette::ATTRITION)
        } else {
            texel(palette::HUD_TEXT)
        };
        paint.outline(
            i32::from(b.tile.x),
            i32::from(b.tile.y),
            kind.width_tiles as i32,
            kind.height_tiles as i32,
            ring,
        );
    }
    if let Some(q) = quad {
        let c = texel(palette::HUD_TEXT);
        for k in 0..4 {
            paint.line(q[k], q[(k + 1) % 4], c, palette::MINIMAP_VIEW);
        }
    }
    px
}

/// The top-left texel of a unit's dot: the [`UNIT_DOT_TEXELS`] square
/// whose centre is nearest `pos`.
pub fn dot_origin(pos: FxVec2) -> (i32, i32) {
    let half = Fx::from_ratio(UNIT_DOT_TEXELS, 2);
    let corner = |v: Fx| (v - half).round_to_int();
    (corner(pos.x), corner(pos.y))
}

/// The texel under a UI hit position `p`, normalised to the node from
/// `(-0.5, -0.5)` (top left) to `(0.5, 0.5)`: `floor((p + 0.5) * size)`,
/// clamped to the image.
pub fn texel_from_norm(p: Vec2, size: (usize, usize)) -> (u16, u16) {
    let axis = |v: f32, n: usize| {
        let max = n.saturating_sub(1);
        let t = ((v + 0.5) * n as f32).floor();
        if t.is_nan() || t < 0.0 {
            0
        } else {
            (t as usize).min(max)
        }
    };
    let clamp16 = |v: usize| u16::try_from(v).unwrap_or(u16::MAX);
    (clamp16(axis(p.x, size.0)), clamp16(axis(p.y, size.1)))
}

/// A logical window position normalised to a UI node the way
/// `bevy_ui`'s picking backend normalises a hit: through the node's
/// `UiGlobalTransform` inverse (physical px) over its computed size.
pub fn norm_from_logical(
    position: Vec2,
    node: &ComputedNode,
    transform: &UiGlobalTransform,
) -> Vec2 {
    let physical = position / node.inverse_scale_factor;
    transform.inverse().transform_point2(physical) / node.size()
}

/// The sim position of tile `(x, y)`'s centre.
pub fn tile_centre(x: u16, y: u16) -> FxVec2 {
    FxVec2::new(
        Fx::from_int(i32::from(x)) + Fx::HALF,
        Fx::from_int(i32::from(y)) + Fx::HALF,
    )
}

/// World metres to minimap texel coordinates (`(0, 0)` is the north-west
/// corner of tile `(0, 0)`).
pub fn texel_from_world(p: Vec3) -> Vec2 {
    let half = crate::ground::HALF_EXTENT;
    Vec2::new(p.x + half, p.z + half)
}

/// The view trapezoid: where the four corners of the world view between
/// the HUD bars (`window` logical px, the top bar above, the bottom bar
/// below) meet the ground (`orders::ground_hit`), in texel coordinates,
/// clockwise from the top left. `None` when a corner misses the ground.
pub fn view_quad(cam: &Camera, cam_tf: &GlobalTransform, window: Vec2) -> Option<[Vec2; 4]> {
    let top = TOP_BAR_HEIGHT_PX;
    let bottom = (window.y - BOTTOM_BAR_HEIGHT_PX).max(top);
    let corners = [
        Vec2::new(0.0, top),
        Vec2::new(window.x, top),
        Vec2::new(window.x, bottom),
        Vec2::new(0.0, bottom),
    ];
    let mut out = [Vec2::ZERO; 4];
    for (o, c) in out.iter_mut().zip(corners) {
        *o = texel_from_world(ground_hit(cam, cam_tf, c)?);
    }
    Some(out)
}

/// The image the minimap shows: `width` x `height` sRGB texels, sampled
/// nearest, kept in the main world so it can be repainted.
pub fn minimap_image(width: usize, height: usize, px: &[Texel]) -> Image {
    let mut image = Image::new(
        Extent3d {
            width: u32::try_from(width).unwrap_or(1),
            height: u32::try_from(height).unwrap_or(1),
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        px.concat(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::nearest();
    image
}

/// Startup, after the HUD: the frame and the image node, the terrain and
/// territory layers. Without a sim (some tests) the map is 128 x 128 grass.
pub fn spawn_minimap(
    mut commands: Commands,
    root: Query<Entity, With<HudRoot>>,
    sim: Option<NonSend<SimHandle>>,
    mut images: ResMut<Assets<Image>>,
) {
    let (map, width, height, owners, shown) = match sim.as_ref() {
        Some(s) => {
            let view = s.view();
            let field = view.territory();
            (
                s.rules().map(&s.setup().map).cloned(),
                usize::from(view.map().width()),
                usize::from(view.map().height()),
                field.owner_grid().to_vec(),
                Some(field.sources().clone()),
            )
        }
        None => {
            let n = crate::ground::GROUND_SIZE_TILES as usize;
            (None, n, n, vec![0; n * n], None)
        }
    };
    let terrain = terrain_layer(map.as_ref(), width, height);
    let territory = territory_layer(&terrain, &owners, width);
    let image = images.add(minimap_image(width, height, &territory));
    commands.insert_resource(MinimapCanvas {
        image: image.clone(),
        size: (width, height),
        terrain,
        territory,
        shown,
        territory_generation: 0,
        last: None,
    });
    let Ok(root) = root.single() else {
        return;
    };
    commands.entity(root).with_children(|root| {
        root.spawn((
            Name::new("Minimap frame"),
            MinimapFrame,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(MINIMAP_LEFT_PX),
                bottom: Val::Px(MINIMAP_BOTTOM_PX),
                width: Val::Px(MINIMAP_FRAME_PX),
                height: Val::Px(MINIMAP_FRAME_PX),
                border: UiRect::all(Val::Px(MINIMAP_BORDER_PX)),
                ..default()
            },
            BorderColor::all(palette::HUD_PANEL),
            BackgroundColor(palette::HUD_PANEL),
        ))
        .with_children(|frame| {
            frame
                .spawn((
                    Name::new("Minimap"),
                    Minimap,
                    ImageNode::new(image),
                    Node {
                        width: Val::Px(MINIMAP_PX),
                        height: Val::Px(MINIMAP_PX),
                        ..default()
                    },
                ))
                .observe(on_minimap_press)
                .observe(on_minimap_drag)
                .observe(on_minimap_click);
        });
    });
}

/// After every stepped tick: rebuild the territory layer when the field's
/// border sources differ from the ones it shows (`field::sync_field`'s
/// trigger), so the next repaint shows the new border.
pub fn sync_territory_layer(sim: NonSend<SimHandle>, canvas: Option<ResMut<MinimapCanvas>>) {
    let Some(mut canvas) = canvas else {
        return;
    };
    let view = sim.view();
    let field = view.territory();
    if canvas.shown.as_ref() == Some(field.sources()) {
        return;
    }
    let width = canvas.size.0;
    canvas.territory = territory_layer(&canvas.terrain, field.owner_grid(), width);
    canvas.shown = Some(field.sources().clone());
    canvas.territory_generation = canvas.territory_generation.wrapping_add(1);
}

/// `PostUpdate`, after transforms propagate: repaint when the
/// [`PaintKey`] changed, timing the paint.
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn paint_minimap(
    sim: NonSend<SimHandle>,
    canvas: Option<ResMut<MinimapCanvas>>,
    mut stats: ResMut<MinimapStats>,
    selection: Res<Selection>,
    rts: Query<&RtsCamera>,
    cameras: Query<(&Camera, &GlobalTransform), With<RtsCamera>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut canvas) = canvas else {
        return;
    };
    let window = windows
        .single()
        .ok()
        .map(|w| Vec2::new(w.width(), w.height()));
    let key = PaintKey {
        tick: sim.tick(),
        camera: rts.single().ok().map(|c| (c.focus, c.zoom)),
        window,
        units: selection.units.clone(),
        building: selection.building,
        territory: canvas.territory_generation,
    };
    if canvas.last.as_ref() == Some(&key) {
        return;
    }
    let start = Instant::now();
    let quad = match (cameras.single(), window) {
        (Ok((cam, cam_tf)), Some(size)) => view_quad(cam, cam_tf, size),
        _ => None,
    };
    let px = paint_dynamic(
        &canvas.territory,
        canvas.size.0,
        &sim.view(),
        sim.rules(),
        &selection,
        quad,
    );
    if let Some(mut image) = images.get_mut(&canvas.image) {
        image.data = Some(px.concat());
    }
    stats.repaints += 1;
    stats.paint_us.push(start.elapsed().as_secs_f64() * 1e6);
    canvas.last = Some(key);
}

/// Focus the RTS camera on tile `(x, y)`'s centre, keeping the zoom.
fn focus_on(cameras: &mut Query<&mut RtsCamera>, stats: &mut MinimapStats, x: u16, y: u16) {
    let at = world_from_sim(tile_centre(x, y), 0.0);
    for mut cam in cameras.iter_mut() {
        cam.focus = Vec3::new(at.x, 0.0, at.z);
    }
    stats.camera_moves += 1;
}

/// A primary press (not in attack-move mode) moves the camera to the
/// pressed tile.
pub fn on_minimap_press(
    press: On<Pointer<Press>>,
    canvas: Res<MinimapCanvas>,
    mode: Res<OrderMode>,
    mut stats: ResMut<MinimapStats>,
    mut cameras: Query<&mut RtsCamera>,
) {
    let event = &press.event().event;
    if event.button != PointerButton::Primary || *mode == OrderMode::AttackMove {
        return;
    }
    let Some(position) = event.hit.position else {
        return;
    };
    let (x, y) = texel_from_norm(position.truncate(), canvas.size);
    focus_on(&mut cameras, &mut stats, x, y);
}

/// A primary drag (not in attack-move mode) keeps the camera on the tile
/// under the pointer.
pub fn on_minimap_drag(
    drag: On<Pointer<Drag>>,
    canvas: Res<MinimapCanvas>,
    mode: Res<OrderMode>,
    nodes: Query<(&ComputedNode, &UiGlobalTransform), With<Minimap>>,
    mut stats: ResMut<MinimapStats>,
    mut cameras: Query<&mut RtsCamera>,
) {
    let pointer = drag.event();
    if pointer.event.button != PointerButton::Primary || *mode == OrderMode::AttackMove {
        return;
    }
    let Ok((node, transform)) = nodes.get(pointer.entity) else {
        return;
    };
    if node.size().min_element() <= 0.0 {
        return;
    }
    let norm = norm_from_logical(pointer.pointer_location.position, node, transform);
    let (x, y) = texel_from_norm(norm, canvas.size);
    stats.drags += 1;
    focus_on(&mut cameras, &mut stats, x, y);
}

/// The world state a minimap order reads, for [`on_minimap_click`].
#[derive(bevy::ecs::system::SystemParam)]
pub struct MinimapOrders<'w> {
    canvas: Res<'w, MinimapCanvas>,
    mode: ResMut<'w, OrderMode>,
    selection: Res<'w, Selection>,
    pending: ResMut<'w, PendingCommands>,
    keys: Option<Res<'w, ButtonInput<KeyCode>>>,
    seat: Option<Res<'w, LocalSeat>>,
    over: Option<Res<'w, MatchOver>>,
    ring_assets: Option<Res<'w, RingAssets>>,
    time: Res<'w, Time>,
}

/// A primary click in attack-move mode issues `AttackMove` to the tile and
/// disarms; a secondary click in the normal mode issues
/// [`ground_order`] (`Move`, or `SetRally` for an own building).
/// Nothing while the local slot is a bot or after the outcome.
pub fn on_minimap_click(
    click: On<Pointer<Click>>,
    sim: NonSend<SimHandle>,
    mut orders: MinimapOrders,
    mut commands: Commands,
) {
    let event = &click.event().event;
    if !local_player_is_human(orders.seat.take()) || orders.over.is_some() {
        return;
    }
    let mode = match (event.button, *orders.mode) {
        (PointerButton::Primary, OrderMode::AttackMove) => OrderMode::AttackMove,
        (PointerButton::Secondary, OrderMode::Normal) => OrderMode::Normal,
        _ => return,
    };
    let Some(position) = event.hit.position else {
        return;
    };
    let (x, y) = texel_from_norm(position.truncate(), orders.canvas.size);
    let target = tile_centre(x, y);
    let queue = orders
        .keys
        .as_ref()
        .is_some_and(|k| k.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]));
    let view = sim.view();
    if mode == OrderMode::AttackMove {
        *orders.mode = OrderMode::Normal;
    }
    let Some(cmd) = ground_order(mode, &view, &orders.selection, target, queue) else {
        return;
    };
    let attack = matches!(cmd, Command::AttackMove { .. });
    let seq = orders.pending.push_local(cmd);
    info!("minimap: seq {seq} to tile ({x}, {y})");
    if let Some(assets) = orders.ring_assets.as_deref() {
        let at = world_from_sim(target, 0.0);
        let now = orders.time.elapsed_secs();
        if attack {
            spawn_attack_marker(&mut commands, assets, at, now);
        } else {
            spawn_move_marker(&mut commands, assets, at, now);
        }
    }
}

/// On the frame an `AppExit` was written, print [`MinimapStats::line`]
/// once (after `ExitSystems`, as `frame_stats` does).
pub fn print_minimap_line(mut exits: MessageReader<AppExit>, mut stats: ResMut<MinimapStats>) {
    if exits.is_empty() || stats.printed {
        exits.clear();
        return;
    }
    exits.clear();
    stats.printed = true;
    println!("{STDOUT_PREFIX}{}", stats.line());
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::camera::NormalizedRenderTarget;
    use bevy::picking::backend::HitData;
    use bevy::picking::pointer::{Location, PointerId};
    use rules::Terrain;
    use sim::{BuildingId, PlayerCommand, PlayerId};

    use super::*;
    use crate::sim_driver::{LOCAL_PLAYER, LocalSeat};

    fn rules() -> Rules {
        Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
            .expect("rules load")
    }

    /// `economy_walkthrough` at tick 1: the standard start (P0's Seat at
    /// (24, 64) with its Yeomen, P1's at (103, 64)) and its tick-0
    /// commands.
    fn walkthrough() -> SimHandle {
        let rules = rules();
        let (setup, stream) = sim::scenarios::economy_walkthrough(&rules);
        let mut sim = SimHandle::from_setup(setup, rules);
        let tick0: Vec<PlayerCommand> = stream
            .iter()
            .filter(|(t, _)| *t == 0)
            .flat_map(|(_, c)| c.iter().cloned())
            .collect();
        sim.step(&tick0);
        sim
    }

    fn step(sim: &mut SimHandle, cmds: Vec<Command>) {
        let batch: Vec<PlayerCommand> = (9000..)
            .zip(cmds)
            .map(|(seq, c)| PlayerCommand::new(LOCAL_PLAYER, seq, c))
            .collect();
        sim.step(&batch);
        for _ in 0..sim.setup().cmd_delay {
            sim.step_once();
        }
    }

    fn layers(sim: &SimHandle) -> (Vec<Texel>, Vec<Texel>, usize) {
        let view = sim.view();
        let (w, h) = (
            usize::from(view.map().width()),
            usize::from(view.map().height()),
        );
        let terrain = terrain_layer(sim.rules().map(&sim.setup().map), w, h);
        let territory = territory_layer(&terrain, view.territory().owner_grid(), w);
        (terrain, territory, w)
    }

    fn own_units(sim: &SimHandle) -> Vec<UnitId> {
        sim.view()
            .units()
            .filter(|u| u.owner == LOCAL_PLAYER)
            .map(|u| u.id)
            .collect()
    }

    #[test]
    fn texel_from_norm_maps_corners_and_clamps() {
        let size = (128, 128);
        assert_eq!(texel_from_norm(Vec2::new(-0.5, -0.5), size), (0, 0));
        assert_eq!(texel_from_norm(Vec2::new(0.0, 0.0), size), (64, 64));
        assert_eq!(texel_from_norm(Vec2::new(0.4999, 0.4999), size), (127, 127));
        assert_eq!(texel_from_norm(Vec2::new(0.5, 0.5), size), (127, 127));
        assert_eq!(texel_from_norm(Vec2::new(-3.0, 9.0), size), (0, 127));
        assert_eq!(texel_from_norm(Vec2::new(f32::NAN, 0.25), size), (0, 96));
        // A non-square map maps each axis on its own.
        assert_eq!(texel_from_norm(Vec2::new(0.25, 0.25), (64, 32)), (48, 24));
    }

    /// Terrain darkened, owned land and its border blended toward the team
    /// colour, the Seats filled and outlined, unit dots brightened, the
    /// selected Yeoman in the HUD text colour.
    #[test]
    fn paint_shows_terrain_territory_buildings_units_and_selection() {
        let mut sim = walkthrough();
        // Two Yeomen alone on open land, so no other dot covers theirs.
        let yeoman_kind = sim::UnitKindId(
            u16::try_from(
                sim.rules()
                    .units
                    .iter()
                    .position(|k| k.id == "yeoman")
                    .unwrap(),
            )
            .unwrap(),
        );
        let lone = [FxVec2::from_ints(64, 20), FxVec2::from_ints(70, 20)];
        step(
            &mut sim,
            lone.iter()
                .map(|&at| Command::DebugSpawn {
                    owner: LOCAL_PLAYER,
                    kind: yeoman_kind,
                    at,
                    count: 1,
                })
                .collect(),
        );
        let (terrain, territory, w) = layers(&sim);
        let view = sim.view();
        let rules = sim.rules();
        let map = rules.map(&sim.setup().map).unwrap();
        let shade = |c: Color| {
            mix(
                texel(c),
                [0, 0, 0, 255],
                1.0 - palette::MINIMAP_TERRAIN_SHADE,
            )
        };
        let at = |x: usize, y: usize| y * w + x;
        let find = |t: Terrain| {
            (0..w * w)
                .find(|i| map.tile((i % w) as u16, (i / w) as u16) == t)
                .unwrap()
        };
        assert_eq!(terrain[find(Terrain::Forest)], shade(palette::FOREST_FLOOR));
        assert_eq!(terrain[find(Terrain::Water)], shade(palette::WATER));
        assert_eq!(terrain[find(Terrain::Mountain)], shade(palette::ROCK));

        // A unit whose dot overlaps no other dot (painted in id order, a
        // later dot covers an earlier one).
        let isolated = |id: UnitId| {
            let me = dot_origin(view.unit(id).unwrap().pos);
            view.units().filter(|o| o.id != id).all(|o| {
                let d = dot_origin(o.pos);
                (d.0 - me.0).abs() >= UNIT_DOT_TEXELS || (d.1 - me.1).abs() >= UNIT_DOT_TEXELS
            })
        };
        let alone: Vec<UnitId> = own_units(&sim)
            .into_iter()
            .filter(|u| isolated(*u))
            .collect();
        assert_eq!(alone.len(), 2, "the two lone Yeomen: {alone:?}");
        let yeoman = alone[0];
        let mut selection = Selection::default();
        selection.set([yeoman]);
        let px = paint_dynamic(&territory, w, &view, rules, &selection, None);

        // Owned land: interior and border, away from buildings and units.
        let owners = view.territory().owner_grid();
        let covered = |x: usize, y: usize| {
            view.buildings().any(|b| {
                let k = rules.building_kind(b.kind.0).unwrap();
                let (bx, by) = (usize::from(b.tile.x), usize::from(b.tile.y));
                (bx.saturating_sub(1)..=bx + k.width_tiles as usize).contains(&x)
                    && (by.saturating_sub(1)..=by + k.height_tiles as usize).contains(&y)
            }) || view.units().any(|u| {
                let (ux, uy) = dot_origin(u.pos);
                (ux..ux + UNIT_DOT_TEXELS).contains(&(x as i32))
                    && (uy..uy + UNIT_DOT_TEXELS).contains(&(y as i32))
            })
        };
        let team0 = texel(palette::team_color(0));
        let (mut interior, mut border) = (0, 0);
        for y in 1..w - 1 {
            for x in 1..w - 1 {
                if owners[at(x, y)] != 1 || covered(x, y) {
                    continue;
                }
                let edge = [at(x - 1, y), at(x + 1, y), at(x, y - 1), at(x, y + 1)]
                    .iter()
                    .any(|&n| owners[n] != 1);
                let t = if edge {
                    border += 1;
                    palette::MINIMAP_BORDER
                } else {
                    interior += 1;
                    palette::MINIMAP_TERRITORY
                };
                assert_eq!(px[at(x, y)], mix(terrain[at(x, y)], team0, t), "({x}, {y})");
            }
        }
        assert!(interior > 100 && border > 20, "{interior} {border}");
        // Neutral land is the terrain alone.
        let neutral = (0..w * w)
            .find(|&i| owners[i] == 0 && !covered(i % w, i / w))
            .unwrap();
        assert_eq!(px[neutral], terrain[neutral]);

        // Each Seat fills its footprint with its owner's colour inside a
        // ring of the HUD text colour.
        for b in view.buildings().filter(|b| b.town_state.is_some()) {
            let k = rules.building_kind(b.kind.0).unwrap();
            let (x, y) = (usize::from(b.tile.x), usize::from(b.tile.y));
            let team = texel(palette::team_color(b.owner.0));
            assert_eq!(px[at(x + 1, y + 1)], team);
            assert_eq!(px[at(x - 1, y - 1)], texel(palette::HUD_TEXT), "ring");
            assert_eq!(
                px[at(x + k.width_tiles as usize, y + 1)],
                texel(palette::HUD_TEXT),
                "ring"
            );
        }

        // Unit dots: brightened team colour; the selected one in HUD text.
        let bright = mix(team0, [255, 255, 255, 255], palette::MINIMAP_UNIT_BRIGHTEN);
        for &id in &alone {
            let (x, y) = dot_origin(view.unit(id).unwrap().pos);
            let want = if id == yeoman {
                texel(palette::HUD_TEXT)
            } else {
                bright
            };
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = at((x + dx) as usize, (y + dy) as usize);
                assert_eq!(px[i], want, "unit {id:?}");
            }
        }
    }

    /// The minimap app: the canvas and the image node under a HUD root,
    /// the RTS camera, and the territory and paint systems in `Update`.
    fn minimap_app(sim: SimHandle) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Assets<Image>>()
            .init_resource::<Selection>()
            .init_resource::<OrderMode>()
            .init_resource::<PendingCommands>()
            .init_resource::<MinimapStats>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(Startup, spawn_minimap)
            .add_systems(Update, (sync_territory_layer, paint_minimap).chain());
        app.world_mut().spawn((HudRoot, Node::default()));
        app.world_mut().insert_non_send(sim);
        app.update();
        app
    }

    fn image_px(app: &App) -> Vec<u8> {
        let canvas = app.world().resource::<MinimapCanvas>();
        app.world()
            .resource::<Assets<Image>>()
            .get(&canvas.image)
            .and_then(|i| i.data.clone())
            .unwrap()
    }

    /// A Watchtower placed in the same update's step shows its new land
    /// on the image that update paints (the M7 "within one frame" clause,
    /// without fog), and the territory layer is rebuilt once.
    #[test]
    fn a_new_border_source_repaints_territory_in_the_same_update() {
        let mut app = minimap_app(walkthrough());
        let before = image_px(&app);
        let generation = app.world().resource::<MinimapCanvas>().territory_generation;
        let owners_before = app
            .world()
            .non_send::<SimHandle>()
            .view()
            .territory()
            .owner_grid()
            .to_vec();
        {
            let mut sim = app.world_mut().non_send_mut::<SimHandle>();
            let tower = sim.rules().building_kind_by_id("watchtower").unwrap().0;
            step(
                &mut sim,
                vec![Command::DebugPlaceBuilding {
                    owner: LOCAL_PLAYER,
                    kind: sim::BuildingKindId(tower),
                    tile: sim::Tile::new(46, 56),
                    complete: true,
                }],
            );
            // The walkthrough's own tick-0 commands apply in these steps
            // too (its outside Croft is rejected on purpose); ours is 9000.
            let events = sim.drain_events();
            assert!(
                !events.iter().any(
                    |e| matches!(e, sim::SimEvent::CommandRejected { seq, .. } if *seq >= 9000)
                ),
                "{events:?}"
            );
        }
        app.update();
        let after = image_px(&app);
        let canvas = app.world().resource::<MinimapCanvas>();
        assert_eq!(canvas.territory_generation, generation + 1);
        let sim = app.world().non_send::<SimHandle>();
        let owners = sim.view().territory().owner_grid();
        // The tiles the tower won, away from its own footprint and dots.
        let won: Vec<usize> = (0..owners.len())
            .filter(|&i| owners_before[i] == 0 && owners[i] == 1)
            .filter(|&i| !(i % 128 == 46 && i / 128 == 56))
            .collect();
        assert!(won.len() >= 8, "{} tiles won", won.len());
        for &i in &won {
            assert_ne!(
                after[i * 4..i * 4 + 4],
                before[i * 4..i * 4 + 4],
                "tile {i}"
            );
            let border = [i - 1, i + 1, i - 128, i + 128]
                .iter()
                .any(|&n| owners[n] != 1);
            let t = if border {
                palette::MINIMAP_BORDER
            } else {
                palette::MINIMAP_TERRITORY
            };
            assert_eq!(
                canvas.territory[i],
                mix(canvas.terrain[i], texel(palette::team_color(0)), t)
            );
        }
    }

    /// [`MinimapPlugin`] on the game's own schedule: `SimPlugin` steps the
    /// walkthrough in `FixedUpdate` (one tick per 50 ms update) with a
    /// Watchtower placed by the scenario stream, and the plugin's
    /// `sync_territory_layer` (after `SimSystems::Step`, only when a tick
    /// stepped) and `paint_minimap` (`PostUpdate`) run where the plugin puts
    /// them. In the very update whose tick gives the tower its land, the
    /// territory layer is rebuilt once and the image shows every won tile
    /// in P0's territory colour: the M7 "mirrors borders within one frame"
    /// clause, without fog. Any update before it leaves the image's
    /// territory as it was.
    #[test]
    fn the_plugin_shows_a_new_border_in_the_update_of_its_tick() {
        use crate::sim_driver::{ScenarioInput, SimPlugin};
        use bevy::time::TimeUpdateStrategy;
        let rules = rules();
        let (setup, mut stream) = sim::scenarios::economy_walkthrough(&rules);
        let tower = rules.building_kind_by_id("watchtower").unwrap().0;
        let place = PlayerCommand::new(
            LOCAL_PLAYER,
            9000,
            Command::DebugPlaceBuilding {
                owner: LOCAL_PLAYER,
                kind: sim::BuildingKindId(tower),
                tile: sim::Tile::new(46, 56),
                complete: true,
            },
        );
        let at = 10;
        match stream.binary_search_by_key(&at, |(t, _)| *t) {
            Ok(i) => stream[i].1.push(place),
            Err(i) => stream.insert(i, (at, vec![place])),
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO))
            .init_resource::<Assets<Image>>()
            .init_resource::<Selection>()
            .init_resource::<OrderMode>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_plugins(SimPlugin::new(
                SimHandle::from_setup(setup, rules),
                Box::new(ScenarioInput::new(stream)),
            ))
            .add_plugins(MinimapPlugin);
        app.world_mut().spawn((HudRoot, Node::default()));
        app.update();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            50,
        )));
        let owners = |app: &App| {
            app.world()
                .non_send::<SimHandle>()
                .view()
                .territory()
                .owner_grid()
                .to_vec()
        };
        let mut checked = false;
        for _ in 0..200 {
            let (owners_before, image_before) = (owners(&app), image_px(&app));
            let generation = app.world().resource::<MinimapCanvas>().territory_generation;
            app.update();
            let owners_after = owners(&app);
            let image_after = image_px(&app);
            let canvas = app.world().resource::<MinimapCanvas>();
            let won: Vec<usize> = (0..owners_after.len())
                .filter(|&i| owners_before[i] == 0 && owners_after[i] == 1)
                .filter(|&i| !(i % 128 == 46 && i / 128 == 56))
                .collect();
            if won.is_empty() {
                continue;
            }
            assert!(won.len() >= 8, "{} tiles won", won.len());
            assert_eq!(
                canvas.territory_generation,
                generation + 1,
                "the layer is rebuilt once, in this update"
            );
            for &i in &won {
                assert_ne!(
                    image_after[i * 4..i * 4 + 4],
                    image_before[i * 4..i * 4 + 4],
                    "tile {i} changes on the image in the update of its tick"
                );
                let interior = [i - 1, i + 1, i - 128, i + 128]
                    .iter()
                    .all(|&n| owners_after[n] == 1);
                if interior {
                    assert_eq!(
                        canvas.territory[i],
                        mix(
                            canvas.terrain[i],
                            texel(palette::team_color(0)),
                            palette::MINIMAP_TERRITORY
                        ),
                        "tile {i}"
                    );
                }
            }
            checked = true;
            break;
        }
        assert!(checked, "the tower never won land");
    }

    /// While P0 annexes P1's Seat (`capture_seat`), its ring is amber and
    /// P0's own Seat keeps the HUD text ring.
    #[test]
    fn a_town_under_annexation_gets_the_amber_outline() {
        let rules = rules();
        let (setup, stream) = sim::scenarios::capture_seat(&rules);
        let mut sim = SimHandle::from_setup(setup, rules);
        let seat = BuildingId(2);
        for t in 0..3000 {
            if sim.view().annexation(seat).is_some() {
                break;
            }
            let cmds: Vec<PlayerCommand> = stream
                .iter()
                .filter(|(k, _)| *k == t)
                .flat_map(|(_, c)| c.iter().cloned())
                .collect();
            sim.step(&cmds);
        }
        let view = sim.view();
        assert!(view.annexation(seat).is_some(), "the timer runs");
        let (_, territory, w) = layers(&sim);
        let px = paint_dynamic(
            &territory,
            w,
            &view,
            sim.rules(),
            &Selection::default(),
            None,
        );
        // Every texel of the ring just outside the footprint, drawn over
        // the attackers' dots that stand there.
        let ring = |b: BuildingId| -> Vec<Texel> {
            let b = view.building(b).unwrap();
            let (x, y) = (usize::from(b.tile.x), usize::from(b.tile.y));
            let mut out = Vec::new();
            for i in x - 1..=x + 3 {
                out.push(px[(y - 1) * w + i]);
                out.push(px[(y + 3) * w + i]);
            }
            for j in y..y + 3 {
                out.push(px[j * w + x - 1]);
                out.push(px[j * w + x + 3]);
            }
            out
        };
        assert!(ring(seat).iter().all(|t| *t == texel(palette::ATTRITION)));
        assert!(
            ring(BuildingId(1))
                .iter()
                .all(|t| *t == texel(palette::HUD_TEXT))
        );
    }

    /// A camera at the default framing over a 1280 x 800 window.
    fn camera(rts: &RtsCamera) -> (Camera, GlobalTransform) {
        let mut camera = Camera::default();
        camera.computed.target_info = Some(bevy::camera::RenderTargetInfo {
            physical_size: UVec2::new(1280, 800),
            scale_factor: 1.0,
        });
        camera.computed.clip_from_view = Projection::Perspective(PerspectiveProjection {
            fov: 40.0_f32.to_radians(),
            aspect_ratio: 1280.0 / 800.0,
            ..default()
        })
        .get_clip_from_view();
        let tf = GlobalTransform::from(
            Transform::from_translation(rts.eye()).looking_at(rts.focus, Vec3::Y),
        );
        (camera, tf)
    }

    /// The trapezoid holds the focus, its far (north) edge is wider than
    /// its near one, and it sits north of the near edge (north up).
    #[test]
    fn frustum_trapezoid_contains_the_focus_and_its_far_edge_is_wider() {
        let rts = RtsCamera {
            focus: Vec3::new(-10.0, 0.0, 5.0),
            ..RtsCamera::default()
        };
        let (cam, tf) = camera(&rts);
        let q = view_quad(&cam, &tf, Vec2::new(1280.0, 800.0)).expect("every corner hits");
        let focus = texel_from_world(rts.focus);
        // Convex and clockwise on screen (y down): every cross product of an
        // edge with the focus has the same sign.
        let side = |a: Vec2, b: Vec2| (b - a).perp_dot(focus - a);
        let signs: Vec<bool> = (0..4).map(|k| side(q[k], q[(k + 1) % 4]) > 0.0).collect();
        assert!(signs.iter().all(|s| *s == signs[0]), "{q:?} {focus}");
        let far = q[1].distance(q[0]);
        let near = q[2].distance(q[3]);
        assert!(far > near * 1.2, "far {far} near {near}");
        assert!(q[0].y < focus.y && q[3].y > focus.y, "north up: {q:?}");
        // Zooming out widens it.
        let wide = RtsCamera {
            zoom: crate::camera::ZOOM_MAX,
            ..rts.clone()
        };
        let (cam, tf) = camera(&wide);
        let q2 = view_quad(&cam, &tf, Vec2::new(1280.0, 800.0)).unwrap();
        assert!(q2[1].distance(q2[0]) > far);
        // The lines are drawn: some texel along the near edge changed.
        let sim = walkthrough();
        let (_, territory, w) = layers(&sim);
        let plain = paint_dynamic(
            &territory,
            w,
            &sim.view(),
            sim.rules(),
            &Selection::default(),
            None,
        );
        let lined = paint_dynamic(
            &territory,
            w,
            &sim.view(),
            sim.rules(),
            &Selection::default(),
            Some(q),
        );
        let mid = (q[2] + q[3]) / 2.0;
        let i = mid.y.floor() as usize * w + mid.x.floor() as usize;
        assert_ne!(plain[i], lined[i]);
        assert_eq!(
            lined[i],
            mix(plain[i], texel(palette::HUD_TEXT), palette::MINIMAP_VIEW)
        );
    }

    /// The input app: [`minimap_app`] with the RTS camera, the image node
    /// laid out by hand (192 px square centred at (115, 595), scale 1).
    fn input_app(sim: SimHandle) -> (App, Entity) {
        let mut app = minimap_app(sim);
        app.world_mut().spawn(RtsCamera::default());
        let minimap = app
            .world_mut()
            .query_filtered::<Entity, With<Minimap>>()
            .single(app.world())
            .unwrap();
        app.world_mut().entity_mut(minimap).insert((
            ComputedNode {
                size: Vec2::splat(MINIMAP_PX),
                inverse_scale_factor: 1.0,
                ..default()
            },
            UiGlobalTransform::from_translation(Vec2::new(115.0, 595.0)),
        ));
        (app, minimap)
    }

    fn location(position: Vec2) -> Location {
        Location {
            target: NormalizedRenderTarget::None {
                width: 1,
                height: 1,
            },
            position,
        }
    }

    fn hit(norm: Vec2) -> HitData {
        HitData::new(Entity::PLACEHOLDER, 0.0, Some(norm.extend(0.0)), None)
    }

    fn press(app: &mut App, minimap: Entity, norm: Vec2) {
        app.world_mut().trigger(Pointer::new(
            PointerId::Mouse,
            location(Vec2::ZERO),
            Press {
                button: PointerButton::Primary,
                hit: hit(norm),
                count: 1,
            },
            minimap,
        ));
    }

    fn drag(app: &mut App, minimap: Entity, position: Vec2) {
        app.world_mut().trigger(Pointer::new(
            PointerId::Mouse,
            location(position),
            Drag {
                button: PointerButton::Primary,
                distance: Vec2::ZERO,
                delta: Vec2::ZERO,
            },
            minimap,
        ));
    }

    fn click(app: &mut App, minimap: Entity, norm: Vec2, button: PointerButton) {
        app.world_mut().trigger(Pointer::new(
            PointerId::Mouse,
            location(Vec2::ZERO),
            Click {
                button,
                hit: hit(norm),
                duration: Duration::ZERO,
                count: 1,
            },
            minimap,
        ));
    }

    fn focus(app: &mut App) -> Vec3 {
        app.world_mut()
            .query::<&RtsCamera>()
            .single(app.world())
            .unwrap()
            .focus
    }

    fn queued(app: &mut App) -> Vec<Command> {
        app.world_mut()
            .resource_mut::<PendingCommands>()
            .drain_sorted()
            .into_iter()
            .map(|pc| pc.cmd)
            .collect()
    }

    /// A press moves the focus to the pressed tile's centre, a drag to the
    /// tile under the pointer (mapped through the node's transform), the
    /// zoom stays; in attack-move mode a press leaves the camera.
    #[test]
    fn left_press_and_drag_move_the_camera() {
        let (mut app, minimap) = input_app(walkthrough());
        let zoom = app
            .world_mut()
            .query::<&RtsCamera>()
            .single(app.world())
            .unwrap()
            .zoom;
        press(&mut app, minimap, Vec2::new(0.25, 0.25));
        // Tile (96, 96): its centre at 96.5 - 64 = 32.5 m.
        assert_eq!(focus(&mut app), Vec3::new(32.5, 0.0, 32.5));
        // 48 px left of the node's centre: norm (-0.25, 0), tile (32, 64).
        drag(&mut app, minimap, Vec2::new(115.0 - 48.0, 595.0));
        assert_eq!(focus(&mut app), Vec3::new(-31.5, 0.0, 0.5));
        let stats = app.world().resource::<MinimapStats>().clone();
        assert_eq!((stats.camera_moves, stats.drags), (2, 1));
        assert_eq!(
            app.world_mut()
                .query::<&RtsCamera>()
                .single(app.world())
                .unwrap()
                .zoom,
            zoom
        );
        // Off the node, a drag clamps to the edge.
        drag(&mut app, minimap, Vec2::new(-500.0, 595.0));
        assert_eq!(focus(&mut app).x, -63.5);

        *app.world_mut().resource_mut::<OrderMode>() = OrderMode::AttackMove;
        let before = focus(&mut app);
        press(&mut app, minimap, Vec2::new(0.25, 0.25));
        drag(&mut app, minimap, Vec2::new(115.0, 595.0));
        assert_eq!(focus(&mut app), before, "attack-move mode keeps the camera");
    }

    /// In attack-move mode a primary click issues one `AttackMove` of the
    /// own selected units to the tile's centre and disarms; the camera
    /// stays. In the normal mode a primary click issues nothing.
    #[test]
    fn primary_click_in_attack_move_mode_issues_attack_move() {
        let sim = walkthrough();
        let own = own_units(&sim);
        let (mut app, minimap) = input_app(sim);
        app.world_mut()
            .resource_mut::<Selection>()
            .set(own.iter().copied());
        click(
            &mut app,
            minimap,
            Vec2::new(0.0, 0.0),
            PointerButton::Primary,
        );
        assert!(queued(&mut app).is_empty(), "normal mode: nothing");

        *app.world_mut().resource_mut::<OrderMode>() = OrderMode::AttackMove;
        let before = focus(&mut app);
        click(
            &mut app,
            minimap,
            Vec2::new(0.0, -0.25),
            PointerButton::Primary,
        );
        assert_eq!(
            queued(&mut app),
            vec![Command::AttackMove {
                units: own,
                target: tile_centre(64, 32),
            }]
        );
        assert_eq!(*app.world().resource::<OrderMode>(), OrderMode::Normal);
        assert_eq!(focus(&mut app), before);
    }

    /// A right click orders the own selected units (an enemy unit in the
    /// selection is left out) to the tile's centre; Shift queues it; in
    /// attack-move mode it issues nothing (the world's right click cancels
    /// that mode).
    #[test]
    fn right_click_orders_own_selected_units_to_the_tile() {
        let sim = walkthrough();
        let own = own_units(&sim);
        let enemy = sim
            .view()
            .units()
            .find(|u| u.owner != LOCAL_PLAYER)
            .unwrap()
            .id;
        let (mut app, minimap) = input_app(sim);
        app.world_mut()
            .resource_mut::<Selection>()
            .set(own.iter().copied().chain([enemy]));
        click(
            &mut app,
            minimap,
            Vec2::new(-0.5, -0.5),
            PointerButton::Secondary,
        );
        assert_eq!(
            queued(&mut app),
            vec![Command::Move {
                units: own.clone(),
                target: tile_centre(0, 0),
                queue: false,
            }]
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ShiftLeft);
        click(
            &mut app,
            minimap,
            Vec2::new(0.1, 0.2),
            PointerButton::Secondary,
        );
        assert_eq!(
            queued(&mut app),
            vec![Command::Move {
                units: own,
                target: tile_centre(76, 89),
                queue: true,
            }]
        );
        *app.world_mut().resource_mut::<OrderMode>() = OrderMode::AttackMove;
        click(&mut app, minimap, Vec2::ZERO, PointerButton::Secondary);
        assert!(queued(&mut app).is_empty());
    }

    /// With an own building selected a right click sets its rally point at
    /// the tile's centre; with another player's building it issues nothing.
    #[test]
    fn right_click_with_a_building_selected_sets_its_rally() {
        let sim = walkthrough();
        let (own, enemy) = {
            let view = sim.view();
            let find = |p: PlayerId| view.buildings().find(|b| b.owner == p).unwrap().id;
            (find(LOCAL_PLAYER), find(PlayerId(1)))
        };
        let (mut app, minimap) = input_app(sim);
        app.world_mut()
            .resource_mut::<Selection>()
            .set_building(own);
        click(&mut app, minimap, Vec2::ZERO, PointerButton::Secondary);
        assert_eq!(
            queued(&mut app),
            vec![Command::SetRally {
                building: own,
                target: tile_centre(64, 64),
            }]
        );
        app.world_mut()
            .resource_mut::<Selection>()
            .set_building(enemy);
        click(&mut app, minimap, Vec2::ZERO, PointerButton::Secondary);
        assert!(queued(&mut app).is_empty());
    }

    /// Nothing is issued with no selection, with only another player's
    /// units, while the local slot is a bot (`--spectate`) or after the
    /// outcome; the press handler itself still moves the camera in each
    /// case. (In the app the outcome overlay covers the minimap, so that
    /// last press never reaches it; this test triggers on the minimap
    /// entity directly, past picking.)
    #[test]
    fn right_click_with_no_own_units_or_after_match_over_issues_nothing() {
        let sim = walkthrough();
        let own = own_units(&sim);
        let enemy: Vec<UnitId> = sim
            .view()
            .units()
            .filter(|u| u.owner != LOCAL_PLAYER)
            .map(|u| u.id)
            .collect();
        let (mut app, minimap) = input_app(sim);
        let right = |app: &mut App| {
            click(app, minimap, Vec2::ZERO, PointerButton::Secondary);
            queued(app)
        };
        assert!(right(&mut app).is_empty(), "no selection");
        app.world_mut().resource_mut::<Selection>().set(enemy);
        assert!(right(&mut app).is_empty(), "only enemy units");

        app.world_mut()
            .resource_mut::<Selection>()
            .set(own.iter().copied());
        app.world_mut().insert_resource(LocalSeat { human: false });
        assert!(right(&mut app).is_empty(), "spectating");
        *app.world_mut().resource_mut::<OrderMode>() = OrderMode::AttackMove;
        click(&mut app, minimap, Vec2::ZERO, PointerButton::Primary);
        assert!(queued(&mut app).is_empty(), "no attack-move either");
        *app.world_mut().resource_mut::<OrderMode>() = OrderMode::Normal;
        app.world_mut().insert_resource(LocalSeat { human: true });
        assert_eq!(right(&mut app).len(), 1, "the positive control");

        app.world_mut().insert_resource(MatchOver {
            result: sim::MatchResult {
                outcome: sim::Outcome::Draw,
                reason: sim::OutcomeReason::Territory,
                tick: 1,
            },
            hash: 0,
            replay: None,
        });
        assert!(right(&mut app).is_empty(), "after the outcome");
        let before = focus(&mut app);
        press(&mut app, minimap, Vec2::new(-0.25, 0.25));
        assert_ne!(
            focus(&mut app),
            before,
            "the handler does not gate camera moves on MatchOver"
        );
    }

    /// One repaint per change of tick, camera, selection or territory, and
    /// none otherwise.
    #[test]
    fn minimap_repaints_only_on_change() {
        let (mut app, _) = input_app(walkthrough());
        let repaints = |app: &App| app.world().resource::<MinimapStats>().repaints;
        app.update();
        let first = repaints(&app);
        assert!(first >= 1);
        app.update();
        app.update();
        assert_eq!(repaints(&app), first, "nothing changed");

        app.world_mut()
            .query::<&mut RtsCamera>()
            .single_mut(app.world_mut())
            .unwrap()
            .focus
            .x += 3.0;
        app.update();
        assert_eq!(repaints(&app), first + 1, "the camera moved");
        app.update();
        assert_eq!(repaints(&app), first + 1);

        let unit = own_units(app.world().non_send::<SimHandle>())[0];
        app.world_mut().resource_mut::<Selection>().set([unit]);
        app.update();
        assert_eq!(repaints(&app), first + 2, "the selection changed");

        app.world_mut().non_send_mut::<SimHandle>().step_once();
        app.update();
        assert_eq!(repaints(&app), first + 3, "a tick passed");
        app.update();
        assert_eq!(repaints(&app), first + 3);
        let stats = app.world().resource::<MinimapStats>();
        assert_eq!(stats.paint_us.len(), stats.repaints as usize);
        assert!(
            stats.line().starts_with("minimap repaints="),
            "{}",
            stats.line()
        );
    }

    /// The HUD laid out at 1280 x 800 with Inter (`ui_test`: Bevy's own
    /// text measure and UI layout over the real nodes that
    /// `economy_hud::spawn_economy_hud`, `markers::spawn_annex_readout` and
    /// [`spawn_minimap`] spawn): every text the message line or the
    /// annexation readout can show, at its longest and shaped with its own
    /// `TextFont`, is laid out clear of the minimap's frame, which spans
    /// their rows. A change to either line's node (its centring, its
    /// width, its bottom) or to the frame's is laid out here too.
    #[test]
    fn minimap_overlaps_no_hud_text() {
        use crate::economy_hud::{MessageLine, spawn_economy_hud};
        use crate::hud::HUD_FONT_PX;
        use crate::markers::{AnnexReadout, spawn_annex_readout};
        use crate::ui_test::{layout_app, rect, settle, single};
        let (w, h) = (1280.0, 800.0);
        let mut app = layout_app(UVec2::new(1280, 800));
        app.world_mut().insert_non_send(walkthrough());
        app.add_systems(
            Startup,
            (
                spawn_hud,
                spawn_economy_hud,
                spawn_annex_readout,
                spawn_minimap,
            )
                .chain(),
        );
        settle(&mut app);
        let frame_entity = single::<MinimapFrame>(&mut app);
        let frame = rect(&app, frame_entity);
        let placed = Rect::new(
            MINIMAP_LEFT_PX,
            h - MINIMAP_BOTTOM_PX - MINIMAP_FRAME_PX,
            MINIMAP_LEFT_PX + MINIMAP_FRAME_PX,
            h - MINIMAP_BOTTOM_PX,
        );
        assert!(
            (frame.min - placed.min).length() < 0.01 && (frame.max - placed.max).length() < 0.01,
            "the frame is laid out at {frame:?}, its constants say {placed:?}"
        );
        assert!(frame.min.x >= crate::camera::EDGE_SCROLL_MARGIN_PX);
        assert!(frame.max.y <= h - BOTTOM_BAR_HEIGHT_PX, "above the bar");
        let message = single::<MessageLine>(&mut app);
        let annex = single::<AnnexReadout>(&mut app);

        let rules = rules();
        let strings = rules::strings::Strings::load(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"),
            "en",
        )
        .unwrap();
        let longest = |names: Vec<&str>| {
            names
                .into_iter()
                .max_by(|a, b| {
                    crate::fonts::measure::text_width_px(a, HUD_FONT_PX)
                        .total_cmp(&crate::fonts::measure::text_width_px(b, HUD_FONT_PX))
                })
                .unwrap_or("")
                .to_string()
        };
        let building = longest(rules.buildings.iter().map(|k| k.name.as_str()).collect());
        let name = longest(
            rules
                .buildings
                .iter()
                .map(|k| k.name.as_str())
                .chain(rules.units.iter().map(|k| k.name.as_str()))
                .chain(rules.techs.iter().map(|t| t.name.as_str()))
                .chain(rules.ages.iter().map(|a| a.name.as_str()))
                .collect(),
        );
        let unit = longest(rules.units.iter().map(|k| k.name.as_str()).collect());
        let resource = longest(
            rules
                .resources
                .resources
                .iter()
                .map(|r| r.name.as_str())
                .collect(),
        );
        let cost = rules
            .resources
            .resources
            .iter()
            .map(|r| format!("9999 {}", r.name))
            .collect::<Vec<_>>()
            .join(" ");
        let fill = |template: &str| {
            rules::strings::fill(
                template,
                &[
                    ("building", &building),
                    ("name", &name),
                    ("unit", &unit),
                    ("resource", &resource),
                    ("cost", &cost),
                    ("count", "9999"),
                    ("cap", "9999"),
                    ("used", "9999"),
                    ("rate", "9999"),
                    ("time", "10:00"),
                    ("key", "W"),
                    ("hash", "0x0123456789abcdef"),
                    ("seed", "18446744073709551615"),
                ],
            )
        };
        // The message line: every rejection, notice and tooltip (a greyed
        // button's click shows its tooltip there), and a rejection with
        // what it requires, as `command_card` joins them.
        let mut lines: Vec<String> = strings.reject.values().map(|t| fill(t)).collect();
        lines.extend(
            strings
                .hud
                .iter()
                .filter(|(k, _)| !k.starts_with("menu.") && !k.starts_with("outcome."))
                .map(|(_, t)| fill(t)),
        );
        let rejects: Vec<String> = strings.reject.values().map(|t| fill(t)).collect();
        let longest_reject = longest(rejects.iter().map(String::as_str).collect());
        lines.push(format!(
            "{longest_reject}. {}",
            fill(strings.hud("tooltip.requires").unwrap())
        ));
        // The readout: an attacking and a defending timer side by side.
        let readout = format!(
            "{}    {}",
            fill(strings.hud("annex.attacking").unwrap()),
            fill(strings.hud("annex.defending").unwrap())
        );
        for (entity, text) in lines
            .iter()
            .map(|l| (message, l.as_str()))
            .chain([(annex, readout.as_str())])
        {
            app.world_mut().get_mut::<Text>(entity).unwrap().0 = text.to_string();
            app.update();
            let laid = rect(&app, entity);
            // Bevy shaped it: within a few px of the sum of Inter's advances
            // (kerning narrows a little), not a stale or empty measure.
            let advances = crate::fonts::measure::text_width_px(text, HUD_FONT_PX);
            assert!(
                (laid.width() - advances).abs() < 0.05 * advances + 2.0,
                "{text:?} laid out {} px wide, its advances sum to {advances}",
                laid.width()
            );
            assert!(laid.max.x < w, "{text:?} fits the window: {laid:?}");
            assert!(
                laid.min.y < frame.max.y && laid.max.y > frame.min.y,
                "the row spans the minimap: {laid:?}"
            );
            assert!(
                laid.min.x > frame.max.x,
                "{text:?} is laid out from x {:.1}, into the minimap's frame (right edge {})",
                laid.min.x,
                frame.max.x
            );
        }
    }
}
