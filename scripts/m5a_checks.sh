#!/usr/bin/env bash
# m5a_checks.sh: the automated proxy of ROADMAP M5a acceptance line 3
# ("`cargo run -p game --features dev -- --scenario capture_seat` shows the
# victory overlay with the final hash and the recorded replay verifies to
# it"). `just m5a-checks` builds both binaries and runs it; it is a local,
# PR-pasted check, not a CI job (M5a owner decision 14).
#
#   1. `sim-cli verify` of the committed golden
#      crates/sim/tests/fixtures/capture_seat.eonreplay prints
#      `OK final_hash=H ticks=T`, and H is capture_seat.hash.
#   2. The game runs `--scenario capture_seat` in background mode
#      (docs/IMPLEMENTER_NOTES.md rule 25) with a screenshot at tick T. It
#      must exit 0 with no ERROR line in its log, print
#      `outcome: victory player=0 decisive=true reason=seat_captured
#      tick=T hash=H` and a `replay:` path, and the PNG must exist: the
#      frame that first shows the overlay over the frozen world.
#   3. `sim-cli verify` of the game's own replay prints the same
#      `OK final_hash=H ticks=T`, with no truncation warning: the recorder
#      finished at the outcome.
#   4. No eonmark process is left (`pgrep -x eonmark`).
#
# On Linux without a display the game runs under `xvfb-run` on Mesa's
# lavapipe (as scripts/render_smoke.sh does). The sim keeps its 20 ticks
# a second there while frames stay under 250 ms (`Time<Virtual>`'s
# max_delta): on the 4-vCPU VM lavapipe drew about 5 frames a second at
# 4 ticks a frame, so the 1628 ticks took about 82 s, as at full rate,
# and the default 150 s is enough. Set M5A_SECONDS=240 only as a margin
# on a slower or loaded host.
#
# Usage: scripts/m5a_checks.sh <eonmark binary> <sim-cli binary> [out-dir]
# Env:   M5A_SECONDS=150  how long the game runs (--exit-after-seconds)
set -euo pipefail

usage="usage: scripts/m5a_checks.sh <eonmark binary> <sim-cli binary> [out-dir]"
bin="${1:?$usage}"
simcli="${2:?$usage}"
out="${3:-}"
if [ -z "$out" ]; then
  out="$(mktemp -d "${TMPDIR:-/tmp}/eonmark-m5a-checks.XXXXXX")"
fi
mkdir -p "$out"
out="$(cd "$out" && pwd)"
seconds="${M5A_SECONDS:-150}"

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

# Background mode for the windowed run (rule 25).
export EONMARK_BACKGROUND=1

# A `--features dev` binary on macOS links libstd through @rpath without an
# LC_RPATH entry; `cargo run` would set this. Ignored on Linux.
export DYLD_FALLBACK_LIBRARY_PATH="$(rustc --print target-libdir)${DYLD_FALLBACK_LIBRARY_PATH:+:$DYLD_FALLBACK_LIBRARY_PATH}"
# The same for a dev build on Linux (`dynamic_linking`).
export LD_LIBRARY_PATH="$(dirname "$bin")/deps:$(rustc --print target-libdir)${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

fail=0
bad() { echo "FAIL  $*"; fail=1; }
ok() { echo "PASS  $*"; }

fixture=crates/sim/tests/fixtures/capture_seat.eonreplay
golden="$(tr -d '[:space:]' < crates/sim/tests/fixtures/capture_seat.hash)"

echo "==> 1. sim-cli verify $fixture"
fixture_line="$("$simcli" verify "$fixture" 2>&1)"
echo "    $fixture_line"
case "$fixture_line" in
  "OK final_hash=$golden ticks="*) ok "the golden verifies to capture_seat.hash $golden" ;;
  *) bad "the golden does not verify to $golden"; exit 1 ;;
esac
hash="${fixture_line#OK final_hash=}"
hash="${hash%% *}"
ticks="${fixture_line##* ticks=}"

echo
echo "==> 2. the game: --scenario capture_seat, overlay screenshot at tick $ticks"
png="$out/m5a-victory-overlay.png"
log="$out/game.log"
rm -f "$png"
run=("$bin" --background --scenario capture_seat --replay-dir "$out"
  --screenshot-at "$ticks:$png" --exit-after-seconds "$seconds")
if [ "$(uname)" = Linux ] && [ -z "${DISPLAY:-}" ]; then
  icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
  if [ -z "$icd" ] || ! command -v xvfb-run >/dev/null; then
    echo "error: no display, and xvfb-run or lavapipe is missing (apt-get install xvfb mesa-vulkan-drivers)" >&2
    exit 1
  fi
  run=(xvfb-run -a -s "-screen 0 1280x800x24" env VK_ICD_FILENAMES="$icd" "${run[@]}")
fi
echo "+ ${run[*]} > $log"
status=0
"${run[@]}" >"$log" 2>&1 || status=$?
if [ "$status" -eq 0 ]; then ok "the game exited 0"; else bad "the game exited $status"; fi
errors=$(sed 's/\x1b\[[0-9;]*m//g' "$log" | grep -E '^[^ ]+ +ERROR ' || true)
if [ -z "$errors" ]; then ok "no ERROR line in $log"; else bad "ERROR lines:"; echo "$errors"; fi
want="outcome: victory player=0 decisive=true reason=seat_captured tick=$ticks hash=$hash"
outcome_line="$(grep '^outcome: ' "$log" || true)"
echo "    $outcome_line"
if [ "$outcome_line" = "$want" ]; then ok "the overlay's outcome and hash are the golden's"; else bad "expected '$want'"; fi
replay="$(sed -n 's/^replay: //p' "$log" | head -1)"
echo "    replay: $replay"
if [ -s "$png" ]; then ok "overlay screenshot $png"; else bad "no screenshot at tick $ticks ($png)"; fi

echo
echo "==> 3. sim-cli verify the game's replay"
if [ -n "$replay" ] && [ -f "$replay" ]; then
  verify_out="$("$simcli" verify "$replay" 2>&1 || true)"
  echo "    $verify_out"
  if [ "$verify_out" = "OK final_hash=$hash ticks=$ticks" ]; then
    ok "the recorded replay verifies to the overlay's hash"
  else
    bad "expected 'OK final_hash=$hash ticks=$ticks' (a warning line means the trailer is missing)"
  fi
else
  bad "no replay file"
fi

echo
echo "==> 4. no game process left"
if pgrep -x eonmark >/dev/null; then
  bad "eonmark still running:"; pgrep -xl eonmark
else
  ok "pgrep -x eonmark is empty"
fi

echo
if [ "$fail" -ne 0 ]; then
  echo "m5a_checks: FAIL (log $log)"
  exit 1
fi
echo "m5a_checks: ok, capture_seat decided at tick $ticks with hash $hash ($out)"
