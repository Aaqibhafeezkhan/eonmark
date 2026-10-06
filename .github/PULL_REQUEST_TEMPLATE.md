<!-- Thanks for the PR. CI is the gate; this checklist is a guide. Delete the tiers that do not apply. -->

## What and why

<!-- One paragraph. Link the issue: "Closes #123". -->

## Tier

Tick the tier that matches the change and complete its items.

### Data PR (files under `data/`)

- [ ] `cargo run -p sim-cli -- data-check data` passes
- [ ] `cargo test -p sim` passes
- [ ] `rules_version` is bumped with a one-line reason if any golden replay hash changed
- [ ] `kind:balance` changes include the play-bots table (M5b) or a playtest note

### Docs PR (files under `docs/`, `README.md`, `data/*/README.md`)

- [ ] `typos` passes
- [ ] `scripts/check_trademark.sh` passes (original names only; see docs/NAME.md)
- [ ] Links are relative paths and resolve

### Code PR (files under `crates/`, `scripts/`, `.github/`)

- [ ] `just ci` passes locally
- [ ] A test or a replay fixture covers the change (CONTRIBUTING.md: replay-or-test-in-PR rule)
- [ ] No `f32`, `f64`, `HashMap`, `HashSet` or `Instant` in `crates/sim`, `crates/rules`, `crates/ai`
- [ ] No Bevy, glam, wgpu or winit dependency outside `crates/game`
- [ ] Every Bevy doc link is pinned to 0.19.1

### Asset PR (files under `assets/`)

- [ ] `scripts/check_assets.sh` passes
- [ ] Every third-party file has an `assets/ATTRIBUTION.md` row (path, author, source URL, license, modifications, download date)
- [ ] Licenses are CC0-1.0, CC-BY-4.0 or OFL-1.1 only; OFL license text sits beside each font
- [ ] No file is larger than 2 MB

## Licensing and disclosure

- [ ] I agree that, unless I explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by me, as defined in the Apache-2.0 license, shall be dual licensed as MIT OR Apache-2.0, without any additional terms or conditions (CONTRIBUTING.md).
- [ ] For original assets under `assets/`: I dedicate them to the public domain under CC0-1.0 and I hold the rights to do so.
- [ ] AI assistance: I have disclosed any AI-assisted parts of this change per AI_CONTRIBUTIONS.md, and I have reviewed and tested them myself.

## How to verify

<!-- Commands a reviewer runs and what they should see. Paste the output of the acceptance commands for milestone PRs. -->
