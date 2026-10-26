# Architecture

SpaceMosaic is one Rust crate with platform bridges. Storage metadata, result
processing, geometry, and rendering have separate responsibilities without a
multi-crate framework.

## Rust modules

- `scan`: local and provider metadata traversal, aggregate size measures, bounded background
  scheduling, result filtering, file-type summaries, and export.
- `treemap`: squarified rectangle layout and hit testing, independent of egui.
- `bridge`: picker acknowledgements and generation-based isolation.
- `provider`: bounded native metadata pages behind the `scan::Storage` cursor.
- `mobile`: paged result access, native commands, and asynchronous save handoff.
- `i18n`: embedded catalogs, language negotiation, and display formatting.
- `theme`: paired light/dark palettes and the system-theme preference.
- `ui`: navigation, shared selection, controls, and rendering.

One scanner worker processes requests. At most one replacement request waits;
submitting another cancels the active session and replaces the pending request.
Each job has its own result channel, cancellation token, and progress snapshot.
The UI only polls its current job, so an old result cannot replace a new one.
Desktop filesystem calls cannot be forcibly interrupted; the worker checks
cancellation between operations. Native providers additionally cancel Android
query signals or iOS file coordination. Providers may still take time to return.
Progress publication is throttled to 100 ms.

Nodes have stable indices within a result, with parent indices preceding
children. A reverse pass aggregates size measures without recursive stack use.
Known bytes, unknown measurements, and overflow are separate fields. Hard links
are counted per path. Symlinks and Windows reparse points are not followed.
Native path bytes/UTF-16 are retained in JSON alongside a human-readable path.

The UI caches filtered rows and map membership when navigation or filters
change. It virtualizes the object list and paints at most 96 map rectangles;
remaining positive-size objects expand through a grouped list. Zero/unknown
weights have no invented rectangle area. Selection remains available in the
object list and details panel.

## Native storage access

Desktop uses `rfd` for directory and export dialogs. Linux uses the desktop
portal backend rather than a bundled GTK application. File-manager actions
invoke fixed system applications with argument arrays, never shell command
strings assembled from file names.

Android uses a `NativeActivity` bridge with SAF tree selection, opaque content
URIs, read-only grants, and query cancellation signals. Retained grants are
checked before restoring the last selected tree. iOS uses UIKit document
picking, balanced security-scoped access, immediately available metadata-only
file coordination, and a temporary file presenter. Picker presentation supports
both scene windows and the legacy window used by winit 0.30.

Rust controls recursive traversal through `scan::Storage`. Each native cursor
opens one directory and returns pages of at most 32 entries and 256 KiB. Rust
aggregates directory sizes from children, retains unknown values and partial
errors, rejects repeated directory locators, and never follows reported links.
Provider URIs remain opaque; Android URIs are never converted to POSIX paths.
Provider identity cannot prove unique block ownership or enforce desktop mount
boundaries. Android allocation sizes remain unknown.

Picker generations isolate selections; distinct monotonic scan IDs also
isolate rescans of the same selection. Late close/cancel callbacks cannot
terminate a newer cursor. Cancellation is recorded before cursor registration
to cover races with queued work. Mobile metadata storage is capped at 100,000
nodes and an estimated 64 MiB, independently of the bounded page protocol.

Native list controls page through the same completed Rust result. They do not
walk storage separately. Requests run off the main thread and coalesce during
search or progress updates. Native commands have a bounded queue and reject
stale picker generations and result-row actions during rescanning.

Exports stream JSON or write the CSV summary to an app-private temporary file.
Android uses `ACTION_CREATE_DOCUMENT` and copies only this export to the
selected output URI. iOS uses a document export picker. Completion, cancellation,
and failure have separate outcomes; matching callbacks remove temporary files.
No source-provider content is read or modified by export.

## Themes and accessibility

The system preference delegates appearance selection to egui's OS theme input;
light and dark choices explicitly override it. Both palettes use neutral
surface steps, no shadows, and a sage section-label accent. Typography and
contrast are tested separately from the storage logic. Animations are disabled.

Desktop enables AccessKit. Mobile uses native browser controls because the
selected egui integration does not supply a complete cross-platform mobile
accessibility adapter. Native controls still require VoiceOver/TalkBack testing;
this project does not claim platform-wide accessibility conformance.

## Dependency updates

The toolchain is pinned to Rust 1.99.1. The egui family is pinned to 0.36.2;
fastframe is pinned to commit `2b04c0eed57e450461a5bef61fdebc93ed7d7226`.
Only `fastframe-i18n` is used, for system-language detection and BCP 47 tags.
No fastframe shell, tray, updater, or system-font discovery module is included.
Desktop-only and mobile-only dependencies are target-specific.

For a fastframe update, review the relevant upstream module, MSRV, and egui
versions; change the immutable commit and lockfile together. Run `cargo tree
-i egui`, format, Clippy, tests, desktop builds, and both mobile build scripts.
Repeat picker and accessibility checks when integration changes. A git `rev`
may need a manual update even when Dependabot has no pending PR.

Dependabot runs monthly with two open version-update PRs per ecosystem. Cargo,
GitHub Actions, and Android Gradle tooling group compatible minor/patch changes;
major changes remain separate. Security updates are not disabled.

Update Gradle wrapper scripts/JAR, distribution URL, and its official SHA-256
together, after checking the Android Gradle Plugin compatibility requirements.
The current wrapper JAR comes from Gradle v8.13.0, SHA-256
`81a82aaea5abcc8ff68b3dfcb58b3c3c429378efd98e7433460610fecd7ae45f`.

## Reference APIs

- [eframe 0.36.2](https://docs.rs/eframe/0.36.2/eframe/)
- [fastframe](https://fastframe.dev/)
- [Android Storage Access Framework](https://developer.android.com/training/data-storage/shared/documents-files)
- [Apple directory access](https://developer.apple.com/documentation/uikit/providing-access-to-directories)
- [Apple metadata-only coordination](https://developer.apple.com/documentation/foundation/nsfilecoordinator/readingoptions/immediatelyavailablemetadataonly)
- [Apple accessibility guidelines](https://developer.apple.com/design/human-interface-guidelines/accessibility)
- [Android accessibility guidelines](https://developer.android.com/guide/topics/ui/accessibility/apps)
