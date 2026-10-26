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

/// Everything under `data/ai/`, attached to [`crate::Rules::ai`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AiRules {
    /// The rows of `ai/personalities.ron`, in file order.
    pub personalities: Vec<Personality>,
    /// The build orders of `ai/build_orders/*.ron`, keyed by file stem
    /// (which equals each one's `id`).
    pub build_orders: BTreeMap<String, BuildOrder>,
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

/// Load and validate `dir/ai/` against the rest of `rules`: every build
/// order under `ai/build_orders/` (non-`.ron` files are ignored, files are
/// read in path order), then `ai/personalities.ron`.
pub(crate) fn load(dir: &Path, rules: &Rules) -> Result<AiRules, Error> {
    let orders_dir = dir.join("ai").join("build_orders");
    let entries = std::fs::read_dir(&orders_dir).map_err(|source| Error::Io {
        path: orders_dir.clone(),
        source,
    })?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "ron"))
        .collect();
    paths.sort();
    let mut build_orders = BTreeMap::new();
    for path in paths {
        let order: BuildOrder = load_ron(&path)?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        if order.id != stem {
            return Err(Error::invalid(
                &path,
                "id",
                format!("{:?} must equal the file stem {stem:?}", order.id),
            ));
        }
        check_build_order(&path, &order, rules)?;
        build_orders.insert(stem, order);
    }
    let personalities_path = dir.join("ai").join("personalities.ron");
    let file: Personalities = load_ron(&personalities_path)?;
    let ai = AiRules {
        personalities: file.personalities,
        build_orders,
    };
    check_personalities(&personalities_path, &ai, rules)?;
    Ok(ai)
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
    }
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

    #[test]
    fn repo_ai_data_loads_one_personality_and_one_build_order() {
        let rules = Rules::load(data_dir()).unwrap();
        assert_eq!(rules.ai.personalities.len(), 1);
        assert_eq!(rules.ai.build_orders.len(), 1);
        let p = rules.ai.personality("standard").unwrap();
        assert_eq!(p.build_order, "standard");
        assert_eq!(p.think_every_ticks(&rules), 10);
        assert_eq!(p.recall_wave, RecallWave::WhileStaging);
        assert_eq!(p.wave_requires_age.as_deref(), Some("masonry"));
        let order = rules.ai.build_order("standard").unwrap();
        assert_eq!(order.steps.len(), 37);
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
        assert!(rules.ai.personality("rush").is_none());
    }

    /// `data/ai` is in `rules_hash`: a knob edit changes the hash.
    #[test]
    fn ai_data_is_in_the_rules_hash() {
        let repo = Rules::load(data_dir()).unwrap();
        let edited = load_edited("hash", &[(PE, "ring_units: 4,", "ring_units: 5,")]).unwrap();
        assert_ne!(repo.rules_hash(), edited.rules_hash());
        let mut cleared = repo.clone();
        cleared.ai = AiRules::default();
        assert_ne!(repo.rules_hash(), cleared.rules_hash());
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
            ["standard", "tiny"]
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
                &["steps[18].age", "age id"],
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
                &["steps[19].kind", "\"engine_yard\"", "masonry", "optional"],
            ),
            (
                "advance-techs",
                &[(BO, "Research(tech: \"trade_1\"),", "Wait(until_ds: 0),")],
                &["steps[18].age", "needs 2 techs", "research 1"],
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
                &["steps[27].tech", "needs \"letters_1\" first"],
            ),
            (
                "per-town",
                &[(
                    BO,
                    "Build(kind: \"croft\", count: 8, at: NearTown, optional: true)",
                    "Build(kind: \"croft\", count: 8, at: NearTown)",
                )],
                &["steps[25].count", "max_per_town"],
            ),
            (
                "ages-go-up",
                &[(BO, "Advance(age: \"charter\")", "Advance(age: \"masonry\")")],
                &["steps[30].age", "must go up"],
            ),
            // Personalities.
            (
                "p-id",
                &[(PE, "id: \"standard\", name", "id: \"\", name")],
                &["personalities.ron", "personalities[0].id"],
            ),
            (
                "p-build-order",
                &[(PE, "build_order: \"standard\"", "build_order: \"rush\"")],
                &["personalities[0].build_order", "\"rush\""],
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
        ];
        for (name, edits, words) in cases {
            let err = load_edited(name, edits).expect_err(name).to_string();
            for w in *words {
                assert!(err.contains(w), "{name}: {err:?} lacks {w:?}");
            }
        }
    }
}
