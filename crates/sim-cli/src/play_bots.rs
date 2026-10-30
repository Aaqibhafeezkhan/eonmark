//! `sim-cli play-bots` (M5b): seeded bot-versus-bot matches, one game per
//! seed, on `--jobs` worker threads, one CSV row per game on stdout in
//! seed order (docs/design/ai.md, "play-bots harness").
//!
//! Each game owns its `Sim`, `Rules` clone and controller, runs under
//! `catch_unwind` (a panic becomes a `panic` row for its seed and the other
//! games go on), and is stepped by `ai::harness::BotMatch` until the match
//! is decided or `--max-ticks`. The CSV holds no wall time, so two runs of
//! the same seeds print the same bytes; timings go to stderr.
//!
//! M6: `--a`/`--b` take `passive` or any difficulty id; when both
//! personality lists have more than one entry the games walk the ordered
//! distinct pairs (`--mirrors` adds the same-id pairs), and `random`
//! resolves per seed and seat ([`ai::resolve_personality`]).

use ai::harness::{BotMatch, CSV_HEADER, GameRow, Winner, exit_code, seats_of};
use clap::Args;
use rules::Rules;
use sim::{AiController, AiMemory, Command, MatchSetup, Outcome, PlayerId, SimView};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Exit status for usage, rules and bot-id errors.
const EXIT_USAGE: u8 = 2;

/// An inclusive seed range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seeds {
    /// First seed.
    pub first: u64,
    /// Last seed, inclusive.
    pub last: u64,
}

impl Seeds {
    /// The seeds in order.
    pub fn list(self) -> Vec<u64> {
        (self.first..=self.last).collect()
    }
}

/// Parse `A..B` (inclusive, `A <= B`) or a single `N` (`N..N`).
pub fn parse_seeds(s: &str) -> Result<Seeds, String> {
    let num = |t: &str| {
        t.trim()
            .parse::<u64>()
            .map_err(|_| format!("{t:?} is not a seed (a non-negative integer)"))
    };
    let seeds = match s.split_once("..") {
        Some((a, b)) => Seeds {
            first: num(a)?,
            last: num(b)?,
        },
        None => {
            let n = num(s)?;
            Seeds { first: n, last: n }
        }
    };
    if seeds.first > seeds.last {
        return Err(format!(
            "{s:?}: the first seed is after the last (the range is inclusive: A..B with A <= B)"
        ));
    }
    Ok(seeds)
}

/// `play-bots` flags.
#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct PlayBotsArgs {
    /// Seeds to play: `A..B` (inclusive) or a single `N`.
    #[arg(long, value_parser = parse_seeds)]
    pub seeds: Seeds,
    /// Bot of player A (`PlayerId(0)`): `passive` or a difficulty id
    /// (`easy`, `standard`, `hard`).
    #[arg(long, default_value = ai::BOT_STANDARD)]
    pub a: String,
    /// Bot of player B (`PlayerId(1)`), as for A.
    #[arg(long, default_value = ai::BOT_STANDARD)]
    pub b: String,
    /// Personalities of A, comma-separated (default `standard`; none for
    /// `passive`; `random` draws per seed). A single list cycles over the
    /// seeds; when both lists have several entries the games walk the
    /// ordered distinct pairs.
    #[arg(long, value_delimiter = ',')]
    pub personality_a: Vec<String>,
    /// Personalities of B, as for A.
    #[arg(long, value_delimiter = ',')]
    pub personality_b: Vec<String>,
    /// With two lists of several entries, also play the same-id pairs
    /// (mirrors).
    #[arg(long)]
    pub mirrors: bool,
    /// Stop a game here if it is still undecided (default: the 45-minute
    /// cap, which decides every game; a game stopped earlier is `none`).
    #[arg(long)]
    pub max_ticks: Option<u32>,
    /// Games played at once.
    #[arg(long, default_value_t = 1)]
    pub jobs: usize,
    /// Exit 0 even when a game ends in a Draw or undecided (a panic still
    /// exits 1).
    #[arg(long)]
    pub allow_undecided: bool,
}

/// One game's row and timings.
#[derive(Debug, Clone)]
pub struct Played {
    /// The CSV row.
    pub row: GameRow,
    /// Ticks stepped (0 after a panic).
    pub ticks: u32,
    /// Wall time of the game, in seconds.
    pub wall_s: f64,
    /// Mean whole-step time (sim and bots), in milliseconds.
    pub step_ms_mean: f64,
    /// Mean time inside the bots' `think`, per tick, in milliseconds.
    pub ai_ms_mean: f64,
    /// The panic message of a game that panicked.
    pub panic: Option<String>,
}

impl Played {
    /// The per-game stderr line.
    pub fn line(&self) -> String {
        match &self.panic {
            Some(msg) => format!("game seed={} panic: {msg}", self.row.seed),
            None => format!(
                "game seed={} ticks={} wall_s={:.2} step_ms_mean={:.4} ai_ms_mean={:.4}",
                self.row.seed, self.ticks, self.wall_s, self.step_ms_mean, self.ai_ms_mean
            ),
        }
    }
}

/// A controller wrapper that adds the wall time of every `think` to a
/// shared counter (wall-clock timing is allowed in sim-cli only).
pub struct Timed {
    /// The wrapped controller.
    pub inner: Box<dyn AiController>,
    /// Nanoseconds spent in `inner.think`.
    pub ns: Arc<AtomicU64>,
}

impl AiController for Timed {
    fn think(&mut self, p: PlayerId, view: &SimView<'_>, memory: &mut AiMemory) -> Vec<Command> {
        let t = Instant::now();
        let out = self.inner.think(p, view, memory);
        let ns = u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.ns.fetch_add(ns, Ordering::Relaxed);
        out
    }
}

/// Builds the controller for one game's setup.
pub type Factory = dyn Fn(&MatchSetup, &Rules) -> Box<dyn AiController> + Sync;

fn ms(ns: u64, ticks: u32) -> f64 {
    if ticks == 0 {
        return 0.0;
    }
    ns as f64 / 1e6 / f64::from(ticks)
}

/// Play one game to its outcome or `max_ticks`.
fn play(setup: &MatchSetup, rules: &Rules, max_ticks: u32, factory: &Factory) -> Played {
    let start = Instant::now();
    let ai_ns = Arc::new(AtomicU64::new(0));
    let controller = Box::new(Timed {
        inner: factory(setup, rules),
        ns: ai_ns.clone(),
    });
    let mut m = BotMatch::new(setup.clone(), rules.clone(), controller);
    let mut step_ns: u64 = 0;
    while m.sim.result().is_none() && m.sim.tick() < max_ticks {
        let t = Instant::now();
        m.step_sim();
        step_ns = step_ns.saturating_add(u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX));
        m.observe();
    }
    let ticks = m.sim.tick();
    Played {
        row: m.row(setup.seed),
        ticks,
        wall_s: start.elapsed().as_secs_f64(),
        step_ms_mean: ms(step_ns, ticks),
        ai_ms_mean: ms(ai_ns.load(Ordering::Relaxed), ticks),
        panic: None,
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic with no message".to_string())
}

/// Play every setup on `jobs` worker threads pulling the next index, each
/// game under `catch_unwind`; the results come back in setup order. The
/// per-game stderr line is printed as each game ends.
pub fn play_all(
    setups: &[MatchSetup],
    rules: &Rules,
    max_ticks: u32,
    jobs: usize,
    factory: &Factory,
) -> Vec<Played> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Played>>> = Mutex::new(vec![None; setups.len()]);
    std::thread::scope(|s| {
        for _ in 0..jobs.clamp(1, setups.len().max(1)) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(setup) = setups.get(i) else {
                        break;
                    };
                    let played =
                        catch_unwind(AssertUnwindSafe(|| play(setup, rules, max_ticks, factory)))
                            .unwrap_or_else(|payload| Played {
                                row: GameRow {
                                    seats: seats_of(setup),
                                    ..GameRow::panicked(setup.seed)
                                },
                                ticks: 0,
                                wall_s: 0.0,
                                step_ms_mean: 0.0,
                                ai_ms_mean: 0.0,
                                panic: Some(panic_message(payload.as_ref())),
                            });
                    eprintln!("{}", played.line());
                    results.lock().expect("no worker panics outside a game")[i] = Some(played);
                }
            });
        }
    });
    results
        .into_inner()
        .expect("no worker panics outside a game")
        .into_iter()
        .map(|p| p.expect("every index was played"))
        .collect()
}

/// The CSV: the header line and one row per game, in order.
pub fn csv(played: &[Played]) -> String {
    let mut out = String::from(CSV_HEADER);
    out.push('\n');
    for p in played {
        out.push_str(&p.row.csv());
        out.push('\n');
    }
    out
}

/// The end-of-run stderr summary.
pub fn summary(played: &[Played], jobs: usize, wall_s: f64) -> String {
    let count = |w: Winner| played.iter().filter(|p| p.row.winner == w).count();
    let outcome = |decisive: bool| {
        played
            .iter()
            .filter(|p| {
                matches!(p.row.result.map(|r| r.outcome), Some(Outcome::Victory { decisive: d, .. }) if d == decisive)
            })
            .count()
    };
    format!(
        "play-bots: games={} jobs={jobs} wall_s={wall_s:.2} a={} b={} draw={} none={} panic={} decisive={} tiebreak={}",
        played.len(),
        count(Winner::A),
        count(Winner::B),
        count(Winner::Draw),
        count(Winner::None),
        count(Winner::Panic),
        outcome(true),
        outcome(false),
    )
}

/// The personality list of one side: the flag's list, or `standard` (none
/// for `passive`). A personality for `passive` is a usage error.
fn personalities(side: &str, bot: &str, given: &[String]) -> Result<Vec<String>, String> {
    if bot == ai::BOT_PASSIVE {
        if given.iter().any(|p| !p.is_empty()) {
            return Err(format!("--personality-{side}: the passive bot takes none"));
        }
        return Ok(vec![String::new()]);
    }
    if given.is_empty() {
        return Ok(vec!["standard".to_string()]);
    }
    Ok(given.to_vec())
}

/// The `(a, b)` personality pairs a run walks, repeated over the seeds in
/// order (M6). When both lists have more than one entry: the ordered
/// pairs with B outer and A inner, in list order, skipping same-id pairs
/// unless `mirrors` (so `rush,boom,tower` on both sides walks (boom,rush),
/// (tower,rush), (rush,boom), (tower,boom), (rush,tower), (boom,tower), and
/// every pairing appears from both seats). `None` otherwise: each list
/// then cycles on its own over the seeds, as in M5b ([`pair_at`]).
pub fn pairs(pa: &[String], pb: &[String], mirrors: bool) -> Option<Vec<(String, String)>> {
    if pa.len() < 2 || pb.len() < 2 {
        return None;
    }
    let mut out = Vec::new();
    for b in pb {
        for a in pa {
            if mirrors || a != b {
                out.push((a.clone(), b.clone()));
            }
        }
    }
    Some(out)
}

/// The personalities of game `i` (0-based, in seed order): the `i`-th of
/// the walked pairs, or each list cycled on its own.
fn pair_at(
    i: usize,
    pa: &[String],
    pb: &[String],
    walked: Option<&[(String, String)]>,
) -> (String, String) {
    match walked {
        Some(walk) => walk[i % walk.len()].clone(),
        None => (pa[i % pa.len()].clone(), pb[i % pb.len()].clone()),
    }
}

/// `random` resolved for `player` at `seed` ([`ai::resolve_personality`]);
/// any other id is itself.
fn resolve(rules: &Rules, id: &str, seed: u64, player: PlayerId) -> Result<String, String> {
    if id == ai::RANDOM_PERSONALITY {
        return ai::resolve_personality(rules, seed, player);
    }
    Ok(id.to_string())
}

/// The setups of a run: `MatchSetup::bots` per seed, each side's
/// personality from [`pairs`] (or each list cycled), `random` resolved
/// into the header. Every setup's bots are built once here, so an unknown
/// bot or personality, or a `random` with no pool, is a usage error before
/// any game runs.
pub fn setups(args: &PlayBotsArgs, rules: &Rules) -> Result<Vec<MatchSetup>, String> {
    let pa = personalities("a", &args.a, &args.personality_a)?;
    let pb = personalities("b", &args.b, &args.personality_b)?;
    let walked = pairs(&pa, &pb, args.mirrors);
    if walked.as_ref().is_some_and(Vec::is_empty) {
        return Err(
            "--personality-a and --personality-b name no distinct pair; pass --mirrors to play mirrors"
                .to_string(),
        );
    }
    let mut out = Vec::new();
    for (i, seed) in args.seeds.list().into_iter().enumerate() {
        let (a, b) = pair_at(i, &pa, &pb, walked.as_deref());
        let a = resolve(rules, &a, seed, PlayerId(0))?;
        let b = resolve(rules, &b, seed, PlayerId(1))?;
        let setup = MatchSetup::bots(rules, seed, (&args.a, &a), (&args.b, &b));
        ai::controller_for(&setup, rules).map_err(|e| e.to_string())?;
        out.push(setup);
    }
    Ok(out)
}

/// `sim-cli play-bots`: exit 0, 1 when a game is undecided (unless
/// `--allow-undecided`) or panicked, 2 for usage, rules or bot-id errors.
pub fn run(data: &Path, args: &PlayBotsArgs) -> ExitCode {
    let rules = match Rules::load(data) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    if args.jobs == 0 {
        eprintln!("play-bots: --jobs must be at least 1");
        return ExitCode::from(EXIT_USAGE);
    }
    let setups = match setups(args, &rules) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("play-bots: {e}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let max_ticks = args.max_ticks.unwrap_or_else(|| rules.match_cap_ticks());
    let factory = |setup: &MatchSetup, rules: &Rules| {
        ai::controller_for(setup, rules).expect("setups() built these bots once")
    };
    let start = Instant::now();
    let played = play_all(&setups, &rules, max_ticks, args.jobs, &factory);
    print!("{}", csv(&played));
    eprintln!(
        "{}",
        summary(&played, args.jobs, start.elapsed().as_secs_f64())
    );
    let rows: Vec<GameRow> = played.into_iter().map(|p| p.row).collect();
    ExitCode::from(exit_code(&rows, args.allow_undecided))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn data() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    fn args(line: &str) -> PlayBotsArgs {
        use clap::Parser;
        #[derive(Parser)]
        struct Cli {
            #[command(flatten)]
            args: PlayBotsArgs,
        }
        let words = std::iter::once("play-bots").chain(line.split_whitespace());
        Cli::try_parse_from(words).expect("parses").args
    }

    fn run_csv(line: &str) -> (Vec<Played>, String) {
        let rules = Rules::load(data()).unwrap();
        let a = args(line);
        let setups = setups(&a, &rules).unwrap();
        let max = a.max_ticks.unwrap_or_else(|| rules.match_cap_ticks());
        let factory = |s: &MatchSetup, r: &Rules| ai::controller_for(s, r).unwrap();
        let played = play_all(&setups, &rules, max, a.jobs, &factory);
        let text = csv(&played);
        (played, text)
    }

    #[test]
    fn csv_header_is_exact() {
        assert_eq!(
            CSV_HEADER,
            "seed,winner,outcome,tick,final_hash,age_reached_tick,first_attack_tick,reason,first_attack_units,first_attack_wains,second_town_tick,tower_tick,bo_done_tick_a,age_reached_tick_b,first_attack_tick_b,first_attack_units_b,first_attack_wains_b,territory_a,territory_b,units_lost_a,units_lost_b,min_enemy_town_hp_permille_a,min_enemy_town_hp_permille_b,rejected_a,rejected_b,second_town_tick_b,tower_tick_b,bo_done_tick_b,bot_a,personality_a,bot_b,personality_b,waves_a,waves_b,raids_a,raids_b,retreats_a,retreats_b,wiped_a,wiped_b,towns_taken_a,towns_taken_b,bonus_a,bonus_b,income_a,income_b,cap_s_a,cap_s_b,first_contact_tick,max_quiet_ticks,first_on_land_tick_a,first_on_land_tick_b,min_own_town_hp_permille_a,min_own_town_hp_permille_b"
        );
    }

    fn list(ids: &str) -> Vec<String> {
        ids.split(',').map(str::to_string).collect()
    }

    fn pair_list(p: &[(String, String)]) -> Vec<(&str, &str)> {
        p.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect()
    }

    /// M6: two lists of several entries walk the ordered distinct pairs, B
    /// outer and A inner, so 20 seeds give the first two pairs 4 games and
    /// the rest 3, and every pairing is played from both seats.
    #[test]
    fn distinct_pairs_walk_a_fastest_and_skip_mirrors() {
        let three = list("rush,boom,tower");
        let walk = pairs(&three, &three, false).unwrap();
        assert_eq!(
            pair_list(&walk),
            [
                ("boom", "rush"),
                ("tower", "rush"),
                ("rush", "boom"),
                ("tower", "boom"),
                ("rush", "tower"),
                ("boom", "tower")
            ]
        );
        let games: Vec<(String, String)> = (0..20)
            .map(|i| pair_at(i, &three, &three, Some(&walk)))
            .collect();
        for (k, pair) in walk.iter().enumerate() {
            let n = games.iter().filter(|g| *g == pair).count();
            assert_eq!(n, if k < 2 { 4 } else { 3 }, "{pair:?}");
            let swapped = (pair.1.clone(), pair.0.clone());
            assert!(games.contains(&swapped), "{pair:?} from the other seat");
        }
        assert!(games.iter().all(|(a, b)| a != b), "no mirror");
        // Through the flags: seeds 1..6 play the six pairs in order.
        let rules = Rules::load(data()).unwrap();
        let mut rules_three = rules.clone();
        for id in ["rush", "boom", "tower"] {
            let mut p = rules.ai.personality("standard").unwrap().clone();
            p.id = id.to_string();
            rules_three.ai.personalities.push(p);
        }
        let s = setups(
            &args("--seeds 1..6 --personality-a rush,boom,tower --personality-b rush,boom,tower"),
            &rules_three,
        )
        .unwrap();
        let got: Vec<(&str, &str)> = s
            .iter()
            .map(|x| {
                (
                    x.ai_seats[0].personality.as_str(),
                    x.ai_seats[1].personality.as_str(),
                )
            })
            .collect();
        assert_eq!(got, pair_list(&walk));
        // Two lists naming one id only have no distinct pair.
        let err = setups(
            &args("--seeds 1 --personality-a standard,standard --personality-b standard,standard"),
            &rules,
        )
        .unwrap_err();
        assert!(err.contains("--mirrors"), "{err}");
    }

    /// M6: `--mirrors` walks all nine ordered pairs, A inner.
    #[test]
    fn mirrors_flag_walks_all_nine() {
        let three = list("rush,boom,tower");
        let walk = pairs(&three, &three, true).unwrap();
        assert_eq!(walk.len(), 9);
        assert_eq!(
            pair_list(&walk[..4]),
            [
                ("rush", "rush"),
                ("boom", "rush"),
                ("tower", "rush"),
                ("rush", "boom")
            ]
        );
        assert!(args("--seeds 1 --mirrors").mirrors);
        let rules = Rules::load(data()).unwrap();
        let s = setups(
            &args("--seeds 1..2 --personality-a standard,standard --personality-b standard,standard --mirrors"),
            &rules,
        )
        .unwrap();
        assert_eq!(s.len(), 2);
    }

    /// M6: when either list has one entry the lists cycle on their own, as
    /// in M5b, `--mirrors` or not.
    #[test]
    fn single_lists_cycle_as_before() {
        let one = list("standard");
        let two = list("x,y");
        assert_eq!(pairs(&one, &two, false), None);
        assert_eq!(pairs(&two, &one, true), None);
        let games: Vec<(String, String)> = (0..4).map(|i| pair_at(i, &two, &one, None)).collect();
        assert_eq!(
            pair_list(&games),
            [
                ("x", "standard"),
                ("y", "standard"),
                ("x", "standard"),
                ("y", "standard")
            ]
        );
        let rules = Rules::load(data()).unwrap();
        let s = setups(&args("--seeds 1..3 --a passive --b hard"), &rules).unwrap();
        assert!(s.iter().all(|x| x.ai_seats[0].bot == "passive"
            && x.ai_seats[0].personality.is_empty()
            && x.ai_seats[1].bot == "hard"
            && x.ai_seats[1].personality == "standard"));
    }

    /// M6: `random` resolves per seed and seat into the header; with no
    /// personality in the pool it is a usage error.
    #[test]
    fn random_resolves_into_the_header_or_is_a_usage_error() {
        let rules = Rules::load(data()).unwrap();
        // The shipped pool (M6 slice 2): rush, boom and tower.
        let s = setups(&args("--seeds 1..12 --personality-b random"), &rules).unwrap();
        let shipped: Vec<&str> = s
            .iter()
            .map(|x| x.ai_seats[1].personality.as_str())
            .collect();
        for id in ["rush", "boom", "tower"] {
            assert!(shipped.contains(&id), "{shipped:?}");
        }
        let mut empty = rules.clone();
        for p in &mut empty.ai.personalities {
            p.random = false;
        }
        let err = setups(&args("--seeds 1 --personality-b random"), &empty).unwrap_err();
        assert!(err.contains("random: true"), "{err}");
        let mut pooled = empty.clone();
        for id in ["p", "q"] {
            let mut p = rules.ai.personality("standard").unwrap().clone();
            p.id = id.to_string();
            p.random = true;
            pooled.ai.personalities.push(p);
        }
        let s = setups(&args("--seeds 1..12 --personality-b random"), &pooled).unwrap();
        let picks: Vec<&str> = s
            .iter()
            .map(|x| x.ai_seats[1].personality.as_str())
            .collect();
        assert!(picks.contains(&"p") && picks.contains(&"q"), "{picks:?}");
        for (x, pick) in s.iter().zip(&picks) {
            assert_eq!(
                ai::resolve_personality(&pooled, x.seed, PlayerId(1)).unwrap(),
                *pick
            );
        }
    }

    #[test]
    fn seed_ranges_parse() {
        assert_eq!(parse_seeds("1..5"), Ok(Seeds { first: 1, last: 5 }));
        assert_eq!(parse_seeds("7..7"), Ok(Seeds { first: 7, last: 7 }));
        assert_eq!(parse_seeds("7"), parse_seeds("7..7"));
        assert_eq!(parse_seeds("1..5").unwrap().list(), [1, 2, 3, 4, 5]);
        assert!(parse_seeds("5..1").unwrap_err().contains("inclusive"));
        assert!(parse_seeds("x").is_err());
        assert!(parse_seeds("1..").is_err());
        assert!(parse_seeds("-1").is_err());
        let a = args(
            "--seeds 1..3 --a passive --b standard --personality-b standard,standard --max-ticks 300 --jobs 3 --allow-undecided",
        );
        assert_eq!(a.seeds, Seeds { first: 1, last: 3 });
        assert_eq!((a.a.as_str(), a.b.as_str()), ("passive", "standard"));
        assert_eq!(a.personality_b, ["standard", "standard"]);
        assert_eq!(
            (a.max_ticks, a.jobs, a.allow_undecided),
            (Some(300), 3, true)
        );
        let d = args("--seeds 2");
        assert_eq!(
            (d.a.as_str(), d.b.as_str(), d.jobs),
            ("standard", "standard", 1)
        );
        assert_eq!(d.max_ticks, None);
    }

    /// Unknown bots and personalities, and a personality for `passive`,
    /// are usage errors before any game runs.
    #[test]
    fn unknown_bots_and_personalities_are_usage_errors() {
        let rules = Rules::load(data()).unwrap();
        let err = |line: &str| setups(&args(line), &rules).unwrap_err();
        assert!(
            err("--seeds 1 --a nightmare")
                .contains("unknown bot \"nightmare\" (known: passive, easy, hard, standard)")
        );
        assert!(err("--seeds 1 --personality-b zerg").contains("no personality \"zerg\""));
        assert!(err("--seeds 1 --b hard --personality-b zerg").contains("no personality \"zerg\""));
        assert!(err("--seeds 1 --a passive --personality-a standard").contains("takes none"));
        let ok = setups(
            &args("--seeds 1..4 --personality-a standard,standard"),
            &rules,
        )
        .unwrap();
        assert_eq!(ok.len(), 4);
        assert_eq!(ok[3].seed, 4);
        assert_eq!(ok[3].ai_seats[0].personality, "standard");
    }

    /// The CSV holds no wall time: one worker and three print the same
    /// bytes for the same seeds.
    #[test]
    fn jobs_1_and_jobs_3_print_identical_csv() {
        let (one, a) = run_csv("--seeds 1..3 --max-ticks 3000 --jobs 1 --allow-undecided");
        let (_, b) = run_csv("--seeds 1..3 --max-ticks 3000 --jobs 3 --allow-undecided");
        assert_eq!(a, b);
        assert_eq!(a.lines().count(), 4);
        assert!(
            one.iter()
                .all(|p| p.ticks == 3000 && p.row.winner == Winner::None)
        );
    }

    /// Two passive bots never fight: the 45-minute cap decides on
    /// territory, 1245 tiles each, an exact tie, so seed 1 is a Draw at
    /// tick 54000 and the run exits 1 unless `--allow-undecided`.
    #[test]
    fn passive_vs_passive_draws_at_the_cap_and_exits_1() {
        let (played, text) = run_csv("--seeds 1 --a passive --b passive");
        let row = text.lines().nth(1).unwrap();
        let cols: Vec<&str> = row.split(',').collect();
        assert_eq!(cols[..4], ["1", "draw", "draw", "54000"], "{row}");
        assert_eq!(cols[7], "territory");
        assert_eq!((cols[17], cols[18]), ("1245", "1245"), "{row}");
        let rows: Vec<GameRow> = played.into_iter().map(|p| p.row).collect();
        assert_eq!(exit_code(&rows, false), 1);
        assert_eq!(exit_code(&rows, true), 0);
    }

    /// A controller that panics on its tenth call.
    struct Boom(u32);

    impl AiController for Boom {
        fn think(&mut self, _: PlayerId, _: &SimView<'_>, _: &mut AiMemory) -> Vec<Command> {
            self.0 += 1;
            assert!(self.0 < 10, "boom");
            Vec::new()
        }
    }

    /// A game whose controller panics is a `panic` row for its seed; the
    /// games after it still run, on the same worker.
    #[test]
    fn a_panicking_controller_becomes_a_panic_row_and_later_seeds_still_run() {
        let rules = Rules::load(data()).unwrap();
        let a = args("--seeds 1..3 --a passive --b passive --jobs 1");
        let setups = setups(&a, &rules).unwrap();
        let factory = |s: &MatchSetup, r: &Rules| -> Box<dyn AiController> {
            if s.seed == 2 {
                Box::new(Boom(0))
            } else {
                ai::controller_for(s, r).unwrap()
            }
        };
        let played = play_all(&setups, &rules, 50, 1, &factory);
        let winners: Vec<Winner> = played.iter().map(|p| p.row.winner).collect();
        assert_eq!(winners, [Winner::None, Winner::Panic, Winner::None]);
        assert_eq!(played[1].panic.as_deref(), Some("boom"));
        assert_eq!(played[2].ticks, 50);
        assert!(played[1].line().starts_with("game seed=2 panic: boom"));
        let rows: Vec<GameRow> = played.into_iter().map(|p| p.row).collect();
        assert_eq!(
            exit_code(&rows, true),
            1,
            "a panic exits 1 even when undecided is allowed"
        );
    }
}
