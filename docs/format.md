# Result format and import

Exports are UTF-8 JSON documents describing one observed scope. Local scans
with folder exclusions write **schema 4**. Scans without exclusions write
**schema 3**. The app reads schemas 1 through 4. Field names, enum values and
numbers are machine-readable and independent of the display language.

## Schema 4

Schema 4 extends schema 3 with a required, nonempty `exclusions` array for local
filesystem results. Older readers reject version 4 so they cannot mistake an
excluded subtree for a missing one. Each entry has two fields.

- `path` is a UTF-8 root-relative directory path with `/` separators. Input
  accepts both `/` and `\` on Windows. Repeated separators and `.` are
  collapsed. A `..` component removes the preceding component. Absolute paths,
  drive prefixes on Windows, paths that escape the root, and the root itself
  are invalid. Paths use exact case and spelling, without Unicode normalization.
  The array is sorted by components, with duplicate and descendant rules removed.
  Windows components follow the [Win32 naming rules](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file).
- `skipped` is true if the scanner encountered a directory at that path and
  skipped it before listing its contents. False means no skip was observed. The
  path may be absent, name another kind of object, or lie beyond an incomplete
  listing. The path still defines the scope for comparison.

Skipped directories and their descendants have no nodes and contribute no
bytes or file counts. `complete` and `listing_complete` cover the remaining
area. Intentional exclusions leave error and stop flags to describe failures,
boundaries, cancellation and limits within that area. Schemas 1 through 3 did
not support folder exclusions and keep their original completeness guarantees.

## Schema 3

Schema 3 retains schema 2 tree/identity semantics and adds `memory_pressure`,
`process_memory_limit` and `capacity_limit` to `limit_reason`, alongside legacy
`node_limit` and `memory_limit`. `cancelled` remains separate. Schema 1/2 readers
reject schema 3 rather than misinterpret a new reason. Opening and re-exporting
an old result retains its original version and precision.

Optional `memory` diagnostics record `budget_bytes`, `estimated_scanner_bytes`,
`baseline_process_bytes`, `baseline_process_metric`, and the `last` platform
snapshot. Snapshot fields are physical/available/process bytes,
`process_metric` (`rss` or `physical_footprint`), process headroom, recommendation,
group limit/headroom and `critical`. Unavailable readings are null. Diagnostics
are informational, not evidence that the tree or the process fit a guaranteed
safe limit; imports never apply the recorded policy to the current machine.

CSV histogram exports now use **CSV schema 2**, independently of the JSON
version. The original six columns remain in order; `cancelled`, `limited`, and
`limit_reason` are appended on every row. A result with no observed files has
one empty-extension, zero-count row so its stop reason is not lost. Consumers
requiring exactly six columns must update. Unknown sizes/overflow and formula
escaping retain their previous meaning. JSON is the full-tree interchange.

## Schema 2 (written since 0.2.0)

Schema 2 is schema 1 plus three additions. Readers ignore unknown members, so
later minor additions do not break 0.2 readers; a larger `schema_version` is
refused with an update hint.

| Member | Meaning |
| --- | --- |
| `scope_identity` | `source` (`filesystem`, `android_saf`, `ios_files`, `provider`), `platform` (`std::env::consts::OS`), `root` (exact native locator of the scope root), optional `root_inode` and `root_created_unix_nanos` of the root object. |
| `nodes[].listing_complete` | Directories only: whether the direct children were enumerated to the end. Unlike `complete` it ignores incomplete descendants. |
| `app_version` | Diagnostic only. |

Everything schema 1 had is unchanged: `started_unix_seconds`, `elapsed_millis`,
`scope`, `cancelled`, `limited`, optional `limit_reason`, `cross_filesystems`,
`errors` and `nodes[]` with `parent`, `name`, `locator`, `native_locator`, `kind`,
`logical`/`allocated` (`known`, `unknown`, `overflow`), `complete`, `hard_link`
and `error`. A size is `known` bytes plus a count of `unknown` contributions;
`overflow` means the sum exceeded `u64` and `known` is not a total.

Nodes are ordered so that every parent index is smaller than its children's, and
index 0 is the root. Indices are meaningful only inside one document.
`native_locator` is `unix-bytes:<hex>` or `windows-utf16:<hex>` for filesystem
paths (exact bytes or UTF-16 units) and the opaque provider URI otherwise;
`locator` and `name` are display text and may be lossy.

## Why schema 1 is read conservatively

Schema 1 records neither the kind of storage, the platform, nor whether a
directory's own listing finished (only whether its entire subtree did). Opening
it never makes the data more precise:

- no identity is invented; the scope can be compared only after a person
  confirms the correspondence (see [comparison](comparison.md));
- `listing_complete` stays absent. For comparisons a directory counts as
  fully listed only if its schema-1 `complete` flag is true, so an incomplete
  descendant makes every ancestor "not proven complete";
- re-exporting an opened schema-1 result writes schema 1 again, without schema-2
  members.

## Import rules

A file is untrusted input. Import runs on a background thread, streams the
document, and can be cancelled; a newer open supersedes an older one. It
refuses, with a specific error code shown with a localized explanation:

- files that are not regular files, or larger than the byte limit (1 GiB
  desktop, 128 MiB mobile);
- an estimated peak for parsed data, validation and opening indexes that
  exceeds the local import memory budget. The importer checks it before growing
  the result as it streams the file. The parser workspace allowance includes
  the longest raw string token, including in fields the importer skips;
- `schema_version` outside 1..=4, unknown `kind`/`limit_reason`/`source` values,
  wrong types, missing members, duplicate members, trailing data, and
  documents nested beyond 127 levels, including in skipped fields;
- structure: node 0 must be the only root, every parent index must be smaller
  than the child's (no cycles, no forward references), and only directories may
  have children;
- impossible values: leaf sizes with overflow or more than one unknown,
  `complete` together with an `error`, hard links on non-files, malformed error
  codes, empty names, oversize strings, invalid or non-lowercase hex locators, a
  `locator` that does not match its Unix `native_locator`, a child locator that
  does not extend its parent's, mixed locator families, a `scope` that differs
  from the root locator;
- **aggregates that its children do not explain.** Directory sizes are
  recomputed from the children and must match (`known` exactly unless overflow
  is justified; `unknown` at least the children's); a directory marked
  `complete` must have only complete children;
- schema-2 consistency: identity present and equal to the root, listing flags
  present exactly on directories and implied by `complete`;
- schema-4 consistency: normalized, ordered, nonredundant `exclusions`, local
  filesystem identity with the corresponding native encoding, direct-child
  native paths, and no recorded directory at an excluded path. Earlier schemas
  cannot declare the `exclusions` member.

`errors`, timestamps and the elapsed time are informational and are not
cross-checked. A deliberately forged but self-consistent file cannot be
detected: it is read as a record, never as proof about the disk.

Opening a file reads only that file. Paths and URIs in it are display data.
They are never used to scan, reveal, open or grant access; actions that act on
locations are not offered for saved results. On Android and iOS the system
picker copies the chosen document into app-private temporary storage, bounded
by the same limit, and the copy is deleted after import.

Exports are only written through the existing explicit export action.

The import worker samples local memory when it starts. Measured headroom
already accounts for an open result, and the worker keeps a reserve for other
allocations. Where reliable headroom is unavailable, a bounded fallback uses
physical RAM and current process usage when available. With neither reading,
the fallback starts at 256 MiB on desktop or 64 MiB on mobile and other local
limits can reduce it. The `memory` object in the file describes the source scan.
Memory already held by opened results, the browsing indexes and a finished
diff is counted together with a second file (see [comparison](comparison.md)).
Every result that was open stays open if validation fails or the local budget
runs out.
