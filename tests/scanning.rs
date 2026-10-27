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
    assert_eq!(decoded.schema_version, 1);
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
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o000)).unwrap();
    let result = fixture.scan(Options::default());
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.errors > 0);
    assert!(!result.nodes[0].complete);
    assert!(
        result
            .nodes
            .iter()
            .any(|node| node.error.as_deref() == Some("permission_denied"))
    );
}
