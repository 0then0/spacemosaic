# Changelog

## Unreleased

### Added

- Background desktop scanning with progress, cancellation, explicit partial
  results, resource limits, and a configurable filesystem-boundary policy.
- Directory navigation, breadcrumbs, name/size sorting, result search, largest
  files, file-type statistics, object details, and system file-manager actions.
- Interactive squarified treemap with shared selection and grouped small items.
- Versioned JSON exports and CSV file-type summaries through system save dialogs.
- System, light, and dark appearance with a restrained neutral visual style.
- Seven interface languages and a bundled offline Japanese font.
- Recursive Android SAF and iOS document-picker analysis through bounded native
  metadata cursors, with cancellation, unknown sizes, and explicit partial results.
- Native mobile result browsing with paging, directory navigation, search, largest
  files, file-type summaries, settings, and system JSON/CSV export.
- Android retained-grant restoration and iOS scoped access for repeated scans.
- Pinned dependencies, desktop/mobile CI build checks, artifact packaging, and
  SHA-256 manifests.

### Changed

- Placed an expandable directory tree above the colored, labeled space map,
  with a resizable divider on wide windows.

- Replaced the primary UI font with bundled Inter Regular 400 and Inter Medium
  500 for headings, retaining Hack monospace and Noto Sans JP fallback.

### Fixed

- Result panes stay separate after window resizing and text scaling, with a
  scrollable fallback when the viewport cannot fit both panes.
- Selected tree rows remain visibly highlighted without hover, and nested or
  grouped selections mark their containing map tile with the selected path.

- Language selection remains open in a dedicated Settings window.
- Desktop windows use the project icon; macOS packages include a complete
  multi-resolution ICNS resource and bundle icon metadata.

- iOS Rust builds request a static library instead of prematurely linking Swift
  symbols into a dynamic library.
- iOS picker presentation supports the legacy window lifecycle used by winit.
- Native metadata results use the same generation checks as the Rust UI;
  separate scan IDs prevent stale cursor cleanup from interrupting a rescan.
- Search matches file names even when opaque provider URIs do not contain them.
- Save cancellation remains distinct from failure; export temporary files are
  removed after completion, and JSON serialization streams to disk.

No release has been published. See the platform verification documentation for
build and runtime coverage.
