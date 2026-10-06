# macOS packaging and platform notes

This document specifies how Eonmark becomes `Eonmark.app` on Apple Silicon: the steps of `scripts/bundle.sh`, the `Info.plist` keys, why ad-hoc signing is the last step, what a tester has to do to get past Gatekeeper, how the optional Developer ID and notarization path is wired, and why the release is arm64 only. It also collects the macOS-specific behaviour the game crate must handle: the Cmd-Q trap and the muda menu, the replay writer thread and why it never calls fsync, trackpad event mapping, Metal limits, the winit floor, and the deployment target. Release steps and tagging live in [../RELEASING.md](../RELEASING.md); this document explains the decisions behind them.

Area: `area:infra` and `platform:macos`. Milestones: M2 (Cmd-Q, replay writer, trackpad), M8 (bundle, release, Gatekeeper path). See [../ROADMAP.md](../ROADMAP.md).

## Target numbers

| Quantity | Value | Where it is set |
| --- | --- | --- |
| Target triple | `aarch64-apple-darwin` only | `scripts/bundle.sh`, `release.yml` |
| Deployment target | macOS 13.0 | `.cargo/config.toml` (`MACOSX_DEPLOYMENT_TARGET`), `LSMinimumSystemVersion` |
| Bundle id | `com.tonianev.eonmark` | `Info.plist` |
| Executable | `eonmark` | `crates/game/Cargo.toml` `[[bin]]` |
| Artifacts per tag | `Eonmark-<tag>-macos-arm64.zip`, `SHA256SUMS`, and `Eonmark-<tag>.dSYM.zip` when the build emits a dSYM (see Debug symbols below) | `release.yml` uploads `dist/Eonmark-*.zip` and `SHA256SUMS` |
| Signing in v0.1 | ad-hoc (`codesign --force --deep --sign -`), last step | `bundle.sh` |
| Replay flush | write + flush every 20 ticks (1 s); `sync_all` only at clean exit; hard kill loses at most 1 s | `sim_driver.rs` writer thread |
| winit floor | >= 0.30.12 (macOS 26 crash fix); 0.30.13 locked | `Cargo.lock` |
| Bevy | 0.19.1 with the `gestures` feature | `Cargo.toml` |
| wgpu | 29.x (29.0.4 locked), Metal backend | transitive |
| Audio stack | `bevy_audio` on rodio 0.22 and cpal 0.17 (0.17.3 locked), one cpal version in the tree | `cargo tree -i cpal` |
| Default window | 1280 x 800 logical, `NSHighResolutionCapable` | `app.rs`, `Info.plist` |
| Draw calls | < 500 | shared meshes and materials |

## scripts/bundle.sh

About 40 lines of shell with no bundler dependency. cargo-bundle, cargo-packager and cargo-dist were considered and rejected: cargo-dist has no `.app` support and the others add a tool to learn for 17 plist keys and one `codesign` call. The script runs locally through `just bundle` and in `release-check.yml` and `release.yml`.

Steps, in order:

1. Assert `uname -s`/`uname -m` print `Darwin`/`arm64`. Resolve the version: `$1` if given, else the nearest tag (`git describe --tags --abbrev=0`), else the workspace `version` in `Cargo.toml`; strip a leading `v`. The build number is `git rev-list --count HEAD`.
2. `cargo build --release --locked -p game` unless `SKIP_BUILD=1`. The host is Apple Silicon, so the default target is `aarch64-apple-darwin`; no `--target` flag and no universal binary.
3. `rm -rf dist/Eonmark.app` and `mkdir -p dist/Eonmark.app/Contents/{MacOS,Resources}`.
4. Write `Contents/Info.plist` with the keys below.
5. If `assets/icon.iconset` exists, `iconutil -c icns` it into `Contents/Resources/Eonmark.icns` and set `CFBundleIconFile`; otherwise skip (no icon at M0).
6. Copy `target/release/eonmark` into `Contents/MacOS/` and copy `assets/` and `data/` into `Contents/MacOS/` beside the binary, because Bevy's default asset root is `assets` relative to the executable and the game resolves `data/` the same way (`<exe dir>/data`).
7. `if otool -L dist/Eonmark.app/Contents/MacOS/eonmark | grep -q bevy_dylib; then echo "dynamic_linking leaked into release"; exit 1; fi`. A `dev` build must never be bundled.
8. `codesign --force --deep --sign - dist/Eonmark.app`. This is the LAST step that touches the bundle. Apple Silicon requires at least an ad-hoc signature, and any later change to the binary (copying resources is fine, `install_name_tool`, rpath edits and `lipo` are not) invalidates it and the process dies with `Killed: 9`.
9. `ditto -c -k --keepParent dist/Eonmark.app dist/Eonmark-v$VERSION-macos-arm64.zip`. `ditto` preserves the bundle structure and extended attributes that `zip` can mangle.
10. If `target/release/eonmark.dSYM` exists, `ditto -c -k --keepParent` it to `dist/Eonmark-v$VERSION.dSYM.zip`.
11. `(cd dist && shasum -a 256 *.zip > SHA256SUMS)`.

Verification, run by `just release-check` on the existing `dist/Eonmark.app` (so the local sequence is `just bundle && just release-check`):

```bash
plutil -lint dist/Eonmark.app/Contents/Info.plist
codesign --verify --deep --strict --verbose=2 dist/Eonmark.app
otool -l dist/Eonmark.app/Contents/MacOS/eonmark | grep -A4 LC_BUILD_VERSION   # minos 13.0
open dist/Eonmark.app                                                           # Dock name Eonmark, icon present
```

Debug symbols. `[profile.release]` sets `strip = "debuginfo"` so panic backtraces keep symbol names while the binary stays small. For step 10 to find a `.dSYM`, the release profile must emit debug info before stripping (macOS packs it into the `.dSYM` with `split-debuginfo = "packed"`, the platform default). If the M8 build produces no `.dSYM`, set `debug = "line-tables-only"` in `[profile.release]` and record the size change in [../BUILD_TIMES.md](../BUILD_TIMES.md).

## Info.plist keys

| Key | Value |
| --- | --- |
| `CFBundleExecutable` | `eonmark` |
| `CFBundleIdentifier` | `com.tonianev.eonmark` |
| `CFBundleName` | `Eonmark` |
| `CFBundleDisplayName` | `Eonmark` |
| `CFBundlePackageType` | `APPL` |
| `CFBundleShortVersionString` | the version: the tag without the `v` (for example `0.1.0`), else the nearest tag, else the workspace version |
| `CFBundleVersion` | `git rev-list --count HEAD`, a monotonic build counter |
| `CFBundleIconFile` | `Eonmark` (for `Contents/Resources/Eonmark.icns`), written only when `assets/icon.iconset` exists |
| `LSMinimumSystemVersion` | `13.0` |
| `LSApplicationCategoryType` | `public.app-category.strategy-games` |
| `NSHighResolutionCapable` | `true` |
| `NSHumanReadableCopyright` | `Copyright <year> Toni Anev and Eonmark contributors. MIT OR Apache-2.0.` (year from `date +%Y`) |
| `LSEnvironment` | `{ RUST_BACKTRACE = 1 }` so a crash report from a tester has a readable backtrace |

The plist is written by the script as a heredoc; `plutil -lint` is the syntax gate.

## Gatekeeper: what a tester does

The v0.1 release is ad-hoc signed and not notarized, so a copy downloaded through a browser carries the `com.apple.quarantine` attribute and macOS blocks the first launch. Since macOS Sequoia (15) there is no Control-click > Open bypass. The documented path, which also appears in `README.md`:

1. Download `Eonmark-<tag>-macos-arm64.zip` from GitHub Releases with Safari and unzip it.
2. Double-click `Eonmark.app`. macOS says it cannot verify the app and blocks it. Close the dialog.
3. Open System Settings > Privacy & Security, scroll to the Security section, and click Open Anyway next to the Eonmark message. The button is only shown for about an hour after the blocked attempt.
4. Confirm with the login password or Touch ID, then click Open in the final dialog. This is needed once per download.

Alternative for people comfortable with a terminal:

```bash
xattr -dr com.apple.quarantine Eonmark.app
```

Checksum before any of this:

```bash
shasum -a 256 -c SHA256SUMS
```

Testers must download with Safari (or another browser). `curl` and `gh release download` do not set the quarantine attribute, so an app fetched that way launches without any prompt. A tester who reports "works for me" after a `curl` download has not tested the path a stranger will take. The M8 owner acceptance is a Safari download on a second Mac or a fresh user account.

## Optional: Developer ID and notarization

Not part of v0.1 (99 USD per year, owner decision in [../ROADMAP.md](../ROADMAP.md)). The steps are already in `release.yml` and run only when all of these secrets exist: `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`, `MACOS_CERT_P12` (plus the certificate's password if it has one). Without them the job skips straight to the ad-hoc path above.

When enabled, the steps replace step 8 of `bundle.sh` and extend the tail:

```bash
# import the Developer ID Application certificate into a temporary keychain (CI only)
codesign --force --deep --options runtime --timestamp \
  --sign "Developer ID Application: <name> (<team id>)" dist/Eonmark.app
ditto -c -k --keepParent dist/Eonmark.app dist/Eonmark-$TAG-macos-arm64.zip
xcrun notarytool submit dist/Eonmark-$TAG-macos-arm64.zip \
  --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" --wait
xcrun stapler staple dist/Eonmark.app
ditto -c -k --keepParent dist/Eonmark.app dist/Eonmark-$TAG-macos-arm64.zip   # re-zip with the stapled ticket
(cd dist && shasum -a 256 *.zip > SHA256SUMS)
```

Requirements Apple states for notarization: a Developer ID Application certificate (not a Mac Distribution, ad hoc or development certificate), the hardened runtime (`--options runtime`), a secure timestamp, and no `com.apple.security.get-task-allow` entitlement. `altool` was retired in 2023; only `notarytool` is accepted. A notarized and stapled app opens on first launch with no Gatekeeper dialog, which is the reason to pay once external players appear.

## arm64 only

The release targets `aarch64-apple-darwin` and nothing else:

- `aarch64-apple-darwin` is a Rust Tier 1 target; `x86_64-apple-darwin` was demoted to Tier 2 with host tools in Rust 1.90 after GitHub discontinued free x86_64 macOS runners (2025-09-01).
- macOS 26 is the last release that runs on Intel Macs, and Rosetta translation is mostly removed in macOS 27.
- A universal binary doubles build time in `release.yml` for a shrinking audience, and the game crate's Linux compile check (`game-linux` job) already keeps the code portable for contributors.

Running on Linux or Windows is unsupported in v0.1; the game crate must keep compiling on Linux (CI-enforced by the `game-linux` job) so contributors there can work on it. Windows is not checked.

## The Cmd-Q trap and the muda menu (M2)

On macOS, winit installs a default application menu whose Quit item calls `NSApp terminate:`. winit 0.30 implements only `applicationWillTerminate:`, and Bevy 0.19.1's winit runner reacts to it by clearing windows and the `World` and returning. No schedule runs, no `AppExit` is sent, no observer fires. A Cmd-Q would therefore end the process without the replay's clean-exit `sync_all` or any other exit logic, while the red close button and an in-game Quit (which go through `WindowCloseRequested` and `AppExit`) would work, so the bug would look intermittent. Bevy exposes no hook to disable or replace that menu.

The fix, under `cfg(target_os = "macos")` in `crates/game/src/macos_menu.rs`:

1. A startup system builds a muda 0.21 `Menu` with the usual predefined items (About, Hide, Hide Others, Show All, Services) and a custom `MenuItem` titled "Quit Eonmark" with the Cmd+Q accelerator, then calls `init_for_nsapp()`. It must be a custom item: muda's `PredefinedMenuItem::quit` also calls `terminate:` directly and emits no event.
2. A per-frame system drains `MenuEvent::receiver().try_recv()` and, when the Quit item's id arrives, writes `AppExit::Success`.
3. Both systems take a `NonSendMarker` parameter so Bevy schedules them on the AppKit main thread; menu work off the main thread is undefined behaviour in AppKit.

With this in place Cmd-Q, the close button and the menus all flow through `AppExit`, the replay writer gets its clean-exit signal, and the M2 acceptance ("play 2 minutes, press Cmd-Q, exit code 0, `sim-cli verify` passes") holds.

## Replay writer thread and the no-fsync rule (M2)

Every session records a replay (see [../DETERMINISM.md](../DETERMINISM.md)). The writer is a dedicated `std::thread` fed by a channel of postcard-encoded tick batches from the FixedUpdate driver. It writes and flushes (`BufWriter::flush`, which is a `write(2)`) every 20 ticks, that is once per second, and calls `File::sync_all` exactly once, at clean exit.

It never calls `sync_all` or `sync_data` per batch. On Apple platforms Rust's standard library implements both as `fcntl(F_FULLFSYNC)`, which forces the drive to flush its cache and costs from a few to tens of milliseconds. Doing that once a second on the FixedUpdate thread would be a visible periodic hitch at the flush cadence, which the M2 acceptance explicitly checks against on the frame-time graph. Surviving `kill -9` does not need it: a hard kill loses at most the last second of un-flushed batches, and the kernel still writes pages that were handed to `write(2)`. Surviving a power loss is not a goal.

The writer is purely a sink. It never reads sim state and never feeds anything back; the sim does not know it exists. Files go to `ProjectDirs::from("com", "tonianev", "Eonmark").data_dir()/replays/<timestamp>.eonreplay`, or to `--replay-dir <path>` so CI writes into a temporary directory.

## Trackpad and input mapping (M2)

winit delivers trackpad and mouse input differently and the camera must treat them differently:

| Device event | Bevy type | Camera action |
| --- | --- | --- |
| Two-finger scroll on a trackpad | `MouseWheel` with `MouseScrollUnit::Pixel` | pan, scaled per pixel |
| Mouse wheel notch | `MouseWheel` with `MouseScrollUnit::Line` | zoom, scaled per line |
| Pinch on a trackpad | `PinchGesture` (requires the Bevy `gestures` feature) | zoom |
| Rotation and double-tap gestures | `RotationGesture`, `DoubleTapGesture` | unused; the camera is yaw locked |
| `WindowEvent::Touch` | not delivered on macOS | nothing |

The `gestures` feature is not a Bevy default and is enabled explicitly in `crates/game/Cargo.toml`; `PinchGesture` does not compile without it. All camera speeds are multiplied by `Time::delta_secs` so a 120 Hz ProMotion display and a 60 Hz external display travel the same distance per second. See [ui.md](ui.md) for the full controls table.

## Metal limits that shape the renderer

| Limit on Metal | Consequence for Eonmark |
| --- | --- |
| No multi-draw-indirect and no GPU occlusion culling in Bevy; lower bindless limits | draw-call count matters; one shared mesh and material per kind, team colour through a small set of materials; dev-panel counter with a 500 budget |
| `ClusteredDecal` unsupported on macOS and iOS | selection rings and move markers are retained gizmos, borders are a material extension on the ground mesh, not decals |
| `PresentMode::Mailbox` unsupported; `AutoNoVsync` falls back to `Immediate` | `PresentMode::AutoVsync` (Fifo) is the shipped setting |
| wgpu 29 Metal surfaces lack an explicit sRGB colour space on wide-gamut displays | the matte palette is judged on a P3 display and an sRGB screenshot; accent chroma stays conservative (see [../ART_STYLE.md](../ART_STYLE.md)) |
| Apple GPUs are tile-based deferred renderers: MSAA resolves in tile memory cheaply, device-memory round trips are expensive | `Msaa::Sample4` is on; there is no post-processing beyond optional low SSAO, no bloom, no deferred G-buffer |

## Toolchain notes

- winit must stay at or above 0.30.12 in `Cargo.lock` (0.30.13 is locked). Earlier versions crash on macOS 26 and later. A contributor with a stale lockfile or a fork pinning older winit will hit it; `cargo tree -i winit` should print one version.
- `.cargo/config.toml` contains only `[env] MACOSX_DEPLOYMENT_TARGET = "13.0"`. No linker override, no `-ld_classic`, no `-ld64`, no `-fuse-ld`, no nightly flags, no cranelift. Xcode 27 removed ld64; rustc's default path through ld-prime is unaffected, and Bevy's own guidance says ld-prime is the fastest option on macOS.
- The dev Mac runs macOS 27.0.1 with Xcode 27; GitHub's `macos-26` runner ships Xcode 26.6. Pinning the deployment target keeps binaries built on either from advertising different minimum OS versions.
- Deployment target 13.0 is possible because `bevy_audio` runs on cpal 0.17, which states no macOS floor. Moving audio to bevy_kira_audio or bevy_seedling (cpal 0.18) would raise the floor to 14.2; that is an owner decision and would change `LSMinimumSystemVersion` and this document.
- Audio goes through a thin `AudioEvents` facade so that swap stays a one-file change. The CoreAudio bindings built against the Xcode 27 SDK at M0 ([../BUILD_TIMES.md](../BUILD_TIMES.md)); if they ever fail against a newer SDK, disable `bevy_audio`/`vorbis` until M7 and record it there. The locked tree resolves cpal 0.17.3 through coreaudio-rs 0.14.2 (no bindgen); the design text names coreaudio-sys 0.2.18, and [../DEPENDENCIES.md](../DEPENDENCIES.md) records the drift.
- Fullscreen uses `WindowMode::BorderlessFullscreen` with `Window::borderless_game` (hides menu bar and Dock). Exclusive fullscreen has a known panic path on exit that Bevy works around and is not used.

## Acceptance checklist

Copied from the M8 milestone in [../ROADMAP.md](../ROADMAP.md), plus the M2 line that this document's Cmd-Q and writer sections exist for.

M2:

- [ ] Play 2 minutes, press Cmd-Q: the process exits with code 0 and `sim-cli verify` on the newest replay exits 0; `kill -9` mid-session also leaves a replay that verifies.

M8:

- [ ] `scripts/bundle.sh` produces dist/Eonmark.app; `plutil -lint dist/Eonmark.app/Contents/Info.plist` OK; `codesign --verify --deep --strict --verbose=2 dist/Eonmark.app` exits 0; `open dist/Eonmark.app` launches with Dock name 'Eonmark' and icon; the bevy_dylib check passes; `otool -l` shows LC_BUILD_VERSION minos 13.0.
- [ ] Pushing tag v0.1.0 makes release.yml publish the zip, dSYM zip and SHA256SUMS; `shasum -a 256 -c SHA256SUMS` passes on the download.
- [ ] [owner] Downloading via Safari on a second Mac or fresh user account (quarantined), following the README 'Open Anyway' steps, launches the game and a match is completable; a replay from the release build verifies with `sim-cli verify` built from the same tag.
- [ ] [owner] Fresh clone on a second Mac with only Xcode CLT + rustup: `cargo run -p game --features dev` builds and opens the game on the first try; README quotes the cold build time from docs/BUILD_TIMES.md.
- [ ] `gh issue list --repo tonianev/eonmark --label 'good first issue' --json number | jq length` >= 15; Discussions has the three categories; the bevy-assets PR is open and the showcase posts exist (links in docs/ROADMAP.md).
- [ ] `cargo deny check`, `cargo machete`, `typos`, `scripts/check_trademark.sh`, `cargo doc --workspace --no-deps` green; at least one Dependabot grouped PR has passed CI; third-party actions SHA-pinned.
- [ ] CI smoke `--headless-run fixtures/smoke.eonreplay` passes; the release-check job's cold wall time is recorded (not gated) in docs/BUILD_TIMES.md.

Commands:

```bash
scripts/bundle.sh v0.1.0
plutil -lint dist/Eonmark.app/Contents/Info.plist
codesign --verify --deep --strict --verbose=2 dist/Eonmark.app
otool -l dist/Eonmark.app/Contents/MacOS/eonmark | grep -A4 LC_BUILD_VERSION
if otool -L dist/Eonmark.app/Contents/MacOS/eonmark | grep -q bevy_dylib; then echo "bevy_dylib linked"; exit 1; fi
(cd dist && shasum -a 256 -c SHA256SUMS)
cargo tree -i winit
cargo tree -i cpal
```

## Open questions

- Developer ID and notarization: the owner decides whether to enrol once external players appear. Until then the Gatekeeper path above is the supported route.
- Deployment target 13.0 versus 14.2: only matters if audio moves off `bevy_audio`. Keep 13.0 unless that happens.
- Whether `[profile.release]` needs `debug = "line-tables-only"` for the dSYM to exist. The first `bundle.sh` run (2026-10-05, `strip = "debuginfo"`, no `debug` key) produced no `.dSYM`, so no dSYM zip is published until this is set. Decide at M8 and record the binary size either way.
- This document was aligned with `scripts/bundle.sh` on 2026-10-05 (version fallback chain, optional `Eonmark.icns`, `CFBundleVersion` as commit count, copyright string, `data/` copied beside the binary). If the script changes, change this document in the same PR.
- A Homebrew cask, itch.io page or Steam listing are out of scope for v0.1; GitHub Releases is the only channel.
