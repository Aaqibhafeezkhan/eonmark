# HUD, menus, picking and controls

This document specifies the shipped user interface of Eonmark: the `bevy_ui` HUD and its panels, the three menus, the picking rules that keep HUD clicks and world clicks apart, the control scheme, the camera, HiDPI handling, how health bars and selection rings are drawn, and the developer-only egui panels. Shipped builds use `bevy_ui` only; `bevy_egui` and `bevy-inspector-egui` exist behind the `dev` feature and never reach a release. Everything here lives in `crates/game` (`hud.rs`, `selection.rs`, `orders.rs`, `camera.rs`, `present.rs`, `macos_menu.rs`, `dev_tools.rs`; `menu/` arrives with the main menu in M5b). The M2 state is marked inline: the HUD is a skeleton (two bars and one button), selection and orders are complete, menus and states do not exist yet.

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

The HUD is a fixed set of `bevy_ui` nodes. Layout-only root nodes carry `Pickable::IGNORE` so they do not swallow clicks; panels that show content block world picking, which is the `bevy_ui` default (`UiPickingSettings::require_markers` is `false`, so every `Node` without `Pickable::IGNORE` blocks lower entities), so a click on a panel never becomes a ground order.

M2 skeleton (`hud.rs`, done): a full-window root node with `Pickable::IGNORE` (`HudRoot`), a top bar of `TOP_BAR_HEIGHT_PX` = 28 logical px (`TopBar`), a bottom bar of `BOTTOM_BAR_HEIGHT_PX` = 96 px (`BottomBar`), and one `Button` of 72 x 28 px labelled Stop (`StopButton`) that issues `Command::Stop` for the selection through `PendingCommands::push_local` from a per-entity `Pointer<Click>` observer (`hud::on_stop_click`). `Interaction` only drives the button's hover and pressed colours: bevy_ui derives it from raw `ButtonInput` plus the cursor position, independently of the picking pipeline, so issuing on `Interaction::Pressed` as well would send two Stops per click. Both bars keep the picking default and so block the ground. The `Update` systems that turn raw mouse input into world actions (drag box, orders) sit in `hud::WorldInputSet`; the `hud_click` check runs before that set so the real systems are exercised under `PointerOverUi`. Colours come from `palette::HUD_*`, sizes are authored in `Val::Px`. The table below is the full v0.1 HUD; every panel in it replaces or fills these two bars in later milestones.

| Region | Panel | Contents |
| --- | --- | --- |
| Top | Resource bar | For each of Grain, Lumber, Ore, Lore: current stockpile and the rate per 30 s. The rate text turns amber when the resource is at its Yield Cap and production is being discarded (see [economy.md](economy.md)). Lore shows its own 999 stockpile cap instead. |
| Top, right of resources | Population | `used / cap` population at the right end of the top bar. |
| Top right, under the bar | Age strip | The current age name (Hearth, Masonry, Charter) and the Advance button (M4b, done), on a strip hanging under the right end of the top bar so the bar's room stays with the resource readouts. The button is always shown until the last age: greyed with the tech count ("Advance: 1 / 2 techs") while that is short, else showing the next age and its cost ("Advance to Masonry Age: 250 Grain 100 Lore"), greyed while a resource is short (owner decision 7, ROADMAP M4b Notes; ART_STYLE "greyed, not hidden"). |
| Bottom left | Selection panel | One selected unit or building: name, HP, owner, current order; for a building, its training or research queue with progress and a Cancel per slot (per-slot Cancel is not built yet: help-wanted issue #41). Several units: a grid of kind icons with counts; clicking an icon narrows the selection to that kind. |
| Bottom right | Command card | Buttons for what the selection can do: build (Yeomen), train (Muster Hall, Engine Yard, Town), research (Scriptorium, Watchtower), rally, stop, attack-move. Each button shows its cost. A button is disabled with a tooltip when the player cannot afford it, is pop-capped, lacks the age or tech, or has no building site; the tooltip text is the same reason string the sim uses in `CommandRejected`. |
| Bottom centre | Minimap | A CPU-painted 128 x 128 `Image` in an `ImageNode` painted from the territory and fog grids plus unit and building dots each frame (M7, see [fog.md](fog.md)). Left click moves the camera. Right click orders the selection. |
| Above the command card | Message line | The last rejection or notice for a few seconds: "Outside your borders", "Not enough Ore", "Population cap reached", "Town annexed". Since M4b also "Letters I researched", "Masonry Age reached", "Watchtower destroyed" and "Muster Hall lost" (see Notices below). |

Costs and names on buttons come from `data/rules/*.ron` and `data/rules/strings/en.ron`; the HUD never hard-codes a number.

The infinite-queue toggle on training buildings repeats the last trained kind while resources allow (not built yet: it needs a repeat flag on the sim's training queue, a `Command` and a hash change; issue #42, help wanted and needs-design). The idle-Yeoman hotkey selects the next idle Yeoman and centres the camera on it.

## Research panel (M4b)

M4b (`command_card.rs`, done): a selected own, complete Scriptorium (the `host` of every tech in `techs.ron`) shows its Train Scribe button and then one Research button per tech line, in `techs.ron` order: Arms, Statecraft, Trade, Letters. Each shows the line's next level, the lowest the player has not researched; a level being researched keeps its button. Once a line's three levels are done, its top level stays on the card, greyed, as "Researched". This is owner decision 8 (ROADMAP M4b Notes): the next level of each line on the card, the blocker on the button, prerequisites in the tooltip, not a full 12-tech grid. Four Research buttons and Train Scribe fit the 4 x 2 card. Card buttons are 140 x 36 px (112 px wide before M4b), so every line a button can show fits one line of it in the card's 12 px font: the longest are "Needs Statecraft II" (19 characters of 7.2 px; the card never shows a "Needs" line, but the code can write one) and the Bowman's cost at the military ramp's cap, "45 Grain 90 Lumber" (`every_card_line_fits_its_button`). At 112 px, Arms II's "100 Ore 150 Lore" and the Bowman's "20 Grain 40 Lumber" wrapped onto a third line that spilled out of the button.

| Part | Contents |
| --- | --- |
| Which buttons | `command_card::research_actions`: for each line the building's kind hosts, the lowest level not in `SimView::researched`, or the top level when all are |
| Enabled or greyed | `SimView::check_research` and nothing else (`research_verdicts_match_the_sim` sends each button's command to a copy of the match and gets exactly the button's verdict, for exact and short stock, a level in progress, a locked level, a finished line and a full queue) |
| First line | The tech's name (`button.research`) |
| Second line, enabled | The quoted cost (`SimView::research_quote`, Letters discount included) |
| Second line, greyed | What blocks it: the cost when only the stockpile is short (the tooltip names the resource); the age that unlocks the level ("Masonry Age"); "Needs Arms II" when the level before is missing, a branch the card never shows: it offers each line's lowest unresearched level, whose level before is researched even after out-of-order debug research, so only a hand-built `CardAction` reaches it (`locked_levels_name_their_prerequisite_and_age`); "Researching 40%" (the queue entry's progress over its queued time; an entry behind another reads 0%); "Researched" |
| Tooltip | Enabled: "Research Letters I, 25 s. Costs 60 Lore" with the quoted time. Greyed: the rejection's message plus what the level requires, the level before it and the age that unlocks it ("Not available in this age. Requires Arms I, Masonry Age"); a first level of the first age requires nothing more |
| Click | `Research { building, tech }` (`command_card::card_command`, as for Train and Cancel); a greyed button puts its tooltip on the message line instead |

A finished line reads "Researched" because a line's top level is unlocked only in the last age and `check_research` checks the age before "already researched". The selection panel adds "Researched: Trade I, Letters I" for a Scriptorium, the top level done in each line it hosts, before the queue line. So that this fifth line never pushes the queue line below the window, the panel is up to `command_card::INFO_WIDTH_PX` = 580 px wide (300 px before M4b) and each of a Scriptorium's lines fits one row: the longest are "Researched: Arms III, Statecraft III, Trade III, Letters III" (60 characters) and "Queue: Statecraft III 99%, Letters III, Trade III, Arms III, Scribe" (67), at 8.4 px a character in the 14 px HUD font, and five rows of 16.8 px fit the 88 px from the bar's top padding to the bottom of the window (`the_scriptoriums_longest_panel_fits_the_bottom_bar`). The panel is a flex item that fills the room left of the card, its text capped at that width, so a window too narrow for the full width narrows the text and wraps it instead of laying it over the card. Per-slot queue Cancel buttons and the infinite-queue toggle are not in M4b (owner decision 9): issues #41 and #42.

## Age indicator and Advance button (M4b)

M4b (`age_hud.rs`, done): the age readout and the Advance button sit on the age strip (`age_hud::AgeStrip`), a panel as tall as the top bar (`AGE_STRIP_HEIGHT_PX` = 28 px) that hangs under the bar's right end, its content right-aligned with the pop readout above it; it grows leftward with the button's label and blocks world clicks as the bars do. The top bar holds only the resource readouts and the pop readout, as in M3b, and its texts never wrap (`TextLayout::no_wrap`): a bar too full for the window runs off its right end instead of splitting every readout onto a second line over the world. With the age on the top bar, the walkthrough's own stocks at tick 1838 left 88 px between the readouts and the age row while one capped rate adds 118 px (", capped at 70"), so a single capped rate wrapped the whole bar. Now the bar fits the 1280 px window with every stockpile at four digits, two rates held at a Trade Yield Cap of 100 and a pop of 100 / 100, with 33 px to spare (`the_top_bar_fits_four_digit_stocks_and_two_capped_rates`, at 8.4 px a character); a third capped rate at that size runs off the end. The `hud_click` points (open ground at the window centre, the empty bottom bar, the Stop button) are unchanged.

| Part | Contents |
| --- | --- |
| Readout | The local player's age name from `ages.ron` (`SimView::age`): "Hearth Age" |
| Button | One line, 24 px tall, the card's font: "Advance: 1 / 2 techs" while the tech count is what the sim rejects (`NotEnoughTechs`), else the next age and its cost (`Rules::amounts` of the next age's cost in `ages.ron`), "Advance to Masonry Age: 250 Grain 100 Lore". So when only a resource is short the greyed button shows the cost, as a greyed Train or Research button does, and its tooltip names the resource. The longest label, "Advance to Charter Age: 500 Grain 300 Lumber 300 Lore", makes the strip about 514 px wide in the Masonry Age |
| Enabled or greyed | `SimView::check_advance_age` and nothing else: greyed for too few techs, greyed with the cost when a resource is short, enabled once the sim would accept `AdvanceAge` (`advance_button_follows_check_advance_age`; the colours follow `CardState` on any button, `button_colours_follow_any_card_state`). A greyed button uses the card's disabled colours (`palette::HUD_DISABLED`, ART_STYLE's Disabled `#6B6B70`, on the pressed-button fill: about 2.1:1, as on every greyed card button since M3b; the hexes are settled at the M7 sign-off) |
| Tooltip | Under the age strip at the right edge (`command_card::TooltipPlace::BelowAgeStrip`; the colours and the tooltip are the command card's, which follow any entity with a `CardState`). Enabled: "Advance to Masonry Age: Costs 250 Grain 100 Lore". Greyed: the rejection's message, the tech count and the cost, "Not enough Grain. 2 / 2 techs researched. Costs 250 Grain 100 Lore" |
| Click | `AdvanceAge { player }` for the local player from a `Pointer<Click>` observer; a greyed click puts the tooltip on the message line, as a greyed card button does |
| Last age | The button is hidden (`Display::None`; the sim answers `FinalAge`), the readout stays (`the_button_is_hidden_in_the_final_age`) |

## Notices (M4b)

M4b (`economy_hud::notice_text`, done): besides rejections and "Scriptorium complete" (M3b), the message line names, for `MESSAGE_SECONDS` (4 s), the latest of these `SimEventMsg` events:

| Event | Notice |
| --- | --- |
| `ResearchCompleted` for the local player | "Letters I researched" |
| `AgeAdvanced` for the local player | "Masonry Age reached" |
| `BuildingDestroyed` of another player's building | "Watchtower destroyed" |
| `BuildingDestroyed` of the local player's building or site | "Muster Hall lost" |
| `TownAtZeroHp` of the local player's Town | "Town has fallen" |

The building's name comes from the event's own `owner` and `kind` (M4a), since the building is gone by the time the event is read; another player's research and age stay silent. `economy_hud::HUD_KEYS` lists every `hud` string key the game reads with the placeholders it fills, and `every_hud_key_the_game_reads_exists_with_whitelisted_placeholders` holds `en.ron` to it (each key exists, its value uses only those placeholders, all on the strings file's whitelist) and checks that every literal key in the game's sources is listed.

## Menus

| Menu | Entered by | Contents |
| --- | --- | --- |
| Main menu | app start, Quit to menu | New Game (difficulty Easy/Standard/Hard, seed, faction: Freeholders), Quit. The seed field defaults to a fresh value and accepts a typed number so a match can be replayed by hand. |
| Pause menu | Esc during a match | Resume, UI scale, volume, Borderless fullscreen toggle (`WindowMode::BorderlessFullscreen` with `Window::borderless_game`; exclusive fullscreen is never used), Quit to menu. The sim does not tick while this menu is open. |
| Game over | `Sim::outcome()` returns `Some` | Victory or Defeat, decisive or tiebreak, duration, units lost by kind, Towns annexed, ages reached, minutes spent at the Yield Cap, the replay's final hash and file name, Play Again (same settings, new seed), Main menu. |

States: `Menu`, `Skirmish`, `Paused`, `GameOver` in `app.rs`, arriving with the menus (M5b New Game, M6 pause and game over); at M2 the app starts straight into a skirmish (or the `--scenario`) and has no states. Pause through the menu and pause through Space are the same sim state; the menu adds the overlay.

The macOS application menu (M2, `macos_menu.rs`) is not a `bevy_ui` menu: it is a native muda menu whose only custom item is `Quit Eonmark` (id `quit`, Cmd+Q) routed to `AppExit::Success`; see [macos-packaging.md](macos-packaging.md).

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
| `PointerOverUi` | `hud::PointerOverUi(bool)`, derived in `PreUpdate` after `PickingSystems::Hover` from `bevy_picking::hover::HoverMap`: `true` when any entity hovered by `PointerId::Mouse` has a `Node`. `orders.rs` and `selection.rs` read it in `Update` and skip world input while it is set, so a press that starts on a panel never becomes a drag box or an order |

The M2 acceptance checks the last two rows with `--scenario hud_click` (`hud::HudClickCheck`): the game spawns 20 units, selects them, then writes synthetic `bevy_picking::pointer::PointerInput` messages (move, press, release) and the matching `ButtonInput<MouseButton>` presses, the same input the real mouse produces. A positive control comes first: a right click on open ground at the window centre must queue exactly one `Move` (with the ground-order path disabled the check fails with `a right click on open ground queued 0 Move(s), expected 1`, measured 2026-10-07), so the later assertions cannot pass vacuously. Then it clicks over the bottom bar and over the Stop button and asserts that no `Move` was queued and that the selection is unchanged, prints `hud_click: ok` and exits 0, or prints `hud_click: FAIL <why>` and exits 1 (`hud::verdict_line`). With `--screenshot` the verdict (and so the exit) waits until the PNG has landed, so the picture shows the HUD with the selection intact ([../screenshots/m2-hud.png](../screenshots/m2-hud.png)). `scripts/m2_checks.sh` runs it.

## Controls

Keys the design fixes are given as such. Keys marked "proposed" are not fixed by the design; M2 fixed S (Stop), A (attack-move) and Esc (clear selection, cancel attack-move) in `orders.rs` and `selection.rs`; the `dev` hotkey cheat-sheet generated from this table is a `good first issue`. Where an action reads more than one key, the row names the module.

| Action | Input | Milestone |
| --- | --- | --- |
| Select unit or building | left click | M2 |
| Add to or remove from selection | shift + left click | M2 |
| Box select | left drag on the ground | M2 |
| Select all of a kind on screen | double click a unit | M2 |
| Assign control group | ctrl + 1 to 9 | M2 |
| Recall control group | 1 to 9 | M2 |
| Context order (move, attack, gather, build site confirm, rally when a building is selected) | right click on ground or target. On an enemy unit, or an enemy building with hit points left, it is `Attack` for the armed selected units when `SimView::check_attack` accepts it (the attack marker, a ring with a cross, flashes on the target: at a unit's feet, or on a building's roof, since a marker at the footprint centre would be hidden inside the opaque box; `orders::target_point`); with no armed unit selected it falls through to an own site or gather building under the pointer, then to `Move`. Priority: enemy unit, enemy building, own site, own gather building, ground (`orders::contexts_of`, `orders::context_order`). The hovered entity is walked up its `ChildOf` ancestors to the unit or building it belongs to, so glTF child meshes resolve. An `InvalidTarget` rejection for a target that died before the command applied is not shown (`orders::IssuedAttacks`) | M2, M3b, M4b (attack) |
| Attack-move | A, or the Attack-move button the command card shows for a selection with armed units and no builders, then left click a point (`orders::OrderMode::AttackMove`; Esc or a right click cancels). The cursor is a crosshair while the mode is on (`orders::sync_attack_cursor`) and the order leaves the attack marker at the point. Armed units engage the enemies they meet on the way and then walk on; unarmed units just move (the sim applied it like `Move` until the M4a combat behaviour slice) | M2, M4a (combat), M4b (cursor, marker, button) |
| Stop | S (`orders.rs`; also the Stop button on the bottom bar) | M2 |
| Read an enemy's hit points | left click an enemy unit: it is selected, the panel shows "HP n / m" and its bar shows; orders, keys and the card name only the player's own units (`orders::own_selected_units`), so such a selection orders nothing. Another player's building is not selectable in M4b (`selection::on_building_click`; selecting one would first need `issue_rally` guarded and its queue hidden), so its hit points show only on its bar, once it is damaged | M4b |
| Set rally point | right click with a training building selected | M3b |
| Infinite queue toggle | button on the command card; key proposed Q | Not in M4b (owner decision 9, 2026-10-22, ROADMAP M4b Notes): help-wanted issue #42 (also needs-design), since it needs a repeat flag on the sim's training queue, a `Command` and a hash change |
| Select next idle Yeoman | period (.): the next idle builder by id, wrapping, and the camera centres on it (`selection::select_idle_builder`; idle as the sim's idle seeking reads it: no job, no order) | M3b |
| Pause | Space | M6 |
| Game speed x1 / x2 / x4 | proposed minus and equals to step down and up | M6 |
| Clear selection, cancel attack-move | Esc (`selection::clear_selection_on_escape`, `orders::order_mode_keys`) | M2 |
| Pause menu | Esc (the same key once the menu exists; it then also clears the selection) | M6 |
| Camera pan | W A S D, arrow keys, edge scroll, two-finger trackpad scroll (`MouseScrollUnit::Pixel`) (`camera.rs`; S and A are shared with Stop and attack-move, see Open questions) | M2 |
| Camera zoom | mouse wheel (`MouseScrollUnit::Line`), trackpad pinch (`PinchGesture`, Bevy `gestures` feature) | M2 |
| Camera jump | left click on the minimap | M7 |
| Order via minimap | right click on the minimap | M7 |

The macOS Cmd+Q menu item (`Quit Eonmark`, id `quit`) quits cleanly through Bevy's `AppExit`; see [macos-packaging.md](macos-packaging.md). Control groups: ctrl+1 to 9 assign, 1 to 9 recall (`selection::control_groups`, `Selection::groups`). Double-click selects every unit of the clicked kind whose projected position lies inside `Camera::logical_viewport_rect`. Edge-scroll margin and camera speed moving into `rules.ron` is a `good first issue`.

## Camera

Perspective camera, pitch about 55 degrees, yaw locked so the map never rotates. Zoom moves the camera along its view direction between 15 m and 60 m above the ground with smoothing toward a target distance. Pan moves the look-at point on the ground plane and clamps it to the map. Every speed is multiplied by `Time::delta_secs`, so travel distance is identical at 30 and 120 fps; the M2 acceptance records two replays with `--max-fps 30` and `--max-fps 120` and compares hashes, and the owner checks camera travel by eye. Edge scroll is active only when the window has focus and the pointer is inside it.

Rendering of units between ticks: `present.rs` keeps the previous and current sim positions per `UnitId` and lerps with `Time<Fixed>::overstep_fraction()`. The camera never reads sim state directly.

## HiDPI and fonts

The bundle declares `NSHighResolutionCapable` (see [macos-packaging.md](macos-packaging.md)). Bevy's `WindowResolution` keeps logical and physical size apart, so a 1280 x 800 window renders at 2560 x 1600 on a Retina display. `UiScale` multiplies `Val::Px` only, so every HUD dimension is authored in `Val::Px` and the UI scale setting is a single persisted multiplier. Percent and viewport units are avoided in the HUD for this reason.

Text uses the Inter variable font through a variable `TextFont`; the OFL license text ships beside the font file. Panels use Kenney UI Pack 9-slice images through `NodeImageMode::Sliced`. The palette is matte and judged on both a P3 display and an sRGB screenshot, because wgpu 29 can oversaturate on wide-gamut surfaces (see [../ART_STYLE.md](../ART_STYLE.md)).

## Health bars, rings and markers

Health bars and selection rings are separate flat entities keyed by `UnitId`, never children of moving unit entities. This keeps unit transforms flat, lets bars be billboarded and scaled by distance independently, and avoids re-parenting when a unit dies. Selection rings and move markers are retained gizmos; `ClusteredDecal` is unavailable on Metal and is not used.

M2 (`selection.rs`, done): two `GizmoAsset` circles are built once in `create_ring_assets` (`RingAssets { selection, marker }`, radius `RING_RADIUS_PER_UNIT_RADIUS` = 1.4 times the unit's collision radius, laid flat with `present::flat_on_ground()`, colours `palette::SELECTION_RING` and `palette::MOVE_MARKER`). `sync_selection_rings` keeps one `(SelectionRing(UnitId), Gizmo, Transform)` entity per selected unit and moves it to the unit's interpolated position each frame; `spawn_move_marker` places a `MoveMarker { expires_at }` ring at the order point and `expire_move_markers` despawns it. Units themselves are one shared mesh and one material per `(kind, team)` (`present::UnitVisuals`), tinted with `palette::TEAM[slot]`. The attrition indicator and supply radius ring (M5a) and the annexation timer (M5a) are drawn the same way. Bars follow the owner's fog visibility (see [fog.md](fog.md)).

Health bars appear in M4b together with distinct primitive silhouettes per unit kind; from M7 the primitives are replaced by glTF through `data/visuals.ron` without touching the HUD.

M4b (`health_bars.rs`, done): a bar is shown only while its target is damaged or selected (owner decision 6, ROADMAP M4b Notes), for units and buildings of both players. It is two flat quads, a neutral track in `palette::BAR_BACK` (#3A3A3F) and a fill in the owner's team colour (`palette::TEAM`) as wide as the hit points left; there is no green-to-red ramp, because team red is the invalid hue. At zero hit points (a Town held at 0) the fill is hidden, never scaled to nothing.

- **Entities.** `HealthBars` maps each `sim::TargetId` (a unit's is `TargetId::Unit(UnitId)`) to its two top-level entities, `HealthBar { target, part: Back | Fill }` with `Mesh3d`, `MeshMaterial3d`, `Transform`, `NotShadowCaster` and `NotShadowReceiver`: no `Pickable` (picking ignores them under `require_markers`) and no `ChildOf`.
- **Lazy.** `sync_bar_targets` (`Update`) runs only when the sim tick advanced or the selection changed. It spawns a pair for each newly damaged or selected target, despawns the pairs of targets that died or are neither, and refreshes the values. A unit's maximum comes from `SimView::unit_stats` once per `(owner, kind)` (`MaxHpTable`, so Arms raises it; the test `max_hp_table_matches_unit_hp` holds it to `SimView::unit_hp`) and its hit points are that maximum less `damage_taken`; a building's are `SimView::building_hp`.
- **Placement.** `place_bars` (`PostUpdate`, before `TransformSystems::Propagate`) billboards each pair with the camera's rotation, `0.9 x 0.12 m` times `clamp(zoom / 40, 0.75, 1.5)`: a unit's bar 0.3 m above its look (`UnitVisuals::top_y`, the art pass changes only `present::look_top_y`) at the interpolated position its mesh uses, a building's 0.4 m above its box (a site's rising box included). The fill sits 0.01 m toward the camera. Writes go through `set_if_neq`.
- **Batches.** Every bar is the one left-anchored `Rectangle` quad (origin on its left edge, so the fill shortens from the right) with one of three unlit, two-sided, fog-free `StandardMaterial`s (track, blue fill, red fill): at most three batches for any number of bars, and no shadow passes (`bars_use_one_quad_and_three_materials`).
- **Hierarchy check (dev only).** With `--features dev`, `BarHierarchyCheckPlugin` looks every frame (in `Last`, before the replay recorder finishes) for a `HealthBar` or `SelectionRing` entity with a `ChildOf`. The first one logs an `error!` with the entity ids and writes `AppExit::from_code(1)`, so the replay still gets its trailer; on any exit it prints `hierarchy_check: ok frames=<n> bar_targets_max=<m> rings_max=<r>` (or `FAIL violations=<k> ...`). The check runs after `bevy::window::ExitSystems`, like the recorder and `frame_stats`' exit lines, so the red close button prints it too (`hierarchy_check_reports_when_the_window_closes`), and before the `frame_stats:` lines, so a failing frame still prints those. `bar_targets_max` counts targets, not entities. Unit entities may have children (glTF scenes at the art pass); bars and rings may not have a parent. The test `hierarchy_check_flags_a_parented_bar` is its positive control. The dev Sim panel shows the bar targets and entities.

Expected draw batches at 200 vs 200 (M4b, the baseline for the art pass's dev-panel counter). Bevy 0.19.1 keeps its batch counts `pub(crate)` and wgpu has no draw counters, so there is no counter yet (the same gap as the ROADMAP M3b line); these are the counts the layout allows, to be checked with an Xcode Metal frame capture on the Mac:

| What | Batches | Why |
| --- | --- | --- |
| Units | at most 48 | 8 kinds x 2 teams = 16 mesh and material bins, each in the main pass and the 2 shadow cascades |
| Health bars | at most 3 | one quad, three unlit materials, no shadow passes, however many bars |
| Buildings | about 12 | about 4 materials (two teams, site scaffold) x 3 passes |
| Ground | about 2 | the `FieldExt` ground and its grid |
| `bevy_ui` | 10 to 40 | bars, card buttons, texts |
| Total | about 75 to 100 | under the 500 budget |
| Selection rings | 1 each | retained gizmos are drawn one by one: all 200 own units selected add about 200, still under 500 |

## Developer-only panels

Behind the `dev` cargo feature only: `bevy/bevy_dev_tools`, `bevy_egui` 0.40.1 and `bevy-inspector-egui` 0.37.0. They provide the FPS overlay, the frame-time graph, the `Sim::hash()` and sub-hash readout, the draw-call counter with its 500 budget, a world inspector (hidden until F12: listing every entity each frame costs several milliseconds with 200 units and 200 rings on screen, and it covers the map), and tuning panels. At M2 the Sim panel also shows `sim_driver::DriverStats` (ticks this and last frame, stalled and dropped ticks, commands recorded), the selection size, pending commands and the mesh and material counts. `--scenario units200` is the 200-unit stress case (`--stress N` was never built). `cargo clippy -p game --features dev` runs in CI so the feature does not rot, and `bundle.sh` fails if `bevy_dylib` is linked, so no dev build ships. No gameplay UI may depend on egui.

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

- [x] In-game: place a Croft outside the border and the ghost is red and the HUD says 'Outside your borders'; inside it turns green, Yeomen walk over and build it; founding a second Town visibly expands the tinted territory and border line; the Grain readout turns amber once the cap is reached; at 25/25 pop the Train button is disabled with a tooltip and the sim rejects with `PopCapReached` (`--scenario economy_walkthrough` replay verifies and a ci_testing screenshot shows the amber readout).
- [ ] `cargo run --release -p game -- --scenario economy_walkthrough` holds >= 60 fps on the dev Mac with the overlay (and with FieldExt if landed); dev-panel draw calls < 500.

M4b:

- [x] In-game: build a Scriptorium, research 2 techs, advance to Masonry Age, build an Engine Yard, train Mangonels and a mixed army, destroy an enemy Watchtower placed via `--scenario enemy_outpost`; the auto-recorded replay verifies (the same sequence exists as a scripted replay for the owner proxy).
- [ ] `cargo run --release -p game -- --scenario enemy_outpost` with 200 vs 200 fighting holds >= 60 fps; health bars are not children of unit entities (asserted by a dev-only hierarchy check).

Line 1 was observed on Linux (ROADMAP M4b, plan decision 21, the M3b precedent): the `enemy_outpost_walkthrough` golden and its windowed run, and `scripts/m4b_outpost_drive.sh` playing the sequence on `--scenario enemy_outpost` with real X input, its replay verifying ([../PLAYTEST.md](../PLAYTEST.md#automated-proxies-m4b)). Line 2 needs the dev Mac (owner decisions 2 and 3); the [owner's Mac checklist](../PLAYTEST.md#owners-mac-checklist-m4b) has the commands. Its Linux proxy under lavapipe, which renders in software at about 6 fps and so can neither pass nor fail the line: the dev build's background run of `--scenario enemy_outpost` printed `hierarchy_check: ok frames=1592 bar_targets_max=37 rings_max=0` and `frame_stats: fight frames=134 seconds=22.29 mean=166.34 p95=196.50 max=239.82 fps=6.0 ticks=151..576 on_screen_max=400 on_screen_min=400/400 units_at_first_hit=200/200` on 2026-10-22, at the S5 review-fix head (`on_screen_min=400/400`: every armed unit alive on screen in every timed frame). The sim's share of the frame is the M4a combat bench, `cargo run -p sim-cli --release -- bench --units 400 --ticks 1200 --combat`: a step mean of 0.51 to 0.56 ms and about 1.1 ms per fighting tick on the same VM, under the 5 ms budget ([../BUILD_TIMES.md](../BUILD_TIMES.md) has the rows).

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
cargo run -p game --features dev -- --scenario enemy_outpost
cargo run --release -p game -- --scenario enemy_outpost
scripts/m4b_outpost_drive.sh target/ci/eonmark target/release/sim-cli <out-dir>   # Linux, Xvfb + lavapipe
```

## Open questions

- Key overlap between camera pan and unit orders: M2 fixed S = Stop and A = attack-move while `camera.rs` also pans on W/A/S/D, so with units selected S both pans the camera down and stops them, and A pans left and arms attack-move. Options: pan on arrow keys and edge scroll only, move Stop and attack-move to other keys, or make the order keys win when a selection exists. The owner kept the overlap for M4b on 2026-10-22 (ROADMAP M4b Notes, decision 4) and revisits it at the controls playtest. Keys for infinite queue, idle Yeoman and the speed steps remain proposed.
- Whether the minimap draws the camera's view rectangle. Not in the design; cheap to add in M7 if the owner wants it.
- Whether game speed x1/x2/x4 ships in release builds or stays `dev`-only. Listed as an owner decision in [../ROADMAP.md](../ROADMAP.md); this document assumes release.
- Tooltip delay and whether tooltips also appear on enabled buttons (cost breakdown). M3b decides; `good first issue` for hover states exists.
- Whether the selection panel shows a unit's attrition state as text in addition to the indicator on the unit (M5a).
