# Streaming Eonmark

How the owner streams a match from the dev Mac: the commands, the settings that bite, OBS, a five-minute pre-flight and what to keep afterwards. Every flag, path and number below was checked against the code at the art pass's close (2026-10-28); `eonmark --help` prints the flags.

## Two stream formats

| Format | Command | Notes |
|---|---|---|
| You against the Standard bot | `cargo run --release -p game -- --skip-menu --window-size 1280x720 --draw-stats` | `--skip-menu` starts the match at once. Without `--seed <n>` the seed comes from the wall clock; it is in the replay's file name (`<yyyymmdd-hhmmss>-<seed>.eonreplay`), which the outcome overlay shows. A seed does not reproduce a match you played: your orders are in the replay, not the seed. |
| Bot against bot (spectate) | `cargo run --release -p game -- --spectate --seed 1 --window-size 1280x720 --draw-stats` | Seed 1 is the `m5_first_match` setup: P0 takes P1's Seat at tick 26012, 21:40 on the overlay's clock. You can select units and move the camera but give no orders. `--fast-forward-to 9000` skips the opening (7:30) before the window opens; it works only with `--spectate` and is refused if the match is decided by then. The replay is recorded from that tick and still verifies. |

- **Why 1280x720:** it is the logical size the art pass measures fps at (AP5 in the ROADMAP's "Art pass (pre-stream)"). On a Retina display it is 2560x1440 physical pixels, which OBS scales down to 1080p. A 1920x1080 window renders 3840x2160 physical pixels, 2.25 times as many, for nothing once OBS scales it to 1080p.
- **`--draw-stats`** costs next to nothing and makes the game print its `frame_stats:` lines on exit (frame times, the window, draw calls); without it a plain match prints only the minimap's line.
- **Framing:** the camera starts over your first units at 35 m, its default zoom; the wheel or a pinch zooms from 15 to 60 m. The art pass is judged at 40 m, and `--camera 24,64,40` opens there over P0's Seat (`--camera <x>,<y>,<zoom>`, a map tile and a zoom).
- **Build:** a release build only. A `dev` build links Bevy dynamically and draws the egui dev panels over the game.

## Settings that bite

1. **Keep the game window uncovered.** macOS stops presenting frames to a window that is fully covered (wgpu checks the window's occlusion before each frame), so a Window Capture in OBS freezes while anything covers the whole game. Put OBS on a second display, or capture the display rather than the window. The `frame_stats: window focused=<n>/<frames> occluded=<n>/<frames> events=...` exit line counts the covered frames.
2. **Unfocused means 60 Hz.** Bevy 0.19.1's default `WinitSettings::game()` updates a focused window continuously and an unfocused one at most every 1/60 s. That suits a 60 fps stream, but click the game before you play so OBS does not hold the focus. Background mode (`--background`) keeps updating continuously; it is for automated runs only.
3. **Power:** plug in the adapter and set System Settings, Battery, Energy Mode to High Power. Apple Silicon lowers its GPU clocks on battery.

## OBS (Apple Silicon)

- **Source:** macOS Screen Capture, method "Window", the Eonmark window, cursor shown.
- **Video:** base canvas 2560x1440; output 1920x1080 with the Lanczos downscale; 60 fps.
- **Encoder:** Apple VT H.264 Hardware Encoder; CBR, keyframe interval 2 s, profile High; 6,000 Kbps for Twitch at 1080p60, 9,000 to 12,000 Kbps for YouTube at 1080p60.
- **Audio:** the game has none yet (M7), so the microphone only.

## Pre-flight (5 minutes)

1. `cargo build --release -p game -p sim-cli`.
2. Run the exact stream command for two minutes off air, with the window focused, then quit with Cmd-Q.
3. Check the exit lines:
   - `frame_stats: window logical=1280x720 physical=2560x1440 scale=2.0`;
   - `frame_stats: frames=<n> seconds=<s> fps=<f>` at 60 or more, and `frame_stats: frame_ms mean=<ms> p95=<ms> ...` with p95 at or under 16.7;
   - `frame_stats: window focused=<n>/<n> occluded=0/<n>`;
   - `frame_stats: draws frames=<n> mean=<f> p95=<n> max=<n> ...` with max under 500.
4. Verify the replay the run printed as `replay: <path>`: `cargo run --release -p sim-cli -- verify <path>` must print `OK final_hash=0x... ticks=<n>`.

## After the stream

- Every match records a replay. The game prints `replay: <path>` on stdout when the match starts; the file is under `~/Library/Application Support/com.tonianev.Eonmark/replays/` unless `--replay-dir` names another directory.
- Verify it with `sim-cli verify` from the same checkout and keep it with the VOD: a later build whose rules differ prints `RULES CHANGED` instead of `OK` ([DETERMINISM.md](DETERMINISM.md)).
- For a spectated match, put the seed in the VOD description: `--spectate --seed <n>` on the same build plays the same match again. A match you played needs its replay file; the game has no replay viewer yet, and `eonmark --headless-run <file>` re-simulates it without a window and checks its final hash.
- A bug seen on stream becomes an issue with the replay attached; `cargo run --release -p sim-cli -- hash-dump <replay> --every 1` prints a hash per tick to bisect a divergence ([DETERMINISM.md](DETERMINISM.md)).
