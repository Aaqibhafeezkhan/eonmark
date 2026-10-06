# 0001: Licensing of code, data and assets

Status: Accepted
Date: 2026-10-05

This record fixes the licence terms for everything in the Eonmark repository before the first external contribution arrives, because relicensing later would need every contributor's consent. It covers code and data, original assets, third-party assets, the contributor agreement, the policy for mentioning the game that inspired Eonmark, and the status of AI-assisted authorship.

## Context

Eonmark's contributor pool is the Rust and Bevy ecosystem, where the norm is `MIT OR Apache-2.0` with an implicit dual-licence contribution clause and no CLA. The simulation and rules crates are meant to be reusable by other projects. The alternative tradition in open-source games is GPL code with CC-BY-SA art; it would block reuse of the sim crate in permissive projects and it did not prevent the known difficulty of relicensing a long-lived game once hundreds of people have contributed.

Assets need separate treatment. Bevy's contribution clause covers contributions "as defined in the Apache-2.0 license", which is code and documentation, not a contributor's model or sound file. CC0 is a waiver that only the rights holder can make, so it must be stated explicitly by the contributor. The SIL Open Font License 1.1 requires that its text accompany any redistributed font.

Much of the initial code is written with AI assistance. The United States Copyright Office's report on copyrightability (Part 2, 2025-01-29) states that output generated entirely by AI without sufficient human authorship is not protected by copyright, while human selection, arrangement and modification can be.

Game mechanics are not copyrightable (17 U.S.C. 102(b)). Exposure to the inspiration's rights holder lies in names, text and art, not in rules.

## Decision

1. Code, data and documentation (`crates/`, `data/`, `scripts/`, `docs/`, workflow files) are licensed `MIT OR Apache-2.0`. `LICENSE-MIT` and `LICENSE-APACHE` sit at the repository root and `Cargo.toml` declares `license = "MIT OR Apache-2.0"`.
2. Original assets under `assets/` (art, audio, fonts, models, textures made for Eonmark) are dedicated to the public domain under CC0-1.0, stated in `assets/LICENSE-ASSETS.md`.
3. Third-party assets are limited to CC0-1.0, CC-BY-4.0 and OFL-1.1. Each has a row in `assets/ATTRIBUTION.md` with asset, author, source URL, licence, modifications and download date. OFL fonts ship their licence text beside the font file. `scripts/check_assets.sh` enforces this in CI. Forbidden: CC-BY-SA, CC-BY-NC, GPL art, Sampling+, Quaternius material (proprietary licence since 2026-08-28), and any text from the inspiration's fan wiki.
4. There is no CLA and no DCO. `CONTRIBUTING.md` reproduces Bevy's implicit clause verbatim: "Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions." It adds one sentence for assets: "Unless you explicitly state otherwise, any original asset (art, audio, font, model, texture) you intentionally submit under assets/ is dedicated to the public domain under CC0-1.0, and you confirm you hold the rights to do so." The PR template has a checkbox for each.
5. Nominative mention policy. Every name in Eonmark is original. The inspiration's title appears only in two files: `README.md`, in one disclaimed sentence stating that Eonmark is an original game inspired by it and is not affiliated with or endorsed by its publisher, and `docs/design/prior-art.md`, in its Inspiration section and its source citations (URLs that carry the title). Its common abbreviation appears nowhere. Fan-wiki URLs are banned. `scripts/check_trademark.sh` enforces this in CI by grepping the working tree.

## Alternatives considered

| Alternative | Why not |
|---|---|
| GPL-3.0 code with CC-BY-SA assets | Blocks reuse of `sim` and `rules` in permissive projects; share-alike art propagates to every derived asset; contributors from the Rust ecosystem expect dual MIT/Apache |
| A CLA | Signing infrastructure and friction for a solo-maintained project; the implicit clause is the accepted norm |
| DCO (`Signed-off-by`) | Reasonable step up if a corporate contributor asks; not needed now and adds a per-commit ritual |
| Assets under the code licence | MIT/Apache attribution requirements are awkward for art; CC0 is the most reuse-friendly and matches the main sources |
| Banning the inspiration's title everywhere including the README | Truthful referential mention with a disclaimer is standard practice and is how strangers find the project |
| Placeholder crates on crates.io to protect the name | Name squatting is against crates.io policy; protection comes from the repository, domain and itch.io page |

## Consequences

- `sim` and `rules` can be reused by any project, permissive or copyleft.
- Contributors do nothing beyond opening a PR; the clause in `CONTRIBUTING.md` and the PR checkbox carry the agreement.
- Every asset PR must add an attribution row or mark the file original; CI rejects it otherwise. Fonts under OFL carry their licence file beside them.
- Only the repository owner may change the licence, and only through a new ADR that supersedes this one. After external contributions exist, such a change would require each contributor's consent.
- Design documents state Eonmark's own numbers and never paste wiki prose or tables.
- The AI-assisted authorship statement below is part of the public record from M0, before the first external PR.

## AI-assisted authorship

Much of Eonmark's initial code, data and documentation is written with AI assistance. The maintainer reviews, edits, selects and arranges that output and takes responsibility for it, and licenses whatever copyright exists in the result under `MIT OR Apache-2.0` as stated above. To the extent that portions of the work are not protected by copyright because they lack sufficient human authorship, as described in the United States Copyright Office's report on copyrightability (Part 2, 2025-01-29), recipients may treat those portions as public domain. That is strictly more permissive than the stated licence, so nobody relying on `MIT OR Apache-2.0` is worse off. Contributors are asked to say in the PR template whether a contribution was AI-assisted; the policy for accepting such contributions is in `AI_CONTRIBUTIONS.md`.
