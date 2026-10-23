//! M4b acceptance tests, sim side: the `enemy_outpost` scenario the owner
//! plays (`--scenario enemy_outpost`), its scripted owner proxy
//! `enemy_outpost_walkthrough` and that proxy's golden fixture. Every test
//! name contains `enemy_outpost`, so `cargo test -p sim enemy_outpost`
//! selects them (the filter matches test paths, not integration-test file
//! names).
//!
//! The tests that are not ignored are cheap: none steps past tick 4. The
//! three that run the 400-unit prologue are `#[ignore]` so `cargo test -p
//! sim` stays under its 10 s budget; CI's headless job runs them with
//! `cargo test -p sim -- --ignored enemy_outpost` and verifies the golden
//! with `sim-cli verify --release`.

use rules::{ModOp, Rules, StatPath, Terrain};
use sim::scenarios::{
    self, ENEMY_OUTPOST_CLEAR_TICK, ENEMY_OUTPOST_ENGINE_YARD, ENEMY_OUTPOST_MUSTER_HALL,
    ENEMY_OUTPOST_MUSTER_UNITS, ENEMY_OUTPOST_PREREQ_MARGIN, ENEMY_OUTPOST_PREWALK,
    ENEMY_OUTPOST_SCRIPTORIUM, ENEMY_OUTPOST_SIEGE_UNITS, ENEMY_OUTPOST_STAGING,
    ENEMY_OUTPOST_T_CHARGE, ENEMY_OUTPOST_TECHS, ENEMY_OUTPOST_TOWER, ENEMY_OUTPOST_TOWER_ID,
    OUTPOST_BATTLE_ARMS, OUTPOST_BATTLE_KINDS, OUTPOST_BATTLE_PER_GROUP, OUTPOST_GRANT_PERMILLE,
    Stream, commands_at,
};
use sim::{
    BuildingId, BuildingKindId, Command, Construction, MatchSetup, PlayerCommand, PlayerId,
    QueueItem, Sim, SimEvent, SimView, StartKind, TargetId, TechId, Tile, UnitId, UnitKindId,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const P0: PlayerId = PlayerId(0);
const P1: PlayerId = PlayerId(1);

struct NoAi;

impl sim::AiController for NoAi {
    fn think(&mut self, _player: PlayerId, _view: &SimView<'_>) -> Vec<Command> {
        Vec::new()
    }
}

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn load_rules() -> Rules {
    Rules::load(data_dir()).expect("data/ loads")
}

fn bkind(rules: &Rules, id: &str) -> BuildingKindId {
    BuildingKindId(
        rules
            .building_kind_by_id(id)
            .expect("kind in buildings.ron")
            .0,
    )
}

fn ukind(rules: &Rules, id: &str) -> UnitKindId {
    let i = rules
        .units
        .iter()
        .position(|u| u.id == id)
        .expect("kind in units.ron");
    UnitKindId(u16::try_from(i).expect("validated unit count"))
}

fn tech(rules: &Rules, id: &str) -> TechId {
    TechId(rules.tech_by_id(id).expect("tech in techs.ron").0)
}

/// Both outpost scenarios by name, as the game and `sim-cli` load them.
fn both(rules: &Rules) -> [(&'static str, MatchSetup, Stream); 2] {
    ["enemy_outpost", "enemy_outpost_walkthrough"].map(|name| {
        let (setup, stream, _) = scenarios::by_name(name, rules).expect(name);
        (name, setup, stream)
    })
}

/// Run `stream` on a fresh sim for `ticks` steps, collecting every event
/// with the tick it happened during.
fn run_events(
    setup: MatchSetup,
    rules: &Rules,
    stream: &Stream,
    ticks: u32,
) -> (Sim, Vec<(u32, SimEvent)>) {
    let mut sim = Sim::new(setup, rules.clone(), Box::new(NoAi));
    let mut events = Vec::new();
    for t in 0..ticks {
        sim.step(commands_at(stream, t));
        events.extend(sim.drain_events().into_iter().map(|e| (t, e)));
    }
    (sim, events)
}

/// Distance in hundredths of a tile, squared, between the square of tile
/// `t` and the tile rectangle `[x0, x1 + 1] x [y0, y1 + 1]`: the nearest any
/// point of the tile (so any unit centred on it) comes to that rectangle.
fn tile_to_rect_sq_x100(t: Tile, x0: u16, y0: u16, x1: u16, y1: u16) -> i64 {
    let gap = |a: u16, lo: u16, hi: u16| -> i64 {
        let (a, lo, hi) = (i64::from(a), i64::from(lo), i64::from(hi));
        (lo - (a + 1)).max(a - (hi + 1)).max(0) * 100
    };
    let (dx, dy) = (gap(t.x, x0, x1), gap(t.y, y0, y1));
    dx * dx + dy * dy
}

/// The tower's footprint rectangle as `(x0, y0, x1, y1)` tiles.
fn tower_rect(rules: &Rules) -> (u16, u16, u16, u16) {
    let def = rules
        .building_kind(bkind(rules, "watchtower").0)
        .expect("watchtower");
    let w = u16::try_from(def.width_tiles).unwrap();
    let h = u16::try_from(def.height_tiles).unwrap();
    let t = ENEMY_OUTPOST_TOWER;
    (t.x, t.y, t.x + w - 1, t.y + h - 1)
}

/// Both scenarios' tick-0 setup, applied (ticks 0..=2): the standard start,
/// P0's vanguard of 200 and its grants, P1's Arms I to III, its complete
/// Watchtower `BuildingId(3)` at (58, 64) and its army of 200, no two
/// units on one tile, the armies out of each other's reach, and nothing
/// rejected. The walkthrough's first own beat (ticks 3 and 4) then places
/// P0's Scriptorium and Muster Hall as `BuildingId(4)` and `BuildingId(5)`.
#[test]
fn enemy_outpost_setup_is_valid() {
    let rules = load_rules();
    let start_units: u32 = rules.standard_start.units.values().sum();
    let per_side =
        u32::from(OUTPOST_BATTLE_PER_GROUP) * u32::try_from(OUTPOST_BATTLE_KINDS.len()).unwrap();
    let vanguard = 2 * start_units + 1..=2 * start_units + per_side;
    let army = 2 * start_units + per_side + 1..=2 * start_units + 2 * per_side;
    assert_eq!((vanguard.clone(), army.clone()), (11..=210, 211..=410));
    let grants = scenarios::outpost_grants(&rules);
    for (name, setup, stream) in both(&rules) {
        assert_eq!(setup.start, StartKind::Standard, "{name}");
        assert_eq!(setup.seed, scenarios::ENEMY_OUTPOST_SEED, "{name}");
        assert!(setup.debug_commands, "{name}");
        assert!(setup.players.iter().all(|p| !p.is_ai), "{name}");
        let delay = setup.cmd_delay;
        let (sim, events) = run_events(setup, &rules, &stream, 1 + delay);
        let view = sim.view();
        // Nothing in the setup is rejected.
        let rejected: Vec<_> = events
            .iter()
            .filter(|(_, e)| matches!(e, SimEvent::CommandRejected { .. }))
            .collect();
        assert!(rejected.is_empty(), "{name}: {rejected:?}");
        // The tower.
        let tower = view.building(ENEMY_OUTPOST_TOWER_ID).expect("tower");
        assert_eq!(tower.owner, P1, "{name}");
        assert_eq!(tower.kind, bkind(&rules, "watchtower"), "{name}");
        assert_eq!(tower.tile, ENEMY_OUTPOST_TOWER, "{name}");
        assert_eq!(tower.construction, Construction::Complete, "{name}");
        let max = i32::try_from(rules.building_kind(tower.kind.0).unwrap().hp).unwrap();
        assert_eq!(
            view.building_hp(ENEMY_OUTPOST_TOWER_ID),
            Some((max, max)),
            "{name}"
        );
        assert_eq!(
            view.buildings().count(),
            3,
            "{name}: two Seats and the tower"
        );
        // 410 units: the standard start's, then the vanguard and the army,
        // each side's in its four groups of one kind.
        assert_eq!(
            view.unit_count(),
            usize::try_from(2 * start_units + 2 * per_side).unwrap(),
            "{name}"
        );
        for u in view.units() {
            // (owner, first id of the side's battle groups) or a start unit.
            let (owner, groups_from) = if u.id.0 <= start_units {
                (P0, None)
            } else if u.id.0 <= 2 * start_units {
                (P1, None)
            } else if vanguard.contains(&u.id.0) {
                (P0, Some(*vanguard.start()))
            } else {
                assert!(army.contains(&u.id.0), "{name}: {:?}", u.id);
                (P1, Some(*army.start()))
            };
            assert_eq!(u.owner, owner, "{name}: {:?}", u.id);
            if let Some(first) = groups_from {
                let g = (u.id.0 - first) / u32::from(OUTPOST_BATTLE_PER_GROUP);
                let kind = OUTPOST_BATTLE_KINDS[usize::try_from(g).unwrap()];
                assert_eq!(u.kind, ukind(&rules, kind), "{name}: {:?}", u.id);
                assert!(u.order.is_none() && u.target.is_none(), "{name}: idle");
            }
        }
        // No two units share a spawn tile. `DebugSpawn` puts one unit per
        // passable tile of the square spiral around its centre and does not
        // skip occupied tiles, so a control run of the same stream with
        // every `DebugSpawnJittered` made a plain `DebugSpawn` leaves every
        // unit exactly on its own tile's centre after the spawn tick: two
        // units on one tile would coincide, and the overlap correction
        // would push both off it. With the jitter no two units share a
        // position.
        let control: Stream = stream
            .iter()
            .map(|(t, cmds)| {
                let cmds = cmds
                    .iter()
                    .map(|c| match c.cmd {
                        Command::DebugSpawnJittered {
                            owner,
                            kind,
                            at,
                            count,
                            ..
                        } => PlayerCommand::new(
                            c.player,
                            c.seq,
                            Command::DebugSpawn {
                                owner,
                                kind,
                                at,
                                count,
                            },
                        ),
                        _ => c.clone(),
                    })
                    .collect();
                (*t, cmds)
            })
            .collect();
        let (control, _) = run_events(sim.setup().clone(), &rules, &control, 1 + delay);
        let cview = control.view();
        let map = cview.map();
        assert_eq!(cview.unit_count(), view.unit_count(), "{name}");
        let mut tiles = BTreeSet::new();
        for u in cview.units() {
            let t = map.tile_of(u.pos);
            assert_eq!(
                u.pos,
                map.center_of(t),
                "{name}: {:?} pushed off {t:?}",
                u.id
            );
            assert!(tiles.insert((t.x, t.y)), "{name}: two units on {t:?}");
        }
        let positions: BTreeSet<(i64, i64)> = view
            .units()
            .map(|u| (u.pos.x.to_bits(), u.pos.y.to_bits()))
            .collect();
        assert_eq!(
            positions.len(),
            view.unit_count(),
            "{name}: shared positions"
        );
        // The armies start beyond each other's reach: for every pair, the
        // longer acquisition reach plus both radii plus a tile.
        let reach_x100 = |k: UnitKindId| {
            let def = rules.unit_kind(k.0).unwrap();
            def.attack.as_ref().map_or(0, |a| rules.acquire_reach(a))
        };
        let radius_x100 = |k: UnitKindId| rules.unit_kind(k.0).unwrap().radius_tiles_x100;
        let mut closest = i64::MAX;
        for a in view.units().filter(|u| vanguard.contains(&u.id.0)) {
            for b in view.units().filter(|u| army.contains(&u.id.0)) {
                let gap = i64::from(reach_x100(a.kind).max(reach_x100(b.kind)))
                    + i64::from(radius_x100(a.kind) + radius_x100(b.kind))
                    + 100;
                // dist_sq_i64 is in I32F32 scale: tiles^2 * 2^32.
                let d_sq_x100 = (i128::from(a.pos.dist_sq_i64(b.pos)) * 10_000) >> 32;
                let d_sq_x100 = i64::try_from(d_sq_x100).unwrap();
                assert!(d_sq_x100 > gap * gap, "{name}: {:?} and {:?}", a.id, b.id);
                closest = closest.min(d_sq_x100);
            }
        }
        // P1's Arms I to III; P0 has its grants on top of the start stock.
        let p1_techs: BTreeSet<TechId> = OUTPOST_BATTLE_ARMS
            .iter()
            .map(|t| tech(&rules, t))
            .collect();
        assert_eq!(view.researched(P1), &p1_techs, "{name}");
        assert!(view.researched(P0).is_empty(), "{name}");
        let stock = &view.economy(P0).stock;
        for (r, res) in rules.resources.resources.iter().enumerate() {
            let start = i64::from(rules.standard_start.stockpile[&res.id]);
            let want = start + i64::from(grants[r]);
            let want = res.stockpile_cap.map_or(want, |c| want.min(i64::from(c)));
            assert_eq!(stock[r], want, "{name}: {}", res.id);
        }
        // Pop: 255 / 25 for P0 until the vanguard is gone; Arms raises P1's
        // cap (its `PopCap` rows).
        let pop_of = |p: PlayerId| -> u32 {
            view.units()
                .filter(|u| u.owner == p)
                .map(|u| rules.unit_kind(u.kind.0).unwrap().pop)
                .sum()
        };
        assert_eq!(view.pop(P0), (pop_of(P0), rules.pop_cap_base), "{name}");
        assert!(view.pop(P0).0 > view.pop(P0).1, "{name}: over the cap");
        let arms_cap = rules::modifiers::resolve(
            i64::from(rules.pop_cap_base),
            rules
                .modifier_rows(0, p1_techs.iter().map(|t| t.0))
                .filter(|m| m.target == StatPath::PopCap),
        );
        assert_eq!(i64::from(view.pop(P1).1), arms_cap, "{name}");
        assert!(view.pop(P1).1 > rules.pop_cap_base, "{name}");
        println!(
            "{name}: tick {} units {} pop P0 {:?} P1 {:?} stock P0 {stock:?} closest armies {} hundredths of a tile",
            view.tick,
            view.unit_count(),
            view.pop(P0),
            view.pop(P1),
            closest.isqrt()
        );
    }
    // The walkthrough's first beat places P0's Scriptorium and Muster Hall
    // as BuildingId(4) and (5) once it applies.
    let (setup, stream) = scenarios::enemy_outpost_walkthrough(&rules);
    let beats = scenarios::enemy_outpost_beats(&rules);
    let ticks = beats.build + setup.cmd_delay + 1;
    assert!(ticks <= 5, "never past tick 4");
    let (sim, events) = run_events(setup, &rules, &stream, ticks);
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, SimEvent::CommandRejected { .. }))
    );
    let view = sim.view();
    for (id, kind, tile) in [
        (4, "scriptorium", ENEMY_OUTPOST_SCRIPTORIUM),
        (5, "muster_hall", ENEMY_OUTPOST_MUSTER_HALL),
    ] {
        let b = view.building(BuildingId(id)).expect("placed");
        assert_eq!((b.owner, b.kind, b.tile), (P0, bkind(&rules, kind), tile));
    }
}

/// The Watchtower cannot shoot anything P0 has or does in the
/// walkthrough until the siege: every tile P0 owns at tick 3, every
/// walkthrough footprint, the pre-walk and staging tiles and every
/// Mountain tile west of the river (where an Ore Pit's workers stand) lie
/// beyond the tower's reach (its range plus the widest unit radius, from
/// any point of the tile). The staging tile also lies beyond a Mangonel's
/// own acquisition reach of the tower plus a tile, so the army waits there
/// until it is ordered in.
#[test]
fn enemy_outpost_tower_cannot_reach_p0s_start_territory() {
    let rules = load_rules();
    let (x0, y0, x1, y1) = tower_rect(&rules);
    let tower = rules.building_kind(bkind(&rules, "watchtower").0).unwrap();
    let widest = rules
        .units
        .iter()
        .map(|u| u.radius_tiles_x100)
        .max()
        .unwrap();
    let reach = i64::from(
        tower
            .attack
            .as_ref()
            .expect("the tower shoots")
            .range_tiles_x100,
    ) + i64::from(widest);
    let beyond = |t: Tile, r: i64| tile_to_rect_sq_x100(t, x0, y0, x1, y1) > r * r;
    let (setup, stream) = scenarios::enemy_outpost(&rules);
    let delay = setup.cmd_delay;
    let (sim, _) = run_events(setup, &rules, &stream, 1 + delay);
    let view = sim.view();
    let map = view.map();
    let mut owned = 0;
    for y in 0..map.height() {
        for x in 0..map.width() {
            let t = Tile::new(x, y);
            if view.territory().owner_at(t) == Some(P0) {
                owned += 1;
                assert!(beyond(t, reach), "P0's {t:?} is in the tower's reach");
            }
        }
    }
    assert!(owned > 1000, "P0 owns its Seat's border: {owned}");
    // The walkthrough's footprints.
    for (kind, at) in [
        ("scriptorium", ENEMY_OUTPOST_SCRIPTORIUM),
        ("muster_hall", ENEMY_OUTPOST_MUSTER_HALL),
        ("engine_yard", ENEMY_OUTPOST_ENGINE_YARD),
    ] {
        let def = rules.building_kind(bkind(&rules, kind).0).unwrap();
        for dy in 0..u16::try_from(def.height_tiles).unwrap() {
            for dx in 0..u16::try_from(def.width_tiles).unwrap() {
                let t = Tile::new(at.x + dx, at.y + dy);
                assert!(beyond(t, reach), "{kind} {t:?}");
                assert_eq!(view.territory().owner_at(t), Some(P0), "{kind} inside");
            }
        }
    }
    let prewalk = map.tile_of(ENEMY_OUTPOST_PREWALK);
    let staging = map.tile_of(ENEMY_OUTPOST_STAGING);
    assert!(beyond(prewalk, reach));
    // The pre-walk tile is at least a tile off the Engine Yard's footprint.
    let yard = rules.building_kind(bkind(&rules, "engine_yard").0).unwrap();
    let e = ENEMY_OUTPOST_ENGINE_YARD;
    let off = tile_to_rect_sq_x100(
        prewalk,
        e.x,
        e.y,
        e.x + u16::try_from(yard.width_tiles).unwrap() - 1,
        e.y + u16::try_from(yard.height_tiles).unwrap() - 1,
    );
    assert!(off >= 100 * 100, "pre-walk {prewalk:?} beside the yard");
    let mangonel = rules.unit_kind(ukind(&rules, "mangonel").0).unwrap();
    let siege_reach = i64::from(rules.acquire_reach(mangonel.attack.as_ref().unwrap()))
        + i64::from(mangonel.radius_tiles_x100)
        + 100;
    assert!(beyond(staging, reach));
    assert!(beyond(staging, siege_reach), "staging {staging:?}");
    // Mountains west of the river (the middle ford is at columns 62-65):
    // an Ore Pit's workers stand up to a tile off them.
    let def = rules.map(&rules.default_map).unwrap();
    let mut mountains = 0;
    for y in 0..map.height() {
        for x in 0..62 {
            if def.tile(x, y) == Terrain::Mountain {
                mountains += 1;
                assert!(beyond(Tile::new(x, y), reach + 100), "Mountain ({x}, {y})");
            }
        }
    }
    assert!(mountains > 0);
}

/// After tick 0 the walkthrough issues no debug command, and only P0
/// issues commands except P1's attack-move during `T_CHARGE`; P0's own
/// commands start at seq 8, follow the beats in tick order and are a
/// superset of `enemy_outpost`'s stream, which is exactly the setup.
#[test]
fn enemy_outpost_walkthrough_issues_only_real_commands_after_setup() {
    let rules = load_rules();
    let (_, setup_stream) = scenarios::outpost_setup(&rules);
    let (_, outpost) = scenarios::enemy_outpost(&rules);
    assert_eq!(outpost, setup_stream);
    let (_, stream) = scenarios::enemy_outpost_walkthrough(&rules);
    let beats = scenarios::enemy_outpost_beats(&rules);
    let is_debug = |c: &Command| {
        matches!(
            c,
            Command::DebugSpawn { .. }
                | Command::DebugSpawnJittered { .. }
                | Command::DebugPlaceBuilding { .. }
                | Command::DebugGrant { .. }
                | Command::DebugResearch { .. }
        )
    };
    let mut p0_seq = Vec::new();
    for (tick, cmds) in &stream {
        for c in cmds {
            if *tick == 0 {
                assert!(is_debug(&c.cmd), "{c:?}");
                continue;
            }
            assert!(!is_debug(&c.cmd), "tick {tick}: {c:?}");
            if c.player == P1 {
                assert_eq!(*tick, ENEMY_OUTPOST_T_CHARGE, "{c:?}");
                assert!(matches!(c.cmd, Command::AttackMove { .. }), "{c:?}");
            } else {
                assert_eq!(c.player, P0, "{c:?}");
                p0_seq.push(c.seq);
            }
        }
        // Every setup command is in the walkthrough unchanged.
        for c in commands_at(&setup_stream, *tick) {
            assert!(cmds.contains(c), "tick {tick}: {c:?}");
        }
    }
    let setup_p0 = setup_stream
        .iter()
        .flat_map(|(_, c)| c)
        .filter(|c| c.player == P0)
        .count();
    assert_eq!(setup_p0, 8, "P0's tick-0 seq 0..=7");
    let n = u32::try_from(p0_seq.len()).unwrap();
    assert_eq!(
        p0_seq,
        (8..8 + n).collect::<Vec<u32>>(),
        "seq in tick order"
    );
    // P0's commands land on the beats and nowhere else.
    let p0_ticks: BTreeSet<u32> = stream
        .iter()
        .filter(|(t, cmds)| *t > 0 && cmds.iter().any(|c| c.player == P0))
        .map(|(t, _)| *t)
        .collect();
    let want: BTreeSet<u32> = [
        beats.build,
        beats.research_1,
        beats.muster,
        beats.research_2,
        beats.age,
        beats.yard,
        beats.siege,
    ]
    .into();
    assert_eq!(p0_ticks, want, "{beats:?}");
    assert!(stream.last().is_some_and(|(t, _)| *t < beats.ticks));
    // AdvanceAge comes before the Engine Yard's Build in its tick.
    let age = commands_at(&stream, beats.age);
    let pos = |f: &dyn Fn(&PlayerCommand) -> bool| age.iter().position(f).unwrap();
    assert!(
        pos(&|c| matches!(c.cmd, Command::AdvanceAge { .. }))
            < pos(&|c| matches!(c.cmd, Command::Build { .. }))
    );
}

/// The walkthrough's bill, rebuilt from the commands its stream issues
/// (each Build at its kind's ramp, each Research at the quote the techs
/// researched before it give, with the `ResearchCost` rows resolved here,
/// the age's cost, each Train at both ramp inputs the sim's
/// `unit_cost_for` reads: `n`, the units of its kind trained or queued
/// before it, which a `PerUnit` ramp reads (P0 has none of these kinds
/// alive at either Train beat), and `k`, its position in its trainer
/// kind's queue, which the `Military` ramp reads), equals
/// `enemy_outpost_bill`; the grants are 1.5x it clamped to the stockpile
/// caps, so they cover it.
#[test]
fn enemy_outpost_grants_cover_the_walkthrough_bill() {
    let rules = load_rules();
    let (_, stream) = scenarios::enemy_outpost_walkthrough(&rules);
    let n_res = rules.resources.resources.len();
    let mut bill = vec![0i64; n_res];
    let mut add = |cost: Vec<i64>| {
        for (b, c) in bill.iter_mut().zip(cost) {
            *b += c;
        }
    };
    let mut built: BTreeMap<BuildingKindId, u32> = BTreeMap::new();
    // Building kind of each BuildingId P0 places, in placement order after
    // the tower; queued entries per trainer kind.
    let mut placed: BTreeMap<BuildingId, BuildingKindId> = BTreeMap::new();
    let mut queued: BTreeMap<BuildingKindId, u32> = BTreeMap::new();
    // Units of each kind trained or queued so far: the `n` of `unit_cost`.
    let mut same_kind: BTreeMap<UnitKindId, u32> = BTreeMap::new();
    let mut researched: Vec<u16> = Vec::new();
    let mut age = 0usize;
    for (_, cmds) in stream.iter().filter(|(t, _)| *t > 0) {
        for c in cmds.iter().filter(|c| c.player == P0) {
            match &c.cmd {
                Command::Build { kind, .. } => {
                    let n = built.entry(*kind).or_insert(0);
                    add(rules.building_cost(rules.building_kind(kind.0).unwrap(), *n));
                    *n += 1;
                    let id = BuildingId(
                        ENEMY_OUTPOST_TOWER_ID.0 + 1 + u32::try_from(placed.len()).unwrap(),
                    );
                    placed.insert(id, *kind);
                }
                Command::Research { tech, .. } => {
                    let t = rules.tech(tech.0).unwrap();
                    let rows: Vec<_> = rules
                        .modifier_rows(0, researched.iter().copied())
                        .filter(|m| m.target == StatPath::ResearchCost)
                        .cloned()
                        .collect();
                    assert!(rows.iter().all(|m| m.op == ModOp::Mul), "{rows:?}");
                    let mut cost = vec![0i64; n_res];
                    for (id, &v) in &t.cost {
                        let r = rules.resource_index(id).unwrap();
                        cost[r] = rows
                            .iter()
                            .fold(i64::from(v), |v, m| v * i64::from(m.value) / 1000);
                    }
                    add(cost);
                    researched.push(tech.0);
                }
                Command::AdvanceAge { .. } => {
                    age += 1;
                    add(rules.amounts(&rules.ages[age].cost));
                }
                Command::Train { building, kind } => {
                    let trainer = placed[building];
                    let k = queued.entry(trainer).or_insert(0);
                    *k += 1;
                    let n = same_kind.entry(*kind).or_insert(0);
                    add(rules.unit_cost(rules.unit_kind(kind.0).unwrap(), *n, *k));
                    *n += 1;
                }
                _ => {}
            }
        }
    }
    assert_eq!(bill, scenarios::enemy_outpost_bill(&rules));
    let grants = scenarios::outpost_grants(&rules);
    for (r, res) in rules.resources.resources.iter().enumerate() {
        let want = bill[r] * OUTPOST_GRANT_PERMILLE / 1000;
        let want = res.stockpile_cap.map_or(want, |c| want.min(i64::from(c)));
        assert_eq!(i64::from(grants[r]), want, "{}", res.id);
        assert!(
            i64::from(grants[r]) >= bill[r],
            "{} {grants:?} {bill:?}",
            res.id
        );
    }
    println!("enemy_outpost bill {bill:?} grants {grants:?}");
}

/// The prologue (`enemy_outpost`): P1's army with Arms I to III attack-moves
/// into P0's idle vanguard, both sides land hits and lose units over at
/// least 100 fighting ticks (a hit while both sides have at least 50 units,
/// the M4a bench threshold), and by `ENEMY_OUTPOST_CLEAR_TICK` P0 has only
/// its five Yeomen and its pop is back to 5 / 25.
#[test]
#[ignore = "runs the 400-unit prologue; CI runs it with -- --ignored enemy_outpost"]
fn enemy_outpost_battle_clears_the_vanguard() {
    let rules = load_rules();
    let (setup, stream) = scenarios::enemy_outpost(&rules);
    let mut sim = Sim::new(setup, rules.clone(), Box::new(NoAi));
    let start_units: u32 = rules.standard_start.units.values().sum();
    let side = |u: UnitId| usize::from(u.0 > 2 * start_units + 200);
    let mut hits = [0u32; 2];
    let mut deaths = [0u32; 2];
    let mut fighting = 0u32;
    let (mut first_hit, mut wipe) = (None, None);
    let mut alive = [0usize; 2];
    for t in 0..ENEMY_OUTPOST_CLEAR_TICK {
        sim.step(commands_at(&stream, t));
        let view = sim.view();
        alive = [P0, P1].map(|p| view.units().filter(|u| u.owner == p).count());
        let mut hit = false;
        for e in sim.drain_events() {
            match e {
                SimEvent::Hit {
                    attacker: TargetId::Unit(a),
                    target: TargetId::Unit(_),
                    ..
                } => {
                    hits[side(a)] += 1;
                    hit = true;
                    first_hit.get_or_insert(t);
                }
                SimEvent::Hit { .. } => panic!("only units fight in the prologue: {e:?}"),
                SimEvent::UnitDied { owner, unit, .. } => {
                    deaths[usize::from(owner.0)] += 1;
                    assert!(unit.0 > 2 * start_units, "a Yeoman died: {unit:?}");
                }
                SimEvent::CommandRejected { .. } => panic!("{e:?}"),
                _ => {}
            }
        }
        if hit && alive.iter().all(|&n| n >= 50 + 5) {
            fighting += 1;
        }
        let vanguard_left = sim
            .view()
            .units()
            .any(|u| u.owner == P0 && u.id.0 > 2 * start_units);
        if !vanguard_left && wipe.is_none() && first_hit.is_some() {
            wipe = Some(t);
        }
    }
    let first_hit = first_hit.expect("the armies met");
    println!(
        "enemy_outpost prologue: first hit during tick {first_hit} (T_CHARGE {ENEMY_OUTPOST_T_CHARGE}), \
         hits P0 {} P1 {}, deaths P0 {} P1 {}, fighting ticks {fighting}, \
         vanguard wiped during tick {wipe:?}, P1 survivors {}, CLEAR_TICK {ENEMY_OUTPOST_CLEAR_TICK}",
        hits[0],
        hits[1],
        deaths[0],
        deaths[1],
        alive[1] - 5
    );
    assert!(
        first_hit > ENEMY_OUTPOST_T_CHARGE,
        "no fight before the charge"
    );
    assert!(hits.iter().all(|&h| h > 0), "both sides hit: {hits:?}");
    assert!(
        deaths.iter().all(|&d| d > 0),
        "both sides lose units: {deaths:?}"
    );
    assert!(fighting >= 100, "fighting ticks {fighting}");
    let wipe = wipe.expect("the vanguard is wiped by CLEAR_TICK");
    assert!(
        wipe + 100 <= ENEMY_OUTPOST_CLEAR_TICK,
        "wiped during {wipe}"
    );
    let view = sim.view();
    assert_eq!(view.tick, ENEMY_OUTPOST_CLEAR_TICK);
    let p0: Vec<u32> = view
        .units()
        .filter(|u| u.owner == P0)
        .map(|u| u.id.0)
        .collect();
    assert_eq!(p0, (1..=start_units).collect::<Vec<u32>>());
    assert_eq!(view.pop(P0), (start_units, rules.pop_cap_base));
}

/// The scripted owner proxy, beat by beat, on the real sim: no command is
/// rejected; P0 completes the Scriptorium, the Muster Hall and the Engine
/// Yard (`BuildingId(4..=6)`), researches Letters I and then Trade I at the
/// Letters price, enters the Masonry Age, trains its mixed army and two
/// Mangonels (`UnitId(411..=417)`, by `Train`); the Mangonels, which
/// outrange the tower and take none of its shots, hit it at least three
/// times, and with the attack-moving army destroy it by `destroy_by` (a
/// Mangonel hit does 120 of its 500 HP, so the army's hits count too); the
/// footprint is passable and the tower's border source gone afterwards. Every
/// prerequisite is met at least `ENEMY_OUTPOST_PREREQ_MARGIN` ticks before
/// the command that needs it, and the vanguard is wiped at least 100 ticks
/// before `ENEMY_OUTPOST_CLEAR_TICK`, as in `enemy_outpost` (both
/// scenarios fight the same prologue).
#[test]
#[ignore = "runs the 400-unit prologue and the build-up (about 5000 ticks); CI runs it with -- --ignored enemy_outpost"]
fn enemy_outpost_walkthrough_beats_happen() {
    let rules = load_rules();
    let (setup, stream) = scenarios::enemy_outpost_walkthrough(&rules);
    let beats = scenarios::enemy_outpost_beats(&rules);
    let delay = setup.cmd_delay;
    let mut sim = Sim::new(setup, rules.clone(), Box::new(NoAi));
    let start_units: u32 = rules.standard_start.units.values().sum();
    let trained = scenarios::enemy_outpost_trained_ids(&rules);
    let (army, siege) = trained.split_at(ENEMY_OUTPOST_MUSTER_UNITS.len());
    let tower = TargetId::Building(ENEMY_OUTPOST_TOWER_ID);
    let (x0, y0, x1, y1) = tower_rect(&rules);
    let mut completed: BTreeMap<BuildingId, u32> = BTreeMap::new();
    let mut research: Vec<(TechId, u32)> = Vec::new();
    let mut aged = None;
    let mut spawned: Vec<(UnitId, UnitKindId, u32)> = Vec::new();
    let mut wipe = None;
    let mut siege_hits = Vec::new();
    let mut destroyed = None;
    let mut trade_ticks = None;
    let mut tower_shots_on_siege = 0;
    let mut tower_shots = 0;
    for t in 0..beats.ticks {
        sim.step(commands_at(&stream, t));
        let events = sim.drain_events();
        let view = sim.view();
        for e in events {
            match e {
                SimEvent::CommandRejected { .. } => panic!("tick {t}: {e:?}"),
                SimEvent::BuildingCompleted { building } if building > ENEMY_OUTPOST_TOWER_ID => {
                    assert_eq!(view.building(building).unwrap().owner, P0);
                    completed.insert(building, t);
                }
                SimEvent::ResearchCompleted { player: P0, tech } => research.push((tech, t)),
                SimEvent::AgeAdvanced { player, age } => {
                    assert_eq!((player, age), (P0, 1), "tick {t}");
                    aged = Some(t);
                }
                SimEvent::UnitSpawned { unit } if t > delay => {
                    let u = view.unit(unit).expect("spawned");
                    assert_eq!(u.owner, P0, "{unit:?}");
                    spawned.push((unit, u.kind, t));
                }
                SimEvent::UnitDied { unit, .. } => {
                    assert!(unit.0 > 2 * start_units, "a Yeoman died during tick {t}");
                }
                SimEvent::Hit {
                    attacker: TargetId::Unit(a),
                    target,
                    ..
                } if target == tower => {
                    assert!(
                        t >= beats.siege + delay,
                        "tower hit before the siege, tick {t}"
                    );
                    if siege.contains(&a) {
                        siege_hits.push((a, t));
                    }
                }
                SimEvent::Hit {
                    attacker,
                    target: TargetId::Unit(u),
                    ..
                } if attacker == tower => {
                    assert!(
                        t >= beats.siege + delay,
                        "tower shot before the siege, tick {t}"
                    );
                    tower_shots += 1;
                    if siege.contains(&u) {
                        tower_shots_on_siege += 1;
                    }
                }
                SimEvent::BuildingDestroyed {
                    building,
                    owner,
                    kind,
                } => {
                    assert_eq!(building, ENEMY_OUTPOST_TOWER_ID, "tick {t}");
                    assert_eq!((owner, kind), (P1, bkind(&rules, "watchtower")));
                    destroyed = Some(t);
                }
                _ => {}
            }
        }
        let vanguard_left = view
            .units()
            .any(|u| u.owner == P0 && u.id.0 > 2 * start_units && u.id < army[0]);
        if wipe.is_none() && !vanguard_left && t > ENEMY_OUTPOST_T_CHARGE {
            wipe = Some(t);
        }
        // Trade I is queued at the Letters price once its command applies.
        if t == beats.research_2 + delay {
            let s = view.building(BuildingId(4)).unwrap();
            let head = s.queue.first().expect("Trade I queued");
            let QueueItem::Research {
                tech: queued,
                ticks,
            } = head.item
            else {
                panic!("{head:?}");
            };
            assert_eq!(queued, tech(&rules, ENEMY_OUTPOST_TECHS[1]));
            let base = rules.ticks_from_ds(rules.tech(queued.0).unwrap().research_time_ds);
            assert_eq!((head.paid.clone(), ticks), view.research_quote(P0, queued));
            assert!(ticks < base, "the Letters discount: {ticks} < {base}");
            trade_ticks = Some(ticks);
        }
    }
    // Buildings 4, 5 and 6 with their kinds.
    let view = sim.view();
    let kinds: Vec<(u32, &str)> = vec![(4, "scriptorium"), (5, "muster_hall"), (6, "engine_yard")];
    for (id, kind) in &kinds {
        let b = view.building(BuildingId(*id)).expect("standing");
        assert_eq!((b.owner, b.kind), (P0, bkind(&rules, kind)), "{id}");
        assert!(completed.contains_key(&BuildingId(*id)), "{kind} completed");
    }
    assert_eq!(completed.len(), 3, "{completed:?}");
    // Letters I, then Trade I.
    let techs: Vec<TechId> = research.iter().map(|(t, _)| *t).collect();
    assert_eq!(
        techs,
        ENEMY_OUTPOST_TECHS.map(|t| tech(&rules, t)).to_vec(),
        "{research:?}"
    );
    let aged = aged.expect("AgeAdvanced(P0, 1)");
    assert_eq!(view.age(P0), 1);
    // The trained units, by Train, with the scripted kinds, the army first.
    let want: Vec<(UnitId, UnitKindId)> = trained
        .iter()
        .zip(
            ENEMY_OUTPOST_MUSTER_UNITS
                .iter()
                .chain(&ENEMY_OUTPOST_SIEGE_UNITS),
        )
        .map(|(id, k)| (*id, ukind(&rules, k)))
        .collect();
    let got: Vec<(UnitId, UnitKindId)> = spawned.iter().map(|(u, k, _)| (*u, *k)).collect();
    assert_eq!(got, want);
    // The siege.
    assert!(
        siege_hits.len() >= 3,
        "Mangonel hits on the tower: {siege_hits:?}"
    );
    assert_eq!(tower_shots_on_siege, 0, "the Mangonels outrange the tower");
    let destroyed = destroyed.expect("BuildingDestroyed(3)");
    assert!(
        destroyed <= beats.destroy_by,
        "destroyed during {destroyed}"
    );
    assert!(view.building(ENEMY_OUTPOST_TOWER_ID).is_none());
    for y in y0..=y1 {
        for x in x0..=x1 {
            assert!(view.map().passable(Tile::new(x, y)), "({x}, {y})");
        }
    }
    assert!(
        !view
            .territory()
            .sources()
            .contains_key(&ENEMY_OUTPOST_TOWER_ID)
    );
    // Prerequisites, each at least the margin before its command.
    let wipe = wipe.expect("vanguard wiped");
    // The walkthrough fights the same prologue as `enemy_outpost`, so
    // `ENEMY_OUTPOST_CLEAR_TICK` bounds its wipe with the same margin.
    assert!(
        wipe + 100 <= ENEMY_OUTPOST_CLEAR_TICK,
        "the walkthrough's prologue: vanguard wiped during {wipe}, CLEAR_TICK {ENEMY_OUTPOST_CLEAR_TICK}"
    );
    let done = |id: u32| completed[&BuildingId(id)];
    let last_army = spawned
        .iter()
        .filter(|(u, _, _)| army.contains(u))
        .map(|(_, _, t)| *t)
        .max()
        .unwrap();
    let last_siege = spawned
        .iter()
        .filter(|(u, _, _)| siege.contains(u))
        .map(|(_, _, t)| *t)
        .max()
        .unwrap();
    let checks = [
        (
            "research_1 needs the Scriptorium",
            done(4),
            beats.research_1,
        ),
        ("muster needs the Muster Hall", done(5), beats.muster),
        ("muster needs the vanguard gone", wipe, beats.muster),
        (
            "research_2 needs Letters I",
            research[0].1,
            beats.research_2,
        ),
        ("age needs Trade I", research[1].1, beats.age),
        ("yard needs the Engine Yard", done(6), beats.yard),
        ("siege needs the mixed army", last_army, beats.siege),
        ("siege needs the Mangonels", last_siege, beats.siege),
    ];
    println!(
        "enemy_outpost_walkthrough beats (issued during tick; applied cmd_delay = {delay} later):"
    );
    for (what, met, at) in checks {
        println!(
            "  {what}: met during tick {met}, issued during tick {at}, margin {}",
            at - met.min(at)
        );
        assert!(
            met + ENEMY_OUTPOST_PREREQ_MARGIN <= at,
            "{what}: met during {met}, command during {at}"
        );
    }
    println!(
        "  build {} research_1 {} muster {} research_2 {} age {} yard {} siege {} destroy_by {} ticks {}",
        beats.build,
        beats.research_1,
        beats.muster,
        beats.research_2,
        beats.age,
        beats.yard,
        beats.siege,
        beats.destroy_by,
        beats.ticks
    );
    println!(
        "  Scriptorium complete {}, Muster Hall {}, Engine Yard {}, Letters I {}, Trade I {} (queued for {:?} ticks), Masonry Age {aged}, vanguard wiped {wipe}",
        done(4),
        done(5),
        done(6),
        research[0].1,
        research[1].1,
        trade_ticks
    );
    for (u, k, t) in &spawned {
        println!(
            "  {u:?} {} out during tick {t}",
            rules.units[usize::from(k.0)].id
        );
    }
    println!(
        "  Mangonel hits on the tower {siege_hits:?}; tower shots {tower_shots} (none on a Mangonel); Watchtower destroyed during tick {destroyed} (destroy_by {})",
        beats.destroy_by
    );
}

/// The fixture holds exactly the scenario: its header is the scenario's
/// setup (the informational git sha aside) and its tick batches are the
/// stream's, so a change to the scenario code or to the rules it derives
/// its beats from cannot leave a stale fixture behind. Reads the file only.
#[test]
fn enemy_outpost_walkthrough_fixture_holds_the_scenario_stream() {
    let rules = load_rules();
    let (setup, stream) = scenarios::enemy_outpost_walkthrough(&rules);
    let file = fixtures_dir().join("enemy_outpost_walkthrough.eonreplay");
    let replay = sim::ReplayReader::open(&file).expect("the fixture reads");
    assert!(!replay.truncated);
    let mut header = replay.setup.clone();
    header.git_sha = setup.git_sha.clone();
    assert_eq!(header, setup);
    let batches: Stream = replay
        .records
        .iter()
        .filter_map(|r| match r {
            sim::replay::Record::Tick(b) if !b.cmds.is_empty() => Some((b.tick, b.cmds.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(batches, stream);
}

/// The golden fixture of the scripted proxy verifies and ends on its
/// recorded hash.
#[test]
#[ignore = "long golden with a 400-unit prologue; verified by sim-cli verify --release in CI"]
fn golden_enemy_outpost_walkthrough() {
    let name = "enemy_outpost_walkthrough";
    let replay = fixtures_dir().join(format!("{name}.eonreplay"));
    let hash_file = fixtures_dir().join(format!("{name}.hash"));
    let expected = std::fs::read_to_string(&hash_file)
        .unwrap_or_else(|e| panic!("{}: {e}", hash_file.display()));
    let outcome = sim::verify(&replay, &data_dir(), Box::new(NoAi))
        .unwrap_or_else(|e| panic!("{}: {e}", replay.display()));
    match outcome {
        sim::VerifyOutcome::Ok { final_hash, .. } => {
            assert_eq!(format!("0x{final_hash:016x}"), expected.trim(), "{name}");
        }
        other => panic!("{name}: {other}"),
    }
}
