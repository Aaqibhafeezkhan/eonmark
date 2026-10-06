# Build times

This document records measured build, test and CI wall times on the development Mac and on GitHub's hosted runners, so contributors know what to expect and so regressions are visible. The local rows were measured on 2026-10-05 during the M0 build; CI rows were filled from the first GitHub Actions runs on 2026-10-06 (cold caches); warm figures are added after the first PR that follows a cached main run; later milestones append rows rather than overwrite. Numbers here are measurements, not gates, except where the design sets a budget (noted in the Notes column). The profile and feature setup that these numbers depend on is explained in [DEPENDENCIES.md](DEPENDENCIES.md) and [ARCHITECTURE.md](ARCHITECTURE.md).

## Machine

| Field | Value |
|---|---|
| Chip | Apple M5 Max, 128 GB RAM |
| macOS | 27.0.1 |
| Xcode | 27 (Command Line Tools; default ld-prime linker) |
| Rust | 1.99.0 (from `rust-toolchain.toml`) |
| Linker overrides | none (`.cargo/config.toml` holds only `MACOSX_DEPLOYMENT_TARGET`) |

## Local builds and tests

| Measurement | Date | Machine | Seconds | Notes |
|---|---|---|---|---|
| Cold `cargo build -p game --features dev` | 2026-10-05 | Apple M5 Max, macOS 27.0.1, Xcode 27, Rust 1.99.0 | 155 (2 min 34 s wall, 2173 s CPU) | Fresh `CARGO_TARGET_DIR`, `--timings`. Only public data point before this: about 5 min on a Ryzen 5600X. |
| Incremental after `touch crates/game/src/main.rs` (no content change) | 2026-10-05 | same | 0.56 | With `dev` (dynamic linking). |
| Incremental after a real source edit in `crates/game` | 2026-10-05 | same | 10.3 | With `dev`; dependencies warm. |
| `cargo build -p game --profile ci` | 2026-10-05 | same | 142 (2 min 22 s wall, 1731 s CPU) | The profile every CI Bevy compile uses. Near-cold: `target/ci` held only clippy metadata. |
| `cargo clippy -p game --features dev --all-targets` | 2026-10-05 | same | 18-20 (0.8 warm) | Dependencies already checked; the higher figure when `rules` needed re-checking. |
| `cargo test -p sim` | 2026-10-05 | same | 0.93 (0.15 warm) | After `touch crates/sim/src/lib.rs`, rebuild plus run, 14 tests. Budget: under 10 s for the non-ignored suite (sim at `opt-level = 3`). |
| `cargo test -p rules` | 2026-10-05 | same | 0.01 run (about 5 compile, cold crate) | 15 tests. |
| `cargo clippy -p sim -p rules -p ai -p sim-cli --all-targets` | 2026-10-05 | same | 0.37 | Warm. |
| `cargo run -q -p sim-cli -- data-check data` | 2026-10-05 | same | 1.8 | Warm target dir, debug profile. |
| `cargo deny check` | 2026-10-05 | same | about 5 | Warm advisory database. |
| `cargo run -p game --profile ci -- --headless-run 200` | 2026-10-05 | same | 3.65 | Including cargo. Budget: under 10 s. M2 switches to a replay fixture. |
| `just ci`, everything warm | 2026-10-05 | same | 7.5 | All steps cached; the cost is the gates themselves, not compilation. |
| Windowed `dev` build, FPS overlay | 2026-10-05 | same | 89-120 fps | 1280x800 logical (Retina 2x), AutoVsync on a 120 Hz display. |

## CI job wall times

| Job | Runner | Date | Cold (s) | Warm (s) | Notes |
|---|---|---|---|---|---|
| `check` | macos-26 (arm64, 3 cores, 7 GB) | 2026-10-06 | 2483 (41 min 23 s) | TBD (first PR after a cached main run) | Run 37395557940. A parallel cold run on the same day (Dependabot PR) took 1569 s (26 min 9 s): macOS runner time varies a lot under capacity constraints. Soft target under 25 min warm. |
| `headless` | ubuntu-24.04 | 2026-10-06 | 997 (16 min 37 s) | TBD | Includes the cold Bevy build for the 200-tick smoke; the engine-free tests alone finish in about a minute. Parallel PR run: 947 s. |
| `game-linux` | ubuntu-24.04 | 2026-10-06 | 240 (4 min 0 s) | TBD | Compile-only clippy. Parallel PR run: 256 s. |
| `release-check` | macos-26 | 2026-10-06 | 812 (13 min 32 s) | TBD | Run 37395557967. Release profile plus `bundle.sh`, plutil, codesign verify. |
| `deny` | ubuntu-24.04 | 2026-10-06 | 54 | 43 | cargo-deny-action, advisory database fetch dominates. |
| `hash-parity` | ubuntu-24.04 | 2026-10-06 | 5 | 6 | Artifact download plus `diff`. Needs `check` and `headless`. |

## Audio backend at M0

| Question | Answer |
|---|---|
| Is `coreaudio-sys` (bindgen) in the graph? | No. cpal 0.17.3 resolves through `coreaudio-rs` 0.14.2 (objc2 bindings, no bindgen). |
| Did the audio backend build against the Xcode 27 SDK? | Yes (2026-10-05). |
| Fallback used (`default-features = false` minus `bevy_audio` and `vorbis`)? | No. |

## Benchmarks

Append a row whenever a bench acceptance passes. Budgets come from the design.

| Bench | Milestone | Date | Mean (ms) | p95 (ms) | Budget | Notes |
|---|---|---|---|---|---|---|
| `step_500_units` (500 movers crossing the map, release) | M1 | TBD (M1) | TBD (M1) | TBD (M1) | mean < 5, p95 < 10 | |
| `astar_budget` (saturated per-tick A* budget) | M1 | TBD (M1) | TBD (M1) | | < 2 | Default budget 4000 expansions; tune in `rules.ron` |
| `territory_recompute` (incremental) | M3a | TBD (M3a) | TBD (M3a) | | < 2 | |
| `step` with 200 vs 200 fighting | M4a | TBD (M4a) | TBD (M4a) | TBD (M4a) | mean < 5, p95 < 10 | |
| AI think time per tick | M5b | TBD (M5b) | TBD (M5b) | | < 0.5 | |
| Frame time, 400 units + two Towns + fog, 1440p release | M7 | TBD (M7) | | | >= 60 fps on a base M-series chip | Draw calls < 500 from the dev-panel counter |

## How to measure

Run from the repository root with the pinned toolchain on `PATH` (see the rustup note in [../CONTRIBUTING.md](../CONTRIBUTING.md)).

Cold build with a per-crate timeline. The HTML report lands in `target/cargo-timings/`.

```bash
cargo clean
/usr/bin/time -p cargo build -p game --features dev --timings
```

Incremental build after a trivial edit.

```bash
touch crates/game/src/main.rs
/usr/bin/time -p cargo build -p game --features dev
```

The CI profile.

```bash
/usr/bin/time -p cargo build -p game --profile ci
```

Sim tests.

```bash
/usr/bin/time -p cargo test -p sim
```

`/usr/bin/time -p` prints `real`, `user` and `sys` in seconds; record `real`. Use the absolute path so the shell builtin does not shadow it. For CI, read the job duration from the GitHub Actions run page; a cold run is one where the `rust-cache` step reports no cache hit.

Benches:

```bash
cargo bench -p sim
cargo run -p sim-cli --release -- bench --units 500 --ticks 1200
```

If a bench fails its budget, profile first (criterion on `sim`, or Bevy's `trace_tracy` feature on `game`) and change algorithms second. Flow fields are justified only by a failing `bench --units 500`.
