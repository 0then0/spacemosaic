# Testing and platform verification

A successful build, a simulator launch, and device testing are different forms
of evidence. Platform badges and release notes must not combine them.

## Current support

The desktop application builds and runs on macOS arm64. Alpha.2 was also
exercised on Windows 11 and Pixel 8 with Android 17; those reports exposed UI
and packaging regressions. The user subsequently confirmed relatively stable
operation on Pixel 8 with Android 17. This does not verify signing, in-place updates, screen readers or
every provider. Windows fixes still need revalidation on that installation.
Windows, macOS, Linux, Android APK packaging, and iOS simulator/device builds have passed GitHub
Actions. Consult the current workflow for each commit. Desktop runtime outside
macOS, mobile picker/lifecycle behavior, screen readers, and physical devices
still require verification. Recursive mobile analysis and native export are
implemented; host tests cover their Rust model and bridge protocol, not real
provider acceptance.

Current build outcomes and downloadable artifacts are recorded in
[GitHub Actions](https://github.com/0then0/spacemosaic/actions/workflows/ci.yml).

The [v0.2.0 release](https://github.com/0then0/spacemosaic/releases/tag/v0.2.0)
distributes builds for all five platforms. macOS arm64 has runtime verification,
and Pixel 8 with Android 17 has user-reported runtime testing. Windows, Linux
and iOS remain experimental. Full mobile acceptance, including accessibility
and in-place upgrades, remains open. A published artifact or successful build
does not establish runtime qualification.

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

## Saved results and comparison

`tests/comparison.rs`, `tests/import.rs` and the UI tests cover: growth, a
missing object and an addition totalling +650 MiB without double-counting
folders; an unreadable-now folder that is unknown and not freed space; an
incomplete baseline that cannot declare objects new; one unknown size giving an
unknown delta; equal sizes reported only as equal sizes; renames, file/folder
replacements and type changes; scope verdicts (matching, insufficient,
incompatible, provider authority and root spelling, platform, schema 1);
exact-name matching for invalid UTF-8, case, Unicode forms and duplicate
provider names; full-range `u64` deltas and overflow; a generated-tree property
check that the net total equals the difference of root sizes and containers sum
their children once; schema 1 import with unknowns intact; rejection of damaged,
cyclic, oversized, endless, deeply nested, contradictory and unsupported
documents; export → delete source → import → compare; sequential opens and
cancellation; the saved banner, no recorded-location actions, comparison rows in
all seven languages, both themes, desktop/phone sizes and 1.6× text, phone Back,
swap/confirmation flow, and a 120,000-object comparison. Mobile result pages and
open requests are tested on the host. The Rust code behind the Android and iOS
bridges type-checks for `aarch64-linux-android` and `aarch64-apple-ios`
(`cargo check --lib --target …`); that is not a build, an installation or a
device test. The Java and Swift changes were only syntax-checked
(`swiftc -parse`; `javac` cannot resolve Android classes without an SDK).

### Saved-result acceptance on devices (not yet performed)

1. Scan a disposable folder with a nested folder and an unreadable one; export JSON.
2. Close the app completely, delete or rename the folder, reopen, **Open saved
   result**, pick the JSON (from a local and a provider location). Expect the
   banner, tree, sizes, unknown values and partial flag; no scan starts; there is
   no "Show in file manager" and Rescan stays unavailable.
3. Restore the folder with changes (grow one file, delete one, add one, make a
   folder unreadable), scan it, choose **Compare with current scan**. Check
   direction, statuses, totals, "not a total" wording with the unreadable folder,
   and **Show in current/baseline result**.
4. Pick a damaged and an oversized file; expect a clear error and no change.
5. Android: system Back from details, comparison and saved result; TalkBack on the
   native browser **Changes** mode. iOS: VoiceOver on the same, and the picker's
   copy semantics. Rotate and use large text throughout.

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

## Android distribution changes

Run `python3 -m unittest discover -s tests -p test_android_distribution.py` for
signing-input validation, monotonic version codes, identity/version rejection,
and the release fail-fast path. The debug CI remains independent of credentials.
[Android distribution](android-distribution.md) records the public APK certificate
audit and the separate disposable A-to-B update procedure. An APK signature or
packaging check does not prove settings/SAF retention across installation.

Local signing validation on 2026-10-27 used macOS arm64, Temurin JDK 17.0.20.1,
Gradle 9.8.2, AGP 9.4.1 and Android SDK Build Tools 36.0.0. Twelve targeted
Python tests passed. A disposable source copy built debug without release
credentials, and direct `assembleRelease` without credentials failed with the
explicit signing error. Two release APKs (codes 4 and 5) were built with the same
temporary test key; `apksigner`, manifest/version, non-debuggable, icon, ABI and
license checks passed for both. Their certificates matched and codes increased.
The private test key was then removed. This isolated Gradle/signing check reused
the published alpha.3 Rust native libraries; it was not a fresh Rust/NDK build.
No emulator was available, so installation, preference/URI retention and
post-update scan/export remain unverified. Exact remaining commands and UI
checks are in the distribution guide, without expanding the general mobile
acceptance checklist.

## Phone workspace

Headless egui tests cover the start/result transition, single-tap folder opening,
shared list/map selection, object details, Back navigation and scan cancellation.
They check viewport bounds in all seven languages, both themes, increased text
scale and portrait/landscape sizes. The large-group regression also verifies
that the phone paints only visible rows from a 25,000-object list. Native
provider tests keep display names separate from storage locators.

Desktop-rendered phone-size previews use disposable synthetic folders; they are
layout evidence, not Android/iOS device acceptance. Device checks still need to
confirm system Back, safe areas/keyboard, picker return and accessibility with
the actual platform bridges.

Regression tests also cover opening file types from either result view, hiding
unsupported extension search, using the scanned root for global result sizes
and bars, and cancelling a second pending folder selection after nested or
grouped navigation.

## Adaptive scan memory verification (2026-11-02)

Host verification on macOS 15.7.9 arm64 / Rust 1.99.1:

- `cargo fmt --all --check`, `cargo clippy --locked --all-targets -- -D warnings`,
  `cargo test --locked --all-targets`, `cargo test --locked --no-default-features`
  and `cargo build --release --locked` passed.
- The full GUI suite passed 132 tests, with one explicit performance test ignored
  by default; that measurement was separately run successfully in release mode.
  The no-default-features suite passed 82 tests. Tests inject readings/pressure,
  cover nested v1/v2 cgroups and unlimited/error values, process metric matching,
  input bounds and old preference migration, budget admission before growth,
  retained unknowns/aggregates, cursor interruption/cleanup, cancellation,
  obsolete pressure IDs, capacity overflow, JSON/CSV reasons, translations and
  a 500,011-object generated scan. Windows 64-bit FFI structure sizes and both
  Linux/Windows sampler bodies type-check on the host, without invoking foreign
  APIs; this is not target/runtime qualification.
- `cargo check --locked --lib --target aarch64-linux-android` and
  `cargo check --locked --lib --target aarch64-apple-ios` passed.
  `swiftc -parse mobile/ios/StorageBridge.swift` passed. Android Java could not
  be compiled because the Android SDK/NDK is absent; iOS UIKit/Swift could not be
  type-checked/linked because only Command Line Tools, not Xcode/iOS SDKs, are
  installed. No mobile low-memory notification, provider cancellation or native
  settings flow was exercised on a device.
- After `python3 scripts/package-desktop.py`,
  `python3 -m unittest discover -s tests -p 'test_*.py'` passed all 21 applicable
  tests (25 total, four platform-specific skips). Fresh local ZIP/DMG artifacts,
  the ad-hoc resource seal, read-only DMG installation layout and manifest were
  verified outside the host sandbox, which prevents the required image mount.
  The earlier stale-archive and mount failures are resolved. These packages
  were built for verification only; no release was published.

Actual scan/peak-memory/frame/cancel measurements and reproduction commands are
in [performance.md](performance.md). All fixtures are disposable; no real OOM
or scan of personal folders is required.

Remaining checks in equipped environments:

```sh
# Windows and Linux runners: native sampler, process/job/cgroup pressure behavior
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
# Android SDK/NDK and JDK 17
bash scripts/build-android.sh
# Full Xcode/iOS SDKs and installed Rust simulator target
bash scripts/build-ios.sh simulator
bash scripts/build-ios.sh device
```

On Android/iOS, choose Automatic and Manual, restart to check persistence, scan
a disposable provider tree, simulate a memory warning (not actual exhaustion),
and confirm partial counts/reasons, JSON/CSV export and separate-folder scans.
Interrupt a blocked provider; replace/reselect/rescan and inject an old-ID
callback, verifying that it neither stops nor closes the newer cursor. Exercise
Android 26–33 legacy trim callbacks separately from API 34+ MemoryInfo fallback.
On iOS check footprint/headroom diagnostics and balanced security-scoped access.
On desktop, check a constrained Windows job and Linux v1/v2 nested cgroups with
synthetic fixtures and ample real RAM. Compilation alone does not qualify any
of these native behaviors.

### Review follow-up

The review covered the task's complete working-tree change relative to
`70bfc92`, including new memory modules and documentation; no dependency or
configuration changes were introduced. One correction pass addressed:

- **R1 (acceptance blocker):** successive allocations could reuse the same
  sampled system/process/group headroom. A deterministic test reproduced the
  failure with 100 MiB headroom, a 64 MiB reserve and two 24 MiB growths. The
  monitor now charges total estimated growth since the sample, rebases only at
  retained allocations after a fresh sample, and preserves that anchor across
  pressure-only checks. The test covers all three headroom sources, rebasing,
  unknown values and the unchanged selected budget. Provider hash growth is
  distinguished from retained auxiliary memory when establishing that anchor.
- **R2 (nonblocking):** Android separate-folder scanning retained the old search
  and result mode and reset navigation even when Rust rejected the command.
  Accepted scans now share Rescan's navigation/filter reset; rejected commands
  preserve the current view. This correction is verified by code-path analysis;
  Android SDK/device verification remains outstanding. Specifically test an
  active search and file-type/changes mode, then a separate-folder scan, and a
  rejected stale/saved-result command.

The R1 reproducer failed before the fix and passed afterwards. The full host
Rust suites, clippy, mobile Rust target checks and Swift parse were rerun after
the correction. These do not prove native UI/device behavior.
