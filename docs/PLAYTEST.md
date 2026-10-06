# Playtest

This document holds two things: the controls checklist that closes M2, and the match log that the M6 fun gate requires. Both are filled by hand by the owner or a tester, with a date, after playing a build from `main`. The checklist states what each control must do; the log records what a full human-versus-Standard match felt like. Empty cells mean not yet tested. The fun-gate thresholds that these logs are checked against are listed at the end.

## Controls checklist (M2)

Run `cargo run -p game --features dev` from `main`. Mark each row Pass or Fail with the date and the commit short sha.

| Control | Expected | Pass/Fail | Date | Commit | Notes |
|---|---|---|---|---|---|
| Two-finger trackpad scroll | Pans the camera; arrives as `MouseScrollUnit::Pixel` | | | | |
| Pinch on trackpad | Zooms between 15 m and 60 m with smoothing | | | | |
| Mouse wheel | Zooms; arrives as `MouseScrollUnit::Line` | | | | |
| W, A, S, D | Pans the camera | | | | |
| Arrow keys | Pans the camera | | | | |
| Edge scroll | Cursor at a screen edge pans in that direction | | | | |
| Camera travel at 30 fps and 120 fps | Holding D for 2 s moves the camera the same distance under `--max-fps 30` and `--max-fps 120` | | | | |
| Left click on a unit | Selects it; selection ring appears | | | | |
| Shift-click on a unit | Adds to or removes from the selection | | | | |
| Drag box | Selects every own unit whose position projects inside the box | | | | |
| Double-click a unit | Selects every unit of the same kind visible on screen | | | | |
| Ctrl+1 | Assigns the selection to group 1 | | | | |
| 1 | Recalls group 1 | | | | |
| Right-click on ground | Issues Move; move marker appears; units arrive | | | | |
| Shift+right-click | Queues a Move after the current one | | | | |
| S | Stop | | | | |
| Esc | Clears the selection (pause menu from M6) | | | | |
| Close via the red window button | Process exits with code 0 (`echo $?`) | | | | |
| Cmd-Q | Same as above through the menu item, and the replay is complete | | | | |
| `kill -9` the process mid-match, then `sim-cli verify` on its replay | `OK`; at most one second of commands lost | | | | |
| Window resize | HUD stays anchored; world picking still correct | | | | |

The replay directory is `~/Library/Application Support/com.tonianev.Eonmark/replays/` unless `--replay-dir` was passed. The exact path is printed at startup.

## Match log (M6)

Play against the Standard bot from New Game to the game-over screen. One row per match. Keep the replay file; its name is the seed and timestamp.

| Date | Commit | Seed | Difficulty | Duration (min) | Outcome | AI reached my borders | Town below 50% HP | Cap readout amber | Fun (1-5) | Notes |
|---|---|---|---|---|---|---|---|---|---|---|
| | | | Standard | | decisive / tiebreak | y / n | y / n | y / n | | |
| | | | Standard | | decisive / tiebreak | y / n | y / n | y / n | | |
| | | | Standard | | decisive / tiebreak | y / n | y / n | y / n | | |

Column meanings:

| Column | Meaning |
|---|---|
| Outcome | `decisive` if the match ended by capital capture or all Towns lost; `tiebreak` if the 45-minute territory tiebreak ended it. Add `win` or `loss`. |
| AI reached my borders | The bot's army crossed into tiles I owned at least once |
| Town below 50% HP | Any of my Towns dropped below half hit points, or was annexed |
| Cap readout amber | The Yield Cap indicator in the resource bar turned amber at least once |
| Fun | Owner's rating, 1 to 5, honest |
| Notes | What was tense, what was dull, which number felt wrong and in which RON file |

## Fun gate thresholds (M6)

M6 cannot close until all of these hold. Bot-side numbers come from `sim-cli play-bots`; human-side numbers come from the log above.

| Measure | Threshold |
|---|---|
| Seeded Standard-vs-Standard bot games (distinct personalities) | 20 of 20 end with a declared winner; territory-tiebreak wins are valid |
| Decisive (capital capture) before tick 48000 (40 min) | at least 10 of 20 |
| Median bot game length | at most 45 min |
| Difficulty ordering (Hard beats Standard beats Easy) | at least 15 of 20 |
| Logged human-vs-Standard matches | 3, each 20 to 40 min |
| AI army reaches the player's borders and annexes or reduces a Town below 50% HP | in at least 2 of 3 |
| Yield Cap readout turns amber | at least once per match |
| Owner fun rating | at least 3 of 5 |

Two to three tuning iterations of `data/rules/rules.ron` are budgeted. The late-game pressure knobs (Charter Age Harrying bonus, optional Yield Cap decay after minute 30) exist for this purpose. Record each iteration as a new set of three rows above with the commit that changed the numbers.
