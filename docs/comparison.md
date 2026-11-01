# Comparing results

A comparison matches two *observations* of recorded sizes: a **baseline**
(before) and a **current** result (after). It is not evidence that anything was
deleted, that disk blocks were freed, or that contents are unchanged. "Same size"
means the recorded sizes are equal; contents are never read.

In 0.2 the baseline is a result opened from a file and the current result is a
scan started in the same session; **Swap direction** reverses them. Results from
two saved files are not compared in the interface (the core API accepts any two
results). Nothing is stored automatically.

## Is it the same area?

Each pair gets one verdict from recorded facts only.

- **Matching**: same storage kind and platform, identical exact root locator, and
  no contradicting root-object facts. This is the strongest verdict: a volume or
  device replaced under the same path cannot be excluded, because neither is
  recorded. The interface says so.
- **Insufficient**: schema-1 data (no identity recorded), a root folder whose
  inode or creation time changed, or iOS roots whose file-URL spelling differs.
  A person may confirm that the areas correspond; the result is then labelled
  "confirmed by you, not technically verified".
- **Incompatible**: different storage kinds, platforms or Android provider
  authorities; different root locators on the filesystem or Android; schema-1
  data whose locator syntax or text differs. Never compared, and confirmation
  cannot override it. A moved or renamed scope root is therefore a different area.

Display names are never identity. For schema 1 the locator syntax
(`unix-bytes:`, `windows-utf16:`, `content://`, `file://`) is used only to rule
combinations out.

Confirmation does not change completeness or measurement limits, which are
shown separately as limitations: a partial baseline or current scan,
different filesystem-boundary settings, missing allocation, or older formats.

## Matching objects

Matching starts at the roots and goes down through matched directories only.
Children are paired by their **exact name bytes**: the bytes after the parent's
native locator for filesystem scans, the reported name for providers. No case
folding, Unicode normalization or lossy decoding is applied, so `File.txt` and
`file.txt`, precomposed and decomposed `é`, and two invalid UTF-8 names that both
display as `�` stay separate. If a name occurs more than once on either side
while also present on the other, nothing is paired and every such object is
"change unknown (ambiguous name)". There is no rename or move detection: a
rename is one missing and one added object.

## Statuses

| Status | Meaning |
| --- | --- |
| Added | Present now; not in the baseline although the baseline listed the parent completely. |
| Missing now | In the baseline; not in the current result although the current scan listed the parent completely. Not "deleted", not "freed". |
| Grew / Shrank | Present in both with the same type and both sizes fully known. |
| Same size | As above with equal sizes. |
| Type changed | Present in both with different types (file, folder, link, other). Old and new sizes count as a removal and an addition. |
| Change unknown | Not decidable; the reason is shown. |

Absence is evidence only when the other result **enumerated the parent
directory to the end**. The decision uses that directory's own listing state
(`listing_complete`), not one global "partial" flag, and an unmatched subtree
below an incompletely listed directory is unknown as a whole. An unreadable or
unlisted folder is never empty; objects below it that exist only in the other
result are "not observed", and its old size is not reported as freed. Unknown
sizes, overflow and links (whose size is never measured) make a change unknown
instead of zero.

## One accounting model

Every difference is attributed to exactly one **event**: an added object, a
missing object (a whole folder counts once), a type change, or a matched
non-folder whose size differs. Matched folders are containers: their row shows
the sum of events below them and is never added again. Totals:

- additions: sizes of added objects, plus the new side of type changes;
- removals: sizes of missing objects, plus the old side of type changes;
- existing objects: growth and shrinkage of matched files;
- net = additions + growth − removals − shrinkage.

All amounts use 128-bit arithmetic, so `u64::MAX` sizes cannot overflow.
When every compared object is fully measured, net equals the difference of the
two root sizes (a tested invariant). Otherwise the tile reads "confirmed net
change", states that it is not a total, and lists how many objects have an
unknown change or only a lower-bound amount. The difference of known sums is
never presented as a full result.

Logical sizes work everywhere. Allocated sizes are compared only when both
results measured them, and per object only where both values are known.

## Limits and budgets

Opening and comparing run on a background thread, one at a time; both results
and the diff stay in memory. Peak use is about two result budgets plus the diff
(96 bytes per diff row); see [performance](performance.md).
