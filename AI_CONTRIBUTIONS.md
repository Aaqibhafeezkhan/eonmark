# AI contributions

This document states how AI tools are used in Eonmark and what is expected of a pull request written with AI help. It exists so contributors know where the code came from and so reviewers apply one standard to every change.

## How this codebase was made

Eonmark is AI-assisted. The design and the M0 scaffold were planned and written with Claude. Implementation is done with AI coding agents under human review: the project lead reads, runs and edits what the agents produce before it merges. The license consequences are recorded in `docs/adr/0001-license.md`. In short, the maintainer licenses whatever copyright exists under MIT OR Apache-2.0, and to the extent portions are not copyrightable, recipients may treat them as public domain, which is strictly more permissive.

## AI-assisted pull requests

AI-assisted pull requests are welcome when all of the following hold:

1. A human author ran the change locally and understands it. You should be able to answer a review question about any line in the diff.
2. CI passes, and a code pull request includes a test or a replay, exactly as for any other code PR. See [CONTRIBUTING.md](CONTRIBUTING.md).
3. You tick the AI-assistance checkbox in the pull request template, [.github/PULL_REQUEST_TEMPLATE.md](.github/PULL_REQUEST_TEMPLATE.md). Disclosure is normal here and does not count against the PR.

Review applies the same standard to every PR, whoever or whatever typed it. A PR whose author cannot explain it is closed, not finished by the reviewer.

## Not accepted

- AI-generated art, audio, models, textures or fonts under `assets/`. Their copyright status is uncertain, and `assets/` is CC0-1.0, a dedication only a rights holder can make. See [assets/LICENSE-ASSETS.md](assets/LICENSE-ASSETS.md) and [assets/ATTRIBUTION.md](assets/ATTRIBUTION.md).
- Unreviewed output: a PR that pastes agent output without running it, or whose description does not match its diff.
- Text copied from fan wikis or from any source with an incompatible license. See the naming rules in [CONTRIBUTING.md](CONTRIBUTING.md).

## Commit messages

Commit messages and PR descriptions carry no AI attribution trailers. The commit style is in [CONTRIBUTING.md](CONTRIBUTING.md).
