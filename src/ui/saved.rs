//! Saved results and change comparison. Opening a saved result reads only that
//! file: nothing recorded in it can start a scan, open a location or reveal an
//! object. Comparison is an explicit choice of baseline and current result.
use super::*;
use crate::{
    compare::{
        self, ChangeSort, CompareError, Comparison, Dimension, Limitation, Reason, ScopeVerdict,
        Status,
    },
    import::ImportError,
    job,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum View {
    /// The result of a scan started in this session.
    Live,
    /// A result opened from a file.
    Saved,
    Changes,
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

pub(super) struct Changes {
    /// Baseline is the live scan and current is the saved result.
    pub reversed: bool,
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
            reversed: false,
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
    /// Forget any comparison; the operands changed or were closed.
    pub fn clear(&mut self) {
        let reversed = self.reversed;
        *self = Self {
            reversed,
            ..Self::default()
        };
    }
}

#[derive(Clone, Copy)]
struct RowLayout {
    depth: usize,
    tree: bool,
    wide: bool,
    height: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Side {
    Baseline,
    Current,
}

impl ExplorerApp {
    pub(super) fn dimension(&self) -> Dimension {
        if self.allocated {
            Dimension::Allocated
        } else {
            Dimension::Logical
        }
    }
    pub(super) fn has_baseline_and_current(&self) -> bool {
        self.saved.is_some() && self.live.is_some()
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
            View::Saved => {
                let saved = self.saved.as_ref().map(|s| s.result.clone());
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
    }
    /// Whether the browsed result came from a file, whichever screen is on top. The
    /// comparison screen keeps the previously browsed result, so the view alone is not enough.
    #[cfg(any(target_os = "android", target_os = "ios", test))]
    pub(super) fn shows_saved_result(&self) -> bool {
        matches!((&self.saved, &self.result), (Some(saved), Some(result)) if Arc::ptr_eq(&saved.result, result))
    }
    pub(super) fn open_saved_dialog(&mut self) {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .pick_file()
        {
            self.open_saved(path, false);
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            self.native_open = crate::mobile::request_open();
            if self.native_open.is_none() {
                self.notice = Some("unavailable");
            }
        }
    }
    /// Start reading one chosen file. A newer request supersedes an older one: the
    /// older job is cancelled and can no longer deliver a result.
    pub(super) fn open_saved(&mut self, path: PathBuf, delete_after: bool) {
        // Drop any queued result before the next worker samples available memory.
        drop(self.open_job.take());
        self.open_error = None;
        self.open_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.open_job = Some(job::open_local(path, delete_after));
    }
    pub(super) fn close_saved(&mut self) {
        self.saved = None;
        self.changes.clear();
        if self.view != View::Live {
            self.view = View::Live;
            let live = self.live.clone();
            self.set_displayed(live);
        }
    }
    pub(super) fn leave_changes(&mut self) {
        let target = if self.saved.is_some() && self.live.is_none() {
            View::Saved
        } else {
            View::Live
        };
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
                Ok(result) => {
                    self.saved = Some(SavedResult {
                        result: Arc::new(result),
                        name: std::mem::take(&mut self.open_name),
                    });
                    self.changes.clear();
                    let saved = self.saved.as_ref().map(|s| s.result.clone());
                    self.view = View::Saved;
                    self.phone_home = false;
                    self.set_displayed(saved);
                }
                Err(ImportError::Cancelled) => {}
                Err(error) => self.show_open_error(error),
            }
        }
        if let Some(job) = &self.changes.job
            && let Some(outcome) = job.take()
        {
            self.changes.job = None;
            match outcome {
                Ok(comparison) => {
                    self.changes.comparison = Some(Arc::new(comparison));
                    self.changes.dirty = true;
                    self.changes.selected = None;
                    self.changes.expanded = HashSet::from([0]);
                    self.sync_native_result();
                }
                Err(CompareError::Incompatible(scope)) => {
                    self.changes.failure = Some(CompareFailure::Incompatible(scope));
                }
                Err(CompareError::NeedsConfirmation(_)) => {
                    self.changes.failure = Some(CompareFailure::Other("cmp_needs_confirmation"));
                }
                Err(CompareError::TooLarge) => {
                    self.changes.failure = Some(CompareFailure::Other("import_error_too_large"));
                }
                Err(CompareError::Cancelled) => {}
            }
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        if let Some(receiver) = &self.native_open
            && let Ok(outcome) = receiver.try_recv()
        {
            self.native_open = None;
            match outcome {
                crate::mobile::OpenOutcome::Ready(path) => self.open_saved(path, true),
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
    fn show_open_error(&mut self, error: ImportError) {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let t = |key| i18n::text(self.language, key);
            let message = format!(
                "{}\n{}\n{}",
                t("import_failed"),
                t(error.key()),
                error.detail()
            );
            let _ = crate::mobile::request(serde_json::json!({
                "op": "saved_open_error", "message": message,
            }));
        }
        self.open_error = Some(error);
    }
    fn operands(&self) -> Option<(Arc<ScanResult>, Arc<ScanResult>)> {
        let saved = self.saved.as_ref()?.result.clone();
        let live = self.live.clone()?;
        Some(if self.changes.reversed {
            (live, saved)
        } else {
            (saved, live)
        })
    }
    pub(super) fn start_compare(&mut self) {
        let Some((baseline, current)) = self.operands() else {
            return;
        };
        self.changes.comparison = None;
        self.changes.failure = None;
        self.changes.job = Some(job::compare(baseline, current, self.changes.confirm_scope));
    }
    pub(super) fn swap_direction(&mut self) {
        self.changes.reversed = !self.changes.reversed;
        self.changes.confirm_scope = false;
        self.changes.comparison = None;
        self.changes.job = None;
        self.changes.failure = None;
        self.changes.selected = None;
        self.changes.dirty = true;
    }
    pub(super) fn open_changes(&mut self) {
        self.show(View::Changes);
        self.phone_overlay = None;
    }
    fn side_view(&self, side: Side) -> View {
        match (side, self.changes.reversed) {
            (Side::Baseline, false) | (Side::Current, true) => View::Saved,
            _ => View::Live,
        }
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
                }
            });
        }
        if let Some(error) = self.open_error.clone() {
            egui::Frame::new()
                .fill(theme::surface(ui.visuals().dark_mode))
                .corner_radius(8)
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(RichText::new(t("import_failed")).strong());
                    ui.label(t(error.key()));
                    ui.label(RichText::new(error.detail()).small().weak());
                    if ui.button(t("close")).clicked() {
                        self.open_error = None;
                    }
                });
        }
    }
    /// Unmistakable banner: the result on screen is an older snapshot.
    pub(super) fn saved_banner(&mut self, ui: &mut egui::Ui) {
        let (Some(saved), View::Saved) = (&self.saved, self.view) else {
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
                        self.open_changes();
                    }
                    if ui.button(t("saved_close")).clicked() {
                        self.close_saved();
                    }
                });
                if self.live.is_none() {
                    ui.label(RichText::new(t("compare_needs_scan")).small().weak());
                }
            });
        ui.add_space(8.0);
    }
    pub(super) fn view_tabs(&mut self, ui: &mut egui::Ui) {
        if self.saved.is_none() {
            return;
        }
        let language = self.language;
        let t = |key| i18n::text(language, key);
        ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            let mut next = None;
            for (view, key, enabled) in [
                (
                    View::Live,
                    "view_current",
                    self.live.is_some() || self.job.is_some(),
                ),
                (View::Saved, "view_saved", true),
                (
                    View::Changes,
                    "view_changes",
                    self.has_baseline_and_current(),
                ),
            ] {
                if ui
                    .add_enabled(
                        enabled,
                        egui::Button::new(t(key)).selected(self.view == view),
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

    fn side_card(&self, ui: &mut egui::Ui, title: &str, view: View) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        let dark = ui.visuals().dark_mode;
        egui::Frame::new()
            .fill(theme::surface(dark))
            .corner_radius(8)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_min_width((ui.available_width()).max(120.0));
                ui.label(RichText::new(title).small().color(theme::accent(dark)));
                match view {
                    View::Saved => {
                        if let Some(saved) = &self.saved {
                            ui.label(RichText::new(t("view_saved")).strong());
                            ui.label(&saved.name);
                            ui.label(self.describe(&saved.result));
                        }
                    }
                    _ => {
                        if let Some(live) = &self.live {
                            ui.label(RichText::new(t("view_current")).strong());
                            ui.label(&live.nodes[0].name);
                            ui.label(self.describe(live));
                        }
                    }
                }
            });
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

    /// Explicit choice of baseline and current result, before comparing.
    fn compare_setup(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        let Some((baseline, current)) = self.operands() else {
            ui.label(t("compare_needs_both"));
            return;
        };
        let (base_view, current_view) = (
            self.side_view(Side::Baseline),
            self.side_view(Side::Current),
        );
        ui.label(RichText::new(t("compare_heading")).size(20.0));
        ui.label(t("compare_direction_help"));
        let wide = ui.available_width() > 560.0;
        let arrow = t("compare_arrow");
        let draw_cards = |ui: &mut egui::Ui, this: &Self| {
            this.side_card(ui, t("compare_baseline"), base_view);
            ui.label(RichText::new(arrow).strong());
            this.side_card(ui, t("compare_current"), current_view);
        };
        if wide {
            ui.columns(3, |columns| {
                self.side_card(&mut columns[0], t("compare_baseline"), base_view);
                columns[1].vertical_centered(|ui| {
                    ui.add_space(24.0);
                    ui.label(RichText::new(arrow).strong());
                });
                self.side_card(&mut columns[2], t("compare_current"), current_view);
            });
        } else {
            draw_cards(ui, self);
        }
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
                    can_compare && self.changes.job.is_none(),
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
        match &self.changes.failure {
            Some(CompareFailure::Incompatible(scope)) => {
                ui.label(t("cmp_verdict_incompatible"));
                ui.label(RichText::new(self.scope_text(scope)).small());
            }
            Some(CompareFailure::Other(key)) => {
                ui.label(t(key));
            }
            None => {}
        }
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
        if let (Some(comparison), true) = (&self.changes.comparison, self.view == View::Changes) {
            let describe = |result: &ScanResult| {
                format!("{} · {}", result.nodes[0].name, self.captured(result))
            };
            ui.label(
                RichText::new(format!(
                    "{}: {}   {}   {}: {}",
                    t("compare_baseline"),
                    describe(&comparison.baseline),
                    t("compare_arrow"),
                    t("compare_current"),
                    describe(&comparison.current)
                ))
                .small(),
            );
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
