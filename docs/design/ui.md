# HUD, menus, picking and controls

This document specifies the shipped user interface of Eonmark: the `bevy_ui` HUD and its panels, the three menus, the picking rules that keep HUD clicks and world clicks apart, the control scheme, the camera, HiDPI handling, how health bars and selection rings are drawn, and the developer-only egui panels. Shipped builds use `bevy_ui` only; `bevy_egui` and `bevy-inspector-egui` exist behind the `dev` feature and never reach a release. Everything here lives in `crates/game` (`hud/`, `menu/`, `selection.rs`, `orders.rs`, `camera.rs`, `present.rs`, `dev_tools.rs`).

Area: `area:ui` and `area:game`. Milestones: M2 (selection, orders, camera, HUD skeleton), M3b (command card, resource bar, build ghost), M4b (research panel, age indicator, health bars), M6 (speed, pause, New Game selector, game-over stats), M7 (minimap, menus, fonts, panels). See [../ROADMAP.md](../ROADMAP.md).

## Target numbers

| Quantity | Value | Where it is set |
| --- | --- | --- |
| Default window | 1280 x 800 logical, Retina scale 2.0 kept separate from logical size | `app.rs` |
| Camera pitch | about 55 degrees, yaw locked | `camera.rs` |
| Camera zoom | 15 m to 60 m with smoothing | `camera.rs` |
| Camera speeds | all multiplied by `Time::delta_secs`; identical travel at 30 and 120 fps | `camera.rs`, M2 acceptance (`--max-fps 30/120`) |
| Sim ticks per frame | at most 8, independent of game speed | `sim_driver.rs` |
| Game speed | x1, x2, x4, plus pause | M6 |
| Frame target | >= 60 fps at 1440p with 400 units + two full Towns + fog, release, base M-series | M7 acceptance |
| Draw calls | < 500 (dev-panel counter) | shared meshes and materials |
| UI scale | persisted `UiScale` multiplier; applies to `Val::Px` only | settings |
| Font | Inter variable (OFL-1.1) through a variable `TextFont` | `assets/fonts/` |
| Panels | Kenney UI Pack 9-slice via `NodeImageMode::Sliced` | `assets/ui/` |
| Control groups | ctrl+1 to ctrl+9 | `selection.rs` |
| Settings in v0.1 | UI scale and volume only | pause menu |

## HUD layout

The HUD is a fixed set of `bevy_ui` nodes. Layout-only root nodes carry `Pickable::IGNORE` so they do not swallow clicks; panels that show content block world picking, which is the `bevy_ui` default, so a click on a panel never becomes a ground order.

| Region | Panel | Contents |
| --- | --- | --- |
| Top | Resource bar | For each of Grain, Lumber, Ore, Lore: current stockpile and the rate per 30 s. The rate text turns amber when the resource is at its Yield Cap and production is being discarded (see [economy.md](economy.md)). Lore shows its own 999 stockpile cap instead. |
| Top, right of resources | Population and age | `used / cap` population and the current age name (Hearth, Masonry, Charter) with the Advance button and its cost when the gate is met (M4b). |
| Bottom left | Selection panel | One selected unit or building: name, HP, owner, current order; for a building, its training or research queue with progress and a Cancel per slot. Several units: a grid of kind icons with counts; clicking an icon narrows the selection to that kind. |
| Bottom right | Command card | Buttons for what the selection can do: build (Yeomen), train (Muster Hall, Engine Yard, Town), research (Scriptorium, Watchtower), rally, stop, attack-move. Each button shows its cost. A button is disabled with a tooltip when the player cannot afford it, is pop-capped, lacks the age or tech, or has no building site; the tooltip text is the same reason string the sim uses in `CommandRejected`. |
| Bottom centre | Minimap | A CPU-painted 128 x 128 `Image` in an `ImageNode` painted from the territory and fog grids plus unit and building dots each frame (M7, see [fog.md](fog.md)). Left click moves the camera. Right click orders the selection. |
| Above the command card | Message line | The last rejection or notice for a few seconds: "Outside your borders", "Not enough Ore", "Population cap reached", "Town annexed". |

Costs and names on buttons come from `data/rules/*.ron` and `data/rules/strings/en.ron`; the HUD never hard-codes a number.

The infinite-queue toggle on training buildings repeats the last trained kind while resources allow. The idle-Yeoman hotkey selects the next idle Yeoman and centres the camera on it.

## Menus

| Menu | Entered by | Contents |
| --- | --- | --- |
| Main menu | app start, Quit to menu | New Game (difficulty Easy/Standard/Hard, seed, faction: Freeholders), Quit. The seed field defaults to a fresh value and accepts a typed number so a match can be replayed by hand. |
| Pause menu | Esc during a match | Resume, UI scale, volume, Borderless fullscreen toggle (`WindowMode::BorderlessFullscreen` with `Window::borderless_game`; exclusive fullscreen is never used), Quit to menu. The sim does not tick while this menu is open. |
| Game over | `Sim::outcome()` returns `Some` | Victory or Defeat, decisive or tiebreak, duration, units lost by kind, Towns annexed, ages reached, minutes spent at the Yield Cap, the replay's final hash and file name, Play Again (same settings, new seed), Main menu. |

States: `Menu`, `Skirmish`, `Paused`, `GameOver` in `app.rs`. Pause through the menu and pause through Space are the same sim state; the menu adds the overlay.

## Picking rules

Mesh picking is expensive if every mesh is a candidate on every pointer move. The rules are set on day one of M2 and are not negotiable:

| Rule | Setting |
| --- | --- |
| Picking backend | `MeshPickingSettings { require_markers: true }`; `MeshPickingCamera` on the RTS camera only |
| Pickable entities | `Pickable` on unit and building entities only; terrain, trees, rings, bars and markers are not pickable |
| HUD roots | `Pickable::IGNORE` on layout-only root nodes; content panels keep the default and block world picking |
| Ground clicks | one ray-plane intersection with the y = 0 plane in `orders.rs`; never a mesh raycast against the terrain |
| Drag box | project each sim unit position to screen space and test against the rectangle; no per-mesh raycasts; done in `selection.rs` |
| Click on HUD | consumed by the HUD; no world order, no deselection |

The M2 acceptance checks the last row with the `hud_click` scenario.

## Controls

Keys the design fixes are given as such. Keys marked "proposed" are not fixed by the design; M2 writes the keybinding table and the `dev` hotkey cheat-sheet is generated from it.

| Action | Input | Milestone |
| --- | --- | --- |
| Select unit or building | left click | M2 |
| Add to or remove from selection | shift + left click | M2 |
| Box select | left drag on the ground | M2 |
| Select all of a kind on screen | double click a unit | M2 |
| Assign control group | ctrl + 1 to 9 | M2 |
| Recall control group | 1 to 9 | M2 |
| Context order (move, attack, gather, build site confirm, rally when a building is selected) | right click on ground or target | M2, M3b |
| Attack-move | A, then left click a point | M4b |
| Stop | S (proposed) | M2 |
| Set rally point | right click with a training building selected | M3b |
| Infinite queue toggle | button on the command card; key proposed Q | M3b |
| Select next idle Yeoman | key proposed period (.) | M3b |
| Pause | Space | M6 |
| Game speed x1 / x2 / x4 | proposed minus and equals to step down and up | M6 |
| Pause menu | Esc | M2 |
| Camera pan | W A S D, arrow keys, edge scroll, two-finger trackpad scroll (`MouseScrollUnit::Pixel`) | M2 |
| Camera zoom | mouse wheel (`MouseScrollUnit::Line`), trackpad pinch (`PinchGesture`, Bevy `gestures` feature) | M2 |
| Camera jump | left click on the minimap | M7 |
| Order via minimap | right click on the minimap | M7 |

The macOS Cmd+Q menu item quits cleanly through Bevy; see [macos-packaging.md](macos-packaging.md). Edge-scroll margin and camera speed moving into `rules.ron` is a `good first issue`.

## Camera

Perspective camera, pitch about 55 degrees, yaw locked so the map never rotates. Zoom moves the camera along its view direction between 15 m and 60 m above the ground with smoothing toward a target distance. Pan moves the look-at point on the ground plane and clamps it to the map. Every speed is multiplied by `Time::delta_secs`, so travel distance is identical at 30 and 120 fps; the M2 acceptance records two replays with `--max-fps 30` and `--max-fps 120` and compares hashes, and the owner checks camera travel by eye. Edge scroll is active only when the window has focus and the pointer is inside it.

Rendering of units between ticks: `present.rs` keeps the previous and current sim positions per `UnitId` and lerps with `Time<Fixed>::overstep_fraction()`. The camera never reads sim state directly.

## HiDPI and fonts

The bundle declares `NSHighResolutionCapable` (see [macos-packaging.md](macos-packaging.md)). Bevy's `WindowResolution` keeps logical and physical size apart, so a 1280 x 800 window renders at 2560 x 1600 on a Retina display. `UiScale` multiplies `Val::Px` only, so every HUD dimension is authored in `Val::Px` and the UI scale setting is a single persisted multiplier. Percent and viewport units are avoided in the HUD for this reason.

Text uses the Inter variable font through a variable `TextFont`; the OFL license text ships beside the font file. Panels use Kenney UI Pack 9-slice images through `NodeImageMode::Sliced`. The palette is matte and judged on both a P3 display and an sRGB screenshot, because wgpu 29 can oversaturate on wide-gamut surfaces (see [../ART_STYLE.md](../ART_STYLE.md)).

## Health bars, rings and markers

Health bars and selection rings are separate flat entities keyed by `UnitId`, never children of moving unit entities. This keeps unit transforms flat, lets bars be billboarded and scaled by distance independently, and avoids re-parenting when a unit dies. Selection rings and move markers are retained gizmos; `ClusteredDecal` is unavailable on Metal and is not used. The attrition indicator and supply radius ring (M5a) and the annexation timer (M5a) are drawn the same way. Bars follow the owner's fog visibility (see [fog.md](fog.md)).

Health bars appear in M4b together with distinct primitive silhouettes per unit kind; from M7 the primitives are replaced by glTF through `data/visuals.ron` without touching the HUD.

## Developer-only panels

Behind the `dev` cargo feature only: `bevy/bevy_dev_tools`, `bevy_egui` 0.40.1 and `bevy-inspector-egui` 0.37.0. They provide the FPS overlay, the frame-time graph, the `Sim::hash()` and sub-hash readout, the draw-call counter with its 500 budget, a world inspector, and tuning panels. `--stress N` spawns N units for profiling. `cargo clippy -p game --features dev` runs in CI so the feature does not rot, and `bundle.sh` fails if `bevy_dylib` is linked, so no dev build ships. No gameplay UI may depend on egui.

## Interactions with other systems

| System | Interaction |
| --- | --- |
| Sim ([../ARCHITECTURE.md](../ARCHITECTURE.md)) | the HUD reads `SimView` and `drain_events()`; it issues commands only through `PendingCommands`, which the driver stamps and feeds to `Sim::step` |
| Economy ([economy.md](economy.md)) | amber cap readout; costs on buttons; rejection reasons on the message line |
| Territory ([territory.md](territory.md)) | the build ghost turns red outside borders and green inside; "Outside your borders" on the message line |
| Fog ([fog.md](fog.md)) | minimap darkening, enemy visibility toggling, bars follow visibility |
| Replays ([../DETERMINISM.md](../DETERMINISM.md)) | the game-over screen shows the replay's final hash so the owner can run `sim-cli verify` against it |
| macOS ([macos-packaging.md](macos-packaging.md)) | trackpad mapping, Cmd+Q, borderless fullscreen |

## Acceptance checklist

Copied from the milestones that build UI, in [../ROADMAP.md](../ROADMAP.md).

M2:

- [ ] `cargo run -p game --features dev -- --scenario units200` spawns 200 units; drag-box all, right-click across the map; all arrive within 60 s game time; FPS overlay stays >= 60 on the dev Mac in the dev profile with no periodic hitch visible in the frame-time graph at the 1 s replay flush.
- [ ] `--scenario scripted_moves --max-fps 30` and `--max-fps 120` produce replays with identical final hashes.
- [ ] [owner] docs/PLAYTEST.md checklist signed: two-finger scroll pans, pinch zooms, wheel zooms, WASD pans, edge scroll pans, camera travel identical at 30 and 120 fps, ctrl+1 then 1 reselects, double-click selects same kind on screen, shift-click adds.
- [ ] Clicking empty HUD area issues no world order; clicking a HUD button does not deselect (automated via `--scenario hud_click` replay + ci_testing screenshot for the owner proxy).

M3b:

- [ ] In-game: place a Croft outside the border and the ghost is red and the HUD says 'Outside your borders'; inside it turns green, Yeomen walk over and build it; founding a second Town visibly expands the tinted territory and border line; the Grain readout turns amber once the cap is reached; at 25/25 pop the Train button is disabled with a tooltip and the sim rejects with `PopCapReached` (`--scenario economy_walkthrough` replay verifies and a ci_testing screenshot shows the amber readout).
- [ ] `cargo run --release -p game -- --scenario economy_walkthrough` holds >= 60 fps on the dev Mac with the overlay (and with FieldExt if landed); dev-panel draw calls < 500.

M6:

- [ ] Game speed x4 and pause work and the replay verifies with the same final hash as the x1 replay of the same scripted input (`--scenario scripted_match --speed 4`).

M7:

- [ ] In-game: unexplored terrain is black, explored darkened, visible lit; enemy units vanish without friendly vision; the minimap mirrors fog and borders within one frame; clicking the minimap moves the camera; right-clicking it orders selected units (`--scenario fog_walkthrough` replay + ci_testing screenshots as the owner proxy).
- [ ] [owner] docs/screenshots/m7_town.png signed off against the ART_STYLE.md checklist (palette, no glow, readable at 40 m, matte HUD) in the PR.

Commands:

```bash
cargo run -p game --features dev -- --scenario units200
cargo run -p game --features dev -- --scenario scripted_moves --max-fps 30
cargo run -p game --features dev -- --scenario scripted_moves --max-fps 120
cargo run -p game --features dev -- --scenario hud_click
cargo run --release -p game -- --scenario economy_walkthrough
```

## Open questions

- Final keys for Stop, infinite queue, idle Yeoman and the speed steps. Proposed above; M2 fixes them in the keybinding table and the `dev` cheat-sheet is generated from that table.
- Whether the minimap draws the camera's view rectangle. Not in the design; cheap to add in M7 if the owner wants it.
- Whether game speed x1/x2/x4 ships in release builds or stays `dev`-only. Listed as an owner decision in [../ROADMAP.md](../ROADMAP.md); this document assumes release.
- Tooltip delay and whether tooltips also appear on enabled buttons (cost breakdown). M3b decides; `good first issue` for hover states exists.
- Whether the selection panel shows a unit's attrition state as text in addition to the indicator on the unit (M5a).
