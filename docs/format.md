# Result format and import

Exports are UTF-8 JSON documents describing one observed scope. SpaceMosaic 0.2
writes **schema 2** and reads schema 1 and 2. Field names, enum values and
numbers are machine-readable and independent of the display language.

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
- more objects than the scanner limit (500,000 desktop, 100,000 mobile) or more
  than the estimated metadata memory (256 MiB desktop, 64 MiB mobile, computed
  exactly like the scanner's budget), checked while reading, so an endless or
  oversized document stops early;
- `schema_version` outside 1..=2, unknown `kind`/`limit_reason`/`source` values,
  wrong types, missing members, duplicate members, trailing data, and
  documents nested beyond the parser's depth limit;
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
  present exactly on directories and implied by `complete`.

`errors`, timestamps and the elapsed time are informational and are not
cross-checked. A deliberately forged but self-consistent file cannot be
detected: it is read as a record, never as proof about the disk.

Opening a file reads only that file. Paths and URIs in it are display data.
They are never used to scan, reveal, open or grant access; actions that act on
locations are not offered for saved results. On Android and iOS the system
picker copies the chosen document into app-private temporary storage, bounded
by the same limit, and the copy is deleted after import.

Exports are only written through the existing explicit export action.
