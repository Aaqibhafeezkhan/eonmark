#!/usr/bin/env bash
# check_assets.sh: enforce the asset policy from assets/LICENSE-ASSETS.md and
# CONTRIBUTING.md.
#
# Rules:
#   1. Every file under assets/ (except the allowlist below) has a row in
#      assets/ATTRIBUTION.md whose first column is the path relative to
#      assets/, for example `fonts/InterVariable.ttf`.
#   2. That row's License column is one of CC0-1.0, CC-BY-4.0, OFL-1.1.
#   3. Every .ttf/.otf has a sibling OFL*.txt (OFL 1.1 requires the text).
#   4. No single file is larger than 2 MB; assets/ totals under 300 MB.
#   5. (art pass) Every row of tools/assets/sources.tsv with a dest is
#      vendored byte for byte: the file under assets/ exists and its sha256
#      is the row's pin. ATTRIBUTION.md says Modifications `none` (or
#      `renamed from`) for these files, and this is what holds it to that;
#      a deliberate change must update the row and its Modifications text.
#      Checked only when the manifest exists under the root.
# An empty or missing assets/ directory passes.
#
# --self-test proves the injection clause of ROADMAP M7's asset line: it
# copies only assets/ into a temporary root, adds models/unattributed.glb,
# and requires this script to exit 1 naming that path on stderr, then to
# exit 0 once the file is removed again. It then proves rule 5 on a second
# root holding assets/ and tools/assets/sources.tsv: one changed byte in a
# pinned file must exit 1 naming that file, and the original bytes exit 0.
#
# Usage: scripts/check_assets.sh [repo-root | --self-test]
set -euo pipefail

if [ "${1:-}" = "--self-test" ]; then
  self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
  repo="$(cd "$(dirname "$0")/.." && pwd)"
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  mkdir -p "$tmp/root"
  cp -R "$repo/assets" "$tmp/root/assets"
  injected="models/unattributed.glb"
  mkdir -p "$tmp/root/assets/models"
  printf 'glTF' >"$tmp/root/assets/$injected"
  set +e
  "$self" "$tmp/root" >"$tmp/out" 2>"$tmp/err"
  code=$?
  set -e
  if [ "$code" -ne 1 ]; then
    echo "check_assets: self-test FAILED: an unattributed $injected gave exit $code, not 1" >&2
    cat "$tmp/out" "$tmp/err" >&2
    exit 1
  fi
  if ! grep -F "$injected" "$tmp/err"; then
    echo "check_assets: self-test FAILED: stderr does not name $injected" >&2
    cat "$tmp/err" >&2
    exit 1
  fi
  rm "$tmp/root/assets/$injected"
  if ! "$self" "$tmp/root"; then
    echo "check_assets: self-test FAILED: the copy fails without the injected file" >&2
    exit 1
  fi
  # Rule 5: a vendored file that no longer matches its pin.
  manifest="tools/assets/sources.tsv"
  pinned=""
  if [ -f "$repo/$manifest" ]; then
    mkdir -p "$tmp/pins/tools/assets"
    cp -R "$repo/assets" "$tmp/pins/assets"
    cp "$repo/$manifest" "$tmp/pins/$manifest"
    pinned="$(awk -F'\t' '/^#/ { next } !header { header = 1; next } $1 != "" && $1 != "-" { print $1; exit }' "$repo/$manifest")"
  fi
  if [ -n "$pinned" ]; then
    cp "$tmp/pins/assets/$pinned" "$tmp/original"
    printf 'x' >>"$tmp/pins/assets/$pinned"
    set +e
    "$self" "$tmp/pins" >"$tmp/out" 2>"$tmp/err"
    code=$?
    set -e
    if [ "$code" -ne 1 ]; then
      echo "check_assets: self-test FAILED: a changed byte in $pinned gave exit $code, not 1" >&2
      cat "$tmp/out" "$tmp/err" >&2
      exit 1
    fi
    if ! grep -F "$pinned" "$tmp/err"; then
      echo "check_assets: self-test FAILED: stderr does not name $pinned" >&2
      cat "$tmp/err" >&2
      exit 1
    fi
    cp "$tmp/original" "$tmp/pins/assets/$pinned"
    if ! "$self" "$tmp/pins"; then
      echo "check_assets: self-test FAILED: the copy fails with $pinned restored" >&2
      exit 1
    fi
    echo "check_assets: self-test ok (an unattributed file fails with its path; removing it passes; a changed pinned file fails with its path; restoring it passes)"
  else
    echo "check_assets: self-test ok (an unattributed file fails with its path; removing it passes; no pinned file to change)"
  fi
  exit 0
fi

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
  size=$(wc -c <"$file" | tr -d ' ')
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

# --- rule 5: vendored files match their sha256 pins --------------------------
manifest="tools/assets/sources.tsv"
if [ -f "$manifest" ]; then
  if command -v sha256sum >/dev/null 2>&1; then
    sha256() { sha256sum "$1" | cut -d' ' -f1; }
  else
    sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
  fi
  pins=0
  header=1
  while IFS=$'\t' read -r dest _url pin _rest; do
    case "$dest" in '' | '#'*) continue ;; esac
    if [ "$header" -eq 1 ]; then
      header=0
      continue
    fi
    [ "$dest" = "-" ] && continue
    pins=$((pins + 1))
    if [ ! -f "$assets_dir/$dest" ]; then
      fail "$dest is pinned in $manifest but missing from $assets_dir/"
      continue
    fi
    got="$(sha256 "$assets_dir/$dest")"
    if [ "$got" != "$pin" ]; then
      fail "$dest: sha256 $got does not match its pin $pin in $manifest; a vendored file stays byte for byte as shipped (ATTRIBUTION.md Modifications none): restore it (just fetch-assets) or record the change in its row and its Modifications text"
    fi
  done <"$manifest"
  if [ "$status" -eq 0 ]; then
    echo "check_assets: pins ok ($pins vendored files match $manifest)"
  fi
fi

if [ "$status" -eq 0 ]; then
  echo "check_assets: ok ($count files, $total bytes)"
fi
exit "$status"
