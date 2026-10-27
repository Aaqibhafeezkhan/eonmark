# Golden replay fixtures

Each fixture is `<name>.eonreplay` (written by `sim-cli record --scenario <name>`)
with a sibling `<name>.hash` holding the final state hash as `0x<16 hex>` and a
newline. `crates/sim/tests/m1.rs::golden(name)` (and its copies in `m3a.rs`
for `economy_second_town`, `m3b.rs` for `economy_walkthrough`, `m4a.rs`
for `combat_skirmish`, `m4b.rs` for `enemy_outpost_walkthrough` and
`m5a.rs` for `capture_seat` and `stalemate_45min`, and
`crates/ai/tests/m5b.rs::golden_m5_first_match`, which rebuilds the bots
from the header with `ai::controller_for`) verifies the replay and
compares the final hash;
`sim-cli verify` does the same from the command line.

| Fixture | Ticks | Scenario (`sim::scenarios`) | Runs in |
| --- | --- | --- | --- |
| `move_500_short` | 300 | the first 300 ticks of `move_500` (seed 42) | `cargo test -p sim` |
| `move_500` | 1200 | 500 Yeomen west to east, Stop for every third id at tick 400 (seed 42) | `sim-cli verify --release` in CI on both OSes (hash-parity job); `cargo test -- --ignored` locally |
| `group_spiral` | 600 | 64 Yeomen to the west edge, then onto a mountain tile (seed 7) | `cargo test -p sim`, `sim-cli verify --release` in CI |
| `snapshot_restore` | 1200 | 200 Yeomen east, 50 more spawned for player 1 after a snapshot/restore swap at tick 300 (seed 7) | `cargo test -p sim`, `sim-cli verify --release` in CI |
| `economy_second_town` | 3600 | from the standard start, mirrored build orders: during tick 0 Yeoman 1 founds a Town at (40, 63) and Yeoman 2 places a Croft at (28, 60), spending exactly the 150 starting Lumber, and the Seat queues a sixth Yeoman (25 Grain); during tick 2400 Yeoman 3 places a Croft at (44, 60), attributed to the new Town; P1 mirrors them with Yeomen 6, 7 and 8 (seed 11) | `cargo test -p sim`, `sim-cli verify --release` in CI |
| `economy_walkthrough` | 2400 | the M3b owner proxy, from the standard start, P0 only: during tick 0 Yeoman 3's Croft at (50, 60) is rejected `OutsideBorders`, Yeoman 1 founds a Town at (40, 63), Yeoman 2 places a Croft at (28, 60) and the Seat queues a Yeoman; during tick 1200 a Lumber grant for five more Crofts; the Town completes at tick 1346; during tick 1400 Yeoman 3 places the Croft at (50, 60) again (accepted) and Yeomen 1, 4, 5 and 11 four more, three Crofts per Town; from tick 2049 six worked Crofts put Grain over the Yield Cap (raw 80, credited 70); during tick 2160 Yeomen are spawned to 24/25 pop, during 2200 a `Train` reaches 25/25 and during 2240 the next is rejected `PopCapReached` (seed 12) | `cargo test -p sim`, `sim-cli verify --release` in CI |
| `combat_skirmish` | 3000 | the M4a combat golden, empty start: during tick 0 P0 places a complete Scriptorium (2, 2) and Engine Yard (30, 12), gets stock and queues Letters I; P1 places a complete Town (22, 38) beside the field and a Watchtower (40, 25); each side spawns six Skirmishers, Shieldbearers, Bowmen and Outriders with `DebugSpawnJittered` (P0's `UnitId(1..=24)` west, P1's `25..=48` mirrored east) and P1 50 Yeomen far south-east; during tick 5 the lines attack-move into each other (the Town shoots into the fight); during tick 150 P0's Bowmen Stop and P1's Outriders Move away; Letters I completes at 501, Trade I (queued during 520) at 1061, Statecraft I queued behind it is cancelled during 540; P0 enters the Masonry Age at 1102 and trains a Mangonel (`UnitId(99)`, queued during 1110, out at 1911), which attacks the Watchtower during 1920 and destroys it with five hits at 2363 while the Yeomen, ordered across the river during 2300, keep a path search suspended (seed 21) | `cargo test -p sim`, `sim-cli verify --release` in CI on both operating systems (hash parity) |
| `enemy_outpost_walkthrough` | 5039 | the M4b owner proxy, from the standard start (seed 13). During tick 0 P0 `DebugSpawnJittered`s a 200-unit vanguard on P1's side of the river (`UnitId(11..=210)`, 50 Skirmishers, Shieldbearers, Bowmen and Outriders, no orders) and is granted 1.5x the walkthrough's bill; P1 gets Arms I to III, a complete Watchtower at (58, 64) (`BuildingId(3)`) and a mirrored army (`211..=410`), which attack-moves into the vanguard during tick 100 and wipes it during tick 711 (P0's pop 255 / 25 back to 5 / 25). P0's real commands, at beat ticks derived from the rules (`enemy_outpost_beats`): during tick 2 Yeomen 1 and 2 place a Scriptorium (27, 58) and a Muster Hall (28, 68) (`BuildingId(4)`, `5`, complete at 635 and 845) and Yeoman 3 pre-walks; Letters I queued during 724 (done 1225); the Muster Hall rallies to (47, 63) and queues two Skirmishers, two Bowmen and a Shieldbearer during 964 (`411..=415`); Trade I queued during 1266 at the Letters price (540 ticks, done 1807); during 1848 P0 enters the Masonry Age and Yeoman 3 places an Engine Yard (40, 62) (`BuildingId(6)`, complete at 2865); two Mangonels queued during 2970 (`416`, `417`, out at 3771 and 4571); during 4692 the Mangonels attack the Watchtower, which they outrange, and the mixed army attack-moves beside it; the tower is destroyed during 4839. Nothing is rejected | `cargo test -p sim -- --ignored enemy_outpost` and the game's `--headless-run` in CI's headless job, `sim-cli verify --release` in CI on both operating systems |
| `capture_seat` | 1628 | the M5a capture golden, from the standard start (seed 31): during tick 0 P1 researches Harrying I (`DebugResearch`); P0 removes P1's five Yeomen and takes P1's Seat (`BuildingId(2)`) to 300 HP with `DebugDamage`, spawns four Mangonels (`UnitId(11..=14)`) that `Attack` the Seat from out of its range and take it to 0 during tick 146, eight Skirmishers (`15..=22`) and a Supply Wain (`23`) inside P1's border, and a Bowman (`24`) outside the Wain's cover, which bleeds; during tick 300 the Skirmishers walk onto eight tiles of the Seat's ring and the Wain follows. The annexation timer starts in step 427 and the Seat flips in step 1627, which decides `Victory { P0, decisive: true }`, reason `SeatCaptured`; `sim-cli record` stops there (`Scenario::stops_at_outcome`), so the fixture ends at tick 1628 and its hash is the state the game's outcome overlay shows. Nothing is rejected | `cargo test -p sim`, `sim-cli verify --release` in CI on both operating systems (hash parity) |
| `stalemate_45min` | 54000 | the M5a 45-minute golden, empty start (seed 45), no unit: during tick 0 P0 places a complete Town at (23, 63) and a Watchtower at (43, 64), P1 a Town at (102, 63), so P0 owns 1248 tiles and P1 1245; the match cap (`match_cap_ds`, 54000 ticks) decides `Victory { P0, decisive: false }`, reason `Territory`, in the last step. Recorded with a hash every 1000 ticks (`Scenario::default_hash_every`) | `cargo test -p sim -- --ignored golden_stalemate_45min` locally, `sim-cli verify --release` in CI on both operating systems (hash parity); `annexation_54000_tick_scripted_stalemate_is_a_tiebreak_victory_for_more_territory` runs the same stream in `cargo test -p sim` |
| `m5_first_match` | 26012 | the M5b first-match golden (ROADMAP M5b acceptance 1): the Standard bot (personality `standard`) in both seats from the standard start (seed 1), no human input and no debug command; a `sim-cli` scenario (`Scenario::FirstMatch`), not in `sim::scenarios::NAMES`, since the sim cannot build bots, so the header's `ai_seats` name the bots and every re-simulation rebuilds them. P0 reaches Masonry at tick 7933 and P1 at 7948; their first waves (P1's launched at 12616, P0's at 12631) clash on the way and both end with two units left before their assault; P0 launches four waves and P1 two; P0's third wave leaves P1's Seat (`BuildingId(2)`) at 656 HP, and its fourth (22 armed units, launched at 23431) finds it at 0 and holds its ring: the annexation timer starts in step 24349 and is reset three times, its last start is in step 24811 and the Seat flips in step 26011, which decides `Victory { P0, decisive: true }`, reason `SeatCaptured`; `sim-cli record` stops there (`Scenario::stops_at_outcome`), so the fixture ends at tick 26012. Recorded with a hash every 100 ticks (`Scenario::default_hash_every`). `play-bots --seeds 1..5 --a standard --b standard` prints this final hash for seed 1 | `cargo test -p ai -- --ignored golden_m5_first_match` locally, `sim-cli verify --release` in CI on both operating systems (hash parity) and the game's `--headless-run` in CI's headless job |

The `snapshot_restore` recording snapshots the sim before tick 300, restores
the bytes into a fresh sim and continues recording on that sim; `verify`
re-simulates without the swap, so an `OK` proves the restore was exact.

Recorded 2026-10-26 at `rules_version` 25, `SIM_VERSION` 17 with
`just rerecord` (ADR 0003, `docs/adr/0003-visuals-outside-rules-hash.md`:
`data/visuals.ron` left `rules_hash`; no value and no sim code changed).
Each of the eleven files differs from its `rules_version` 24 recording in
the header's `rules_version` and `rules_hash` only (10 bytes, offsets 8 to
17; every other byte is equal), and no `.hash` file changed. A per-tick
`sim-cli hash-dump --every 1` of `move_500`, `combat_skirmish`,
`capture_seat` and `m5_first_match` as recorded at 24, run by the
pre-bump binary with its own `data/`, equals that of the re-recorded
fixture run by the new binary on every tick. From here an edit of
`data/visuals.ron` alone needs no bump and no re-record.

Recorded 2026-10-23 at `rules_version` 24, `SIM_VERSION` 17 with
`just rerecord` (the M5b waves slice's review fixes: `data/ai/` gained the
`defend_guard_pop_ratio` knob, and the Standard bot's defence, build-order
saving and site choice changed in `crates/ai`; no fixture but
`m5_first_match` has a Standard seat). Each of the ten earlier files
differs from its `rules_version` 23 recording in the header's
`rules_version` and `rules_hash` only (9 bytes; every other byte is
equal), and no `.hash` file changed but `m5_first_match`'s
(`0x5a690149db4330e6` to `0x3d2552033d51b4a2`), which is new in the M5b
pull request, so it is re-recorded with the bots' changes and needs no
bump of its own (plan decision 5: `SIM_VERSION` versions bot code once
M5b is merged).

Recorded 2026-10-23 at `rules_version` 23, `SIM_VERSION` 17 with
`just rerecord` (M5b waves slice: only `data/ai/` changed, the Standard
build order, and no other fixture has a Standard seat). Each of the ten
earlier files differs from its `rules_version` 22 recording in the
header's `rules_version` and `rules_hash` only (10 bytes; every other byte
is equal), and no `.hash` file changed. `m5_first_match`
(`0x5a690149db4330e6`, `just rerecord m5_first_match`) is the first
recording of a new scenario, which needs no bump by itself.

Recorded 2026-10-23 at `rules_version` 22, `SIM_VERSION` 17 with
`just rerecord` (M5b economy slice: only `data/ai/` changed, the Standard
build order and `resource_radius_tiles`, and no fixture has a Standard
seat). Each file differs from its `rules_version` 21 recording in the
header's `rules_version` and `rules_hash` only (10 bytes before, 11 after;
every other byte is equal), and no `.hash` file changed.

Recorded 2026-10-23 at `rules_version` 21, `SIM_VERSION` 17 with
`just rerecord` (M5b bot plumbing: the header gained `ai_seats` as its last
field and every `Player` the hashed `ai_memory`, in the `meta` sub-hash;
`data/ai` joined `rules_hash`). No fixture has a bot that writes a memory,
so nothing behaves differently: a per-tick `sim-cli hash-dump --every 1`
of each fixture as recorded at `SIM_VERSION` 16, run by that binary with its
own `data/`, against the re-recorded fixture run by the `SIM_VERSION` 17
binary with the current `data/`, differs in the whole hash and the `meta`
column on every tick and in no other column. Every `.hash` file changes for
that reason only: `capture_seat` `0xbd9f9df3f98fcc23`, `combat_skirmish`
`0x9c42bb7bfd4dcef3`, `economy_second_town` `0x4b3429bcfd9ec100`,
`economy_walkthrough` `0x88bb89e34ec941bd`, `enemy_outpost_walkthrough`
`0xd0684c93f3398915`, `group_spiral` `0xf468c100c2b042e1`, `move_500`
`0x02e11c8e91350d52`, `move_500_short` `0x46eaeacec41e1b86`,
`snapshot_restore` `0x9625510823fec01f`, `stalemate_45min`
`0x261fad6cc4323139`.

Recorded 2026-10-23 at `rules_version` 20, `SIM_VERSION` 16 with
`just rerecord` (M5a annexation and outcome: `TownState` gained the hashed
`hold` and `State` the hashed `outcome`, in the `meta` sub-hash; step 5d
annexes Towns at 0 HP, step 6e decides the match, and the annex-hold guard
rule applies beside an enemy Town at 0 HP). None of the eight re-recorded
fixtures' Towns ever loses a hit point, so in them no timer starts, the
hold rule never applies and nothing is decided (the two new recordings
below are decided: `capture_seat` takes P1's Seat to 0 and annexes it,
`stalemate_45min` ends at the cap): re-simulating the eight fixtures emits
no `TownAtZeroHp`, annexation event or `MatchDecided`. A per-tick `sim-cli
hash-dump --every 1` of each fixture as recorded at `SIM_VERSION` 15, run
by that binary with its own `data/`, against the re-recorded fixture run by
the `SIM_VERSION` 16 binary with the current `data/`, differs in the whole hash
and the `meta` column on every tick, in the `buildings` column on every
tick with a complete Town (the four fixtures with one) and in no other
column, and the buildings and meta hashed without the new fields equal the
old columns on every tick. Every `.hash` file changes for that reason
only: `combat_skirmish` `0xb200f42aaaf0f1db`, `economy_second_town`
`0x757fac5776ab9e13`, `economy_walkthrough` `0x213f4462ac4404bd`,
`enemy_outpost_walkthrough` `0x8f838952c76889f6`, `group_spiral`
`0xaade86a7b573b680`, `move_500` `0xdce11af94a970c1f`, `move_500_short`
`0xfe406e123a8dd8dc`, `snapshot_restore` `0xa19f97ffe1618779`.
`capture_seat` (`0x2c9a9ef52a5184d0`, `just rerecord capture_seat
stalemate_45min`) and `stalemate_45min` (`0x85a531db76eb0b04`) are first
recordings of new scenarios, which need no bump by themselves.

Recorded 2026-10-23 at `rules_version` 19, `SIM_VERSION` 15 with
`just rerecord` (M5a attrition: `Unit` gained the hashed `exposed_ticks`,
step 5b' applies Harrying, `Command::DebugDamage` is appended). No fixture
researches Harrying, so nobody is ever exposed: re-simulating the eight
fixtures emits no `Attrition` event and every `exposed_ticks` stays 0. A
per-tick `sim-cli hash-dump --every 1` of each fixture as recorded at
`SIM_VERSION` 14, run by that binary with its own `data/`, against the
re-recorded fixture run by the `SIM_VERSION` 15 binary with the current
`data/`, differs in the whole hash and the `units` column on every tick
with a unit and in no other column, and the units hashed without the new
field equal the old `units` column on every tick. Every `.hash` file
changes for that reason only (each unit's encoding grew by one byte):
`combat_skirmish` `0x4914c1138c1f4255`, `economy_second_town`
`0xfd88a57c5cb6bf16`, `economy_walkthrough` `0xd783a5f9e81110a5`,
`enemy_outpost_walkthrough` `0xe6127d167a9d064d`, `group_spiral`
`0x723eae548d0a8f26`, `move_500` `0xdabf913fcf0dd1b0`, `move_500_short`
`0x69cb2eaad81cf7f8`, `snapshot_restore` `0xc601c749c206ee7f`.

Recorded 2026-10-22 at `rules_version` 18, `SIM_VERSION` 14 with
`just rerecord` (M5a rules data: the Watchtower's Harrying I and II with
`counts_toward_age: false`, the `HarryingLevel` stat path, the attrition,
annexation and match-end fields of `rules.ron` and the attrition and
annexation flags of `units.ron` and `buildings.ron` joined `rules_hash`).
The sim reads only `counts_toward_age`, and `AdvanceAge` counting the
techs that count toward an age is the old count for every fixture: none
researches Harrying, and every other tech counts. Every final hash and
every `.hash` file is unchanged; only the headers differ.

Recorded 2026-10-22 at `rules_version` 17, `SIM_VERSION` 14 with
`just rerecord enemy_outpost_walkthrough` (M4b scripted owner proxy:
`enemy_outpost_walkthrough` is a first recording of a new scenario, final
hash `0x6bbf4dc88f1a9b2e`, so nothing bumps; no other fixture was
re-recorded, and every other `.eonreplay` and `.hash` file is
byte-identical).

Recorded 2026-10-22 at `rules_version` 17, `SIM_VERSION` 14 with
`just rerecord` (M4b unit looks: `data/visuals.ron` gave each kind its own
look with the new `Body` visual and `Cone` shape; the file stays in
`rules_hash`, but the sim never reads it). Every final hash and every
`.hash` file is unchanged; only the headers differ.

Recorded 2026-10-22 at `rules_version` 16, `SIM_VERSION` 14 with
`just rerecord` (M4a combat behaviour: idle armed units and attack-movers
acquire targets, `AttackMove` engages, complete Watchtowers and Towns
shoot, `DebugSpawnJittered` is appended). No fixture has a unit with an
attack, and no enemy unit comes within range of the Seats or the second
Towns, so nothing acquires, aims or draws: re-simulating the six
fixtures emits no `Hit`, `UnitDied`, `BuildingDestroyed`, `TownAtZeroHp`
or `PathUnreachable` and leaves the `rng` sub-hash at its seeded value,
and every final hash and every `.hash` file is unchanged; only the
headers differ. `combat_skirmish` is a first recording of a new scenario
(`just rerecord combat_skirmish`, final hash `0x61bdf5745c3c25b1`), which
needs no bump by itself.

Recorded 2026-10-21 at `rules_version` 15, `SIM_VERSION` 13 with
`just rerecord` (M4a combat core: `Unit` gained the hashed
`damage_taken`, `ready_at`, `target` and `intent`, and `Building` gained
`ready_at`). No fixture attacks (none issues an `Attack`, and nothing
acquires targets until the combat behaviour slice), so every unit and
building behaves as before: a per-tick `sim-cli hash-dump --every 1` of
each fixture as recorded at `SIM_VERSION` 12, run by that binary with its
own `data/`, against the re-recorded fixture run by the `SIM_VERSION` 13
binary with the current `data/`, differs in the whole hash and the
`units` column on every tick with a unit, in the `buildings` column on
every tick of the two economy fixtures (the only ones with buildings),
and in no other column; re-simulating every fixture emits no `Hit`,
`UnitDied`, `BuildingDestroyed` or `TownAtZeroHp` event. Every `.hash`
file changes for that reason.

Recorded 2026-10-21 at `rules_version` 14, `SIM_VERSION` 12 with
`just rerecord` (M4a tech: the hashed state gained `State.tech`, every
player's age and researched techs, which the `tech` sub-hash now covers
instead of a placeholder). No fixture researches, advances an age or
reaches Town tier 2, so every behaviour is unchanged: a per-tick
`sim-cli hash-dump --every 1` of each fixture as recorded at
`SIM_VERSION` 11, run by that binary with its own `data/`, against the
re-recorded fixture run by the `SIM_VERSION` 12 binary with the current
`data/`, differs in the whole hash and the `tech` column on every tick
and in no other column. (Neither binary reads the other's recording: it
reports `SIM VERSION MISMATCH`.) Every `.hash` file changes for that
reason.

Recorded 2026-10-21 at `rules_version` 13, `SIM_VERSION` 11 with
`just rerecord` (M4a rules data: `techs.ron`, `ages.ron` and
`factions.ron`, the five new unit kinds, the unit and shooter combat
fields, the Skirmisher's own speed and the counter table joined
`rules_hash`, and `yield_cap_table` became `yield_cap_base`). The sim does not read `techs.ron`, `ages.ron`,
`factions.ron`, the combat fields or the counter table yet. It does read
the new unit kinds: until the M4a tech slice adds the age gate, a `Train`
of any of them at its trainer is accepted, and a Skirmisher now spawns at
2.20 tiles/s. The fixtures build only the Town and the Croft and train and
spawn only Yeomen, and the sim keeps no rules hash in its state, so every
final hash and every `.hash` file is unchanged; only the headers differ.

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
