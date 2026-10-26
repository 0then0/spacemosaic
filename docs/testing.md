# Testing and platform verification

A successful build, a simulator launch, and device testing are different forms
of evidence. Platform badges and release notes must not combine them.

## Current support

The desktop application builds and runs on macOS arm64. Windows, macOS, Linux,
Android APK packaging, and iOS simulator/device builds have passed GitHub
Actions. Consult the current workflow for each commit. Desktop runtime outside
macOS, mobile picker/lifecycle behavior, screen readers, and physical devices
still require verification. Recursive mobile analysis and native export are
implemented; host tests cover their Rust model and bridge protocol, not real
provider acceptance.

Current build outcomes and downloadable artifacts are recorded in
[GitHub Actions](https://github.com/0then0/spacemosaic/actions/workflows/ci.yml).

The `v0.1.0-alpha.1` prerelease distributes macOS arm64 binaries only. No mobile
device support is certified. The full mobile acceptance criteria remain open
until runtime evidence is available. Other platforms remain experimental CI
artifacts; a successful build does not qualify them for a supported release.

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
highlights, and nested/grouped selections.

Provider fixtures exercise recursive multi-page traversal, opaque URI export,
unknown sizes, incomplete provider pages, denied children, cycles, resource
limits, cancellation before cursor registration, blocked cursor interruption,
and late cleanup from an older scan. Native result tests cover paging, adaptive
byte limits, filename search independent of URI syntax, stale commands, and
save cancellation/temporary-file cleanup. These tests use synthetic transports;
they do not substitute for native integration tests.

Packaging tests on
macOS validate the bundle icon, archived resources, and code-signature seal. Invalid UTF-8 filesystem names are
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
