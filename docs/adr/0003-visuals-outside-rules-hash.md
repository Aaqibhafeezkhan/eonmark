# 0003: `data/visuals.ron` outside `rules_hash`

Status: Accepted
Date: 2026-10-26

This record takes the look data, `data/visuals.ron`, out of the rules hash that every replay header carries, while keeping it loaded and validated with the rest of the rules. It is the owner's decision of 2026-10-26 (question 1 of the pre-stream art pass plan, the plan's default) and lands as its own small pull request after M5b, before the art pass itself.

## Context

- **The hash and the file.** `rules_hash` is xxh3 over postcard of the whole loaded `Rules` struct (`Rules::rules_hash`, `crates/rules/src/lib.rs`). Every replay header stores it, and `verify` reports `RULES CHANGED since recording` when the loaded rules hash differently. `Rules::load` attaches `data/visuals.ron` as `Rules::visuals` (`#[serde(skip_deserializing)]` until now), so the file has been in the hash since M2 (`rules_version` 4).
- **M4b owner decision 5** (ROADMAP, M4b Notes, 2026-10-22): "`visuals.ron` stays in `rules_hash` for M4b (one `rules_version` bump); an ADR on taking it out may come before the art pass." M4b's looks cost one bump (16 to 17) and a header-only re-record of every fixture.
- **The simulation never reads it.** `crates/rules/src/visuals.rs` says so in its module doc; only the game's presenter (`crates/game/src/present.rs`) maps a `Visual` to a mesh. No sim, bot or replay code reads `Rules::visuals`, so a look edit can change no state hash: M4b's bump re-recorded every fixture with every `.hash` file byte-identical.
- **Precedent.** The UI strings (`data/rules/strings/en.ron`, M3b) are presentation data outside `Rules` and outside the hash; `sim-cli data-check` validates them anyway (`crates/sim-cli/src/main.rs`, `data_check`), and `docs/DATA_FORMAT.md` documents that editing them needs no bump.
- **The art pass's churn.** The art pass replaces the primitive looks with CC0 glTF models through `visuals.ron`. Its plan edits the file in three slices and again at every 40 m readability review: about four to six bumps, each re-recording the eleven fixture headers for no state change, and every later model swap (the M7 good first issue that replaces the Croft with a KayKit model "via `visuals.ron`") would cost one more. ROADMAP M7's model-swap walkthrough would read "one RON line plus a bump plus `just rerecord`" instead of one RON line.

## Decision

1. `Rules::visuals` becomes `#[serde(skip)]` instead of `#[serde(skip_deserializing)]`. `Visuals` already derives `Default`, and `Rules` is serialised only by `rules_hash`, so the hash stops covering `data/visuals.ron` and nothing else changes.
2. `Rules::load` still parses `data/visuals.ron` and runs `Visuals::validate` against the unit kinds, so `sim-cli data-check` still fails on a bad, missing or unknown entry and on a missing file. Tests hold both halves: `rules::tests::rules_hash_ignores_visuals_ron` (an in-memory change and an edited copy of the file keep the hash, and the edit is loaded) and `bad_visual_still_fails_to_load_naming_file_and_field`, and in `crates/sim-cli/tests/data_check.rs` a kind without a visual, an out-of-range look and a missing `visuals.ron` each exit 1 naming the file, while a valid look edit prints the repository's own `OK` line, `rules_hash` included.
3. `replay::verify`, the replay format and `SIM_VERSION` are unchanged. One `rules_version` bump (24 to 25) re-records the eleven fixtures for their headers; after it, a `visuals.ron` edit never needs a bump or a re-record.
4. Nothing the simulation or a bot reads may live in `visuals.ron`. If a look ever needs a number the sim uses (a footprint, a range), that number goes in a hashed file under `data/rules/` or `data/ai/`.

## Alternatives considered

| Alternative | Why not |
|---|---|
| Keep `visuals.ron` in the hash and bump on every look edit | Four to six bumps in the art pass alone and one per later model swap, each re-recording every fixture header with no state change; old replays report `RULES CHANGED` after every one of them. The bumps would buy nothing, since the sim cannot diverge on a look. |
| A separate `visuals_hash` field in the replay header | A header layout change means a `SIM_VERSION` bump and a re-record of every fixture, and nothing would ever compare the field: `verify` does not need the looks to re-simulate. |
| Move the look data out of `Rules` into a game-side loader, as the UI strings are | Larger change for the same effect: the game reads looks through `Rules::visual`, the validator checks them against `Rules::units`, and `data-check` would need a second load path. `#[serde(skip)]` keeps one load and one validation. |

## Consequences

- One `rules_version` bump now (24 to 25, "data/visuals.ron leaves rules_hash (ADR 0003); no sim change") with `just rerecord` in the same commit. Each of the eleven fixtures changes in its header's `rules_version` and `rules_hash` only (10 bytes); every `.hash` file stays byte-identical, and a per-tick `hash-dump` of `move_500`, `combat_skirmish`, `capture_seat` and `m5_first_match` is identical before and after.
- Replays recorded before the bump report `RULES CHANGED` on this build and every later one. A match recorded on an earlier build, the M5b owner match included, is verified with the build that recorded it. No command prints a header's `rules_version` (24 or lower for such a replay); the `replay 0x…` hash in the `RULES CHANGED` line names the build, since `sim-cli data-check data` at the recording commit prints the same `rules_hash` (`0xb231ab753cb4197e` is `rules_version` 24, `e84af88`).
- A replay no longer records which looks were loaded. `verify` never needed that, and the game's looks do not affect any hash.
- From `rules_version` 25 a look edit is one RON line: `sim-cli verify` of every fixture still prints `OK` and `data-check` still validates the entry. ROADMAP M7's swap walkthrough and the art pass need no further bump for looks.
- Docs that said the file is hashed are updated in the same pull request: `docs/DETERMINISM.md`, `docs/DATA_FORMAT.md`, `data/README.md`, `data/rules/README.md`, `docs/ART_STYLE.md`, `docs/ARCHITECTURE.md`, `CONTRIBUTING.md`, the module doc of `crates/rules/src/visuals.rs`, the comment at the top of `data/visuals.ron` and the `rules_hash` doc in `crates/rules/src/lib.rs`.
