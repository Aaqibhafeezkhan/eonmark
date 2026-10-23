//! Headless command-line tooling for the Eonmark simulation.
//!
//! Runs on any OS without a GPU. M0 implemented `selftest`, `data-check` and
//! the scripted `hash-dump`; M1 adds `verify`, `bench`, `fuzz`, `record` and
//! the replay form of `hash-dump`; `play-bots` arrives in M5b.
//!
//! Wall-clock timing (`std::time::Instant`) is allowed here and banned in
//! `sim`, `rules` and `ai`: this crate measures the sim, it is not part of it.
#![forbid(unsafe_code)]

mod scenarios;

use clap::{Parser, Subcommand};
use scenarios::Scenario;
use sim::replay::DEFAULT_HASH_EVERY;
use sim::{
    AStarSearch, BuildingId, FxVec2, Map, MatchResult, MatchSetup, Outcome, OutcomeReason,
    PlayerId, ReplayFile, ReplayReader, ReplayWriter, Rules, SearchStatus, Sim, SimEvent, SimView,
    Source, SourceOwner, SubHashes, TerritoryField, Tile, UnitId, VerifyOutcome,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

#[derive(Parser)]
#[command(
    name = "sim-cli",
    about = "Headless Eonmark simulation tooling",
    version
)]
struct Cli {
    /// Data directory (rules, maps).
    #[arg(long, global = true, default_value = "data")]
    data: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run scripted ticks twice on two threads and compare hashes.
    Selftest {
        /// Ticks to simulate in each run.
        #[arg(long, default_value_t = 1000)]
        ticks: u32,
        /// Match seed.
        #[arg(long, default_value_t = 42)]
        seed: u64,
    },
    /// Load and validate everything under a data directory.
    DataCheck {
        /// Path to the data directory (overrides --data).
        dir: Option<PathBuf>,
    },
    /// Print per-tick hashes: of a replay's re-simulation (with sub-hashes
    /// every --every ticks) when a replay path is given, otherwise of the
    /// scripted M0 match.
    HashDump {
        /// Replay file (.eonreplay) to re-simulate.
        replay: Option<PathBuf>,
        /// Print a line every N ticks (replay form).
        #[arg(long, default_value_t = 20)]
        every: u32,
        /// Ticks to simulate (scripted form).
        #[arg(long, default_value_t = 100)]
        ticks: u32,
        /// Match seed (scripted form).
        #[arg(long, default_value_t = 42)]
        seed: u64,
    },
    /// Re-simulate a replay and report OK / DIVERGED / SIM VERSION MISMATCH /
    /// RULES CHANGED (exit 0 only for OK).
    Verify {
        /// Replay file (.eonreplay).
        replay: PathBuf,
    },
    /// Measure step time: N units crossing the map, N units fighting, one
    /// saturated A* budget, or territory recompute.
    Bench {
        /// Units to spawn and move (--combat: both sides together, four kinds
        /// a side, rounded down to a multiple of 8).
        #[arg(long, default_value_t = 500)]
        units: u16,
        /// Ticks to simulate.
        #[arg(long, default_value_t = 1200)]
        ticks: u32,
        /// Benchmark a saturated A* tick instead of the crossing.
        #[arg(long, conflicts_with = "territory")]
        astar: bool,
        /// Benchmark incremental territory recompute instead of the crossing.
        #[arg(long)]
        territory: bool,
        /// Benchmark a battle instead of the crossing: --units split into two
        /// sides that attack-move at each other.
        #[arg(long, conflicts_with_all = ["astar", "territory"])]
        combat: bool,
        /// With --combat: switch attrition on (M5a). Both players research
        /// Harrying I and II, P1 owns a Town beside the arena, and each side
        /// attack-moves a Supply Wain with its army.
        #[arg(long, requires = "combat")]
        harrying: bool,
        /// Border sources placed before the timed edits (--territory).
        #[arg(long, default_value_t = 16)]
        sources: u32,
        /// Timed source edits (--territory).
        #[arg(long, default_value_t = 100)]
        edits: u32,
        /// Expansion budget for --astar (defaults to rules.ron).
        #[arg(long)]
        budget: Option<u32>,
        /// Match seed.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// List every unit that did not arrive (id, position, order state).
        #[arg(long)]
        verbose: bool,
        /// Print the arrived percentage every 100 ticks and the first tick
        /// at which 99% of the units are within 3 tiles of their goal.
        #[arg(long)]
        arrival_curve: bool,
    },
    /// Feed seeded random command streams to the sim and compare two runs.
    Fuzz {
        /// Ticks per run.
        #[arg(long, default_value_t = 300)]
        ticks: u32,
        /// Seed of the first case; one case per seed.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Number of cases.
        #[arg(long, default_value_t = 32)]
        cases: u32,
    },
    /// Record a scripted scenario to an .eonreplay file.
    Record {
        /// Scenario name (`move_500`, `move_500_short`, `group_spiral`,
        /// `snapshot_restore`, `selftest`, ...; an unknown name lists them).
        #[arg(long)]
        scenario: String,
        /// Ticks to simulate (default: the scenario's fixture length;
        /// `capture_seat` stops at the step that decides the match).
        #[arg(long)]
        ticks: Option<u32>,
        /// Output file.
        #[arg(long)]
        out: PathBuf,
        /// Write a hash record every tick instead of every 20.
        #[arg(long, conflicts_with = "hash_every")]
        hash_every_tick: bool,
        /// Write a hash record every N ticks (default: the scenario's own,
        /// 1000 for `stalemate_45min`, else 20).
        #[arg(long)]
        hash_every: Option<u32>,
        /// Match seed (default: the scenario's own seed; fixtures must keep it).
        #[arg(long)]
        seed: Option<u64>,
        /// Sleep this many milliseconds after every tick (to land a `kill -9`
        /// mid-recording when testing truncation tolerance).
        #[arg(long, default_value_t = 0)]
        slow_ms: u64,
    },
    /// Run seeded bot-vs-bot matches in parallel (M5b).
    PlayBots,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Selftest { ticks, seed } => selftest(&cli.data, ticks, seed),
        Cmd::DataCheck { dir } => data_check(dir.as_deref().unwrap_or(&cli.data)),
        Cmd::HashDump {
            replay: None,
            ticks,
            seed,
            ..
        } => hash_dump(&cli.data, ticks, seed),
        Cmd::HashDump {
            replay: Some(replay),
            every,
            ..
        } => hash_dump_replay(&cli.data, &replay, every),
        Cmd::Verify { replay } => verify(&cli.data, &replay),
        Cmd::Bench {
            combat: true,
            harrying,
            units,
            ticks,
            seed,
            ..
        } => bench_combat(&cli.data, units, ticks, seed, harrying),
        Cmd::Bench {
            territory: true,
            sources,
            edits,
            seed,
            ..
        } => bench_territory(&cli.data, sources, edits, seed),
        Cmd::Bench {
            units,
            ticks,
            astar,
            budget,
            seed,
            verbose,
            arrival_curve,
            ..
        } => bench(
            &cli.data,
            units,
            ticks,
            astar,
            budget,
            seed,
            verbose,
            arrival_curve,
        ),
        Cmd::Fuzz { ticks, seed, cases } => fuzz(&cli.data, ticks, seed, cases),
        Cmd::Record {
            scenario,
            ticks,
            out,
            hash_every_tick,
            hash_every,
            seed,
            slow_ms,
        } => record(
            &cli.data,
            &scenario,
            ticks,
            &out,
            if hash_every_tick { Some(1) } else { hash_every },
            seed,
            slow_ms,
        ),
        Cmd::PlayBots => not_yet("play-bots", "M5b"),
    }
}

/// Exit status for a failure that is neither a verify outcome nor success:
/// unreadable files, unloadable rules, unknown scenario names.
const EXIT_ERROR: u8 = 2;

fn not_yet(name: &str, milestone: &str) -> ExitCode {
    eprintln!("{name}: not implemented until {milestone}");
    ExitCode::from(EXIT_ERROR)
}

fn load_rules(data: &Path) -> Result<Rules, ExitCode> {
    Rules::load(data).map_err(|e| {
        eprintln!("error: {e}");
        ExitCode::from(EXIT_ERROR)
    })
}

/// Build the scripted M0 match: human slot driven by
/// `ai::selftest_human_commands`, AI slot by `ai::Scripted::selftest`.
fn scripted_sim(data: &Path, seed: u64) -> Result<Sim, rules::Error> {
    let rules = Rules::load(data)?;
    let setup = MatchSetup::skirmish(&rules, seed);
    Ok(Sim::new(setup, rules, Box::new(ai::Scripted::selftest())))
}

fn run_scripted(data: &Path, ticks: u32, seed: u64) -> Result<u64, rules::Error> {
    let mut sim = scripted_sim(data, seed)?;
    for t in 0..ticks {
        sim.step(&ai::selftest_human_commands(t));
    }
    Ok(sim.hash())
}

fn selftest(data: &Path, ticks: u32, seed: u64) -> ExitCode {
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| run_scripted(data, ticks, seed));
        let b = s.spawn(|| run_scripted(data, ticks, seed));
        (a.join().expect("thread a"), b.join().expect("thread b"))
    });
    let (a, b) = match (a, b) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };
    println!("selftest ticks={ticks} seed={seed}");
    println!("thread A hash=0x{a:016x}");
    println!("thread B hash=0x{b:016x}");
    if a == b {
        println!("OK");
        ExitCode::SUCCESS
    } else {
        println!("MISMATCH");
        ExitCode::from(1)
    }
}

fn data_check(dir: &Path) -> ExitCode {
    // UI strings are presentation data outside `Rules` (and `rules_hash`),
    // checked here so a broken string file fails CI like any rules file.
    match Rules::load(dir).and_then(|r| Ok((r, rules::strings::Strings::load(dir, "en")?))) {
        Ok((r, _strings)) => {
            println!(
                "OK rules_version={} rules_hash=0x{:016x} resources={} units={} buildings={} ages={} techs={} side_techs={} factions={} maps={} strings=en",
                r.rules_version,
                r.rules_hash(),
                r.resources.resources.len(),
                r.units.len(),
                r.buildings.len(),
                r.ages.len(),
                r.counting_techs(),
                r.techs.len() - r.counting_techs(),
                r.factions.len(),
                r.maps.keys().cloned().collect::<Vec<_>>().join(",")
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn hash_dump(data: &Path, ticks: u32, seed: u64) -> ExitCode {
    let mut sim = match scripted_sim(data, seed) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };
    println!("tick hash");
    for t in 0..ticks {
        sim.step(&ai::selftest_human_commands(t));
        println!("{} 0x{:016x}", sim.tick(), sim.hash());
    }
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------
// M1: verify, hash-dump <replay>, record, bench, fuzz
// ---------------------------------------------------------------------------

/// The controller that drives AI slots during a re-simulation, plus the
/// warnings a re-simulation prints on stderr before its result. M1 replays
/// are scenario recordings with two human slots; a header with an AI slot is
/// driven by `ai::Passive` with a warning until the header names a bot (M5b).
/// A file without the clean-exit trailer (a recording killed mid-match) is
/// re-simulated up to its last complete record with a warning; the OK line
/// itself is unchanged so CI's `^OK` grep and hash-parity diff still apply.
fn replay_controller(replay: &Path) -> Box<dyn sim::AiController> {
    if let Ok(ReplayFile {
        setup,
        truncated,
        undecoded_bytes,
        records,
    }) = ReplayReader::open(replay)
    {
        if setup.players.iter().any(|p| p.is_ai) {
            eprintln!(
                "warning: {} has AI slots; they are driven by ai::Passive (bot ids in the header arrive in M5b)",
                replay.display()
            );
        }
        if truncated {
            eprintln!(
                "warning: {} has no clean-exit trailer (recording was cut); checking {} complete records, {undecoded_bytes} trailing bytes dropped",
                replay.display(),
                records.len()
            );
        }
    }
    Box::new(ai::Passive)
}

fn verify(data: &Path, replay: &Path) -> ExitCode {
    match sim::verify(replay, data, replay_controller(replay)) {
        Ok(outcome) => {
            println!("{outcome}");
            ExitCode::from(outcome.exit_code())
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

/// One `hash-dump` line: whole-state hash then every sub-hash in
/// `SubHashes::NAMES` order.
fn hash_dump_line(tick: u32, hash: u64, sub: &SubHashes) -> String {
    let mut line = format!("tick={tick} hash=0x{hash:016x}");
    for (name, value) in SubHashes::NAMES.iter().zip(sub.values()) {
        line.push_str(&format!(" {name}=0x{value:016x}"));
    }
    line
}

fn hash_dump_replay(data: &Path, replay: &Path, every: u32) -> ExitCode {
    use std::io::Write;
    let ai = replay_controller(replay);
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    let result = sim::replay::hash_dump(replay, data, ai, every, |tick, hash, sub| {
        // `hash-dump ... | head` closes the pipe early; that is not an error.
        if writeln!(out, "{}", hash_dump_line(tick, hash, sub)).is_err() {
            std::process::exit(0);
        }
    });
    if out.flush().is_err() {
        return ExitCode::SUCCESS;
    }
    drop(out);
    match result {
        Ok(VerifyOutcome::Ok { .. }) => ExitCode::SUCCESS,
        Ok(outcome) => {
            eprintln!("{outcome}");
            ExitCode::from(outcome.exit_code())
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

fn record(
    data: &Path,
    name: &str,
    ticks: Option<u32>,
    out: &Path,
    hash_every: Option<u32>,
    seed: Option<u64>,
    slow_ms: u64,
) -> ExitCode {
    let Some(scenario) = Scenario::parse(name) else {
        eprintln!(
            "record: unknown scenario {name:?}; one of {}",
            Scenario::names().join(", ")
        );
        return ExitCode::from(EXIT_ERROR);
    };
    let rules = match load_rules(data) {
        Ok(r) => r,
        Err(code) => return code,
    };
    let ticks = ticks.unwrap_or_else(|| scenario.default_ticks(&rules));
    let hash_every = hash_every.unwrap_or_else(|| scenario.default_hash_every());
    if hash_every == 0 {
        eprintln!("record: --hash-every must be at least 1");
        return ExitCode::from(EXIT_ERROR);
    }
    match record_scenario(&rules, scenario, ticks, out, hash_every, seed, slow_ms) {
        Ok(rec) if scenario.stops_at_outcome() && rec.result.is_none() => {
            eprintln!(
                "error: {name} decided no outcome in {ticks} ticks; {} is incomplete",
                out.display()
            );
            ExitCode::from(EXIT_ERROR)
        }
        Ok(rec) => {
            println!(
                "recorded {} scenario={name} ticks={} records={} final_hash=0x{:016x}{}",
                out.display(),
                rec.ticks,
                rec.records,
                rec.final_hash,
                outcome_summary(rec.result)
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

/// The `record` summary's outcome fields: ` outcome=<victory player=N
/// decisive=B|draw> reason=<reason> outcome_tick=T`, or nothing while the
/// match is undecided.
fn outcome_summary(result: Option<MatchResult>) -> String {
    let Some(r) = result else {
        return String::new();
    };
    let outcome = match r.outcome {
        Outcome::Victory { player, decisive } => {
            format!("victory player={} decisive={decisive}", player.0)
        }
        Outcome::Draw => "draw".to_string(),
    };
    let reason = match r.reason {
        OutcomeReason::SeatCaptured => "seat_captured",
        OutcomeReason::AllTownsLost => "all_towns_lost",
        OutcomeReason::Surrender => "surrender",
        OutcomeReason::Territory => "territory",
    };
    format!(" outcome={outcome} reason={reason} outcome_tick={}", r.tick)
}

/// What [`record_scenario`] wrote.
struct Recorded {
    /// The final state hash.
    final_hash: u64,
    /// Ticks simulated (fewer than asked when the scenario stops at its
    /// outcome).
    ticks: u32,
    /// Records written.
    records: u64,
    /// The match result at the end, if decided.
    result: Option<MatchResult>,
}

/// Run `scenario` for `ticks` ticks writing `out`: a `Tick` record for every
/// tick with commands, a `Hash` record every `hash_every` ticks and at the
/// end, a `flush` every 20 ticks (no fsync) and `finish` at clean exit, so a
/// `kill -9` leaves a file that verifies up to its last complete record.
/// For `snapshot_restore` the recording sim is snapshotted before the step
/// at its snapshot tick, a fresh sim is restored from the bytes and the
/// recording continues on that sim, so `verify` (which re-simulates without
/// interruption) passing on the fixture proves the restore was exact. A
/// scenario that stops at its outcome (`capture_seat`) ends right after
/// the step that decides the match, with the final hash at that tick.
fn record_scenario(
    rules: &Rules,
    scenario: Scenario,
    ticks: u32,
    out: &Path,
    hash_every: u32,
    seed: Option<u64>,
    slow_ms: u64,
) -> Result<Recorded, sim::ReplayError> {
    let (setup, stream) = scenario.setup_and_stream(rules, seed, ticks);
    let mut sim = scenarios::build(&setup, rules);
    let mut writer = ReplayWriter::create(out, sim.setup())?;
    let mut last_hashed = 0;
    for t in 0..ticks {
        if scenario.snapshot_tick() == Some(t) {
            let snap = sim.snapshot();
            let mut restored = scenarios::build(&setup, rules);
            restored
                .restore(&snap)
                .expect("a snapshot of the same setup restores");
            assert_eq!(
                restored.hash(),
                sim.hash(),
                "restore must reproduce the hash before tick {t}"
            );
            sim = restored;
            eprintln!(
                "snapshot/restore swap before tick {t}: hash 0x{:016x}",
                sim.hash()
            );
        }
        let cmds = sim::scenarios::commands_at(&stream, t);
        writer.tick(t, cmds)?;
        sim.step(cmds);
        if sim.tick().is_multiple_of(hash_every) {
            writer.hash(sim.tick(), sim.hash(), &sim.sub_hashes())?;
            last_hashed = sim.tick();
        }
        if sim.tick().is_multiple_of(DEFAULT_HASH_EVERY) {
            writer.flush()?;
        }
        if slow_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(slow_ms));
        }
        if scenario.stops_at_outcome() && sim.result().is_some() {
            break;
        }
    }
    if last_hashed != sim.tick() {
        writer.hash(sim.tick(), sim.hash(), &sim.sub_hashes())?;
    }
    let records = writer.records();
    writer.finish()?;
    Ok(Recorded {
        final_hash: sim.hash(),
        ticks: sim.tick(),
        records,
        result: sim.result(),
    })
}

/// Mean, 95th percentile and maximum of `samples` (milliseconds).
fn stats_ms(samples: &[f64]) -> (f64, f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let rank = ((sorted.len() as f64 * 0.95).ceil() as usize).clamp(1, sorted.len());
    (mean, sorted[rank - 1], sorted[sorted.len() - 1])
}

/// Squared distance, in `dist_sq_i64` scale, below which a unit counts as
/// arrived: 3 tiles.
const ARRIVED_DIST_SQ: i64 = (3 * 3) << 32;

/// The arrival gate (docs/design/pathing.md): this percentage of the units
/// within 3 tiles of their goal.
const ARRIVAL_GATE_PCT: f64 = 99.0;

/// `--arrival-curve` prints a line every this many ticks.
const ARRIVAL_CURVE_EVERY: u32 = 100;

/// Units within 3 tiles of their goal, over the goals captured so far.
fn arrived_count(view: &SimView<'_>, goals: &BTreeMap<UnitId, FxVec2>) -> usize {
    goals
        .iter()
        .filter(|(id, goal)| {
            view.unit(**id)
                .is_some_and(|u| u.pos.dist_sq_i64(**goal) <= ARRIVED_DIST_SQ)
        })
        .count()
}

/// `arrived` as a percentage of `total` (0 when nothing has a goal yet).
fn arrived_pct(arrived: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * arrived as f64 / total as f64
    }
}

/// An `Fx` as a float for printing (this crate may use floats; the sim may not).
fn fx_f64(v: sim::Fx) -> f64 {
    v.to_bits() as f64 / f64::from(1u32 << 31) / 2.0
}

/// Where the units that are *not* within 3 tiles of their goal stand at the
/// end of `bench`, by order state.
#[derive(Debug, Default)]
struct ArrivalTally {
    /// Order completed (`UnitArrived`) but since pushed more than 3 tiles away.
    displaced: usize,
    /// Still following a path.
    moving: usize,
    /// Order held, path request outstanding (queued or suspended).
    waiting_path: usize,
    /// Order dropped as unreachable (`PathUnreachable`), or never ordered.
    no_order: usize,
}

#[allow(clippy::too_many_arguments)]
fn bench(
    data: &Path,
    units: u16,
    ticks: u32,
    astar: bool,
    budget: Option<u32>,
    seed: u64,
    verbose: bool,
    arrival_curve: bool,
) -> ExitCode {
    let rules = match load_rules(data) {
        Ok(r) => r,
        Err(code) => return code,
    };
    if astar {
        return bench_astar(&rules, budget.unwrap_or(rules.path_budget_expansions));
    }
    let (setup, stream) = Scenario::Crossing { units }.setup_and_stream(&rules, Some(seed), ticks);
    let mut sim = scenarios::build(&setup, &rules);
    // Each unit's goal, captured while its order is live (arrival clears it).
    // `order.goal` is the unit's own spiral-offset tile centre from
    // `movement::group_targets`, already corrected to a passable tile of the
    // click's component; the path's last tile is that same tile for every
    // unit that starts in the click's component (all of them here).
    let mut goals: BTreeMap<UnitId, FxVec2> = BTreeMap::new();
    let mut arrived_events: BTreeSet<UnitId> = BTreeSet::new();
    let mut unreachable: BTreeSet<UnitId> = BTreeSet::new();
    let mut step_ms = Vec::with_capacity(ticks as usize);
    // First tick (after its step) at which the gate held, and the first tick
    // from which it held through every later tick (a shove can break it
    // again). `None` when it never held.
    let mut gate_first: Option<u32> = None;
    let mut gate_since: Option<u32> = None;
    let mut curve: Vec<(u32, f64)> = Vec::new();
    println!("bench units={units} ticks={ticks} seed={seed}");
    for t in 0..ticks {
        let cmds = sim::scenarios::commands_at(&stream, t);
        let start = Instant::now();
        sim.step(cmds);
        step_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        for u in sim.view().units() {
            if let Some(order) = &u.order {
                goals.entry(u.id).or_insert(order.goal);
            }
        }
        for e in sim.drain_events() {
            match e {
                SimEvent::UnitArrived { unit } => {
                    arrived_events.insert(unit);
                }
                SimEvent::PathUnreachable { unit } => {
                    unreachable.insert(unit);
                }
                _ => {}
            }
        }
        // Outside the timed step: the gate check every tick is a few hundred
        // squared distances.
        let tick = sim.tick();
        let pct = arrived_pct(arrived_count(&sim.view(), &goals), goals.len());
        if pct >= ARRIVAL_GATE_PCT {
            gate_first.get_or_insert(tick);
            gate_since.get_or_insert(tick);
        } else {
            gate_since = None;
        }
        if arrival_curve && tick.is_multiple_of(ARRIVAL_CURVE_EVERY) {
            curve.push((tick, pct));
        }
    }
    let (mean, p95, max) = stats_ms(&step_ms);
    let view = sim.view();
    let mut tally = ArrivalTally::default();
    let mut arrived = 0usize;
    let mut near_but_moving = 0usize;
    let mut stragglers = Vec::new();
    for (id, goal) in &goals {
        let Some(u) = view.unit(*id) else {
            continue;
        };
        // The acceptance gate: within 3 tiles of the goal, whatever the
        // order state (a unit on its last half tile counts).
        if u.pos.dist_sq_i64(*goal) <= ARRIVED_DIST_SQ {
            arrived += 1;
            if u.order.is_some() {
                near_but_moving += 1;
            }
            continue;
        }
        let state = match &u.order {
            None if arrived_events.contains(id) => {
                tally.displaced += 1;
                if u.post.is_some_and(|p| p.returning) {
                    "arrived, displaced, returning to post"
                } else {
                    "arrived, then displaced"
                }
            }
            None => {
                tally.no_order += 1;
                if unreachable.contains(id) {
                    "unreachable"
                } else {
                    "no order"
                }
            }
            Some(order) if order.path.is_empty() => {
                tally.waiting_path += 1;
                "waiting for path"
            }
            Some(_) => {
                tally.moving += 1;
                "moving"
            }
        };
        stragglers.push((*id, u.pos, *goal, state));
    }
    let pct = arrived_pct(arrived, goals.len());
    println!("step_ms mean={mean:.3} p95={p95:.3} max={max:.3}");
    println!(
        "arrived {pct:.1}% ({arrived}/{}) within 3 tiles of the goal at tick {ticks}",
        goals.len()
    );
    println!(
        "not within 3 tiles: moving={} waiting_path={} displaced={} no_order={}; arrived events={} unreachable={} near_but_still_moving={near_but_moving}",
        tally.moving,
        tally.waiting_path,
        tally.displaced,
        tally.no_order,
        arrived_events.len(),
        unreachable.len()
    );
    match (gate_first, gate_since) {
        (Some(first), Some(since)) => println!(
            "gate >= {ARRIVAL_GATE_PCT}%: first held after tick {first}, held from tick {since} to the end"
        ),
        (Some(first), None) => println!(
            "gate >= {ARRIVAL_GATE_PCT}%: first held after tick {first}, not holding at tick {ticks}"
        ),
        _ => println!("gate >= {ARRIVAL_GATE_PCT}%: not reached by tick {ticks}"),
    }
    if arrival_curve {
        for (tick, pct) in &curve {
            println!("  tick {tick:>5} arrived {pct:5.1}%");
        }
    }
    if verbose {
        for (id, pos, goal, state) in &stragglers {
            println!(
                "  unit {} at ({:.2}, {:.2}) goal ({:.1}, {:.1}) {state}",
                id.0,
                fx_f64(pos.x),
                fx_f64(pos.y),
                fx_f64(goal.x),
                fx_f64(goal.y)
            );
        }
    }
    println!("final_hash=0x{:016x}", sim.hash());
    ExitCode::SUCCESS
}

/// Iterations of the saturated search in `bench --astar`.
const ASTAR_ITERATIONS: u32 = 100;

/// Time one `resume` with the full `budget` on a search that cannot finish
/// inside it: from the west start towards a river tile, which is blocked, so
/// the open set never reaches the goal and every expansion is spent.
fn bench_astar(rules: &Rules, budget: u32) -> ExitCode {
    let def = rules
        .map(&rules.default_map)
        .expect("default map is validated by Rules::load");
    let map = Map::from_def(def);
    let from = map.tile_of(sim::scenarios::WEST);
    let to = map.tile_of(scenarios::RIVER);
    let mut tick_ms = Vec::with_capacity(ASTAR_ITERATIONS as usize);
    let mut found = 0u32;
    let mut exhausted = 0u32;
    let mut suspended = 0u32;
    let mut used = 0u32;
    for _ in 0..ASTAR_ITERATIONS {
        let mut search = AStarSearch::new(&map, from, to);
        let mut remaining = budget;
        let start = Instant::now();
        let status = search.resume(&map, &mut remaining);
        tick_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        used = budget - remaining;
        match status {
            SearchStatus::Found(_) => found += 1,
            SearchStatus::Exhausted => exhausted += 1,
            SearchStatus::Suspended => suspended += 1,
        }
    }
    let (mean, p95, max) = stats_ms(&tick_ms);
    println!(
        "bench astar budget={budget} iterations={ASTAR_ITERATIONS} from={from:?} to={to:?} (blocked goal)"
    );
    println!("tick_ms mean={mean:.3} p95={p95:.3} max={max:.3}");
    println!("expansions_used={used} found={found} exhausted={exhausted} suspended={suspended}");
    ExitCode::SUCCESS
}

/// What `bench --territory` measured.
struct TerritoryBench {
    /// Field size.
    width: u16,
    /// Field size.
    height: u16,
    /// Milliseconds per timed edit.
    edit_ms: Vec<f64>,
    /// Milliseconds for one full recompute of the final source set.
    full_ms: f64,
    /// The incrementally maintained field equals the full recompute.
    matches: bool,
}

/// The territory workload, deterministic for a seed: `sources` border
/// sources on the default map's field (owners alternate between the two
/// players, every eighth on the neutral layer, centres uniform over the
/// map), then `edits` timed edits. An edit is a tier change (40%), a move
/// by up to 8 tiles (20%), an owner change including to and from the
/// neutral layer (20%), or a removal plus an add under a new id (20%).
/// Radii are the Town tiers from `rules.ron`, each plus 0 to 3 for
/// Statecraft (M4a).
fn run_territory_bench(rules: &Rules, sources: u32, edits: u32, seed: u64) -> TerritoryBench {
    use rand_core::{Rng, SeedableRng};
    let def = rules
        .map(&rules.default_map)
        .expect("default map is validated by Rules::load");
    let (w, h) = (def.width, def.height);
    let tier = |grown: bool, statecraft: u32| -> u16 {
        let r = if grown {
            rules.town_radius_grown_tiles
        } else {
            rules.town_radius_tiles
        };
        u16::try_from(r + statecraft).expect("radius fits u16")
    };
    let mut rng = rand_pcg::Pcg32::seed_from_u64(seed);
    let mut below = |n: u32| rng.next_u32() % n;
    let mut field = TerritoryField::new(w, h, 2);
    let mut next = 1u32;
    for i in 0..sources {
        let owner = if i % 8 == 7 {
            SourceOwner::Neutral
        } else {
            SourceOwner::Player(PlayerId(u8::try_from(i % 2).expect("0 or 1")))
        };
        let centre = Tile::new(
            u16::try_from(below(u32::from(w))).expect("< width"),
            u16::try_from(below(u32::from(h))).expect("< height"),
        );
        let radius = tier(below(2) == 1, below(4));
        field.add_source(
            BuildingId(next),
            Source {
                owner,
                centre,
                radius,
            },
        );
        next += 1;
    }
    let mut edit_ms = Vec::with_capacity(edits as usize);
    for _ in 0..edits {
        let live: Vec<(BuildingId, Source)> =
            field.sources().iter().map(|(&id, &s)| (id, s)).collect();
        let (id, old) = live[below(u32::try_from(live.len()).expect("few sources")) as usize];
        let kind = below(10);
        let shift = |c: u16, size: u16, d: u32| -> u16 {
            let v = i32::from(c) + i32::try_from(d).expect("small") - 8;
            u16::try_from(v.clamp(0, i32::from(size) - 1)).expect("clamped")
        };
        let new = match kind {
            0..=3 => Source {
                radius: tier(below(2) == 1, below(4)),
                ..old
            },
            4..=5 => Source {
                centre: Tile::new(
                    shift(old.centre.x, w, below(17)),
                    shift(old.centre.y, h, below(17)),
                ),
                ..old
            },
            6..=7 => Source {
                owner: match below(3) {
                    2 => SourceOwner::Neutral,
                    p => SourceOwner::Player(PlayerId(u8::try_from(p).expect("0 or 1"))),
                },
                ..old
            },
            _ => Source {
                centre: Tile::new(
                    u16::try_from(below(u32::from(w))).expect("< width"),
                    u16::try_from(below(u32::from(h))).expect("< height"),
                ),
                ..old
            },
        };
        let start = Instant::now();
        if kind >= 8 {
            field.remove_source(id);
            field.add_source(BuildingId(next), new);
        } else {
            field.change_source(id, new);
        }
        edit_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        if kind >= 8 {
            next += 1;
        }
    }
    let mut full = field.clone();
    let start = Instant::now();
    full.full_recompute();
    let full_ms = start.elapsed().as_secs_f64() * 1000.0;
    TerritoryBench {
        width: w,
        height: h,
        edit_ms,
        full_ms,
        matches: full == field,
    }
}

/// `bench --territory`: time incremental recompute (docs/design/territory.md,
/// budget: mean < 2 ms for 16 sources and 100 edits). Exits 1 when the
/// incrementally maintained field differs from a full recompute.
fn bench_territory(data: &Path, sources: u32, edits: u32, seed: u64) -> ExitCode {
    let rules = match load_rules(data) {
        Ok(r) => r,
        Err(code) => return code,
    };
    if sources == 0 {
        eprintln!("error: --sources must be at least 1");
        return ExitCode::from(2);
    }
    let b = run_territory_bench(&rules, sources, edits, seed);
    let (mean, p95, max) = stats_ms(&b.edit_ms);
    println!(
        "bench territory sources={sources} edits={edits} seed={seed} map={}x{}",
        b.width, b.height
    );
    println!("recompute_ms mean={mean:.4} p95={p95:.4} max={max:.4}");
    println!(
        "full_recompute_ms={:.4} incremental_matches_full={}",
        b.full_ms, b.matches
    );
    if b.matches {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// `bench --combat` exits 1 when fewer ticks than this were fighting ticks:
/// the battle was too short to measure.
const COMBAT_MIN_FIGHTING_TICKS: usize = 100;

/// What `bench --combat` measured.
struct CombatBench {
    /// Units spawned in the armies: `--units` rounded down to a multiple of
    /// 8 (the `--harrying` Wains not included).
    units: u16,
    /// Milliseconds per step, every tick.
    step_ms: Vec<f64>,
    /// Milliseconds per step over the fighting ticks: ticks with at least
    /// one `Hit` after whose step both sides still have a quarter of their
    /// armed units (50 of 200 at the acceptance size, 400 units).
    fighting_ms: Vec<f64>,
    /// `Hit` events.
    hits: u64,
    /// `UnitDied` events per owner (a `--harrying` Wain's death included).
    deaths: [u32; 2],
    /// `BuildingDestroyed` events.
    destroyed: u32,
    /// `--harrying` only: what attrition did (`None` without the flag).
    attrition: Option<AttritionCounts>,
    /// Hash after the last tick.
    final_hash: u64,
}

/// What attrition did in a `bench --combat --harrying` run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct AttritionCounts {
    /// `Attrition` events: pulses that took hit points.
    pulses: u64,
    /// Unit-ticks, counted after each step, of a unit that stood where
    /// attrition would bite (not immune, on another player's land whose
    /// Harrying level is at least 1) but that one of its own Supply Wains
    /// covered: the attrition that supply prevented.
    supplied: u64,
    /// Unit-ticks, counted after each step, of a unit whose exposure
    /// counter was running (`exposed_ticks > 0`): bleeding, unsupplied.
    exposed: u64,
}

/// One step's share of [`AttritionCounts::supplied`] and
/// [`AttritionCounts::exposed`], read from the view after the step.
fn attrition_unit_ticks(view: &SimView<'_>, rules: &Rules) -> (u64, u64) {
    let players = [PlayerId(0), PlayerId(1)];
    let harrying = players.map(|p| view.harrying_level(p) > 0);
    let wains = players.map(|p| view.supply_wains(p));
    let r2 = i64::from(view.supply_radius_tiles()).pow(2);
    let (mut supplied, mut exposed) = (0, 0);
    for u in view.units() {
        if u.exposed_ticks > 0 {
            exposed += 1;
        }
        if rules
            .unit_kind(u.kind.0)
            .is_none_or(|k| k.immune_to_attrition)
        {
            continue;
        }
        let tile = view.map().tile_of(u.pos);
        let Some(owner) = view.territory().owner_at(tile) else {
            continue;
        };
        if owner == u.owner || !harrying.get(usize::from(owner.0)).copied().unwrap_or(false) {
            continue;
        }
        let covered = wains.get(usize::from(u.owner.0)).is_some_and(|ws| {
            ws.iter().any(|w| {
                let dx = i64::from(tile.x) - i64::from(w.x);
                let dy = i64::from(tile.y) - i64::from(w.y);
                dx * dx + dy * dy <= r2
            })
        });
        if covered {
            supplied += 1;
        }
    }
    (supplied, exposed)
}

/// Run [`sim::scenarios::combat_bench`] (or, with `harrying`,
/// [`sim::scenarios::combat_bench_harrying`]) for `ticks` ticks, timing each
/// `step` alone (events, unit counts and attrition counts are read outside
/// the timed region).
fn run_combat_bench(
    rules: &Rules,
    units: u16,
    ticks: u32,
    seed: u64,
    harrying: bool,
) -> CombatBench {
    let (setup, stream) = if harrying {
        sim::scenarios::combat_bench_harrying(rules, units, seed)
    } else {
        sim::scenarios::combat_bench(rules, units, seed)
    };
    let mut sim = scenarios::build(&setup, rules);
    let per_side_quarter = usize::from(units / 8);
    let mut b = CombatBench {
        units: units / 8 * 8,
        step_ms: Vec::with_capacity(ticks as usize),
        fighting_ms: Vec::new(),
        hits: 0,
        deaths: [0; 2],
        destroyed: 0,
        attrition: harrying.then(AttritionCounts::default),
        final_hash: 0,
    };
    for t in 0..ticks {
        let cmds = sim::scenarios::commands_at(&stream, t);
        let start = Instant::now();
        sim.step(cmds);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        b.step_ms.push(ms);
        let mut tick_hits = 0u64;
        for e in sim.drain_events() {
            match e {
                SimEvent::Hit { .. } => tick_hits += 1,
                SimEvent::UnitDied { owner, .. } => b.deaths[usize::from(owner.0)] += 1,
                SimEvent::BuildingDestroyed { .. } => b.destroyed += 1,
                SimEvent::Attrition { .. } => {
                    if let Some(a) = b.attrition.as_mut() {
                        a.pulses += 1;
                    }
                }
                _ => {}
            }
        }
        b.hits += tick_hits;
        let view = sim.view();
        let mut alive = [0usize; 2];
        for u in view.units() {
            if rules
                .unit_kind(u.kind.0)
                .is_some_and(|k| k.attack.is_some())
            {
                alive[usize::from(u.owner.0)] += 1;
            }
        }
        if tick_hits > 0 && alive.iter().all(|&n| n >= per_side_quarter) {
            b.fighting_ms.push(ms);
        }
        if let Some(a) = b.attrition.as_mut() {
            let (supplied, exposed) = attrition_unit_ticks(&view, rules);
            a.supplied += supplied;
            a.exposed += exposed;
        }
    }
    b.final_hash = sim.hash();
    b
}

/// The `bench --combat` report, one line per entry; `--harrying` adds
/// ` harrying` to the first line and an attrition line before the hash.
fn combat_bench_report(b: &CombatBench, ticks: u32, seed: u64) -> Vec<String> {
    let (mean, p95, max) = stats_ms(&b.step_ms);
    let (f_mean, f_p95, _) = stats_ms(&b.fighting_ms);
    let variant = if b.attrition.is_some() {
        " harrying"
    } else {
        ""
    };
    let mut lines = vec![
        format!(
            "bench combat units={} ticks={ticks} seed={seed}{variant}",
            b.units
        ),
        format!("step_ms mean={mean:.3} p95={p95:.3} max={max:.3}"),
        format!(
            "fighting_ticks={} fighting_mean_ms={f_mean:.3} fighting_p95_ms={f_p95:.3}",
            b.fighting_ms.len()
        ),
        format!(
            "hits={} deaths_p0={} deaths_p1={} destroyed={}",
            b.hits, b.deaths[0], b.deaths[1], b.destroyed
        ),
    ];
    if let Some(a) = b.attrition {
        lines.push(format!(
            "attrition_pulses={} supplied={} exposed={}",
            a.pulses, a.supplied, a.exposed
        ));
    }
    lines.push(format!("final_hash=0x{:016x}", b.final_hash));
    lines
}

/// `bench --combat`: step time while `units` units fight, half on each side
/// (ROADMAP M4a: mean step < 5 ms with 200 vs 200 fighting; the PR also
/// reads the fighting-tick mean against 5 ms and its p95 against 10 ms).
/// With `harrying` (M5a) the same battle runs with attrition on; the PR
/// reads its mean against the plain run's (it may add at most 0.1 ms).
/// Exits 1 only when fewer than [`COMBAT_MIN_FIGHTING_TICKS`] ticks were
/// fighting ticks, not on the timing budget, like the other benches.
fn bench_combat(data: &Path, units: u16, ticks: u32, seed: u64, harrying: bool) -> ExitCode {
    let rules = match load_rules(data) {
        Ok(r) => r,
        Err(code) => return code,
    };
    let b = run_combat_bench(&rules, units, ticks, seed, harrying);
    for line in combat_bench_report(&b, ticks, seed) {
        println!("{line}");
    }
    if b.fighting_ms.len() < COMBAT_MIN_FIGHTING_TICKS {
        eprintln!(
            "bench combat: {} fighting ticks, fewer than {COMBAT_MIN_FIGHTING_TICKS}: too short a battle to measure",
            b.fighting_ms.len()
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// What one fuzz case observed.
struct FuzzCase {
    commands: usize,
    rejected: usize,
}

/// Run one seeded random stream twice; the hashes must agree on every tick
/// and the rejected `(player, seq)` set must equal the generator's
/// expectation.
fn fuzz_case(rules: &Rules, seed: u64, ticks: u32, movement: bool) -> Result<FuzzCase, String> {
    let (stream, expected) = scenarios::random_commands(seed, ticks, movement);
    let setup = MatchSetup::scenario(rules, seed);
    let run = || {
        let mut sim = scenarios::build(&setup, rules);
        let mut hashes = Vec::with_capacity(stream.len());
        for cmds in &stream {
            sim.step(cmds);
            hashes.push(sim.hash());
        }
        // Let the last tick's commands pass through the delay queue.
        for _ in 0..=sim.setup().cmd_delay {
            sim.step(&[]);
        }
        let rejected: BTreeSet<(PlayerId, u32)> = sim
            .drain_events()
            .into_iter()
            .filter_map(|e| match e {
                SimEvent::CommandRejected { player, seq, .. } => Some((player, seq)),
                _ => None,
            })
            .collect();
        (hashes, rejected)
    };
    let (hashes_a, rejected_a) = run();
    let (hashes_b, rejected_b) = run();
    if hashes_a != hashes_b {
        let tick = hashes_a
            .iter()
            .zip(&hashes_b)
            .position(|(a, b)| a != b)
            .map_or(hashes_a.len(), |i| i + 1);
        return Err(format!("seed={seed}: two runs diverged at tick {tick}"));
    }
    if rejected_a != rejected_b {
        return Err(format!("seed={seed}: two runs rejected different commands"));
    }
    let expected: BTreeSet<(PlayerId, u32)> = expected.into_iter().collect();
    if rejected_a != expected {
        let missing: Vec<_> = expected.difference(&rejected_a).collect();
        let unexpected: Vec<_> = rejected_a.difference(&expected).collect();
        return Err(format!(
            "seed={seed}: rejection mismatch; not rejected={missing:?} unexpectedly rejected={unexpected:?}"
        ));
    }
    Ok(FuzzCase {
        commands: stream.iter().map(Vec::len).sum(),
        rejected: rejected_a.len(),
    })
}

fn fuzz(data: &Path, ticks: u32, seed: u64, cases: u32) -> ExitCode {
    let rules = match load_rules(data) {
        Ok(r) => r,
        Err(code) => return code,
    };
    let mut commands = 0;
    let mut rejected = 0;
    for case in 0..u64::from(cases) {
        match fuzz_case(&rules, seed + case, ticks, true) {
            Ok(c) => {
                commands += c.commands;
                rejected += c.rejected;
            }
            Err(msg) => {
                println!("fuzz FAILED {msg}");
                return ExitCode::from(1);
            }
        }
    }
    println!(
        "fuzz cases={cases} ticks={ticks} seed={seed} commands={commands} rejected={rejected} OK"
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eonmark-sim-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn scripted_run_is_repeatable() {
        let a = run_scripted(&data(), 200, 42).unwrap();
        let b = run_scripted(&data(), 200, 42).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, run_scripted(&data(), 200, 43).unwrap());
    }

    #[test]
    fn cli_parses_m1_subcommands() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["sim-cli", "verify", "m.eonreplay"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Verify { replay } if replay == Path::new("m.eonreplay")));
        assert!(Cli::try_parse_from(["sim-cli", "verify"]).is_err());
        let cli =
            Cli::try_parse_from(["sim-cli", "--data", "x", "hash-dump", "--ticks", "3"]).unwrap();
        assert_eq!(cli.data, PathBuf::from("x"));
        assert!(matches!(
            cli.cmd,
            Cmd::HashDump {
                replay: None,
                ticks: 3,
                every: 20,
                ..
            }
        ));
        let cli =
            Cli::try_parse_from(["sim-cli", "hash-dump", "m.eonreplay", "--every", "1"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::HashDump {
                replay: Some(_),
                every: 1,
                ..
            }
        ));
        let cli = Cli::try_parse_from([
            "sim-cli", "bench", "--units", "500", "--ticks", "1200", "--astar", "--budget", "4000",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Bench {
                units: 500,
                ticks: 1200,
                astar: true,
                budget: Some(4000),
                verbose: false,
                arrival_curve: false,
                ..
            }
        ));
        let cli = Cli::try_parse_from(["sim-cli", "bench", "--ticks", "2400", "--arrival-curve"])
            .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Bench {
                units: 500,
                ticks: 2400,
                astar: false,
                arrival_curve: true,
                ..
            }
        ));
        let cli = Cli::try_parse_from(["sim-cli", "fuzz", "--ticks", "10", "--seed", "7"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Fuzz {
                ticks: 10,
                seed: 7,
                ..
            }
        ));
        let cli = Cli::try_parse_from([
            "sim-cli",
            "record",
            "--scenario",
            "move_500",
            "--ticks",
            "1200",
            "--out",
            "x.eonreplay",
            "--hash-every-tick",
            "--slow-ms",
            "5",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Record {
                hash_every_tick: true,
                ticks: Some(1200),
                slow_ms: 5,
                seed: None,
                ..
            }
        ));
    }

    #[test]
    fn record_then_verify_selftest_scenario() {
        let rules = Rules::load(data()).unwrap();
        let out = temp("selftest.eonreplay");
        let Recorded {
            final_hash,
            records,
            ticks,
            result,
        } = record_scenario(
            &rules,
            Scenario::Selftest,
            90,
            &out,
            DEFAULT_HASH_EVERY,
            None,
            0,
        )
        .unwrap();
        assert_eq!((ticks, result), (90, None), "the selftest decides nothing");
        // Hash records at 20, 40, 60, 80 and the final one at 90, plus the
        // tick batches the selftest stream issues.
        assert!(records >= 5);
        let outcome = sim::verify(&out, &data(), Box::new(ai::Passive)).unwrap();
        assert_eq!(
            outcome,
            VerifyOutcome::Ok {
                final_hash,
                ticks: 90
            }
        );
        // --hash-every-tick: one hash record per tick, same final hash.
        let out2 = temp("selftest_every.eonreplay");
        let final_hash2 = record_scenario(&rules, Scenario::Selftest, 90, &out2, 1, None, 0)
            .unwrap()
            .final_hash;
        assert_eq!(final_hash2, final_hash);
        let recs = ReplayReader::open(&out2).unwrap().records;
        let hashes = recs
            .iter()
            .filter(|r| matches!(r, sim::replay::Record::Hash(_)))
            .count();
        assert_eq!(hashes, 90);
        // Nothing in the stream draws from the rng: its sub-hash never moves.
        let rng: std::collections::BTreeSet<u64> = recs
            .iter()
            .filter_map(|r| match r {
                sim::replay::Record::Hash(h) => Some(h.sub.rng),
                _ => None,
            })
            .collect();
        assert_eq!(rng.len(), 1, "the selftest stream draws nothing");
        // A different seed changes the final hash.
        let out3 = temp("selftest_seed2.eonreplay");
        let final_hash3 = record_scenario(
            &rules,
            Scenario::Selftest,
            90,
            &out3,
            DEFAULT_HASH_EVERY,
            Some(2),
            0,
        )
        .unwrap()
        .final_hash;
        assert_ne!(final_hash3, final_hash);
        // --hash-every 30: records at 30, 60 and 90 only, same final hash.
        let out4 = temp("selftest_30.eonreplay");
        let rec = record_scenario(&rules, Scenario::Selftest, 90, &out4, 30, None, 0).unwrap();
        assert_eq!(rec.final_hash, final_hash);
        let ticks: Vec<u32> = ReplayReader::open(&out4)
            .unwrap()
            .records
            .iter()
            .filter_map(|r| match r {
                sim::replay::Record::Hash(h) => Some(h.tick),
                _ => None,
            })
            .collect();
        assert_eq!(ticks, [30, 60, 90]);
    }

    /// M5a: `--hash-every N` parses, conflicts with `--hash-every-tick`,
    /// and the scenarios' own cadence is 1000 for `stalemate_45min` and the
    /// replay default otherwise; only `capture_seat` stops at its outcome.
    #[test]
    fn record_hash_every_flag_and_scenario_defaults() {
        let cli = Cli::try_parse_from([
            "sim-cli",
            "record",
            "--scenario",
            "stalemate_45min",
            "--out",
            "x.eonreplay",
            "--hash-every",
            "500",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Record {
                hash_every: Some(500),
                hash_every_tick: false,
                ..
            }
        ));
        assert!(
            Cli::try_parse_from([
                "sim-cli",
                "record",
                "--scenario",
                "move_500",
                "--out",
                "x.eonreplay",
                "--hash-every",
                "5",
                "--hash-every-tick",
            ])
            .is_err()
        );
        let stalemate = Scenario::parse("stalemate_45min").unwrap();
        assert_eq!(stalemate.default_hash_every(), 1000);
        assert!(!stalemate.stops_at_outcome());
        let capture = Scenario::parse("capture_seat").unwrap();
        assert_eq!(capture.default_hash_every(), DEFAULT_HASH_EVERY);
        assert!(capture.stops_at_outcome());
        assert_eq!(
            Scenario::parse("move_500").unwrap().default_hash_every(),
            DEFAULT_HASH_EVERY
        );
        let rules = Rules::load(data()).unwrap();
        assert_eq!(stalemate.default_ticks(&rules), rules.match_cap_ticks());
    }

    /// `capture_seat` records up to the step that decides the match and
    /// no further, and its replay verifies to the recorder's final hash at
    /// that tick; the summary names the outcome.
    #[test]
    fn capture_seat_recording_stops_at_its_outcome() {
        let rules = Rules::load(data()).unwrap();
        let scenario = Scenario::parse("capture_seat").unwrap();
        let cap = scenario.default_ticks(&rules);
        let out = temp("capture_seat.eonreplay");
        let rec = record_scenario(&rules, scenario, cap, &out, 20, None, 0).unwrap();
        let result = rec.result.expect("the Seat falls before the cap");
        assert_eq!(rec.ticks, result.tick);
        assert!(rec.ticks < cap, "{} < {cap}", rec.ticks);
        assert_eq!(
            result.outcome,
            Outcome::Victory {
                player: PlayerId(0),
                decisive: true
            }
        );
        let summary = outcome_summary(rec.result);
        assert_eq!(
            summary,
            format!(
                " outcome=victory player=0 decisive=true reason=seat_captured outcome_tick={}",
                rec.ticks
            )
        );
        assert_eq!(outcome_summary(None), "");
        let outcome = sim::verify(&out, &data(), Box::new(ai::Passive)).unwrap();
        assert_eq!(
            outcome,
            VerifyOutcome::Ok {
                final_hash: rec.final_hash,
                ticks: rec.ticks
            }
        );
    }

    #[test]
    fn hash_dump_line_has_every_subsystem() {
        let line = hash_dump_line(7, 1, &SubHashes::default());
        assert!(line.starts_with("tick=7 hash=0x0000000000000001 units=0x"));
        for name in SubHashes::NAMES {
            assert!(line.contains(&format!(" {name}=0x")), "{line}");
        }
    }

    #[test]
    fn fuzz_case_without_movement_matches_expected_rejections() {
        let rules = Rules::load(data()).unwrap();
        for seed in 1..=4 {
            let case = fuzz_case(&rules, seed, 150, false).unwrap();
            assert!(case.commands > 0);
            assert!(
                case.rejected > 0,
                "seed {seed} produced no invalid commands"
            );
        }
    }

    #[test]
    fn arrived_pct_guards_empty_goals() {
        assert_eq!(arrived_pct(0, 0), 0.0);
        assert!((arrived_pct(495, 500) - 99.0).abs() < 1e-9);
        assert!(arrived_pct(494, 500) < ARRIVAL_GATE_PCT);
    }

    #[test]
    fn cli_parses_bench_territory_flags() {
        let cli = Cli::try_parse_from([
            "sim-cli",
            "bench",
            "--territory",
            "--sources",
            "16",
            "--edits",
            "100",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Bench {
                territory: true,
                astar: false,
                sources: 16,
                edits: 100,
                ..
            }
        ));
        let cli = Cli::try_parse_from(["sim-cli", "bench", "--territory"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Bench {
                territory: true,
                sources: 16,
                edits: 100,
                seed: 1,
                ..
            }
        ));
        assert!(Cli::try_parse_from(["sim-cli", "bench", "--territory", "--astar"]).is_err());
    }

    #[test]
    fn bench_territory_small_run_matches_full() {
        let rules = load_rules(&data()).unwrap();
        let b = run_territory_bench(&rules, 6, 40, 3);
        assert_eq!(b.edit_ms.len(), 40);
        assert!(b.matches);
        assert_eq!((b.width, b.height), (128, 128));
    }

    #[test]
    fn cli_parses_bench_combat_flags() {
        let cli = Cli::try_parse_from([
            "sim-cli", "bench", "--units", "400", "--ticks", "1200", "--combat",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Bench {
                combat: true,
                astar: false,
                territory: false,
                units: 400,
                ticks: 1200,
                seed: 1,
                ..
            }
        ));
        for other in ["--astar", "--territory"] {
            assert!(Cli::try_parse_from(["sim-cli", "bench", "--combat", other]).is_err());
        }
        let cli = Cli::try_parse_from([
            "sim-cli",
            "bench",
            "--units",
            "400",
            "--ticks",
            "1200",
            "--combat",
            "--harrying",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Bench {
                combat: true,
                harrying: true,
                units: 400,
                ticks: 1200,
                ..
            }
        ));
        // --harrying is a variant of the battle only.
        assert!(Cli::try_parse_from(["sim-cli", "bench", "--harrying"]).is_err());
    }

    /// The Harrying bench at a tenth of its size: P0's army is supplied
    /// while its Wain lives, then bleeds, and the report adds its line.
    #[test]
    fn bench_combat_harrying_small_run_bleeds_and_supplies() {
        let rules = load_rules(&data()).unwrap();
        let b = run_combat_bench(&rules, 40, 400, 1, true);
        assert_eq!(b.units, 40, "the Wains are not counted");
        let a = b.attrition.expect("--harrying counts attrition");
        assert!(a.pulses > 0 && a.supplied > 0 && a.exposed > 0, "{a:?}");
        assert!(!b.fighting_ms.is_empty());
        assert_eq!(b.destroyed, 0, "the Town never fights");
        assert_eq!(
            run_combat_bench(&rules, 40, 400, 1, true).final_hash,
            b.final_hash
        );
        assert_ne!(
            run_combat_bench(&rules, 40, 400, 1, false).final_hash,
            b.final_hash
        );
        let report = combat_bench_report(&b, 400, 1);
        let prefixes = [
            "bench combat units=40 ticks=400 seed=1 harrying",
            "step_ms mean=",
            "fighting_ticks=",
            "hits=",
            "attrition_pulses=",
            "final_hash=0x",
        ];
        assert_eq!(report.len(), prefixes.len());
        for (line, prefix) in report.iter().zip(prefixes) {
            assert!(line.starts_with(prefix), "{line}");
        }
        assert_eq!(
            report[4],
            format!(
                "attrition_pulses={} supplied={} exposed={}",
                a.pulses, a.supplied, a.exposed
            )
        );
    }

    /// The bench's own supplied and exposed counts agree, unit by unit and
    /// tick by tick, with the sim's view: a unit counts as supplied exactly
    /// when the view says one of its Wains covers its tile on land where
    /// attrition would otherwise bite (and then `SimView::attrition` says it
    /// is not exposed).
    #[test]
    fn bench_attrition_counts_agree_with_the_view() {
        let rules = load_rules(&data()).unwrap();
        let (setup, stream) = sim::scenarios::combat_bench_harrying(&rules, 40, 1);
        let mut sim = scenarios::build(&setup, &rules);
        let (mut total_supplied, mut total_exposed) = (0, 0);
        for t in 0..400 {
            sim.step(sim::scenarios::commands_at(&stream, t));
            let view = sim.view();
            let (mut supplied, mut exposed) = (0, 0);
            for u in view.units() {
                if u.exposed_ticks > 0 {
                    exposed += 1;
                }
                let tile = view.map().tile_of(u.pos);
                let bites = !rules.unit_kind(u.kind.0).unwrap().immune_to_attrition
                    && view
                        .territory()
                        .owner_at(tile)
                        .is_some_and(|o| o != u.owner && view.harrying_level(o) > 0);
                let covered = view.supplied(u.owner, tile);
                if bites && covered {
                    supplied += 1;
                }
                if bites {
                    assert_eq!(view.attrition(u.id).is_none(), covered, "{:?} at {t}", u.id);
                }
            }
            assert_eq!(
                attrition_unit_ticks(&view, &rules),
                (supplied, exposed),
                "{t}"
            );
            total_supplied += supplied;
            total_exposed += exposed;
        }
        assert!(total_supplied > 0 && total_exposed > 0);
    }

    /// The acceptance bench at a tenth of its size: the lines fight, both
    /// sides lose units, and the run is repeatable.
    #[test]
    fn bench_combat_small_run_fights() {
        let rules = load_rules(&data()).unwrap();
        let b = run_combat_bench(&rules, 43, 300, 1, false);
        assert_eq!(b.units, 40, "rounded down to a multiple of 8");
        assert_eq!(b.step_ms.len(), 300);
        assert!(b.hits > 0);
        assert!(b.deaths.iter().all(|&d| d > 0), "{:?}", b.deaths);
        assert!(!b.fighting_ms.is_empty());
        assert_eq!(b.destroyed, 0, "no buildings on the field");
        // 43 rounds down to the same 40-unit battle, and it repeats exactly.
        assert_eq!(
            run_combat_bench(&rules, 40, 300, 1, false).final_hash,
            b.final_hash
        );
        let report = combat_bench_report(&b, 300, 1);
        let prefixes = [
            "bench combat units=40 ticks=300 seed=1",
            "step_ms mean=",
            "fighting_ticks=",
            "hits=",
            "final_hash=0x",
        ];
        assert_eq!(report.len(), prefixes.len());
        for (line, prefix) in report.iter().zip(prefixes) {
            assert!(line.starts_with(prefix), "{line}");
        }
        assert!(
            report[2].contains(" fighting_mean_ms=") && report[2].contains(" fighting_p95_ms=")
        );
        assert!(report[3].contains(" deaths_p0=") && report[3].contains(" destroyed=0"));
        // Exit 1 only for too few fighting ticks: 50 ticks cannot hold 100.
        assert_eq!(bench_combat(&data(), 40, 50, 1, false), ExitCode::FAILURE);
    }

    #[test]
    fn stats_ms_percentiles() {
        let s: Vec<f64> = (1..=100).map(f64::from).collect();
        let (mean, p95, max) = stats_ms(&s);
        assert!((mean - 50.5).abs() < 1e-9);
        assert!((p95 - 95.0).abs() < 1e-9);
        assert!((max - 100.0).abs() < 1e-9);
        assert_eq!(stats_ms(&[]), (0.0, 0.0, 0.0));
    }
}
