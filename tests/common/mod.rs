//! Synthetic result builder. It produces results through the same aggregation the
//! scanner uses, so fixtures stay valid imports without touching a real filesystem.
#![allow(dead_code)]
use spacemosaic::{
    compare::{Comparison, Dimension, Status},
    scan::{self, Kind, Measure, Node, ScanResult, ScopeIdentity, SourceKind},
};

pub const MIB: u64 = 1024 * 1024;

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub struct Builder {
    nodes: Vec<Node>,
    paths: Vec<Vec<u8>>,
    provider: bool,
    pub v1: bool,
    pub cross_filesystems: bool,
    pub platform: &'static str,
    pub source: Option<SourceKind>,
    pub cancelled: bool,
    pub inode: Option<u64>,
    pub created: Option<u64>,
}

impl Builder {
    pub fn fs(root: &str) -> Self {
        let mut builder = Self::empty(false);
        builder.push(
            None,
            root.as_bytes(),
            root.as_bytes(),
            Kind::Directory,
            None,
        );
        builder
    }
    pub fn provider(root: &str) -> Self {
        let mut builder = Self::empty(true);
        builder.push(None, b"root", root.as_bytes(), Kind::Directory, None);
        builder
    }
    fn empty(provider: bool) -> Self {
        Self {
            nodes: Vec::new(),
            paths: Vec::new(),
            provider,
            v1: false,
            cross_filesystems: false,
            platform: "macos",
            source: None,
            cancelled: false,
            inode: Some(2),
            created: Some(1_700_000_000_000_000_000),
        }
    }
    fn push(
        &mut self,
        parent: Option<usize>,
        name: &[u8],
        path: &[u8],
        kind: Kind,
        size: Option<Option<u64>>,
    ) -> usize {
        let (locator, native) = if self.provider {
            let id = if self.nodes.is_empty() {
                String::from_utf8_lossy(path).into_owned()
            } else {
                let root = String::from_utf8_lossy(&self.paths[0]).into_owned();
                format!(
                    "{}/document/{}",
                    root.split("/document/").next().unwrap_or(""),
                    self.nodes.len()
                )
            };
            (id.clone(), id)
        } else {
            (
                String::from_utf8_lossy(path).into_owned(),
                format!("unix-bytes:{}", hex(path)),
            )
        };
        let (logical, allocated) = match (kind, size) {
            (Kind::Directory, _) => (Measure::default(), Measure::default()),
            (_, Some(Some(bytes))) => (
                Measure {
                    known: bytes,
                    ..Default::default()
                },
                if self.provider {
                    Measure {
                        unknown: 1,
                        ..Default::default()
                    }
                } else {
                    Measure {
                        known: bytes,
                        ..Default::default()
                    }
                },
            ),
            _ => (
                Measure {
                    unknown: 1,
                    ..Default::default()
                },
                Measure {
                    unknown: 1,
                    ..Default::default()
                },
            ),
        };
        self.nodes.push(Node {
            parent,
            name: String::from_utf8_lossy(name).into_owned(),
            locator,
            native_locator: native,
            kind,
            logical,
            allocated,
            complete: true,
            hard_link: false,
            error: None,
            listing_complete: (kind == Kind::Directory).then_some(true),
            path: None,
        });
        self.paths.push(path.to_vec());
        self.nodes.len() - 1
    }
    fn child_path(&self, parent: usize, name: &[u8]) -> Vec<u8> {
        let mut path = self.paths[parent].clone();
        path.push(b'/');
        path.extend_from_slice(name);
        path
    }
    pub fn dir(&mut self, parent: usize, name: &str) -> usize {
        self.dir_raw(parent, name.as_bytes())
    }
    pub fn dir_raw(&mut self, parent: usize, name: &[u8]) -> usize {
        let path = self.child_path(parent, name);
        self.push(Some(parent), name, &path, Kind::Directory, None)
    }
    pub fn file(&mut self, parent: usize, name: &str, size: u64) -> usize {
        self.file_raw(parent, name.as_bytes(), Some(size))
    }
    pub fn unknown_file(&mut self, parent: usize, name: &str) -> usize {
        self.file_raw(parent, name.as_bytes(), None)
    }
    pub fn file_raw(&mut self, parent: usize, name: &[u8], size: Option<u64>) -> usize {
        let path = self.child_path(parent, name);
        self.push(Some(parent), name, &path, Kind::File, Some(size))
    }
    pub fn link(&mut self, parent: usize, name: &str) -> usize {
        let path = self.child_path(parent, name.as_bytes());
        self.push(Some(parent), name.as_bytes(), &path, Kind::Link, Some(None))
    }
    /// Directory that could not be read at all.
    pub fn deny(&mut self, dir: usize) {
        let node = &mut self.nodes[dir];
        node.error = Some("permission_denied".into());
        node.complete = false;
        node.listing_complete = Some(false);
        node.logical.unknown += 1;
        node.allocated.unknown += 1;
    }
    /// Directory whose listing stopped early (cancellation or a limit).
    pub fn abort_listing(&mut self, dir: usize) {
        let node = &mut self.nodes[dir];
        node.complete = false;
        node.listing_complete = Some(false);
    }
    pub fn build(mut self) -> ScanResult {
        scan::aggregate(&mut self.nodes);
        let root = self.nodes[0].native_locator.clone();
        let errors = self.nodes.iter().filter(|n| n.error.is_some()).count() as u64;
        let incomplete = !self.nodes[0].complete;
        if self.v1 {
            for node in &mut self.nodes {
                node.listing_complete = None;
            }
        }
        ScanResult {
            schema_version: if self.v1 { 1 } else { 2 },
            started_unix_seconds: 1_700_000_000,
            elapsed_millis: 5,
            scope: self.nodes[0].locator.clone(),
            cancelled: self.cancelled,
            limited: false,
            limit_reason: None,
            cross_filesystems: self.cross_filesystems,
            errors,
            scope_identity: (!self.v1).then(|| ScopeIdentity {
                source: self.source.unwrap_or(if self.provider {
                    SourceKind::AndroidSaf
                } else {
                    SourceKind::Filesystem
                }),
                platform: if self.provider {
                    "android".into()
                } else {
                    self.platform.into()
                },
                root,
                root_inode: if self.provider { None } else { self.inode },
                root_created_unix_nanos: if self.provider { None } else { self.created },
            }),
            app_version: (!self.v1).then(|| "test".into()),
            nodes: {
                let _ = incomplete;
                self.nodes
            },
        }
    }
}

/// Row whose relative path (names from the compared root) equals `path`.
pub fn find(comparison: &Comparison, path: &str) -> usize {
    (0..comparison.nodes.len())
        .find(|&i| comparison.path(i) == path)
        .unwrap_or_else(|| panic!("no row {path}"))
}
pub fn status(comparison: &Comparison, path: &str) -> Status {
    comparison.nodes[find(comparison, path)].status(Dimension::Logical)
}
