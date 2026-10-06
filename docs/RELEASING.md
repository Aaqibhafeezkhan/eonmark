# Releasing

This document is the release procedure for Eonmark: what must be true before a tag, what `release.yml` and `scripts/bundle.sh` do and in what order, what goes into the release notes, what a user must do to open an ad-hoc-signed app on macOS, and how notarization is switched on later. The process is active from M8, when `scripts/bundle.sh` and `release.yml` are finished; before that, `release-check.yml` exercises the build and bundle steps on every push to `main` so they do not rot. Signing details and the notarization path are in [design/macos-packaging.md](design/macos-packaging.md).

Status: active from M8.

## Before tagging

All of these must hold on `main`.

1. `just ci` passes locally.
2. `just bundle && just release-check` passes locally. `just bundle` does the release build and `bundle.sh` (including the `bevy_dylib` absence check); `just release-check` then asserts `plutil -lint`, `codesign --verify --deep --strict`, no `bevy_dylib`, arm64 only, `data/` present and `SHA256SUMS` valid on the existing `dist/Eonmark.app`.
3. The three-match playtest log in [PLAYTEST.md](PLAYTEST.md) is updated for this version (M6 onward).
4. `sim_version` (`sim::SIM_VERSION`) and `rules_version` (`data/rules/rules.ron`) are the values you intend to ship, and every golden fixture passes `sim-cli verify`.
5. The workspace `version` in `Cargo.toml` matches the tag without the `v`.
6. [DEPENDENCIES.md](DEPENDENCIES.md) reflects the lockfile.

## Tag

```bash
git checkout main && git pull --ff-only
git tag -a vX.Y.Z -m "Eonmark vX.Y.Z"
git push origin vX.Y.Z
```

The tag pattern `v*` triggers `.github/workflows/release.yml` on a `macos-26` runner.

## What `release.yml` does

| Step | Command or action | Notes |
|---|---|---|
| 1 | `actions/checkout`, toolchain from `rust-toolchain.toml` | |
| 2 | `cargo build --release --locked -p game` | `aarch64-apple-darwin` only. Release profile: `lto = "thin"`, `codegen-units = 1`, `strip = "debuginfo"` (symbols kept for readable backtraces). Never with the `dev` feature. |
| 3 | `scripts/bundle.sh vX.Y.Z` | Steps below. |
| 4 | Upload `dist/Eonmark-*.zip` (the app zip, plus `Eonmark-vX.Y.Z.dSYM.zip` when `bundle.sh` produced one) and `SHA256SUMS` | `softprops/action-gh-release@v3` with generated notes and the build-info header below. |
| 5 | Notarization | Runs only when the secrets listed below exist. |

## What `scripts/bundle.sh` does

The order matters. Signing is the last step because any modification after signing invalidates the signature and macOS kills the process.

| Order | Step |
|---|---|
| 1 | `cargo build --release --locked -p game` (if not already built) |
| 2 | Create `dist/Eonmark.app/Contents/MacOS` and `dist/Eonmark.app/Contents/Resources` |
| 3 | Write `Info.plist`: `CFBundleExecutable` eonmark, `CFBundleIdentifier` com.tonianev.eonmark, `CFBundleName` and `CFBundleDisplayName` Eonmark, `CFBundlePackageType` APPL, `CFBundleShortVersionString` = version (tag without `v`; falls back to the nearest tag, then the workspace version), `CFBundleVersion` = `git rev-list --count HEAD`, `CFBundleIconFile` Eonmark (only when `assets/icon.iconset` exists), `LSMinimumSystemVersion` 13.0, `LSApplicationCategoryType` public.app-category.strategy-games, `NSHighResolutionCapable` true, `NSHumanReadableCopyright`, `LSEnvironment { RUST_BACKTRACE = 1 }` |
| 4 | `iconutil -c icns assets/icon.iconset -o Contents/Resources/Eonmark.icns`, only when `assets/icon.iconset` exists (it does not at M0) |
| 5 | Copy the `eonmark` binary, the `assets/` directory and the `data/` directory into `Contents/MacOS` (both sit beside the binary; `just release-check` asserts `Contents/MacOS/data` exists) |
| 6 | `if otool -L dist/Eonmark.app/Contents/MacOS/eonmark \| grep -q bevy_dylib; then exit 1; fi` (a `dev` build must never ship) |
| 7 | `codesign --force --deep --sign - dist/Eonmark.app` (ad-hoc, LAST) |
| 8 | `ditto -c -k --keepParent dist/Eonmark.app dist/Eonmark-vX.Y.Z-macos-arm64.zip` |
| 9 | If `target/release/eonmark.dSYM` exists, zip it to `dist/Eonmark-vX.Y.Z.dSYM.zip`. With the current `[profile.release]` (`strip = "debuginfo"`, no `debug` setting) no dSYM is produced; whether to set `debug = "line-tables-only"` is an M8 open question in [design/macos-packaging.md](design/macos-packaging.md) |
| 10 | `shasum -a 256` over every zip in `dist/` into `dist/SHA256SUMS` |

`ditto` rather than `zip` because it preserves the bundle's extended attributes and resource structure. No bundler dependency. `SKIP_BUILD=1 scripts/bundle.sh` reuses an existing `target/release/eonmark`.

## Release notes

The generated notes are edited to include, at the top:

| Line | Source |
|---|---|
| `sim_version` | `sim::SIM_VERSION` (M1: `release.yml` gains a `sim-cli` version line) |
| `rules_version` | `data/rules/rules.ron` |
| `rules_hash` | `cargo run -p sim-cli -- data-check data` output; `release.yml` already embeds this line as `rules check` in the notes header |
| Supported platform | Apple Silicon Macs, macOS 13.0 or later |
| Install steps | Copy of the Gatekeeper section below, or a link to the README section |
| Known issues | Open issues labelled for this release |

Replays recorded by this build carry the same `sim_version` and `rules_hash`, so a bug report with a replay attached can be verified against the exact release.

## Gatekeeper instructions for users

The app is ad-hoc signed, not notarized. macOS blocks it on first open. Put this in the README and the release notes.

1. Download the zip with Safari. Tools like `curl` or `gh` skip the quarantine flag, which hides the exact behaviour testers will see.
2. Optionally verify the download: `shasum -a 256 Eonmark-vX.Y.Z-macos-arm64.zip` and compare with `SHA256SUMS`.
3. Double-click the zip to extract `Eonmark.app`, then double-click the app. macOS says it cannot verify the developer. Click Done.
4. Open System Settings, then Privacy & Security. Near the bottom, an entry for Eonmark offers Open Anyway. It is shown for about an hour after the blocked attempt. Click it and enter your password.
5. Alternative for the terminal: remove the quarantine flag and open normally.

```bash
xattr -dr com.apple.quarantine Eonmark.app
open Eonmark.app
```

Control-click then Open no longer bypasses Gatekeeper on current macOS; only the Open Anyway path or the `xattr` command works.

## Optional notarization

Developer ID signing and notarization cost an Apple Developer Program membership (99 USD per year). The steps are wired into `release.yml` and run only when all five secrets exist in the repository settings.

| Secret | Holds |
|---|---|
| `APPLE_ID` | Apple ID email of the account that holds the Developer ID |
| `APPLE_TEAM_ID` | Ten-character team identifier |
| `APPLE_APP_PASSWORD` | An app-specific password for `notarytool` |
| `MACOS_CERT_P12` | Base64 of the Developer ID Application certificate and key (`.p12`) |
| `MACOS_CERT_PASSWORD` | Password of that `.p12` file |

When enabled, the signing step changes from ad-hoc to:

```bash
codesign --force --deep --options runtime --timestamp \
  --sign "Developer ID Application: <name> (<team id>)" dist/Eonmark.app
ditto -c -k --keepParent dist/Eonmark.app dist/Eonmark-vX.Y.Z-macos-arm64.zip
xcrun notarytool submit dist/Eonmark-vX.Y.Z-macos-arm64.zip \
  --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" --wait
xcrun stapler staple dist/Eonmark.app
ditto -c -k --keepParent dist/Eonmark.app dist/Eonmark-vX.Y.Z-macos-arm64.zip
```

The zip is rebuilt after stapling so the shipped bundle carries the ticket. The Gatekeeper section above then becomes unnecessary for users.

## After the release

1. Download the zip through Safari on a second Mac or a fresh user account and follow the Gatekeeper steps exactly as a stranger would.
2. Play one skirmish to the game-over screen and run `sim-cli verify` on the replay it wrote.
3. Open the release page and confirm the app zip, `SHA256SUMS` (and the dSYM zip when produced) and the build-info lines in the notes.
4. Bump the workspace `version` on `main` to the next development version.

## Versioning

Tags are `vX.Y.Z`. The first public release is `v0.1.0` at M8. Before `v0.1.0` there are no tags; `release-check.yml` is the only release-path exercise. From `v0.1.0`, the GOVERNANCE invariants "main is always playable" and "no PR removes a shipped feature" apply.
