# Testing and platform verification

A successful build, a simulator launch, and device testing are different forms
of evidence. Platform badges and release notes must not combine them.

## Current support

The desktop application builds and runs on macOS arm64. Alpha.2 was also
exercised on Windows 11 and Pixel 8 with Android 17; those reports exposed UI
and packaging regressions. Current fixes still need revalidation on that phone
and Windows installation. Windows, macOS, Linux,
Android APK packaging, and iOS simulator/device builds have passed GitHub
Actions. Consult the current workflow for each commit. Desktop runtime outside
macOS, mobile picker/lifecycle behavior, screen readers, and physical devices
still require verification. Recursive mobile analysis and native export are
implemented; host tests cover their Rust model and bridge protocol, not real
provider acceptance.

Current build outcomes and downloadable artifacts are recorded in
[GitHub Actions](https://github.com/0then0/spacemosaic/actions/workflows/ci.yml).

The `v0.1.0-alpha.3` prerelease distributes builds for all five platforms.
macOS arm64 is runtime-verified; Windows, Linux, Android, and iOS downloads remain
experimental. No mobile device support is certified. The full mobile acceptance
criteria remain open until runtime evidence is available. A published artifact
or successful build does not establish runtime qualification.

## Automated checks

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo test --locked --no-default-features
cargo build --release --locked
python3 scripts/package-desktop.py
python3 -m unittest discover -s tests -p 'test_*.py'
```

Tests use disposable directories and cover metadata traversal and aggregation,
unknown data, explicit cancellation, resource limits, session replacement,
symlinks/cycles and hard links on Unix, sparse allocation, result filtering,
sorting, JSON/CSV export, treemap geometry, hit regions, theme selection and
contrast, translation-key completeness, tree expansion/selection, desktop
tree/map placement, and pointer-based language selection. Regression checks
exercise resizing, text scaling, scroll access to the map, persistent selection
highlights, and nested/grouped selections. Phone regressions cover toolbar
height and complete export menus, safe-area settings after rotation/text scaling,
stable file-type columns during scrolling, and divider dragging in both themes.
File-type header tests click all three columns in both directions, check stable
ties, preserve unknown/overflow size metadata, and verify that sorting reuses
the existing scan result.
Breadcrumb tests cover long paths in all seven languages, both themes, phone and
desktop widths, and enlarged text. They check a single scrolling path row,
visible current-folder/path labels, ancestor navigation, and the separate Back action.
The grouped-map regression uses 25,000 synthetic items: one tile/button click
or keyboard activation exposes the complete virtualized list, supports search
and sorting, and returns to the original map with the selection preserved.
Preference tests save and reload all language/theme combinations using disposable
files, reconstruct the UI after language/theme selection, recover from malformed
or unsupported settings, and preserve normal window bounds across maximization,
minimization, text zoom, changed display scaling, and disconnected monitors.
Failed-write tests verify retry throttling, a visible failure notice, and recovery
when the settings directory becomes writable. Partial-scan fixtures reproduce
an unread `.agents` directory under the object limit, then scan it separately
and verify its exact 125,739-byte logical total. Tests distinguish empty/unknown
sizes, both resource limits, cancellation, and backward-compatible JSON decoding;
native mobile result labels and explanations are checked in all seven languages.

Provider fixtures exercise recursive multi-page traversal, opaque URI export,
unknown sizes, incomplete provider pages, denied children, cycles, resource
limits, cancellation before cursor registration, blocked cursor interruption,
and late cleanup from an older scan. Native result tests cover paging, adaptive
byte limits, filename search independent of URI syntax, stale commands, and
save cancellation/temporary-file cleanup. These tests use synthetic transports;
they do not substitute for native integration tests.

Packaging tests validate portable archive contents and every manifest entry.
macOS checks mount the read-only DMG, inspect its Applications shortcut and
embedded notices, and verify the app's icon, resources, and code-signature seal.
Windows checks inspect the PE GUI subsystem, silently install Setup.exe into
a disposable directory, verify shortcuts and the uninstall entry, then uninstall
and confirm unrelated files are preserved. Icon checks validate adaptive/round
resource selection and the glyph's circular safe zone.
Linux checks inspect TAR executable modes and extract the DEB into a disposable
directory to verify paths, launcher/icon files, version ordering, and ownership.
Android packaging verifies APK signatures and native libraries for both ABIs;
iOS packaging checks version metadata and license resources. These are package
checks, not proof that the application launches on a device. Invalid UTF-8 filesystem names are
tested on Linux; macOS filesystems may reject creation of such names.

`cargo test` does not validate real provider behavior, operating-system dialogs,
filesystem snapshots, screen readers, or every possible cloud implementation.

CI also runs Android APK compilation/packaging and iOS simulator/device builds.
The iOS Rust step emits a static library; Xcode resolves its Swift callbacks
when linking the final application. Those jobs do not require personal signing
certificates. The repository does not publish releases automatically.

## Manual acceptance scenarios

Use a disposable folder with a known-size file, an empty file, nested folders,
a file without an extension, and long/non-Latin names. Verify:

1. Choose and drag a desktop folder; keep the window responsive during scanning.
2. Cancel and confirm a partial result; replace a scan and reject late results.
3. Compare expected aggregates, largest files, extension groups, and exports.
4. Navigate through breadcrumbs, Back, list controls, and treemap selection.
5. Inspect unreadable/disappearing objects, links, and filesystem boundaries.
6. Switch System → Light → Dark → System, then change OS appearance.
   Restart after each choice and after changing the language. Move and resize
   the desktop window, restart, then repeat while maximized and after disconnecting
   its previous display. On Wayland, verify size only; placement belongs to the compositor.
7. Check all seven languages at narrow/wide widths, both themes, and large text.
8. Use keyboard-only navigation and an actual desktop screen reader.

For Android and iOS, separately verify picker cancellation, revoked access,
offline/unavailable providers, background/foreground, rotation, and repeated
selection. Include more than 32 immediate children and several nested folders;
compare totals, largest files, file types, and exported JSON against the fixture.
Cancel a scan while a provider query is blocked, then start a new selection and
rescan it. Verify that late callbacks neither replace results nor close the new
cursor. Test missing sizes, loading/incomplete responses, and cloud-only files
without triggering downloads. Confirm restored Android grants and revoked grants
on restart; on iOS verify balanced access across repeated scans.

Save JSON and CSV to a local and provider destination. Cancel the save picker,
exercise a failed write, and confirm that a partial write is not reported as
success. Use TalkBack/VoiceOver to choose a scope, read progress, navigate nested
directories, search, inspect sizes/errors, change settings, and export through
the native browser. Do not infer success from native widget types or a
syntactically valid bridge.

## Reproducing mobile builds

Follow README prerequisites, then run:

```sh
bash scripts/build-android.sh
bash scripts/build-ios.sh simulator
bash scripts/build-ios.sh device
```

Missing Android SDK/NDK, target standard libraries, or Xcode SDKs are environment
blockers, not passing checks. Swift parse-only checks do not type-check UIKit.
On a physical iPhone, configure provisioning and signing in Xcode; an unsigned
device build cannot be installed as a generic downloadable application.

## Recording verification

For release qualification, record the commit, OS version, architecture,
toolchain, build command, device/simulator model, and observed result. Include
signing/notarization status and artifact SHA-256. Never include private paths,
provider grants, signing secrets, or personal file names in public test reports.
