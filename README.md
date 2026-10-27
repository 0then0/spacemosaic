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
  Click a file-type column header to sort by extension, known size, or file count;
  click it again to reverse the order.
- Search the completed result and sort by name or logical size without reading
  the disk again.
- Select an object in the tree or squarified treemap to inspect the same details.
  Tiles use stable colors for folders and file extensions, with explicit folder
  labels so navigation does not depend on color. Selecting a nested file marks
  its containing tile and shows the selected path above the map.
  Open **Smaller items** once to browse the entire group in a virtualized list,
  search or sort it, then use **Back** to return to the map.
  The labeled path bar separates ancestor links with `›` and highlights the
  current folder. Long paths scroll horizontally; Back remains a separate action.
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

An unread or incomplete directory is not an empty directory. When no bytes are
known, the UI displays **Unknown** instead of `0 B`. Partial results identify
cancellation, the object limit, or the metadata memory limit. Desktop object
details offer **Scan this folder separately** for incomplete directories; this
starts a new scan with that folder as the selected scope.

Desktop scans are limited to 500,000 objects and an estimated 256 MiB metadata
budget; mobile scans use 100,000 objects and 64 MiB. Reaching either limit leaves
an explicit partial result. These budgets guard stored node data, not total
process memory. Provider cursors, filtering, font atlases, and the UI require
additional memory. JSON exports stream to a temporary file before saving.

## Download the alpha

[v0.1.0-alpha.3](https://github.com/0then0/spacemosaic/releases/tag/v0.1.0-alpha.3)
provides downloads for all five platforms. macOS arm64 has runtime verification;
the other platform builds remain experimental.

| Platform | Architecture | Download | Status |
| --- | --- | --- | --- |
| Windows | x86-64 | [![Download Windows installer](https://img.shields.io/badge/Setup.exe-0078D4?style=for-the-badge&logo=data%3Aimage%2Fsvg%2Bxml%3Bbase64%2CPHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCI%2BPHBhdGggZmlsbD0id2hpdGUiIGQ9Ik0xIDFoMTB2MTBIMXpNMTMgMWgxMHYxMEgxM3pNMSAxM2gxMHYxMEgxek0xMyAxM2gxMHYxMEgxM3oiLz48L3N2Zz4%3D&logoColor=white)](https://github.com/0then0/spacemosaic/releases/download/v0.1.0-alpha.3/spacemosaic-windows-amd64-setup.exe) [![Download Windows portable ZIP](https://img.shields.io/badge/Portable%20ZIP-0078D4?style=for-the-badge&logo=data%3Aimage%2Fsvg%2Bxml%3Bbase64%2CPHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCI%2BPHBhdGggZmlsbD0id2hpdGUiIGQ9Ik0xIDFoMTB2MTBIMXpNMTMgMWgxMHYxMEgxM3pNMSAxM2gxMHYxMEgxek0xMyAxM2gxMHYxMEgxM3oiLz48L3N2Zz4%3D&logoColor=white)](https://github.com/0then0/spacemosaic/releases/download/v0.1.0-alpha.3/spacemosaic-windows-amd64.zip) | Experimental · unsigned |
| macOS | Apple Silicon (arm64) | [![Download macOS DMG](https://img.shields.io/badge/DMG-555555?style=for-the-badge&logo=apple&logoColor=white)](https://github.com/0then0/spacemosaic/releases/download/v0.1.0-alpha.3/spacemosaic-darwin-arm64.dmg) [![Download macOS ZIP](https://img.shields.io/badge/ZIP-555555?style=for-the-badge&logo=apple&logoColor=white)](https://github.com/0then0/spacemosaic/releases/download/v0.1.0-alpha.3/spacemosaic-darwin-arm64.zip) | Alpha · ad-hoc signed |
| Linux | x86-64 · Ubuntu 24.04 build | [![Download Linux DEB](https://img.shields.io/badge/DEB-FCC624?style=for-the-badge&logo=linux&logoColor=black)](https://github.com/0then0/spacemosaic/releases/download/v0.1.0-alpha.3/spacemosaic-linux-x86_64.deb) [![Download Linux portable TAR.GZ](https://img.shields.io/badge/Portable%20TAR.GZ-FCC624?style=for-the-badge&logo=linux&logoColor=black)](https://github.com/0then0/spacemosaic/releases/download/v0.1.0-alpha.3/spacemosaic-linux-x86_64.tar.gz) [![Download Linux ZIP](https://img.shields.io/badge/ZIP-FCC624?style=for-the-badge&logo=linux&logoColor=black)](https://github.com/0then0/spacemosaic/releases/download/v0.1.0-alpha.3/spacemosaic-linux-x86_64.zip) | Experimental |
| Android 8.0+ | arm64-v8a / x86_64 | [![Download Android test APK](https://img.shields.io/badge/Test%20APK-3DDC84?style=for-the-badge&logo=android&logoColor=black)](https://github.com/0then0/spacemosaic/releases/download/v0.1.0-alpha.3/spacemosaic-debug.apk) | Experimental · debug signed |
| iOS 16+ | arm64 · device and simulator | [![Download unsigned iOS device build](https://img.shields.io/badge/Device%20%C2%B7%20signing%20required-555555?style=for-the-badge&logo=apple&logoColor=white)](https://github.com/0then0/spacemosaic/releases/download/v0.1.0-alpha.3/spacemosaic-device.zip) [![Download iOS simulator app](https://img.shields.io/badge/Simulator-555555?style=for-the-badge&logo=apple&logoColor=white)](https://github.com/0then0/spacemosaic/releases/download/v0.1.0-alpha.3/spacemosaic-simulator.zip) | Experimental · device signing required |

For any release download, save [SHA256SUMS](https://github.com/0then0/spacemosaic/releases/download/v0.1.0-alpha.3/SHA256SUMS)
into the same directory. Verify only the files you downloaded:

```sh
shasum -a 256 -c --ignore-missing SHA256SUMS
```

On macOS, open the DMG and drag `SpaceMosaic.app` to **Applications**, or extract
the ZIP and open `spacemosaic-darwin-arm64/SpaceMosaic.app`. The app is ad-hoc
signed, without Developer ID signing or notarization. macOS may require approval
in **System Settings → Privacy & Security** after the first launch attempt.

The Windows `spacemosaic-windows-amd64-setup.exe` installer installs into the current
user's profile without administrator rights, adds a Start menu shortcut, and
registers an uninstaller. A portable ZIP is also available.

On Windows, extract the complete portable ZIP and run `spacemosaic.exe`.
The executable has no Authenticode signature. On Ubuntu 24.04, install the DEB
with `sudo apt install ./spacemosaic-linux-x86_64.deb`, or extract a portable
archive and run `spacemosaic`. Portable Linux builds still require compatible
system libraries; the DEB declares their package dependencies.

The currently published Android debug APK can be installed with the system
package installer or `adb install`. It uses a debug signing key. The published
alpha.2 and alpha.3 certificates differ; see the
[compatibility audit](docs/android-distribution.md#published-apk-compatibility).
Signatures can differ between CI runs, so in-place updates from earlier test builds are not guaranteed.

The iOS device ZIP contains an **unsigned app**, not an installable IPA. Physical
iPhones require provisioning and signing in Xcode. The simulator ZIP is only
for an Apple Silicon iOS simulator. The complete Xcode project and Rust sources
are available in the release's source archives. See [mobile build instructions](#mobile-development).

All downloads are attached directly to the release; they do not depend on CI
artifact retention. Build validation does not establish runtime support.
See [verification status](docs/testing.md).

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
The application runs on macOS arm64. The user has also confirmed relatively
stable operation on Pixel 8 with Android 17; this does not verify APK upgrades,
screen readers or every document provider.
Windows, Linux, macOS, Android APK, and iOS simulator/device builds have passed
CI. Check the latest workflow for the current commit and artifact downloads.

To package a desktop build, use `hdiutil` on macOS, `dpkg-deb` on Ubuntu,
and Inno Setup **7.1.0** on Windows. In PowerShell, prepare the verified compiler
before packaging:

```powershell
. ./scripts/setup-inno.ps1
```

The script verifies the official compiler download's pinned SHA-256. Package with:

```sh
python3 scripts/package-desktop.py
```

The script creates a portable ZIP and SHA-256 manifest in `dist/`, plus a macOS
DMG or Linux TAR.GZ and DEB on the corresponding host. Extract the archive and
run `SpaceMosaic.app`, `spacemosaic.exe`, or `spacemosaic`. On Linux, restore the
executable bit with `chmod +x spacemosaic` if the extraction tool removed it.
macOS bundles include a multi-resolution project icon for Finder, Dock, and
About. The running desktop window also uses the bundled project icon.
All packages include third-party dependency notices and font licenses; macOS
apps retain them inside the application bundle after drag-and-drop installation.
macOS packaging uses local ad-hoc signing; it is not Developer ID signing or
notarization. Windows CI packages include an unsigned per-user installer and
an unsigned portable archive.

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
The default command produces a development debug APK. For a signed distribution
APK with a persistent owner-provided identity, use the separate
[Android release build and upgrade guide](docs/android-distribution.md).
It documents signing inputs, CI isolation, versionCode, certificate verification
and migration from published debug APKs. No production signing key is bundled.

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
Language and appearance preferences are saved locally in `settings.json`.
Desktop also remembers the window's normal size, position, and maximized state.
The System language and appearance options continue to follow the operating
system after restarting. Window placement remains controlled by the compositor
on Wayland; mobile window placement is managed by Android and iOS.
Settings live in `%APPDATA%\SpaceMosaic` on Windows,
`~/Library/Application Support/SpaceMosaic` on macOS and iOS (inside the app
sandbox on iOS), `$XDG_CONFIG_HOME/spacemosaic` or `~/.config/spacemosaic` on
Linux, and the app's private internal directory on Android. No scan results,
file names, paths, or content URIs are stored in this settings file.
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
