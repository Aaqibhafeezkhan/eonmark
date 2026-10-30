#!/usr/bin/env bash
# check_attack_before.sh: ROADMAP M6 acceptance 4, first clause, over a
# `sim-cli play-bots` CSV (docs/design/ai.md, "Difficulty table"): for every
# row the side's first attack-move into the enemy's land AND its first armed
# unit standing on that land come before tick N:
#
#   first_attack_tick[_b] < N and first_on_land_tick_[a|b] < N
#
# The second clause is stricter than the ROADMAP line (units on the
# player's land, not only the order). Columns are found by their header
# names; an empty column fails the row.
#
#   play-bots --seeds 1..3 --a passive --b hard --max-ticks 14400 --allow-undecided > h.csv
#   scripts/check_attack_before.sh --side b --before 14400 h.csv
#
# Usage: scripts/check_attack_before.sh --side a|b --before N <csv>
# Exit 0 when every row passes (and there is at least one), 1 otherwise,
# 2 on usage errors.
set -euo pipefail

side=
before=
csv=
while [ $# -gt 0 ]; do
  case "$1" in
    --side)
      [ $# -ge 2 ] || { echo "check_attack_before: --side needs a or b" >&2; exit 2; }
      side="$2"; shift 2 ;;
    --before)
      [ $# -ge 2 ] || { echo "check_attack_before: --before needs a tick" >&2; exit 2; }
      before="$2"; shift 2 ;;
    -*) echo "check_attack_before: unknown flag $1" >&2; exit 2 ;;
    *) csv="$1"; shift ;;
  esac
done
case "$side" in a|b) ;; *) echo "check_attack_before: --side is a or b" >&2; exit 2 ;; esac
case "$before" in ''|*[!0-9]*) echo "check_attack_before: --before is a tick" >&2; exit 2 ;; esac
if [ -z "$csv" ] || [ ! -r "$csv" ]; then
  echo "usage: scripts/check_attack_before.sh --side a|b --before N <csv>" >&2
  exit 2
fi

awk -F, -v side="$side" -v before="$before" '
  NR == 1 {
    for (i = 1; i <= NF; i++) col[$i] = i
    attack = (side == "b") ? "first_attack_tick_b" : "first_attack_tick"
    land = "first_on_land_tick_" side
    if (!(attack in col) || !(land in col)) { print "check_attack_before: no column " attack " or " land > "/dev/stderr"; bad = 2; exit }
    next
  }
  {
    rows++
    a = $col[attack]; l = $col[land]
    why = ""
    if (a == "") why = why " no attack"
    else if (a + 0 >= before) why = why " attack at " a
    if (l == "") why = why " never on enemy land"
    else if (l + 0 >= before) why = why " on enemy land at " l
    line = sprintf("seed %s side %s: %s=%s %s=%s", $1, side, attack, a, land, l)
    if (why == "") { print "ok   " line } else { print "FAIL " line ":" why " (not before " before ")"; bad = 1 }
  }
  END {
    if (bad == 2) exit 2
    if (rows == 0) { print "check_attack_before: no rows" > "/dev/stderr"; exit 1 }
    if (bad) { print "check_attack_before: FAILED"; exit 1 }
    printf "check_attack_before: %d/%d rows pass (side %s, before %d)\n", rows, rows, side, before
  }
' "$csv"
