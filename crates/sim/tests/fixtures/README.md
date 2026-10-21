# Golden replay fixtures

Each fixture is `<name>.eonreplay` (written by `sim-cli record --scenario <name>`)
with a sibling `<name>.hash` holding the final state hash as `0x<16 hex>` and a
newline. `crates/sim/tests/m1.rs::golden(name)` (and its copies in `m3a.rs`
for `economy_second_town` and `m3b.rs` for `economy_walkthrough`) verifies
the replay and compares the final hash;
`sim-cli verify` does the same from the command line.

| Fixture | Ticks | Scenario (`sim::scenarios`) | Runs in |
| --- | --- | --- | --- |
| `move_500_short` | 300 | the first 300 ticks of `move_500` (seed 42) | `cargo test -p sim` |
| `move_500` | 1200 | 500 Yeomen west to east, Stop for every third id at tick 400 (seed 42) | `sim-cli verify --release` in CI on both OSes (hash-parity job); `cargo test -- --ignored` locally |
| `group_spiral` | 600 | 64 Yeomen to the west edge, then onto a mountain tile (seed 7) | `cargo test -p sim`, `sim-cli verify --release` in CI |
| `snapshot_restore` | 1200 | 200 Yeomen east, 50 more spawned for player 1 after a snapshot/restore swap at tick 300 (seed 7) | `cargo test -p sim`, `sim-cli verify --release` in CI |
| `economy_second_town` | 3600 | from the standard start, mirrored build orders: during tick 0 Yeoman 1 founds a Town at (40, 63) and Yeoman 2 places a Croft at (28, 60), spending exactly the 150 starting Lumber, and the Seat queues a sixth Yeoman (25 Grain); during tick 2400 Yeoman 3 places a Croft at (44, 60), attributed to the new Town; P1 mirrors them with Yeomen 6, 7 and 8 (seed 11) | `cargo test -p sim`, `sim-cli verify --release` in CI |
| `economy_walkthrough` | 2400 | the M3b owner proxy, from the standard start, P0 only: during tick 0 Yeoman 3's Croft at (50, 60) is rejected `OutsideBorders`, Yeoman 1 founds a Town at (40, 63), Yeoman 2 places a Croft at (28, 60) and the Seat queues a Yeoman; during tick 1200 a Lumber grant for five more Crofts; the Town completes at tick 1346; during tick 1400 Yeoman 3 places the Croft at (50, 60) again (accepted) and Yeomen 1, 4, 5 and 11 four more, three Crofts per Town; from tick 2049 six worked Crofts put Grain over the Yield Cap (raw 80, credited 70); during tick 2160 Yeomen are spawned to 24/25 pop, during 2200 a `Train` reaches 25/25 and during 2240 the next is rejected `PopCapReached` (seed 12) | `cargo test -p sim`, `sim-cli verify --release` in CI |

The `snapshot_restore` recording snapshots the sim before tick 300, restores
the bytes into a fresh sim and continues recording on that sim; `verify`
re-simulates without the swap, so an `OK` proves the restore was exact.

Recorded 2026-10-21 at `rules_version` 12, `SIM_VERSION` 11 with
`just rerecord economy_walkthrough` (M3b owner proxy: `economy_walkthrough`
is a first recording of a new scenario, so nothing bumps; no other fixture
was re-recorded).

Recorded 2026-10-14 at `rules_version` 12, `SIM_VERSION` 11 with `just rerecord`
(second review follow-up: a rally point names the spawn side only when its
own tile is passable, and a trainer with every ring tile blocked picks
among all components). Again every final hash and every `.hash` file is
unchanged; only the headers differ.

Recorded 2026-10-14 at `rules_version` 11, `SIM_VERSION` 10 with `just rerecord`
(review follow-up: a trained unit spawns in the rally goal's component when
that touches its building's ring, or else in the largest component touching
the ring, so a sealed pocket takes it only when the rally point is inside).
No fixture trains next to a pocket, so every final hash and every
`.hash` file is unchanged; only the headers differ.

Recorded 2026-10-14 at `rules_version` 10, `SIM_VERSION` 9 with `just rerecord`
(M3a training: `Train`, the queue `Cancel` and `SetRally` have real
handlers and step 6b trains the queue heads). In `economy_second_town` each
Seat now also queues a Yeoman during tick 0, so its final hash changed. The
four M1 fixtures train nothing, and the queue and rally fields have been
hashed since `SIM_VERSION` 6, so `group_spiral`, `move_500`,
`move_500_short` and `snapshot_restore` keep every hash and only their
headers differ.

Recorded 2026-10-14 at `rules_version` 9, `SIM_VERSION` 8 with `just rerecord`
(M3a gathering: idle Yeomen set and renew the hashed `Unit::seek_at` work
timer, a finished builder takes a slot at its building, worked slots earn
income). `move_500` and `snapshot_restore` have new final hashes because
their arrived, idle Yeomen now run the timer. In `economy_second_town` the
Croft builders work their Crofts once the Crofts complete and the idle Yeomen
keep retrying, so its hash changed too. `group_spiral` keeps its final hash
(its units are still walking at the end) but its intermediate hash records
changed; `move_500_short` (no arrival in 300 ticks) keeps every hash and
only its header differs.

Recorded 2026-10-14 at `rules_version` 8, `SIM_VERSION` 7 with `just rerecord`
(a `Build` builder walks only to ring tiles it can reach, and standard-start
units spawn after every Seat; no fixture meets either case, so every final
hash is unchanged from 7/6 and only the headers differ). Before that, the
same day, at `rules_version` 7, `SIM_VERSION` 6 with `just rerecord`
(M3a Town loop: `State` gained hashed fields, namely the stockpiles and
income accumulators, the territory field and the M3a building and unit
fields, and the header gained `MatchSetup.start`; the four M1 scenarios
start empty and use no M3a command, but every `.hash` changed because the
hashed state grew; `economy_second_town` is a first recording).

Recorded 2026-10-14 at `rules_version` 6, `SIM_VERSION` 5 with `just rerecord`
(M3a rules data: `buildings.ron`, the Scribe and Skirmisher, the ramp,
queue, idle-seek and standard-start fields and per-kind vision joined
`rules_hash`; the sim does not read them yet, so every final hash is
unchanged from 5/5 and only the headers differ). Before that, 2026-10-10 at
`rules_version` 5, `SIM_VERSION` 5 (a search suspended
on an older cost grid is requeued instead of resumed, and `AStarSearch.generation`
joined the serialised state; the cost grid never changes in these fixtures, so
every final hash is unchanged from 4/4 and only the headers and the periodic
hash records taken while a search was suspended differ). Before that,
2026-10-06 at `rules_version` 4, `SIM_VERSION` 4 (M2: `visuals.ron`
joined `rules_hash`, and `Command::AttackMove` is applied like `Move` instead
of being rejected; no fixture contains an `AttackMove`, so every final hash is
unchanged from 3/3 and only the headers differ). Before that, the same day,
3/3 (return to post:
an arrived unit keeps a `Post` and walks back to it when pushed more than
`return_to_post_radius_tiles_x100` away; `Unit.post` joined the serialised
state, so every hash changed, including `move_500_short`, whose 300 ticks see
no arrival). Earlier recordings the same day: 1/1 (first creation) and 2/2
(movement constants moved into `rules.ron` and `units.ron`, replay clean-exit
trailer, `Pathing::service` no longer parks a zero-expansion search; final
hashes unchanged from 1/1). Command:
`cargo run -p sim-cli --release -- record --scenario <name> --out crates/sim/tests/fixtures/<name>.eonreplay`,
then the printed `final_hash` goes into `<name>.hash`.

To regenerate after a `rules_version` bump, run `just rerecord` (every
fixture) or `just rerecord <name> ...` (only those). It runs
`scripts/rerecord_fixtures.sh`, which builds `sim-cli` in release and, per
fixture, runs the `record` command above, then `sim-cli verify` on the new
file, and writes the `final_hash` from verify's `OK` line into
`<name>.hash`, so each hash comes from an independent re-simulation rather
than from the recorder. It finishes by checking the working tree against the
merge base with main: if a fixture differs, the `rules_version` in
`data/rules/rules.ron` must be larger than the base's (CI's
`scripts/check_fixture_policy.sh` checks the same on the commits).

Regeneration policy (docs/DETERMINISM.md, "Golden fixtures"): fixtures change
only in a commit that bumps `rules_version` in `data/rules/rules.ron` with a
one-line reason. A changed hash without that bump is a bug. CI runs
`scripts/check_fixture_policy.sh` against the PR base to enforce it.
