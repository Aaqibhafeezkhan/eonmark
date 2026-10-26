#!/usr/bin/env bash
# m2_checks.sh: the M2 acceptance proxies (docs/ROADMAP.md, "M2"), end to end
# against the dev build. The verifiers and the owner run this; `just
# m2-checks` wraps it.
#
#   1. headless replay of the move_500 fixture: last stdout line is
#      `tick=1200 hash=<move_500.hash>`, exit 0, wall time under 10 s. Timed
#      twice: the dev binary (perl wall clock) and the `--profile ci` binary
#      the ROADMAP names (/usr/bin/time -p; M2_CHECKS_CI_PROFILE=0 skips it).
#   2. `--scenario scripted_moves` at `--max-fps 30` and `--max-fps 120`
#      (35 s each): both replays verify OK, and `sim-cli hash-dump --every 1`
#      of the two agrees tick for tick over their common prefix, which must
#      cover the whole 600-tick script.
#   3. `--skip-menu --exit-after-seconds 5`: exit 0 and the replay verifies
#      OK with no truncation warning (clean-exit trailer present).
#   4. a `--skip-menu` windowed run killed with `kill -9` after 8 s:
#      `sim-cli verify` prints a truncation `warning:` and still `OK`.
#   Checks 3 and 4 pass --skip-menu since M5b: a plain launch opens on the
#   main menu, where the sim does not step and nothing is recorded.
#   5. `--scenario hud_click`: exit 0 and `hud_click: ok` on stdout.
#   6. `--scenario units200_auto` (65 s): exit 0, `frame_stats:` lines on
#      stdout, replay verifies OK, frame-time p95 under 16.7 ms, and the
#      window was never fully covered (macOS presents nothing for a covered
#      window, so its frame times would not measure rendering).
#
# Every windowed run uses background mode (EONMARK_BACKGROUND=1, exported
# below): the window opens unfocused, below other windows, in the top-left
# corner of the primary monitor, and the game hands focus back to the app
# that had it, so the checks never pop up in front of the owner's work.
#
# Every step echoes its command and tees output under the scratch dir. The
# script prints PASS/FAIL per check and exits 1 if any check failed.
#
# Usage: scripts/m2_checks.sh [scratch-dir]
# Env:   ONLY="1 3 5"           run a subset of the checks
#        M2_CHECKS_CI_PROFILE=0 skip building and timing the --profile ci binary in check 1
#        UNITS200_SECONDS=65    how long check 6 runs
#        CARGO_TARGET_DIR       honoured (default: <repo>/target)
#        M2_CHECKS_BIN, M2_CHECKS_SIMCLI
#                               use these eonmark and sim-cli binaries instead of
#                               building the dev ones (scripts/m5b_checks.sh runs
#                               checks 3 and 4 on its own binaries this way)
set -euo pipefail

# rustup is keg-only in Homebrew; make cargo visible without a shell profile.
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

scratch="${1:-}"
if [ -z "$scratch" ]; then
  scratch="$(mktemp -d "${TMPDIR:-/tmp}/eonmark-m2-checks.XXXXXX")"
fi
mkdir -p "$scratch"
scratch="$(cd "$scratch" && pwd)"

target_dir="${CARGO_TARGET_DIR:-$root/target}"
bin="${M2_CHECKS_BIN:-$target_dir/debug/eonmark}"
simcli="${M2_CHECKS_SIMCLI:-$target_dir/debug/sim-cli}"

# A `--features dev` binary links libstd dynamically through @rpath without an
# LC_RPATH entry; `cargo run` adds this directory to the dyld fallback path for
# its child. The checks exec the binary directly instead (so `kill -9` hits the
# game, not cargo), so set it here. Ignored on Linux.
export DYLD_FALLBACK_LIBRARY_PATH="$(rustc --print target-libdir)${DYLD_FALLBACK_LIBRARY_PATH:+:$DYLD_FALLBACK_LIBRARY_PATH}"
# The same for a dev build on Linux (`dynamic_linking`).
export LD_LIBRARY_PATH="$(dirname "$bin")/deps:$(rustc --print target-libdir)${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

# Background mode for every windowed run (docs/IMPLEMENTER_NOTES.md: automated
# and agent-run windowed checks always use it; manual playtests do not).
export EONMARK_BACKGROUND=1

# macOS: keep the display (and the system) awake until this script exits. The
# game survives a display sleep (crates/game/src/monitor_loss.rs), but a
# sleeping display shows nothing, so frame times from that span would not
# measure rendering and check 6 would fail on its own terms. `-w $$` releases
# the assertion when this shell exits, however it exits.
if command -v caffeinate >/dev/null 2>&1; then
  caffeinate -d -i -w $$ </dev/null >/dev/null 2>&1 &
fi

# Wall clock in seconds with centiseconds (macOS `date` has no %N; perl ships).
now() { perl -MTime::HiRes=time -e 'printf "%.2f", time'; }
only="${ONLY:-1 2 3 4 5 6}"
units200_seconds="${UNITS200_SECONDS:-65}"
fixture=crates/sim/tests/fixtures/move_500.eonreplay
fixture_hash="$(tr -d '[:space:]' < crates/sim/tests/fixtures/move_500.hash)"
# sim::scenarios::SCRIPTED_MOVES_TICKS: the two replays must agree at least this far.
scripted_moves_ticks=600

results=()
failures=0
bg_pid=""

cleanup() {
  if [ -n "$bg_pid" ] && kill -0 "$bg_pid" 2>/dev/null; then
    echo "cleanup: killing eonmark pid $bg_pid" >&2
    kill -9 "$bg_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT

step() { echo; echo "==> check $1: $2"; }
say() { echo "    $*"; }
pass() { results+=("PASS  check $1: $2"); echo "PASS  check $1: $2"; }
fail() { results+=("FAIL  check $1: $2"); echo "FAIL  check $1: $2"; failures=$((failures + 1)); }
wants() { case " $only " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# Run the game: background it, poll, and `kill -9` once the deadline passes
# so no window is ever left behind. Sets `last_status` (137 after a kill);
# stdout and stderr go to the log.
last_status=0
windowed() {
  local deadline=$1 log=$2
  shift 2
  echo "+ $bin $*  (deadline ${deadline}s) > $log"
  "$bin" "$@" >"$log" 2>&1 &
  bg_pid=$!
  local waited=0
  while kill -0 "$bg_pid" 2>/dev/null; do
    if [ "$waited" -ge "$deadline" ]; then
      echo "deadline of ${deadline}s passed; kill -9 $bg_pid" >&2
      kill -9 "$bg_pid" 2>/dev/null || true
    fi
    sleep 1
    waited=$((waited + 1))
  done
  last_status=0
  wait "$bg_pid" || last_status=$?
  bg_pid=""
  say "exit status $last_status after ${waited}s"
}

# The replay a windowed run wrote: the `replay: <path>` stdout line, or the
# newest .eonreplay under the run's --replay-dir.
replay_from() {
  local log=$1 dir=$2 path
  path="$(sed -n 's/^replay: //p' "$log" | head -n 1)"
  if [ -z "$path" ] || [ ! -f "$path" ]; then
    path="$(ls -t "$dir"/*.eonreplay 2>/dev/null | head -n 1 || true)"
  fi
  printf '%s' "$path"
}

# sim-cli verify into a log; sets verify_status. stdout and stderr are kept
# apart so the `warning:` tests (checks 3 and 4) read only stderr.
verify_status=0
verify() {
  local replay=$1 log=$2
  echo "+ $simcli verify $replay > $log (stderr: $log.err)"
  verify_status=0
  "$simcli" verify "$replay" >"$log" 2>"$log.err" || verify_status=$?
  sed 's/^/    verify: /' "$log" "$log.err"
}

verify_ok() { [ "$verify_status" -eq 0 ] && grep -q '^OK' "$1"; }

echo "m2_checks: repo $root"
echo "m2_checks: scratch $scratch"
echo "m2_checks: target $target_dir"
echo "m2_checks: checks: $only"

echo
echo "==> build"
if [ -n "${M2_CHECKS_BIN:-}" ] || [ -n "${M2_CHECKS_SIMCLI:-}" ]; then
  echo "using the given binaries: $bin, $simcli"
else
  echo "+ cargo build -p game --features dev --locked"
  cargo build -p game --features dev --locked 2>&1 | tail -n 3
  echo "+ cargo build -p sim-cli --locked"
  cargo build -p sim-cli --locked 2>&1 | tail -n 3
fi
test -x "$bin" || { echo "error: $bin missing after the build" >&2; exit 2; }
test -x "$simcli" || { echo "error: $simcli missing after the build" >&2; exit 2; }

# --- 1. headless fixture replay --------------------------------------------
headless_real=""
headless_ok=0
time_headless() {
  # $1 binary, $2 label; sets headless_real and headless_ok
  local exe=$1 label=$2 status=0 last log tlog t0 t1
  log="$scratch/01-headless-$label.txt"
  tlog="$scratch/01-time-$label.txt"
  if [ "$label" = ci ]; then
    echo "+ /usr/bin/time -p $exe --headless-run $fixture > $log"
    /usr/bin/time -p -o "$tlog" "$exe" --headless-run "$fixture" >"$log" 2>"$log.err" || status=$?
    headless_real="$(awk '/^real/ { print $2 }' "$tlog")"
  else
    # /usr/bin/time is SIP-protected: dyld strips DYLD_* from its environment
    # before it execs the child, so the dev binary cannot start under it.
    echo "+ $exe --headless-run $fixture > $log  (perl wall clock)"
    t0="$(now)"
    "$exe" --headless-run "$fixture" >"$log" 2>"$log.err" || status=$?
    t1="$(now)"
    headless_real="$(awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.2f", b - a }')"
    printf 'real %s\n' "$headless_real" >"$tlog"
  fi
  last="$(tail -n 1 "$log" 2>/dev/null || true)"
  say "$label: exit $status, real ${headless_real:-?} s, last line: ${last:-<none>}"
  if [ -s "$log.err" ]; then sed 's/^/    stderr: /' "$log.err" | tail -n 5; fi
  headless_ok=1
  [ "$status" -eq 0 ] || { say "$label: exit status $status"; headless_ok=0; }
  [ "$last" = "tick=1200 hash=$fixture_hash" ] || { say "$label: expected 'tick=1200 hash=$fixture_hash'"; headless_ok=0; }
  if [ -z "$headless_real" ] || ! awk -v r="$headless_real" 'BEGIN { exit !(r + 0 <= 10) }'; then
    say "$label: over the 10 s budget"; headless_ok=0
  fi
}

if wants 1; then
  step 1 "headless replay of $fixture (budget 10 s)"
  time_headless "$bin" dev
  ok=$headless_ok
  msg="dev ${headless_real:-?} s"
  if [ "${M2_CHECKS_CI_PROFILE:-1}" != "0" ]; then
    echo "+ cargo build -p game --profile ci --locked"
    cargo build -p game --profile ci --locked 2>&1 | tail -n 3
    time_headless "$target_dir/ci/eonmark" ci
    msg="$msg, ci ${headless_real:-?} s (/usr/bin/time)"
    [ "$headless_ok" -eq 1 ] || ok=0
  fi
  if [ "$ok" -eq 1 ]; then pass 1 "headless move_500 replay: $msg, final line matches move_500.hash"; else fail 1 "headless move_500 replay ($msg); see $scratch/01-*"; fi
fi

# --- 2. scripted_moves at 30 and 120 fps -----------------------------------
if wants 2; then
  step 2 "scripted_moves at --max-fps 30 and 120 (35 s each): identical hashes"
  ok=1
  for fps in 30 120; do
    dir="$scratch/02-replays-$fps"; mkdir -p "$dir"
    windowed 80 "$scratch/02-run-$fps.txt" --scenario scripted_moves --max-fps "$fps" --exit-after-seconds 35 --replay-dir "$dir"
    [ "$last_status" -eq 0 ] || { say "fps $fps: exit $last_status"; ok=0; }
    replay="$(replay_from "$scratch/02-run-$fps.txt" "$dir")"
    if [ -z "$replay" ]; then say "fps $fps: no replay written"; ok=0; continue; fi
    say "fps $fps: replay $replay"
    verify "$replay" "$scratch/02-verify-$fps.txt"
    verify_ok "$scratch/02-verify-$fps.txt" || { say "fps $fps: verify not OK"; ok=0; }
    echo "+ $simcli hash-dump $replay --every 1 > $scratch/02-dump-$fps.txt"
    "$simcli" hash-dump "$replay" --every 1 >"$scratch/02-dump-$fps.txt" 2>"$scratch/02-dump-$fps.txt.err" || { say "fps $fps: hash-dump failed"; ok=0; }
  done
  common=0
  if [ "$ok" -eq 1 ]; then
    n30="$(wc -l < "$scratch/02-dump-30.txt" | tr -d ' ')"
    n120="$(wc -l < "$scratch/02-dump-120.txt" | tr -d ' ')"
    common=$(( n30 < n120 ? n30 : n120 ))
    say "hash-dump lines: fps30=$n30 fps120=$n120 common=$common (need >= $scripted_moves_ticks)"
    if [ "$common" -lt "$scripted_moves_ticks" ]; then say "the sessions did not both reach tick $scripted_moves_ticks"; ok=0; fi
    if ! cmp -s <(head -n "$common" "$scratch/02-dump-30.txt") <(head -n "$common" "$scratch/02-dump-120.txt"); then
      say "hash-dump prefixes differ; first difference:"
      diff <(head -n "$common" "$scratch/02-dump-30.txt") <(head -n "$common" "$scratch/02-dump-120.txt") | head -n 6 | sed 's/^/    /'
      ok=0
    fi
    say "fps30  final: $(grep '^OK' "$scratch/02-verify-30.txt" || true)"
    say "fps120 final: $(grep '^OK' "$scratch/02-verify-120.txt" || true)"
    if cmp -s <(grep '^OK' "$scratch/02-verify-30.txt" | sed 's/ ticks=.*//') <(grep '^OK' "$scratch/02-verify-120.txt" | sed 's/ ticks=.*//'); then
      say "final hashes identical"
    else
      say "final hashes differ (the sessions ran different tick counts; the common prefix is the gate)"
    fi
  fi
  if [ "$ok" -eq 1 ]; then pass 2 "scripted_moves 30 vs 120 fps: hashes identical over $common ticks"; else fail 2 "scripted_moves 30 vs 120 fps; see $scratch/02-*"; fi
fi

# --- 3. clean exit ---------------------------------------------------------
if wants 3; then
  step 3 "--skip-menu --exit-after-seconds 5: exit 0, replay verifies OK without a warning"
  ok=1
  dir="$scratch/03-replays"; mkdir -p "$dir"
  windowed 45 "$scratch/03-run.txt" --skip-menu --exit-after-seconds 5 --replay-dir "$dir"
  [ "$last_status" -eq 0 ] || { say "exit $last_status"; ok=0; }
  replay="$(replay_from "$scratch/03-run.txt" "$dir")"
  if [ -z "$replay" ]; then say "no replay written"; ok=0; else
    say "replay $replay"
    verify "$replay" "$scratch/03-verify.txt"
    verify_ok "$scratch/03-verify.txt" || { say "verify not OK"; ok=0; }
    # Only the truncation warning counts. Since M5b verify rebuilds the
    # plain launch's Standard bot from the header and prints no other warning.
    if grep -q 'warning:.*no clean-exit trailer' "$scratch/03-verify.txt.err"; then say "verify reports no clean-exit trailer"; ok=0; fi
  fi
  if [ "$ok" -eq 1 ]; then pass 3 "clean exit: $(grep '^OK' "$scratch/03-verify.txt")"; else fail 3 "clean exit; see $scratch/03-*"; fi
fi

# --- 4. kill -9 ------------------------------------------------------------
if wants 4; then
  step 4 "--skip-menu, kill -9 after 8 s: replay verifies OK with a truncation warning"
  ok=1
  dir="$scratch/04-replays"; mkdir -p "$dir"
  log="$scratch/04-run.txt"
  echo "+ $bin --skip-menu --exit-after-seconds 60 --replay-dir $dir > $log &  (kill -9 after 8 s)"
  "$bin" --skip-menu --exit-after-seconds 60 --replay-dir "$dir" >"$log" 2>&1 &
  bg_pid=$!
  sleep 8
  if kill -0 "$bg_pid" 2>/dev/null; then
    kill -9 "$bg_pid"
  else
    say "the game exited before the kill"; ok=0
  fi
  last_status=0
  wait "$bg_pid" || last_status=$?
  bg_pid=""
  say "exit status $last_status (137 = killed)"
  [ "$last_status" -eq 137 ] || ok=0
  replay="$(replay_from "$log" "$dir")"
  if [ -z "$replay" ]; then say "no replay written"; ok=0; else
    say "replay $replay ($(wc -c < "$replay" | tr -d ' ') bytes)"
    verify "$replay" "$scratch/04-verify.txt"
    verify_ok "$scratch/04-verify.txt" || { say "verify not OK"; ok=0; }
    grep -q 'warning:.*no clean-exit trailer' "$scratch/04-verify.txt.err" || { say "verify printed no truncation warning"; ok=0; }
  fi
  if [ "$ok" -eq 1 ]; then pass 4 "kill -9: truncated replay verifies: $(grep '^OK' "$scratch/04-verify.txt")"; else fail 4 "kill -9; see $scratch/04-*"; fi
fi

# --- 5. hud_click ----------------------------------------------------------
if wants 5; then
  step 5 "--scenario hud_click: exit 0 and 'hud_click: ok'"
  ok=1
  dir="$scratch/05-replays"; mkdir -p "$dir"
  windowed 45 "$scratch/05-run.txt" --scenario hud_click --exit-after-seconds 10 --replay-dir "$dir"
  [ "$last_status" -eq 0 ] || { say "exit $last_status"; ok=0; }
  verdict="$(grep '^hud_click: ' "$scratch/05-run.txt" | head -n 1 || true)"
  say "verdict: ${verdict:-<none>}"
  [ "$verdict" = "hud_click: ok" ] || ok=0
  if [ "$ok" -eq 1 ]; then pass 5 "hud_click: ok"; else fail 5 "hud_click: ${verdict:-no verdict line}; see $scratch/05-run.txt"; fi
fi

# --- 6. units200_auto ------------------------------------------------------
if wants 6; then
  step 6 "--scenario units200_auto (${units200_seconds} s): frame_stats on exit, replay verifies"
  ok=1
  dir="$scratch/06-replays"; mkdir -p "$dir"
  windowed $((units200_seconds + 45)) "$scratch/06-run.txt" --scenario units200_auto --exit-after-seconds "$units200_seconds" --replay-dir "$dir"
  [ "$last_status" -eq 0 ] || { say "exit $last_status"; ok=0; }
  if grep -q '^frame_stats:' "$scratch/06-run.txt"; then
    grep '^frame_stats:' "$scratch/06-run.txt" | sed 's/^/    /'
  else
    say "no frame_stats: lines on stdout"; ok=0
  fi
  # docs/PLAYTEST.md's gate for this proxy: p95 under 16.7 ms (60 fps).
  p95="$(sed -n 's/^frame_stats: frame_ms .* p95=\([0-9.]*\) .*/\1/p' "$scratch/06-run.txt" | head -n 1)"
  if [ -z "$p95" ] || ! awk -v p="$p95" 'BEGIN { exit !(p + 0 < 16.7) }'; then
    say "frame-time p95 ${p95:-?} ms is not under 16.7 ms"; ok=0
  fi
  occluded="$(sed -n 's/^frame_stats: window .* occluded=\([0-9]*\)\/.*/\1/p' "$scratch/06-run.txt" | head -n 1)"
  if [ "${occluded:-x}" != "0" ]; then
    say "the window was fully covered for ${occluded:-?} frames: macOS presents nothing then, so the frame times are not valid (uncover the top-left corner of the primary monitor and rerun)"; ok=0
  fi
  replay="$(replay_from "$scratch/06-run.txt" "$dir")"
  if [ -z "$replay" ]; then say "no replay written"; ok=0; else
    verify "$replay" "$scratch/06-verify.txt"
    verify_ok "$scratch/06-verify.txt" || { say "verify not OK"; ok=0; }
  fi
  if [ "$ok" -eq 1 ]; then pass 6 "units200_auto: $(grep '^frame_stats:' "$scratch/06-run.txt" | head -n 1)"; else fail 6 "units200_auto; see $scratch/06-*"; fi
fi

echo
echo "==> summary ($scratch)"
printf '%s\n' "${results[@]}"
if [ "$failures" -gt 0 ]; then
  echo "m2_checks: $failures check(s) FAILED"
  exit 1
fi
echo "m2_checks: all checks passed"
