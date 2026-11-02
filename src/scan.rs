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

/// Version written by this release. Schemas 1 and 2 remain readable; see `import`.
pub const SCHEMA_VERSION: u32 = 3;

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
    MemoryPressure,
    ProcessMemoryLimit,
    CapacityLimit,
}

impl LimitReason {
    pub fn code(self) -> &'static str {
        match self {
            Self::NodeLimit => "node_limit",
            Self::MemoryLimit => "memory_limit",
            Self::MemoryPressure => "memory_pressure",
            Self::ProcessMemoryLimit => "process_memory_limit",
            Self::CapacityLimit => "capacity_limit",
        }
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<crate::memory::Diagnostics>,
    pub errors: u64,
    /// Schema 2 and later; `None` means the scan predates identity recording.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_identity: Option<ScopeIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    pub nodes: Vec<Node>,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub cross_filesystems: bool,
    /// Optional caller/test bound, not an ordinary scan policy.
    pub node_limit: usize,
    /// Optional caller/test ceiling in addition to the adaptive policy.
    pub memory_limit: usize,
    pub memory_mode: crate::memory::BudgetMode,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            cross_filesystems: false,
            node_limit: usize::MAX,
            memory_limit: usize::MAX,
            memory_mode: crate::memory::BudgetMode::Automatic,
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

/// Capacities and owned buffers, with a small allocator allowance per buffer.
/// This is an estimate, not a global allocator or process memory measurement.
fn node_heap(node: &Node) -> usize {
    let strings = node
        .name
        .capacity()
        .saturating_add(node.locator.capacity())
        .saturating_add(node.native_locator.capacity())
        .saturating_add(node.error.as_ref().map_or(0, String::capacity));
    strings
        .saturating_add(node.path.as_ref().map_or(0, PathBuf::capacity))
        .saturating_add(5 * 16)
}
fn result_heap(result: &ScanResult) -> usize {
    let identity = result.scope_identity.as_ref().map_or(0, |s| {
        s.root.capacity().saturating_add(s.platform.capacity())
    });
    std::mem::size_of::<ScanResult>()
        .saturating_add(result.scope.capacity())
        .saturating_add(identity)
        .saturating_add(result.app_version.as_ref().map_or(0, String::capacity))
}
fn bytes<T>(capacity: usize) -> usize {
    capacity.saturating_mul(std::mem::size_of::<T>())
}
fn grown(capacity: usize, len: usize) -> Option<usize> {
    if len < capacity {
        Some(capacity)
    } else {
        capacity.max(4).checked_mul(2)
    }
}
fn estimate(
    nodes: usize,
    heap: usize,
    stack: usize,
    seen: usize,
    seen_heap: usize,
    workspace: usize,
) -> usize {
    bytes::<Node>(nodes)
        .saturating_add(heap)
        .saturating_add(bytes::<usize>(stack))
        // Hash table control bytes, load factor and implementation rounding.
        .saturating_add(
            seen.saturating_mul(2)
                .saturating_mul(std::mem::size_of::<String>() + 1),
        )
        .saturating_add(seen_heap)
        .saturating_add(workspace)
        .saturating_add(64 * 1024)
}
fn stop_listing(node: &mut Node) {
    node.complete = false;
    if node.kind == Kind::Directory {
        node.listing_complete = Some(false);
    }
}
fn stop(result: &mut ScanResult, parent: usize, reason: LimitReason) {
    result.limited = true;
    result.limit_reason = Some(reason);
    result.nodes[parent].complete = false;
    if result.nodes[parent].kind == Kind::Directory {
        result.nodes[parent].listing_complete = Some(false);
    }
}
#[derive(Clone, Copy, Default)]
struct Auxiliary {
    seen: usize,
    heap: usize,
    workspace: usize,
}
/// Predict Vec growth and its old allocation surviving a realloc. Reserve fallibly
/// only after the prediction fits; never create a temporary copy of the tree.
fn admit(
    nodes: &mut Vec<Node>,
    stack: &mut Vec<usize>,
    heap: usize,
    child: &Node,
    aux: Auxiliary,
    next_aux: Auxiliary,
    monitor: &mut crate::memory::Monitor,
) -> Result<(), LimitReason> {
    let node_cap = grown(nodes.capacity(), nodes.len()).ok_or(LimitReason::CapacityLimit)?;
    let directory = child.kind == Kind::Directory && child.error.is_none();
    let stack_cap = if directory {
        grown(stack.capacity(), stack.len()).ok_or(LimitReason::CapacityLimit)?
    } else {
        stack.capacity()
    };
    let retained = estimate(
        node_cap,
        heap.saturating_add(node_heap(child)),
        stack_cap,
        next_aux.seen,
        next_aux.heap,
        next_aux.workspace,
    );
    let transient = retained
        .saturating_add(if node_cap > nodes.capacity() {
            bytes::<Node>(nodes.capacity())
        } else {
            0
        })
        .saturating_add(if stack_cap > stack.capacity() {
            bytes::<usize>(stack.capacity())
        } else {
            0
        });
    let current = estimate(
        nodes.capacity(),
        heap,
        stack.capacity(),
        aux.seen,
        aux.heap,
        aux.workspace,
    );
    if let Some(reason) = monitor.check_growth(current as u64, transient as u64) {
        return Err(reason);
    }
    nodes
        .try_reserve_exact(node_cap - nodes.len())
        .map_err(|_| LimitReason::MemoryLimit)?;
    if directory {
        stack
            .try_reserve_exact(stack_cap - stack.len())
            .map_err(|_| LimitReason::MemoryLimit)?;
    }
    Ok(())
}

pub fn scan(
    path: &Path,
    options: Options,
    cancel: &AtomicBool,
    progress: &Mutex<Progress>,
) -> ScanResult {
    let mut monitor = crate::memory::Monitor::new(options.memory_mode, options.memory_limit, None);
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
        memory: None,
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
    let mut heap = node_heap(&result.nodes[0]).saturating_add(result_heap(&result));
    if let Some(reason) =
        monitor.check(estimate(result.nodes.capacity(), heap, stack.capacity(), 0, 0, 0) as u64)
    {
        stop(&mut result, 0, reason);
        stack.clear();
    }
    if cancel.load(Ordering::Relaxed) {
        result.cancelled = true;
        result.limited = false;
        result.limit_reason = None;
    }
    let mut known_bytes = 0_u64;
    let mut published = Instant::now();
    'walk: while let Some(parent) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            result.cancelled = true;
            break;
        }
        if let Some(reason) =
            monitor.check(estimate(result.nodes.capacity(), heap, stack.capacity(), 0, 0, 0) as u64)
        {
            stop(&mut result, parent, reason);
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
                let before = node_heap(&result.nodes[parent]);
                fail(&mut result.nodes[parent], io_code(&error));
                heap = heap.saturating_add(node_heap(&result.nodes[parent]).saturating_sub(before));
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
            if let Some(reason) =
                monitor.check(
                    estimate(result.nodes.capacity(), heap, stack.capacity(), 0, 0, 0) as u64,
                )
            {
                stop(&mut result, parent, reason);
                break 'walk;
            }
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(error) => {
                    let before = node_heap(&result.nodes[parent]);
                    fail(&mut result.nodes[parent], io_code(&error));
                    heap = heap
                        .saturating_add(node_heap(&result.nodes[parent]).saturating_sub(before));
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
            if let Err(reason) = admit(
                &mut result.nodes,
                &mut stack,
                heap,
                &child,
                Auxiliary::default(),
                Auxiliary::default(),
                &mut monitor,
            ) {
                stop(&mut result, parent, reason);
                break 'walk;
            }
            heap = heap.saturating_add(node_heap(&child));
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
        if cancel.load(Ordering::Relaxed) {
            result.cancelled = true;
            stop_listing(&mut result.nodes[parent]);
            break;
        }
        if let Some(reason) = monitor.check_pressure() {
            stop(&mut result, parent, reason);
            break;
        }
    }
    heap = result.nodes.iter().fold(result_heap(&result), |sum, n| {
        sum.saturating_add(node_heap(n))
    });
    monitor.record(estimate(result.nodes.capacity(), heap, stack.capacity(), 0, 0, 0) as u64);
    result.memory = Some(monitor.diagnostics.clone());
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

/// Avoid allocating a lowercase copy for every comparison in wide directories.
/// ASCII ordering agrees with Unicode lowercase ordering; other names retain
/// the existing Unicode behavior.
pub(crate) fn compare_names(a: &str, b: &str) -> std::cmp::Ordering {
    if a.is_ascii() && b.is_ascii() {
        a.bytes()
            .map(|c| c.to_ascii_lowercase())
            .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
    } else {
        a.to_lowercase().cmp(&b.to_lowercase())
    }
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
                Some(LimitReason::MemoryPressure) => "scan_memory_pressure",
                Some(LimitReason::ProcessMemoryLimit) => "scan_process_memory",
                Some(LimitReason::CapacityLimit) => "scan_capacity_limit",
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
        rows.sort_unstable_by(|a, b| {
            let name = || compare_names(&self.nodes[*a].name, &self.nodes[*b].name).then(a.cmp(b));
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
            "schema_version,extension,file_count,known_logical_bytes,unknown_sizes,overflow,cancelled,limited,limit_reason\r\n",
        );
        for (extension, count, size) in self.extensions() {
            // Quoted fields also prevent spreadsheet formula execution in extension names.
            let safe = if extension.starts_with(['=', '+', '-', '@', '\t', '\r']) {
                format!("'{extension}")
            } else {
                extension
            };
            out.push_str(&format!(
                "2,\"{}\",{count},{},{},{},{},{},{}\r\n",
                safe.replace('"', "\"\""),
                size.known,
                size.unknown,
                size.overflow,
                self.cancelled,
                self.limited,
                self.limit_reason.map_or("", LimitReason::code)
            ));
        }
        if self.nodes.iter().all(|n| n.kind != Kind::File) {
            out.push_str(&format!(
                "2,\"\",0,0,0,false,{},{},{}\r\n",
                self.cancelled,
                self.limited,
                self.limit_reason.map_or("", LimitReason::code)
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
    /// Bounded native/page workspace retained outside the result tree.
    fn memory_reserve(&self) -> usize {
        0
    }
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
    let monitor = crate::memory::Monitor::new(
        options.memory_mode,
        options.memory_limit,
        storage.interrupt(),
    );
    scan_storage_monitored(storage, options, cancel, progress, monitor)
}
fn scan_storage_monitored(
    storage: &mut dyn Storage,
    options: Options,
    cancel: &AtomicBool,
    progress: &Mutex<Progress>,
    mut monitor: crate::memory::Monitor,
) -> ScanResult {
    use std::collections::HashSet;
    let workspace = storage.memory_reserve();
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
        memory: None,
        errors: 0,
        scope_identity: Some(provider_identity(&root.native_locator)),
        app_version: Some(env!("CARGO_PKG_VERSION").into()),
        nodes: vec![root],
    };
    let mut stack = vec![0];
    let mut heap = node_heap(&result.nodes[0]).saturating_add(result_heap(&result));
    let mut seen_heap = result.nodes[0].locator.capacity().saturating_add(16);
    if let Some(reason) = monitor.check(estimate(
        result.nodes.capacity(),
        heap,
        stack.capacity(),
        seen.capacity(),
        seen_heap,
        workspace,
    ) as u64)
    {
        stop(&mut result, 0, reason);
        stack.clear();
    }
    if cancel.load(Ordering::Relaxed) {
        result.cancelled = true;
        result.limited = false;
        result.limit_reason = None;
    }
    let mut known = 0_u64;
    let mut published = Instant::now();
    'walk: while let Some(parent) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            result.cancelled = true;
            break;
        }
        if let Some(reason) = monitor.check(estimate(
            result.nodes.capacity(),
            heap,
            stack.capacity(),
            seen.capacity(),
            seen_heap,
            workspace,
        ) as u64)
        {
            stop(&mut result, parent, reason);
            break;
        }
        if let Err(error) = storage.open_directory(&result.nodes[parent].locator) {
            if !cancel.load(Ordering::Relaxed)
                && let Some(reason) = monitor.check_pressure()
            {
                stop(&mut result, parent, reason);
                break;
            }
            if error == "cancelled" || cancel.load(Ordering::Relaxed) {
                result.cancelled = true;
                break;
            }
            let before = node_heap(&result.nodes[parent]);
            fail(&mut result.nodes[parent], error);
            heap = heap.saturating_add(node_heap(&result.nodes[parent]).saturating_sub(before));
            result.errors += 1;
            storage.close_directory();
            continue;
        }
        loop {
            if cancel.load(Ordering::Relaxed) {
                result.cancelled = true;
                break 'walk;
            }
            if let Some(reason) = monitor.check(estimate(
                result.nodes.capacity(),
                heap,
                stack.capacity(),
                seen.capacity(),
                seen_heap,
                workspace,
            ) as u64)
            {
                stop(&mut result, parent, reason);
                break 'walk;
            }
            let entry = storage.next_entry();
            if cancel.load(Ordering::Relaxed) {
                result.cancelled = true;
                break 'walk;
            }
            if let Some(reason) = monitor.check_pressure() {
                stop(&mut result, parent, reason);
                break 'walk;
            }
            let mut child = match entry {
                Ok(Some(child)) => child,
                Ok(None) => {
                    result.nodes[parent].complete = true;
                    result.nodes[parent].listing_complete = Some(true);
                    break;
                }
                Err(error) => {
                    if !cancel.load(Ordering::Relaxed)
                        && let Some(reason) = monitor.check_pressure()
                    {
                        stop(&mut result, parent, reason);
                        break 'walk;
                    }
                    if error == "cancelled" || cancel.load(Ordering::Relaxed) {
                        result.cancelled = true;
                        break 'walk;
                    }
                    let before = node_heap(&result.nodes[parent]);
                    fail(&mut result.nodes[parent], error);
                    heap = heap
                        .saturating_add(node_heap(&result.nodes[parent]).saturating_sub(before));
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
                if seen.contains(&child.locator) {
                    fail(&mut child, "provider_cycle".into());
                }
            } else if child.kind == Kind::Link {
                child.logical = Measure::value(None);
                child.allocated = Measure::value(None);
            }
            let add_seen = child.kind == Kind::Directory && child.error.is_none();
            let seen_cap = if add_seen && seen.len() == seen.capacity() {
                seen.capacity().max(4).saturating_mul(2)
            } else {
                seen.capacity()
            };
            let next_seen_heap = seen_heap.saturating_add(if add_seen {
                child.locator.capacity().saturating_add(16)
            } else {
                0
            });
            if let Err(reason) = admit(
                &mut result.nodes,
                &mut stack,
                heap,
                &child,
                Auxiliary {
                    seen: seen.capacity(),
                    heap: seen_heap,
                    workspace,
                },
                Auxiliary {
                    seen: seen_cap,
                    heap: next_seen_heap,
                    workspace,
                },
                &mut monitor,
            ) {
                stop(&mut result, parent, reason);
                break 'walk;
            }
            if add_seen {
                if seen.try_reserve(1).is_err() {
                    stop(&mut result, parent, LimitReason::MemoryLimit);
                    break 'walk;
                }
                seen.insert(child.locator.clone());
                seen_heap = next_seen_heap;
            }
            heap = heap.saturating_add(node_heap(&child));
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
    heap = result.nodes.iter().fold(result_heap(&result), |sum, n| {
        sum.saturating_add(node_heap(n))
    });
    monitor.record(estimate(
        result.nodes.capacity(),
        heap,
        stack.capacity(),
        seen.capacity(),
        seen_heap,
        workspace,
    ) as u64);
    result.memory = Some(monitor.diagnostics.clone());
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

#[cfg(test)]
pub(crate) mod memory_tests {
    use super::*;
    use crate::memory::{BudgetMode, Monitor, Snapshot};
    use std::sync::atomic::{AtomicU64, AtomicUsize};

    /// Generates entries lazily: fixtures do not preallocate a second tree.
    pub(crate) struct Generated {
        pub count: usize,
        next: usize,
        pressure: Option<(usize, u64)>,
        pub closed: usize,
        interrupted: Arc<AtomicBool>,
    }
    impl Generated {
        pub fn new(count: usize) -> Self {
            Self {
                count,
                next: 0,
                pressure: None,
                closed: 0,
                interrupted: Arc::new(AtomicBool::new(false)),
            }
        }
    }
    impl Storage for Generated {
        fn root(&self) -> Node {
            let mut n = node(PathBuf::from("/synthetic"), None);
            n.kind = Kind::Directory;
            n.path = None;
            n
        }
        fn open_directory(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn next_entry(&mut self) -> Result<Option<Node>, String> {
            if let Some((index, id)) = self.pressure
                && self.next == index
            {
                crate::memory::pressure(id);
                return Err("cancelled".into()); // Native cursor interruption, not user cancellation.
            }
            if self.next == self.count {
                return Ok(None);
            }
            let mut n = node(
                PathBuf::from(format!("/synthetic/item-{}", self.next)),
                Some(0),
            );
            n.kind = Kind::File;
            n.complete = true;
            n.path = None;
            // A missing size must remain unknown even in a limited result.
            n.logical = Measure::value(if self.next.is_multiple_of(7) {
                None
            } else {
                Some(10)
            });
            n.allocated = Measure::value(None);
            self.next += 1;
            Ok(Some(n))
        }
        fn close_directory(&mut self) {
            self.closed += 1;
        }
        fn interrupt(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
            let interrupted = self.interrupted.clone();
            Some(Arc::new(move || {
                interrupted.store(true, Ordering::Relaxed);
            }))
        }
    }
    fn run(storage: &mut Generated, monitor: Monitor, cancel: bool) -> ScanResult {
        scan_storage_monitored(
            storage,
            Options::default(),
            &AtomicBool::new(cancel),
            &Mutex::new(Progress::default()),
            monitor,
        )
    }
    #[test]
    fn beyond_both_previous_object_caps_with_sufficient_budget() {
        let mut storage = Generated::new(500_010);
        let result = run(
            &mut storage,
            Monitor::simulated(BudgetMode::Manual { mib: 1024 }, Snapshot::default(), None),
            false,
        );
        assert_eq!(result.nodes.len(), 500_011);
        assert!(result.nodes[0].complete);
        assert!(!result.limited);
        assert!(storage.closed > 0);
        assert!(result.memory.unwrap().estimated_scanner_bytes < 1024 * 1024 * 1024);
    }
    #[test]
    fn estimated_budget_stops_before_growth_and_keeps_aggregates() {
        let mut storage = Generated::new(100_000);
        let result = run(
            &mut storage,
            Monitor::simulated(BudgetMode::Manual { mib: 16 }, Snapshot::default(), None),
            false,
        );
        assert_eq!(result.limit_reason, Some(LimitReason::MemoryLimit));
        assert!(result.nodes.len() > 1 && result.nodes.len() < 100_000);
        assert!(!result.nodes[0].complete);
        assert_eq!(result.nodes[0].listing_complete, Some(false));
        assert_eq!(
            result.nodes[0].logical.known,
            result.nodes[1..]
                .iter()
                .map(|n| n.logical.known)
                .sum::<u64>()
        );
        assert!(result.nodes[0].logical.unknown > 0);
        assert!(storage.closed > 0);
        assert!(result.memory.as_ref().unwrap().estimated_scanner_bytes <= 16 * 1024 * 1024);
        assert_eq!(
            serde_json::from_slice::<ScanResult>(&result.json().unwrap())
                .unwrap()
                .limit_reason,
            result.limit_reason
        );
    }
    #[test]
    fn critical_callback_interrupts_cursor_and_is_distinct_from_cancel() {
        let mut storage = Generated::new(100);
        let calls = Arc::new(AtomicUsize::new(0));
        let observer = calls.clone();
        let monitor = Monitor::simulated(
            BudgetMode::Manual { mib: 4096 },
            Snapshot::default(),
            Some(Arc::new(move || {
                observer.fetch_add(1, Ordering::Relaxed);
            })),
        );
        storage.pressure = Some((3, monitor.id()));
        let result = run(&mut storage, monitor, false);
        assert_eq!(result.limit_reason, Some(LimitReason::MemoryPressure));
        assert!(!result.cancelled);
        assert_eq!(result.nodes.len(), 4);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(storage.closed > 0);
        assert!(!result.nodes[0].complete);
        assert_eq!(result.nodes[0].logical.known, 20);
        let cancelled = run(
            &mut storage,
            Monitor::simulated(
                BudgetMode::Automatic,
                Snapshot {
                    critical: true,
                    ..Default::default()
                },
                None,
            ),
            true,
        );
        assert!(cancelled.cancelled);
        assert_eq!(cancelled.limit_reason, None);
    }
    #[test]
    fn initial_pressure_and_arithmetic_boundaries() {
        for a in ["A", "a", "file-123", "Ä", "日本語", "İ"] {
            for b in ["a", "B", "file-19", "ä", "日本語", "i"] {
                assert_eq!(compare_names(a, b), a.to_lowercase().cmp(&b.to_lowercase()));
            }
        }
        assert_eq!(grown(usize::MAX, usize::MAX), None);
        assert_eq!(bytes::<Node>(usize::MAX), usize::MAX);
        assert_eq!(
            estimate(
                usize::MAX,
                usize::MAX,
                usize::MAX,
                usize::MAX,
                usize::MAX,
                usize::MAX
            ),
            usize::MAX
        );
        let mut storage = Generated::new(100);
        let result = run(
            &mut storage,
            Monitor::simulated(
                BudgetMode::Automatic,
                Snapshot {
                    critical: true,
                    ..Default::default()
                },
                None,
            ),
            false,
        );
        assert_eq!(result.nodes.len(), 1);
        assert!(result.limited && !result.nodes[0].complete);
        assert!(storage.closed > 0);
        for reason in [
            LimitReason::MemoryPressure,
            LimitReason::ProcessMemoryLimit,
            LimitReason::CapacityLimit,
        ] {
            let mut exported = result.clone();
            exported.limit_reason = Some(reason);
            let json = exported.json().unwrap();
            assert_eq!(
                serde_json::from_slice::<ScanResult>(&json)
                    .unwrap()
                    .limit_reason,
                Some(reason)
            );
            assert!(exported.csv().contains(reason.code()));
            let imported = crate::import::import_reader(
                std::io::Cursor::new(json),
                crate::import::Limits::DESKTOP,
                &AtomicBool::new(false),
                &AtomicU64::new(0),
            );
            assert!(imported.is_ok(), "{imported:?}");
        }
    }
    #[test]
    fn pressure_unblocks_native_io_and_old_callback_cannot_stop_next_scan() {
        use std::sync::{Condvar, mpsc};
        struct Blocking {
            state: Arc<(Mutex<bool>, Condvar)>,
            entered: mpsc::SyncSender<()>,
            closed: Arc<AtomicBool>,
        }
        impl Storage for Blocking {
            fn root(&self) -> Node {
                Generated::new(0).root()
            }
            fn open_directory(&mut self, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn next_entry(&mut self) -> Result<Option<Node>, String> {
                self.entered.send(()).unwrap();
                let (lock, wake) = &*self.state;
                let mut stopped = lock.lock().unwrap();
                while !*stopped {
                    stopped = wake.wait(stopped).unwrap();
                }
                Err("cancelled".into())
            }
            fn close_directory(&mut self) {
                self.closed.store(true, Ordering::Relaxed);
            }
            fn interrupt(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
                let state = self.state.clone();
                Some(Arc::new(move || {
                    let (lock, wake) = &*state;
                    *lock.lock().unwrap() = true;
                    wake.notify_all();
                }))
            }
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut storage = Blocking {
            state: Arc::new((Mutex::new(false), Condvar::new())),
            entered: sender,
            closed: Arc::new(AtomicBool::new(false)),
        };
        let closed = storage.closed.clone();
        let previous = Monitor::simulated(BudgetMode::Automatic, Snapshot::default(), None);
        let old_id = previous.id();
        drop(previous);
        let monitor = Monitor::simulated(
            BudgetMode::Manual { mib: 4096 },
            Snapshot::default(),
            storage.interrupt(),
        );
        let id = monitor.id();
        let worker = std::thread::spawn(move || {
            scan_storage_monitored(
                &mut storage,
                Options::default(),
                &AtomicBool::new(false),
                &Mutex::new(Progress::default()),
                monitor,
            )
        });
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        crate::memory::pressure(old_id);
        assert!(!closed.load(Ordering::Relaxed));
        crate::memory::pressure(id);
        let result = worker.join().unwrap();
        assert_eq!(result.limit_reason, Some(LimitReason::MemoryPressure));
        assert!(!result.cancelled);
        assert!(closed.load(Ordering::Relaxed));
    }
}
