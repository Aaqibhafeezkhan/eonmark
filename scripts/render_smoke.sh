#!/usr/bin/env bash
# render_smoke.sh: run the windowed game on a virtual X display with Mesa's
# software Vulkan driver (lavapipe) and fail on any render error.
#
# Bevy compiles WGSL only when it builds a pipeline. A shader that fails to
# compile is logged (`ERROR bevy_render::render_resource::pipeline_cache:
# failed to process shader`) while the game runs on without that mesh and
# exits 0, so no build, lint or unit test catches it; a shader whose
# `#import` names a missing module is retried silently, and the game's
# `render_watchdog` logs an ERROR for any pipeline still not ready 20 s
# after it started waiting. This run checks:
#   1. the game exits 0;
#   2. a PNG captured once the sim reaches a tick exists (the window
#      presented frames up to that tick; it says nothing about pipelines);
#   3. the log has no ERROR line, which covers both shader cases above.
# The game runs in background mode, as every automated windowed check does
# (docs/IMPLEMENTER_NOTES.md rule 25).
#
# Needs xvfb (xvfb-run), mesa-vulkan-drivers (lavapipe) and the X libraries
# winit loads at run time (libxkbcommon-x11-0, libx11-xcb1, libxcursor1,
# libxi6, libxrandr2); CI's game-linux job installs them. Usage:
# scripts/render_smoke.sh <eonmark binary> [out-dir] (out-dir keeps the PNG,
# the log and the replay; default a temp dir).
set -euo pipefail

bin="${1:?usage: scripts/render_smoke.sh <eonmark binary> [out-dir]}"
out="${2:-$(mktemp -d)}"
# Ticks run in real time while frames take under 250 ms; lavapipe on a CI
# runner can be slower, so the tick is early and the deadline generous.
tick=100
seconds=60

icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
if [ -z "$icd" ]; then
  echo "error: lavapipe not found (apt-get install mesa-vulkan-drivers)" >&2
  exit 1
fi
if ! command -v xvfb-run >/dev/null; then
  echo "error: xvfb-run not found (apt-get install xvfb)" >&2
  exit 1
fi
# winit opens these with dlopen and panics when one is missing. The list is
# read once: `ldconfig -p | grep -q` can fail under pipefail on SIGPIPE.
libs=$("$(command -v ldconfig || echo /sbin/ldconfig)" -p)
for lib in libxkbcommon-x11.so.0:libxkbcommon-x11-0 libX11-xcb.so.1:libx11-xcb1 \
  libXcursor.so.1:libxcursor1 libXi.so.6:libxi6 libXrandr.so.2:libxrandr2; do
  if ! grep -qF "${lib%%:*}" <<<"$libs"; then
    echo "error: ${lib%%:*} not found (apt-get install ${lib##*:})" >&2
    exit 1
  fi
done

mkdir -p "$out"
png="$out/render_smoke.png"
log="$out/render_smoke.log"
rm -f "$png"

status=0
xvfb-run -a -s "-screen 0 1280x800x24" env VK_ICD_FILENAMES="$icd" \
  "$bin" --background --scenario economy_walkthrough --camera 38,64,60 \
  --exit-after-seconds "$seconds" --screenshot-at "$tick:$png" \
  --replay-dir "$out" >"$log" 2>&1 || status=$?

fail=0
if [ "$status" -ne 0 ]; then
  echo "error: the game exited $status" >&2
  fail=1
fi
if [ ! -s "$png" ]; then
  echo "error: no screenshot at tick $tick within $seconds s ($png)" >&2
  fail=1
fi
# Strip colour codes; tracing prints the level as a padded word.
errors=$(sed 's/\x1b\[[0-9;]*m//g' "$log" | grep -E '^[^ ]+ +ERROR ' || true)
if [ -n "$errors" ]; then
  echo "error: the log has ERROR lines:" >&2
  echo "$errors" >&2
  fail=1
fi
if [ "$fail" -ne 0 ]; then
  echo "--- $log (last 40 lines) ---" >&2
  tail -n 40 "$log" >&2
  exit 1
fi
echo "render_smoke: ok ($png)"
