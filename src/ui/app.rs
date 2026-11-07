use super::*;

impl ExplorerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::new_with_preferences(cc, preferences::path())
    }
    pub(super) fn new_with_preferences(
        cc: &eframe::CreationContext<'_>,
        path: Option<PathBuf>,
    ) -> Self {
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
        crate::mobile::configure_memory(app.preferences.scan_memory);
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
    pub(super) fn empty() -> Self {
        Self {
            compact_layout: false,
            phone_home: true,
            phone_view: PhoneView::List,
            phone_overlay: None,
            language: i18n::system_language(),
            preferences: Preferences::default(),
            preferences_path: None,
            saved_preferences: Preferences::default(),
            last_preferences_write: Instant::now(),
            preferences_write_failed: false,
            settings_open: false,
            tree: tree::TreeState::default(),
            appearance: Appearance::System,
            service: scan::Service::default(),
            job: None,
            result: None,
            live: None,
            view: View::Live,
            saved: None,
            second: None,
            open_job: None,
            open_name: String::new(),
            open_target: OpenTarget::Auto,
            open_queue: Default::default(),
            open_error: None,
            changes: Changes::default(),
            #[cfg(test)]
            map_panel_id: None,
            #[cfg(any(target_os = "android", target_os = "ios"))]
            native_open: None,
            #[cfg(any(target_os = "android", target_os = "ios"))]
            native_open_target: OpenTarget::Auto,
            #[cfg(test)]
            memory_budget: None,
            scope: None,
            provider_scope: None,
            navigation: navigation::NavigationState::default(),
            search: search::SearchState::default(),
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
            map: treemap::MapState::default(),
            #[cfg(any(target_os = "android", target_os = "ios"))]
            native_settings: None,
        }
    }
    pub(super) fn persist_preferences(&mut self, force: bool) {
        let Some(path) = &self.preferences_path else {
            return;
        };
        self.preferences.appearance = self.appearance;
        crate::mobile::configure_memory(self.preferences.scan_memory);
        if self.preferences == self.saved_preferences {
            return;
        }
        let settings_changed = self.preferences.appearance != self.saved_preferences.appearance
            || self.preferences.language != self.saved_preferences.language
            || self.preferences.scan_memory != self.saved_preferences.scan_memory;
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
        self.phone_home = false;
        self.phone_overlay = None;
        self.job = Some(self.service.start(
            path.clone(),
            scan::Options {
                cross_filesystems: self.cross_filesystems,
                memory_mode: self.preferences.scan_memory,
                ..Default::default()
            },
        ));
        self.scope = Some(path);
        self.view = View::Live;
        self.live = None;
        self.live_changed();
        self.result = None;
        self.navigation.selected = None;
        self.navigation.directory = 0;
        self.rows.clear();
        self.tree = tree::TreeState::default();
        self.extensions.clear();
        self.search.clear();
        self.notice = None;
        self.dirty = true;
        self.map.group = None;
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    pub(super) fn start_provider(
        &mut self,
        generation: u64,
        root: String,
        display_name: Option<String>,
    ) {
        self.phone_home = false;
        self.phone_overlay = None;
        let provider =
            crate::provider::Provider::new(root.clone(), generation, crate::mobile::transport())
                .with_display_name(display_name);
        let job = self.service.start_storage(
            Box::new(provider),
            scan::Options {
                memory_mode: self.preferences.scan_memory,
                ..Default::default()
            },
        );
        crate::mobile::scanning(generation, job.progress.clone());
        self.job = Some(job);
        self.provider_scope = Some((generation, root));
        self.scope = None;
        self.view = View::Live;
        self.live = None;
        self.live_changed();
        self.result = None;
        self.navigation.selected = None;
        self.navigation.directory = 0;
        self.rows.clear();
        self.tree = tree::TreeState::default();
        self.extensions.clear();
        self.search.clear();
        self.notice = None;
        self.map.group = None;
        self.dirty = true;
    }
    /// Open an exported JSON result for viewing; the folder it describes is not read.
    pub fn open_result(&mut self, path: PathBuf) {
        self.open_saved(path, false);
    }
    pub(super) fn scan_folder(&mut self, index: usize) {
        if self.view != View::Live {
            return;
        }
        let Some(result) = self.result.clone() else {
            return;
        };
        let Some(node) = result
            .nodes
            .get(index)
            .filter(|n| n.kind == Kind::Directory)
        else {
            return;
        };
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if let Some(path) = &node.path {
            self.start(path.clone());
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        if let Some((generation, _)) = self.provider_scope.clone() {
            self.start_provider(generation, node.locator.clone(), Some(node.name.clone()));
        }
    }
    pub(super) fn rescan(&mut self) {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        if let Some((generation, root)) = self.provider_scope.clone() {
            let display_name = self.result.as_ref().map(|r| r.nodes[0].name.clone());
            self.start_provider(generation, root, display_name);
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if let Some(path) = self.scope.clone() {
            self.start(path);
        }
    }
    pub(super) fn choose(&mut self) {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            self.start(path);
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let generation = bridge::begin();
            self.job = None;
            self.view = View::Live;
            self.live = None;
            self.live_changed();
            self.result = None;
            self.provider_scope = None;
            self.preview = None;
            self.notice = None;
            #[cfg(target_os = "android")]
            let launched = crate::android::pick(generation);
            #[cfg(target_os = "ios")]
            let launched = crate::ios::pick(generation);
            self.mobile_busy = launched;
            self.phone_home = !launched;
            if !launched {
                self.notice = Some("unavailable");
            }
        }
    }
    pub(super) fn refresh_rows(&mut self) {
        if self.dirty {
            self.search.cancel();
            if self.search.remaining(Instant::now()).is_some() {
                self.clear_pending_rows();
                return;
            }
            let Some(result) = &self.result else {
                self.dirty = false;
                return;
            };
            let filtered = self.map.group.is_some()
                || !self.search.query.is_empty()
                || self.navigation.page == Page::Largest;
            if filtered {
                let request = search::Request {
                    result: result.clone(),
                    query: self.search.query.clone(),
                    group: self.map.group.clone(),
                    files_only: self.navigation.page == Page::Largest,
                    sort: self.sort,
                    allocated: self.allocated,
                };
                if result.nodes.len() >= search::BACKGROUND_THRESHOLD {
                    self.search.start(request);
                    self.clear_pending_rows();
                    self.dirty = false;
                    return;
                }
                self.rows = request
                    .run(&std::sync::atomic::AtomicBool::new(false))
                    .unwrap();
            } else {
                self.tree
                    .refresh(result, self.navigation.directory, self.sort, self.allocated);
                self.rows = self.tree.children(self.navigation.directory).to_vec();
                if self.allocated && self.sort == Sort::Size {
                    self.rows.sort_unstable_by(|a, b| {
                        result.nodes[*b]
                            .allocated
                            .known
                            .cmp(&result.nodes[*a].allocated.known)
                            .then(a.cmp(b))
                    });
                }
            }
            self.dirty = false;
        } else if let Some(rows) = self.search.take() {
            self.rows = rows;
        } else {
            return;
        }
        if let Some(result) = &self.result {
            self.map.rebuild(
                result,
                &self.rows,
                !self.search.query.is_empty(),
                self.allocated,
            );
        }
        self.map.selected = self.selected_tile();
    }
    /// Rows and map membership must describe the same completed request.
    fn clear_pending_rows(&mut self) {
        self.rows.clear();
        self.map.objects.clear();
        self.map.weights.clear();
        self.map.grouped.clear();
        self.map.selected = None;
    }
    pub(super) fn poll(&mut self, context: &egui::Context) {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            if self
                .provider_scope
                .as_ref()
                .is_some_and(|(generation, _)| !bridge::is_current(*generation))
            {
                self.job = None;
                if self.view == View::Live {
                    self.result = None;
                }
                self.live = None;
                self.live_changed();
                self.provider_scope = None;
            }
            if let Some(command) = crate::mobile::take_command() {
                use crate::mobile::Command;
                match command {
                    Command::Rescan => self.rescan(),
                    Command::ScanFolder(index) => self.scan_folder(index),
                    Command::Memory(mode) => {
                        self.preferences.scan_memory = mode;
                        crate::mobile::configure_memory(mode);
                    }
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
                    Command::Search(query) => {
                        self.search.query = query;
                        self.search.edited(Instant::now());
                        self.dirty = true;
                    }
                    Command::Back => {
                        self.phone_back();
                    }
                    Command::CloseSettings => self.settings_open = false,
                    Command::OpenSaved => {
                        if self.open_job.is_none() {
                            self.open_saved_dialog();
                        }
                    }
                    Command::OpenFor(_)
                    | Command::SwapDirection
                    | Command::ConfirmScope(_)
                    | Command::StartCompare
                    | Command::ShowSide(_) => self.native_compare_command(command),
                    Command::Allocated(allocated) => {
                        self.allocated = allocated
                            && self.result.as_ref().is_some_and(|r| {
                                r.nodes[0].allocated.known > 0 || r.nodes[0].allocated.unknown == 0
                            });
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
            let result = Arc::new(result);
            self.live = Some(result.clone());
            self.settle_operands();
            self.job = None;
            if self.view == View::Live {
                #[cfg(any(target_os = "android", target_os = "ios"))]
                if let Some((generation, _)) = &self.provider_scope {
                    crate::mobile::publish(*generation, result.clone());
                }
                self.set_displayed(Some(result));
                self.phone_home = false;
            }
        }
        self.poll_saved();
        if let Some(listing) = bridge::take() {
            self.mobile_busy = false;
            #[cfg(any(target_os = "android", target_os = "ios"))]
            if listing.status == bridge::Status::Ready
                && let Some(root) = &listing.scope
            {
                self.start_provider(
                    listing.generation,
                    root.clone(),
                    listing.display_name.clone(),
                );
            }
            if listing.status != bridge::Status::Ready {
                self.phone_home = true;
                self.notice = Some(match listing.status {
                    bridge::Status::Cancelled => "cancelled",
                    bridge::Status::Denied => "denied",
                    _ => "unavailable",
                });
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
        if self.job.is_some()
            || self.mobile_busy
            || self.export.is_some()
            || self.open_job.is_some()
            || self.changes.job.is_some()
        {
            context.request_repaint_after(Duration::from_millis(100));
        }
    }
    pub(super) fn save(&mut self, json: bool) {
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
    pub(super) fn settings(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        if ui
            .add(egui::Button::new(t("settings")).wrap_mode(egui::TextWrapMode::Extend))
            .clicked()
        {
            self.settings_open = !self.settings_open;
        }
    }
    pub(super) fn settings_window(&mut self, context: &egui::Context) {
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
            .id(egui::Id::unique("settings-window"))
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
                ui.separator();
                ui.label(t("memory_settings"));
                let mut automatic = matches!(
                    self.preferences.scan_memory,
                    crate::memory::BudgetMode::Automatic
                );
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut automatic, true, t("memory_automatic"));
                    ui.selectable_value(&mut automatic, false, t("memory_manual"));
                });
                self.preferences.scan_memory = if automatic {
                    crate::memory::BudgetMode::Automatic
                } else {
                    let mut mib = match self.preferences.scan_memory {
                        crate::memory::BudgetMode::Manual { mib } => mib,
                        _ => 64,
                    };
                    ui.add(
                        egui::DragValue::new(&mut mib)
                            .range(crate::memory::MIN_MANUAL_MIB..=crate::memory::MAX_MANUAL_MIB)
                            .suffix(" MiB"),
                    );
                    crate::memory::BudgetMode::Manual { mib }
                };
                ui.label(t("memory_help"));
                if ui.button(t("close")).clicked() {
                    close = true;
                }
            });
        self.settings_open = open && !close;
    }
    pub(super) fn header(&mut self, ui: &mut egui::Ui) {
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
                    if ui
                        .add_enabled(
                            self.open_job.is_none(),
                            egui::Button::new(t("open_saved"))
                                .wrap_mode(egui::TextWrapMode::Extend),
                        )
                        .clicked()
                    {
                        self.open_saved_dialog();
                    }
                    if self.result.is_some() && self.view != View::Changes {
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
                self.view_tabs(ui);
            });
    }
    pub(super) fn summary(&mut self, ui: &mut egui::Ui) {
        self.open_status(ui);
        self.saved_banner(ui);
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
        // The comparison screen has its own summary; the last browsed result is not it.
        if let Some(result) = self.result.as_ref().filter(|_| self.view != View::Changes) {
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
            if let Some(memory) = &result.memory {
                ui.label(
                    RichText::new(i18n::memory_diagnostics(language, memory))
                        .small()
                        .weak(),
                );
            }
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
    pub(super) fn details(&mut self, ui: &mut egui::Ui) {
        let Some(result) = self.result.clone() else {
            return;
        };
        let Some(index) = self.navigation.selected else {
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
                if self.compact_layout {
                    ui.label(RichText::new(t("phone_location")).small().weak());
                }
                ui.label(readable_locator(&node.locator));
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
                    if self.view == View::Live && ui.button(t("scan_folder_separately")).clicked() {
                        self.scan_folder(index);
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
                    if self.view == View::Live && ui.button(t("reveal")).clicked() {
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
    pub(super) fn mobile_preview(&mut self, ui: &mut egui::Ui) {
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
    pub(super) fn sort_extensions(&mut self) {
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
    pub(super) fn extension_table(&mut self, ui: &mut egui::Ui) {
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
                        egui::Role::Button,
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
    pub(super) fn object_workspace(&mut self, ui: &mut egui::Ui, wide: bool) {
        if self.search.pending() {
            ui.spinner();
            return;
        }
        let remaining = ui.clip_rect().bottom() - ui.cursor().top();
        if self.map.group.is_some() {
            ui.label(i18n::counted(
                self.language,
                self.rows.len() as u64,
                "object",
            ));
            if self.navigation.selected.is_some() {
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
                    #[cfg(test)]
                    {
                        self.map_panel_id =
                            Some(ui.scope_id().with_salt(egui::IdSalt::new("treemap-panel")));
                    }
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
                                if let Some(name) = self.navigation.selected.and_then(|i| {
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
    pub(super) fn draw(&mut self, ui: &mut egui::Ui) {
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
        {
            let dropped: Vec<PathBuf> = ui.ctx().input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .collect()
            });
            let (folders, files): (Vec<_>, Vec<_>) =
                dropped.into_iter().partition(|path| path.is_dir());
            if let Some(folder) = folders.into_iter().next_back() {
                self.start(folder);
            }
            self.open_results(
                files
                    .into_iter()
                    .filter(|path| {
                        path.extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
                    })
                    .collect(),
            );
        }
        if ui
            .ctx()
            .input(|i| i.modifiers.alt && i.key_pressed(egui::Key::ArrowLeft))
        {
            if self.compact_layout {
                self.phone_back();
            } else {
                self.back();
            }
        }
        let safe = ui
            .available_rect_before_wrap()
            .intersect(ui.ctx().content_rect());
        ui.scope_builder(egui::UiBuilder::new().max_rect(safe), |ui| {
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
                    self.compact_layout = !desktop;
                    if !desktop {
                        self.phone(ui);
                        return;
                    }
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
                            if self.view == View::Changes {
                                self.changes_workspace(ui, desktop);
                            } else if self.result.is_some() {
                                ui.add_space(12.0);
                                self.navigation(ui);
                                self.refresh_rows();
                                ui.add_space(12.0);
                                if self.navigation.page == Page::Extensions {
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
                                ui.label(t(
                                    if cfg!(any(target_os = "android", target_os = "ios")) {
                                        "mobile_scope"
                                    } else {
                                        "empty_body"
                                    },
                                ));
                                ui.add_space(14.0);
                                ui.label(t("empty_saved_hint"));
                                ui.add_space(14.0);
                                ui.label(RichText::new(t("privacy_note")).weak());
                            }
                            #[cfg(any(target_os = "android", target_os = "ios"))]
                            if self.result.is_none() && self.job.is_none() {
                                self.mobile_preview(ui);
                            }
                        });
                });
        });
        if self.view != View::Changes
            && !(self.compact_layout && (self.phone_home || self.phone_overlay.is_some()))
        {
            self.search.repaint(ui.ctx());
        }
        self.settings_window(ui.ctx());
        #[cfg(any(target_os = "android", target_os = "ios"))]
        crate::mobile::focus(
            if self.compact_layout && self.phone_home {
                0
            } else {
                self.navigation.directory
            },
            !self.phone_home && self.map.group.is_some(),
            self.settings_open,
            self.compact_layout
                && (!self.phone_home
                    || self.mobile_busy
                    || self.job.is_some()
                    || self.phone_overlay.is_some()),
        );
    }
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
