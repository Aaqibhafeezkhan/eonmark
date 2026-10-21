#!/usr/bin/env bash
# rerecord_fixtures.sh: re-record every golden replay fixture after a
# rules_version bump (docs/DETERMINISM.md, "Golden fixtures"; the data-only
# contributor path in CONTRIBUTING.md). No Rust is touched.
#
# For each crates/sim/tests/fixtures/<name>.eonreplay:
#   1. sim-cli record --scenario <name> --out <file> (release build)
#   2. sim-cli verify <file>, which re-simulates from the header
#   3. <name>.hash gets the final_hash from verify's OK line, so the hash
#      comes from an independent re-simulation, not from the recorder.
# Then it checks the fixture policy on the working tree: if any fixture now
# differs from the merge base with origin/main (or main), the working-tree
# rules_version must be larger than the base's. (CI's
# scripts/check_fixture_policy.sh checks the same on commits.)
#
# Usage: scripts/rerecord_fixtures.sh [name ...]   (default: every fixture)
set -euo pipefail
cd "$(dirname "$0")/.."

dir=crates/sim/tests/fixtures
if [ $# -eq 0 ]; then
  set --
  for f in "$dir"/*.eonreplay; do
    name="$(basename "$f" .eonreplay)"
    set -- "$@" "$name"
  done
fi

cargo build -q -p sim-cli --locked --release
bin="${CARGO_TARGET_DIR:-target}/release/sim-cli"

for name in "$@"; do
  file="$dir/$name.eonreplay"
  "$bin" record --scenario "$name" --out "$file"
  ok="$("$bin" verify "$file")"
  hash="$(printf '%s\n' "$ok" | grep -o 'final_hash=0x[0-9a-f]\{16\}' | head -n 1 | cut -d= -f2)"
  case "$ok" in
    OK*) ;;
    *) echo "rerecord: verify of $file did not print OK: $ok" >&2; exit 1 ;;
  esac
  if [ -z "$hash" ]; then
    echo "rerecord: no final_hash in verify output for $file: $ok" >&2
    exit 1
  fi
  printf '%s\n' "$hash" > "$dir/$name.hash"
  echo "rerecord: $name $hash"
done

base="$(git merge-base origin/main HEAD 2>/dev/null || git merge-base main HEAD)"
version() { sed -nE 's/^[[:space:]]*rules_version:[[:space:]]*([0-9]+),?.*/\1/p' | head -n 1; }
old="$(git show "$base:data/rules/rules.ron" | version)"
new="$(version < data/rules/rules.ron)"
if git diff --quiet "$base" -- "$dir"; then
  echo "rerecord: no fixture differs from ${base:0:12}"
elif [ -n "$old" ] && [ -n "$new" ] && [ "$new" -gt "$old" ]; then
  echo "rerecord: fixtures differ from ${base:0:12} with rules_version $old -> $new"
else
  echo "rerecord: fixtures differ from ${base:0:12} but rules_version is ${new:-?} (base ${old:-?}); bump it in data/rules/rules.ron" >&2
  exit 1
fi
