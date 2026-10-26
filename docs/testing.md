# Testing and platform verification

A successful build, a simulator launch, and device testing are different forms
of evidence. Platform badges and release notes must not combine them.

## Current support

The desktop application builds and runs on macOS arm64. Windows and Linux have
CI build jobs; their runtime behavior requires verification. Android and iOS
have native source integrations and CI build jobs, but do not yet have verified
picker, lifecycle, accessibility, or physical-device coverage. They expose
metadata browsing rather than the desktop application's recursive analysis.

No release is published and no mobile device support is certified. The full
mobile acceptance criteria remain open until runtime evidence is available.

## Automated checks

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo test --locked --no-default-features
cargo build --release --locked
```

Tests use disposable directories and cover metadata traversal and aggregation,
unknown data, explicit cancellation, resource limits, session replacement,
symlinks/cycles and hard links on Unix, sparse allocation, result filtering,
sorting, JSON/CSV export, treemap geometry, hit regions, theme selection and
contrast, and translation-key completeness. Invalid UTF-8 filesystem names are
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
selection. Use TalkBack/VoiceOver to open the native accessible browser, choose
a scope, read status and metadata, and close it. Do not infer success from
native widget types or a syntactically valid bridge.

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
