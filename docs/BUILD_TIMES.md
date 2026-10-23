# Build times

This document records measured build, test and CI wall times on the development Mac and on GitHub's hosted runners, so contributors know what to expect and so regressions are visible. The local rows were measured on 2026-10-05 during the M0 build; CI rows were filled from the first GitHub Actions runs on 2026-10-06 (cold caches); warm figures come from the first external PR (#17), whose jobs reused the cache written by the main run; later milestones append rows rather than overwrite. Numbers here are measurements, not gates, except where the design sets a budget (noted in the Notes column). The profile and feature setup that these numbers depend on is explained in [DEPENDENCIES.md](DEPENDENCIES.md) and [ARCHITECTURE.md](ARCHITECTURE.md).

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
| `cargo test -p sim` (M1 suite: 57 lib tests, 6 M0 tests, 15 M1 tests: determinism, snapshot/restore, proptests at 32 cases, path oracle, return-to-post convergence, short goldens; 1 long golden ignored) | 2026-10-06 | same | 2.86 (rebuild plus run; 2.06 warm) | Budget: under 10 s. Measured with `/usr/bin/time -p cargo test -p sim` after a source touch; 78 tests run. |
| `cargo test -p sim` (M3a suite: 74 lib tests, 6 M0, 16 M1 with the economy commands in the proptest stream at 32 cases, 8 M2, 47 M3a; 1 long golden ignored; 151 tests run) | 2026-10-14 | 4-vCPU Intel Xeon 2.8 GHz Linux cloud VM, not the dev Mac | 69.4 (rebuild plus run after a touch of `crates/sim/src/lib.rs`; 9.1 to 9.3 warm) | Budget: under 10 s on the dev Mac for the non-ignored suite. Measured with the shell clock (`date` before and after; the VM has no `/usr/bin/time`). The m1 proptests take 7.6 s of the warm run, m3a 0.9 s, everything else under 0.3 s; the rebuild cost is the `opt-level = 3` sim and its five test binaries. The M1 row above measured 2.06 s warm on the dev Mac, and the m1 stream has grown since (Crofts, Build, Cancel, Gather, Train and SetRally), so re-measure on the dev Mac; if it approaches 10 s, lower the local proptest case count before the budget is missed. |
| `cargo test -p sim --profile ci` (M4a suite: 80 lib tests, 6 M0, 16 M1, 8 M2, 51 M3a, 2 M3b, 67 M4a with the counter duels, the m4a proptests and the `combat_skirmish` golden; 1 long golden ignored; 230 tests run) | 2026-10-22 | 4-vCPU Intel Xeon 2.1 GHz Linux cloud VM, not the dev Mac | 9.2 to 9.5 warm (three runs: 9.47, 9.18, 9.36) | Budget: under 10 s on the dev Mac for the non-ignored suite. Measured with the shell clock around a warm `cargo test -p sim --profile ci` (`SIM_VERSION` 14, `rules_version` 16). The m1 proptests take 5.8 s of the run at 16 local cases, m4a 1.5 s, m3a 1.0 s, everything else under 0.3 s each. The local m1 proptest case count went from 32 to 16 in the M4a combat behaviour slice (`proptest_cases()` in `crates/sim/tests/m1.rs`) to keep this run under its 10 s budget; CI still runs 256 (`PROPTEST_CASES=256`, or 256 whenever `CI` is set), so CI coverage is unchanged. The M1 deliverables line in `docs/ROADMAP.md` still reads "32 cases locally" as written at M1; the M1 and M3a rows above were measured at 32. Near the budget on this VM; re-measure on the dev Mac at the M4a close and lower the local proptest case counts if it nears 10 s there. |
| `cargo test -p sim --profile ci` at the M4a close (82 lib tests, 6 M0, 16 M1, 8 M2, 51 M3a, 2 M3b, 68 M4a; 1 long golden ignored; 233 tests run) | 2026-10-22 | same 4-vCPU Intel Xeon 2.1 GHz Linux cloud VM, not the dev Mac | 8.8 to 9.4 warm (three runs: 8.93, 8.81, 9.44) | Budget: under 10 s on the dev Mac for the non-ignored suite. Same shell-clock method, after the slice 4 review fixes (the lib tests `segments_follow_the_straight_walk_not_the_tile_centres` and `path_unreachable_ends_an_explicit_attack_without_a_request_storm`, the m4a test `combat_guard_keeps_a_target_its_straight_chase_can_reach`). The m1 proptests (16 local cases) take 5.9 s, m4a 1.6 s, m3a 0.9 s. Re-measure on the dev Mac. |
| `cargo test -p sim --profile ci` in M4b, local proptest cases lowered (84 lib tests, 6 M0, 16 M1, 8 M2, 51 M3a, 2 M3b, 68 M4a, 5 M4b; 1 long M1 golden and 3 M4b `enemy_outpost` tests ignored; 240 tests run) | 2026-10-22 | same 4-vCPU Intel Xeon 2.1 GHz Linux cloud VM, not the dev Mac | 9.2 to 10.0 warm (ten runs with the VM idle before each batch: 9.54, 9.58, 9.33, 9.15, 9.25, then at the S3 review-fix head 10.00, 9.42, 9.50, 9.51, 9.68; one 10.41 s run came straight after other runs, at load 2.4) | Budget: under 10 s on the dev Mac for the non-ignored suite. Same shell-clock method. At the M4b S3 head, with no sim, rules or ai source changed since the M4a close, the quiet VM measured 10.45 to 10.53 s (m1 6.3 s, m4a 1.5 s, m3a 1.0 s), over the budget; the VM is slower than at the M4a close. The local proptest case counts went from 16 to 8 in `crates/sim/tests/m1.rs` and from 32 to 16 in `crates/sim/tests/m3a.rs` (`proptest_cases()`, the lever the rows above name); with m1 alone at 8 the runs were 8.7 to 10.4 s. CI still runs 256 cases (`PROPTEST_CASES=256`, or 256 whenever `CI` is set), so CI coverage is unchanged. m1 now takes 5.9 s, most of it `same_seed_same_hash_two_threads` (4.5 s alone: five 600-tick `move_500` runs, two of them on two threads), which no case count shortens. Re-measure on the dev Mac. |
| `cargo test -p sim --profile ci` at the M4b close (84 lib tests, 6 M0, 16 M1, 8 M2, 51 M3a, 2 M3b, 68 M4a, 5 M4b; 1 long M1 golden and 3 M4b `enemy_outpost` tests ignored; 240 tests run) | 2026-10-22 | same 4-vCPU Intel Xeon 2.1 GHz Linux cloud VM, not the dev Mac | 9.3 to 10.2 warm (ten runs in two batches: 9.54, 10.19, 9.39, 9.26, 9.29, then 9.52, 9.49, 9.38, 9.74, 10.01) | Budget: under 10 s on the dev Mac for the non-ignored suite. Same shell-clock method, at the M4b S5 head; no sim, rules or ai source changed since the S3 review fixes, so the spread is this VM's noise: m1 took 5.98 to 6.32 s of the runs (8 local proptest cases), m4a 1.5 to 1.6 s, m3a 0.6 to 0.7 s. Two of the ten runs reached 10 s, each straight after other runs. Re-measure on the dev Mac (owner's Mac session); if it goes over 10 s there, the owner allows lowering the local m1 proptest case count again (M5a owner decision 15 of 2026-10-22, numbered in docs/HANDOFF.md until it moves to the ROADMAP's M5a Notes; CI keeps 256). |
| `cargo test -p rules` | 2026-10-05 | same | 0.01 run (about 5 compile, cold crate) | 15 tests. |
| `cargo clippy -p sim -p rules -p ai -p sim-cli --all-targets` | 2026-10-05 | same | 0.37 | Warm. |
| `cargo run -q -p sim-cli -- data-check data` | 2026-10-05 | same | 1.8 | Warm target dir, debug profile. |
| `cargo deny check` | 2026-10-05 | same | about 5 | Warm advisory database. |
| `cargo run -p game --profile ci -- --headless-run 200` | 2026-10-05 | same | 3.65 | Including cargo. Budget: under 10 s. M2 switched CI to the replay fixture row below. |
| `cargo run -p game --profile ci -- --headless-run crates/sim/tests/fixtures/move_500.eonreplay` (1200 ticks, 500 units; the CI hash-parity step since M2) | 2026-10-06 | same | 1.69 (binary alone) | Budget: under 10 s (ROADMAP M2 acceptance). `scripts/m2_checks.sh` check 1: `/usr/bin/time -p target/ci/eonmark --headless-run ...` real 1.69 s, last line `tick=1200 hash=0xc9d94eca7bde8f2e` (= `move_500.hash`), exit 0; the `dev` binary (sim at `opt-level = 3`, dynamic linking) takes 0.75 s; through `cargo run --profile ci` with a warm target dir about 1.9 s. |
| `cargo test -p game` (49 lib tests plus `tests/headless_replay.rs`: `move_500_short` through `SimPlugin` on `MinimalPlugins`, one tick per update) | 2026-10-06 | same | 4.23 (0.67 fully warm) | `/usr/bin/time -p cargo test -p game` after a `touch` of `crates/game/src/main.rs` with dependencies warm (M2 integration build): the test binaries relink, 49 + 1 tests run in 0.18 + 0.21 s. Budget: a few seconds for the non-ignored test; the 1200-tick variant is `#[ignore]` (0.56 s when run with `--ignored`). |
| `just ci`, everything warm | 2026-10-05 | same | 7.5 | All steps cached; the cost is the gates themselves, not compilation. |
| Windowed `dev` build, FPS overlay | 2026-10-05 | same | 89-120 fps | 1280x800 logical (Retina 2x), AutoVsync on a 120 Hz display. |
| Windowed `dev` build, `--scenario units200_auto` frame time (200 Yeomen crossing the map; `frame_stats:` lines printed on exit) | 2026-10-06 | same | mean 8.53 ms, p95 8.72 ms, max 104.79 ms (first frame, shader compile), max at a flush 16.46 ms, dropped ticks 0, 117.2 fps over 7617 frames | Proxy for the ROADMAP M2 line "FPS >= 60, no periodic hitch at the 1 s replay flush": mean under 16.7 ms and p95 well under 33 ms with 0 dropped ticks. From `scripts/m2_checks.sh` check 6 (`--exit-after-seconds 65`, `--replay-dir` in the scratch dir); AutoVsync on the 120 Hz display, World Inspector hidden (its default since M2; F12 shows it). `arrived=200/200 by tick 1199`. With the World Inspector open the same run measured mean 11.5 to 15.9 ms, p95 16.8 to 17.5 ms, 63 to 87 fps: listing 200 units and 200 rings in egui every frame is the single largest cost in the dev build. |
| Windowed `dev` build, `--scenario units200_auto` frame time in background mode (`EONMARK_BACKGROUND=1`, window unfocused, `WinitSettings::continuous()`) | 2026-10-07 | same | mean 8.35 ms, p95 8.76 ms, max 99.52 ms (startup frame 3), max at a flush 8.97 ms, 119.8 fps over 7788 frames, `occluded=0/7788`, ticks 1300 of 1300, dropped ticks 0 | `scripts/m2_checks.sh` check 6, which now gates p95 < 16.7 ms and no covered frames. AutoVsync on the 120 Hz display. |
| Same, unfocused with Bevy's default `WinitSettings::game()` (`reactive_low_power` 1/60 s when unfocused) | 2026-10-07 | same | mean 16.67 ms, p95 17.61 ms, 60.0 fps over 2400 frames (40 s) | The 60 Hz unfocused throttle, not frame cost; the same numbers as the 2026-10-06 acceptance runs whose window had lost focus. |
| Same, background mode, vsync off (`--max-fps 1000`) | 2026-10-07 | same | mean 3.65 ms, p95 6.73 ms, 274.1 fps over 10963 frames (40 s) | The uncapped cost of 200 units. A `sample` profile of 8 s shows the render thread busy most of the frame and the game's own systems under 1 % of samples. |
| Windowed `ci` build under Xvfb with Mesa lavapipe, `--scenario enemy_outpost` (the M4b 200 vs 200 prologue at its default framing, 106,32 at 55 m; background mode; the `frame_stats: fight` line of ROADMAP M4b line 2) | 2026-10-22 | 4-vCPU Intel Xeon 2.1 GHz Linux cloud VM, llvmpipe (LLVM 20.1.2, Mesa 25.2.8) software Vulkan, 1280x800; not the dev Mac | Dev build (`--features dev`, `--exit-after-seconds 240`): fight 4.5 fps, mean 224.28 ms, p95 258.96 ms, max 279.74 ms over 100 frames (22.43 s, ticks 151..576), `on_screen_max=400`, `units_at_first_hit=200/200`; whole run 6.3 fps, p95 215.03 ms, `ticks=4797 dropped_ticks=0`, `hierarchy_check: ok frames=1520 bar_targets_max=37 rings_max=0`. `ci` build with `--max-fps 1000` (90 s): fight 6.0 fps, mean 167.42 ms, p95 191.37 ms over 133 frames, `on_screen_max=400`; whole run 6.5 fps | A Linux proxy that can neither pass nor fail ROADMAP M4b line 2 (>= 60 fps on the dev Mac, owner decisions 2 and 3): lavapipe renders in software, so these frame times measure the CPU rasteriser, not the game. What the proxy does show: the fight window opens at the first `Hit` (tick 151, after the charge at tick 100) with both armies whole and all 400 units on screen, it closes when the vanguard drops under 50 armed units (tick 576), the sim keeps 20 ticks a second with 0 dropped ticks throughout, no health bar or ring ever has a parent, and every run's replay verifies (`OK final_hash=0xdea3c3cd4a7725cf ticks=4797`, `OK final_hash=0xb2cb726b4d0c1695 ticks=1801`). `window focused=0/<n>` because background mode is unfocused by design. The log says `GPU preprocessing is fully supported on this device.` The scripted `enemy_outpost_walkthrough` run (which moves the camera to a 30 m close-up at tick 300) measured fight 5.4 fps, p95 220.15 ms over 121 frames with `on_screen_max=400`. The real numbers come from the owner's Mac run in docs/PLAYTEST.md, "Owner's Mac checklist (M4b)". |
| Same, re-run at the M4b S5 review-fix head, after the `fight` line gained `on_screen_min` | 2026-10-22 | same VM, llvmpipe software Vulkan, 1280x800; not the dev Mac | Dev build (`--exit-after-seconds 240`): fight 6.0 fps, mean 166.34 ms, p95 196.50 ms, max 239.82 ms over 134 frames (22.29 s, ticks 151..576), `on_screen_max=400 on_screen_min=400/400`, `units_at_first_hit=200/200`; whole run 6.6 fps, p95 174.77 ms, `ticks=4802 dropped_ticks=0`, `hierarchy_check: ok frames=1592 bar_targets_max=37 rings_max=0`. `ci` build with `--max-fps 1000` (90 s): fight 6.2 fps, mean 162.55 ms, p95 179.38 ms over 138 frames, `on_screen_min=400/400`; whole run 6.8 fps | Still a proxy that can neither pass nor fail ROADMAP M4b line 2. `on_screen_min=400/400`: at the default framing every timed fight frame drew every armed unit then alive. The scripted walkthrough, whose director moves the camera to a 30 m close-up at tick 300, now prints `on_screen_max=400 on_screen_min=304/348` (fight 5.7 fps, p95 194.08 ms over 126 frames), where the S5 line printed only `on_screen_max=400`. The dev build's fight rate went from 4.5 to 6.0 fps with no render change: lavapipe frame times swing with this VM. The sim's share of the frame is the M4b row of the combat bench under Benchmarks (about 1.1 ms per fighting tick). Replays verify: `OK final_hash=0x0e4fa9d8547bdc9e ticks=4802`, `OK final_hash=0x987f0c9b46195352 ticks=1800`, `OK final_hash=0x67e818f30bbc0819 ticks=6602`; the log says `GPU preprocessing is fully supported on this device.` |

## CI job wall times

| Job | Runner | Date | Cold (s) | Warm (s) | Notes |
|---|---|---|---|---|---|
| `check` | macos-26 (arm64, 3 cores, 7 GB) | 2026-10-06 | 2483 (41 min 23 s) | 126 (2 min 6 s) | Run 37395557940 (cold); warm figure from PR #17 run 37421181508 reusing main's rust-cache. A parallel cold run on the same day (Dependabot PR) took 1569 s (26 min 9 s): macOS runner time varies a lot under capacity constraints. Soft target under 25 min warm. |
| `headless` | ubuntu-24.04 | 2026-10-06 | 997 (16 min 37 s) | 74 | Includes the cold Bevy build for the 200-tick smoke; the engine-free tests alone finish in about a minute. Parallel PR run: 947 s. |
| `game-linux` | ubuntu-24.04 | 2026-10-06 | 240 (4 min 0 s) | 41 | Compile-only clippy. Parallel PR run: 256 s. |
| `game-linux` with the render smoke test | ubuntu-24.04 | 2026-10-21 | 1181 (19 min 41 s) | not yet measured | Since M3b the job also builds the `ci` binary and runs `scripts/render_smoke.sh` (60 s under Xvfb and lavapipe). Run 39533356188 (PR #31, a7f1499): clippy 229 s, the render smoke step 911 s including the cold build. The job's cache key became `game-linux-build`, so main's next run saves a 1 GB cache with the built binary and later runs should be warm. Still shorter than `check` on macOS (29 min on the same run), so the PR's wall time is unchanged. |
| `release-check` | macos-26 | 2026-10-06 | 812 (13 min 32 s) | 345 (5 min 45 s) | Cold: run 37395557967. Warm: run 37464362381 on main (9f63e6c), rust-cache hit. Release profile plus `bundle.sh`, plutil, codesign verify. |
| `deny` | ubuntu-24.04 | 2026-10-06 | 54 | 46 | cargo-deny-action, advisory database fetch dominates. |
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
| `step_500_units` (500 movers crossing the map through all three fords, release; `sim-cli bench --units 500 --ticks 1200`) | M1 | 2026-10-06 | 0.511 | 0.861 | mean < 5, p95 < 10 | max 1.191 ms; `SIM_VERSION` 3, `rules_version` 3 (return to post). Arrival: 35.0% at tick 1200, 100% (500/500, 0 moving, 0 displaced) at tick 2400; >= 99% first held at tick 1624. Mean over 2400 ticks 0.350 ms, p95 0.837 ms. Earlier single-ford stream: 0.409 / 0.852 ms, 85.4% arrived at 1200 with 25 units displaced before return to post existed. |
| `astar_budget` (one saturated 4000-expansion A* tick, release; `sim-cli bench --astar --budget 4000`) | M1 | 2026-10-06 | 0.309 | 0.351 | < 2 | max 0.501 ms over 100 iterations toward a blocked goal (`expansions_used=4000 suspended=100`); earlier runs 0.244 / 0.336 and 0.276 / 0.372 ms. |
| `territory_recompute` (incremental; `sim-cli bench --territory --sources 16 --edits 100`, release) | M3a | 2026-10-10 | 0.036 | 0.057 | < 2 | Measured on a 4-vCPU Intel Xeon 2.1 GHz Linux cloud VM, not the dev Mac: three runs gave mean 0.0363 / 0.0356 / 0.0368 ms, p95 0.0567 / 0.0553 / 0.0573 ms, max under 0.095 ms; full recompute of the 16 sources 0.20 to 0.47 ms; `incremental_matches_full=true` every run. `--sources 64 --edits 1000 --seed 9`: mean 0.0387 ms. Criterion (`cargo bench -p sim --bench territory_recompute`): 43 us per tier toggle (two `change_source` calls). Re-measure on the dev Mac at the M3a close. |
| `territory_recompute` at the M3a close (same command, `SIM_VERSION` 9, `rules_version` 10) | M3a | 2026-10-14 | 0.054 | 0.085 | < 2 | Same class of Linux cloud VM (4-vCPU Xeon 2.8 GHz), measured while other builds ran on the machine: max 0.133 ms, full recompute 0.31 ms, `incremental_matches_full=true`. Still not measured on the dev Mac. |
| `step` with 200 vs 200 fighting (`sim-cli bench --units 400 --ticks 1200 --combat`, release: `sim::scenarios::combat_bench`, four line kinds a side attack-moving at each other in the west rectangle of `plains_1v1`) | M4a | 2026-10-22 | 0.52 to 0.58 | 1.70 to 2.03 | mean < 5, p95 < 10 | Measured on a 4-vCPU Intel Xeon 2.1 GHz Linux cloud VM, not the dev Mac; `SIM_VERSION` 14, `rules_version` 16, Rust 1.99.0. Three runs at seed 1: mean 0.579 / 0.515 / 0.577 ms, p95 1.913 / 1.698 / 2.026 ms, max 4.744 / 5.736 / 3.088 ms; `fighting_ticks=501` (ticks with at least one `Hit` while both sides keep at least 50 units), fighting mean 1.232 / 1.070 / 1.194 ms, fighting p95 2.170 / 1.800 / 2.251 ms; `hits=2836 deaths_p0=173 deaths_p1=200`, `final_hash=0x9e1b5bfe9bb8c731` every run. Seed 2: mean 0.508, p95 1.695, max 3.984, 490 fighting ticks at mean 1.065 and p95 1.820 ms. Seeds 1 to 10 gave 454 to 501 fighting ticks (the exit-1 threshold is 100). The costliest stretch is the approach and first clash with all 400 alive (ticks 0 to 49: mean 2.1 ms); the step falls with the unit count, and the battle is over by about tick 800. Criterion (`cargo bench -p sim --bench combat_step`, the 20 steps from the first fighting tick): 32.8 ms, 1.64 ms a step. A callgrind run of the whole bench (instruction counts, not time) put 59% in `movement::step`, including 14% in `SpatialGrid::neighbors` (about 7 points growing its result vector and 4 sorting it); about 32% in `Sim::step` itself, where combat's think and resolve are inlined (`combat::within` 4%); and 4% in pathing (960 searches, mostly clearing the search grids). Re-measure on the dev Mac at the M4a close. |
| `step` with 200 vs 200 fighting at the M4a close (same command, after the direct-chase review fix; `SIM_VERSION` 14, `rules_version` 16) | M4a | 2026-10-22 | 0.502 to 0.506 | 1.71 to 2.15 | mean < 5, p95 < 10 | Same 4-vCPU Intel Xeon 2.1 GHz Linux cloud VM, not the dev Mac. Two runs at seed 1: mean 0.502 / 0.506 ms, p95 2.152 / 1.713 ms, max 4.056 / 2.962 ms; `fighting_ticks=501`, fighting mean 1.013 / 1.060 ms, fighting p95 2.287 / 1.859 ms; `hits=2836 deaths_p0=173 deaths_p1=200 destroyed=0` and `final_hash=0x9e1b5bfe9bb8c731`, the same as before the fix. Re-measure on the dev Mac. |
| `step` with 200 vs 200 fighting at the M4b close (same command; `SIM_VERSION` 14, `rules_version` 17), the sim's share of ROADMAP M4b line 2's frame | M4b | 2026-10-22 | 0.51 to 0.56 | 1.66 to 2.40 | mean < 5, p95 < 10 | Same 4-vCPU Intel Xeon 2.1 GHz Linux cloud VM, not the dev Mac, idle. Three runs at seed 1: mean 0.514 / 0.560 / 0.561 ms, p95 1.664 / 1.752 / 2.403 ms, max 3.061 / 5.012 / 4.626 ms; `fighting_ticks=501`, fighting mean 1.073 / 1.152 / 1.141 ms, fighting p95 1.733 / 1.894 / 2.549 ms; `final_hash=0x9e1b5bfe9bb8c731`, the M4a hash (no sim or scenario source changed since; the `rules_version` 17 bump is the unit looks). Cited as the sim's share of the line-2 frame: at 60 fps one frame in three carries a tick, and a fighting tick takes about 1.1 ms of the 16.7 ms, so the render-side levers come first (docs/PLAYTEST.md, "Owner's Mac checklist (M4b)"). |
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
cargo run -p sim-cli --release -- bench --units 400 --ticks 1200 --combat
```

If a bench fails its budget, profile first (criterion on `sim`, or Bevy's `trace_tracy` feature on `game`) and change algorithms second. Flow fields are justified only by a failing `bench --units 500`.
