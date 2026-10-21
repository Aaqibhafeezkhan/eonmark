//! The meeting point between Bevy and the simulation (M2, design decisions
//! 1-3): the `SimHandle` non-send resource, the `FixedUpdate` driver, the
//! `InputSource` trait with its three sources, `PendingCommands`, and the
//! replay recorder thread.
//!
//! One frame: `reset_frame_budget` (`PreUpdate`) zeroes the per-frame tick
//! counter; `step_sim` runs in `FixedUpdate` zero to [`MAX_TICKS_PER_FRAME`]
//! times (`Time<Virtual>::max_delta` is 250 ms, so at 20 Hz the schedule
//! never asks for more than 5; the cap guards a future higher rate). For
//! each due tick it asks the active [`InputSource`] for the commands issued
//! during that tick: `None` is a stall (no step, keep waiting), `Some(cmds)`
//! steps the sim, hands the batch to the recorder and, every
//! [`HashCadence::every`] ticks, a hash checkpoint. When the budget is spent
//! the remaining overstep is discarded and counted in [`DriverStats`].
//! `finish_recorder_on_exit` (Last) sees `AppExit` messages written earlier
//! in the frame, sends `Finish` and joins the writer thread with a bounded
//! wait, so Cmd-Q (through `macos_menu`), the close button and
//! `--exit-after-seconds` all leave a replay with the clean-exit trailer.
//!
//! The headless replay runner (`headless.rs`) reuses [`drive_tick`],
//! [`ReplayInput`] and [`ReplayFinished`] without the fixed clock or the
//! recorder; `frame_stats.rs` reads [`DriverStats::flushes_sent`] to
//! attribute frame-time spikes to the recorder flush.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bevy::app::AppExit;
use bevy::prelude::*;
use rules::Rules;
use sim::replay::{
    DEFAULT_HASH_EVERY, Record, ReplayError, ReplayFile, ReplayReader, ReplayWriter, TickBatch,
};
use sim::scenarios::{self, Stream};
use sim::{
    AiController, Command, MatchSetup, PlayerCommand, PlayerId, Sim, SimView, SubHashes,
    sort_commands,
};

/// Hard cap on simulation ticks stepped in one rendered frame.
pub const MAX_TICKS_PER_FRAME: u32 = 8;

/// `Time<Virtual>::max_delta`: a frame longer than this is clamped, so a
/// debugger pause or a window drag never queues more than 5 ticks at 20 Hz.
pub const MAX_DELTA: Duration = Duration::from_millis(250);

/// The recorder flushes (write, no fsync) after every this many ticks.
pub const FLUSH_EVERY_TICKS: u32 = DEFAULT_HASH_EVERY;

/// How long `finish_recorder_on_exit` waits for the writer thread.
pub const RECORDER_JOIN_TIMEOUT: Duration = Duration::from_secs(3);

/// The human player's slot in every windowed match.
pub const LOCAL_PLAYER: PlayerId = PlayerId(0);

/// The running match. Non-send resource: `Sim` holds a `Box<dyn
/// AiController>` with no `Sync` bound, so it is inserted with
/// `insert_non_send` and read through `NonSend` / `NonSendMut`. Nothing
/// outside this module calls `step`; every other module reads `view()`.
pub struct SimHandle {
    sim: Sim,
}

/// A controller for slots no scenario has: scenario setups are two human
/// slots, so it is never consulted.
struct NoSlots;

impl AiController for NoSlots {
    fn think(&mut self, _player: PlayerId, _view: &SimView<'_>) -> Vec<Command> {
        Vec::new()
    }
}

impl SimHandle {
    /// A skirmish from loaded rules and a seed with the passive bot in slot 1.
    pub fn skirmish(rules: Rules, seed: u64) -> Self {
        let setup = MatchSetup::skirmish(&rules, seed);
        Self {
            sim: Sim::new(setup, rules, Box::new(ai::Passive)),
        }
    }

    /// A match from an explicit setup (a scenario's or a replay header's).
    /// Human-only setups get a never-consulted controller; a setup with an
    /// AI slot gets the passive bot, which is what M0-M2 skirmishes use.
    pub fn from_setup(setup: MatchSetup, rules: Rules) -> Self {
        let ai: Box<dyn AiController> = if setup.players.iter().any(|p| p.is_ai) {
            Box::new(ai::Passive)
        } else {
            Box::new(NoSlots)
        };
        Self {
            sim: Sim::new(setup, rules, ai),
        }
    }

    /// Advance one tick with the commands issued during the current tick.
    pub fn step(&mut self, cmds: &[PlayerCommand]) {
        self.sim.step(cmds);
    }

    /// Advance one tick with no human commands (headless `--headless-run <ticks>`).
    pub fn step_once(&mut self) {
        self.sim.step(&[]);
    }

    /// Completed ticks.
    pub fn tick(&self) -> u32 {
        self.sim.tick()
    }

    /// Hash of the current simulation state.
    pub fn hash(&self) -> u64 {
        self.sim.hash()
    }

    /// Per-subsystem hashes (for hash checkpoints and the dev panel).
    pub fn sub_hashes(&self) -> SubHashes {
        self.sim.sub_hashes()
    }

    /// Ticks per second the match runs at.
    pub fn tick_rate_hz(&self) -> u32 {
        self.rules().tick_rate_hz
    }

    /// Seed the match was created with.
    pub fn seed(&self) -> u64 {
        self.sim.setup().seed
    }

    /// Read-only view for the presenter, selection and HUD.
    pub fn view(&self) -> SimView<'_> {
        self.sim.view()
    }

    /// The replay header / match setup.
    pub fn setup(&self) -> &MatchSetup {
        self.sim.setup()
    }

    /// The rules this match runs under (visuals, unit kinds).
    pub fn rules(&self) -> &Rules {
        self.sim.rules()
    }

    /// Events since the last drain (`UnitSpawned`, `UnitArrived`,
    /// `CommandRejected`, ...). The fixed-step driver drains them every tick
    /// and forwards them as [`SimEventMsg`].
    pub fn drain_events(&mut self) -> Vec<sim::SimEvent> {
        self.sim.drain_events()
    }
}

/// Counters the dev panel shows. Reset per frame where noted.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverStats {
    /// Ticks the fixed schedule asked for beyond [`MAX_TICKS_PER_FRAME`] and
    /// that were discarded as time dilation. Cumulative.
    pub dropped_ticks: u32,
    /// Fixed steps where the input source returned `None`. Cumulative.
    pub stalled_ticks: u32,
    /// Ticks stepped so far this frame (reset in `PreUpdate`).
    pub ticks_this_frame: u32,
    /// Ticks stepped in the previous frame.
    pub ticks_last_frame: u32,
    /// Commands handed to the recorder so far.
    pub commands_recorded: u64,
    /// Ticks after which the recorder was asked to flush (every
    /// [`FLUSH_EVERY_TICKS`]). `frame_stats` watches this to find the frame
    /// of each flush. Cumulative.
    pub flushes_sent: u32,
    /// `SimEvent::CommandRejected` drained so far. Cumulative.
    pub commands_rejected: u64,
    /// Every `SimEvent` drained so far. Cumulative.
    pub events_seen: u64,
}

/// Commands the UI issued this frame for the local player, stamped with a
/// per-player monotonically increasing `seq` at push time (decision 1:
/// the driver stamps nothing). Drained once per tick by [`LocalInput`] or
/// [`ScenarioInput`]. The scripted AI never appears here: it runs inside
/// `Sim::step`.
#[derive(Resource, Default, Debug)]
pub struct PendingCommands {
    queue: Vec<PlayerCommand>,
    next_seq: BTreeMap<PlayerId, u32>,
}

impl PendingCommands {
    /// Queue `cmd` for `player`; returns the sequence number it was stamped with.
    pub fn push(&mut self, player: PlayerId, cmd: Command) -> u32 {
        let seq = self.next_seq.entry(player).or_insert(0);
        let stamped = *seq;
        *seq += 1;
        self.queue.push(PlayerCommand::new(player, stamped, cmd));
        stamped
    }

    /// Queue `cmd` for [`LOCAL_PLAYER`].
    pub fn push_local(&mut self, cmd: Command) -> u32 {
        self.push(LOCAL_PLAYER, cmd)
    }

    /// Take every queued command, sorted by `(player, seq)`.
    pub fn drain_sorted(&mut self) -> Vec<PlayerCommand> {
        let mut out = std::mem::take(&mut self.queue);
        sort_commands(&mut out);
        out
    }

    /// Queued commands not yet handed to the sim, in push order (the dev
    /// panel lists them; the HUD may from M3b).
    #[cfg_attr(not(feature = "dev"), allow(dead_code))]
    pub fn peek(&self) -> &[PlayerCommand] {
        &self.queue
    }

    /// Number of queued commands (the dev panel shows it).
    #[cfg_attr(not(feature = "dev"), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// `true` when nothing is queued.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// The next sequence number `player` would be stamped with.
    pub fn next_seq(&self, player: PlayerId) -> u32 {
        self.next_seq.get(&player).copied().unwrap_or(0)
    }
}

/// Where a replay source stops and what it expects there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayEnd {
    /// The sim tick at which the recording ends: the tick of its last hash
    /// record (tick batches at or after it are dropped, since nothing
    /// recorded can check them), or one past its last tick batch when the
    /// file has no hash records. The driver steps while
    /// `sim.tick() < final_tick` and never beyond.
    pub final_tick: u32,
    /// The last recorded whole-state hash, if the file has hash records.
    pub final_hash: Option<u64>,
}

/// Where a tick's commands come from. One source is active per app and
/// lives in [`ActiveInput`]; a lockstep peer would be a fourth
/// implementation.
pub trait InputSource: Send + Sync + 'static {
    /// The commands issued during `tick`, to be handed to `Sim::step` for
    /// that tick. `None` means stall: the driver does not step this fixed
    /// update and asks again for the same tick next time. `local` is the
    /// UI's queue; sources that accept local input drain it, a replay
    /// ignores it (and the HUD is not expected to push while replaying).
    fn commands_for(
        &mut self,
        tick: u32,
        local: &mut PendingCommands,
    ) -> Option<Vec<PlayerCommand>>;

    /// `Some` for finite sources (replays): where to stop and what hash to
    /// expect. The driver writes [`ReplayFinished`] once `sim.tick()`
    /// reaches `final_tick` and steps no further.
    fn end_of_input(&self) -> Option<ReplayEnd> {
        None
    }

    /// Name for logs and the dev panel.
    fn name(&self) -> &'static str;

    /// One line for logs and the dev panel: the name plus whatever the
    /// source knows about its progress (a replay names its file and the
    /// batches left).
    fn describe(&self) -> String {
        self.name().to_owned()
    }
}

/// The player's own commands: drains [`PendingCommands`] every tick.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalInput;

impl InputSource for LocalInput {
    fn commands_for(
        &mut self,
        _tick: u32,
        local: &mut PendingCommands,
    ) -> Option<Vec<PlayerCommand>> {
        if local.is_empty() {
            return Some(Vec::new());
        }
        Some(local.drain_sorted())
    }

    fn name(&self) -> &'static str {
        "local"
    }
}

/// Replays recorded tick batches; ticks without a batch get `Some(empty)`.
#[derive(Debug)]
pub struct ReplayInput {
    batches: VecDeque<TickBatch>,
    end: ReplayEnd,
    path: PathBuf,
    truncated: bool,
    undecoded_bytes: usize,
    records: usize,
}

impl ReplayInput {
    /// Open and decode `path`; returns the header (to build the sim from)
    /// and the source. A truncated file (no clean-exit trailer) is accepted
    /// like `sim-cli verify` does; corrupt files are errors.
    pub fn open(path: &Path) -> Result<(MatchSetup, ReplayInput), ReplayError> {
        let file = ReplayReader::open(path)?;
        Ok(Self::from_file(path, file))
    }

    /// Build from an already decoded file.
    pub fn from_file(path: &Path, file: ReplayFile) -> (MatchSetup, ReplayInput) {
        let mut batches = VecDeque::new();
        let mut last_hash: Option<(u32, u64)> = None;
        let mut last_batch_tick: Option<u32> = None;
        let records = file.records.len();
        for record in file.records {
            match record {
                Record::Tick(batch) => {
                    last_batch_tick = Some(batch.tick);
                    batches.push_back(batch);
                }
                Record::Hash(rec) => last_hash = Some((rec.tick, rec.hash)),
            }
        }
        let end = match last_hash {
            // The hash can only be checked at its own tick: batches at or
            // past it (a recording cut after the writer flushed a tick batch
            // that follows the last checkpoint) are dropped, which is what
            // `sim-cli verify` reports for the same file.
            Some((tick, hash)) => {
                batches.retain(|b| b.tick < tick);
                ReplayEnd {
                    final_tick: tick,
                    final_hash: Some(hash),
                }
            }
            None => ReplayEnd {
                final_tick: last_batch_tick.map_or(0, |t| t + 1),
                final_hash: None,
            },
        };
        (
            file.setup,
            ReplayInput {
                batches,
                end,
                path: path.to_path_buf(),
                truncated: file.truncated,
                undecoded_bytes: file.undecoded_bytes,
                records,
            },
        )
    }

    /// `true` when the file has no clean-exit trailer (the recording was
    /// cut by `kill -9` or a crash); the batches up to the last complete
    /// record are replayed, like `sim-cli verify`.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// The `warning:` line `sim-cli verify` prints for a truncated file, or
    /// `None` for a cleanly finished one.
    pub fn truncation_warning(&self) -> Option<String> {
        self.truncated().then(|| {
            format!(
                "warning: {} has no clean-exit trailer (recording was cut); replaying {} complete records, {} trailing bytes dropped",
                self.path().display(),
                self.records,
                self.undecoded_bytes
            )
        })
    }

    /// The file being replayed.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Tick batches not yet handed out.
    pub fn remaining_batches(&self) -> usize {
        self.batches.len()
    }
}

impl InputSource for ReplayInput {
    fn commands_for(
        &mut self,
        tick: u32,
        _local: &mut PendingCommands,
    ) -> Option<Vec<PlayerCommand>> {
        if tick >= self.end.final_tick {
            return None;
        }
        // Drop batches the sim is already past (never happens on a fresh
        // sim; keeps a restored sim from replaying stale commands).
        while self.batches.front().is_some_and(|b| b.tick < tick) {
            self.batches.pop_front();
        }
        if self.batches.front().is_some_and(|b| b.tick == tick) {
            return self.batches.pop_front().map(|b| b.cmds);
        }
        Some(Vec::new())
    }

    fn end_of_input(&self) -> Option<ReplayEnd> {
        Some(self.end)
    }

    fn name(&self) -> &'static str {
        "replay"
    }

    fn describe(&self) -> String {
        format!(
            "replay {} ({} batches left, ends at tick {})",
            self.path().display(),
            self.remaining_batches(),
            self.end.final_tick
        )
    }
}

/// A `sim::scenarios` stream merged with the player's input: the scripted
/// commands for the tick first, then the local ones, all for the same tick.
/// Seq numbers are per player, so the scenario's (player 0, seq 0..n) and
/// the UI's stamps would collide if both used player 0 from zero; the
/// constructor therefore advances the local counter past the scenario's
/// highest seq per player on first use.
#[derive(Debug)]
pub struct ScenarioInput {
    stream: Stream,
    local: LocalInput,
    seeded_local_seq: bool,
}

impl ScenarioInput {
    /// Wrap a stream (from `sim::scenarios::by_name`).
    pub fn new(stream: Stream) -> Self {
        Self {
            stream,
            local: LocalInput,
            seeded_local_seq: false,
        }
    }

    /// Highest scripted seq per player, so local stamps start above it.
    fn seed_local_seq(&mut self, local: &mut PendingCommands) {
        if self.seeded_local_seq {
            return;
        }
        self.seeded_local_seq = true;
        let mut max_seq: BTreeMap<PlayerId, u32> = BTreeMap::new();
        for (_, cmds) in &self.stream {
            for c in cmds {
                let e = max_seq.entry(c.player).or_insert(0);
                *e = (*e).max(c.seq + 1);
            }
        }
        for (player, seq) in max_seq {
            let next = local.next_seq(player).max(seq);
            local.next_seq.insert(player, next);
        }
    }
}

impl InputSource for ScenarioInput {
    fn commands_for(
        &mut self,
        tick: u32,
        local: &mut PendingCommands,
    ) -> Option<Vec<PlayerCommand>> {
        self.seed_local_seq(local);
        let mut cmds: Vec<PlayerCommand> = scenarios::commands_at(&self.stream, tick).to_vec();
        cmds.extend(self.local.commands_for(tick, local)?);
        Some(cmds)
    }

    fn name(&self) -> &'static str {
        "scenario"
    }

    fn describe(&self) -> String {
        let scripted: usize = self.stream.iter().map(|(_, cmds)| cmds.len()).sum();
        format!(
            "scenario ({scripted} scripted commands over {} ticks) + local",
            self.stream.last().map_or(0, |(tick, _)| tick + 1)
        )
    }
}

/// The one active [`InputSource`].
#[derive(Resource)]
pub struct ActiveInput(pub Box<dyn InputSource>);

/// Hash checkpoint cadence in ticks (20, or 1 under `--hash-every-tick`).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct HashCadence {
    /// A checkpoint is recorded after every step whose resulting tick is a
    /// multiple of this.
    pub every: u32,
}

impl Default for HashCadence {
    fn default() -> Self {
        Self {
            every: DEFAULT_HASH_EVERY,
        }
    }
}

/// Written once when a finite input source (a replay) reaches its end.
/// The headless replay runner compares `recorded_hash` with `sim_hash`,
/// prints the final `tick=... hash=...` line and exits 0 or 1.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayFinished {
    /// Tick the sim stopped at (`ReplayEnd::final_tick`).
    pub final_tick: u32,
    /// The replay's last recorded hash, if any.
    pub recorded_hash: Option<u64>,
    /// The re-simulated hash at `final_tick`.
    pub sim_hash: u64,
}

impl ReplayFinished {
    /// `true` when the recording had no hash or it matches.
    pub fn matches(&self) -> bool {
        self.recorded_hash.is_none_or(|h| h == self.sim_hash)
    }
}

/// Message to the recorder thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecorderMessage {
    /// The commands handed to `Sim::step` for `tick` (may be empty; the
    /// writer skips empty batches).
    Tick {
        /// Tick the commands were issued during.
        tick: u32,
        /// The batch, already sorted.
        cmds: Vec<PlayerCommand>,
    },
    /// A hash checkpoint after the step that produced `tick`.
    Hash {
        /// Resulting tick.
        tick: u32,
        /// Whole-state hash.
        hash: u64,
        /// Per-subsystem hashes.
        sub: SubHashes,
    },
    /// Clean exit: write the trailer, flush, `sync_all`, return.
    Finish,
}

/// The writer thread body: owns the `ReplayWriter`, flushes every
/// [`FLUSH_EVERY_TICKS`] ticks, never fsyncs until `Finish`. Returns the
/// number of records written. A closed channel without `Finish` (the app
/// panicked) flushes and returns without the trailer, which the reader
/// reports as a truncated recording.
pub fn writer_thread(
    mut writer: ReplayWriter,
    rx: Receiver<RecorderMessage>,
) -> Result<u64, ReplayError> {
    let mut last_hashed: Option<u32> = None;
    let mut last_tick: Option<u32> = None;
    loop {
        match rx.recv() {
            Ok(RecorderMessage::Tick { tick, cmds }) => {
                writer.tick(tick, &cmds)?;
                last_tick = Some(tick + 1);
                if (tick + 1).is_multiple_of(FLUSH_EVERY_TICKS) {
                    writer.flush()?;
                }
            }
            Ok(RecorderMessage::Hash { tick, hash, sub }) => {
                writer.hash(tick, hash, &sub)?;
                last_hashed = Some(tick);
            }
            Ok(RecorderMessage::Finish) => {
                // The caller sends a final Hash before Finish; nothing else
                // can be synthesised here (the sim lives on the other thread).
                let _ = (last_hashed, last_tick);
                let records = writer.records();
                writer.finish()?;
                return Ok(records);
            }
            Err(_) => {
                writer.flush()?;
                return Ok(writer.records());
            }
        }
    }
}

/// Handle to the recorder thread. Dropping it without `finish` leaves a
/// truncated (but verifiable) file, like `kill -9`.
#[derive(Debug)]
pub struct Recorder {
    tx: Option<Sender<RecorderMessage>>,
    thread: Option<JoinHandle<Result<u64, ReplayError>>>,
    path: PathBuf,
    warned_dead: AtomicBool,
}

impl Recorder {
    /// Create `path` (parent directories included), write the header and
    /// start the writer thread.
    pub fn spawn(path: &Path, setup: &MatchSetup) -> Result<Recorder, ReplayError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ReplayError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let writer = ReplayWriter::create(path, setup)?;
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("eonmark-replay-writer".into())
            .spawn(move || writer_thread(writer, rx))
            .map_err(|source| ReplayError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(Recorder {
            tx: Some(tx),
            thread: Some(thread),
            path: path.to_path_buf(),
            warned_dead: AtomicBool::new(false),
        })
    }

    /// Queue a message; a dead writer is logged once and otherwise ignored
    /// (recording is a sink and never stalls the sim).
    pub fn send(&self, msg: RecorderMessage) {
        if let Some(tx) = &self.tx
            && tx.send(msg).is_err()
            && self.first_dead_send()
        {
            warn!(
                "replay writer thread is gone; {} stays truncated",
                self.path.display()
            );
        }
    }

    /// `true` only the first time a send found the writer gone: after a
    /// writer error the sim keeps sending 20+ messages a second, and one
    /// warning is enough.
    fn first_dead_send(&self) -> bool {
        !self.warned_dead.swap(true, Ordering::Relaxed)
    }

    /// File being written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Send `Finish` and join the thread, waiting at most `timeout`.
    /// Returns the records written, or a message describing why the file
    /// may lack its trailer.
    pub fn finish(&mut self, timeout: Duration) -> Result<u64, String> {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(RecorderMessage::Finish);
        }
        let Some(thread) = self.thread.take() else {
            return Err("recorder already finished".into());
        };
        let deadline = std::time::Instant::now() + timeout;
        while !thread.is_finished() {
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "replay writer did not finish within {timeout:?}; {} may lack its trailer",
                    self.path.display()
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        match thread.join() {
            Ok(Ok(records)) => Ok(records),
            Ok(Err(e)) => Err(format!("replay writer failed: {e}")),
            Err(_) => Err("replay writer panicked".into()),
        }
    }
}

/// The session's recorder, or `None` when this session does not record
/// (headless replay runs).
#[derive(Resource, Default)]
pub struct ReplayRecorder(pub Option<Recorder>);

/// `ProjectDirs::from("com", "tonianev", "Eonmark").data_dir()/replays`, or
/// `None` when the platform has no data directory.
pub fn default_replay_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("com", "tonianev", "Eonmark")
        .map(|d| d.data_dir().join("replays"))
}

/// `--replay-dir` when given, else [`default_replay_dir`], else `./replays`.
pub fn replay_dir(override_dir: Option<&Path>) -> PathBuf {
    override_dir
        .map(Path::to_path_buf)
        .or_else(default_replay_dir)
        .unwrap_or_else(|| PathBuf::from("replays"))
}

/// `<yyyymmdd-hhmmss>-<seed>.eonreplay` for a UTC Unix timestamp. Wall
/// clock is fine in the game crate; the sim never sees it.
pub fn replay_file_name(seed: u64, unix_secs: u64) -> String {
    let (y, m, d, hh, mm, ss) = civil_from_unix(unix_secs);
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}-{seed}.eonreplay")
}

/// A fresh replay path under `dir` for now.
pub fn new_replay_path(dir: &Path, seed: u64) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    dir.join(replay_file_name(seed, now))
}

/// Proleptic Gregorian civil date and time from Unix seconds (UTC).
/// Howard Hinnant's `civil_from_days`; no chrono dependency.
fn civil_from_unix(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m,
        d,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

/// One sim event, forwarded from `step_sim` after the tick that raised it
/// (the HUD message line reads `CommandRejected`; the presenter needs none
/// of them, it diffs ids). `tick` is `sim.tick()` after that step.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct SimEventMsg {
    /// The completed tick the event belongs to.
    pub tick: u32,
    /// The event.
    pub event: sim::SimEvent,
}

/// System set for the fixed-step sim tick; presentation syncs run
/// `.after(SimSystems::Step)` in `FixedUpdate` or in `FixedPostUpdate`.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SimSystems {
    /// `step_sim`.
    Step,
}

/// `true` when the current `FixedUpdate` iteration stepped the sim. A
/// stalled iteration (the source returned `None`), a cap-dropped one and
/// the iteration after a replay ended leave it `false`, so presentation
/// systems gated with [`sim_stepped`] do not advance their interpolation
/// past a tick that never happened.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SimStepped(pub bool);

/// Run condition for `FixedUpdate` systems after [`SimSystems::Step`]: the
/// sim stepped in this iteration.
pub fn sim_stepped(stepped: Res<SimStepped>) -> bool {
    stepped.0
}

#[derive(Resource, Default)]
struct FrameBudget {
    used: u32,
    /// `ReplayFinished` already written this run.
    finished: bool,
}

/// Installs the sim, the active input source, the fixed clock and the
/// recorder. Not a value plugin in the usual sense: `Sim` is `Send` but not
/// `Sync`, so the handle and the source are parked in mutexes and taken out
/// in `build`, which inserts the handle with `insert_non_send`.
pub struct SimPlugin {
    handle: Mutex<Option<SimHandle>>,
    input: Mutex<Option<Box<dyn InputSource>>>,
    /// Record this session to this path (every windowed session records;
    /// headless replay runs pass `None`).
    pub record_to: Option<PathBuf>,
    /// Hash checkpoint cadence.
    pub hash_every: u32,
}

impl SimPlugin {
    /// A plugin driving `handle` from `input`, not recording, hashing every 20 ticks.
    pub fn new(handle: SimHandle, input: Box<dyn InputSource>) -> Self {
        Self {
            handle: Mutex::new(Some(handle)),
            input: Mutex::new(Some(input)),
            record_to: None,
            hash_every: DEFAULT_HASH_EVERY,
        }
    }

    /// Record to `path` (`None` disables recording).
    pub fn recording(mut self, path: Option<PathBuf>) -> Self {
        self.record_to = path;
        self
    }

    /// Hash checkpoint cadence in ticks (at least 1).
    pub fn hash_every(mut self, every: u32) -> Self {
        self.hash_every = every.max(1);
        self
    }
}

impl Plugin for SimPlugin {
    fn build(&self, app: &mut App) {
        let handle = self
            .handle
            .lock()
            .expect("SimPlugin handle mutex")
            .take()
            .expect("SimPlugin::build runs once");
        let input = self
            .input
            .lock()
            .expect("SimPlugin input mutex")
            .take()
            .expect("SimPlugin::build runs once");
        let hz = f64::from(handle.tick_rate_hz());

        let recorder = match &self.record_to {
            Some(path) => match Recorder::spawn(path, handle.setup()) {
                Ok(rec) => {
                    info!("recording replay to {}", rec.path().display());
                    println!("replay: {}", rec.path().display());
                    Some(rec)
                }
                Err(e) => {
                    eprintln!("eonmark: cannot record replay to {}: {e}", path.display());
                    None
                }
            },
            None => None,
        };

        info!("input source: {}", input.describe());
        app.world_mut().insert_non_send(handle);
        app.insert_resource(ActiveInput(input))
            .insert_resource(Time::<Fixed>::from_hz(hz))
            .insert_resource(HashCadence {
                every: self.hash_every.max(1),
            })
            .insert_resource(ReplayRecorder(recorder))
            .init_resource::<DriverStats>()
            .init_resource::<PendingCommands>()
            .init_resource::<FrameBudget>()
            .init_resource::<SimStepped>()
            .add_message::<ReplayFinished>()
            .add_message::<SimEventMsg>()
            .add_systems(Startup, configure_virtual_time)
            .add_systems(PreUpdate, reset_frame_budget)
            .add_systems(FixedUpdate, step_sim.in_set(SimSystems::Step))
            // After `bevy_window::ExitSystems`: the red close button's
            // `AppExit` is written by `exit_on_all_closed` in `Last`, and the
            // winit runner exits right after this frame, so the recorder must
            // run later in `Last` or the trailer is skipped. The set is only
            // a label when `WindowPlugin` is absent (headless), which is fine.
            .add_systems(
                Last,
                finish_recorder_on_exit.after(bevy::window::ExitSystems),
            );
    }
}

fn configure_virtual_time(mut time: ResMut<Time<Virtual>>) {
    time.set_max_delta(MAX_DELTA);
}

fn reset_frame_budget(mut budget: ResMut<FrameBudget>, mut stats: ResMut<DriverStats>) {
    budget.used = 0;
    stats.ticks_last_frame = stats.ticks_this_frame;
    stats.ticks_this_frame = 0;
}

/// What one fixed step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickOutcome {
    /// `Sim::step` ran for this tick.
    Stepped {
        /// The tick that was completed (`sim.tick()` after the step).
        tick: u32,
    },
    /// The input source had nothing for this tick yet.
    Stalled,
    /// The source is finite and the sim is at its end.
    Finished(ReplayFinished),
}

/// Ask `input` for the current tick's commands, step, record. Shared by the
/// fixed-step system and the headless replay runner (agent A), so both
/// record and finish identically.
pub fn drive_tick(
    sim: &mut SimHandle,
    input: &mut dyn InputSource,
    pending: &mut PendingCommands,
    recorder: Option<&Recorder>,
    cadence: HashCadence,
    stats: &mut DriverStats,
) -> TickOutcome {
    let tick = sim.tick();
    if let Some(end) = input.end_of_input()
        && tick >= end.final_tick
    {
        return TickOutcome::Finished(ReplayFinished {
            final_tick: tick,
            recorded_hash: end.final_hash,
            sim_hash: sim.hash(),
        });
    }
    let Some(cmds) = input.commands_for(tick, pending) else {
        stats.stalled_ticks += 1;
        return TickOutcome::Stalled;
    };
    sim.step(&cmds);
    stats.ticks_this_frame += 1;
    if let Some(rec) = recorder {
        stats.commands_recorded += cmds.len() as u64;
        rec.send(RecorderMessage::Tick { tick, cmds });
        let done = sim.tick();
        if done.is_multiple_of(FLUSH_EVERY_TICKS) {
            stats.flushes_sent += 1;
        }
        if done.is_multiple_of(cadence.every.max(1)) {
            rec.send(RecorderMessage::Hash {
                tick: done,
                hash: sim.hash(),
                sub: sim.sub_hashes(),
            });
        }
    }
    TickOutcome::Stepped { tick: sim.tick() }
}

#[allow(clippy::too_many_arguments)] // Bevy system: each parameter is one resource or query
fn step_sim(
    mut sim: NonSendMut<SimHandle>,
    mut input: ResMut<ActiveInput>,
    mut pending: ResMut<PendingCommands>,
    recorder: Res<ReplayRecorder>,
    cadence: Res<HashCadence>,
    mut stats: ResMut<DriverStats>,
    mut budget: ResMut<FrameBudget>,
    mut fixed: ResMut<Time<Fixed>>,
    mut finished: MessageWriter<ReplayFinished>,
    mut events: MessageWriter<SimEventMsg>,
    mut stepped: ResMut<SimStepped>,
) {
    stepped.0 = false;
    if budget.used >= MAX_TICKS_PER_FRAME {
        // This step plus every further one the overstep would ask for are
        // dropped as time dilation; discarding the overstep ends the loop.
        let step = fixed.timestep().as_secs_f64().max(f64::EPSILON);
        let remaining = (fixed.overstep().as_secs_f64() / step).floor() as u32;
        stats.dropped_ticks += 1 + remaining;
        fixed.discard_overstep(Duration::MAX);
        return;
    }
    budget.used += 1;
    match drive_tick(
        &mut sim,
        input.0.as_mut(),
        &mut pending,
        recorder.0.as_ref(),
        *cadence,
        &mut stats,
    ) {
        TickOutcome::Stepped { tick } => {
            stepped.0 = true;
            // Events are not hashed state: drain them every tick so the
            // buffer never grows, count them, and forward them as
            // `SimEventMsg` for the HUD.
            for event in sim.drain_events() {
                stats.events_seen += 1;
                if matches!(event, sim::SimEvent::CommandRejected { .. }) {
                    stats.commands_rejected += 1;
                }
                events.write(SimEventMsg { tick, event });
            }
        }
        TickOutcome::Stalled => {}
        TickOutcome::Finished(end) => {
            if !budget.finished {
                budget.finished = true;
                finished.write(end);
            }
        }
    }
}

/// Runs in `Last`, after `bevy_window::ExitSystems`: on the frame an
/// `AppExit` was written, record a final hash checkpoint, send `Finish` and
/// join the writer (bounded wait). The winit runner checks
/// `App::should_exit` only after the frame, so this system sees messages
/// written in `Update` (Cmd-Q, `--exit-after-seconds`) and, thanks to the
/// ordering, the one `exit_on_all_closed` writes in `Last` for the close
/// button (verified with `--close-window-after-seconds`, M2-C).
fn finish_recorder_on_exit(
    mut exits: MessageReader<AppExit>,
    sim: NonSend<SimHandle>,
    mut recorder: ResMut<ReplayRecorder>,
) {
    if exits.is_empty() {
        return;
    }
    exits.clear();
    let Some(mut rec) = recorder.0.take() else {
        return;
    };
    rec.send(RecorderMessage::Hash {
        tick: sim.tick(),
        hash: sim.hash(),
        sub: sim.sub_hashes(),
    });
    match rec.finish(RECORDER_JOIN_TIMEOUT) {
        Ok(records) => info!(
            "replay finished: {} ({records} records, tick {}, hash {:#018x})",
            rec.path().display(),
            sim.tick(),
            sim.hash()
        ),
        Err(why) => eprintln!("eonmark: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::replay::HashRecord;
    use std::path::Path;

    fn rules() -> Rules {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")).unwrap()
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eonmark-game-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn pending_commands_stamp_per_player_and_drain_sorted() {
        let mut p = PendingCommands::default();
        assert!(p.is_empty());
        assert_eq!(p.push(PlayerId(1), Command::Surrender), 0);
        assert_eq!(p.push_local(Command::Surrender), 0);
        assert_eq!(p.push_local(Command::Surrender), 1);
        assert_eq!(p.next_seq(LOCAL_PLAYER), 2);
        assert_eq!(p.len(), 3);
        let drained = p.drain_sorted();
        let keys: Vec<_> = drained.iter().map(PlayerCommand::sort_key).collect();
        assert_eq!(
            keys,
            vec![(PlayerId(0), 0), (PlayerId(0), 1), (PlayerId(1), 0)]
        );
        assert!(p.is_empty());
        // Seq keeps counting across drains.
        assert_eq!(p.push_local(Command::Surrender), 2);
    }

    #[test]
    fn local_input_drains_everything_each_tick() {
        let mut p = PendingCommands::default();
        p.push_local(Command::Surrender);
        let mut src = LocalInput;
        assert_eq!(src.commands_for(0, &mut p).unwrap().len(), 1);
        assert_eq!(src.commands_for(1, &mut p).unwrap().len(), 0);
        assert!(src.end_of_input().is_none());
    }

    #[test]
    fn scenario_input_merges_scripted_then_local_and_avoids_seq_collisions() {
        let rules = rules();
        let (_, stream, _) = scenarios::by_name("units200_auto", &rules).unwrap();
        let mut src = ScenarioInput::new(stream);
        let mut p = PendingCommands::default();
        // Local seq starts above the scenario's highest seq for player 0 (1).
        let t0 = src.commands_for(0, &mut p).unwrap();
        assert_eq!(t0.len(), 1, "the spawn");
        assert_eq!(p.next_seq(LOCAL_PLAYER), 2);
        p.push_local(Command::Stop {
            units: vec![sim::UnitId(1)],
        });
        let t20 = src
            .commands_for(scenarios::UNITS200_AUTO_MOVE_TICK, &mut p)
            .unwrap();
        assert_eq!(t20.len(), 2);
        assert!(matches!(t20[0].cmd, Command::Move { .. }), "scripted first");
        assert!(matches!(t20[1].cmd, Command::Stop { .. }), "local second");
        assert_eq!(t20[0].seq, 1);
        assert_eq!(t20[1].seq, 2);
        assert!(src.commands_for(21, &mut p).unwrap().is_empty());
    }

    #[test]
    fn replay_input_hands_out_batches_and_ends_at_the_last_hash() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../sim/tests/fixtures/move_500_short.eonreplay");
        let (setup, mut src) = ReplayInput::open(&path).unwrap();
        assert!(setup.debug_commands);
        let end = src.end_of_input().unwrap();
        assert_eq!(end.final_tick, scenarios::MOVE_500_SHORT_TICKS);
        let expected: u64 = u64::from_str_radix(
            std::fs::read_to_string(path.with_extension("hash"))
                .unwrap()
                .trim()
                .trim_start_matches("0x"),
            16,
        )
        .unwrap();
        assert_eq!(end.final_hash, Some(expected));
        let mut p = PendingCommands::default();
        assert_eq!(src.commands_for(0, &mut p).unwrap().len(), 1, "spawn at 0");
        assert!(src.commands_for(1, &mut p).unwrap().is_empty());
        assert_eq!(src.commands_for(5, &mut p).unwrap().len(), 1, "move at 5");
        assert!(src.commands_for(299, &mut p).unwrap().is_empty());
        assert!(src.commands_for(300, &mut p).is_none(), "past the end");
        assert_eq!(src.remaining_batches(), 0);
    }

    #[test]
    fn drive_tick_replays_a_fixture_to_its_recorded_hash() {
        let rules = rules();
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../sim/tests/fixtures/move_500_short.eonreplay");
        let (setup, mut src) = ReplayInput::open(&path).unwrap();
        let mut sim = SimHandle::from_setup(setup, rules);
        let mut pending = PendingCommands::default();
        let mut stats = DriverStats::default();
        let cadence = HashCadence::default();
        let finished = loop {
            match drive_tick(&mut sim, &mut src, &mut pending, None, cadence, &mut stats) {
                TickOutcome::Stepped { .. } => {}
                TickOutcome::Stalled => panic!("a replay never stalls"),
                TickOutcome::Finished(f) => break f,
            }
        };
        assert_eq!(finished.final_tick, scenarios::MOVE_500_SHORT_TICKS);
        assert!(finished.matches(), "{finished:?}");
        assert_eq!(stats.stalled_ticks, 0);
    }

    #[test]
    fn replay_input_ends_at_the_last_hash_and_drops_later_batches() {
        let rules = rules();
        // A recording cut after the writer flushed a tick batch that follows
        // the last hash checkpoint: the hash can only be checked at its own
        // tick, so the later batch is dropped (as `sim-cli verify` does).
        let file = ReplayFile {
            setup: MatchSetup::scenario(&rules, 5),
            records: vec![
                Record::Tick(TickBatch {
                    tick: 3,
                    cmds: vec![PlayerCommand::new(PlayerId(0), 0, Command::Surrender)],
                }),
                Record::Hash(HashRecord {
                    tick: 20,
                    hash: 0xabc,
                    sub: SubHashes::default(),
                }),
                Record::Tick(TickBatch {
                    tick: 20,
                    cmds: vec![PlayerCommand::new(PlayerId(0), 1, Command::Surrender)],
                }),
            ],
            truncated: true,
            undecoded_bytes: 4,
        };
        let (_, mut src) = ReplayInput::from_file(Path::new("cut.eonreplay"), file);
        assert_eq!(
            src.end_of_input(),
            Some(ReplayEnd {
                final_tick: 20,
                final_hash: Some(0xabc),
            })
        );
        assert_eq!(src.remaining_batches(), 1, "the tick-20 batch is dropped");
        let mut p = PendingCommands::default();
        assert!(src.commands_for(20, &mut p).is_none());
    }

    #[test]
    fn drive_tick_matches_a_cut_replay_with_a_batch_after_its_last_hash() {
        let rules = rules();
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../sim/tests/fixtures/move_500_short.eonreplay");
        let mut file = ReplayReader::open(&path).unwrap();
        // Keep everything up to the first hash checkpoint after the move,
        // then append a command batch at that checkpoint's tick, the shape a
        // `kill -9` leaves when a command was issued in the last flushed
        // 20-tick window.
        let cut = file
            .records
            .iter()
            .position(|r| matches!(r, Record::Hash(h) if h.tick > 5))
            .unwrap();
        file.records.truncate(cut + 1);
        let Record::Hash(last) = &file.records[cut] else {
            unreachable!()
        };
        let (hash_tick, hash) = (last.tick, last.hash);
        file.records.push(Record::Tick(TickBatch {
            tick: hash_tick,
            cmds: vec![PlayerCommand::new(
                PlayerId(0),
                1000,
                Command::Stop {
                    units: vec![sim::UnitId(1)],
                },
            )],
        }));
        file.truncated = true;
        let (setup, mut src) = ReplayInput::from_file(&path, file);
        let mut sim = SimHandle::from_setup(setup, rules);
        let mut pending = PendingCommands::default();
        let mut stats = DriverStats::default();
        let finished = loop {
            match drive_tick(
                &mut sim,
                &mut src,
                &mut pending,
                None,
                HashCadence::default(),
                &mut stats,
            ) {
                TickOutcome::Stepped { .. } => {}
                TickOutcome::Stalled => panic!("a replay never stalls"),
                TickOutcome::Finished(f) => break f,
            }
        };
        assert_eq!(finished.final_tick, hash_tick);
        assert_eq!(finished.recorded_hash, Some(hash));
        assert!(finished.matches(), "{finished:?}");
    }

    #[test]
    fn a_dead_writer_is_reported_once() {
        // A writer thread that died (disk full, write error) drops its
        // receiver; every later send fails.
        let (tx, rx) = mpsc::channel();
        drop(rx);
        let rec = Recorder {
            tx: Some(tx),
            thread: None,
            path: PathBuf::from("dead.eonreplay"),
            warned_dead: AtomicBool::new(false),
        };
        for tick in 0..3 {
            rec.send(RecorderMessage::Tick {
                tick,
                cmds: Vec::new(),
            });
        }
        assert!(
            !rec.first_dead_send(),
            "the first failed send already used up the one warning"
        );
        let fresh = Recorder {
            warned_dead: AtomicBool::new(false),
            ..rec
        };
        assert!(fresh.first_dead_send());
        assert!(!fresh.first_dead_send());
    }

    #[test]
    fn recorder_writes_a_verifiable_replay_with_trailer() {
        let rules = rules();
        let (setup, stream, ticks) = scenarios::by_name("hud_click", &rules).unwrap();
        let mut sim = SimHandle::from_setup(setup.clone(), rules.clone());
        let path = temp("recorder.eonreplay");
        let mut rec = Recorder::spawn(&path, &setup).unwrap();
        let mut src = ScenarioInput::new(stream);
        let mut pending = PendingCommands::default();
        let mut stats = DriverStats::default();
        let cadence = HashCadence::default();
        for _ in 0..ticks {
            let out = drive_tick(
                &mut sim,
                &mut src,
                &mut pending,
                Some(&rec),
                cadence,
                &mut stats,
            );
            assert!(matches!(out, TickOutcome::Stepped { .. }));
        }
        rec.send(RecorderMessage::Hash {
            tick: sim.tick(),
            hash: sim.hash(),
            sub: sim.sub_hashes(),
        });
        let records = rec.finish(Duration::from_secs(5)).unwrap();
        assert!(records > u64::from(ticks) / 20, "{records}");
        let file = ReplayReader::open(&path).unwrap();
        assert!(!file.truncated, "finish wrote the trailer");
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let outcome = sim::replay::verify(&path, &data, Box::new(ai::Passive)).unwrap();
        assert_eq!(outcome.exit_code(), 0, "{outcome}");
        assert_eq!(stats.commands_recorded, 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn recorder_without_finish_leaves_a_truncated_but_readable_file() {
        let rules = rules();
        let setup = MatchSetup::scenario(&rules, 9);
        let path = temp("truncated.eonreplay");
        {
            let rec = Recorder::spawn(&path, &setup).unwrap();
            rec.send(RecorderMessage::Tick {
                tick: 0,
                cmds: vec![PlayerCommand::new(PlayerId(0), 0, Command::Surrender)],
            });
            rec.send(RecorderMessage::Hash {
                tick: 20,
                hash: 1,
                sub: SubHashes::default(),
            });
            // Dropped without Finish: the thread sees the closed channel,
            // flushes and returns; no trailer.
            drop(rec);
        }
        // Give the thread a moment to flush after the channel closed.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let file = loop {
            if let Ok(f) = ReplayReader::open(&path)
                && f.records.len() == 2
            {
                break f;
            }
            assert!(std::time::Instant::now() < deadline, "writer never flushed");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(file.truncated);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn local_input_assigns_seq_in_push_order_and_sorts_by_player_then_seq() {
        let mut p = PendingCommands::default();
        let mut src = LocalInput;
        // Interleaved pushes for two players: each player's seq counts on
        // its own, and the drained batch is ordered (player, seq).
        assert_eq!(p.push(PlayerId(1), Command::Surrender), 0);
        assert_eq!(p.push_local(Command::Surrender), 0);
        assert_eq!(p.push(PlayerId(1), Command::Surrender), 1);
        assert_eq!(p.push_local(Command::Surrender), 1);
        assert_eq!(p.push_local(Command::Surrender), 2);
        let batch = src.commands_for(7, &mut p).unwrap();
        let keys: Vec<_> = batch.iter().map(PlayerCommand::sort_key).collect();
        assert_eq!(
            keys,
            vec![
                (PlayerId(0), 0),
                (PlayerId(0), 1),
                (PlayerId(0), 2),
                (PlayerId(1), 0),
                (PlayerId(1), 1),
            ]
        );
        assert!(src.commands_for(8, &mut p).unwrap().is_empty());
        // A later tick's pushes continue the per-player counters.
        assert_eq!(p.push_local(Command::Surrender), 3);
        assert_eq!(p.push(PlayerId(1), Command::Surrender), 2);
        assert_eq!(p.next_seq(PlayerId(2)), 0);
    }

    #[test]
    fn replay_input_end_detection_without_hash_records() {
        let rules = rules();
        let setup = MatchSetup::scenario(&rules, 5);
        // A cut recording that never reached its first hash checkpoint: the
        // end is one past the last batch and there is nothing to compare.
        let file = ReplayFile {
            setup: setup.clone(),
            records: vec![
                Record::Tick(TickBatch {
                    tick: 0,
                    cmds: vec![PlayerCommand::new(PlayerId(0), 0, Command::Surrender)],
                }),
                Record::Tick(TickBatch {
                    tick: 7,
                    cmds: vec![PlayerCommand::new(PlayerId(0), 1, Command::Surrender)],
                }),
            ],
            truncated: true,
            undecoded_bytes: 3,
        };
        let (_, mut src) = ReplayInput::from_file(Path::new("cut.eonreplay"), file);
        assert!(src.truncated());
        assert!(
            src.truncation_warning()
                .unwrap()
                .starts_with("warning: cut.eonreplay")
        );
        assert_eq!(
            src.end_of_input(),
            Some(ReplayEnd {
                final_tick: 8,
                final_hash: None,
            })
        );
        let mut p = PendingCommands::default();
        for tick in 0..8 {
            let cmds = src.commands_for(tick, &mut p).unwrap();
            assert_eq!(
                cmds.len(),
                usize::from(tick == 0 || tick == 7),
                "tick {tick}"
            );
        }
        assert!(src.commands_for(8, &mut p).is_none());
        assert!(src.describe().contains("0 batches left"));

        // A hash record past the last batch sets the end; an empty file ends at 0.
        let file = ReplayFile {
            setup: setup.clone(),
            records: vec![
                Record::Tick(TickBatch {
                    tick: 3,
                    cmds: vec![PlayerCommand::new(PlayerId(0), 0, Command::Surrender)],
                }),
                Record::Hash(HashRecord {
                    tick: 20,
                    hash: 0xabc,
                    sub: SubHashes::default(),
                }),
            ],
            truncated: false,
            undecoded_bytes: 0,
        };
        let (_, src) = ReplayInput::from_file(Path::new("x.eonreplay"), file);
        assert!(!src.truncated());
        assert!(src.truncation_warning().is_none());
        assert_eq!(
            src.end_of_input(),
            Some(ReplayEnd {
                final_tick: 20,
                final_hash: Some(0xabc),
            })
        );
        let empty = ReplayFile {
            setup,
            records: Vec::new(),
            truncated: false,
            undecoded_bytes: 0,
        };
        let (_, src) = ReplayInput::from_file(Path::new("e.eonreplay"), empty);
        assert_eq!(src.end_of_input().unwrap().final_tick, 0);
        // drive_tick finishes such a source immediately, matching trivially.
        let mut sim = SimHandle::from_setup(MatchSetup::scenario(&rules, 5), rules);
        let mut stats = DriverStats::default();
        let mut src = src;
        let TickOutcome::Finished(f) = drive_tick(
            &mut sim,
            &mut src,
            &mut p,
            None,
            HashCadence::default(),
            &mut stats,
        ) else {
            panic!("an empty replay is finished at tick 0");
        };
        assert_eq!(f.final_tick, 0);
        assert!(f.matches());
    }

    /// A `MinimalPlugins` app with the sim plugin and a manual clock, after
    /// its first update (which only primes `Time<Real>`).
    fn fixed_clock_app(input: Box<dyn InputSource>) -> App {
        let handle = SimHandle::skirmish(rules(), 1);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::ZERO,
        ));
        app.add_plugins(SimPlugin::new(handle, input));
        app.update();
        assert_eq!(app.world().non_send::<SimHandle>().tick(), 0);
        app
    }

    fn advance(app: &mut App, by: Duration) {
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(by));
        app.update();
    }

    #[test]
    fn a_one_second_stall_is_clamped_to_max_delta() {
        let mut app = fixed_clock_app(Box::new(LocalInput));
        assert_eq!(
            app.world().resource::<Time<Virtual>>().max_delta(),
            MAX_DELTA,
            "configure_virtual_time ran at Startup"
        );
        advance(&mut app, Duration::from_secs(1));
        // 250 ms at 20 Hz = 5 ticks; nothing reaches the 8-tick cap.
        assert_eq!(app.world().non_send::<SimHandle>().tick(), 5);
        let stats = *app.world().resource::<DriverStats>();
        assert_eq!(stats.ticks_this_frame, 5);
        assert_eq!(stats.dropped_ticks, 0);
        assert_eq!(stats.stalled_ticks, 0);
        assert_eq!(
            app.world().resource::<Time<Fixed>>().overstep(),
            Duration::ZERO
        );
    }

    #[test]
    fn tick_cap_runs_exactly_eight_ticks_and_discards_the_overstep() {
        let mut app = fixed_clock_app(Box::new(LocalInput));
        // Lift the clamp so the whole stall reaches the fixed clock: a
        // 1 s stall at 20 Hz asks for 20 ticks.
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .set_max_delta(Duration::from_secs(2));
        advance(&mut app, Duration::from_secs(1));
        assert_eq!(
            app.world().non_send::<SimHandle>().tick(),
            MAX_TICKS_PER_FRAME
        );
        let stats = *app.world().resource::<DriverStats>();
        assert_eq!(stats.ticks_this_frame, MAX_TICKS_PER_FRAME);
        assert_eq!(stats.dropped_ticks, 20 - MAX_TICKS_PER_FRAME);
        assert_eq!(
            app.world().resource::<Time<Fixed>>().overstep(),
            Duration::ZERO,
            "the remaining overstep was discarded, not carried over"
        );
        // The next frame starts from a clean budget: 100 ms = 2 ticks.
        advance(&mut app, Duration::from_millis(100));
        assert_eq!(
            app.world().non_send::<SimHandle>().tick(),
            MAX_TICKS_PER_FRAME + 2
        );
        let stats = *app.world().resource::<DriverStats>();
        assert_eq!(stats.ticks_this_frame, 2);
        assert_eq!(stats.ticks_last_frame, MAX_TICKS_PER_FRAME);
        assert_eq!(stats.dropped_ticks, 20 - MAX_TICKS_PER_FRAME, "cumulative");
    }

    #[test]
    fn a_stalling_source_steps_nothing_and_counts_the_stall() {
        struct Never;
        impl InputSource for Never {
            fn commands_for(
                &mut self,
                _tick: u32,
                _local: &mut PendingCommands,
            ) -> Option<Vec<PlayerCommand>> {
                None
            }
            fn name(&self) -> &'static str {
                "never"
            }
        }
        let mut app = fixed_clock_app(Box::new(Never));
        advance(&mut app, Duration::from_millis(100));
        assert_eq!(app.world().non_send::<SimHandle>().tick(), 0);
        let stats = *app.world().resource::<DriverStats>();
        assert_eq!(stats.stalled_ticks, 2);
        assert_eq!(stats.ticks_this_frame, 0);
        assert_eq!(stats.dropped_ticks, 0);
        assert!(!app.world().resource::<SimStepped>().0);
    }

    #[test]
    fn replay_file_names_are_sortable_timestamps() {
        // 2026-10-06 14:03:09 UTC.
        assert_eq!(
            replay_file_name(42, 1_791_295_389),
            "20261006-140309-42.eonreplay"
        );
        assert_eq!(civil_from_unix(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil_from_unix(951_782_400), (2000, 2, 29, 0, 0, 0));
        let dir = replay_dir(Some(Path::new("/tmp/r")));
        assert_eq!(dir, PathBuf::from("/tmp/r"));
        assert!(
            new_replay_path(&dir, 1)
                .to_string_lossy()
                .ends_with("-1.eonreplay")
        );
        assert!(replay_dir(None).ends_with("replays"));
    }
}
