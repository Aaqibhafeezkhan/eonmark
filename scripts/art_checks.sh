#!/usr/bin/env bash
# art_checks.sh: the automated proxies of the art pass's acceptance list
# (ROADMAP "Art pass (pre-stream)"), and the producer of its four sign-off
# screenshots and the dev-panel one. `just art-checks` builds the binaries
# and runs it; it is a local, PR-pasted check like scripts/m5b_checks.sh.
# The real-input checks (AP6's minimap, AP10's clicks) are
# scripts/art_drive.sh.
#
#   1. `sim-cli data-check data` prints OK (rules_version, rules_hash, the
#      visuals counts and models=<n>).
#   2. AP2, the swap walkthrough of docs/ART_STYLE.md on a scratch copy of
#      data/: the "yeoman" line of visuals.ron becomes
#      `Primitive(Capsule)`. The copy differs from data/ in that one line
#      only, its data-check prints OK with the same rules_hash and one model
#      fewer, and every golden fixture verifies OK against it (ADR 0003:
#      visuals.ron is not in rules_hash). The checkout is not touched.
#   3. AP3: `inspect_gltf check data assets` prints `check: ok <n>
#      scene(s)`.
#   4. AP1: `scripts/check_assets.sh` and `scripts/check_assets.sh
#      --self-test` pass.
#   5. art-units-40m.png: `--scenario unit_looks` (its own 40 m framing),
#      tick 100.
#   6. art-town-40m.png: `--scenario art_bench --camera 24,64,40` (P0's
#      Seat, its buildings, the sites and the forests at 40 m), tick 300.
#   7. art-battle-60m.png: `--scenario art_bench` in its own framing
#      (60 m over P0's Town and the 200 vs 200 fight, every P0 unit
#      selected at tick 40), tick 300, mid-fight. AP4's gate:
#      scripts/check_draws.sh --fight (draws max < 500, and the fight
#      measured: a non-zero fight_max under 500 and a `frame_stats: fight`
#      line with fight frames and ticks); the triangles, fight and minimap
#      lines are printed, and the minimap's paint p95 must be at most
#      500 us (AP6).
#   8. art-match-minimap.png: `--spectate --seed 1` (the m5_first_match
#      setup) fast-forwarded to T/2, with a screenshot 200 ticks later,
#      where T is the tick `sim-cli verify` of the m5_first_match golden
#      prints: a match in progress with its minimap.
#   9. art-dev-panel.png (AP7): the dev build's Sim panel with its `draw
#      calls` row over `--scenario art_bench`, tick 250 (the counter skips
#      the first 30 frames, which on lavapipe last past tick 100). Skipped
#      (not failed) when no dev binary is given.
#   Every game run of steps 5 to 9 uses --background (docs/IMPLEMENTER_NOTES.md
#   rule 25), --window-size 1280x720 (docs/ART_STYLE.md's screenshot
#   size), --draw-stats and --screenshot-at, and must exit 0 with no ERROR
#   line, save its PNG and print its `frame_stats: draws` line under 500;
#   steps 5 to 8 also `sim-cli verify` the replay the run recorded.
#   10. No game and no Xvfb this script started is left: every game run
#      of steps 5 to 9 passes `--replay-dir <out-dir>`, the dev binary
#      (eonmark-dev) too, and every Xvfb is xvfb-run's with
#      `-auth <out-dir>/xvfb.auth`, so `pgrep -f` on those finds exactly
#      this run's processes. Neither a name match (`pgrep -x eonmark`,
#      which misses eonmark-dev and flags other sessions' games and
#      displays) nor `pgrep -f 'target/.*/eonmark'` (which matches this
#      script's own command line) would.
#
# On Linux without a display every game runs under `xvfb-run` on Mesa's
# lavapipe (1280 x 800 screen). There `art_bench` draws about one frame a
# second with its 400 character models, and the sim advances at most
# 250 ms a frame, so tick 300 comes about 85 s in: its runs last
# ART_BENCH_SECONDS (120). The screenshots are not copied anywhere; the PR copies
# the four sign-off shots and the dev-panel shot to docs/screenshots/.
#
# Usage: scripts/art_checks.sh <eonmark binary> <sim-cli binary> <inspect_gltf binary> [out-dir] [eonmark dev binary]
# Env:   ART_BENCH_SECONDS=120  how long each art_bench run lasts
#        ART_LOOKS_SECONDS=60   how long the unit_looks run lasts
#        ART_MATCH_SECONDS=90   how long the spectated match run lasts
#        ART_DEV_SECONDS=120    how long the dev-panel run lasts
set -euo pipefail

usage="usage: scripts/art_checks.sh <eonmark binary> <sim-cli binary> <inspect_gltf binary> [out-dir] [eonmark dev binary]"
bin="${1:?$usage}"
simcli="${2:?$usage}"
inspect="${3:?$usage}"
out="${4:-}"
devbin="${5:-}"
if [ -z "$out" ]; then
  out="$(mktemp -d "${TMPDIR:-/tmp}/eonmark-art-checks.XXXXXX")"
fi
mkdir -p "$out"
out="$(cd "$out" && pwd)"
abs() { echo "$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"; }
bin="$(abs "$bin")"
simcli="$(abs "$simcli")"
inspect="$(abs "$inspect")"
if [ -n "$devbin" ]; then devbin="$(abs "$devbin")"; fi
bench_seconds="${ART_BENCH_SECONDS:-120}"
looks_seconds="${ART_LOOKS_SECONDS:-60}"
match_seconds="${ART_MATCH_SECONDS:-90}"
dev_seconds="${ART_DEV_SECONDS:-120}"

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

# Background mode for every windowed run (rule 25).
export EONMARK_BACKGROUND=1

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
  # Its own auth file, so step 10 can tell this run's Xvfb from others.
  : >"$out/xvfb.auth"
  wrap=(xvfb-run -a -f "$out/xvfb.auth" -s "-screen 0 1280x800x24" env VK_ICD_FILENAMES="$icd")
fi

# $1 as an extended regular expression that matches only itself.
ere_quote() { printf '%s' "$1" | sed 's/[][\.*^$+?(){}|]/\\&/g'; }

# No ERROR line in the log $1.
no_errors() {
  local errors
  errors=$(sed 's/\x1b\[[0-9;]*m//g' "$1" | grep -E '^[^ ]+ +ERROR ' || true)
  if [ -z "$errors" ]; then ok "no ERROR line"; else bad "ERROR lines in $1:"; echo "$errors"; fi
}

# Run the game binary $1 with "${@:4}" into the log $2, saving the PNG $3:
# exit 0, no ERROR line, the PNG, and the draws line under 500
# (scripts/check_draws.sh; a non-empty $fight adds --fight, which requires
# that the fight was measured too). Sets `status` and `log`.
status=0
log=""
fight=""
run_game() {
  local game=$1 png=$3
  log=$2
  shift 3
  rm -f "$png"
  echo "+ ${wrap[*]:-} $game --background --window-size 1280x720 $* > $log"
  status=0
  # `${wrap[@]+...}`: an empty array under `set -u` in macOS's bash 3.2.
  ${wrap[@]+"${wrap[@]}"} "$game" --background --window-size 1280x720 "$@" >"$log" 2>&1 || status=$?
  if [ "$status" -eq 0 ]; then ok "the game exited 0"; else bad "the game exited $status"; fi
  no_errors "$log"
  if [ -s "$png" ]; then
    ok "screenshot $png ($(wc -c <"$png" | tr -d ' ') bytes)"
  else
    bad "no screenshot ($png)"
  fi
  grep -E '^frame_stats: (draws |triangles |fight |minimap |window logical=)' "$log" | sed 's/^/    /' || true
  local gate=() verdict
  if [ -n "$fight" ]; then gate=(--fight); fi
  if verdict=$(scripts/check_draws.sh ${gate[@]+"${gate[@]}"} "$log" 2>&1); then
    ok "$(tail -n 1 <<<"$verdict")"
  else
    bad "the draw-call gate:"
    sed 's/^/    /' <<<"$verdict"
  fi
}

# `sim-cli verify` of the replay the run whose log is $log recorded.
verify_run() {
  local replay verify_out
  replay=$(sed -n 's/^replay: //p' "$log" | head -n 1)
  if [ -n "$replay" ] && [ -f "$replay" ]; then
    verify_out=$("$simcli" verify "$replay" 2>&1 || true)
    echo "    $verify_out"
    case "$verify_out" in
      "OK final_hash="*) ok "the run's replay verifies ($(basename "$replay"))" ;;
      *) bad "sim-cli verify $replay did not print OK" ;;
    esac
  else
    bad "no replay: line or file in $log"
  fi
}

# The minimap's paint p95 in the log $log is at most 500 us (AP6).
minimap_p95() {
  local line p95
  line=$(grep '^frame_stats: minimap ' "$log" || true)
  p95=$(sed -n 's/.* p95=\([0-9.]*\) .*/\1/p' <<<"$line")
  if [ -n "$p95" ] && awk -v p="$p95" 'BEGIN { exit !(p <= 500) }'; then
    ok "minimap paint p95=$p95 us <= 500"
  else
    bad "minimap paint p95=${p95:-missing} us, want <= 500 ($line)"
  fi
}

echo "==> 1. sim-cli data-check data"
dc=$("$simcli" data-check data 2>&1) || true
echo "    $dc"
case "$dc" in
  "OK rules_version="*) ok "data-check" ;;
  *) bad "data-check did not print OK" ;;
esac
rules_hash=$(sed -n 's/.* rules_hash=\(0x[0-9a-f]*\) .*/\1/p' <<<"$dc")
models=$(sed -n 's/.* models=\([0-9]*\).*/\1/p' <<<"$dc")

echo
echo "==> 2. AP2: the Yeoman swap walkthrough on a scratch copy of data/"
w="$out/ap2"
rm -rf "$w"
mkdir -p "$w"
cp -R data "$w/data"
sed 's|^    "yeoman": Scene(.*|    "yeoman": Primitive(Capsule),|' data/visuals.ron >"$w/data/visuals.ron"
changed=$(diff -rq data "$w/data" || true)
lines=$(diff data/visuals.ron "$w/data/visuals.ron" | grep -c '^[<>]' || true)
echo "    $changed"
diff data/visuals.ron "$w/data/visuals.ron" | grep '^>' | sed 's/^/    /' || true
if [ "$(wc -l <<<"$changed")" -eq 1 ] && [[ "$changed" == *"visuals.ron"* ]] && [ "$lines" -eq 2 ]; then
  ok "the copy differs from data/ in one line of visuals.ron"
else
  bad "the swap changed more than the yeoman line"
fi
dc2=$("$simcli" data-check "$w/data" --assets assets 2>&1) || true
echo "    $dc2"
hash2=$(sed -n 's/.* rules_hash=\(0x[0-9a-f]*\) .*/\1/p' <<<"$dc2")
models2=$(sed -n 's/.* models=\([0-9]*\).*/\1/p' <<<"$dc2")
case "$dc2" in
  "OK rules_version="*) ok "data-check of the swapped copy" ;;
  *) bad "data-check of the swapped copy did not print OK" ;;
esac
if [ -n "$hash2" ] && [ "$hash2" = "$rules_hash" ]; then ok "rules_hash unchanged ($hash2)"; else bad "rules_hash moved: $rules_hash -> ${hash2:-missing}"; fi
if [ -n "$models" ] && [ -n "$models2" ] && [ "$models2" -eq $((models - 1)) ]; then
  ok "models=$models2, one fewer: the Yeoman's model is no longer loaded"
else
  bad "models=${models2:-missing}, want $((${models:-1} - 1))"
fi
verified=0
total=0
for f in crates/sim/tests/fixtures/*.eonreplay; do
  total=$((total + 1))
  v=$("$simcli" verify --data "$w/data" "$f" 2>&1 || true)
  echo "    $(basename "$f"): $v"
  case "$v" in "OK final_hash="*) verified=$((verified + 1)) ;; esac
done
if [ "$verified" -eq "$total" ] && [ "$total" -gt 0 ]; then
  ok "all $total golden fixtures verify against the swapped copy"
else
  bad "$verified of $total fixtures verify against the swapped copy"
fi

echo
echo "==> 3. AP3: inspect_gltf check data assets"
ic=$("$inspect" check data assets 2>&1) && ic_status=0 || ic_status=$?
grep -E '^ok units\[' <<<"$ic" | sed 's/^/    /' || true
summary=$(tail -n 1 <<<"$ic")
echo "    $summary"
if [ "$ic_status" -eq 0 ] && [[ "$summary" == "check: ok "* ]]; then ok "$summary"; else bad "inspect_gltf check (exit $ic_status)"; fi

echo
echo "==> 4. AP1: scripts/check_assets.sh and its --self-test"
if ca=$(scripts/check_assets.sh 2>&1); then ok "$(tail -n 1 <<<"$ca")"; else bad "check_assets.sh:"; echo "$ca"; fi
if st=$(scripts/check_assets.sh --self-test 2>&1); then
  grep -E 'unattributed|self-test' <<<"$st" | sed 's/^/    /' || true
  ok "check_assets.sh --self-test"
else
  bad "check_assets.sh --self-test:"
  echo "$st"
fi

echo
echo "==> 5. art-units-40m.png: unit_looks at 40 m, tick 100"
png="$out/art-units-40m.png"
fight=""
run_game "$bin" "$out/units.log" "$png" --scenario unit_looks --draw-stats \
  --replay-dir "$out" --exit-after-seconds "$looks_seconds" --screenshot-at "100:$png"
verify_run

echo
echo "==> 6. art-town-40m.png: art_bench at --camera 24,64,40, tick 300"
png="$out/art-town-40m.png"
fight=1
run_game "$bin" "$out/town.log" "$png" --scenario art_bench --camera 24,64,40 --draw-stats \
  --replay-dir "$out" --exit-after-seconds "$bench_seconds" --screenshot-at "300:$png"
verify_run

echo
echo "==> 7. art-battle-60m.png: art_bench in its framing, tick 300 (AP4's gate)"
png="$out/art-battle-60m.png"
fight=1
run_game "$bin" "$out/battle.log" "$png" --scenario art_bench --draw-stats \
  --replay-dir "$out" --exit-after-seconds "$bench_seconds" --screenshot-at "300:$png"
minimap_p95
verify_run

echo
echo "==> 8. art-match-minimap.png: --spectate --seed 1, fast-forwarded to T/2"
fixture_line=$("$simcli" verify crates/sim/tests/fixtures/m5_first_match.eonreplay 2>&1 || true)
echo "    m5_first_match: $fixture_line"
case "$fixture_line" in
  "OK final_hash="*" ticks="*)
    ticks="${fixture_line##* ticks=}"
    from=$((ticks / 2))
    at=$((from + 200))
    png="$out/art-match-minimap.png"
    fight=""
    run_game "$bin" "$out/match.log" "$png" --spectate --seed 1 --fast-forward-to "$from" --draw-stats \
      --replay-dir "$out" --exit-after-seconds "$match_seconds" --screenshot-at "$at:$png"
    ff=$(grep '^fast_forward: ' "$log" || true)
    echo "    $ff"
    case "$ff" in
      "fast_forward: tick=$from "*) ok "fast-forwarded to tick $from, screenshot at tick $at" ;;
      *) bad "expected 'fast_forward: tick=$from ...'" ;;
    esac
    minimap_p95
    verify_run
    ;;
  *) bad "the m5_first_match golden does not verify" ;;
esac

echo
echo "==> 9. art-dev-panel.png: the dev build's Sim panel over art_bench, tick 250"
if [ -z "$devbin" ]; then
  echo "SKIP  no dev binary given (just art-checks builds one)"
else
  # A `--features dev` binary links Bevy (`dynamic_linking`) and libstd
  # dynamically; `cargo run` would point the loader at both.
  libdir="$(rustc --print target-libdir)"
  deps="$(dirname "$devbin")/deps"
  LD_LIBRARY_PATH="$deps:$libdir${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  DYLD_FALLBACK_LIBRARY_PATH="$deps:$libdir${DYLD_FALLBACK_LIBRARY_PATH:+:$DYLD_FALLBACK_LIBRARY_PATH}"
  export LD_LIBRARY_PATH DYLD_FALLBACK_LIBRARY_PATH
  png="$out/art-dev-panel.png"
  fight=1
  run_game "$devbin" "$out/dev.log" "$png" --scenario art_bench \
    --replay-dir "$out" --exit-after-seconds "$dev_seconds" --screenshot-at "250:$png"
  grep '^hierarchy_check: ' "$log" | sed 's/^/    /' || true
fi

echo
echo "==> 10. no game process left"
games_re="--replay-dir $(ere_quote "$out")( |\$)"
xvfb_re="-auth $(ere_quote "$out/xvfb.auth")( |\$)"
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if pgrep -f -- "$games_re" >/dev/null || pgrep -f -- "$xvfb_re" >/dev/null; then sleep 1; else break; fi
done
if left=$(pgrep -f -- "$games_re"); then
  bad "a game this script started is still running:"
  ps -o pid=,args= -p "${left//$'\n'/,}" || true
else
  ok "no game this script started is running (pgrep -f -- '--replay-dir $out' is empty; eonmark and eonmark-dev alike)"
fi
if [ ${#wrap[@]} -gt 0 ]; then
  if left=$(pgrep -f -- "$xvfb_re"); then
    bad "an Xvfb this script started is still running:"
    ps -o pid=,args= -p "${left//$'\n'/,}" || true
  else
    ok "no Xvfb this script started is running (pgrep -f -- '-auth $out/xvfb.auth' is empty)"
  fi
fi

echo
ls -l "$out"/art-*.png 2>/dev/null | awk '{print "    " $5 " " $NF}' || true
if [ "$fail" -ne 0 ]; then
  echo "art_checks: FAIL ($out)"
  exit 1
fi
echo "art_checks: ok, every step passed; the screenshots are in $out"
