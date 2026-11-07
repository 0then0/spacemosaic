//! Saved results and change comparison. Opening a saved result reads only that
//! file: nothing recorded in it can start a scan, open a location or reveal an
//! object. Comparison is an explicit choice of the result that is *Before* and
//! the one that is *After*; either may be the current scan or one of two opened
//! files.
use super::*;
use crate::{
    compare::{
        self, ChangeSort, CompareError, Comparison, Dimension, Limitation, Reason, ScopeVerdict,
        Side, Status,
    },
    import::ImportError,
    job,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum View {
    /// The result of a scan started in this session.
    Live,
    /// The first result opened from a file.
    Saved,
    /// The second result opened from a file.
    Second,
    Changes,
}
impl View {
    /// The views that show a result opened from a file.
    pub(super) fn is_snapshot(self) -> bool {
        matches!(self, Self::Saved | Self::Second)
    }
}

pub(super) struct SavedResult {
    pub result: Arc<ScanResult>,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChangeMode {
    List,
    Tree,
}

pub(super) enum CompareFailure {
    Incompatible(compare::ScopeAssessment),
    Other(&'static str),
}

/// Where an opened file goes. The slot is chosen when the file arrives, from
/// what is open then, so a request made earlier cannot overwrite a newer choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OpenTarget {
    /// Any free slot, otherwise the one on screen.
    Auto,
    /// The result for this side of the comparison; the other side is untouched.
    Side(Side),
    /// The file goes opposite `View` (the snapshot that was on screen), which
    /// takes the other side; then the comparison screen is shown.
    Compare(Side, View),
}

pub(super) struct Changes {
    /// What the comparison starts from and what it ends at.
    pub baseline: View,
    pub current: View,
    pub confirm_scope: bool,
    pub comparison: Option<Arc<Comparison>>,
    pub job: Option<job::CompareJob>,
    pub failure: Option<CompareFailure>,
    pub statuses: u8,
    pub query: String,
    pub sort: ChangeSort,
    pub mode: ChangeMode,
    pub rows: Vec<u32>,
    pub tree: Vec<(u32, usize)>,
    pub expanded: HashSet<u32>,
    pub selected: Option<u32>,
    pub dirty: bool,
}
impl Default for Changes {
    fn default() -> Self {
        Self {
            baseline: View::Saved,
            current: View::Live,
            confirm_scope: false,
            comparison: None,
            job: None,
            failure: None,
            statuses: Status::CHANGED,
            query: String::new(),
            sort: ChangeSort::Growth,
            mode: ChangeMode::List,
            rows: Vec::new(),
            tree: Vec::new(),
            expanded: HashSet::new(),
            selected: None,
            dirty: true,
        }
    }
}
impl Changes {
    /// Forget any comparison; the operands changed or were closed. The choice of
    /// which result is Before and which is After is kept.
    pub fn clear(&mut self) {
        let (baseline, current) = (self.baseline, self.current);
        *self = Self {
            baseline,
            current,
            ..Self::default()
        };
    }
    fn operand(&self, side: Side) -> View {
        match side {
            Side::Baseline => self.baseline,
            Side::Current => self.current,
        }
    }
    fn set(&mut self, side: Side, view: View) {
        match side {
            Side::Baseline => self.baseline = view,
            Side::Current => self.current = view,
        }
    }
}

#[derive(Clone, Copy)]
struct RowLayout {
    depth: usize,
    tree: bool,
    wide: bool,
    height: f32,
}

impl ExplorerApp {
    pub(super) fn dimension(&self) -> Dimension {
        if self.allocated {
            Dimension::Allocated
        } else {
            Dimension::Logical
        }
    }

    // ----- Operands -----

    pub(super) fn snapshot(&self, view: View) -> Option<&SavedResult> {
        match view {
            View::Saved => self.saved.as_ref(),
            View::Second => self.second.as_ref(),
            _ => None,
        }
    }
    fn snapshot_slot(&mut self, view: View) -> &mut Option<SavedResult> {
        match view {
            View::Second => &mut self.second,
            _ => &mut self.saved,
        }
    }
    /// Whether any file is open, so the saved-result controls apply.
    pub(super) fn has_snapshot(&self) -> bool {
        self.saved.is_some() || self.second.is_some()
    }
    /// The result a view or comparison side stands for, when it exists now.
    pub(super) fn operand(&self, view: View) -> Option<Arc<ScanResult>> {
        match view {
            View::Live => self.live.clone(),
            View::Saved | View::Second => self.snapshot(view).map(|s| s.result.clone()),
            View::Changes => None,
        }
    }
    fn operand_name(&self, view: View) -> String {
        match view {
            View::Live => self
                .live
                .as_ref()
                .map(|live| live.nodes[0].name.clone())
                .unwrap_or_default(),
            _ => self
                .snapshot(view)
                .map(|s| s.name.clone())
                .unwrap_or_default(),
        }
    }
    fn operands(&self) -> Option<(Arc<ScanResult>, Arc<ScanResult>)> {
        let (baseline, current) = (self.changes.baseline, self.changes.current);
        if baseline == current {
            return None;
        }
        Some((self.operand(baseline)?, self.operand(current)?))
    }
    fn available_operands(&self) -> Vec<View> {
        [View::Saved, View::Second, View::Live]
            .into_iter()
            .filter(|&view| self.operand(view).is_some())
            .collect()
    }
    /// Keep the chosen Before and After if both still exist. Otherwise keep
    /// whichever survives and fill the other; two opened files are then ordered by
    /// capture time, earlier as Before. A choice is made only when the previous
    /// one became impossible, and it is always shown and can be reversed.
    pub(super) fn settle_operands(&mut self) {
        let (baseline, current) = (self.changes.baseline, self.changes.current);
        let present = self.available_operands();
        let valid = |view: View| present.contains(&view);
        if baseline != current && valid(baseline) && valid(current) {
            return;
        }
        let spare = |keep: View| present.iter().copied().find(|&view| view != keep);
        let captured = |view: View| self.snapshot(view).map(|s| s.result.started_unix_seconds);
        let ordered = |first: View, second: View| {
            if first.is_snapshot()
                && second.is_snapshot()
                && captured(second)
                    .zip(captured(first))
                    .is_some_and(|(b, a)| b < a)
            {
                (second, first)
            } else {
                (first, second)
            }
        };
        let (baseline, current) = match (valid(baseline), valid(current)) {
            (true, _) if spare(baseline).is_some() => {
                ordered(baseline, spare(baseline).unwrap_or(current))
            }
            (_, true) if spare(current).is_some() => {
                ordered(spare(current).unwrap_or(baseline), current)
            }
            _ => match (self.saved.is_some(), self.second.is_some()) {
                (true, true) => ordered(View::Saved, View::Second),
                (true, false) => (View::Saved, View::Live),
                (false, true) => (View::Second, View::Live),
                (false, false) => (baseline, current),
            },
        };
        self.changes.baseline = baseline;
        self.changes.current = current;
        self.changes.clear();
    }
    /// Estimated bytes the session retains: every distinct open result, the
    /// indexes over the one on screen and any finished comparison. Memory work on
    /// saved results is admitted against this together with the new allocation.
    pub(super) fn held_bytes(&self) -> usize {
        let mut seen: Vec<*const ScanResult> = Vec::new();
        let mut total = 0_usize;
        for result in [
            self.live.as_ref(),
            self.saved.as_ref().map(|s| &s.result),
            self.second.as_ref().map(|s| &s.result),
            self.result.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            let identity = Arc::as_ptr(result);
            if !seen.contains(&identity) {
                seen.push(identity);
                total = total.saturating_add(result.retained_bytes());
            }
        }
        if let Some(result) = &self.result {
            total = total.saturating_add(crate::import::index_bytes(result.nodes.len()));
        }
        if let Some(comparison) = &self.changes.comparison {
            total = total.saturating_add(comparison.retained_bytes());
        }
        total
    }
    /// The current scan was discarded or is being replaced. Only a comparison
    /// that used it is forgotten; one between two opened files stays valid.
    pub(super) fn live_changed(&mut self) {
        if self.changes.baseline == View::Live || self.changes.current == View::Live {
            self.changes.clear();
        }
    }
    /// Replace the browsed result and reset navigation, as a new scan does.
    pub(super) fn set_displayed(&mut self, result: Option<Arc<ScanResult>>) {
        self.extensions = result.as_ref().map(|r| r.extensions()).unwrap_or_default();
        self.sort_extensions();
        if result
            .as_ref()
            .is_some_and(|r| r.nodes[0].allocated.known == 0 && r.nodes[0].allocated.unknown > 0)
        {
            self.allocated = false;
        }
        self.result = result;
        self.navigation.selected = None;
        self.navigation.directory = 0;
        self.rows.clear();
        self.tree = tree::TreeState::default();
        self.search.clear();
        self.map.group = None;
        self.navigation.page = Page::Directory;
        self.phone_overlay = None;
        self.dirty = true;
        self.sync_native_result();
    }
    pub(super) fn show(&mut self, view: View) {
        if self.view == view {
            return;
        }
        self.view = view;
        match view {
            View::Live => {
                let live = self.live.clone();
                self.set_displayed(live);
            }
            View::Saved | View::Second => {
                let saved = self.operand(view);
                self.set_displayed(saved);
            }
            View::Changes => self.sync_native_result(),
        }
        self.phone_home = false;
    }
    /// Keep the native list in step with what the canvas shows, flagged as a saved
    /// snapshot so native actions never use its recorded locations.
    pub(super) fn sync_native_result(&self) {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            if let Some(result) = &self.result {
                crate::mobile::publish_view(
                    bridge::generation(),
                    result.clone(),
                    self.shows_saved_result(),
                    self.changes.comparison.clone(),
                );
            } else {
                crate::mobile::clear_view(bridge::generation());
            }
            crate::mobile::publish_setup(bridge::generation(), self.native_setup());
        }
    }
    /// The facts the native comparison page shows about the chosen Before and After.
    #[cfg(any(target_os = "android", target_os = "ios", test))]
    pub(super) fn native_setup(&self) -> crate::mobile::Setup {
        use crate::mobile::{Setup, SetupFailure, SetupOperand};
        let operand = |view: View| {
            self.operand(view).map(|result| SetupOperand {
                name: self.operand_name(view),
                live: view == View::Live,
                captured: result.started_unix_seconds,
                objects: result.nodes.len() as u64,
                partial: compare::is_partial(&result),
            })
        };
        Setup {
            before: operand(self.changes.baseline),
            after: operand(self.changes.current),
            assessment: self
                .operands()
                .map(|(baseline, current)| compare::assess_scope(&baseline, &current)),
            confirmed: self.changes.confirm_scope,
            comparing: self.changes.job.is_some(),
            opening: self.open_job.is_some(),
            failure: self.changes.failure.as_ref().map(|failure| match failure {
                CompareFailure::Incompatible(scope) => SetupFailure::Incompatible(scope.clone()),
                CompareFailure::Other(key) => SetupFailure::Other(key),
            }),
            finished: self.changes.comparison.is_some(),
        }
    }
    /// Run a command from the native comparison page.
    #[cfg(any(target_os = "android", target_os = "ios", test))]
    pub(super) fn native_compare_command(&mut self, command: crate::mobile::Command) {
        use crate::mobile::Command;
        match command {
            Command::OpenFor(side) if self.open_job.is_none() => {
                self.open_saved_dialog_as(OpenTarget::Side(side));
            }
            Command::SwapDirection => self.swap_direction(),
            Command::ConfirmScope(confirmed) => {
                self.changes.confirm_scope = confirmed;
                self.sync_native_result();
            }
            Command::StartCompare
                if self.changes.job.is_none()
                    && self.open_job.is_none()
                    && self.changes.comparison.is_none() =>
            {
                self.start_compare();
            }
            Command::ShowSide(side) => {
                let view = self.side_view(side);
                if self.operand(view).is_some() {
                    self.show(view);
                    if self.view == View::Changes {
                        self.view = view;
                    }
                }
            }
            _ => {}
        }
    }
    /// Whether the browsed result came from a file, whichever screen is on top. The
    /// comparison screen keeps the previously browsed result, so the view alone is not enough.
    #[cfg(any(target_os = "android", target_os = "ios", test))]
    pub(super) fn shows_saved_result(&self) -> bool {
        self.result.as_ref().is_some_and(|result| {
            [self.saved.as_ref(), self.second.as_ref()]
                .into_iter()
                .flatten()
                .any(|saved| Arc::ptr_eq(&saved.result, result))
        })
    }
    pub(super) fn open_saved_dialog(&mut self) {
        self.open_saved_dialog_as(OpenTarget::Auto);
    }
    /// Ask for a file; where it goes is decided when it has been read.
    pub(super) fn open_saved_dialog_as(&mut self, target: OpenTarget) {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .pick_file()
        {
            self.open_saved_as(path, false, target);
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            self.native_open_target = target;
            self.native_open = crate::mobile::request_open();
            if self.native_open.is_none() {
                self.notice = Some("unavailable");
            }
        }
    }
    /// Start reading one chosen file. A newer request supersedes an older one: the
    /// older job is cancelled and can no longer deliver a result.
    pub(super) fn open_saved(&mut self, path: PathBuf, delete_after: bool) {
        self.open_saved_as(path, delete_after, OpenTarget::Auto);
    }
    pub(super) fn open_saved_as(&mut self, path: PathBuf, delete_after: bool, target: OpenTarget) {
        // A request from the person supersedes anything still waiting its turn.
        self.open_queue.clear();
        self.begin_open(path, delete_after, target, true);
    }
    /// Several files given at once (command line, drop): they are read one after
    /// another into free slots instead of each cancelling the one before.
    pub fn open_results(&mut self, mut paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let first = paths.remove(0);
        self.open_saved_as(first, false, OpenTarget::Auto);
        self.open_queue = paths.into();
    }
    fn begin_open(
        &mut self,
        path: PathBuf,
        delete_after: bool,
        target: OpenTarget,
        reset_error: bool,
    ) {
        // Drop any queued result before the next worker samples available memory.
        drop(self.open_job.take());
        // A comparison in flight describes operands that this file may replace, and
        // would otherwise compete with the import for the same memory.
        self.changes.job = None;
        if reset_error {
            self.open_error = None;
        }
        self.open_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.open_target = target;
        let held = self.held_bytes();
        self.open_job = Some(self.spawn_open(path, delete_after, held));
    }
    fn spawn_open(&self, path: PathBuf, delete_after: bool, held: usize) -> job::OpenJob {
        #[cfg(test)]
        if let Some(total) = self.memory_budget {
            let limits = crate::import::Limits {
                memory_limit: total.saturating_sub(held),
                ..crate::import::Limits::platform()
            };
            return job::open(path, limits, delete_after);
        }
        job::open_local(path, delete_after, held)
    }
    fn spawn_compare(
        &self,
        baseline: Arc<ScanResult>,
        current: Arc<ScanResult>,
        held: usize,
    ) -> job::CompareJob {
        let confirm = self.changes.confirm_scope;
        #[cfg(test)]
        if let Some(total) = self.memory_budget {
            return job::compare(baseline, current, confirm, total.saturating_sub(held));
        }
        job::compare_local(baseline, current, confirm, held)
    }
    /// The slot a file read for `target` is stored in.
    fn slot_for(&self, target: OpenTarget) -> View {
        let free = [View::Saved, View::Second]
            .into_iter()
            .find(|&view| self.snapshot(view).is_none());
        match target {
            OpenTarget::Side(side) => {
                let own = self.changes.operand(side);
                if own.is_snapshot() {
                    own
                } else if let Some(free) = free {
                    free
                } else if self.changes.operand(side.other()) == View::Saved {
                    View::Second
                } else {
                    View::Saved
                }
            }
            // Never over the snapshot it is compared with.
            OpenTarget::Compare(_, anchor) => {
                if anchor == View::Saved {
                    View::Second
                } else {
                    View::Saved
                }
            }
            OpenTarget::Auto => free.unwrap_or(if self.view.is_snapshot() {
                self.view
            } else {
                View::Saved
            }),
        }
    }
    /// Close one opened file. Any comparison that used it is forgotten.
    pub(super) fn close_saved(&mut self, view: View) {
        if !view.is_snapshot() {
            return;
        }
        let closed = self.snapshot_slot(view).take();
        let displayed = closed.as_ref().is_some_and(|closed| {
            self.result
                .as_ref()
                .is_some_and(|shown| Arc::ptr_eq(shown, &closed.result))
        });
        self.settle_operands();
        self.changes.clear();
        if displayed || self.view == view {
            let next = self.fallback_view();
            let shown = self.operand(next);
            if self.view == View::Changes && shown.is_some() {
                // Stay on the comparison screen, but do not keep the closed result alive.
            } else {
                self.view = next;
            }
            self.set_displayed(shown);
            self.phone_home = false;
        }
        self.sync_native_result();
    }
    /// What to browse when the comparison screen is left or a result goes away.
    fn fallback_view(&self) -> View {
        if self.live.is_some() {
            View::Live
        } else if self.saved.is_some() {
            View::Saved
        } else if self.second.is_some() {
            View::Second
        } else {
            View::Live
        }
    }
    pub(super) fn leave_changes(&mut self) {
        let target = self.fallback_view();
        self.show(target);
        if self.view == View::Changes {
            self.view = target;
        }
    }
    pub(super) fn poll_saved(&mut self) {
        if let Some(job) = &self.open_job
            && let Some(outcome) = job.take()
        {
            self.open_job = None;
            match outcome {
                Ok(result) => self.store_opened(result),
                Err(ImportError::Cancelled) => {}
                Err(error) => self.show_open_error(error),
            }
            if let Some(next) = self.open_queue.pop_front() {
                self.begin_open(next, false, OpenTarget::Auto, false);
            }
        }
        if let Some(job) = &self.changes.job
            && let Some(outcome) = job.take()
        {
            self.changes.job = None;
            match outcome {
                // Only a diff of the results chosen now may be shown.
                Ok(comparison) if self.matches_operands(&comparison) => {
                    self.changes.comparison = Some(Arc::new(comparison));
                    self.changes.dirty = true;
                    self.changes.selected = None;
                    self.changes.expanded = HashSet::from([0]);
                    self.sync_native_result();
                }
                Ok(_) => {}
                Err(CompareError::Incompatible(scope)) => {
                    self.changes.failure = Some(CompareFailure::Incompatible(scope));
                }
                Err(CompareError::NeedsConfirmation(_)) => {
                    self.changes.failure = Some(CompareFailure::Other("cmp_needs_confirmation"));
                }
                Err(CompareError::TooLarge) => {
                    self.changes.failure = Some(CompareFailure::Other("import_error_too_large"));
                }
                Err(CompareError::MemoryLimit) => {
                    self.changes.failure = Some(CompareFailure::Other("compare_memory_limit"));
                }
                Err(CompareError::Cancelled) => {}
            }
            self.sync_native_result();
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        if let Some(receiver) = &self.native_open
            && let Ok(outcome) = receiver.try_recv()
        {
            self.native_open = None;
            match outcome {
                crate::mobile::OpenOutcome::Ready(path) => {
                    self.open_saved_as(path, true, self.native_open_target)
                }
                crate::mobile::OpenOutcome::Cancelled => {}
                crate::mobile::OpenOutcome::TooLarge => {
                    self.show_open_error(ImportError::TooLarge {
                        limit: crate::import::Limits::platform().max_bytes,
                    });
                }
                crate::mobile::OpenOutcome::Failed => {
                    self.show_open_error(ImportError::Io(std::io::ErrorKind::Other));
                }
            }
        }
    }
    fn matches_operands(&self, comparison: &Comparison) -> bool {
        self.operands().is_some_and(|(baseline, current)| {
            Arc::ptr_eq(&baseline, &comparison.baseline)
                && Arc::ptr_eq(&current, &comparison.current)
        })
    }
    /// Keep a file that finished reading. Replacing a result resets the diff and
    /// the confirmation made for the previous pair.
    fn store_opened(&mut self, result: ScanResult) {
        let target = self.open_target;
        let slot = self.slot_for(target);
        let name = std::mem::take(&mut self.open_name);
        let replaced = self.snapshot(slot).map(|s| s.result.clone());
        *self.snapshot_slot(slot) = Some(SavedResult {
            result: Arc::new(result),
            name,
        });
        match target {
            OpenTarget::Side(side) => self.changes.set(side, slot),
            OpenTarget::Compare(side, anchor) => {
                self.changes.set(side, slot);
                self.changes.set(side.other(), anchor);
            }
            OpenTarget::Auto => {}
        }
        self.settle_operands();
        self.changes.clear();
        self.phone_home = false;
        let shown = self.operand(slot);
        // A result that was on screen must not stay alive behind the new one.
        let was_shown = replaced.is_some_and(|old| {
            self.result
                .as_ref()
                .is_some_and(|result| Arc::ptr_eq(result, &old))
        });
        match target {
            OpenTarget::Compare(..) => {
                self.view = slot;
                self.set_displayed(shown);
                self.open_changes();
            }
            OpenTarget::Side(_) if self.view == View::Changes => {
                if was_shown {
                    self.set_displayed(shown);
                } else {
                    self.sync_native_result();
                }
            }
            _ => {
                self.view = slot;
                self.set_displayed(shown);
            }
        }
    }
    fn show_open_error(&mut self, error: ImportError) {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let t = |key| i18n::text(self.language, key);
            let mut message = format!(
                "{}\n{}\n{}",
                t("import_failed"),
                t(error.key()),
                error.detail()
            );
            if self.has_snapshot() || self.live.is_some() {
                message.push('\n');
                message.push_str(t("import_kept_open"));
            }
            let _ = crate::mobile::request(serde_json::json!({
                "op": "saved_open_error", "message": message,
            }));
        }
        self.open_error = Some(error);
    }
    pub(super) fn start_compare(&mut self) {
        let Some((baseline, current)) = self.operands() else {
            return;
        };
        self.changes.comparison = None;
        self.changes.failure = None;
        let held = self.held_bytes();
        self.changes.job = Some(self.spawn_compare(baseline, current, held));
        self.sync_native_result();
    }
    /// Forget the diff, the confirmation and the selected row; the operands changed.
    fn reset_diff(&mut self) {
        self.changes.confirm_scope = false;
        self.changes.comparison = None;
        self.changes.job = None;
        self.changes.failure = None;
        self.changes.selected = None;
        self.changes.dirty = true;
        self.sync_native_result();
    }
    pub(super) fn swap_direction(&mut self) {
        std::mem::swap(&mut self.changes.baseline, &mut self.changes.current);
        self.reset_diff();
    }
    /// Choose which result stands on one side. Picking the one on the other side
    /// reverses the direction instead.
    pub(super) fn set_operand(&mut self, side: Side, view: View) {
        if self.changes.operand(side) == view || self.operand(view).is_none() {
            return;
        }
        if self.changes.operand(side.other()) == view {
            self.swap_direction();
            return;
        }
        self.changes.set(side, view);
        self.reset_diff();
    }
    pub(super) fn open_changes(&mut self) {
        self.show(View::Changes);
        self.phone_overlay = None;
    }
    /// Compare the result on screen with the current scan.
    pub(super) fn compare_with_current(&mut self, view: View) {
        if view.is_snapshot() && self.live.is_some() {
            self.changes.baseline = view;
            self.changes.current = View::Live;
            self.reset_diff();
        }
        self.open_changes();
    }
    fn side_view(&self, side: Side) -> View {
        self.changes.operand(side)
    }
    /// Show the object a change row refers to in the result it came from.
    pub(super) fn goto(&mut self, side: Side, node: u32) {
        let view = self.side_view(side);
        self.show(view);
        if self.view != view {
            self.view = view;
        }
        let Some(result) = self.result.clone() else {
            return;
        };
        let index = node as usize;
        if index >= result.nodes.len() {
            return;
        }
        let parent = result.nodes[index].parent.unwrap_or(0);
        self.navigate(parent);
        self.select(index);
        self.phone_overlay = None;
    }

    // ----- Presentation -----

    fn captured(&self, result: &ScanResult) -> String {
        i18n::timestamp(result.started_unix_seconds)
    }
    fn describe(&self, result: &ScanResult) -> String {
        let t = |key| i18n::text(self.language, key);
        format!(
            "{} · {} · {}",
            self.captured(result),
            i18n::counted(self.language, result.nodes.len() as u64, "object"),
            t(if compare::is_partial(result) {
                "partial"
            } else {
                "complete"
            })
        )
    }
    /// File name for an opened result, scan root name for the current scan, with
    /// the capture time: what identifies one side of a comparison.
    fn operand_line(&self, view: View) -> String {
        let t = |key| i18n::text(self.language, key);
        match self.operand(view) {
            Some(result) => {
                let name = self.operand_name(view);
                let label = if view == View::Live {
                    t("view_current")
                } else {
                    t("saved_file")
                };
                format!("{label}: {name} · {}", self.captured(&result))
            }
            None => t("compare_not_chosen").to_owned(),
        }
    }
    pub(super) fn open_status(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        if let Some(job) = &self.open_job {
            let read = job.progress.load(std::sync::atomic::Ordering::Relaxed);
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label(t("opening_saved"));
                ui.label(&self.open_name);
                ui.label(i18n::bytes(language, read));
                if ui.button(t("cancel")).clicked() {
                    self.open_job = None;
                    self.open_queue.clear();
                }
            });
        }
        if let Some(error) = self.open_error.clone() {
            let kept = self.has_snapshot() || self.live.is_some();
            egui::Frame::new()
                .fill(theme::surface(ui.visuals().dark_mode))
                .corner_radius(8)
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(RichText::new(t("import_failed")).strong());
                    ui.label(t(error.key()));
                    ui.label(RichText::new(error.detail()).small().weak());
                    if kept {
                        ui.label(t("import_kept_open"));
                    }
                    if ui.button(t("close")).clicked() {
                        self.open_error = None;
                    }
                });
        }
    }
    /// Unmistakable banner: the result on screen is an older snapshot.
    pub(super) fn saved_banner(&mut self, ui: &mut egui::Ui) {
        let view = self.view;
        let Some(saved) = self.snapshot(view).filter(|_| view.is_snapshot()) else {
            return;
        };
        let (result, name) = (saved.result.clone(), saved.name.clone());
        let language = self.language;
        let t = |key| i18n::text(language, key);
        let dark = ui.visuals().dark_mode;
        egui::Frame::new()
            .fill(theme::surface(dark))
            .stroke(egui::Stroke::new(1.5, theme::accent(dark)))
            .corner_radius(8)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(
                    RichText::new(t("saved_banner"))
                        .strong()
                        .color(theme::accent(dark)),
                );
                ui.label(format!(
                    "{}: {}",
                    t("saved_captured"),
                    self.captured(&result)
                ));
                ui.label(format!("{}: {}", t("saved_file"), name));
                ui.label(t("saved_snapshot_note"));
                if result.schema_version < 2 {
                    ui.label(RichText::new(t("saved_schema_one")).small());
                }
                ui.horizontal_wrapped(|ui| {
                    if self.live.is_some() && ui.button(t("compare_with_current")).clicked() {
                        self.compare_with_current(view);
                    }
                    if ui
                        .add_enabled(
                            self.open_job.is_none(),
                            egui::Button::new(t("compare_with_file")),
                        )
                        .clicked()
                    {
                        // This file is Before; the chosen file becomes After once it is read.
                        self.open_saved_dialog_as(OpenTarget::Compare(Side::Current, view));
                    }
                    if ui.button(t("saved_close")).clicked() {
                        self.close_saved(view);
                    }
                });
                if self.live.is_none() {
                    ui.label(RichText::new(t("compare_needs_scan")).small().weak());
                }
            });
        ui.add_space(8.0);
    }
    pub(super) fn view_tabs(&mut self, ui: &mut egui::Ui) {
        if !self.has_snapshot() {
            return;
        }
        let language = self.language;
        let t = |key| i18n::text(language, key);
        let both = self.saved.is_some() && self.second.is_some();
        ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            let mut next = None;
            let tabs = [
                (
                    View::Live,
                    t("view_current").to_owned(),
                    self.live.is_some() || self.job.is_some(),
                ),
                (
                    View::Saved,
                    if both {
                        self.operand_name(View::Saved)
                    } else {
                        t("view_saved").to_owned()
                    },
                    self.saved.is_some(),
                ),
                (
                    View::Second,
                    self.operand_name(View::Second),
                    self.second.is_some(),
                ),
                (View::Changes, t("view_changes").to_owned(), true),
            ];
            for (view, label, enabled) in tabs {
                if view == View::Second && !enabled {
                    continue;
                }
                if ui
                    .add_enabled(
                        enabled,
                        egui::Button::new(label).selected(self.view == view),
                    )
                    .clicked()
                {
                    next = Some(view);
                }
            }
            match next {
                Some(View::Changes) => self.open_changes(),
                Some(view) => self.show(view),
                None => {}
            }
        });
    }

    /// One side of the comparison: what it is, when it was captured, and the
    /// choice of which open result stands here.
    fn side_card(&mut self, ui: &mut egui::Ui, side: Side) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        let dark = ui.visuals().dark_mode;
        let view = self.changes.operand(side);
        let title = t(match side {
            Side::Baseline => "compare_baseline",
            Side::Current => "compare_current",
        });
        let options: Vec<(View, String)> = self
            .available_operands()
            .into_iter()
            .map(|option| {
                let label = if option == View::Live {
                    t("view_current").to_owned()
                } else {
                    self.operand_name(option)
                };
                (option, label)
            })
            .collect();
        let mut chosen = None;
        let mut open = false;
        egui::Frame::new()
            .fill(theme::surface(dark))
            .corner_radius(8)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_min_width((ui.available_width()).max(120.0));
                ui.label(RichText::new(title).small().color(theme::accent(dark)));
                match self.operand(view) {
                    Some(result) => {
                        ui.label(
                            RichText::new(if view == View::Live {
                                t("view_current")
                            } else {
                                t("saved_file")
                            })
                            .strong(),
                        );
                        ui.label(self.operand_name(view));
                        ui.label(self.describe(&result));
                        if result.schema_version < 2 {
                            ui.label(RichText::new(t("saved_schema_one")).small());
                        }
                    }
                    None => {
                        ui.label(RichText::new(t("compare_not_chosen")).strong());
                    }
                }
                let source = ui.label(RichText::new(t("compare_source")).small());
                let selected = options
                    .iter()
                    .find(|(option, _)| *option == view)
                    .map_or_else(|| t("compare_not_chosen").to_owned(), |(_, l)| l.clone());
                let combo =
                    egui::ComboBox::from_id_salt(("compare-source", side == Side::Baseline))
                        .selected_text(selected)
                        .width(ui.available_width().min(320.0))
                        .show_ui(ui, |ui| {
                            for (option, label) in &options {
                                if ui.selectable_label(*option == view, label).clicked() {
                                    chosen = Some(*option);
                                }
                            }
                        });
                combo.response.labelled_by(source.id);
                open = ui
                    .add_enabled(
                        self.open_job.is_none(),
                        egui::Button::new(t(match side {
                            Side::Baseline => "compare_open_before",
                            Side::Current => "compare_open_after",
                        }))
                        .wrap(),
                    )
                    .clicked();
            });
        if let Some(option) = chosen {
            self.set_operand(side, option);
        }
        if open {
            self.open_saved_dialog_as(OpenTarget::Side(side));
        }
    }

    fn scope_text(&self, assessment: &compare::ScopeAssessment) -> String {
        let t = |key| i18n::text(self.language, key);
        assessment
            .notes
            .iter()
            .map(|note| t(note.key()))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn compare_failure(&self, ui: &mut egui::Ui) {
        let t = |key| i18n::text(self.language, key);
        let dark = ui.visuals().dark_mode;
        let frame = |ui: &mut egui::Ui, add: &dyn Fn(&mut egui::Ui)| {
            egui::Frame::new()
                .fill(theme::surface(dark))
                .stroke(egui::Stroke::new(1.0, theme::accent(dark)))
                .corner_radius(8)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    add(ui);
                });
        };
        match &self.changes.failure {
            Some(CompareFailure::Incompatible(scope)) => frame(ui, &|ui| {
                ui.label(t("cmp_verdict_incompatible"));
                ui.label(RichText::new(self.scope_text(scope)).small());
            }),
            Some(CompareFailure::Other(key)) => frame(ui, &|ui| {
                ui.label(t(key));
                if *key == "compare_memory_limit" {
                    ui.label(t("import_kept_open"));
                }
            }),
            None => {}
        }
    }

    /// Explicit choice of Before and After, before comparing.
    fn compare_setup(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        ui.label(RichText::new(t("compare_heading")).size(20.0));
        ui.label(t("compare_direction_help"));
        // The outcome of the last attempt comes first: on a phone the buttons are far below.
        self.compare_failure(ui);
        let wide = ui.available_width() > 560.0;
        let arrow = t("compare_arrow");
        if wide {
            ui.columns(3, |columns| {
                self.side_card(&mut columns[0], Side::Baseline);
                columns[1].vertical_centered(|ui| {
                    ui.add_space(24.0);
                    ui.label(RichText::new(arrow).strong());
                });
                self.side_card(&mut columns[2], Side::Current);
            });
        } else {
            self.side_card(ui, Side::Baseline);
            ui.label(RichText::new(arrow).strong());
            self.side_card(ui, Side::Current);
        }
        let Some((baseline, current)) = self.operands() else {
            ui.label(t(if self.changes.baseline == self.changes.current {
                "compare_same_source"
            } else {
                "compare_needs_two"
            }));
            return;
        };
        if ui.button(t("compare_swap")).clicked() {
            self.swap_direction();
            return;
        }
        let assessment = compare::assess_scope(&baseline, &current);
        ui.add_space(8.0);
        ui.label(
            RichText::new(t("compare_scope"))
                .small()
                .color(theme::accent(ui.visuals().dark_mode)),
        );
        ui.label(t(match assessment.verdict {
            ScopeVerdict::Matching => "cmp_verdict_matching",
            ScopeVerdict::Insufficient => "cmp_verdict_insufficient",
            ScopeVerdict::Incompatible => "cmp_verdict_incompatible",
        }));
        ui.label(RichText::new(self.scope_text(&assessment)).small());
        let mut can_compare = true;
        match assessment.verdict {
            ScopeVerdict::Incompatible => can_compare = false,
            ScopeVerdict::Insufficient => {
                ui.checkbox(&mut self.changes.confirm_scope, t("compare_confirm_scope"));
                can_compare = self.changes.confirm_scope;
            }
            ScopeVerdict::Matching => {}
        }
        for (result, key) in [
            (&baseline, "cmp_limit_baseline_partial"),
            (&current, "cmp_limit_current_partial"),
        ] {
            if compare::is_partial(result) {
                ui.label(RichText::new(t(key)).small());
            }
        }
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    can_compare && self.changes.job.is_none() && self.open_job.is_none(),
                    egui::Button::new(t("compare_start")),
                )
                .clicked()
            {
                self.start_compare();
            }
            if self.changes.job.is_some() {
                ui.spinner();
                ui.label(t("comparing"));
                if ui.button(t("cancel")).clicked() {
                    self.changes.job = None;
                }
            }
        });
    }

    fn refresh_changes(&mut self) {
        let Some(comparison) = self.changes.comparison.clone() else {
            return;
        };
        if !self.changes.dirty {
            return;
        }
        let dimension = self.dimension();
        let changes = &mut self.changes;
        changes.rows = comparison.events(dimension, changes.statuses, &changes.query, changes.sort);
        changes.tree.clear();
        let mut pending = vec![(0_u32, 0_usize)];
        while let Some((index, depth)) = pending.pop() {
            if depth > 0 || index != 0 {
                changes.tree.push((index, depth));
            }
            if index == 0 || changes.expanded.contains(&index) {
                let children = comparison.tree_children(
                    index as usize,
                    dimension,
                    changes.statuses,
                    &changes.query,
                    changes.sort,
                );
                pending.extend(children.into_iter().rev().map(|c| (c, depth + 1)));
            }
        }
        changes.dirty = false;
    }

    fn change_texts(&self, comparison: &Comparison, index: usize) -> (String, String, String) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        let dimension = self.dimension();
        let node = &comparison.nodes[index];
        let (before, now) = comparison.sizes(index, dimension);
        let reason = node.reason(dimension);
        let side = |size: Option<crate::scan::Measure>, unobserved: bool| match size {
            Some(size) => measure(language, size),
            None if unobserved => t("cmp_not_observed").to_owned(),
            None => t("cmp_absent").to_owned(),
        };
        let before = side(before, reason == Reason::BaselineNotObserved);
        let now = side(now, reason == Reason::CurrentNotObserved);
        let status = comparison.event_status(index, dimension);
        let partial = node.unknown(dimension) > 0;
        let change = if node.is_container() {
            let amount = i18n::signed_bytes(language, node.confirmed(dimension));
            if partial {
                format!("{amount} · {}", t("cmp_partial_suffix"))
            } else {
                amount
            }
        } else if status == Status::Unknown {
            t("unknown").to_owned()
        } else {
            let amount = i18n::signed_bytes(language, node.confirmed(dimension));
            if partial {
                format!("{amount} · {}", t("cmp_partial_suffix"))
            } else {
                amount
            }
        };
        (before, now, change)
    }

    fn status_label(&self, comparison: &Comparison, index: usize) -> String {
        let status = if comparison.nodes[index].is_container() {
            comparison.nodes[index].status(self.dimension())
        } else {
            comparison.event_status(index, self.dimension())
        };
        let mut label = format!(
            "{} {}",
            status.symbol(),
            i18n::text(self.language, status.key())
        );
        if comparison.nodes[index].is_container()
            && comparison.nodes[index].unknown(self.dimension()) > 0
        {
            label.push_str(" ·");
            label.push(' ');
            label.push_str(i18n::text(self.language, "cmp_partial_suffix"));
        }
        label
    }

    fn change_row(
        &mut self,
        ui: &mut egui::Ui,
        comparison: &Comparison,
        index: u32,
        layout: RowLayout,
    ) {
        let RowLayout {
            depth,
            tree,
            wide,
            height: row_height,
        } = layout;
        let language = self.language;
        let t = |key| i18n::text(language, key);
        let dark = ui.visuals().dark_mode;
        let node = &comparison.nodes[index as usize];
        let (before, now, change) = self.change_texts(comparison, index as usize);
        let status = self.status_label(comparison, index as usize);
        let name = comparison.name(index as usize).to_owned();
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), row_height),
            egui::Sense::click(),
        );
        let selected = self.changes.selected == Some(index);
        if selected {
            ui.painter()
                .rect_filled(rect, 4.0, theme::accent(dark).gamma_multiply(0.18));
        } else if response.hovered() {
            ui.painter()
                .rect_filled(rect, 4.0, theme::accent(dark).gamma_multiply(0.07));
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect,
                4.0,
                egui::Stroke::new(1.5, ui.visuals().text_color()),
                egui::StrokeKind::Inside,
            );
        }
        let summary = format!("{status}: {name}, {before} → {now}, {change}");
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, summary.clone()));
        let indent = (depth.saturating_sub(1) as f32 * 16.0).min(rect.width() * 0.3);
        let mut left = rect.left() + 4.0 + indent;
        if tree {
            let toggle = egui::Rect::from_min_size(
                egui::pos2(left, rect.top()),
                egui::vec2(24.0, rect.height()),
            );
            if node.is_container() && node.child_count > 0 {
                let open = self.changes.expanded.contains(&index);
                let response = ui.interact(
                    toggle,
                    ui.scope_id().with(("change-toggle", index)),
                    egui::Sense::click(),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Button,
                        true,
                        format!("{}: {}", t(if open { "collapse" } else { "expand" }), name),
                    )
                });
                let center = toggle.center();
                let points = if open {
                    vec![
                        center + egui::vec2(-4.0, -2.0),
                        center + egui::vec2(4.0, -2.0),
                        center + egui::vec2(0.0, 3.0),
                    ]
                } else {
                    vec![
                        center + egui::vec2(-2.0, -4.0),
                        center + egui::vec2(-2.0, 4.0),
                        center + egui::vec2(3.0, 0.0),
                    ]
                };
                ui.painter().add(egui::Shape::convex_polygon(
                    points,
                    ui.visuals().text_color(),
                    egui::Stroke::NONE,
                ));
                if response.clicked() {
                    if open {
                        self.changes.expanded.remove(&index);
                    } else {
                        self.changes.expanded.insert(index);
                    }
                    self.changes.dirty = true;
                }
            }
            left += 26.0;
        }
        let label = |ui: &mut egui::Ui, bounds: egui::Rect, text: RichText| {
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(bounds)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    ui.add(egui::Label::new(text).truncate().selectable(false));
                },
            );
        };
        let right = rect.right() - 4.0;
        if wide {
            let free = (right - left).max(100.0);
            let widths = [
                free * 0.20,
                free * 0.30,
                free * 0.16,
                free * 0.16,
                free * 0.18,
            ];
            let mut x = left;
            let cells = [status, name, before, now, change];
            for (column, (cell, width)) in cells.into_iter().zip(widths).enumerate() {
                let bounds = egui::Rect::from_min_size(
                    egui::pos2(x, rect.top()),
                    egui::vec2((width - 6.0).max(1.0), rect.height()),
                );
                let text = RichText::new(cell);
                label(
                    ui,
                    bounds,
                    if column >= 2 { text.monospace() } else { text },
                );
                x += width;
            }
        } else {
            let half = rect.height() / 2.0;
            let top = egui::Rect::from_min_max(
                egui::pos2(left, rect.top() + 2.0),
                egui::pos2(right, rect.top() + half),
            );
            let bottom = egui::Rect::from_min_max(
                egui::pos2(left, rect.top() + half),
                egui::pos2(right, rect.bottom() - 2.0),
            );
            label(
                ui,
                top,
                RichText::new(format!("{status} · {name}")).strong(),
            );
            label(
                ui,
                bottom,
                RichText::new(format!("{before} → {now} · {change}")),
            );
        }
        if response.clicked() {
            self.changes.selected = Some(index);
            if self.compact_layout {
                self.phone_overlay = Some(PhoneOverlay::ChangeDetails);
            }
        }
        if response.double_clicked() && node.is_container() && tree {
            if !self.changes.expanded.remove(&index) {
                self.changes.expanded.insert(index);
            }
            self.changes.dirty = true;
        }
    }

    fn change_summary(&self, ui: &mut egui::Ui, comparison: &Comparison) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        let dark = ui.visuals().dark_mode;
        let dimension = self.dimension();
        let totals = comparison.totals(dimension);
        let signed = |value: i128| i18n::signed_bytes(language, value);
        let tile = |ui: &mut egui::Ui, title: &str, value: String, detail: String| {
            egui::Frame::new()
                .fill(theme::surface(dark))
                .corner_radius(8)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_min_width(130.0);
                    ui.label(RichText::new(title).small().color(theme::accent(dark)));
                    ui.label(RichText::new(value).size(20.0));
                    ui.label(RichText::new(detail).small().weak());
                });
        };
        ui.horizontal_wrapped(|ui| {
            tile(
                ui,
                t("cmp_tile_added"),
                signed(totals.added as i128),
                i18n::counted(
                    language,
                    totals.added_count + totals.type_changed_count,
                    "object",
                ),
            );
            tile(
                ui,
                t("cmp_tile_removed"),
                signed(-(totals.removed as i128)),
                i18n::counted(
                    language,
                    totals.removed_count + totals.type_changed_count,
                    "object",
                ),
            );
            tile(
                ui,
                t("cmp_tile_changed"),
                format!(
                    "{} / {}",
                    signed(totals.grown as i128),
                    signed(-(totals.shrunk as i128))
                ),
                format!(
                    "{} · {}",
                    i18n::counted(language, totals.grew_count, "object"),
                    i18n::counted(language, totals.shrank_count, "object")
                ),
            );
            tile(
                ui,
                t(if totals.is_complete() {
                    "cmp_tile_net"
                } else {
                    "cmp_tile_net_partial"
                }),
                signed(totals.net()),
                t(if totals.is_complete() {
                    "cmp_net_complete"
                } else {
                    "cmp_net_partial"
                })
                .to_owned(),
            );
        });
        if !totals.is_complete() {
            ui.label(format!(
                "{} {}",
                t("cmp_unknown_part"),
                i18n::counted(language, totals.unknown_count, "object")
            ));
            if totals.lower_bound_count > 0 {
                ui.label(format!(
                    "{} {}",
                    t("cmp_lower_bound"),
                    i18n::counted(language, totals.lower_bound_count, "object")
                ));
            }
        }
        ui.label(RichText::new(t("cmp_disclaimer")).small().weak());
        for limitation in &comparison.limitations {
            if matches!(limitation, Limitation::AllocatedUnavailable) && !self.allocated {
                continue;
            }
            ui.label(RichText::new(format!("• {}", t(limitation.key()))).small());
        }
    }

    fn change_controls(&mut self, ui: &mut egui::Ui, comparison: &Comparison) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        let dimension = self.dimension();
        let totals = comparison.totals(dimension);
        ui.horizontal_wrapped(|ui| {
            let label = ui.label(t("search"));
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.changes.query)
                        .desired_width(220.0)
                        .hint_text(t("cmp_search_hint")),
                )
                .labelled_by(label.id)
                .changed()
            {
                self.changes.dirty = true;
            }
            egui::ComboBox::from_label(t("sort"))
                .selected_text(t(match self.changes.sort {
                    ChangeSort::Growth => "cmp_sort_growth",
                    ChangeSort::Decrease => "cmp_sort_decrease",
                    ChangeSort::Name => "name",
                }))
                .show_ui(ui, |ui| {
                    for (sort, key) in [
                        (ChangeSort::Growth, "cmp_sort_growth"),
                        (ChangeSort::Decrease, "cmp_sort_decrease"),
                        (ChangeSort::Name, "name"),
                    ] {
                        if ui
                            .selectable_value(&mut self.changes.sort, sort, t(key))
                            .changed()
                        {
                            self.changes.dirty = true;
                        }
                    }
                });
            for (mode, key) in [
                (ChangeMode::List, "cmp_mode_list"),
                (ChangeMode::Tree, "cmp_mode_tree"),
            ] {
                if ui
                    .selectable_value(&mut self.changes.mode, mode, t(key))
                    .changed()
                {
                    self.changes.dirty = true;
                }
            }
            if ui
                .add_enabled(
                    comparison.allocated_available,
                    egui::Checkbox::new(&mut self.allocated, t("allocated")),
                )
                .changed()
            {
                self.changes.dirty = true;
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for status in Status::ALL {
                let count = match status {
                    Status::Added => totals.added_count,
                    Status::Removed => totals.removed_count,
                    Status::Grew => totals.grew_count,
                    Status::Shrank => totals.shrank_count,
                    Status::SameSize => totals.same_count,
                    Status::Unknown => totals.unknown_count,
                    Status::TypeChanged => totals.type_changed_count,
                };
                let on = self.changes.statuses & status.bit() != 0;
                let response = ui.add(
                    egui::Button::new(format!(
                        "{} {} ({})",
                        status.symbol(),
                        t(status.key()),
                        i18n::integer(language, count)
                    ))
                    .selected(on),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, t(status.key()))
                });
                if response.clicked() {
                    self.changes.statuses ^= status.bit();
                    self.changes.dirty = true;
                }
            }
        });
    }

    fn change_details(&mut self, ui: &mut egui::Ui, comparison: &Comparison) {
        let Some(index) = self.changes.selected else {
            return;
        };
        let language = self.language;
        let t = |key| i18n::text(language, key);
        let dimension = self.dimension();
        let i = index as usize;
        let node = comparison.nodes[i];
        let dark = ui.visuals().dark_mode;
        egui::Frame::new()
            .fill(theme::surface(dark))
            .corner_radius(10)
            .inner_margin(16)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(RichText::new(comparison.name(i)).strong());
                ui.label(RichText::new(t("cmp_relative_path")).small().weak());
                let path = comparison.path(i);
                ui.label(if path.is_empty() {
                    comparison.name(i).to_owned()
                } else {
                    path.clone()
                });
                ui.label(self.status_label(comparison, i));
                if let Some(key) = node.reason(dimension).key() {
                    ui.label(t(key));
                }
                let (old_kind, new_kind) = comparison.kinds(i);
                let kind = |kind: Option<Kind>| kind.map_or("—", |k| t(kind_key(k)));
                ui.label(format!(
                    "{}: {} → {}",
                    t("cmp_kind"),
                    kind(old_kind),
                    kind(new_kind)
                ));
                for (dimension, key) in [
                    (Dimension::Logical, "logical_bytes"),
                    (Dimension::Allocated, "allocated"),
                ] {
                    let (before, now) = comparison.sizes(i, dimension);
                    let show = |m: Option<crate::scan::Measure>| {
                        m.map_or("—".to_owned(), |m| measure(language, m))
                    };
                    ui.label(format!("{}: {} → {}", t(key), show(before), show(now)));
                }
                ui.label(format!(
                    "{}: {}",
                    t("cmp_change"),
                    self.change_texts(comparison, i).2
                ));
                if node.is_container() {
                    let totals = comparison.subtree_totals(i, dimension);
                    ui.label(format!(
                        "{}: +{} · −{} · ↑{} · ↓{} · ?{}",
                        t("cmp_inside"),
                        i18n::integer(language, totals.added_count),
                        i18n::integer(language, totals.removed_count),
                        i18n::integer(language, totals.grew_count),
                        i18n::integer(language, totals.shrank_count),
                        i18n::integer(language, totals.unknown_count),
                    ));
                }
                ui.horizontal_wrapped(|ui| {
                    if let Some(old) = node.old
                        && ui.button(t("cmp_show_baseline")).clicked()
                    {
                        self.goto(Side::Baseline, old);
                    }
                    if let Some(new) = node.new
                        && ui.button(t("cmp_show_current")).clicked()
                    {
                        self.goto(Side::Current, new);
                    }
                    if ui.button(t("copy_path")).clicked() {
                        #[cfg(not(any(target_os = "android", target_os = "ios")))]
                        ui.ctx().copy_text(path.clone());
                    }
                });
            });
    }

    /// Header of the comparison: direction, exit.
    fn change_header(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        ui.horizontal_wrapped(|ui| {
            if ui.button(t("cmp_exit")).clicked() {
                self.leave_changes();
                return;
            }
            if ui.button(t("compare_swap")).clicked() {
                self.swap_direction();
            }
        });
        if self.changes.comparison.is_some() && self.view == View::Changes {
            // The comparison is dropped whenever an operand changes, so these are its sides.
            for (side, view) in [
                (Side::Baseline, self.changes.baseline),
                (Side::Current, self.changes.current),
            ] {
                let title = t(match side {
                    Side::Baseline => "compare_baseline",
                    Side::Current => "compare_current",
                });
                let arrow = if side == Side::Baseline {
                    ""
                } else {
                    t("compare_arrow")
                };
                ui.label(
                    RichText::new(format!("{arrow} {title}: {}", self.operand_line(view)).trim())
                        .small(),
                );
            }
        }
    }

    pub(super) fn changes_workspace(&mut self, ui: &mut egui::Ui, wide: bool) {
        self.change_header(ui);
        ui.add_space(8.0);
        let Some(comparison) = self.changes.comparison.clone() else {
            self.compare_setup(ui);
            return;
        };
        self.refresh_changes();
        self.change_summary(ui, &comparison);
        ui.add_space(8.0);
        self.change_controls(ui, &comparison);
        ui.add_space(8.0);
        let tree = self.changes.mode == ChangeMode::Tree;
        let count = if tree {
            self.changes.tree.len()
        } else {
            self.changes.rows.len()
        };
        if count == 0 {
            ui.label(t_key(self.language, "no_results"));
        } else {
            let row_height = if wide { 32.0 } else { 56.0 };
            if wide {
                ui.horizontal(|ui| {
                    let free = ui.available_width() - 36.0;
                    let widths = [
                        free * 0.20,
                        free * 0.30,
                        free * 0.16,
                        free * 0.16,
                        free * 0.18,
                    ];
                    ui.add_space(4.0 + if tree { 26.0 } else { 0.0 });
                    for (key, width) in [
                        "cmp_col_status",
                        "name",
                        "cmp_col_before",
                        "cmp_col_now",
                        "cmp_col_change",
                    ]
                    .into_iter()
                    .zip(widths)
                    {
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, 24.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(t_key(self.language, key)).strong(),
                                    )
                                    .truncate(),
                                );
                            },
                        );
                    }
                });
            }
            egui::ScrollArea::vertical()
                .id_salt("change-rows")
                .max_height(if wide { 420.0 } else { 360.0 })
                .auto_shrink([false, true])
                .show_rows(ui, row_height, count, |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for position in range {
                        let (index, depth) = if tree {
                            self.changes.tree[position]
                        } else {
                            (self.changes.rows[position], 0)
                        };
                        self.change_row(
                            ui,
                            &comparison,
                            index,
                            RowLayout {
                                depth,
                                tree,
                                wide,
                                height: row_height,
                            },
                        );
                    }
                });
        }
        ui.add_space(8.0);
        self.change_details(ui, &comparison);
    }

    /// Phone layout: one screen for setup or the result list, details in a sheet.
    pub(super) fn phone_changes(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        ui.horizontal(|ui| {
            if Self::phone_icon(ui, "←", t("back")) {
                self.phone_back();
            }
            ui.add_sized(
                [(ui.available_width() - 72.0).max(1.0), 48.0],
                egui::Label::new(RichText::new(t("view_changes")).strong()).truncate(),
            );
            if ui
                .add_sized([64.0, 48.0], egui::Button::new(t("phone_more")).truncate())
                .clicked()
            {
                self.phone_overlay = Some(PhoneOverlay::ChangeFilter);
            }
        });
        self.open_status(ui);
        let Some(comparison) = self.changes.comparison.clone() else {
            egui::ScrollArea::vertical()
                .id_salt("phone-compare-setup")
                .show(ui, |ui| self.compare_setup(ui));
            return;
        };
        self.refresh_changes();
        let dimension = self.dimension();
        let totals = comparison.totals(dimension);
        ui.label(
            RichText::new(format!(
                "{} {}",
                t(if totals.is_complete() {
                    "cmp_tile_net"
                } else {
                    "cmp_tile_net_partial"
                }),
                i18n::signed_bytes(language, totals.net())
            ))
            .strong(),
        );
        ui.add(
            egui::Label::new(
                RichText::new(format!(
                    "+{} · −{} · ↑{} · ↓{} · ?{}",
                    i18n::integer(language, totals.added_count + totals.type_changed_count),
                    i18n::integer(language, totals.removed_count),
                    i18n::integer(language, totals.grew_count),
                    i18n::integer(language, totals.shrank_count),
                    i18n::integer(language, totals.unknown_count),
                ))
                .small(),
            )
            .truncate(),
        );
        ui.separator();
        let tree = self.changes.mode == ChangeMode::Tree;
        let count = if tree {
            self.changes.tree.len()
        } else {
            self.changes.rows.len()
        };
        if count == 0 {
            ui.label(t("no_results"));
            return;
        }
        let row_height = (ui.text_style_height(&egui::TextStyle::Body) * 2.0 + 20.0).max(60.0);
        egui::ScrollArea::vertical()
            .id_salt("phone-change-rows")
            .auto_shrink([false, false])
            .show_rows(ui, row_height, count, |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for position in range {
                    let (index, depth) = if tree {
                        self.changes.tree[position]
                    } else {
                        (self.changes.rows[position], 0)
                    };
                    self.change_row(
                        ui,
                        &comparison,
                        index,
                        RowLayout {
                            depth,
                            tree,
                            wide: false,
                            height: row_height,
                        },
                    );
                }
            });
    }

    pub(super) fn phone_change_sheet(&mut self, ui: &mut egui::Ui, overlay: PhoneOverlay) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        let Some(comparison) = self.changes.comparison.clone() else {
            if Self::phone_button(ui, t("compare_swap")) {
                self.swap_direction();
                self.phone_overlay = None;
            }
            if Self::phone_button(ui, t("cmp_exit")) {
                self.phone_overlay = None;
                self.leave_changes();
            }
            return;
        };
        match overlay {
            PhoneOverlay::ChangeDetails => self.change_details(ui, &comparison),
            _ => {
                self.change_summary(ui, &comparison);
                ui.add_space(8.0);
                self.change_controls(ui, &comparison);
                ui.add_space(8.0);
                if Self::phone_button(ui, t("compare_swap")) {
                    self.swap_direction();
                    self.phone_overlay = None;
                }
                if Self::phone_button(ui, t("cmp_exit")) {
                    self.phone_overlay = None;
                    self.leave_changes();
                }
            }
        }
    }
}

fn t_key(language: usize, key: &str) -> &str {
    i18n::text(language, key)
}
