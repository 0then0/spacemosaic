//! Storage exploration UI. The list and treemap share a single selection and directory.
use crate::{
    bridge::{self, Listing},
    i18n,
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
    map_selected: Option<usize>,
    map_weights: Vec<f64>,
    grouped: Vec<usize>,
    #[cfg(any(target_os = "android", target_os = "ios"))]
    native_settings: Option<(Appearance, usize)>,
}
impl ExplorerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_fonts(crate::fonts::definitions());
        theme::install(&cc.egui_ctx);
        let repaint = cc.egui_ctx.clone();
        bridge::set_waker(Arc::new(move || repaint.request_repaint()));
        cc.egui_ctx.set_theme(egui::ThemePreference::System);
        Self::empty()
    }
    fn empty() -> Self {
        Self {
            language: i18n::system_language(),
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
            map_selected: None,
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
        self.expanded.insert(index);
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
                if self.map_objects.len() < 95
                    && (self.map_group.is_some() || weight as f64 >= total * 0.015)
                {
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
        if ui.button(t("settings")).clicked() {
            self.settings_open = !self.settings_open;
        }
    }
    fn settings_window(&mut self, context: &egui::Context) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        egui::Window::new(t("settings"))
            .id(egui::Id::new("settings-window"))
            .open(&mut self.settings_open)
            .resizable(false)
            .default_width(280.0)
            .show(context, |ui| {
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
                    .selected_text(i18n::TAGS[self.language])
                    .show_ui(ui, |ui| {
                        for (index, tag) in i18n::TAGS.iter().enumerate() {
                            ui.selectable_value(&mut self.language, index, *tag);
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
            ui.label(t("no_results"));
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
            .id_salt("object-list")
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
            if response.clicked() {
                if let Some(index) = objects[position] {
                    self.select(index);
                } else {
                    self.map_group = Some(self.grouped.clone());
                    self.dirty = true;
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
    fn object_workspace(&mut self, ui: &mut egui::Ui, wide: bool) {
        let remaining = ui.clip_rect().bottom() - ui.cursor().top();
        if wide && remaining >= 360.0 {
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), remaining),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Panel::bottom("treemap-panel")
                        .resizable(true)
                        .default_size(remaining * 0.48)
                        .size_range(160.0..=(remaining - 140.0))
                        .show(ui, |ui| {
                            ui.set_min_height(ui.available_height());
                            self.map(ui, ui.available_height(), true);
                        });
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
                let desktop = ui.available_width() >= 700.0 && ui.available_height() >= 600.0;
                if desktop {
                    ui.spacing_mut().interact_size.y = 30.0;
                    ui.spacing_mut().button_padding.y = 5.0;
                    ui.spacing_mut().item_spacing.y = 6.0;
                }
                self.header(ui);
                ui.add_space(12.0);
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
                        self.mobile_preview(ui);
                    });
            });
        self.settings_window(ui.ctx());
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
    fn language_popup_stays_open_and_selects_a_language_in_settings() {
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        let mut app = ExplorerApp::empty();
        app.language = 0;
        frame(&mut app, &context, vec![]);
        let output = frame(&mut app, &context, vec![]);
        let pos = text_position(&output, "Settings");
        click(&mut app, &context, pos);
        let output = frame(&mut app, &context, vec![]);
        assert!(app.settings_open);
        let pos = text_position(&output, "en");
        click(&mut app, &context, pos);
        let output = frame(&mut app, &context, vec![]);
        let pos = text_position(&output, "ru");
        click(&mut app, &context, pos);
        assert_eq!(app.language, 1);
        assert!(app.settings_open);
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
        assert!(
            app.map_objects[app.selected_tile().unwrap()].is_none(),
            "a selected small file must mark the grouped tile"
        );
        app.map_group = Some(app.grouped.clone());
        app.dirty = true;
        app.refresh_rows();
        assert_eq!(app.rows.len(), grouped_count);
        assert_eq!(app.map_objects.len(), 96);
        assert!(app.grouped.len() < grouped_count);
        assert_eq!(app.selected, Some(1000));
    }
}
