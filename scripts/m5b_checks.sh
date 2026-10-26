#!/usr/bin/env bash
# m5b_checks.sh: the automated proxy of ROADMAP M5b acceptance line 4 (the
# [owner] line: "From the window: New Game starts a match vs the Standard
# bot on plains_1v1; the owner plays to a victory or defeat overlay in one
# sitting; the auto-recorded replay verifies to the hash shown on the
# overlay"), with the evidence substitutions of M5b owner decision 8: the
# overlay screenshot is taken with --screenshot-at instead of ci_testing,
# and the proxy replay begins at the fast-forward tick. `just m5b-checks`
# builds both binaries and runs it; it is a local, PR-pasted check like
# scripts/m5a_checks.sh, not a CI job.
#
#   1. `sim-cli verify` of the committed golden
#      crates/sim/tests/fixtures/m5_first_match.eonreplay prints
#      `OK final_hash=H ticks=T`, and H is m5_first_match.hash.
#   2. The overlay: the game runs `--spectate --seed 1` (the Standard bot
#      against itself, m5_first_match's setup) fast-forwarded to T-400
#      with a screenshot at tick T. It must exit 0 with no ERROR line,
#      print `fast_forward: tick=<T-400>`, `outcome: victory player=<0|1>
#      ... tick=T hash=H` and a `replay:` path, and save the PNG
#      (m5b-overlay.png, the frame that first shows the overlay). Then
#      `sim-cli verify` of the game's own replay, recorded from tick
#      T-400, prints the same `OK final_hash=H ticks=T`.
#   3. The menu: a plain launch for 6 s with --screenshot (m5b-menu.png).
#      It must exit 0 and stay on the menu: no `match:` and no `replay:`
#      line, and its --replay-dir stays empty (the lazy recorder).
#   4. New Game: a plain launch with --seed 7 whose
#      --new-game-after-seconds proxy clicks the real button. It must
#      print `new-game: clicked at`, `match: new game vs standard` and a
#      `replay:` path, and `sim-cli verify` of that replay must print `OK`
#      with more than 600 ticks. The bot trains a Yeoman at its first
#      plan, so only the Standard bot rebuilt from the header verifies it
#      (a Passive rebuild diverges, game test
#      `a_match_vs_the_standard_bot_records_a_replay_that_verifies_to_the_outcome_hash`).
#   5. `scripts/m2_checks.sh` checks 3 and 4 (clean exit and kill -9, both
#      with --skip-menu) on the same binaries.
#   6. No eonmark (and no Xvfb) process is left.
#
# On Linux without a display every game runs under `xvfb-run` on Mesa's
# lavapipe (as scripts/render_smoke.sh and m5a_checks.sh do). There the
# fast-forward takes about 1.5 s, the 400 live ticks about 20 s, and the
# outcome comes about 25 s in; 90 s is a wide margin.
#
# Usage: scripts/m5b_checks.sh <eonmark binary> <sim-cli binary> [out-dir]
# Env:   M5B_OVERLAY_SECONDS=90  how long the overlay run lasts
#        M5B_CLICK_SECONDS=45    how long the New Game run lasts
set -euo pipefail

usage="usage: scripts/m5b_checks.sh <eonmark binary> <sim-cli binary> [out-dir]"
bin="${1:?$usage}"
simcli="${2:?$usage}"
out="${3:-}"
if [ -z "$out" ]; then
  out="$(mktemp -d "${TMPDIR:-/tmp}/eonmark-m5b-checks.XXXXXX")"
fi
mkdir -p "$out"
out="$(cd "$out" && pwd)"
bin="$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")"
simcli="$(cd "$(dirname "$simcli")" && pwd)/$(basename "$simcli")"
overlay_seconds="${M5B_OVERLAY_SECONDS:-90}"
click_seconds="${M5B_CLICK_SECONDS:-45}"

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

# Background mode for every windowed run (rule 25).
export EONMARK_BACKGROUND=1

# A `--features dev` binary on macOS links libstd through @rpath without an
# LC_RPATH entry; `cargo run` would set this. Ignored on Linux.
export DYLD_FALLBACK_LIBRARY_PATH="$(rustc --print target-libdir)${DYLD_FALLBACK_LIBRARY_PATH:+:$DYLD_FALLBACK_LIBRARY_PATH}"
# The same for a dev build on Linux (`dynamic_linking`).
export LD_LIBRARY_PATH="$(dirname "$bin")/deps:$(rustc --print target-libdir)${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

fail=0
bad() { echo "FAIL  $*"; fail=1; }
ok() { echo "PASS  $*"; }

# Linux without a display: a virtual one on lavapipe.
wrap=()
if [ "$(uname)" = Linux ] && [ -z "${DISPLAY:-}" ]; then
  icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
  if [ -z "$icd" ] || ! command -v xvfb-run >/dev/null; then
    echo "error: no display, and xvfb-run or lavapipe is missing (apt-get install xvfb mesa-vulkan-drivers)" >&2
    exit 1
  fi
  wrap=(xvfb-run -a -s "-screen 0 1280x800x24" env VK_ICD_FILENAMES="$icd")
fi

# Run the game with "$@" into the log $1; sets `status`.
status=0
game() {
  local log=$1
  shift
  echo "+ ${wrap[*]:-} $bin $* > $log"
  status=0
  # `${wrap[@]+...}`: an empty array under `set -u` in macOS's bash 3.2.
  ${wrap[@]+"${wrap[@]}"} "$bin" "$@" >"$log" 2>&1 || status=$?
}

# No ERROR line in the log $1.
no_errors() {
  local errors
  errors=$(sed 's/\x1b\[[0-9;]*m//g' "$1" | grep -E '^[^ ]+ +ERROR ' || true)
  if [ -z "$errors" ]; then ok "no ERROR line in $1"; else bad "ERROR lines in $1:"; echo "$errors"; fi
}

fixture=crates/sim/tests/fixtures/m5_first_match.eonreplay
golden="$(tr -d '[:space:]' < crates/sim/tests/fixtures/m5_first_match.hash)"

echo "==> 1. sim-cli verify $fixture"
fixture_line="$("$simcli" verify "$fixture" 2>&1)"
echo "    $fixture_line"
case "$fixture_line" in
  "OK final_hash=$golden ticks="*) ok "the golden verifies to m5_first_match.hash $golden" ;;
  *) bad "the golden does not verify to $golden"; exit 1 ;;
esac
hash="${fixture_line#OK final_hash=}"
hash="${hash%% *}"
ticks="${fixture_line##* ticks=}"
from=$((ticks - 400))

echo
echo "==> 2. the overlay: --spectate --seed 1 --fast-forward-to $from, screenshot at tick $ticks"
o1="$out/overlay-replays"
rm -rf "$o1"; mkdir -p "$o1"
png="$out/m5b-overlay.png"
log="$out/overlay.log"
rm -f "$png"
game "$log" --background --spectate --seed 1 --fast-forward-to "$from" --replay-dir "$o1" \
  --camera 100,64,40 --screenshot-at "$ticks:$png" --exit-after-seconds "$overlay_seconds"
if [ "$status" -eq 0 ]; then ok "the game exited 0"; else bad "the game exited $status"; fi
no_errors "$log"
ff_line="$(grep '^fast_forward: ' "$log" || true)"
echo "    $ff_line"
case "$ff_line" in
  "fast_forward: tick=$from "*) ok "fast-forwarded to tick $from before the window opened" ;;
  *) bad "expected 'fast_forward: tick=$from wall_s=...'" ;;
esac
outcome_line="$(grep '^outcome: ' "$log" || true)"
echo "    $outcome_line"
case "$outcome_line" in
  "outcome: victory player="[01]" "*" tick=$ticks hash=$hash") ok "the overlay's outcome is at tick $ticks with the golden's hash" ;;
  *) bad "expected 'outcome: victory player=<0|1> ... tick=$ticks hash=$hash'" ;;
esac
replay="$(sed -n 's/^replay: //p' "$log" | head -1)"
echo "    replay: $replay"
if [ -s "$png" ]; then ok "overlay screenshot $png ($(wc -c < "$png" | tr -d ' ') bytes)"; else bad "no screenshot at tick $ticks ($png)"; fi
if [ -n "$replay" ] && [ -f "$replay" ]; then
  verify_out="$("$simcli" verify "$replay" 2>&1 || true)"
  echo "    $verify_out"
  if [ "$verify_out" = "OK final_hash=$hash ticks=$ticks" ]; then
    ok "the replay recorded from tick $from verifies to the overlay's hash"
  else
    bad "expected 'OK final_hash=$hash ticks=$ticks'"
  fi
else
  bad "no replay file"
fi

echo
echo "==> 3. the menu: a plain launch for 6 s, with a screenshot"
o2="$out/menu-replays"
rm -rf "$o2"; mkdir -p "$o2"
menu_png="$out/m5b-menu.png"
log="$out/menu.log"
rm -f "$menu_png"
game "$log" --background --replay-dir "$o2" --screenshot "$menu_png" --exit-after-seconds 6
if [ "$status" -eq 0 ]; then ok "the game exited 0"; else bad "the game exited $status"; fi
no_errors "$log"
if [ -s "$menu_png" ]; then ok "menu screenshot $menu_png ($(wc -c < "$menu_png" | tr -d ' ') bytes)"; else bad "no menu screenshot ($menu_png)"; fi
if grep -qE '^(replay|match): ' "$log"; then bad "the menu run started a match:"; grep -E '^(replay|match): ' "$log"; else ok "no match: and no replay: line"; fi
if [ -n "$(ls -A "$o2")" ]; then bad "the menu run wrote $(ls "$o2")"; else ok "no replay written while the menu was open"; fi

echo
echo "==> 4. New Game: the proxy clicks the real button 3 s in"
o3="$out/new-game-replays"
rm -rf "$o3"; mkdir -p "$o3"
log="$out/new-game.log"
game "$log" --background --seed 7 --replay-dir "$o3" --new-game-after-seconds 3 \
  --exit-after-seconds "$click_seconds"
if [ "$status" -eq 0 ]; then ok "the game exited 0"; else bad "the game exited $status"; fi
no_errors "$log"
for want in "new-game: clicked at " "match: new game vs standard personality=standard seed=7 map=plains_1v1"; do
  line="$(grep "^$want" "$log" | head -1 || true)"
  if [ -n "$line" ]; then ok "$line"; else bad "no '$want' line"; fi
done
replay="$(sed -n 's/^replay: //p' "$log" | head -1)"
echo "    replay: $replay"
if [ -n "$replay" ] && [ -f "$replay" ]; then
  verify_out="$("$simcli" verify "$replay" 2>&1 || true)"
  echo "    $verify_out"
  case "$verify_out" in
    "OK final_hash="*" ticks="*)
      n="${verify_out##* ticks=}"
      if [ "$n" -gt 600 ]; then
        ok "the New Game replay verifies with the Standard bot from its header ($n ticks)"
      else
        bad "the New Game replay has only $n ticks (want > 600)"
      fi
      ;;
    *) bad "expected 'OK final_hash=... ticks=...'" ;;
  esac
else
  bad "no replay file"
fi

echo
echo "==> 5. scripts/m2_checks.sh checks 3 and 4 (with --skip-menu)"
m2_status=0
${wrap[@]+"${wrap[@]}"} env ONLY="3 4" M2_CHECKS_BIN="$bin" M2_CHECKS_SIMCLI="$simcli" \
  scripts/m2_checks.sh "$out/m2" >"$out/m2.log" 2>&1 || m2_status=$?
# Each verdict once (m2_checks prints it as it goes and again in its summary).
grep -E '^(PASS|FAIL)  check' "$out/m2.log" | awk '!seen[$0]++' | sed 's/^/    /' || true
if [ "$m2_status" -eq 0 ]; then ok "m2_checks 3 and 4"; else bad "m2_checks 3 and 4 (log $out/m2.log)"; fi

echo
echo "==> 6. no game process left"
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if pgrep -x eonmark >/dev/null || pgrep -x Xvfb >/dev/null; then sleep 1; else break; fi
done
if pgrep -x eonmark >/dev/null; then
  bad "eonmark still running:"; pgrep -xl eonmark
else
  ok "pgrep -x eonmark is empty"
fi
if [ ${#wrap[@]} -gt 0 ]; then
  if pgrep -x Xvfb >/dev/null; then bad "Xvfb still running:"; pgrep -xl Xvfb; else ok "pgrep -x Xvfb is empty"; fi
fi

echo
if [ "$fail" -ne 0 ]; then
  echo "m5b_checks: FAIL ($out)"
  exit 1
fi
echo "m5b_checks: ok, m5_first_match decided at tick $ticks with hash $hash; menu and New Game proxies pass ($out)"
