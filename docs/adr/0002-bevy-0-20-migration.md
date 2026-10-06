# 0002: Migration to Bevy 0.20

Status: Proposed
Date:

This record defines when and how Eonmark moves from Bevy 0.19.1 to Bevy 0.20. It exists now, while the project is on 0.19.1, so that the trigger conditions are agreed before 0.20 stable ships and nobody upgrades mid-milestone. The migration itself is milestone M9 in [ROADMAP.md](../ROADMAP.md). Dependency pins and the compatibility chain this record depends on are in [DEPENDENCIES.md](../DEPENDENCIES.md).

## Context

Bevy breaks its API on every minor release. As of 2026-10-05 the latest stable is 0.19.1 (2026-08-13) and 0.20.0-rc.2 is out (2026-09-28). The 0.19 line went from rc.3 to stable in about nine days, so 0.20 stable is expected while M0 to M3 are in progress. The published 0.19-to-0.20 migration guide is a draft with roughly 70 entries. Entries that touch Eonmark: pointer events are flattened (`Pointer<Press>` becomes `PointerPress`), `Interaction` is replaced by `Hovered` and `Pressed`, `iter_many` returns `Result`, sprites move to `Mesh2d`, and shaders move from `.wgsl` to `.wesl`, which affects the one custom shader `assets/shaders/field.wgsl`.

Eonmark's dev tooling depends on `bevy_egui` and `bevy-inspector-egui`, which must share one `egui` version and both target the same Bevy. Ecosystem crates lag Bevy by one to three months. `bevy_egui 0.43.0-rc.1` already targets 0.20; an inspector release that pairs with it did not exist on 2026-10-05.

`Cargo.toml` pins `bevy = "=0.19.1"`, `bevy_egui = "=0.40.1"` and `bevy-inspector-egui = "=0.37.0"`. Dependabot ignores all three for minor and major bumps. CI fails if `cargo tree -i bevy_ecs --depth 0` prints more than one line.

## Decision

Migrate to Bevy 0.20 in a single PR, as milestone M9, only when all trigger conditions hold. Never migrate mid-milestone. Do not start development on a 0.20 release candidate.

### Trigger conditions

| Condition | Met on (date) | Evidence |
|---|---|---|
| `bevy 0.20.0` stable is on crates.io (not an rc) | | |
| A `bevy_egui` release depends on `bevy ^0.20` | | |
| A `bevy-inspector-egui` release depends on `bevy ^0.20` and on that `bevy_egui` line | | |
| Those two releases resolve to a single `egui` version | | |
| The 0.19-to-0.20 migration guide is no longer marked draft | | |

If the inspector lags for more than two months after `bevy_egui` and Bevy are ready, the fallback is to drop `bevy-inspector-egui` from the `dev` feature, keep `bevy_egui` for the tuning panels, and record that in this ADR.

### Migration steps

1. Open a branch `bevy-0.20`. Bump the three pins in `[workspace.dependencies]` together; run `cargo update -p bevy -p bevy_egui -p bevy-inspector-egui`.
2. Confirm `cargo tree -i bevy_ecs --depth 0` prints one line and `Cargo.lock` has one `egui`.
3. Walk the official 0.19-to-0.20 migration guide top to bottom. For each entry that matches a compile error or a grep hit in `crates/game`, apply the change. Expected areas: picking and pointer events in `selection.rs` and `orders.rs`, UI interaction in `hud/` and `menu/`, `iter_many` call sites in `present.rs`, and the field shader rename to `.wesl` with any syntax changes.
4. Rebuild the `dev` feature: `cargo clippy -p game --features dev --profile ci -- -D warnings`.
5. Rebuild on Linux through the `game-linux` CI job.
6. Run every golden fixture: `cargo test -p sim` and `sim-cli verify --release` on the long ones. The sim crates do not depend on Bevy, so no hash may change. If one does, the branch has leaked into the sim side and the PR stops until that is found.
7. Run `--headless-run` on the smoke fixture and play one skirmish to the game-over screen on the development Mac. Verify the replay.
8. Walk the [PLAYTEST.md](../PLAYTEST.md) controls checklist once; picking and gestures are the most likely regressions.
9. Update [DEPENDENCIES.md](../DEPENDENCIES.md): pins, the egui compatibility table, every pinned doc link from `0.19.1` to `0.20.0`, and the drift log.
10. Fill in the date fields in this ADR and set the status to Accepted in the same PR.

### Acceptance

| Check | Required result |
|---|---|
| All golden fixtures | Unchanged hashes; `sim-cli verify` prints `OK` for every fixture |
| `just ci` | Green locally and in GitHub Actions on the PR, all five jobs |
| `cargo tree -i bevy_ecs --depth 0` | Exactly one line |
| `Cargo.lock` | Exactly one `egui`, one `cpal`, one `wgpu` |
| `cargo clippy -p game --features dev` | Passes with `-D warnings` |
| `game-linux` job | Passes |
| Controls checklist | Every row passes on the development Mac |
| Docs | [DEPENDENCIES.md](../DEPENDENCIES.md) has no `0.19.1` doc links left |

## Alternatives considered

| Alternative | Why not |
|---|---|
| Start on 0.20.0-rc now | Ecosystem crates have not caught up; two Bevy versions would enter the graph; rc APIs still move |
| Track Bevy `main` or `latest` | Breaks the pinned-docs rule and the reproducible build; every PR would fight churn |
| Skip 0.20 and wait for 0.21 | Each skipped release doubles the migration surface; the field shader rename lands in 0.20 regardless |
| Let Dependabot propose the bump | A grouped Dependabot PR would pull `bevy_egui 0.43` before the inspector is ready and fail the single-Bevy check |

## Consequences

- No Bevy work is blocked while waiting: 0.19.1 is pinned and documented, and the pinned-docs rule keeps references accurate.
- The migration is one PR with a clear acceptance list; it cannot be partially merged.
- The sim side is untouched by design, which is what makes "all golden fixtures unchanged" a hard requirement rather than a hope.
- After acceptance, this file records the date of migration or, if the conditions were never met, the unmet conditions and the decision taken instead.

## Record

| Field | Value |
|---|---|
| Trigger conditions met on | |
| Migration PR | |
| Merged on | |
| Fixtures unchanged | |
| Notes | |
