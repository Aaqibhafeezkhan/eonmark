# The name

This document records why the project is called Eonmark, which availability checks were done on 2026-10-05 and what they found, which checks remain for the owner, and the runner-up names in case a trademark search rejects the primary. It also states the rule that no placeholder crates are published. The only place the inspiration's title appears is [design/prior-art.md](design/prior-art.md) and the one disclaimed sentence in [README.md](../README.md); this file does not name it.

## Why Eonmark

"Eon" for the ages a match moves through. "Mark" for a borderland, as in Denmark or the medieval marches. The two signature systems of the game, tech-gated ages and territory borders, in one calm word. It is short, pronounceable, spelled as it sounds, and has no existing meaning in software or games that we could find.

Where it appears: the repository `tonianev/eonmark`, the binary `eonmark`, the bundle identifier `com.tonianev.eonmark`, the window title, the `.app` name and the GitHub topics. The workspace crates are named `sim`, `rules`, `ai`, `game` and `sim-cli`; none of them is published.

## Checked on 2026-10-05

| Where | Query | Result |
|---|---|---|
| crates.io | `eonmark` | 404, no crate |
| Steam store search | `eonmark` | 0 results |
| itch.io search | `eonmark` | 0 results |
| GitHub repository search | `eonmark` | Only an unrelated repository named `eonmarket` |
| GitHub user or organization | `eonmark` | Free |
| GitHub repository | `tonianev/eonmark` | Free |

## Remaining for the owner

These checks need a human, a browser, and in some cases an account. Record the date and save a screenshot of each result page; link the screenshot in the Evidence column. Do them before M0 closes; the M0 acceptance list in [ROADMAP.md](ROADMAP.md) requires it.

| Check | Where | What to look for | Date | Result | Evidence |
|---|---|---|---|---|---|
| USPTO trademark search, class 9 | tmsearch.uspto.gov | Live marks for "eonmark" or close spellings in class 9 (software, downloadable games) | | | |
| USPTO trademark search, class 41 | tmsearch.uspto.gov | Live marks in class 41 (entertainment services, online games) | | | |
| EUIPO eSearch plus | euipo.europa.eu | EU trade marks for "eonmark" | | | |
| WIPO Global Brand Database | branddb.wipo.int | International registrations for "eonmark" | | | |
| Domain `eonmark.com` | any registrar | Available or parked | | | |
| Domain `eonmark.dev` | any registrar | Available or parked | | | |
| Domain `eonmark.games` | any registrar | Available or parked | | | |
| itch.io slug | itch.io | `eonmark.itch.io` or the project slug `eonmark` free | | | |
| GitHub user or org `eonmark` | github.com | Still free at the time of the public launch | | | |

A trademark hit in class 9 or 41 for games or software is a stop: switch to a runner-up before the repository goes public. A hit in an unrelated class (clothing, cosmetics) is noted and does not block.

## Runner-up names

In order of preference. Each was checked on crates.io, Steam and GitHub on 2026-10-05 unless marked otherwise.

| Name | Rationale | Status |
|---|---|---|
| Marchfall | "March" is a frontier province; slightly martial; second choice if the trademark search rejects Eonmark | crates.io 404, Steam 0, GitHub 0 |
| Hearthmarch | Hearth (towns as anchors) plus march (border land); warm and calm; longer to type | crates.io 404, Steam 0, GitHub 0 |
| Boundstone | Boundary stones mark borders; a GitHub org `boundstone` and an unrelated repository already exist, so the slug is contested | crates.io 404, Steam 0, itch 0 |
| Cadastre | The land register of parcel ownership; thematically exact but a generic GIS term with many unrelated repositories and an existing Rust strategy game using the word | crates.io 404, Steam 0 |
| Bournmark | "Bourn" (archaic boundary) plus "mark"; obscure enough that conflicts are unlikely | Not individually verified; fallback only |
| Isoline | The contour where ownership changes, which is literally the border shader; likely collides with cartography software | Not verified; lowest priority |

Rejected outright: Demesne (existing Steam game, 2016), Marchland (existing board game), Tilth and Ambit (crates.io names taken).

## No placeholder crates

crates.io forbids name squatting, so no empty `eonmark`, `eonmark-sim` or `eonmark-rules` crate is published to reserve the name. The workspace crates are `publish = false`. If the owner wants crates published, `eonmark-sim` and `eonmark-rules` ship with real content after M1 and M3a respectively; this is a v0.2 roadmap item in [ROADMAP.md](ROADMAP.md). Name protection comes from the repository, the domain and the itch.io page, not from copyleft and not from placeholder crates.

## Policy for mentioning the inspiration

Eonmark uses original names for everything: ages, resources, the faction, units, buildings, identifiers, data, assets, UI strings and commit messages. The inspiration's title appears only in [README.md](../README.md) (the single disclaimed sentence) and [design/prior-art.md](design/prior-art.md) (its Inspiration section and source citations). Its common abbreviation appears nowhere. `scripts/check_trademark.sh` enforces this in CI. See [adr/0001-license.md](adr/0001-license.md) for the reasoning.
