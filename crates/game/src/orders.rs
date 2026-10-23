//! Orders (M2 decision 6): a right click on the ground becomes
//! `Command::Move { units: selection, target, queue: shift }`, S is Stop,
//! A then left click is `AttackMove`: the sim moves the selection there,
//! and its armed units fight any enemy they meet on the way, then walk on
//! (M4a; unarmed units just move). The ground hit is one ray-plane
//! intersection with y = 0 (`Camera::viewport_to_world`, then solve for
//! t): never a mesh raycast. World clicks are ignored while the pointer is over the HUD
//! (`PointerOverUi`).
//!
//! Every order goes through `PendingCommands::push_local`, which stamps the
//! seq; nothing here touches the sim. Orders name only the local player's
//! units ([`own_selected_units`]): a click can select another player's
//! unit to read its hit points, and such a selection issues nothing.
//!
//! A right click is contextual (M3b, M4b): with units selected, on an
//! enemy unit, or an enemy building with hit points left, it is `Attack`
//! when `SimView::check_attack` accepts it (some selected unit is armed),
//! otherwise it falls through to the next context; on one of the player's
//! construction sites it is `Build` on that site (the nearest selected
//! builder takes it over), on one of their buildings that has gather slots
//! it is `Gather`; with a building selected it is `SetRally` at the ground
//! point; otherwise it is `Move` ([`context_order`]). What is under the
//! pointer is the hovered entity or its nearest ancestor that mirrors a
//! unit or building, so a model with child meshes (the art pass) still
//! resolves to its unit.
//! The sim validates every one of them, so a wrong guess comes back as a
//! rejection on the message line, never as a silent change. One rejection
//! is not shown: `InvalidTarget` for an `Attack` whose target died before
//! the command applied ([`IssuedAttacks`]).
//!
//! Feedback (M4b): attack-move mode shows the crosshair cursor
//! ([`sync_attack_cursor`]); an `Attack` or `AttackMove` leaves the attack
//! marker (a ring with a cross, `selection::spawn_attack_marker`) at its
//! target (a building's roof, [`target_point`]), other ground orders the
//! plain move marker. The command card has
//! an Attack-move button for armed selections.
//!
//! [`OrderMode::Place`] is the build-placement mode the command card enters;
//! `placement.rs` owns its left click, a right click or Esc cancels it.
//!
//! Keys: S = Stop, A = attack-move mode (when an own unit is selected), Esc
//! or a right click cancels the mode. The camera also pans on A and S
//! (`camera.rs`, WASD); the owner kept that overlap for M4b (ROADMAP M4b
//! Notes, decision 4) and revisits it at the controls playtest.

use std::collections::VecDeque;

use bevy::ecs::system::SystemParam;
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::PointerId;
use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};
use rules::Rules;
use sim::{BuildingId, BuildingKindId, Command, FxVec2, RejectReason, SimView, TargetId, UnitId};

use crate::buildings::{BuildingRef, building_transform};
use crate::camera::RtsCamera;
use crate::hud::{PointerOverUi, WorldInputSet};
use crate::present::{UnitRef, sim_from_world, world_from_sim};
use crate::selection::{RingAssets, Selection, spawn_attack_marker, spawn_move_marker};
use crate::sim_driver::{LOCAL_PLAYER, PendingCommands, SimHandle};

/// What the next left click on the ground means.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderMode {
    /// Left click selects, right click moves.
    #[default]
    Normal,
    /// After A: the next left click on the ground is an `AttackMove`; Esc or
    /// a right click cancels back to `Normal`.
    AttackMove,
    /// Placing a building of this kind: a ghost follows the cursor and a
    /// left click on the ground issues `Build` (`placement.rs`); Esc or a
    /// right click cancels back to `Normal`.
    Place(BuildingKindId),
}

/// Where the cursor ray meets the ground plane y = 0, in world metres, or
/// `None` when the ray misses (looking at the sky) or the camera has no
/// viewport yet.
pub fn ground_hit(
    camera: &Camera,
    camera_transform: &GlobalTransform,
    cursor: Vec2,
) -> Option<Vec3> {
    let ray = camera.viewport_to_world(camera_transform, cursor).ok()?;
    let t = ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Dir3::Y))?;
    Some(ray.get_point(t))
}

/// The local player's units in `selection`, in id order: every order is
/// issued for these. Another player's unit can be selected (a click selects
/// any unit, to read its hit points) but is never ordered.
pub fn own_selected_units(view: &SimView<'_>, selection: &Selection) -> Vec<UnitId> {
    selection
        .units
        .iter()
        .copied()
        .filter(|id| view.unit(*id).is_some_and(|u| u.owner == LOCAL_PLAYER))
        .collect()
}

/// Queue a Move for `units` (the local player's selected units); returns
/// the seq, or `None` when there are none (no command is queued).
pub fn issue_move(
    pending: &mut PendingCommands,
    units: &[UnitId],
    target: FxVec2,
    queue: bool,
) -> Option<u32> {
    if units.is_empty() {
        return None;
    }
    Some(pending.push_local(Command::Move {
        units: units.to_vec(),
        target,
        queue,
    }))
}

/// Queue a Stop for `units`.
pub fn issue_stop(pending: &mut PendingCommands, units: &[UnitId]) -> Option<u32> {
    if units.is_empty() {
        return None;
    }
    Some(pending.push_local(Command::Stop {
        units: units.to_vec(),
    }))
}

/// Queue an `AttackMove` for `units`: armed units engage enemies on the
/// way and resume; unarmed ones move as with `Move`.
pub fn issue_attack_move(
    pending: &mut PendingCommands,
    units: &[UnitId],
    target: FxVec2,
) -> Option<u32> {
    if units.is_empty() {
        return None;
    }
    Some(pending.push_local(Command::AttackMove {
        units: units.to_vec(),
        target,
    }))
}

/// Queue a Gather for `units` onto `node`.
pub fn issue_gather(
    pending: &mut PendingCommands,
    units: &[UnitId],
    node: BuildingId,
) -> Option<u32> {
    if units.is_empty() {
        return None;
    }
    Some(pending.push_local(Command::Gather {
        units: units.to_vec(),
        node,
    }))
}

/// Queue a `SetRally` for the selected building.
pub fn issue_rally(
    pending: &mut PendingCommands,
    selection: &Selection,
    target: FxVec2,
) -> Option<u32> {
    let building = selection.building?;
    Some(pending.push_local(Command::SetRally { building, target }))
}

/// The last [`IssuedAttacks::CAPACITY`] `Attack` commands the local player
/// issued, as `(seq, target)`. A target can die in the `cmd_delay` ticks
/// between the click and the command applying, and the sim then rejects it
/// with `InvalidTarget`; the message line drops that rejection
/// ([`IssuedAttacks::hides`]), since the target is gone anyway.
#[derive(Resource, Debug, Default, Clone, PartialEq, Eq)]
pub struct IssuedAttacks(VecDeque<(u32, TargetId)>);

impl IssuedAttacks {
    /// How many recent attacks are remembered.
    pub const CAPACITY: usize = 16;

    /// Remember that `seq` attacked `target`, forgetting the oldest beyond
    /// [`Self::CAPACITY`].
    pub fn record(&mut self, seq: u32, target: TargetId) {
        if self.0.len() == Self::CAPACITY {
            self.0.pop_front();
        }
        self.0.push_back((seq, target));
    }

    /// The target of the remembered attack `seq`.
    pub fn target(&self, seq: u32) -> Option<TargetId> {
        self.0.iter().find(|(s, _)| *s == seq).map(|(_, t)| *t)
    }

    /// `true` when the local rejection of `seq` for `reason` should not be
    /// shown: an `InvalidTarget` for a remembered attack whose target is
    /// gone now (`gone` says so).
    pub fn hides(&self, seq: u32, reason: &RejectReason, gone: impl Fn(TargetId) -> bool) -> bool {
        *reason == RejectReason::InvalidTarget && self.target(seq).is_some_and(gone)
    }
}

/// `true` when `target` no longer exists in `view`, or is a building with
/// no hit points left (nothing an `Attack` could still hit).
pub fn target_gone(view: &SimView<'_>, target: TargetId) -> bool {
    match target {
        TargetId::Unit(id) => view.unit(id).is_none(),
        TargetId::Building(id) => view.building(id).is_none_or(|b| b.hp <= 0),
    }
}

/// Right click, S and A handling, the attack-move cursor.
pub struct OrdersPlugin;

impl Plugin for OrdersPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OrderMode>()
            .init_resource::<IssuedAttacks>()
            .add_systems(
                Update,
                (order_mode_keys, issue_pointer_orders)
                    .chain()
                    .in_set(WorldInputSet),
            )
            .add_systems(Update, sync_attack_cursor.after(WorldInputSet));
    }
}

/// A enters [`OrderMode::AttackMove`] when an own unit is selected; Esc
/// returns to `Normal` (a right click does too, in
/// [`issue_pointer_orders`]); S issues a Stop for the own selected units
/// (`issue_stop`).
pub fn order_mode_keys(
    keys: Res<ButtonInput<KeyCode>>,
    sim: NonSend<SimHandle>,
    mut mode: ResMut<OrderMode>,
    selection: Res<Selection>,
    mut pending: ResMut<PendingCommands>,
) {
    if keys.just_pressed(KeyCode::Escape) && *mode != OrderMode::Normal {
        *mode = OrderMode::Normal;
    }
    let pressed_a = keys.just_pressed(KeyCode::KeyA);
    let pressed_s = keys.just_pressed(KeyCode::KeyS);
    if !(pressed_a || pressed_s) || selection.is_empty() {
        return;
    }
    let own = own_selected_units(&sim.view(), &selection);
    if pressed_a && !own.is_empty() {
        *mode = OrderMode::AttackMove;
    }
    if pressed_s && let Some(seq) = issue_stop(&mut pending, &own) {
        info!("Stop seq {seq} for {} units", own.len());
    }
}

/// What the mouse pointer is over in the world: the picking hover map, the
/// `ChildOf` links to walk up from a hovered child mesh, and the unit and
/// building markers.
#[derive(SystemParam)]
pub struct HoverTargets<'w, 's> {
    hover: Res<'w, HoverMap>,
    parents: Query<'w, 's, &'static ChildOf>,
    refs: Query<'w, 's, (Option<&'static UnitRef>, Option<&'static BuildingRef>)>,
}

/// A unit or building under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoverHit {
    /// A unit entity.
    Unit(UnitId),
    /// A building entity.
    Building(BuildingId),
}

impl HoverTargets<'_, '_> {
    /// The unit or building each entity under the mouse mirrors: the entity
    /// itself or its nearest ancestor with a [`UnitRef`] or [`BuildingRef`].
    pub fn hits(&self) -> Vec<HoverHit> {
        let Some(hovered) = self.hover.get(&PointerId::Mouse) else {
            return Vec::new();
        };
        hovered
            .keys()
            .filter_map(|e| {
                std::iter::once(*e)
                    .chain(self.parents.iter_ancestors(*e))
                    .find_map(|a| match self.refs.get(a) {
                        Ok((Some(u), _)) => Some(HoverHit::Unit(u.0)),
                        Ok((None, Some(b))) => Some(HoverHit::Building(b.0)),
                        _ => None,
                    })
            })
            .collect()
    }
}

/// What a right click on something in the world means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Context {
    /// An enemy unit, or an enemy building with hit points left: attack it.
    Attack(TargetId),
    /// An own construction site: a selected builder takes it over.
    Site(BuildingId),
    /// An own complete building with gather slots: the units work it.
    Gather(BuildingId),
}

/// The contexts of `hits`, most important first: enemy units, enemy
/// buildings with hit points, own sites, own gather buildings. Anything
/// else under the pointer (an own unit, an own building without slots, a
/// destroyed Town at 0 hit points) has no context, so the click is a move.
pub fn contexts_of(
    view: &SimView<'_>,
    rules: &Rules,
    hits: impl IntoIterator<Item = HoverHit>,
) -> Vec<Context> {
    let mut ranked: Vec<(u8, Context)> = hits
        .into_iter()
        .filter_map(|hit| match hit {
            HoverHit::Unit(id) => view
                .unit(id)
                .filter(|u| u.owner != LOCAL_PLAYER)
                .map(|_| (0, Context::Attack(TargetId::Unit(id)))),
            HoverHit::Building(id) => {
                let b = view.building(id)?;
                if b.owner != LOCAL_PLAYER {
                    return (b.hp > 0).then_some((1, Context::Attack(TargetId::Building(id))));
                }
                match b.construction {
                    sim::Construction::Site { .. } => Some((2, Context::Site(id))),
                    sim::Construction::Complete => rules
                        .building_kind(b.kind.0)
                        .is_some_and(|k| k.gather.is_some())
                        .then_some((3, Context::Gather(id))),
                }
            }
        })
        .collect();
    // By rank, then by id, so the hover map's order never matters; a
    // target hit through two child meshes counts once.
    ranked.sort();
    ranked.dedup();
    ranked.into_iter().map(|(_, c)| c).collect()
}

/// The command a right click issues for the selection's own units, given
/// the contexts under the pointer ([`contexts_of`], most important first)
/// and the ground point; `None` when nothing is issued (no own unit
/// selected, or an own site no selected unit can build). The first context
/// that applies wins: `Attack` when `SimView::check_attack` accepts the
/// own units against the target (only the armed ones are named), a site's
/// `Build` for the nearest selected builder, `Gather`; with none, `Move`
/// (queued when `queue`).
pub fn context_order(
    view: &SimView<'_>,
    rules: &Rules,
    selection: &Selection,
    contexts: &[Context],
    point: FxVec2,
    queue: bool,
) -> Option<Command> {
    let own = own_selected_units(view, selection);
    if own.is_empty() {
        return None;
    }
    for context in contexts {
        match *context {
            Context::Attack(target) => {
                if let Ok(units) = view.check_attack(LOCAL_PLAYER, &own, target) {
                    return Some(Command::Attack { units, target });
                }
            }
            Context::Site(site) => return resume_order(view, rules, selection, site),
            Context::Gather(node) => {
                return Some(Command::Gather { units: own, node });
            }
        }
    }
    Some(Command::Move {
        units: own,
        target: point,
        queue,
    })
}

/// Where an attack marker for `target` goes: a unit's position on the
/// ground, or the roof of a building's box over its footprint centre (a
/// site's box at its current height). A building is an opaque box standing
/// on the ground, so a marker at its ground centre would be drawn inside it
/// and depth-tested away.
pub fn target_point(view: &SimView<'_>, rules: &Rules, target: TargetId) -> Option<Vec3> {
    match target {
        TargetId::Unit(id) => view.unit(id).map(|u| world_from_sim(u.pos, 0.0)),
        TargetId::Building(id) => view.building(id).map(|b| {
            let t = building_transform(rules, b);
            t.translation + Vec3::Y * t.scale.y
        }),
    }
}

/// On right-button release (Normal) or left-button release (`AttackMove`),
/// not over UI: `ground_hit` at the cursor, `sim_from_world`, then the
/// right click's [`context_order`] (or `SetRally` with a building
/// selected) or `issue_attack_move`; on success a marker (the attack
/// marker at an `Attack`'s target or an `AttackMove`'s point, else the move
/// marker at the hit) and the mode returns to `Normal`. An `Attack` is
/// remembered in [`IssuedAttacks`]. A right click in `AttackMove` mode only
/// cancels the mode.
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn issue_pointer_orders(
    sim: NonSend<SimHandle>,
    targets: HoverTargets,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<RtsCamera>>,
    over_ui: Res<PointerOverUi>,
    mut mode: ResMut<OrderMode>,
    selection: Res<Selection>,
    mut pending: ResMut<PendingCommands>,
    mut attacks: ResMut<IssuedAttacks>,
    ring_assets: Option<Res<RingAssets>>,
    time: Res<Time>,
    mut commands: Commands,
) {
    let right = buttons.just_released(MouseButton::Right);
    let left = buttons.just_released(MouseButton::Left);
    if !(right || left) {
        return;
    }
    if matches!(*mode, OrderMode::AttackMove | OrderMode::Place(_)) && right {
        *mode = OrderMode::Normal;
        return;
    }
    if over_ui.0 {
        return;
    }
    let Some(cursor) = windows.single().ok().and_then(Window::cursor_position) else {
        return;
    };
    let Ok((cam, cam_tf)) = camera.single() else {
        return;
    };
    let Some(hit) = ground_hit(cam, cam_tf, cursor) else {
        return;
    };
    let target = sim_from_world(hit);
    let view = sim.view();
    let rules = sim.rules();
    // The issued seq and where its marker goes (`true`: attack marker).
    let issued: Option<(u32, Vec3, bool)> = match *mode {
        OrderMode::Normal if right && selection.building.is_some() => {
            issue_rally(&mut pending, &selection, target).map(|seq| (seq, hit, false))
        }
        OrderMode::Normal if right => {
            let contexts = contexts_of(&view, rules, targets.hits());
            let queue = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
            context_order(&view, rules, &selection, &contexts, target, queue).map(|cmd| {
                let attacked = match &cmd {
                    Command::Attack { target, .. } => Some(*target),
                    _ => None,
                };
                let seq = pending.push_local(cmd);
                match attacked {
                    Some(t) => {
                        attacks.record(seq, t);
                        (seq, target_point(&view, rules, t).unwrap_or(hit), true)
                    }
                    None => (seq, hit, false),
                }
            })
        }
        OrderMode::AttackMove if left => {
            *mode = OrderMode::Normal;
            issue_attack_move(&mut pending, &own_selected_units(&view, &selection), target)
                .map(|seq| (seq, hit, true))
        }
        _ => None,
    };
    if let Some((_, at, attack)) = issued
        && let Some(assets) = ring_assets.as_deref()
    {
        if attack {
            spawn_attack_marker(&mut commands, assets, at, time.elapsed_secs());
        } else {
            spawn_move_marker(&mut commands, assets, at, time.elapsed_secs());
        }
    }
}

/// The crosshair cursor on the primary window while in
/// [`OrderMode::AttackMove`]; the default cursor again afterwards
/// (removing `CursorIcon` makes `bevy_winit` reset it).
pub fn sync_attack_cursor(
    mode: Res<OrderMode>,
    windows: Query<(Entity, Has<CursorIcon>), With<PrimaryWindow>>,
    mut commands: Commands,
) {
    if !mode.is_changed() {
        return;
    }
    let attack = *mode == OrderMode::AttackMove;
    for (window, has_icon) in &windows {
        if attack {
            commands
                .entity(window)
                .insert(CursorIcon::from(SystemCursorIcon::Crosshair));
        } else if has_icon {
            commands.entity(window).remove::<CursorIcon>();
        }
    }
}

/// A `Build` that hands `site` to the selected builder nearest it (a
/// resume: no cost). `None` when no selected unit can build.
fn resume_order(
    view: &SimView<'_>,
    rules: &Rules,
    selection: &Selection,
    site: BuildingId,
) -> Option<Command> {
    let b = view.building(site)?;
    let kind = rules.building_kind(b.kind.0)?;
    let centre = sim::construction::centre_fx(kind, b.tile);
    let unit = crate::placement::nearest_builder(view, rules, selection, centre)?;
    Some(Command::Build {
        unit,
        kind: b.kind,
        tile: b.tile,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::RING_Y;
    use sim::{PlayerCommand, PlayerId, Tile, UnitKindId};

    fn units(n: u32) -> Vec<UnitId> {
        (1..=n).map(UnitId).collect()
    }

    #[test]
    fn orders_go_through_pending_commands_with_the_selection() {
        let mut pending = PendingCommands::default();
        assert_eq!(issue_move(&mut pending, &[], FxVec2::ZERO, false), None);
        assert!(pending.is_empty());
        let sel = units(3);
        let target = FxVec2::from_ints(10, 20);
        assert_eq!(issue_move(&mut pending, &sel, target, true), Some(0));
        assert_eq!(issue_stop(&mut pending, &sel), Some(1));
        assert_eq!(issue_attack_move(&mut pending, &sel, target), Some(2));
        let cmds = pending.drain_sorted();
        assert_eq!(cmds.len(), 3);
        assert_eq!(
            cmds[0].cmd,
            Command::Move {
                units: vec![UnitId(1), UnitId(2), UnitId(3)],
                target,
                queue: true
            }
        );
        assert!(matches!(cmds[1].cmd, Command::Stop { .. }));
        assert!(matches!(cmds[2].cmd, Command::AttackMove { .. }));
    }

    #[test]
    fn ground_hit_solves_the_y0_plane() {
        // A camera 10 m up looking straight down at the origin with a
        // known viewport: the viewport centre hits the origin, a corner
        // misses nothing but lands away from it; a camera looking up misses.
        let mut camera = Camera::default();
        // Fake the computed viewport so viewport_to_world has a size.
        camera.computed.target_info = Some(bevy::camera::RenderTargetInfo {
            physical_size: UVec2::new(800, 600),
            scale_factor: 1.0,
        });
        let projection = Projection::Perspective(PerspectiveProjection::default());
        camera.computed.clip_from_view = projection.get_clip_from_view();
        let down = GlobalTransform::from(
            Transform::from_xyz(0.0, 10.0, 0.0).looking_at(Vec3::ZERO, Vec3::NEG_Z),
        );
        let hit = ground_hit(&camera, &down, Vec2::new(400.0, 300.0)).expect("centre hits");
        assert!(hit.length() < 1e-3, "{hit}");
        assert!(hit.y.abs() < 1e-4);
        let corner = ground_hit(&camera, &down, Vec2::new(0.0, 0.0)).expect("corner hits");
        assert!(corner.length() > 1.0);
        assert!(corner.y.abs() < 1e-3);
        let up = GlobalTransform::from(
            Transform::from_xyz(0.0, 10.0, 0.0).looking_at(Vec3::new(0.0, 20.0, 0.0), Vec3::Z),
        );
        assert_eq!(ground_hit(&camera, &up, Vec2::new(400.0, 300.0)), None);
    }

    struct NoAi;

    impl sim::AiController for NoAi {
        fn think(&mut self, _: PlayerId, _: &SimView<'_>) -> Vec<Command> {
            Vec::new()
        }
    }

    const P1: PlayerId = PlayerId(1);

    fn kind(rules: &Rules, id: &str) -> UnitKindId {
        UnitKindId(u16::try_from(rules.units.iter().position(|u| u.id == id).unwrap()).unwrap())
    }

    fn building_kind(rules: &Rules, id: &str) -> BuildingKindId {
        BuildingKindId(rules.building_kind_by_id(id).unwrap().0)
    }

    /// The standard start (P0's Seat is `BuildingId` 1, its Yeomen `UnitId`
    /// 1..=5; P1's Seat is `BuildingId` 2, its Yeomen 6..=10), then, applied
    /// by tick 3 in `(player, seq)` order: two P0 Skirmishers (11, 12) east
    /// of the Seat, a P0 Scriptorium site (`BuildingId` 3) and a complete P0
    /// Croft (4), a P1 Skirmisher (13) and a P1 Watchtower (5) far from
    /// everything.
    fn arena() -> sim::Sim {
        let (setup, rules, cmds) = arena_setup();
        let mut sim = sim::Sim::new(setup, rules, Box::new(NoAi));
        sim.step(&cmds);
        for _ in 0..sim.setup().cmd_delay {
            sim.step(&[]);
        }
        sim
    }

    /// The [`arena`] in the game's own `SimHandle`, for systems.
    fn arena_handle() -> SimHandle {
        let (setup, rules, cmds) = arena_setup();
        let mut handle = SimHandle::from_setup(setup, rules);
        handle.step(&cmds);
        for _ in 0..handle.setup().cmd_delay {
            handle.step(&[]);
        }
        handle
    }

    /// The arena's setup, rules and tick-0 commands, sorted.
    fn arena_setup() -> (sim::MatchSetup, Rules, Vec<PlayerCommand>) {
        let rules =
            Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
                .expect("rules load");
        let mut setup = sim::MatchSetup::scenario(&rules, 1);
        setup.start = sim::StartKind::Standard;
        let sk = kind(&rules, "skirmisher");
        let place = |owner, k: &str, tile, complete| Command::DebugPlaceBuilding {
            owner,
            kind: building_kind(&rules, k),
            tile,
            complete,
        };
        let cmds = vec![
            PlayerCommand::new(
                LOCAL_PLAYER,
                0,
                Command::DebugSpawn {
                    owner: LOCAL_PLAYER,
                    kind: sk,
                    at: FxVec2::from_ints(36, 64),
                    count: 2,
                },
            ),
            PlayerCommand::new(
                P1,
                0,
                Command::DebugSpawn {
                    owner: P1,
                    kind: sk,
                    at: FxVec2::from_ints(40, 30),
                    count: 1,
                },
            ),
            PlayerCommand::new(P1, 1, place(P1, "watchtower", Tile::new(10, 30), true)),
            PlayerCommand::new(
                LOCAL_PLAYER,
                1,
                place(LOCAL_PLAYER, "scriptorium", Tile::new(27, 58), false),
            ),
            PlayerCommand::new(
                LOCAL_PLAYER,
                2,
                place(LOCAL_PLAYER, "croft", Tile::new(28, 68), true),
            ),
        ];
        let mut cmds = cmds;
        sim::command::sort_commands(&mut cmds);
        (setup, rules, cmds)
    }

    fn select(ids: &[u32]) -> Selection {
        let mut s = Selection::default();
        s.set(ids.iter().copied().map(UnitId));
        s
    }

    #[test]
    fn the_arena_is_as_described() {
        let sim = arena();
        let view = sim.view();
        let rules = sim.rules();
        let kind_of = |id: u32| {
            rules
                .unit_kind(view.unit(UnitId(id)).unwrap().kind.0)
                .unwrap()
                .id
                .clone()
        };
        assert_eq!(kind_of(1), "yeoman");
        assert_eq!(kind_of(11), "skirmisher");
        assert_eq!(view.unit(UnitId(13)).unwrap().owner, P1);
        let b = |id: u32| view.building(BuildingId(id)).unwrap();
        assert_eq!(
            (b(3).owner, b(4).owner, b(5).owner),
            (LOCAL_PLAYER, LOCAL_PLAYER, P1)
        );
        assert!(matches!(b(3).construction, sim::Construction::Site { .. }));
        assert!(matches!(b(4).construction, sim::Construction::Complete));
        assert!(matches!(b(5).construction, sim::Construction::Complete));
        assert_eq!(view.unit_count(), 13);
    }

    #[test]
    fn right_click_on_an_enemy_with_armed_units_attacks() {
        let sim = arena();
        let (view, rules) = (sim.view(), sim.rules());
        let point = FxVec2::from_ints(40, 30);
        // Yeomen and Skirmishers together: the armed ones attack.
        let selection = select(&[1, 2, 11, 12]);
        for (hit, target) in [
            (HoverHit::Unit(UnitId(13)), TargetId::Unit(UnitId(13))),
            (
                HoverHit::Building(BuildingId(5)),
                TargetId::Building(BuildingId(5)),
            ),
            (
                HoverHit::Building(BuildingId(2)),
                TargetId::Building(BuildingId(2)),
            ),
        ] {
            let contexts = contexts_of(&view, rules, [hit]);
            assert_eq!(contexts, [Context::Attack(target)]);
            assert_eq!(
                context_order(&view, rules, &selection, &contexts, point, false),
                Some(Command::Attack {
                    units: vec![UnitId(11), UnitId(12)],
                    target
                }),
                "{hit:?}"
            );
        }
        // An enemy unit beats an enemy building and an own site under the
        // same pointer.
        let contexts = contexts_of(
            &view,
            rules,
            [
                HoverHit::Building(BuildingId(3)),
                HoverHit::Building(BuildingId(5)),
                HoverHit::Unit(UnitId(13)),
            ],
        );
        assert_eq!(
            contexts,
            [
                Context::Attack(TargetId::Unit(UnitId(13))),
                Context::Attack(TargetId::Building(BuildingId(5))),
                Context::Site(BuildingId(3)),
            ]
        );
        // The verdict is the sim's.
        assert!(
            view.check_attack(LOCAL_PLAYER, &[UnitId(11)], TargetId::Unit(UnitId(13)))
                .is_ok()
        );
    }

    #[test]
    fn right_click_on_an_enemy_with_only_yeomen_moves() {
        let sim = arena();
        let (view, rules) = (sim.view(), sim.rules());
        let point = FxVec2::from_ints(40, 30);
        let selection = select(&[1, 2]);
        let contexts = contexts_of(&view, rules, [HoverHit::Unit(UnitId(13))]);
        assert_eq!(
            view.check_attack(
                LOCAL_PLAYER,
                &[UnitId(1), UnitId(2)],
                TargetId::Unit(UnitId(13))
            ),
            Err(RejectReason::NoValidUnits)
        );
        assert_eq!(
            context_order(&view, rules, &selection, &contexts, point, true),
            Some(Command::Move {
                units: vec![UnitId(1), UnitId(2)],
                target: point,
                queue: true
            })
        );
        // Yeomen over an enemy unit standing on an own site: the site wins
        // once the attack does not apply.
        let contexts = contexts_of(
            &view,
            rules,
            [
                HoverHit::Unit(UnitId(13)),
                HoverHit::Building(BuildingId(3)),
            ],
        );
        assert!(matches!(
            context_order(&view, rules, &selection, &contexts, point, false),
            Some(Command::Build {
                unit: UnitId(1 | 2),
                ..
            })
        ));
    }

    #[test]
    fn own_site_and_gather_contexts_still_win_over_ground() {
        let sim = arena();
        let (view, rules) = (sim.view(), sim.rules());
        let point = FxVec2::from_ints(28, 59);
        let yeomen = select(&[1, 2, 3]);
        // Own units and own buildings without slots are no context.
        assert!(contexts_of(&view, rules, [HoverHit::Unit(UnitId(11))]).is_empty());
        assert!(contexts_of(&view, rules, [HoverHit::Building(BuildingId(1))]).is_empty());
        // The site: the nearest selected builder resumes it.
        let site = contexts_of(&view, rules, [HoverHit::Building(BuildingId(3))]);
        assert_eq!(site, [Context::Site(BuildingId(3))]);
        let Some(Command::Build { kind, tile, .. }) =
            context_order(&view, rules, &yeomen, &site, point, false)
        else {
            panic!("a Build");
        };
        assert_eq!(
            (kind, tile),
            (building_kind(rules, "scriptorium"), Tile::new(27, 58))
        );
        // Nobody selected can build: nothing is issued, as before M4b.
        assert_eq!(
            context_order(&view, rules, &select(&[11]), &site, point, false),
            None
        );
        // The Croft: Gather for the own units.
        let croft = contexts_of(&view, rules, [HoverHit::Building(BuildingId(4))]);
        assert_eq!(croft, [Context::Gather(BuildingId(4))]);
        assert_eq!(
            context_order(&view, rules, &yeomen, &croft, point, false),
            Some(Command::Gather {
                units: vec![UnitId(1), UnitId(2), UnitId(3)],
                node: BuildingId(4)
            })
        );
        // The ground: Move.
        assert!(matches!(
            context_order(&view, rules, &yeomen, &[], point, false),
            Some(Command::Move { .. })
        ));
    }

    #[test]
    fn enemy_selection_issues_nothing() {
        let sim = arena();
        let (view, rules) = (sim.view(), sim.rules());
        let point = FxVec2::from_ints(40, 60);
        // Only P1's units (an enemy Skirmisher and Yeomen) are selected.
        let enemy = select(&[6, 13]);
        assert!(own_selected_units(&view, &enemy).is_empty());
        for contexts in [
            vec![],
            vec![Context::Attack(TargetId::Unit(UnitId(11)))],
            vec![Context::Gather(BuildingId(4))],
        ] {
            assert_eq!(
                context_order(&view, rules, &enemy, &contexts, point, false),
                None
            );
        }
        // Mixed: only the own units are ordered.
        let mixed = select(&[1, 6, 11, 13]);
        assert_eq!(own_selected_units(&view, &mixed), [UnitId(1), UnitId(11)]);
        assert_eq!(
            context_order(&view, rules, &mixed, &[], point, false),
            Some(Command::Move {
                units: vec![UnitId(1), UnitId(11)],
                target: point,
                queue: false
            })
        );
        let mut pending = PendingCommands::default();
        assert_eq!(
            issue_stop(&mut pending, &own_selected_units(&view, &enemy)),
            None
        );
        assert!(pending.is_empty());
    }

    /// Where the attack marker goes: a unit's position on the ground, a
    /// building's roof over its footprint centre. Buildings are opaque
    /// boxes standing on the ground, so a marker at their ground centre
    /// would be drawn inside the box and never seen.
    #[test]
    fn attack_marker_points_sit_on_the_target() {
        let sim = arena();
        let (view, rules) = (sim.view(), sim.rules());
        let unit = view.unit(UnitId(13)).unwrap();
        assert_eq!(
            target_point(&view, rules, TargetId::Unit(UnitId(13))),
            Some(world_from_sim(unit.pos, 0.0))
        );
        // P1's 1 x 1 Watchtower, P0's rising Scriptorium site, P1's Seat.
        for id in [5, 3, 2] {
            let b = view.building(BuildingId(id)).unwrap();
            let t = building_transform(rules, b);
            let p = target_point(&view, rules, TargetId::Building(BuildingId(id))).unwrap();
            assert_eq!(
                (p.x, p.z),
                (t.translation.x, t.translation.z),
                "over the footprint centre of {id}"
            );
            assert!(
                p.y >= t.translation.y + t.scale.y,
                "on the roof of {id}: {p} vs {t:?}"
            );
        }
        assert_eq!(
            target_point(&view, rules, TargetId::Building(BuildingId(99))),
            None
        );
    }

    #[test]
    fn issued_attacks_hide_invalid_target_only_for_gone_targets() {
        let sim = arena();
        let view = sim.view();
        let mut attacks = IssuedAttacks::default();
        attacks.record(4, TargetId::Unit(UnitId(13)));
        attacks.record(5, TargetId::Unit(UnitId(999)));
        attacks.record(6, TargetId::Building(BuildingId(5)));
        let gone = |t| target_gone(&view, t);
        assert!(!gone(TargetId::Unit(UnitId(13))));
        assert!(gone(TargetId::Unit(UnitId(999))));
        assert!(!gone(TargetId::Building(BuildingId(5))));
        assert!(gone(TargetId::Building(BuildingId(99))));
        // Only an InvalidTarget for a remembered attack on a gone target.
        assert!(attacks.hides(5, &RejectReason::InvalidTarget, gone));
        assert!(
            !attacks.hides(4, &RejectReason::InvalidTarget, gone),
            "alive: shown"
        );
        assert!(
            !attacks.hides(6, &RejectReason::InvalidTarget, gone),
            "alive: shown"
        );
        assert!(
            !attacks.hides(5, &RejectReason::NoValidUnits, gone),
            "another reason"
        );
        assert!(
            !attacks.hides(7, &RejectReason::InvalidTarget, gone),
            "not an attack"
        );
        // A ring of the last sixteen.
        for seq in 100..120 {
            attacks.record(seq, TargetId::Unit(UnitId(999)));
        }
        assert_eq!(attacks.target(5), None, "forgotten");
        assert_eq!(attacks.target(103), None);
        assert_eq!(attacks.target(104), Some(TargetId::Unit(UnitId(999))));
        assert_eq!(attacks.0.len(), IssuedAttacks::CAPACITY);
    }

    /// What the pointer is over in [`pointer_app`]: a child entity of a
    /// unit or building entity (as a glTF model's mesh would be), or the
    /// open ground.
    #[derive(Clone, Copy)]
    enum Hover {
        ChildOfUnit(u32),
        ChildOfBuilding(u32),
        Ground,
    }

    /// Where the camera in [`pointer_app`] looks: the viewport centre, where
    /// the cursor is, hits the ground here (sim tile (50, 50)), away from
    /// every unit and building of the arena.
    fn pointer_ground() -> Vec3 {
        world_from_sim(FxVec2::from_ints(50, 50), 0.0)
    }

    /// The real right-click system over the [`arena`]: a primary window with
    /// the cursor at its centre, an RTS camera whose viewport centre looks
    /// at [`pointer_ground`], the hover map naming `hover`, `selected` as
    /// the selection, and the right button just released.
    fn pointer_app(selected: &[u32], hover: Hover) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<OrderMode>()
            .init_resource::<IssuedAttacks>()
            .init_resource::<PendingCommands>()
            .init_resource::<PointerOverUi>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(select(selected))
            .insert_resource(RingAssets {
                selection: Handle::default(),
                marker: Handle::default(),
                attack: Handle::default(),
            })
            .add_systems(Update, issue_pointer_orders);
        app.world_mut().insert_non_send(arena_handle());
        let mut buttons = ButtonInput::<MouseButton>::default();
        buttons.press(MouseButton::Right);
        buttons.release(MouseButton::Right);
        app.insert_resource(buttons);
        let mut window = Window::default();
        let size = UVec2::new(window.physical_width(), window.physical_height());
        window.set_cursor_position(Some(size.as_vec2() / 2.0));
        app.world_mut().spawn((window, PrimaryWindow));
        // The faked viewport of `ground_hit_solves_the_y0_plane`.
        let mut camera = Camera::default();
        camera.computed.target_info = Some(bevy::camera::RenderTargetInfo {
            physical_size: size,
            scale_factor: 1.0,
        });
        camera.computed.clip_from_view =
            Projection::Perspective(PerspectiveProjection::default()).get_clip_from_view();
        let rts = RtsCamera {
            focus: pointer_ground(),
            ..default()
        };
        let transform = Transform::from_translation(rts.eye()).looking_at(rts.focus, Vec3::Y);
        let cam = app
            .world_mut()
            .spawn((rts, camera, transform, GlobalTransform::from(transform)))
            .id();
        let hovered = match hover {
            Hover::ChildOfUnit(id) => {
                let unit = app.world_mut().spawn(UnitRef(UnitId(id))).id();
                Some(app.world_mut().spawn(ChildOf(unit)).id())
            }
            Hover::ChildOfBuilding(id) => {
                let building = app.world_mut().spawn(BuildingRef(BuildingId(id))).id();
                Some(app.world_mut().spawn(ChildOf(building)).id())
            }
            Hover::Ground => None,
        };
        let mut map = HoverMap::default();
        map.insert(
            PointerId::Mouse,
            hovered
                .map(|e| {
                    (
                        e,
                        bevy::picking::backend::HitData::new(cam, 1.0, None, None),
                    )
                })
                .into_iter()
                .collect(),
        );
        app.insert_resource(map);
        app
    }

    /// Every marker entity: its name and position.
    fn markers(app: &mut App) -> Vec<(String, Vec3)> {
        let mut q = app
            .world_mut()
            .query_filtered::<(&Name, &Transform), With<crate::selection::MoveMarker>>();
        q.iter(app.world())
            .map(|(n, t)| (n.as_str().to_string(), t.translation))
            .collect()
    }

    /// The right-click system end to end: the hovered child mesh resolves
    /// through `ChildOf` to its unit or building, an `Attack` goes through
    /// `PendingCommands` and is remembered in `IssuedAttacks`, and the
    /// attack marker stands on the target (a building's roof), not at the
    /// ground point under the cursor; with only Yeomen it is a `Move` with
    /// the move marker at the ground point; an enemy-only selection orders
    /// nothing.
    #[test]
    fn a_right_click_resolves_the_hovered_child_and_marks_the_target() {
        let sim = arena();
        let (view, rules) = (sim.view(), sim.rules());
        let ground = pointer_ground();
        for (selected, hover, target) in [
            (
                &[1, 2, 11, 12][..],
                Hover::ChildOfUnit(13),
                TargetId::Unit(UnitId(13)),
            ),
            (
                &[11, 12][..],
                Hover::ChildOfBuilding(5),
                TargetId::Building(BuildingId(5)),
            ),
        ] {
            let mut app = pointer_app(selected, hover);
            app.update();
            let cmds = app
                .world_mut()
                .resource_mut::<PendingCommands>()
                .drain_sorted();
            assert_eq!(cmds.len(), 1, "{target:?}: {cmds:?}");
            assert_eq!(
                cmds[0].cmd,
                Command::Attack {
                    units: vec![UnitId(11), UnitId(12)],
                    target
                }
            );
            assert_eq!(
                app.world().resource::<IssuedAttacks>().target(cmds[0].seq),
                Some(target),
                "remembered under its seq"
            );
            let at = target_point(&view, rules, target).unwrap();
            assert_eq!(
                markers(&mut app),
                [("Attack marker".to_string(), at + Vec3::Y * RING_Y)],
                "on the target, not at the cursor's ground point {ground}"
            );
        }
        // Only Yeomen over the same enemy unit: a Move to the ground point.
        let mut app = pointer_app(&[1, 2], Hover::ChildOfUnit(13));
        app.update();
        let cmds = app
            .world_mut()
            .resource_mut::<PendingCommands>()
            .drain_sorted();
        let [cmd] = &cmds[..] else {
            panic!("one command: {cmds:?}")
        };
        let Command::Move { units, target, .. } = &cmd.cmd else {
            panic!("a Move: {cmd:?}")
        };
        assert_eq!(units, &[UnitId(1), UnitId(2)]);
        let to = world_from_sim(*target, 0.0);
        assert!((to - ground).length() < 0.01, "{to} vs {ground}");
        let placed = markers(&mut app);
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].0, "Move marker");
        assert!((placed[0].1 - (ground + Vec3::Y * RING_Y)).length() < 0.01);
        assert!(app.world().resource::<IssuedAttacks>().0.is_empty());
        // An enemy-only selection: nothing, wherever the pointer is.
        for hover in [Hover::ChildOfUnit(11), Hover::Ground] {
            let mut app = pointer_app(&[6, 13], hover);
            app.update();
            assert!(app.world().resource::<PendingCommands>().is_empty());
            assert!(markers(&mut app).is_empty());
        }
    }

    /// A and S through the real key system: with only another player's
    /// units selected neither does anything; with a mixed selection A
    /// enters attack-move and S stops only the own units.
    #[test]
    fn a_and_s_need_an_own_unit_selected() {
        let mut app = App::new();
        app.init_resource::<OrderMode>()
            .init_resource::<PendingCommands>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(select(&[6, 13]))
            .add_systems(Update, order_mode_keys);
        app.world_mut().insert_non_send(arena_handle());
        let press_a_and_s = |app: &mut App| {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.reset_all();
            keys.press(KeyCode::KeyA);
            keys.press(KeyCode::KeyS);
        };
        press_a_and_s(&mut app);
        app.update();
        assert_eq!(*app.world().resource::<OrderMode>(), OrderMode::Normal);
        assert!(app.world().resource::<PendingCommands>().is_empty());
        // Mixed: P0's Yeoman 1 and Skirmisher 11 with P1's 6 and 13.
        app.insert_resource(select(&[1, 6, 11, 13]));
        press_a_and_s(&mut app);
        app.update();
        assert_eq!(*app.world().resource::<OrderMode>(), OrderMode::AttackMove);
        let cmds = app
            .world_mut()
            .resource_mut::<PendingCommands>()
            .drain_sorted();
        let stops: Vec<&Command> = cmds.iter().map(|c| &c.cmd).collect();
        assert_eq!(
            stops,
            [&Command::Stop {
                units: vec![UnitId(1), UnitId(11)]
            }]
        );
    }

    #[test]
    fn crosshair_cursor_while_in_attack_move_mode() {
        let mut app = App::new();
        app.init_resource::<OrderMode>()
            .add_systems(Update, sync_attack_cursor);
        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        let other = app.world_mut().spawn(Window::default()).id();
        app.update();
        assert!(app.world().get::<CursorIcon>(window).is_none());
        *app.world_mut().resource_mut::<OrderMode>() = OrderMode::AttackMove;
        app.update();
        assert_eq!(
            app.world().get::<CursorIcon>(window),
            Some(&CursorIcon::from(SystemCursorIcon::Crosshair))
        );
        assert!(
            app.world().get::<CursorIcon>(other).is_none(),
            "the primary window only"
        );
        // Placement is not attack-move.
        *app.world_mut().resource_mut::<OrderMode>() = OrderMode::Place(BuildingKindId(0));
        app.update();
        assert!(app.world().get::<CursorIcon>(window).is_none());
        *app.world_mut().resource_mut::<OrderMode>() = OrderMode::AttackMove;
        app.update();
        *app.world_mut().resource_mut::<OrderMode>() = OrderMode::Normal;
        app.update();
        assert!(
            app.world().get::<CursorIcon>(window).is_none(),
            "back to the default"
        );
    }
}
