#!/usr/bin/env bash
# check_draws.sh: the draw-call gate of the art pass (ROADMAP "Art pass
# (pre-stream)", AP4): read the `frame_stats: draws` exit line that a run
# with --draw-stats printed (crates/game/src/draw_stats.rs) and fail unless
# its run maximum, and its fight-window maximum when there is one, is under
# the budget (500 draw calls a frame, docs/design/ui.md "Draw calls").
#
#   frame_stats: draws frames=75 mean=105.8 p95=109 max=109 indirect_max=97
#     direct_max=0 sorted_max=12 mode=culling fight_max=109
#
# The line must exist, count at least one frame and name a GPU
# preprocessing mode other than `none` (GL only, where the counter would
# miss the indirect draws; never one of our targets).
#
# --fight makes the gate prove that the fight's frames were measured
# (AP4's heaviest frames: 200 vs 200 with projectiles, bars and lunges).
# `fight_max` is the largest draw count of a counted frame inside the
# fight window, so it is never above `max` and `fight_max < 500` alone
# adds nothing; and the game prints `fight_max=` whenever the scenario has
# a fight window (`art_bench` and the `enemy_outpost` pair), as 0 when no
# counted frame fell inside it. So with --fight the field must be there and
# non-zero (a counted frame always draws at least the HUD), and the run's
# `frame_stats: fight` line must show fight frames (`frames=` > 0) and the
# fight's ticks (`ticks=<first>..<last>`, not `none`): the armies fought
# and the counter saw it.
# CI's game-linux job runs this over every render smoke's log, and
# scripts/art_checks.sh over its `art_bench` run.
#
# Usage: scripts/check_draws.sh [--fight] <log> [budget]
#   budget defaults to 500; a frame passes with fewer draws than that.
set -euo pipefail

usage="usage: scripts/check_draws.sh [--fight] <log> [budget]"
need_fight=0
if [ "${1:-}" = "--fight" ]; then
  need_fight=1
  shift
fi
log="${1:?$usage}"
budget="${2:-500}"
case "$budget" in
  '' | *[!0-9]*) echo "error: budget must be a whole number, got '$budget'" >&2; exit 2 ;;
esac
if [ ! -f "$log" ]; then
  echo "error: no log at $log" >&2
  exit 2
fi

line=$(grep '^frame_stats: draws ' "$log" | tail -n 1 || true)
if [ -z "$line" ]; then
  echo "check_draws: FAIL no 'frame_stats: draws' line in $log (was --draw-stats passed?)"
  exit 1
fi
echo "$line"

# The value of field $1 (`name=value`), matched as a whole word so that
# `max` never reads `indirect_max`.
field() {
  local want=$1 word
  for word in $line; do
    if [ "${word%%=*}" = "$want" ]; then
      echo "${word#*=}"
      return
    fi
  done
}

# A whole number, or nothing when the field is missing or malformed.
number() {
  case "$1" in
    '' | *[!0-9]*) echo "" ;;
    *) echo "$1" ;;
  esac
}

fail=0
fight_ticks=""
fight_frames=""
frames=$(number "$(field frames)")
max=$(number "$(field max)")
mode=$(field mode)
fight_raw=$(field fight_max)
fight=$(number "$fight_raw")
if [ -n "$fight_raw" ] && [ -z "$fight" ]; then
  echo "check_draws: FAIL fight_max=$fight_raw is not a number"
  fail=1
fi
if [ -z "$frames" ] || [ "$frames" -eq 0 ]; then
  echo "check_draws: FAIL no counted frame (frames=${frames:-missing})"
  fail=1
fi
if [ -z "$mode" ] || [ "$mode" = none ]; then
  echo "check_draws: FAIL mode=${mode:-missing}: the counter does not count that mode (GL only), so the gate fails rather than miscount"
  fail=1
fi
if [ -z "$max" ] || [ "$max" -ge "$budget" ]; then
  echo "check_draws: FAIL max=${max:-missing}, budget < $budget"
  fail=1
fi
if [ -n "$fight" ] && [ "$fight" -ge "$budget" ]; then
  echo "check_draws: FAIL fight_max=$fight, budget < $budget"
  fail=1
fi
if [ "$need_fight" -eq 1 ]; then
  if [ -z "$fight_raw" ]; then
    echo "check_draws: FAIL no fight_max field (not a scenario with a fight window)"
    fail=1
  elif [ "$fight" = 0 ]; then
    echo "check_draws: FAIL fight_max=0: no counted frame fell inside the fight window (the armies never fought, or the counter missed it)"
    fail=1
  fi
  fight_line=$(grep '^frame_stats: fight ' "$log" | tail -n 1 || true)
  if [ -z "$fight_line" ]; then
    echo "check_draws: FAIL no 'frame_stats: fight' line in $log"
    fail=1
  else
    echo "$fight_line"
    line=$fight_line
    fight_frames=$(number "$(field frames)")
    fight_ticks=$(field ticks)
    if [ -z "$fight_frames" ] || [ "$fight_frames" -eq 0 ] || [ -z "$fight_ticks" ] || [ "$fight_ticks" = none ]; then
      echo "check_draws: FAIL the fight line shows no fight (frames=${fight_frames:-missing} ticks=${fight_ticks:-missing})"
      fail=1
    fi
  fi
fi
if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "check_draws: ok max=$max${fight:+ fight_max=$fight} < $budget mode=$mode${fight_ticks:+ fight ticks=$fight_ticks frames=$fight_frames} ($log)"
