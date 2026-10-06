# Prior art

This document records where Eonmark's ideas come from and what it chose not to copy. It is the only design document that names the commercial game that inspired the project; every other document, the code, the data files, the asset names and the UI strings use Eonmark's own names, and `scripts/check_trademark.sh` enforces that in CI. The second half lists the open-source strategy games and Rust game projects whose public practices shaped how this repository is organised. Game mechanics are ideas and are not protected by copyright; names, text, art and audio are, so this document discusses mechanics only and cites public sources rather than quoting them.

Area: `area:docs`. Milestone: M0.

## Inspiration

Eonmark is an original game inspired by Rise of Nations (Big Huge Games, 2003; Extended Edition 2014). It is not affiliated with or endorsed by Microsoft.

What drew the project to that game is its macro layer: cities that project borders, attrition that punishes armies far from supply, an economy capped by a commerce limit, costs that ramp with what you own, and ages gated by how much you have researched rather than by a single building. Eonmark keeps those ideas, renames every system, chooses its own numbers, and leaves out most of the rest.

| Concept in the inspiration | Eonmark's system | Design document |
| --- | --- | --- |
| National borders projected by cities | Territory field: Towns, Watchtowers and Statecraft levels stamp integer strength discs; the strongest layer owns a tile and an exact tie is neutral | [territory.md](territory.md) |
| Attrition on unsupplied invaders, cancelled by a supply unit | Harrying (researched at a Watchtower) and attrition, cancelled by a Supply Wain within 14 tiles | [attrition.md](attrition.md) |
| Cities that are captured, not destroyed | Towns and the Seat; annexation after a 60 s timer; capture the Seat to win | [towns.md](towns.md) |
| Eight ages gated by cumulative tech count | Three ages, Hearth, Masonry and Charter, gated by 2 and then 5 cumulative techs plus resource costs | [tech-and-ages.md](tech-and-ages.md) |
| Knowledge as a research currency | Lore from Scriptoria and Scribes, exempt from the Yield Cap, with its own 999 stockpile cap | [economy.md](economy.md) |
| Commerce cap on rate-based income | Yield Cap per resource, 70 raised to 100, 150, 200 by Trade techs, overflow discarded, amber HUD readout | [economy.md](economy.md) |
| Four library tech lines | Arms, Statecraft, Trade and Letters, three levels each, effects as integer Modifier rows in RON | [tech-and-ages.md](tech-and-ages.md) |
| Ramping costs per copy owned | Yeomen +1 Grain each, buildings +20 % per copy, military triangular per training building capped at 2.25x | [economy.md](economy.md) |
| Auto-seeking citizens | Yeomen seek work after a 5 s idle delay | [economy.md](economy.md) |
| Infantry, cavalry and siege counters | Skirmisher, Shieldbearer, Bowman, Outrider, Mangonel on a fixed counter table | [combat.md](combat.md) |
| AI difficulty as an income cheat plus an aggression switch | Easy, Standard and Hard with income intervals 30, 25 and 20 s, documented as a cheat | [ai.md](ai.md) |
| Territory-percentage victory | 45-minute cap with a territory-percentage tiebreak so every match ends | [towns.md](towns.md) |

Deliberately left out of v0.1 (the list in [../ROADMAP.md](../ROADMAP.md) is authoritative):

| Left out | Why |
| --- | --- |
| Wonders | a second victory track and a large art budget; the territory tiebreak already guarantees an ending |
| Rare resources and merchants | a sixth resource and a scouting minigame; Lore already gives the fourth resource a purpose |
| Caravans, markets and trade routes | wealth as a side system; v0.1 has no wealth resource |
| Eight ages | three ages fit a 20 to 40 minute match; the age list is data, so a fourth age is a community issue |
| Navy, air, missiles and a doomsday clock | the map is flat and has no water pathing |
| Conquer the World (a strategic turn layer) | the most expensive subsystem in the inspiration's own history; out of scope by design |
| Generals, spies, formations, stances, garrisoning, militia, ruins | micro texture that the macro-over-micro design does not need |
| Multiple nations with bonus bundles | one faction, Freeholders, with no bonuses; a second arrives as pure data in M9 |

Sources consulted for the mechanics (no fan wiki text was copied; fan wiki prose is CC-BY-SA and is not used in this repository):

- https://en.wikipedia.org/wiki/Rise_of_Nations
- https://www.gamedeveloper.com/disciplines/postmortem-big-huge-games-i-rise-of-nations-i-
- https://gamedeveloper.com/design/ruling-the-world-with-rise-of-nations-again-
- https://www.hardcoregaming101.net/rise-of-nations/

## Open-source RTS projects we learned from

Every long-lived open-source RTS shares two traits: a deterministic command-driven simulation separated from rendering, and unit, building and tech definitions in plain text that non-programmers can edit. Eonmark adopts both (see [../ARCHITECTURE.md](../ARCHITECTURE.md) and [../DATA_FORMAT.md](../DATA_FORMAT.md)). The projects below are where those practices and their failure modes were observed; the research that produced these notes is summarised here with its sources.

0 A.D. (Wildfire Games). A C++ engine with game logic in JavaScript and a turn manager that batches commands so every peer stays in lockstep. Two lessons: the sim/render split works at scale, and an unregistered name plus no contributor agreement left the project unable to stop a stranger from selling a renamed copy on Steam in 2022. Eonmark claims its name early and records its licence decision in an ADR before the first external PR.
Sources: https://trac.wildfiregames.com/wiki/EngineDocumentation?version=7, https://www.gamingonlinux.com/2022/10/someone-released-the-foss-rts-0-ad-on-steam-without-speaking-to-the-developers/

OpenRA. Every client runs the full simulation and only player commands cross the wire; actors are collections of traits whose properties are set in MiniYaml, so simple mods are text only. Eonmark's RON data and `data-check` validator follow this pattern. OpenRA also shows how a feature-driven release schedule stretches to a year or more between stable builds, which is why Eonmark's roadmap is milestone-gated with acceptance commands.
Sources: https://www.openra.net/book/glossary.html, https://github.com/OpenRA/OpenRA/wiki/Modding-Guide, https://www.openra.net/news/

Spring / Recoil and Beyond All Reason. The Spring 106 release broke the engine's Lua compatibility contract and the community forked into Recoil; Beyond All Reason, built on Recoil, is the current open-source RTS success with a large contributor base, a CONTRIBUTING guide and a good-first-issue listing, but it ships no macOS build and its asset licensing mixes CC-BY-SA with non-commercial, no-derivatives Creative Commons licences, which makes its content unusable downstream. Eonmark's lessons: never land a change that removes a shipped feature, keep one licence per asset class with per-file provenance, and ship Mac first. Recoil also publishes an AI-usage policy, which is why `AI_CONTRIBUTIONS.md` exists from day one.
Sources: https://github.com/beyond-all-reason/Beyond-All-Reason, https://github.com/beyond-all-reason/RecoilEngine, https://raw.githubusercontent.com/beyond-all-reason/Beyond-All-Reason/master/license_general.txt, https://www.gamingonlinux.com/2022/01/spring-rts-v106-released-with-opengl-4-support/

Warzone 2100. Source released in 2004 and still shipping, with every stat in JSON under `data/mp/stats` and a time-boxed release cadence, and it produces macOS builds in CI. Eonmark's `data/rules/*.ron` with a schema README per directory is the same idea; its time-boxed milestones and recorded CI wall times come from watching which projects ship predictably.
Sources: https://github.com/Warzone2100/warzone2100, https://github.com/Warzone2100/warzone2100/tree/master/data/mp/stats, https://wz2100.net/

Veloren. A Rust game with about 150 contributors, a contributor book, a review-request workflow and nightly builds. It also shows cadence decay: release gaps grew from six to thirteen months and the weekly devlog became quarterly. Eonmark's `just devlog` generates the fortnightly post from merged PR titles so the habit cannot quietly stop, and GOVERNANCE.md names a second-maintainer goal because solo-maintained Rust RTS projects (Digital Extinction, Oxidator, Chariot) were all archived.
Sources: https://book.veloren.net/contributors/before-you-contribute.html, https://veloren.net/blog/, https://github.com/DigitalExtinction/Game, https://github.com/Ruddle/oxidator

Bevy. Not an RTS, but the engine and the governance template. Bevy's organisation document (lead, maintainers, subject-matter experts, working groups, a trivial-change label as the good-first-issue marker) was written when a single lead became the bottleneck; GOVERNANCE.md adopts the same shape at the three-regular-contributors threshold. Bevy's quarterly breaking releases are the reason the simulation is engine-free and the engine pin is upgraded in a scheduled ADR (M9).
Sources: https://bevy.org/news/scaling-bevy-development/, https://bevy.org/learn/contribute/project-information/bevy-organization/, https://bevy.org/news/bevy-0-19/

## Open questions

- Whether to add a short "credits and influences" line to the game-over screen or About box. The design keeps the inspiration out of UI strings; this document and the single README sentence are the only mentions, and that should stay so unless the owner decides otherwise.
- As new open-source projects publish AI-contribution policies, `AI_CONTRIBUTIONS.md` may need revising; this document only records the state on 2026-10-05.
