#!/usr/bin/env bash
# check_assets.sh: enforce the asset policy from assets/LICENSE-ASSETS.md and
# CONTRIBUTING.md.
#
# Rules:
#   1. Every file under assets/ (except the allowlist below) has a row in
#      assets/ATTRIBUTION.md whose first column is the path relative to
#      assets/, for example `fonts/Inter.ttf`.
#   2. That row's License column is one of CC0-1.0, CC-BY-4.0, OFL-1.1.
#   3. Every .ttf/.otf has a sibling OFL*.txt (OFL 1.1 requires the text).
#   4. No single file is larger than 2 MB; assets/ totals under 300 MB.
# An empty or missing assets/ directory passes.
#
# Usage: scripts/check_assets.sh [repo-root]
set -euo pipefail

root="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
cd "$root"

assets_dir="assets"
attribution="$assets_dir/ATTRIBUTION.md"
max_file_bytes=$((2 * 1024 * 1024))
max_total_bytes=$((300 * 1024 * 1024))
allowed_licenses="CC0-1.0 CC-BY-4.0 OFL-1.1"
status=0

fail() {
  echo "error: $*" >&2
  status=1
}

if [ ! -d "$assets_dir" ]; then
  echo "check_assets: no assets/ directory, nothing to check"
  exit 0
fi

# --- parse ATTRIBUTION.md table rows into "path<TAB>license" -----------------
# The header row decides which column holds the license (default: 4th).
rows=""
if [ -f "$attribution" ]; then
  rows="$(awk -F'|' '
    function trim(s) { gsub(/^[ \t`]+|[ \t`]+$/, "", s); return s }
    /^\|/ {
      if (!header_seen) {
        header_seen = 1; lic_col = 0
        for (i = 2; i < NF; i++) if (tolower(trim($i)) == "license") lic_col = i
        if (lic_col == 0) lic_col = 5   # 1 = empty before first pipe, 2 = path, ..., 5 = 4th column
        next
      }
      if ($0 ~ /^\|[ \t]*:?-+/) next   # separator row
      path = trim($2); lic = trim($lic_col)
      if (path != "") printf "%s\t%s\n", path, lic
    }' "$attribution")"
fi

total=0
count=0
while IFS= read -r -d '' file; do
  rel="${file#"$assets_dir"/}"
  size=$(stat -f %z "$file" 2>/dev/null || stat -c %s "$file")
  total=$((total + size))
  count=$((count + 1))

  if [ "$size" -gt "$max_file_bytes" ]; then
    fail "$rel is $size bytes; the limit is $max_file_bytes (2 MB). Downscale or compress it."
  fi

  case "$rel" in
    ATTRIBUTION.md|LICENSE-ASSETS.md|shaders/*|.gitkeep|*/.gitkeep) continue ;;
  esac

  if [ ! -f "$attribution" ]; then
    fail "$rel exists but $attribution is missing"
    continue
  fi

  lic="$(printf '%s\n' "$rows" | awk -F'\t' -v p="$rel" '$1 == p { print $2; exit }')"
  if [ -z "$lic" ]; then
    fail "$rel has no row in $attribution (first column must be '$rel')"
  else
    ok=0
    for a in $allowed_licenses; do [ "$lic" = "$a" ] && ok=1; done
    if [ "$ok" -eq 0 ]; then
      fail "$rel is listed with license '$lic'; allowed: $allowed_licenses"
    fi
  fi

  case "$rel" in
    *.ttf|*.otf|*.TTF|*.OTF)
      dir="$(dirname "$file")"
      if ! ls "$dir"/OFL*.txt >/dev/null 2>&1; then
        fail "$rel has no sibling OFL*.txt license text in $dir/"
      fi
      ;;
  esac
done < <(find "$assets_dir" -type f ! -name '.DS_Store' -print0 | sort -z)

if [ "$total" -ge "$max_total_bytes" ]; then
  fail "assets/ totals $total bytes; the limit is $max_total_bytes (300 MB)"
fi

if [ "$status" -eq 0 ]; then
  echo "check_assets: ok ($count files, $total bytes)"
fi
exit "$status"
