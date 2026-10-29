use serde_json::{Value, json};
use spacemosaic::{
    provider::{Provider, Transport},
    scan::{self, Kind, Measure, Node, Options, Progress, Storage},
};
use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex, atomic::AtomicBool, mpsc},
    time::Duration,
};

fn entry(name: &str, kind: &str, bytes: Option<u64>) -> Value {
    json!({"name":name,"locator":format!("content://fixture/document/{name}"),"kind":kind,
        "logical_bytes":bytes,"allocated_bytes":null})
}
fn provider(tree: HashMap<String, Vec<Value>>) -> Provider {
    let current = Mutex::new(Vec::<Value>::new());
    let transport: Transport = Arc::new(move |request| {
        let request: Value = serde_json::from_str(request).unwrap();
        let mut current = current.lock().unwrap();
        match request["op"].as_str().unwrap() {
            "open" => {
                let locator = request["locator"].as_str().unwrap();
                if locator.ends_with("denied") {
                    return Err("access_revoked".into());
                }
                *current = tree.get(locator).cloned().unwrap_or_default();
                Ok(json!({"entries":[],"done":false,"error":null}).to_string())
            }
            "next" => {
                let size = current.len().min(32);
                let entries: Vec<_> = current.drain(..size).collect();
                Ok(json!({"entries":entries,"done":current.is_empty(),"error":null}).to_string())
            }
            "close" | "finish" | "cancel" => {
                Ok(json!({"entries":[],"done":true,"error":null}).to_string())
            }
            op => panic!("unexpected operation, metadata only: {op}"),
        }
    });
    Provider::new("content://fixture/document/root".into(), 1, transport)
}
fn run(storage: &mut dyn Storage, options: Options) -> scan::ScanResult {
    scan::scan_storage(
        storage,
        options,
        &AtomicBool::new(false),
        &Mutex::new(Progress::default()),
    )
}
#[test]
fn recursive_provider_pages_aggregate_unknown_sizes_and_export_opaque_uris() {
    let mut root: Vec<_> = (0..70)
        .map(|i| entry(&format!("item-{i}.dat"), "file", Some(10)))
        .collect();
    root.push(entry("folder", "directory", Some(999_999)));
    let mut provider = provider(HashMap::from([
        ("content://fixture/document/root".into(), root),
        (
            "content://fixture/document/folder".into(),
            vec![
                entry("unknown", "file", None),
                entry("nested.txt", "file", Some(20)),
                entry("link", "link", Some(900)),
            ],
        ),
    ]));
    let result = run(&mut provider, Options::default());
    assert_eq!(result.nodes.len(), 75);
    assert_eq!(result.nodes[0].logical.known, 720);
    assert_eq!(result.nodes[0].logical.unknown, 2);
    assert!(result.nodes[0].complete);
    assert!(result.nodes.iter().all(|n| n.path.is_none()
        && n.native_locator == n.locator
        && n.locator.starts_with("content://")));
    assert_eq!(
        result
            .nodes
            .iter()
            .find(|n| n.name == "link")
            .unwrap()
            .logical
            .known,
        0
    );
    let json: Value = serde_json::from_slice(&result.json().unwrap()).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["nodes"][0]["logical"]["unknown"], 2);
    assert!(result.csv().contains("dat"));
    assert_eq!(result.search("nested", true, scan::Sort::Size).len(), 1);
}
#[test]
fn provider_cycles_and_denied_children_are_partial_without_rewalking() {
    let mut provider = provider(HashMap::from([
        (
            "content://fixture/document/root".into(),
            vec![
                entry("folder", "directory", None),
                entry("denied", "directory", None),
            ],
        ),
        (
            "content://fixture/document/folder".into(),
            vec![entry("root", "directory", None)],
        ),
    ]));
    let result = run(&mut provider, Options::default());
    assert_eq!(result.nodes.len(), 4);
    assert!(!result.nodes[0].complete);
    assert_eq!(result.errors, 2);
    assert!(
        result
            .nodes
            .iter()
            .any(|n| n.error.as_deref() == Some("provider_cycle"))
    );
    assert!(
        result
            .nodes
            .iter()
            .any(|n| n.error.as_deref() == Some("access_revoked"))
    );
}
#[test]
fn provider_resource_limits_and_exact_empty_root_are_explicit() {
    let mut empty = provider(HashMap::new());
    let result = run(
        &mut empty,
        Options {
            node_limit: 1,
            ..Default::default()
        },
    );
    assert!(result.nodes[0].complete && !result.limited);
    let mut full = provider(HashMap::from([(
        "content://fixture/document/root".into(),
        vec![entry("file", "file", Some(10))],
    )]));
    let result = run(
        &mut full,
        Options {
            node_limit: 1,
            ..Default::default()
        },
    );
    assert!(result.limited && !result.nodes[0].complete);
    assert_eq!(result.limit_reason, Some(scan::LimitReason::NodeLimit));
    assert!(result.nodes[0].logical.unknown > 0);
    let result = run(
        &mut full,
        Options {
            memory_limit: 1,
            ..Default::default()
        },
    );
    assert!(result.limited && result.nodes.len() == 1);
    assert_eq!(result.limit_reason, Some(scan::LimitReason::MemoryLimit));
}
#[test]
fn malformed_and_nonprogressing_pages_are_rejected() {
    for response in [
        "{}".into(),
        json!({"entries":[],"done":false,"error":null}).to_string(),
        json!({"entries":vec![entry("a","file",Some(1));33],"done":true,"error":null}).to_string(),
    ] {
        let transport: Transport = Arc::new(move |request| {
            if serde_json::from_str::<Value>(request).unwrap()["op"] == "next" {
                Ok(response.clone())
            } else {
                Ok(json!({"entries":[],"done":false,"error":null}).to_string())
            }
        });
        let result = run(
            &mut Provider::new("content://fixture/root".into(), 1, transport),
            Options::default(),
        );
        assert!(!result.nodes[0].complete);
        assert_eq!(result.errors, 1);
    }
}
#[test]
fn provider_page_errors_keep_observed_entries_and_unknown_remainder() {
    let transport: Transport = Arc::new(|request| {
        let op = serde_json::from_str::<Value>(request).unwrap()["op"]
            .as_str()
            .unwrap()
            .to_owned();
        Ok(if op == "next" {
            json!({"entries":[entry("observed","file",Some(19))],"done":true,"error":"disappeared"})
        } else {
            json!({"entries":[],"done":false,"error":null})
        }
        .to_string())
    });
    let result = run(
        &mut Provider::new("content://fixture/root".into(), 1, transport),
        Options::default(),
    );
    assert_eq!(result.nodes[0].logical.known, 19);
    assert_eq!(result.errors, 1);
    assert!(!result.nodes[0].complete);
    assert!(result.nodes[0].logical.unknown > 0);
}
struct Blocked {
    gate: Arc<(Mutex<bool>, Condvar)>,
    started: mpsc::SyncSender<()>,
}
impl Storage for Blocked {
    fn root(&self) -> Node {
        Node {
            parent: None,
            name: "blocked".into(),
            locator: "content://blocked".into(),
            native_locator: "content://blocked".into(),
            kind: Kind::Directory,
            logical: Measure::default(),
            allocated: Measure::default(),
            complete: false,
            hard_link: false,
            error: None,
            path: None,
        }
    }
    fn open_directory(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn next_entry(&mut self) -> Result<Option<Node>, String> {
        let _ = self.started.try_send(());
        let (lock, wake) = &*self.gate;
        let guard = lock.lock().unwrap();
        let (guard, timeout) = wake
            .wait_timeout_while(guard, Duration::from_secs(3), |cancelled| !*cancelled)
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "native cancellation hook was not invoked"
        );
        assert!(*guard);
        Err("cancelled".into())
    }
    fn close_directory(&mut self) {}
    fn interrupt(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let gate = self.gate.clone();
        Some(Arc::new(move || {
            *gate.0.lock().unwrap() = true;
            gate.1.notify_all();
        }))
    }
}
#[test]
fn replacing_a_blocked_provider_interrupts_it_and_isolates_results() {
    let service = scan::Service::default();
    let (sender, receiver) = mpsc::sync_channel(1);
    let old = service.start_storage(
        Box::new(Blocked {
            gate: Arc::new((Mutex::new(false), Condvar::new())),
            started: sender,
        }),
        Options::default(),
    );
    receiver.recv_timeout(Duration::from_secs(3)).unwrap();
    let new = service.start_storage(Box::new(provider(HashMap::new())), Options::default());
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut old_result = None;
    let mut new_result = None;
    while std::time::Instant::now() < deadline && (old_result.is_none() || new_result.is_none()) {
        old_result = old_result.or_else(|| old.take());
        new_result = new_result.or_else(|| new.take());
        std::thread::yield_now();
    }
    let old = old_result.unwrap();
    let new = new_result.unwrap();
    assert!(old.cancelled && !old.nodes[0].complete);
    assert_eq!(old.errors, 0);
    assert_eq!(new.scope, "content://fixture/document/root");
    assert!(!new.cancelled && new.nodes[0].complete);
}
#[test]
fn cancellation_before_cursor_open_preserves_an_unknown_partial_root() {
    let mut p = provider(HashMap::new());
    let result = scan::scan_storage(
        &mut p,
        Options::default(),
        &AtomicBool::new(true),
        &Mutex::new(Progress::default()),
    );
    assert!(result.cancelled && !result.nodes[0].complete);
    assert_eq!(result.errors, 0);
    assert_eq!(result.nodes[0].logical.unknown, 1);
}

#[test]
fn late_cleanup_from_an_older_scan_cannot_cancel_or_close_the_new_scan() {
    let active = Arc::new(Mutex::new(None::<u64>));
    let captured = active.clone();
    let transport: Transport = Arc::new(move |request| {
        let request: Value = serde_json::from_str(request).unwrap();
        let id = request["scan"]
            .as_u64()
            .expect("every cursor operation has a session ID");
        let mut active = captured.lock().unwrap();
        match request["op"].as_str().unwrap() {
            "open" => *active = Some(id),
            "cancel" | "finish" | "close" if *active == Some(id) => *active = None,
            _ => {}
        }
        Ok(json!({"entries":[],"done":true,"error":null}).to_string())
    });
    let mut old = Provider::new("content://same/root".into(), 17, transport.clone());
    old.open_directory("content://same/root").unwrap();
    let old_id = *active.lock().unwrap();
    let late_cancel = old.interrupt().unwrap();
    let mut current = Provider::new("content://same/root".into(), 17, transport);
    current.open_directory("content://same/root").unwrap();
    let current_id = *active.lock().unwrap();
    assert_ne!(old_id, current_id);
    late_cancel();
    old.close_directory();
    drop(old);
    assert_eq!(*active.lock().unwrap(), current_id);
    current.close_directory();
    assert_eq!(*active.lock().unwrap(), None);
}

#[test]
fn provider_display_name_is_independent_from_an_encoded_root_uri() {
    let uri = "content://provider/tree/primary%3AAudiobooks/document/primary%3AAudiobooks";
    let transport: Transport =
        Arc::new(|_| Ok(json!({"entries":[],"done":true,"error":null}).to_string()));
    let provider = Provider::new(uri.into(), 1, transport.clone())
        .with_display_name(Some("Audiobooks 日本語".into()));
    let root = provider.root();
    assert_eq!(root.name, "Audiobooks 日本語");
    assert_eq!(root.locator, uri);
    assert_eq!(root.native_locator, uri);
    assert!(root.path.is_none());
    let unnamed = Provider::new(uri.into(), 2, transport).with_display_name(Some("  ".into()));
    assert!(!unnamed.root().name.is_empty());
}
