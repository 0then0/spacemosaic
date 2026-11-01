//! Comparison of two observed results. It matches recorded size observations by
//! exact name under matched parents; it proves neither deletion, freed blocks nor
//! unchanged content. Anything not covered by both observations stays unknown.
//!
//! Accounting model: every difference is attributed to exactly one *event*.
//! An event is an added or removed object (its whole subtree counts once), a
//! type change, or a matched non-directory whose size differs. Matched
//! directories are containers; they report the sum of the events below them and
//! are never counted themselves, so directories and their descendants are not
//! added together.
use crate::{
    import::{Encoding, encoding},
    scan::{Kind, Measure, ScanResult, ScopeIdentity, SourceKind},
};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dimension {
    Logical = 0,
    Allocated = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Status {
    Added = 0,
    Removed = 1,
    Grew = 2,
    Shrank = 3,
    SameSize = 4,
    Unknown = 5,
    TypeChanged = 6,
}
impl Status {
    pub const ALL: [Status; 7] = [
        Self::Added,
        Self::Removed,
        Self::Grew,
        Self::Shrank,
        Self::SameSize,
        Self::Unknown,
        Self::TypeChanged,
    ];
    pub const fn bit(self) -> u8 {
        1 << self as u8
    }
    /// Statuses shown before the user opts into "same size" rows.
    pub const CHANGED: u8 = 0b110_1111;
    pub const EVERYTHING: u8 = 0b111_1111;
    /// Translation key for the status label. The label never depends on color.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Added => "cmp_added",
            Self::Removed => "cmp_removed",
            Self::Grew => "cmp_grew",
            Self::Shrank => "cmp_shrank",
            Self::SameSize => "cmp_same_size",
            Self::Unknown => "cmp_unknown",
            Self::TypeChanged => "cmp_type_changed",
        }
    }
    /// Leading symbol that duplicates the status text for quick scanning.
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Added => "+",
            Self::Removed => "−",
            Self::Grew => "↑",
            Self::Shrank => "↓",
            Self::SameSize => "=",
            Self::Unknown => "?",
            Self::TypeChanged => "⇄",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    None,
    SizeUnknown,
    SizeOverflow,
    /// Present now; the baseline did not enumerate the parent completely.
    BaselineNotObserved,
    /// Present in the baseline; the current scan did not enumerate the parent completely.
    CurrentNotObserved,
    AmbiguousName,
    /// A directory whose listing was incomplete in at least one result.
    ListingIncomplete,
    /// A directory with a confirmed part and an undetermined part.
    PartialContent,
}
impl Reason {
    pub const fn key(self) -> Option<&'static str> {
        Some(match self {
            Self::None => return None,
            Self::SizeUnknown => "cmp_reason_size_unknown",
            Self::SizeOverflow => "cmp_reason_overflow",
            Self::BaselineNotObserved => "cmp_reason_baseline_not_observed",
            Self::CurrentNotObserved => "cmp_reason_current_not_observed",
            Self::AmbiguousName => "cmp_reason_ambiguous",
            Self::ListingIncomplete => "cmp_reason_listing_incomplete",
            Self::PartialContent => "cmp_reason_partial",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeVerdict {
    /// Everything this format records about the area agrees.
    Matching,
    /// Not enough recorded to decide; a person may confirm the correspondence.
    Insufficient,
    /// Recorded facts contradict; never compared.
    Incompatible,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeNote {
    SameRecordedRoot,
    NoRecordedIdentity,
    DifferentSource,
    DifferentPlatform,
    DifferentProviderAuthority,
    DifferentRoot,
    ProviderRootSpelling,
    RootObjectChanged,
    VolumeNotVerified,
}
impl ScopeNote {
    pub const fn key(self) -> &'static str {
        match self {
            Self::SameRecordedRoot => "cmp_scope_same_root",
            Self::NoRecordedIdentity => "cmp_scope_no_identity",
            Self::DifferentSource => "cmp_scope_different_source",
            Self::DifferentPlatform => "cmp_scope_different_platform",
            Self::DifferentProviderAuthority => "cmp_scope_different_authority",
            Self::DifferentRoot => "cmp_scope_different_root",
            Self::ProviderRootSpelling => "cmp_scope_provider_spelling",
            Self::RootObjectChanged => "cmp_scope_object_changed",
            Self::VolumeNotVerified => "cmp_scope_volume_unverified",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeAssessment {
    pub verdict: ScopeVerdict,
    pub notes: Vec<ScopeNote>,
}

struct Facts<'a> {
    source: SourceKind,
    family: &'a str,
    platform: Option<&'a str>,
    root: &'a str,
    recorded: Option<&'a ScopeIdentity>,
}
fn family_of(platform: &str) -> &str {
    match platform {
        "windows" => "windows",
        "android" => "android",
        "ios" => "ios",
        "macos" | "linux" | "freebsd" | "netbsd" | "openbsd" | "dragonfly" | "solaris"
        | "illumos" => "unix",
        other => other,
    }
}
fn facts(result: &ScanResult) -> Facts<'_> {
    let root = result.nodes[0].native_locator.as_str();
    if let Some(identity) = &result.scope_identity {
        return Facts {
            source: identity.source,
            family: family_of(&identity.platform),
            platform: Some(&identity.platform),
            root,
            recorded: Some(identity),
        };
    }
    // Schema 1 recorded no identity. The syntax of the stored locator only rules
    // combinations out; it never establishes that two scopes are the same.
    let (source, family) = match encoding(root) {
        Encoding::Unix => (SourceKind::Filesystem, "unix"),
        Encoding::Windows => (SourceKind::Filesystem, "windows"),
        Encoding::Opaque if root.starts_with("content://") => (SourceKind::AndroidSaf, "android"),
        Encoding::Opaque if root.starts_with("file://") => (SourceKind::IosFiles, "ios"),
        Encoding::Opaque => (SourceKind::Filesystem, "unknown"),
    };
    Facts {
        source,
        family,
        platform: None,
        root,
        recorded: None,
    }
}
fn authority(root: &str) -> Option<&str> {
    root.strip_prefix("content://")
        .map(|rest| rest.split('/').next().unwrap_or(rest))
}

/// Decide whether two results describe the same area, from recorded facts only.
/// Matching is the strongest verdict available: neither format can prove that a
/// volume or device was not replaced under an unchanged path.
pub fn assess_scope(baseline: &ScanResult, current: &ScanResult) -> ScopeAssessment {
    use ScopeNote::*;
    let (a, b) = (facts(baseline), facts(current));
    let verdict = |verdict, notes: Vec<ScopeNote>| ScopeAssessment { verdict, notes };
    if a.source != b.source {
        return verdict(ScopeVerdict::Incompatible, vec![DifferentSource]);
    }
    if a.family != b.family || a.platform.zip(b.platform).is_some_and(|(x, y)| x != y) {
        return verdict(ScopeVerdict::Incompatible, vec![DifferentPlatform]);
    }
    if a.root != b.root {
        return match a.source {
            SourceKind::IosFiles => verdict(ScopeVerdict::Insufficient, vec![ProviderRootSpelling]),
            SourceKind::AndroidSaf if authority(a.root) != authority(b.root) => {
                verdict(ScopeVerdict::Incompatible, vec![DifferentProviderAuthority])
            }
            _ => verdict(ScopeVerdict::Incompatible, vec![DifferentRoot]),
        };
    }
    let (Some(x), Some(y)) = (a.recorded, b.recorded) else {
        return verdict(ScopeVerdict::Insufficient, vec![NoRecordedIdentity]);
    };
    if differs(x.root_inode, y.root_inode)
        || differs(x.root_created_unix_nanos, y.root_created_unix_nanos)
    {
        return verdict(ScopeVerdict::Insufficient, vec![RootObjectChanged]);
    }
    verdict(
        ScopeVerdict::Matching,
        vec![SameRecordedRoot, VolumeNotVerified],
    )
}
fn differs(a: Option<u64>, b: Option<u64>) -> bool {
    a.zip(b).is_some_and(|(x, y)| x != y)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limitation {
    BaselinePartial,
    CurrentPartial,
    CrossFilesystemsDiffer,
    AllocatedUnavailable,
    OlderSchema,
    ScopeConfirmedByUser,
    VolumeNotVerified,
}
impl Limitation {
    pub const fn key(self) -> &'static str {
        match self {
            Self::BaselinePartial => "cmp_limit_baseline_partial",
            Self::CurrentPartial => "cmp_limit_current_partial",
            Self::CrossFilesystemsDiffer => "cmp_limit_crossing",
            Self::AllocatedUnavailable => "cmp_limit_allocated",
            Self::OlderSchema => "cmp_limit_older_schema",
            Self::ScopeConfirmedByUser => "cmp_limit_user_confirmed",
            Self::VolumeNotVerified => "cmp_scope_volume_unverified",
        }
    }
}
pub fn is_partial(result: &ScanResult) -> bool {
    result.cancelled || result.limited || !result.nodes[0].complete
}
pub fn allocated_measured(result: &ScanResult) -> bool {
    result.nodes[0].allocated.known > 0 || result.nodes[0].allocated.unknown == 0
}
/// Listing completeness of one directory. Schema 1 never recorded it; there only
/// a fully complete subtree proves a complete listing.
fn listing_complete(result: &ScanResult, index: usize) -> bool {
    let node = &result.nodes[index];
    node.listing_complete.unwrap_or(node.complete)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    Container,
    Pair,
    TypeChanged,
    OldOnly,
    NewOnly,
    Ambiguous,
}

#[derive(Clone, Copy, Debug)]
pub struct DiffNode {
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub parent: Option<u32>,
    pub first_child: u32,
    pub child_count: u32,
    form: Form,
    coverage: Reason,
    own_unknown: bool,
    status: [Status; 2],
    reason: [Reason; 2],
    confirmed: [i128; 2],
    unknown: [u32; 2],
    mask: [u8; 2],
}
impl DiffNode {
    pub fn status(&self, dimension: Dimension) -> Status {
        self.status[dimension as usize]
    }
    pub fn reason(&self, dimension: Dimension) -> Reason {
        self.reason[dimension as usize]
    }
    /// Net confirmed byte change of the events at or below this row. Directories
    /// show the sum of their events, never a difference of aggregated sizes.
    pub fn confirmed(&self, dimension: Dimension) -> i128 {
        self.confirmed[dimension as usize]
    }
    /// Events at or below this row whose size or change is not fully determined.
    pub fn unknown(&self, dimension: Dimension) -> u32 {
        self.unknown[dimension as usize]
    }
    /// Union of the status bits of all events at or below this row.
    pub fn mask(&self, dimension: Dimension) -> u8 {
        self.mask[dimension as usize]
    }
    pub fn is_container(&self) -> bool {
        self.form == Form::Container
    }
    /// A row that is itself an event, rather than only summing events below it.
    pub fn is_event(&self) -> bool {
        self.form != Form::Container || self.own_unknown
    }
    pub fn children(&self) -> std::ops::Range<usize> {
        self.first_child as usize..(self.first_child + self.child_count) as usize
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    /// Bytes of added objects (plus the new side of type changes).
    pub added: u128,
    /// Bytes of removed objects (plus the old side of type changes).
    pub removed: u128,
    /// Growth of objects present in both results.
    pub grown: u128,
    /// Shrinkage of objects present in both results.
    pub shrunk: u128,
    pub added_count: u64,
    pub removed_count: u64,
    pub grew_count: u64,
    pub shrank_count: u64,
    pub same_count: u64,
    pub type_changed_count: u64,
    /// Events whose change could not be determined.
    pub unknown_count: u64,
    /// Added, removed or type-changed events whose byte amount is only a lower bound.
    pub lower_bound_count: u64,
}
impl Totals {
    /// Confirmed net change. It is a complete total only when `is_complete`.
    pub fn net(&self) -> i128 {
        self.added as i128 + self.grown as i128 - self.removed as i128 - self.shrunk as i128
    }
    pub fn is_complete(&self) -> bool {
        self.unknown_count == 0 && self.lower_bound_count == 0
    }
}

#[derive(Clone, Debug)]
pub struct Comparison {
    pub baseline: Arc<ScanResult>,
    pub current: Arc<ScanResult>,
    pub scope: ScopeAssessment,
    pub scope_confirmed_by_user: bool,
    pub limitations: Vec<Limitation>,
    pub allocated_available: bool,
    pub nodes: Vec<DiffNode>,
    totals: [Totals; 2],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompareError {
    Cancelled,
    NeedsConfirmation(ScopeAssessment),
    Incompatible(ScopeAssessment),
    TooLarge,
}

/// Children of each node as contiguous ranges; avoids a `Vec` per directory.
struct Children {
    start: Vec<u32>,
    list: Vec<u32>,
}
impl Children {
    fn new(result: &ScanResult) -> Self {
        let count = result.nodes.len();
        let mut start = vec![0_u32; count + 1];
        for node in &result.nodes {
            if let Some(parent) = node.parent {
                start[parent + 1] += 1;
            }
        }
        for index in 0..count {
            start[index + 1] += start[index];
        }
        let mut fill = start.clone();
        let mut list = vec![0_u32; start[count] as usize];
        for (index, node) in result.nodes.iter().enumerate() {
            if let Some(parent) = node.parent {
                list[fill[parent] as usize] = index as u32;
                fill[parent] += 1;
            }
        }
        Self { start, list }
    }
    fn of(&self, index: usize) -> &[u32] {
        &self.list[self.start[index] as usize..self.start[index + 1] as usize]
    }
}

/// The exact-name key of a child. Native hex locators compare bytes, so names that
/// differ in case, normalization or invalid UTF-8 never merge; provider locators
/// are opaque, so their reported name is the only available key.
fn child_key(result: &ScanResult, mode: Encoding, parent: usize, child: usize) -> &str {
    let node = &result.nodes[child];
    let tail = match mode {
        Encoding::Opaque => None,
        _ => node
            .native_locator
            .strip_prefix(result.nodes[parent].native_locator.as_str()),
    };
    match (mode, tail) {
        (Encoding::Unix, Some(tail)) => tail.strip_prefix("2f").unwrap_or(tail),
        (Encoding::Windows, Some(tail)) => tail
            .strip_prefix("005c")
            .or_else(|| tail.strip_prefix("002f"))
            .unwrap_or(tail),
        _ => &node.name,
    }
}

const CHECK_INTERVAL: usize = 1024;

/// Compare two results. `confirm_scope` is a person's statement that the areas
/// correspond; it can only upgrade an insufficient verdict, never an incompatible
/// one, and it does not alter any completeness or measurement limitation.
pub fn compare(
    baseline: Arc<ScanResult>,
    current: Arc<ScanResult>,
    confirm_scope: bool,
    cancel: &AtomicBool,
) -> Result<Comparison, CompareError> {
    let scope = assess_scope(&baseline, &current);
    match scope.verdict {
        ScopeVerdict::Incompatible => return Err(CompareError::Incompatible(scope)),
        ScopeVerdict::Insufficient if !confirm_scope => {
            return Err(CompareError::NeedsConfirmation(scope));
        }
        _ => {}
    }
    if baseline.nodes.len() >= u32::MAX as usize / 2 || current.nodes.len() >= u32::MAX as usize / 2
    {
        return Err(CompareError::TooLarge);
    }
    let (old, new) = (&*baseline, &*current);
    let (old_children, new_children) = (Children::new(old), Children::new(new));
    let mode = {
        let (a, b) = (
            encoding(&old.nodes[0].native_locator),
            encoding(&new.nodes[0].native_locator),
        );
        if a == b { a } else { Encoding::Opaque }
    };
    let mut nodes: Vec<DiffNode> = Vec::new();
    let root_form = pair_form(old.nodes[0].kind, new.nodes[0].kind);
    nodes.push(DiffNode {
        old: Some(0),
        new: Some(0),
        parent: None,
        first_child: 0,
        child_count: 0,
        form: root_form,
        coverage: Reason::None,
        own_unknown: false,
        status: [Status::SameSize; 2],
        reason: [Reason::None; 2],
        confirmed: [0; 2],
        unknown: [0; 2],
        mask: [0; 2],
    });
    let mut queue = VecDeque::new();
    if root_form == Form::Container {
        queue.push_back(0_u32);
    }
    let mut processed = 0_usize;
    while let Some(diff) = queue.pop_front() {
        processed += 1;
        if processed.is_multiple_of(CHECK_INTERVAL) && cancel.load(Ordering::Relaxed) {
            return Err(CompareError::Cancelled);
        }
        let (o, n) = (
            nodes[diff as usize].old.expect("container old") as usize,
            nodes[diff as usize].new.expect("container new") as usize,
        );
        let (old_listed, new_listed) = (listing_complete(old, o), listing_complete(new, n));
        let mut old_keys: Vec<(&str, u32)> = old_children
            .of(o)
            .iter()
            .map(|&c| (child_key(old, mode, o, c as usize), c))
            .collect();
        let mut new_keys: Vec<(&str, u32)> = new_children
            .of(n)
            .iter()
            .map(|&c| (child_key(new, mode, n, c as usize), c))
            .collect();
        old_keys.sort_unstable();
        new_keys.sort_unstable();
        let first = nodes.len() as u32;
        let push = |nodes: &mut Vec<DiffNode>,
                    old: Option<u32>,
                    new: Option<u32>,
                    form: Form,
                    coverage: Reason| {
            nodes.push(DiffNode {
                old,
                new,
                parent: Some(diff),
                first_child: 0,
                child_count: 0,
                form,
                coverage,
                own_unknown: false,
                status: [Status::SameSize; 2],
                reason: [Reason::None; 2],
                confirmed: [0; 2],
                unknown: [0; 2],
                mask: [0; 2],
            });
            nodes.len() as u32 - 1
        };
        let (mut i, mut j) = (0, 0);
        while i < old_keys.len() || j < new_keys.len() {
            let ordering = match (old_keys.get(i), new_keys.get(j)) {
                (Some(a), Some(b)) => a.0.cmp(b.0),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, _) => std::cmp::Ordering::Greater,
            };
            let key = match ordering {
                std::cmp::Ordering::Greater => new_keys[j].0,
                _ => old_keys[i].0,
            };
            let a_end = i + old_keys[i..].iter().take_while(|k| k.0 == key).count();
            let b_end = j + new_keys[j..].iter().take_while(|k| k.0 == key).count();
            let (a_count, b_count) = (a_end - i, b_end - j);
            if a_count == 1 && b_count == 1 {
                let (oc, nc) = (old_keys[i].1, new_keys[j].1);
                let form = pair_form(old.nodes[oc as usize].kind, new.nodes[nc as usize].kind);
                let child = push(&mut nodes, Some(oc), Some(nc), form, Reason::None);
                if form == Form::Container {
                    queue.push_back(child);
                }
            } else if a_count > 0 && b_count > 0 {
                // Several objects share one name on a side: any pairing would be a guess.
                for &(_, oc) in &old_keys[i..a_end] {
                    push(
                        &mut nodes,
                        Some(oc),
                        None,
                        Form::Ambiguous,
                        Reason::AmbiguousName,
                    );
                }
                for &(_, nc) in &new_keys[j..b_end] {
                    push(
                        &mut nodes,
                        None,
                        Some(nc),
                        Form::Ambiguous,
                        Reason::AmbiguousName,
                    );
                }
            } else if a_count > 0 {
                let coverage = if new_listed {
                    Reason::None
                } else {
                    Reason::CurrentNotObserved
                };
                for &(_, oc) in &old_keys[i..a_end] {
                    push(&mut nodes, Some(oc), None, Form::OldOnly, coverage);
                }
            } else {
                let coverage = if old_listed {
                    Reason::None
                } else {
                    Reason::BaselineNotObserved
                };
                for &(_, nc) in &new_keys[j..b_end] {
                    push(&mut nodes, None, Some(nc), Form::NewOnly, coverage);
                }
            }
            i = a_end;
            j = b_end;
        }
        let count = nodes.len() as u32 - first;
        let entry = &mut nodes[diff as usize];
        entry.first_child = first;
        entry.child_count = count;
        entry.own_unknown = !old_listed || !new_listed;
    }
    let allocated_available = allocated_measured(old) && allocated_measured(new);
    for dimension in [Dimension::Logical, Dimension::Allocated] {
        evaluate(&mut nodes, old, new, dimension, cancel)?;
    }
    let totals = [
        totals(&nodes, old, new, Dimension::Logical),
        totals(&nodes, old, new, Dimension::Allocated),
    ];
    let mut limitations = Vec::new();
    if is_partial(old) {
        limitations.push(Limitation::BaselinePartial);
    }
    if is_partial(new) {
        limitations.push(Limitation::CurrentPartial);
    }
    if old.nodes[0].kind == Kind::Directory
        && matches!(
            old.scope_identity.as_ref().map(|i| i.source),
            None | Some(SourceKind::Filesystem)
        )
        && old.cross_filesystems != new.cross_filesystems
    {
        limitations.push(Limitation::CrossFilesystemsDiffer);
    }
    if !allocated_available {
        limitations.push(Limitation::AllocatedUnavailable);
    }
    if old.schema_version < 2 || new.schema_version < 2 {
        limitations.push(Limitation::OlderSchema);
    }
    if scope.verdict == ScopeVerdict::Insufficient {
        limitations.push(Limitation::ScopeConfirmedByUser);
    }
    if scope.notes.contains(&ScopeNote::VolumeNotVerified) {
        limitations.push(Limitation::VolumeNotVerified);
    }
    Ok(Comparison {
        baseline,
        current,
        scope,
        scope_confirmed_by_user: confirm_scope,
        limitations,
        allocated_available,
        nodes,
        totals,
    })
}

fn pair_form(old: Kind, new: Kind) -> Form {
    match (old, new) {
        (Kind::Directory, Kind::Directory) => Form::Container,
        (a, b) if a == b => Form::Pair,
        _ => Form::TypeChanged,
    }
}

fn measure(result: &ScanResult, index: Option<u32>, dimension: Dimension) -> Option<Measure> {
    index.map(|i| {
        let node = &result.nodes[i as usize];
        match dimension {
            Dimension::Logical => node.logical,
            Dimension::Allocated => node.allocated,
        }
    })
}

/// Fill status, confirmed amount, unknown count and status mask for one dimension.
/// Parents precede children, so a reverse pass aggregates containers.
fn evaluate(
    nodes: &mut [DiffNode],
    old: &ScanResult,
    new: &ScanResult,
    dimension: Dimension,
    cancel: &AtomicBool,
) -> Result<(), CompareError> {
    let d = dimension as usize;
    for index in (0..nodes.len()).rev() {
        if index.is_multiple_of(CHECK_INTERVAL) && cancel.load(Ordering::Relaxed) {
            return Err(CompareError::Cancelled);
        }
        let (o, n) = (
            measure(old, nodes[index].old, dimension),
            measure(new, nodes[index].new, dimension),
        );
        let node = &nodes[index];
        let (status, reason, confirmed, unknown) = match node.form {
            Form::Container => {
                let (mut confirmed, mut unknown, mut mask) = (0_i128, 0_u32, 0_u8);
                for child in node.children() {
                    confirmed += nodes[child].confirmed[d];
                    unknown = unknown.saturating_add(nodes[child].unknown[d]);
                    mask |= nodes[child].mask[d];
                }
                if node.own_unknown {
                    unknown = unknown.saturating_add(1);
                    mask |= Status::Unknown.bit();
                }
                let (status, reason) = match (confirmed, unknown) {
                    (c, 0) if c > 0 => (Status::Grew, Reason::None),
                    (c, 0) if c < 0 => (Status::Shrank, Reason::None),
                    (_, 0) => (Status::SameSize, Reason::None),
                    (c, _) => {
                        let reason = if node.own_unknown {
                            Reason::ListingIncomplete
                        } else {
                            Reason::PartialContent
                        };
                        (
                            if c > 0 {
                                Status::Grew
                            } else if c < 0 {
                                Status::Shrank
                            } else {
                                Status::Unknown
                            },
                            reason,
                        )
                    }
                };
                nodes[index].mask[d] = mask;
                (status, reason, confirmed, unknown)
            }
            Form::Pair => {
                let (o, n) = (o.expect("old"), n.expect("new"));
                if o.determined() && n.determined() {
                    let delta = i128::from(n.known) - i128::from(o.known);
                    let status = match delta {
                        d if d > 0 => Status::Grew,
                        d if d < 0 => Status::Shrank,
                        _ => Status::SameSize,
                    };
                    (status, Reason::None, delta, 0)
                } else {
                    let reason = if o.overflow || n.overflow {
                        Reason::SizeOverflow
                    } else {
                        Reason::SizeUnknown
                    };
                    (Status::Unknown, reason, 0, 1)
                }
            }
            Form::TypeChanged => {
                let (o, n) = (o.expect("old"), n.expect("new"));
                let partial = !(o.determined() && n.determined());
                (
                    Status::TypeChanged,
                    if partial {
                        Reason::SizeUnknown
                    } else {
                        Reason::None
                    },
                    i128::from(n.known) - i128::from(o.known),
                    u32::from(partial),
                )
            }
            Form::OldOnly => {
                let o = o.expect("old");
                if node.coverage == Reason::None {
                    (
                        Status::Removed,
                        if o.determined() {
                            Reason::None
                        } else {
                            Reason::SizeUnknown
                        },
                        -i128::from(o.known),
                        u32::from(!o.determined()),
                    )
                } else {
                    (Status::Unknown, node.coverage, 0, 1)
                }
            }
            Form::NewOnly => {
                let n = n.expect("new");
                if node.coverage == Reason::None {
                    (
                        Status::Added,
                        if n.determined() {
                            Reason::None
                        } else {
                            Reason::SizeUnknown
                        },
                        i128::from(n.known),
                        u32::from(!n.determined()),
                    )
                } else {
                    (Status::Unknown, node.coverage, 0, 1)
                }
            }
            Form::Ambiguous => (Status::Unknown, Reason::AmbiguousName, 0, 1),
        };
        let node = &mut nodes[index];
        node.status[d] = status;
        node.reason[d] = reason;
        node.confirmed[d] = confirmed;
        node.unknown[d] = unknown;
        if node.form != Form::Container {
            node.mask[d] = status.bit();
        }
    }
    Ok(())
}

fn totals(nodes: &[DiffNode], old: &ScanResult, new: &ScanResult, dimension: Dimension) -> Totals {
    let mut totals = Totals::default();
    for node in nodes {
        add_event(&mut totals, node, old, new, dimension);
    }
    totals
}

fn add_event(
    totals: &mut Totals,
    node: &DiffNode,
    old: &ScanResult,
    new: &ScanResult,
    dimension: Dimension,
) {
    if node.form == Form::Container {
        if node.own_unknown {
            totals.unknown_count += 1;
        }
        return;
    }
    let status = node.status(dimension);
    let (o, n) = (
        measure(old, node.old, dimension),
        measure(new, node.new, dimension),
    );
    let delta = node.confirmed(dimension);
    match status {
        Status::Added => {
            totals.added += delta as u128;
            totals.added_count += 1;
        }
        Status::Removed => {
            totals.removed += delta.unsigned_abs();
            totals.removed_count += 1;
        }
        Status::Grew => {
            totals.grown += delta as u128;
            totals.grew_count += 1;
        }
        Status::Shrank => {
            totals.shrunk += delta.unsigned_abs();
            totals.shrank_count += 1;
        }
        Status::SameSize => totals.same_count += 1,
        Status::Unknown => totals.unknown_count += 1,
        Status::TypeChanged => {
            totals.removed += u128::from(o.map_or(0, |m| m.known));
            totals.added += u128::from(n.map_or(0, |m| m.known));
            totals.type_changed_count += 1;
        }
    }
    if matches!(
        status,
        Status::Added | Status::Removed | Status::TypeChanged
    ) && node.unknown(dimension) > 0
    {
        totals.lower_bound_count += 1;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeSort {
    Growth,
    Decrease,
    Name,
}

impl Comparison {
    pub fn totals(&self, dimension: Dimension) -> Totals {
        self.totals[dimension as usize]
    }
    pub fn root(&self) -> &DiffNode {
        &self.nodes[0]
    }
    pub fn old_node(&self, index: usize) -> Option<&crate::scan::Node> {
        self.nodes[index]
            .old
            .map(|i| &self.baseline.nodes[i as usize])
    }
    pub fn new_node(&self, index: usize) -> Option<&crate::scan::Node> {
        self.nodes[index]
            .new
            .map(|i| &self.current.nodes[i as usize])
    }
    /// Name as shown: from the current result when present, else from the baseline.
    pub fn name(&self, index: usize) -> &str {
        self.new_node(index)
            .or_else(|| self.old_node(index))
            .map_or("", |node| node.name.as_str())
    }
    /// Kinds before and after, when present.
    pub fn kinds(&self, index: usize) -> (Option<Kind>, Option<Kind>) {
        (
            self.old_node(index).map(|n| n.kind),
            self.new_node(index).map(|n| n.kind),
        )
    }
    pub fn sizes(&self, index: usize, dimension: Dimension) -> (Option<Measure>, Option<Measure>) {
        (
            measure(&self.baseline, self.nodes[index].old, dimension),
            measure(&self.current, self.nodes[index].new, dimension),
        )
    }
    /// Names from the compared root to this row, for display only.
    pub fn path(&self, index: usize) -> String {
        let mut names = Vec::new();
        let mut at = Some(index as u32);
        while let Some(i) = at {
            let node = &self.nodes[i as usize];
            if node.parent.is_some() {
                names.push(self.name(i as usize));
            }
            at = node.parent;
        }
        names.reverse();
        names.join("/")
    }
    /// The status shown for an event row: a directory that is only an event
    /// because a listing was incomplete reports Unknown, not its subtree status.
    pub fn event_status(&self, index: usize, dimension: Dimension) -> Status {
        let node = &self.nodes[index];
        if node.form == Form::Container {
            Status::Unknown
        } else {
            node.status(dimension)
        }
    }
    /// Event rows matching a status filter and a case-insensitive name query.
    pub fn events(
        &self,
        dimension: Dimension,
        statuses: u8,
        query: &str,
        sort: ChangeSort,
    ) -> Vec<u32> {
        let query = query.to_lowercase();
        let mut rows: Vec<u32> = (0..self.nodes.len() as u32)
            .filter(|&i| {
                let node = &self.nodes[i as usize];
                node.is_event()
                    && self.event_status(i as usize, dimension).bit() & statuses != 0
                    && (query.is_empty() || self.name(i as usize).to_lowercase().contains(&query))
            })
            .collect();
        let amount = |i: &u32| {
            if self.nodes[*i as usize].form == Form::Container {
                0
            } else {
                self.nodes[*i as usize].confirmed(dimension)
            }
        };
        match sort {
            ChangeSort::Growth => rows.sort_by(|a, b| amount(b).cmp(&amount(a)).then(a.cmp(b))),
            ChangeSort::Decrease => rows.sort_by(|a, b| amount(a).cmp(&amount(b)).then(a.cmp(b))),
            ChangeSort::Name => rows.sort_by(|a, b| {
                self.name(*a as usize)
                    .to_lowercase()
                    .cmp(&self.name(*b as usize).to_lowercase())
                    .then(a.cmp(b))
            }),
        }
        rows
    }
    /// Direct children of a row that should be listed for a status filter, sorted
    /// like `events`. Containers appear when any event below them matches.
    pub fn tree_children(
        &self,
        index: usize,
        dimension: Dimension,
        statuses: u8,
        query: &str,
        sort: ChangeSort,
    ) -> Vec<u32> {
        let query = query.to_lowercase();
        let mut rows: Vec<u32> = self.nodes[index]
            .children()
            .map(|i| i as u32)
            .filter(|&i| {
                let node = &self.nodes[i as usize];
                node.mask(dimension) & statuses != 0
                    && (query.is_empty()
                        || self.name(i as usize).to_lowercase().contains(&query)
                        || node.is_container())
            })
            .collect();
        let amount = |i: &u32| self.nodes[*i as usize].confirmed(dimension);
        match sort {
            ChangeSort::Growth => rows.sort_by(|a, b| amount(b).cmp(&amount(a)).then(a.cmp(b))),
            ChangeSort::Decrease => rows.sort_by(|a, b| amount(a).cmp(&amount(b)).then(a.cmp(b))),
            ChangeSort::Name => rows.sort_by(|a, b| {
                self.name(*a as usize)
                    .to_lowercase()
                    .cmp(&self.name(*b as usize).to_lowercase())
                    .then(a.cmp(b))
            }),
        }
        rows
    }
    /// Event counts and amounts inside one row's subtree, computed on demand.
    pub fn subtree_totals(&self, index: usize, dimension: Dimension) -> Totals {
        let mut stack = vec![index];
        let mut totals = Totals::default();
        while let Some(i) = stack.pop() {
            let node = &self.nodes[i];
            add_event(&mut totals, node, &self.baseline, &self.current, dimension);
            if node.form == Form::Container {
                stack.extend(node.children());
            }
        }
        totals
    }
}
