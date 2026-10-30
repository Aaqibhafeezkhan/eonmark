#!/usr/bin/env bash
# check_ordering.sh: ROADMAP M6 acceptance 2 over a `sim-cli play-bots` CSV
# (docs/design/ai.md, "Difficulty table"): count the rows whose winner is
# the given side and require at least <min> of them. Only `winner == side`
# counts; a draw, an undecided game or a panic counts for nobody.
#
#   play-bots --a easy --b standard --seeds 1..20 > e_s.csv
#   scripts/check_ordering.sh e_s.csv b 15
#
# A second line reads the games that reached the 54000-tick cap: how many
# were draws and tiebreaks, how many of them were untouched (neither side
# took a Town) and how many of those ended within 3 tiles of territory.
# Two intact Standard-bot seats build mirror images (mirror-aware site
# ties), so an untouched cap game is an exact-tie Draw or a 3-tile
# tiebreak that the bot's site noise decides, not play (docs/design/ai.md,
# "Difficulty table (as tuned)"). The line is information; it never
# changes the exit status.
#
# Usage: scripts/check_ordering.sh <csv> <a|b> <min>
# Exit 0 when the side won at least <min> games, 1 otherwise (or with no
# rows), 2 on usage errors.
set -euo pipefail

if [ $# -ne 3 ]; then
  echo "usage: scripts/check_ordering.sh <csv> <a|b> <min>" >&2
  exit 2
fi
csv="$1"; side="$2"; min="$3"
case "$side" in a|b) ;; *) echo "check_ordering: the side is a or b" >&2; exit 2 ;; esac
case "$min" in ''|*[!0-9]*) echo "check_ordering: <min> is a whole number" >&2; exit 2 ;; esac
[ -r "$csv" ] || { echo "check_ordering: cannot read $csv" >&2; exit 2; }

awk -F, -v side="$side" -v min="$min" '
  NR == 1 {
    for (i = 1; i <= NF; i++) col[$i] = i
    if (!("winner" in col)) { print "check_ordering: no winner column" > "/dev/stderr"; bad = 2; exit }
    next
  }
  {
    rows++
    w = $col["winner"]
    won[w]++
    if (w == side) seeds = seeds " " $1
    if (("outcome" in col) && ($col["outcome"] == "tiebreak" || $col["outcome"] == "draw")) {
      cap++
      if ("towns_taken_a" in col && "towns_taken_b" in col && $col["towns_taken_a"] == 0 && $col["towns_taken_b"] == 0) {
        untouched++
        if ("territory_a" in col && "territory_b" in col) {
          gap = $col["territory_a"] - $col["territory_b"]
          if (gap < 0) gap = -gap
          if (gap <= 3) close3++
        }
      }
    }
  }
  END {
    if (bad == 2) exit 2
    if (rows == 0) { print "check_ordering: no rows" > "/dev/stderr"; exit 1 }
    n = won[side] + 0
    printf "check_ordering: %s won %d/%d (need %d; a=%d b=%d draw=%d none=%d panic=%d) seeds:%s\n", side, n, rows, min, won["a"], won["b"], won["draw"], won["none"], won["panic"], seeds
    printf "check_ordering: cap games %d (draws %d), untouched (no Town taken) %d, of them within 3 tiles %d\n", cap, won["draw"], untouched, close3
    if (n < min) { print "check_ordering: FAILED"; exit 1 }
  }
' "$csv"
