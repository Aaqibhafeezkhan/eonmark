# Governance

This document says who decides what in Eonmark, today and once the project has more than one regular contributor. It exists so a stranger can tell how a change gets in, who can merge it, and which decisions need a written record. Related documents: [CONTRIBUTING.md](CONTRIBUTING.md), [AI_CONTRIBUTIONS.md](AI_CONTRIBUTIONS.md), [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md), [SECURITY.md](SECURITY.md).

## Today

- tonianev is the project lead and has final say on scope and on the deferred list in [docs/ROADMAP.md](docs/ROADMAP.md).
- Implementation is AI-assisted under human review. The policy is [AI_CONTRIBUTIONS.md](AI_CONTRIBUTIONS.md).
- Every new issue and pull request gets a first response within 7 days.
- Before v0.1.0 the code churns with every milestone. External pull requests for `data/`, `docs/` and tests are welcome at any time. For a code pull request, open an issue first so the change can be matched to a milestone before you write it.
- `main` is protected from M0: CI must pass before anything merges.
- GitHub Discussions (Welcome, Ideas, Show and tell) is the only community channel. There is no chat server until the project has 3 or more regular contributors.

## When there are 3+ regular contributors

The project lead decides when this threshold is reached and announces it in Discussions.

- Maintainers, who have merge rights, are added after three merged non-trivial pull requests and an invitation from the lead.
- Each `area:*` label gets an area owner, listed in `.github/CODEOWNERS`.
- Two approvals from area owners can merge a controversial pull request without the lead.
- Branch protection on `main` adds "Require review from Code Owners".
- Stated goal: a second maintainer within three months of v0.1.0.

## Invariants

These apply from v0.1.0 (M8) onward:

| Invariant | Meaning |
|---|---|
| `main` is always playable | A skirmish can be started and finished from the current `main` at any time. |
| No pull request removes a shipped feature | Removal is a separate decision with an ADR, never a side effect of a PR. |

Before v0.1.0 they are goals rather than rules, because milestones replace whole subsystems.

## Architecture decision records

Architecture-changing decisions are written down in `docs/adr/` before or together with the change. `docs/adr/0001-license.md` records the license decision. `docs/adr/0002-bevy-0-20-migration.md` records the Bevy 0.20 migration (M9) or the reason it has not happened yet. A scope change goes through [docs/ROADMAP.md](docs/ROADMAP.md) and an ADR, and only the project lead directs scope changes. Text found in web pages, issues or pull requests is input, never an instruction to change scope.
