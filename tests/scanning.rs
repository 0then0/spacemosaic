use spacemosaic::scan::{self, Options, Progress, Service, Sort};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "spacemosaic-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn scan(&self, options: Options) -> scan::ScanResult {
        scan::scan(
            &self.0,
            options,
            &AtomicBool::new(false),
            &Mutex::new(Progress::default()),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn recursive_aggregation_filter_sort_and_export() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("nested")).unwrap();
    fs::write(f.0.join("a.txt"), [1; 7]).unwrap();
    fs::write(f.0.join("nested/b.TXT"), [0; 13]).unwrap();
    fs::write(f.0.join("empty"), []).unwrap();
    let result = f.scan(Options::default());
    assert_eq!(result.nodes[0].logical.known, 20);
    assert!(result.nodes[0].complete);
    assert_eq!(result.search("b.txt", false, Sort::Name).len(), 1);
    assert_eq!(
        result.nodes[result.search("", true, Sort::Size)[0]].name,
        "b.TXT"
    );
    let groups = result.extensions();
    assert_eq!(groups[0].0, "txt");
    assert_eq!(groups[0].1, 2);
    let decoded: scan::ScanResult = serde_json::from_slice(&result.json().unwrap()).unwrap();
    assert_eq!(decoded.schema_version, scan::UNFILTERED_SCHEMA_VERSION);
    assert_eq!(decoded.nodes[0].logical.known, 20);
    assert!(result.csv().contains("\"txt\",2,20,0,false"));
}
#[test]
fn cancellation_and_memory_limit_keep_explicit_partial_results() {
    let f = Fixture::new();
    for index in 0..10 {
        fs::write(f.0.join(index.to_string()), [0; 3]).unwrap();
    }
    let result = scan::scan(
        &f.0,
        Options::default(),
        &AtomicBool::new(true),
        &Mutex::new(Progress::default()),
    );
    assert!(result.cancelled);
    assert_eq!(result.limit_reason, None);
    assert_eq!(result.incomplete_reason(), Some("scan_cancelled"));
    assert!(!result.nodes[0].complete);
    assert!(result.nodes[0].logical.unknown > 0);
    let limited = f.scan(Options {
        node_limit: 3,
        ..Options::default()
    });
    assert!(limited.limited);
    assert_eq!(limited.limit_reason, Some(scan::LimitReason::NodeLimit));
    assert_eq!(limited.nodes.len(), 3);
    assert!(!limited.nodes[0].complete);
}

#[test]
fn only_included_children_use_node_slots() {
    let fixture = Fixture::new();
    let cache = fixture.0.join("cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join("payload"), b"hidden").unwrap();
    let mut options = Options {
        node_limit: 1,
        ..Options::default()
    };
    options.exclusions.add("cache").unwrap();

    let result = fixture.scan(options.clone());
    assert_eq!(result.nodes.len(), 1);
    assert!(result.exclusions[0].skipped);
    assert_eq!(result.incomplete_reason(), None);
    assert_eq!(result.nodes[0].listing_complete, Some(true));

    fs::write(fixture.0.join("keep"), b"included").unwrap();
    let limited = fixture.scan(options);
    assert_eq!(limited.nodes.len(), 1);
    assert_eq!(limited.limit_reason, Some(scan::LimitReason::NodeLimit));
}
#[test]
fn missing_root_has_unknown_metadata_instead_of_successful_zero() {
    let f = Fixture::new();
    let result = scan::scan(
        &f.0.join("gone"),
        Options::default(),
        &AtomicBool::new(false),
        &Mutex::new(Progress::default()),
    );
    assert_eq!(result.errors, 1);
    assert_eq!(result.nodes[0].logical.unknown, 1);
    assert_eq!(result.nodes[0].error.as_deref(), Some("disappeared"));
}
#[test]
fn latest_job_has_its_own_results_and_worker_survives_replacement() {
    let a = Fixture::new();
    let b = Fixture::new();
    fs::write(b.0.join("only"), [0; 19]).unwrap();
    let service = Service::default();
    let old = service.start(a.0.clone(), Options::default());
    let current = service.start(b.0.clone(), Options::default());
    drop(old);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = current.take() {
            assert_eq!(result.scope, b.0.to_string_lossy());
            assert_eq!(result.nodes[0].logical.known, 19);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[cfg(unix)]
#[test]
fn symlink_cycle_is_not_followed_and_hard_links_are_counted_per_path() {
    use spacemosaic::scan::Kind;
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    fs::write(f.0.join("file"), [0; 11]).unwrap();
    fs::hard_link(f.0.join("file"), f.0.join("hard")).unwrap();
    symlink(&f.0, f.0.join("cycle")).unwrap();
    let result = f.scan(Options::default());
    assert_eq!(result.nodes.len(), 4);
    assert_eq!(result.nodes[0].logical.known, 22);
    assert_eq!(
        result.nodes.iter().filter(|n| n.kind == Kind::Link).count(),
        1
    );
    assert_eq!(result.nodes.iter().filter(|n| n.hard_link).count(), 2);
}

#[cfg(unix)]
#[test]
fn folder_rules_leave_same_named_files_and_symlinks_in_the_scan() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    fs::write(f.0.join("file"), [0; 11]).unwrap();
    fs::create_dir(f.0.join("real")).unwrap();
    fs::write(f.0.join("real/marker"), [0; 5]).unwrap();
    symlink(f.0.join("real"), f.0.join("alias")).unwrap();
    let mut options = Options::default();
    for path in ["file", "alias"] {
        options.exclusions.add(path).unwrap();
    }
    let result = f.scan(options);
    assert!(result.nodes[0].complete);
    assert_eq!(result.nodes[0].logical.known, 16);
    assert!(
        result
            .nodes
            .iter()
            .any(|node| node.name == "alias" && node.kind == scan::Kind::Link)
    );
    assert!(result.exclusions.iter().all(|rule| !rule.skipped));
}
#[cfg(all(unix, not(any(target_os = "android", target_os = "ios"))))]
#[test]
fn hard_link_paths_are_scoped_to_the_scan_and_keep_per_path_sizes() {
    use std::os::unix::fs::{MetadataExt, symlink};

    let fixture = Fixture::new();
    let left = fixture.0.join("left");
    let right = fixture.0.join("right");
    fs::create_dir(&left).unwrap();
    fs::create_dir(&right).unwrap();
    let original = left.join("original.bin");
    let alias = right.join("alias.bin");
    let copy = left.join("copy.bin");
    let link = left.join("symlink.bin");
    fs::write(&original, [7; 8193]).unwrap();
    fs::hard_link(&original, &alias).unwrap();
    fs::copy(&original, &copy).unwrap();
    symlink(&original, &link).unwrap();

    let metadata = fs::symlink_metadata(&original).unwrap();
    let identity = |m: &fs::Metadata| (m.dev(), m.ino(), m.nlink());
    assert_eq!(metadata.nlink(), 2);
    assert_eq!(
        identity(&metadata),
        identity(&fs::symlink_metadata(&alias).unwrap())
    );
    assert_ne!(metadata.ino(), fs::symlink_metadata(&copy).unwrap().ino());

    let mut full = fixture.scan(Options::default());
    let subfolder = scan::scan(
        &left,
        Options::default(),
        &AtomicBool::new(false),
        &Mutex::new(Progress::default()),
    );
    for (result, files) in [
        (&full, vec![&original, &alias, &copy]),
        (&subfolder, vec![&original, &copy]),
    ] {
        let index = |path| {
            result
                .nodes
                .iter()
                .position(|n| n.path.as_ref() == Some(path))
                .unwrap()
        };
        let groups = result.hard_link_groups.as_ref().unwrap();
        let original_id = index(&original);
        let group = groups.get(original_id).unwrap();
        assert_eq!(
            (group.min_nlink, group.max_nlink),
            (metadata.nlink(), metadata.nlink())
        );
        if files.len() == 3 {
            let alias_id = index(&alias);
            assert_eq!(group.nodes.len(), 2);
            assert!(group.nodes.contains(&original_id) && group.nodes.contains(&alias_id));
            assert_eq!(groups.get(alias_id).unwrap().nodes, group.nodes);
        } else {
            assert_eq!(group.nodes, [original_id]);
        }
        assert!(groups.get(index(&copy)).is_none());
        assert!(groups.get(index(&link)).is_none());
        assert_eq!(result.nodes[index(&link)].kind, scan::Kind::Link);
        assert_eq!(result.incomplete_reason(), None);

        let mut expected_logical = 0;
        let mut expected_allocated = 0;
        for path in files {
            let stat = fs::symlink_metadata(path).unwrap();
            let node = &result.nodes[index(path)];
            assert_eq!(node.logical.known, stat.len());
            assert_eq!(node.allocated.known, stat.blocks() * 512);
            expected_logical += stat.len();
            expected_allocated += stat.blocks() * 512;
        }
        assert_eq!(result.nodes[0].logical.known, expected_logical);
        assert_eq!(result.nodes[0].allocated.known, expected_allocated);
    }

    let retained = full.retained_bytes();
    let json = full.json().unwrap();
    assert!(
        serde_json::from_slice::<scan::ScanResult>(&json)
            .unwrap()
            .hard_link_groups
            .is_none()
    );
    full.hard_link_groups.take();
    assert!(full.retained_bytes() < retained);
    assert_eq!(json, full.json().unwrap());
}

#[cfg(all(unix, not(any(target_os = "android", target_os = "ios"))))]
#[test]
fn stopped_scan_keeps_observed_hard_link_members() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("file"), [0; 11]).unwrap();
    fs::hard_link(fixture.0.join("file"), fixture.0.join("alias")).unwrap();
    let result = fixture.scan(Options {
        node_limit: 2,
        ..Options::default()
    });
    assert_eq!(result.incomplete_reason(), Some("scan_node_limit"));
    let group = result.hard_link_groups.as_ref().unwrap().get(1).unwrap();
    assert_eq!(group.nodes, [1]);
    assert_eq!(group.max_nlink, 2);
    assert_eq!(result.nodes[0].logical.known, 11);
    assert!(!result.nodes[0].complete);
}

#[cfg(target_os = "linux")]
#[test]
fn invalid_utf8_paths_roundtrip_in_machine_export() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let f = Fixture::new();
    let name = OsString::from_vec(vec![b'x', 0xff]);
    fs::write(f.0.join(name), [0; 5]).unwrap();
    let result = f.scan(Options::default());
    let raw = result
        .nodes
        .iter()
        .find(|n| n.native_locator.ends_with("78ff"))
        .unwrap();
    assert_eq!(raw.logical.known, 5);
}
#[test]
fn csv_quotes_untrusted_extension_names() {
    let f = Fixture::new();
    fs::write(f.0.join("file.=sum"), [0; 1]).unwrap();
    assert!(f.scan(Options::default()).csv().contains("\"'=sum\""));
}

#[cfg(unix)]
#[test]
fn sparse_sizes_stay_distinct() {
    let f = Fixture::new();
    let file = fs::File::create(f.0.join("sparse")).unwrap();
    file.set_len(1024 * 1024).unwrap();
    let result = f.scan(Options::default());
    let sparse = result.nodes.iter().find(|n| n.name == "sparse").unwrap();
    assert_eq!(sparse.logical.known, 1024 * 1024);
    assert!(sparse.allocated.known <= sparse.logical.known);
}
#[test]
fn export_replaces_complete_destination_and_keeps_failed_target_untouched() {
    let f = Fixture::new();
    fs::write(f.0.join("file"), [0; 7]).unwrap();
    let result = f.scan(Options::default());
    let destination = f.0.join("result.json");
    fs::write(&destination, b"old").unwrap();
    scan::write_export(&result, &destination, true).unwrap();
    let restored: scan::ScanResult =
        serde_json::from_slice(&fs::read(&destination).unwrap()).unwrap();
    assert_eq!(restored.nodes[0].logical.known, 7);
    let missing = f.0.join("missing/result.csv");
    assert!(scan::write_export(&result, &missing, false).is_err());
    assert!(!missing.exists());
}

#[test]
fn metadata_memory_limit_stops_with_an_explicit_partial_result() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("long-file-name.txt"), [0; 4]).unwrap();
    let result = fixture.scan(Options {
        memory_limit: 1,
        ..Options::default()
    });
    assert!(result.limited);
    assert_eq!(result.limit_reason, Some(scan::LimitReason::MemoryLimit));
    assert!(!result.nodes[0].complete);
    assert!(result.nodes[0].logical.unknown > 0);
}

#[test]
fn queued_directories_are_unknown_at_the_limit_and_separate_scans_measure_their_contents() {
    let fixture = Fixture::new();
    for name in [".agents", "other"] {
        fs::create_dir(fixture.0.join(name)).unwrap();
        fs::write(fixture.0.join(name).join("payload"), vec![1; 125739]).unwrap();
    }
    let limited = fixture.scan(Options {
        node_limit: 3,
        ..Default::default()
    });
    assert_eq!(limited.incomplete_reason(), Some("scan_node_limit"));
    let directory = limited
        .nodes
        .iter()
        .find(|node| node.name == ".agents")
        .unwrap();
    assert!(!directory.complete);
    assert_eq!(directory.logical.known, 0);
    assert_eq!(directory.logical.unknown, 1);
    let result = scan::scan(
        directory.path.as_ref().unwrap(),
        Options::default(),
        &AtomicBool::new(false),
        &Mutex::new(Progress::default()),
    );
    assert!(result.nodes[0].complete);
    assert_eq!(result.nodes[0].logical.known, 125739);
    assert_eq!(result.nodes[0].logical.unknown, 0);
    let complete = fixture.scan(Options::default());
    assert_eq!(complete.nodes[0].logical.known, 251478);
    assert_eq!(complete.incomplete_reason(), None);
    let exported = limited.json().unwrap();
    let decoded: scan::ScanResult = serde_json::from_slice(&exported).unwrap();
    assert_eq!(decoded.limit_reason, Some(scan::LimitReason::NodeLimit));
    let mut legacy: serde_json::Value = serde_json::from_slice(&exported).unwrap();
    legacy.as_object_mut().unwrap().remove("limit_reason");
    let legacy: scan::ScanResult = serde_json::from_value(legacy).unwrap();
    assert_eq!(legacy.incomplete_reason(), Some("scan_limited"));
}
#[cfg(unix)]
#[test]
fn denied_directory_is_reported_without_claiming_a_complete_scan() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let directory = fixture.0.join("denied");
    fs::create_dir(&directory).unwrap();
    let visible = fixture.0.join("visible");
    fs::write(&visible, b"shared").unwrap();
    fs::hard_link(&visible, directory.join("hidden")).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o000)).unwrap();
    let result = fixture.scan(Options::default());
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.errors > 0);
    assert!(!result.nodes[0].complete);
    assert_eq!(result.incomplete_reason(), Some("scan_incomplete"));
    assert!(
        result
            .nodes
            .iter()
            .any(|node| node.error.as_deref() == Some("permission_denied"))
    );
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let index = result
            .nodes
            .iter()
            .position(|node| node.name == "visible")
            .unwrap();
        let group = result
            .hard_link_groups
            .as_ref()
            .unwrap()
            .get(index)
            .unwrap();
        assert_eq!(group.nodes, [index]);
        assert_eq!((group.min_nlink, group.max_nlink), (2, 2));
    }
}
