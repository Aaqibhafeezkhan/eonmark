#!/usr/bin/env bash
# draw_trace_xcheck.sh: cross-check the draw-call counter
# (crates/game/src/draw_stats.rs) against Bevy's own record of every draw.
#
# With the `bevy/detailed_trace` feature, Bevy 0.19.1's TrackedRenderPass
# logs each draw at TRACE level (bevy_render::render_phase::draw_state):
# `draw:`, `draw indexed:`, `draw indirect:` and `draw indexed indirect:`
# are one draw each, `multi draw ... indirect: <buffer> <offset>, <n>x` is n,
# and `multi draw ... indirect count: ..., max <n>x` is at most n. The
# counter logs every counted frame at DEBUG level (`draw_stats: frame <f>
# total=<t> ...`, target game::draw_stats) in PrepareBindGroups, before
# that frame's passes are encoded, so the trace lines between two counter
# lines are that frame's draws. This script builds the game once with the
# feature, runs each scenario under Xvfb on lavapipe twice -- in the default
# mode (`mode=culling` on lavapipe) and with
# EONMARK_WGPU_DISABLE=indirect_first_instance (`mode=preprocessing_only`,
# where every view draws directly) -- and requires, per run, that the
# counter's sum over its counted frames equals the trace's sum over the
# same frames within one frame's draws (the last frame may be cut short by
# the exit), and that the run reported the expected mode. Full-screen
# passes (tonemapping, upscaling) use raw wgpu passes and appear in
# neither count.
#
# Usage: scripts/draw_trace_xcheck.sh <out-dir> [scenarios...]
#   scenarios default to `unit_looks art_bench`.
# Environment:
#   XCHECK_TARGET_DIR  cargo --target-dir for the trace build; the binary
#                      lands in <dir>/ci/eonmark (default target/trace, a
#                      second copy of Bevy, about 4 GB). `target`, the
#                      repository's own dir, builds into target/ci beside
#                      the normal artifacts and replaces target/ci/eonmark,
#                      so run `cargo build -p game --profile ci` afterwards
#                      to put the normal binary back. Not `target/ci`: that
#                      nests a new target/ci/ci.
#   XCHECK_SECONDS     --exit-after-seconds per run (default 40 for
#                      unit_looks, 90 for art_bench, 60 otherwise).
# Needs what scripts/render_smoke.sh needs (xvfb-run, lavapipe). Local only;
# kept for the Bevy 0.20 migration (ADR 0002's checklist).
set -euo pipefail

usage="usage: scripts/draw_trace_xcheck.sh <out-dir> [scenarios...]"
out="${1:?$usage}"
shift
scenarios=("$@")
if [ "${#scenarios[@]}" -eq 0 ]; then
  scenarios=(unit_looks art_bench)
fi
target_dir="${XCHECK_TARGET_DIR:-target/trace}"

icd=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
if [ -z "$icd" ]; then
  echo "error: lavapipe not found (apt-get install mesa-vulkan-drivers)" >&2
  exit 1
fi
if ! command -v xvfb-run >/dev/null; then
  echo "error: xvfb-run not found (apt-get install xvfb)" >&2
  exit 1
fi

mkdir -p "$out"
echo "==> building game with bevy/detailed_trace into $target_dir"
cargo build -p game --locked --profile ci --features bevy/detailed_trace \
  --target-dir "$target_dir"
bin="$out/eonmark-trace"
cp "$target_dir/ci/eonmark" "$bin"

# Sum one run's log: per counted frame, the counter's total and the trace's
# draws, then the totals. Prints `frames= equal= counter_sum= trace_sum=
# max_frame= worst_diff=`.
summarise() {
  sed 's/\x1b\[[0-9;]*m//g' "$1" | awk '
    function flush() {
      if (frame == "") return
      frames++
      counter_sum += counter
      trace_sum += trace
      if (counter == trace) equal++
      d = counter - trace; if (d < 0) d = -d
      if (d > worst) worst = d
      if (counter > max_frame) max_frame = counter
    }
    /draw_stats: frame [0-9]+ total=/ {
      flush()
      match($0, /frame [0-9]+/); frame = substr($0, RSTART + 6, RLENGTH - 6)
      match($0, /total=[0-9]+/); counter = substr($0, RSTART + 6, RLENGTH - 6) + 0
      trace = 0
      next
    }
    / bevy_render::render_phase::draw_state: / {
      if (frame == "") next
      msg = $0; sub(/.* bevy_render::render_phase::draw_state: /, "", msg)
      if (msg ~ /^multi draw .*count: .*max [0-9]+x$/) {
        match(msg, /max [0-9]+x$/); trace += substr(msg, RSTART + 4, RLENGTH - 5)
      } else if (msg ~ /^multi draw .*, [0-9]+x$/) {
        match(msg, /, [0-9]+x$/); trace += substr(msg, RSTART + 2, RLENGTH - 3)
      } else if (msg ~ /^draw( indexed)?( indirect)?: /) {
        trace += 1
      }
    }
    END {
      flush()
      printf "frames=%d equal=%d counter_sum=%d trace_sum=%d max_frame=%d worst_diff=%d\n", \
        frames, equal, counter_sum, trace_sum, max_frame, worst
    }'
}

fail=0
for scenario in "${scenarios[@]}"; do
  case "$scenario" in
    unit_looks) seconds="${XCHECK_SECONDS:-40}" ;;
    art_bench) seconds="${XCHECK_SECONDS:-90}" ;;
    *) seconds="${XCHECK_SECONDS:-60}" ;;
  esac
  for mode in culling preprocessing_only; do
    disable=""
    if [ "$mode" = preprocessing_only ]; then disable=indirect_first_instance; fi
    log="$out/$scenario-$mode.log"
    echo "==> $scenario, $mode ($seconds s)"
    status=0
    xvfb-run -a -s "-screen 0 1280x800x24" env VK_ICD_FILENAMES="$icd" \
      EONMARK_WGPU_DISABLE="$disable" \
      RUST_LOG="warn,bevy_render::render_phase::draw_state=trace,game::draw_stats=debug" \
      "$bin" --background --scenario "$scenario" --draw-stats \
      --exit-after-seconds "$seconds" --replay-dir "$out" >"$log" 2>&1 || status=$?
    draws=$(grep -m1 '^frame_stats: draws ' "$log" || true)
    echo "    $draws"
    sums=$(summarise "$log")
    echo "    $sums"
    got_mode=$(sed -n 's/^frame_stats: draws .* mode=\([a-z_]*\).*/\1/p' "$log" | head -n 1)
    counter_sum=$(sed -n 's/.* counter_sum=\([0-9]*\) .*/\1/p' <<<"$sums")
    trace_sum=$(sed -n 's/.* trace_sum=\([0-9]*\) .*/\1/p' <<<"$sums")
    max_frame=$(sed -n 's/.* max_frame=\([0-9]*\) .*/\1/p' <<<"$sums")
    frames=$(sed -n 's/^frames=\([0-9]*\) .*/\1/p' <<<"$sums")
    diff=$((counter_sum - trace_sum))
    if [ "$diff" -lt 0 ]; then diff=$((-diff)); fi
    verdict=PASS
    if [ "$status" -ne 0 ]; then verdict="FAIL (exit $status)"; fi
    if [ "${frames:-0}" -eq 0 ]; then verdict="FAIL (no counted frame)"; fi
    if [ "$got_mode" != "$mode" ]; then verdict="FAIL (mode=$got_mode, wanted $mode)"; fi
    if [ "$diff" -gt "${max_frame:-0}" ]; then verdict="FAIL (sums differ by $diff, more than one frame's $max_frame)"; fi
    echo "draw_trace_xcheck: $scenario mode=$got_mode $sums diff=$diff $verdict"
    if [ "$verdict" != PASS ]; then fail=1; fi
  done
done
if pgrep -f "$bin" >/dev/null; then
  echo "error: a game process is still running" >&2
  fail=1
fi
exit "$fail"
