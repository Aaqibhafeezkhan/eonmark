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
8. Derived caches (influence maps, spatial grid, path cache, component ids) are never hashed. The path cache key includes `cost_grid_generation` and the whole cache is cleared on any cost-grid mutation. `restore()` rebuilds every derived structure before returning. A property test runs the sim with the path cache disabled and enabled and asserts identical hashes.
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

The cost grid carries a `u32` `cost_grid_generation` that increments on every change (a building placed or destroyed, a tile blocked). The path cache is keyed by `(start_tile, goal_tile, cost_grid_generation)` and is cleared wholesale on any mutation. Therefore a cache hit returns exactly what a fresh search would return. A cache keyed by `(start, goal)` alone would return stale paths after a building changes the grid and would make the result depend on history, which breaks the snapshot/restore rule.

### Snapshot and restore rule

`snapshot()` serializes every hashed field verbatim with postcard. `restore()` deserializes them and then eagerly rebuilds influence maps, the spatial grid, the supply grid and connected components, and clears the path cache, before returning. No derived structure is lazily rebuilt, so the first `step` after restore sees the same inputs as the uninterrupted run. The command delay queue (`pending`, a `BTreeMap` keyed by application tick) is hashed state, so a snapshot carries in-flight commands and a restore applies them on the right tick.

## Hashing

`Sim::hash()` is `xxh3_64` over the postcard serialization of all hashed state. Postcard is canonical for a given struct, and every container in the state is ordered, so equal states produce equal bytes.

`Sim::sub_hashes()` returns one hash per subsystem so a divergence can be localized.

| Sub-hash | Covers |
|---|---|
| `units` | The `BTreeMap<UnitId, Unit>` |
| `buildings` | The `BTreeMap<BuildingId, Building>` |
| `economy` | Stockpiles, accumulators, cap state, ramping counters |
| `territory` | `TerritoryField` strength layers and owners |
| `tech` | Researched techs, queues, ages |
| `pathing` | The path request queue (not the cache) |
| `rng` | The `Pcg32` state |

Hashes are taken every 20 ticks in release and every tick under `--hash-every-tick` (flag arrives in M2 with the replay writer).

At M0 the hashed state is the full `State` (`tick`, `seed`, `rng`, `ids`, `players`, `units`, `buildings`, `pending`, `commands_applied`; see "What exists at M0" below) and `hash()` is already the real scheme. Sub-hashes arrive in M1.

## Replays

A replay is a postcard stream: a `MatchSetup` header, then one `TickCommands` batch per tick, a hash record every 20 ticks, and a final hash.

### Header fields

| Field | Type | Compared by `verify` | Meaning |
|---|---|---|---|
| `sim_version` | `u32` | Yes, fails fast | Hand-bumped whenever sim behaviour changes for the same inputs. Constant `sim::SIM_VERSION`. |
| `git_sha` | `String` | Never | Informational. Comparing it would invalidate every fixture on every commit. |
| `rules_version` | `u32` | Reported | Human label from `rules.ron`, bumped when goldens regenerate. |
| `rules_hash` | `u64` | Yes, reports `RULES CHANGED` | xxh3 over postcard of the loaded `Rules`, covering every file `Rules::load` reads: match rules, resources and every map at M0, and `data/ai` once it is loaded (M5b). |
| `map` | `String` | Loaded | Map id under `data/maps/`. |
| `seed` | `u64` | Loaded | Seed for the `Pcg32`. |
| `tick_rate_hz` | `u32` | Loaded | 20. |
| `cmd_delay` | `u32` | Loaded | Ticks between issue and application. |
| `players` | `Vec<PlayerSlot>` | Loaded | Slot id and `is_ai` per player, in id order. `MatchSetup::skirmish` makes slot 0 human and slot 1 the AI. |

AI difficulty ids (M6) and faction ids (M9) are added to the header when those systems arrive. The same struct is the future lockstep handshake payload.

Why two version fields: a git sha changes every commit, so it cannot gate fixtures. A manually bumped `rules_version` can be forgotten, so an unbumped RON edit would masquerade as a sim divergence. The content hash catches that case with its own message.

### Writer

The replay is written by a dedicated `std::thread` fed by a channel of encoded tick batches. It writes and flushes every 20 ticks and calls `sync_all` only at clean exit. It never fsyncs during play: on Apple platforms both `sync_all` and `sync_data` issue `fcntl(F_FULLFSYNC)`, which costs milliseconds and would hitch the frame once a second for no benefit. Surviving `kill -9` needs only written pages, which a flush provides. A hard kill loses at most one second of commands. The writer is purely a sink: nothing reads from it, and the sim does not wait for it.

Replays go to `ProjectDirs::from("com", "tonianev", "Eonmark").data_dir()/replays/<timestamp>.eonreplay`, or to `--replay-dir <path>`. CI uses a temporary directory.

### The four verify outcomes

```bash
cargo run -p sim-cli --release -- verify path/to/match.eonreplay
```

| Output | Exit code | Meaning |
|---|---|---|
| `OK final_hash=0x...` | 0 | Re-simulation reproduced every recorded hash. |
| `DIVERGED at tick N (subsystem: economy)` | 1 | The first recorded hash that did not match, with the first differing sub-hash. |
| `SIM VERSION MISMATCH` | 1 | `header.sim_version != SIM_VERSION`. The replay predates a behaviour change; nothing is simulated. |
| `RULES CHANGED since recording` | 1 | `header.rules_hash != Rules::load(...).rules_hash()`. Someone edited RON without bumping `rules_version` and regenerating. |

## Bisecting a desync

When `verify` prints `DIVERGED at tick N (subsystem: X)`, the recorded hash at tick N differs, but hashes are only recorded every 20 ticks in release. Narrow it down with `hash-dump`. The replay-reading form below arrives in M1; at M0, `sim-cli hash-dump --ticks N --seed S` prints the whole-state hash after every tick of the scripted selftest match, which is already enough to compare two machines line by line.

```bash
# 1. Per-tick sub-hashes from the recording side (if it was recorded with --hash-every-tick)
#    or from a fresh run of the same setup on the machine that produced the replay.
cargo run -p sim-cli --release -- hash-dump match.eonreplay --every 1 > a.txt

# 2. The same dump on the machine or build under suspicion.
cargo run -p sim-cli --release -- hash-dump match.eonreplay --every 1 > b.txt

# 3. First differing line is the first tick and subsystem that drifted.
diff a.txt b.txt | head -n 5
```

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

The `check` job on macOS and the `headless` job on Ubuntu each upload the final hash line of the headless run (the 200-tick `--headless-run 200` smoke at M0; fixture replays from M1). The `hash-parity` job downloads both and `diff`s them. Any difference fails CI. This job is never negotiable.

## Golden fixtures

Fixtures live under `crates/sim/tests/fixtures/` as `.eonreplay` files with a sibling `.hash` file. Short fixtures (at most 5000 ticks) run inside `cargo test -p sim`. Long ones (for example `m5_first_match`) are `#[ignore]` locally and verified by `sim-cli verify` in release in the headless CI job.

Regeneration policy:

- Fixtures regenerate only in a commit that bumps `rules_version` in `data/rules/rules.ron` and states the reason in one line of the commit message.
- A changed hash without that bump is a bug, not a fixture update. Find it with the bisect procedure above.
- `RULES CHANGED since recording` means someone edited RON without bumping. Either revert the edit or bump and regenerate in the same commit.
- A behaviour change in Rust that is intended (a new movement rule, a fixed bug) bumps `SIM_VERSION` as well, and the commit regenerates every fixture.

## What exists at M0

The three `clippy.toml` files and the `[profile]` overrides. `Fx` and `FxVec2`. Monotonic ids from `IdGen`, which is itself hashed state so a restore never reuses an id. The full `Command` enum and `sort_commands`. The `MatchSetup` header with every field in the table above. A `Sim` whose hashed state is `{ tick, seed, rng: Pcg32, ids, players, units, buildings, pending, commands_applied }`; `step` queues commands for `tick + cmd_delay`, stamps and queues the AI's commands the same way, and applies everything due at the current tick in `(player, seq)` order. `hash()` as xxh3 over postcard, `snapshot()` and `restore()` with an empty `rebuild_derived`. `sim-cli selftest --ticks N --seed S` runs the scripted match twice on two threads and compares; `sim-cli hash-dump --ticks N --seed S` prints the per-tick hash. `sub_hashes`, the replay format, `verify` and the test list in rule 10 arrive in M1.
