//! Storage exploration UI. The list and treemap share a single selection and directory.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use crate::preferences::WindowState;
use crate::{
    bridge::{self, Listing},
    i18n,
    preferences::{self, Preferences},
    scan::{self, Kind, ScanResult, Sort},
    theme::{self, Appearance},
    treemap,
};
use eframe::egui::{self, RichText};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use std::path::Path;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Directory,
    Largest,
    Extensions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExtensionColumn {
    Name,
    Size,
    Files,
}
impl ExtensionColumn {
    fn key(self) -> &'static str {
        match self {
            Self::Name => "extension",
            Self::Size => "size",
            Self::Files => "file_count",
        }
    }
}

pub struct ExplorerApp {
    language: usize,
    preferences: Preferences,
    preferences_path: Option<PathBuf>,
    saved_preferences: Preferences,
    last_preferences_write: Instant,
    preferences_write_failed: bool,
    settings_open: bool,
    expanded: HashSet<usize>,
    children: Vec<Vec<usize>>,
    children_sort: Option<(Sort, bool)>,
    tree_rows: Vec<(usize, usize)>,
    appearance: Appearance,
    service: scan::Service,
    job: Option<scan::Job>,
    result: Option<Arc<ScanResult>>,
    scope: Option<PathBuf>,
    provider_scope: Option<(u64, String)>,
    directory: usize,
    selected: Option<usize>,
    page: Page,
    query: String,
    sort: Sort,
    allocated: bool,
    cross_filesystems: bool,
    rows: Vec<usize>,
    extensions: Vec<(String, u64, scan::Measure)>,
    extension_sort: (ExtensionColumn, bool), // Column and descending order.
    dirty: bool,
    preview: Option<Listing>,
    mobile_busy: bool,
    notice: Option<&'static str>,
    export: Option<mpsc::Receiver<crate::mobile::ExportOutcome>>,
    map_group: Option<Vec<usize>>,
    map_objects: Vec<Option<usize>>,
    map_selected: Option<usize>,
    map_weights: Vec<f64>,
    grouped: Vec<usize>,
    #[cfg(any(target_os = "android", target_os = "ios"))]
    native_settings: Option<(Appearance, usize)>,
}
impl ExplorerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::new_with_preferences(cc, preferences::path())
    }
    fn new_with_preferences(cc: &eframe::CreationContext<'_>, path: Option<PathBuf>) -> Self {
        cc.egui_ctx.set_fonts(crate::fonts::definitions());
        theme::install(&cc.egui_ctx);
        let repaint = cc.egui_ctx.clone();
        bridge::set_waker(Arc::new(move || repaint.request_repaint()));
        let mut app = Self::empty();
        app.preferences_path = path;
        if let Some(path) = &app.preferences_path {
            app.preferences = Preferences::load(path);
        }
        app.language = app.preferences.language_index();
        app.appearance = app.preferences.appearance;
        app.saved_preferences = app.preferences.clone();
        cc.egui_ctx.set_theme(app.appearance.preference());
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if let (Some(state), Some(window)) = (app.preferences.window, cc.winit_window()) {
            let monitor = window
                .available_monitors()
                .find(|monitor| {
                    let Some([x, y]) = state.position else {
                        return false;
                    };
                    let origin = monitor.position();
                    let size = monitor.size();
                    (x as i64 >= origin.x as i64
                        && (x as i64) < origin.x as i64 + size.width as i64)
                        && (y as i64 >= origin.y as i64
                            && (y as i64) < origin.y as i64 + size.height as i64)
                })
                .or_else(|| window.current_monitor())
                .or_else(|| window.available_monitors().next());
            if let Some(monitor) = monitor {
                let mut position = monitor.position();
                let extent = monitor.size();
                let (point, dimensions) = state.fit(
                    [position.x, position.y],
                    [extent.width, extent.height],
                    monitor.scale_factor(),
                );
                if state.position.is_some() {
                    position.x = point[0];
                    position.y = point[1];
                    window.set_outer_position(position);
                }
                let mut size = window.inner_size();
                size.width = dimensions[0];
                size.height = dimensions[1];
                let _ = window.request_inner_size(size);
            }
            window.set_maximized(state.maximized);
        }
        app
    }
    fn empty() -> Self {
        Self {
            language: i18n::system_language(),
            preferences: Preferences::default(),
            preferences_path: None,
            saved_preferences: Preferences::default(),
            last_preferences_write: Instant::now(),
            preferences_write_failed: false,
            settings_open: false,
            expanded: HashSet::from([0]),
            children: Vec::new(),
            children_sort: None,
            tree_rows: Vec::new(),
            appearance: Appearance::System,
            service: scan::Service::default(),
            job: None,
            result: None,
            scope: None,
            provider_scope: None,
            directory: 0,
            selected: None,
            page: Page::Directory,
            query: String::new(),
            sort: Sort::Size,
            allocated: false,
            cross_filesystems: false,
            rows: Vec::new(),
            extensions: Vec::new(),
            extension_sort: (ExtensionColumn::Size, true),
            dirty: true,
            preview: None,
            mobile_busy: false,
            notice: None,
            export: None,
            map_group: None,
            map_objects: Vec::new(),
            map_selected: None,
            map_weights: Vec::new(),
            grouped: Vec::new(),
            #[cfg(any(target_os = "android", target_os = "ios"))]
            native_settings: None,
        }
    }
    fn persist_preferences(&mut self, force: bool) {
        let Some(path) = &self.preferences_path else {
            return;
        };
        self.preferences.appearance = self.appearance;
        if self.preferences == self.saved_preferences {
            return;
        }
        let settings_changed = self.preferences.appearance != self.saved_preferences.appearance
            || self.preferences.language != self.saved_preferences.language;
        if !force
            && (self.preferences_write_failed || !settings_changed)
            && self.last_preferences_write.elapsed() < Duration::from_secs(1)
        {
            return;
        }
        self.last_preferences_write = Instant::now();
        match self.preferences.save(path) {
            Ok(()) => {
                self.saved_preferences = self.preferences.clone();
                self.preferences_write_failed = false;
            }
            Err(error) => {
                self.preferences_write_failed = true;
                eprintln!("Could not save UI preferences: {error}");
            }
        }
    }
    pub fn start(&mut self, path: PathBuf) {
        self.job = Some(self.service.start(
            path.clone(),
            scan::Options {
                cross_filesystems: self.cross_filesystems,
                ..Default::default()
            },
        ));
        self.scope = Some(path);
        self.result = None;
        self.selected = None;
        self.directory = 0;
        self.rows.clear();
        self.children.clear();
        self.children_sort = None;
        self.tree_rows.clear();
        self.expanded = HashSet::from([0]);
        self.extensions.clear();
        self.query.clear();
        self.notice = None;
        self.dirty = true;
        self.map_group = None;
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    fn start_provider(&mut self, generation: u64, root: String) {
        let provider =
            crate::provider::Provider::new(root.clone(), generation, crate::mobile::transport());
        let job = self.service.start_storage(
            Box::new(provider),
            scan::Options {
                node_limit: 100_000,
                memory_limit: 64 * 1024 * 1024,
                ..Default::default()
            },
        );
        crate::mobile::scanning(generation, job.progress.clone());
        self.job = Some(job);
        self.provider_scope = Some((generation, root));
        self.scope = None;
        self.result = None;
        self.selected = None;
        self.directory = 0;
        self.rows.clear();
        self.tree_rows.clear();
        self.children.clear();
        self.children_sort = None;
        self.expanded = HashSet::from([0]);
        self.extensions.clear();
        self.query.clear();
        self.notice = None;
        self.map_group = None;
        self.dirty = true;
    }
    fn rescan(&mut self) {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        if let Some((generation, root)) = self.provider_scope.clone() {
            self.start_provider(generation, root);
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if let Some(path) = self.scope.clone() {
            self.start(path);
        }
    }
    fn choose(&mut self) {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            self.start(path);
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let generation = bridge::begin();
            self.job = None;
            self.result = None;
            self.provider_scope = None;
            self.preview = None;
            self.notice = None;
            #[cfg(target_os = "android")]
            let launched = crate::android::pick(generation);
            #[cfg(target_os = "ios")]
            let launched = crate::ios::pick(generation);
            self.mobile_busy = launched;
            if !launched {
                self.notice = Some("unavailable");
            }
        }
    }
    fn navigate(&mut self, index: usize) {
        self.directory = index;
        self.expanded.insert(index);
        self.selected = Some(index);
        self.page = Page::Directory;
        self.query.clear();
        self.map_group = None;
        self.dirty = true;
    }
    fn back(&mut self) {
        if self.map_group.take().is_some() {
            self.query.clear();
            self.dirty = true;
            return;
        }
        if let Some(parent) = self
            .result
            .as_ref()
            .and_then(|r| r.nodes[self.directory].parent)
        {
            self.navigate(parent);
        }
    }
    fn open_group(&mut self) {
        if self.map_group.is_none() && !self.grouped.is_empty() {
            self.map_group = Some(self.grouped.clone());
            self.dirty = true;
        }
    }
    fn refresh_rows(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(result) = &self.result {
            if self.children.len() != result.nodes.len() {
                self.children = vec![Vec::new(); result.nodes.len()];
                for (index, node) in result.nodes.iter().enumerate() {
                    if let Some(parent) = node.parent {
                        self.children[parent].push(index);
                    }
                }
            }
            if self.children_sort != Some((self.sort, self.allocated)) {
                for children in &mut self.children {
                    children.sort_by(|a, b| {
                        let size = |i: usize| {
                            if self.allocated {
                                result.nodes[i].allocated.known
                            } else {
                                result.nodes[i].logical.known
                            }
                        };
                        let by_name = || {
                            result.nodes[*a]
                                .name
                                .to_lowercase()
                                .cmp(&result.nodes[*b].name.to_lowercase())
                                .then(a.cmp(b))
                        };
                        if self.sort == Sort::Size {
                            size(*b).cmp(&size(*a)).then_with(by_name)
                        } else {
                            by_name()
                        }
                    });
                }
                self.children_sort = Some((self.sort, self.allocated));
            }
            self.tree_rows.clear();
            let mut pending = vec![(self.directory, 0)];
            while let Some((index, depth)) = pending.pop() {
                self.tree_rows.push((index, depth));
                if self.expanded.contains(&index) {
                    pending.extend(self.children[index].iter().rev().map(|i| (*i, depth + 1)));
                }
            }
            self.rows = if let Some(group) = &self.map_group {
                let query = self.query.to_lowercase();
                let mut rows: Vec<_> = group
                    .iter()
                    .copied()
                    .filter(|index| {
                        let node = &result.nodes[*index];
                        query.is_empty()
                            || node.name.to_lowercase().contains(&query)
                            || node.locator.to_lowercase().contains(&query)
                    })
                    .collect();
                result.sort(&mut rows, self.sort);
                rows
            } else if !self.query.is_empty() {
                result.search(&self.query, self.page == Page::Largest, self.sort)
            } else if self.page == Page::Largest {
                result.search("", true, self.sort)
            } else {
                result.children(self.directory, "", self.sort)
            };
        }
        if self.allocated
            && self.sort == Sort::Size
            && let Some(result) = &self.result
        {
            self.rows.sort_by(|a, b| {
                result.nodes[*b]
                    .allocated
                    .known
                    .cmp(&result.nodes[*a].allocated.known)
                    .then(a.cmp(b))
            });
        }
        self.map_objects.clear();
        self.map_weights.clear();
        self.grouped.clear();
        // A grouped tile opens its complete virtualized list, never another map
        // containing a slightly smaller copy of the same grouped tile.
        if self.map_group.is_none()
            && let Some(result) = &self.result
        {
            let total: f64 = self
                .rows
                .iter()
                .filter_map(|i| {
                    let node = &result.nodes[*i];
                    if !self.query.is_empty() && node.kind == Kind::Directory {
                        None
                    } else {
                        Some(if self.allocated {
                            node.allocated.known as f64
                        } else {
                            node.logical.known as f64
                        })
                    }
                })
                .sum();
            for index in self.rows.iter().copied() {
                let node = &result.nodes[index];
                let weight = if self.allocated {
                    node.allocated.known
                } else {
                    node.logical.known
                };
                if weight == 0 || (!self.query.is_empty() && node.kind == Kind::Directory) {
                    continue;
                }
                if self.map_objects.len() < 95 && weight as f64 >= total * 0.015 {
                    self.map_objects.push(Some(index));
                    self.map_weights.push(weight as f64);
                } else {
                    self.grouped.push(index);
                }
            }
            if !self.grouped.is_empty() {
                self.map_objects.push(None);
                self.map_weights.push(
                    self.grouped
                        .iter()
                        .map(|i| {
                            if self.allocated {
                                result.nodes[*i].allocated.known as f64
                            } else {
                                result.nodes[*i].logical.known as f64
                            }
                        })
                        .sum(),
                );
            }
        }
        self.map_selected = self.selected_tile();
        self.dirty = false;
    }
    fn poll(&mut self, context: &egui::Context) {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            if self
                .provider_scope
                .as_ref()
                .is_some_and(|(generation, _)| !bridge::is_current(*generation))
            {
                self.job = None;
                self.result = None;
                self.provider_scope = None;
            }
            if let Some(command) = crate::mobile::take_command() {
                use crate::mobile::Command;
                match command {
                    Command::Rescan => self.rescan(),
                    Command::Cancel => {
                        if let Some(job) = &self.job {
                            job.cancel();
                        }
                    }
                    Command::Export(json) => {
                        if self.export.is_none() {
                            self.save(json);
                        }
                    }
                    Command::Back => self.back(),
                    Command::CloseSettings => self.settings_open = false,
                    Command::Allocated(allocated) => {
                        self.allocated = allocated;
                        self.dirty = true;
                    }
                    Command::Language(index) => {
                        self.preferences.language =
                            i18n::TAGS.get(index).map(|tag| (*tag).to_owned());
                        self.language = if index < i18n::TAGS.len() {
                            index
                        } else {
                            i18n::system_language()
                        };
                    }
                    Command::Appearance(index) => {
                        if let Some(appearance) = Appearance::ALL.get(index) {
                            self.appearance = *appearance;
                            context.set_theme(appearance.preference());
                        }
                    }
                    Command::Navigate(index) | Command::Select(index) => {
                        if let Some(node) = self.result.as_ref().and_then(|r| r.nodes.get(index)) {
                            if matches!(command, Command::Navigate(_))
                                && node.kind == Kind::Directory
                            {
                                self.navigate(index);
                            } else {
                                self.select(index);
                            }
                        }
                    }
                }
            }
        }
        if let Some(result) = self.job.as_ref().and_then(scan::Job::take) {
            self.extensions = result.extensions();
            self.sort_extensions();
            let result = Arc::new(result);
            #[cfg(any(target_os = "android", target_os = "ios"))]
            if let Some((generation, _)) = &self.provider_scope {
                crate::mobile::publish(*generation, result.clone());
            }
            self.result = Some(result);
            self.job = None;
            self.dirty = true;
        }
        if let Some(listing) = bridge::take() {
            self.mobile_busy = false;
            #[cfg(any(target_os = "android", target_os = "ios"))]
            if listing.status == bridge::Status::Ready
                && let Some(root) = &listing.scope
            {
                self.start_provider(listing.generation, root.clone());
            }
            self.preview = Some(listing);
        }
        if let Some(receiver) = &self.export
            && let Ok(success) = receiver.try_recv()
        {
            self.notice = Some(match success {
                crate::mobile::ExportOutcome::Saved => "exported",
                crate::mobile::ExportOutcome::Cancelled => "export_cancelled",
                crate::mobile::ExportOutcome::Failed => "export_failed",
            });
            self.export = None;
        }
        if self.job.is_some() || self.mobile_busy || self.export.is_some() {
            context.request_repaint_after(Duration::from_millis(100));
        }
    }
    fn save(&mut self, json: bool) {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let Some(result) = self.result.clone() else {
                return;
            };
            let extension = if json { "json" } else { "csv" };
            let Some(path) = rfd::FileDialog::new()
                .set_file_name(format!("spacemosaic.{extension}"))
                .add_filter(extension, &[extension])
                .save_file()
            else {
                return;
            };
            let (sender, receiver) = mpsc::sync_channel(1);
            self.export = Some(receiver);
            std::thread::spawn(move || {
                let outcome = if scan::write_export(&result, &path, json).is_ok() {
                    crate::mobile::ExportOutcome::Saved
                } else {
                    crate::mobile::ExportOutcome::Failed
                };
                let _ = sender.send(outcome);
            });
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            if let Some(result) = self.result.clone() {
                self.export = crate::mobile::export(result, json);
                if self.export.is_none() {
                    self.notice = Some("export_failed");
                }
            }
        }
    }
    fn settings(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        if ui
            .add(egui::Button::new(t("settings")).wrap_mode(egui::TextWrapMode::Extend))
            .clicked()
        {
            self.settings_open = !self.settings_open;
        }
    }
    fn settings_window(&mut self, context: &egui::Context) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        let bounds = context.content_rect().shrink(8.0);
        let mut open = self.settings_open;
        let mut close = context.input(|i| i.key_pressed(egui::Key::Escape));
        egui::Window::new(t("settings"))
            .constrain_to(bounds)
            .default_pos(bounds.min + egui::vec2(8.0, 8.0))
            .max_width(bounds.width())
            .max_height(bounds.height())
            .vscroll(true)
            .id(egui::Id::new("settings-window"))
            .open(&mut open)
            .resizable(false)
            .default_width(280.0)
            .show(context, |ui| {
                if bounds.width() < 500.0 {
                    ui.spacing_mut().interact_size.y = 48.0;
                }
                ui.label(t("appearance"));
                ui.horizontal_wrapped(|ui| {
                    for appearance in Appearance::ALL {
                        if ui
                            .selectable_value(&mut self.appearance, appearance, t(appearance.key()))
                            .changed()
                        {
                            ui.ctx().set_theme(appearance.preference());
                        }
                    }
                });
                let language_label = ui.label(t("language"));
                egui::ComboBox::from_id_salt("application-language")
                    .selected_text(if self.preferences.language.is_none() {
                        format!("{} ({})", t("system"), i18n::TAGS[self.language])
                    } else {
                        i18n::TAGS[self.language].to_owned()
                    })
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(false, t("system")).clicked() {
                            self.language = i18n::system_language();
                            self.preferences.language = None;
                        }
                        for (index, tag) in i18n::TAGS.iter().enumerate() {
                            if ui
                                .selectable_value(&mut self.language, index, *tag)
                                .clicked()
                            {
                                self.preferences.language = Some((*tag).to_owned());
                            }
                        }
                    })
                    .response
                    .labelled_by(language_label.id);
                ui.horizontal_wrapped(|ui| {
                    if ui.button(t("smaller")).clicked() {
                        ui.ctx()
                            .set_zoom_factor((ui.ctx().zoom_factor() - 0.15).max(1.0));
                    }
                    if ui.button(t("larger")).clicked() {
                        ui.ctx()
                            .set_zoom_factor((ui.ctx().zoom_factor() + 0.15).min(2.5));
                    }
                });
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                {
                    ui.checkbox(&mut self.cross_filesystems, t("cross_filesystems"));
                    ui.label(t("mount_policy"));
                }
                if ui.button(t("close")).clicked() {
                    close = true;
                }
            });
        self.settings_open = open && !close;
    }
    fn header(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        egui::Frame::new()
            .fill(theme::surface(ui.visuals().dark_mode))
            .inner_margin(16)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.heading("SpaceMosaic");
                    ui.label(
                        RichText::new(t("read_only"))
                            .small()
                            .color(theme::accent(ui.visuals().dark_mode)),
                    );
                    self.settings(ui);
                });
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add(egui::Button::new(t("choose")).wrap_mode(egui::TextWrapMode::Extend))
                        .clicked()
                    {
                        self.choose();
                    }
                    if ui
                        .add_enabled(
                            (self.scope.is_some() || self.provider_scope.is_some())
                                && self.job.is_none(),
                            egui::Button::new(t("rescan")).wrap_mode(egui::TextWrapMode::Extend),
                        )
                        .clicked()
                    {
                        self.rescan();
                    }
                    if (self.job.is_some() || self.mobile_busy)
                        && ui
                            .add(
                                egui::Button::new(t("cancel"))
                                    .wrap_mode(egui::TextWrapMode::Extend),
                            )
                            .clicked()
                    {
                        if let Some(job) = &self.job {
                            job.cancel();
                        }
                        if self.mobile_busy {
                            bridge::begin();
                            self.mobile_busy = false;
                            self.notice = Some("cancelled");
                        }
                    }
                    if self.result.is_some() {
                        if ui.max_rect().width() < 500.0 {
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                            ui.menu_button(t("export"), |ui| {
                                for (key, json) in [("export_json", true), ("export_csv", false)] {
                                    if ui
                                        .add_enabled(
                                            self.export.is_none(),
                                            egui::Button::new(t(key)),
                                        )
                                        .clicked()
                                    {
                                        self.save(json);
                                        ui.close();
                                    }
                                }
                            });
                        } else {
                            for (key, json) in [("export_json", true), ("export_csv", false)] {
                                if ui
                                    .add_enabled(
                                        self.export.is_none(),
                                        egui::Button::new(t(key))
                                            .wrap_mode(egui::TextWrapMode::Extend),
                                    )
                                    .clicked()
                                {
                                    self.save(json);
                                }
                            }
                        }
                    }
                });
            });
    }
    fn summary(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        if let Some(job) = &self.job {
            let progress = job
                .progress
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label(t("scanning"));
                ui.label(i18n::counted(language, progress.objects as u64, "object"));
                ui.label(i18n::bytes(language, progress.known_bytes));
                ui.label(i18n::counted(language, progress.errors, "error"));
            });
        }
        if let Some(result) = &self.result {
            ui.label(
                RichText::new(t("scope"))
                    .small()
                    .color(theme::accent(ui.visuals().dark_mode)),
            );
            ui.label(&result.scope);
            ui.horizontal_wrapped(|ui| {
                let total = if self.allocated {
                    result.nodes[0].allocated
                } else {
                    result.nodes[0].logical
                };
                ui.label(RichText::new(measure(language, total)).size(28.0));
                ui.label(t(
                    if result.cancelled || result.limited || !result.nodes[0].complete {
                        "partial"
                    } else {
                        "complete"
                    },
                ));
                ui.label(i18n::counted(language, result.nodes.len() as u64, "object"));
                ui.label(i18n::counted(language, result.errors, "error"));
                ui.label(format!(
                    "{} ms",
                    i18n::integer(language, result.elapsed_millis)
                ));
            });
            ui.label(RichText::new(t("size_caveat")).small().weak());
            if let Some(reason) = result.incomplete_reason() {
                ui.label(t(reason));
            }
        }
        if self.preferences_write_failed {
            ui.label(t("preferences_save_failed"));
        }
        if let Some(notice) = self.notice {
            ui.label(t(notice));
        }
    }
    fn navigation(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for (page, key) in [
                (Page::Directory, "directory"),
                (Page::Largest, "largest"),
                (Page::Extensions, "extensions"),
            ] {
                if ui.selectable_value(&mut self.page, page, t(key)).changed() {
                    self.map_group = None;
                    if self.page == Page::Extensions {
                        self.allocated = false;
                    }
                    self.dirty = true;
                }
            }
        });
        if self.page != Page::Extensions {
            ui.horizontal_wrapped(|ui| {
                let label = ui.label(t("search"));
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.query)
                            .desired_width(240.0)
                            .hint_text(t("search_hint")),
                    )
                    .labelled_by(label.id)
                    .changed()
                {
                    self.dirty = true;
                }
                egui::ComboBox::from_label(t("sort"))
                    .selected_text(t(if self.sort == Sort::Size {
                        "size"
                    } else {
                        "name"
                    }))
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_value(&mut self.sort, Sort::Size, t("size"))
                            .changed()
                        {
                            self.dirty = true;
                        }
                        if ui
                            .selectable_value(&mut self.sort, Sort::Name, t("name"))
                            .changed()
                        {
                            self.dirty = true;
                        }
                    });
                if ui.checkbox(&mut self.allocated, t("allocated")).changed() {
                    self.dirty = true;
                }
            });
        }
        if self.page == Page::Directory || self.map_group.is_some() {
            self.path_navigation(ui);
        }
    }
    fn back_button(&mut self, ui: &mut egui::Ui) {
        if ui
            .add_enabled(
                self.directory != 0 || self.map_group.is_some(),
                egui::Button::new(i18n::text(self.language, "back")),
            )
            .clicked()
        {
            self.back();
        }
    }
    fn path_navigation(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        let compact = ui.available_width() < 400.0;
        ui.vertical(|ui| {
            if compact {
                self.back_button(ui);
            }
            ui.horizontal(|ui| {
                if !compact {
                    self.back_button(ui);
                    ui.separator();
                }
                egui::Frame::new()
                    .fill(theme::surface(ui.visuals().dark_mode))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::symmetric(8, 0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(t("path")).small().weak());
                            egui::ScrollArea::horizontal()
                                .id_salt("breadcrumb-path")
                                .min_scrolled_width(0.0)
                                .auto_shrink([false, true])
                                .stick_to_right(true)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                                        if let Some(result) = self.result.clone() {
                                            for (position, index) in result
                                                .breadcrumbs(self.directory)
                                                .into_iter()
                                                .enumerate()
                                            {
                                                if position > 0 {
                                                    ui.label(RichText::new("›").weak());
                                                }
                                                let node = &result.nodes[index];
                                                if index == self.directory
                                                    && self.map_group.is_none()
                                                {
                                                    ui.label(RichText::new(&node.name).strong())
                                                        .on_hover_text(&node.locator);
                                                } else if ui
                                                    .add_sized(
                                                        [0.0, ui.spacing().interact_size.y],
                                                        egui::Button::new(
                                                            RichText::new(&node.name).underline(),
                                                        )
                                                        .frame(false)
                                                        .wrap_mode(egui::TextWrapMode::Extend),
                                                    )
                                                    .on_hover_text(&node.locator)
                                                    .clicked()
                                                {
                                                    self.navigate(index);
                                                }
                                            }
                                        }
                                        if self.map_group.is_some() {
                                            ui.label(RichText::new("›").weak());
                                            ui.label(RichText::new(t("grouped_items")).italics());
                                        }
                                    });
                                });
                        });
                    });
            });
        })
        .response
    }
    fn table(&mut self, ui: &mut egui::Ui, height: f32) {
        let Some(result) = self.result.clone() else {
            return;
        };
        let language = self.language;
        let t = |key| i18n::text(language, key);
        ui.label(
            RichText::new(t("items"))
                .small()
                .color(theme::accent(ui.visuals().dark_mode)),
        );
        if self.rows.is_empty() {
            ui.label(t(
                if !result.nodes[self.directory].complete && self.query.is_empty() {
                    "incomplete_directory"
                } else {
                    "no_results"
                },
            ));
            return;
        }
        let tree =
            self.page == Page::Directory && self.query.is_empty() && self.map_group.is_none();
        let count = if tree {
            self.tree_rows.len()
        } else {
            self.rows.len()
        };
        let compact = ui.available_width() >= 600.0;
        let row_height = if compact { 32.0 } else { 48.0 };
        ui.horizontal(|ui| {
            ui.strong(t("name"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.strong(t(if self.allocated { "allocated" } else { "size" }));
            });
        });
        egui::ScrollArea::vertical()
            .id_salt((
                "object-list",
                self.map_group
                    .as_ref()
                    .and_then(|group| group.first())
                    .copied(),
            ))
            .max_height(height)
            .auto_shrink([false, false])
            .show_rows(ui, row_height, count, |ui, range| {
                for position in range {
                    let (index, depth) = if tree {
                        self.tree_rows[position]
                    } else {
                        (self.rows[position], 0)
                    };
                    let node = &result.nodes[index];
                    let size = measure(
                        language,
                        if self.allocated {
                            node.allocated
                        } else {
                            node.logical
                        },
                    );
                    ui.push_id(index, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.horizontal(|ui| {
                            ui.add_space((depth as f32 * 18.0).min(ui.available_width() * 0.35));
                            if tree
                                && node.kind == Kind::Directory
                                && !self.children[index].is_empty()
                            {
                                let open = self.expanded.contains(&index);
                                let response = ui.add_sized(
                                    [28.0, row_height],
                                    egui::Button::new("").frame_when_inactive(false),
                                );
                                response.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::WidgetType::Button,
                                        true,
                                        format!(
                                            "{}: {}",
                                            t(if open { "collapse" } else { "expand" }),
                                            node.name
                                        ),
                                    )
                                });
                                let center = response.rect.center();
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
                                if response
                                    .on_hover_text(format!("{}: {}", t("directory"), node.name))
                                    .clicked()
                                {
                                    if open {
                                        self.expanded.remove(&index);
                                    } else {
                                        self.expanded.insert(index);
                                    }
                                    self.dirty = true;
                                }
                            } else {
                                ui.add_space(28.0);
                            }
                            let (icon_rect, _) = ui.allocate_exact_size(
                                egui::vec2(16.0, row_height),
                                egui::Sense::hover(),
                            );
                            let center = icon_rect.center();
                            let icon = egui::Rect::from_center_size(center, egui::vec2(14.0, 11.0));
                            if node.kind == Kind::Directory {
                                ui.painter().rect_filled(
                                    icon,
                                    2.0,
                                    theme::accent(ui.visuals().dark_mode),
                                );
                                ui.painter().rect_filled(
                                    egui::Rect::from_min_size(
                                        icon.min - egui::vec2(0.0, 3.0),
                                        egui::vec2(7.0, 5.0),
                                    ),
                                    1.0,
                                    theme::accent(ui.visuals().dark_mode),
                                );
                            } else {
                                ui.painter().rect_stroke(
                                    icon,
                                    1.0,
                                    egui::Stroke::new(1.0, ui.visuals().text_color()),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            let width = (ui.available_width()
                                - if compact { 180.0 } else { 115.0 })
                            .max(35.0);
                            let response = ui.add_sized(
                                [width, row_height],
                                egui::Button::new(&node.name)
                                    .right_text(())
                                    .frame_when_inactive(self.selected == Some(index))
                                    .selected(self.selected == Some(index))
                                    .truncate(),
                            );
                            if response.clicked() {
                                self.select(index);
                            }
                            if response.double_clicked() && node.kind == Kind::Directory {
                                self.navigate(index);
                            }
                            response.on_hover_text(format!(
                                "{}: {}",
                                t(kind_key(node.kind)),
                                node.locator
                            ));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.monospace(size);
                                },
                            );
                        });
                    });
                }
            });
    }
    fn select(&mut self, index: usize) {
        self.selected = Some(index);
        if let Some(result) = &self.result {
            for ancestor in result
                .breadcrumbs(index)
                .into_iter()
                .take_while(|i| *i != index)
            {
                self.expanded.insert(ancestor);
            }
        }
        self.dirty = true;
    }
    /// Map a nested selection to its nearest visible ancestor or grouped tile.
    fn selected_tile(&self) -> Option<usize> {
        let result = self.result.as_ref()?;
        let mut index = self.selected?;
        loop {
            if let Some(position) = self.map_objects.iter().position(|i| *i == Some(index)) {
                return Some(position);
            }
            if self.grouped.contains(&index) {
                return self.map_objects.iter().position(Option::is_none);
            }
            index = result.nodes[index].parent?;
        }
    }
    fn map(&mut self, ui: &mut egui::Ui, height: f32, fit: bool) {
        let Some(result) = self.result.clone() else {
            return;
        };
        let language = self.language;
        let t = |key| i18n::text(language, key);
        ui.label(
            RichText::new(t("treemap"))
                .small()
                .color(theme::accent(ui.visuals().dark_mode)),
        );
        ui.label(RichText::new(t("map_help")).small().weak());
        let selected_tile = self.map_selected;
        if let Some(index) = self.selected {
            let path = format!("{}: {}", t("selected_item"), result.nodes[index].locator);
            ui.add(egui::Label::new(&path).truncate())
                .on_hover_text(path);
        }
        let objects = self.map_objects.clone();
        let weights = self.map_weights.clone();
        let group_height = if self.grouped.is_empty() {
            0.0
        } else {
            ui.spacing().interact_size.y + ui.spacing().item_spacing.y
        };
        let canvas_height = if fit {
            height.min(ui.available_height() - group_height).max(0.0)
        } else {
            height
        };
        if canvas_height < 1.0 {
            return;
        }
        let size = egui::vec2(ui.available_width(), canvas_height);
        let (bounds, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        if weights.is_empty() {
            ui.painter().text(
                bounds.center(),
                egui::Align2::CENTER_CENTER,
                t("no_map"),
                egui::FontId::proportional(14.0),
                ui.visuals().text_color(),
            );
            return;
        }
        for (position, rect) in treemap::squarify(
            &weights,
            treemap::Rect {
                width: bounds.width() as f64,
                height: bounds.height() as f64,
                ..Default::default()
            },
        ) {
            let area = egui::Rect::from_min_size(
                bounds.min + egui::vec2(rect.x as f32, rect.y as f32),
                egui::vec2(rect.width as f32, rect.height as f32),
            )
            .shrink(2.0);
            if area.width() < 1.0 || area.height() < 1.0 {
                continue;
            }
            let label = objects[position]
                .map(|i| result.nodes[i].name.as_str())
                .unwrap_or(t("grouped_items"));
            let selected = selected_tile == Some(position);
            let response =
                ui.interact(area, ui.id().with(("tile", position)), egui::Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
            });
            let color = tile_color(objects[position].map(|i| &result.nodes[i]));
            ui.painter().rect_filled(area, 4.0, color);
            if selected || response.has_focus() {
                ui.painter().rect_stroke(
                    area,
                    4.0,
                    egui::Stroke::new(2.0, ui.visuals().text_color()),
                    egui::StrokeKind::Inside,
                );
            }
            if area.width() > 72.0 && area.height() > 38.0 {
                let painter = ui.painter().with_clip_rect(area.shrink(7.0));
                painter.text(
                    area.min + egui::vec2(10.0, 10.0),
                    egui::Align2::LEFT_TOP,
                    objects[position]
                        .map(|i| format!("{}: {}", t(kind_key(result.nodes[i].kind)), label))
                        .unwrap_or_else(|| label.into()),
                    egui::FontId::proportional(13.0),
                    egui::Color32::WHITE,
                );
                painter.text(
                    area.min + egui::vec2(10.0, 29.0),
                    egui::Align2::LEFT_TOP,
                    i18n::bytes(language, weights[position].min(u64::MAX as f64) as u64),
                    egui::FontId::monospace(12.0),
                    egui::Color32::WHITE,
                );
            }
            let double_clicked = response.double_clicked();
            if response.clicked()
                || (response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            {
                if let Some(index) = objects[position] {
                    self.select(index);
                } else {
                    self.open_group();
                }
            }
            if (double_clicked
                || (response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))))
                && let Some(index) = objects[position]
                && result.nodes[index].kind == Kind::Directory
            {
                self.navigate(index);
            }
            response.on_hover_text(label);
        }
        if !self.grouped.is_empty()
            && ui
                .button(format!(
                    "{} ({})",
                    t("grouped_items"),
                    i18n::integer(language, self.grouped.len() as u64)
                ))
                .clicked()
        {
            self.open_group();
        }
    }
    fn details(&mut self, ui: &mut egui::Ui) {
        let Some(result) = self.result.clone() else {
            return;
        };
        let Some(index) = self.selected else {
            return;
        };
        let node = &result.nodes[index];
        let language = self.language;
        let t = |key| i18n::text(language, key);
        egui::Frame::new()
            .fill(theme::surface(ui.visuals().dark_mode))
            .inner_margin(16)
            .corner_radius(10)
            .show(ui, |ui| {
                ui.label(RichText::new(&node.name).strong());
                ui.label(&node.locator);
                ui.label(format!(
                    "{}: {}",
                    t("logical_bytes"),
                    measure(language, node.logical)
                ));
                ui.label(format!(
                    "{}: {}",
                    t("allocated"),
                    measure(language, node.allocated)
                ));
                if node.hard_link {
                    ui.label(t("hard_link_note"));
                }
                if let Some(error) = &node.error {
                    ui.label(t(error));
                }
                if node.kind == Kind::Directory && !node.complete {
                    ui.label(t("incomplete_directory"));
                    if let Some(reason) = result.incomplete_reason() {
                        ui.label(t(reason));
                    }
                    #[cfg(not(any(target_os = "android", target_os = "ios")))]
                    if let Some(path) = &node.path
                        && ui.button(t("scan_folder_separately")).clicked()
                    {
                        self.start(path.clone());
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button(t("copy_path")).clicked() {
                        #[cfg(not(any(target_os = "android", target_os = "ios")))]
                        ui.ctx().copy_text(node.locator.clone());
                        #[cfg(any(target_os = "android", target_os = "ios"))]
                        {
                            let _ = crate::mobile::request(
                                serde_json::json!({"op":"copy", "locator":node.locator}),
                            );
                        }
                    }
                    if node.kind == Kind::Directory && ui.button(t("open_folder")).clicked() {
                        self.navigate(index);
                    }
                    #[cfg(any(target_os = "android", target_os = "ios"))]
                    if ui.button(t("reveal")).clicked() {
                        let _ = crate::mobile::request(
                            serde_json::json!({"op":"view", "locator":node.locator}),
                        );
                    }
                    #[cfg(not(any(target_os = "android", target_os = "ios")))]
                    if let Some(path) = &node.path
                        && ui.button(t("reveal")).clicked()
                        && !reveal(path)
                    {
                        self.notice = Some("open_failed");
                    }
                });
            });
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    fn mobile_preview(&mut self, ui: &mut egui::Ui) {
        use crate::bridge::Status;
        let language = self.language;
        let t = |key| i18n::text(language, key);
        if self.preview.is_some() || self.mobile_busy {
            ui.label(t("mobile_scope"));
        }
        if self.mobile_busy {
            ui.spinner();
            ui.label(t("waiting"));
        }
        if let Some(listing) = &self.preview {
            ui.label(t(match listing.status {
                Status::Ready => "ready",
                Status::Cancelled => "cancelled",
                Status::Denied => "denied",
                Status::Unavailable => "unavailable",
            }));
            if let Some(error) = &listing.error {
                ui.label(t(error));
            }
            if let Some(scope) = &listing.scope {
                ui.label(scope);
            }
            if listing.truncated {
                ui.label(t("truncated"));
            }
            for entry in &listing.entries {
                ui.label(&entry.name);
                ui.label(
                    entry
                        .logical_bytes
                        .map(|b| i18n::bytes(language, b))
                        .unwrap_or_else(|| t("unknown").into()),
                );
            }
        }
    }
}
impl ExplorerApp {
    fn sort_extensions(&mut self) {
        let (column, descending) = self.extension_sort;
        self.extensions.sort_by(|a, b| {
            let order = match column {
                ExtensionColumn::Name => a.0.cmp(&b.0),
                ExtensionColumn::Size => {
                    a.2.overflow
                        .cmp(&b.2.overflow)
                        .then(a.2.known.cmp(&b.2.known))
                }
                ExtensionColumn::Files => a.1.cmp(&b.1),
            };
            (if descending { order.reverse() } else { order }).then(a.0.cmp(&b.0))
        });
    }
    fn extension_table(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let width = (ui.available_width()
            - ui.spacing().scroll.allocated_width()
            - 2.0 * ui.spacing().item_spacing.x)
            .max(0.0);
        let columns = [width * 0.50, width * 0.28, width * 0.22];
        let mut resorted = false;
        ui.horizontal(|ui| {
            for (index, column) in [
                ExtensionColumn::Name,
                ExtensionColumn::Size,
                ExtensionColumn::Files,
            ]
            .into_iter()
            .enumerate()
            {
                let active = self.extension_sort.0 == column;
                let heading = i18n::text(language, column.key());
                let direction = i18n::text(
                    language,
                    if self.extension_sort.1 {
                        "descending"
                    } else {
                        "ascending"
                    },
                );
                let label = if active {
                    format!(
                        "{heading} {}",
                        if self.extension_sort.1 { "↓" } else { "↑" }
                    )
                } else {
                    heading.to_owned()
                };
                let response = ui.add_sized(
                    [columns[index], 40.0],
                    egui::Button::new(label).selected(active).truncate(),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::Button,
                        true,
                        active,
                        if active {
                            format!("{heading}: {direction}")
                        } else {
                            heading.to_owned()
                        },
                    )
                });
                let clicked = response.clicked();
                response.on_hover_text(if active {
                    format!("{heading}: {direction}")
                } else {
                    heading.to_owned()
                });
                if clicked {
                    self.extension_sort = (
                        column,
                        if active {
                            !self.extension_sort.1
                        } else {
                            column != ExtensionColumn::Name
                        },
                    );
                    self.sort_extensions();
                    resorted = true;
                }
            }
        });
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("extensions")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .auto_shrink([false, true])
            .max_height(500.0);
        if resorted {
            scroll = scroll.vertical_scroll_offset(0.0);
        }
        scroll.show_rows(ui, 40.0, self.extensions.len(), |ui, range| {
            for index in range {
                let (name, count, size) = &self.extensions[index];
                let values = [
                    if name.is_empty() {
                        i18n::text(language, "no_extension").to_owned()
                    } else {
                        name.clone()
                    },
                    measure(language, *size),
                    i18n::counted(language, *count, "file"),
                ];
                ui.horizontal(|ui| {
                    for column in 0..3 {
                        ui.allocate_ui_with_layout(
                            egui::vec2(columns[column], 40.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_min_width(columns[column]);
                                ui.set_min_height(40.0);
                                let text = if column == 1 {
                                    RichText::new(&values[column]).monospace()
                                } else {
                                    RichText::new(&values[column])
                                };
                                ui.add(egui::Label::new(text).truncate())
                                    .on_hover_text(&values[column]);
                            },
                        );
                    }
                });
            }
        });
    }
    fn object_workspace(&mut self, ui: &mut egui::Ui, wide: bool) {
        let remaining = ui.clip_rect().bottom() - ui.cursor().top();
        if self.map_group.is_some() {
            ui.label(i18n::counted(
                self.language,
                self.rows.len() as u64,
                "object",
            ));
            if self.selected.is_some() {
                egui::CollapsingHeader::new(i18n::text(self.language, "selected_item"))
                    .id_salt("group-selection-details")
                    .show(ui, |ui| self.details(ui));
            }
            self.table(
                ui,
                if wide {
                    (remaining - 100.0).max(200.0)
                } else {
                    320.0
                },
            );
            return;
        }
        if wide && remaining >= 360.0 {
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), remaining),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    let panel = egui::Panel::bottom("treemap-panel")
                        .resizable(true)
                        .default_size(remaining * 0.48)
                        .size_range(160.0..=(remaining - 140.0))
                        .show(ui, |ui| {
                            ui.set_min_height(ui.available_height());
                            ui.add_space(12.0);
                            self.map(ui, ui.available_height(), true);
                        });
                    let strip = egui::Rect::from_min_size(
                        panel.response.rect.min,
                        egui::vec2(panel.response.rect.width(), 12.0),
                    );
                    let painter = ui.painter();
                    painter.rect_filled(strip, 0.0, theme::surface(ui.visuals().dark_mode));
                    painter.hline(
                        strip.x_range(),
                        strip.center().y,
                        egui::Stroke::new(2.0, theme::accent(ui.visuals().dark_mode)),
                    );
                    let label = i18n::text(self.language, "resize_map");
                    let galley = painter.layout_no_wrap(
                        label.into(),
                        egui::FontId::proportional(10.0),
                        ui.visuals().text_color(),
                    );
                    let label_rect = egui::Rect::from_center_size(
                        strip.center(),
                        galley.size() + egui::vec2(20.0, 0.0),
                    );
                    painter.rect_filled(label_rect, 3.0, theme::surface(ui.visuals().dark_mode));
                    painter.galley(
                        label_rect.center() - galley.size() * 0.5,
                        galley,
                        ui.visuals().text_color(),
                    );
                    let tree_bounds = ui.available_rect_before_wrap();
                    ui.scope_builder(egui::UiBuilder::new().max_rect(tree_bounds), |ui| {
                        ui.set_clip_rect(tree_bounds.intersect(ui.clip_rect()));
                        egui::ScrollArea::vertical()
                            .id_salt("tree-and-details")
                            .show(ui, |ui| {
                                if let Some(name) = self.selected.and_then(|i| {
                                    self.result.as_ref().map(|r| r.nodes[i].name.clone())
                                }) {
                                    egui::CollapsingHeader::new(name)
                                        .id_salt("selection-details")
                                        .show(ui, |ui| self.details(ui));
                                }
                                self.table(ui, (tree_bounds.height() - 70.0).max(80.0));
                            });
                    });
                },
            );
        } else {
            // Natural vertical flow preserves access when toolbars, translation,
            // text scaling, or an expanded details panel consume the viewport.
            self.table(ui, 240.0);
            ui.add_space(12.0);
            ui.separator();
            self.map(ui, 260.0, false);
            ui.add_space(12.0);
            self.details(ui);
        }
    }
    fn draw(&mut self, ui: &mut egui::Ui) {
        self.poll(ui.ctx());
        #[cfg(any(target_os = "android", target_os = "ios"))]
        if self.native_settings != Some((self.appearance, self.language)) {
            let preference = match self.appearance {
                Appearance::System => 0,
                Appearance::Light => 1,
                Appearance::Dark => 2,
            };
            #[cfg(target_os = "android")]
            crate::android::configure(preference, i18n::TAGS[self.language]);
            #[cfg(target_os = "ios")]
            crate::ios::configure(preference, i18n::TAGS[self.language]);
            self.native_settings = Some((self.appearance, self.language));
        }
        #[cfg(target_os = "ios")]
        crate::ios::install_accessibility();
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        for path in ui.ctx().input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect::<Vec<_>>()
        }) {
            if path.is_dir() {
                self.start(path);
            }
        }
        if ui
            .ctx()
            .input(|i| i.modifiers.alt && i.key_pressed(egui::Key::ArrowLeft))
        {
            self.back();
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ui, |ui| {
                #[cfg(target_os = "ios")]
                ui.set_max_height((ui.available_height() - 80.0).max(0.0));
                let desktop = ui.available_width() >= 700.0 && ui.available_height() >= 600.0;
                if desktop {
                    ui.spacing_mut().interact_size.y = 30.0;
                    ui.spacing_mut().button_padding.y = 5.0;
                    ui.spacing_mut().item_spacing.y = 6.0;
                }
                if desktop {
                    self.header(ui);
                    ui.add_space(12.0);
                } else {
                    ui.spacing_mut().interact_size.y = 48.0;
                }
                egui::ScrollArea::vertical()
                    .id_salt("workspace")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if !desktop {
                            self.header(ui);
                            ui.add_space(12.0);
                        }
                        self.summary(ui);
                        if self.result.is_some() {
                            ui.add_space(12.0);
                            self.navigation(ui);
                            self.refresh_rows();
                            ui.add_space(12.0);
                            if self.page == Page::Extensions {
                                self.extension_table(ui);
                                ui.add_space(12.0);
                                self.details(ui);
                            } else {
                                self.object_workspace(ui, desktop);
                            }
                        } else if self.job.is_none() && self.preview.is_none() {
                            let language = self.language;
                            let t = |key| i18n::text(language, key);
                            ui.add_space(36.0);
                            ui.label(RichText::new(t("empty_heading")).size(30.0));
                            ui.label(t(if cfg!(any(target_os = "android", target_os = "ios")) {
                                "mobile_scope"
                            } else {
                                "empty_body"
                            }));
                            ui.add_space(14.0);
                            ui.label(RichText::new(t("privacy_note")).weak());
                        }
                        #[cfg(any(target_os = "android", target_os = "ios"))]
                        if self.result.is_none() && self.job.is_none() {
                            self.mobile_preview(ui);
                        }
                    });
            });
        self.settings_window(ui.ctx());
        #[cfg(any(target_os = "android", target_os = "ios"))]
        crate::mobile::focus(self.directory, self.map_group.is_some(), self.settings_open);
    }
}
// Stable colors keep the same folder or file type recognizable across navigation.
fn tile_color(node: Option<&scan::Node>) -> egui::Color32 {
    let palette = [
        0x285b87, 0x7850a1, 0x267065, 0x95602b, 0x93465e, 0x495fa1, 0x556c2c, 0x8b4d31,
    ];
    let Some(node) = node else {
        return egui::Color32::from_gray(72);
    };
    let key = if node.kind == Kind::Directory {
        node.locator.clone()
    } else {
        PathBuf::from(&node.name)
            .extension()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    };
    let hash = key.bytes().fold(2166136261_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16777619)
    });
    let color = palette[hash as usize % palette.len()];
    egui::Color32::from_rgb((color >> 16) as u8, (color >> 8) as u8, color as u8)
}
fn kind_key(kind: Kind) -> &'static str {
    match kind {
        Kind::Directory => "directory",
        Kind::File => "file",
        Kind::Link => "link",
        Kind::Other => "other",
    }
}
fn measure(language: usize, size: scan::Measure) -> String {
    i18n::measure(language, size)
}
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn reveal(path: &Path) -> bool {
    let parent = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    #[cfg(target_os = "macos")]
    let command = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
    #[cfg(target_os = "windows")]
    let command = std::process::Command::new("explorer.exe")
        .arg(parent)
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let command = std::process::Command::new("xdg-open").arg(parent).spawn();
    let _ = parent;
    command.is_ok()
}

impl eframe::App for ExplorerApp {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
    #[cfg(target_os = "android")]
    fn raw_input_hook(&mut self, context: &egui::Context, input: &mut egui::RawInput) {
        crate::android::apply_safe_area(context, input);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let zoom = ui.ctx().zoom_factor();
            self.preferences.window = ui.ctx().input(|input| {
                WindowState::capture(
                    self.preferences.window,
                    input.viewport(),
                    input
                        .viewport()
                        .inner_rect
                        .map_or(input.viewport_rect().size(), |rect| rect.size()),
                    zoom,
                )
            });
        }
        self.draw(ui);
        self.persist_preferences(false);
        if self.preferences != self.saved_preferences && self.preferences_path.is_some() {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }
    }
}

impl Drop for ExplorerApp {
    fn drop(&mut self) {
        self.persist_preferences(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_preferences_writes_are_throttled_and_recover() {
        let files = preferences::tests::Fixture::new();
        std::fs::write(files.path(), b"blocking parent").unwrap();
        let destination = files.path().join("settings.json");
        let mut app = ExplorerApp::empty();
        app.preferences_path = Some(destination.clone());
        app.appearance = Appearance::Dark;
        app.persist_preferences(false);
        assert!(app.preferences_write_failed);
        let failed_at = app.last_preferences_write;
        for _ in 0..5 {
            app.persist_preferences(false);
        }
        assert_eq!(app.last_preferences_write, failed_at);
        let context = egui::Context::default();
        let output = frame(&mut app, &context, vec![]);
        assert!(output.shapes.iter().any(|shape|matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.text()==i18n::text(app.language,"preferences_save_failed"))));
        std::fs::remove_file(files.path()).unwrap();
        app.last_preferences_write = Instant::now() - Duration::from_secs(2);
        app.persist_preferences(false);
        assert!(!app.preferences_write_failed);
        assert_eq!(Preferences::load(&destination).appearance, Appearance::Dark);
    }
    #[test]
    fn unknown_sizes_never_look_like_verified_empty_objects_in_any_language() {
        for language in 0..i18n::TAGS.len() {
            assert_eq!(
                measure(language, scan::Measure::default()),
                i18n::bytes(language, 0)
            );
            let unknown = measure(
                language,
                scan::Measure {
                    unknown: 1,
                    ..Default::default()
                },
            );
            assert!(unknown.starts_with(i18n::text(language, "unknown")));
            assert!(!unknown.contains(&i18n::bytes(language, 0)));
            assert_eq!(
                measure(
                    language,
                    scan::Measure {
                        known: 125739,
                        ..Default::default()
                    }
                ),
                i18n::bytes(language, 125739)
            );
            assert!(
                measure(
                    language,
                    scan::Measure {
                        known: 10,
                        unknown: 1,
                        ..Default::default()
                    }
                )
                .starts_with(&i18n::bytes(language, 10))
            );
        }
    }
    #[test]
    fn incomplete_directory_explains_the_limit_and_can_be_scanned_separately() {
        let files = preferences::tests::Fixture::new();
        let directory = files.path().with_extension("directory");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("payload"), vec![1; 125739]).unwrap();
        let mut app = fixture();
        let result = Arc::make_mut(app.result.as_mut().unwrap());
        result.nodes.truncate(2);
        result.limited = true;
        result.limit_reason = Some(scan::LimitReason::MemoryLimit);
        for node in &mut result.nodes {
            node.complete = false;
            node.logical = scan::Measure {
                unknown: 1,
                ..Default::default()
            };
        }
        result.nodes[1].path = Some(directory.clone());
        app.directory = 1;
        app.selected = Some(1);
        let context = egui::Context::default();
        context.set_fonts(crate::fonts::definitions());
        theme::install(&context);
        for _ in 0..3 {
            frame(&mut app, &context, vec![]);
        }
        let output = context.run_ui(egui::RawInput::default(), |ui| app.details(ui));
        let mut output = output;
        output.textures_delta.clear();
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.text()==i18n::text(0,"scan_memory_limit"))));
        let pos = text_position(&output, i18n::text(0, "scan_folder_separately"));
        for pressed in [true, false] {
            let mut output = context.run_ui(
                egui::RawInput {
                    events: vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ui| app.details(ui),
            );
            output.textures_delta.clear();
        }
        assert_eq!(app.scope.as_ref(), Some(&directory));
        let deadline = Instant::now() + Duration::from_secs(2);
        let result = loop {
            if let Some(result) = app.job.as_ref().and_then(scan::Job::take) {
                break result;
            }
            assert!(Instant::now() < deadline, "separate scan did not finish");
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(result.nodes[0].logical.known, 125739);
        assert!(result.nodes[0].complete);
    }
    #[test]
    fn settings_save_immediately_and_shutdown_flushes_pending_window_changes() {
        let files = preferences::tests::Fixture::new();
        let mut app = ExplorerApp::empty();
        app.preferences_path = Some(files.path());
        app.appearance = Appearance::Light;
        app.preferences.language = Some("fr".into());
        app.persist_preferences(false);
        let saved = Preferences::load(&files.path());
        assert_eq!(saved.language.as_deref(), Some("fr"));
        assert_eq!(saved.appearance, Appearance::Light);
        app.preferences.window = Some(crate::preferences::WindowState {
            position: Some([240, 160]),
            size: [900.0, 640.0],
            maximized: false,
        });
        app.last_preferences_write = Instant::now();
        app.persist_preferences(false);
        assert_eq!(Preferences::load(&files.path()).window, None);
        let expected = app.preferences.clone();
        drop(app);
        assert_eq!(Preferences::load(&files.path()), expected);
    }
    fn fixture() -> ExplorerApp {
        let mut app = ExplorerApp::empty();
        let nodes = [
            (None, "Scope", Kind::Directory, 120),
            (Some(0), "Photos", Kind::Directory, 100),
            (Some(1), "Trips", Kind::Directory, 100),
            (Some(2), "image.jpg", Kind::File, 100),
            (Some(0), "notes.txt", Kind::File, 20),
        ]
        .into_iter()
        .map(|(parent, name, kind, known)| scan::Node {
            parent,
            name: name.into(),
            locator: name.into(),
            native_locator: name.into(),
            kind,
            logical: scan::Measure {
                known,
                ..Default::default()
            },
            allocated: scan::Measure::default(),
            complete: true,
            hard_link: false,
            error: None,
            path: None,
        })
        .collect();
        app.result = Some(Arc::new(ScanResult {
            schema_version: 1,
            started_unix_seconds: 0,
            elapsed_millis: 1,
            scope: "Scope".into(),
            cancelled: false,
            limited: false,
            limit_reason: None,
            cross_filesystems: false,
            errors: 0,
            nodes,
        }));
        app.language = 0;
        app
    }
    fn frame(
        app: &mut ExplorerApp,
        context: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        frame_at(app, context, events, egui::vec2(1180.0, 900.0))
    }
    fn frame_at(
        app: &mut ExplorerApp,
        context: &egui::Context,
        events: Vec<egui::Event>,
        size: egui::Vec2,
    ) -> egui::FullOutput {
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ui| app.draw(ui),
        );
        output.textures_delta.clear();
        output
    }
    fn text_position(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing rendered text: {label}"))
    }
    fn click(app: &mut ExplorerApp, context: &egui::Context, pos: egui::Pos2) -> egui::FullOutput {
        for pressed in [true, false] {
            frame(
                app,
                context,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        frame(app, context, vec![])
    }
    #[test]
    fn phone_export_buttons_do_not_turn_into_vertical_letter_columns() {
        for language in 0..i18n::TAGS.len() {
            let context = egui::Context::default();
            theme::install(&context);
            context.set_fonts(crate::fonts::definitions());
            let mut app = fixture();
            app.language = language;
            for _ in 0..4 {
                frame_at(&mut app, &context, vec![], egui::vec2(360.0, 800.0));
            }
            let output = frame_at(&mut app, &context, vec![], egui::vec2(360.0, 800.0));
            for key in ["choose", "rescan", "export"] {
                let label = i18n::text(language, key);
                let text = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == label => Some(text),
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("missing phone action: {language} {key}"));
                assert_eq!(
                    text.galley.rows.len(),
                    1,
                    "wrapped phone action: {language} {key}"
                );
                assert!(
                    text.pos.y + text.galley.size().y < 330.0,
                    "phone toolbar consumes the page: {language} {key}"
                );
            }
        }
    }
    #[test]
    fn compact_export_menu_keeps_both_formats_available() {
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        let mut app = fixture();
        let size = egui::vec2(360.0, 800.0);
        for _ in 0..4 {
            frame_at(&mut app, &context, vec![], size);
        }
        let output = frame_at(&mut app, &context, vec![], size);
        let pos = text_position(&output, "Export");
        for pressed in [true, false] {
            frame_at(
                &mut app,
                &context,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
                size,
            );
        }
        let output = frame_at(&mut app, &context, vec![], size);
        for label in ["Export JSON", "Export CSV"] {
            let pos = text_position(&output, label);
            assert!(pos.x >= 0.0 && pos.x < size.x && pos.y < size.y);
        }
    }
    #[test]
    fn settings_close_stays_inside_safe_area_after_rotation_and_large_text() {
        for appearance in [Appearance::Light, Appearance::Dark] {
            let context = egui::Context::default();
            theme::install(&context);
            context.set_fonts(crate::fonts::definitions());
            context.set_theme(appearance.preference());
            let mut app = fixture();
            app.language = 1;
            app.settings_open = true;
            for (size, top, bottom, zoom) in [
                (egui::vec2(412.0, 915.0), 32.0, 88.0, 1.0),
                (egui::vec2(915.0, 412.0), 24.0, 74.0, 1.0),
                (egui::vec2(412.0, 915.0), 32.0, 88.0, 1.6),
            ] {
                context.set_zoom_factor(zoom);
                let size = size / zoom;
                let top = top / zoom;
                let bottom = bottom / zoom;
                for _ in 0..4 {
                    let mut output = context.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                            safe_area_insets: Some(egui::SafeAreaInsets(egui::epaint::MarginF32 {
                                top,
                                bottom,
                                ..Default::default()
                            })),
                            ..Default::default()
                        },
                        |ui| app.draw(ui),
                    );
                    output.textures_delta.clear();
                }
                let window =
                    egui::containers::AreaState::load(&context, egui::Id::new("settings-window"))
                        .unwrap();
                assert!(
                    window.rect().top() >= top,
                    "settings title is under the status bar"
                );
                assert!(
                    window.rect().right() <= size.x,
                    "settings close is off screen"
                );
            }
            // Feed Escape through a real frame rather than mutating widget state.
            frame_at(
                &mut app,
                &context,
                vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
                egui::vec2(412.0, 915.0),
            );
            assert!(!app.settings_open);
        }
    }
    #[test]
    fn file_type_headers_sort_each_column_in_both_directions_without_rescanning() {
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        let mut app = fixture();
        app.page = Page::Extensions;
        app.extensions = [("txt", 2, 20), ("jpg", 5, 10), ("", 1, 30), ("zip", 3, 20)]
            .into_iter()
            .map(|(name, count, known)| {
                (
                    name.into(),
                    count,
                    scan::Measure {
                        known,
                        ..Default::default()
                    },
                )
            })
            .collect();
        app.sort_extensions();
        let result = app.result.clone().unwrap();
        for (column, descending, expected) in [
            (ExtensionColumn::Name, false, ["", "jpg", "txt", "zip"]),
            (ExtensionColumn::Name, true, ["zip", "txt", "jpg", ""]),
            (ExtensionColumn::Size, true, ["", "txt", "zip", "jpg"]),
            (ExtensionColumn::Size, false, ["jpg", "txt", "zip", ""]),
            (ExtensionColumn::Files, true, ["jpg", "zip", "txt", ""]),
            (ExtensionColumn::Files, false, ["", "txt", "zip", "jpg"]),
        ] {
            for _ in 0..3 {
                frame(&mut app, &context, vec![]);
            }
            let output = frame(&mut app, &context, vec![]);
            let heading = i18n::text(0, column.key());
            let label = if app.extension_sort.0 == column {
                format!("{heading} {}", if app.extension_sort.1 { "↓" } else { "↑" })
            } else {
                heading.to_owned()
            };
            let position = text_position(&output, &label);
            click(&mut app, &context, position);
            assert_eq!(app.extension_sort, (column, descending));
            assert_eq!(
                app.extensions
                    .iter()
                    .map(|row| row.0.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            assert!(Arc::ptr_eq(app.result.as_ref().unwrap(), &result));
            assert!(app.job.is_none());
        }
    }
    #[test]
    fn file_type_sorting_preserves_unknown_sizes_and_orders_overflow_explicitly() {
        let mut app = fixture();
        let unknown = scan::Measure {
            unknown: 2,
            ..Default::default()
        };
        app.extensions = vec![
            ("unknown".into(), 2, unknown),
            ("empty".into(), 1, scan::Measure::default()),
            (
                "large".into(),
                3,
                scan::Measure {
                    known: u64::MAX,
                    ..Default::default()
                },
            ),
            (
                "overflow".into(),
                4,
                scan::Measure {
                    known: u64::MAX,
                    overflow: true,
                    ..Default::default()
                },
            ),
        ];
        app.sort_extensions();
        assert_eq!(
            app.extensions
                .iter()
                .map(|row| row.0.as_str())
                .collect::<Vec<_>>(),
            ["overflow", "large", "empty", "unknown"]
        );
        app.extension_sort.1 = false;
        app.sort_extensions();
        assert_eq!(
            app.extensions
                .iter()
                .map(|row| row.0.as_str())
                .collect::<Vec<_>>(),
            ["empty", "unknown", "large", "overflow"]
        );
        assert_eq!(app.extensions[1].2, unknown);
    }
    #[test]
    fn extension_columns_do_not_move_when_long_names_scroll_into_view() {
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        let mut app = fixture();
        app.page = Page::Extensions;
        app.extensions = (0..200)
            .map(|i| {
                (
                    if i < 12 {
                        format!("ext{i}")
                    } else {
                        "long-extension-".repeat(40)
                    },
                    10,
                    scan::Measure {
                        known: 4096,
                        ..Default::default()
                    },
                )
            })
            .collect();
        for _ in 0..5 {
            frame(&mut app, &context, vec![]);
        }
        let first = frame(&mut app, &context, vec![]);
        let size_x = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == i18n::bytes(0, 4096) => {
                        Some(text.pos.x)
                    }
                    _ => None,
                })
                .expect("missing extension sizes")
        };
        let before = size_x(&first);
        frame(
            &mut app,
            &context,
            vec![
                egui::Event::PointerMoved(egui::pos2(300.0, 450.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -900.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: Default::default(),
                },
            ],
        );
        for _ in 0..16 {
            frame(&mut app, &context, vec![]);
        }
        let after = frame(&mut app, &context, vec![]);
        assert!(after.shapes.iter().any(|shape| matches!(&shape.shape,egui::Shape::Text(text) if text.galley.text().contains("long-extension-"))));
        assert!(
            (size_x(&after) - before).abs() < 1.0,
            "extension size column moved while scrolling"
        );
    }
    #[test]
    fn map_divider_is_visible_and_draggable_in_both_themes() {
        for appearance in [Appearance::Light, Appearance::Dark] {
            let context = egui::Context::default();
            theme::install(&context);
            context.set_fonts(crate::fonts::definitions());
            context.set_theme(appearance.preference());
            let mut app = fixture();
            for _ in 0..12 {
                frame(&mut app, &context, vec![]);
            }
            let output = frame(&mut app, &context, vec![]);
            let grip = text_position(&output, i18n::text(0, "resize_map"));
            let state = || {
                egui::containers::panel::PanelState::load(&context, egui::Id::new("treemap-panel"))
                    .unwrap()
            };
            let before = state().size().y;
            frame(
                &mut app,
                &context,
                vec![
                    egui::Event::PointerMoved(grip),
                    egui::Event::PointerButton {
                        pos: grip,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    },
                ],
            );
            frame(
                &mut app,
                &context,
                vec![egui::Event::PointerMoved(grip + egui::vec2(0.0, 40.0))],
            );
            frame(
                &mut app,
                &context,
                vec![egui::Event::PointerButton {
                    pos: grip + egui::vec2(0.0, 40.0),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
            );
            assert!(
                (state().size().y - before).abs() > 20.0,
                "divider drag did not resize map"
            );
        }
    }
    #[test]
    fn language_popup_stays_open_and_selects_a_language_in_settings() {
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        let files = preferences::tests::Fixture::new();
        let mut app = ExplorerApp::empty();
        app.preferences_path = Some(files.path());
        app.language = 0;
        frame(&mut app, &context, vec![]);
        let output = frame(&mut app, &context, vec![]);
        let pos = text_position(&output, "Settings");
        click(&mut app, &context, pos);
        let output = frame(&mut app, &context, vec![]);
        assert!(app.settings_open);
        let pos = text_position(&output, "Dark");
        click(&mut app, &context, pos);
        let output = frame(&mut app, &context, vec![]);
        let pos = text_position(&output, "System (en)");
        click(&mut app, &context, pos);
        let output = frame(&mut app, &context, vec![]);
        let pos = text_position(&output, "ru");
        click(&mut app, &context, pos);
        assert_eq!(app.language, 1);
        assert!(app.settings_open);
        drop(app);
        let restored_context = egui::Context::default();
        let restored = ExplorerApp::new_with_preferences(
            &eframe::CreationContext::_new_kittest(restored_context.clone()),
            Some(files.path()),
        );
        assert_eq!(restored.language, 1);
        assert_eq!(restored.appearance, Appearance::Dark);
        assert_eq!(restored.preferences.language.as_deref(), Some("ru"));
        assert_eq!(restored_context.theme(), egui::Theme::Dark);
    }
    #[test]
    fn tree_expansion_sorting_and_map_selection_preserve_hierarchy() {
        let mut app = fixture();
        app.refresh_rows();
        assert_eq!(app.tree_rows, vec![(0, 0), (1, 1), (4, 1)]);
        app.select(3);
        app.refresh_rows();
        assert_eq!(app.tree_rows, vec![(0, 0), (1, 1), (2, 2), (3, 3), (4, 1)]);
        app.expanded.remove(&1);
        app.dirty = true;
        app.refresh_rows();
        assert_eq!(app.tree_rows.len(), 3);
        assert_eq!(app.selected, Some(3));
        app.sort = Sort::Name;
        app.dirty = true;
        app.refresh_rows();
        assert_eq!(app.tree_rows[1].0, 4);
        assert_eq!(app.map_objects.iter().flatten().count(), 2);
        app.navigate(1);
        app.refresh_rows();
        assert_eq!(app.tree_rows[0], (1, 0));
        app.back();
        assert_eq!(app.directory, 0);
    }
    #[test]
    fn desktop_tree_stays_above_map_with_both_visible() {
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        let mut app = fixture();
        frame(&mut app, &context, vec![]);
        for _ in 0..12 {
            frame(&mut app, &context, vec![]);
        }
        let output = frame(&mut app, &context, vec![]);
        let panel =
            egui::containers::panel::PanelState::load(&context, egui::Id::new("treemap-panel"))
                .unwrap();
        assert!(
            panel.size().y >= 200.0,
            "map panel shrank: {:?}",
            panel.size()
        );
        let tree = text_position(&output, "CONTENTS");
        let map = text_position(&output, "SPACE MAP");
        assert!(tree.y < map.y);
        assert!(map.y < 800.0);
    }
    #[test]
    fn result_rows_do_not_overlap_map_after_resize_or_text_scaling() {
        for language in 0..i18n::TAGS.len() {
            for appearance in [Appearance::Light, Appearance::Dark] {
                let context = egui::Context::default();
                theme::install(&context);
                context.set_fonts(crate::fonts::definitions());
                context.set_theme(appearance.preference());
                let mut app = fixture();
                app.language = language;
                app.select(3);
                for (size, zoom) in [
                    (egui::vec2(1180.0, 900.0), 1.0),
                    (egui::vec2(744.0, 684.0), 1.0),
                    (egui::vec2(1180.0, 900.0), 1.6),
                    (egui::vec2(320.0, 640.0), 1.0),
                ] {
                    context.set_zoom_factor(zoom);
                    for _ in 0..4 {
                        frame_at(&mut app, &context, vec![], size);
                    }
                    let mut output = frame_at(&mut app, &context, vec![], size);
                    let folder_color = tile_color(Some(&app.result.as_ref().unwrap().nodes[1]));
                    let map_position = |output: &egui::FullOutput| {
                        output.shapes.iter().find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text)
                                if text.galley.text() == i18n::text(language, "treemap") =>
                            {
                                Some(text.pos.y)
                            }
                            egui::Shape::Rect(rect) if rect.fill == folder_color => {
                                Some(rect.rect.top())
                            }
                            _ => None,
                        })
                    };
                    if map_position(&output).is_none() {
                        for _ in 0..8 {
                            output = frame_at(
                                &mut app,
                                &context,
                                vec![
                                    egui::Event::PointerMoved(egui::pos2(
                                        24.0,
                                        size.y / zoom - 40.0,
                                    )),
                                    egui::Event::MouseWheel {
                                        unit: egui::MouseWheelUnit::Point,
                                        phase: egui::TouchPhase::Move,
                                        delta: egui::vec2(0.0, -180.0),
                                        modifiers: Default::default(),
                                    },
                                ],
                                size,
                            );
                            if map_position(&output).is_some() {
                                break;
                            }
                        }
                    }
                    let map_y = map_position(&output).unwrap_or_else(|| {
                        panic!("map is unreachable: {size:?}, zoom {zoom}, language {language}")
                    });
                    for shape in &output.shapes {
                        if let egui::Shape::Text(text) = &shape.shape
                            && ["Scope", "Photos", "Trips", "notes.txt"]
                                .contains(&text.galley.text())
                        {
                            let bounds = egui::Rect::from_min_size(text.pos, text.galley.size())
                                .intersect(shape.clip_rect);
                            if bounds.is_positive() {
                                assert!(
                                    bounds.bottom() < map_y,
                                    "tree overlaps map: {size:?}, zoom {zoom}, language {language}, {bounds:?} >= {map_y}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn selected_tree_row_has_a_background_without_hover() {
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        let mut app = fixture();
        app.select(3);
        for _ in 0..3 {
            frame(&mut app, &context, vec![]);
        }
        let output = frame(
            &mut app,
            &context,
            vec![egui::Event::PointerMoved(egui::Pos2::ZERO)],
        );
        let text = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "image.jpg" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .next_back()
            .unwrap();
        let fill = context.style_of(context.theme()).visuals.selection.bg_fill;
        assert!(
            output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Rect(rect) if rect.fill == fill && rect.rect.contains(text))),
            "selected row has no persistent selection background"
        );
    }
    #[test]
    fn nested_selection_marks_its_visible_containing_tile_and_path() {
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        let mut app = fixture();
        app.select(3);
        for _ in 0..3 {
            frame(&mut app, &context, vec![]);
        }
        let output = frame(&mut app, &context, vec![]);
        assert_eq!(app.selected_tile(), Some(0));
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.text().contains("Selected item: image.jpg"))));
        app.navigate(2);
        app.select(3);
        app.refresh_rows();
        assert_eq!(app.map_objects[app.selected_tile().unwrap()], Some(3));
        app.back();
        app.refresh_rows();
        assert_eq!(app.directory, 1);
    }
    #[test]
    fn all_languages_render_at_phone_and_desktop_widths_in_each_theme() {
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        let mut app = fixture();
        app.settings_open = true;
        for language in 0..i18n::TAGS.len() {
            app.language = language;
            for width in [320.0, 1180.0] {
                for appearance in [Appearance::Light, Appearance::Dark] {
                    context.set_theme(appearance.preference());
                    let mut output = context.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 820.0),
                            )),
                            ..Default::default()
                        },
                        |ui| app.draw(ui),
                    );
                    assert!(!output.shapes.is_empty());
                    output.textures_delta.clear();
                }
            }
        }
    }
    #[test]
    fn long_breadcrumbs_remain_one_path_row_in_all_languages_and_themes() {
        for language in 0..i18n::TAGS.len() {
            for appearance in [Appearance::Light, Appearance::Dark] {
                for width in [320.0, 1180.0] {
                    let context = egui::Context::default();
                    context.set_fonts(crate::fonts::definitions());
                    theme::install(&context);
                    context.set_theme(appearance.preference());
                    context.set_zoom_factor(1.6);
                    let mut app = fixture();
                    app.language = language;
                    let result = Arc::make_mut(app.result.as_mut().unwrap());
                    for index in 5..20 {
                        result.nodes.push(scan::Node {
                            parent: Some(if index == 5 { 2 } else { index - 1 }),
                            name: if index == 19 {
                                "current-folder".into()
                            } else {
                                format!("a-very-long-parent-directory-{index}")
                            },
                            locator: format!("/Scope/Photos/Trips/folder-{index}"),
                            native_locator: String::new(),
                            kind: Kind::Directory,
                            logical: scan::Measure::default(),
                            allocated: scan::Measure::default(),
                            complete: true,
                            hard_link: false,
                            error: None,
                            path: None,
                        });
                    }
                    app.directory = 19;
                    let mut path_rect = egui::Rect::NOTHING;
                    let mut output = egui::FullOutput::default();
                    for _ in 0..4 {
                        output = context.run_ui(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(width / 1.6, 500.0),
                                )),
                                ..Default::default()
                            },
                            |ui| {
                                path_rect = app.path_navigation(ui).rect;
                            },
                        );
                        output.textures_delta.clear();
                    }
                    assert!(
                        path_rect.right() <= width / 1.6 + 1.0,
                        "path exceeds viewport: {language} {width} {path_rect:?}"
                    );
                    assert!(
                        path_rect.height() < if width / 1.6 < 400.0 { 130.0 } else { 90.0 },
                        "path navigation grew beyond its compact layout: {path_rect:?}"
                    );
                    let mut centers = Vec::new();
                    let mut current_visible = false;
                    let mut path_label_visible = false;
                    for shape in &output.shapes {
                        if let egui::Shape::Text(text) = &shape.shape {
                            let label = text.galley.text();
                            let center = text.pos + text.galley.size() * 0.5;
                            if label == "›"
                                || label == "current-folder"
                                || label.starts_with("a-very-long-parent-")
                            {
                                centers.push(center.y);
                            }
                            if label == "current-folder" {
                                current_visible = shape.clip_rect.contains(center);
                            }
                            if label == i18n::text(language, "path") {
                                path_label_visible = shape.clip_rect.contains(center);
                            }
                        }
                    }
                    assert!(
                        current_visible && path_label_visible,
                        "current folder or path label is hidden: {language} {width}"
                    );
                    assert!(
                        centers.iter().all(|y| (*y - centers[0]).abs() < 3.0),
                        "breadcrumb segments wrapped: {centers:?}"
                    );
                }
            }
        }
    }
    #[test]
    fn breadcrumb_links_navigate_to_ancestors_and_back_remains_separate() {
        let context = egui::Context::default();
        context.set_fonts(crate::fonts::definitions());
        theme::install(&context);
        let mut app = fixture();
        app.navigate(2);
        for _ in 0..4 {
            frame(&mut app, &context, vec![]);
        }
        let output = frame(&mut app, &context, vec![]);
        let photos = text_position(&output, "Photos");
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.text()=="Photos"
                && text.galley.job.sections.iter().any(|section| section.format.underline.width>0.0))));
        click(&mut app, &context, photos);
        assert_eq!(app.directory, 1);
        let output = frame(&mut app, &context, vec![]);
        let back = text_position(&output, "Back");
        click(&mut app, &context, back);
        assert_eq!(app.directory, 0);
    }
    #[test]
    fn a_single_group_click_opens_all_25000_items_without_recursive_grouping() {
        let mut app = ExplorerApp::empty();
        app.language = 0;
        let nodes = (0..25001)
            .map(|i| scan::Node {
                parent: if i == 0 { None } else { Some(0) },
                name: format!("item-{i}"),
                locator: i.to_string(),
                native_locator: i.to_string(),
                kind: if i == 0 { Kind::Directory } else { Kind::File },
                logical: scan::Measure {
                    known: if i == 0 {
                        25000 * 25001 / 2
                    } else {
                        25001 - i as u64
                    },
                    ..Default::default()
                },
                allocated: scan::Measure::default(),
                complete: true,
                hard_link: false,
                error: None,
                path: None,
            })
            .collect();
        app.result = Some(Arc::new(ScanResult {
            schema_version: 1,
            started_unix_seconds: 0,
            elapsed_millis: 0,
            scope: String::new(),
            cancelled: false,
            limited: false,
            limit_reason: None,
            cross_filesystems: false,
            errors: 0,
            nodes,
        }));
        app.selected = Some(25000);
        app.refresh_rows();
        assert!(app.map_objects.len() <= 96);
        assert!(!app.grouped.is_empty());
        let grouped_count = app.grouped.len();
        assert!(
            app.map_objects[app.selected_tile().unwrap()].is_none(),
            "a selected small file must mark the grouped tile"
        );
        assert_eq!(grouped_count, 25000);
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        for label in [
            i18n::text(0, "grouped_items").to_owned(),
            format!(
                "{} ({})",
                i18n::text(0, "grouped_items"),
                i18n::integer(0, 25000)
            ),
        ] {
            for _ in 0..5 {
                frame(&mut app, &context, vec![]);
            }
            let output = frame(&mut app, &context, vec![]);
            let position = text_position(&output, &label);
            click(&mut app, &context, position);
            let output = frame(&mut app, &context, vec![]);
            assert_eq!(app.rows.len(), grouped_count);
            assert_eq!(app.map_group.as_ref().unwrap().len(), grouped_count);
            assert!(app.map_objects.is_empty());
            assert!(app.grouped.is_empty());
            assert_eq!(app.selected, Some(25000));
            let visible_names = output
                .shapes
                .iter()
                .filter(|shape| {
                    matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.text().starts_with("item-"))
                })
                .count();
            assert!(
                visible_names < 100,
                "group rows must remain virtualized: {visible_names}"
            );
            assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.text() == i18n::text(0,"treemap"))));
            app.sort = Sort::Name;
            app.dirty = true;
            app.refresh_rows();
            let mut expected = app.map_group.clone().unwrap();
            app.result.as_ref().unwrap().sort(&mut expected, Sort::Name);
            assert_eq!(app.rows, expected);
            app.query = "item-25000".into();
            app.dirty = true;
            app.refresh_rows();
            assert_eq!(app.rows, vec![25000]);
            assert_eq!(app.map_group.as_ref().unwrap().len(), grouped_count);
            let output = frame(&mut app, &context, vec![]);
            let back = text_position(&output, i18n::text(0, "back"));
            click(&mut app, &context, back);
            frame(&mut app, &context, vec![]);
            assert!(app.map_group.is_none());
            assert!(app.query.is_empty());
            assert_eq!(app.directory, 0);
            assert_eq!(app.selected, Some(25000));
            assert_eq!(app.grouped.len(), grouped_count);
            assert!(app.map_objects.len() <= 96);
        }
        let _ = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1180.0, 900.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            },
            |ui| {
                ui.memory_mut(|memory| memory.request_focus(ui.id().with(("tile", 0usize))));
                app.map(ui, 260.0, false);
            },
        );
        app.refresh_rows();
        assert_eq!(app.map_group.as_ref().unwrap().len(), 25000);
        assert_eq!(app.rows.len(), 25000);
        assert!(app.grouped.is_empty());
    }
}
