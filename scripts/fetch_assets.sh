#!/usr/bin/env bash
# fetch_assets.sh: download the art pass's third-party files, check their
# licences and sha256 pins, and copy the vendored ones into assets/.
# Maintainer-only (`just fetch-assets`); CI never downloads. The vendored
# copies under assets/ are the source of truth; this script reproduces them.
#
# The manifest (default tools/assets/sources.tsv) has a header row and one
# tab-separated row per remote file:
#   dest           path under assets/, or - for a tool-only input (the red
#                  KayKit variants for `inspect_gltf swatches`, the character
#                  sources for `inspect_gltf bake`) kept in the cache only
#   url            https://raw.githubusercontent.com/... (the only reachable
#                  host for these packs; github.com pages and git are not)
#   sha256         the pin; an empty cell prints the computed hash and fails,
#                  so a first-use pin is a deliberate paste
#   licence        SPDX id (CC0-1.0 or OFL-1.1)
#   author, page   the ATTRIBUTION.md Author and Source URL
#   licence_proof  only on licence rows: an extended regex the text must match
#
# Order: every licence row is fetched and checked first, and a text that does
# not match its proof stops the script before it is installed and before
# anything else is downloaded.
# Then each other row is fetched into a fresh directory, hashed and checked,
# and moved to <cache>/<sha12>/<name>; a file already there with the pinned
# hash is not downloaded again. Every file but the licence texts is also
# copied to <cache>/files/ under its own name, where `inspect_gltf swatches`
# and `inspect_gltf bake` read it, so a .gltf finds its .bin and texture (a
# name shipped in several folders must have the same bytes in each). Rows with a dest
# are copied into assets/ only when the bytes differ, so a second run leaves
# `git status --porcelain assets/` empty.
#
# Downloads are data: nothing in the cache is executed; only inspect_gltf
# parses it. The cache is EONMARK_ASSET_CACHE, default target/asset-cache
# (inside the gitignored /target).
#
# Usage: scripts/fetch_assets.sh [manifest]
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
manifest="${1:-tools/assets/sources.tsv}"
cache="${EONMARK_ASSET_CACHE:-target/asset-cache}"
header=$'dest\turl\tsha256\tlicence\tauthor\tpage\tlicence_proof'

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

die() {
  echo "fetch_assets: error: $*" >&2
  exit 1
}

[ -f "$manifest" ] || die "no manifest $manifest"
lines="$(grep -v '^#' "$manifest" | grep -v '^[[:space:]]*$' || true)"
[ "$(printf '%s\n' "$lines" | head -n 1)" = "$header" ] || die "$manifest must start with the header row: $header"
body="$(printf '%s\n' "$lines" | tail -n +2)"
bad="$(printf '%s\n' "$body" | awk -F'\t' 'NF != 7 { print NR + 1 ": " NF " cells"; exit }')"
[ -z "$bad" ] || die "$manifest data row $bad, expected 7 tab-separated cells"

mkdir -p "$cache/files"
unpinned=0
fetched=0
reused=0
installed=0

# fetch_row DEST URL PIN FLAT: download (or reuse) one file and set $stored
# to its path in the cache; FLAT=yes also copies it into <cache>/files/
# (licence texts all share the name LICENSE.txt and stay out of it). It does
# not touch assets/; see vendor_file.
fetch_row() {
  local dest="$1" url="$2" pin="$3" flat="$4" name tmp got
  case "$url" in
    https://raw.githubusercontent.com/*) ;;
    *) die "$url: only https://raw.githubusercontent.com/ URLs are fetched" ;;
  esac
  name="${url##*/}"
  case "$name" in
    ''|.*|*/*|*..*) die "$url: unusable file name" ;;
  esac
  if [ -n "$pin" ] && [ -f "$cache/${pin:0:12}/$name" ] && [ "$(sha256 "$cache/${pin:0:12}/$name")" = "$pin" ]; then
    stored="$cache/${pin:0:12}/$name"
    reused=$((reused + 1))
  else
    tmp="$(mktemp -d "$cache/download.XXXXXX")"
    curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 --retry 2 --max-time 120 \
      -o "$tmp/$name" "$url" || { rm -rf "$tmp"; die "$url: download failed"; }
    got="$(sha256 "$tmp/$name")"
    if [ -z "$pin" ]; then
      echo "fetch_assets: unpinned: $url sha256 $got (paste it into $manifest)" >&2
      unpinned=$((unpinned + 1))
    elif [ "$got" != "$pin" ]; then
      rm -rf "$tmp"
      die "$url: sha256 $got does not match the pin $pin"
    fi
    mkdir -p "$cache/${got:0:12}"
    mv -f "$tmp/$name" "$cache/${got:0:12}/$name"
    rmdir "$tmp"
    stored="$cache/${got:0:12}/$name"
    fetched=$((fetched + 1))
  fi
  if [ "$flat" = yes ]; then
    if [ -f "$cache/files/$name" ] && ! cmp -s "$cache/files/$name" "$stored"; then
      die "$url: $name is already in $cache/files with different bytes"
    fi
    cp "$stored" "$cache/files/$name"
  fi
}

# vendor_file DEST URL PIN: copy $stored to assets/DEST when the bytes differ.
# A tool-only row (-) is never installed, and an unpinned file stays in the
# cache until its hash is pasted.
vendor_file() {
  local dest="$1" url="$2" pin="$3"
  if [ "$dest" = "-" ] || [ -z "$pin" ]; then
    return
  fi
  case "$dest" in
    /*|*..*) die "$url: dest $dest must be a relative path under assets/" ;;
  esac
  if ! cmp -s "$stored" "assets/$dest" 2>/dev/null; then
    mkdir -p "$(dirname "assets/$dest")"
    cp "$stored" "assets/$dest"
    chmod 0644 "assets/$dest"
    installed=$((installed + 1))
  fi
}

# rows PASS: the data rows with their tabs mapped to \x1f, so empty cells
# survive `read` (tab is whitespace to IFS, and runs of it would collapse).
# PASS licences keeps the rows with a licence_proof, PASS files the others.
rows() {
  printf '%s\n' "$body" | awk -F'\t' -v pass="$1" '(pass == "licences") == ($7 != "")' | tr '\t' '\037'
}
stored=""

echo "fetch_assets: licences first"
while IFS=$'\037' read -r dest url pin licence _ _ proof; do
  fetch_row "$dest" "$url" "$pin" no
  if ! grep -Eq "$proof" "$stored"; then
    die "$url ($licence) does not match its licence proof /$proof/; stopping before any other download"
  fi
  vendor_file "$dest" "$url" "$pin"
  echo "fetch_assets: licence ok: $licence ${dest} ($url)"
done < <(rows licences)

while IFS=$'\037' read -r dest url pin _ _ _ _; do
  fetch_row "$dest" "$url" "$pin" yes
  vendor_file "$dest" "$url" "$pin"
done < <(rows files)

if [ "$unpinned" -gt 0 ]; then
  die "$unpinned row(s) have no sha256 pin; paste the hashes printed above"
fi
echo "fetch_assets: ok (downloaded $fetched, reused $reused, installed $installed into assets/; cache $cache)"
