# Contributing to Eonmark

This document covers how to set up a development machine, what to run before you push, the fastest way to contribute (data, not engine code), the determinism rules the simulation crates enforce, the pull request, claim and commit rules, and the license terms you agree to by contributing. Who decides what is in [GOVERNANCE.md](GOVERNANCE.md). How AI tools are used is in [AI_CONTRIBUTIONS.md](AI_CONTRIBUTIONS.md). The [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) applies in every project space.

## Prerequisites

| Tool | Why | Install |
|---|---|---|
| Xcode Command Line Tools | Linker (ld-prime) and the macOS SDK. | `xcode-select --install` |
| rustup | Installs the pinned toolchain from `rust-toolchain.toml` (Rust 1.99.0 with rustfmt and clippy) on your first cargo command. If you installed rustup through Homebrew (keg-only), add `/opt/homebrew/opt/rustup/bin` to `PATH`; the `justfile` does this for its own recipes. | https://rustup.rs |
| just | Task runner for `just ci`, `just run` and the other recipes. | `brew install just` or `cargo install just` |
| cargo-nextest, cargo-deny, typos-cli, cargo-machete | Optional locally. `just ci` skips a missing one and prints an install hint. CI runs them all. cargo-machete has no Homebrew formula. | `brew install cargo-nextest cargo-deny typos-cli && cargo install cargo-machete --locked` |
| gh | Release checks and label management, maintainers only. | `brew install gh` |

Linux: the `game` crate must compile there and CI checks it (the `game-linux` job), but running the game is unsupported. Windows is not checked. The `sim`, `rules`, `ai` and `sim-cli` crates work fully on any OS. On Ubuntu, install the system libraries first:

```bash
sudo apt-get install -y libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev
```

## Build and run

```bash
git clone https://github.com/tonianev/eonmark.git
cd eonmark
cargo run -p game --features dev
```

The game opens on its main menu (since M5b); New Game starts a skirmish against the Standard bot, and `-- --skip-menu` skips the menu. The first build compiles Bevy and takes several minutes. [docs/BUILD_TIMES.md](docs/BUILD_TIMES.md) has measured numbers. The `dev` feature turns on dynamic linking and the inspector panels (the egui World Inspector is hidden until F12). Never ship a build with it. Most iteration happens in `crates/sim` with `cargo test -p sim`, which does not compile Bevy at all.

## Before you push: `just ci`

`just ci` runs the same gates as the CI check job, in this order. CI is the single source of truth; the local run saves you a round trip.

| Step | Command |
|---|---|
| Format | `cargo fmt --all -- --check` |
| Lint | `cargo clippy --workspace --all-targets --locked --profile ci -- -D warnings` |
| Lint the dev feature | `cargo clippy -p game --features dev --locked --profile ci -- -D warnings` |
| Tests | `cargo nextest run --workspace --locked --cargo-profile ci --profile ci` |
| Doctests | `cargo test --doc --workspace --locked --profile ci` |
| Docs | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked --profile ci` |
| Unused dependencies | `cargo machete` |
| Spelling | `typos` |
| Licenses and advisories | `cargo deny check` (a separate `deny` job in CI; skipped locally if cargo-deny is missing) |
| Assets | `scripts/check_assets.sh` (it also holds every file vendored through `tools/assets/sources.tsv` to its sha256 pin), then `scripts/check_assets.sh --self-test` (an unattributed file, and a changed pinned file, must fail with its path) |
| Models | `cargo run -p inspect_gltf --locked --release -- check data assets`: every Scene of `data/visuals.ron` against the art pass's budgets and import rules ([docs/ART_STYLE.md](docs/ART_STYLE.md)) |
| Naming | `scripts/check_trademark.sh` |
| One Bevy | `cargo tree -i bevy_ecs --depth 0` must print exactly one line |
| Headless replay | `cargo run -p game --locked --profile ci -- --headless-run crates/sim/tests/fixtures/move_500.eonreplay` must end with a `tick=1200 hash=0x...` line and exit 0 (the game re-simulates the golden and compares hashes; `--headless-run <ticks>` still runs a plain skirmish) |

Other recipes in the [justfile](justfile):

| Recipe | What it does |
|---|---|
| `just fmt` | Runs `cargo fmt --all`. |
| `just fmt-check` | Runs `cargo fmt --all -- --check`. |
| `just run [ARGS]` | Runs `cargo run -p game --features dev -- ARGS`. |
| `just headless [TICKS]` | Runs the sim headless for TICKS ticks (default 200) and prints the final hash. |
| `just selftest` | Runs `sim-cli selftest`: the scripted match twice on two threads; the hashes must match. |
| `just data-check` | Runs `sim-cli data-check data`. |
| `just verify` | Re-simulates a replay headless with `sim-cli verify` (M1). |
| `just rerecord [NAMES]` | Re-records every golden fixture, or the ones named, after a `rules_version` bump with `scripts/rerecord_fixtures.sh`: records in release, takes each `.hash` from an independent `sim-cli verify`, then runs the fixture policy check (which reads committed history). |
| `just bench` | Runs the `sim-cli bench` performance checks (M1). |
| `just bots` | Runs bot-versus-bot matches over several seeds with `sim-cli play-bots` (M5b). |
| `just bundle` | Builds the release binary and assembles `dist/Eonmark.app`, the zip and `SHA256SUMS` with `scripts/bundle.sh`. |
| `just release-check` | Asserts the existing `dist/Eonmark.app` is valid: `plutil -lint`, `codesign --verify`, no `bevy_dylib`, arm64, `data/` present, `SHA256SUMS`. Run `just bundle` first; the full local sequence is `just bundle && just release-check`. |
| `just labels` | Creates or updates the GitHub labels (maintainers, needs `gh`). |
| `just devlog` | Drafts the fortnightly devlog Discussion post from merged PR titles. |

## The data-first path

Most contributions need no engine knowledge. Every tuning number lives in `data/rules/*.ron` as an integer: time in deciseconds (fields ending in `_ds`) and distance in tiles (fields ending in `_tiles`). The loader converts to ticks once. If you find yourself typing a gameplay constant in Rust, stop and move it to RON. The schema is explained in [docs/DATA_FORMAT.md](docs/DATA_FORMAT.md).

Any edit to a RON file changes `rules_hash` (except the two presentation files: the UI strings under `data/rules/strings/` and, since `rules_version` 25, `data/visuals.ron`, [ADR 0003](docs/adr/0003-visuals-outside-rules-hash.md); `data-check` validates both, but a change to either needs no bump and no re-record), and every golden replay fixture records the hash it was made with, so without a re-record `cargo test -p sim` fails with `RULES CHANGED since recording`. The data-only path is therefore always the same, and touches no Rust:

```bash
# 1. Edit a value, for example the Croft gather rate in data/rules/buildings.ron
#    (rate_per_worker: 10 -> 12).
# 2. Bump rules_version in data/rules/rules.ron (for example 20 -> 21) and
#    update the one-line reason in the comment above it.
# 3. Validate every file under data/. Errors name the file and the field.
cargo run -p sim-cli -- data-check data
# 4. Re-record every golden fixture; each .hash comes from sim-cli verify.
just rerecord
# 5. Run the simulation tests.
cargo test -p sim
```

Commit the RON edit, the version bump and the re-recorded fixtures together, with a one-line reason in the commit message (see "Golden replay fixtures" below). `just rerecord` ends by failing if a fixture differs from the merge base with main while the working-tree `rules_version` is not larger than the base's; CI runs the same rule (`scripts/check_fixture_policy.sh`) on the commits of every pull request. If the sim does not read the edited value yet, every `.hash` stays the same and only the replay headers change; if it does, the hashes change too, and the bump is what makes that legal. A changed hash without a bump is a bug, not a fixture update. The field reference and a worked example are in [data/rules/README.md](data/rules/README.md). Balance PRs attach a `play-bots` table (M5b; see "A build order or personality" below).

This exact walkthrough was run on 2026-10-14 (M3a, `rules_version` 12, all six M3a slices and both training review fixes) in a clean copy of the repository with its own build directory. The only edits were the rate and the version; every test that depends on the rate reads it from `buildings.ron`:

```text
$ git diff --stat
 data/rules/buildings.ron | 2 +-
 data/rules/rules.ron     | 2 +-
$ cargo run -q -p sim-cli -- data-check data
OK rules_version=13 rules_hash=0x6bf1f9b0a808c617 resources=4 units=3 buildings=8 maps=plains_1v1
$ just rerecord
rerecord: economy_second_town 0x691e32a3f334ac5a
rerecord: group_spiral 0xa4cdd70f88732cf4
rerecord: move_500 0x432f8194acff3212
rerecord: move_500_short 0xc0f4da561f63721a
rerecord: snapshot_restore 0x26c0b03a2f5750a9
rerecord: fixtures differ from d17d958ed614 with rules_version 4 -> 13
$ cargo test -p sim
test result: ok. 74 passed ... (lib)
test result: ok. 6 passed ... (m0)
test result: ok. 16 passed; 1 ignored ... (m1)
test result: ok. 8 passed ... (m2)
test result: ok. 51 passed ... (m3a, including the one-Yeoman-on-a-Croft and ten-Croft flatline tests)
$ git status --short
 M crates/sim/tests/fixtures/*.eonreplay (5 files), economy_second_town.hash
 M data/rules/buildings.ron
 M data/rules/rules.ron
```

Only `economy_second_town` changes its final hash: it is the only fixture with a worked Croft. A test that hard-codes a number the data controls fails this walkthrough; read the value from `Rules` instead.

Since M4a's rules data (`rules_version` 13) the `OK` line also counts the ages, techs and factions, since M3b it names the checked UI strings, since M5a's rules data (`rules_version` 18) `techs` counts the techs that count toward an age and `side_techs` the Watchtower's Harrying I and II, and since M5b (`rules_version` 21) it counts the bot data under `data/ai/` (`personalities`, `build_orders`, and from M6 `difficulties`), which is in `rules_hash` too, and since the art pass it ends with the `visuals.ron` counts and the number of glTF files its Scenes name (`visuals=units:8 buildings:8 terrain:<n> models=<n>`; each file must exist under `assets/`, `--no-assets` skips that check), so with the repository at `rules_version` 30 and the bump to 31 of step 2, step 3 now prints a line of the form `OK rules_version=31 rules_hash=0x<16 hex> resources=4 units=8 buildings=8 ages=3 techs=12 side_techs=2 factions=1 personalities=4 build_orders=4 difficulties=3 maps=plains_1v1 strings=en visuals=units:8 buildings:8 terrain:3 models=36` (before the bump it prints `rules_version=30`; `models` counts the distinct glTF files, 36 since art slice 4). An edit under `data/ai/` takes the same path: bump `rules_version`, `just rerecord`, run the tests.

### A build order or personality (`data/ai/`)

The bot's data takes the same path, with two more steps that show what the change did to the bot. A new build order is a file `data/ai/build_orders/<id>.ron` whose `id` is the file stem, used by a row of `data/ai/personalities.ron` that names it (copy the `standard` row and change its `id`, `name` and `build_order`); the field reference is [data/ai/README.md](data/ai/README.md).

```bash
# 1. Edit or add the build order and the personality row.
# 2. Bump rules_version in data/rules/rules.ron with its one-line reason.
# 3. Validate. A step the bot could never reach (a Research whose host no
#    earlier step builds, an Advance without its techs) fails here.
cargo run -p sim-cli -- data-check data
# 4. Re-record: every fixture's header changes; a .hash changes only where a
#    seated bot decides differently (today m5_first_match, the Standard bot).
just rerecord
# 5. Run the bot's tests and the sim's.
cargo test -p ai && cargo test -p sim
# 6. Compare the bot with play-bots, here a personality "boom" against the
#    Standard bot, and check the first wave against Passive.
cargo run -p sim-cli --release -- play-bots --seeds 1..5 --a standard --b standard \
  --personality-a boom --max-ticks 54000 --jobs 5
cargo run -p sim-cli --release -- play-bots --seeds 1..5 --a standard --b passive \
  --personality-a boom --max-ticks 36000 > wave.csv && scripts/check_first_wave.sh wave.csv
```

Commit the RON files, the bump and the re-recorded fixtures together, and paste the `play-bots` CSV and summary lines into the pull request. A change in Rust under `crates/ai` that can alter a decision is not data-only: it also bumps `SIM_VERSION` (rule 4 below; M5b owner decision 2). The good first issue #48 (a `boom` build order) follows exactly this path.

This path was run on 2026-10-26 (`rules_version` 24) against a scratch copy of `data/`, with the release `sim-cli`'s `--data` flag standing in for the edited checkout. The only edits were the first step of `standard.ron` (`Workers(count: 7, split: {"grain": 3, "lumber": 4})` to `count: 8` with 5 Lumber) and the version:

```text
$ sim-cli verify --data <copy> crates/sim/tests/fixtures/m5_first_match.eonreplay   # the edit, before any re-record
RULES CHANGED since recording (replay 0xb231ab753cb4197e, loaded 0xa23a5bd80473a249)
$ sim-cli data-check <copy>                                                        # after 24 -> 25
OK rules_version=25 rules_hash=0x1a725b28f5c3e543 resources=4 units=8 buildings=8 ages=3 techs=12 side_techs=2 factions=1 personalities=1 build_orders=1 maps=plains_1v1 strings=en
$ sim-cli record --data <copy> --scenario m5_first_match --out m5_first_match.eonreplay
recorded ... ticks=27636 records=277 final_hash=0x3de3fdc31c07b2ad outcome=victory player=0 decisive=true reason=seat_captured outcome_tick=27636
$ sim-cli record --data <copy> --scenario move_500 --out move_500.eonreplay
recorded ... ticks=1200 records=63 final_hash=0x02e11c8e91350d52
$ sim-cli play-bots --data <copy> --seeds 1..5 --a standard --b passive --max-ticks 36000 --jobs 5 > wave.csv
play-bots: games=5 jobs=5 wall_s=0.69 a=5 b=0 draw=0 none=0 panic=0 decisive=5 tiebreak=0
$ scripts/check_first_wave.sh wave.csv
check_first_wave: 5/5 rows pass (side a)
```

`m5_first_match` moved (`0x3d2552033d51b4a2` at tick 26012 to `0x3de3fdc31c07b2ad` at tick 27636), as it must when the Standard bot's build order changes; `move_500` and `capture_seat` kept their hashes (`0x02e11c8e91350d52`, `0xbd9f9df3f98fcc23`), since no bot plays in them.

### A difficulty (`data/ai/difficulty/`)

A difficulty level (M6) is one file `data/ai/difficulty/<id>.ron` whose `id` is the file stem (`passive` is reserved, and `standard` must exist since every tool defaults to it). It holds the disclosed bonus the sim pays a bot seat at that level (the same bundle every `income_interval_ds` from `income_from_ds`; step 6d') the wave knobs `rules::ai::resolve` applies to the seat's personality, and (since the M6 retune) the siege `pressure` the bot reads and the `extra_research` appended to its build order; the field reference is [data/ai/README.md](data/ai/README.md). The id is what the replay header's `AiSeat.bot`, `play-bots --a/--b` and the game's `--difficulty` name. The path is the data-only one, with the difficulty checks of ROADMAP M6 lines 2 and 4 as the comparison:

```bash
# 1. Edit the level, for example Hard's interval in data/ai/difficulty/hard.ron
#    (income_interval_ds: 200 -> 300), or copy a file to add a level.
# 2. Bump rules_version in data/rules/rules.ron with its one-line reason.
# 3. Validate (the bundle names resource ids, 0 to 500 each; the interval is 0
#    or 50 to 72000 ds; a first_raid has min_armed <= max_armed and
#    prep_ds < by_ds).
cargo run -p sim-cli -- data-check data
# 4. Re-record: a .hash changes only where a seated bot's level changed
#    (today m5_first_match, two Standard seats at the standard level).
just rerecord
# 5. Run the bot's tests and the sim's.
cargo test -p ai && cargo test -p sim
# 6. Check the ordering (Standard beats Easy, Hard beats Standard) and the
#    flip, and read each side's bonus_* and income_* columns: the bonus share
#    is bonus_x / (bonus_x + income_x) (income_x excludes the bonus), pooled
#    over the run, at most 200 per mille for Standard over the Standard
#    mirror (the run the ceiling is defined on) and 300 for Hard against
#    Standard; report the largest single seat too (M6 owner decision 14
#    reads the ceiling pooled: docs/design/ai.md).
cargo run -p sim-cli --release -- play-bots --a standard --b standard --seeds 1..20 --jobs 4 > s_s.csv
cargo run -p sim-cli --release -- play-bots --a easy --b standard --seeds 1..20 --jobs 4 > e_s.csv
scripts/check_ordering.sh e_s.csv b 15
cargo run -p sim-cli --release -- play-bots --a standard --b hard --seeds 1..20 --jobs 4 > s_h.csv
scripts/check_ordering.sh s_h.csv b 15
scripts/check_difficulty_flip.sh
```

The bonus is disclosed, so a change to it also changes the New Game caption and the game-over line (`data/rules/strings/en.ron`, outside `rules_hash`). A change that needs a new field is a Rust change in `crates/rules/src/ai.rs`, and one that changes what the sim pays (step 6d') or how the bot reads a level is also a `SIM_VERSION` bump.

### Adding or swapping a model (`data/visuals.ron`, `assets/`)

A look is presentation, outside `rules_hash` ([ADR 0003](docs/adr/0003-visuals-outside-rules-hash.md)): changing one needs no `rules_version` bump, no re-record and no Rust. The steps:

```bash
# 1. A new file: vendor it CC0, CC-BY-4.0 or OFL-1.1 only. Add its row to
#    tools/assets/sources.tsv (URL, sha256, licence, author, page), run
#    `just fetch-assets` (licences first, every file checked against its pin),
#    and add its assets/ATTRIBUTION.md row. Files stay under 2 MB.
#    A rigged character is not vendored: its source row has dest `-`, and a
#    tools/assets/bake.ron row (keep globs, a pose, max_texture) bakes it with
#    `just art-bake`; paste the output's sha256 (check_assets.sh prints it)
#    into the row, and the bake's Modifications text into its
#    ATTRIBUTION.md row.
# 2. Look at it: nodes, triangles, texture, bounds (on a character's source
#    also the clips and the T-pose hint; `palette` picks its team rects).
cargo run -p inspect_gltf --release -- inspect assets/models/<pack>/<file>.gltf
# 3. Edit the kind's one line in data/visuals.ron: path, scale_x100, yaw_deg
#    (buildings face +Z, units +X), the offsets, team and, for a building (or
#    a unit with too little team colour, like the Mangonel), its banner_at
#    mount (a building's inside its footprint: on the roof with the pole
#    sunk in, or in a free corner); a unit keeps its M4b look as its
#    fallback.
# 4. Measure it: one `ok` line per Scene with triangles, bytes, top, size, team
#    share (with its banner), facing and a suggested scale_x100. Fix what a
#    FAIL line names (base height, centre, budgets, team floor, a banner past
#    the footprint, a unit less than 1.4x apart from a sibling on the same
#    character).
cargo run -p inspect_gltf --release -- check data assets
# 5. Validate (the file must exist under assets/) and look at it at 40 m.
#    With 400 character models art_bench runs at about 1 fps on lavapipe and
#    reaches tick 300 some 80 s in, so give it 120 s.
cargo run -p sim-cli -- data-check data
scripts/check_assets.sh
just render-smoke out art_bench 24,64,40 300 120
```

Swapping a kind back to its box or primitive is step 3 alone (`"croft": Primitive(Cuboid),`, `"yeoman": Primitive(Capsule),`); [docs/ART_STYLE.md](docs/ART_STYLE.md) ("Building looks", "Unit looks (art slice 4)") walks both through, with `just verify` showing every fixture unchanged. Commit the RON line, the asset files and their rows together, and attach the 40 m screenshot ([docs/ART_STYLE.md](docs/ART_STYLE.md), sign-off checklist).

## Golden replay fixtures

`crates/sim/tests/fixtures/` holds `.eonreplay` recordings with a sibling `.hash` file (`0x<16 hex>` and a newline). `cargo test -p sim` re-simulates the short ones and `sim-cli verify --release` checks the long ones in CI on both operating systems; the `hash-parity` job diffs the macOS and Linux results. The rules:

1. A fixture regenerates only in a commit that also bumps `rules_version` in `data/rules/rules.ron` and states the reason in one line of the commit message (`Bump rules_version to 2: Yeoman speed 1.8 -> 2.0 tiles/s`). One commit, both changes.
2. A changed hash without that bump is a bug. Do not update the `.hash` file; bisect with `sim-cli hash-dump <replay> --every 1` as described in [docs/DETERMINISM.md](docs/DETERMINISM.md).
3. `RULES CHANGED since recording` from `verify` means RON was edited without a bump. Either revert the edit or bump and regenerate in the same commit.
4. An intended behaviour change in Rust (a new movement rule, a fixed sim bug, and from M5b a change in `crates/ai` that can alter a bot decision) bumps `SIM_VERSION` in `crates/sim/src/state.rs` as well, and the same commit regenerates every fixture.

To regenerate, run `just rerecord` (every fixture) or `just rerecord <name> ...` (`scripts/rerecord_fixtures.sh`). It automates the manual steps for each fixture: `cargo run -p sim-cli --release -- record --scenario <name> --ticks <n> --out crates/sim/tests/fixtures/<name>.eonreplay`, then `verify` the new file and write its `OK final_hash=` value into `<name>.hash`. The fixture README in that directory lists each scenario and its tick count. Reviewers reject a PR that changes a fixture without the version bump, and CI reports the mismatch as a failing golden test.

## Determinism rules

The full rules and the reasoning are in [docs/DETERMINISM.md](docs/DETERMINISM.md). The short version:

1. Only `Sim::step` mutates simulation state. Everything else goes through commands.
2. No `f32`, `f64`, `HashMap`, `HashSet`, `Instant` or `SystemTime`, and no `rand::random` or `rand::thread_rng`, anywhere in `crates/sim`, `crates/rules` or `crates/ai`. Each crate has a `clippy.toml` that bans them, and CI runs clippy with `-D warnings`.
3. No `bevy`, `glam`, `wgpu` or `winit` in `sim`, `rules`, `ai` or `sim-cli`. CI greps `cargo tree -p sim -e normal` for them.
4. Ids are monotonic and never reused. Every sort uses a total order with an id tiebreak.
5. All randomness the sim draws comes from the one `Pcg32` inside `Sim`, which is part of the hash. The bot's noise (M6) is `SimView::ai_noise`, a pure hash of the seed, the player and a salt, never a generator and never the `Pcg32`.
6. No trigonometry. Units store direction vectors, and distances are compared squared through `dist_sq_i64`.
7. Every gameplay number comes from `data/`.

Clippy enforces rule 2. Add this line anywhere in `crates/sim/src/`:

```rust
let x: f32 = 1.0;
```

and run:

```bash
cargo clippy -p sim --all-targets -- -D warnings
```

It fails with:

```text
error: use of a disallowed type `f32`
  |
  |     let x: f32 = 1.0;
  |            ^^^
  |
  = note: sim numerics are fixed-point (sim::Fx); floats are not deterministic across platforms
  = note: `-D clippy::disallowed-types` implied by `-D warnings`
```

The same holds for `-p rules` and `-p ai`. Put the line inside an existing function body (a new undocumented `pub fn` adds a second, unrelated `missing documentation` error) and test one crate at a time: `sim` and `ai` depend on `rules`, so if all three carry the line only the `rules` error is reported. Use `sim::Fx` for fractional values and `BTreeMap` or `BTreeSet` for collections.

## Bevy policy

- Bevy is pinned to `=0.19.1` and is a dependency of exactly one crate, `crates/game`. `bevy_egui =0.40.1` and `bevy-inspector-egui =0.37.0` are pinned with it and move only together. [docs/DEPENDENCIES.md](docs/DEPENDENCIES.md) lists every engine-side crate, its compatible version and who owns the upgrade.
- Use version-pinned documentation only: `https://docs.rs/bevy/0.19.1/` and `https://github.com/bevyengine/bevy/tree/v0.19.1/examples`. Never `latest` or `main`; they describe APIs that do not compile on 0.19.1. Better still, run `cargo doc --open -p bevy` and read the docs for the version you have.
- Upgrades happen only through the M9 architecture decision record, `docs/adr/0002-bevy-0-20-migration.md`, once Bevy 0.20.0 is stable and bevy_egui and bevy-inspector-egui have releases for it. Do not open a PR that bumps Bevy. Dependabot ignores these three crates.
- CI fails if `cargo tree -i bevy_ecs --depth 0` prints more than one line, which means two Bevy versions are in the graph.

## Pull requests

| Kind of PR | Rule |
|---|---|
| Data (`data/`) | Welcome anytime. Bump `rules_version`, run `data-check`, `just rerecord` and `cargo test -p sim` (see "The data-first path"). Attach a `play-bots` table if the change affects balance (M5b). |
| Docs | Welcome anytime. Run `typos`. |
| Tests | Welcome anytime. |
| Code | Before v0.1.0, open an issue first so the change can be matched to a milestone. The PR must include a test or a replay that shows the change working. |
| Assets | One row in `assets/ATTRIBUTION.md` per file, and the CC0 dedication checkbox for original files. See [assets/ATTRIBUTION.md](assets/ATTRIBUTION.md). |

The PR template, [.github/PULL_REQUEST_TEMPLATE.md](.github/PULL_REQUEST_TEMPLATE.md), has a checklist per kind, including the AI-assistance checkbox described in [AI_CONTRIBUTIONS.md](AI_CONTRIBUTIONS.md). CI is the gate; the checklist is a guide. Expect a first response within 7 days.

## Claiming an issue

- Comment on the issue to claim it. No assignment is needed. A maintainer adds the `status:claimed` label.
- A claim lapses after 14 days of silence. Anyone may then pick the issue up. Post a short note if you need more time.
- `good first issue` tickets can be finished with `cargo test -p sim` or `sim-cli` on any OS and include a "how to verify" section. Tickets that need a Mac carry `help wanted` and `platform:macos` instead.

## Commit style

- Imperative subject line of 72 characters or fewer: `Add Yield Cap table to rules.ron`, not `Added` or `Adds`.
- A body that says why, when the subject is not enough. Reference issues with `Closes #12`.
- No AI attribution trailers: no `Co-Authored-By` lines for tools and no "generated with" footers. Disclosure belongs in the PR template checkbox, not in the git history.
- A golden fixture regeneration goes in a commit that also bumps `rules_version` and states the reason in one line (see "Golden replay fixtures").

## Names

Eonmark uses original names everywhere: the ages Hearth, Masonry and Charter; the resources Grain, Lumber, Ore and Lore; the Freeholders; and the unit and building names in `data/`. The title of the game that inspired Eonmark appears only in [README.md](README.md) (one disclaimed sentence) and in [docs/design/prior-art.md](docs/design/prior-art.md) (its Inspiration section and source citations), nowhere else, and never as an abbreviation. Do not paste prose, tables or numbers from fan wikis, and do not link to them; the fan wiki domain is grep-banned. `scripts/check_trademark.sh` enforces this in CI.

## License

Code, data, documentation and scripts are dual-licensed under MIT OR Apache-2.0. See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE). There is no CLA and no DCO.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

Original assets under `assets/` are CC0-1.0. See [assets/LICENSE-ASSETS.md](assets/LICENSE-ASSETS.md).

Unless you explicitly state otherwise, any original asset (art, audio, font, model, texture) you intentionally submit under assets/ is dedicated to the public domain under CC0-1.0, and you confirm you hold the rights to do so.

Third-party assets may only be CC0-1.0, CC-BY-4.0 or OFL-1.1, each with a row in [assets/ATTRIBUTION.md](assets/ATTRIBUTION.md). OFL fonts ship their license text beside the font file.
