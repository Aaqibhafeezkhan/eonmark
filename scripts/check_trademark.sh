#!/usr/bin/env bash
# check_trademark.sh: enforce the naming rules from docs/NAME.md.
#
# The inspiration's title may appear only in README.md (one disclaimed
# sentence) and docs/design/prior-art.md (its Inspiration section and source
# citations). Spaced, underscored and hyphenated forms all count, so URLs and
# slugs are caught too. Its three-letter abbreviation and links to the
# inspiration's fan wiki may not appear anywhere.
# The check runs
# over the working tree (not the git index) so it works before the first
# commit and catches unstaged files. target/, dist/ and .git/ are skipped.
#
# Usage: scripts/check_trademark.sh [repo-root]
# Exit 0 when clean; exit 1 and print the offending lines otherwise.
set -euo pipefail

root="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
cd "$root"

allowed_files=("./README.md" "./docs/design/prior-art.md")
# Assembled from parts so this script does not match its own rules.
abbr="Ro""N"
status=0

grep_tree() {
  grep -rIn --exclude-dir=target --exclude-dir=dist --exclude-dir=.git "$@" . || true
}

filter_allowed() {
  local pattern
  pattern="$(printf '%s\n' "${allowed_files[@]}" | sed 's|[.]|\\.|g; s|^|^|; s|$|:|' | paste -sd '|' -)"
  grep -Ev "$pattern" || true
}

hits="$(grep_tree -i -E 'rise[ _-]+of[ _-]+nations|fandom\.com' | filter_allowed)"
if [ -n "$hits" ]; then
  echo "error: the inspiration's title or a fan-wiki link appears outside README.md and docs/design/prior-art.md:" >&2
  printf '%s\n' "$hits" >&2
  status=1
fi

hits="$(grep_tree -w "$abbr")"
if [ -n "$hits" ]; then
  echo "error: the abbreviation '$abbr' is not allowed anywhere:" >&2
  printf '%s\n' "$hits" >&2
  status=1
fi

if [ "$status" -eq 0 ]; then
  echo "check_trademark: ok"
fi
exit "$status"
