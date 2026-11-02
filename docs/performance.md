# Performance measurement

The benchmark creates a disposable synthetic tree with 100 directories and a
configurable number of sparse files. Logical lengths vary from 0 to 8191 bytes.
It measures metadata scanning rather than file-content throughput. Fixture
creation and cleanup are excluded from the scanner's reported elapsed time.

```sh
cargo build --release --locked --example scan_benchmark
./target/release/examples/scan_benchmark 100000
```

For peak process memory, run the executable with the platform's resource tool:

```sh
# macOS: inspect maximum resident set size in bytes.
/usr/bin/time -l ./target/release/examples/scan_benchmark 100000
# Linux: inspect maximum resident set size in KiB.
/usr/bin/time -v ./target/release/examples/scan_benchmark 100000
```

The resource-tool measurement includes fixture creation and cleanup. Report it
separately from `scan_millis`. Record OS, CPU architecture, Rust version,
filesystem, object count, errors, cache state, and whether a resource limit
was reached. A sparse fixture's logical total is not its physical consumption.

The scanner publishes progress at most every 100 ms. The UI virtualizes rows,
caches result filtering/map membership, and draws at most 96 map rectangles.
These implementation bounds do not establish a frame-time guarantee. Measure
interactive latency separately while scanning, navigating, and changing filters.
No cross-product speed claims or unmeasured responsiveness numbers are made.

## Saved results and comparison

`examples/compare_benchmark.rs` builds a synthetic baseline and a later tree
(about 2% of objects removed, 8% of files grown, 3% added, some folders unreadable
and a few files replaced by folders), writes both as schema 2 JSON, then measures
bounded import and comparison:

```sh
cargo build --release --locked --example compare_benchmark
./target/release/examples/compare_benchmark 500000 --write DIR
/usr/bin/time -l ./target/release/examples/compare_benchmark --read DIR
```

Measured on macOS 15.7.9, Apple Silicon, Rust 1.99.1, release build, warm file cache
(synthetic fixture, not a cold disk or a real file tree):

| Objects per result | JSON size | Import of both | Compare | Diff rows | Peak RSS (`--read`) |
| --- | --- | --- | --- | --- | --- |
| 200,000 | 124 MB + 105 MB | 0.79 s | 64 ms | 171,410 | not measured |
| 500,000 / 423,665 | 317 MB + 269 MB | 2.05 s | 157 ms | 431,976 | 589 MB |

These are historical measurements under the former fixed scan limits
(500,000 objects / 256 MiB desktop; 100,000 / 64 MiB mobile). Scan admission now
uses the [adaptive memory policy](memory.md); saved-file import retains its
independent bounds. These measurements do not establish current mobile
allocation limits. No mobile device was measured.

UI: a headless egui test with 120,000 objects per result (debug build, software
layout only, no GPU) compared in 289 ms, painted the first change-list frame in
1.5 ms with fewer than 2,000 shapes (rows are virtualized), and rebuilt the
folder tree with a filter in 21 ms. These numbers do not describe real frame
rates on a display or on a phone.

## Reference measurement

One local measurement on macOS 15.7.9, Apple Silicon (`aarch64`), Rust 1.99.1:
100,000 files plus 100 directories produced 100,101 scanned objects, zero
errors, and no resource-limit truncation. The scanner reported **493 ms**.
`/usr/bin/time -l` reported **105,873,408 bytes maximum resident set size** and
10.87 seconds total wall time including fixture creation and cleanup.

This is a newly generated sparse fixture whose metadata is warmed by creation,
not a cold-storage throughput test. It does not measure UI frame latency or
predict performance on network/cloud providers. Re-run on the intended machine
and filesystem before drawing operational conclusions.

## Adaptive scan memory measurement (2026-11-02)

Environment: macOS 15.7.9 (24G830), arm64 Apple Silicon, 8 GiB physical RAM
reported by the native sampler, Rust 1.99.1, release build, APFS temporary
storage. Fixtures are synthetic and warmed by creation. No home directory was
scanned and no OOM was induced. The working tree is uncommitted.

The file benchmark accepts an optional manual MiB argument and reports the
chosen budget, estimated scanner data, baseline/latest process samples and stop
reason separately:

```sh
cargo build --release --locked --example scan_benchmark
/usr/bin/time -l ./target/release/examples/scan_benchmark 550000 1024
```

The host sandbox allowed own-process sampling but blocked `time -l`'s
`kern.clockrate` query, preventing its final resource statistics. Peak-RSS runs
were performed with the same command outside that sandbox. This is a measurement
permission issue, not an application permission requirement.

A separate Automatic run on 550,000 files selected 512 MiB, retained 524,288
objects, and stopped for `memory_limit` before doubling node capacity. It
reported 24,234 ms scanning and 498,808,315 estimated scanner bytes. Peak RSS was
unavailable for this sandboxed run; total command wall time was 126.40 s including
fixture creation/cleanup. This illustrates why object count is no longer the
admission policy: path lengths and the next capacity growth matter.

The final manual file run completed all **550,101 objects** (550,000 files,
100 folders and root), zero errors, no truncation: **21,134 ms** scanning,
1,073,741,824-byte budget, **619,079,744 estimated scanner bytes**.
`time -l` reported **515,211,264 bytes maximum RSS**, **507,873,344 bytes peak
physical footprint**, and 104.66 s total wall time including fixture creation
and cleanup. RSS and footprint differ because residence and dirty/compressed
accounting differ; the scanner estimate also includes reserved but possibly
untouched vector capacity and traversal workspaces. The latest sampled RSS
(407,928,832 bytes) is a periodic sample, not a peak. No comparison with a
previous product version or colder storage is inferred.

For UI/cancellation measurements, a lazy provider generates 600,000 files in one
very wide directory, including unknown sizes. The test uses the real worker
and `ExplorerApp::draw`, without GPU/display/event-loop timing. A warmed font
atlas is excluded from per-frame times. One explicit performance test is ignored
in the routine suite because it is a measurement, not a timing assertion:

```sh
cargo test --release --locked --lib --no-run
# Use the executable path printed by Cargo; timing the test binary excludes compilation.
/usr/bin/time -l target/release/deps/spacemosaic-<hash> \
  scan_memory_responsiveness_measurement --ignored --nocapture --test-threads=1
```

Final host run: 600,001 objects, 440 ms scanning, 1,073,741,824-byte chosen
budget and 321,770,638 estimated scanner bytes. Across 20 frames during scanning,
p95/max frame work was 918 microseconds. The first completed-result frame was
341,058 microseconds. A second synthetic scan was cancelled at 147,128 objects;
cancel-to-partial-result was 4,536 microseconds, including aggregation and 1 ms
polling. Maximum process RSS for the entire test was 352,468,992 bytes and peak
physical footprint 333,875,136 bytes. These process measurements include the UI,
indexes, both sequential test phases and allocator retention; they are not the
scanner estimate.

The initial measurement exposed duplicate directory sorting and repeated
lowercase string allocation in comparisons: its first-result frame was
1,986,077 microseconds. Reusing the existing sorted child index, using in-place
sorting with an index tie-breaker, and comparing ASCII names without allocating
removed that duplicate work. The measurements are observations on this fixture,
not a general speedup claim. Unicode lowercasing/order remains unchanged.

The remaining 341 ms first-result preparation is a visible pause for an
exceptionally wide directory. Index construction, Unicode sorting, searches and
comparison remain synchronous and can take longer on larger trees or slower
hardware. There is no frame-time guarantee, no promise of unlimited results,
and no mobile performance qualification. The policy reserves memory for those
operations but does not move them to a new processing architecture.

Review regression run after R1/R2: the same release UI benchmark completed
600,001 objects in 374 ms, scanning-frame p95/max 638 microseconds, first-result
frame 332,323 microseconds. Cancellation at 158,009 objects returned a partial
result in 3,773 microseconds. Whole-test peak RSS was 349,552,640 bytes and peak
physical footprint 335,300,352 bytes. The selected budget and scanner estimate
were unchanged. This separate observation does not establish a speedup; native
mobile frame/input latency remains unmeasured.
