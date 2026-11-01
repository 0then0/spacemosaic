//! Measures export, bounded import and comparison on a large synthetic pair of results.
//! Both trees are built in memory with native locators; no filesystem is scanned.
use spacemosaic::{
    compare::{self, Dimension},
    import::{self, Limits},
    scan::{self, Kind, Measure, Node, ScanResult, ScopeIdentity, SourceKind},
};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64},
    },
    time::Instant,
};

struct Rng(u64);
impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

fn hex(path: &str) -> String {
    path.bytes().map(|b| format!("{b:02x}")).collect()
}
fn node(parent: Option<usize>, path: &str, kind: Kind, size: u64) -> Node {
    let measure = if kind == Kind::Directory {
        Measure::default()
    } else {
        Measure {
            known: size,
            ..Default::default()
        }
    };
    Node {
        parent,
        name: path.rsplit('/').next().unwrap_or(path).to_owned(),
        locator: path.to_owned(),
        native_locator: format!("unix-bytes:{}", hex(path)),
        kind,
        logical: measure,
        allocated: measure,
        complete: true,
        hard_link: false,
        error: None,
        listing_complete: (kind == Kind::Directory).then_some(true),
        path: None,
    }
}

fn result(nodes: Vec<Node>) -> ScanResult {
    ScanResult {
        schema_version: scan::SCHEMA_VERSION,
        started_unix_seconds: 1_790_000_000,
        elapsed_millis: 1,
        scope: "/data".into(),
        cancelled: false,
        limited: false,
        limit_reason: None,
        cross_filesystems: false,
        errors: 0,
        scope_identity: Some(ScopeIdentity {
            source: SourceKind::Filesystem,
            platform: std::env::consts::OS.into(),
            root: nodes[0].native_locator.clone(),
            root_inode: Some(2),
            root_created_unix_nanos: Some(1),
        }),
        app_version: Some("benchmark".into()),
        nodes,
    }
}

/// `count` objects below `/data`: folders hold up to 40 children, one in six is a folder.
fn baseline(count: usize) -> ScanResult {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut nodes = vec![node(None, "/data", Kind::Directory, 0)];
    let mut open: Vec<usize> = vec![0];
    let mut children = vec![0_usize];
    let mut depth = vec![0_u8];
    for serial in 0..count.saturating_sub(1) {
        let slot = rng.below(open.len() as u64) as usize;
        let parent = open[slot];
        let is_dir = serial % 6 == 0;
        let path = format!(
            "{}/{}{serial}",
            nodes[parent].locator,
            if is_dir { "dir" } else { "file" }
        );
        let kind = if is_dir { Kind::Directory } else { Kind::File };
        nodes.push(node(Some(parent), &path, kind, rng.below(1 << 24)));
        children.push(0);
        depth.push(depth[parent] + 1);
        children[parent] += 1;
        let index = nodes.len() - 1;
        if is_dir && depth[index] < 9 {
            open.push(index);
        }
        if children[parent] >= 40 && open.len() > 1 {
            open.swap_remove(slot);
        }
    }
    scan::aggregate(&mut nodes);
    result(nodes)
}

/// A later observation of the same tree: some objects removed, resized, added,
/// replaced by another type, or unreadable.
fn later(old: &ScanResult) -> ScanResult {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let mut map: Vec<Option<usize>> = Vec::with_capacity(old.nodes.len());
    let mut nodes: Vec<Node> = Vec::with_capacity(old.nodes.len());
    for (index, source) in old.nodes.iter().enumerate() {
        let parent = source.parent.map(|p| map[p]);
        if parent == Some(None) || (index > 0 && rng.below(100) < 2) {
            map.push(None);
            continue;
        }
        let mut copy = source.clone();
        copy.parent = parent.flatten();
        if copy.kind == Kind::Directory {
            copy.logical = Measure::default();
            copy.allocated = Measure::default();
            copy.complete = true;
            copy.listing_complete = Some(true);
        } else if rng.below(100) < 8 {
            let grown = copy.logical.known + rng.below(1 << 20);
            copy.logical.known = grown;
            copy.allocated.known = grown;
        } else if rng.below(1000) < 3 {
            // File replaced by a folder of the same name.
            copy.kind = Kind::Directory;
            copy.logical = Measure::default();
            copy.allocated = Measure::default();
            copy.listing_complete = Some(true);
        }
        map.push(Some(nodes.len()));
        nodes.push(copy);
    }
    let directories: Vec<usize> = (0..nodes.len())
        .filter(|i| nodes[*i].kind == Kind::Directory)
        .collect();
    for index in directories.iter().copied().skip(1).step_by(97) {
        nodes[index].complete = false;
        nodes[index].listing_complete = Some(false);
        nodes[index].error = Some("permission_denied".into());
        nodes[index].logical.unknown += 1;
        nodes[index].allocated.unknown += 1;
    }
    let additions = old.nodes.len() * 3 / 100;
    for serial in 0..additions {
        let parent = directories[rng.below(directories.len() as u64) as usize];
        let path = format!("{}/new{serial}", nodes[parent].locator);
        nodes.push(node(Some(parent), &path, Kind::File, rng.below(1 << 24)));
    }
    scan::aggregate(&mut nodes);
    result(nodes)
}

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    if let Some(directory) = std::env::args()
        .nth(2)
        .filter(|_| std::env::args().nth(1).as_deref() == Some("--read"))
    {
        // Import and compare existing files only, so peak memory excludes fixture building.
        let directory = PathBuf::from(directory);
        let (cancel, progress) = (AtomicBool::new(false), AtomicU64::new(0));
        let started = Instant::now();
        let read = |name: &str| {
            import::import_file(&directory.join(name), Limits::DESKTOP, &cancel, &progress)
                .expect("imports")
        };
        let (baseline, current) = (read("baseline.json"), read("current.json"));
        let import_millis = started.elapsed().as_millis();
        let objects = (baseline.nodes.len(), current.nodes.len());
        let started = Instant::now();
        let comparison = compare::compare(Arc::new(baseline), Arc::new(current), false, &cancel)
            .expect("compares");
        println!(
            "{}",
            serde_json::json!({"objects":[objects.0, objects.1], "import_both_millis": import_millis,
                "compare_millis": started.elapsed().as_millis(), "diff_rows": comparison.nodes.len()})
        );
        return Ok(());
    }
    let count = args.next().and_then(|a| a.parse().ok()).unwrap_or(200_000);
    let write: Option<PathBuf> = match (args.next().as_deref(), args.next()) {
        (Some("--write"), Some(dir)) => Some(dir.into()),
        _ => None,
    };
    let directory = write.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("spacemosaic-compare-bench-{}", std::process::id()))
    });
    fs::create_dir_all(&directory)?;
    let started = Instant::now();
    let old = baseline(count);
    let new = later(&old);
    let build_millis = started.elapsed().as_millis();
    let (old_path, new_path) = (
        directory.join("baseline.json"),
        directory.join("current.json"),
    );
    let started = Instant::now();
    scan::write_export(&old, &old_path, true)?;
    scan::write_export(&new, &new_path, true)?;
    let export_millis = started.elapsed().as_millis();
    let (old_bytes, new_bytes) = (
        fs::metadata(&old_path)?.len(),
        fs::metadata(&new_path)?.len(),
    );
    let cancel = AtomicBool::new(false);
    let progress = AtomicU64::new(0);
    let started = Instant::now();
    let baseline = import::import_file(&old_path, Limits::DESKTOP, &cancel, &progress)
        .expect("baseline imports");
    let current = import::import_file(&new_path, Limits::DESKTOP, &cancel, &progress)
        .expect("current imports");
    let import_millis = started.elapsed().as_millis();
    let started = Instant::now();
    let comparison =
        compare::compare(Arc::new(baseline), Arc::new(current), false, &cancel).expect("compares");
    let compare_millis = started.elapsed().as_millis();
    let totals = comparison.totals(Dimension::Logical);
    let started = Instant::now();
    let rows = comparison.events(
        Dimension::Logical,
        compare::Status::CHANGED,
        "",
        compare::ChangeSort::Growth,
    );
    let list_millis = started.elapsed().as_millis();
    println!(
        "{}",
        serde_json::json!({
            "objects_per_result": count, "build_millis": build_millis, "export_millis": export_millis,
            "baseline_json_bytes": old_bytes, "current_json_bytes": new_bytes,
            "import_both_millis": import_millis, "compare_millis": compare_millis,
            "diff_rows": comparison.nodes.len(), "diff_row_bytes": std::mem::size_of::<compare::DiffNode>(),
            "event_list_millis": list_millis, "event_rows": rows.len(),
            "added": totals.added_count, "removed": totals.removed_count, "grew": totals.grew_count,
            "shrank": totals.shrank_count, "same": totals.same_count, "unknown": totals.unknown_count,
            "net_bytes": totals.net().to_string(), "complete_total": totals.is_complete(),
            "platform": std::env::consts::OS, "architecture": std::env::consts::ARCH,
        })
    );
    if write.is_none() {
        let _ = fs::remove_dir_all(&directory);
    }
    Ok(())
}
