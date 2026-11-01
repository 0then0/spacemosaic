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

The 500,000-object case is the desktop limit; the estimated metadata budget of
each result was close to its 256 MiB cap, so peak memory is roughly two result
budgets plus 96 bytes per diff row. Mobile budgets (100,000 objects, 64 MiB)
are four times smaller per result. No mobile device was measured.

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
