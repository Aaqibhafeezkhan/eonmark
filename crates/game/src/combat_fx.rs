//! Hit feedback (art pass, owner decision 11; the first thing cut on an
//! overrun): fights read as more than hit points dropping. Presentation
//! only: [`spawn_hit_fx`] reads `SimEvent::Hit` after the sim step and
//! never writes to the sim, its commands or its state.
//!
//! For every hit ([`hit_fx`], from the attacker's data, not its name):
//!
//! - a ranged attacker (a unit kind whose range is over one tile, the
//!   Bowman; or a building, the Town and the Watchtower) looses an arrow,
//!   a [`ARROW_LENGTH_M`] dark streak flying from it to the target in
//!   [`FLIGHT_SECS`] on a low arc ([`ARROW_ARC_M`]) that clears the heads
//!   in between, pointing along its path;
//! - a Siege kind (the Mangonel) lobs a [`STONE_SIDE_M`] stone on an arc
//!   [`STONE_ARC_M`] high in the same time;
//! - a melee attacker lunges [`LUNGE_M`] toward its target and back in
//!   [`LUNGE_SECS`] ([`Lunge`], added to the unit's interpolated pose after
//!   `present::sync_transforms`).
//!
//! Arrows and stones are two shared meshes with two unlit materials
//! (`palette::ARROW`, `palette::STONE_SHOT`; no emissive, nothing glows or
//! pulses) and cast no shadow, so they add two draw bins. They come from a
//! pool of at most [`POOL_CAP`] top-level entities, hidden and reused when
//! they land; a hit while all are in flight draws nothing
//! ([`FxPool::dropped`]). A hit whose attacker is gone (killed in the same
//! tick) draws nothing; a target killed by the hit is aimed at where it
//! was drawn last.

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use rules::{Rules, UnitClass};
use sim::{SimEvent, SimView, TargetId};

use crate::buildings::{building_top_y, footprint_centre};
use crate::models::ModelLibrary;
use crate::palette;
use crate::present::{UnitEntities, UnitRef, sync_lifecycle, sync_transforms, world_from_sim};
use crate::sim_driver::{SimEventMsg, SimHandle, SimSystems, sim_stepped};

/// Most arrows and stones in flight at once.
pub const POOL_CAP: usize = 256;
/// How long an arrow or a stone flies, seconds.
pub const FLIGHT_SECS: f32 = 0.25;
/// An arrow's length, metres.
pub const ARROW_LENGTH_M: f32 = 0.5;
/// An arrow's thickness, metres.
pub const ARROW_THICKNESS_M: f32 = 0.06;
/// A stone's side, metres.
pub const STONE_SIDE_M: f32 = 0.25;
/// A stone's arc above the straight line at its middle, metres.
pub const STONE_ARC_M: f32 = 2.0;
/// An arrow's arc at its middle, metres: low, but enough to carry it over
/// the heads between the archer and the target (figures stand 1.3-1.45 m;
/// a straight shot at chest height flew inside them and could not be seen
/// in the 40 m shots).
pub const ARROW_ARC_M: f32 = 0.8;
/// How far a melee attacker lunges toward its target, metres.
pub const LUNGE_M: f32 = 0.2;
/// How long a lunge lasts, out and back, seconds.
pub const LUNGE_SECS: f32 = 0.15;
/// Where a unit looses an arrow and where one strikes, metres above the
/// ground (chest height of a 1.3-1.45 m figure).
pub const SHOT_Y: f32 = 0.9;
/// A unit kind with a longer range than this (tiles x 100) shoots; up to
/// it, it strikes in melee (the melee kinds reach 25).
pub const MELEE_RANGE_X100: u32 = 100;
/// Share of a building's top a shot leaves from.
pub const BUILDING_SHOT_TOP: f32 = 0.8;

/// What a hit looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitFx {
    /// A dark streak from a ranged attacker.
    Arrow,
    /// A stone on an arc from a Siege kind.
    Stone,
    /// A melee lunge.
    Lunge,
}

/// The look of a hit by `attacker`, from its data: a building shoots
/// arrows; a unit of the Siege class throws stones, one with a range over
/// [`MELEE_RANGE_X100`] shoots arrows, anything else lunges. `None` when
/// the attacker is gone or has no attack.
pub fn hit_fx(view: &SimView<'_>, rules: &Rules, attacker: TargetId) -> Option<HitFx> {
    match attacker {
        TargetId::Building(id) => view.building(id).map(|_| HitFx::Arrow),
        TargetId::Unit(id) => {
            let kind = rules.unit_kind(view.unit(id)?.kind.0)?;
            let attack = kind.attack.as_ref()?;
            Some(if kind.class == UnitClass::Siege {
                HitFx::Stone
            } else if attack.range_tiles_x100 > MELEE_RANGE_X100 {
                HitFx::Arrow
            } else {
                HitFx::Lunge
            })
        }
    }
}

/// One arrow or stone in flight.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Flight {
    /// Where it left, world metres.
    pub from: Vec3,
    /// Where it lands.
    pub to: Vec3,
    /// Its arc's height above the straight line at the middle.
    pub arc: f32,
    /// The fixed clock's time of the tick that launched it ([`fx_now`]).
    pub started: f32,
}

impl Flight {
    /// Its position at `now`, and whether it has landed.
    pub fn at(&self, now: f32) -> (Vec3, bool) {
        let t = ((now - self.started) / FLIGHT_SECS).max(0.0);
        let s = t.min(1.0);
        let pos = self.from.lerp(self.to, s) + Vec3::Y * (self.arc * 4.0 * s * (1.0 - s));
        (pos, t >= 1.0)
    }
}

/// A melee lunge on a unit's entity, removed when it ends.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Lunge {
    /// Toward the target, unit length, on the ground plane.
    pub dir: Vec3,
    /// The fixed clock's time of the tick that started it ([`fx_now`]).
    pub started: f32,
}

impl Lunge {
    /// The offset at `now`: out to [`LUNGE_M`] and back (half a sine), and
    /// whether it has ended.
    pub fn offset(&self, now: f32) -> (Vec3, bool) {
        let t = (now - self.started) / LUNGE_SECS;
        if !(0.0..1.0).contains(&t) {
            return (Vec3::ZERO, t >= 1.0);
        }
        (self.dir * LUNGE_M * (t * std::f32::consts::PI).sin(), false)
    }
}

/// The two meshes and materials every arrow and stone shares.
#[derive(Resource, Debug, Clone)]
pub struct FxAssets {
    /// An arrow, long along its local Z.
    pub arrow: Handle<Mesh>,
    /// A stone.
    pub stone: Handle<Mesh>,
    /// Unlit `palette::ARROW`.
    pub arrow_material: Handle<StandardMaterial>,
    /// Unlit `palette::STONE_SHOT`.
    pub stone_material: Handle<StandardMaterial>,
}

/// The projectile pool.
#[derive(Resource, Debug, Default, Clone)]
pub struct FxPool {
    /// Hidden entities ready to fly again.
    pub free: Vec<Entity>,
    /// Entities spawned so far (at most [`POOL_CAP`]).
    pub spawned: usize,
    /// Hits that drew nothing because every projectile was in flight.
    pub dropped: u64,
}

/// Arrows, stones and lunges from `SimEvent::Hit`. Added by `app::run`.
pub struct CombatFxPlugin;

impl Plugin for CombatFxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FxPool>()
            .add_systems(Startup, create_fx_assets)
            .add_systems(
                FixedUpdate,
                spawn_hit_fx
                    .after(SimSystems::Step)
                    .before(sync_lifecycle)
                    .run_if(sim_stepped),
            )
            .add_systems(Update, fly)
            .add_systems(
                PostUpdate,
                apply_lunges
                    .after(sync_transforms)
                    .before(bevy::transform::TransformSystems::Propagate),
            );
    }
}

/// The arrow (a thin box along Z) and the stone (a cube), each with an
/// unlit material.
pub fn create_fx_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let unlit = |color: Color| StandardMaterial {
        base_color: color,
        unlit: true,
        ..default()
    };
    commands.insert_resource(FxAssets {
        arrow: meshes.add(Cuboid::new(
            ARROW_THICKNESS_M,
            ARROW_THICKNESS_M,
            ARROW_LENGTH_M,
        )),
        stone: meshes.add(Cuboid::from_length(STONE_SIDE_M)),
        arrow_material: materials.add(unlit(palette::ARROW)),
        stone_material: materials.add(unlit(palette::STONE_SHOT)),
    });
}

/// Where a target of a hit is: a unit's position (or, if the hit killed it,
/// where its entity was drawn), a building's footprint centre, at
/// [`SHOT_Y`].
fn target_pos(
    view: &SimView<'_>,
    rules: &Rules,
    target: TargetId,
    drawn: &dyn Fn(sim::UnitId) -> Option<Vec3>,
) -> Option<Vec3> {
    match target {
        TargetId::Unit(id) => view
            .unit(id)
            .map(|u| world_from_sim(u.pos, SHOT_Y))
            .or_else(|| drawn(id).map(|p| Vec3::new(p.x, SHOT_Y, p.z))),
        TargetId::Building(id) => view
            .building(id)
            .map(|b| footprint_centre(rules, b) + Vec3::Y * SHOT_Y),
    }
}

/// After each stepped tick, before the presenter despawns the dead: for
/// every `Hit`, an arrow or a stone from the pool, or a lunge on the
/// attacker's entity.
#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
pub fn spawn_hit_fx(
    sim: NonSend<SimHandle>,
    mut events: MessageReader<SimEventMsg>,
    assets: Option<Res<FxAssets>>,
    models: Option<Res<ModelLibrary>>,
    entities: Res<UnitEntities>,
    units: Query<&Transform, With<UnitRef>>,
    mut pool: ResMut<FxPool>,
    fixed: Res<Time<Fixed>>,
    mut commands: Commands,
) {
    let Some(assets) = assets else {
        events.clear();
        return;
    };
    let view = sim.view();
    let rules = sim.rules();
    // Inside the fixed loop this is the time of the tick being presented,
    // so the hits of several ticks run in one frame start where their
    // ticks put them.
    let now = fixed.elapsed_secs();
    let drawn = |id: sim::UnitId| {
        entities
            .0
            .get(&id)
            .and_then(|e| units.get(*e).ok())
            .map(|t| t.translation)
    };
    for msg in events.read() {
        let SimEvent::Hit {
            attacker, target, ..
        } = msg.event
        else {
            continue;
        };
        let Some(fx) = hit_fx(&view, rules, attacker) else {
            continue;
        };
        let Some(to) = target_pos(&view, rules, target, &drawn) else {
            continue;
        };
        let from = match attacker {
            TargetId::Unit(id) => view.unit(id).map(|u| world_from_sim(u.pos, SHOT_Y)),
            TargetId::Building(id) => view.building(id).map(|b| {
                footprint_centre(rules, b)
                    + Vec3::Y * (building_top_y(rules, b, models.as_deref()) * BUILDING_SHOT_TOP)
            }),
        };
        let Some(from) = from else {
            continue;
        };
        match fx {
            HitFx::Lunge => {
                let TargetId::Unit(id) = attacker else {
                    continue;
                };
                let dir = Vec3::new(to.x - from.x, 0.0, to.z - from.z).normalize_or_zero();
                if let Some(entity) = entities.0.get(&id) {
                    commands.entity(*entity).insert(Lunge { dir, started: now });
                }
            }
            HitFx::Arrow | HitFx::Stone => {
                let flight = Flight {
                    from,
                    to,
                    arc: if fx == HitFx::Stone {
                        STONE_ARC_M
                    } else {
                        ARROW_ARC_M
                    },
                    started: now,
                };
                let (mesh, material) = if fx == HitFx::Stone {
                    (assets.stone.clone(), assets.stone_material.clone())
                } else {
                    (assets.arrow.clone(), assets.arrow_material.clone())
                };
                let transform = flight_transform(&flight, now);
                let bundle = (
                    flight,
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    transform,
                    Visibility::Visible,
                );
                if let Some(entity) = pool.free.pop() {
                    commands.entity(entity).insert(bundle);
                } else if pool.spawned < POOL_CAP {
                    pool.spawned += 1;
                    commands.spawn((Name::new("Projectile"), NotShadowCaster, bundle));
                } else {
                    pool.dropped += 1;
                }
            }
        }
    }
}

/// The presentation time of hit feedback, seconds: the fixed clock's last
/// tick plus its overstep, the same clock `present::sync_transforms`
/// interpolates units with. A projectile started at its tick's fixed time
/// is then where it should be even when a slow frame runs several ticks
/// (on lavapipe one frame runs five), and dropped ticks never desync it.
pub fn fx_now(fixed: &Time<Fixed>) -> f32 {
    fixed.elapsed_secs() + fixed.overstep().as_secs_f32()
}

/// A projectile's transform at `now`: on its path, pointing along it.
pub fn flight_transform(flight: &Flight, now: f32) -> Transform {
    let (pos, _) = flight.at(now);
    let (ahead, _) = flight.at(now + FLIGHT_SECS * 0.05);
    let dir = if (ahead - pos).length_squared() > 1e-8 {
        ahead - pos
    } else {
        flight.to - flight.from
    };
    let mut t = Transform::from_translation(pos);
    if dir.length_squared() > 1e-8 {
        t.look_to(dir, Vec3::Y);
    }
    t
}

/// Every frame: move each projectile along its path; a landed one is
/// hidden, loses its [`Flight`] and goes back to the pool.
pub fn fly(
    fixed: Res<Time<Fixed>>,
    mut flights: Query<(Entity, &Flight, &mut Transform, &mut Visibility)>,
    mut pool: ResMut<FxPool>,
    mut commands: Commands,
) {
    let now = fx_now(&fixed);
    for (entity, flight, mut transform, mut visibility) in &mut flights {
        if flight.at(now).1 {
            *visibility = Visibility::Hidden;
            commands.entity(entity).remove::<Flight>();
            pool.free.push(entity);
        } else {
            *transform = flight_transform(flight, now);
        }
    }
}

/// After the presenter placed every unit this frame: offset the lunging
/// ones, and drop each lunge once it ended.
pub fn apply_lunges(
    fixed: Res<Time<Fixed>>,
    mut lunges: Query<(Entity, &Lunge, &mut Transform)>,
    mut commands: Commands,
) {
    let now = fx_now(&fixed);
    for (entity, lunge, mut transform) in &mut lunges {
        let (offset, ended) = lunge.offset(now);
        transform.translation += offset;
        if ended {
            commands.entity(entity).remove::<Lunge>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rules::Rules;
    use sim::{Command, PlayerCommand, PlayerId, UnitId, UnitKindId};

    fn rules() -> Rules {
        Rules::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
            .expect("rules load")
    }

    fn kind(rules: &Rules, id: &str) -> UnitKindId {
        UnitKindId(u16::try_from(rules.units.iter().position(|k| k.id == id).unwrap()).unwrap())
    }

    /// A standard start with one P0 Bowman, Skirmisher and Mangonel spawned
    /// (ids in that order after the start's units).
    fn sim_with_attackers() -> (SimHandle, [UnitId; 3]) {
        let rules = rules();
        let mut setup = sim::MatchSetup::scenario(&rules, 1);
        setup.start = sim::StartKind::Standard;
        let mut sim = SimHandle::from_setup(setup, rules.clone());
        let at = sim.view().unit(UnitId(1)).unwrap().pos;
        let cmds: Vec<_> = ["bowman", "skirmisher", "mangonel"]
            .iter()
            .enumerate()
            .map(|(i, k)| {
                PlayerCommand::new(
                    PlayerId(0),
                    u32::try_from(i).unwrap(),
                    Command::DebugSpawn {
                        owner: PlayerId(0),
                        kind: kind(&rules, k),
                        at,
                        count: 1,
                    },
                )
            })
            .collect();
        sim.step(&cmds);
        for _ in 0..sim.setup().cmd_delay {
            sim.step_once();
        }
        let ids: Vec<UnitId> = {
            let view = sim.view();
            let mut ids: Vec<UnitId> = view.units().map(|u| u.id).collect();
            ids.sort();
            ids.split_off(ids.len() - 3)
        };
        (sim, [ids[0], ids[1], ids[2]])
    }

    /// An app with the fx assets, the pool and `spawn_hit_fx` and `fly` in
    /// `Update`, over `sim`, with one presenter entity per unit.
    fn fx_app(sim: SimHandle) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SimEventMsg>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<FxPool>()
            .init_resource::<UnitEntities>()
            .add_systems(Startup, create_fx_assets)
            .add_systems(Update, (spawn_hit_fx, fly).chain());
        let ids: Vec<UnitId> = sim.view().units().map(|u| u.id).collect();
        for id in ids {
            let e = app
                .world_mut()
                .spawn((UnitRef(id), Transform::default()))
                .id();
            app.world_mut()
                .resource_mut::<UnitEntities>()
                .0
                .insert(id, e);
        }
        app.world_mut().insert_non_send(sim);
        app.update();
        app
    }

    fn hit(app: &mut App, attacker: UnitId, target: TargetId) {
        app.world_mut().write_message(SimEventMsg {
            tick: 1,
            event: SimEvent::Hit {
                attacker: TargetId::Unit(attacker),
                target,
                damage: 1,
            },
        });
    }

    fn flights(app: &mut App) -> Vec<(Entity, Flight)> {
        app.world_mut()
            .query::<(Entity, &Flight)>()
            .iter(app.world())
            .map(|(e, f)| (e, *f))
            .collect()
    }

    /// The Bowman's hit flies an arrow on its low arc, the Mangonel's a
    /// stone on its high one, both from the attacker to the target; the
    /// Skirmisher's lunges toward the target and flies nothing. Data
    /// decides, not names: the kinds' classes and ranges.
    #[test]
    fn hits_spawn_projectiles_for_ranged_and_lunges_for_melee() {
        let (sim, [bowman, skirmisher, mangonel]) = sim_with_attackers();
        {
            let view = sim.view();
            let rules = sim.rules();
            let fx = |id| hit_fx(&view, rules, TargetId::Unit(id));
            assert_eq!(fx(bowman), Some(HitFx::Arrow));
            assert_eq!(fx(skirmisher), Some(HitFx::Lunge));
            assert_eq!(fx(mangonel), Some(HitFx::Stone));
            assert_eq!(fx(UnitId(1)), None, "a Yeoman has no attack");
            let town = view.buildings().next().unwrap().id;
            assert_eq!(
                hit_fx(&view, rules, TargetId::Building(town)),
                Some(HitFx::Arrow)
            );
        }
        // A P1 Yeoman at the other Seat, far from the attackers.
        let enemy = sim
            .view()
            .units()
            .find(|u| u.owner == PlayerId(1))
            .unwrap()
            .id;
        let target = TargetId::Unit(enemy);
        let mut app = fx_app(sim);
        let assets = app.world().resource::<FxAssets>().clone();

        hit(&mut app, bowman, target);
        app.update();
        let f = flights(&mut app);
        assert_eq!(f.len(), 1, "one arrow");
        let (arrow, flight) = f[0];
        assert_eq!(flight.arc, ARROW_ARC_M);
        assert!(flight.from.distance(flight.to) > 50.0, "Seat to Seat");
        assert_eq!(flight.from.y, SHOT_Y);
        let mesh = app.world().get::<Mesh3d>(arrow).unwrap().0.clone();
        assert_eq!(mesh, assets.arrow);
        assert!(app.world().get::<ChildOf>(arrow).is_none(), "top-level");
        assert!(app.world().get::<NotShadowCaster>(arrow).is_some());

        hit(&mut app, mangonel, target);
        app.update();
        let stones: Vec<_> = flights(&mut app)
            .into_iter()
            .filter(|(_, f)| f.arc == STONE_ARC_M)
            .collect();
        assert_eq!(stones.len(), 1, "one stone");
        let stone = app.world().get::<Mesh3d>(stones[0].0).unwrap().0.clone();
        assert_eq!(stone, assets.stone);
        let (mid, _) = stones[0].1.at(stones[0].1.started + FLIGHT_SECS / 2.0);
        let line = stones[0].1.from.lerp(stones[0].1.to, 0.5);
        assert!((mid.y - line.y - STONE_ARC_M).abs() < 1e-4, "the arc's top");

        let before = flights(&mut app).len();
        hit(&mut app, skirmisher, target);
        app.update();
        assert_eq!(flights(&mut app).len(), before, "melee flies nothing");
        let entity = app.world().resource::<UnitEntities>().0[&skirmisher];
        let lunge = *app
            .world()
            .get::<Lunge>(entity)
            .expect("the Skirmisher lunges");
        assert!((lunge.dir.length() - 1.0).abs() < 1e-4, "{lunge:?}");
        assert!(lunge.dir.x > 0.9, "east, toward P1: {lunge:?}");
        let (peak, ended) = lunge.offset(lunge.started + LUNGE_SECS / 2.0);
        assert!(!ended);
        assert!((peak.length() - LUNGE_M).abs() < 1e-4);
        assert!(lunge.offset(lunge.started + LUNGE_SECS).1, "it ends");
    }

    /// At most `POOL_CAP` projectiles exist: hits past that draw nothing
    /// and are counted; a landed one is hidden and flies again, so the
    /// entity count never grows past the cap.
    #[test]
    fn pool_caps_at_256() {
        let (sim, [bowman, ..]) = sim_with_attackers();
        let mut app = fx_app(sim);
        for _ in 0..POOL_CAP + 44 {
            hit(&mut app, bowman, TargetId::Unit(UnitId(1)));
        }
        app.update();
        assert_eq!(flights(&mut app).len(), POOL_CAP);
        assert_eq!(app.world().resource::<FxPool>().dropped, 44);
        let projectiles = |app: &mut App| {
            app.world_mut()
                .query_filtered::<Entity, With<NotShadowCaster>>()
                .iter(app.world())
                .count()
        };
        assert_eq!(projectiles(&mut app), POOL_CAP);

        // Let them land: every one hides and returns to the pool.
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_secs_f32(FLIGHT_SECS * 2.0));
        app.update();
        assert!(flights(&mut app).is_empty());
        assert_eq!(app.world().resource::<FxPool>().free.len(), POOL_CAP);

        // New hits reuse them.
        for _ in 0..10 {
            hit(&mut app, bowman, TargetId::Unit(UnitId(1)));
        }
        app.update();
        assert_eq!(flights(&mut app).len(), 10);
        assert_eq!(projectiles(&mut app), POOL_CAP, "no new entity");
        assert_eq!(app.world().resource::<FxPool>().dropped, 44);
    }
}
