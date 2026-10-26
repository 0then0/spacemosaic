<p align="center"><img src="assets/icon.svg" width="96" height="96" alt="SpaceMosaic: a folder containing a storage map"></p>

# SpaceMosaic

[![CI](https://img.shields.io/github/actions/workflow/status/0then0/spacemosaic/ci.yml?label=CI)](https://github.com/0then0/spacemosaic/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/0then0/spacemosaic?include_prereleases)](https://github.com/0then0/spacemosaic/releases)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![Verified build](https://img.shields.io/badge/verified%20build-macOS%20arm64-blue)](docs/testing.md)

SpaceMosaic is a local, read-only storage explorer built with Rust and egui.
Choose a folder, inspect its directory tree, and compare large files using an
interactive space map. Files stay on your computer; the application never
reads their contents to calculate sizes.

**Development status:** alpha. Desktop and mobile share the Rust scan model,
recursive aggregation, treemap, and exports. Android and iOS use native storage
providers and offer a native result browser alongside the canvas. Mobile
runtime and device acceptance remain unverified; build checks are separate
from those checks. See [platform support](docs/testing.md).

## Explore storage

- Choose a folder with the system dialog, drag a folder into the desktop window,
  or pass its path on the command line.
- Scan on a background worker with progress, cancellation, and explicit partial
  results. Starting another scan replaces the previous session.
- Expand folders in the tree above the space map. On wide windows, drag the
  divider to resize the two areas. When space is limited, the result scrolls
  vertically so controls and content remain accessible. Navigate with breadcrumbs and Back, inspect
  the largest files, and review file types, including files without an extension.
- Search the completed result and sort by name or logical size without reading
  the disk again.
- Select an object in the tree or squarified treemap to inspect the same details.
  Tiles use stable colors for folders and file extensions, with explicit folder
  labels so navigation does not depend on color. Selecting a nested file marks
  its containing tile and shows the selected path above the map.
  Double-click a directory to zoom in. Smaller map items expand into a list;
  zero and unknown sizes remain available in the list.
- Copy a path, reveal an object in the system file manager, or export a
  versioned JSON result and CSV file-type summary through a system save dialog.

Open **Settings** to choose a language, text size, or **System**, **Light**,
and **Dark** appearance. The system option follows OS appearance changes; explicit choices override it. The visual
style uses layered neutral surfaces without shadows, readable typography, and
restrained sage section labels. Theme, language, and text-size choices currently
apply to the current session.

English is the primary project language. The interface includes English,
Russian, Spanish, German, French, Japanese, and Brazilian Portuguese. A bundled
Japanese font works offline. Inter Regular 400 is the main UI font, Inter
Medium 500 is used for headings, and Hack is used for monospace values.
See [font licensing and asset checksums](assets/fonts/README.md).

## Understand the numbers

SpaceMosaic measures the **selected scope**, not total device usage or volume
capacity. Logical size and allocated space are separate. Allocated bytes are
available from Unix metadata; unsupported values, including Windows allocation
metadata, stay unknown. Neither number guarantees reclaimable space.

Hard-linked files are counted once **per path**, without deduplication. Shared
APFS/CoW blocks, filesystem compression, and sparse allocation prevent these
sums from representing unique physical consumption. Symbolic links and Windows
reparse points are not followed. Their target size is unknown rather than zero.

Filesystem boundaries are not crossed by default on Unix. Enable crossing in
Settings before starting a new scan. Windows reparse points are skipped;
Windows volume identities are not otherwise compared. A scan is a live
metadata observation, not an atomic snapshot. Permission failures, disappearing
files, cancellation, numeric overflow, and resource limits remain visible as
partial or unknown results.

Desktop scans are limited to 500,000 objects and an estimated 256 MiB metadata
budget; mobile scans use 100,000 objects and 64 MiB. Reaching either limit leaves
an explicit partial result. These budgets guard stored node data, not total
process memory. Provider cursors, filtering, font atlases, and the UI require
additional memory. JSON exports stream to a temporary file before saving.

## Download the alpha

[v0.1.0-alpha.1](https://github.com/0then0/spacemosaic/releases/tag/v0.1.0-alpha.1)
is a prerelease for **macOS on Apple Silicon (arm64)**. Download
`spacemosaic-darwin-arm64.zip` and `SHA256SUMS` from the release, then verify:

```sh
shasum -a 256 -c SHA256SUMS
```

Extract the ZIP and open `spacemosaic-darwin-arm64/SpaceMosaic.app`.
The app is ad-hoc signed, without Developer ID signing or notarization.
macOS may require you to allow it in **System Settings → Privacy & Security**
after the first launch attempt. This is an alpha; scan results are live
observations and may be incomplete.

Windows, Linux, Android, and iOS remain experimental. Their CI artifacts and
source build instructions are available below; they are not runtime-qualified
release downloads. See [verification status](docs/testing.md).

## Build and run

Use the pinned Rust toolchain and committed `Cargo.lock`:

```sh
cargo build --release --locked
cargo run --release --locked
# Optional initial scope:
cargo run --release --locked -- /path/to/folder
```

Desktop targets are Windows with MSVC, macOS, and Linux with X11/Wayland.
Linux builds require a C/C++ compiler, pkg-config, libxkbcommon, Wayland, and EGL
development packages. The CI workflow includes the Ubuntu package list.
The full application is currently runtime-verified on macOS arm64 only.
Windows, Linux, macOS, Android APK, and iOS simulator/device builds have passed
CI. Check the latest workflow for the current commit and artifact downloads.

To package a desktop build:

```sh
python3 scripts/package-desktop.py
```

The script creates a ZIP and SHA-256 manifest in `dist/`. Extract the archive and
run `SpaceMosaic.app`, `spacemosaic.exe`, or `spacemosaic`. On Linux, restore the
executable bit with `chmod +x spacemosaic` if the extraction tool removed it.
macOS bundles include a multi-resolution project icon for Finder, Dock, and
About. The running desktop window also uses the bundled project icon.
The macOS arm64 archive includes third-party dependency notices and font licenses.
macOS packaging uses local ad-hoc signing; it is not Developer ID signing or
notarization. The first prerelease distributes the macOS arm64 archive only.

Verify downloads with `shasum -a 256 -c <manifest>` or, on Windows,
`Get-FileHash -Algorithm SHA256 <archive>`.

## Mobile development

Mobile applications use native eframe rendering and platform bridges, without
WebView. Rust recursively visits the selected tree using native metadata
cursors, with at most 32 entries and 256 KiB in each bridge response. This page
size does not limit the tree to 32 objects. Cancel stops further traversal and
retains a partial result; rescanning replaces the previous session.

The native **Accessible folder browser** reads the completed Rust result in
pages. It provides directory navigation, search, sorting, largest files,
file-type statistics, object details, copying/opening, and JSON/CSV export.
Its system controls provide an alternative to the egui canvas. VoiceOver and
TalkBack still require simulator/device testing; native controls alone are not
a conformance claim.

Android uses the Storage Access Framework and `DocumentsContract` with content
URIs. It requests read access only, retains a persistable grant when offered,
and handles provider failures. It does not request `MANAGE_EXTERNAL_STORAGE`,
access private application data, or promise a whole-device scan. On restart,
it restores the last retained tree only if the OS still grants read access.
Revocation is reported explicitly. SAF providers expose logical sizes only;
missing sizes remain unknown. Providers that report loading or incomplete
listings produce a partial result rather than an apparently complete total.
SAF does not expose reliable inode, hard-link, or filesystem-boundary identities.

```sh
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk --version 4.1.2 --locked
# Requires JDK 17 and ANDROID_HOME pointing to an Android SDK.
"$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" "platforms;android-36" "build-tools;36.0.0" "ndk;29.0.14206865"
bash scripts/build-android.sh
adb install -r dist/spacemosaic-debug.apk
adb shell am start -n dev.spacemosaic/dev.spacemosaic.StorageActivity
```

Minimum Android API: 26. The Gradle 9.8.2 wrapper and Android Gradle Plugin 9.4.1
are pinned. Developers accept SDK licenses when setting up their environment.
The APK uses a locally generated debug key, without personal signing secrets.

iOS uses the system document picker, security-scoped access, file coordination,
and immediately available metadata. Access is limited to the selected provider
folder. A scan retains security-scoped access until traversal ends or is
cancelled, then releases it. Rescanning reacquires access to the selected URL
within the current application session; bookmarks are not retained across
restarts. Symbolic links and aliases are not followed. Available allocation
metadata and hard-link counts do not establish unique APFS block ownership.
A full Xcode installation, iOS SDK, and Apple Silicon Mac are required.

Both platforms save JSON and CSV through the system document picker. An export
uses an application-private temporary file, which is removed after completion,
cancellation, or a reported failure. Cancelling the save dialog is distinct
from a failed write. Source files are never modified. Cloud-only entries are
not downloaded to measure their contents; unavailable metadata stays unknown.

```sh
rustup target add aarch64-apple-ios-sim aarch64-apple-ios
bash scripts/build-ios.sh simulator
open mobile/ios/SpaceMosaic.xcodeproj
# With a simulator already booted:
xcrun simctl install booted dist/ios-simulator/Build/Products/Release-iphonesimulator/SpaceMosaic.app
xcrun simctl launch booted dev.spacemosaic
```

Minimum iOS: 16. `bash scripts/build-ios.sh device` checks an unsigned device
build. It does **not** produce an application installable on arbitrary iPhones.
Physical devices require a development team, provisioning, and signing in
Xcode. No team IDs, profiles, certificates, or private keys are included.

## Privacy and safety

There are no accounts, telemetry, uploads, automatic updates, deletion tools,
cleanup actions, or user-supplied shell commands. Storage metadata stays in
memory unless you explicitly export it. Exports contain paths and file names;
review them before sharing. Android may retain an OS-managed read grant.
Cloud providers can use their own network connection to retrieve metadata;
SpaceMosaic does not request file-content downloads.

## Project documentation

- [Architecture and dependency policy](docs/architecture.md)
- [Testing and platform verification](docs/testing.md)
- [Performance measurement](docs/performance.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)
- [Code of conduct](CODE_OF_CONDUCT.md)
- [Changelog](CHANGELOG.md)
