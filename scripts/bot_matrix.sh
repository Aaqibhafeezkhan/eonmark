#!/usr/bin/env bash
# bot_matrix.sh: the M6 tuning matrix (docs/design/ai.md, "Tuning"; the
# plan's loop A): play a set of personality pairings with `sim-cli
# play-bots` and print the boring and frustrating proxies per pairing.
#
#   --set distinct   every ordered distinct pair of --personalities (6 for
#                    rush,boom,tower), Standard difficulty on both seats
#   --set standard   each of --personalities against standard, both seat
#                    orders
#   --set mirrors    each of --personalities against itself (seat bias)
#
# One play-bots run per pairing over --seeds (default 1..5) at --jobs 4,
# --max-ticks 72000. The CSVs and stderr files stay under --out (default a
# fresh scratch directory); `all.csv` concatenates them. Per pairing:
#
#   B1 distinct (winner, tick) pairs; B2 A's win share; B3 median
#   first_contact_tick; B4 tiebreaks and the longest max_quiet_ticks; B5
#   mean first_attack_tick and second_town_tick per side; B6 decisive
#   before tick 18000; F1 wiped/waves; F2 retreats/waves and the games in
#   which the loser drove a Town of the winner below 500 per mille; F3 the
#   bonus share of each side; F5 draws; F6 rejected commands and empty
#   bo_done_tick columns of a side still alive.
#
# Usage: scripts/bot_matrix.sh [--set distinct|standard|mirrors] [--seeds A..B]
#        [--personalities a,b,c] [--difficulty ID] [--data D] [--jobs N] [--out DIR]
# SIM_CLI names the sim-cli binary (default target/release/sim-cli).
# Exit 0, or 2 on usage errors and when play-bots rejects its arguments.
set -euo pipefail

set_name=distinct
seeds=1..5
personalities=rush,boom,tower
difficulty=standard
data=data
jobs=4
out=
while [ $# -gt 0 ]; do
  [ $# -ge 2 ] || { echo "bot_matrix: $1 needs a value" >&2; exit 2; }
  case "$1" in
    --set) set_name="$2" ;;
    --seeds) seeds="$2" ;;
    --personalities) personalities="$2" ;;
    --difficulty) difficulty="$2" ;;
    --data) data="$2" ;;
    --jobs) jobs="$2" ;;
    --out) out="$2" ;;
    *) echo "bot_matrix: unknown flag $1" >&2; exit 2 ;;
  esac
  shift 2
done
sim_cli="${SIM_CLI:-target/release/sim-cli}"
if [ ! -x "$sim_cli" ]; then
  cargo build --release --locked -p sim-cli
fi
[ -n "$out" ] || out=$(mktemp -d "${TMPDIR:-/tmp}/bot_matrix.XXXXXX")
mkdir -p "$out"
IFS=, read -r -a ps <<< "$personalities"

pairs=()
case "$set_name" in
  distinct)
    for b in "${ps[@]}"; do for a in "${ps[@]}"; do [ "$a" = "$b" ] || pairs+=("$a/$b"); done; done ;;
  standard)
    for p in "${ps[@]}"; do pairs+=("$p/standard" "standard/$p"); done ;;
  mirrors)
    for p in "${ps[@]}"; do pairs+=("$p/$p"); done ;;
  *) echo "bot_matrix: --set is distinct, standard or mirrors" >&2; exit 2 ;;
esac

start=$(date +%s)
rm -f "$out/all.csv"
for pair in "${pairs[@]}"; do
  a="${pair%/*}"; b="${pair#*/}"
  name="${a}_vs_${b}"
  code=0
  "$sim_cli" --data "$data" play-bots --seeds "$seeds" --a "$difficulty" --b "$difficulty" \
    --personality-a "$a" --personality-b "$b" --max-ticks 72000 --jobs "$jobs" --allow-undecided \
    > "$out/$name.csv" 2> "$out/$name.err" || code=$?
  if [ "$code" = 2 ]; then
    echo "bot_matrix: play-bots refused $pair:" >&2
    tail -n 3 "$out/$name.err" >&2
    exit 2
  fi
  if [ -f "$out/all.csv" ]; then tail -n +2 "$out/$name.csv" >> "$out/all.csv"; else cp "$out/$name.csv" "$out/all.csv"; fi
done
end=$(date +%s)

awk -F, '
  NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
  function v(name) { return (name in col) ? $col[name] : "" }
  function num(x) { return x == "" ? 0 : x + 0 }
  {
    p = v("personality_a") "/" v("personality_b")
    if (!(p in n)) order[++np] = p
    n[p]++
    w = v("winner"); t = v("tick")
    if (w == "a") wa[p]++
    if (w == "b") wb[p]++
    if (w == "draw") dr[p]++
    if (v("outcome") == "tiebreak") tb[p]++
    if (v("reason") == "seat_captured" && num(t) < 18000) early[p]++
    k = p SUBSEP w ":" t
    if (!(k in seen)) { seen[k] = 1; distinct[p]++ }
    fc = v("first_contact_tick"); if (fc != "") { fcs[p] = fcs[p] " " fc }
    q = num(v("max_quiet_ticks")); if (q > quiet[p]) quiet[p] = q
    if (v("first_attack_tick") != "") { fa[p] += v("first_attack_tick"); nfa[p]++ }
    if (v("first_attack_tick_b") != "") { fb[p] += v("first_attack_tick_b"); nfb[p]++ }
    if (v("second_town_tick") != "") { sa[p] += v("second_town_tick"); nsa[p]++ }
    if (v("second_town_tick_b") != "") { sb[p] += v("second_town_tick_b"); nsb[p]++ }
    waves[p] += num(v("waves_a")) + num(v("waves_b"))
    wiped[p] += num(v("wiped_a")) + num(v("wiped_b"))
    retreats[p] += num(v("retreats_a")) + num(v("retreats_b"))
    # F2: the loser drove a Town the winner started with below 500 per mille.
    if (w == "a" && num(v("min_own_town_hp_permille_a")) < 500) pressed[p]++
    if (w == "b" && num(v("min_own_town_hp_permille_b")) < 500) pressed[p]++
    bon_a[p] += num(v("bonus_a")); inc_a[p] += num(v("income_a"))
    bon_b[p] += num(v("bonus_b")); inc_b[p] += num(v("income_b"))
    rej[p] += num(v("rejected_a")) + num(v("rejected_b"))
    if (v("bo_done_tick_a") == "" && w != "b") stall[p]++
    if (v("bo_done_tick_b") == "" && w != "a") stall[p]++
  }
  function mean(s, c) { return c ? sprintf("%d", s / c) : "-" }
  function median(list,   a, m, i, j, x) {
    m = split(list, a, " ")
    if (m == 0) return "-"
    for (i = 2; i <= m; i++) { x = a[i] + 0; for (j = i - 1; j >= 1 && a[j] + 0 > x; j--) a[j + 1] = a[j]; a[j + 1] = x }
    return (m % 2) ? a[(m + 1) / 2] : sprintf("%d", (a[m / 2] + a[m / 2 + 1]) / 2)
  }
  function share(b, i) { return (b + i) ? sprintf("%d", b * 1000 / (b + i)) : "-" }
  END {
    printf "%-20s %5s %7s %4s %3s %5s %4s %6s %6s %6s %6s %6s %6s %3s %8s %9s %9s %7s %6s %4s %5s\n", "pair a/b", "games", "a:b", "B1", "B2%", "B3", "B4tb", "B4quiet", "B5atkA", "B5atkB", "B5twnA", "B5twnB", "B6", "F5", "F1wiped", "F2retreat", "F2pressed", "F3a/b", "F6rej", "F6st", ""
    for (i = 1; i <= np; i++) {
      p = order[i]
      printf "%-20s %5d %3d:%-3d %4d %3d %5s %4d %6d %6s %6s %6s %6s %6d %3d %4d/%-3d %4d/%-4d %4d/%-4d %3s/%-3s %6d %4d\n", p, n[p], wa[p], wb[p], distinct[p], n[p] ? wa[p] * 100 / n[p] : 0, median(fcs[p]), tb[p], quiet[p], mean(fa[p], nfa[p]), mean(fb[p], nfb[p]), mean(sa[p], nsa[p]), mean(sb[p], nsb[p]), early[p], dr[p], wiped[p], waves[p], retreats[p], waves[p], pressed[p], n[p], share(bon_a[p], inc_a[p]), share(bon_b[p], inc_b[p]), rej[p], stall[p]
    }
  }
' "$out/all.csv"
echo "bot_matrix: set=$set_name seeds=$seeds pairings=${#pairs[@]} difficulty=$difficulty wall_s=$((end - start)) out=$out"
