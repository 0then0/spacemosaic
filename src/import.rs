//! Bounded import of exported JSON results. The file is untrusted input: it is
//! parsed as a stream, limited in input bytes and estimated memory, and
//! checked structurally before any view or comparison can use it. Nothing in it
//! is ever used to open, reveal or rescan a location.
use crate::scan::{Kind, LimitReason, Measure, Node, SCHEMA_VERSION, ScanResult, ScopeIdentity};
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::{
    cell::{Cell, RefCell},
    ffi::OsStr,
    fmt, fs,
    io::{self, BufRead, Read},
    mem::size_of,
    path::Path,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

/// Oldest schema this release can open. Schema 1 is read without gaining
/// precision; see `docs/format.md`.
pub const MIN_SCHEMA_VERSION: u32 = 1;
const MAX_NAME_BYTES: usize = 4 * 1024;
const MAX_LOCATOR_BYTES: usize = 32 * 1024;
const MAX_NATIVE_BYTES: usize = 128 * 1024;
const MAX_CODE_BYTES: usize = 64;
const UNIX_PREFIX: &str = "unix-bytes:";
const WINDOWS_PREFIX: &str = "windows-utf16:";
const READ_BUFFER_BYTES: usize = 64 * 1024;
// Match serde_json's limit for fields it parses structurally.
const MAX_JSON_DEPTH: usize = 127;
const CACHED_EXTENSIONS: usize = 16;

/// Independent bounds on input bytes and estimated peak import allocations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_bytes: u64,
    pub memory_limit: usize,
}
impl Limits {
    pub const DESKTOP: Self = Self {
        max_bytes: 1024 * 1024 * 1024,
        memory_limit: 256 * 1024 * 1024,
    };
    pub const MOBILE: Self = Self {
        max_bytes: 128 * 1024 * 1024,
        memory_limit: 64 * 1024 * 1024,
    };
    pub const fn platform() -> Self {
        if cfg!(any(target_os = "android", target_os = "ios")) {
            Self::MOBILE
        } else {
            Self::DESKTOP
        }
    }

    /// Local limits while the session already retains `held` estimated bytes.
    pub(crate) fn for_device_holding(held: usize) -> Self {
        Self {
            memory_limit: crate::memory::operation_budget(held),
            ..Self::platform()
        }
    }
}

/// Estimated memory for the lookup structures built to browse one result:
/// adjacency, visible rows, direct children and grouped map items.
pub(crate) fn index_bytes(nodes: usize) -> usize {
    nodes.saturating_mul(12 * size_of::<usize>())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    Io(io::ErrorKind),
    NotAFile,
    TooLarge {
        limit: u64,
    },
    MemoryLimit,
    UnsupportedVersion(u64),
    Malformed,
    InvalidValue,
    Invalid {
        code: &'static str,
        node: Option<usize>,
    },
    Cancelled,
}
impl ImportError {
    /// Translation key for the headline; `detail` carries the diagnostic text.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Io(_) | Self::NotAFile => "import_error_io",
            Self::TooLarge { .. } => "import_error_too_large",
            Self::MemoryLimit => "import_error_memory",
            Self::UnsupportedVersion(_) => "import_error_version",
            Self::Malformed | Self::InvalidValue => "import_error_malformed",
            Self::Invalid { .. } => "import_error_invalid",
            Self::Cancelled => "import_error_cancelled",
        }
    }
    pub fn detail(&self) -> String {
        match self {
            Self::Io(kind) => format!("io: {kind}"),
            Self::NotAFile => "not a regular file".into(),
            Self::TooLarge { limit } => format!("file larger than {limit} bytes"),
            Self::MemoryLimit => "local import memory budget exceeded".into(),
            Self::UnsupportedVersion(version) => format!("schema_version {version}"),
            Self::Malformed => "not valid JSON".into(),
            Self::InvalidValue => "unexpected field type or unknown value".into(),
            Self::Invalid {
                code,
                node: Some(node),
            } => format!("{code} (object {node})"),
            Self::Invalid { code, node: None } => (*code).into(),
            Self::Cancelled => "cancelled".into(),
        }
    }
}
impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.key(), self.detail())
    }
}
impl std::error::Error for ImportError {}

fn invalid(code: &'static str, node: usize) -> ImportError {
    ImportError::Invalid {
        code,
        node: Some(node),
    }
}

struct Context<'a> {
    cancel: &'a AtomicBool,
    limits: Limits,
    error: RefCell<Option<ImportError>>,
    node_bytes: Cell<usize>,
    result_peak: Cell<usize>,
    metadata_bytes: Cell<usize>,
    string_workspace: Cell<usize>,
}
impl Context<'_> {
    /// Remember the precise error; serde only carries a generic message.
    fn fail<E: de::Error>(&self, error: ImportError) -> E {
        self.error.borrow_mut().get_or_insert(error);
        E::custom("import rejected")
    }

    fn check_memory(&self, parsing: usize, result_peak: usize) -> Result<(), ImportError> {
        let parse_peak = parsing
            .saturating_add(READ_BUFFER_BYTES)
            .saturating_add(self.string_workspace.get());
        let peak = parse_peak
            .max(result_peak)
            .saturating_add(self.metadata_bytes.get());
        if peak > self.limits.memory_limit {
            return Err(ImportError::MemoryLimit);
        }
        Ok(())
    }

    fn keep_metadata<E: de::Error>(&self, bytes: usize) -> Result<(), E> {
        self.metadata_bytes
            .set(self.metadata_bytes.get().saturating_add(bytes));
        self.check_memory(self.node_bytes.get(), self.result_peak.get())
            .map_err(|error| self.fail(error))
    }
}

fn text_bytes(capacity: usize) -> usize {
    if capacity == 0 {
        0
    } else {
        capacity.saturating_add(3 * size_of::<usize>())
    }
}

/// Workspace allocated after parsing. Validation and UI setup run in separate
/// phases. Extension strings can remain live when the directory index is built.
fn opening_workspace(nodes: usize, extensions: usize, extension_bytes: usize) -> usize {
    let validation = nodes.saturating_mul(2 * size_of::<Sum>() + size_of::<bool>());
    let entry = size_of::<(String, u64, Measure)>();
    // BTreeMap nodes, result entries, and temporary storage for stable sorting.
    let group_extensions = extensions.saturating_mul(4 * entry);
    // Adjacency, visible rows and stack, direct children, and grouped map items.
    let build_index = index_bytes(nodes).saturating_add(extensions.saturating_mul(entry));
    validation.max(
        group_extensions
            .max(build_index)
            .saturating_add(extension_bytes),
    )
}

fn valid_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= MAX_CODE_BYTES
        && code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// A path encoding whose bytes can be compared exactly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Encoding {
    Unix,
    Windows,
    Opaque,
}
pub(crate) fn encoding(native: &str) -> Encoding {
    if native.starts_with(UNIX_PREFIX) {
        Encoding::Unix
    } else if native.starts_with(WINDOWS_PREFIX) {
        Encoding::Windows
    } else {
        Encoding::Opaque
    }
}
fn lower_hex(text: &str) -> bool {
    text.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn decode_unix(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks(2)
        .map(|pair| {
            let digit = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
            digit(pair[0]) * 16 + digit(pair[1])
        })
        .collect()
}

fn leaf_measure_ok(measure: Measure) -> bool {
    !measure.overflow && measure.unknown <= 1 && (measure.unknown == 0 || measure.known == 0)
}

/// Checks that need only the node and its already-read ancestors.
fn check_node(index: usize, node: &Node, earlier: &[Node]) -> Result<(), ImportError> {
    match node.parent {
        None if index != 0 => return Err(invalid("root_parent", index)),
        Some(_) if index == 0 => return Err(invalid("root_parent", index)),
        Some(parent) => {
            // Parents precede children, which rules out cycles and forward references.
            let Some(parent) = earlier.get(parent) else {
                return Err(invalid("parent_order", index));
            };
            if parent.kind != Kind::Directory {
                return Err(invalid("parent_kind", index));
            }
            if node.name.is_empty() {
                return Err(invalid("empty_name", index));
            }
        }
        None => {}
    }
    if node.name.len() > MAX_NAME_BYTES
        || node.locator.len() > MAX_LOCATOR_BYTES
        || node.native_locator.len() > MAX_NATIVE_BYTES
    {
        return Err(invalid("text_length", index));
    }
    if node.native_locator.is_empty() {
        return Err(invalid("locator_text", index));
    }
    if let Some(code) = &node.error
        && !valid_code(code)
    {
        return Err(invalid("error_code", index));
    }
    if node.complete && node.error.is_some() {
        return Err(invalid("complete_flag", index));
    }
    if node.hard_link && node.kind != Kind::File {
        return Err(invalid("hard_link", index));
    }
    if node.kind != Kind::Directory
        && !(leaf_measure_ok(node.logical) && leaf_measure_ok(node.allocated))
    {
        return Err(invalid("measure", index));
    }
    match encoding(&node.native_locator) {
        Encoding::Unix => {
            let hex = &node.native_locator[UNIX_PREFIX.len()..];
            if hex.is_empty() || !hex.len().is_multiple_of(2) || !lower_hex(hex) {
                return Err(invalid("locator_text", index));
            }
            if String::from_utf8_lossy(&decode_unix(hex)) != node.locator.as_str() {
                return Err(invalid("locator_text", index));
            }
        }
        Encoding::Windows => {
            let hex = &node.native_locator[WINDOWS_PREFIX.len()..];
            if hex.is_empty() || !hex.len().is_multiple_of(4) || !lower_hex(hex) {
                return Err(invalid("locator_text", index));
            }
        }
        Encoding::Opaque => {}
    }
    Ok(())
}

struct Nodes<'a>(&'a Context<'a>);
impl<'de> DeserializeSeed<'de> for Nodes<'_> {
    type Value = Vec<Node>;
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Vec<Node>, D::Error> {
        deserializer.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for Nodes<'_> {
    type Value = Vec<Node>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an array of objects")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<Node>, A::Error> {
        let context = self.0;
        let mut nodes: Vec<Node> = Vec::new();
        let mut strings = 0usize;
        // Empty extensions all share one entry. Other suffixes may each be unique.
        let mut extensions = 1usize;
        let mut extension_bytes = 0usize;
        let mut cache: [Option<Box<OsStr>>; CACHED_EXTENSIONS] = Default::default();
        let mut cached = 0;
        let mut cache_bytes = 0usize;
        loop {
            if context.cancel.load(Ordering::Relaxed) {
                return Err(context.fail(ImportError::Cancelled));
            }
            let Some(node) = seq.next_element::<Node>()? else {
                break;
            };
            let index = nodes.len();
            let old_capacity = nodes.capacity();
            let grow = index == old_capacity;
            let capacity = if grow {
                old_capacity.saturating_mul(2).max(4)
            } else {
                old_capacity
            };
            strings = strings
                .saturating_add(text_bytes(node.name.capacity()))
                .saturating_add(text_bytes(node.locator.capacity()))
                .saturating_add(text_bytes(node.native_locator.capacity()))
                .saturating_add(text_bytes(node.error.as_ref().map_or(0, String::capacity)));
            let mut cache_suffix = None;
            if node.kind == Kind::File
                && let Some(suffix) = Path::new(&node.name).extension()
                && !suffix.is_empty()
                && !cache[..cached]
                    .iter()
                    .flatten()
                    .any(|known| &**known == suffix)
            {
                extensions = extensions.saturating_add(1);
                // Lowercasing may expand Unicode, and the resulting String may grow.
                extension_bytes =
                    extension_bytes.saturating_add(text_bytes(suffix.len().saturating_mul(6)));
                // Bound the cache itself and charge uncached suffixes conservatively.
                if cached < cache.len() {
                    cache_suffix = Some(suffix);
                    cache_bytes = cache_bytes.saturating_add(text_bytes(suffix.len()));
                }
            }
            let retained = capacity
                .saturating_mul(size_of::<Node>())
                .saturating_add(strings);
            let result_peak =
                retained.saturating_add(opening_workspace(index + 1, extensions, extension_bytes));
            // Reallocation can keep both buffers live. Unix locator validation
            // can allocate decoded bytes and a lossy UTF-8 string before the push.
            let transition = if grow {
                old_capacity * size_of::<Node>()
            } else {
                0
            };
            let decoding = if encoding(&node.native_locator) == Encoding::Unix {
                node.native_locator.len().min(MAX_NATIVE_BYTES) * 4
            } else {
                0
            };
            context
                .check_memory(
                    retained
                        .saturating_add(cache_bytes)
                        .saturating_add(transition.max(decoding)),
                    result_peak,
                )
                .map_err(|error| context.fail(error))?;
            check_node(index, &node, &nodes).map_err(|error| context.fail(error))?;
            if grow {
                nodes
                    .try_reserve_exact(capacity - index)
                    .map_err(|_| context.fail(ImportError::MemoryLimit))?;
            }
            if let Some(suffix) = cache_suffix {
                cache[cached] = Some(suffix.into());
                cached += 1;
            }
            nodes.push(node);
            context.node_bytes.set(retained.saturating_add(cache_bytes));
            context.result_peak.set(result_peak);
        }
        context
            .node_bytes
            .set(context.node_bytes.get().saturating_sub(cache_bytes));
        Ok(nodes)
    }
}

struct Document<'a>(&'a Context<'a>);
impl<'de> DeserializeSeed<'de> for Document<'_> {
    type Value = ScanResult;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<ScanResult, D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for Document<'_> {
    type Value = ScanResult;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a SpaceMosaic result object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ScanResult, A::Error> {
        let context = self.0;
        let mut version = None::<u32>;
        let mut started = None::<u64>;
        let mut elapsed = None::<u64>;
        let mut scope = None::<String>;
        let mut cancelled = None::<bool>;
        let mut limited = None::<bool>;
        let mut limit_reason = None::<Option<LimitReason>>;
        let mut cross = None::<bool>;
        let mut errors = None::<u64>;
        let mut identity = None::<Option<ScopeIdentity>>;
        let mut app_version = None::<Option<String>>;
        let mut memory = None::<Option<crate::memory::Diagnostics>>;
        let mut nodes = None::<Vec<Node>>;
        macro_rules! once {
            ($slot:ident, $value:expr) => {{
                if $slot.is_some() {
                    return Err(context.fail(ImportError::Invalid {
                        code: "duplicate_field",
                        node: None,
                    }));
                }
                $slot = Some($value);
            }};
        }
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "schema_version" => {
                    let value = map.next_value::<u64>()?;
                    if value < u64::from(MIN_SCHEMA_VERSION) || value > u64::from(SCHEMA_VERSION) {
                        return Err(context.fail(ImportError::UnsupportedVersion(value)));
                    }
                    once!(version, value as u32);
                }
                "started_unix_seconds" => once!(started, map.next_value()?),
                "elapsed_millis" => once!(elapsed, map.next_value()?),
                "scope" => once!(scope, {
                    let value: String = map.next_value()?;
                    context.keep_metadata(text_bytes(value.capacity()))?;
                    value
                }),
                "cancelled" => once!(cancelled, map.next_value()?),
                "limited" => once!(limited, map.next_value()?),
                "limit_reason" => once!(limit_reason, map.next_value()?),
                "cross_filesystems" => once!(cross, map.next_value()?),
                "errors" => once!(errors, map.next_value()?),
                "scope_identity" => once!(identity, {
                    let value: Option<ScopeIdentity> = map.next_value()?;
                    if let Some(identity) = &value {
                        context.keep_metadata(
                            text_bytes(identity.platform.capacity())
                                .saturating_add(text_bytes(identity.root.capacity())),
                        )?;
                    }
                    value
                }),
                "app_version" => once!(app_version, {
                    let value: Option<String> = map.next_value()?;
                    if let Some(version) = &value {
                        context.keep_metadata(text_bytes(version.capacity()))?;
                    }
                    value
                }),
                "memory" => once!(memory, map.next_value()?),
                "nodes" => once!(nodes, map.next_value_seed(Nodes(context))?),
                _ => {
                    // Unknown fields from later minor additions are skipped, not trusted.
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        let missing = || {
            context.fail::<A::Error>(ImportError::Invalid {
                code: "missing_field",
                node: None,
            })
        };
        Ok(ScanResult {
            schema_version: version.ok_or_else(missing)?,
            started_unix_seconds: started.ok_or_else(missing)?,
            elapsed_millis: elapsed.ok_or_else(missing)?,
            scope: scope.ok_or_else(missing)?,
            cancelled: cancelled.ok_or_else(missing)?,
            limited: limited.ok_or_else(missing)?,
            limit_reason: limit_reason.flatten(),
            cross_filesystems: cross.ok_or_else(missing)?,
            errors: errors.ok_or_else(missing)?,
            scope_identity: identity.flatten(),
            app_version: app_version.flatten(),
            memory: memory.flatten(),
            nodes: nodes.ok_or_else(missing)?,
        })
    }
}

#[derive(Default, Clone, Copy)]
struct Sum {
    known: u128,
    unknown: u64,
    overflow: bool,
}
impl Sum {
    fn add(&mut self, measure: Measure) {
        self.known += u128::from(measure.known);
        self.unknown = self.unknown.saturating_add(measure.unknown);
        self.overflow |= measure.overflow;
    }
    /// A directory's recorded aggregate must be explained by its children. The
    /// scanner leaves `known` out of a sum that overflowed, so equality is only
    /// required when no overflow is claimed.
    fn explains(&self, recorded: Measure) -> bool {
        if recorded.overflow {
            return self.overflow || self.known > u128::from(u64::MAX);
        }
        !self.overflow
            && self.known == u128::from(recorded.known)
            && recorded.unknown >= self.unknown
    }
}

/// Relationship checks that need the complete tree. Aggregates are recomputed from
/// the children and must agree; they are never accepted on trust.
fn check_tree(result: &ScanResult, cancel: &AtomicBool) -> Result<(), ImportError> {
    let nodes = &result.nodes;
    let Some(root) = nodes.first() else {
        return Err(ImportError::Invalid {
            code: "empty",
            node: None,
        });
    };
    if result.scope != root.locator {
        return Err(ImportError::Invalid {
            code: "scope_mismatch",
            node: Some(0),
        });
    }
    if result.limit_reason.is_some() && !result.limited {
        return Err(ImportError::Invalid {
            code: "limit_reason",
            node: None,
        });
    }
    let family = encoding(&root.native_locator);
    let schema_two = result.schema_version >= 2;
    match (&result.scope_identity, schema_two) {
        (None, true) | (Some(_), false) => {
            return Err(ImportError::Invalid {
                code: "identity",
                node: None,
            });
        }
        (Some(identity), true)
            if identity.root != root.native_locator
                || identity.platform.is_empty()
                || identity.platform.len() > MAX_CODE_BYTES =>
        {
            return Err(ImportError::Invalid {
                code: "identity",
                node: None,
            });
        }
        _ => {}
    }
    if result
        .app_version
        .as_ref()
        .is_some_and(|v| v.len() > MAX_CODE_BYTES)
    {
        return Err(ImportError::Invalid {
            code: "text_length",
            node: None,
        });
    }
    let mut logical = validation_buffer(nodes.len(), Sum::default())?;
    let mut allocated = validation_buffer(nodes.len(), Sum::default())?;
    let mut all_complete = validation_buffer(nodes.len(), true)?;
    for index in (0..nodes.len()).rev() {
        if cancel.load(Ordering::Relaxed) {
            return Err(ImportError::Cancelled);
        }
        let node = &nodes[index];
        if encoding(&node.native_locator) != family {
            return Err(invalid("locator_family", index));
        }
        if node.kind == Kind::Directory {
            if !logical[index].explains(node.logical) || !allocated[index].explains(node.allocated)
            {
                return Err(invalid("aggregate", index));
            }
            if node.complete && !all_complete[index] {
                return Err(invalid("complete_flag", index));
            }
        }
        let flag_ok = match (
            schema_two,
            node.kind == Kind::Directory,
            node.listing_complete,
        ) {
            (false, _, None) => true,
            (true, true, Some(listed)) => listed || !node.complete,
            (true, false, None) => true,
            _ => false,
        };
        if !flag_ok {
            return Err(invalid("listing_flag", index));
        }
        if let Some(parent) = node.parent {
            let parent_native = &nodes[parent].native_locator;
            if family != Encoding::Opaque {
                // Child identity is derived from the exact bytes after the parent's.
                let tail = node.native_locator.strip_prefix(parent_native.as_str());
                if tail.is_none_or(str::is_empty) {
                    return Err(invalid("locator_hierarchy", index));
                }
            }
            logical[parent].add(node.logical);
            allocated[parent].add(node.allocated);
            all_complete[parent] &= node.complete;
        }
    }
    Ok(())
}

fn validation_buffer<T: Clone>(len: usize, value: T) -> Result<Vec<T>, ImportError> {
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(len)
        .map_err(|_| ImportError::MemoryLimit)?;
    buffer.resize(len, value);
    Ok(buffer)
}

struct Guard<'a, R> {
    inner: R,
    context: &'a Context<'a>,
    progress: &'a AtomicU64,
    string_len: Option<usize>,
    escaped: bool,
    depth: usize,
}
impl<R: Read> Read for Guard<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let context = self.context;
        if context.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let count = self.inner.read(buffer)?;
        self.progress.fetch_add(count as u64, Ordering::Relaxed);
        // serde_json grows a scratch buffer while reading strings, before the
        // field limits can run. Bound it plus in-flight string copies using raw
        // token lengths. Leave syntax and decoded lengths to the deserializer.
        let mut workspace = context.string_workspace.get();
        for &byte in &buffer[..count] {
            match &mut self.string_len {
                Some(len) if self.escaped => {
                    *len += 1;
                    self.escaped = false;
                }
                Some(len) if byte == b'"' => {
                    workspace = workspace.max(len.saturating_mul(8));
                    self.string_len = None;
                }
                Some(len) => {
                    *len += 1;
                    self.escaped = byte == b'\\';
                }
                None => match byte {
                    b'"' => self.string_len = Some(0),
                    b'[' | b'{' => {
                        // IgnoredAny tracks nesting in serde_json's scratch Vec
                        // without applying its normal recursive depth check.
                        if self.depth == MAX_JSON_DEPTH {
                            context
                                .error
                                .borrow_mut()
                                .get_or_insert(ImportError::Malformed);
                            return Err(io::ErrorKind::InvalidData.into());
                        }
                        self.depth += 1;
                    }
                    b']' | b'}' => self.depth = self.depth.saturating_sub(1),
                    _ => {}
                },
            }
        }
        workspace = workspace.max(self.string_len.unwrap_or(0).saturating_mul(8));
        if workspace > context.string_workspace.get() {
            context.string_workspace.set(workspace);
            if let Err(error) =
                context.check_memory(context.node_bytes.get(), context.result_peak.get())
            {
                context.error.borrow_mut().get_or_insert(error);
                return Err(io::ErrorKind::OutOfMemory.into());
            }
        }
        Ok(count)
    }
}

/// Read and validate one exported result from a regular file. `progress`
/// receives the number of bytes consumed so far.
pub fn import_file(
    path: &Path,
    limits: Limits,
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> Result<ScanResult, ImportError> {
    // Decide from metadata before opening: opening a FIFO or device could block forever.
    if !fs::metadata(path)
        .map_err(|error| ImportError::Io(error.kind()))?
        .is_file()
    {
        return Err(ImportError::NotAFile);
    }
    let file = fs::File::open(path).map_err(|error| ImportError::Io(error.kind()))?;
    let metadata = file
        .metadata()
        .map_err(|error| ImportError::Io(error.kind()))?;
    if !metadata.is_file() {
        return Err(ImportError::NotAFile);
    }
    if metadata.len() > limits.max_bytes {
        return Err(ImportError::TooLarge {
            limit: limits.max_bytes,
        });
    }
    import_reader(file, limits, cancel, progress)
}

pub fn import_reader(
    reader: impl Read,
    limits: Limits,
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> Result<ScanResult, ImportError> {
    progress.store(0, Ordering::Relaxed);
    if cancel.load(Ordering::Relaxed) {
        return Err(ImportError::Cancelled);
    }
    let context = Context {
        cancel,
        limits,
        error: RefCell::new(None),
        node_bytes: Cell::new(0),
        result_peak: Cell::new(0),
        metadata_bytes: Cell::new(0),
        string_workspace: Cell::new(0),
    };
    context.check_memory(0, 0)?;
    // One byte beyond the limit distinguishes "too large" from "truncated".
    let bounded = reader.take(limits.max_bytes.saturating_add(1));
    let mut buffered = io::BufReader::with_capacity(
        READ_BUFFER_BYTES,
        Guard {
            inner: bounded,
            context: &context,
            progress,
            string_len: None,
            escaped: false,
            depth: 0,
        },
    );
    if buffered
        .fill_buf()
        .map_err(|error| {
            context
                .error
                .take()
                .unwrap_or_else(|| io_error(error, cancel))
        })?
        .starts_with(&[0xEF, 0xBB, 0xBF])
    {
        buffered.consume(3);
    }
    let mut deserializer = serde_json::Deserializer::from_reader(buffered);
    let parsed = Document(&context).deserialize(&mut deserializer);
    let outcome = parsed.and_then(|result| deserializer.end().map(|()| result));
    drop(deserializer);
    let result = match outcome {
        Ok(result) => result,
        Err(error) => {
            if let Some(precise) = context.error.take() {
                return Err(precise);
            }
            if cancel.load(Ordering::Relaxed) {
                return Err(ImportError::Cancelled);
            }
            if progress.load(Ordering::Relaxed) > limits.max_bytes {
                return Err(ImportError::TooLarge {
                    limit: limits.max_bytes,
                });
            }
            return Err(match error.classify() {
                serde_json::error::Category::Io => {
                    ImportError::Io(error.io_error_kind().unwrap_or(io::ErrorKind::InvalidData))
                }
                serde_json::error::Category::Data => ImportError::InvalidValue,
                _ => ImportError::Malformed,
            });
        }
    };
    if progress.load(Ordering::Relaxed) > limits.max_bytes {
        return Err(ImportError::TooLarge {
            limit: limits.max_bytes,
        });
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(ImportError::Cancelled);
    }
    check_tree(&result, cancel)?;
    Ok(result)
}

fn io_error(error: io::Error, cancel: &AtomicBool) -> ImportError {
    if cancel.load(Ordering::Relaxed) {
        ImportError::Cancelled
    } else {
        ImportError::Io(error.kind())
    }
}
