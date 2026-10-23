#!/usr/bin/env bash
# m4b_outpost_drive.sh: play ROADMAP M4b acceptance line 1 on the
# interactive `--scenario enemy_outpost` with real X input, then verify the
# replay the game recorded.
#
# The game runs under Xvfb with Mesa's lavapipe in background mode (rule 25
# of docs/IMPLEMENTER_NOTES.md), with nothing scripted for the local player:
# every order of the sequence below comes from xdotool's XTEST mouse
# events on the real window, through the same picking, card, placement and
# order systems a player's clicks go through. The camera stays at
# `--camera 41,64,50` (P0's base and the Watchtower in one view, no key
# that pans is pressed), so every world point below is a fixed pixel,
# computed with the RTS camera's pose (55 degree pitch, 40 degree vertical
# field of view, 1280 x 800). The steps wait for sim ticks, not seconds:
# each one waits for the game to log that it reached a `--screenshot-at`
# tick, which also leaves a picture of the moment in the output directory.
#
#   tick   40  box-select the five Yeomen around the Seat
#   tick   60  Build Scriptorium on the card, place it at tile (28, 59)
#   tick  100  Build Muster Hall on the card, place it at (29, 69)
#   tick  900  click the Scriptorium, research Letters I and Trade I
#   tick 1250  click the Muster Hall, right-click a rally point at (46, 59),
#              train Skirmisher x2, Bowman x2, Shieldbearer (the vanguard
#              is gone by then, so pop is free)
#   tick 2200  click Advance (both techs are done)
#   tick 2240  box-select the base's Yeomen, Build Engine Yard at (41, 63)
#   tick 3700  click the Engine Yard, rally at (47, 64), train 2 Mangonels
#   tick 5600  box-select the mixed army, Attack-move button, click (56, 64)
#   tick 5620  box-select the Mangonels (and any line unit beside them),
#              right-click the Watchtower
#   tick 6000  the end: the window is closed (WM_DELETE_WINDOW, what a
#              window manager sends for the close button)
#
# Each step waits about 100 ticks past the moment its building is complete
# or its units stand at their rally point in a measured drive (each click
# takes 1.5 s, about 30 ticks, so the later clicks of a step land later).
#
# Pass when the game exits 0 with no ERROR line, `sim-cli verify` prints
# OK for the recorded replay with no truncation warning, its ticks are past
# 6000, and the step pictures show the sequence: the two sites, the research
# card, the Muster Hall's queue, "Masonry Age", the Engine Yard's Mangonels,
# and at tick 6000 no tower and "Watchtower destroyed" on the message line.
# The script itself exits 1 when its Xvfb does not start, when the game
# exits early, non-zero or with an ERROR line, when `verify` fails, and when
# the replay lacks the `EONRDONE` clean-exit trailer (`verify` only warns
# about that and still prints OK, and closing the window is the path that
# once lost the trailer). Xvfb picks a free display itself (`-displayfd`),
# so two drives, or a drive beside another windowed run, never share an X
# server and each other's clicks. Under lavapipe the sim keeps its 20 ticks
# a second, so a drive takes about five minutes. Run it from the repository
# root (`sim-cli verify` reads `data/`).
#
# Needs Xvfb, xdotool, python3 (ctypes and libX11 for the close), Mesa's
# lavapipe and the X libraries render_smoke.sh lists. Usage:
#   scripts/m4b_outpost_drive.sh <eonmark binary> <sim-cli binary> <out-dir>
# Build the binaries with `cargo build -p game --profile ci` and
# `cargo build -p sim-cli --release`.
set -euo pipefail

usage="usage: scripts/m4b_outpost_drive.sh <eonmark binary> <sim-cli binary> <out-dir>"
bin="${1:?$usage}"
simcli="${2:?$usage}"
out="${3:?$usage}"
for tool in Xvfb xdotool python3; do
  command -v "$tool" >/dev/null || { echo "error: $tool not found" >&2; exit 1; }
done
icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
[ -n "$icd" ] || { echo "error: lavapipe not found (apt-get install mesa-vulkan-drivers)" >&2; exit 1; }

mkdir -p "$out"
log="$out/game.log"
say() { echo "drive: $*"; }
# Xvfb takes the first free display and writes its number to fd 3 once it
# accepts connections.
rm -f "$out/display"
Xvfb -displayfd 3 -screen 0 1280x800x24 3>"$out/display" >"$out/xvfb.log" 2>&1 &
xvfb=$!
game=""
cleanup() {
  if [ -n "$game" ]; then kill "$game" 2>/dev/null || true; fi
  kill "$xvfb" 2>/dev/null || true
  wait 2>/dev/null || true
}
trap cleanup EXIT
for _ in $(seq 100); do
  [ -s "$out/display" ] && break
  kill -0 "$xvfb" 2>/dev/null || break
  sleep 0.1
done
if ! [ -s "$out/display" ] || ! kill -0 "$xvfb" 2>/dev/null; then
  echo "error: Xvfb did not start (see $out/xvfb.log)" >&2; exit 1
fi
display=":$(tr -d '[:space:]' <"$out/display")"
export DISPLAY="$display"
say "Xvfb $xvfb on $display"

# One `--screenshot-at` per step: the log line is the step's tick signal.
steps=(40 60 100 900 1250 2200 2240 3700 5600 5620 6000)
shots=()
for t in "${steps[@]}"; do
  shots+=(--screenshot-at "$t:$out/drive-$(printf %05d "$t").png")
done
# The deadline is a safety net only; the drive closes the window itself.
VK_ICD_FILENAMES="$icd" "$bin" --background --scenario enemy_outpost \
  --camera 41,64,50 --exit-after-seconds 900 --replay-dir "$out" "${shots[@]}" \
  >"$log" 2>&1 &
game=$!

# Wait until the game logs that the sim reached tick $1.
at_tick() {
  local t="$1"
  until grep -q "reached $t, capturing" "$log"; do
    kill -0 "$game" 2>/dev/null || { say "the game exited before tick $t"; exit 1; }
    sleep 0.2
  done
  say "tick $t"
}
# A press and release a few frames apart (lavapipe draws about 6 a second).
click() { xdotool mousemove "$1" "$2"; sleep 0.6; xdotool mousedown "${3:-1}"; sleep 0.3; xdotool mouseup "${3:-1}"; sleep 0.6; }
# A left-button drag box from ($1, $2) to ($3, $4).
box() {
  xdotool mousemove "$1" "$2"; sleep 0.4
  xdotool mousedown 1; sleep 0.4
  xdotool mousemove $(( ($1 + $3) / 2 )) $(( ($2 + $4) / 2 )); sleep 0.4
  xdotool mousemove "$3" "$4"; sleep 0.6
  xdotool mouseup 1; sleep 0.8
}
# A card button, then a site for the ghost: hover until it turns green.
place() { click "$1" "$2"; xdotool mousemove "$3" "$4"; sleep 1.5; click "$3" "$4"; }

# Pixels at --camera 41,64,50 (tile centres on the ground unless noted).
CARD_TOP=732; CARD_BOTTOM=772; CARD_ONE_ROW=752  # two-row cards, one-row cards
COL=(690 834 978 1122)                           # card columns, left to right
SCRIPTORIUM_SITE=(379 323)  # (28.5, 59.5): footprint (27..29, 58..60)
MUSTER_SITE=(370 506)       # (29.5, 69.5): footprint (28..30, 68..70)
YARD_SITE=(651 391)         # (41.5, 63.5): footprint (40..42, 62..64)
# A building is clicked on its roof (1.7 m up for a 3 x 3), away from the
# Yeomen that stand beside it once they are done building.
SCRIPTORIUM_ROOF=(372 300)
MUSTER_ROOF=(362 485)
YARD_ROOF=(651 369)
RALLY_ARMY=(755 323)        # (46.5, 59.5)
RALLY_SIEGE=(784 409)       # (47.5, 64.5)
TOWER=(1033 398)            # the Watchtower's roof, (58.5, 64.5) 0.9 m up
SIEGE_GOAL=(983 409)        # (56.5, 64.5), beside the tower
BASE_BOX=(150 280 470 560)  # tiles about x 18..32, y 57..72
ARMY_BOX=(680 268 852 362)  # tiles about x 43..51, y 56..62
SIEGE_BOX=(705 372 872 458) # tiles about x 44..51, y 62..67
ADVANCE=(1200 42)           # on the age strip whatever its label

at_tick 40
until win=$(xdotool search --name '^Eonmark$' 2>/dev/null | head -1) && [ -n "$win" ]; do sleep 0.5; done
say "window $win"
xdotool mousemove 640 400
sleep 0.5
box "${BASE_BOX[@]}"

at_tick 60
# Build card (8 kinds, two rows): Scriptorium is the 5th, Muster Hall the
# 6th, Engine Yard the 8th.
place "${COL[0]}" "$CARD_BOTTOM" "${SCRIPTORIUM_SITE[@]}"

at_tick 100
place "${COL[1]}" "$CARD_BOTTOM" "${MUSTER_SITE[@]}"

at_tick 900
click "${SCRIPTORIUM_ROOF[@]}"
# Scriptorium card: Scribe, Arms I, Statecraft I, Trade I / Letters I.
click "${COL[3]}" "$CARD_BOTTOM"   # Letters I
click "${COL[3]}" "$CARD_TOP"      # Trade I
xdotool mousemove 640 400

at_tick 1250
click "${MUSTER_ROOF[@]}"
click "${RALLY_ARMY[@]}" 3
# Muster Hall card, one row: Skirmisher, Shieldbearer, Bowman, Outrider.
click "${COL[0]}" "$CARD_ONE_ROW"
click "${COL[0]}" "$CARD_ONE_ROW"
click "${COL[2]}" "$CARD_ONE_ROW"
click "${COL[2]}" "$CARD_ONE_ROW"
click "${COL[1]}" "$CARD_ONE_ROW"
xdotool mousemove 640 400

at_tick 2200
click "${ADVANCE[@]}"
xdotool mousemove 640 400

at_tick 2240
box "${BASE_BOX[@]}"
place "${COL[3]}" "$CARD_BOTTOM" "${YARD_SITE[@]}"

at_tick 3700
click "${YARD_ROOF[@]}"
click "${RALLY_SIEGE[@]}" 3
# Engine Yard card, one row ending at the right: Mangonel, Supply Wain.
click "${COL[2]}" "$CARD_ONE_ROW"
click "${COL[2]}" "$CARD_ONE_ROW"
xdotool mousemove 640 400

at_tick 5600
box "${ARMY_BOX[@]}"
# An armed selection with no Yeoman: the card's one button is Attack-move.
click "${COL[3]}" "$CARD_ONE_ROW"
click "${SIEGE_GOAL[@]}"

at_tick 5620
box "${SIEGE_BOX[@]}"
xdotool mousemove "${TOWER[@]}"
sleep 1.5
click "${TOWER[@]}" 3
xdotool mousemove 640 400

at_tick 6000
sleep 2
say "closing the window"
python3 - "$win" <<'PY'
import ctypes, sys
x = ctypes.cdll.LoadLibrary("libX11.so.6")
x.XOpenDisplay.restype = ctypes.c_void_p
x.XInternAtom.restype = ctypes.c_ulong
x.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
x.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.c_void_p]
x.XFlush.argtypes = [ctypes.c_void_p]
x.XCloseDisplay.argtypes = [ctypes.c_void_p]
class ClientMessage(ctypes.Structure):
    _fields_ = [("type", ctypes.c_int), ("serial", ctypes.c_ulong), ("send_event", ctypes.c_int),
                ("display", ctypes.c_void_p), ("window", ctypes.c_ulong), ("message_type", ctypes.c_ulong),
                ("format", ctypes.c_int), ("data", ctypes.c_long * 5)]
class XEvent(ctypes.Union):
    _fields_ = [("xclient", ClientMessage), ("pad", ctypes.c_long * 24)]
d = x.XOpenDisplay(None)
win = int(sys.argv[1])
ev = XEvent()
ev.xclient.type = 33  # ClientMessage
ev.xclient.window = win
ev.xclient.message_type = x.XInternAtom(d, b"WM_PROTOCOLS", 0)
ev.xclient.format = 32
ev.xclient.data[0] = x.XInternAtom(d, b"WM_DELETE_WINDOW", 0)
x.XSendEvent(d, win, 0, 0, ctypes.byref(ev))
x.XFlush(d)
x.XCloseDisplay(d)
PY
status=0
wait "$game" || status=$?
game=""
say "game exit $status"
if grep -q "ERROR" "$log"; then
  say "ERROR lines in $log:"; grep "ERROR" "$log" >&2; exit 1
fi
[ "$status" -eq 0 ] || exit 1
# The game prints the path it records to as `replay: <path>` at start-up.
replay=$(sed -n 's/^replay: //p' "$log" | head -1)
say "verify $replay"
verified=0
"$simcli" verify "$replay" 2>"$out/verify.err" || verified=$?
cat "$out/verify.err" >&2
[ "$verified" -eq 0 ] || exit "$verified"
# Only the truncation warning counts, as in scripts/m2_checks.sh step 3.
if grep -q 'warning:.*no clean-exit trailer' "$out/verify.err" \
  || [ "$(tail -c 8 "$replay")" != EONRDONE ]; then
  say "the replay has no clean-exit trailer"; exit 1
fi
