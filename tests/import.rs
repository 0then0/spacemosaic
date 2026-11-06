mod common;
use common::*;
use serde_json::{Value, json};
use spacemosaic::{
    compare::{self, Dimension, Status},
    import::{self, ImportError, Limits},
    job, memory,
    scan::{self, Options, Progress, ScanResult},
};
use std::{
    fs,
    io::Read,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

fn load(bytes: &[u8]) -> Result<ScanResult, ImportError> {
    load_with(bytes, Limits::DESKTOP)
}
fn load_with(bytes: &[u8], limits: Limits) -> Result<ScanResult, ImportError> {
    import::import_reader(bytes, limits, &AtomicBool::new(false), &AtomicU64::new(0))
}
fn sample(v1: bool) -> ScanResult {
    let mut b = Builder::fs("/data");
    b.v1 = v1;
    let d = b.dir(0, "docs");
    b.file(d, "a.txt", 100);
    b.unknown_file(d, "unreadable.bin");
    let locked = b.dir(0, "locked");
    b.deny(locked);
    b.link(0, "alias");
    b.build()
}
fn value(result: &ScanResult) -> Value {
    serde_json::to_value(result).unwrap()
}
#[track_caller]
fn rejected(mutate: impl FnOnce(&mut Value), v1: bool) -> ImportError {
    let mut document = value(&sample(v1));
    mutate(&mut document);
    load(&serde_json::to_vec(&document).unwrap()).unwrap_err()
}
#[track_caller]
fn code(error: ImportError) -> &'static str {
    match error {
        ImportError::Invalid { code, .. } => code,
        other => panic!("expected an invalid-document error, got {other:?}"),
    }
}

/// A schema-1 document in the exact shape written by 0.1.x releases.
const SCHEMA_ONE: &str = r#"{
  "schema_version": 1, "started_unix_seconds": 1790000000, "elapsed_millis": 12,
  "scope": "/data", "cancelled": true, "limited": true, "limit_reason": "memory_limit",
  "cross_filesystems": false, "errors": 1,
  "nodes": [
    {"parent": null, "name": "data", "locator": "/data", "native_locator": "unix-bytes:2f64617461",
     "kind": "directory", "logical": {"known": 5, "unknown": 1, "overflow": false},
     "allocated": {"known": 4096, "unknown": 1, "overflow": false},
     "complete": false, "hard_link": false, "error": null},
    {"parent": 0, "name": "a", "locator": "/data/a", "native_locator": "unix-bytes:2f646174612f61",
     "kind": "file", "logical": {"known": 5, "unknown": 0, "overflow": false},
     "allocated": {"known": 4096, "unknown": 0, "overflow": false},
     "complete": true, "hard_link": false, "error": null},
    {"parent": 0, "name": "b", "locator": "/data/b", "native_locator": "unix-bytes:2f646174612f62",
     "kind": "directory", "logical": {"known": 0, "unknown": 1, "overflow": false},
     "allocated": {"known": 0, "unknown": 1, "overflow": false},
     "complete": false, "hard_link": false, "error": "permission_denied"}
  ]
}"#;

#[test]
fn schema_one_files_open_with_unknowns_and_partial_flags_intact() {
    let result = load(SCHEMA_ONE.as_bytes()).unwrap();
    assert_eq!(result.schema_version, 1);
    assert!(result.cancelled && result.limited);
    assert_eq!(result.limit_reason, Some(scan::LimitReason::MemoryLimit));
    assert!(result.scope_identity.is_none());
    assert!(
        result
            .nodes
            .iter()
            .all(|n| n.listing_complete.is_none() && n.path.is_none())
    );
    assert_eq!(result.nodes[0].logical.unknown, 1);
    assert_eq!(result.nodes[2].error.as_deref(), Some("permission_denied"));
    // Re-saving does not upgrade the document or invent precision.
    let again = value(&result);
    assert_eq!(again["schema_version"], 1);
    assert!(again.get("scope_identity").is_none());
    assert!(again["nodes"][0].get("listing_complete").is_none());
    assert!(load(&serde_json::to_vec(&again).unwrap()).is_ok());
}

#[test]
fn schema_one_listing_completeness_is_inferred_only_from_complete_subtrees() {
    // In schema 1 only a complete directory proves a complete listing; the root here is
    // incomplete, so no object may be called new or removed directly under it.
    let old = Arc::new(load(SCHEMA_ONE.as_bytes()).unwrap());
    let mut b = Builder::fs("/data");
    b.v1 = true;
    b.file(0, "a", 5);
    b.file(0, "extra", 1);
    let new = Arc::new(b.build());
    let comparison = compare::compare(old, new, true, &AtomicBool::new(false)).unwrap();
    let extra = (0..comparison.nodes.len())
        .find(|&i| comparison.name(i) == "extra")
        .unwrap();
    assert_eq!(
        comparison.nodes[extra].status(Dimension::Logical),
        Status::Unknown
    );
}

#[test]
fn damaged_or_unsupported_documents_are_rejected_with_specific_errors() {
    let good = serde_json::to_vec(&value(&sample(false))).unwrap();
    assert!(load(&good).is_ok());
    assert_eq!(load(b"").unwrap_err(), ImportError::Malformed);
    assert_eq!(load(b"[1,2]").unwrap_err(), ImportError::InvalidValue);
    assert_eq!(
        load(&good[..good.len() / 2]).unwrap_err(),
        ImportError::Malformed
    );
    let mut trailing = good.clone();
    trailing.extend_from_slice(b" []");
    assert_eq!(load(&trailing).unwrap_err(), ImportError::Malformed);
    for version in [0, u64::from(scan::SCHEMA_VERSION) + 1, 99] {
        assert_eq!(
            rejected(|d| d["schema_version"] = json!(version), false),
            ImportError::UnsupportedVersion(version)
        );
    }
    assert_eq!(
        rejected(|d| d["schema_version"] = json!("2"), false),
        ImportError::InvalidValue
    );
    assert_eq!(
        rejected(|d| d["nodes"][1]["kind"] = json!("socket"), false),
        ImportError::InvalidValue
    );
    assert_eq!(
        rejected(|d| d["limit_reason"] = json!("bored"), false),
        ImportError::InvalidValue
    );
    assert_eq!(
        rejected(|d| d["nodes"][1]["logical"]["known"] = json!(-1), false),
        ImportError::InvalidValue
    );
    assert_eq!(
        rejected(|d| d["nodes"][1]["logical"]["known"] = json!(1.5e30), false),
        ImportError::InvalidValue
    );
    assert_eq!(
        code(rejected(
            |d| {
                d.as_object_mut().unwrap().remove("scope");
            },
            false
        )),
        "missing_field"
    );
    assert_eq!(code(rejected(|d| d["nodes"] = json!([]), false)), "empty");
}

#[test]
fn structure_cycles_and_forward_references_are_refused_without_recursion() {
    assert_eq!(
        code(rejected(|d| d["nodes"][1]["parent"] = json!(1), false)),
        "parent_order"
    );
    assert_eq!(
        code(rejected(|d| d["nodes"][1]["parent"] = json!(2), false)),
        "parent_order"
    );
    assert_eq!(
        code(rejected(|d| d["nodes"][1]["parent"] = json!(9999), false)),
        "parent_order"
    );
    assert_eq!(
        code(rejected(|d| d["nodes"][0]["parent"] = json!(0), false)),
        "root_parent"
    );
    assert_eq!(
        code(rejected(|d| d["nodes"][2]["parent"] = Value::Null, false)),
        "root_parent"
    );
    // A file cannot be a parent.
    assert_eq!(
        code(rejected(|d| d["nodes"][3]["parent"] = json!(2), false)),
        "parent_kind"
    );
    // A long cyclic chain of forward pointers is rejected at its first violation.
    let nodes: Vec<Value> = (0..1000)
        .map(|i| {
            let mut n = value(&sample(false))["nodes"][1].clone();
            n["parent"] = json!((i + 1) % 1000);
            n
        })
        .collect();
    let mut document = value(&sample(false));
    document["nodes"] = Value::Array(nodes);
    assert!(load(&serde_json::to_vec(&document).unwrap()).is_err());
}

#[test]
fn aggregates_flags_and_locators_must_agree_with_the_measured_leaves() {
    assert_eq!(
        code(rejected(
            |d| d["nodes"][0]["logical"]["known"] = json!(999),
            false
        )),
        "aggregate"
    );
    assert_eq!(
        code(rejected(
            |d| d["nodes"][1]["logical"]["known"] = json!(1),
            false
        )),
        "aggregate"
    );
    assert_eq!(
        code(rejected(
            |d| d["nodes"][1]["logical"]["unknown"] = json!(0),
            false
        )),
        "aggregate"
    );
    assert_eq!(
        code(rejected(
            |d| d["nodes"][0]["allocated"]["known"] = json!(1),
            false
        )),
        "aggregate"
    );
    assert_eq!(
        code(rejected(
            |d| d["nodes"][0]["logical"]["overflow"] = json!(true),
            false
        )),
        "aggregate"
    );
    // A file with a negative-looking unknown count, overflow or two unknowns is impossible.
    assert_eq!(
        code(rejected(
            |d| d["nodes"][3]["logical"]["overflow"] = json!(true),
            false
        )),
        "measure"
    );
    assert_eq!(
        code(rejected(
            |d| d["nodes"][3]["logical"]["unknown"] = json!(2),
            false
        )),
        "measure"
    );
    assert_eq!(
        code(rejected(|d| d["nodes"][4]["complete"] = json!(true), false)),
        "complete_flag"
    );
    assert_eq!(
        code(rejected(|d| d["nodes"][0]["complete"] = json!(true), false)),
        "complete_flag"
    );
    assert_eq!(
        code(rejected(
            |d| d["nodes"][1]["error"] = json!("Not A Code!"),
            false
        )),
        "error_code"
    );
    assert_eq!(
        code(rejected(|d| d["nodes"][1]["name"] = json!(""), false)),
        "empty_name"
    );
    assert_eq!(
        code(rejected(
            |d| d["nodes"][1]["name"] = json!("x".repeat(5000)),
            false
        )),
        "text_length"
    );
    assert_eq!(
        code(rejected(
            |d| d["nodes"][1]["native_locator"] = json!("unix-bytes:zz"),
            false
        )),
        "locator_text"
    );
    assert_eq!(
        code(rejected(
            |d| d["nodes"][1]["locator"] = json!("/etc/passwd"),
            false
        )),
        "locator_text"
    );
    assert_eq!(
        code(rejected(
            |d| d["nodes"][1]["native_locator"] = json!("unix-bytes:2f6f74686572"),
            false
        )),
        "locator_text"
    );
    assert_eq!(
        code(rejected(|d| d["scope"] = json!("/somewhere/else"), false)),
        "scope_mismatch"
    );
    assert_eq!(
        code(rejected(|d| d["limit_reason"] = json!("node_limit"), false)),
        "limit_reason"
    );
    assert_eq!(
        code(rejected(
            |d| d["nodes"][1]["hard_link"] = json!(true),
            false
        )),
        "hard_link"
    );
    // Schema 2 requires identity and listing flags that match the rest of the document.
    assert_eq!(
        code(rejected(
            |d| {
                d.as_object_mut().unwrap().remove("scope_identity");
            },
            false
        )),
        "identity"
    );
    assert_eq!(
        code(rejected(
            |d| d["scope_identity"]["root"] = json!("unix-bytes:00"),
            false
        )),
        "identity"
    );
    assert_eq!(
        code(rejected(
            |d| {
                d["nodes"][1]
                    .as_object_mut()
                    .unwrap()
                    .remove("listing_complete");
            },
            false
        )),
        "listing_flag"
    );
    assert_eq!(
        code(rejected(
            |d| d["nodes"][1]["listing_complete"] = json!(false),
            false
        )),
        "listing_flag"
    );
    assert_eq!(
        code(rejected(
            |d| d["nodes"][3]["listing_complete"] = json!(true),
            false
        )),
        "listing_flag"
    );
    assert_eq!(
        code(rejected(
            |d| d["scope_identity"] =
                json!({"source":"filesystem","platform":"macos","root":"unix-bytes:2f64617461"}),
            true
        )),
        "identity"
    );
    // A child must lie under its parent's exact bytes.
    let mut b = Builder::fs("/data");
    let sub = b.dir(0, "sub");
    b.file(sub, "f", 1);
    let mut document = value(&b.build());
    document["nodes"][2]["parent"] = json!(0);
    document["nodes"][1]["logical"] = json!({"known":0,"unknown":0,"overflow":false});
    document["nodes"][1]["allocated"] = json!({"known":0,"unknown":0,"overflow":false});
    document["nodes"][2]["native_locator"] = json!("unix-bytes:2f6f7468657273");
    document["nodes"][2]["locator"] = json!("/others");
    assert!(load(&serde_json::to_vec(&document).unwrap()).is_err());
}

#[test]
fn byte_and_memory_budgets_are_enforced_while_streaming() {
    let document = serde_json::to_vec(&value(&sample(false))).unwrap();
    let small = Limits {
        max_bytes: 200,
        ..Limits::DESKTOP
    };
    assert_eq!(
        load_with(&document, small).unwrap_err(),
        ImportError::TooLarge { limit: 200 }
    );
    let tight = Limits {
        memory_limit: 100,
        ..Limits::DESKTOP
    };
    assert_eq!(
        load_with(&document, tight).unwrap_err(),
        ImportError::MemoryLimit
    );
    // An endless array of valid objects runs out of memory budget before input bytes.
    struct Endless {
        head: Vec<u8>,
        node: Vec<u8>,
        pos: usize,
    }
    impl Read for Endless {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            for slot in buffer.iter_mut() {
                *slot = if self.pos < self.head.len() {
                    self.head[self.pos]
                } else {
                    self.node[(self.pos - self.head.len()) % self.node.len()]
                };
                self.pos += 1;
            }
            Ok(buffer.len())
        }
    }
    let mut node = value(&sample(false))["nodes"][3].clone();
    node["parent"] = json!(0);
    let mut head = br#"{"schema_version":2,"nodes":[{"parent":null,"name":"data","locator":"/data","native_locator":"unix-bytes:2f64617461","kind":"directory","logical":{"known":0,"unknown":0,"overflow":false},"allocated":{"known":0,"unknown":0,"overflow":false},"complete":true,"hard_link":false,"error":null,"listing_complete":true}"#.to_vec();
    head.push(b',');
    let mut node_text = serde_json::to_vec(&node).unwrap();
    node_text.push(b',');
    let started = Instant::now();
    let endless = Endless {
        head,
        node: node_text,
        pos: 0,
    };
    let limits = Limits {
        max_bytes: MIB,
        memory_limit: 128 * 1024,
    };
    let progress = AtomicU64::new(0);
    assert_eq!(
        import::import_reader(endless, limits, &AtomicBool::new(false), &progress).unwrap_err(),
        ImportError::MemoryLimit
    );
    assert!(progress.load(Ordering::Relaxed) < limits.max_bytes);
    assert!(started.elapsed() < Duration::from_secs(10));
    // An unbounded stream of whitespace is cut off by the byte budget.
    struct Spaces;
    impl Read for Spaces {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            buffer.fill(b' ');
            Ok(buffer.len())
        }
    }
    let limits = Limits {
        max_bytes: 1 << 20,
        ..Limits::DESKTOP
    };
    assert_eq!(
        import::import_reader(Spaces, limits, &AtomicBool::new(false), &AtomicU64::new(0))
            .unwrap_err(),
        ImportError::TooLarge { limit: 1 << 20 }
    );
    // Mobile budgets are stricter than desktop ones.
    const { assert!(Limits::MOBILE.max_bytes < Limits::DESKTOP.max_bytes) };
    const { assert!(Limits::MOBILE.memory_limit < Limits::DESKTOP.memory_limit) };
}

#[test]
fn unknown_fields_are_skipped_within_the_nesting_limit() {
    let mut document = value(&sample(false));
    let delimiters = r#"\"[{"#.repeat(256);
    document["future"] = json!({"items": [1, 2, {"text": delimiters}]});
    assert!(load(&serde_json::to_vec(&document).unwrap()).is_ok());

    let mut document = sample(false).json().unwrap();
    assert_eq!(document.pop(), Some(b'}'));
    document.extend_from_slice(br#", "future":"#);
    document.extend(std::iter::repeat_n(b'[', 256));
    document.extend_from_slice(b"null");
    document.extend(std::iter::repeat_n(b']', 256));
    document.push(b'}');
    assert_eq!(load(&document).unwrap_err(), ImportError::Malformed);
}

#[test]
fn an_oversized_string_runs_out_of_memory_budget_while_streaming() {
    let limits = Limits {
        max_bytes: 2 * MIB,
        memory_limit: 512 * 1024,
    };
    let mut builder = Builder::fs("/data");
    let name = r#"a "quote" and a \backslash.txt"#;
    builder.file(0, name, 1);
    let document = builder.build().json().unwrap();
    // Split an escape across reader fills.
    let escape = document.iter().position(|&byte| byte == b'\\').unwrap();
    let (head, tail) = document.split_at(escape + 1);
    let loaded = import::import_reader(
        head.chain(tail),
        limits,
        &AtomicBool::new(false),
        &AtomicU64::new(0),
    )
    .unwrap();
    assert_eq!(loaded.nodes[1].name, name);

    // The parser must stop before its scratch buffer grows past the budget.
    let prefix = b"{\"schema_version\":3,\"scope\":\"";
    let endless = prefix.as_slice().chain(std::io::repeat(b'x'));
    let progress = AtomicU64::new(0);
    assert_eq!(
        import::import_reader(endless, limits, &AtomicBool::new(false), &progress).unwrap_err(),
        ImportError::MemoryLimit
    );
    let read = progress.load(Ordering::Relaxed);
    assert!(read > 0 && read < limits.memory_limit as u64);
}

#[test]
fn byte_order_marks_are_tolerated_and_cancellation_is_honored() {
    let mut document = vec![0xEF, 0xBB, 0xBF];
    document.extend(serde_json::to_vec(&value(&sample(false))).unwrap());
    assert!(load(&document).is_ok());
    let cancelled = AtomicBool::new(true);
    assert_eq!(
        import::import_reader(
            document.as_slice(),
            Limits::DESKTOP,
            &cancelled,
            &AtomicU64::new(0)
        )
        .unwrap_err(),
        ImportError::Cancelled
    );
}

struct Folder(PathBuf);
impl Folder {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("spacemosaic-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const LARGE_GROUPS: usize = 500;
const FILES_PER_GROUP: usize = 1_000;
const FILE_BYTES: u64 = 37;

fn large_partial_snapshot() -> ScanResult {
    let mut builder = Builder::fs("/synthetic");
    for group in 0..LARGE_GROUPS {
        let dir = builder.dir(0, &format!("group-{group}"));
        for file in 0..FILES_PER_GROUP {
            let suffix = if file.is_multiple_of(2) { "bin" } else { "txt" };
            builder.file(dir, &format!("item-{file}.{suffix}"), FILE_BYTES);
        }
    }
    let locked = builder.dir(0, "locked");
    builder.deny(locked);
    builder.abort_listing(0);
    let mut result = builder.build();
    result.schema_version = scan::SCHEMA_VERSION;
    result.limited = true;
    result.limit_reason = Some(scan::LimitReason::MemoryLimit);
    result.memory = Some(memory::Diagnostics {
        budget_bytes: 256 * MIB,
        ..Default::default()
    });
    result
}

#[test]
fn a_large_export_with_repeated_extensions_reopens_within_budget() {
    let saved = Folder::new("large-round-trip");
    let path = saved.0.join("large-partial.json");
    scan::write_export(&large_partial_snapshot(), &path, true).unwrap();
    let file_count = LARGE_GROUPS * FILES_PER_GROUP;
    let cancel = AtomicBool::new(false);
    let progress = AtomicU64::new(0);
    let ample = Limits {
        memory_limit: 256 * MIB as usize,
        ..Limits::DESKTOP
    };
    let imported = import::import_file(&path, ample, &cancel, &progress).unwrap();
    assert_eq!(imported.nodes.len(), file_count + LARGE_GROUPS + 2);
    assert_eq!(imported.schema_version, scan::SCHEMA_VERSION);
    let root = &imported.nodes[0];
    for total in [root.logical, root.allocated] {
        assert_eq!(total.known, file_count as u64 * FILE_BYTES);
        assert_eq!(total.unknown, 1);
        assert!(!total.overflow);
    }
    assert!(!root.complete);
    assert_eq!(root.listing_complete, Some(false));
    assert_eq!(imported.errors, 1);
    assert_eq!(imported.incomplete_reason(), Some("scan_memory_limit"));
    assert_eq!(imported.memory.as_ref().unwrap().budget_bytes, 256 * MIB);
    let dirs = imported.children(0, "group-499", scan::Sort::Name);
    assert_eq!(dirs.len(), 1);
    let files = imported.children(dirs[0], "item-999.txt", scan::Sort::Name);
    assert_eq!(files.len(), 1);
    assert_eq!(imported.breadcrumbs(files[0]), [0, dirs[0], files[0]]);
    assert!(imported.nodes[files[0]].path.is_none());
    drop(imported);

    let tight = Limits {
        memory_limit: 32 * MIB as usize,
        ..ample
    };
    progress.store(0, Ordering::Relaxed);
    assert_eq!(
        import::import_file(&path, tight, &cancel, &progress).unwrap_err(),
        ImportError::MemoryLimit
    );
    let read = progress.load(Ordering::Relaxed);
    assert!(read > 0 && read < fs::metadata(path).unwrap().len());
}

#[test]
fn recorded_scan_budget_cannot_raise_the_local_import_limit() {
    let mut builder = Builder::fs("/data");
    for file in 0..2_048 {
        builder.file(0, &format!("file-{file}"), 1);
    }
    let mut result = builder.build();
    result.schema_version = scan::SCHEMA_VERSION;
    let tight = Limits {
        memory_limit: 256 * 1024,
        ..Limits::DESKTOP
    };
    for budget_bytes in [1, u64::MAX] {
        result.memory = Some(memory::Diagnostics {
            budget_bytes,
            ..Default::default()
        });
        let document = result.json().unwrap();
        assert_eq!(
            load_with(&document, tight).unwrap_err(),
            ImportError::MemoryLimit
        );
        assert_eq!(
            load(&document).unwrap().memory.unwrap().budget_bytes,
            budget_bytes
        );
    }
}

#[test]
fn export_then_import_works_after_the_source_folder_is_gone() {
    let source = Folder::new("round-trip-source");
    fs::create_dir(source.0.join("inner")).unwrap();
    fs::write(source.0.join("inner/a.bin"), vec![1; 1234]).unwrap();
    fs::write(source.0.join("b.bin"), vec![2; 77]).unwrap();
    let scanned = scan::scan(
        &source.0,
        Options::default(),
        &AtomicBool::new(false),
        &Mutex::new(Progress::default()),
    );
    let saved = Folder::new("round-trip-saved");
    let file = saved.0.join("result.json");
    scan::write_export(&scanned, &file, true).unwrap();
    drop(source); // The folder no longer exists; the import must not need it.
    let imported = import::import_file(
        &file,
        Limits::DESKTOP,
        &AtomicBool::new(false),
        &AtomicU64::new(0),
    )
    .unwrap();
    assert_eq!(imported.schema_version, scan::SCHEMA_VERSION);
    assert_eq!(imported.nodes[0].logical.known, 1234 + 77);
    assert!(
        imported.nodes.iter().all(|n| n.path.is_none()),
        "imported objects carry no filesystem handle"
    );
    assert_eq!(
        imported.json().unwrap(),
        scanned.json().unwrap(),
        "lossless round trip"
    );
    let comparison = compare::compare(
        Arc::new(imported.clone()),
        Arc::new(imported),
        false,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(comparison.totals(Dimension::Logical).net(), 0);
    assert_eq!(comparison.scope.verdict, compare::ScopeVerdict::Matching);
}

#[test]
fn real_scans_record_identity_and_per_directory_listing_state() {
    let source = Folder::new("identity");
    fs::create_dir(source.0.join("inner")).unwrap();
    let scanned = scan::scan(
        &source.0,
        Options::default(),
        &AtomicBool::new(false),
        &Mutex::new(Progress::default()),
    );
    assert_eq!(scanned.schema_version, scan::SCHEMA_VERSION);
    let identity = scanned.scope_identity.as_ref().unwrap();
    assert_eq!(identity.root, scanned.nodes[0].native_locator);
    assert_eq!(identity.source, scan::SourceKind::Filesystem);
    assert_eq!(identity.platform, std::env::consts::OS);
    assert!(
        scanned
            .nodes
            .iter()
            .all(|n| (n.kind == scan::Kind::Directory) == n.listing_complete.is_some())
    );
    // A cancelled scan leaves the unvisited directory's listing incomplete.
    let cancelled = scan::scan(
        &source.0,
        Options::default(),
        &AtomicBool::new(true),
        &Mutex::new(Progress::default()),
    );
    assert!(cancelled.cancelled);
    assert_eq!(cancelled.nodes[0].listing_complete, Some(false));
    let restored: ScanResult = serde_json::from_slice(&cancelled.json().unwrap()).unwrap();
    assert_eq!(restored.nodes[0].listing_complete, Some(false));
}

#[test]
fn sequential_opens_deliver_only_the_latest_result() {
    let folder = Folder::new("sequential");
    let write = |name: &str, size: u64| {
        let mut b = Builder::fs("/data");
        b.file(0, "f", size);
        let path = folder.0.join(name);
        fs::write(&path, serde_json::to_vec(&b.build()).unwrap()).unwrap();
        path
    };
    let (first, second) = (write("one.json", 1), write("two.json", 2));
    let stale = job::open(first, Limits::DESKTOP, false);
    let latest = job::open(second, Limits::DESKTOP, false);
    assert!(latest.ticket > stale.ticket);
    // The superseded job is dropped like the UI does; its result has nowhere to go.
    drop(stale);
    let deadline = Instant::now() + Duration::from_secs(10);
    let result = loop {
        if let Some(result) = latest.take() {
            break result.unwrap();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(result.nodes[0].logical.known, 2);
    // A cancelled job reports cancellation instead of a half-read document.
    let doomed = job::open(folder.0.join("two.json"), Limits::DESKTOP, false);
    doomed.cancel();
    let outcome = loop {
        if let Some(outcome) = doomed.take() {
            break outcome;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    };
    assert!(matches!(outcome, Ok(_) | Err(ImportError::Cancelled)));
    assert!(doomed.progress.load(Ordering::Relaxed) <= 1 << 20);
}

#[cfg(unix)]
#[test]
fn a_named_pipe_is_refused_instead_of_blocking_the_import() {
    let folder = Folder::new("pipe");
    let pipe = folder.0.join("result.json");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .unwrap()
            .success()
    );
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let outcome = import::import_file(
            &pipe,
            Limits::DESKTOP,
            &AtomicBool::new(false),
            &AtomicU64::new(0),
        );
        let _ = sender.send(outcome.unwrap_err());
    });
    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
        ImportError::NotAFile
    );
}

#[test]
fn only_regular_files_are_opened() {
    let folder = Folder::new("kinds");
    let cancel = AtomicBool::new(false);
    let progress = AtomicU64::new(0);
    assert_eq!(
        import::import_file(&folder.0, Limits::DESKTOP, &cancel, &progress).unwrap_err(),
        ImportError::NotAFile
    );
    assert!(matches!(
        import::import_file(
            &folder.0.join("missing.json"),
            Limits::DESKTOP,
            &cancel,
            &progress
        )
        .unwrap_err(),
        ImportError::Io(_)
    ));
}
