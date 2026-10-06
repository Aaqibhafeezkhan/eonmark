# Fog of war

This document specifies Eonmark's three-state fog of war: the per-player visibility grid the simulation owns, how it is stamped from vision radii, how the AI is made to respect it through `SimView`, and how the game crate renders it through the B channel of the field texture, toggles enemy visibility and darkens the minimap. The grid is hashed simulation state because it changes AI decisions. The vision data exists from M3a; the system is built and switched on in M7.

Area: `area:sim` (grid, `SimView`) and `area:game` (shader channel, visibility toggling, minimap). Milestone: M7, with vision radii authored in M3a (see [../ROADMAP.md](../ROADMAP.md)).

## Target numbers

| Quantity | Value | Where it is set |
| --- | --- | --- |
| Grid | 128 x 128 u8 per player (16 KiB per player) | `VisibilityGrid` in `crates/sim/src/visibility.rs` |
| States | 0 unexplored, 1 explored, 2 visible | `visibility.rs` |
| Stamp cadence | every tick, inside `Sim::step` | `visibility.rs` |
| Vision radii | M0 placeholder: `vision: (unit_tiles: 8, building_tiles: 10, town_tiles: 14)`, three values by class | `data/rules/rules.ron`; per-kind radii, if wanted, replace the three values at M3a with a `rules_version` bump |
| Albedo multiplier | 0.0 unexplored, 0.45 explored, 1.0 visible | `assets/shaders/field.wgsl` (`FieldExt`) |
| Texture channel | B of the 128 x 128 RGBA8 field texture (R owner, G strength, B fog, A supply) | `crates/game/src/ground.rs` |
| Enemy units | hidden unless their tile is state 2 for the viewing player | `present.rs` |
| Enemy buildings | shown with current state when their tile is state 1 or 2; no ghosting | `present.rs` |
| Minimap | same grid, same multipliers, repainted every frame | `hud/minimap.rs` |
| Frame budget | 400 units + two full Towns + fog at >= 60 fps, 1440p, release, base M-series | M7 acceptance |
| Hash impact | one `rules_version` bump in M7; every earlier golden fixture must verify after it | M7 acceptance |

Vision radius values are not fixed by the design. M0 ships a three-value placeholder in `rules.ron` (`vision.unit_tiles` 8, `vision.building_tiles` 10, `vision.town_tiles` 14, validated but unused). If per-kind radii are wanted, they replace that block at M3a with a `rules_version` bump, so the rules hash and every fixture from M3a onward already contain them; the values are tuned at M7.

## The visibility grid

`VisibilityGrid` holds one `Vec<u8>` of 16384 bytes per player, indexed `y * 128 + x` like the cost grid in [pathing.md](pathing.md). It is part of the hashed state and is serialised by `snapshot()`; nothing about it is derived.

Each tick, for each player in `PlayerId` order, the stamp runs in two passes:

1. Decay. Every tile at state 2 becomes state 1. Explored tiles never fall back to 0.
2. Stamp. For each unit and each building owned by the player, in id order, every tile whose centre lies within the owner's vision radius becomes state 2. The test is integer: with the source at tile `(cx, cy)` and radius `r` in tiles, tile `(x, y)` is visible when `(x - cx)^2 + (y - cy)^2 <= r^2`. Unit tile positions are the floor of their fixed-point position.

Stamping is a maximum operation, so the order of sources within a player does not change the result; the id order is kept anyway so every loop in the sim reads the same way. Buildings under construction and Towns under annexation stamp for their current owner. A Town whose land is neutral during an annexation timer (see [towns.md](towns.md)) still gives vision to the defender until it flips.

Cost. Two passes over 16 KiB per player plus one disc per source. With 200 units and a few dozen buildings this is well under the step budget; it is measured by the M7 bench line rather than given its own gate.

## What the AI is allowed to know

The scripted bot in [ai.md](ai.md) runs inside `Sim::step` and reads the world only through `SimView`. `SimView` applies the viewing player's grid before handing out anything about other players:

| Query | Rule |
| --- | --- |
| Own units, buildings, stockpiles, territory | always available |
| Enemy units | only those whose tile is state 2 for the viewer |
| Enemy buildings | only those whose tile is state 1 or 2 for the viewer, with their current HP and owner |
| Territory field | always available (borders are public information in v0.1; see open questions) |
| The grid itself | `visibility(player) -> &[u8]`, `state(player, tile) -> u8` |

Because the bot sees the same filtered view a human sees, a hidden army is hidden from both. The M6 influence maps are built from this filtered view, so unseen enemy strength does not appear in them. This is the reason the grid is hashed: a different grid gives the bot different inputs and therefore a different command stream.

Human orders are not filtered. The player may order units into unexplored or explored tiles; the design is silent and allowing it is the usual RTS behaviour.

## Rendering

The game crate uploads the field texture through `sync_field_texture` each frame. The B channel carries the human player's visibility state per tile. The `FieldExt` material extension (`ExtendedMaterial<StandardMaterial, FieldExt>`) reads the texture in the fragment shader and multiplies the ground albedo by 0.0 for unexplored, 0.45 for explored and 1.0 for visible, after the territory tint and border isoline from [territory.md](territory.md) are applied. The exact byte encoding of the three states in the channel is the implementer's choice; the shader maps whatever encoding is chosen to the three multipliers.

The FieldExt shader is optional in M3b (a vertex-coloured overlay mesh may ship first) and mandatory from M7, because fog rides the same texture. There is no separate fog pass and no post-processing.

Enemy units. `present.rs` mirrors `UnitId` to `Entity`. Each frame it sets `Visibility::Hidden` on enemy unit entities whose tile is not state 2 for the human player and `Visibility::Inherited` otherwise. Health bars and selection rings are separate entities keyed by `UnitId` (see [ui.md](ui.md)) and follow the same toggle.

Enemy buildings. A building in a state 1 or 2 tile is drawn with its current model, HP bar and owner colour. In v0.1 there is no ghosting: the game does not remember what a building looked like when last seen, it shows the live state whenever the tile has been explored. Buildings in state 0 tiles are hidden. Ghosting is listed as out of scope in [../ROADMAP.md](../ROADMAP.md).

Own units and buildings are always drawn.

## Minimap

The minimap is a CPU-painted 128 x 128 `Image` shown through an `ImageNode`, one pixel per tile, repainted every frame from sim data:

1. Base colour by tile type (grass, forest floor, rock, water).
2. Territory tint from the owner layer, same colours as the ground.
3. Fog darkening with the same 0.0 / 0.45 / 1.0 multipliers.
4. Dots: own units and buildings always; enemy units only on state 2 tiles; enemy buildings on state 1 or 2 tiles.

Left-clicking the minimap moves the camera to that tile. Right-clicking it orders the current selection there, through the same command path as a right-click on the ground. The minimap must mirror fog and borders within one frame of the sim change that caused them.

## Interactions with other systems

| System | Interaction |
| --- | --- |
| Rules ([../DATA_FORMAT.md](../DATA_FORMAT.md)) | vision radii are data; `sim-cli data-check` fails if any unit or building kind lacks one once the field exists |
| Territory ([territory.md](territory.md)) | shares the field texture; fog multiplies after tint and isoline |
| Combat ([combat.md](combat.md)) | auto-acquire uses weapon range, which the data should keep at or below vision radius so units never shoot what their owner cannot see (see open questions) |
| Attrition ([attrition.md](attrition.md)) | independent of fog; the supply coverage channel (A) is the same texture |
| AI ([ai.md](ai.md)) | reads only the filtered `SimView`; influence maps inherit the filter |
| Replays | the grid is replayed, not recorded; verifying a replay reproduces it tick for tick |
| Snapshot | serialised as plain state; nothing to rebuild in `restore()` |

## Acceptance checklist (M7)

Copied from the M7 milestone in [../ROADMAP.md](../ROADMAP.md); the lines that concern fog and the minimap. The remaining M7 lines (assets, visuals.ron, audio) are owned by [../ART_STYLE.md](../ART_STYLE.md).

- [ ] `cargo test -p sim fog`: visibility grid matches vision radii for a fixture; an enemy unit outside all vision cells is hidden in SimView; all earlier golden fixtures verify after the single rules_version bump in this milestone.
- [ ] In-game: unexplored terrain is black, explored darkened, visible lit; enemy units vanish without friendly vision; the minimap mirrors fog and borders within one frame; clicking the minimap moves the camera; right-clicking it orders selected units (`--scenario fog_walkthrough` replay + ci_testing screenshots as the owner proxy).
- [ ] `cargo run --release -p game` shows >= 60 fps at 1440p with 400 units + two full Towns + fog; draw calls < 500; `cargo tree -i cpal` shows exactly one cpal version.

Commands:

```bash
cargo test -p sim fog
cargo run -p sim-cli --release -- verify crates/sim/tests/fixtures/move_500.eonreplay
cargo run --release -p game -- --scenario fog_walkthrough
```

## Open questions

- Vision radius starting values per kind (Yeoman, Scribe, military kinds, Towns, Watchtower). Set at M3a in `rules.ron`; the Watchtower should see further than any unit so its border placement by the bot has a purpose.
- Should `data-check` enforce `vision_radius >= attack_range` for every kind that shoots? Recommended, so no unit attacks an invisible target; decide at M4a when ranges are authored.
- Whether the enemy's territory field should be hidden under state 0 tiles. v0.1 shows borders everywhere, which keeps the minimap readable; a stricter rule would also have to filter `SimView` for the bot.
- Fog edge softness. Bilinear sampling of the B channel gives a one-tile soft edge for free; nearest sampling gives hard tile edges. Judge on the M7 screenshot against [../ART_STYLE.md](../ART_STYLE.md).
- The design's sub-hash list (units, buildings, economy, territory, tech, pathing queue, rng) does not name visibility. Adding a `visibility` sub-hash at M7 makes a fog desync bisectable with `sim-cli hash-dump`.
