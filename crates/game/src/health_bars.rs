//! Health bars (M4b, `docs/design/ui.md`, "Health bars, rings and
//! markers"): two flat quads per target, a neutral track
//! ([`palette::BAR_BACK`]) and a fill in the owner's team colour, shown only
//! while the target is damaged or selected (owner decision 6; no
//! green-to-red, team red is the invalid hue).
//!
//! Bars are separate top-level entities keyed by [`TargetId`] (a unit's is
//! `TargetId::Unit(UnitId)`), never children of unit or building entities
//! (rule 16): unit transforms stay flat, a death re-parents nothing, and the
//! art pass may give units glTF children without touching bars. They carry
//! no `Pickable` (picking ignores them under `require_markers`) and cast and
//! receive no shadows. Every bar is the same left-anchored unit quad
//! ([`BarAssets::quad`]) with one of three unlit materials, so all bars on
//! screen are at most three batches.
//!
//! Lazy: [`sync_bar_targets`] spawns a pair when a target becomes damaged
//! or selected and despawns it when the target dies or is neither. It runs
//! only when the sim tick advanced or the selection changed, and builds the
//! units' maximum hit points once per `(owner, kind)` from
//! `SimView::unit_stats` instead of resolving modifiers per unit. Buildings
//! read `SimView::building_hp`. [`place_bars`] billboards the pairs every
//! frame in `PostUpdate` before transform propagation: a unit's bar sits
//! [`UNIT_BAR_GAP`] above its look ([`crate::present::UnitVisuals::top_y`])
//! at its interpolated position, a building's [`BUILDING_BAR_GAP`] above its
//! box, both scaled with the camera distance.
//!
//! The dev-only `BarHierarchyCheckPlugin` asserts the flat layout while
//! the game runs: any health bar or selection ring with a `ChildOf` logs an
//! error and ends the session with exit code 1, and the exit prints
//! `hierarchy_check: ok|FAIL frames=<n> bar_targets_max=<m> rings_max=<r>`
//! (ROADMAP M4b acceptance line 2).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use rules::Rules;
use sim::{BuildingId, PlayerId, SimView, TargetId, Unit, UnitId, UnitKindId};

use crate::buildings::building_transform;
use crate::camera::RtsCamera;
use crate::hud::WorldInputSet;
use crate::palette;
use crate::present::{FALLBACK_VISUAL, Interp, UnitEntities, UnitVisuals, look_top_y};
use crate::selection::Selection;
#[cfg(any(test, feature = "dev"))]
use crate::selection::SelectionRing;
use crate::sim_driver::SimHandle;

/// Bar width in metres at the reference zoom.
pub const BAR_WIDTH: f32 = 0.9;
/// Bar height in metres at the reference zoom.
pub const BAR_HEIGHT: f32 = 0.12;
/// Gap between the top of a unit's look and its bar, metres.
pub const UNIT_BAR_GAP: f32 = 0.3;
/// Gap between the top of a building's box and its bar, metres.
pub const BUILDING_BAR_GAP: f32 = 0.4;
/// The fill sits this far in front of the track, toward the camera, so the
/// two never z-fight.
pub const FILL_LIFT: f32 = 0.01;
/// Camera distance (metres) at which a bar has its nominal size.
pub const BAR_REFERENCE_ZOOM: f32 = 40.0;
/// Smallest and largest bar scale over the camera's zoom range.
pub const BAR_SCALE_RANGE: (f32, f32) = (0.75, 1.5);

/// Which of a target's two quads an entity is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BarPart {
    /// The neutral track, full width.
    Back,
    /// The team-coloured fill, as wide as the hit points left.
    Fill,
}

/// One quad of a health bar: the target it belongs to and which part.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthBar {
    /// The unit or building the bar shows.
    pub target: TargetId,
    /// Track or fill.
    pub part: BarPart,
}

/// Where a bar stands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BarAnchor {
    /// Above a unit's interpolated position, this high (its look's top).
    Unit {
        /// Height of the top of the unit's look, metres.
        top_y: f32,
    },
    /// A fixed world point (above a building's box).
    Fixed(Vec3),
}

/// What a target's bar shows: its owner, hit points and anchor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarSpec {
    /// Owner (the fill colour).
    pub owner: PlayerId,
    /// Hit points now.
    pub hp: i32,
    /// Maximum hit points.
    pub max: i32,
    /// Where the bar stands.
    pub anchor: BarAnchor,
}

impl BarSpec {
    /// The fill fraction, `hp / max` clamped to `0..=1`.
    pub fn fraction(&self) -> f32 {
        if self.max <= 0 {
            return 0.0;
        }
        (self.hp.max(0) as f32 / self.max as f32).min(1.0)
    }
}

/// A shown bar: its two entities and what it shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    /// The track entity.
    pub back: Entity,
    /// The fill entity.
    pub fill: Entity,
    /// The values the quads are placed from.
    pub spec: BarSpec,
}

/// Every shown bar by target. A target is here exactly while it is damaged
/// or selected.
#[derive(Resource, Default, Debug)]
pub struct HealthBars(pub HashMap<TargetId, Bar>);

/// The one quad and three materials every bar uses: a 1 x 1 m rectangle in
/// the XY plane facing +Z with its origin on the left edge, the track
/// material and one fill per team. All unlit, two-sided and fog-free.
#[derive(Resource, Debug, Clone)]
pub struct BarAssets {
    /// The shared left-anchored quad.
    pub quad: Handle<Mesh>,
    /// The track material ([`palette::BAR_BACK`]).
    pub back: Handle<StandardMaterial>,
    /// The fill material per team slot ([`palette::TEAM`]).
    pub fill: [Handle<StandardMaterial>; 2],
}

impl BarAssets {
    /// The fill material for `owner`'s bars.
    pub fn fill_for(&self, owner: PlayerId) -> Handle<StandardMaterial> {
        self.fill[usize::from(owner.0).min(self.fill.len() - 1)].clone()
    }
}

/// The unit quad every bar scales: `Rectangle::new(1.0, 1.0)` moved so its
/// origin is the left edge, so scaling x shortens the fill from the right.
pub fn bar_quad() -> Mesh {
    Rectangle::new(1.0, 1.0)
        .mesh()
        .build()
        .translated_by(Vec3::X * 0.5)
}

/// An unlit, two-sided, fog-free flat colour for a bar quad.
pub fn bar_material(color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: color,
        unlit: true,
        cull_mode: None,
        fog_enabled: false,
        ..default()
    }
}

/// Health bars for damaged or selected units and buildings.
pub struct HealthBarsPlugin;

impl Plugin for HealthBarsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HealthBars>()
            .add_systems(Startup, create_bar_assets)
            .add_systems(Update, sync_bar_targets.after(WorldInputSet))
            .add_systems(PostUpdate, place_bars.before(TransformSystems::Propagate));
    }
}

fn create_bar_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(BarAssets {
        quad: meshes.add(bar_quad()),
        back: materials.add(bar_material(palette::BAR_BACK)),
        fill: palette::TEAM.map(|c| materials.add(bar_material(c))),
    });
}

/// Maximum hit points per `(owner, kind)`, filled on first use from
/// `SimView::unit_stats` (the `UnitHp` modifier rows, so Arms raises it):
/// sixteen calls a tick for two players and eight kinds, instead of one
/// modifier resolution per unit.
#[derive(Debug, Default)]
pub struct MaxHpTable(BTreeMap<(PlayerId, UnitKindId), i32>);

impl MaxHpTable {
    /// `u`'s hit points now and its maximum, `(hp, max)`, as
    /// `SimView::unit_hp` gives them.
    pub fn hp(&mut self, view: &SimView<'_>, u: &Unit) -> (i32, i32) {
        let max = *self
            .0
            .entry((u.owner, u.kind))
            .or_insert_with(|| view.unit_stats(u.owner, u.kind).max_hp);
        (max - u.damage_taken, max)
    }

    /// How many `(owner, kind)` maxima were resolved.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` before the first lookup.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The targets that carry a bar now and what each shows: every unit that
/// has taken damage or is selected, every building below its kind's hit
/// points or selected. `top_y` gives the height of a unit kind's look.
pub fn bar_specs(
    view: &SimView<'_>,
    rules: &Rules,
    selection: &Selection,
    top_y: impl Fn(UnitKindId) -> f32,
) -> BTreeMap<TargetId, BarSpec> {
    let mut table = MaxHpTable::default();
    let mut specs = BTreeMap::new();
    for u in view.units() {
        if u.damage_taken <= 0 && !selection.contains(u.id) {
            continue;
        }
        let (hp, max) = table.hp(view, u);
        specs.insert(
            TargetId::Unit(u.id),
            BarSpec {
                owner: u.owner,
                hp,
                max,
                anchor: BarAnchor::Unit {
                    top_y: top_y(u.kind),
                },
            },
        );
    }
    for b in view.buildings() {
        let Some((hp, max)) = view.building_hp(b.id) else {
            continue;
        };
        if hp >= max && selection.building != Some(b.id) {
            continue;
        }
        // A site's box rises with its progress; the bar rides on top.
        let t = building_transform(rules, b);
        specs.insert(
            TargetId::Building(b.id),
            BarSpec {
                owner: b.owner,
                hp,
                max,
                anchor: BarAnchor::Fixed(t.translation + Vec3::Y * (t.scale.y + BUILDING_BAR_GAP)),
            },
        );
    }
    specs
}

/// Height of the top of `kind`'s look: the presenter's cached value, or
/// computed from the rules when the kind's mesh was not built yet.
fn unit_top_y(visuals: &UnitVisuals, rules: &Rules, kind: UnitKindId) -> f32 {
    visuals.top_y(kind.0).unwrap_or_else(|| {
        let radius = rules
            .unit_kind(kind.0)
            .map_or(0.35, |k| k.radius_tiles_x100 as f32 / 100.0);
        look_top_y(rules.visual(kind.0).unwrap_or(&FALLBACK_VISUAL), radius)
    })
}

/// The tick and selection [`sync_bar_targets`] last ran for.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncedFor {
    tick: u32,
    units: BTreeSet<UnitId>,
    building: Option<BuildingId>,
}

/// After a tick or a selection change: spawn a bar pair for every target
/// that became damaged or selected (top-level entities with the shared
/// quad, `NotShadowCaster`, `NotShadowReceiver`, no `Pickable`, no
/// `ChildOf`), update the values of the others (and the fill's material
/// when an annexation changed the owner), and despawn the pairs of targets
/// that died or are neither damaged nor selected.
pub fn sync_bar_targets(
    sim: NonSend<SimHandle>,
    selection: Res<Selection>,
    visuals: Res<UnitVisuals>,
    assets: Option<Res<BarAssets>>,
    mut bars: ResMut<HealthBars>,
    mut synced: Local<Option<SyncedFor>>,
    mut commands: Commands,
) {
    let Some(assets) = assets else {
        return;
    };
    let tick = sim.tick();
    if synced.as_ref().is_some_and(|s| {
        s.tick == tick && s.building == selection.building && s.units == selection.units
    }) {
        return;
    }
    *synced = Some(SyncedFor {
        tick,
        units: selection.units.clone(),
        building: selection.building,
    });
    let view = sim.view();
    let rules = sim.rules();
    let specs = bar_specs(&view, rules, &selection, |k| unit_top_y(&visuals, rules, k));
    bars.0.retain(|target, bar| {
        if specs.contains_key(target) {
            true
        } else {
            commands.entity(bar.back).try_despawn();
            commands.entity(bar.fill).try_despawn();
            false
        }
    });
    for (target, spec) in specs {
        match bars.0.get_mut(&target) {
            Some(bar) => {
                // Annexation (M5a) hands a damaged building to a new owner
                // under the same id: its fill takes the new owner's colour.
                if bar.spec.owner != spec.owner {
                    commands
                        .entity(bar.fill)
                        .try_insert(MeshMaterial3d(assets.fill_for(spec.owner)));
                }
                bar.spec = spec;
            }
            None => {
                let back = spawn_part(&mut commands, &assets, target, BarPart::Back, spec.owner);
                let fill = spawn_part(&mut commands, &assets, target, BarPart::Fill, spec.owner);
                bars.0.insert(target, Bar { back, fill, spec });
            }
        }
    }
}

/// One quad of a bar, hidden until [`place_bars`] puts it in place.
fn spawn_part(
    commands: &mut Commands,
    assets: &BarAssets,
    target: TargetId,
    part: BarPart,
    owner: PlayerId,
) -> Entity {
    let material = match part {
        BarPart::Back => assets.back.clone(),
        BarPart::Fill => assets.fill_for(owner),
    };
    commands
        .spawn((
            Name::new(format!("Health bar {part:?} {target:?}")),
            HealthBar { target, part },
            Mesh3d(assets.quad.clone()),
            MeshMaterial3d(material),
            Transform::default(),
            Visibility::Hidden,
            NotShadowCaster,
            NotShadowReceiver,
        ))
        .id()
}

/// Bar scale for a camera `zoom` metres from its focus: 1 at
/// [`BAR_REFERENCE_ZOOM`], clamped to [`BAR_SCALE_RANGE`], so bars stay
/// readable zoomed out without covering units zoomed in.
pub fn bar_scale(zoom: f32) -> f32 {
    (zoom / BAR_REFERENCE_ZOOM).clamp(BAR_SCALE_RANGE.0, BAR_SCALE_RANGE.1)
}

/// The two quads of one bar, placed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarLayout {
    /// The track's transform.
    pub back: Transform,
    /// The fill's transform.
    pub fill: Transform,
    /// Whether the fill is drawn: not at zero hit points, where it is
    /// hidden rather than scaled to nothing.
    pub fill_visible: bool,
}

/// Billboard a bar centred on `anchor`, facing the camera (`camera_rotation`,
/// so the quad's +Z points back at it and its +X is screen right), `scale`
/// times its nominal size, filled to `fraction`. The fill sits
/// [`FILL_LIFT`] toward the camera; at a fraction of 0 it keeps the track's
/// width and is hidden.
pub fn bar_layout(anchor: Vec3, camera_rotation: Quat, scale: f32, fraction: f32) -> BarLayout {
    let (w, h) = (BAR_WIDTH * scale, BAR_HEIGHT * scale);
    let right = camera_rotation * Vec3::X;
    let toward_camera = camera_rotation * Vec3::Z;
    let left = anchor - right * (w * 0.5);
    let fill_visible = fraction > 0.0;
    let fill_w = if fill_visible {
        w * fraction.min(1.0)
    } else {
        w
    };
    BarLayout {
        back: Transform {
            translation: left,
            rotation: camera_rotation,
            scale: Vec3::new(w, h, 1.0),
        },
        fill: Transform {
            translation: left + toward_camera * FILL_LIFT,
            rotation: camera_rotation,
            scale: Vec3::new(fill_w, h, 1.0),
        },
        fill_visible,
    }
}

/// Every frame, in `PostUpdate` before transform propagation: place each
/// bar over its target ([`bar_layout`]) facing the RTS camera, a unit's at
/// its interpolated position (the same `Interp::sample` its mesh uses).
/// Writes only what changed.
pub fn place_bars(
    bars: Res<HealthBars>,
    entities: Res<UnitEntities>,
    interps: Query<&Interp>,
    fixed: Res<Time<Fixed>>,
    camera: Query<(&Transform, &RtsCamera), Without<HealthBar>>,
    mut parts: Query<(&mut Transform, &mut Visibility), With<HealthBar>>,
) {
    let Ok((camera_transform, rts)) = camera.single() else {
        return;
    };
    let t = fixed.overstep_fraction();
    let scale = bar_scale(rts.zoom);
    for (target, bar) in &bars.0 {
        let anchor = match (bar.spec.anchor, target) {
            (BarAnchor::Fixed(at), _) => at,
            (BarAnchor::Unit { top_y }, TargetId::Unit(id)) => {
                let Some(interp) = entities.0.get(id).and_then(|e| interps.get(*e).ok()) else {
                    continue;
                };
                interp.sample(t).0 + Vec3::Y * (top_y + UNIT_BAR_GAP)
            }
            (BarAnchor::Unit { .. }, TargetId::Building(_)) => continue,
        };
        let layout = bar_layout(
            anchor,
            camera_transform.rotation,
            scale,
            bar.spec.fraction(),
        );
        if let Ok((mut transform, mut visibility)) = parts.get_mut(bar.back) {
            transform.set_if_neq(layout.back);
            visibility.set_if_neq(Visibility::Inherited);
        }
        if let Ok((mut transform, mut visibility)) = parts.get_mut(bar.fill) {
            transform.set_if_neq(layout.fill);
            visibility.set_if_neq(if layout.fill_visible {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            });
        }
    }
}

/// Health bars and selection rings that have a parent: the flat-layout
/// rule says there are none. Unit entities may have children (glTF scenes
/// at the art pass); bars and rings may not have a parent.
#[cfg(any(test, feature = "dev"))]
pub type ParentedBarFilter = (Or<(With<HealthBar>, With<SelectionRing>)>, With<ChildOf>);

/// Every health bar or selection ring entity that has a `ChildOf`.
#[cfg(any(test, feature = "dev"))]
pub fn bar_hierarchy_violations(world: &mut World) -> Vec<Entity> {
    let mut query = world.query_filtered::<Entity, ParentedBarFilter>();
    query.iter(world).collect()
}

/// What the dev-only hierarchy check has seen so far.
#[cfg(any(test, feature = "dev"))]
#[derive(Resource, Debug, Default, Clone, PartialEq, Eq)]
pub struct HierarchyCheck {
    /// Frames checked.
    pub frames: u32,
    /// Most bar targets (not entities) shown in one frame.
    pub bar_targets_max: usize,
    /// Most selection ring entities in one frame.
    pub rings_max: usize,
    /// Every bar or ring entity found with a parent.
    pub violations: Vec<Entity>,
    /// The exit line was printed.
    reported: bool,
}

#[cfg(any(test, feature = "dev"))]
impl HierarchyCheck {
    /// The line printed on exit.
    pub fn line(&self) -> String {
        let verdict = if self.violations.is_empty() {
            "ok".to_string()
        } else {
            format!("FAIL violations={}", self.violations.len())
        };
        format!(
            "hierarchy_check: {verdict} frames={} bar_targets_max={} rings_max={}",
            self.frames, self.bar_targets_max, self.rings_max
        )
    }
}

/// Dev builds: every frame, in `Last` before the replay recorder finishes,
/// look for health bars and selection rings with a parent. The first one
/// found logs an error naming the entities and writes
/// `AppExit::from_code(1)` (not a panic, so the replay still gets its
/// trailer); on any exit the `hierarchy_check:` line is printed.
///
/// Both systems run after `bevy::window::ExitSystems`: the red close
/// button's `AppExit` is written there by `exit_on_all_closed`, in `Last`,
/// and the winit runner exits right after that frame, so a report that ran
/// earlier would never see it (the recorder has the same ordering). They
/// also run before `frame_stats`' exit lines, so a failing check's frame
/// still prints them. Without `WindowPlugin` (tests) the set is only a
/// label.
#[cfg(any(test, feature = "dev"))]
pub struct BarHierarchyCheckPlugin;

#[cfg(any(test, feature = "dev"))]
impl Plugin for BarHierarchyCheckPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HierarchyCheck>().add_systems(
            Last,
            (check_bar_hierarchy, report_bar_hierarchy)
                .chain()
                .after(bevy::window::ExitSystems)
                .before(crate::frame_stats::print_on_exit)
                .before(crate::sim_driver::finish_recorder_on_exit),
        );
    }
}

#[cfg(any(test, feature = "dev"))]
fn check_bar_hierarchy(
    parented: Query<Entity, ParentedBarFilter>,
    bars: Option<Res<HealthBars>>,
    rings: Query<(), With<SelectionRing>>,
    mut check: ResMut<HierarchyCheck>,
    mut exit: MessageWriter<AppExit>,
) {
    check.frames += 1;
    let targets = bars.map_or(0, |b| b.0.len());
    check.bar_targets_max = check.bar_targets_max.max(targets);
    check.rings_max = check.rings_max.max(rings.iter().count());
    let found: Vec<Entity> = parented.iter().collect();
    if found.is_empty() {
        return;
    }
    error!("hierarchy_check: health bars or selection rings with a parent: {found:?}");
    if check.violations.is_empty() {
        exit.write(AppExit::from_code(1));
    }
    for e in found {
        if !check.violations.contains(&e) {
            check.violations.push(e);
        }
    }
}

#[cfg(any(test, feature = "dev"))]
fn report_bar_hierarchy(mut exits: MessageReader<AppExit>, mut check: ResMut<HierarchyCheck>) {
    if exits.is_empty() {
        return;
    }
    exits.clear();
    if !check.reported {
        check.reported = true;
        println!("{}", check.line());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buildings::BuildingsPlugin;
    use crate::present::PresentPlugin;
    use crate::sim_driver::{LOCAL_PLAYER, ScenarioInput, SimPlugin};
    use sim::{Command, FxVec2, PlayerCommand, Tile};
    use std::time::Duration;

    fn rules() -> Rules {
        Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
            .expect("rules load")
    }

    fn unit_kind(rules: &Rules, id: &str) -> UnitKindId {
        UnitKindId(u16::try_from(rules.units.iter().position(|u| u.id == id).unwrap()).unwrap())
    }

    const P1: PlayerId = PlayerId(1);

    fn spawn(
        owner: PlayerId,
        seq: u32,
        kind: UnitKindId,
        at: (i32, i32),
        count: u16,
    ) -> PlayerCommand {
        PlayerCommand::new(
            owner,
            seq,
            Command::DebugSpawn {
                owner,
                kind,
                at: FxVec2::from_ints(at.0, at.1),
                count,
            },
        )
    }

    fn attack(owner: PlayerId, seq: u32, units: &[u32], target: TargetId) -> PlayerCommand {
        PlayerCommand::new(
            owner,
            seq,
            Command::Attack {
                units: units.iter().copied().map(UnitId).collect(),
                target,
            },
        )
    }

    /// A headless app with the sim driven by `stream`, the unit and
    /// building presenters, the bars and a camera at 40 m, advanced to
    /// tick 1 (one tick per update from then on).
    fn bar_app(stream: sim::scenarios::Stream) -> App {
        let rules = rules();
        let setup = sim::MatchSetup::scenario(&rules, 7);
        let handle = SimHandle::from_setup(setup, rules);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(Assets::<Mesh>::default())
            .insert_resource(Assets::<StandardMaterial>::default())
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                Duration::ZERO,
            ))
            .init_resource::<Selection>()
            .add_plugins((
                SimPlugin::new(handle, Box::new(ScenarioInput::new(stream))),
                PresentPlugin,
                BuildingsPlugin,
                HealthBarsPlugin,
            ));
        let camera = RtsCamera {
            zoom: BAR_REFERENCE_ZOOM,
            zoom_target: BAR_REFERENCE_ZOOM,
            ..default()
        };
        let transform = Transform::from_translation(camera.eye()).looking_at(camera.focus, Vec3::Y);
        app.world_mut().spawn((camera, transform));
        app.update();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_millis(50),
        ));
        app
    }

    fn tick(app: &App) -> u32 {
        app.world().non_send::<SimHandle>().tick()
    }

    /// Update until the sim reaches `t`.
    fn run_to(app: &mut App, t: u32) {
        for _ in 0..10_000 {
            if tick(app) >= t {
                return;
            }
            app.update();
        }
        panic!("tick {t} not reached");
    }

    fn bars(app: &App) -> &HashMap<TargetId, Bar> {
        &app.world().resource::<HealthBars>().0
    }

    /// `(target, part)` of every bar entity, sorted.
    fn bar_entities(app: &mut App) -> Vec<(TargetId, BarPart, Entity)> {
        let mut q = app.world_mut().query::<(Entity, &HealthBar)>();
        let mut v: Vec<_> = q
            .iter(app.world())
            .map(|(e, b)| (b.target, b.part, e))
            .collect();
        v.sort_by_key(|(t, p, _)| (*t, *p == BarPart::Fill));
        v
    }

    fn hp_of(app: &App, target: TargetId) -> Option<(i32, i32)> {
        let sim = app.world().non_send::<SimHandle>();
        let view = sim.view();
        match target {
            TargetId::Unit(id) => view.unit_hp(id),
            TargetId::Building(id) => view.building_hp(id),
        }
    }

    /// Two P0 Skirmishers (`UnitId` 1, 2) and two P1 Skirmishers (3, 4) ten
    /// tiles apart on one row, beyond acquisition, and a P0 Watchtower
    /// (`BuildingId` 1) far from both; at tick 5 each side's pair attacks the
    /// other's front unit (2 and 3; 1 and 4 queue behind them).
    fn duel(rules: &Rules) -> sim::scenarios::Stream {
        let sk = unit_kind(rules, "skirmisher");
        let (tower, _) = rules.building_kind_by_id("watchtower").unwrap();
        vec![
            (
                0,
                vec![
                    spawn(LOCAL_PLAYER, 0, sk, (20, 30), 2),
                    PlayerCommand::new(
                        LOCAL_PLAYER,
                        1,
                        Command::DebugPlaceBuilding {
                            owner: LOCAL_PLAYER,
                            kind: sim::BuildingKindId(tower),
                            tile: Tile::new(4, 4),
                            complete: true,
                        },
                    ),
                    spawn(P1, 0, sk, (30, 30), 2),
                ],
            ),
            (
                5,
                vec![
                    attack(LOCAL_PLAYER, 2, &[1, 2], TargetId::Unit(UnitId(3))),
                    attack(P1, 1, &[3, 4], TargetId::Unit(UnitId(2))),
                ],
            ),
        ]
    }

    #[test]
    fn bars_are_top_level_entities_keyed_by_target() {
        let rules = rules();
        let mut app = bar_app(duel(&rules));
        run_to(&mut app, 5);
        assert!(bars(&app).is_empty(), "no one is damaged or selected yet");
        // Until both targets took a hit.
        let damaged = [TargetId::Unit(UnitId(2)), TargetId::Unit(UnitId(3))];
        for _ in 0..400 {
            app.update();
            if damaged.iter().all(|t| bars(&app).contains_key(t)) {
                break;
            }
        }
        let shown: BTreeSet<TargetId> = bars(&app).keys().copied().collect();
        assert_eq!(
            shown,
            damaged.into_iter().collect(),
            "exactly the damaged ones"
        );
        // One track and one fill per target, as the map says, all roots.
        let entities = bar_entities(&mut app);
        assert_eq!(entities.len(), 2 * damaged.len());
        for (target, part, e) in &entities {
            let bar = bars(&app)[target];
            let want = match part {
                BarPart::Back => bar.back,
                BarPart::Fill => bar.fill,
            };
            assert_eq!(*e, want, "{target:?} {part:?}");
            assert!(
                app.world().get::<ChildOf>(*e).is_none(),
                "{e:?} has a parent"
            );
            assert!(
                app.world().get::<Pickable>(*e).is_none(),
                "bars are not pickable"
            );
            assert!(app.world().get::<NotShadowCaster>(*e).is_some());
        }
        assert!(bar_hierarchy_violations(app.world_mut()).is_empty());
        // Placed over the unit, at the camera's rotation, and shown.
        let unit = app.world().resource::<UnitEntities>().0[&UnitId(2)];
        let unit_pos = app.world().get::<Transform>(unit).unwrap().translation;
        let back = bars(&app)[&damaged[0]].back;
        let back_tf = *app.world().get::<Transform>(back).unwrap();
        assert!(
            back_tf.translation.y > 1.5,
            "above the 1.5 m cone: {back_tf:?}"
        );
        assert!(
            (back_tf.translation.x + BAR_WIDTH / 2.0 - unit_pos.x).abs() < 0.3,
            "centred over the unit: {back_tf:?} vs {unit_pos}"
        );
        assert_eq!(
            app.world().get::<Visibility>(back),
            Some(&Visibility::Inherited)
        );
    }

    #[test]
    fn hierarchy_check_flags_a_parented_bar() {
        let rules = rules();
        let mut app = bar_app(duel(&rules));
        run_to(&mut app, 5);
        for _ in 0..400 {
            app.update();
            if !bars(&app).is_empty() {
                break;
            }
        }
        assert!(bar_hierarchy_violations(app.world_mut()).is_empty());
        // Positive control: parent one bar under its unit.
        let (&target, bar) = bars(&app).iter().next().expect("a bar");
        let back = bar.back;
        let TargetId::Unit(id) = target else {
            panic!("a unit's bar")
        };
        let unit = app.world().resource::<UnitEntities>().0[&id];
        app.world_mut().entity_mut(back).insert(ChildOf(unit));
        assert_eq!(bar_hierarchy_violations(app.world_mut()), vec![back]);

        // The dev plugin: clean frames pass, a parented bar or ring exits 1.
        let mut check = App::new();
        check
            .add_plugins(MinimalPlugins)
            .add_plugins(BarHierarchyCheckPlugin);
        let parent = check.world_mut().spawn_empty().id();
        check.world_mut().spawn(HealthBar {
            target: TargetId::Unit(UnitId(1)),
            part: BarPart::Back,
        });
        check.world_mut().spawn(SelectionRing(UnitId(1)));
        check.update();
        assert_eq!(check.should_exit(), None);
        assert_eq!(check.world().resource::<HierarchyCheck>().rings_max, 1);
        let ring = check
            .world_mut()
            .spawn((SelectionRing(UnitId(2)), ChildOf(parent)))
            .id();
        check.update();
        assert_eq!(check.should_exit(), Some(AppExit::from_code(1)));
        let state = check.world().resource::<HierarchyCheck>().clone();
        assert_eq!(state.violations, vec![ring]);
        assert_eq!(state.frames, 2);
        assert!(
            state
                .line()
                .starts_with("hierarchy_check: FAIL violations=1 frames=2"),
            "{}",
            state.line()
        );
        assert!(
            HierarchyCheck::default()
                .line()
                .starts_with("hierarchy_check: ok frames=0")
        );
    }

    /// The red close button: `bevy_window::exit_on_all_closed` writes
    /// `AppExit` in `Last`, inside `ExitSystems`, and the winit runner
    /// exits right after that frame, so the check must print in that same
    /// frame. A `WindowPlugin` with no window stands in for the closed one:
    /// `exit_on_all_closed` exits on the first update.
    #[test]
    fn hierarchy_check_reports_when_the_window_closes() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(bevy::window::WindowPlugin {
                primary_window: None,
                ..default()
            })
            .add_plugins(BarHierarchyCheckPlugin);
        app.update();
        assert_eq!(
            app.should_exit(),
            Some(AppExit::Success),
            "no window open: exit_on_all_closed exits"
        );
        let check = app.world().resource::<HierarchyCheck>();
        assert_eq!(check.frames, 1);
        assert!(check.reported, "the line was printed: {}", check.line());
    }

    /// A failed check ends the session; the frame-time lines still print on
    /// that frame, after the check wrote its `AppExit`.
    #[test]
    fn a_failed_check_still_prints_the_frame_stats_lines() {
        let rules = rules();
        let handle = SimHandle::from_setup(sim::MatchSetup::scenario(&rules, 7), rules);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins((
            // The window messages frame_stats reads, and no exit of its own.
            bevy::window::WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            },
            SimPlugin::new(handle, Box::new(crate::sim_driver::LocalInput)),
            BarHierarchyCheckPlugin,
            crate::frame_stats::FrameStatsPlugin {
                arrival: None,
                fight: false,
            },
        ));
        let parent = app.world_mut().spawn_empty().id();
        app.world_mut()
            .spawn((SelectionRing(UnitId(1)), ChildOf(parent)));
        app.update();
        assert_eq!(app.should_exit(), Some(AppExit::from_code(1)));
        assert!(app.world().resource::<HierarchyCheck>().reported);
        assert!(
            app.world()
                .resource::<crate::frame_stats::FrameStats>()
                .printed(),
            "frame_stats printed on the failing frame"
        );
    }

    #[test]
    fn bar_fill_tracks_hp_and_despawns_with_its_target() {
        let rules = rules();
        let mut app = bar_app(duel(&rules));
        run_to(&mut app, 5);
        let target = TargetId::Unit(UnitId(3));
        let mut seen = Vec::new();
        let mut entities = None;
        for _ in 0..2000 {
            app.update();
            match (bars(&app).get(&target), hp_of(&app, target)) {
                (Some(bar), Some(hp)) => {
                    assert_eq!((bar.spec.hp, bar.spec.max), hp, "the sim's hit points");
                    // The fill quad itself, as `place_bars` left it this
                    // frame: as wide as the hit points left, and drawn.
                    let fill = *app.world().get::<Transform>(bar.fill).unwrap();
                    let want =
                        BAR_WIDTH * bar_scale(BAR_REFERENCE_ZOOM) * hp.0 as f32 / hp.1 as f32;
                    assert!(
                        (fill.scale.x - want).abs() < 1e-5,
                        "fill at {}/{}: {} wide, want {want}",
                        hp.0,
                        hp.1,
                        fill.scale.x
                    );
                    assert_eq!(
                        app.world().get::<Visibility>(bar.fill),
                        Some(&Visibility::Inherited)
                    );
                    if seen.last() != Some(&hp.0) {
                        seen.push(hp.0);
                    }
                    entities = Some((bar.back, bar.fill));
                }
                (None, Some((hp, max))) => assert_eq!(hp, max, "undamaged has no bar"),
                (_, None) => break,
            }
        }
        assert!(hp_of(&app, target).is_none(), "unit 3 died");
        assert!(seen.len() >= 3, "several hits seen: {seen:?}");
        assert!(seen.windows(2).all(|w| w[1] < w[0]), "falling: {seen:?}");
        // One more frame: the despawn commands of the death tick applied.
        app.update();
        assert!(!bars(&app).contains_key(&target));
        let (back, fill) = entities.expect("the bar existed");
        assert!(app.world().get_entity(back).is_err(), "track despawned");
        assert!(app.world().get_entity(fill).is_err(), "fill despawned");
    }

    /// At zero hit points (a Town held at 0) `place_bars` hides the fill
    /// quad and leaves it as wide as the track, never scaled to nothing.
    #[test]
    fn place_bars_hides_an_empty_fill() {
        use bevy::ecs::system::RunSystemOnce;
        let rules = rules();
        let mut app = bar_app(duel(&rules));
        run_to(&mut app, 3);
        app.world_mut()
            .resource_mut::<Selection>()
            .set_building(BuildingId(1));
        app.update();
        let target = TargetId::Building(BuildingId(1));
        let bar = bars(&app)[&target];
        assert_eq!(
            app.world().get::<Visibility>(bar.fill),
            Some(&Visibility::Inherited),
            "a full bar shows its fill"
        );
        app.world_mut()
            .resource_mut::<HealthBars>()
            .0
            .get_mut(&target)
            .unwrap()
            .spec
            .hp = 0;
        app.world_mut().run_system_once(place_bars).unwrap();
        assert_eq!(
            app.world().get::<Visibility>(bar.fill),
            Some(&Visibility::Hidden)
        );
        let back = app.world().get::<Transform>(bar.back).unwrap().scale.x;
        let fill = app.world().get::<Transform>(bar.fill).unwrap().scale.x;
        assert!(back > 0.0);
        assert_eq!(fill, back, "the track's width, not zero");
        assert_eq!(
            app.world().get::<Visibility>(bar.back),
            Some(&Visibility::Inherited),
            "the track still shows"
        );
    }

    #[test]
    fn the_fill_is_as_wide_as_the_hit_points_left() {
        let anchor = Vec3::new(3.0, 2.0, -4.0);
        let rotation = Transform::from_xyz(0.0, 30.0, 20.0)
            .looking_at(Vec3::ZERO, Vec3::Y)
            .rotation;
        let full = bar_layout(anchor, rotation, 1.0, 1.0);
        let half = bar_layout(anchor, rotation, 1.0, 0.5);
        assert_eq!(full.back, half.back, "the track does not change");
        assert!((full.fill.scale.x - BAR_WIDTH).abs() < 1e-6);
        assert!((half.fill.scale.x - BAR_WIDTH / 2.0).abs() < 1e-6);
        // Left-anchored: both fills start where the track starts, a little
        // toward the camera.
        let toward = rotation * Vec3::Z;
        assert!(
            (half.fill.translation - (half.back.translation + toward * FILL_LIFT)).length() < 1e-6
        );
        // Centred: the quad's left edge is half a width left of the anchor.
        let right = rotation * Vec3::X;
        assert!((full.back.translation + right * BAR_WIDTH / 2.0 - anchor).length() < 1e-5);
        // Facing the camera: the quad's normal is the camera's back vector.
        assert!((full.back.rotation * Vec3::Z - toward).length() < 1e-6);
        // Zoom scales the bar within its range.
        assert_eq!(bar_scale(BAR_REFERENCE_ZOOM), 1.0);
        assert_eq!(bar_scale(15.0), BAR_SCALE_RANGE.0);
        assert_eq!(bar_scale(60.0), 1.5);
        assert!(
            (bar_layout(anchor, rotation, 1.5, 1.0).back.scale.x - 1.5 * BAR_WIDTH).abs() < 1e-6
        );
    }

    #[test]
    fn zero_fill_is_hidden_not_scaled() {
        let rotation = Quat::from_rotation_x(-0.9);
        let empty = bar_layout(Vec3::ZERO, rotation, 1.0, 0.0);
        assert!(!empty.fill_visible);
        assert!(
            empty.fill.scale.x > 0.0,
            "never scaled to nothing: {:?}",
            empty.fill
        );
        assert_eq!(empty.fill.scale.x, empty.back.scale.x);
        assert!(bar_layout(Vec3::ZERO, rotation, 1.0, 0.01).fill_visible);
        let spec = |hp| BarSpec {
            owner: LOCAL_PLAYER,
            hp,
            max: 500,
            anchor: BarAnchor::Fixed(Vec3::ZERO),
        };
        assert_eq!(spec(0).fraction(), 0.0);
        assert_eq!(spec(-30).fraction(), 0.0, "overkill clamps");
        assert_eq!(spec(250).fraction(), 0.5);
        assert_eq!(spec(500).fraction(), 1.0);
    }

    #[test]
    fn bars_show_when_damaged_or_selected_only() {
        let rules = rules();
        let mut app = bar_app(duel(&rules));
        run_to(&mut app, 3);
        assert!(bars(&app).is_empty(), "nothing damaged, nothing selected");
        // Freeze the clock: the next updates change only the selection, so
        // the bars follow it without a tick.
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::ZERO,
        ));
        let frozen = tick(&app);
        // Selecting an undamaged unit (own or enemy) shows a full bar.
        app.world_mut()
            .resource_mut::<Selection>()
            .set([UnitId(2), UnitId(4)]);
        app.update();
        assert_eq!(tick(&app), frozen, "no tick: the selection alone");
        let shown: BTreeSet<TargetId> = bars(&app).keys().copied().collect();
        assert_eq!(
            shown,
            [TargetId::Unit(UnitId(2)), TargetId::Unit(UnitId(4))]
                .into_iter()
                .collect()
        );
        let bar = bars(&app)[&TargetId::Unit(UnitId(4))];
        assert_eq!(bar.spec.hp, bar.spec.max);
        assert_eq!(bar.spec.owner, P1);
        // Deselected again: gone.
        app.world_mut().resource_mut::<Selection>().clear();
        app.update();
        assert_eq!(tick(&app), frozen);
        assert!(bars(&app).is_empty());
        // A selected building shows one too, at full hit points.
        app.world_mut()
            .resource_mut::<Selection>()
            .set_building(BuildingId(1));
        app.update();
        assert_eq!(tick(&app), frozen);
        let shown: Vec<TargetId> = bars(&app).keys().copied().collect();
        assert_eq!(shown, [TargetId::Building(BuildingId(1))]);
        let bar = bars(&app)[&TargetId::Building(BuildingId(1))];
        assert_eq!((bar.spec.hp, bar.spec.max), (500, 500));
        app.world_mut().resource_mut::<Selection>().clear();
        // One tick per update again for the fight.
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_millis(50),
        ));
        // The fight damages units 2 and 3: their bars appear unselected.
        for _ in 0..400 {
            app.update();
            if bars(&app).len() == 2 {
                break;
            }
        }
        let shown: BTreeSet<TargetId> = bars(&app).keys().copied().collect();
        assert_eq!(
            shown,
            [TargetId::Unit(UnitId(2)), TargetId::Unit(UnitId(3))]
                .into_iter()
                .collect()
        );
    }

    /// A short real fight on the sim alone: after every tick, the table's
    /// value for every unit equals `SimView::unit_hp`, Arms rows included.
    #[test]
    fn max_hp_table_matches_unit_hp() {
        struct NoAi;
        impl sim::AiController for NoAi {
            fn think(&mut self, _: PlayerId, _: &SimView<'_>) -> Vec<Command> {
                Vec::new()
            }
        }
        let rules = rules();
        let arms = sim::TechId(rules.tech_by_id("arms_1").unwrap().0);
        let mut stream = duel(&rules);
        stream[0].1.push(PlayerCommand::new(
            P1,
            1,
            Command::DebugResearch {
                owner: P1,
                tech: arms,
            },
        ));
        stream[1].1[1].seq = 2;
        let setup = sim::MatchSetup::scenario(&rules, 7);
        let mut sim = sim::Sim::new(setup, rules, Box::new(NoAi));
        let mut checked = 0;
        for t in 0..300 {
            sim.step(sim::scenarios::commands_at(&stream, t));
            let view = sim.view();
            let mut table = MaxHpTable::default();
            for u in view.units() {
                assert_eq!(
                    Some(table.hp(&view, u)),
                    view.unit_hp(u.id),
                    "unit {:?} at {t}",
                    u.id
                );
                checked += 1;
            }
            assert!(table.len() <= 2, "one entry per (owner, kind)");
        }
        assert!(checked > 600);
    }

    #[test]
    fn arms_raises_the_bar_max() {
        struct NoAi;
        impl sim::AiController for NoAi {
            fn think(&mut self, _: PlayerId, _: &SimView<'_>) -> Vec<Command> {
                Vec::new()
            }
        }
        let rules = rules();
        let sk = unit_kind(&rules, "skirmisher");
        let base = rules.unit_kind(sk.0).unwrap().hp;
        let tech = |id: &str| sim::TechId(rules.tech_by_id(id).unwrap().0);
        let cmds: Vec<PlayerCommand> = vec![
            spawn(LOCAL_PLAYER, 0, sk, (20, 30), 1),
            spawn(P1, 0, sk, (40, 30), 1),
            PlayerCommand::new(
                P1,
                1,
                Command::DebugResearch {
                    owner: P1,
                    tech: tech("arms_1"),
                },
            ),
            PlayerCommand::new(
                P1,
                2,
                Command::DebugResearch {
                    owner: P1,
                    tech: tech("arms_2"),
                },
            ),
            PlayerCommand::new(
                P1,
                3,
                Command::DebugResearch {
                    owner: P1,
                    tech: tech("arms_3"),
                },
            ),
        ];
        let setup = sim::MatchSetup::scenario(&rules, 7);
        let mut sim = sim::Sim::new(setup, rules.clone(), Box::new(NoAi));
        sim.step(&cmds);
        for _ in 0..sim.setup().cmd_delay {
            sim.step(&[]);
        }
        let view = sim.view();
        let mut selection = Selection::default();
        selection.set([UnitId(1), UnitId(2)]);
        let specs = bar_specs(&view, &rules, &selection, |_| 1.0);
        let own = specs[&TargetId::Unit(UnitId(1))];
        let theirs = specs[&TargetId::Unit(UnitId(2))];
        assert_eq!(
            own.max,
            i32::try_from(base).unwrap(),
            "no Arms: the kind's hp"
        );
        assert_eq!(theirs.max, view.unit_stats(P1, sk).max_hp);
        assert!(theirs.max > own.max, "{} vs {}", theirs.max, own.max);
        assert_eq!(theirs.hp, theirs.max, "a full bar");
    }

    /// Three P1 Mangonels destroy a P0 Watchtower (`BuildingId` 1) in two
    /// volleys (3 x 120 against 500 HP): its bar appears with the first
    /// hit, tracks `building_hp` and goes with the tower.
    #[test]
    fn a_destroyed_watchtower_drops_its_bar() {
        let rules = rules();
        let (tower, _) = rules.building_kind_by_id("watchtower").unwrap();
        let mangonel = unit_kind(&rules, "mangonel");
        let stream = vec![
            (
                0,
                vec![PlayerCommand::new(
                    LOCAL_PLAYER,
                    0,
                    Command::DebugPlaceBuilding {
                        owner: LOCAL_PLAYER,
                        kind: sim::BuildingKindId(tower),
                        tile: Tile::new(20, 30),
                        complete: true,
                    },
                )],
            ),
            (
                4,
                vec![
                    spawn(P1, 0, mangonel, (24, 33), 3),
                    attack(P1, 1, &[1, 2, 3], TargetId::Building(BuildingId(1))),
                ],
            ),
        ];
        let mut app = bar_app(stream);
        let target = TargetId::Building(BuildingId(1));
        run_to(&mut app, 4);
        assert!(!bars(&app).contains_key(&target), "a full tower has no bar");
        let mut lowest = None;
        let mut entities = None;
        for _ in 0..2000 {
            app.update();
            match (bars(&app).get(&target), hp_of(&app, target)) {
                (Some(bar), Some(hp)) => {
                    assert_eq!((bar.spec.hp, bar.spec.max), hp);
                    assert_eq!(bar.spec.owner, LOCAL_PLAYER);
                    assert!(matches!(bar.spec.anchor, BarAnchor::Fixed(p) if p.y > 0.9));
                    lowest = Some(hp.0);
                    entities = Some((bar.back, bar.fill));
                }
                (_, None) => break,
                (None, Some(_)) => {}
            }
        }
        assert!(hp_of(&app, target).is_none(), "the tower was destroyed");
        assert!(lowest.is_some_and(|hp| hp < 500), "{lowest:?}");
        app.update();
        assert!(!bars(&app).contains_key(&target));
        let (back, fill) = entities.expect("the tower had a bar");
        assert!(app.world().get_entity(back).is_err());
        assert!(app.world().get_entity(fill).is_err());
    }

    /// An annexed building keeps its bar but takes its new owner's fill:
    /// P1's Town (`BuildingId` 1) at nw (20, 30) is taken to 0 HP, so it has
    /// a bar in P1's red with the fill hidden; four P0 Skirmishers on the
    /// tiles north of it annex it, it flips to P0 at a quarter of its hit
    /// points, and the same bar now fills in P0's blue.
    #[test]
    fn an_annexed_building_s_fill_takes_the_new_owner_s_colour() {
        let rules = rules();
        let (town_kind, _) = rules.building_kind_by_id("town").unwrap();
        let sk = unit_kind(&rules, "skirmisher");
        let town = BuildingId(1);
        let mut attackers = vec![PlayerCommand::new(
            LOCAL_PLAYER,
            0,
            Command::DebugDamage {
                target: TargetId::Building(town),
                amount: u32::MAX,
            },
        )];
        for (seq, x) in (1..).zip(20..24) {
            attackers.push(PlayerCommand::new(
                LOCAL_PLAYER,
                seq,
                Command::DebugSpawn {
                    owner: LOCAL_PLAYER,
                    kind: sk,
                    at: FxVec2::new(
                        sim::Fx::from_ratio(2 * x + 1, 2),
                        sim::Fx::from_ratio(59, 2),
                    ),
                    count: 1,
                },
            ));
        }
        let stream = vec![
            (
                0,
                vec![PlayerCommand::new(
                    P1,
                    0,
                    Command::DebugPlaceBuilding {
                        owner: P1,
                        kind: sim::BuildingKindId(town_kind),
                        tile: Tile::new(20, 30),
                        complete: true,
                    },
                )],
            ),
            (1, attackers),
        ];
        let mut app = bar_app(stream);
        let target = TargetId::Building(town);
        let assets = app.world().resource::<BarAssets>().clone();
        let fill_material = |app: &App, bar: &Bar| {
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(bar.fill)
                .expect("the fill entity")
                .0
                .id()
        };
        let owner_of = |app: &App| {
            let sim = app.world().non_send::<SimHandle>();
            sim.view().building(town).map(|b| b.owner)
        };
        // Until the Town is at 0 HP and has its (empty) bar in P1's colour.
        for _ in 0..20 {
            app.update();
            if bars(&app).contains_key(&target) {
                break;
            }
        }
        let before = *bars(&app).get(&target).expect("the 0 HP Town has a bar");
        assert_eq!(before.spec.owner, P1);
        assert_eq!(before.spec.hp, 0);
        assert_eq!(
            fill_material(&app, &before),
            assets.fill[1].id(),
            "P1's red"
        );
        // Until it changes hands.
        for _ in 0..3000 {
            if owner_of(&app) == Some(LOCAL_PLAYER) {
                break;
            }
            app.update();
        }
        assert_eq!(owner_of(&app), Some(LOCAL_PLAYER), "the Town was annexed");
        app.update();
        let after = *bars(&app)
            .get(&target)
            .expect("the annexed Town is damaged");
        assert_eq!(
            (after.back, after.fill),
            (before.back, before.fill),
            "the same pair"
        );
        assert_eq!(after.spec.owner, LOCAL_PLAYER);
        assert_eq!(
            after.spec.hp * 4,
            after.spec.max,
            "a quarter of its hit points"
        );
        assert_eq!(
            fill_material(&app, &after),
            assets.fill[0].id(),
            "the fill is now P0's blue"
        );
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(after.back)
                .unwrap()
                .0
                .id(),
            assets.back.id(),
            "the track keeps its neutral colour"
        );
    }

    /// The batching guard: however many bars, one mesh and at most three
    /// materials (the track and one fill per team).
    #[test]
    fn bars_use_one_quad_and_three_materials() {
        let rules = rules();
        let mut app = bar_app(duel(&rules));
        run_to(&mut app, 3);
        app.world_mut().resource_mut::<Selection>().set([
            UnitId(1),
            UnitId(2),
            UnitId(3),
            UnitId(4),
        ]);
        app.update();
        assert_eq!(bars(&app).len(), 4);
        let mut q = app
            .world_mut()
            .query_filtered::<(&Mesh3d, &MeshMaterial3d<StandardMaterial>), With<HealthBar>>();
        let (meshes, materials): (BTreeSet<_>, BTreeSet<_>) = q
            .iter(app.world())
            .map(|(m, mat)| (m.0.id(), mat.0.id()))
            .unzip();
        assert_eq!(meshes.len(), 1, "one shared quad");
        assert_eq!(materials.len(), 3, "track, blue fill, red fill");
        let assets = app.world().resource::<BarAssets>().clone();
        // Each quad has its own part's material: the track, or the fill of
        // its target's owner.
        for bar in bars(&app).values() {
            let material = |e: Entity| {
                app.world()
                    .get::<MeshMaterial3d<StandardMaterial>>(e)
                    .unwrap()
                    .0
                    .id()
            };
            assert_eq!(material(bar.back), assets.back.id());
            assert_eq!(
                material(bar.fill),
                assets.fill[usize::from(bar.spec.owner.0)].id(),
                "{:?}'s fill",
                bar.spec.owner
            );
        }
        assert!(materials.contains(&assets.back.id()));
        assert!(materials.contains(&assets.fill[0].id()));
        assert!(materials.contains(&assets.fill[1].id()));
        let mats = app.world().resource::<Assets<StandardMaterial>>();
        for handle in [&assets.back, &assets.fill[0], &assets.fill[1]] {
            let m = mats.get(handle).unwrap();
            assert!(m.unlit && !m.fog_enabled && m.cull_mode.is_none());
        }
        assert_eq!(
            mats.get(&assets.back).unwrap().base_color,
            palette::BAR_BACK
        );
        assert_eq!(
            mats.get(&assets.fill[1]).unwrap().base_color,
            palette::TEAM[1]
        );
        // The quad's origin is its left edge: x from 0 to 1.
        use bevy::camera::primitives::MeshAabb;
        let quad = app
            .world()
            .resource::<Assets<Mesh>>()
            .get(&assets.quad)
            .unwrap();
        let aabb = quad.compute_aabb().unwrap();
        assert!((aabb.min().x - 0.0).abs() < 1e-6 && (aabb.max().x - 1.0).abs() < 1e-6);
    }
}
