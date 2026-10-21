# Determinism

This document is the contract that makes Eonmark's simulation reproducible: the same rules, seed and commands must produce the same state hash on every machine, every operating system and every frame rate. It lists the rules, explains the numeric and container choices behind them, describes the hashing and replay scheme, shows how to bisect a divergence, and states how the rules are enforced. It is the reference for the `clippy.toml` files in `crates/sim`, `crates/rules` and `crates/ai`. Structure is in [ARCHITECTURE.md](ARCHITECTURE.md); data conventions are in [DATA_FORMAT.md](DATA_FORMAT.md).

## The rules

1. Only `Sim::step` mutates sim state. Nothing outside the sim crate touches it except through commands.
2. No `f32`, `f64`, `std::collections::HashMap`, `HashSet`, `std::time::Instant`, `SystemTime`, `rand::random`, `rand::thread_rng` or `rand::rng` in `crates/sim`, `crates/rules` or `crates/ai`. Each crate has a `clippy.toml` with the ban list; CI runs clippy with `-D warnings`.
3. `cargo tree -p sim -e normal | grep -E 'bevy|glam|wgpu|winit'` must print nothing. The same holds for `rules`, `ai` and `sim-cli`. `pathfinding` is a dev-dependency only.
4. Ids are monotonic and never reused. Every sort key is a total order with a `UnitId` or `BuildingId` tiebreak.
5. All randomness comes from the single `Pcg32` stored in `Sim`. It is part of the hashed state. The seed comes from the replay header.
6. Commands are sorted by `(player_id, client_seq)` before application. A command issued during tick N applies at tick N + `cmd_delay` in every mode: live, replay and any future network mode.
7. No trigonometry. Units store a facing vector; heading is derived render-side in `f32`. Squared distances use `dist_sq_i64`. Overflow in the sim crate panics in release (`overflow-checks = true`) rather than wrapping silently.
8. Derived caches (influence maps, spatial grid, path cache, component ids) are never hashed. The path request queue and the active (suspended) A* search are hashed state, serialised in full. The path cache key includes `cost_grid_generation`, the whole cache is cleared on any cost-grid mutation and on `restore()`, and a hit is honoured only when it can charge the budget exactly as the real search would have (the transparent-cache rule in [design/pathing.md](design/pathing.md)). `restore()` rebuilds every derived structure before returning. A property test runs the sim with the path cache disabled and enabled and asserts identical hashes on every tick.
9. Golden fixtures regenerate only in a commit that bumps `rules_version` with a one-line reason. `verify` fails fast on `sim_version` mismatch and reports `RULES CHANGED since recording` on `rules_hash` mismatch.
10. Tests from M1: same seed and commands twice (sequentially and on two threads) give identical hashes; snapshot at tick 300 then continue to 1200 equals the uninterrupted run, including spawning 50 units after the restore; restore, clear caches, step one tick matches; proptest random command streams; cross-OS final hash equality of fixtures in CI (ubuntu-24.04 against macos-26).

## Why fixed-point

Rust's standard library documents that the precision of transcendental float functions (`sin`, `sqrt`, `powf` and friends) is not specified and can differ between platforms and library versions. A sim built on `f32` therefore cannot prove on a Linux CI runner that a macOS hash is platform-independent. Fixed-point arithmetic is integer arithmetic, so it is bit-exact everywhere.

The sim uses `Fx`, a newtype over `fixed::types::I32F32`: 32 integer bits and 32 fraction bits in an `i64`. One tile is `1.0`. Positions, velocities, ranges and radii are `Fx`. Hit points, damage and timers are `i32`. Stockpiles and income accumulators are `i64`. Tick counters are `u32`.

Multiplying two `I32F32` values needs 128 bits of intermediate precision, so squared distance is computed explicitly:

```
dist_sq_i64(a, b) = ((dx.to_bits() as i128)^2 + (dy.to_bits() as i128)^2) >> 32    as i64, in I32F32 scale
```

A unit test checks the corners of 128 and 256 tile maps. Range checks compare `dist_sq_i64` against a squared range, or use `hypot`/`dist` and `checked_mul` from the `fixed` crate. There is no trig anywhere: a unit stores a facing vector, and group movement uses square-spiral integer offsets.

## Why integer micro-unit income

Base income is 10 resource per 30 seconds, which is 10 per 600 ticks at 20 Hz. The value 10/600 is not representable in `I32F32`, and 600 truncated fixed-point additions sum to 9.999..., so a test asserting "exactly 10 Grain after 30 seconds" would fail or need an epsilon.

Income therefore uses integer micro-units. Per `(player, resource)` the sim keeps an `i64` accumulator:

```
acc += min(sum_rate_per_30s, yield_cap)      every tick
stockpile += acc / 600
acc %= 600
```

A rate of 10 per 30 s adds exactly 10 to the stockpile every 600 ticks by construction. Lore has its own 999 stockpile cap and no Yield Cap. Worked numbers are in [DATA_FORMAT.md](DATA_FORMAT.md).

M3a implements this in `economy::income_step` (tick step 6d) with the period read as `Rules::income_period_ticks()` (600 at 20 Hz); after each tick's credit every stockpile with a `stockpile_cap` is clamped to it. Only the stockpiles and accumulators are stored. Rates, pop and the ramp counters are computed from the units and buildings whenever they are needed and never stored, so nothing has to be kept in sync with them.

## Containers and ids

Units live in `BTreeMap<UnitId, Unit>` and buildings in `BTreeMap<BuildingId, Building>`. Ids are `u32`, handed out by the `IdGen` allocator in `crates/sim/src/ids.rs` (`IdGen::unit()`, `IdGen::building()`), itself part of the hashed state, and never reused. Iteration order is id order, which is canonical by construction and costs nothing extra at a few hundred entries.

### The slotmap pitfall

A generational arena such as `slotmap` looks like the natural fit and was rejected for one reason that matters here: its `Deserialize` implementation rebuilds the free list by scanning slots in index order, while the live map's free list is last-in-first-out by removal order. After a snapshot and restore the next spawn lands in a different slot than it would have in the uninterrupted run. Slot-order iteration then differs, and the state hash diverges on the first spawn after restore. The restore test in rule 10 ("spawn 50 units after the restore") exists to catch exactly this class of bug.

The fix is not to serialize the free list by hand but to avoid slot reuse entirely. Monotonic ids in a `BTreeMap` serialize and deserialize to the same structure with no hidden state. If `BTreeMap` ever benches badly, the fallback is a hand-written arena that serializes `free_head` and every `next_free` verbatim, not `slotmap`.

### Sort keys

Every ordering decision in the sim is a total order that ends in an id. Examples: path requests sort by `(request_tick, UnitId)`, spiral offsets are assigned in `UnitId` order, separation and circle push resolve in id order, target acquisition ties break by id, and seeded ties use the RNG only after the deterministic keys are exhausted. A test, `sort_keys_are_total_orders`, exists from M1.

## Derived caches

Four structures are recomputed from hashed state and are never hashed themselves: influence maps (4x4-tile cells, every 10 ticks), the uniform spatial grid (2x2-tile cells, rebuilt each tick), connected-component ids (recomputed per `cost_grid_generation`) and the bounded path cache.

### Path cache generation rule

The cost grid carries a `u32` `cost_grid_generation` that increments on every change (a building placed or destroyed, a tile blocked; a whole footprint is one change, through `Map::set_blocked_many`). The path cache is keyed by `(start_tile, goal_tile, cost_grid_generation)` and is cleared wholesale on any mutation. Therefore a cache hit returns exactly what a fresh search would return. A cache keyed by `(start, goal)` alone would return stale paths after a building changes the grid and would make the result depend on history, which breaks the snapshot/restore rule.

### Snapshot and restore rule

`snapshot()` serializes every hashed field verbatim with postcard. `restore()` deserializes them and then eagerly rebuilds influence maps, the spatial grid, the supply grid and connected components, and clears the path cache, before returning. No derived structure is lazily rebuilt, so the first `step` after restore sees the same inputs as the uninterrupted run. The command delay queue (`pending`, a `BTreeMap` keyed by application tick) is hashed state, so a snapshot carries in-flight commands and a restore applies them on the right tick.

### The active A* search is hashed, the cache is not

Decided in M1. A path search that ran out of budget mid-tick is sim state: it decides on which tick a path appears. The sim stores it as `Pathing.active: Option<(PathRequest, AStarSearch)>` and serialises the `AStarSearch` in full: the open `BinaryHeap<Node>` as a sequence in its internal array order, the `closed`, `g` and `parent` arrays, and the expansion counter. `std::collections::BinaryHeap` deserialises by pushing in that order, which reproduces the identical array, so a snapshot taken mid-search restores the exact search and the next `resume` pops the same nodes. The alternative, storing an expansion counter and replaying the search on restore, was rejected: it spends the replayed expansions at restore time and cannot represent a search that the cache fallback would have skipped. A unit test round-trips a half-finished search through postcard and asserts identical paths and hashes; the queue and the active search together form the `pathing` sub-hash.

The path cache is the opposite case: it must never change results, so it is `#[serde(skip)]`, never hashed, cleared on every generation change and on `restore()`, and transparent by construction. Each entry stores the path and `expansions_used`; a hit is honoured only when `expansions_used` fits in the tick's remaining budget, charging exactly that amount, so the hashed state at the end of every tick is identical with the cache on or off. When it does not fit, the real search runs and suspends as it would have anyway. `path_cache_is_transparent` asserts equal hashes on every tick of a random command stream.

## Hashing

`Sim::hash()` is `xxh3_64` over the postcard serialization of all hashed state. Postcard is canonical for a given struct, and every container in the state is ordered, so equal states produce equal bytes.

`Sim::sub_hashes()` returns one hash per subsystem so a divergence can be localized.

| Sub-hash | Covers | Since |
|---|---|---|
| `units` | The `BTreeMap<UnitId, Unit>` | M1 |
| `buildings` | The `BTreeMap<BuildingId, Building>`, sites included | M1 (empty until M3a) |
| `economy` | `Economy`: per player, an `i64` stockpile and an `i64` income accumulator per resource. Cap state, rates, pop and ramp counters are computed, never stored | M3a (a placeholder before) |
| `territory` | `TerritoryField`: the sources, the `u16` strength layers and the `u8` owner grid, each grid packed as one little-endian byte string | M3a (a placeholder before) |
| `tech` | Researched techs, queues, ages | placeholder until M4a |
| `pathing` | The path request queue and the active (suspended) A* search; never the cache | M1 |
| `rng` | The `Pcg32` state | M1 |
| `map` | The cost grid and `cost_grid_generation` | M1 |
| `meta` | `tick`, `seed`, the `IdGen` allocator, players, the pending command queue, `commands_applied` | M1 |

The field order of `SubHashes` is the comparison order: `SubHashes::first_difference` returns the name of the first field that differs, which is what `verify` prints. `SubHashes::NAMES` lists the nine names in that order and names the `key=value` fields of a `hash-dump` line. Subsystems that do not exist yet hash an empty placeholder so the layout is stable across milestones (from M3a only `tech` does). Every field of the hashed `State` belongs to exactly one sub-hash, so any divergence has a name.

Hashes are recorded every 20 ticks by default (`replay::DEFAULT_HASH_EVERY`) and every tick under `--hash-every-tick` (`sim-cli record` from M1; the game's writer thread from M2).

The hashed state is the full `State` (`tick`, `seed`, `rng`, `ids`, `players`, `units`, `buildings`, `map`, `pathing` without its cache, `pending`, `commands_applied`, and from M3a `economy` and `territory`) and `hash()` is xxh3 over its postcard bytes. The `MatchSetup` is not hashed; it is the replay header, and what it selects (seed, map, `start`) shows up in the state from tick 0. `sub_hashes()` hashes the pieces listed above separately.

## Replays

A replay is an `.eonreplay` file, format version 1, written and read by `crates/sim/src/replay.rs`:

```text
"EONR"                      4 magic bytes                       replay::MAGIC
u16 little-endian           format version, 1                   replay::FORMAT_VERSION
postcard(MatchSetup)        header (fields below)
postcard(Record)*           stream, no framing, in tick order
"EONRDONE"                  8-byte clean-exit trailer           replay::TRAILER (only when the writer finished)
```

| Record | Fields | Written |
|---|---|---|
| `Record::Tick(TickBatch)` | `tick: u32`, `cmds: Vec<PlayerCommand>` | for every tick whose `step` call received at least one command; the commands exactly as handed to `step`, before the command delay |
| `Record::Hash(HashRecord)` | `tick: u32`, `hash: u64`, `sub: SubHashes` | after every 20 ticks (`--hash-every-tick`: every tick) and once more at clean exit |

A tick's `Tick` record precedes its `Hash` record. Hash records carry the sub-hashes, not just the whole-state hash, so that `verify` can name the diverging subsystem from the replay alone: without them, naming the subsystem would need a second re-simulation on the machine that recorded the file, which is exactly what is unavailable when a replay comes in from somewhere else.

The reader tolerates truncation. It decodes records with `postcard::take_from_bytes` and stops at the first record that does not decode, keeping everything before it; postcard encodings are self-delimiting and prefix-free per type, so a cut-off tail never decodes as a shorter valid record. A file cut by `kill -9` therefore verifies up to its last complete hash record and `verify` reports that tick count. The clean-exit trailer tells a cut apart from damage: `ReplayWriter::finish` appends `"EONRDONE"` after the last record, so a file that ends with the trailer was closed cleanly and every byte before the trailer must decode. If one does not, the reader returns `ReplayError::Corrupt { path, offset, undecoded }` and `verify` exits 2 instead of silently verifying the prefix (a flipped byte that breaks a record's encoding is reported this way; a flipped byte that keeps the record decodable but changes a command is a `DIVERGED` outcome; a flipped byte the sim never reads, such as a `queue` flag or a sub-tile bit of a target, is neither and verifies `OK`). A file without the trailer is a truncated recording: `ReplayFile::truncated` is set, the CLI prints a `warning:` line on stderr naming the complete records kept and the trailing bytes dropped, and the `OK` line itself is unchanged so CI's `^OK` grep and hash-parity diff still apply. A file shorter than its header is `ReplayError::Header`; wrong magic is `BadMagic`; a version other than 1 is `UnsupportedVersion`. Before decoding the header the reader peeks its first field, `sim_version` (a postcard `u32`); when it is not `sim::SIM_VERSION` the reader returns `ReplayError::SimVersion { path, found }` without decoding the rest, because the header layout itself can differ between sim versions (M3a appended `start`) and decoding an older header with the current layout would misread the record stream. The framing did not change, so `FORMAT_VERSION` stays 1. A record whose tick lies before the tick the stream has already reached is a corrupt file, not truncation: the reader returns `ReplayError::OutOfOrder { path, tick, reached }` and `verify` reports it as an error (exit 2) rather than skipping it.

### Header fields

| Field | Type | Compared by `verify` | Meaning |
|---|---|---|---|
| `sim_version` | `u32` | Yes, fails fast | Hand-bumped whenever sim behaviour changes for the same inputs. Constant `sim::SIM_VERSION`. |
| `git_sha` | `String` | Never | Informational. Comparing it would invalidate every fixture on every commit. |
| `rules_version` | `u32` | Reported | Human label from `rules.ron`, bumped when goldens regenerate. |
| `rules_hash` | `u64` | Yes, reports `RULES CHANGED` | xxh3 over postcard of the loaded `Rules`, covering every file `Rules::load` reads: match rules, resources and every map at M0, `units.ron` from M1, `visuals.ron` from M2 (the sim never reads it, but a replay still records which visuals were loaded), and `data/ai` once it is loaded (M5b). |
| `map` | `String` | Loaded | Map id under `data/maps/`. |
| `seed` | `u64` | Loaded | Seed for the `Pcg32`. |
| `tick_rate_hz` | `u32` | Loaded | 20. |
| `cmd_delay` | `u32` | Loaded | Ticks between issue and application. |
| `players` | `Vec<PlayerSlot>` | Loaded | Slot id and `is_ai` per player, in id order. `MatchSetup::skirmish` makes slot 0 human and slot 1 the AI; `MatchSetup::scenario` makes two human slots. |
| `debug_commands` | `bool` | Loaded | Whether the debug commands are accepted: `DebugSpawn`, and from M3a `DebugPlaceBuilding` and `DebugGrant`. `false` for `skirmish`; `true` for `scenario` (fixtures, benches, tests). A debug command in a match with `false` is rejected with `CommandRejected { reason: DebugCommandsDisabled }` and changes nothing; like every applied or rejected command it still increments `commands_applied` (hashed in `meta`), so the hash reflects the full command stream. Only the not-yet-implemented commands draw from the RNG on rejection (since M3a training: `Attack`, `Research` and `AdvanceAge`). |
| `start` | `StartKind` | Loaded | What each player has at tick 0 (M3a, the last field). `Standard` applies `standard_start` from `rules.ron` in `Sim::new`: per player in id order the Seat, the start units and the stockpile. `Empty` starts with nothing. `skirmish` uses `Standard`, `scenario` uses `Empty`; a scenario may set `Standard`, as `economy_second_town` and `economy_walkthrough` do. |

AI difficulty ids (M6) and faction ids (M9) are added to the header when those systems arrive. New fields go at the end, as `start` did in M3a, together with a `SIM_VERSION` bump, so the reader's `sim_version` peek reports an older file as a version mismatch rather than a decode error. The same struct is the future lockstep handshake payload.

Why two version fields: a git sha changes every commit, so it cannot gate fixtures. A manually bumped `rules_version` can be forgotten, so an unbumped RON edit would masquerade as a sim divergence. The content hash catches that case with its own message.

### Writer

`sim::replay::ReplayWriter` is the sink: `create(path, &setup)` writes the magic, version and header; `tick(tick, cmds)` and `hash(tick, hash, &sub)` append records; `flush()` writes the buffer; `finish()` appends the clean-exit trailer, flushes and `sync_all`s once. `sim-cli record` drives it directly (M1). The game drives it from a dedicated thread (M2, below).

### The game's recorder thread (M2)

`crates/game/src/sim_driver.rs` owns the recorder. Every windowed session records; headless replay runs (`--headless-run <file>`) do not.

| Piece | What it is |
| --- | --- |
| Thread | one `std::thread` named `eonmark-replay-writer` running `writer_thread(ReplayWriter, Receiver)`; it owns the `ReplayWriter` and nothing else touches the file |
| Channel | `std::sync::mpsc` of `RecorderMessage`: `Tick { tick, cmds }` (the batch handed to `Sim::step` for that tick, already sorted; empty batches are skipped by the writer), `Hash { tick, hash, sub }` (a checkpoint after the step that produced `tick`), `Finish` (clean exit) |
| Producer | `drive_tick`, inside the `FixedUpdate` step: after `sim.step(&cmds)` it sends `Tick`, and every `HashCadence::every` ticks (20 by default, 1 under `--hash-every-tick`) a `Hash`. `Recorder::send` never blocks and never fails loudly: a dead writer logs one `warn!` and the sim carries on |
| Flush cadence | the writer calls `flush()` after every tick whose number is a multiple of 20 (`FLUSH_EVERY_TICKS` = `DEFAULT_HASH_EVERY`), that is once per second at 20 Hz; `flush` is a `write(2)`, never an fsync |
| Finish | `finish_recorder_on_exit` runs in `Last`, ordered `.after(bevy::window::ExitSystems)`. On the frame any `AppExit` message is present (Cmd-Q through `macos_menu`, the red close button through `bevy_window::exit_on_all_closed`, which itself runs in `Last` inside `ExitSystems`, `--exit-after-seconds`) it sends one more `Hash` for the current tick, then `Finish`, then joins the thread with a bounded wait (`RECORDER_JOIN_TIMEOUT` = 3 s). The ordering matters: the winit runner exits right after the frame that produced the `AppExit`, so a recorder system that ran earlier in `Last` would miss the close button's message and leave a truncated file (measured with `--close-window-after-seconds` before the constraint was added). On `Finish` the writer calls `ReplayWriter::finish`: trailer `EONRDONE`, flush, `sync_all` once, return the record count (logged as `replay finished: <path> (<n> records, tick <t>, hash <h>)`) |
| Why no fsync during play | on Apple platforms both `File::sync_all` and `sync_data` are `fcntl(F_FULLFSYNC)`, which flushes the drive cache and costs from a few to tens of milliseconds; once a second that is a visible hitch, and even off the main thread it contends for the file. Surviving `kill -9` needs only pages handed to `write(2)`, which `flush` provides: a hard kill loses at most one second of commands and the reader reports a truncated recording that still verifies. Surviving a power loss is not a goal |
| Hard kill | the channel closes without `Finish`; the writer flushes what it has and returns without the trailer. `sim-cli verify` prints a `warning:` naming the dropped tail and then `OK` for the complete records |
| Path | `--replay-dir <dir>`, else `ProjectDirs::from("com", "tonianev", "Eonmark").data_dir()/replays/` (macOS: `~/Library/Application Support/com.tonianev.Eonmark/replays/`), else `./replays`; parent directories are created. File name `<yyyymmdd-hhmmss>-<seed>.eonreplay` (UTC wall clock, which is allowed in the game crate; the sim never sees it). The path is printed as `replay: <path>` on stdout at start |

The writer is purely a sink: nothing reads from it, the sim does not wait for it, and it never feeds anything back. Because `Finish` is sent from `Last` of the same frame that produced the `AppExit`, a Cmd-Q and a close-button exit are indistinguishable in the file from `--exit-after-seconds`; the automated stand-in for the owner's Cmd-Q check is `--quit-via-menu-after-seconds` (see [PLAYTEST.md](PLAYTEST.md)). CI and `scripts/m2_checks.sh` pass `--replay-dir` with a temporary directory.

### The four verify outcomes

```bash
cargo run -p sim-cli --release -- verify path/to/match.eonreplay
```

`verify` prints exactly one line, the `Display` of `sim::replay::VerifyOutcome`, and exits with `VerifyOutcome::exit_code()`. The strings are fixed and tested; CI greps for `^OK`.

| Output (exact format) | Exit code | Meaning |
|---|---|---|
| `OK final_hash=0x<16 hex> ticks=<n>` | 0 | Re-simulation reproduced every recorded hash. `final_hash` and `ticks` come from the last hash record in the file (a replay without any hash record reports the re-simulated hash at the last tick seen). |
| `DIVERGED at tick <N> (subsystem: <name>)` | 1 | The first hash record that did not match. `<name>` is the first differing field of `SubHashes` in field order (`units`, `buildings`, `economy`, `territory`, `tech`, `pathing`, `rng`, `map`, `meta`), or `state` if the whole hash differs while every sub-hash matches. |
| `SIM VERSION MISMATCH (replay <a>, binary <b>)` | 1 | `header.sim_version != sim::SIM_VERSION`. The replay predates a behaviour change; nothing is simulated and the rules are not loaded. The version is read from the header's first field before the rest is decoded, so a header whose layout has changed still reports this outcome. |
| `RULES CHANGED since recording (replay 0x<16 hex>, loaded 0x<16 hex>)` | 1 | `header.rules_hash != Rules::load(data_dir).rules_hash()`. Someone edited RON without bumping `rules_version` and regenerating; nothing is simulated. |

The checks run in that order: version, then rules, then re-simulation. Read errors (`ReplayError`: missing file, bad magic, unsupported format version, undecodable header, a corrupt record stream before the clean-exit trailer, out-of-order records) are not outcomes; the CLI reports them as `error: <message>` with exit 2 and never prints one of the four lines. The one exception is `ReplayError::SimVersion`: `verify` and the replay form of `hash-dump` turn it into the `SIM VERSION MISMATCH` outcome (exit 1), and the game's `--headless-run` prints the same line on stderr and exits 1. A truncated recording (no trailer) is not an error: the CLI prints a `warning:` on stderr and then the outcome for the records it has.

## Bisecting a desync

When `verify` prints `DIVERGED at tick N (subsystem: X)`, the recorded hash at tick N differs and X is the first sub-hash that disagrees with the recording, but hashes are only recorded every 20 ticks by default, so the drift began somewhere in the 20 ticks before N. Narrow it down with the replay form of `hash-dump`, which re-simulates the replay's command stream on the current machine and prints one line per tick:

```bash
cargo run -p sim-cli --release -- hash-dump match.eonreplay --every 1
```

```text
tick=1 hash=0x9e0f95c3cdc50f1e units=0xc44bdff4074eecdb buildings=0xc44bdff4074eecdb economy=0xf8e0f87a0ec2c396 territory=0x6ffe02d5dd6f6879 tech=0x2d06800538d394c2 pathing=0x3325230e1f285505 rng=0x45713480d687fdbd map=0x38587eaeac2ca50a meta=0x5440e9088c79a4e6
tick=2 hash=0x164b4194f084e9f8 units=0xc44bdff4074eecdb ...
```

Each line is `key=value` pairs: `tick`, the whole-state `hash`, then one field per name in `SubHashes::NAMES`, in field order (the lines above are the first two of `move_500_short` at `SIM_VERSION` 11, unchanged since 6: no unit exists yet at ticks 1 and 2). A line is printed after the step that completes that tick, so the last line of a complete dump carries the replay's final hash. `--every N` prints every N-th tick (default 20, matching the recording cadence). `hash-dump` does not compare anything itself; it is the input to `diff`. The scripted M0 form, `hash-dump --ticks N --seed S`, still exists and prints the whole-state hash of the selftest match.

To bisect across two machines (or two builds, or two operating systems):

```bash
# 1. On the machine that produced the replay (or any machine that verifies it OK):
cargo run -p sim-cli --release -- hash-dump match.eonreplay --every 1 > a.txt

# 2. On the machine or build under suspicion, with the same replay file and data/:
cargo run -p sim-cli --release -- hash-dump match.eonreplay --every 1 > b.txt

# 3. The first differing line is the first tick that drifted; the first differing
#    column on that line is the subsystem. Everything before it is identical.
diff a.txt b.txt | head -n 5
```

Both dumps are deterministic functions of the replay and the data directory, so `a.txt` is reproducible: if the recording machine is gone, a machine where `verify` prints `OK` for that replay stands in for it. If both machines print identical dumps but `verify` still diverges, the recording itself was made by a different binary or data set; check `sim_version` and `rules_hash` in the header first (`verify` does this before simulating, so this case normally surfaces as one of the two mismatch outcomes). Within one machine the same procedure finds a non-determinism bug: dump the same replay twice; any difference is a bug in the sim, and the column names it.

Then look at what changed in that subsystem on that tick. The usual suspects, in order of frequency: a new `HashMap` or unsorted `Vec` iteration (check `clippy` passed with `-D warnings`), a float that leaked in through a dependency or a `Rules` conversion, a derived cache that was read before `restore()` rebuilt it, a sort key missing its id tiebreak, and an RNG draw that happens in a non-deterministic order. Run the proptest suite and the cache on/off test before suspecting anything exotic.

If the two sides are different operating systems and everything else is equal, the bug is in rule 2 or 7: find the float.

## Enforcement

### clippy.toml ban list

Each of `crates/sim`, `crates/rules` and `crates/ai` carries the same `clippy.toml`.

| Kind | Banned | Reason |
|---|---|---|
| type | `f32`, `f64` | Not deterministic across platforms; use `sim::fx::Fx` |
| type | `std::collections::HashMap`, `HashSet` | Randomised iteration order; use `BTreeMap`, `BTreeSet` |
| type | `std::time::Instant`, `SystemTime` | Wall clock is not simulation state |
| method | `rand::random`, `rand::thread_rng`, `rand::rng` | All randomness comes from the `Pcg32` in `Sim` |

CI runs `cargo clippy --workspace --all-targets --locked --profile ci -- -D warnings`, so a banned type anywhere in those crates fails the build. The ban is demonstrated once in a scratch branch (`let x: f32 = 1.0;` in `crates/sim`) and documented in [CONTRIBUTING.md](../CONTRIBUTING.md).

### Boundary check

```bash
for c in sim rules ai sim-cli; do
  if cargo tree -p "$c" -e normal | grep -E 'bevy|glam|wgpu|winit'; then
    echo "engine dependency leaked into $c"; exit 1
  fi
done
echo "boundary ok"
```

`grep -c` exits 1 on zero matches, so every "must not contain" assertion in CI uses the `if ... | grep -q X; then exit 1; fi` shape, never `grep -c`.

### Cross-OS parity

The `check` job on macOS and the `headless` job on Ubuntu each upload two lines: the `tick=1200 hash=0x...` line of the game's `--headless-run crates/sim/tests/fixtures/move_500.eonreplay` run (from M2; `--headless-run 200` printed `tick=200 hash=0x...` before that; only the `^tick=` line is kept, and the run itself exits 1 if the re-simulated hash differs from the recorded one), and the `OK final_hash=0x... ticks=1200` line that `sim-cli verify --release crates/sim/tests/fixtures/move_500.eonreplay` prints (only the `^OK` line is kept, so a non-OK outcome fails the producing job rather than the diff). The `hash-parity` job downloads both artifacts and `diff`s each pair. Any difference fails CI. This job is never negotiable. The `headless` job additionally verifies the `group_spiral`, `snapshot_restore`, `economy_second_town` and `economy_walkthrough` fixtures in release and asserts that `pathfinding` is absent from `cargo tree -p sim -e normal`.

## Golden fixtures

Fixtures live under `crates/sim/tests/fixtures/` as `.eonreplay` files with a sibling `.hash` file holding the final hash as `0x<16 hex>` and a newline. They are recorded with `sim-cli record --scenario <name> --ticks <n> --out <file>` from scenario definitions shared with the tests, under `MatchSetup::scenario` (two human slots, `debug_commands: true`, an empty start unless the scenario sets `start: StartKind::Standard`, as `economy_second_town` and `economy_walkthrough` do). Short fixtures (at most 5000 ticks) run inside `cargo test -p sim` through the `golden(name)` helper. Long ones are `#[ignore]` locally and verified by `sim-cli verify` in release in the headless CI job.

| Fixture | Ticks | Verified by |
|---|---|---|
| `move_500_short` | 300 (the first 300 ticks of `move_500`, before its Stop) | `cargo test -p sim` |
| `group_spiral` | 600 | `cargo test -p sim`, `sim-cli verify --release` in CI |
| `snapshot_restore` | 1200 (recorded across a snapshot/restore swap at tick 300) | `cargo test -p sim`, `sim-cli verify --release` in CI |
| `move_500` | 1200 | `sim-cli verify --release` in CI on both operating systems; the hash-parity fixture |
| `economy_second_town` | 3600 (standard start, mirrored build orders that found a second Town per player) | `cargo test -p sim`, `sim-cli verify --release` in CI |
| `economy_walkthrough` | 2400 (standard start, P0 only: the M3b owner proxy's place-outside, place-inside, second Town, Yield Cap and pop cap beats) | `cargo test -p sim`, `sim-cli verify --release` in CI |

Regeneration policy:

- Fixtures regenerate only in a commit that bumps `rules_version` in `data/rules/rules.ron` and states the reason in one line of the commit message. CI enforces it: `scripts/check_fixture_policy.sh <base>` fails a pull request whose diff touches `crates/sim/tests/fixtures/*.eonreplay` or `*.hash` unless `rules_version` increased in the same diff (`just fixture-policy` runs it locally against the merge base with main).
- A changed hash without that bump is a bug, not a fixture update. Find it with the bisect procedure above.
- `RULES CHANGED since recording` means someone edited RON without bumping. Either revert the edit or bump and regenerate in the same commit.
- `just rerecord [NAMES]` (`scripts/rerecord_fixtures.sh`) is the regeneration command: it re-records every fixture, or the ones named, in release, writes each `.hash` from an independent `sim-cli verify` OK line rather than from the recorder, and then fails if a fixture differs from the merge base with main while the working-tree `rules_version` is not larger than the base's (`scripts/check_fixture_policy.sh` checks the same on commits in CI). It is also the re-record step of the data-only contributor path in [../CONTRIBUTING.md](../CONTRIBUTING.md).
- A behaviour change in Rust that is intended (a new movement rule, a fixed bug) bumps `SIM_VERSION` as well, and the commit regenerates every fixture.

## What exists at M0

The three `clippy.toml` files and the `[profile]` overrides. `Fx` and `FxVec2`. Monotonic ids from `IdGen`, which is itself hashed state so a restore never reuses an id. The full `Command` enum and `sort_commands`. The `MatchSetup` header with every field in the table above except `debug_commands` and `start`. A `Sim` whose hashed state is `{ tick, seed, rng: Pcg32, ids, players, units, buildings, pending, commands_applied }`; `step` queues commands for `tick + cmd_delay`, stamps and queues the AI's commands the same way, and applies everything due at the current tick in `(player, seq)` order. `hash()` as xxh3 over postcard, `snapshot()` and `restore()` with an empty `rebuild_derived`. `sim-cli selftest --ticks N --seed S` runs the scripted match twice on two threads and compares; `sim-cli hash-dump --ticks N --seed S` prints the per-tick hash.

## What M1 adds

The hashed `State` gains `map` (cost grid and generation) and `pathing` (request queue and active search). `Sim::step` runs, in order: queue the tick's commands, run the AI, apply due commands, `Pathing::service` under the rules budget, rebuild the spatial grid, `movement::step`, then `tick += 1`. `Command::Move` and `Command::Stop` have real handlers; `Command::DebugSpawn` spawns units on a deterministic square spiral of passable tiles and is accepted only under `debug_commands: true`. `sub_hashes()` with the nine fields above, `SimEvent`s drained through `drain_events()`, `rebuild_derived()` that rebuilds components and the spatial grid and clears the path cache. The replay format with its clean-exit trailer, `ReplayWriter`, `ReplayReader` (`ReplayFile` with `truncated`), `verify`, and `sim-cli verify`, `record`, `bench`, `fuzz` and the replay form of `hash-dump`. The movement constants (repath cadence, separation push caps, waypoint and slowdown radii) live in `rules.ron` and `units.ron`, not in Rust. The tests in rule 10 and the four golden fixtures.

## What M2 adds

Sim side, deliberately small: `Command::AttackMove` is applied exactly like `Command::Move` through the shared `Sim::apply_move` (no rejection, no RNG draw) so the game's A-then-click placeholder never produces `CommandRejected`; real attack-move behaviour arrives with combat in M4a. That is a behaviour change for the same inputs, so `SIM_VERSION` went 3 -> 4 and `rules_version` 3 -> 4 (`visuals.ron` joined `rules_hash` in the same commit); the four golden fixtures were re-recorded and every `.hash` file is unchanged, because none of them contains an `AttackMove`. `sim::scenarios` gains `units200`, `units200_auto`, `scripted_moves` (which does include an `AttackMove`, during tick 450) and `hud_click`, all pure data, resolved by `by_name` for `sim-cli record` and the game's `--scenario`.

Game side: the recorder thread above; `ReplayInput`, which replays recorded `TickBatch`es through the same `drive_tick` as live play and ends the run with a `ReplayFinished { final_tick, recorded_hash, sim_hash }` message; `--headless-run <file.eonreplay>` on `MinimalPlugins`, printing `tick=<n> hash=0x<16 hex>` as its last stdout line and exiting 0 only when the recorded and re-simulated hashes agree (header mismatches are reported like `sim-cli verify`, before simulating; a recording cut by `kill -9` ends at its last hash record and any tick batch at or after that tick is dropped, because no recorded hash can check it, which is exactly what `verify` reports for the same file); `--hash-every-tick`; `--max-fps <n>`, which sleeps in `Last` and must not change any hash (the `scripted_moves` 30 vs 120 fps check). The acceptance that frame rate never changes a hash is the first direct test of rule 6 from the engine side.

## What M3a adds

The Town loop, the first M3a slice (`SIM_VERSION` 5 -> 6, `rules_version` 6 -> 7; no RON content changed besides the version, which moved because every fixture was re-recorded; a follow-up, `SIM_VERSION` 7 and `rules_version` 8, makes a builder walk only to ring tiles it can reach with the footprint blocked and spawns standard-start units after every Seat). The hashed `State` gains `economy` and `territory`, which fill those two sub-hashes. `Unit` gains `job` (`Job::Build { site }` or `Job::Gather { node, slot }`, the single record of who builds a site or works a slot) and `seek_at`; `Building` gains `hp`, `construction` (`Site { progress }` or `Complete`), `paid`, `town`, `town_state` (`TownState { seat, tier }`), `slots`, `queue` and `rally`. The header gains `start` as its last field, and the reader peeks `sim_version` before decoding the rest (above). `Sim::new` asserts that the player slots are `PlayerId(0)`, `PlayerId(1)`, ... in order (stockpiles and territory layers are indexed by id) and applies the standard start when `start` is `Standard`: on `plains_1v1` the Seats are `BuildingId(1)` and `BuildingId(2)` (one generation bump each), P0's Yeomen `UnitId(1..=5)` and P1's `UnitId(6..=10)`.

`Sim::step` gains step 6 after movement, each part in id order: 6a construction (a site whose builder is at work gains one tick of progress and completes at its kind's build time; a completed Town or Watchtower adds its border source), 6b training (from the training slice, below), 6c jobs (a build job whose site is gone, complete or no longer the unit's owner's, or whose worker stopped where it cannot work, is dropped), 6d income (above); then `tick += 1`. `Build` and `Cancel` have real handlers, and `DebugPlaceBuilding` and `DebugGrant` are appended to `Command` after `DebugSpawn`, gated by `debug_commands`. `Build` is validated by a pure function (`construction::validate_build`, also behind `SimView::check_build`), so a rejected `Build` or `Cancel` changes nothing but `commands_applied` and draws no randomness. An accepted `Build` deducts the ramped cost and blocks the footprint with one `Map::set_blocked_many` call; a site `Cancel` (slot 0) refunds `paid`, clamped to the stockpile caps, and unblocks the footprint the same way. `Move`, `AttackMove` and `Stop` clear `job` and `seek_at`. New events: `BuildingPlaced`, `BuildingCompleted`, `BuildingRemoved` and `TerritoryChanged`. `rebuild_derived()` is unchanged: neither the economy nor the territory field has a derived member.

Tests: `economy_restore_mid_construction_matches_uninterrupted` restores mid-construction and compares every later tick; `territory_build_outside_own_borders_is_rejected_without_state_change` compares every hash and sub-hash with a control sim whose command is rejected for another reason. The M1 random command stream now places a complete P0 Town with `DebugPlaceBuilding`, grants Lumber and issues random Croft `Build`s and site `Cancel`s, so the determinism and path cache transparency proptests cover cost grid changes while searches are suspended (`random_commands_place_and_cancel_sites` checks that the stream really does both); `sim-cli fuzz` adds a `Cancel` of `BuildingId(0)` (`UnknownBuilding`) and a `Build` by `UnitId(0)` (`NoValidUnits`) to its expected rejections. Every fixture was re-recorded: the four M1 fixtures start empty and contain no M3a command, but `State` gained hashed fields, so every `.hash` changed. `economy_second_town` is the new golden fixture.

Gathering, the next M3a slice (`SIM_VERSION` 7 -> 8, `rules_version` 8 -> 9; again no RON content changed besides the version), adds `crates/sim/src/gather.rs`. `Gather` has a real handler: a pure `gather::validate_gather` (the rejections `UnderConstruction`, `NotAGatherNode` and `NoFreeSlot` are new), then the valid units in `UnitId` order take the lowest free slots. A rejected `Gather` changes nothing but `commands_applied`, and `Gather` no longer draws from the RNG. Slot occupancy has no record of its own: a slot is taken by the unit whose `Job::Gather` names it and worked while that unit is at work, both computed when needed, like the rates. Step 6c now also drops gather jobs that can no longer be done and then runs idle seeking in id order: an idle worker (a Yeoman or Scribe with no job and no order) without a timer sets `seek_at`, a busy one clears it, and one whose timer has run out takes the nearest free slot, ordered by squared distance to the footprint centre (`dist_sq_i64`) and then `BuildingId`, a total order, or sets `seek_at` again for the retry. Step 6a gives a finished builder a free slot at its building when its kind works it, and a completion may grow the attributed Town to tier 2 (a `change_source`; the new event `TownTierChanged`). Step 6d adds worked slots to the base yields.

`seek_at` is hashed in `units`, so a fixture whose units stand idle changes even though nothing gathers: `move_500` and `snapshot_restore` have new final hashes because their arrived Yeomen now run the timer, and `group_spiral`'s intermediate hash records changed while its final hash did not (its units are walking again at the end). `move_500_short` sees no arrival in its 300 ticks and differs only in its header. For the same reason the M1 return-to-post settle test compares positions, posts and orders instead of the `units` sub-hash, which the retry timers keep changing. Tests: `economy_restore_mid_gather_matches_uninterrupted` snapshots with workers walking, working and waiting on their timers; the M1 random command stream also issues `Gather`s onto random building ids, and `sim-cli fuzz` adds a `Gather` onto `BuildingId(0)` (`UnknownBuilding`) to its expected rejections.

Training, the last M3a slice (`SIM_VERSION` 8 -> 9, `rules_version` 9 -> 10; again no RON content changed besides the version), adds `crates/sim/src/training.rs`. `Train` is validated by a pure `training::validate_train` (also behind `SimView::check_train`; the rejections `CannotTrainHere`, `QueueFull` and `PopCapReached` are new), then deducts the ramped cost and appends a `QueueEntry` to the building's queue. `Cancel` of a queue slot on a complete building refunds that entry's `paid` (the new event `TrainingCancelled`), and `SetRally` sets the building's `rally`. A rejected `Train`, `SetRally` or queue `Cancel` changes nothing but `commands_applied`, and `Train` and `SetRally` no longer draw from the RNG. Step 6b gives the queue head of every complete building, in `BuildingId` order, one tick of progress; a head that reaches its kind's training time becomes a unit on the first passable tile of the spiral around the footprint centre, which walks to the rally point, takes a slot at its building or starts idle. Pop and the unit ramp counters are computed from the units and queues, like the rates. `Building.queue` (`QueueEntry { item, paid, progress }`) and `Building.rally` have been hashed state in `buildings` since `SIM_VERSION` 6, so `State` gains no field. Only `economy_second_town`, where each Seat now also queues a Yeoman during tick 0, has a new final hash; the four M1 fixtures train nothing and differ only in their headers. Tests: `economy_restore_mid_training_matches_uninterrupted` snapshots with the head half trained and compares every later tick across a `SetRally`, a `Train` and a queue `Cancel`; `economy_train_rejections` compares every hash and sub-hash with a control sim. The M1 random command stream also trains Yeomen at the debug Town and sets rally points on it, and `sim-cli fuzz` adds a `Train` and a `SetRally` on `BuildingId(0)` (`UnknownBuilding`) to its expected rejections.

Two review follow-ups (`SIM_VERSION` 9 -> 10 -> 11, `rules_version` 10 -> 11 -> 12) change where a trained unit appears. The first passable tile of the spiral could lie in a pocket that neighbouring buildings seal against the trainer, which trapped the unit for good (a complete building cannot be removed). Now the spawn tile is the first passable spiral tile in the spawn component. The candidates are the components touching the building's ring, or every component when the ring is fully blocked; the spawn component is the rally point's when the rally point's own tile is passable and its component is a candidate, otherwise the largest candidate by tile count and then the lower id (`Map::components` is derived from the cost grid, so the choice is a function of hashed state). A sealed pocket therefore takes the unit only when the rally point is inside it. `SimView::check_build` and `SimView::check_train` now make the player-level checks `Sim::apply` makes first (`UnknownPlayer`, `Surrendered`), so a check always agrees with the command. Every fixture was re-recorded at each step and every final hash is unchanged: no fixture trains next to a sealed pocket. Tests: `economy_trained_unit_spawns_in_a_sealed_pocket_only_when_rallied_into_it` and `economy_trained_unit_at_an_enclosed_trainer_follows_the_component_rule` (every branch of the rule), `economy_view_checks_agree_with_apply_for_unknown_and_surrendered_players`, stronger ramp tests (`economy_unit_ramps_count_only_their_kind_class_and_owner`; a non-tail queue `Cancel` in the 1f test), head-only progress with two queued units, and `PopCapReached` and a foreign queue `Cancel` in the rejection test against a control sim.
