#!/usr/bin/env bash
# art_drive.sh: the art pass's real-input checks (ROADMAP "Art pass
# (pre-stream)", AP10 and AP6), with real X input from xdotool on the real
# window, the scripts/m4b_outpost_drive.sh precedent. Synthetic pointer
# input fails under lavapipe (issue #33), so the game only prints what to
# click and judges what the clicks did; this script does the clicking.
# `just art-drive` builds both binaries and runs it. Local only, like
# scripts/m5b_checks.sh: pasted into the PR, not a CI job.
#
#   1. Click check (AP10): `--scenario unit_looks --click-check`. At tick
#      60 the game prints one `click_check: target <i> unit=<id> kind=<k>
#      team=<t> px=<x>,<y>` line per unit, a pixel whose ray hits that
#      unit's model first (crates/game/src/click_check.rs). The script
#      clicks each target in turn and waits for the game's `click_check:
#      selected <i> ...` (or `miss <i> ...`) line before the next. Pass
#      when the game prints `click_check: ok 16/16` (all eight kinds of both
#      teams) and exits 0.
#   2. Minimap check (AP6): `--scenario economy_walkthrough
#      --minimap-check`. From tick 40 the game prints the minimap's rect,
#      then one step at a time (crates/game/src/minimap_check.rs):
#      `step 1 press px=<x>,<y>` (a left press must move the camera's
#      focus into that quarter), `step 2 drag from=<x>,<y> to=<x>,<y>` (a
#      left drag, the focus must follow), `step 3 right_click px=<x>,<y>`
#      (with the director's Yeomen selected, exactly one Move to that tile
#      must be queued) and `step 4 key=a left_click px=<x>,<y>` (A arms
#      attack-move, then a left click on the minimap must queue one
#      AttackMove). The script performs each step when it is printed. Pass
#      when the game prints `minimap_check: ok` and exits 0.
#   Each run must also exit with no ERROR line in its log, and `sim-cli
#   verify` of the replay it recorded must print OK: the clicks were real
#   commands through the recorder.
#   3. No game and no Xvfb this script started is left: the script keeps
#      the process id and name of each game and Xvfb it starts and checks
#      that none of them is still running, so another session's game or
#      display on the same machine does not fail it.
#
# Each run gets its own Xvfb (1280 x 800, a free display picked with
# -displayfd), runs in background mode (docs/IMPLEMENTER_NOTES.md rule 25:
# the window opens unfocused in the top-left corner, so window pixels are
# screen pixels with no window manager) and has a 240 s deadline as a
# safety net; on the 4-vCPU VM the click check ends about 40 s in and the
# minimap check about 30 s in. Background windows start unfocused, so the
# script focuses the window (`xdotool windowfocus`) before it sends the A
# key. Run it from the repository root's checkout (`sim-cli verify` reads
# data/ from there).
#
# Needs Xvfb, xdotool, Mesa's lavapipe (mesa-vulkan-drivers) and the X
# libraries scripts/render_smoke.sh lists. Usage:
#   scripts/art_drive.sh <eonmark binary> <sim-cli binary> [out-dir]
# Build the binaries with `cargo build -p game --profile ci` and
# `cargo build -p sim-cli --release`.
set -euo pipefail

usage="usage: scripts/art_drive.sh <eonmark binary> <sim-cli binary> [out-dir]"
bin="${1:?$usage}"
simcli="${2:?$usage}"
out="${3:-}"
if [ -z "$out" ]; then
  out="$(mktemp -d "${TMPDIR:-/tmp}/eonmark-art-drive.XXXXXX")"
fi
mkdir -p "$out"
out="$(cd "$out" && pwd)"
bin="$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")"
simcli="$(cd "$(dirname "$simcli")" && pwd)/$(basename "$simcli")"
for tool in Xvfb xdotool; do
  command -v "$tool" >/dev/null || { echo "error: $tool not found (apt-get install xvfb xdotool)" >&2; exit 1; }
done
icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
[ -n "$icd" ] || { echo "error: lavapipe not found (apt-get install mesa-vulkan-drivers)" >&2; exit 1; }

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

fail=0
bad() { echo "FAIL  $*"; fail=1; }
ok() { echo "PASS  $*"; }
say() { echo "    drive: $*"; }

xvfb=""
game=""
# "<pid> <name>" of every process this script started, for step 3.
started=()
cleanup() {
  if [ -n "$game" ]; then kill "$game" 2>/dev/null || true; fi
  if [ -n "$xvfb" ]; then kill "$xvfb" 2>/dev/null || true; fi
  wait 2>/dev/null || true
  game=""
  xvfb=""
}
trap cleanup EXIT

# Start an Xvfb for the run in directory $1 and export its DISPLAY.
start_xvfb() {
  local dir=$1
  rm -f "$dir/display"
  Xvfb -displayfd 3 -screen 0 1280x800x24 3>"$dir/display" >"$dir/xvfb.log" 2>&1 &
  xvfb=$!
  started+=("$xvfb Xvfb")
  for _ in $(seq 100); do
    [ -s "$dir/display" ] && break
    kill -0 "$xvfb" 2>/dev/null || break
    sleep 0.1
  done
  if ! [ -s "$dir/display" ] || ! kill -0 "$xvfb" 2>/dev/null; then
    echo "error: Xvfb did not start (see $dir/xvfb.log)" >&2
    exit 1
  fi
  DISPLAY=":$(tr -d '[:space:]' <"$dir/display")"
  export DISPLAY
}

# Wait until the log $1 has a line matching the regex $2 and print the
# last such line; returns 1 if the game exits first.
wait_for() {
  local log=$1 pattern=$2
  until grep -qE "$pattern" "$log"; do
    if ! kill -0 "$game" 2>/dev/null; then
      say "the game exited while waiting for /$pattern/"
      return 1
    fi
    sleep 0.2
  done
  grep -E "$pattern" "$log" | tail -n 1
}

# A press and release a few frames apart (lavapipe draws a few a second).
click() {
  xdotool mousemove "$1" "$2"
  sleep 0.6
  xdotool mousedown "${3:-1}"
  sleep 0.3
  xdotool mouseup "${3:-1}"
}

# The value of field $2 (`name=value`) in line $1.
field() {
  local word
  for word in $1; do
    if [ "${word%%=*}" = "$2" ]; then
      echo "${word#*=}"
      return
    fi
  done
}

# Wait for the game to exit; sets `status`.
status=0
finish_game() {
  status=0
  wait "$game" || status=$?
  game=""
}

# The common verdict lines for the run whose log is $1: exit code, no
# ERROR line, and `sim-cli verify` of the recorded replay.
judge_run() {
  local log=$1 errors replay verify_out
  if [ "$status" -eq 0 ]; then ok "the game exited 0"; else bad "the game exited $status"; fi
  errors=$(sed 's/\x1b\[[0-9;]*m//g' "$log" | grep -E '^[^ ]+ +ERROR ' || true)
  if [ -z "$errors" ]; then ok "no ERROR line in $log"; else bad "ERROR lines in $log:"; echo "$errors"; fi
  replay=$(sed -n 's/^replay: //p' "$log" | head -n 1)
  if [ -n "$replay" ] && [ -f "$replay" ]; then
    verify_out=$("$simcli" verify "$replay" 2>&1 || true)
    echo "    $verify_out"
    case "$verify_out" in
      "OK final_hash="*) ok "the run's replay verifies ($(basename "$replay"))" ;;
      *) bad "sim-cli verify $replay did not print OK" ;;
    esac
  else
    bad "no replay file in $log"
  fi
}

echo "==> 1. click check: --scenario unit_looks --click-check (AP10)"
d1="$out/click"
rm -rf "$d1"
mkdir -p "$d1"
log="$d1/game.log"
start_xvfb "$d1"
say "Xvfb on $DISPLAY"
VK_ICD_FILENAMES="$icd" "$bin" --background --scenario unit_looks --click-check \
  --exit-after-seconds 240 --replay-dir "$d1" --screenshot-at "60:$d1/targets.png" \
  >"$log" 2>&1 &
game=$!
started+=("$game $(basename "$bin")")
if wait_for "$log" '^click_check: (target|FAIL)' >/dev/null; then
  # The targets are printed together in one frame; let the frame finish.
  sleep 1
  mapfile -t targets < <(grep '^click_check: target ' "$log")
  say "${#targets[@]} targets"
  for t in "${targets[@]}"; do
    i=$(awk '{print $3}' <<<"$t")
    px=$(field "$t" px)
    click "${px%,*}" "${px#*,}"
    if ! wait_for "$log" "^click_check: (selected|miss) $i " >/dev/null; then
      bad "no verdict for target $i"
      break
    fi
    grep -E "^click_check: (selected|miss) $i " "$log" | sed 's/^/    /'
    sleep 0.8
  done
fi
finish_game
verdict=$(grep -E '^click_check: (ok|FAIL)' "$log" || true)
echo "    ${verdict:-no click_check verdict}"
if [ "$verdict" = "click_check: ok 16/16" ]; then
  ok "every unit kind of both teams was selected by a real click"
else
  bad "expected 'click_check: ok 16/16'"
fi
judge_run "$log"
cleanup

echo
echo "==> 2. minimap check: --scenario economy_walkthrough --minimap-check (AP6)"
d2="$out/minimap"
rm -rf "$d2"
mkdir -p "$d2"
log="$d2/game.log"
start_xvfb "$d2"
say "Xvfb on $DISPLAY"
VK_ICD_FILENAMES="$icd" "$bin" --background --scenario economy_walkthrough --minimap-check \
  --exit-after-seconds 240 --replay-dir "$d2" >"$log" 2>&1 &
game=$!
started+=("$game $(basename "$bin")")
steps_ok=0
while :; do
  l=$(wait_for "$log" '^minimap_check: (step 1 |FAIL)') || break
  grep '^minimap_check: rect ' "$log" | sed 's/^/    /'
  echo "    $l"
  case "$l" in *FAIL*) break ;; esac
  px=$(field "$l" px)
  click "${px%,*}" "${px#*,}"

  l=$(wait_for "$log" '^minimap_check: (step 2 |FAIL)') || break
  echo "    $l"
  case "$l" in *FAIL*) break ;; esac
  from=$(field "$l" from)
  to=$(field "$l" to)
  x=${from%,*} y=${from#*,} x2=${to%,*} y2=${to#*,}
  xdotool mousemove "$x" "$y"
  sleep 0.6
  xdotool mousedown 1
  sleep 0.4
  for s in 1 2 3 4 5 6; do
    xdotool mousemove $((x + (x2 - x) * s / 6)) $((y + (y2 - y) * s / 6))
    sleep 0.4
  done
  sleep 0.4
  xdotool mouseup 1

  l=$(wait_for "$log" '^minimap_check: (step 3 |FAIL)') || break
  echo "    $l"
  case "$l" in *FAIL*) break ;; esac
  px=$(field "$l" px)
  click "${px%,*}" "${px#*,}" 3

  l=$(wait_for "$log" '^minimap_check: (step 4 |FAIL)') || break
  echo "    $l"
  case "$l" in *FAIL*) break ;; esac
  px=$(field "$l" px)
  xdotool mousemove "${px%,*}" "${px#*,}"
  sleep 0.6
  # Background mode opens the window unfocused, and X sends key presses
  # to the focused window only.
  win=$(xdotool search --name '^Eonmark$' | head -n 1 || true)
  if [ -n "$win" ]; then xdotool windowfocus --sync "$win" 2>/dev/null || true; fi
  xdotool key a
  sleep 1.0
  xdotool mousedown 1
  sleep 0.3
  xdotool mouseup 1
  steps_ok=1
  break
done
if [ "$steps_ok" -eq 1 ]; then
  wait_for "$log" '^minimap_check: (ok|FAIL)' >/dev/null || true
fi
finish_game
grep -E '^minimap_check: (pass|ok|FAIL)' "$log" | sed 's/^/    /' || true
grep '^frame_stats: minimap ' "$log" | sed 's/^/    /' || true
if grep -qx 'minimap_check: ok' "$log"; then
  ok "press, drag, right click and attack-move click on the minimap did what they should"
else
  bad "expected 'minimap_check: ok'"
fi
judge_run "$log"
cleanup

echo
echo "==> 3. no game process left"
# The processes of `started` still running under their own name (a
# process id the system reused for another program does not count; the
# kernel keeps the first 15 bytes of a name).
alive() {
  local entry pid name comm
  for entry in "${started[@]}"; do
    pid=${entry%% *}
    name=${entry#* }
    comm=$(ps -o comm= -p "$pid" 2>/dev/null || true)
    if [ -n "$comm" ] && [ "$comm" = "${name:0:15}" ]; then echo "$pid $name"; fi
  done
}
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if [ -n "$(alive)" ]; then sleep 1; else break; fi
done
left=$(alive)
if [ -n "$left" ]; then
  bad "still running:"
  sed 's/^/    /' <<<"$left"
else
  ok "none of the ${#started[@]} games and Xvfb servers this script started is running"
fi

echo
if [ "$fail" -ne 0 ]; then
  echo "art_drive: FAIL ($out)"
  exit 1
fi
echo "art_drive: ok, click_check: ok 16/16 and minimap_check: ok, both replays verify ($out)"
