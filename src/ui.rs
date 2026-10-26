//! Storage exploration UI. The list and treemap share a single selection and directory.
use crate::{
    bridge::{self, Listing},
    i18n,
    scan::{self, Kind, ScanResult, Sort},
    theme::{self, Appearance},
    treemap,
};
use eframe::egui::{self, RichText};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Duration,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Directory,
    Largest,
    Extensions,
}

pub struct ExplorerApp {
    language: usize,
    appearance: Appearance,
    service: scan::Service,
    job: Option<scan::Job>,
    result: Option<Arc<ScanResult>>,
    scope: Option<PathBuf>,
    directory: usize,
    selected: Option<usize>,
    page: Page,
    query: String,
    sort: Sort,
    allocated: bool,
    cross_filesystems: bool,
    rows: Vec<usize>,
    extensions: Vec<(String, u64, scan::Measure)>,
    dirty: bool,
    preview: Option<Listing>,
    mobile_busy: bool,
    notice: Option<&'static str>,
    export: Option<mpsc::Receiver<bool>>,
    map_group: Option<Vec<usize>>,
    map_objects: Vec<Option<usize>>,
    map_weights: Vec<f64>,
    grouped: Vec<usize>,
    #[cfg(any(target_os = "android", target_os = "ios"))]
    native_settings: Option<(Appearance, usize)>,
}
impl ExplorerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "japanese".into(),
            Arc::new(
                egui::FontData::from_static(include_bytes!("../assets/fonts/NotoSansJP.ttf"))
                    .tweak(egui::FontTweak {
                        coords: egui::epaint::text::VariationCoords::new([(b"wght", 400.0)]),
                        ..Default::default()
                    }),
            ),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("japanese".into());
        }
        cc.egui_ctx.set_fonts(fonts);
        theme::install(&cc.egui_ctx);
        let repaint = cc.egui_ctx.clone();
        bridge::set_waker(Arc::new(move || repaint.request_repaint()));
        cc.egui_ctx.set_theme(egui::ThemePreference::System);
        Self::empty()
    }
    fn empty() -> Self {
        Self {
            language: i18n::system_language(),
            appearance: Appearance::System,
            service: scan::Service::default(),
            job: None,
            result: None,
            scope: None,
            directory: 0,
            selected: None,
            page: Page::Directory,
            query: String::new(),
            sort: Sort::Size,
            allocated: false,
            cross_filesystems: false,
            rows: Vec::new(),
            extensions: Vec::new(),
            dirty: true,
            preview: None,
            mobile_busy: false,
            notice: None,
            export: None,
            map_group: None,
            map_objects: Vec::new(),
            map_weights: Vec::new(),
            grouped: Vec::new(),
            #[cfg(any(target_os = "android", target_os = "ios"))]
            native_settings: None,
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
        self.extensions.clear();
        self.query.clear();
        self.notice = None;
        self.dirty = true;
        self.map_group = None;
    }
    fn choose(&mut self) {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            self.start(path);
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let generation = bridge::begin();
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
        self.selected = Some(index);
        self.page = Page::Directory;
        self.query.clear();
        self.map_group = None;
        self.dirty = true;
    }
    fn back(&mut self) {
        if self.map_group.take().is_some() {
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
    fn refresh_rows(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(result) = &self.result {
            self.rows = if let Some(group) = &self.map_group {
                group.clone()
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
        if let Some(result) = &self.result {
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
        self.dirty = false;
    }
    fn poll(&mut self, context: &egui::Context) {
        if let Some(result) = self.job.as_ref().and_then(scan::Job::take) {
            self.extensions = result.extensions();
            self.result = Some(Arc::new(result));
            self.job = None;
            self.dirty = true;
        }
        if let Some(listing) = bridge::take() {
            self.preview = Some(listing);
            self.mobile_busy = false;
        }
        if let Some(receiver) = &self.export
            && let Ok(success) = receiver.try_recv()
        {
            self.notice = Some(if success { "exported" } else { "export_failed" });
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
                let _ = sender.send(scan::write_export(&result, &path, json).is_ok());
            });
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let _ = json;
            self.notice = Some("mobile_export_unavailable");
        }
    }
    fn settings(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        ui.menu_button(t("settings"), |ui| {
            ui.set_max_width(300.0);
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
            egui::ComboBox::from_label(t("language"))
                .selected_text(i18n::TAGS[self.language])
                .show_ui(ui, |ui| {
                    for (index, tag) in i18n::TAGS.iter().enumerate() {
                        ui.selectable_value(&mut self.language, index, *tag);
                    }
                });
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
            ui.checkbox(&mut self.cross_filesystems, t("cross_filesystems"));
            ui.label(t("mount_policy"));
        });
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
                    if ui.button(t("choose")).clicked() {
                        self.choose();
                    }
                    if ui
                        .add_enabled(
                            self.scope.is_some() && self.job.is_none(),
                            egui::Button::new(t("rescan")),
                        )
                        .clicked()
                        && let Some(path) = &self.scope
                    {
                        self.start(path.clone());
                    }
                    if (self.job.is_some() || self.mobile_busy) && ui.button(t("cancel")).clicked()
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
                        ui.add_enabled_ui(self.export.is_none(), |ui| {
                            if ui.button(t("export_json")).clicked() {
                                self.save(true);
                            }
                            if ui.button(t("export_csv")).clicked() {
                                self.save(false);
                            }
                        });
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
        }
        if let Some(notice) = self.notice {
            ui.label(t(notice));
        }
    }
    fn navigation(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        ui.horizontal_wrapped(|ui| {
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
                    self.map_group = None;
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
        if self.page == Page::Directory {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.directory != 0 || self.map_group.is_some(),
                        egui::Button::new(t("back")),
                    )
                    .clicked()
                {
                    self.back();
                }
                if let Some(result) = self.result.clone() {
                    for index in result.breadcrumbs(self.directory) {
                        if ui.button(&result.nodes[index].name).clicked() {
                            self.navigate(index);
                        }
                    }
                }
                if self.map_group.is_some() {
                    ui.label(t("grouped_items"));
                }
            });
        }
    }
    fn table(&mut self, ui: &mut egui::Ui) {
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
            ui.label(t("no_results"));
            return;
        }
        let row_height = 60.0;
        egui::ScrollArea::vertical()
            .id_salt("object-list")
            .max_height(420.0)
            .show_rows(ui, row_height, self.rows.len(), |ui, range| {
                for position in range {
                    let index = self.rows[position];
                    let node = &result.nodes[index];
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            let name = format!("{}  {}", t(kind_key(node.kind)), node.name);
                            let response = ui.add_sized(
                                egui::vec2((ui.available_width() - 120.0).max(60.0), 44.0),
                                egui::Button::new(name)
                                    .selected(self.selected == Some(index))
                                    .truncate(),
                            );
                            if response.clicked() {
                                self.selected = Some(index);
                            }
                            if response.double_clicked() && node.kind == Kind::Directory {
                                self.navigate(index);
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.monospace(measure(
                                language,
                                if self.allocated {
                                    node.allocated
                                } else {
                                    node.logical
                                },
                            ));
                        });
                    });
                }
            });
    }
    fn map(&mut self, ui: &mut egui::Ui) {
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
        let objects = self.map_objects.clone();
        let weights = self.map_weights.clone();
        let size = egui::vec2(ui.available_width(), 300.0);
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
            let selected = objects[position].is_some() && self.selected == objects[position];
            let response =
                ui.interact(area, ui.id().with(("tile", position)), egui::Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
            });
            let dark = ui.visuals().dark_mode;
            let shade = if selected || response.hovered() {
                if dark { 95 } else { 172 }
            } else {
                if dark { 43 } else { 216 }
            };
            ui.painter()
                .rect_filled(area, 4.0, egui::Color32::from_gray(shade));
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
                    label,
                    egui::FontId::proportional(13.0),
                    ui.visuals().text_color(),
                );
                painter.text(
                    area.min + egui::vec2(10.0, 29.0),
                    egui::Align2::LEFT_TOP,
                    i18n::bytes(language, weights[position].min(u64::MAX as f64) as u64),
                    egui::FontId::monospace(12.0),
                    ui.visuals().text_color(),
                );
            }
            let double_clicked = response.double_clicked();
            if response.clicked() {
                if let Some(index) = objects[position] {
                    self.selected = Some(index);
                } else {
                    self.map_group = Some(self.grouped.clone());
                    self.dirty = true;
                }
            }
            if double_clicked
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
            self.map_group = Some(self.grouped.clone());
            self.dirty = true;
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
                ui.horizontal_wrapped(|ui| {
                    if ui.button(t("copy_path")).clicked() {
                        ui.ctx().copy_text(node.locator.clone());
                    }
                    if node.kind == Kind::Directory && ui.button(t("open_folder")).clicked() {
                        self.navigate(index);
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
        ui.label(t("mobile_scope"));
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
                self.header(ui);
                ui.add_space(16.0);
                egui::ScrollArea::vertical()
                    .id_salt("workspace")
                    .show(ui, |ui| {
                        self.summary(ui);
                        if self.result.is_some() {
                            ui.add_space(12.0);
                            self.navigation(ui);
                            self.refresh_rows();
                            ui.add_space(12.0);
                            if self.page == Page::Extensions {
                                let language = self.language;
                                let t = |key| i18n::text(language, key);
                                egui::ScrollArea::vertical()
                                    .id_salt("extensions")
                                    .max_height(500.0)
                                    .show_rows(ui, 40.0, self.extensions.len(), |ui, range| {
                                        for index in range {
                                            let (name, count, size) = &self.extensions[index];
                                            ui.horizontal_wrapped(|ui| {
                                                ui.label(if name.is_empty() {
                                                    t("no_extension")
                                                } else {
                                                    name
                                                });
                                                ui.monospace(measure(language, *size));
                                                ui.label(i18n::counted(language, *count, "file"));
                                            });
                                        }
                                    });
                            } else if ui.available_width() >= 940.0 {
                                ui.columns(2, |columns| {
                                    self.table(&mut columns[0]);
                                    self.map(&mut columns[1]);
                                });
                            } else {
                                self.map(ui);
                                ui.add_space(12.0);
                                self.table(ui);
                            }
                            ui.add_space(12.0);
                            self.details(ui);
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
                        self.mobile_preview(ui);
                    });
            });
    }
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
    let mut text = if size.overflow {
        i18n::text(language, "overflow").into()
    } else {
        i18n::bytes(language, size.known)
    };
    if size.unknown > 0 {
        text.push_str(&format!(
            " + {} {}",
            i18n::integer(language, size.unknown),
            i18n::text(language, "unknown")
        ));
    }
    text
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
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        self.draw(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_languages_render_at_phone_and_desktop_widths_in_each_theme() {
        let context = egui::Context::default();
        theme::install(&context);
        let mut app = ExplorerApp::empty();
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
    fn map_grouping_is_bounded_and_expands_without_losing_selection() {
        let mut app = ExplorerApp::empty();
        let nodes = (0..1001)
            .map(|i| scan::Node {
                parent: if i == 0 { None } else { Some(0) },
                name: format!("item-{i}"),
                locator: i.to_string(),
                native_locator: i.to_string(),
                kind: if i == 0 { Kind::Directory } else { Kind::File },
                logical: scan::Measure {
                    known: 1001 - i as u64,
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
            cross_filesystems: false,
            errors: 0,
            nodes,
        }));
        app.selected = Some(1000);
        app.refresh_rows();
        assert!(app.map_objects.len() <= 96);
        assert!(!app.grouped.is_empty());
        let grouped_count = app.grouped.len();
        app.map_group = Some(app.grouped.clone());
        app.dirty = true;
        app.refresh_rows();
        assert_eq!(app.rows.len(), grouped_count);
        assert_eq!(app.selected, Some(1000));
    }
}
