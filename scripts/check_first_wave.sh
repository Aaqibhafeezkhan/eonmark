#!/usr/bin/env bash
# check_first_wave.sh: ROADMAP M5b acceptance 2 over a `sim-cli play-bots`
# CSV (docs/design/ai.md, "play-bots harness"). For every row, the side's
# columns must show it reaching Masonry and then attack-moving into the
# enemy's land with at least 8 units, a Supply Wain among them, before
# tick 18000:
#
#   age_reached_tick is set, age_reached_tick <= first_attack_tick < 18000,
#   first_attack_units >= 8 and first_attack_wains >= 1.
#
# With --decisive it also checks, for a game against Passive, owner
# decision 4's Seat capture (the side wins it decisively, reason
# seat_captured) and the plan's extra gates from the ROADMAP M5b Goal: its
# second Town and border Watchtower stand before tick 18000. Columns are
# found by their header names; side b reads the `_b` columns.
#
# Usage: scripts/check_first_wave.sh [--side a|b] [--decisive] <csv>
# Exit 0 when every row passes (and there is at least one), 1 otherwise,
# 2 on usage errors.
set -euo pipefail

side=a
decisive=0
csv=
while [ $# -gt 0 ]; do
  case "$1" in
    --side)
      [ $# -ge 2 ] || { echo "check_first_wave: --side needs a or b" >&2; exit 2; }
      side="$2"; shift 2 ;;
    --decisive) decisive=1; shift ;;
    -*) echo "check_first_wave: unknown flag $1" >&2; exit 2 ;;
    *) csv="$1"; shift ;;
  esac
done
case "$side" in a|b) ;; *) echo "check_first_wave: --side is a or b" >&2; exit 2 ;; esac
if [ -z "$csv" ] || [ ! -r "$csv" ]; then
  echo "usage: scripts/check_first_wave.sh [--side a|b] [--decisive] <csv>" >&2
  exit 2
fi

awk -F, -v side="$side" -v decisive="$decisive" '
  NR == 1 {
    for (i = 1; i <= NF; i++) col[$i] = i
    s = (side == "b") ? "_b" : ""
    # Side a reads the unsuffixed columns, side b the _b ones.
    need["age"] = (side == "b") ? "age_reached_tick_b" : "age_reached_tick"
    need["attack"] = "first_attack_tick" s
    need["units"] = "first_attack_units" s
    need["wains"] = "first_attack_wains" s
    need["town"] = "second_town_tick" s
    need["tower"] = "tower_tick" s
    for (k in need) if (!(need[k] in col)) { print "check_first_wave: no column " need[k] > "/dev/stderr"; bad = 2; exit }
    if (!("winner" in col) || !("outcome" in col) || !("reason" in col)) { print "check_first_wave: no winner/outcome/reason column" > "/dev/stderr"; bad = 2; exit }
    next
  }
  {
    rows++
    age = $col[need["age"]]; attack = $col[need["attack"]]
    units = $col[need["units"]]; wains = $col[need["wains"]]
    why = ""
    if (age == "") why = why " no Masonry"
    if (attack == "") why = why " no attack"
    if (age != "" && attack != "" && age + 0 > attack + 0) why = why " attack before Masonry"
    if (attack != "" && attack + 0 >= 18000) why = why " attack at " attack " (not before 18000)"
    if (units == "" || units + 0 < 8) why = why " " (units == "" ? 0 : units) " units (< 8)"
    if (wains == "" || wains + 0 < 1) why = why " no Supply Wain"
    if (decisive) {
      town = $col[need["town"]]; tower = $col[need["tower"]]
      if ($col["winner"] != side || $col["outcome"] != "decisive" || $col["reason"] != "seat_captured")
        why = why " not a decisive Seat capture by " side " (" $col["winner"] "," $col["outcome"] "," $col["reason"] ")"
      if (town == "" || town + 0 >= 18000) why = why " no second Town before 18000"
      if (tower == "" || tower + 0 >= 18000) why = why " no Watchtower before 18000"
    }
    line = sprintf("seed %s side %s: age_reached_tick=%s first_attack_tick=%s first_attack_units=%s first_attack_wains=%s", $1, side, age, attack, units, wains)
    if (why == "") { print "ok   " line } else { print "FAIL " line ":" why; bad = 1 }
  }
  END {
    if (bad == 2) exit 2
    if (rows == 0) { print "check_first_wave: no rows" > "/dev/stderr"; exit 1 }
    if (bad) { print "check_first_wave: FAILED"; exit 1 }
    printf "check_first_wave: %d/%d rows pass (side %s%s)\n", rows, rows, side, decisive ? ", decisive" : ""
  }
' "$csv"
