//! `sim-cli match-log <replay>` (M6): re-simulate a replay with the bots
//! its header names (exactly as `verify` does, every recorded hash
//! checked), feed `sim::MatchStats` after every step, and print the
//! `docs/PLAYTEST.md` match-log row, a statistics table and the verify
//! line. A logged match can so be re-derived from its replay at any time.
//!
//! "You" is the first human slot of the header (P0 when every slot is a
//! bot, as in `m5_first_match`); the opponent is the other player. The
//! header carries no date and an empty `git_sha`, so `--date` and
//! `--commit` fill those cells (`just match-log` passes today's date and
//! `git rev-parse --short HEAD`).

use ai::standard::BotStats;
use sim::{
    MatchResult, MatchSetup, MatchStats, Outcome, PlayerId, ReplayError, Rules, SimEvent, SimView,
    VerifyOutcome,
};
use std::path::Path;

/// What one re-simulation of a replay gathered.
pub struct MatchLog {
    /// The replay header.
    pub setup: MatchSetup,
    /// The rules it ran under.
    pub rules: Rules,
    /// The statistics, fed after every step.
    pub stats: MatchStats,
    /// Each player's bot statistics at the end (`None` for a human slot
    /// or a Passive seat).
    pub bots: Vec<Option<BotStats>>,
    /// The match result at the end, if decided.
    pub result: Option<MatchResult>,
    /// The last tick re-simulated.
    pub last_tick: u32,
    /// The verify outcome.
    pub outcome: VerifyOutcome,
}

/// Re-simulate `replay` against `data` through `sim::verify_observed`.
/// `Ok(Err(outcome))` is a version or rules mismatch, where nothing was
/// simulated.
pub fn collect(replay: &Path, data: &Path) -> Result<Result<MatchLog, VerifyOutcome>, ReplayError> {
    let mut stats: Option<MatchStats> = None;
    let mut first: Option<(MatchSetup, Rules)> = None;
    let mut bots: Vec<Option<BotStats>> = Vec::new();
    let mut result = None;
    let mut last_tick = 0;
    let mut observe = |view: &SimView<'_>, events: &[SimEvent]| {
        if first.is_none() {
            first = Some((view.setup().clone(), view.rules().clone()));
        }
        stats
            .get_or_insert_with(|| MatchStats::new(view.setup(), view.rules()))
            .observe(view, events);
        bots = view
            .players()
            .iter()
            .map(|p| ai::bot_stats(view, p.id))
            .collect();
        result = view.result();
        last_tick = view.tick;
    };
    let outcome = sim::verify_observed(
        replay,
        data,
        Box::new(|setup: &MatchSetup, rules: &Rules| {
            ai::controller_for(setup, rules).map_err(|e| e.to_string())
        }),
        &mut observe,
    )?;
    let (Some((setup, rules)), Some(stats)) = (first, stats) else {
        return Ok(Err(outcome));
    };
    Ok(Ok(MatchLog {
        setup,
        rules,
        stats,
        bots,
        result,
        last_tick,
        outcome,
    }))
}

/// `m:ss` of match time at `tick`.
pub fn clock(tick: u32, rate_hz: u32) -> String {
    let secs = tick / rate_hz.max(1);
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Tenths, as `"27.4"`, of `num / den`, rounded down.
fn tenths(num: u64, den: u64) -> String {
    let t = num * 10 / den.max(1);
    format!("{}.{}", t / 10, t % 10)
}

impl MatchLog {
    /// "You": the first human slot, else P0.
    pub fn me(&self) -> PlayerId {
        self.setup
            .players
            .iter()
            .find(|s| !s.is_ai)
            .map_or(PlayerId(0), |s| s.id)
    }

    /// The opponent: the first other player.
    pub fn opponent(&self) -> PlayerId {
        let me = self.me();
        self.setup
            .players
            .iter()
            .map(|s| s.id)
            .find(|p| *p != me)
            .unwrap_or(PlayerId(1))
    }

    fn rate(&self) -> u32 {
        self.rules.tick_rate_hz
    }

    /// The opponent's `(bot, personality)` from the header (`human` for a
    /// human slot).
    fn opponent_seat(&self) -> (String, String) {
        let them = self.opponent();
        self.setup
            .ai_seats
            .iter()
            .find(|s| s.player == them)
            .map_or_else(
                || ("human".to_string(), String::new()),
                |s| (s.bot.clone(), s.personality.clone()),
            )
    }

    /// The difficulty's display name, else the bot id.
    fn difficulty_name(&self, bot: &str) -> String {
        self.rules
            .ai
            .difficulty(bot)
            .map_or_else(|| bot.to_string(), |d| d.name.clone())
    }

    /// The deciding tick, else the last tick re-simulated.
    pub fn end_tick(&self) -> u32 {
        self.result.map_or(self.last_tick, |r| r.tick)
    }

    /// The outcome cell from "your" side: `win`, `loss` or `draw`, then
    /// `decisive` or `tiebreak` (`undecided` before an outcome).
    pub fn outcome_cell(&self) -> String {
        let Some(r) = self.result else {
            return "undecided".to_string();
        };
        match r.outcome {
            Outcome::Draw => "draw tiebreak".to_string(),
            Outcome::Victory { player, decisive } => format!(
                "{} {}",
                if player == self.me() { "win" } else { "loss" },
                if decisive { "decisive" } else { "tiebreak" }
            ),
        }
    }

    /// The `docs/PLAYTEST.md` match-log row (M6): `| Date | Commit | Seed |
    /// Difficulty | Style | rules_version | Duration (min) | Outcome | AI
    /// reached my borders | Town below 50% HP | Cap readout amber (min) |
    /// Fun (1-5) | Notes | Replay | Verify |`, Fun and Notes left for the
    /// owner.
    pub fn row(&self, date: &str, commit: &str, replay_name: &str) -> String {
        let rate = self.rate();
        let me = self.stats.player(self.me());
        let them = self.stats.player(self.opponent());
        let (bot, style) = self.opponent_seat();
        let duration = tenths(u64::from(self.end_tick()), u64::from(rate) * 60);
        let borders = them
            .first_on_land_tick
            .map_or_else(|| "n".to_string(), |t| format!("y {}", clock(t, rate)));
        let town = if me.towns_lost > 0 && me.min_own_town_hp_permille == 0 {
            "y annexed".to_string()
        } else {
            let pct = me.min_own_town_hp_permille / 10;
            let below = me.min_own_town_hp_permille < 500;
            format!("{} {pct}%", if below { "y" } else { "n" })
        };
        let amber = if me.cap_seconds > 0 {
            format!("y {}", tenths(u64::from(me.cap_seconds), 60))
        } else {
            "n".to_string()
        };
        let verify = match &self.outcome {
            VerifyOutcome::Ok { final_hash, .. } => format!("OK 0x{final_hash:016x}"),
            other => other.to_string(),
        };
        format!(
            "| {date} | {commit} | {} | {} | {} | {} | {duration} | {} | {borders} | {town} | {amber} | | | {replay_name} | {verify} |",
            self.setup.seed,
            self.difficulty_name(&bot),
            if style.is_empty() { "-" } else { &style },
            self.setup.rules_version,
            self.outcome_cell(),
        )
    }

    /// One player's column of the statistics table.
    fn column(&self, p: PlayerId) -> Vec<String> {
        let rate = self.rate();
        let s = self.stats.player(p);
        let kinds = |m: &std::collections::BTreeMap<sim::UnitKindId, u32>| {
            let total: u32 = m.values().sum();
            let by: Vec<String> = m
                .iter()
                .map(|(k, n)| {
                    let id = self
                        .rules
                        .unit_kind(k.0)
                        .map_or_else(|| format!("kind {}", k.0), |u| u.id.clone());
                    format!("{id} {n}")
                })
                .collect();
            if by.is_empty() {
                total.to_string()
            } else {
                format!("{total} ({})", by.join(", "))
            }
        };
        let ages: Vec<String> = s
            .ages
            .iter()
            .map(|(age, t)| {
                let id = self
                    .rules
                    .ages
                    .get(usize::from(*age))
                    .map_or("?", |a| a.id.as_str());
                format!("{id} {}", clock(*t, rate))
            })
            .collect();
        let bot = self
            .bots
            .get(usize::from(p.0))
            .copied()
            .flatten()
            .map_or_else(
                || "-".to_string(),
                |b| {
                    format!(
                        "{} / {} / {} / {}",
                        b.waves_launched, b.raids, b.retreats, b.wiped
                    )
                },
            );
        let town = if s.towns_lost > 0 && s.min_own_town_hp_permille == 0 {
            "annexed".to_string()
        } else {
            format!("{}%", s.min_own_town_hp_permille / 10)
        };
        vec![
            kinds(&s.units_trained),
            kinds(&s.units_lost),
            format!("{} / {}", s.towns_taken, s.towns_lost),
            if ages.is_empty() {
                "-".to_string()
            } else {
                ages.join(", ")
            },
            s.peak_army.to_string(),
            self.stats.income_total(p).to_string(),
            format!(
                "{} ({}%)",
                s.bonus_total(),
                self.stats.bonus_share_permille(p) / 10
            ),
            clock(s.cap_seconds * rate, rate),
            s.first_on_land_tick
                .map_or_else(|| "-".to_string(), |t| clock(t, rate)),
            town,
            bot,
        ]
    }

    /// The statistics table: one row per statistic, "you" then the
    /// opponent, then the whole-match facts.
    pub fn table(&self) -> Vec<String> {
        let rate = self.rate();
        let (me, them) = (self.me(), self.opponent());
        let (bot, style) = self.opponent_seat();
        let names = [
            "Units trained",
            "Units lost",
            "Towns taken / lost",
            "Ages",
            "Peak army",
            "Income",
            "Bonus income (share)",
            "Cap amber",
            "First on enemy land",
            "Lowest own Town",
            "Waves / raids / retreats / wiped",
        ];
        let mut out = vec![
            format!(
                "| Stat | You (P{}) | {} {} (P{}) |",
                me.0,
                self.difficulty_name(&bot),
                style,
                them.0
            ),
            "|---|---|---|".to_string(),
        ];
        for ((name, a), b) in names.iter().zip(self.column(me)).zip(self.column(them)) {
            out.push(format!("| {name} | {a} | {b} |"));
        }
        let opt = |t: Option<u32>| t.map_or_else(|| "-".to_string(), |t| clock(t, rate));
        out.push(format!(
            "| First contact | {} | |",
            opt(self.stats.first_contact_tick())
        ));
        out.push(format!(
            "| Longest quiet | {} | |",
            opt(self.stats.max_quiet_ticks())
        ));
        out.push(format!(
            "| Duration | {} (tick {}) | |",
            clock(self.end_tick(), rate),
            self.end_tick()
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai::harness::BotMatch;
    use std::path::PathBuf;

    fn data() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    #[test]
    fn clock_and_tenths() {
        assert_eq!(clock(0, 20), "0:00");
        assert_eq!(clock(14_040, 20), "11:42");
        assert_eq!(clock(54_000, 20), "45:00");
        assert_eq!(tenths(32_880, 1200), "27.4");
        assert_eq!(tenths(390, 60), "6.5");
    }

    /// `match-log` re-derives exactly the statistics the play-bots
    /// harness observed for the same match (4400 ticks of Hard against
    /// Easy, written as a replay with no commands and one final hash), and
    /// verifies the replay.
    #[test]
    fn match_log_stats_equal_the_harness() {
        let rules = Rules::load(data()).unwrap();
        let setup = MatchSetup::bots(&rules, 3, ("hard", "standard"), ("easy", "standard"));
        let mut m = BotMatch::with_bots(setup.clone(), rules.clone()).unwrap();
        m.run(4400);
        let dir = std::env::temp_dir().join(format!("eonmark-match-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hard_easy.eonreplay");
        let mut w = sim::ReplayWriter::create(&path, &setup).unwrap();
        w.hash(m.sim.tick(), m.sim.hash(), &m.sim.sub_hashes())
            .unwrap();
        w.finish().unwrap();
        let log = collect(&path, &data()).unwrap().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            log.outcome,
            VerifyOutcome::Ok {
                final_hash: m.sim.hash(),
                ticks: 4400
            }
        );
        assert_eq!(&log.stats, m.observer.stats());
        assert!(log.stats.player(PlayerId(0)).bonus_total() > 0);
        assert_eq!(log.me(), PlayerId(0), "no human slot: P0");
        let row = log.row("2026-10-28", "abc1234", "hard_easy.eonreplay");
        assert!(
            row.starts_with("| 2026-10-28 | abc1234 | 3 | Easy | standard | "),
            "{row}"
        );
        assert!(row.contains(" | 3.6 | undecided | n | n 100% | "), "{row}");
        assert!(
            row.ends_with(&format!(
                " | | | hard_easy.eonreplay | OK 0x{:016x} |",
                m.sim.hash()
            )),
            "{row}"
        );
        assert_eq!(row.matches('|').count(), 16);
        let table = log.table();
        assert!(table[0].starts_with("| Stat | You (P0) | Easy standard (P1) |"));
        assert_eq!(table.len(), 2 + 11 + 3);
    }

    /// The replay of a human-against-bot match carries the human's
    /// commands as `Tick` records, and verify steps those ticks through its
    /// own branch. `match-log` must observe those steps too: the statistics
    /// it re-derives equal a `MatchStats` fed after every step of the
    /// recorded run, past first contact and the human's first losses, with
    /// commands on ticks whose next step is a 20-tick sample.
    #[test]
    fn match_log_stats_equal_the_recorded_run_with_human_commands() {
        let rules = Rules::load(data()).unwrap();
        let setup = MatchSetup::vs_bot(&rules, 5, "hard", "standard");
        let (worker, _) = rules::ai::worker_kind(&rules).unwrap();
        let mut sim = sim::Sim::new(
            setup.clone(),
            rules.clone(),
            ai::controller_for(&setup, &rules).unwrap(),
        );
        let p0 = PlayerId(0);
        let seat = sim::BuildingId(1);
        let enemy_seat = {
            let view = sim.view();
            let tile = view
                .buildings()
                .find(|b| b.owner == PlayerId(1))
                .map(|b| b.tile)
                .unwrap();
            view.map().center_of(tile)
        };
        let dir = std::env::temp_dir().join(format!("eonmark-match-log-h-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("human_hard.eonreplay");
        let mut w = sim::ReplayWriter::create(&path, &setup).unwrap();
        let mut stats = MatchStats::new(&setup, &rules);
        stats.observe(&sim.view(), &[]);
        let mut seq = 0u32;
        let mut command_ticks = Vec::new();
        for _ in 0..4000 {
            let t = sim.tick();
            let mut cmds = Vec::new();
            if t == 300 {
                // Two Yeomen walk to the enemy Seat: its shots are first
                // contact, and they die.
                cmds.push(sim::Command::Move {
                    units: vec![sim::UnitId(4), sim::UnitId(5)],
                    target: enemy_seat,
                    queue: false,
                });
            }
            if t % 1000 == 19 || t == 0 {
                cmds.push(sim::Command::Train {
                    building: seat,
                    kind: sim::UnitKindId(worker),
                });
            }
            let cmds: Vec<sim::PlayerCommand> = cmds
                .into_iter()
                .map(|c| {
                    seq += 1;
                    sim::PlayerCommand::new(p0, seq, c)
                })
                .collect();
            if !cmds.is_empty() {
                command_ticks.push(t);
            }
            w.tick(t, &cmds).unwrap();
            sim.step(&cmds);
            let events = sim.drain_events();
            stats.observe(&sim.view(), &events);
        }
        w.hash(sim.tick(), sim.hash(), &sim.sub_hashes()).unwrap();
        w.finish().unwrap();
        let log = collect(&path, &data()).unwrap().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            log.outcome,
            VerifyOutcome::Ok {
                final_hash: sim.hash(),
                ticks: 4000
            }
        );
        assert!(command_ticks.iter().filter(|t| *t % 20 == 19).count() >= 3);
        assert!(stats.first_contact_tick().is_some(), "contact");
        assert!(stats.player(p0).units_lost_total() > 0, "the Yeomen fell");
        assert!(stats.player(p0).units_trained_total() > 0);
        assert_eq!(log.me(), p0);
        assert_eq!(log.stats, stats);
    }
}
