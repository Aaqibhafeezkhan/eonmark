# HUD, menus, picking and controls

This document specifies the shipped user interface of Eonmark: the `bevy_ui` HUD and its panels, the three menus, the picking rules that keep HUD clicks and world clicks apart, the control scheme, the camera, HiDPI handling, how health bars and selection rings are drawn, and the developer-only egui panels. Shipped builds use `bevy_ui` only; `bevy_egui` and `bevy-inspector-egui` exist behind the `dev` feature and never reach a release. Everything here lives in `crates/game` (`hud.rs`, `selection.rs`, `orders.rs`, `camera.rs`, `present.rs`, `macos_menu.rs`, `dev_tools.rs`, and since M5b `menu/` with the main menu and the app's states). The M2 state is marked inline: the HUD is a skeleton (two bars and one button), selection and orders are complete, menus and states do not exist yet; M5b adds the minimal main menu and the `Menu` and `Skirmish` states ([Main menu (M5b)](#main-menu-m5b)).

Area: `area:ui` and `area:game`. Milestones: M2 (selection, orders, camera, HUD skeleton), M3b (command card, resource bar, build ghost), M4b (research panel, age indicator, health bars), M5a (attrition indicator, supply ring, annexation timer, the outcome overlay), M5b (the minimal main menu with New Game, `--spectate`), M6 (speed, pause, New Game selector, game-over stats), M7 (minimap, menus, fonts, panels). See [../ROADMAP.md](../ROADMAP.md).

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
| Above the command card | Message line | The last rejection or notice for a few seconds: "Outside your borders", "Not enough Ore", "Population cap reached", "Town annexed". Since M4b also "Letters I researched", "Masonry Age reached", "Watchtower destroyed" and "Muster Hall lost", and since M5a the annexation notices, "Town is being annexed", "Annexation stopped", "Town annexed" and "Town was lost" (see Notices below). |
| Above the message line | Annexation readout (M5a) | Each running annexation timer the local player is part of, with the time left until the flip: "Annexing Town: 0:42" or "Town is being annexed: 0:42" (`markers::annex_readout_text`, the `annex.attacking` and `annex.defending` strings; the seconds are rounded up, so it reads 0:01 in the last step; `markers_follow_exposure_selection_and_the_timer` reads it for the attacker, the defender and a bystander, and before any timer runs). It sits 24 px above the message line because M4b's top bar has no room left; empty when no timer runs. |

Costs and names on buttons come from `data/rules/*.ron` and `data/rules/strings/en.ron`; the HUD never hard-codes a number.

The infinite-queue toggle on training buildings repeats the last trained kind while resources allow (not built yet: it needs a repeat flag on the sim's training queue, a `Command` and a hash change; issue #42, help wanted and needs-design). The idle-Yeoman hotkey selects the next idle Yeoman and centres the camera on it.

## Research panel (M4b)

M4b (`command_card.rs`, done): a selected own, complete Scriptorium (the `host` of every Scriptorium tech in `techs.ron`) shows its Train Scribe button and then one Research button per tech line, in `techs.ron` order: Arms, Statecraft, Trade, Letters. Each shows the line's next level, the lowest the player has not researched; a level being researched keeps its button. Once a line's three levels are done, its top level stays on the card, greyed, as "Researched". This is owner decision 8 (ROADMAP M4b Notes): the next level of each line on the card, the blocker on the button, prerequisites in the tooltip, not a full 12-tech grid. Four Research buttons and Train Scribe fit the 4 x 2 card. Card buttons are 140 x 36 px (112 px wide before M4b), so every line a button can show fits one line of it in the card's 12 px font: the longest are "Needs Statecraft II" (19 characters of 7.2 px; the card never shows a "Needs" line, but the code can write one) and the Bowman's cost at the military ramp's cap, "45 Grain 90 Lumber" (`every_card_line_fits_its_button`). At 112 px, Arms II's "100 Ore 150 Lore" and the Bowman's "20 Grain 40 Lumber" wrapped onto a third line that spilled out of the button.

| Part | Contents |
| --- | --- |
| Which buttons | `command_card::research_actions`: for each line the building's kind hosts, the lowest level not in `SimView::researched`, or the top level when all are. Since M5a's rules data a Watchtower hosts the Harrying line (two levels), so a selected own, complete Watchtower shows one Research button, Harrying I greyed as "Masonry Age" in the Hearth Age, then Harrying II greyed as "Charter Age" (`watchtower_card_offers_harrying_from_masonry`); nothing in the game names the line or assumes three levels |
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
| Button | One line, 24 px tall, the card's font: "Advance: 1 / 2 techs" while the tech count is what the sim rejects (`NotEnoughTechs`; the count is `SimView::age_tech_count`, the techs that count toward an age, so since M5a the Harrying side techs never raise it, `advance_tooltip_counts_only_age_techs`), else the next age and its cost (`Rules::amounts` of the next age's cost in `ages.ron`), "Advance to Masonry Age: 250 Grain 100 Lore". So when only a resource is short the greyed button shows the cost, as a greyed Train or Research button does, and its tooltip names the resource. The longest label, "Advance to Charter Age: 500 Grain 300 Lumber 300 Lore", makes the strip about 514 px wide in the Masonry Age |
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

M5a adds the annexation events, for a Town the local player owns or annexes (a flip between two other players says nothing):

| Event | Notice |
| --- | --- |
| `AnnexationStarted` at the local player's Town, or by the local player | "Town is being annexed" |
| `AnnexationReset` at the local player's Town, or of a timer the local player was running | "Annexation stopped" |
| `TownAnnexed` to the local player | "Town annexed" (the text this table's message line row has named since M2) |
| `TownAnnexed` from the local player | "Town was lost" |

A reset names only the Town, so the message line remembers the attacker of each timer it heard start (`MessageState::annexing`) to tell the attacker too (`annexation_notices_name_the_owner_and_the_attacker`). `MatchDecided` gets no notice: the outcome overlay shows it.

The building's name comes from the event's own `owner` and `kind` (M4a), since the building is gone by the time the event is read; another player's research and age stay silent. `economy_hud::HUD_KEYS` lists every `hud` string key the game reads with the placeholders it fills, and `every_hud_key_the_game_reads_exists_with_whitelisted_placeholders` holds `en.ron` to it (each key exists, its value uses only those placeholders, all on the strings file's whitelist) and checks that every literal key in the game's sources is listed.

## Outcome overlay (M5a)

M5a (`outcome.rs` and `sim_driver.rs`, done): the minimal end of a match; the full game-over screen is M6 (Menus, below).

| Part | Behaviour |
| --- | --- |
| Deciding step | The sim emits `MatchDecided` once, in the step that decides the match (`victory::step`, 6e). `sim_driver::step_sim` then, in that same fixed step: finishes the replay recorder (`finish_recorder`: a final hash record at the deciding tick, the trailer, the writer joined), so the replay ends at the outcome and a `kill -9` after the overlay still leaves a verified file; prints `outcome: victory player=0 decisive=true reason=seat_captured tick=1628 hash=<capture_seat.hash>` on stdout (capture_seat's line; `<capture_seat.hash>` stands for the hash in `crates/sim/tests/fixtures/capture_seat.hash`, which moves with every `SIM_VERSION` or fixture bump) (`outcome_line`; `outcome: draw reason=... tick=... hash=...` for a draw); and inserts `MatchOver { result, hash, replay }` |
| Freeze | From then on `drive_tick` returns `TickOutcome::Decided` for a live source (local input or a scenario's stream merged with it) without asking the source or stepping: the live game freezes at the outcome, whatever the player queues. A replay never freezes (`--headless-run` plays it to its recorded end; `replays_never_freeze_at_an_outcome`). `Time<Virtual>` is not paused, because the camera reads it; instead `present::settle_at_outcome` sets every unit's `Interp` to its final pose once, after the deciding step, so units, bars and rings stand still instead of swinging between their last two poses as the overstep keeps cycling |
| Overlay | A full-window node (all four edges at 0 px), hidden until `MatchOver` appears, on the `palette::OUTCOME_SCRIM` slate at 80 % alpha at `GlobalZIndex(50)`, with five centred lines (`overlay_texts`): "Victory", "Defeat" or "Draw" for the local player (32 px); the reason, "The Seat was captured", "Every Town was lost", "Surrender" or "Territory at the time limit: 1248 tiles to 1245" (the local player's tiles first); "1:21, tick 1628"; "Final hash <capture_seat.hash>"; "Replay 20261023-041314-31.eonreplay". The hash is the one `sim-cli verify` prints for that replay |
| Input | The overlay keeps the `bevy_ui` picking default, so it blocks clicks on the world, the panels and the card under it; `hud::WorldInputSet` (drag box, orders, placement, keys) and the command card's systems stop with `run_if(not(resource_exists::<MatchOver>))`, and an order mode is left. `show_outcome` also drops a drag box held at the outcome and hides the placement ghost, since the systems that would do it are in the stopped set; the move markers' expiry and the drag-box node (`selection::add_world_input_feedback`) run after the set, not in it, so a marker placed just before the outcome still shrinks and goes (`the_overlay_shows_once_the_match_is_over_and_input_stops`). The camera still pans and zooms |
| Framing | `--scenario capture_seat` opens at 40 m over (100, 64), the Seat's ring seen from the west with the Mangonels (`app::default_framing`) |

The acceptance run: `just m5a-checks` (`scripts/m5a_checks.sh`, background mode) verifies the golden, runs `--scenario capture_seat` with a screenshot at the deciding tick, and requires the `outcome:` line's hash to equal both `capture_seat.hash` and `sim-cli verify` of the game's own replay ([../PLAYTEST.md](../PLAYTEST.md#automated-proxies-m5a), [../screenshots/m5a-victory-overlay.png](../screenshots/m5a-victory-overlay.png)).

## Menus

| Menu | Entered by | Contents |
| --- | --- | --- |
| Main menu | app start, Quit to menu | New Game (difficulty Easy/Standard/Hard, seed, faction: Freeholders), Quit. The seed field defaults to a fresh value and accepts a typed number so a match can be replayed by hand. |
| Pause menu | Esc during a match | Resume, UI scale, volume, Borderless fullscreen toggle (`WindowMode::BorderlessFullscreen` with `Window::borderless_game`; exclusive fullscreen is never used), Quit to menu. The sim does not tick while this menu is open. |
| Game over | `Sim::outcome()` returns `Some` | Victory or Defeat, decisive or tiebreak, duration, units lost by kind, Towns annexed, ages reached, minutes spent at the Yield Cap, the replay's final hash and file name, Play Again (same settings, new seed), Main menu. M6; M5a ships the minimal overlay below, without the statistics and buttons. |

States: `Menu`, `Skirmish`, `Paused`, `GameOver`, arriving with the menus (M5b New Game, M6 pause and game over); at M2 the app started straight into a skirmish (or the `--scenario`) and had no states. M5a marks the end of a match with the `sim_driver::MatchOver` resource instead of a state. M5b adds `menu::AppState` with `Menu` and `Skirmish` only (`crates/game/src/menu/mod.rs`, below); `Paused` and `GameOver` come with M6's menus. Pause through the menu and pause through Space are the same sim state; the menu adds the overlay.

### Main menu (M5b)

M5b ships a subset of the main menu row above (M5b owner decision 5): no difficulty or seed selector, no faction choice, no Play Again (M6), full menus in M7.

| Part | Behaviour |
| --- | --- |
| States | `#[derive(States)] enum AppState { Menu, Skirmish }` in `menu/mod.rs`. `app::run` inserts the initial state explicitly (`menu::initial_state`): `Menu` for a plain launch, `Skirmish` for `--scenario`, `--skip-menu` and `--spectate`. `--headless-run` (`MinimalPlugins`) has no states, and the run conditions below treat a missing state as a running match, so headless runs and the tests without states behave as before |
| The match | Built at launch, before the menu (`app::build_match`): a plain launch and `--skip-menu` play `MatchSetup::vs_bot` (the local player in slot 0, the Standard bot with personality `standard` in slot 1, `plains_1v1`, the standard start). The seed is `--seed` when given (`Cli::seed_explicit`), else fresh from the wall clock (`app::fresh_seed`, the low 32 bits of the nanoseconds since the epoch; M5b owner decision 6); it is shown on the menu and is the replay name's last part (`<yyyymmdd-hhmmss>-<seed>.eonreplay`). A scenario keeps its own seed unless `--seed` is given. New Game only switches the state; M6's Play Again will rebuild the match on entering `Skirmish` instead |
| Panel | `menu::main_menu::spawn_main_menu` on `OnEnter(Menu)`: a full-window column (all four edges at 0 px) on `palette::OUTCOME_SCRIM` at `GlobalZIndex(60)`, above the HUD and the outcome overlay's 50, with `DespawnOnExit(AppState::Menu)`. Three lines from `en.ron`: "Eonmark" (`menu.title`, 32 px), "Skirmish against the Standard bot on plains_1v1" (`menu.opponent`, the map's name from the setup), "Seed 2707532408" (`menu.seed`, the new `{seed}` placeholder); then New Game and Quit, each `command_card::CARD_BUTTON_PX` (140 x 36 px) like a card button. The panel keeps the `bevy_ui` picking default, so it blocks the world and the HUD under it ([../screenshots/m5b-menu.png](../screenshots/m5b-menu.png)) |
| Clicks | `Pointer<Click>` observers, as the Stop button's: New Game sets `NextState(AppState::Skirmish)`, Quit writes `AppExit::Success`. `Interaction` only drives the hover and pressed colours (`menu_button_colors`). On entering `Skirmish` the game prints `match: new game vs standard personality=standard seed=<s> map=plains_1v1` (`menu::match_line`; `match: spectate ...` under `--spectate`, nothing for a scenario) |
| The sim waits | `sim_driver::step_sim` runs only while `sim_driver::match_running` (no `State<AppState>`, or `Skirmish`). While the menu is open the fixed loop still spends its time with the system skipped, so the frame after New Game steps one tick, not a burst for the time spent on the menu (`menu::tests::new_game_click_enters_skirmish_and_the_sim_starts_stepping`) |
| Lazy recorder | `SimPlugin` keeps the replay's path in `sim_driver::PendingRecording`; `step_sim` spawns the `Recorder` and prints `replay: <path>` right before the first tick it steps. A quit from the menu writes no replay at all (`the_sim_does_not_step_and_no_replay_is_written_while_the_menu_is_open`); every scenario and `--skip-menu` run still prints `replay:` once its first tick steps; after `--spectate --fast-forward-to N` the first record is past tick N |
| Input | `hud::WorldInputSet` (drag box, selection keys, orders, placement, keys) also stops while the menu is open (`match_running`) and while the local slot is a bot (`sim_driver::local_player_is_human`, read from the `sim_driver::LocalSeat` resource); the command card's systems stop with the latter, and the card's, the Advance button's and the Stop button's click observers do nothing then (`hud::tests::a_spectator_has_no_world_input_card_or_advance`). Clicking a unit or building selects it through observers outside the set, so a spectator can still select a unit to read it, but nothing orders it. As defence in depth `LocalInput`, and `ScenarioInput`'s merge of it, drop any queued local command for an AI slot (`PendingCommands::for_setup`, `dropped_for_ai`; `local_commands_for_an_ai_slot_are_dropped`, and through the real `SimPlugin` `sim_plugin_drops_local_commands_for_an_ai_slot`) |
| `--skip-menu` | Starts the vs-bot match at once; `scripts/m2_checks.sh` checks 3 and 4 pass it, since a plain launch would sit on the menu |
| `--spectate` | A CLI aid for watching bot games and streams, not a menu entry: both slots are the Standard bot (`MatchSetup::bots`, `m5_first_match`'s setup with `--seed 1`, a fresh seed without it), no orders can be given (a click still selects a unit), the camera still moves, and the outcome overlay reads from P0's side (a spectator title is M6). `--fast-forward-to <tick>` (only with `--spectate`) steps the sim synchronously in `app::build_match`, draining the events after every step, before the window opens, and prints `fast_forward: tick=<n> wall_s=<s>`; it is refused ("outcome at tick T before --fast-forward-to N", exit 2) when the match is decided on the way, because the overlay needs a live deciding step. The replay the lazy recorder starts at tick N still verifies, since a spectated match has no human input (M5b owner decision 8) |
| New Game proxy | `--new-game-after-seconds <s>` (`main_menu::click_new_game_after`): after `s` seconds the synthetic mouse of the `hud_click` check (`hud::SyntheticMouse`) moves to the centre of the real New Game button, presses and releases there through the picking pipeline, and prints `new-game: clicked at (x, y) after <t> s`; the real cursor is put back each frame (`hud::RealCursor`). Refused with `--skip-menu`, `--spectate` or `--scenario` |

The acceptance proxy: `just m5b-checks` (`scripts/m5b_checks.sh`, background mode) verifies the `m5_first_match` golden, runs `--spectate --seed 1 --fast-forward-to <T-400>` with a screenshot of the overlay at the deciding tick T and requires the `outcome:` line's hash to equal both `m5_first_match.hash` and `sim-cli verify` of the game's own replay ([../screenshots/m5b-overlay.png](../screenshots/m5b-overlay.png)), screenshots the menu and checks it recorded nothing, clicks New Game through the proxy and verifies that replay with the Standard bot rebuilt from its header, and runs `scripts/m2_checks.sh` checks 3 and 4 ([../PLAYTEST.md](../PLAYTEST.md#automated-proxies-m5b)).

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

M2 (`selection.rs`, done): two `GizmoAsset` circles are built once in `create_ring_assets` (`RingAssets { selection, marker }`, radius `RING_RADIUS_PER_UNIT_RADIUS` = 1.4 times the unit's collision radius, laid flat with `present::flat_on_ground()`, colours `palette::SELECTION_RING` and `palette::MOVE_MARKER`). `sync_selection_rings` keeps one `(SelectionRing(UnitId), Gizmo, Transform)` entity per selected unit and moves it to the unit's interpolated position each frame; `spawn_move_marker` places a `MoveMarker { expires_at }` ring at the order point and `expire_move_markers` despawns it. Units themselves are one shared mesh and one material per `(kind, team)` (`present::UnitVisuals`), tinted with `palette::TEAM[slot]`. The attrition indicator and supply radius ring (M5a) and the annexation timer (M5a) are drawn the same way (below). Bars follow the owner's fog visibility (see [fog.md](fog.md)).

M5a (`markers.rs`, done), all flat on the ground at `RING_Y`, steady (nothing pulses), no new hue:

| Marker | Look and rule |
| --- | --- |
| Attrition indicator | A diamond (a gizmo circle of 4 segments) in `palette::ATTRITION`, the Yield Cap amber #D29B3B, under every unit with `exposed_ticks > 0`, the sim's own bleeding state, which stays on between pulses. Its half-diagonal is 1.8 unit radii, outside the 1.4 of the selection ring: at the plan's 0.8 the diamond would lie inside the unit's own footprint, under its body. One retained `Gizmo` entity per `UnitId`, never a child, placed at the unit's interpolated pose every frame (`sync_attrition_markers`) and despawned when the exposure ends or the unit dies. [../screenshots/m5a-attrition.png](../screenshots/m5a-attrition.png) |
| Supply ring | A Timber circle (`palette::SUPPLY_RING`, #8B6B4A, the Wain's role colour) of radius `supply_radius_tiles + 0.5` = 14.5 m, the outer edge of the tiles step 5b' counts as covered, around each selected unit of a kind that `supplies`, centred on the tile it stands on (the sim's disc is tile to tile, so the ring steps a tile at a time with the Wain). One retained gizmo per `UnitId` (`sync_supply_rings`). The supply hatching in the field texture's A channel waits for M7 with fog (M5a owner decision 10) |
| Annexation timer | For each Town under a running timer (`SimView::annexation`), an immediate-mode track circle in `palette::ANNEX_TRACK` (#3A3A3F at 80 %) of radius `max(w, h) / 2 + 2` m (3.5 m for a Town, just outside the units on the tiles around it) and the attacker's team-coloured arc over it, filled clockwise from north by `(ticks + overstep) / total`; both 3 px wide in their own gizmo group (`AnnexGizmos`), since the default group draws the 1 px grid. [../screenshots/m5a-markers.png](../screenshots/m5a-markers.png) shows it with the supply ring and the readout |

Health bars appear in M4b together with distinct primitive silhouettes per unit kind; from M7 the primitives are replaced by glTF through `data/visuals.ron` without touching the HUD.

M4b (`health_bars.rs`, done): a bar is shown only while its target is damaged or selected (owner decision 6, ROADMAP M4b Notes), for units and buildings of both players. It is two flat quads, a neutral track in `palette::BAR_BACK` (#3A3A3F) and a fill in the owner's team colour (`palette::TEAM`) as wide as the hit points left; there is no green-to-red ramp, because team red is the invalid hue. At zero hit points (a Town held at 0) the fill is hidden, never scaled to nothing.

- **Entities.** `HealthBars` maps each `sim::TargetId` (a unit's is `TargetId::Unit(UnitId)`) to its two top-level entities, `HealthBar { target, part: Back | Fill }` with `Mesh3d`, `MeshMaterial3d`, `Transform`, `NotShadowCaster` and `NotShadowReceiver`: no `Pickable` (picking ignores them under `require_markers`) and no `ChildOf`.
- **Lazy.** `sync_bar_targets` (`Update`) runs only when the sim tick advanced or the selection changed. It spawns a pair for each newly damaged or selected target, despawns the pairs of targets that died or are neither, and refreshes the values. A building that changes hands in an annexation (M5a) keeps its pair, and its fill takes the new owner's material (`an_annexed_building_s_fill_takes_the_new_owner_s_colour`). A unit's maximum comes from `SimView::unit_stats` once per `(owner, kind)` (`MaxHpTable`, so Arms raises it; the test `max_hp_table_matches_unit_hp` holds it to `SimView::unit_hp`) and its hit points are that maximum less `damage_taken`; a building's are `SimView::building_hp`.
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

Behind the `dev` cargo feature only: `bevy/bevy_dev_tools`, `bevy_egui` 0.40.1 and `bevy-inspector-egui` 0.37.0. They provide the FPS overlay, the frame-time graph, the `Sim::hash()` and sub-hash readout, the draw-call counter with its 500 budget, a world inspector (hidden until F12: listing every entity each frame costs several milliseconds with 200 units and 200 rings on screen, and it covers the map), and tuning panels. At M2 the Sim panel also shows `sim_driver::DriverStats` (ticks this and last frame, stalled and dropped ticks, commands recorded), the selection size, pending commands and the mesh and material counts. `--scenario units200` is the 200-unit stress case (`--stress N` was never built). Since M5b the Sim panel has one line per bot seat, `bot P1 standard` followed by `ai::describe` (the build-order window, the `Military` latch, the wave's phase and size, the next wave tick, the waves launched and the defend latch); the recording row reads "from the first tick" until the lazy recorder starts. `cargo clippy -p game --features dev` runs in CI so the feature does not rot, and `bundle.sh` fails if `bevy_dylib` is linked, so no dev build ships. No gameplay UI may depend on egui.

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

M5a:

- [x] `cargo run -p game --features dev -- --scenario capture_seat` shows the victory overlay with the final hash and the recorded replay verifies to it.

  Observed on PR #43, 2026-10-23, on Linux under Xvfb with lavapipe (the M3b and M4b precedent): the command with background mode and a deadline appended exits 0 with `outcome: victory player=0 decisive=true reason=seat_captured tick=1628 hash=0x2c9a9ef52a5184d0`, its screenshot shows the overlay with that hash, and `sim-cli verify` of its replay prints `OK final_hash=0x2c9a9ef52a5184d0 ticks=1628`; `just m5a-checks` passes (docs/ROADMAP.md, M5a line 3). The owner's run on the Mac and the [Controls checklist (M5a)](../PLAYTEST.md#controls-checklist-m5a) rows are open.

M5b:

- [ ] **[owner]** From the window: New Game starts a match vs the Standard bot on plains_1v1; the owner plays to a victory or defeat overlay in one sitting; the auto-recorded replay verifies to the hash shown on the overlay. Automated proxy: `m5_first_match.eonreplay` verifies in CI and a `ci_testing` screenshot of the overlay with its final hash is attached to the PR.

  The proxy, with M5b owner decision 8's substitutions (`--screenshot-at` instead of `ci_testing`, the replay recorded from the fast-forward tick), is `just m5b-checks`; the owner's run is the [Owner's Mac checklist (M5b)](../PLAYTEST.md#owners-mac-checklist-m5b).

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
cargo run -p game --features dev -- --scenario capture_seat
just m5a-checks   # also under lavapipe: the default 150 s is enough
cargo run --release -p game                                   # M5b: the main menu, then New Game
cargo run --release -p game -- --spectate                     # the Standard bot against itself
cargo run --release -p game -- --spectate --seed 1 --fast-forward-to 25612   # m5_first_match's last 400 ticks
just m5b-checks   # the M5b proxy: overlay, menu, New Game click, m2_checks 3 and 4
```

## Open questions

- Key overlap between camera pan and unit orders: M2 fixed S = Stop and A = attack-move while `camera.rs` also pans on W/A/S/D, so with units selected S both pans the camera down and stops them, and A pans left and arms attack-move. Options: pan on arrow keys and edge scroll only, move Stop and attack-move to other keys, or make the order keys win when a selection exists. The owner kept the overlap for M4b on 2026-10-22 (ROADMAP M4b Notes, decision 4) and revisits it at the controls playtest. Keys for infinite queue, idle Yeoman and the speed steps remain proposed.
- Whether the minimap draws the camera's view rectangle. Not in the design; cheap to add in M7 if the owner wants it.
- Whether game speed x1/x2/x4 ships in release builds or stays `dev`-only. Listed as an owner decision in [../ROADMAP.md](../ROADMAP.md); this document assumes release.
- Tooltip delay and whether tooltips also appear on enabled buttons (cost breakdown). M3b decides; `good first issue` for hover states exists.
- Whether the selection panel shows a unit's attrition state as text in addition to the indicator on the unit. M5a ships the indicator only (`SimView::attrition` has the pulse, its interval and the owner for such a line); the text waits for the owner's playtest.
