#!/usr/bin/env bash
# fun_gate.sh: ROADMAP M6 acceptance 1 over a `sim-cli play-bots` CSV
# (docs/design/ai.md, "The M6 fun gate"), with the owner's readings of
# 2026-10-28 (M6 owner decision 1):
#
#   a winner        winner is a or b (territory-tiebreak wins count); a
#                   draw or a panic fails
#   decisive        reason == seat_captured and tick < 48000
#   median          the mean of the two middle ticks of `sort -n` (the
#                   middle one for an odd count), at most 54000
#   past 24000      tick > 24000
#
# The literal line plays 20 games and needs >= 10/20 decisive and >= 10/20
# past 24000; for another count the bar is half the games, rounded up.
# The first line is the gate:
#
#   fun_gate: games=20 winners=20 draws=0 panics=0 decisive_lt_48000=N median_tick=M past_24000=K tiebreaks=T
#
# then diagnostics (not gates): wins and mean tick per personality pair,
# the seat split, the distinct (winner, tick) pairs per pairing, the
# interquartile range of first_attack_tick and, given the play-bots stderr
# file, the largest per-game ai_ms_mean.
#
# Usage: scripts/fun_gate.sh <csv> [stderr-file]
# Exit 0 when every clause holds, 1 when one fails, 2 on usage errors.
set -euo pipefail

if [ $# -lt 1 ] || [ $# -gt 2 ]; then
  echo "usage: scripts/fun_gate.sh <csv> [stderr-file]" >&2
  exit 2
fi
csv="$1"
err="${2:-}"
[ -r "$csv" ] || { echo "fun_gate: cannot read $csv" >&2; exit 2; }
if [ -n "$err" ] && [ ! -r "$err" ]; then
  echo "fun_gate: cannot read $err" >&2
  exit 2
fi
for c in seed winner outcome tick reason first_attack_tick personality_a personality_b; do
  if ! head -n 1 "$csv" | tr ',' '\n' | grep -qx "$c"; then
    echo "fun_gate: no column $c in $csv" >&2
    exit 2
  fi
done

# col <name>: the 1-based index of a header column.
col() { head -n 1 "$csv" | tr ',' '\n' | grep -nx "$1" | cut -d: -f1; }
tick_col=$(col tick)
attack_col=$(col first_attack_tick)

# The middle of the sorted ticks, and the quartiles of first_attack_tick
# (nearest rank), with `sort -n` (mawk has no asort).
median=$(tail -n +2 "$csv" | cut -d, -f"$tick_col" | grep . | sort -n | awk '
  { v[NR] = $1 }
  END {
    if (NR == 0) { print "none"; exit }
    if (NR % 2) print v[(NR + 1) / 2]
    else printf "%g\n", (v[NR / 2] + v[NR / 2 + 1]) / 2
  }')
iqr=$(tail -n +2 "$csv" | cut -d, -f"$attack_col" | grep . | sort -n | awk '
  { v[NR] = $1 }
  END {
    if (NR == 0) { print "none"; exit }
    q1 = int((NR + 3) / 4); q3 = int((3 * NR + 3) / 4)
    if (q1 < 1) q1 = 1
    if (q3 > NR) q3 = NR
    printf "%d (q1=%d q3=%d n=%d)\n", v[q3] - v[q1], v[q1], v[q3], NR
  }')

status=0
awk -F, -v median="$median" -v iqr="$iqr" '
  NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
  {
    games++
    w = $col["winner"]; t = $col["tick"] + 0
    pair = $col["personality_a"] "/" $col["personality_b"]
    if (w == "a" || w == "b") winners++
    if (w == "draw") draws++
    if (w == "panic") panics++
    if ($col["reason"] == "seat_captured" && $col["tick"] != "" && t < 48000) decisive++
    if ($col["tick"] != "" && t > 24000) past++
    if ($col["outcome"] == "tiebreak") tiebreaks++
    if (!(pair in seen)) { seen[pair] = 1; order[++npairs] = pair }
    n[pair]++; sum[pair] += t
    if (w == "a") wa[pair]++
    if (w == "b") wb[pair]++
    key = pair SUBSEP w ":" $col["tick"]
    if (!(key in distinct)) { distinct[key] = 1; ndistinct[pair]++ }
    seat[w]++
  }
  END {
    games += 0; winners += 0; draws += 0; panics += 0; decisive += 0; past += 0; tiebreaks += 0
    need = int((games + 1) / 2)
    printf "fun_gate: games=%d winners=%d draws=%d panics=%d decisive_lt_48000=%d median_tick=%s past_24000=%d tiebreaks=%d\n", games, winners, draws, panics, decisive, median, past, tiebreaks
    fail = ""
    if (games == 0) fail = fail " no games;"
    if (winners < games) fail = fail " " games - winners " game(s) without a winner;"
    if (decisive < need) fail = fail " decisive_lt_48000 " decisive " < " need ";"
    if (median == "none" || median + 0 > 54000) fail = fail " median_tick " median " > 54000;"
    if (past < need) fail = fail " past_24000 " past " < " need ";"
    for (i = 1; i <= npairs; i++) {
      p = order[i]
      printf "pair %s: games=%d a=%d b=%d mean_tick=%d distinct_winner_tick=%d\n", p, n[p], wa[p], wb[p], sum[p] / n[p], ndistinct[p]
    }
    printf "seats: a=%d b=%d draw=%d none=%d panic=%d\n", seat["a"], seat["b"], seat["draw"], seat["none"], seat["panic"]
    printf "first_attack_tick_iqr: %s\n", iqr
    if (fail != "") { print "fun_gate: FAILED:" fail; exit 1 }
    printf "fun_gate: ok (need %d decisive before 48000 and %d past 24000 of %d)\n", need, need, games
  }
' "$csv" || status=$?
if [ -n "$err" ]; then
  awk '
    /^game seed=/ {
      for (i = 1; i <= NF; i++) if ($i ~ /^ai_ms_mean=/) { v = substr($i, 12) + 0; if (v > max) { max = v; at = $2 } }
      n++
    }
    END { if (n) printf "ai_ms_mean_max: %.4f (%s, %d games)\n", max, at, n; else print "ai_ms_mean_max: no game lines" }
  ' "$err"
fi
exit "$status"
