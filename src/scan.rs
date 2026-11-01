//! Read-only metadata scanning. A single worker keeps at most one pending request.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const DEFAULT_NODE_LIMIT: usize = 500_000;
/// Version written by this release. Schema 1 files remain readable; see `import`.
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Measure {
    pub known: u64,
    pub unknown: u64,
    pub overflow: bool,
}
impl Measure {
    fn value(value: Option<u64>) -> Self {
        match value {
            Some(known) => Self {
                known,
                ..Self::default()
            },
            None => Self {
                unknown: 1,
                ..Self::default()
            },
        }
    }
    /// Every contributing size was measured and none overflowed.
    pub fn determined(&self) -> bool {
        self.unknown == 0 && !self.overflow
    }
    pub(crate) fn add(&mut self, other: Self) {
        match self.known.checked_add(other.known) {
            Some(n) => self.known = n,
            None => self.overflow = true,
        }
        self.unknown = self.unknown.saturating_add(other.unknown);
        self.overflow |= other.overflow;
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Directory,
    File,
    Link,
    Other,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Node {
    pub parent: Option<usize>,
    pub name: String,
    pub locator: String,
    /// Exact native path representation for names that cannot be represented as UTF-8.
    pub native_locator: String,
    pub kind: Kind,
    pub logical: Measure,
    pub allocated: Measure,
    pub complete: bool,
    pub hard_link: bool,
    pub error: Option<String>,
    /// Schema 2: for a directory, whether its direct children were enumerated to the
    /// end. Unlike `complete` this ignores incomplete descendants. Absent for
    /// non-directories and schema 1 data, where it was never recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing_complete: Option<bool>,
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LimitReason {
    NodeLimit,
    MemoryLimit,
}

/// Kind of storage access that produced a result.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Filesystem,
    AndroidSaf,
    IosFiles,
    Provider,
}

/// Schema 2 facts that identify the scanned area. They support compatibility
/// checks only; none of them proves that two scans share a volume or device.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ScopeIdentity {
    pub source: SourceKind,
    /// `std::env::consts::OS` of the process that scanned.
    pub platform: String,
    /// Exact native locator of the scope root.
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_inode: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_created_unix_nanos: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ScanResult {
    pub schema_version: u32,
    pub started_unix_seconds: u64,
    pub elapsed_millis: u64,
    pub scope: String,
    pub cancelled: bool,
    pub limited: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_reason: Option<LimitReason>,
    pub cross_filesystems: bool,
    pub errors: u64,
    /// Schema 2 only; `None` means the scan predates identity recording.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_identity: Option<ScopeIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    pub nodes: Vec<Node>,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub cross_filesystems: bool,
    pub node_limit: usize,
    pub memory_limit: usize,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            cross_filesystems: false,
            node_limit: DEFAULT_NODE_LIMIT,
            memory_limit: 256 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub objects: usize,
    pub known_bytes: u64,
    pub errors: u64,
}

fn native_locator(path: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut encoded = String::from("unix-bytes:");
        for byte in path.as_os_str().as_bytes() {
            use std::fmt::Write;
            let _ = write!(encoded, "{byte:02x}");
        }
        encoded
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut encoded = String::from("windows-utf16:");
        for unit in path.as_os_str().encode_wide() {
            use std::fmt::Write;
            let _ = write!(encoded, "{unit:04x}");
        }
        encoded
    }
    #[cfg(not(any(unix, windows)))]
    {
        path.to_string_lossy().into_owned()
    }
}

fn node(path: PathBuf, parent: Option<usize>) -> Node {
    Node {
        parent,
        name: path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned(),
        locator: path.to_string_lossy().into_owned(),
        native_locator: native_locator(&path),
        kind: Kind::Other,
        logical: Measure::default(),
        allocated: Measure::default(),
        complete: false,
        hard_link: false,
        error: None,
        listing_complete: None,
        path: Some(path),
    }
}

#[cfg(unix)]
fn device(metadata: &fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.dev())
}
#[cfg(not(unix))]
fn device(_: &fs::Metadata) -> Option<u64> {
    None
}
fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
fn allocated(metadata: &fs::Metadata) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.blocks().checked_mul(512)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}
fn hard_link(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.nlink() > 1
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}
fn classify(node: &mut Node, metadata: &fs::Metadata) {
    if is_link(metadata) {
        node.kind = Kind::Link;
        node.complete = true;
        node.logical = Measure::value(None);
        node.allocated = Measure::value(None);
    } else if metadata.is_dir() {
        node.kind = Kind::Directory;
        node.listing_complete = Some(false);
    } else if metadata.is_file() {
        node.kind = Kind::File;
        node.logical = Measure::value(Some(metadata.len()));
        node.allocated = Measure::value(allocated(metadata));
        node.hard_link = hard_link(metadata);
        node.complete = true;
    } else {
        node.kind = Kind::Other;
        node.complete = true;
    }
}
fn fail(node: &mut Node, error: String) {
    node.error = Some(error);
    node.complete = false;
    if node.listing_complete.is_some() {
        node.listing_complete = Some(false);
    }
    node.logical.unknown = node.logical.unknown.saturating_add(1);
    node.allocated.unknown = node.allocated.unknown.saturating_add(1);
}
fn io_code(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => "permission_denied",
        std::io::ErrorKind::NotFound => "disappeared",
        _ => "metadata_unavailable",
    }
    .into()
}

fn filesystem_identity(root: &str, metadata: Option<&fs::Metadata>) -> ScopeIdentity {
    #[cfg(unix)]
    let inode = metadata.map(|m| {
        use std::os::unix::fs::MetadataExt;
        m.ino()
    });
    #[cfg(not(unix))]
    let inode = None;
    let created = metadata
        .and_then(|m| m.created().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| u64::try_from(d.as_nanos()).ok());
    ScopeIdentity {
        source: SourceKind::Filesystem,
        platform: std::env::consts::OS.into(),
        root: root.into(),
        root_inode: inode,
        root_created_unix_nanos: created,
    }
}

fn provider_identity(root: &str) -> ScopeIdentity {
    ScopeIdentity {
        source: if root.starts_with("content://") {
            SourceKind::AndroidSaf
        } else if root.starts_with("file://") {
            SourceKind::IosFiles
        } else {
            SourceKind::Provider
        },
        platform: std::env::consts::OS.into(),
        root: root.into(),
        root_inode: None,
        root_created_unix_nanos: None,
    }
}

pub fn scan(
    path: &Path,
    options: Options,
    cancel: &AtomicBool,
    progress: &Mutex<Progress>,
) -> ScanResult {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let path = absolute.as_path();
    let started = Instant::now();
    let mut result = ScanResult {
        schema_version: SCHEMA_VERSION,
        started_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        elapsed_millis: 0,
        scope: path.to_string_lossy().into_owned(),
        cancelled: false,
        limited: false,
        limit_reason: None,
        cross_filesystems: options.cross_filesystems,
        errors: 0,
        scope_identity: None,
        app_version: Some(env!("CARGO_PKG_VERSION").into()),
        nodes: vec![node(path.to_owned(), None)],
    };
    let root_metadata = fs::symlink_metadata(path);
    let root_device = root_metadata.as_ref().ok().and_then(device);
    result.scope_identity = Some(filesystem_identity(
        &result.nodes[0].native_locator,
        root_metadata.as_ref().ok(),
    ));
    let mut stack = Vec::new();
    match root_metadata {
        Ok(metadata) => {
            classify(&mut result.nodes[0], &metadata);
            if result.nodes[0].kind == Kind::Directory {
                stack.push(0);
            }
        }
        Err(error) => {
            fail(&mut result.nodes[0], io_code(&error));
            result.errors += 1;
        }
    }
    let mut memory_used = std::mem::size_of::<Node>()
        + result.nodes[0].native_locator.len()
        + result.nodes[0].locator.len() * 2;
    let mut known_bytes = 0_u64;
    let mut published = Instant::now();
    'walk: while let Some(parent) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            result.cancelled = true;
            break;
        }
        let parent_path = result.nodes[parent]
            .path
            .as_ref()
            .expect("local path")
            .clone();
        let entries = match fs::read_dir(parent_path) {
            Ok(entries) => entries,
            Err(error) => {
                fail(&mut result.nodes[parent], io_code(&error));
                result.errors += 1;
                continue;
            }
        };
        result.nodes[parent].complete = true;
        result.nodes[parent].listing_complete = Some(true);
        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                result.cancelled = true;
                result.nodes[parent].complete = false;
                result.nodes[parent].listing_complete = Some(false);
                break 'walk;
            }
            if result.nodes.len() >= options.node_limit.max(1) {
                result.limited = true;
                result.limit_reason = Some(LimitReason::NodeLimit);
                result.nodes[parent].complete = false;
                result.nodes[parent].listing_complete = Some(false);
                break 'walk;
            }
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(error) => {
                    fail(&mut result.nodes[parent], io_code(&error));
                    result.errors += 1;
                    continue;
                }
            };
            let mut child = node(path, Some(parent));
            match fs::symlink_metadata(child.path.as_ref().expect("local path")) {
                Ok(metadata) => {
                    classify(&mut child, &metadata);
                    if child.kind == Kind::Directory
                        && !options.cross_filesystems
                        && device(&metadata) != root_device
                    {
                        fail(&mut child, "mount_boundary".into());
                    }
                }
                Err(error) => {
                    fail(&mut child, io_code(&error));
                    result.errors += 1;
                }
            }
            memory_used = memory_used.saturating_add(
                std::mem::size_of::<Node>()
                    + child.native_locator.len()
                    + child.locator.len() * 2
                    + child.name.len(),
            );
            if memory_used > options.memory_limit {
                result.limited = true;
                result.limit_reason = Some(LimitReason::MemoryLimit);
                result.nodes[parent].complete = false;
                result.nodes[parent].listing_complete = Some(false);
                break 'walk;
            }
            known_bytes = known_bytes.saturating_add(child.logical.known);
            let index = result.nodes.len();
            if child.kind == Kind::Directory && child.error.is_none() {
                stack.push(index);
            }
            result.nodes.push(child);
            if published.elapsed() >= Duration::from_millis(100) {
                *progress.lock().unwrap_or_else(|p| p.into_inner()) = Progress {
                    objects: result.nodes.len(),
                    known_bytes,
                    errors: result.errors,
                };
                published = Instant::now();
            }
        }
    }
    finish(&mut result, started, progress);
    result
}

/// Aggregate directory measures from leaf measurements. Parent IDs precede
/// descendants, so a reverse pass needs no recursion.
pub fn aggregate(nodes: &mut [Node]) {
    for index in (0..nodes.len()).rev() {
        if nodes[index].kind == Kind::Directory
            && !nodes[index].complete
            && nodes[index].logical.unknown == 0
        {
            nodes[index].logical.unknown += 1;
            nodes[index].allocated.unknown += 1;
        }
        if let Some(parent) = nodes[index].parent {
            let (logical, allocated, complete) = (
                nodes[index].logical,
                nodes[index].allocated,
                nodes[index].complete,
            );
            nodes[parent].logical.add(logical);
            nodes[parent].allocated.add(allocated);
            nodes[parent].complete &= complete;
        }
    }
}

fn finish(result: &mut ScanResult, started: Instant, progress: &Mutex<Progress>) {
    aggregate(&mut result.nodes);
    result.elapsed_millis = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    *progress.lock().unwrap_or_else(|p| p.into_inner()) = Progress {
        objects: result.nodes.len(),
        known_bytes: result.nodes[0].logical.known,
        errors: result.errors,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Size,
    Name,
}
impl ScanResult {
    /// Translation key for a partial traversal; absent for a completed traversal.
    pub fn incomplete_reason(&self) -> Option<&'static str> {
        if self.cancelled {
            Some("scan_cancelled")
        } else if self.limited {
            Some(match self.limit_reason {
                Some(LimitReason::NodeLimit) => "scan_node_limit",
                Some(LimitReason::MemoryLimit) => "scan_memory_limit",
                None => "scan_limited",
            })
        } else if self.nodes.first().is_some_and(|node| !node.complete) {
            Some("scan_incomplete")
        } else {
            None
        }
    }

    pub fn children(&self, parent: usize, query: &str, sort: Sort) -> Vec<usize> {
        let query = query.to_lowercase();
        let mut rows: Vec<_> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                (n.parent == Some(parent)
                    && (query.is_empty() || n.name.to_lowercase().contains(&query)))
                .then_some(i)
            })
            .collect();
        self.sort(&mut rows, sort);
        rows
    }
    pub fn search(&self, query: &str, files_only: bool, sort: Sort) -> Vec<usize> {
        let query = query.to_lowercase();
        let mut rows: Vec<_> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                (i != 0
                    && (!files_only || n.kind == Kind::File)
                    && (query.is_empty()
                        || n.name.to_lowercase().contains(&query)
                        || n.locator.to_lowercase().contains(&query)))
                .then_some(i)
            })
            .collect();
        self.sort(&mut rows, sort);
        rows
    }
    pub(crate) fn sort(&self, rows: &mut [usize], sort: Sort) {
        rows.sort_by(|a, b| {
            let name = || {
                self.nodes[*a]
                    .name
                    .to_lowercase()
                    .cmp(&self.nodes[*b].name.to_lowercase())
                    .then(a.cmp(b))
            };
            match sort {
                Sort::Name => name(),
                Sort::Size => self.nodes[*b]
                    .logical
                    .known
                    .cmp(&self.nodes[*a].logical.known)
                    .then_with(name),
            }
        });
    }
    pub fn extensions(&self) -> Vec<(String, u64, Measure)> {
        let mut groups = BTreeMap::<String, (u64, Measure)>::new();
        for file in self.nodes.iter().filter(|n| n.kind == Kind::File) {
            let extension = Path::new(&file.name)
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let group = groups.entry(extension).or_default();
            group.0 += 1;
            group.1.add(file.logical);
        }
        let mut groups: Vec<_> = groups
            .into_iter()
            .map(|(name, (count, size))| (name, count, size))
            .collect();
        groups.sort_by(|a, b| b.2.known.cmp(&a.2.known).then(a.0.cmp(&b.0)));
        groups
    }
    pub fn breadcrumbs(&self, mut index: usize) -> Vec<usize> {
        let mut path = vec![index];
        while let Some(parent) = self.nodes[index].parent {
            path.push(parent);
            index = parent;
        }
        path.reverse();
        path
    }
    pub fn json(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec_pretty(self)
    }
    pub fn csv(&self) -> String {
        let mut out = String::from(
            "schema_version,extension,file_count,known_logical_bytes,unknown_sizes,overflow\r\n",
        );
        for (extension, count, size) in self.extensions() {
            // Quoted fields also prevent spreadsheet formula execution in extension names.
            let safe = if extension.starts_with(['=', '+', '-', '@', '\t', '\r']) {
                format!("'{extension}")
            } else {
                extension
            };
            out.push_str(&format!(
                "1,\"{}\",{count},{},{},{}\r\n",
                safe.replace('"', "\"\""),
                size.known,
                size.unknown,
                size.overflow
            ));
        }
        out
    }
}

/// A metadata-only directory cursor. Implementations keep at most one native
/// cursor open; traversal, limits, aggregation, and cancellation remain in Rust.
pub trait Storage: Send {
    fn root(&self) -> Node;
    fn open_directory(&mut self, locator: &str) -> Result<(), String>;
    fn next_entry(&mut self) -> Result<Option<Node>, String>;
    fn close_directory(&mut self);
    fn interrupt(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        None
    }
}

pub fn scan_storage(
    storage: &mut dyn Storage,
    options: Options,
    cancel: &AtomicBool,
    progress: &Mutex<Progress>,
) -> ScanResult {
    use std::collections::HashSet;
    let started = Instant::now();
    let mut root = storage.root();
    root.parent = None;
    root.path = None;
    root.listing_complete = (root.kind == Kind::Directory).then_some(false);
    let mut seen = HashSet::from([root.locator.clone()]);
    let mut result = ScanResult {
        schema_version: SCHEMA_VERSION,
        started_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        elapsed_millis: 0,
        scope: root.locator.clone(),
        cancelled: false,
        limited: false,
        limit_reason: None,
        cross_filesystems: false,
        errors: 0,
        scope_identity: Some(provider_identity(&root.native_locator)),
        app_version: Some(env!("CARGO_PKG_VERSION").into()),
        nodes: vec![root],
    };
    let mut stack = vec![0];
    let mut memory = std::mem::size_of::<Node>() + result.scope.len() * 3;
    if memory > options.memory_limit {
        result.limited = true;
        result.limit_reason = Some(LimitReason::MemoryLimit);
        stack.clear();
    }
    let mut known = 0_u64;
    let mut published = Instant::now();
    'walk: while let Some(parent) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            result.cancelled = true;
            break;
        }
        if let Err(error) = storage.open_directory(&result.nodes[parent].locator) {
            if error == "cancelled" || cancel.load(Ordering::Relaxed) {
                result.cancelled = true;
                break;
            }
            fail(&mut result.nodes[parent], error);
            result.errors += 1;
            storage.close_directory();
            continue;
        }
        loop {
            if cancel.load(Ordering::Relaxed) {
                result.cancelled = true;
                break 'walk;
            }
            let mut child = match storage.next_entry() {
                Ok(Some(child)) => child,
                Ok(None) => {
                    result.nodes[parent].complete = true;
                    result.nodes[parent].listing_complete = Some(true);
                    break;
                }
                Err(error) => {
                    if error == "cancelled" || cancel.load(Ordering::Relaxed) {
                        result.cancelled = true;
                        break 'walk;
                    }
                    fail(&mut result.nodes[parent], error);
                    result.errors += 1;
                    break;
                }
            };
            if result.nodes.len() >= options.node_limit.max(1) {
                result.limited = true;
                result.limit_reason = Some(LimitReason::NodeLimit);
                break 'walk;
            }
            child.parent = Some(parent);
            child.path = None;
            child.listing_complete = (child.kind == Kind::Directory).then_some(false);
            if child.kind == Kind::Directory {
                child.logical = Measure::default();
                child.allocated = Measure::default();
                child.complete = false;
                if !seen.insert(child.locator.clone()) {
                    fail(&mut child, "provider_cycle".into());
                }
            } else if child.kind == Kind::Link {
                child.logical = Measure::value(None);
                child.allocated = Measure::value(None);
            }
            memory = memory.saturating_add(
                std::mem::size_of::<Node>()
                    + child.name.len()
                    + child.locator.len() * 2
                    + child.native_locator.len(),
            );
            if memory > options.memory_limit {
                result.limited = true;
                result.limit_reason = Some(LimitReason::MemoryLimit);
                break 'walk;
            }
            if child.error.is_some() {
                child.complete = false;
                result.errors += 1;
            }
            known = known.saturating_add(child.logical.known);
            let index = result.nodes.len();
            if child.kind == Kind::Directory && child.error.is_none() {
                stack.push(index);
            }
            result.nodes.push(child);
            if published.elapsed() >= Duration::from_millis(100) {
                *progress.lock().unwrap_or_else(|p| p.into_inner()) = Progress {
                    objects: result.nodes.len(),
                    known_bytes: known,
                    errors: result.errors,
                };
                published = Instant::now();
            }
        }
        storage.close_directory();
    }
    storage.close_directory();
    finish(&mut result, started, progress);
    result
}

struct Cancellation {
    flag: AtomicBool,
    interrupt: Option<Arc<dyn Fn() + Send + Sync>>,
}
impl Cancellation {
    fn cancel(&self) {
        if !self.flag.swap(true, Ordering::Relaxed)
            && let Some(interrupt) = &self.interrupt
        {
            interrupt();
        }
    }
}

enum Source {
    Local(PathBuf),
    Provider(Box<dyn Storage>),
}

pub struct Job {
    pub progress: Arc<Mutex<Progress>>,
    cancel: Arc<Cancellation>,
    receiver: Receiver<ScanResult>,
}
impl Job {
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
    pub fn take(&self) -> Option<ScanResult> {
        self.receiver.try_recv().ok()
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}
struct Task {
    source: Source,
    options: Options,
    cancel: Arc<Cancellation>,
    progress: Arc<Mutex<Progress>>,
    sender: SyncSender<ScanResult>,
}
#[derive(Default)]
struct Queue {
    pending: Option<Task>,
    active: Option<Arc<Cancellation>>,
    stopped: bool,
}
pub struct Service {
    queue: Arc<(Mutex<Queue>, Condvar)>,
}
impl Default for Service {
    fn default() -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let thread_queue = queue.clone();
        std::thread::Builder::new()
            .name("storage-scan".into())
            .spawn(move || {
                loop {
                    let mut task = {
                        let (lock, wake) = &*thread_queue;
                        let mut state = lock.lock().unwrap_or_else(|p| p.into_inner());
                        while state.pending.is_none() && !state.stopped {
                            state = wake.wait(state).unwrap_or_else(|p| p.into_inner());
                        }
                        if state.stopped {
                            break;
                        }
                        let task = state.pending.take().expect("pending task");
                        state.active = Some(task.cancel.clone());
                        task
                    };
                    let result = match &mut task.source {
                        Source::Local(path) => {
                            scan(path, task.options, &task.cancel.flag, &task.progress)
                        }
                        Source::Provider(storage) => scan_storage(
                            storage.as_mut(),
                            task.options,
                            &task.cancel.flag,
                            &task.progress,
                        ),
                    };
                    let _ = task.sender.try_send(result);
                    thread_queue
                        .0
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .active = None;
                }
            })
            .expect("create scanner worker");
        Self { queue }
    }
}
impl Service {
    pub fn start(&self, path: PathBuf, options: Options) -> Job {
        self.enqueue(Source::Local(path), options, None)
    }
    pub fn start_storage(&self, storage: Box<dyn Storage>, options: Options) -> Job {
        let interrupt = storage.interrupt();
        self.enqueue(Source::Provider(storage), options, interrupt)
    }
    fn enqueue(
        &self,
        source: Source,
        options: Options,
        interrupt: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Job {
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancel = Arc::new(Cancellation {
            flag: AtomicBool::new(false),
            interrupt,
        });
        let progress = Arc::new(Mutex::new(Progress::default()));
        let mut state = self.queue.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(active) = &state.active {
            active.cancel();
        }
        if let Some(pending) = &state.pending {
            pending.cancel.cancel();
        }
        state.pending = Some(Task {
            source,
            options,
            cancel: cancel.clone(),
            progress: progress.clone(),
            sender,
        });
        self.queue.1.notify_one();
        Job {
            progress,
            cancel,
            receiver,
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        let mut state = self.queue.0.lock().unwrap_or_else(|p| p.into_inner());
        state.stopped = true;
        if let Some(active) = &state.active {
            active.cancel();
        }
        self.queue.1.notify_one();
    }
}

/// Replace an export only after the complete new file has been written successfully.
pub fn write_export(result: &ScanResult, destination: &Path, json: bool) -> std::io::Result<()> {
    use std::io::Write;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let serial = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(".spacemosaic-{}-{serial}.tmp", std::process::id()));
    let outcome = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let mut writer = std::io::BufWriter::new(&mut file);
        if json {
            serde_json::to_writer_pretty(&mut writer, result).map_err(std::io::Error::other)?;
        } else {
            writer.write_all(result.csv().as_bytes())?;
        }
        writer.flush()?;
        drop(writer);
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, destination)
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(temporary);
    }
    outcome
}
