//! The `.eonreplay` file format, its writer and reader, and `verify`.
//! Specification: `docs/DETERMINISM.md`, "Replays".
//!
//! # Layout
//!
//! ```text
//! "EONR"                      4 magic bytes            (MAGIC)
//! u16 little-endian           format version, 1       (FORMAT_VERSION)
//! postcard(MatchSetup)        header
//! postcard(Record)*           stream, no framing
//! "EONRDONE"                  8-byte clean-exit trailer (TRAILER), only
//!                             when the writer `finish`ed
//! ```
//!
//! Records are [`Record::Tick`] for every tick that had commands (the
//! commands handed to `Sim::step` for that tick, before the command delay)
//! and [`Record::Hash`] after every `hash_every` ticks (20 by default, 1 under
//! `--hash-every-tick`) plus one final hash at clean exit. Records are
//! written in tick order; a tick's `Tick` record precedes its `Hash` record.
//!
//! The reader tolerates truncation (`kill -9`): it decodes records with
//! `postcard::take_from_bytes` and stops at the first record that does not
//! decode, keeping everything before it. Postcard encodings are
//! self-delimiting and prefix-free per type, so a cut-off tail never decodes
//! as a shorter valid record. The trailer tells truncation apart from
//! corruption: a file that ends with [`TRAILER`] was closed cleanly, so any
//! undecodable bytes before the trailer are damage, not a cut, and the
//! reader returns [`ReplayError::Corrupt`] instead of silently dropping the
//! rest of the stream. A file without the trailer is a truncated recording:
//! every complete record is kept, [`ReplayFile::truncated`] is set and the
//! CLI prints a warning while `verify` still reports the ticks it checked.
//!
//! # Writer
//!
//! [`ReplayWriter`] wraps a `BufWriter<File>`. `tick` and `hash` append
//! records; `flush` writes the buffer (the game calls it every 20 ticks);
//! `finish` appends the trailer, flushes and `sync_all`s once at clean exit.
//! No fsync during play (Apple `F_FULLFSYNC` cost). The dedicated writer
//! thread and channel live in the game crate (M2); this type is the sink.
//!
//! # Verify
//!
//! [`verify`] reads the file, fails fast on `sim_version` mismatch, loads
//! the rules from `data_dir` and compares `rules_hash`, then re-simulates:
//! for each tick from 0, `step` with that tick's recorded commands (none
//! when no `Tick` record exists), and at every `Hash` record compares the
//! whole hash; on mismatch the first differing field of
//! [`SubHashes`] names the subsystem. The result is exactly one
//! [`VerifyOutcome`], whose `Display` is the string the CLI prints.

use crate::ai_hook::AiController;
use crate::command::PlayerCommand;
use crate::state::{MatchSetup, SIM_VERSION, Sim, SubHashes};
use rules::Rules;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// File magic.
pub const MAGIC: [u8; 4] = *b"EONR";
/// Clean-exit trailer appended by [`ReplayWriter::finish`]. Its first byte
/// (`E`, 0x45) is not a valid [`Record`] tag, so the record loop can never
/// read it as a record.
pub const TRAILER: [u8; 8] = *b"EONRDONE";
/// Format version written by this build.
pub const FORMAT_VERSION: u16 = 1;
/// Default interval between hash records, in ticks.
pub const DEFAULT_HASH_EVERY: u32 = 20;

/// The decoded fixed-size prefix plus header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayHeader {
    /// Always [`FORMAT_VERSION`] for files this build writes.
    pub format_version: u16,
    /// Everything needed to reproduce the match from tick 0.
    pub setup: MatchSetup,
}

/// The commands handed to `Sim::step` on one tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickBatch {
    /// Tick the commands were issued on (the `step` call index).
    pub tick: u32,
    /// Commands in the order they were passed to `step`.
    pub cmds: Vec<PlayerCommand>,
}

/// The state hash after `tick` completed steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HashRecord {
    /// `Sim::tick()` when the hash was taken.
    pub tick: u32,
    /// `Sim::hash()`.
    pub hash: u64,
    /// `Sim::sub_hashes()`, so `verify` can name the diverging subsystem.
    pub sub: SubHashes,
}

/// One record of the stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Record {
    /// Commands for a tick.
    Tick(TickBatch),
    /// A hash checkpoint.
    Hash(HashRecord),
}

/// Errors reading or writing a replay. Sim divergence is not an error; it
/// is a [`VerifyOutcome`].
#[derive(Debug)]
pub enum ReplayError {
    /// File system failure.
    Io {
        /// File involved.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// The file does not start with [`MAGIC`].
    BadMagic {
        /// File involved.
        path: PathBuf,
    },
    /// The format version is not one this build reads.
    UnsupportedVersion {
        /// File involved.
        path: PathBuf,
        /// Version found in the file.
        found: u16,
    },
    /// The header was written by a different `SIM_VERSION`. Detected by
    /// peeking the header's first field before decoding the rest, because
    /// the header layout itself changes between versions (M3a appended
    /// `MatchSetup::start`): decoding an older header with this build's
    /// layout would misread the record stream. [`verify`] and
    /// [`hash_dump`] report it as [`VerifyOutcome::SimVersionMismatch`].
    SimVersion {
        /// File involved.
        path: PathBuf,
        /// `sim_version` in the file's header.
        found: u32,
    },
    /// The header did not decode (the file is shorter than a header or corrupt).
    Header {
        /// File involved.
        path: PathBuf,
        /// Decoder error.
        source: postcard::Error,
    },
    /// A record could not be encoded (writer side; should not happen).
    Encode(postcard::Error),
    /// The rules under `data_dir` did not load.
    Rules(rules::Error),
    /// A record's tick lies before the tick the stream had already reached.
    /// Records are written in tick order; this file was spliced or not
    /// written by [`ReplayWriter`].
    OutOfOrder {
        /// File involved.
        path: PathBuf,
        /// Tick of the offending record.
        tick: u32,
        /// Tick the re-simulation had already reached.
        reached: u32,
    },
    /// The file ends with the clean-exit [`TRAILER`] but a record before it
    /// does not decode: the stream was damaged after it was written (a
    /// truncated file has no trailer and is not an error).
    Corrupt {
        /// File involved.
        path: PathBuf,
        /// Byte offset of the first record that does not decode.
        offset: usize,
        /// Bytes between that record and the trailer.
        undecoded: usize,
    },
    /// The controller factory of [`verify_with`] or [`hash_dump_with`]
    /// refused the header's AI seats (an unknown bot or personality id, or
    /// seats that do not match the AI slots; M5b). Reported after the
    /// version and rules checks, before anything is simulated.
    Controller(String),
}

impl core::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ReplayError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            ReplayError::BadMagic { path } => {
                write!(f, "{}: not an .eonreplay file (bad magic)", path.display())
            }
            ReplayError::UnsupportedVersion { path, found } => write!(
                f,
                "{}: unsupported replay format version {found} (this build reads {FORMAT_VERSION})",
                path.display()
            ),
            ReplayError::SimVersion { path, found } => write!(
                f,
                "{}: {}",
                path.display(),
                VerifyOutcome::SimVersionMismatch {
                    replay: *found,
                    binary: SIM_VERSION,
                }
            ),
            ReplayError::Header { path, source } => {
                write!(
                    f,
                    "{}: replay header does not decode: {source}",
                    path.display()
                )
            }
            ReplayError::Encode(e) => write!(f, "replay record does not encode: {e}"),
            ReplayError::Rules(e) => write!(f, "{e}"),
            ReplayError::OutOfOrder {
                path,
                tick,
                reached,
            } => write!(
                f,
                "{}: record for tick {tick} after the stream reached tick {reached}",
                path.display()
            ),
            ReplayError::Corrupt {
                path,
                offset,
                undecoded,
            } => write!(
                f,
                "{}: corrupt record stream: {undecoded} undecodable bytes at offset {offset} before the clean-exit trailer",
                path.display()
            ),
            ReplayError::Controller(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ReplayError {}

impl From<rules::Error> for ReplayError {
    fn from(e: rules::Error) -> ReplayError {
        ReplayError::Rules(e)
    }
}

/// Appends records to an `.eonreplay` file. See the module docs.
#[derive(Debug)]
pub struct ReplayWriter {
    path: PathBuf,
    out: BufWriter<File>,
    records: u64,
}

impl ReplayWriter {
    /// Create (truncate) `path` and write the magic, version and header.
    pub fn create(path: &Path, setup: &MatchSetup) -> Result<ReplayWriter, ReplayError> {
        let io = |source| ReplayError::Io {
            path: path.to_path_buf(),
            source,
        };
        let file = File::create(path).map_err(io)?;
        let mut out = BufWriter::new(file);
        out.write_all(&MAGIC).map_err(io)?;
        out.write_all(&FORMAT_VERSION.to_le_bytes()).map_err(io)?;
        let header = postcard::to_allocvec(setup).map_err(ReplayError::Encode)?;
        out.write_all(&header).map_err(io)?;
        Ok(ReplayWriter {
            path: path.to_path_buf(),
            out,
            records: 0,
        })
    }

    fn record(&mut self, record: &Record) -> Result<(), ReplayError> {
        let bytes = postcard::to_allocvec(record).map_err(ReplayError::Encode)?;
        self.out
            .write_all(&bytes)
            .map_err(|source| ReplayError::Io {
                path: self.path.clone(),
                source,
            })?;
        self.records += 1;
        Ok(())
    }

    /// Append a [`Record::Tick`]. A no-op when `cmds` is empty.
    pub fn tick(&mut self, tick: u32, cmds: &[PlayerCommand]) -> Result<(), ReplayError> {
        if cmds.is_empty() {
            return Ok(());
        }
        self.record(&Record::Tick(TickBatch {
            tick,
            cmds: cmds.to_vec(),
        }))
    }

    /// Append a [`Record::Hash`].
    pub fn hash(&mut self, tick: u32, hash: u64, sub: &SubHashes) -> Result<(), ReplayError> {
        self.record(&Record::Hash(HashRecord {
            tick,
            hash,
            sub: *sub,
        }))
    }

    /// Write buffered bytes to the OS (no fsync).
    pub fn flush(&mut self) -> Result<(), ReplayError> {
        self.out.flush().map_err(|source| ReplayError::Io {
            path: self.path.clone(),
            source,
        })
    }

    /// Append the clean-exit [`TRAILER`], flush and `sync_all`; consumes the
    /// writer. Call once at clean exit; a recording that is killed never
    /// reaches it, which is how the reader knows the file is truncated.
    pub fn finish(mut self) -> Result<(), ReplayError> {
        self.out
            .write_all(&TRAILER)
            .map_err(|source| ReplayError::Io {
                path: self.path.clone(),
                source,
            })?;
        self.flush()?;
        self.out
            .get_ref()
            .sync_all()
            .map_err(|source| ReplayError::Io {
                path: self.path.clone(),
                source,
            })
    }

    /// File being written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Records appended so far.
    pub fn records(&self) -> u64 {
        self.records
    }
}

/// A decoded replay: the header, every complete record, and whether the
/// recording was closed cleanly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayFile {
    /// The header.
    pub setup: MatchSetup,
    /// Every record that decoded, in file order.
    pub records: Vec<Record>,
    /// `true` when the file does not end with [`TRAILER`]: the recording was
    /// cut (`kill -9`, crash, disk full) and `records` holds everything up
    /// to the last complete record. `false` means the writer `finish`ed and
    /// every record decoded.
    pub truncated: bool,
    /// Bytes after the last complete record that did not decode (a partial
    /// record left by the cut). Always 0 when `truncated` is `false`.
    pub undecoded_bytes: usize,
}

/// Reads an `.eonreplay` file, tolerating a truncated tail.
#[derive(Debug)]
pub struct ReplayReader;

impl ReplayReader {
    /// Read the whole file: header (magic and version checked) and every
    /// complete record. A partial final record without a trailer is dropped
    /// and reported through [`ReplayFile::truncated`]; an undecodable record
    /// in a file that has the trailer is [`ReplayError::Corrupt`].
    pub fn open(path: &Path) -> Result<ReplayFile, ReplayError> {
        let bytes = std::fs::read(path).map_err(|source| ReplayError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        ReplayReader::decode(path, &bytes)
    }

    /// Decode from bytes already in memory (the body of `open`; tests use it).
    pub fn decode(path: &Path, bytes: &[u8]) -> Result<ReplayFile, ReplayError> {
        if bytes.len() < MAGIC.len() || bytes[..MAGIC.len()] != MAGIC {
            return Err(ReplayError::BadMagic {
                path: path.to_path_buf(),
            });
        }
        let rest = &bytes[MAGIC.len()..];
        let Some((version, rest)) = rest.split_first_chunk::<2>() else {
            return Err(ReplayError::Header {
                path: path.to_path_buf(),
                source: postcard::Error::DeserializeUnexpectedEnd,
            });
        };
        let found = u16::from_le_bytes(*version);
        if found != FORMAT_VERSION {
            return Err(ReplayError::UnsupportedVersion {
                path: path.to_path_buf(),
                found,
            });
        }
        // A clean exit ends the file with the trailer; strip it before the
        // record loop so the loop sees exactly the record stream.
        let finished = rest.ends_with(&TRAILER);
        let stream = if finished {
            &rest[..rest.len() - TRAILER.len()]
        } else {
            rest
        };
        // `sim_version` is the header's first field; a file from another
        // version may lay out the rest of the header differently.
        if let Ok((found, _)) = postcard::take_from_bytes::<u32>(stream)
            && found != SIM_VERSION
        {
            return Err(ReplayError::SimVersion {
                path: path.to_path_buf(),
                found,
            });
        }
        let (setup, mut rest): (MatchSetup, &[u8]) =
            postcard::take_from_bytes(stream).map_err(|source| ReplayError::Header {
                path: path.to_path_buf(),
                source,
            })?;
        let mut records = Vec::new();
        // Stop at the first record that does not decode: a `kill -9` leaves a
        // partial final record, which is dropped with everything after it.
        while let Ok((record, tail)) = postcard::take_from_bytes::<Record>(rest) {
            records.push(record);
            rest = tail;
        }
        let undecoded_bytes = rest.len();
        if finished && undecoded_bytes > 0 {
            return Err(ReplayError::Corrupt {
                path: path.to_path_buf(),
                offset: bytes.len() - TRAILER.len() - undecoded_bytes,
                undecoded: undecoded_bytes,
            });
        }
        Ok(ReplayFile {
            setup,
            records,
            truncated: !finished,
            undecoded_bytes,
        })
    }
}

/// Result of [`verify`]. `Display` yields exactly the line the CLI prints;
/// [`VerifyOutcome::exit_code`] is the process exit status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// Every recorded hash was reproduced.
    Ok {
        /// Hash at the last hash record.
        final_hash: u64,
        /// Ticks re-simulated (the tick of the last hash record).
        ticks: u32,
    },
    /// A recorded hash did not match.
    Diverged {
        /// Tick of the first mismatching hash record.
        tick: u32,
        /// Name of the first differing sub-hash (see [`SubHashes::first_difference`]).
        subsystem: &'static str,
    },
    /// `header.sim_version != SIM_VERSION`; nothing was simulated.
    SimVersionMismatch {
        /// Version in the replay.
        replay: u32,
        /// `crate::SIM_VERSION`.
        binary: u32,
    },
    /// `header.rules_hash != Rules::load(data_dir).rules_hash()`.
    RulesChanged {
        /// Hash in the replay.
        replay: u64,
        /// Hash of the loaded rules.
        loaded: u64,
    },
}

impl VerifyOutcome {
    /// Process exit status: 0 for `Ok`, 1 otherwise.
    pub fn exit_code(&self) -> u8 {
        match self {
            VerifyOutcome::Ok { .. } => 0,
            _ => 1,
        }
    }
}

impl core::fmt::Display for VerifyOutcome {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            VerifyOutcome::Ok { final_hash, ticks } => {
                write!(f, "OK final_hash=0x{final_hash:016x} ticks={ticks}")
            }
            VerifyOutcome::Diverged { tick, subsystem } => {
                write!(f, "DIVERGED at tick {tick} (subsystem: {subsystem})")
            }
            VerifyOutcome::SimVersionMismatch { replay, binary } => {
                write!(f, "SIM VERSION MISMATCH (replay {replay}, binary {binary})")
            }
            VerifyOutcome::RulesChanged { replay, loaded } => write!(
                f,
                "RULES CHANGED since recording (replay 0x{replay:016x}, loaded 0x{loaded:016x})"
            ),
        }
    }
}

/// What [`verify_with`] and [`hash_dump_with`] call, once the header's
/// version and rules have passed, to build the controller for the header's
/// AI seats (`ai::controller_for` in sim-cli and the game). An `Err` is
/// [`ReplayError::Controller`].
pub type ControllerFactory<'a> =
    dyn FnOnce(&MatchSetup, &Rules) -> Result<Box<dyn AiController>, String> + 'a;

/// Re-simulate `path` against the rules under `data_dir` (see the module
/// docs). `ai` drives every slot with `is_ai == true` exactly as during the
/// recording; scenario recordings have no AI slots and pass a no-op bot.
/// [`verify_with`] builds the controller from the header instead.
pub fn verify(
    path: &Path,
    data_dir: &Path,
    ai: Box<dyn AiController>,
) -> Result<VerifyOutcome, ReplayError> {
    verify_with(
        path,
        data_dir,
        Box::new(move |_: &MatchSetup, _: &Rules| Ok(ai)),
    )
}

/// [`verify`] with the controller built by `factory` from the header and
/// the loaded rules (M5b), after the version and rules checks: a header
/// naming bots re-simulates with those bots. The four outcome lines are
/// unchanged; a factory error is [`ReplayError::Controller`].
pub fn verify_with(
    path: &Path,
    data_dir: &Path,
    factory: Box<ControllerFactory<'_>>,
) -> Result<VerifyOutcome, ReplayError> {
    let ReplayFile { setup, records, .. } = match open_for_check(path)? {
        Ok(file) => file,
        Err(outcome) => return Ok(outcome),
    };
    let mut sim = match prepare(setup, data_dir, factory)? {
        Ok(sim) => sim,
        Err(outcome) => return Ok(outcome),
    };
    let mut last_hash: Option<(u32, u64)> = None;
    for record in &records {
        let tick = record_tick(record);
        advance_to(&mut sim, tick, path)?;
        match record {
            Record::Tick(batch) => sim.step(&batch.cmds),
            Record::Hash(rec) => {
                if sim.hash() != rec.hash {
                    let subsystem = sim
                        .sub_hashes()
                        .first_difference(&rec.sub)
                        .unwrap_or("state");
                    return Ok(VerifyOutcome::Diverged {
                        tick: rec.tick,
                        subsystem,
                    });
                }
                last_hash = Some((rec.tick, rec.hash));
            }
        }
    }
    Ok(match last_hash {
        Some((ticks, final_hash)) => VerifyOutcome::Ok { final_hash, ticks },
        None => VerifyOutcome::Ok {
            final_hash: sim.hash(),
            ticks: sim.tick(),
        },
    })
}

/// Re-simulate `path` like [`verify`] without comparing hashes, calling
/// `emit(tick, hash, sub_hashes)` after every step whose resulting tick is a
/// multiple of `every` (at least 1) and once more at the end. The run ends
/// at the larger of the last hash record's tick and one past the last tick
/// batch. Version and rules mismatches are returned as in [`verify`]; a
/// completed dump returns `Ok` with the re-simulation's own final hash and
/// tick. `sim-cli hash-dump <replay> --every N` prints the emitted lines;
/// `docs/DETERMINISM.md` explains bisecting with them.
pub fn hash_dump(
    path: &Path,
    data_dir: &Path,
    ai: Box<dyn AiController>,
    every: u32,
    emit: impl FnMut(u32, u64, &SubHashes),
) -> Result<VerifyOutcome, ReplayError> {
    hash_dump_with(
        path,
        data_dir,
        Box::new(move |_: &MatchSetup, _: &Rules| Ok(ai)),
        every,
        emit,
    )
}

/// [`hash_dump`] with the controller built by `factory` from the header
/// and the loaded rules, like [`verify_with`] (M5b).
pub fn hash_dump_with(
    path: &Path,
    data_dir: &Path,
    factory: Box<ControllerFactory<'_>>,
    every: u32,
    mut emit: impl FnMut(u32, u64, &SubHashes),
) -> Result<VerifyOutcome, ReplayError> {
    let ReplayFile { setup, records, .. } = match open_for_check(path)? {
        Ok(file) => file,
        Err(outcome) => return Ok(outcome),
    };
    let mut sim = match prepare(setup, data_dir, factory)? {
        Ok(sim) => sim,
        Err(outcome) => return Ok(outcome),
    };
    let every = every.max(1);
    let end = records
        .iter()
        .map(|r| match r {
            Record::Tick(b) => b.tick.saturating_add(1),
            Record::Hash(h) => h.tick,
        })
        .max()
        .unwrap_or(0);
    let mut batches = records.iter().filter_map(|r| match r {
        Record::Tick(b) => Some(b),
        Record::Hash(_) => None,
    });
    let mut next = batches.next();
    while sim.tick() < end {
        let tick = sim.tick();
        match next {
            Some(b) if b.tick < tick => {
                return Err(ReplayError::OutOfOrder {
                    path: path.to_path_buf(),
                    tick: b.tick,
                    reached: tick,
                });
            }
            Some(b) if b.tick == tick => {
                sim.step(&b.cmds);
                next = batches.next();
            }
            _ => sim.step(&[]),
        }
        let now = sim.tick();
        if now.is_multiple_of(every) || now == end {
            emit(now, sim.hash(), &sim.sub_hashes());
        }
    }
    if let Some(b) = next {
        // Only a batch for a tick already passed can remain once `end` is reached.
        return Err(ReplayError::OutOfOrder {
            path: path.to_path_buf(),
            tick: b.tick,
            reached: sim.tick(),
        });
    }
    Ok(VerifyOutcome::Ok {
        final_hash: sim.hash(),
        ticks: sim.tick(),
    })
}

/// Open `path` for [`verify`] or [`hash_dump`]: a header from another
/// `SIM_VERSION` is the outcome [`VerifyOutcome::SimVersionMismatch`], not
/// an error.
fn open_for_check(path: &Path) -> Result<Result<ReplayFile, VerifyOutcome>, ReplayError> {
    match ReplayReader::open(path) {
        Ok(file) => Ok(Ok(file)),
        Err(ReplayError::SimVersion { found, .. }) => Ok(Err(VerifyOutcome::SimVersionMismatch {
            replay: found,
            binary: SIM_VERSION,
        })),
        Err(e) => Err(e),
    }
}

/// The checks shared by [`verify_with`] and [`hash_dump_with`]:
/// `sim_version` before anything is loaded, then the rules and their hash,
/// then the controller from `factory`. `Ok(Err(outcome))` is a mismatch;
/// `Ok(Ok(sim))` is a fresh sim at tick 0.
fn prepare(
    setup: MatchSetup,
    data_dir: &Path,
    factory: Box<ControllerFactory<'_>>,
) -> Result<Result<Sim, VerifyOutcome>, ReplayError> {
    if setup.sim_version != SIM_VERSION {
        return Ok(Err(VerifyOutcome::SimVersionMismatch {
            replay: setup.sim_version,
            binary: SIM_VERSION,
        }));
    }
    let rules = Rules::load(data_dir)?;
    if rules.rules_hash() != setup.rules_hash {
        return Ok(Err(VerifyOutcome::RulesChanged {
            replay: setup.rules_hash,
            loaded: rules.rules_hash(),
        }));
    }
    let ai = factory(&setup, &rules).map_err(ReplayError::Controller)?;
    Ok(Ok(Sim::new(setup, rules, ai)))
}

fn record_tick(record: &Record) -> u32 {
    match record {
        Record::Tick(b) => b.tick,
        Record::Hash(h) => h.tick,
    }
}

/// Step with empty command lists until `sim.tick() == tick`.
fn advance_to(sim: &mut Sim, tick: u32, path: &Path) -> Result<(), ReplayError> {
    if tick < sim.tick() {
        return Err(ReplayError::OutOfOrder {
            path: path.to_path_buf(),
            tick,
            reached: sim.tick(),
        });
    }
    while sim.tick() < tick {
        sim.step(&[]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_outcome_strings_and_exit_codes_are_exact() {
        let ok = VerifyOutcome::Ok {
            final_hash: 0x0123_4567_89ab_cdef,
            ticks: 1200,
        };
        assert_eq!(
            ok.to_string(),
            "OK final_hash=0x0123456789abcdef ticks=1200"
        );
        assert_eq!(ok.exit_code(), 0);
        let d = VerifyOutcome::Diverged {
            tick: 40,
            subsystem: "units",
        };
        assert_eq!(d.to_string(), "DIVERGED at tick 40 (subsystem: units)");
        assert_eq!(d.exit_code(), 1);
        let v = VerifyOutcome::SimVersionMismatch {
            replay: 1,
            binary: 2,
        };
        assert_eq!(v.to_string(), "SIM VERSION MISMATCH (replay 1, binary 2)");
        assert_eq!(v.exit_code(), 1);
        let r = VerifyOutcome::RulesChanged {
            replay: 1,
            loaded: 2,
        };
        assert_eq!(
            r.to_string(),
            "RULES CHANGED since recording (replay 0x0000000000000001, loaded 0x0000000000000002)"
        );
        assert_eq!(r.exit_code(), 1);
    }

    #[test]
    fn records_round_trip_through_postcard_and_take_from_bytes() {
        let a = Record::Tick(TickBatch {
            tick: 3,
            cmds: vec![PlayerCommand::new(
                crate::ids::PlayerId(0),
                1,
                crate::command::Command::Surrender,
            )],
        });
        let b = Record::Hash(HashRecord {
            tick: 20,
            hash: 7,
            sub: SubHashes::default(),
        });
        let mut bytes = postcard::to_allocvec(&a).unwrap();
        bytes.extend(postcard::to_allocvec(&b).unwrap());
        let (ra, rest): (Record, &[u8]) = postcard::take_from_bytes(&bytes).unwrap();
        let (rb, rest): (Record, &[u8]) = postcard::take_from_bytes(rest).unwrap();
        assert_eq!(ra, a);
        assert_eq!(rb, b);
        assert!(rest.is_empty());
        // A truncated tail does not decode.
        let cut = &bytes[..bytes.len() - 3];
        let (_, rest): (Record, &[u8]) = postcard::take_from_bytes(cut).unwrap();
        assert!(postcard::take_from_bytes::<Record>(rest).is_err());
    }

    // ---- end-to-end tests over a scenario the M1 contract sim runs today --

    use crate::command::Command;
    use crate::fx::FxVec2;
    use crate::ids::{PlayerId, UnitId, UnitKindId};

    use crate::ai_hook::NoAi;

    fn data_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eonmark-replay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{name}.eonreplay"))
    }

    fn scenario_setup(seed: u64) -> (MatchSetup, Rules) {
        let rules = Rules::load(data_dir()).expect("data/ loads");
        (MatchSetup::scenario(&rules, seed), rules)
    }

    const SPAWN_COUNT: u16 = 3;

    /// Spawn, a not-yet-implemented command (advances the rng) and a Stop:
    /// everything the contract sim applies without the pathing and movement
    /// bodies (fixtures with Move orders are recorded by sim-cli once those
    /// land).
    fn scenario_commands(tick: u32) -> Vec<PlayerCommand> {
        match tick {
            0 => vec![PlayerCommand::new(
                PlayerId(0),
                0,
                Command::DebugSpawn {
                    owner: PlayerId(0),
                    kind: UnitKindId(0),
                    at: FxVec2::from_ints(24, 64),
                    count: SPAWN_COUNT,
                },
            )],
            10 => vec![PlayerCommand::new(
                PlayerId(0),
                1,
                Command::AttackMove {
                    units: vec![UnitId(1)],
                    target: FxVec2::from_ints(100, 64),
                },
            )],
            30 => vec![PlayerCommand::new(
                PlayerId(1),
                0,
                Command::Stop {
                    units: vec![UnitId(2)],
                },
            )],
            _ => Vec::new(),
        }
    }

    /// Record `ticks` ticks of the scenario with a hash every `hash_every`
    /// ticks plus the final one; returns the recorded `(tick, hash)` pairs.
    fn record(path: &Path, setup: MatchSetup, ticks: u32, hash_every: u32) -> Vec<(u32, u64)> {
        let rules = Rules::load(data_dir()).unwrap();
        let mut sim = Sim::new(setup.clone(), rules, Box::new(NoAi));
        let mut w = ReplayWriter::create(path, &setup).unwrap();
        let mut hashes = Vec::new();
        for t in 0..ticks {
            let cmds = scenario_commands(t);
            w.tick(t, &cmds).unwrap();
            sim.step(&cmds);
            if sim.tick().is_multiple_of(hash_every) || sim.tick() == ticks {
                w.hash(sim.tick(), sim.hash(), &sim.sub_hashes()).unwrap();
                hashes.push((sim.tick(), sim.hash()));
            }
            if sim.tick().is_multiple_of(DEFAULT_HASH_EVERY) {
                w.flush().unwrap();
            }
        }
        w.finish().unwrap();
        hashes
    }

    #[test]
    fn writer_and_reader_round_trip() {
        let path = temp_path("round_trip");
        let (setup, _) = scenario_setup(5);
        let hashes = record(&path, setup.clone(), 50, 20);
        assert_eq!(hashes.iter().map(|h| h.0).collect::<Vec<_>>(), [20, 40, 50]);
        let file = ReplayReader::open(&path).unwrap();
        assert_eq!(file.setup, setup);
        assert!(!file.truncated, "finish() wrote the trailer");
        assert_eq!(file.undecoded_bytes, 0);
        let records = file.records;
        let ticks: Vec<u32> = records
            .iter()
            .filter_map(|r| match r {
                Record::Tick(b) => Some(b.tick),
                Record::Hash(_) => None,
            })
            .collect();
        assert_eq!(ticks, [0, 10, 30]);
        let recorded: Vec<(u32, u64)> = records
            .iter()
            .filter_map(|r| match r {
                Record::Hash(h) => Some((h.tick, h.hash)),
                Record::Tick(_) => None,
            })
            .collect();
        assert_eq!(recorded, hashes);
        // Tick order: Hash(20) sits between Tick(10) and Tick(30).
        let order: Vec<u32> = records.iter().map(record_tick).collect();
        assert!(order.windows(2).all(|w| w[0] <= w[1]), "{order:?}");
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], &MAGIC);
        assert_eq!(bytes[4..6], FORMAT_VERSION.to_le_bytes());
        assert!(bytes.ends_with(&TRAILER));
    }

    #[test]
    fn reader_tolerates_truncation_mid_record() {
        let path = temp_path("truncated");
        let (setup, _) = scenario_setup(5);
        record(&path, setup, 60, 20);
        let bytes = std::fs::read(&path).unwrap();
        let full = ReplayReader::decode(&path, &bytes).unwrap().records;
        // The last record is Hash(60), followed by the trailer; drop the
        // trailer (a kill between the last record and finish) and then cut
        // the record in half.
        let last = postcard::to_allocvec(full.last().unwrap()).unwrap();
        let stream = &bytes[..bytes.len() - TRAILER.len()];
        assert!(stream.ends_with(&last));
        let cut = &stream[..stream.len() - last.len() / 2];
        let file = ReplayReader::decode(&path, cut).unwrap();
        assert_eq!(file.records.len(), full.len() - 1);
        assert_eq!(file.records, full[..full.len() - 1]);
        assert!(file.truncated);
        assert_eq!(file.undecoded_bytes, last.len() - last.len() / 2);
        // Cut at a record boundary: still every complete record, no partial.
        let cut = &stream[..stream.len() - last.len()];
        let file = ReplayReader::decode(&path, cut).unwrap();
        assert_eq!(file.records, full[..full.len() - 1]);
        assert!(file.truncated);
        assert_eq!(file.undecoded_bytes, 0);
        // Killed right after the last record, before `finish`: complete
        // stream, still reported as truncated.
        let file = ReplayReader::decode(&path, stream).unwrap();
        assert_eq!(file.records, full);
        assert!(file.truncated);
        assert_eq!(file.undecoded_bytes, 0);
        // Cut inside the header.
        let err = ReplayReader::decode(&path, &bytes[..10]).unwrap_err();
        assert!(matches!(err, ReplayError::Header { .. }), "{err}");
        // Shorter than the version field.
        let err = ReplayReader::decode(&path, &bytes[..5]).unwrap_err();
        assert!(matches!(err, ReplayError::Header { .. }), "{err}");
        // Bad magic and an unsupported version.
        let err = ReplayReader::decode(&path, b"EONX").unwrap_err();
        assert!(matches!(err, ReplayError::BadMagic { .. }), "{err}");
        let err = ReplayReader::decode(&path, b"EO").unwrap_err();
        assert!(matches!(err, ReplayError::BadMagic { .. }), "{err}");
        let mut v2 = bytes.clone();
        v2[4..6].copy_from_slice(&2u16.to_le_bytes());
        let err = ReplayReader::decode(&path, &v2).unwrap_err();
        assert!(
            matches!(err, ReplayError::UnsupportedVersion { found: 2, .. }),
            "{err}"
        );
    }

    #[test]
    fn corruption_before_the_trailer_is_an_error_not_truncation() {
        let path = temp_path("corrupt");
        let (setup, _) = scenario_setup(5);
        record(&path, setup, 60, 20);
        let bytes = std::fs::read(&path).unwrap();
        let full = ReplayReader::decode(&path, &bytes).unwrap();
        // Overwrite the enum tag of the first record (right after the
        // header) with a value that is not a Record variant.
        let header_len = postcard::to_allocvec(&full.setup).unwrap().len();
        let first = MAGIC.len() + 2 + header_len;
        let mut bad = bytes.clone();
        bad[first] = 0x09;
        let err = ReplayReader::decode(&path, &bad).unwrap_err();
        match err {
            ReplayError::Corrupt {
                offset, undecoded, ..
            } => {
                assert_eq!(offset, first);
                assert_eq!(undecoded, bytes.len() - TRAILER.len() - first);
            }
            other => panic!("{other}"),
        }
        assert!(err.to_string().contains("corrupt record stream"), "{err}");
        // The same damage on a file without the trailer is read as a
        // truncation at that record (the reader cannot tell them apart).
        let cut = &bad[..bad.len() - TRAILER.len()];
        let file = ReplayReader::decode(&path, cut).unwrap();
        assert!(file.records.is_empty());
        assert!(file.truncated);
        assert_eq!(file.undecoded_bytes, cut.len() - first);
        // Damage in the middle: everything before it is kept in the
        // truncated reading, and the trailer makes it an error.
        let hash40 = postcard::to_allocvec(
            full.records
                .iter()
                .find(|r| matches!(r, Record::Hash(h) if h.tick == 40))
                .unwrap(),
        )
        .unwrap();
        let at = bytes
            .windows(hash40.len())
            .position(|w| w == hash40.as_slice())
            .unwrap();
        let mut bad = bytes.clone();
        bad[at] = 0x09;
        let err = ReplayReader::decode(&path, &bad).unwrap_err();
        assert!(
            matches!(err, ReplayError::Corrupt { offset, .. } if offset == at),
            "{err}"
        );
        let err = verify(&path, &data_dir(), Box::new(NoAi));
        assert!(err.is_ok(), "the undamaged file still verifies");
        let bad_path = temp_path("corrupt_copy");
        std::fs::write(&bad_path, &bad).unwrap();
        let err = verify(&bad_path, &data_dir(), Box::new(NoAi)).unwrap_err();
        assert!(matches!(err, ReplayError::Corrupt { .. }), "{err}");
    }

    #[test]
    fn verify_reproduces_a_recording_and_a_truncated_copy() {
        let path = temp_path("verify_ok");
        let (setup, _) = scenario_setup(9);
        let hashes = record(&path, setup, 70, 20);
        let outcome = verify(&path, &data_dir(), Box::new(NoAi)).unwrap();
        let (ticks, final_hash) = *hashes.last().unwrap();
        assert_eq!(outcome, VerifyOutcome::Ok { final_hash, ticks });
        assert_eq!(ticks, 70);
        assert_eq!(outcome.exit_code(), 0);
        // Truncate mid-record (as `kill -9` would, so no trailer): the file
        // verifies up to the last complete hash record.
        let bytes = std::fs::read(&path).unwrap();
        let cut_path = temp_path("verify_cut");
        std::fs::write(&cut_path, &bytes[..bytes.len() - TRAILER.len() - 5]).unwrap();
        assert!(ReplayReader::open(&cut_path).unwrap().truncated);
        let outcome = verify(&cut_path, &data_dir(), Box::new(NoAi)).unwrap();
        assert_eq!(
            outcome,
            VerifyOutcome::Ok {
                final_hash: hashes[2].1,
                ticks: 60
            }
        );
    }

    #[test]
    fn verify_reports_sim_version_mismatch_before_loading_rules() {
        let path = temp_path("sim_version");
        let (mut setup, _) = scenario_setup(1);
        setup.sim_version = SIM_VERSION + 1;
        let mut w = ReplayWriter::create(&path, &setup).unwrap();
        w.hash(0, 0, &SubHashes::default()).unwrap();
        w.finish().unwrap();
        // A data dir that does not exist proves the rules are never loaded.
        let outcome = verify(&path, Path::new("/nonexistent/data"), Box::new(NoAi)).unwrap();
        assert_eq!(
            outcome,
            VerifyOutcome::SimVersionMismatch {
                replay: SIM_VERSION + 1,
                binary: SIM_VERSION
            }
        );
        assert_eq!(outcome.exit_code(), 1);
    }

    /// A finished replay from `SIM_VERSION` 5, whose header has no `start`
    /// field: without the version peek the reader would decode `start` from
    /// the first record byte and report the file as corrupt (exit 2). With
    /// it, `verify` and `hash_dump` report the version mismatch (exit 1) and
    /// the reader names the version.
    #[test]
    fn older_sim_version_header_reports_sim_version_mismatch() {
        #[derive(Serialize)]
        struct SetupV5 {
            sim_version: u32,
            git_sha: String,
            rules_version: u32,
            rules_hash: u64,
            map: String,
            seed: u64,
            tick_rate_hz: u32,
            cmd_delay: u32,
            players: Vec<crate::state::PlayerSlot>,
            debug_commands: bool,
        }
        let (setup, _) = scenario_setup(1);
        let old = SetupV5 {
            sim_version: 5,
            git_sha: String::new(),
            rules_version: setup.rules_version,
            rules_hash: setup.rules_hash,
            map: setup.map.clone(),
            seed: setup.seed,
            tick_rate_hz: setup.tick_rate_hz,
            cmd_delay: setup.cmd_delay,
            players: setup.players.clone(),
            debug_commands: setup.debug_commands,
        };
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend(postcard::to_allocvec(&old).unwrap());
        bytes.extend(
            postcard::to_allocvec(&Record::Hash(HashRecord {
                tick: 0,
                hash: 1,
                sub: SubHashes::default(),
            }))
            .unwrap(),
        );
        bytes.extend_from_slice(&TRAILER);
        let path = temp_path("sim_version_v5");
        std::fs::write(&path, &bytes).unwrap();

        let err = ReplayReader::open(&path).unwrap_err();
        assert!(
            matches!(err, ReplayError::SimVersion { found: 5, .. }),
            "{err}"
        );
        assert!(err.to_string().contains("SIM VERSION MISMATCH"), "{err}");
        let want = VerifyOutcome::SimVersionMismatch {
            replay: 5,
            binary: SIM_VERSION,
        };
        let outcome = verify(&path, Path::new("/nonexistent/data"), Box::new(NoAi)).unwrap();
        assert_eq!(outcome, want);
        assert_eq!(outcome.exit_code(), 1);
        let dumped = hash_dump(
            &path,
            Path::new("/nonexistent/data"),
            Box::new(NoAi),
            1,
            |_, _, _| panic!("nothing is simulated"),
        )
        .unwrap();
        assert_eq!(dumped, want);
    }

    #[test]
    fn verify_reports_rules_changed() {
        let path = temp_path("rules_hash");
        let (mut setup, rules) = scenario_setup(1);
        setup.rules_hash ^= 0x1;
        let mut w = ReplayWriter::create(&path, &setup).unwrap();
        w.hash(0, 0, &SubHashes::default()).unwrap();
        w.finish().unwrap();
        let outcome = verify(&path, &data_dir(), Box::new(NoAi)).unwrap();
        assert_eq!(
            outcome,
            VerifyOutcome::RulesChanged {
                replay: setup.rules_hash,
                loaded: rules.rules_hash()
            }
        );
        assert_eq!(outcome.exit_code(), 1);
        // Missing data is an error, not an outcome.
        let err = verify(&path, Path::new("/nonexistent/data"), Box::new(NoAi)).unwrap_err();
        assert!(matches!(err, ReplayError::Rules(_)), "{err}");
    }

    #[test]
    fn verify_detects_a_flipped_command_byte() {
        let path = temp_path("diverged");
        let (setup, _) = scenario_setup(3);
        record(&path, setup, 60, 20);
        let mut bytes = std::fs::read(&path).unwrap();
        // Locate the spawn record and bump its `count` by one.
        let original = postcard::to_allocvec(&Record::Tick(TickBatch {
            tick: 0,
            cmds: scenario_commands(0),
        }))
        .unwrap();
        let mut flipped_cmds = scenario_commands(0);
        let Command::DebugSpawn { count, .. } = &mut flipped_cmds[0].cmd else {
            unreachable!()
        };
        *count += 1;
        let flipped = postcard::to_allocvec(&Record::Tick(TickBatch {
            tick: 0,
            cmds: flipped_cmds,
        }))
        .unwrap();
        assert_eq!(original.len(), flipped.len());
        let diff: Vec<usize> = (0..original.len())
            .filter(|&i| original[i] != flipped[i])
            .collect();
        assert_eq!(diff.len(), 1, "exactly one byte differs");
        let at = bytes
            .windows(original.len())
            .position(|w| w == original.as_slice())
            .expect("spawn record present");
        bytes[at + diff[0]] = flipped[diff[0]];
        let bad = temp_path("diverged_copy");
        std::fs::write(&bad, &bytes).unwrap();
        let outcome = verify(&bad, &data_dir(), Box::new(NoAi)).unwrap();
        assert_eq!(
            outcome,
            VerifyOutcome::Diverged {
                tick: 20,
                subsystem: "units"
            }
        );
        assert_eq!(outcome.exit_code(), 1);
        assert_eq!(
            outcome.to_string(),
            "DIVERGED at tick 20 (subsystem: units)"
        );
    }

    #[test]
    fn verify_without_hash_records_reports_the_last_tick() {
        let path = temp_path("no_hashes");
        let (setup, rules) = scenario_setup(2);
        let mut sim = Sim::new(setup.clone(), rules, Box::new(NoAi));
        let mut w = ReplayWriter::create(&path, &setup).unwrap();
        for t in 0..35 {
            let cmds = scenario_commands(t);
            w.tick(t, &cmds).unwrap();
            sim.step(&cmds);
        }
        w.finish().unwrap();
        // The last Tick record is at 30, so the stream reaches tick 31.
        let outcome = verify(&path, &data_dir(), Box::new(NoAi)).unwrap();
        let mut expect = Sim::new(setup, Rules::load(data_dir()).unwrap(), Box::new(NoAi));
        for t in 0..31 {
            expect.step(&scenario_commands(t));
        }
        assert_eq!(
            outcome,
            VerifyOutcome::Ok {
                final_hash: expect.hash(),
                ticks: 31
            }
        );
    }

    #[test]
    fn out_of_order_records_are_an_error() {
        let path = temp_path("out_of_order");
        let (setup, _) = scenario_setup(2);
        let mut w = ReplayWriter::create(&path, &setup).unwrap();
        w.tick(10, &scenario_commands(0)).unwrap();
        w.tick(2, &scenario_commands(0)).unwrap();
        w.finish().unwrap();
        // Tick(10) is stepped (the stream reaches 11), then Tick(2) is in the past.
        let err = verify(&path, &data_dir(), Box::new(NoAi)).unwrap_err();
        assert!(
            matches!(
                err,
                ReplayError::OutOfOrder {
                    tick: 2,
                    reached: 11,
                    ..
                }
            ),
            "{err}"
        );
        let err = hash_dump(&path, &data_dir(), Box::new(NoAi), 1, |_, _, _| {}).unwrap_err();
        assert!(matches!(err, ReplayError::OutOfOrder { .. }), "{err}");
    }

    #[test]
    fn hash_dump_emits_every_n_and_the_final_tick() {
        let path = temp_path("hash_dump");
        let (setup, _) = scenario_setup(4);
        let hashes = record(&path, setup, 45, 20);
        let mut lines = Vec::new();
        let outcome = hash_dump(&path, &data_dir(), Box::new(NoAi), 10, |t, h, sub| {
            lines.push((t, h, *sub));
        })
        .unwrap();
        let ticks: Vec<u32> = lines.iter().map(|l| l.0).collect();
        assert_eq!(ticks, [10, 20, 30, 40, 45]);
        // Dumped hashes agree with the recorded ones where both exist.
        for (t, h) in &hashes {
            let line = lines.iter().find(|l| l.0 == *t).unwrap();
            assert_eq!(line.1, *h);
        }
        assert_eq!(
            outcome,
            VerifyOutcome::Ok {
                final_hash: hashes.last().unwrap().1,
                ticks: 45
            }
        );
        // Sub-hashes differ between ticks (tick lives in `meta`).
        assert_ne!(lines[0].2.meta, lines[1].2.meta);
        assert_eq!(lines[0].2.first_difference(&lines[0].2), None);
    }

    /// The M5b header field: `ai_seats` (each AI slot's bot and
    /// personality) survives the writer and the reader, after `start`.
    #[test]
    fn header_round_trips_ai_seats() {
        let rules = Rules::load(data_dir()).unwrap();
        for setup in [
            MatchSetup::bots(&rules, 7, ("standard", "standard"), ("passive", "")),
            MatchSetup::vs_bot(&rules, 8, "standard", "standard"),
            MatchSetup::skirmish(&rules, 9),
            MatchSetup::scenario(&rules, 10),
        ] {
            let path = temp_path(&format!("ai_seats_{}", setup.seed));
            let mut w = ReplayWriter::create(&path, &setup).unwrap();
            w.hash(0, 0, &SubHashes::default()).unwrap();
            w.finish().unwrap();
            let file = ReplayReader::open(&path).unwrap();
            assert_eq!(file.setup, setup);
        }
        let bots = MatchSetup::bots(&rules, 7, ("standard", "standard"), ("passive", ""));
        assert_eq!(
            bots.ai_seats,
            vec![
                crate::state::AiSeat {
                    player: PlayerId(0),
                    bot: "standard".into(),
                    personality: "standard".into(),
                },
                crate::state::AiSeat {
                    player: PlayerId(1),
                    bot: "passive".into(),
                    personality: String::new(),
                },
            ]
        );
        assert!(bots.players.iter().all(|p| p.is_ai));
        assert!(!bots.debug_commands);
        assert_eq!(MatchSetup::skirmish(&rules, 1).ai_seats[0].bot, "passive");
        assert!(MatchSetup::scenario(&rules, 1).ai_seats.is_empty());
    }

    /// A bot that surrenders P1 at tick 3, so a re-simulation without it
    /// diverges (`meta`).
    struct SurrenderAt3;

    impl AiController for SurrenderAt3 {
        fn think(
            &mut self,
            _p: PlayerId,
            view: &crate::ai_hook::SimView<'_>,
            _memory: &mut crate::ai_hook::AiMemory,
        ) -> Vec<Command> {
            if view.tick == 3 {
                vec![Command::Surrender]
            } else {
                Vec::new()
            }
        }
    }

    /// `verify_with` checks the version, then the rules, and only then
    /// asks its factory for a controller, which it hands the header and the
    /// loaded rules: a mismatch never calls the factory, a factory error is
    /// `ReplayError::Controller` (no outcome line), and the controller it
    /// builds drives the AI slots of the re-simulation.
    #[test]
    fn verify_with_builds_the_controller_after_the_version_and_rules_checks() {
        use std::cell::Cell;
        let rules = Rules::load(data_dir()).unwrap();
        let setup = MatchSetup::vs_bot(&rules, 3, "surrender", "");
        let path = temp_path("verify_with");
        let mut sim = Sim::new(setup.clone(), rules.clone(), Box::new(SurrenderAt3));
        let mut w = ReplayWriter::create(&path, &setup).unwrap();
        for _ in 0..20 {
            sim.step(&[]);
        }
        w.hash(sim.tick(), sim.hash(), &sim.sub_hashes()).unwrap();
        w.finish().unwrap();
        assert!(sim.view().player(PlayerId(1)).unwrap().surrendered);

        // The factory sees the header and the loaded rules.
        let called = Cell::new(false);
        let outcome = verify_with(
            &path,
            &data_dir(),
            Box::new(|h: &MatchSetup, r: &Rules| {
                called.set(true);
                assert_eq!(h.ai_seats[0].bot, "surrender");
                assert_eq!(r.rules_hash(), h.rules_hash);
                Ok(Box::new(SurrenderAt3) as Box<dyn AiController>)
            }),
        )
        .unwrap();
        assert!(called.get());
        assert_eq!(
            outcome,
            VerifyOutcome::Ok {
                final_hash: sim.hash(),
                ticks: 20
            }
        );
        // Another controller re-simulates another match.
        let outcome = verify_with(
            &path,
            &data_dir(),
            Box::new(|_: &MatchSetup, _: &Rules| Ok(Box::new(NoAi) as Box<dyn AiController>)),
        )
        .unwrap();
        assert_eq!(
            outcome,
            VerifyOutcome::Diverged {
                tick: 20,
                subsystem: "meta"
            }
        );
        // A factory error is an error, not an outcome.
        let err = verify_with(
            &path,
            &data_dir(),
            Box::new(|_: &MatchSetup, _: &Rules| Err("unknown bot \"surrender\"".to_string())),
        )
        .unwrap_err();
        assert!(matches!(err, ReplayError::Controller(_)), "{err}");
        assert_eq!(err.to_string(), "unknown bot \"surrender\"");

        // Version, then rules: neither mismatch calls the factory.
        let never = |_: &MatchSetup, _: &Rules| -> Result<Box<dyn AiController>, String> {
            panic!("the factory runs after the version and rules checks")
        };
        for (name, bump_version) in [("verify_with_version", true), ("verify_with_rules", false)] {
            let mut bad = setup.clone();
            if bump_version {
                bad.sim_version += 1;
            } else {
                bad.rules_hash ^= 1;
            }
            let p = temp_path(name);
            let mut w = ReplayWriter::create(&p, &bad).unwrap();
            w.hash(0, 0, &SubHashes::default()).unwrap();
            w.finish().unwrap();
            let outcome = verify_with(&p, &data_dir(), Box::new(never)).unwrap();
            let dumped = hash_dump_with(&p, &data_dir(), Box::new(never), 1, |_, _, _| {
                panic!("nothing is simulated")
            })
            .unwrap();
            assert_eq!(outcome, dumped);
            if bump_version {
                assert_eq!(
                    outcome,
                    VerifyOutcome::SimVersionMismatch {
                        replay: SIM_VERSION + 1,
                        binary: SIM_VERSION
                    }
                );
            } else {
                assert_eq!(
                    outcome,
                    VerifyOutcome::RulesChanged {
                        replay: bad.rules_hash,
                        loaded: rules.rules_hash()
                    }
                );
            }
        }
    }
}
