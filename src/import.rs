//! Bounded import of exported JSON results. The file is untrusted input: it is
//! parsed as a stream, limited in size, object count and estimated memory, and
//! checked structurally before any view or comparison can use it. Nothing in it
//! is ever used to open, reveal or rescan a location.
use crate::scan::{Kind, LimitReason, Measure, Node, SCHEMA_VERSION, ScanResult, ScopeIdentity};
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::{
    cell::{Cell, RefCell},
    fmt, fs,
    io::{self, BufRead, Read},
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

/// Independent defensive bounds for one untrusted import. Scans now have an
/// adaptive budget and can produce a file larger than these import limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_bytes: u64,
    pub max_nodes: usize,
    pub memory_limit: usize,
}
impl Limits {
    pub const DESKTOP: Self = Self {
        max_bytes: 1024 * 1024 * 1024,
        max_nodes: 500_000,
        memory_limit: 256 * 1024 * 1024,
    };
    pub const MOBILE: Self = Self {
        max_bytes: 128 * 1024 * 1024,
        max_nodes: 100_000,
        memory_limit: 64 * 1024 * 1024,
    };
    pub const fn platform() -> Self {
        if cfg!(any(target_os = "android", target_os = "ios")) {
            Self::MOBILE
        } else {
            Self::DESKTOP
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    Io(io::ErrorKind),
    NotAFile,
    TooLarge {
        limit: u64,
    },
    TooManyObjects {
        limit: usize,
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
            Self::TooLarge { .. } | Self::TooManyObjects { .. } | Self::MemoryLimit => {
                "import_error_too_large"
            }
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
            Self::TooManyObjects { limit } => format!("more than {limit} objects"),
            Self::MemoryLimit => "estimated metadata memory limit exceeded".into(),
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
    memory: Cell<usize>,
}
impl Context<'_> {
    /// Remember the precise error; serde only carries a generic message.
    fn fail<E: de::Error>(&self, error: ImportError) -> E {
        self.error.borrow_mut().get_or_insert(error);
        E::custom("import rejected")
    }
}

fn node_cost(node: &Node) -> usize {
    std::mem::size_of::<Node>()
        + node.native_locator.len()
        + node.locator.len() * 2
        + node.name.len()
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
        loop {
            if context.cancel.load(Ordering::Relaxed) {
                return Err(context.fail(ImportError::Cancelled));
            }
            let Some(node) = seq.next_element::<Node>()? else {
                break;
            };
            let index = nodes.len();
            if index >= context.limits.max_nodes {
                return Err(context.fail(ImportError::TooManyObjects {
                    limit: context.limits.max_nodes,
                }));
            }
            let memory = context.memory.get().saturating_add(node_cost(&node));
            context.memory.set(memory);
            if memory > context.limits.memory_limit {
                return Err(context.fail(ImportError::MemoryLimit));
            }
            check_node(index, &node, &nodes).map_err(|error| context.fail(error))?;
            nodes.push(node);
        }
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
                "scope" => once!(scope, map.next_value()?),
                "cancelled" => once!(cancelled, map.next_value()?),
                "limited" => once!(limited, map.next_value()?),
                "limit_reason" => once!(limit_reason, map.next_value()?),
                "cross_filesystems" => once!(cross, map.next_value()?),
                "errors" => once!(errors, map.next_value()?),
                "scope_identity" => once!(identity, map.next_value()?),
                "app_version" => once!(app_version, map.next_value()?),
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
fn check_tree(result: &ScanResult) -> Result<(), ImportError> {
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
    let mut logical = vec![Sum::default(); nodes.len()];
    let mut allocated = vec![Sum::default(); nodes.len()];
    let mut all_complete = vec![true; nodes.len()];
    for index in (0..nodes.len()).rev() {
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

struct Guard<'a, R> {
    inner: R,
    cancel: &'a AtomicBool,
    progress: &'a AtomicU64,
}
impl<R: Read> Read for Guard<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let count = self.inner.read(buffer)?;
        self.progress.fetch_add(count as u64, Ordering::Relaxed);
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
    // One byte beyond the limit distinguishes "too large" from "truncated".
    let bounded = reader.take(limits.max_bytes.saturating_add(1));
    let mut buffered = io::BufReader::with_capacity(
        64 * 1024,
        Guard {
            inner: bounded,
            cancel,
            progress,
        },
    );
    if buffered
        .fill_buf()
        .map_err(|error| io_error(error, cancel))?
        .starts_with(&[0xEF, 0xBB, 0xBF])
    {
        buffered.consume(3);
    }
    let context = Context {
        cancel,
        limits,
        error: RefCell::new(None),
        memory: Cell::new(0),
    };
    let mut deserializer = serde_json::Deserializer::from_reader(buffered);
    let parsed = Document(&context).deserialize(&mut deserializer);
    let outcome = parsed.and_then(|result| deserializer.end().map(|()| result));
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
    check_tree(&result)?;
    Ok(result)
}

fn io_error(error: io::Error, cancel: &AtomicBool) -> ImportError {
    if cancel.load(Ordering::Relaxed) {
        ImportError::Cancelled
    } else {
        ImportError::Io(error.kind())
    }
}
