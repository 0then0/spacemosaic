mod common;
use common::*;
use spacemosaic::{
    compare::{
        self, ChangeSort, CompareError, Comparison, Dimension, Limitation, Reason, ScopeNote,
        ScopeVerdict, Status, assess_scope,
    },
    scan::{self, Kind, Options, Progress},
};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

const LOGICAL: Dimension = Dimension::Logical;

fn run(old: scan::ScanResult, new: scan::ScanResult) -> Comparison {
    compare::compare(Arc::new(old), Arc::new(new), true, &AtomicBool::new(false)).unwrap()
}

/// Photos, videos and a file that only exists later, spread over three folders.
fn growth_fixture() -> (scan::ScanResult, scan::ScanResult) {
    let mut old = Builder::fs("/data");
    let photos = old.dir(0, "photos");
    old.file(photos, "grows.bin", 100 * MIB);
    old.file(photos, "stays.bin", 7 * MIB);
    let videos = old.dir(0, "videos");
    old.file(videos, "gone.bin", 400 * MIB);
    let mut new = Builder::fs("/data");
    let photos = new.dir(0, "photos");
    new.file(photos, "grows.bin", 150 * MIB);
    new.file(photos, "stays.bin", 7 * MIB);
    new.dir(0, "videos");
    let fresh = new.dir(0, "fresh");
    new.file(fresh, "big.bin", 1000 * MIB);
    (old.build(), new.build())
}

#[test]
fn growth_removal_and_addition_total_650_mib_without_double_counting_folders() {
    let (old, new) = growth_fixture();
    let expected = new.nodes[0].logical.known as i128 - old.nodes[0].logical.known as i128;
    let comparison = run(old, new);
    assert_eq!(status(&comparison, "photos/grows.bin"), Status::Grew);
    assert_eq!(status(&comparison, "videos/gone.bin"), Status::Removed);
    assert_eq!(status(&comparison, "fresh"), Status::Added);
    assert_eq!(status(&comparison, "photos/stays.bin"), Status::SameSize);
    let totals = comparison.totals(LOGICAL);
    assert_eq!(totals.net(), 650 * MIB as i128);
    assert_eq!(totals.net(), expected);
    assert_eq!(totals.added, 1000 * MIB as u128);
    assert_eq!(totals.removed, 400 * MIB as u128);
    assert_eq!(totals.grown, 50 * MIB as u128);
    assert_eq!(
        (totals.added_count, totals.removed_count, totals.grew_count),
        (1, 1, 1)
    );
    assert!(totals.is_complete());
    // Containers show the sum of what is below them, and the root is not an event itself.
    assert_eq!(comparison.root().confirmed(LOGICAL), 650 * MIB as i128);
    let photos = find(&comparison, "photos");
    assert_eq!(
        comparison.nodes[photos].confirmed(LOGICAL),
        50 * MIB as i128
    );
    assert!(comparison.nodes[photos].is_container() && !comparison.nodes[photos].is_event());
    // The added folder is one event; its file is not counted a second time.
    let events = comparison.events(LOGICAL, Status::CHANGED, "", ChangeSort::Growth);
    assert_eq!(events.len(), 3);
    assert_eq!(comparison.name(events[0] as usize), "fresh");
    assert_eq!(comparison.name(events[2] as usize), "gone.bin");
    let by_decrease = comparison.events(LOGICAL, Status::CHANGED, "", ChangeSort::Decrease);
    assert_eq!(comparison.name(by_decrease[0] as usize), "gone.bin");
}

#[test]
fn direction_is_explicit_reversing_swaps_growth_and_removal() {
    let (old, new) = growth_fixture();
    let comparison = run(new, old);
    assert_eq!(status(&comparison, "fresh"), Status::Removed);
    assert_eq!(status(&comparison, "videos/gone.bin"), Status::Added);
    assert_eq!(status(&comparison, "photos/grows.bin"), Status::Shrank);
    assert_eq!(comparison.totals(LOGICAL).net(), -(650 * MIB as i128));
}

#[test]
fn directory_unreadable_now_is_unknown_and_its_old_size_is_not_freed_space() {
    let mut old = Builder::fs("/data");
    let cache = old.dir(0, "cache");
    old.file(cache, "huge.bin", 5000 * MIB);
    old.file(0, "keep.bin", MIB);
    let mut new = Builder::fs("/data");
    let cache = new.dir(0, "cache");
    new.deny(cache);
    new.file(0, "keep.bin", MIB);
    let comparison = run(old.build(), new.build());
    let cache = find(&comparison, "cache");
    assert_eq!(comparison.nodes[cache].status(LOGICAL), Status::Unknown);
    assert_eq!(
        comparison.nodes[cache].reason(LOGICAL),
        Reason::ListingIncomplete
    );
    let huge = comparison.nodes[find(&comparison, "cache/huge.bin")];
    assert_eq!(huge.status(LOGICAL), Status::Unknown);
    assert_eq!(huge.reason(LOGICAL), Reason::CurrentNotObserved);
    let totals = comparison.totals(LOGICAL);
    assert_eq!(totals.removed, 0);
    assert_eq!(totals.net(), 0);
    assert!(!totals.is_complete());
    assert_eq!(totals.unknown_count, 2);
    assert!(comparison.root().unknown(LOGICAL) >= 2);
}

#[test]
fn incomplete_baseline_never_declares_missing_objects_new() {
    let mut old = Builder::fs("/data");
    let incomplete = old.dir(0, "interrupted");
    old.file(incomplete, "seen.bin", MIB);
    old.abort_listing(incomplete);
    let complete = old.dir(0, "listed");
    old.file(complete, "a.bin", MIB);
    let mut new = Builder::fs("/data");
    let incomplete = new.dir(0, "interrupted");
    new.file(incomplete, "seen.bin", 3 * MIB);
    new.file(incomplete, "later.bin", 9 * MIB);
    let complete = new.dir(0, "listed");
    new.file(complete, "a.bin", MIB);
    new.file(complete, "b.bin", 2 * MIB);
    let comparison = run(old.build(), new.build());
    // Present in both: still a confirmed change, even inside an incomplete listing.
    assert_eq!(status(&comparison, "interrupted/seen.bin"), Status::Grew);
    // Absent from an incomplete listing is not evidence of being new.
    let later = comparison.nodes[find(&comparison, "interrupted/later.bin")];
    assert_eq!(later.status(LOGICAL), Status::Unknown);
    assert_eq!(later.reason(LOGICAL), Reason::BaselineNotObserved);
    // A fully listed sibling folder keeps the confident verdict.
    assert_eq!(status(&comparison, "listed/b.bin"), Status::Added);
    let totals = comparison.totals(LOGICAL);
    assert_eq!(totals.added, 2 * MIB as u128);
    assert_eq!(totals.grown, 2 * MIB as u128);
    assert!(!totals.is_complete());
    assert_eq!(
        comparison.nodes[find(&comparison, "interrupted")].status(LOGICAL),
        Status::Grew,
        "a confirmed part is reported with its unknown part, not hidden"
    );
    assert_eq!(
        comparison.nodes[find(&comparison, "interrupted")].reason(LOGICAL),
        Reason::ListingIncomplete
    );
}

#[test]
fn a_global_cancelled_flag_does_not_replace_per_directory_listing_decisions() {
    let mut old = Builder::fs("/data");
    let a = old.dir(0, "done");
    old.file(a, "x", 1);
    let b = old.dir(0, "stopped");
    old.abort_listing(b);
    old.cancelled = true;
    let mut new = Builder::fs("/data");
    let a = new.dir(0, "done");
    new.file(a, "x", 1);
    new.file(a, "y", 5);
    new.dir(0, "stopped");
    let comparison = run(old.build(), new.build());
    assert_eq!(status(&comparison, "done/y"), Status::Added);
    assert!(
        comparison
            .limitations
            .contains(&Limitation::BaselinePartial)
    );
}

#[test]
fn unknown_size_on_either_side_makes_the_delta_unknown_not_zero() {
    let mut old = Builder::fs("/data");
    old.unknown_file(0, "a");
    old.file(0, "b", 10);
    let mut new = Builder::fs("/data");
    new.file(0, "a", 4096);
    new.unknown_file(0, "b");
    let comparison = run(old.build(), new.build());
    for name in ["a", "b"] {
        let node = comparison.nodes[find(&comparison, name)];
        assert_eq!(node.status(LOGICAL), Status::Unknown);
        assert_eq!(node.reason(LOGICAL), Reason::SizeUnknown);
        assert_eq!(node.confirmed(LOGICAL), 0);
    }
    assert_eq!(comparison.totals(LOGICAL).net(), 0);
    assert!(!comparison.totals(LOGICAL).is_complete());
}

#[test]
fn equal_sizes_are_reported_as_size_equality_only_and_no_contents_are_read() {
    let root: PathBuf =
        std::env::temp_dir().join(format!("spacemosaic-same-size-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let file = root.join("data.bin");
    let scan_once = || {
        scan::scan(
            &root,
            Options::default(),
            &AtomicBool::new(false),
            &Mutex::new(Progress::default()),
        )
    };
    fs::write(&file, b"aaaaaaaaaa").unwrap();
    let old = scan_once();
    fs::write(&file, b"bbbbbbbbbb").unwrap();
    let new = scan_once();
    let comparison = run(old, new);
    let row = comparison.nodes[find(&comparison, "data.bin")];
    assert_eq!(row.status(LOGICAL), Status::SameSize);
    assert_eq!(Status::SameSize.key(), "cmp_same_size");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn rename_is_a_removal_plus_an_addition_and_type_changes_are_explicit() {
    let mut old = Builder::fs("/data");
    old.file(0, "report.txt", 10 * MIB);
    let thing = old.file(0, "thing", 3 * MIB);
    let _ = thing;
    let folder = old.dir(0, "folder");
    old.file(folder, "inner", 4 * MIB);
    let mut new = Builder::fs("/data");
    new.file(0, "report-final.txt", 10 * MIB);
    let thing = new.dir(0, "thing");
    new.file(thing, "inside", 8 * MIB);
    new.file(0, "folder", 6 * MIB);
    let comparison = run(old.build(), new.build());
    assert_eq!(status(&comparison, "report.txt"), Status::Removed);
    assert_eq!(status(&comparison, "report-final.txt"), Status::Added);
    assert_eq!(status(&comparison, "thing"), Status::TypeChanged);
    assert_eq!(status(&comparison, "folder"), Status::TypeChanged);
    let totals = comparison.totals(LOGICAL);
    assert_eq!(totals.type_changed_count, 2);
    assert_eq!(totals.removed, (10 + 3 + 4) * MIB as u128);
    assert_eq!(totals.added, (10 + 8 + 6) * MIB as u128);
    // Old children of a replaced directory are inside the removed subtree.
    assert!(comparison.nodes.iter().all(|n| {
        n.old
            .is_none_or(|o| comparison.baseline.nodes[o as usize].name != "inner")
    }));
}

#[test]
fn scope_identity_distinguishes_matching_insufficient_and_incompatible() {
    let build = |root: &str| {
        let mut b = Builder::fs(root);
        b.file(0, "a", 1);
        b.build()
    };
    let v1 = |root: &str| {
        let mut b = Builder::fs(root);
        b.v1 = true;
        b.file(0, "a", 1);
        b.build()
    };
    let same = assess_scope(&build("/data"), &build("/data"));
    assert_eq!(same.verdict, ScopeVerdict::Matching);
    assert!(same.notes.contains(&ScopeNote::VolumeNotVerified));
    // Another path is another area; there is no move detection.
    assert_eq!(
        assess_scope(&build("/data"), &build("/archive")).verdict,
        ScopeVerdict::Incompatible
    );
    // A different root object under the same path needs a person's confirmation.
    let mut replaced = Builder::fs("/data");
    replaced.created = Some(1_800_000_000_000_000_000);
    replaced.file(0, "a", 1);
    let changed = assess_scope(&build("/data"), &replaced.build());
    assert_eq!(changed.verdict, ScopeVerdict::Insufficient);
    assert_eq!(changed.notes, vec![ScopeNote::RootObjectChanged]);
    // Schema 1 records no identity.
    let old = assess_scope(&v1("/data"), &v1("/data"));
    assert_eq!(old.verdict, ScopeVerdict::Insufficient);
    assert_eq!(
        assess_scope(&v1("/data"), &build("/data")).verdict,
        ScopeVerdict::Insufficient
    );
    assert_eq!(
        assess_scope(&v1("/data"), &v1("/other")).verdict,
        ScopeVerdict::Incompatible
    );
    // Platforms and storage kinds are never interchangeable.
    let mut windows = Builder::fs("/data");
    windows.platform = "windows";
    windows.file(0, "a", 1);
    assert_eq!(
        assess_scope(&build("/data"), &windows.build()).verdict,
        ScopeVerdict::Incompatible
    );
    let saf = |root: &str| {
        let mut b = Builder::provider(root);
        b.file(0, "a", 1);
        b.build()
    };
    let a = saf(
        "content://com.android.externalstorage.documents/tree/primary%3ADocs/document/primary%3ADocs",
    );
    let b = saf(
        "content://com.android.externalstorage.documents/tree/primary%3ADocs/document/primary%3ADocs",
    );
    let other_tree = saf(
        "content://com.android.externalstorage.documents/tree/primary%3APics/document/primary%3APics",
    );
    let other_provider = saf("content://com.example.cloud/tree/1/document/1");
    assert_eq!(assess_scope(&a, &b).verdict, ScopeVerdict::Matching);
    assert_eq!(
        assess_scope(&a, &other_tree).notes,
        vec![ScopeNote::DifferentRoot]
    );
    assert_eq!(
        assess_scope(&a, &other_provider).notes,
        vec![ScopeNote::DifferentProviderAuthority]
    );
    assert_eq!(
        assess_scope(&a, &build("/data")).notes,
        vec![ScopeNote::DifferentSource]
    );
}

#[test]
fn comparison_requires_confirmation_for_insufficient_scopes_and_refuses_incompatible_ones() {
    let make = |root: &str, v1: bool| {
        let mut b = Builder::fs(root);
        b.v1 = v1;
        b.file(0, "a", 1);
        Arc::new(b.build())
    };
    let cancel = AtomicBool::new(false);
    let (old, new) = (make("/data", true), make("/data", true));
    assert!(matches!(
        compare::compare(old.clone(), new.clone(), false, &cancel),
        Err(CompareError::NeedsConfirmation(_))
    ));
    let confirmed = compare::compare(old, new, true, &cancel).unwrap();
    assert!(
        confirmed
            .limitations
            .contains(&Limitation::ScopeConfirmedByUser)
    );
    assert!(confirmed.limitations.contains(&Limitation::OlderSchema));
    // Confirmation cannot override contradictory facts.
    assert!(matches!(
        compare::compare(
            make("/data", false),
            make("/elsewhere", false),
            true,
            &cancel
        ),
        Err(CompareError::Incompatible(_))
    ));
}

#[test]
fn traversal_differences_and_missing_allocation_are_reported_not_hidden() {
    let mut old = Builder::fs("/data");
    old.cross_filesystems = true;
    let mount = old.dir(0, "mount");
    old.file(mount, "f", 9 * MIB);
    let mut new = Builder::fs("/data");
    let mount = new.dir(0, "mount");
    new.deny(mount);
    let comparison = run(old.build(), new.build());
    assert!(
        comparison
            .limitations
            .contains(&Limitation::CrossFilesystemsDiffer)
    );
    assert_eq!(status(&comparison, "mount/f"), Status::Unknown);
    // Provider scans carry no allocation: the allocated dimension must not be offered.
    let provider = |size| {
        let mut b = Builder::provider("content://p/tree/1/document/1");
        b.file(0, "a", size);
        b.build()
    };
    let comparison = run(provider(5), provider(9));
    assert!(!comparison.allocated_available);
    assert!(
        comparison
            .limitations
            .contains(&Limitation::AllocatedUnavailable)
    );
    assert_eq!(status(&comparison, "a"), Status::Grew);
    let row = comparison.nodes[find(&comparison, "a")];
    assert_eq!(row.status(Dimension::Allocated), Status::Unknown);
    // Where both sides measured allocation it is compared separately from logical size.
    let mut old = Builder::fs("/data");
    old.file(0, "a", 5);
    let mut new = Builder::fs("/data");
    new.file(0, "a", 9);
    let comparison = run(old.build(), new.build());
    assert!(comparison.allocated_available);
    assert_eq!(
        comparison.nodes[find(&comparison, "a")].confirmed(Dimension::Allocated),
        4
    );
}

#[test]
fn native_names_unicode_case_and_duplicates_never_merge_objects() {
    let mut old = Builder::fs("/data");
    old.file_raw(0, b"bad-\xff", Some(1));
    old.file_raw(0, "File.txt".as_bytes(), Some(2));
    old.file_raw(0, "caf\u{e9}".as_bytes(), Some(3)); // precomposed
    old.file_raw(0, "same".as_bytes(), Some(4));
    let mut new = Builder::fs("/data");
    new.file_raw(0, b"bad-\xfe", Some(1));
    new.file_raw(0, "file.txt".as_bytes(), Some(2));
    new.file_raw(0, "cafe\u{301}".as_bytes(), Some(3)); // decomposed
    new.file_raw(0, "same".as_bytes(), Some(4));
    let (old, new) = (old.build(), new.build());
    // Both invalid names display as U+FFFD; only the native bytes tell them apart.
    assert_eq!(old.nodes[1].name, "bad-\u{fffd}");
    assert_eq!(new.nodes[1].name, "bad-\u{fffd}");
    let comparison = run(old, new);
    let totals = comparison.totals(LOGICAL);
    assert_eq!((totals.removed_count, totals.added_count), (3, 3));
    assert_eq!(totals.same_count, 1);
    // Provider names are the only key; duplicates on both sides cannot be paired.
    let provider = |sizes: &[u64]| {
        let mut b = Builder::provider("content://p/tree/1/document/1");
        for size in sizes {
            b.file(0, "photo.jpg", *size);
        }
        b.build()
    };
    let comparison = run(provider(&[1, 2]), provider(&[1, 5]));
    let totals = comparison.totals(LOGICAL);
    assert_eq!(totals.unknown_count, 4);
    assert_eq!(totals.net(), 0);
    assert!(
        comparison
            .nodes
            .iter()
            .skip(1)
            .all(|n| n.reason(LOGICAL) == Reason::AmbiguousName)
    );
    // A duplicate with no counterpart is still plainly removed.
    let comparison = run(provider(&[1, 2]), provider(&[]));
    assert_eq!(comparison.totals(LOGICAL).removed_count, 2);
}

#[test]
fn deltas_cover_the_whole_u64_range_without_signed_overflow() {
    let mut old = Builder::fs("/data");
    old.file(0, "huge", u64::MAX);
    old.file(0, "tiny", 0);
    old.file(0, "gone", u64::MAX);
    let mut new = Builder::fs("/data");
    new.file(0, "huge", 0);
    new.file(0, "tiny", u64::MAX);
    new.file(0, "born", u64::MAX);
    let comparison = run(old.build(), new.build());
    assert_eq!(
        comparison.nodes[find(&comparison, "huge")].confirmed(LOGICAL),
        -(u64::MAX as i128)
    );
    assert_eq!(
        comparison.nodes[find(&comparison, "tiny")].confirmed(LOGICAL),
        u64::MAX as i128
    );
    // The sum of those rows exceeds u64 and i64 and is still exact.
    let totals = comparison.totals(LOGICAL);
    assert_eq!(totals.grown, u64::MAX as u128);
    assert_eq!(totals.shrunk, u64::MAX as u128);
    assert_eq!(totals.added, u64::MAX as u128);
    assert_eq!(totals.removed, u64::MAX as u128);
    assert_eq!(totals.net(), 0);
}

#[test]
fn aggregate_overflow_makes_the_enclosing_change_a_lower_bound() {
    let mut new = Builder::fs("/data");
    let big = new.dir(0, "big");
    new.file(big, "a", u64::MAX);
    new.file(big, "b", u64::MAX);
    let old = Builder::fs("/data");
    let comparison = run(old.build(), new.build());
    let row = comparison.nodes[find(&comparison, "big")];
    assert_eq!(row.status(LOGICAL), Status::Added);
    assert_eq!(row.reason(LOGICAL), Reason::SizeUnknown);
    assert!(!comparison.totals(LOGICAL).is_complete());
    assert_eq!(comparison.totals(LOGICAL).lower_bound_count, 1);
}

#[test]
fn links_are_not_measured_so_their_change_is_unknown() {
    let mut old = Builder::fs("/data");
    old.link(0, "shortcut");
    let mut new = Builder::fs("/data");
    new.link(0, "shortcut");
    let comparison = run(old.build(), new.build());
    assert_eq!(status(&comparison, "shortcut"), Status::Unknown);
}

/// Deterministic generator; the test needs reproducible trees, not randomness quality.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn random_pair(seed: u64, files: usize) -> (scan::ScanResult, scan::ScanResult) {
    let mut rng = Rng(seed);
    let mut old = Builder::fs("/data");
    let mut new = Builder::fs("/data");
    let mut old_dirs = vec![0];
    let mut new_dirs = vec![0];
    for i in 0..files {
        let dir_index = rng.below(old_dirs.len() as u64) as usize;
        if rng.below(10) == 0 {
            let name = format!("d{i}");
            let (o, n) = (old_dirs[dir_index], new_dirs[dir_index]);
            old_dirs.push(old.dir(o, &name));
            new_dirs.push(new.dir(n, &name));
            continue;
        }
        let name = format!("f{i}");
        let size = rng.below(10_000);
        let (o, n) = (old_dirs[dir_index], new_dirs[dir_index]);
        match rng.below(6) {
            0 => {
                old.file(o, &name, size);
            }
            1 => {
                new.file(n, &name, size);
            }
            2 => {
                old.file(o, &name, size);
                new.file(n, &name, size + rng.below(500));
            }
            3 => {
                old.file(o, &name, size + 700);
                new.file(n, &name, size);
            }
            4 => {
                old.file(o, &name, size);
                new.dir(n, &name);
            }
            _ => {
                old.file(o, &name, size);
                new.file(n, &name, size);
            }
        }
    }
    (old.build(), new.build())
}

#[test]
fn fully_determined_comparisons_equal_the_difference_of_root_sizes() {
    for seed in 1..=40 {
        let (old, new) = random_pair(seed * 7919, 400);
        let expected = new.nodes[0].logical.known as i128 - old.nodes[0].logical.known as i128;
        let comparison = run(old, new);
        let totals = comparison.totals(LOGICAL);
        assert!(totals.is_complete(), "seed {seed}");
        assert_eq!(totals.net(), expected, "seed {seed}");
        assert_eq!(
            comparison.root().confirmed(LOGICAL),
            expected,
            "seed {seed}"
        );
        // Containers sum their children exactly once.
        for node in comparison.nodes.iter().filter(|n| n.is_container()) {
            let sum: i128 = node
                .children()
                .map(|c| comparison.nodes[c].confirmed(LOGICAL))
                .sum();
            assert_eq!(node.confirmed(LOGICAL), sum);
        }
        let events = comparison.events(LOGICAL, Status::EVERYTHING, "", ChangeSort::Growth);
        let per_event: i128 = events
            .iter()
            .map(|&i| comparison.nodes[i as usize].confirmed(LOGICAL))
            .sum();
        assert_eq!(per_event, expected, "seed {seed}");
    }
}

#[test]
fn identical_results_report_no_change_and_swapping_inverts_every_amount() {
    for seed in [3, 11] {
        let (old, new) = random_pair(seed, 300);
        let forward = run(old.clone(), new.clone());
        let backward = run(new, old);
        assert_eq!(
            forward.totals(LOGICAL).net(),
            -backward.totals(LOGICAL).net()
        );
        assert_eq!(
            forward.totals(LOGICAL).added,
            backward.totals(LOGICAL).removed
        );
        assert_eq!(
            forward.totals(LOGICAL).grown,
            backward.totals(LOGICAL).shrunk
        );
    }
    let (old, _) = random_pair(5, 300);
    let comparison = run(old.clone(), old);
    assert_eq!(comparison.totals(LOGICAL).net(), 0);
    assert!(
        comparison
            .events(LOGICAL, Status::CHANGED, "", ChangeSort::Name)
            .is_empty()
    );
}

#[test]
fn filters_search_and_navigation_targets_work_on_events() {
    let (old, new) = growth_fixture();
    let comparison = run(old, new);
    assert_eq!(
        comparison
            .events(LOGICAL, Status::Removed.bit(), "", ChangeSort::Name)
            .len(),
        1
    );
    assert_eq!(
        comparison
            .events(LOGICAL, Status::CHANGED, "BIG", ChangeSort::Name)
            .len(),
        0,
        "an added folder's contents are not separate rows"
    );
    assert_eq!(
        comparison
            .events(LOGICAL, Status::CHANGED, "GONE", ChangeSort::Name)
            .len(),
        1
    );
    let gone = find(&comparison, "videos/gone.bin");
    let node = &comparison.nodes[gone];
    assert!(node.old.is_some() && node.new.is_none());
    assert_eq!(
        comparison.baseline.nodes[node.old.unwrap() as usize].name,
        "gone.bin"
    );
    let tree = comparison.tree_children(0, LOGICAL, Status::CHANGED, "", ChangeSort::Growth);
    let names: Vec<_> = tree.iter().map(|&i| comparison.name(i as usize)).collect();
    assert_eq!(names, ["fresh", "photos", "videos"]);
    let none =
        comparison.tree_children(0, LOGICAL, Status::TypeChanged.bit(), "", ChangeSort::Name);
    assert!(none.is_empty());
}

#[test]
fn kinds_survive_in_rows_for_navigation() {
    let (old, new) = growth_fixture();
    let comparison = run(old, new);
    let fresh = find(&comparison, "fresh");
    assert_eq!(comparison.kinds(fresh), (None, Some(Kind::Directory)));
}

// ----- Two saved files, memory allowance, portability -----

/// Smallest allowance `compare_within` admits, found by bisection. The check is
/// monotonic in the allowance, so this is the estimate for this pair.
fn minimal_allowance(old: &Arc<scan::ScanResult>, new: &Arc<scan::ScanResult>) -> usize {
    let flag = AtomicBool::new(false);
    let admitted =
        |limit| compare::compare_within(old.clone(), new.clone(), true, &flag, limit).is_ok();
    let (mut refused, mut accepted) = (0_usize, 256 * MIB as usize);
    assert!(admitted(accepted), "256 MiB must be enough for a test tree");
    assert!(!admitted(refused));
    while refused + 1 < accepted {
        let middle = refused + (accepted - refused) / 2;
        if admitted(middle) {
            accepted = middle;
        } else {
            refused = middle;
        }
    }
    accepted
}

#[test]
fn comparison_is_refused_before_allocating_beyond_its_allowance() {
    let (old, new) = growth_fixture();
    let (old, new) = (Arc::new(old), Arc::new(new));
    let flag = AtomicBool::new(false);
    let needed = minimal_allowance(&old, &new);
    let refused = compare::compare_within(old.clone(), new.clone(), true, &flag, needed - 1);
    assert_eq!(refused.unwrap_err(), CompareError::MemoryLimit);
    let admitted = compare::compare_within(old.clone(), new.clone(), true, &flag, needed).unwrap();
    let unlimited = compare::compare(old.clone(), new.clone(), true, &flag).unwrap();
    assert_eq!(admitted.totals(LOGICAL), unlimited.totals(LOGICAL));
    assert_eq!(admitted.nodes.len(), unlimited.nodes.len());
    // The estimate covers what the finished diff and its interface indexes keep.
    assert!(admitted.retained_bytes() <= needed);
    // Scope verdicts come first: no allowance turns an incompatible pair into a memory error.
    let mut elsewhere = Builder::fs("/other");
    elsewhere.file(0, "a", 1);
    let elsewhere = Arc::new(elsewhere.build());
    assert!(matches!(
        compare::compare_within(old.clone(), elsewhere, true, &flag, 0),
        Err(CompareError::Incompatible(_))
    ));
}

#[test]
fn the_allowance_grows_with_the_widest_folder_and_the_row_count() {
    let wide = |files: usize, grow: u64| {
        let mut b = Builder::fs("/data");
        for i in 0..files {
            b.file(0, &format!("f{i}"), 1 + grow * (i as u64 % 3));
        }
        Arc::new(b.build())
    };
    let small = minimal_allowance(&wide(100, 0), &wide(100, 1));
    let large = minimal_allowance(&wide(20_000, 0), &wide(20_000, 1));
    // Sibling keys of both sides (24 bytes each) plus 20,000 diff rows.
    assert!(large >= 2 * 20_000 * 24, "{large}");
    assert!(large > small * 50, "{small} -> {large}");
}

#[test]
fn a_cancelled_comparison_publishes_no_diff() {
    let tree = |extra: u64| {
        let mut b = Builder::fs("/data");
        for i in 0..2_500 {
            let dir = b.dir(0, &format!("d{i}"));
            b.file(dir, "f", 1 + extra);
        }
        Arc::new(b.build())
    };
    let (old, new) = (tree(0), tree(1));
    let cancelled = AtomicBool::new(true);
    assert_eq!(
        compare::compare_within(old.clone(), new.clone(), true, &cancelled, usize::MAX)
            .unwrap_err(),
        CompareError::Cancelled
    );
    assert!(compare::compare_within(old, new, true, &AtomicBool::new(false), usize::MAX).is_ok());
}

fn exported(directory: &std::path::Path, name: &str, result: &scan::ScanResult) -> PathBuf {
    let path = directory.join(name);
    scan::write_export(result, &path, true).unwrap();
    path
}
fn imported(path: &std::path::Path) -> Arc<scan::ScanResult> {
    Arc::new(
        spacemosaic::import::import_file(
            path,
            spacemosaic::import::Limits::DESKTOP,
            &AtomicBool::new(false),
            &std::sync::atomic::AtomicU64::new(0),
        )
        .unwrap(),
    )
}

#[test]
fn android_snapshots_compare_on_any_host_by_their_recorded_source() {
    let folder =
        std::env::temp_dir().join(format!("spacemosaic-android-pair-{}", std::process::id()));
    let _ = fs::remove_dir_all(&folder);
    fs::create_dir_all(&folder).unwrap();
    let root = "content://com.android.externalstorage.documents/tree/primary%3ATest/document/primary%3ATest";
    let snapshot = |day: u64, tail: u64, extra: bool| {
        let mut b = Builder::provider(root);
        let sub = b.dir(0, "Sub");
        b.file(sub, "keep.bin", 10 * MIB);
        b.file(sub, "grows.bin", tail * MIB);
        if extra {
            b.file(0, "new.bin", 3 * MIB);
        } else {
            b.file(0, "gone.bin", 9 * MIB);
        }
        let mut result = b.build();
        result.started_unix_seconds = 1_790_000_000 + day * 86_400;
        result
    };
    let before = exported(&folder, "pixel-day1.json", &snapshot(0, 4, false));
    let after = exported(&folder, "pixel-day2.json", &snapshot(2, 6, true));
    let (before, after) = (imported(&before), imported(&after));
    assert_eq!(before.scope_identity.as_ref().unwrap().platform, "android");
    let assessment = assess_scope(&before, &after);
    assert_eq!(assessment.verdict, ScopeVerdict::Matching);
    let comparison = compare::compare(
        before.clone(),
        after.clone(),
        false,
        &AtomicBool::new(false),
    )
    .expect("no confirmation is needed: the recorded source decides");
    assert_eq!(status(&comparison, "Sub/grows.bin"), Status::Grew);
    assert_eq!(status(&comparison, "gone.bin"), Status::Removed);
    assert_eq!(status(&comparison, "new.bin"), Status::Added);
    assert_eq!(status(&comparison, "Sub/keep.bin"), Status::SameSize);
    let totals = comparison.totals(LOGICAL);
    assert_eq!(
        (totals.added, totals.removed, totals.grown),
        (3 * MIB as u128, 9 * MIB as u128, 2 * MIB as u128)
    );
    assert_eq!(totals.net(), -(4 * MIB as i128));
    assert!(totals.is_complete());
    // Providers do not report allocation, so that dimension is not offered.
    assert!(!comparison.allocated_available);
    // Reversing the pair inverts every amount.
    let back = compare::compare(
        after.clone(),
        before.clone(),
        false,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(status(&back, "gone.bin"), Status::Added);
    assert_eq!(back.totals(LOGICAL).net(), 4 * MIB as i128);
    // A filesystem snapshot from the host is a different kind of area and stays so.
    let mut local = Builder::fs("/Users/test/Pixel");
    local.file(0, "a", 1);
    let local = Arc::new(local.build());
    assert_eq!(
        assess_scope(&before, &local).notes,
        vec![ScopeNote::DifferentSource]
    );
    assert!(matches!(
        compare::compare(before, local, true, &AtomicBool::new(false)),
        Err(CompareError::Incompatible(_))
    ));
    let _ = fs::remove_dir_all(&folder);
}
