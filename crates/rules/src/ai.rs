//! Bot data from `data/ai/` (M5b): the personalities in
//! `ai/personalities.ron` and the build orders in `ai/build_orders/*.ron`.
//!
//! [`crate::Rules::load`] attaches them as [`crate::Rules::ai`], so they are
//! in `rules_hash`: an unbumped edit reports `RULES CHANGED` instead of a
//! divergence. Every number is an integer: `_ds` fields are deciseconds,
//! `_tiles` fields tiles, `_permille` fields per mille. The validators run
//! at load (and so in `sim-cli data-check`), and every error names the file
//! and the field.
//!
//! A build order is a list of steps the bot keeps satisfying from the top
//! (`crates/ai`); a step is done when the view shows it, never when its
//! command is issued. The static reachability rules below make a build
//! order that could stall for ever fail here instead of at run time: a
//! step that cannot be reached from the steps before it must be
//! `optional`, and an `Advance` never may be.
//!
//! M6 adds the difficulty levels, one file per level under
//! `ai/difficulty/<id>.ron` ([`Difficulty`]): the disclosed bonus bundle
//! the sim pays a bot seat (the income cheat) and the wave knobs the level
//! scales, applied to a personality by [`resolve`]. A header's
//! `AiSeat.bot` names a difficulty id (or `passive`).

use crate::{Error, Rules, UnitClass, load_ron};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Most steps a build order may have.
pub const MAX_STEPS: usize = 64;
/// Largest `count` of a `Workers`, `Build` or `Train` step.
pub const MAX_STEP_COUNT: u32 = 200;
/// Largest per-plan cap (`lookahead`, `max_commands_per_think`, ...) and
/// largest unit count knob.
pub const MAX_AI_CAP: u32 = 64;
/// Largest wave (`max_wave_units`): bounds the roster the bot keeps in its
/// hashed memory.
pub const MAX_WAVE_UNITS: u32 = 64;
/// Largest `_tiles` knob.
pub const MAX_AI_TILES: u32 = 64;
/// Largest `_ds` knob and `Wait` time (two hours).
pub const MAX_AI_DS: u32 = 72_000;
/// Largest `think_every_ds` (one minute).
pub const MAX_THINK_EVERY_DS: u32 = 600;
/// Largest `army_mix` weight and `toward_enemy_penalty`.
pub const MAX_AI_WEIGHT: u32 = 1000;
/// Largest amount of one resource in a difficulty's bonus bundle.
pub const MAX_BONUS_AMOUNT: u32 = 500;
/// Shortest non-zero bonus interval (`income_interval_ds`, 5 s).
pub const MIN_INCOME_INTERVAL_DS: u32 = 50;
/// Smallest `worker_target_permille` (M6).
pub const MIN_WORKER_TARGET_PERMILLE: u32 = 500;
/// Most `extra_research` entries of a difficulty (M6 owner decision 12).
pub const MAX_EXTRA_RESEARCH: usize = 8;
/// Smallest difficulty multiplier (`wave_size_permille`,
/// `wave_interval_permille`).
pub const MIN_DIFFICULTY_PERMILLE: u32 = 250;
/// Largest difficulty multiplier.
pub const MAX_DIFFICULTY_PERMILLE: u32 = 4000;
/// Largest counter factor cap (`counter_cap_permille`) and counter weight.
pub const MAX_COUNTER_PERMILLE: u32 = 8000;
/// Largest jitter share (`noise_wave_interval_permille`,
/// `noise_mix_permille`).
pub const MAX_NOISE_PERMILLE: u32 = 500;
/// Largest `noise_site_top_k`.
pub const MAX_SITE_TOP_K: u32 = 8;
/// Largest `influence_cell_tiles`.
pub const MAX_INFLUENCE_CELL_TILES: u32 = 16;
/// Largest `influence_radius_cells`.
pub const MAX_INFLUENCE_RADIUS_CELLS: u32 = 8;
/// Largest `retreat_ratio_permille` and `raid_distance_permille`.
pub const MAX_AI_RATIO_PERMILLE: u32 = 10_000;
/// The bot id of the bot that never acts: reserved, so no difficulty file
/// may take it.
pub const PASSIVE_ID: &str = "passive";
/// The difficulty every tool defaults to (`play-bots`, New Game); its file
/// must exist.
pub const STANDARD_DIFFICULTY: &str = "standard";

/// Everything under `data/ai/`, attached to [`crate::Rules::ai`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AiRules {
    /// The rows of `ai/personalities.ron`, in file order.
    pub personalities: Vec<Personality>,
    /// The build orders of `ai/build_orders/*.ron`, keyed by file stem
    /// (which equals each one's `id`).
    pub build_orders: BTreeMap<String, BuildOrder>,
    /// The difficulty levels of `ai/difficulty/*.ron` (M6), keyed by file
    /// stem (which equals each one's `id`).
    pub difficulties: BTreeMap<String, Difficulty>,
}

impl AiRules {
    /// The personality with the given id, if any.
    pub fn personality(&self, id: &str) -> Option<&Personality> {
        self.personalities.iter().find(|p| p.id == id)
    }

    /// The build order with the given id, if any.
    pub fn build_order(&self, id: &str) -> Option<&BuildOrder> {
        self.build_orders.get(id)
    }

    /// The difficulty level with the given id, if any (M6).
    pub fn difficulty(&self, id: &str) -> Option<&Difficulty> {
        self.difficulties.get(id)
    }
}

/// Which age gate a difficulty puts on a personality's waves (M6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaveAgeGate {
    /// The personality's own `wave_requires_age`.
    Personality,
    /// No wave launches before the bot reaches this age (an age id).
    Age(String),
    /// No age gate at all.
    Off,
}

/// A difficulty's guaranteed early raid (M6, slice 2's behaviour): by
/// `by_ds` the bot attacks with at least `min_armed` armed units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstRaid {
    /// The raid launches by this match time (`by_ds` below the match cap).
    pub by_ds: u32,
    /// Emergency production toward `min_armed` starts this long before
    /// `by_ds` (less than `by_ds`).
    pub prep_ds: u32,
    /// Armed units the raid needs (at least 1).
    pub min_armed: u32,
    /// Most armed units it takes (`min_armed..=MAX_WAVE_UNITS`).
    pub max_armed: u32,
}

/// How a difficulty presses its sieges (M6 owner decision 12, 2026-10-29):
/// the behaviour the ROADMAP names for each level (Easy defensive; Standard
/// and Hard keep a siege going until a Town falls). [`Pressure::OFF`] is
/// the slice-2 bot: no reinforcement, no commit, the personality's retreat
/// and no extra home guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pressure {
    /// While a siege wave assaults or holds its target, home armed units
    /// assembled at the rally join it once at least this many are there
    /// (beyond the home guard), and during the all-in the joiners and the
    /// launch go in such groups from the rally instead of one by one; 0:
    /// never (`0..=MAX_WAVE_UNITS`).
    pub reinforce_min_units: u32,
    /// A siege wave whose target Town lost hit points within this long
    /// neither retreats nor ends on `wave_max_ds`; 0: off
    /// (`0..=MAX_AI_DS`).
    pub commit_ds: u32,
    /// Scales the personality's `retreat_ratio_permille` (0: the level
    /// never retreats; 1000 keeps it), `0..=MAX_DIFFICULTY_PERMILLE`.
    pub retreat_ratio_permille: u32,
    /// Scales the personality's `retreat_cooldown_ds`,
    /// `MIN_DIFFICULTY_PERMILLE..=MAX_DIFFICULTY_PERMILLE`.
    pub retreat_cooldown_permille: u32,
    /// Armed units every launch, reinforcement and all-in joiner leaves at
    /// home (`0..=MAX_WAVE_UNITS`), besides the one a full pop keeps.
    pub home_guard_units: u32,
    /// For up to this long after its assault order a siege wave holds at
    /// its Wain point (out of every enemy shooter's reach) while enemy
    /// armed units stand within `siege_focus_tiles` of its target, so the
    /// defenders come out to it and the fight is not under the Town's and
    /// Watchtowers' fire; 0: off (`0..=MAX_AI_DS`).
    pub lure_ds: u32,
    /// While a siege wave assaults, its armed units more than this many
    /// tiles nearer the goal than its rear quarter stop and wait, so
    /// the wave arrives together instead of fast units first; 0: off
    /// (`0..=MAX_AI_TILES`).
    pub cohesion_tiles: u32,
    /// A siege wave (not the all-in, not a raid) launches only when the
    /// Lanchester strength (`sum(hp) x sum(dps)`) of the units it would
    /// take times 1000 reaches this times the strength of the enemy armed
    /// units standing on the enemy's own land; 0: off
    /// (`0..=MAX_AI_RATIO_PERMILLE`).
    pub launch_ratio_permille: u32,
    /// A staged siege wave (not the all-in) starts its assault only when
    /// its Lanchester strength times 1000 reaches this times that of the
    /// enemy armed units on the enemy's own land; until then it holds at
    /// its stage point, where reinforcements join it, and if it is still
    /// short at `stage_max_ds` it comes home; 0: off
    /// (`0..=MAX_AI_RATIO_PERMILLE`).
    pub assault_ratio_permille: u32,
}

impl Pressure {
    /// No pressure behaviour: the bot as slice 2 built it.
    pub const OFF: Pressure = Pressure {
        reinforce_min_units: 0,
        commit_ds: 0,
        retreat_ratio_permille: 1000,
        retreat_cooldown_permille: 1000,
        home_guard_units: 0,
        lure_ds: 0,
        cohesion_tiles: 0,
        launch_ratio_permille: 0,
        assault_ratio_permille: 0,
    };
}

impl Default for Pressure {
    fn default() -> Pressure {
        Pressure::OFF
    }
}

/// One difficulty level, `data/ai/difficulty/<id>.ron` (M6). Difficulty is
/// a disclosed cheat plus wave timing: the sim pays a bot seat whose
/// `AiSeat.bot` names the level the same `income` bundle every
/// `income_interval_ds`, from `income_from_ds` on (step 6d'), [`resolve`]
/// scales the personality's wave and retreat knobs, and (M6 owner decision
/// 12) its [`Pressure`] sets how hard the level presses a siege. Counters,
/// influence and noise are personality knobs, equal at every level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Difficulty {
    /// Equals the file stem; never `passive`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// The bonus is paid every this many deciseconds; 0 pays nothing.
    /// Otherwise `MIN_INCOME_INTERVAL_DS..=MAX_AI_DS`, and more than
    /// `cmd_delay_ticks` in ticks.
    pub income_interval_ds: u32,
    /// The first payment completes one interval after this match time
    /// (below the match cap).
    pub income_from_ds: u32,
    /// The bonus bundle per resource id, each `0..=MAX_BONUS_AMOUNT`.
    pub income: BTreeMap<String, u32>,
    /// The age gate of the bot's waves.
    pub wave_age_gate: WaveAgeGate,
    /// Every wave waits for the personality's largest wave size.
    pub largest_wave_only: bool,
    /// Caps every wave (and `max_wave_units`) at this many armed units.
    pub max_wave_units: Option<u32>,
    /// Scales every `wave_sizes` row (rounded up, at least 1),
    /// `MIN_DIFFICULTY_PERMILLE..=MAX_DIFFICULTY_PERMILLE`.
    pub wave_size_permille: u32,
    /// Scales `wave_interval_ds` (rounded to nearest), same range.
    pub wave_interval_permille: u32,
    /// Replaces the personality's `all_in_after_ds` (`None` keeps it).
    pub all_in_after_ds: Option<u32>,
    /// The guaranteed early raid, if any.
    pub first_raid: Option<FirstRaid>,
    /// Scales every `Workers` count of the bot's build order (rounded up,
    /// at least 1), `MIN_WORKER_TARGET_PERMILLE..=1000`: the disclosed
    /// economic handicap M6 owner decision 2 allows for Easy alone (850: 15%
    /// fewer Yeomen); 1000 at the other levels.
    pub worker_target_permille: u32,
    /// Techs the level researches once its build order is through (M6
    /// owner decision 12: a defensive level widens the border it holds),
    /// each appended to the build order as an optional `Research` step
    /// unless the order already researches it; at most
    /// `MAX_EXTRA_RESEARCH`, each a tech id, each once.
    pub extra_research: Vec<String>,
    /// How the level presses its sieges (M6 owner decision 12).
    pub pressure: Pressure,
}

impl Difficulty {
    /// The bonus interval in ticks (0: no bonus).
    pub fn income_interval_ticks(&self, rules: &Rules) -> u32 {
        rules.ticks_from_ds(self.income_interval_ds)
    }

    /// The match time the bonus schedule counts from, in ticks.
    pub fn income_from_ticks(&self, rules: &Rules) -> u32 {
        rules.ticks_from_ds(self.income_from_ds)
    }

    /// The bonus bundle as a dense vector in `resources.ron` order.
    pub fn bundle(&self, rules: &Rules) -> Vec<i64> {
        rules.amounts(&self.income)
    }

    /// Whether the level pays any bonus at all: a non-zero interval and a
    /// non-empty bundle.
    pub fn pays(&self) -> bool {
        self.income_interval_ds > 0 && self.income.values().any(|v| *v > 0)
    }
}

/// The knobs a bot of difficulty `d` plays `p` with (M6): a pure function
/// of the two rows, so a bot rebuilt mid-match resolves the same knobs.
///
/// - `max_wave_units` becomes the smaller of `p`'s and `d`'s cap;
/// - every `wave_sizes[i].armed` is scaled by `wave_size_permille`, rounded
///   up, at least 1, at most that cap (the rows stay non-decreasing);
/// - `largest_wave_only` sets every row to the last (largest) row's size;
/// - `wave_interval_ds` is scaled by `wave_interval_permille`, rounded to
///   nearest (half up), at least 1 and at most `MAX_AI_DS`;
/// - the age gate is `d.wave_age_gate`, the all-in time
///   `d.all_in_after_ds` when set, and `min_siege`, `raid_max_armed` and
///   `raid_min_armed` at most the cap.
///
/// - (M6 owner decision 12) `retreat_ratio_permille` and
///   `retreat_cooldown_ds` are scaled by the level's
///   `pressure.retreat_ratio_permille` and `retreat_cooldown_permille`,
///   rounded to nearest (half up), at most `MAX_AI_RATIO_PERMILLE` and
///   `MAX_AI_DS`.
///
/// Nothing else changes: counters, influence and noise are the
/// personality's at every level.
pub fn resolve(p: &Personality, d: &Difficulty) -> Personality {
    let mut out = p.clone();
    let cap = d
        .max_wave_units
        .map_or(p.max_wave_units, |m| m.min(p.max_wave_units))
        .max(1);
    out.max_wave_units = cap;
    for w in &mut out.wave_sizes {
        let scaled = (u64::from(w.armed) * u64::from(d.wave_size_permille)).div_ceil(1000);
        let scaled = u32::try_from(scaled).unwrap_or(u32::MAX);
        w.armed = scaled.clamp(1, cap);
    }
    if d.largest_wave_only
        && let Some(last) = out.wave_sizes.last().map(|w| w.armed)
    {
        for w in &mut out.wave_sizes {
            w.armed = last;
        }
    }
    let interval =
        (u64::from(p.wave_interval_ds) * u64::from(d.wave_interval_permille) + 500) / 1000;
    out.wave_interval_ds = u32::try_from(interval)
        .unwrap_or(MAX_AI_DS)
        .clamp(1, MAX_AI_DS);
    out.wave_requires_age = match &d.wave_age_gate {
        WaveAgeGate::Personality => p.wave_requires_age.clone(),
        WaveAgeGate::Age(age) => Some(age.clone()),
        WaveAgeGate::Off => None,
    };
    if let Some(ds) = d.all_in_after_ds {
        out.all_in_after_ds = ds;
    }
    out.min_siege = out.min_siege.min(cap);
    out.raid_max_armed = out.raid_max_armed.min(cap);
    out.raid_min_armed = out.raid_min_armed.min(out.raid_max_armed);
    let scale = |v: u32, permille: u32, max: u32| -> u32 {
        let scaled = (u64::from(v) * u64::from(permille) + 500) / 1000;
        u32::try_from(scaled).unwrap_or(max).min(max)
    };
    out.retreat_ratio_permille = scale(
        p.retreat_ratio_permille,
        d.pressure.retreat_ratio_permille,
        MAX_AI_RATIO_PERMILLE,
    );
    out.retreat_cooldown_ds = scale(
        p.retreat_cooldown_ds,
        d.pressure.retreat_cooldown_permille,
        MAX_AI_DS,
    );
    out
}

/// The build order a bot of difficulty `d` plays (M6): `order` with every
/// `Workers` count scaled by `d.worker_target_permille`, rounded up, at
/// least 1 (the split weights stay as they are: they are shares), and (M6
/// owner decision 12) an optional `Research` step appended for each of
/// `d.extra_research` that the order does not research already, in the
/// level's order. A pure function of the two rows, like [`resolve`].
pub fn resolve_order(order: &BuildOrder, d: &Difficulty) -> BuildOrder {
    let mut out = order.clone();
    for step in &mut out.steps {
        if let Step::Workers { count, .. } = step {
            let scaled = (u64::from(*count) * u64::from(d.worker_target_permille)).div_ceil(1000);
            *count = u32::try_from(scaled).unwrap_or(u32::MAX).max(1);
        }
    }
    for tech in &d.extra_research {
        let has = out
            .steps
            .iter()
            .any(|s| matches!(s, Step::Research { tech: t, .. } if t == tech));
        if !has {
            out.steps.push(Step::Research {
                tech: tech.clone(),
                optional: true,
            });
        }
    }
    out
}

/// Contents of `data/ai/personalities.ron`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Personalities {
    /// One row per personality (M5b ships `standard`).
    pub personalities: Vec<Personality>,
}

/// One build order file, `data/ai/build_orders/<id>.ron`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildOrder {
    /// Equals the file stem.
    pub id: String,
    /// The steps, in priority order (`1..=MAX_STEPS`).
    pub steps: Vec<Step>,
}

/// One build-order step and when the bot counts it as done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Step {
    /// Workers (the worker kind, [`worker_kind`]) alive plus queued reach
    /// `count`; the latest `Workers` step at or before the bot's window
    /// sets the gather split, weights per resource id.
    Workers {
        /// Worker target.
        count: u32,
        /// Gather weights per resource id; at least one entry, each at
        /// least 1, summing to at most `count`.
        split: BTreeMap<String, u32>,
    },
    /// Own buildings of `kind`, sites included, reach `count`.
    Build {
        /// Building kind id.
        kind: String,
        /// Target count, the Seat included for the Town kind.
        count: u32,
        /// Where the bot places them.
        at: Placement,
        /// May be skipped while blocked; required when the step is not
        /// reachable from the steps before it.
        #[serde(default)]
        optional: bool,
    },
    /// Units of `kind` alive plus queued reach `count`.
    Train {
        /// Unit kind id.
        kind: String,
        /// Target count.
        count: u32,
        /// As for `Build`.
        #[serde(default)]
        optional: bool,
    },
    /// `tech` is researched or queued.
    Research {
        /// Tech id.
        tech: String,
        /// As for `Build`.
        #[serde(default)]
        optional: bool,
    },
    /// The bot has reached `age`. Never optional.
    Advance {
        /// Age id.
        age: String,
    },
    /// Always done; crossing it latches army production in the bot's
    /// memory.
    Military,
    /// Done once the match time reaches `until_ds`.
    Wait {
        /// Match time in deciseconds.
        until_ds: u32,
    },
}

impl Step {
    /// Whether the step may be skipped while it is blocked.
    pub fn optional(&self) -> bool {
        match self {
            Step::Build { optional, .. }
            | Step::Train { optional, .. }
            | Step::Research { optional, .. } => *optional,
            Step::Workers { .. } | Step::Advance { .. } | Step::Military | Step::Wait { .. } => {
                false
            }
        }
    }
}

/// Where a `Build` step's buildings go (the bot's site policies).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Placement {
    /// Around an own Town, `near_town_min_tiles..=near_town_max_tiles`
    /// from its centre.
    NearTown,
    /// Beside the resource terrain of a kind with `Adjacent` slots.
    Terrain,
    /// A second Town (the Town kind only).
    Expansion,
    /// The owned tile nearest the enemy start (kinds with
    /// `border_radius_tiles` only).
    BorderEdge,
}

/// When an attacking wave is called home to defend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecallWave {
    /// While it is still staging and the defence still blocks launches
    /// (for at most `defend_max_block_ds`), or when the threatened Town is
    /// contested or at or below `recall_wave_town_hp_permille` (M5b owner
    /// decision 7).
    WhileStaging,
    /// Whenever an own Town is threatened (docs/design/ai.md rule 6 as
    /// first written).
    Always,
}

/// How a personality's waves start (M6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaveMode {
    /// Every wave is a siege wave (M5b).
    Siege,
    /// While no siege unit is assembled at the rally, the bot sends raids
    /// (no siege, no Supply Wain) every `raid_every_ds`; siege waves once
    /// it has one.
    RaidUntilSiege,
}

/// When a siege wave needs a Supply Wain to launch (M6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WainRule {
    /// Every wave takes one (M5b).
    Always,
    /// Only once the enemy has a Harrying level (attrition exists only
    /// then); Wains are trained from then on too.
    IfEnemyHarrying,
}

/// One entry of a personality's army mix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MixEntry {
    /// An armed, non-civilian unit kind id.
    pub kind: String,
    /// Its share of the army, `1..=MAX_AI_WEIGHT`.
    pub weight: u32,
}

/// From `from_ds` on, a wave launches with at least `armed` armed units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaveSize {
    /// Match time this row starts at, in deciseconds.
    pub from_ds: u32,
    /// Armed units a wave needs.
    pub armed: u32,
}

/// A bot personality: a build order plus the knobs of every rule
/// (docs/design/ai.md). M5b ships one row, `standard`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Personality {
    /// Stable id (`MatchSetup.ai_seats[..].personality`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// A build order id (a file stem under `ai/build_orders/`).
    pub build_order: String,
    /// The bot plans every this many deciseconds; must be more than
    /// `cmd_delay_ticks` in ticks, so a plan's commands apply before the
    /// next plan reads the view.
    pub think_every_ds: u32,
    /// Unsatisfied build-order steps worked on at once.
    pub lookahead: u32,
    /// Most commands one plan issues.
    pub max_commands_per_think: u32,
    /// Most `check_build` probes (a full-map search each) one plan makes.
    pub site_checks_per_think: u32,
    /// Most gatherers one plan moves between resources.
    pub rebalance_per_think: u32,
    /// A step blocked for a reason other than cost is retried after this
    /// long.
    pub blocked_retry_ds: u32,
    /// Free passable ring kept around every footprint the bot places.
    pub building_gap_tiles: u32,
    /// `NearTown` sites start this far from the Town centre.
    pub near_town_min_tiles: u32,
    /// `NearTown` sites end this far from the Town centre.
    pub near_town_max_tiles: u32,
    /// A second Town is at least this far from every own Town.
    pub expansion_min_tiles: u32,
    /// Resource tiles within this radius count toward an expansion site.
    pub resource_radius_tiles: u32,
    /// Expansion score lost per tile gained toward the enemy start.
    pub toward_enemy_penalty: u32,
    /// Terrain and border sites are searched within this radius of the
    /// own start.
    pub border_scan_tiles: u32,
    /// Border Watchtowers keep at least this far apart.
    pub tower_spacing_tiles: u32,
    /// The army's composition by weight.
    pub army_mix: Vec<MixEntry>,
    /// The unit kind each wave takes along to cancel attrition (a kind
    /// that `supplies`).
    pub supply_kind: String,
    /// The bot keeps at most this many entries queued per building.
    pub max_queue_per_building: u32,
    /// The rally point lies this far from the Seat toward the enemy start.
    pub rally_tiles_from_seat: u32,
    /// Armed units within this radius of the rally count as assembled.
    pub assemble_radius_tiles: u32,
    /// A unit already heading for or holding a point this close to its
    /// intended goal gets no new order.
    pub goal_tolerance_tiles: u32,
    /// No wave launches before the bot reaches this age (`None`: no gate).
    pub wave_requires_age: Option<String>,
    /// No wave launches before this match time.
    pub first_wave_not_before_ds: u32,
    /// Wave sizes over match time: `from_ds` strictly increasing from 0,
    /// `armed` non-decreasing.
    pub wave_sizes: Vec<WaveSize>,
    /// Most armed units in one wave (`..=MAX_WAVE_UNITS`).
    pub max_wave_units: u32,
    /// Time between two launches.
    pub wave_interval_ds: u32,
    /// Siege units a wave needs before it launches (0 for none).
    pub min_siege: u32,
    /// Supply units the bot keeps for its waves.
    pub wains_per_wave: u32,
    /// The stage point keeps this much beyond the reach of the enemy's
    /// shooters.
    pub stage_margin_tiles: u32,
    /// Units within this radius of the stage point count as staged.
    pub stage_radius_tiles: u32,
    /// Share of the wave's armed units that must be staged before the
    /// assault.
    pub stage_ready_permille: u32,
    /// The assault starts after this long staging anyway.
    pub stage_max_ds: u32,
    /// Siege within this radius of the target attacks it explicitly.
    pub siege_focus_tiles: u32,
    /// The Supply Wain keeps this far outside every enemy shooter's range.
    pub wain_standoff_tiles: u32,
    /// The Wain is moved when it is this far from its point.
    pub wain_leash_tiles: u32,
    /// Units posted on the ring of a Town at 0 HP to annex it.
    pub ring_units: u32,
    /// Units sent to clear defenders around a Town being annexed.
    pub clear_defenders_units: u32,
    /// A wave that took its target goes on with at least this many armed
    /// units.
    pub continue_min_units: u32,
    /// A wave with fewer armed units than this comes home.
    pub recall_below_units: u32,
    /// A wave ends after this long unless it is annexing.
    pub wave_max_ds: u32,
    /// From this match time on, every armed unit joins the wave and none
    /// is recalled.
    pub all_in_after_ds: u32,
    /// Enemy military on own land this close to an own Town is a threat.
    pub defend_radius_tiles: u32,
    /// A guard is re-ordered when its goal is farther than this from the
    /// nearest threat.
    pub defend_regoal_tiles: u32,
    /// Defence ends after this long with no threat.
    pub defend_release_ds: u32,
    /// Defence blocks launches (and calls a staging wave home) for at
    /// most this long.
    pub defend_max_block_ds: u32,
    /// Past `defend_max_block_ds`, while a launch is due, the home guard
    /// is the home armed units nearest the threat until their pop reaches
    /// this many times the threat's pop (at least one unit); the rest are
    /// left to the wave at the rally.
    pub defend_guard_pop_ratio: u32,
    /// When a wave is called home to defend.
    pub recall_wave: RecallWave,
    /// An assaulting wave is recalled when the threatened Town is at or
    /// below this share of its hit points.
    pub recall_wave_town_hp_permille: u32,
    /// In the pool the `random` personality choice draws from (M6): rush,
    /// boom and tower; the Balanced `standard` row is an explicit choice.
    pub random: bool,
    // M6 (docs/design/ai.md, "Personalities"): every field below is
    // explicit in every row, and none of them is scaled by a difficulty
    // (`resolve` leaves them alone, but caps the raid sizes).
    /// `Siege` or `RaidUntilSiege`.
    pub wave_mode: WaveMode,
    /// When a siege wave needs a Supply Wain.
    pub wain_rule: WainRule,
    /// No wave or raid launches before this tech is researched (`None`: no
    /// gate); the deadline raid ignores it.
    pub wave_requires_tech: Option<String>,
    /// A wave skips the enemy Seat while the enemy holds another complete,
    /// uncontested Town.
    pub seat_last: bool,
    /// Home armed units a raid needs (`RaidUntilSiege`).
    pub raid_min_armed: u32,
    /// Most armed units in a raid (`raid_min_armed..=MAX_WAVE_UNITS`).
    pub raid_max_armed: u32,
    /// Time between two raids.
    pub raid_every_ds: u32,
    /// A raid comes home after this long.
    pub raid_max_ds: u32,
    /// A raid comes home after this long with no enemy unit within
    /// `raid_idle_tiles` of any of its units.
    pub raid_idle_ds: u32,
    /// See `raid_idle_ds`.
    pub raid_idle_tiles: u32,
    /// Raid target score per worker at the target building.
    pub raid_worker_weight: u32,
    /// Raid target score lost per tile from the rally, per mille.
    pub raid_distance_permille: u32,
    /// A wave retreats when its own strength times 1000 is below the
    /// enemy army's near it times this (and `retreat_loss_permille` holds);
    /// 0 never retreats.
    pub retreat_ratio_permille: u32,
    /// ... and it has lost at least this share of the armed units it
    /// launched with.
    pub retreat_loss_permille: u32,
    /// No retreat within this long of the assault order (a raid's
    /// launch).
    pub retreat_commit_ds: u32,
    /// After a retreat the next launch waits at least this long.
    pub retreat_cooldown_ds: u32,
    /// No retreat while the wave's target Town is at or below this share
    /// of its hit points.
    pub retreat_commit_town_hp_permille: u32,
    /// Influence map cell side.
    pub influence_cell_tiles: u32,
    /// Cells around a point an influence query sums (a block of `2r + 1`
    /// cells a side).
    pub influence_radius_cells: u32,
    /// Siege target score per unit of `isqrt` of the enemy strength at the
    /// target (units and shooters), against 1000 per tile of distance.
    pub target_strength_weight: u32,
    /// How strongly counters bend the army mix, per mille (0: off).
    pub counter_weight_permille: u32,
    /// The smallest counter factor, per mille (`..=1000`).
    pub counter_floor_permille: u32,
    /// The largest counter factor, per mille (`1000..=MAX_COUNTER_PERMILLE`).
    pub counter_cap_permille: u32,
    /// A `counter_kinds` entry joins the mix while its factor reaches this
    /// (`1000..=counter_cap_permille`).
    pub counter_join_permille: u32,
    /// Armed kinds outside `army_mix` that join it, with their weight,
    /// while they counter what the enemy shows.
    pub counter_kinds: Vec<MixEntry>,
    /// Enemy military near an own worked gathering building is a threat
    /// too.
    pub defend_economy: bool,
    /// See `defend_economy`.
    pub defend_economy_tiles: u32,
    /// The guard attacks enemy Supply Wains among the threats first.
    pub defend_wains_first: bool,
    /// After a released defence, the bot adds one Watchtower near the
    /// defended Town, up to this many in the match.
    pub reactive_towers: u32,
    /// The wave interval is jittered by up to this share, per mille.
    pub noise_wave_interval_permille: u32,
    /// Each wave needs up to this many more armed units (upward only).
    pub noise_wave_units: u32,
    /// The first wave waits up to this much longer.
    pub noise_first_wave_ds: u32,
    /// Each army-mix weight is jittered by up to this share, per mille.
    pub noise_mix_permille: u32,
    /// `Expansion` and `BorderEdge` sites are picked among the first this
    /// many candidates that pass the cheap tests (1: always the best).
    pub noise_site_top_k: u32,
}

impl Personality {
    /// The planning cadence in ticks (`think_every_ds` converted once).
    pub fn think_every_ticks(&self, rules: &Rules) -> u32 {
        rules.ticks_from_ds(self.think_every_ds)
    }
}

/// The worker kind the `Workers` step trains and gathers with: the first
/// unit kind, in `UnitKindId` order, that the Town kind trains and that
/// builds (the Yeoman). `None` when the rules have none.
pub fn worker_kind(rules: &Rules) -> Option<(u16, &crate::UnitKind)> {
    let town = rules.building_kind(rules.town_kind())?;
    rules
        .units
        .iter()
        .enumerate()
        .find(|(_, u)| u.builder && u.trainer == town.id)
        .map(|(i, u)| (u16::try_from(i).expect("validated len"), u))
}

/// The `.ron` files of `dir`, in path order (other files are ignored).
fn ron_files(dir: &Path) -> Result<Vec<PathBuf>, Error> {
    let entries = std::fs::read_dir(dir).map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "ron"))
        .collect();
    paths.sort();
    Ok(paths)
}

/// The file stem of `path`, which a keyed file's `id` must equal.
fn stem_of(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

/// `id` must equal the file stem of `path`.
fn check_stem(path: &Path, id: &str) -> Check {
    let stem = stem_of(path);
    if id != stem {
        return Err(Error::invalid(
            path,
            "id",
            format!("{id:?} must equal the file stem {stem:?}"),
        ));
    }
    Ok(())
}

/// Load and validate `dir/ai/` against the rest of `rules`: every build
/// order under `ai/build_orders/` (non-`.ron` files are ignored, files are
/// read in path order), then `ai/personalities.ron`, then (M6) every
/// difficulty level under `ai/difficulty/`, which must include
/// `standard`.
pub(crate) fn load(dir: &Path, rules: &Rules) -> Result<AiRules, Error> {
    let mut build_orders = BTreeMap::new();
    for path in ron_files(&dir.join("ai").join("build_orders"))? {
        let order: BuildOrder = load_ron(&path)?;
        check_stem(&path, &order.id)?;
        check_build_order(&path, &order, rules)?;
        build_orders.insert(stem_of(&path), order);
    }
    let personalities_path = dir.join("ai").join("personalities.ron");
    let file: Personalities = load_ron(&personalities_path)?;
    let mut ai = AiRules {
        personalities: file.personalities,
        build_orders,
        difficulties: BTreeMap::new(),
    };
    check_personalities(&personalities_path, &ai, rules)?;
    let difficulty_dir = dir.join("ai").join("difficulty");
    for path in ron_files(&difficulty_dir)? {
        let level: Difficulty = load_ron(&path)?;
        check_stem(&path, &level.id)?;
        check_difficulty(&path, &level, rules)?;
        ai.difficulties.insert(stem_of(&path), level);
    }
    if !ai.difficulties.contains_key(STANDARD_DIFFICULTY) {
        return Err(Error::invalid(
            &difficulty_dir.join(format!("{STANDARD_DIFFICULTY}.ron")),
            "id",
            format!(
                "no {STANDARD_DIFFICULTY:?} difficulty (found: {}); every tool defaults to it",
                ai.difficulties
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    Ok(ai)
}

/// One difficulty level: identity, the bonus bundle and its schedule, the
/// multipliers, the age gate and the first raid (M6).
fn check_difficulty(path: &Path, d: &Difficulty, rules: &Rules) -> Check {
    if d.id == PASSIVE_ID {
        return Err(Error::invalid(
            path,
            "id",
            format!("{PASSIVE_ID:?} is reserved for the bot that never acts"),
        ));
    }
    if d.name.is_empty() {
        return Err(Error::invalid(path, "name", "must not be empty"));
    }
    for (resource, amount) in &d.income {
        let at = format!("income.{resource}");
        if rules.resource_index(resource).is_none() {
            return Err(Error::invalid(
                path,
                &at,
                format!("{resource:?} is not a resource id in resources.ron"),
            ));
        }
        in_range(path, &at, *amount, 0, MAX_BONUS_AMOUNT)?;
    }
    if d.income_interval_ds != 0 {
        in_range(
            path,
            "income_interval_ds",
            d.income_interval_ds,
            MIN_INCOME_INTERVAL_DS,
            MAX_AI_DS,
        )?;
        if d.income_interval_ticks(rules) <= rules.cmd_delay_ticks {
            return Err(Error::invalid(
                path,
                "income_interval_ds",
                format!(
                    "{} ticks must be more than cmd_delay_ticks ({}) in rules.ron",
                    d.income_interval_ticks(rules),
                    rules.cmd_delay_ticks
                ),
            ));
        }
    }
    let below_cap = |field: &str, ds: u32| -> Check {
        if ds >= rules.match_cap_ds {
            return Err(Error::invalid(
                path,
                field,
                format!(
                    "{ds} is not below match_cap_ds ({}) in rules.ron",
                    rules.match_cap_ds
                ),
            ));
        }
        Ok(())
    };
    below_cap("income_from_ds", d.income_from_ds)?;
    for (name, v) in [
        ("wave_size_permille", d.wave_size_permille),
        ("wave_interval_permille", d.wave_interval_permille),
    ] {
        in_range(
            path,
            name,
            v,
            MIN_DIFFICULTY_PERMILLE,
            MAX_DIFFICULTY_PERMILLE,
        )?;
    }
    if let Some(n) = d.max_wave_units {
        in_range(path, "max_wave_units", n, 1, MAX_WAVE_UNITS)?;
    }
    in_range(
        path,
        "worker_target_permille",
        d.worker_target_permille,
        MIN_WORKER_TARGET_PERMILLE,
        1000,
    )?;
    if d.extra_research.len() > MAX_EXTRA_RESEARCH {
        return Err(Error::invalid(
            path,
            "extra_research",
            format!(
                "{} techs; a level adds at most {MAX_EXTRA_RESEARCH}",
                d.extra_research.len()
            ),
        ));
    }
    for (i, tech) in d.extra_research.iter().enumerate() {
        let at = format!("extra_research[{i}]");
        if rules.tech_by_id(tech).is_none() {
            return Err(Error::invalid(
                path,
                &at,
                format!("{tech:?} is not a tech id in techs.ron"),
            ));
        }
        if d.extra_research[..i].contains(tech) {
            return Err(Error::invalid(
                path,
                &at,
                format!("{tech:?} is listed twice"),
            ));
        }
    }
    let pr = &d.pressure;
    in_range(
        path,
        "pressure.reinforce_min_units",
        pr.reinforce_min_units,
        0,
        MAX_WAVE_UNITS,
    )?;
    in_range(path, "pressure.commit_ds", pr.commit_ds, 0, MAX_AI_DS)?;
    in_range(
        path,
        "pressure.retreat_ratio_permille",
        pr.retreat_ratio_permille,
        0,
        MAX_DIFFICULTY_PERMILLE,
    )?;
    in_range(
        path,
        "pressure.retreat_cooldown_permille",
        pr.retreat_cooldown_permille,
        MIN_DIFFICULTY_PERMILLE,
        MAX_DIFFICULTY_PERMILLE,
    )?;
    in_range(
        path,
        "pressure.home_guard_units",
        pr.home_guard_units,
        0,
        MAX_WAVE_UNITS,
    )?;
    in_range(path, "pressure.lure_ds", pr.lure_ds, 0, MAX_AI_DS)?;
    in_range(
        path,
        "pressure.cohesion_tiles",
        pr.cohesion_tiles,
        0,
        MAX_AI_TILES,
    )?;
    in_range(
        path,
        "pressure.launch_ratio_permille",
        pr.launch_ratio_permille,
        0,
        MAX_AI_RATIO_PERMILLE,
    )?;
    in_range(
        path,
        "pressure.assault_ratio_permille",
        pr.assault_ratio_permille,
        0,
        MAX_AI_RATIO_PERMILLE,
    )?;
    if let Some(ds) = d.all_in_after_ds {
        in_range(path, "all_in_after_ds", ds, 0, MAX_AI_DS)?;
    }
    if let WaveAgeGate::Age(age) = &d.wave_age_gate
        && rules.age_index(age).is_none()
    {
        return Err(Error::invalid(
            path,
            "wave_age_gate",
            format!("{age:?} is not an age id in ages.ron"),
        ));
    }
    if let Some(raid) = &d.first_raid {
        in_range(
            path,
            "first_raid.min_armed",
            raid.min_armed,
            1,
            MAX_WAVE_UNITS,
        )?;
        in_range(
            path,
            "first_raid.max_armed",
            raid.max_armed,
            raid.min_armed,
            MAX_WAVE_UNITS,
        )?;
        below_cap("first_raid.by_ds", raid.by_ds)?;
        if raid.prep_ds >= raid.by_ds {
            return Err(Error::invalid(
                path,
                "first_raid.prep_ds",
                format!("{} must be less than by_ds ({})", raid.prep_ds, raid.by_ds),
            ));
        }
    }
    Ok(())
}

type Check = Result<(), Error>;

fn in_range(path: &Path, field: &str, v: u32, lo: u32, hi: u32) -> Check {
    if v < lo || v > hi {
        return Err(Error::invalid(
            path,
            field,
            format!("{v} is outside {lo}..={hi}"),
        ));
    }
    Ok(())
}

/// The ids, ranges and static reachability of one build order.
fn check_build_order(path: &Path, order: &BuildOrder, rules: &Rules) -> Check {
    if order.steps.is_empty() || order.steps.len() > MAX_STEPS {
        return Err(Error::invalid(
            path,
            "steps",
            format!(
                "{} steps; a build order has 1..={MAX_STEPS}",
                order.steps.len()
            ),
        ));
    }
    let town = rules
        .building_kind(rules.town_kind())
        .expect("rules have a town kind");
    // What the steps before the current one guarantee (optional steps
    // guarantee nothing): the age, the kinds built (the Seat from the
    // start), the techs researched, and the Towns a Build of the Town kind
    // asked for.
    let mut age: u8 = 0;
    let mut built: BTreeSet<&str> = BTreeSet::from([town.id.as_str()]);
    let mut researched: BTreeSet<&str> = BTreeSet::new();
    let mut towns: u32 = 1;
    let mut advanced = false;
    for (i, step) in order.steps.iter().enumerate() {
        let field = |name: &str| format!("steps[{i}].{name}");
        let unreachable = |name: &str, why: String| {
            Error::invalid(
                path,
                &field(name),
                format!("{why}; mark the step `optional: true` or add the missing step before it"),
            )
        };
        let optional = step.optional();
        match step {
            Step::Workers { count, split } => {
                in_range(path, &field("count"), *count, 1, MAX_STEP_COUNT)?;
                let Some((_, worker)) = worker_kind(rules) else {
                    return Err(Error::invalid(
                        path,
                        &field("count"),
                        "no unit kind is trained at the Town kind and builds (the worker kind)",
                    ));
                };
                if split.is_empty() {
                    return Err(Error::invalid(
                        path,
                        &field("split"),
                        "needs at least one resource",
                    ));
                }
                let mut sum: u32 = 0;
                for (resource, weight) in split {
                    let at = field(&format!("split.{resource}"));
                    if !rules.resources.resources.iter().any(|r| &r.id == resource) {
                        return Err(Error::invalid(
                            path,
                            &at,
                            format!("{resource:?} is not a resource id in resources.ron"),
                        ));
                    }
                    let gathered = rules.buildings.iter().any(|b| {
                        b.gather
                            .as_ref()
                            .is_some_and(|g| &g.resource == resource && g.worker == worker.id)
                    });
                    if !gathered {
                        return Err(Error::invalid(
                            path,
                            &at,
                            format!(
                                "no building kind in buildings.ron is worked by {:?} for {resource:?}",
                                worker.id
                            ),
                        ));
                    }
                    in_range(path, &at, *weight, 1, MAX_STEP_COUNT)?;
                    sum = sum.saturating_add(*weight);
                }
                if sum > *count {
                    return Err(Error::invalid(
                        path,
                        &field("split"),
                        format!("weights sum to {sum}, more than the count {count}"),
                    ));
                }
            }
            Step::Build {
                kind, count, at, ..
            } => {
                in_range(path, &field("count"), *count, 1, MAX_STEP_COUNT)?;
                let Some((index, def)) = rules.building_kind_by_id(kind) else {
                    return Err(Error::invalid(
                        path,
                        &field("kind"),
                        format!("{kind:?} is not a building kind id in buildings.ron"),
                    ));
                };
                let placement_ok = match at {
                    Placement::Expansion => def.town,
                    Placement::Terrain => def
                        .gather
                        .as_ref()
                        .is_some_and(|g| matches!(g.slots, crate::Slots::Adjacent { .. })),
                    Placement::BorderEdge => def.border_radius_tiles.is_some(),
                    Placement::NearTown => true,
                };
                if !placement_ok {
                    let why = match at {
                        Placement::Expansion => "Expansion places only the Town kind",
                        Placement::Terrain => "Terrain places only kinds with Adjacent slots",
                        Placement::BorderEdge => {
                            "BorderEdge places only kinds with border_radius_tiles"
                        }
                        Placement::NearTown => unreachable!("NearTown places any kind"),
                    };
                    return Err(Error::invalid(
                        path,
                        &field("at"),
                        format!("{why}, not {kind:?}"),
                    ));
                }
                let needs = rules.age_of_building(index);
                if needs > age && !optional {
                    return Err(unreachable(
                        "kind",
                        format!(
                            "{kind:?} is unlocked in the {:?} age, which no earlier Advance reaches",
                            rules.ages[usize::from(needs)].id
                        ),
                    ));
                }
                if let Some(per_town) = def.max_per_town
                    && *count > per_town.saturating_mul(towns)
                    && !optional
                {
                    return Err(unreachable(
                        "count",
                        format!(
                            "{count} {kind:?} need more than {per_town} per Town (max_per_town) at the {towns} Town(s) earlier steps build"
                        ),
                    ));
                }
                if !optional {
                    built.insert(def.id.as_str());
                    if def.town {
                        towns = towns.max(*count);
                    }
                }
            }
            Step::Train { kind, count, .. } => {
                in_range(path, &field("count"), *count, 1, MAX_STEP_COUNT)?;
                let Some(index) = rules.units.iter().position(|u| &u.id == kind) else {
                    return Err(Error::invalid(
                        path,
                        &field("kind"),
                        format!("{kind:?} is not a unit kind id in units.ron"),
                    ));
                };
                let def = &rules.units[index];
                let index = u16::try_from(index).expect("validated len");
                let needs = rules.age_of_unit(index);
                if needs > age && !optional {
                    return Err(unreachable(
                        "kind",
                        format!(
                            "{kind:?} is unlocked in the {:?} age, which no earlier Advance reaches",
                            rules.ages[usize::from(needs)].id
                        ),
                    ));
                }
                if !built.contains(def.trainer.as_str()) && !optional {
                    return Err(unreachable(
                        "kind",
                        format!(
                            "{kind:?} trains at a {:?}, which no earlier step builds",
                            def.trainer
                        ),
                    ));
                }
                if let Some(tech) = &def.requires_tech
                    && !researched.contains(tech.as_str())
                    && !optional
                {
                    return Err(unreachable(
                        "kind",
                        format!("{kind:?} requires {tech:?}, which no earlier step researches"),
                    ));
                }
            }
            Step::Research { tech, .. } => {
                let Some((index, def)) = rules.tech_by_id(tech) else {
                    return Err(Error::invalid(
                        path,
                        &field("tech"),
                        format!("{tech:?} is not a tech id in techs.ron"),
                    ));
                };
                let needs = rules.age_of_tech(index);
                if needs > age && !optional {
                    return Err(unreachable(
                        "tech",
                        format!(
                            "{tech:?} is unlocked in the {:?} age, which no earlier Advance reaches",
                            rules.ages[usize::from(needs)].id
                        ),
                    ));
                }
                if !built.contains(def.host.as_str()) && !optional {
                    return Err(unreachable(
                        "tech",
                        format!(
                            "{tech:?} is researched at a {:?}, which no earlier step builds",
                            def.host
                        ),
                    ));
                }
                if let Some(pred) = rules.tech_predecessor(index) {
                    let pred = &rules.techs[usize::from(pred)].id;
                    if !researched.contains(pred.as_str()) && !optional {
                        return Err(unreachable(
                            "tech",
                            format!(
                                "{tech:?} needs {pred:?} first, which no earlier step researches"
                            ),
                        ));
                    }
                }
                if !optional {
                    researched.insert(def.id.as_str());
                }
            }
            Step::Advance { age: id } => {
                let Some(index) = rules.age_index(id) else {
                    return Err(Error::invalid(
                        path,
                        &field("age"),
                        format!("{id:?} is not an age id in ages.ron"),
                    ));
                };
                if index <= age {
                    return Err(Error::invalid(
                        path,
                        &field("age"),
                        format!(
                            "{id:?} is not later than the age the earlier steps reach; Advance steps must go up"
                        ),
                    ));
                }
                let need = rules.ages[usize::from(index)].techs_required;
                let have = researched
                    .iter()
                    .filter(|t| {
                        rules
                            .tech_by_id(t)
                            .is_some_and(|(_, d)| d.counts_toward_age)
                    })
                    .count();
                let have = u32::try_from(have).expect("few techs");
                if have < need {
                    return Err(Error::invalid(
                        path,
                        &field("age"),
                        format!(
                            "{id:?} needs {need} techs that count toward an age, but the earlier non-optional steps research {have}; an Advance is never optional"
                        ),
                    ));
                }
                // M6: a difficulty's deadline raid trains armed units before
                // the first Advance, so a trainer of an armed kind of the
                // start age must stand by then.
                if !advanced && !armed_trainer_built(rules, &built, &researched, age) {
                    return Err(Error::invalid(
                        path,
                        &field("age"),
                        "no earlier non-optional step builds a trainer of an armed kind (such as a Muster Hall); every build order builds one before its first Advance",
                    ));
                }
                advanced = true;
                age = index;
            }
            Step::Military => {}
            Step::Wait { until_ds } => {
                in_range(path, &field("until_ds"), *until_ds, 0, MAX_AI_DS)?;
            }
        }
    }
    Ok(())
}

/// Whether a building kind in `built` trains an armed, non-civilian unit
/// kind unlocked by `age` whose `requires_tech`, if any, is `researched`.
fn armed_trainer_built(
    rules: &Rules,
    built: &BTreeSet<&str>,
    researched: &BTreeSet<&str>,
    age: u8,
) -> bool {
    rules.units.iter().enumerate().any(|(i, u)| {
        let index = u16::try_from(i).expect("validated len");
        u.class != UnitClass::Civilian
            && u.attack.is_some()
            && built.contains(u.trainer.as_str())
            && rules.age_of_unit(index) <= age
            && u.requires_tech
                .as_ref()
                .is_none_or(|t| researched.contains(t.as_str()))
    })
}

/// The personality rows: ids, build orders, ranges and cross-checks.
fn check_personalities(path: &Path, ai: &AiRules, rules: &Rules) -> Check {
    let mut ids = BTreeSet::new();
    for (i, p) in ai.personalities.iter().enumerate() {
        let f = |name: &str| format!("personalities[{i}].{name}");
        if p.id.is_empty() || !ids.insert(p.id.as_str()) {
            return Err(Error::invalid(
                path,
                &f("id"),
                format!("{:?} is empty or used twice", p.id),
            ));
        }
        if !ai.build_orders.contains_key(&p.build_order) {
            return Err(Error::invalid(
                path,
                &f("build_order"),
                format!(
                    "{:?} is not a file stem under ai/build_orders/ (found: {})",
                    p.build_order,
                    ai.build_orders
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        in_range(
            path,
            &f("think_every_ds"),
            p.think_every_ds,
            1,
            MAX_THINK_EVERY_DS,
        )?;
        if p.think_every_ticks(rules) <= rules.cmd_delay_ticks {
            return Err(Error::invalid(
                path,
                &f("think_every_ds"),
                format!(
                    "{} ticks must be more than cmd_delay_ticks ({}) in rules.ron, so a plan's commands apply before the next plan",
                    p.think_every_ticks(rules),
                    rules.cmd_delay_ticks
                ),
            ));
        }
        for (name, v) in [
            ("lookahead", p.lookahead),
            ("max_commands_per_think", p.max_commands_per_think),
            ("site_checks_per_think", p.site_checks_per_think),
            ("rebalance_per_think", p.rebalance_per_think),
            ("max_queue_per_building", p.max_queue_per_building),
            ("wains_per_wave", p.wains_per_wave),
            ("ring_units", p.ring_units),
            ("clear_defenders_units", p.clear_defenders_units),
            ("continue_min_units", p.continue_min_units),
            ("recall_below_units", p.recall_below_units),
            ("defend_guard_pop_ratio", p.defend_guard_pop_ratio),
        ] {
            in_range(path, &f(name), v, 1, MAX_AI_CAP)?;
        }
        in_range(
            path,
            &f("max_wave_units"),
            p.max_wave_units,
            1,
            MAX_WAVE_UNITS,
        )?;
        in_range(path, &f("min_siege"), p.min_siege, 0, p.max_wave_units)?;
        if p.max_queue_per_building > rules.train_queue_max {
            return Err(Error::invalid(
                path,
                &f("max_queue_per_building"),
                format!(
                    "{} is more than train_queue_max ({}) in rules.ron",
                    p.max_queue_per_building, rules.train_queue_max
                ),
            ));
        }
        for (name, v) in [
            ("building_gap_tiles", p.building_gap_tiles),
            ("near_town_min_tiles", p.near_town_min_tiles),
            ("near_town_max_tiles", p.near_town_max_tiles),
            ("expansion_min_tiles", p.expansion_min_tiles),
            ("resource_radius_tiles", p.resource_radius_tiles),
            ("border_scan_tiles", p.border_scan_tiles),
            ("tower_spacing_tiles", p.tower_spacing_tiles),
            ("rally_tiles_from_seat", p.rally_tiles_from_seat),
            ("assemble_radius_tiles", p.assemble_radius_tiles),
            ("goal_tolerance_tiles", p.goal_tolerance_tiles),
            ("stage_margin_tiles", p.stage_margin_tiles),
            ("stage_radius_tiles", p.stage_radius_tiles),
            ("siege_focus_tiles", p.siege_focus_tiles),
            ("wain_standoff_tiles", p.wain_standoff_tiles),
            ("wain_leash_tiles", p.wain_leash_tiles),
            ("defend_radius_tiles", p.defend_radius_tiles),
            ("defend_regoal_tiles", p.defend_regoal_tiles),
        ] {
            in_range(path, &f(name), v, 1, MAX_AI_TILES)?;
        }
        if p.near_town_min_tiles > p.near_town_max_tiles {
            return Err(Error::invalid(
                path,
                &f("near_town_min_tiles"),
                format!(
                    "{} is more than near_town_max_tiles ({})",
                    p.near_town_min_tiles, p.near_town_max_tiles
                ),
            ));
        }
        in_range(
            path,
            &f("toward_enemy_penalty"),
            p.toward_enemy_penalty,
            0,
            MAX_AI_WEIGHT,
        )?;
        for (name, v, lo) in [
            ("blocked_retry_ds", p.blocked_retry_ds, 1),
            ("first_wave_not_before_ds", p.first_wave_not_before_ds, 0),
            ("wave_interval_ds", p.wave_interval_ds, 1),
            ("stage_max_ds", p.stage_max_ds, 1),
            ("wave_max_ds", p.wave_max_ds, 1),
            ("all_in_after_ds", p.all_in_after_ds, 0),
            ("defend_release_ds", p.defend_release_ds, 1),
            ("defend_max_block_ds", p.defend_max_block_ds, 1),
        ] {
            in_range(path, &f(name), v, lo, MAX_AI_DS)?;
        }
        in_range(
            path,
            &f("stage_ready_permille"),
            p.stage_ready_permille,
            1,
            1000,
        )?;
        in_range(
            path,
            &f("recall_wave_town_hp_permille"),
            p.recall_wave_town_hp_permille,
            0,
            1000,
        )?;
        if p.army_mix.is_empty() {
            return Err(Error::invalid(
                path,
                &f("army_mix"),
                "needs at least one kind",
            ));
        }
        let mut mix = BTreeSet::new();
        for (j, e) in p.army_mix.iter().enumerate() {
            let at = f(&format!("army_mix[{j}].kind"));
            let Some(def) = rules.unit_kind_by_id(&e.kind) else {
                return Err(Error::invalid(
                    path,
                    &at,
                    format!("{:?} is not a unit kind id in units.ron", e.kind),
                ));
            };
            if def.class == UnitClass::Civilian || def.attack.is_none() {
                return Err(Error::invalid(
                    path,
                    &at,
                    format!("{:?} is not an armed, non-civilian kind", e.kind),
                ));
            }
            if !mix.insert(e.kind.as_str()) {
                return Err(Error::invalid(
                    path,
                    &at,
                    format!("{:?} is listed twice", e.kind),
                ));
            }
            in_range(
                path,
                &f(&format!("army_mix[{j}].weight")),
                e.weight,
                1,
                MAX_AI_WEIGHT,
            )?;
        }
        if !rules
            .unit_kind_by_id(&p.supply_kind)
            .is_some_and(|k| k.supplies)
        {
            return Err(Error::invalid(
                path,
                &f("supply_kind"),
                format!(
                    "{:?} is not a unit kind that supplies in units.ron",
                    p.supply_kind
                ),
            ));
        }
        if p.wave_sizes.is_empty() {
            return Err(Error::invalid(
                path,
                &f("wave_sizes"),
                "needs at least one row",
            ));
        }
        for (j, w) in p.wave_sizes.iter().enumerate() {
            let at = |name: &str| f(&format!("wave_sizes[{j}].{name}"));
            in_range(path, &at("from_ds"), w.from_ds, 0, MAX_AI_DS)?;
            in_range(path, &at("armed"), w.armed, 1, p.max_wave_units)?;
            match j.checked_sub(1).map(|k| &p.wave_sizes[k]) {
                None if w.from_ds != 0 => {
                    return Err(Error::invalid(
                        path,
                        &at("from_ds"),
                        "the first row starts at 0",
                    ));
                }
                Some(prev) if w.from_ds <= prev.from_ds || w.armed < prev.armed => {
                    return Err(Error::invalid(
                        path,
                        &at("from_ds"),
                        "from_ds must increase and armed must not decrease from row to row",
                    ));
                }
                _ => {}
            }
        }
        if let Some(age) = &p.wave_requires_age
            && rules.age_index(age).is_none()
        {
            return Err(Error::invalid(
                path,
                &f("wave_requires_age"),
                format!("{age:?} is not an age id in ages.ron"),
            ));
        }
        check_m6_knobs(path, &f, p, rules)?;
    }
    Ok(())
}

/// The M6 knobs of one personality row (`f` names its fields).
fn check_m6_knobs(
    path: &Path,
    f: &dyn Fn(&str) -> String,
    p: &Personality,
    rules: &Rules,
) -> Check {
    if let Some(tech) = &p.wave_requires_tech
        && rules.tech_by_id(tech).is_none()
    {
        return Err(Error::invalid(
            path,
            &f("wave_requires_tech"),
            format!("{tech:?} is not a tech id in techs.ron"),
        ));
    }
    in_range(
        path,
        &f("raid_min_armed"),
        p.raid_min_armed,
        1,
        MAX_WAVE_UNITS,
    )?;
    in_range(
        path,
        &f("raid_max_armed"),
        p.raid_max_armed,
        p.raid_min_armed,
        MAX_WAVE_UNITS,
    )?;
    for (name, v, lo) in [
        ("raid_every_ds", p.raid_every_ds, 1),
        ("raid_max_ds", p.raid_max_ds, 1),
        ("raid_idle_ds", p.raid_idle_ds, 1),
        ("retreat_commit_ds", p.retreat_commit_ds, 0),
        ("retreat_cooldown_ds", p.retreat_cooldown_ds, 0),
        ("noise_first_wave_ds", p.noise_first_wave_ds, 0),
    ] {
        in_range(path, &f(name), v, lo, MAX_AI_DS)?;
    }
    for (name, v) in [
        ("raid_idle_tiles", p.raid_idle_tiles),
        ("defend_economy_tiles", p.defend_economy_tiles),
    ] {
        in_range(path, &f(name), v, 1, MAX_AI_TILES)?;
    }
    in_range(
        path,
        &f("raid_worker_weight"),
        p.raid_worker_weight,
        0,
        MAX_AI_WEIGHT,
    )?;
    in_range(
        path,
        &f("target_strength_weight"),
        p.target_strength_weight,
        0,
        MAX_AI_WEIGHT,
    )?;
    for (name, v) in [
        ("raid_distance_permille", p.raid_distance_permille),
        ("retreat_ratio_permille", p.retreat_ratio_permille),
    ] {
        in_range(path, &f(name), v, 0, MAX_AI_RATIO_PERMILLE)?;
    }
    for (name, v) in [
        ("retreat_loss_permille", p.retreat_loss_permille),
        (
            "retreat_commit_town_hp_permille",
            p.retreat_commit_town_hp_permille,
        ),
    ] {
        in_range(path, &f(name), v, 0, 1000)?;
    }
    in_range(
        path,
        &f("influence_cell_tiles"),
        p.influence_cell_tiles,
        1,
        MAX_INFLUENCE_CELL_TILES,
    )?;
    in_range(
        path,
        &f("influence_radius_cells"),
        p.influence_radius_cells,
        0,
        MAX_INFLUENCE_RADIUS_CELLS,
    )?;
    in_range(
        path,
        &f("counter_weight_permille"),
        p.counter_weight_permille,
        0,
        MAX_COUNTER_PERMILLE,
    )?;
    in_range(
        path,
        &f("counter_floor_permille"),
        p.counter_floor_permille,
        0,
        1000,
    )?;
    in_range(
        path,
        &f("counter_cap_permille"),
        p.counter_cap_permille,
        1000,
        MAX_COUNTER_PERMILLE,
    )?;
    in_range(
        path,
        &f("counter_join_permille"),
        p.counter_join_permille,
        1000,
        p.counter_cap_permille,
    )?;
    let mut kinds = BTreeSet::new();
    for (j, e) in p.counter_kinds.iter().enumerate() {
        let at = f(&format!("counter_kinds[{j}].kind"));
        let Some(def) = rules.unit_kind_by_id(&e.kind) else {
            return Err(Error::invalid(
                path,
                &at,
                format!("{:?} is not a unit kind id in units.ron", e.kind),
            ));
        };
        if def.class == UnitClass::Civilian || def.attack.is_none() {
            return Err(Error::invalid(
                path,
                &at,
                format!("{:?} is not an armed, non-civilian kind", e.kind),
            ));
        }
        if !kinds.insert(e.kind.as_str()) {
            return Err(Error::invalid(
                path,
                &at,
                format!("{:?} is listed twice", e.kind),
            ));
        }
        in_range(
            path,
            &f(&format!("counter_kinds[{j}].weight")),
            e.weight,
            1,
            MAX_AI_WEIGHT,
        )?;
    }
    in_range(
        path,
        &f("reactive_towers"),
        p.reactive_towers,
        0,
        MAX_AI_CAP,
    )?;
    for (name, v) in [
        (
            "noise_wave_interval_permille",
            p.noise_wave_interval_permille,
        ),
        ("noise_mix_permille", p.noise_mix_permille),
    ] {
        in_range(path, &f(name), v, 0, MAX_NOISE_PERMILLE)?;
    }
    in_range(
        path,
        &f("noise_wave_units"),
        p.noise_wave_units,
        0,
        MAX_WAVE_UNITS,
    )?;
    in_range(
        path,
        &f("noise_site_top_k"),
        p.noise_site_top_k,
        1,
        MAX_SITE_TOP_K,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    fn copy_dir(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let (from, to) = (entry.path(), dst.join(entry.file_name()));
            if from.is_dir() {
                copy_dir(&from, &to);
            } else {
                std::fs::copy(&from, &to).unwrap();
            }
        }
    }

    /// Load a copy of `data/` after replacing, in each `(file, from, to)`,
    /// the first `from` with `to` (asserting it is there).
    fn load_edited(name: &str, edits: &[(&str, &str, &str)]) -> Result<Rules, Error> {
        let dir =
            std::env::temp_dir().join(format!("eonmark-ai-rules-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        copy_dir(&data_dir(), &dir);
        for (file, from, to) in edits {
            let path = dir.join(file);
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.contains(from), "{file} no longer contains {from:?}");
            std::fs::write(&path, text.replacen(from, to, 1)).unwrap();
        }
        let rules = Rules::load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        rules
    }

    const BO: &str = "ai/build_orders/standard.ron";
    const PE: &str = "ai/personalities.ron";
    const EASY: &str = "ai/difficulty/easy.ron";
    const HARD: &str = "ai/difficulty/hard.ron";

    /// M6: the four personalities (standard first, then the `random`
    /// pool rush, boom and tower) and their build orders; the M5b
    /// `standard` row and build order are as they were.
    #[test]
    fn repo_ai_data_loads_four_personalities_and_build_orders() {
        let rules = Rules::load(data_dir()).unwrap();
        assert_eq!(
            rules
                .ai
                .personalities
                .iter()
                .map(|p| (p.id.as_str(), p.build_order.as_str(), p.random))
                .collect::<Vec<_>>(),
            [
                ("standard", "standard", false),
                ("rush", "rush", true),
                ("boom", "boom", true),
                ("tower", "tower", true)
            ]
        );
        assert_eq!(
            rules
                .ai
                .build_orders
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["boom", "rush", "standard", "tower"]
        );
        let rush = rules.ai.personality("rush").unwrap();
        assert_eq!(
            (rush.wave_mode, rush.wain_rule, rush.seat_last),
            (WaveMode::RaidUntilSiege, WainRule::IfEnemyHarrying, false)
        );
        let tower = rules.ai.personality("tower").unwrap();
        assert_eq!(tower.wave_requires_tech.as_deref(), Some("harrying_1"));
        assert!(tower.defend_wains_first && tower.reactive_towers > 0);
        let p = rules.ai.personality("standard").unwrap();
        assert_eq!(p.build_order, "standard");
        assert_eq!(p.think_every_ticks(&rules), 10);
        assert_eq!(p.recall_wave, RecallWave::WhileStaging);
        assert_eq!(p.wave_requires_age.as_deref(), Some("masonry"));
        let order = rules.ai.build_order("standard").unwrap();
        assert_eq!(order.steps.len(), 38);
        assert_eq!(
            order.steps[0],
            Step::Workers {
                count: 7,
                split: BTreeMap::from([("grain".into(), 3), ("lumber".into(), 4)]),
            }
        );
        assert!(order.steps[10].optional(), "the second Town is optional");
        assert!(matches!(order.steps[12], Step::Military));
        assert_eq!(
            worker_kind(&rules).map(|(i, u)| (i, u.id.as_str())),
            Some((0, "yeoman"))
        );
        let standard = rules.ai.personality("standard").unwrap();
        assert_eq!(
            (standard.wave_mode, standard.wain_rule, standard.seat_last),
            (WaveMode::Siege, WainRule::Always, true)
        );
    }

    /// `data/ai` is in `rules_hash`: a knob edit changes the hash, and so
    /// (M6) does a difficulty edit, the line-4 flip's one-line edit of
    /// `hard.ron` included.
    #[test]
    fn ai_data_is_in_the_rules_hash() {
        let repo = Rules::load(data_dir()).unwrap();
        let edited = load_edited("hash", &[(PE, "ring_units: 4,", "ring_units: 5,")]).unwrap();
        assert_ne!(repo.rules_hash(), edited.rules_hash());
        let mut cleared = repo.clone();
        cleared.ai = AiRules::default();
        assert_ne!(repo.rules_hash(), cleared.rules_hash());
        let flipped = load_edited(
            "hash-flip",
            &[(HARD, "income_interval_ds: 200,", "income_interval_ds: 300,")],
        )
        .unwrap();
        assert_ne!(repo.rules_hash(), flipped.rules_hash());
        assert_eq!(
            flipped.ai.difficulty("hard").unwrap().income_interval_ds,
            300
        );
        let mut no_levels = repo.clone();
        no_levels.ai.difficulties.remove("easy");
        assert_ne!(repo.rules_hash(), no_levels.rules_hash());
    }

    /// M6: the three shipped levels, their schedules and the knobs they
    /// scale. The bundle is the same at every level, so the interval alone
    /// sizes the cheat.
    #[test]
    fn repo_difficulties_are_easy_standard_hard() {
        let rules = Rules::load(data_dir()).unwrap();
        assert_eq!(
            rules
                .ai
                .difficulties
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["easy", "hard", "standard"]
        );
        let level = |id: &str| rules.ai.difficulty(id).unwrap();
        let (easy, standard, hard) = (level("easy"), level("standard"), level("hard"));
        assert_eq!(
            [easy, standard, hard].map(|d| d.income_interval_ticks(&rules)),
            [600, 500, 400]
        );
        // From 3:00; the first payment completes one interval later.
        for d in [easy, standard, hard] {
            assert_eq!(d.income_from_ticks(&rules), 3600);
            assert!(d.pays());
            assert_eq!(d.bundle(&rules), standard.bundle(&rules));
        }
        let bundle = standard.bundle(&rules);
        let grain = rules.resource_index("grain").unwrap();
        let lore = rules.resource_index("lore").unwrap();
        assert_eq!(bundle[grain], 20);
        assert_eq!(bundle[lore], 0);
        assert_eq!(easy.wave_age_gate, WaveAgeGate::Age("masonry".into()));
        assert_eq!(standard.wave_age_gate, WaveAgeGate::Personality);
        assert_eq!(hard.wave_age_gate, WaveAgeGate::Off);
        assert!(easy.largest_wave_only && easy.first_raid.is_none());
        assert_eq!(hard.first_raid.map(|r| r.by_ds), Some(6000));
        assert_eq!(standard.first_raid.map(|r| r.by_ds), Some(9000));
        assert!(rules.ai.difficulty("passive").is_none());
    }

    /// M6: `resolve` scales the wave sizes once (rounded up, at least 1,
    /// capped), the interval (rounded to nearest), sets the age gate and
    /// the all-in time, and `largest_wave_only` flattens the rows; it
    /// leaves every other knob alone. Standard resolves to the
    /// personality with its all-in at 36:00 (M6 owner decision 12); it
    /// retreats as the personality does, and Hard never retreats.
    #[test]
    fn resolve_scales_sizes_and_intervals_once_and_caps_largest_only() {
        let rules = Rules::load(data_dir()).unwrap();
        let p = rules.ai.personality("standard").unwrap();
        let level = |id: &str| rules.ai.difficulty(id).unwrap();
        let mut want = p.clone();
        want.all_in_after_ds = 21600;
        assert_eq!(resolve(p, level("standard")), want);
        assert_eq!(resolve(p, level("hard")).retreat_ratio_permille, 0);
        let sizes = |k: &Personality| k.wave_sizes.iter().map(|w| w.armed).collect::<Vec<_>>();
        let easy = resolve(p, level("easy"));
        assert_eq!(sizes(&easy), [14, 14, 14, 14]);
        assert_eq!(easy.max_wave_units, 14);
        assert_eq!(easy.wave_interval_ds, 2700);
        assert_eq!(easy.wave_requires_age.as_deref(), Some("masonry"));
        assert_eq!(easy.all_in_after_ds, 19800);
        let hard = resolve(p, level("hard"));
        assert_eq!(sizes(&hard), sizes(p));
        assert_eq!(hard.wave_interval_ds, 1440);
        assert_eq!(hard.wave_requires_age, None);
        assert_eq!(hard.all_in_after_ds, p.all_in_after_ds);
        // Rounding: sizes up, at least 1, under the cap; the interval to
        // nearest, half up (the level's all-in and pressure off, so nothing
        // else moves).
        let mut d = level("standard").clone();
        d.all_in_after_ds = None;
        d.pressure = Pressure::OFF;
        d.wave_size_permille = 250;
        d.wave_interval_permille = 333;
        d.max_wave_units = Some(3);
        let mut q = p.clone();
        q.wave_sizes[0].armed = 1;
        q.wave_interval_ds = 1500;
        q.min_siege = 5;
        let r = resolve(&q, &d);
        assert_eq!(
            sizes(&r),
            [1, 3, 3, 3],
            "1 -> 1, 12 -> 3, 16 -> 4 capped, 24 -> 6 capped"
        );
        assert_eq!(r.wave_interval_ds, 500, "1500 x 333 / 1000 = 499.5 -> 500");
        assert_eq!((r.max_wave_units, r.min_siege), (3, 3));
        // M6: the raid sizes are capped too.
        assert_eq!((r.raid_min_armed, r.raid_max_armed), (3, 3));
        // Nothing else moves.
        let mut same = r.clone();
        same.wave_sizes = q.wave_sizes.clone();
        same.wave_interval_ds = q.wave_interval_ds;
        same.max_wave_units = q.max_wave_units;
        same.min_siege = q.min_siege;
        same.raid_min_armed = q.raid_min_armed;
        same.raid_max_armed = q.raid_max_armed;
        assert_eq!(same, q);
    }

    /// M6: a disclosed Yeomen handicap (M6 owner decision 2's fallback,
    /// `worker_target_permille` 850) scales every `Workers` count of the
    /// shipped Standard build order, rounded up (15% fewer Yeomen), and
    /// leaves the splits and every other step alone; at 1000 (every shipped
    /// level since the M6 retune: line 2 passes without the fallback) the
    /// order's steps are unchanged, and a count never drops below 1.
    #[test]
    fn resolve_order_scales_worker_counts_by_the_easy_handicap() {
        let rules = Rules::load(data_dir()).unwrap();
        let order = rules.ai.build_order("standard").unwrap();
        let level = |id: &str| rules.ai.difficulty(id).unwrap();
        let counts = |o: &BuildOrder| {
            o.steps
                .iter()
                .filter_map(|s| match s {
                    Step::Workers { count, .. } => Some(*count),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(counts(order), [7, 9, 11, 14, 17, 20, 23]);
        let mut handicap = level("easy").clone();
        handicap.worker_target_permille = 850;
        handicap.extra_research.clear();
        let easy = resolve_order(order, &handicap);
        assert_eq!(counts(&easy), [6, 8, 10, 12, 15, 17, 20]);
        assert_eq!(easy.id, order.id);
        assert_eq!(easy.steps.len(), order.steps.len());
        for (got, was) in easy.steps.iter().zip(&order.steps) {
            match (got, was) {
                (Step::Workers { split: a, .. }, Step::Workers { split: b, .. }) => {
                    assert_eq!(a, b, "the split weights stay");
                }
                _ => assert_eq!(got, was, "a non-Workers step stays"),
            }
        }
        for id in ["standard", "hard"] {
            assert_eq!(level(id).worker_target_permille, 1000, "{id}");
            assert_eq!(&resolve_order(order, level(id)), order, "{id}");
        }
        // Shipped Easy: no Yeomen handicap; its Statecraft II is appended.
        assert_eq!(level("easy").worker_target_permille, 1000);
        let shipped_easy = resolve_order(order, level("easy"));
        assert_eq!(counts(&shipped_easy), counts(order));
        assert_eq!(&shipped_easy.steps[..order.steps.len()], &order.steps[..]);
        let one = BuildOrder {
            id: "one".into(),
            steps: vec![Step::Workers {
                count: 1,
                split: BTreeMap::from([("grain".to_string(), 1)]),
            }],
        };
        let mut floor = level("easy").clone();
        floor.worker_target_permille = MIN_WORKER_TARGET_PERMILLE;
        assert_eq!(counts(&resolve_order(&one, &floor)), [1]);
    }

    /// M6 owner decision 12: a level's pressure scales the personality's
    /// retreat ratio and cooldown (rounded to nearest, half up; 0 never
    /// retreats) and touches no other knob.
    #[test]
    fn pressure_scales_the_retreat_knobs_only() {
        let rules = Rules::load(data_dir()).unwrap();
        let p = rules.ai.personality("standard").unwrap();
        let mut d = rules.ai.difficulty("standard").unwrap().clone();
        d.wave_age_gate = WaveAgeGate::Personality;
        d.largest_wave_only = false;
        d.max_wave_units = None;
        d.wave_size_permille = 1000;
        d.wave_interval_permille = 1000;
        d.all_in_after_ds = None;
        d.pressure = Pressure::OFF;
        assert_eq!(&resolve(p, &d), p, "no pressure: the personality itself");
        d.pressure.retreat_ratio_permille = 500;
        d.pressure.retreat_cooldown_permille = 333;
        let r = resolve(p, &d);
        assert_eq!(
            r.retreat_ratio_permille,
            (p.retreat_ratio_permille * 500 + 500) / 1000
        );
        assert_eq!(
            r.retreat_cooldown_ds,
            (p.retreat_cooldown_ds * 333 + 500) / 1000
        );
        let mut same = r.clone();
        same.retreat_ratio_permille = p.retreat_ratio_permille;
        same.retreat_cooldown_ds = p.retreat_cooldown_ds;
        assert_eq!(&same, p, "nothing else moves");
        d.pressure.retreat_ratio_permille = 0;
        assert_eq!(resolve(p, &d).retreat_ratio_permille, 0, "never retreats");
    }

    /// M6 owner decision 12 (mirrored sites must not make cap games exact
    /// copies): the research a level or a build order adds is what keeps
    /// two seats from building mirror images, so it is pinned here. Easy's
    /// resolved `standard` order differs from Standard's and Hard's (its
    /// Statecraft II), and the `tower` order researches Statecraft II
    /// itself (as `boom` does); Standard and Hard resolve the `standard`
    /// order identically,
    /// the known limit (an untouched Standard-vs-Hard or Standard mirror cap
    /// game is still decided by site noise; docs/design/ai.md, "Difficulty
    /// table (as retuned)").
    #[test]
    fn the_research_that_breaks_mirrored_cap_games_is_pinned() {
        let rules = Rules::load(data_dir()).unwrap();
        let order = rules.ai.build_order("standard").unwrap();
        let resolved = |id: &str| resolve_order(order, rules.ai.difficulty(id).unwrap());
        let statecraft_2 = |o: &BuildOrder| {
            o.steps
                .iter()
                .any(|s| matches!(s, Step::Research { tech, .. } if tech == "statecraft_2"))
        };
        let (easy, standard, hard) = (resolved("easy"), resolved("standard"), resolved("hard"));
        assert!(statecraft_2(&easy), "Easy researches Statecraft II");
        assert!(!statecraft_2(&standard) && !statecraft_2(&hard));
        assert_ne!(easy, standard, "Easy vs Standard");
        assert_ne!(easy, hard, "Hard vs Easy (the flip)");
        assert_eq!(
            standard, hard,
            "Standard vs Hard: the same order (the known limit)"
        );
        // Since the retune `tower` researches Statecraft II too (boom has
        // since slice 2), so a tower seat's untouched cap game is not a
        // mirror of a rush or Balanced seat's.
        let has = |id: &str| statecraft_2(rules.ai.build_order(id).unwrap());
        assert!(has("tower"), "tower researches Statecraft II");
        assert!(has("boom"));
        assert!(!has("standard") && !has("rush"));
    }

    /// M6 owner decision 12: `extra_research` appends one optional
    /// `Research` step per tech the build order does not research yet, in
    /// the level's order, and leaves every other step alone.
    #[test]
    fn extra_research_appends_optional_steps_once() {
        let rules = Rules::load(data_dir()).unwrap();
        let order = rules.ai.build_order("standard").unwrap();
        let mut d = rules.ai.difficulty("standard").unwrap().clone();
        d.worker_target_permille = 1000;
        d.extra_research = Vec::new();
        assert_eq!(&resolve_order(order, &d), order);
        d.extra_research = vec!["statecraft_2".into(), "arms_1".into(), "letters_2".into()];
        let o = resolve_order(order, &d);
        let n = order.steps.len();
        assert_eq!(o.steps[..n], order.steps[..], "the order itself stays");
        assert_eq!(
            o.steps[n..],
            [
                Step::Research {
                    tech: "statecraft_2".into(),
                    optional: true
                },
                Step::Research {
                    tech: "letters_2".into(),
                    optional: true
                },
            ],
            "arms_1 is researched already"
        );
    }

    /// M6: `passive` is reserved for the bot that never acts, a `standard`
    /// level must exist, and non-`.ron` files under `ai/difficulty/` are
    /// ignored.
    #[test]
    fn difficulty_files_reserve_passive_and_require_standard() {
        let dir = std::env::temp_dir().join(format!(
            "eonmark-ai-rules-difficulty-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        copy_dir(&data_dir(), &dir);
        let levels = dir.join("ai").join("difficulty");
        std::fs::write(levels.join("NOTES.md"), "not a level").unwrap();
        assert_eq!(Rules::load(&dir).unwrap().ai.difficulties.len(), 3);
        let hard = std::fs::read_to_string(levels.join("hard.ron")).unwrap();
        std::fs::write(
            levels.join("passive.ron"),
            hard.replacen("id: \"hard\"", "id: \"passive\"", 1),
        )
        .unwrap();
        let err = Rules::load(&dir).unwrap_err().to_string();
        assert!(
            err.contains("passive.ron") && err.contains("reserved"),
            "{err}"
        );
        std::fs::remove_file(levels.join("passive.ron")).unwrap();
        std::fs::remove_file(levels.join("standard.ron")).unwrap();
        let err = Rules::load(&dir).unwrap_err().to_string();
        assert!(
            err.contains("standard.ron") && err.contains("no \"standard\" difficulty"),
            "{err}"
        );
        std::fs::remove_dir_all(&levels).unwrap();
        let err = Rules::load(&dir).unwrap_err().to_string();
        assert!(err.contains("difficulty"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Non-`.ron` files under `ai/build_orders/` are ignored, and a second
    /// valid build order loads under its file stem.
    #[test]
    fn build_orders_are_the_ron_files_keyed_by_stem() {
        let dir =
            std::env::temp_dir().join(format!("eonmark-ai-rules-extra-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        copy_dir(&data_dir(), &dir);
        let orders = dir.join(BO).parent().unwrap().to_path_buf();
        std::fs::write(orders.join("NOTES.md"), "not a build order").unwrap();
        std::fs::write(
            orders.join("tiny.ron"),
            "(id: \"tiny\", steps: [Workers(count: 6, split: {\"grain\": 6}), Military])",
        )
        .unwrap();
        let rules = Rules::load(&dir).unwrap();
        assert_eq!(
            rules
                .ai
                .build_orders
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["boom", "rush", "standard", "tiny", "tower"]
        );
        std::fs::write(orders.join("empty.ron"), "(id: \"empty\", steps: [])").unwrap();
        let err = Rules::load(&dir).unwrap_err().to_string();
        assert!(err.contains("empty.ron") && err.contains("steps"), "{err}");
        std::fs::remove_file(orders.join("empty.ron")).unwrap();
        std::fs::remove_file(dir.join(PE)).unwrap();
        let err = Rules::load(&dir).unwrap_err().to_string();
        assert!(err.contains("personalities.ron"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every validator: each edit of the shipped data fails to load with an
    /// error naming the file, the field and the reason. The step indices
    /// are those of the shipped `standard.ron`.
    #[test]
    fn every_ai_validator_names_the_file_and_the_field() {
        let cases: &[(&str, &[(&str, &str, &str)], &[&str])] = &[
            // Build orders: ids and ranges.
            (
                "id",
                &[(BO, "id: \"standard\"", "id: \"other\"")],
                &["standard.ron", "id", "file stem"],
            ),
            (
                "kind",
                &[(
                    BO,
                    "Build(kind: \"croft\", count: 2",
                    "Build(kind: \"crofty\", count: 2",
                )],
                &["steps[1].kind", "crofty"],
            ),
            (
                "count0",
                &[(BO, "Workers(count: 7,", "Workers(count: 0,")],
                &["steps[0].count", "1..=200"],
            ),
            (
                "count201",
                &[(
                    BO,
                    "Train(kind: \"scribe\", count: 3)",
                    "Train(kind: \"scribe\", count: 201)",
                )],
                &["steps[5].count", "1..=200"],
            ),
            (
                "split-empty",
                &[(BO, "split: {\"grain\": 3, \"lumber\": 4}", "split: {}")],
                &["steps[0].split", "at least one"],
            ),
            (
                "split-sum",
                &[(
                    BO,
                    "split: {\"grain\": 3, \"lumber\": 4}",
                    "split: {\"grain\": 3, \"lumber\": 5}",
                )],
                &["steps[0].split", "sum to 8"],
            ),
            (
                "split-id",
                &[(
                    BO,
                    "{\"grain\": 3, \"lumber\": 4}",
                    "{\"grain\": 3, \"wood\": 4}",
                )],
                &["steps[0].split.wood", "resource id"],
            ),
            (
                "split-worker",
                &[(
                    BO,
                    "{\"grain\": 3, \"lumber\": 4}",
                    "{\"grain\": 3, \"lore\": 4}",
                )],
                &["steps[0].split.lore", "worked by \"yeoman\""],
            ),
            (
                "train-kind",
                &[(BO, "Train(kind: \"scribe\"", "Train(kind: \"monk\"")],
                &["steps[5].kind", "unit kind id"],
            ),
            (
                "tech",
                &[(
                    BO,
                    "Research(tech: \"arms_1\")",
                    "Research(tech: \"arms_9\")",
                )],
                &["steps[13].tech", "tech id"],
            ),
            (
                "age",
                &[(BO, "Advance(age: \"masonry\")", "Advance(age: \"bronze\")")],
                &["steps[19].age", "age id"],
            ),
            (
                "wait",
                &[(BO, "    Military,", "    Wait(until_ds: 72001),")],
                &["steps[12].until_ds", "0..=72000"],
            ),
            (
                "unknown-field",
                &[(
                    BO,
                    "Train(kind: \"scribe\", count: 3)",
                    "Train(kind: \"scribe\", count: 3, at: NearTown)",
                )],
                &["standard.ron", "at"],
            ),
            (
                "optional-advance",
                &[(
                    BO,
                    "Advance(age: \"masonry\")",
                    "Advance(age: \"masonry\", optional: true)",
                )],
                &["standard.ron", "optional"],
            ),
            // Placement kinds.
            (
                "expansion",
                &[(
                    BO,
                    "Build(kind: \"croft\", count: 2, at: NearTown)",
                    "Build(kind: \"croft\", count: 2, at: Expansion)",
                )],
                &["steps[1].at", "Expansion places only the Town kind"],
            ),
            (
                "terrain",
                &[(
                    BO,
                    "Build(kind: \"scriptorium\", count: 1, at: NearTown)",
                    "Build(kind: \"scriptorium\", count: 1, at: Terrain)",
                )],
                &["steps[3].at", "Adjacent"],
            ),
            (
                "border",
                &[(
                    BO,
                    "Build(kind: \"muster_hall\", count: 1, at: NearTown)",
                    "Build(kind: \"muster_hall\", count: 1, at: BorderEdge)",
                )],
                &["steps[11].at", "border_radius_tiles"],
            ),
            // Static reachability.
            (
                "unlock",
                &[
                    (BO, "Advance(age: \"masonry\"),", "Wait(until_ds: 0),"),
                    (BO, "Advance(age: \"charter\"),", "Wait(until_ds: 0),"),
                ],
                &["steps[21].kind", "\"engine_yard\"", "masonry", "optional"],
            ),
            (
                "advance-techs",
                &[(BO, "Research(tech: \"trade_1\"),", "Wait(until_ds: 0),")],
                &["steps[19].age", "needs 2 techs", "research 1"],
            ),
            (
                "trainer",
                &[(
                    BO,
                    "Build(kind: \"scriptorium\", count: 1, at: NearTown)",
                    "Build(kind: \"scriptorium\", count: 1, at: NearTown, optional: true)",
                )],
                &["steps[5].kind", "trains at a \"scriptorium\""],
            ),
            (
                "host",
                &[
                    (
                        BO,
                        "Build(kind: \"scriptorium\", count: 1, at: NearTown)",
                        "Build(kind: \"scriptorium\", count: 1, at: NearTown, optional: true)",
                    ),
                    (
                        BO,
                        "Train(kind: \"scribe\", count: 3)",
                        "Train(kind: \"scribe\", count: 3, optional: true)",
                    ),
                ],
                &["steps[13].tech", "researched at a \"scriptorium\""],
            ),
            (
                "predecessor",
                &[(
                    BO,
                    "Research(tech: \"letters_1\")",
                    "Research(tech: \"letters_2\")",
                )],
                &["steps[29].tech", "needs \"letters_1\" first"],
            ),
            (
                "per-town",
                &[(
                    BO,
                    "Build(kind: \"croft\", count: 8, at: NearTown, optional: true)",
                    "Build(kind: \"croft\", count: 8, at: NearTown)",
                )],
                &["steps[27].count", "max_per_town"],
            ),
            (
                "ages-go-up",
                &[(BO, "Advance(age: \"charter\")", "Advance(age: \"masonry\")")],
                &["steps[32].age", "must go up"],
            ),
            // Personalities.
            (
                "p-id",
                &[(PE, "id: \"standard\", name", "id: \"\", name")],
                &["personalities.ron", "personalities[0].id"],
            ),
            (
                "p-build-order",
                &[(PE, "build_order: \"standard\"", "build_order: \"zerg\"")],
                &["personalities[0].build_order", "\"zerg\""],
            ),
            (
                "p-cadence",
                &[(PE, "think_every_ds: 5,", "think_every_ds: 1,")],
                &["personalities[0].think_every_ds", "cmd_delay_ticks"],
            ),
            (
                "p-cadence-max",
                &[(PE, "think_every_ds: 5,", "think_every_ds: 601,")],
                &["personalities[0].think_every_ds", "1..=600"],
            ),
            (
                "p-cap0",
                &[(PE, "lookahead: 3,", "lookahead: 0,")],
                &["personalities[0].lookahead", "1..=64"],
            ),
            (
                "p-cap65",
                &[(
                    PE,
                    "max_commands_per_think: 16,",
                    "max_commands_per_think: 65,",
                )],
                &["personalities[0].max_commands_per_think", "1..=64"],
            ),
            (
                "p-queue",
                &[(
                    PE,
                    "max_queue_per_building: 1,",
                    "max_queue_per_building: 6,",
                )],
                &["personalities[0].max_queue_per_building", "train_queue_max"],
            ),
            (
                "p-guard0",
                &[(
                    PE,
                    "defend_guard_pop_ratio: 2,",
                    "defend_guard_pop_ratio: 0,",
                )],
                &["personalities[0].defend_guard_pop_ratio", "1..=64"],
            ),
            (
                "p-radius0",
                &[(PE, "defend_radius_tiles: 16,", "defend_radius_tiles: 0,")],
                &["personalities[0].defend_radius_tiles", "1..=64"],
            ),
            (
                "p-radius65",
                &[(PE, "border_scan_tiles: 40,", "border_scan_tiles: 65,")],
                &["personalities[0].border_scan_tiles", "1..=64"],
            ),
            (
                "p-near-town",
                &[(PE, "near_town_min_tiles: 4,", "near_town_min_tiles: 13,")],
                &[
                    "personalities[0].near_town_min_tiles",
                    "near_town_max_tiles",
                ],
            ),
            (
                "p-penalty",
                &[(
                    PE,
                    "toward_enemy_penalty: 2,",
                    "toward_enemy_penalty: 1001,",
                )],
                &["personalities[0].toward_enemy_penalty", "0..=1000"],
            ),
            (
                "p-ds",
                &[(PE, "wave_interval_ds: 1800,", "wave_interval_ds: 72001,")],
                &["personalities[0].wave_interval_ds", "72000"],
            ),
            (
                "p-ds0",
                &[(PE, "defend_release_ds: 150,", "defend_release_ds: 0,")],
                &["personalities[0].defend_release_ds", "1..=72000"],
            ),
            (
                "p-permille",
                &[(
                    PE,
                    "stage_ready_permille: 750,",
                    "stage_ready_permille: 1001,",
                )],
                &["personalities[0].stage_ready_permille", "1..=1000"],
            ),
            (
                "p-hp-permille",
                &[(
                    PE,
                    "recall_wave_town_hp_permille: 500,",
                    "recall_wave_town_hp_permille: 1001,",
                )],
                &["personalities[0].recall_wave_town_hp_permille", "0..=1000"],
            ),
            (
                "p-mix-empty",
                &[(
                    PE,
                    "army_mix: [(kind: \"skirmisher\", weight: 3), (kind: \"bowman\", weight: 2),\n                 (kind: \"shieldbearer\", weight: 1), (kind: \"mangonel\", weight: 2)]",
                    "army_mix: []",
                )],
                &["personalities[0].army_mix", "at least one"],
            ),
            (
                "p-mix-civilian",
                &[(
                    PE,
                    "(kind: \"skirmisher\", weight: 3)",
                    "(kind: \"yeoman\", weight: 3)",
                )],
                &["personalities[0].army_mix[0].kind", "armed, non-civilian"],
            ),
            (
                "p-mix-unarmed",
                &[(
                    PE,
                    "(kind: \"skirmisher\", weight: 3)",
                    "(kind: \"supply_wain\", weight: 3)",
                )],
                &["personalities[0].army_mix[0].kind", "armed, non-civilian"],
            ),
            (
                "p-mix-unknown",
                &[(
                    PE,
                    "(kind: \"skirmisher\", weight: 3)",
                    "(kind: \"knight\", weight: 3)",
                )],
                &["personalities[0].army_mix[0].kind", "unit kind id"],
            ),
            (
                "p-mix-twice",
                &[(
                    PE,
                    "(kind: \"bowman\", weight: 2)",
                    "(kind: \"skirmisher\", weight: 2)",
                )],
                &["personalities[0].army_mix[1].kind", "twice"],
            ),
            (
                "p-mix-weight",
                &[(
                    PE,
                    "(kind: \"bowman\", weight: 2)",
                    "(kind: \"bowman\", weight: 0)",
                )],
                &["personalities[0].army_mix[1].weight", "1..=1000"],
            ),
            (
                "p-supply",
                &[(
                    PE,
                    "supply_kind: \"supply_wain\"",
                    "supply_kind: \"mangonel\"",
                )],
                &["personalities[0].supply_kind", "supplies"],
            ),
            (
                "p-wave-empty",
                &[(
                    PE,
                    "wave_sizes: [(from_ds: 0, armed: 8), (from_ds: 9000, armed: 12),\n                   (from_ds: 12000, armed: 16), (from_ds: 15000, armed: 24)]",
                    "wave_sizes: []",
                )],
                &["personalities[0].wave_sizes", "at least one"],
            ),
            (
                "p-wave-first",
                &[(PE, "(from_ds: 0, armed: 8)", "(from_ds: 10, armed: 8)")],
                &["personalities[0].wave_sizes[0].from_ds", "starts at 0"],
            ),
            (
                "p-wave-order",
                &[(PE, "(from_ds: 9000, armed: 12)", "(from_ds: 0, armed: 12)")],
                &["personalities[0].wave_sizes[1].from_ds", "must increase"],
            ),
            (
                "p-wave-armed",
                &[(
                    PE,
                    "(from_ds: 9000, armed: 12)",
                    "(from_ds: 9000, armed: 7)",
                )],
                &[
                    "personalities[0].wave_sizes[1].from_ds",
                    "must not decrease",
                ],
            ),
            (
                "p-wave-armed0",
                &[(PE, "(from_ds: 0, armed: 8)", "(from_ds: 0, armed: 0)")],
                &["personalities[0].wave_sizes[0].armed", "1..=40"],
            ),
            (
                "p-wave-max",
                &[(PE, "max_wave_units: 40,", "max_wave_units: 20,")],
                &["personalities[0].wave_sizes[3].armed", "1..=20"],
            ),
            (
                "p-wave-units",
                &[(PE, "max_wave_units: 40,", "max_wave_units: 65,")],
                &["personalities[0].max_wave_units", "1..=64"],
            ),
            (
                "p-siege",
                &[(PE, "min_siege: 1,", "min_siege: 41,")],
                &["personalities[0].min_siege", "0..=40"],
            ),
            (
                "p-age",
                &[(PE, "Some(\"masonry\")", "Some(\"bronze\")")],
                &["personalities[0].wave_requires_age", "age id"],
            ),
            (
                "p-unknown-field",
                &[(PE, "ring_units: 4,", "ring_units: 4, ring_size: 2,")],
                &["personalities.ron", "ring_size"],
            ),
            (
                "p-recall",
                &[(PE, "recall_wave: WhileStaging", "recall_wave: Sometimes")],
                &["personalities.ron", "Sometimes"],
            ),
            // M6: the knobs of slice 2.
            (
                "p-tech",
                &[(
                    PE,
                    "wave_requires_tech: None,",
                    "wave_requires_tech: Some(\"arms_9\"),",
                )],
                &["personalities[0].wave_requires_tech", "tech id"],
            ),
            (
                "p-raid-min",
                &[(PE, "raid_min_armed: 6,", "raid_min_armed: 0,")],
                &["personalities[0].raid_min_armed", "1..=64"],
            ),
            (
                "p-raid-max",
                &[(PE, "raid_max_armed: 10,", "raid_max_armed: 5,")],
                &["personalities[0].raid_max_armed", "6..=64"],
            ),
            (
                "p-raid-every",
                &[(PE, "raid_every_ds: 1200,", "raid_every_ds: 0,")],
                &["personalities[0].raid_every_ds", "1..=72000"],
            ),
            (
                "p-raid-tiles",
                &[(PE, "raid_idle_tiles: 10,", "raid_idle_tiles: 65,")],
                &["personalities[0].raid_idle_tiles", "1..=64"],
            ),
            (
                "p-raid-weight",
                &[(PE, "raid_worker_weight: 100,", "raid_worker_weight: 1001,")],
                &["personalities[0].raid_worker_weight", "0..=1000"],
            ),
            (
                "p-retreat-ratio",
                &[(
                    PE,
                    "retreat_ratio_permille: 650,",
                    "retreat_ratio_permille: 10001,",
                )],
                &["personalities[0].retreat_ratio_permille", "0..=10000"],
            ),
            (
                "p-retreat-loss",
                &[(
                    PE,
                    "retreat_loss_permille: 300,",
                    "retreat_loss_permille: 1001,",
                )],
                &["personalities[0].retreat_loss_permille", "0..=1000"],
            ),
            (
                "p-cell",
                &[(PE, "influence_cell_tiles: 4,", "influence_cell_tiles: 0,")],
                &["personalities[0].influence_cell_tiles", "1..=16"],
            ),
            (
                "p-cells",
                &[(
                    PE,
                    "influence_radius_cells: 1,",
                    "influence_radius_cells: 9,",
                )],
                &["personalities[0].influence_radius_cells", "0..=8"],
            ),
            (
                "p-counter-floor",
                &[(
                    PE,
                    "counter_floor_permille: 250,",
                    "counter_floor_permille: 1001,",
                )],
                &["personalities[0].counter_floor_permille", "0..=1000"],
            ),
            (
                "p-counter-cap",
                &[(
                    PE,
                    "counter_cap_permille: 4000,",
                    "counter_cap_permille: 999,",
                )],
                &["personalities[0].counter_cap_permille", "1000..=8000"],
            ),
            (
                "p-counter-join",
                &[(
                    PE,
                    "counter_join_permille: 1500,",
                    "counter_join_permille: 4001,",
                )],
                &["personalities[0].counter_join_permille", "1000..=4000"],
            ),
            (
                "p-counter-kind",
                &[(
                    PE,
                    "(kind: \"outrider\", weight: 2)",
                    "(kind: \"supply_wain\", weight: 2)",
                )],
                &[
                    "personalities[0].counter_kinds[0].kind",
                    "armed, non-civilian",
                ],
            ),
            (
                "p-counter-weight",
                &[(
                    PE,
                    "(kind: \"outrider\", weight: 2)",
                    "(kind: \"outrider\", weight: 0)",
                )],
                &["personalities[0].counter_kinds[0].weight", "1..=1000"],
            ),
            (
                "p-economy-tiles",
                &[(PE, "defend_economy_tiles: 8,", "defend_economy_tiles: 0,")],
                &["personalities[0].defend_economy_tiles", "1..=64"],
            ),
            (
                "p-towers",
                &[(PE, "reactive_towers: 0,", "reactive_towers: 65,")],
                &["personalities[0].reactive_towers", "0..=64"],
            ),
            (
                "p-noise",
                &[(PE, "noise_mix_permille: 200,", "noise_mix_permille: 501,")],
                &["personalities[0].noise_mix_permille", "0..=500"],
            ),
            (
                "p-top-k",
                &[(PE, "noise_site_top_k: 2,", "noise_site_top_k: 0,")],
                &["personalities[0].noise_site_top_k", "1..=8"],
            ),
            (
                "p-mode",
                &[(PE, "wave_mode: Siege,", "wave_mode: Sometimes,")],
                &["personalities.ron", "Sometimes"],
            ),
            (
                "p-random-missing",
                &[(PE, "      random: false,\n", "")],
                &["personalities.ron", "random"],
            ),
            (
                "d-workers",
                &[(
                    EASY,
                    "worker_target_permille: ",
                    "worker_target_permille: 1",
                )],
                &["easy.ron", "worker_target_permille", "500..=1000"],
            ),
            (
                "d-research-id",
                &[(EASY, "extra_research: [", "extra_research: [\"nope\", ")],
                &["easy.ron", "extra_research[0]", "nope"],
            ),
            (
                "d-research-twice",
                &[(
                    EASY,
                    "extra_research: [",
                    "extra_research: [\"arms_1\", \"arms_1\", ",
                )],
                &["easy.ron", "extra_research[1]", "twice"],
            ),
            // M6 owner decision 12: the pressure block's ranges (the
            // trailing `//` comments out the shipped value).
            (
                "d-reinforce",
                &[(EASY, "reinforce_min_units: ", "reinforce_min_units: 65, //")],
                &["easy.ron", "pressure.reinforce_min_units", "0..=64"],
            ),
            (
                "d-commit",
                &[(EASY, "commit_ds: ", "commit_ds: 72001, //")],
                &["easy.ron", "pressure.commit_ds", "0..=72000"],
            ),
            (
                "d-retreat",
                &[(
                    HARD,
                    "retreat_ratio_permille: ",
                    "retreat_ratio_permille: 4001, //",
                )],
                &["hard.ron", "pressure.retreat_ratio_permille", "0..=4000"],
            ),
            (
                "d-cooldown",
                &[(
                    HARD,
                    "retreat_cooldown_permille: ",
                    "retreat_cooldown_permille: 249, //",
                )],
                &[
                    "hard.ron",
                    "pressure.retreat_cooldown_permille",
                    "250..=4000",
                ],
            ),
            (
                "d-guard",
                &[(EASY, "home_guard_units: ", "home_guard_units: 65, //")],
                &["easy.ron", "pressure.home_guard_units", "0..=64"],
            ),
            (
                "d-lure",
                &[(HARD, "lure_ds: ", "lure_ds: 72001, //")],
                &["hard.ron", "pressure.lure_ds", "0..=72000"],
            ),
            (
                "d-cohesion",
                &[(HARD, "cohesion_tiles: ", "cohesion_tiles: 65, //")],
                &["hard.ron", "pressure.cohesion_tiles", "0..=64"],
            ),
            (
                "d-launch",
                &[(
                    HARD,
                    "launch_ratio_permille: ",
                    "launch_ratio_permille: 10001, //",
                )],
                &["hard.ron", "pressure.launch_ratio_permille", "0..=10000"],
            ),
            (
                "d-assault",
                &[(
                    HARD,
                    "assault_ratio_permille: ",
                    "assault_ratio_permille: 10001, //",
                )],
                &["hard.ron", "pressure.assault_ratio_permille", "0..=10000"],
            ),
            // M6: a trainer of an armed kind before the first Advance.
            (
                "armed-trainer",
                &[
                    (
                        BO,
                        "Build(kind: \"muster_hall\", count: 1, at: NearTown)",
                        "Build(kind: \"muster_hall\", count: 1, at: NearTown, optional: true)",
                    ),
                    (
                        BO,
                        "Build(kind: \"muster_hall\", count: 2, at: NearTown)",
                        "Build(kind: \"muster_hall\", count: 2, at: NearTown, optional: true)",
                    ),
                ],
                &["standard.ron", "steps[19].age", "trainer of an armed kind"],
            ),
            // M6: difficulty levels.
            (
                "d-id",
                &[(HARD, "id: \"hard\"", "id: \"harder\"")],
                &["hard.ron", "id", "file stem"],
            ),
            (
                "d-name",
                &[(HARD, "name: \"Hard\"", "name: \"\"")],
                &["hard.ron", "name", "empty"],
            ),
            (
                "d-income-id",
                &[(HARD, "\"ore\": 10", "\"iron\": 10")],
                &["hard.ron", "income.iron", "resource id"],
            ),
            (
                "d-income-max",
                &[(HARD, "\"grain\": 20", "\"grain\": 501")],
                &["hard.ron", "income.grain", "0..=500"],
            ),
            (
                "d-interval-short",
                &[(HARD, "income_interval_ds: 200,", "income_interval_ds: 49,")],
                &["hard.ron", "income_interval_ds", "50..=72000"],
            ),
            (
                "d-interval-long",
                &[(
                    HARD,
                    "income_interval_ds: 200,",
                    "income_interval_ds: 72001,",
                )],
                &["hard.ron", "income_interval_ds", "50..=72000"],
            ),
            (
                "d-from",
                &[(HARD, "income_from_ds: 1800,", "income_from_ds: 27000,")],
                &["hard.ron", "income_from_ds", "match_cap_ds"],
            ),
            (
                "d-size-permille",
                &[(
                    HARD,
                    "wave_size_permille: 1000,",
                    "wave_size_permille: 249,",
                )],
                &["hard.ron", "wave_size_permille", "250..=4000"],
            ),
            (
                "d-interval-permille",
                &[(
                    HARD,
                    "wave_interval_permille: 800,",
                    "wave_interval_permille: 4001,",
                )],
                &["hard.ron", "wave_interval_permille", "250..=4000"],
            ),
            (
                "d-max-units",
                &[(
                    EASY,
                    "max_wave_units: Some(14),",
                    "max_wave_units: Some(65),",
                )],
                &["easy.ron", "max_wave_units", "1..=64"],
            ),
            (
                "d-all-in",
                &[(
                    EASY,
                    "all_in_after_ds: Some(19800),",
                    "all_in_after_ds: Some(72001),",
                )],
                &["easy.ron", "all_in_after_ds", "0..=72000"],
            ),
            (
                "d-age",
                &[(EASY, "Age(\"masonry\")", "Age(\"bronze\")")],
                &["easy.ron", "wave_age_gate", "age id"],
            ),
            (
                "d-gate",
                &[(HARD, "wave_age_gate: Off,", "wave_age_gate: Never,")],
                &["hard.ron", "Never"],
            ),
            (
                "d-raid-min",
                &[(HARD, "min_armed: 6,", "min_armed: 0,")],
                &["hard.ron", "first_raid.min_armed", "1..=64"],
            ),
            (
                "d-raid-max",
                &[(HARD, "max_armed: 10)", "max_armed: 5)")],
                &["hard.ron", "first_raid.max_armed", "6..=64"],
            ),
            (
                "d-raid-by",
                &[(HARD, "by_ds: 6000,", "by_ds: 27000,")],
                &["hard.ron", "first_raid.by_ds", "match_cap_ds"],
            ),
            (
                "d-raid-prep",
                &[(HARD, "prep_ds: 1500,", "prep_ds: 6000,")],
                &["hard.ron", "first_raid.prep_ds", "less than by_ds"],
            ),
            (
                "d-unknown-field",
                &[(
                    HARD,
                    "largest_wave_only: false,",
                    "largest_wave_only: false, cheat: true,",
                )],
                &["hard.ron", "cheat"],
            ),
        ];
        for (name, edits, words) in cases {
            let err = load_edited(name, edits).expect_err(name).to_string();
            for w in *words {
                assert!(err.contains(w), "{name}: {err:?} lacks {w:?}");
            }
        }
    }
}
