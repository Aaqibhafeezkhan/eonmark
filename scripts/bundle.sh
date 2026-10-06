#!/usr/bin/env bash
# bundle.sh: build the release binary and assemble dist/Eonmark.app for
# Apple Silicon, then zip it and write SHA256SUMS. No bundler dependency.
#
# Steps: cargo build --release -> Contents/{MacOS,Resources} -> Info.plist ->
# optional icon from assets/icon.iconset -> copy assets/ and data/ beside the
# binary -> refuse bevy_dylib -> ad-hoc codesign LAST -> ditto zip -> dSYM zip
# if present -> SHA256SUMS.
#
# Usage: scripts/bundle.sh [version]
#   version defaults to the nearest tag (without the leading v), or the
#   workspace version from Cargo.toml before the first tag.
# Env:  SKIP_BUILD=1 reuses target/release/eonmark without rebuilding.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
  echo "error: bundle.sh builds arm64 macOS bundles and must run on an Apple Silicon Mac" >&2
  exit 1
fi

version="${1:-}"
if [ -z "$version" ]; then
  version="$(git describe --tags --abbrev=0 2>/dev/null || true)"
fi
if [ -z "$version" ]; then
  version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
fi
version="${version#v}"
build_number="$(git rev-list --count HEAD 2>/dev/null || echo 1)"
year="$(date +%Y)"

bin=target/release/eonmark
app=dist/Eonmark.app
contents="$app/Contents"
zip="dist/Eonmark-v${version}-macos-arm64.zip"

if [ "${SKIP_BUILD:-0}" != "1" ]; then
  cargo build --release --locked -p game
fi
test -x "$bin" || { echo "error: $bin not found" >&2; exit 1; }

dsym_zip="dist/Eonmark-v${version}.dSYM.zip"
rm -rf "$app" "$zip" "$dsym_zip" dist/Eonmark.dSYM.zip dist/SHA256SUMS
mkdir -p "$contents/MacOS" "$contents/Resources"
cp "$bin" "$contents/MacOS/eonmark"

icon_key=""
if [ -d assets/icon.iconset ]; then
  iconutil -c icns assets/icon.iconset -o "$contents/Resources/Eonmark.icns"
  icon_key="  <key>CFBundleIconFile</key>
  <string>Eonmark</string>"
fi

cat > "$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>eonmark</string>
  <key>CFBundleIdentifier</key>
  <string>com.tonianev.eonmark</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Eonmark</string>
  <key>CFBundleDisplayName</key>
  <string>Eonmark</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${version}</string>
  <key>CFBundleVersion</key>
  <string>${build_number}</string>
${icon_key}
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.strategy-games</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSHumanReadableCopyright</key>
  <string>Copyright ${year} Toni Anev and Eonmark contributors. MIT OR Apache-2.0.</string>
  <key>LSEnvironment</key>
  <dict>
    <key>RUST_BACKTRACE</key>
    <string>1</string>
  </dict>
</dict>
</plist>
PLIST
plutil -lint "$contents/Info.plist"

# Bevy resolves `assets/` relative to the executable; data/ follows the same rule.
[ -d assets ] && cp -R assets "$contents/MacOS/assets"
[ -d data ] && cp -R data "$contents/MacOS/data"

if otool -L "$contents/MacOS/eonmark" | grep -q bevy_dylib; then
  echo "error: dynamic linking leaked into the release binary (never ship --features dev)" >&2
  exit 1
fi

# Signing is the LAST modification of the bundle. Apple Silicon refuses to run
# unsigned code; release.yml re-signs with a Developer ID when secrets exist.
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

ditto -c -k --keepParent "$app" "$zip"
if [ -d target/release/eonmark.dSYM ]; then
  ditto -c -k --keepParent target/release/eonmark.dSYM "$dsym_zip"
fi
(cd dist && shasum -a 256 ./*.zip > SHA256SUMS)

echo "bundle: $app (version ${version}, build ${build_number})"
cat dist/SHA256SUMS
