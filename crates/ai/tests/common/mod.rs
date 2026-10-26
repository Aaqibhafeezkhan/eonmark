//! The bot testkit (M5b): an [`Arena`] holds a real `Sim` with chosen AI
//! seats and debug setup commands, and hands out a live plan context or a
//! plan of the Standard bot without stepping. Shared by the `crates/ai`
//! integration tests (`mod common;`) and, through `#[path]`, by the
//! crate's own unit tests (`crate::testkit`).
//!
//! A [`Arena::frozen`] arena runs no bot inside the sim: the tests drive
//! each Standard seat themselves with [`Arena::act`], which plans with the
//! arena's own memory for the seat and applies the plan's commands as that
//! player's, so one plan's effect can be checked at a time.
#![allow(dead_code)]

use ai::standard::{BotMemory, Ctx, MapAnalysis, StandardBot};
use rules::Rules;
use sim::{
    AiMemory, BuildingId, BuildingKindId, Command, FxVec2, MatchSetup, NoAi, PlayerCommand,
    PlayerId, Sim, Tile, UnitId, UnitKindId,
};
use std::path::{Path, PathBuf};

/// The repository's `data/`.
pub fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
}

/// The repository's rules.
pub fn rules() -> Rules {
    Rules::load(data_dir()).expect("data/ loads")
}

/// A unit kind's id by its string id.
pub fn unit_kind(rules: &Rules, id: &str) -> UnitKindId {
    let i = rules
        .units
        .iter()
        .position(|u| u.id == id)
        .expect("unit kind");
    UnitKindId(u16::try_from(i).expect("few kinds"))
}

/// A building kind's id by its string id.
pub fn building_kind(rules: &Rules, id: &str) -> BuildingKindId {
    BuildingKindId(rules.building_kind_by_id(id).expect("building kind").0)
}

/// The first sequence number of the commands an arena issues itself
/// (setup and [`Arena::issue`]); a bot's own start at 0, far below.
const ARENA_SEQ: u32 = 1 << 30;

/// A match under test.
pub struct Arena {
    /// The sim, with the header's own bots (`ai::controller_for`), or no
    /// bot at all for a frozen arena.
    pub sim: Sim,
    /// A Standard bot per seat whose header names one (for `ctx`, `plan`
    /// and `act`), `None` for other seats.
    pub bots: Vec<Option<StandardBot>>,
    /// A frozen arena's memory per seat (the sim's bots do not run).
    pub memories: Vec<AiMemory>,
    frozen: bool,
    seq: u32,
}

impl Arena {
    /// A match of `setup` with its header's bots. `setup_commands` are
    /// applied on tick 0 (issued by `PlayerId(0)` with sequence numbers
    /// from `1 << 30`, so they never meet a bot's own), which needs
    /// `setup.debug_commands` for debug commands. The arena steps once, so
    /// they apply `cmd_delay - 1` steps later.
    pub fn new(setup: MatchSetup, setup_commands: Vec<Command>) -> Arena {
        let rules = rules();
        let controller = ai::controller_for(&setup, &rules).expect("the header's bots build");
        let mut arena = Arena::build(setup, rules, controller, false);
        arena.submit(PlayerId(0), setup_commands);
        arena
    }

    /// A match of `setup` whose sim runs no bot: the seats act only when a
    /// test calls [`Arena::act`]. `setup_commands` are applied before this
    /// returns (the sim has stepped `cmd_delay + 1` times).
    pub fn frozen(setup: MatchSetup, setup_commands: Vec<Command>) -> Arena {
        let rules = rules();
        let mut arena = Arena::build(setup, rules, Box::new(NoAi), true);
        arena.issue(PlayerId(0), setup_commands);
        arena
    }

    fn build(
        setup: MatchSetup,
        rules: Rules,
        controller: Box<dyn sim::AiController>,
        frozen: bool,
    ) -> Arena {
        let bots: Vec<Option<StandardBot>> = setup
            .players
            .iter()
            .map(|slot| {
                let seat = setup.ai_seats.iter().find(|s| s.player == slot.id)?;
                (seat.bot == ai::BOT_STANDARD).then(|| {
                    let knobs = rules
                        .ai
                        .personality(&seat.personality)
                        .expect("personality");
                    StandardBot::new(&rules, &setup, slot.id, knobs)
                })
            })
            .collect();
        let memories = vec![AiMemory::default(); setup.players.len()];
        Arena {
            sim: Sim::new(setup, rules, controller),
            bots,
            memories,
            frozen,
            seq: ARENA_SEQ,
        }
    }

    /// Step once with `cmds` issued by `p`.
    fn submit(&mut self, p: PlayerId, cmds: Vec<Command>) {
        let stamped: Vec<PlayerCommand> = cmds
            .into_iter()
            .map(|c| {
                self.seq += 1;
                PlayerCommand::new(p, self.seq, c)
            })
            .collect();
        self.sim.step(&stamped);
    }

    /// Issue `cmds` as player `p` and step until they have applied.
    pub fn issue(&mut self, p: PlayerId, cmds: Vec<Command>) {
        self.submit(p, cmds);
        let delay = self.sim.setup().cmd_delay;
        self.run(delay);
    }

    fn bot(&self, me: PlayerId) -> &StandardBot {
        self.bots[usize::from(me.0)]
            .as_ref()
            .expect("the seat is a standard bot")
    }

    /// The static map analysis of `me`'s Standard bot.
    pub fn map(&self, me: PlayerId) -> &MapAnalysis {
        self.bot(me).map()
    }

    /// `me`'s memory bytes: the arena's for a frozen arena, else the sim's.
    pub fn memory(&self, me: PlayerId) -> AiMemory {
        if self.frozen {
            return self.memories[usize::from(me.0)].clone();
        }
        let mut m = AiMemory::default();
        m.set(self.sim.view().ai_memory(me).to_vec());
        m
    }

    /// `me`'s decoded memory.
    pub fn mem(&self, me: PlayerId) -> BotMemory {
        BotMemory::decode(self.memory(me).bytes())
    }

    /// Replace a frozen arena's memory for `me`.
    pub fn set_mem(&mut self, me: PlayerId, mem: &BotMemory) {
        assert!(self.frozen, "only a frozen arena keeps memories");
        self.memories[usize::from(me.0)].set(mem.encode());
    }

    /// A live plan context for `me` at the current tick.
    pub fn ctx(&self, me: PlayerId) -> Ctx<'_> {
        let view = self.sim.view();
        let knobs = self.bot(me).knobs(view.rules());
        Ctx::new(view, me, knobs, self.bot(me).map())
    }

    /// What `me`'s Standard bot would plan now, whatever its cadence, and
    /// the memory it would leave; nothing is stepped.
    pub fn plan(&self, me: PlayerId) -> (Vec<Command>, AiMemory) {
        let view = self.sim.view();
        let mut memory = self.memory(me);
        let cmds = self.bot(me).plan(me, &view, &mut memory);
        (cmds, memory)
    }

    /// A frozen arena: plan for `me` now, keep the memory, and apply the
    /// plan's commands as `me`'s (stepping until they have applied).
    /// Returns the commands.
    pub fn act(&mut self, me: PlayerId) -> Vec<Command> {
        assert!(self.frozen, "act drives a frozen arena");
        let (cmds, memory) = self.plan(me);
        self.memories[usize::from(me.0)] = memory;
        self.issue(me, cmds.clone());
        cmds
    }

    /// Step `n` ticks with no human input.
    pub fn run(&mut self, n: u32) {
        for _ in 0..n {
            self.sim.step(&[]);
        }
    }

    /// `p`'s units of `kind`, in id order.
    pub fn units_of(&self, p: PlayerId, kind: &str) -> Vec<UnitId> {
        let k = unit_kind(self.sim.rules(), kind);
        self.sim
            .view()
            .units()
            .filter(|u| u.owner == p && u.kind == k)
            .map(|u| u.id)
            .collect()
    }

    /// `p`'s buildings and sites of `kind`, in id order.
    pub fn buildings_of(&self, p: PlayerId, kind: &str) -> Vec<BuildingId> {
        let k = building_kind(self.sim.rules(), kind);
        self.sim
            .view()
            .buildings()
            .filter(|b| b.owner == p && b.kind == k)
            .map(|b| b.id)
            .collect()
    }

    /// `p`'s Seat.
    pub fn seat(&self, p: PlayerId) -> BuildingId {
        self.sim
            .view()
            .buildings()
            .find(|b| b.owner == p && b.town_state.is_some_and(|t| t.seat))
            .expect("a Seat")
            .id
    }
}

/// `DebugSpawn` of `count` units of `kind` for `owner` around `(x, y)`.
pub fn spawn(rules: &Rules, owner: PlayerId, kind: &str, x: i32, y: i32, count: u16) -> Command {
    Command::DebugSpawn {
        owner,
        kind: unit_kind(rules, kind),
        at: FxVec2::from_ints(x, y),
        count,
    }
}

/// `DebugPlaceBuilding` of a complete `kind` for `owner` at `(x, y)`.
pub fn place(rules: &Rules, owner: PlayerId, kind: &str, x: u16, y: u16) -> Command {
    Command::DebugPlaceBuilding {
        owner,
        kind: building_kind(rules, kind),
        tile: Tile::new(x, y),
        complete: true,
    }
}

/// `DebugGrant` of `amount` of every resource to `owner`.
pub fn grant_all(rules: &Rules, owner: PlayerId, amount: u32) -> Vec<Command> {
    (0..rules.resources.resources.len())
        .map(|r| Command::DebugGrant {
            owner,
            resource: u8::try_from(r).expect("few resources"),
            amount,
        })
        .collect()
}

/// `DebugResearch` of `tech` for `owner`.
pub fn research(rules: &Rules, owner: PlayerId, tech: &str) -> Command {
    Command::DebugResearch {
        owner,
        tech: sim::TechId(rules.tech_by_id(tech).expect("tech").0),
    }
}

/// Player 0, the Standard bot's seat in [`standard_vs_passive`].
pub const P0: PlayerId = PlayerId(0);
/// Player 1.
pub const P1: PlayerId = PlayerId(1);

/// A standard-start match of the Standard bot (P0) against Passive (P1)
/// with debug commands on, for frozen arenas.
pub fn standard_vs_passive(rules: &Rules) -> MatchSetup {
    let mut s = MatchSetup::bots(rules, 1, ("standard", "standard"), ("passive", ""));
    s.debug_commands = true;
    s
}

/// The commands in `cmds` that order `unit` (a unit list or a builder).
pub fn orders_of(cmds: &[Command], unit: UnitId) -> Vec<&Command> {
    cmds.iter()
        .filter(|c| match c {
            Command::Move { units, .. }
            | Command::AttackMove { units, .. }
            | Command::Attack { units, .. }
            | Command::Stop { units }
            | Command::Gather { units, .. } => units.contains(&unit),
            Command::Build { unit: u, .. } => *u == unit,
            _ => false,
        })
        .collect()
}
