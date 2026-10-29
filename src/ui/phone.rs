//! A folder-first workspace: fixed navigation, a bounded list or map, and separate details.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PhoneView {
    List,
    Map,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PhoneOverlay {
    Actions,
    Details,
    ScanInfo,
    Filter,
}

impl ExplorerApp {
    /// The system Back button follows the visible hierarchy, preserving the scan.
    pub(super) fn phone_back(&mut self) -> bool {
        if self.settings_open {
            self.settings_open = false;
        } else if let Some(overlay) = self.phone_overlay.take() {
            if overlay == PhoneOverlay::ScanInfo {
                self.phone_overlay = Some(PhoneOverlay::Actions);
            }
        } else if self.phone_home && self.job.is_none() && !self.mobile_busy {
            return false;
        } else if let Some(job) = &self.job {
            job.cancel();
        } else if self.mobile_busy {
            bridge::begin();
            self.mobile_busy = false;
            self.phone_home = true;
            self.notice = Some("cancelled");
        } else if self.map_group.is_some() {
            self.back();
            self.phone_view = PhoneView::Map;
        } else if self.directory != 0 {
            self.back();
        } else if !self.phone_home {
            self.phone_home = true;
        } else {
            return false;
        }
        true
    }
    /// Search and statistics cover the entire scan, while browsing and groups
    /// retain their directory scope. Use the same scope for labels and bars.
    pub(super) fn phone_scope(&self) -> usize {
        if self.map_group.is_none() && (self.page != Page::Directory || !self.query.is_empty()) {
            0
        } else {
            self.directory
        }
    }
    fn phone_button(ui: &mut egui::Ui, label: &str) -> bool {
        ui.add_sized(
            [ui.available_width(), 48.0],
            egui::Button::new(label).wrap(),
        )
        .clicked()
    }
    fn phone_icon(ui: &mut egui::Ui, icon: &str, label: &str) -> bool {
        let response = ui.add_sized([48.0, 48.0], egui::Button::new(icon));
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
        response.on_hover_text(label).clicked()
    }
    pub(super) fn phone(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().interact_size.y = 48.0;
        ui.spacing_mut().button_padding = egui::vec2(10.0, 8.0);
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.phone_back();
        }
        let language = self.language;
        let t = |key| i18n::text(language, key);
        if let Some(overlay) = self.phone_overlay {
            self.phone_sheet(ui, overlay);
            return;
        }
        if self.phone_home && self.job.is_none() && !self.mobile_busy {
            ui.heading("SpaceMosaic");
            self.settings(ui);
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("phone-home")
                .show(ui, |ui| {
                    ui.add_space(24.0);
                    ui.heading(t("phone_intro"));
                    ui.label(t("phone_intro_body"));
                    ui.add_space(16.0);
                    if Self::phone_button(ui, t("choose")) {
                        self.choose();
                    }
                    if self.result.is_some() && Self::phone_button(ui, t("phone_continue")) {
                        self.phone_home = false;
                    }
                    if let Some(key) = self.notice {
                        ui.label(t(key));
                    }
                    ui.add_space(16.0);
                    ui.label(
                        RichText::new(t("read_only")).color(theme::accent(ui.visuals().dark_mode)),
                    );
                    ui.label(RichText::new(t("privacy_note")).small().weak());
                });
            return;
        }
        if self.result.is_none() {
            ui.heading(t("scanning"));
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("phone-progress")
                .show(ui, |ui| {
                    ui.label(t("phone_scan_body"));
                    self.summary(ui);
                    if self.mobile_busy {
                        ui.spinner();
                        ui.label(t("waiting"));
                    }
                    if Self::phone_button(ui, t("cancel")) {
                        if let Some(job) = &self.job {
                            job.cancel();
                        } else {
                            bridge::begin();
                            self.mobile_busy = false;
                            self.phone_home = true;
                            self.notice = Some("cancelled");
                        }
                    }
                });
            return;
        }
        let scope = self.phone_scope();
        // Rows and map share the directory/selection; no URI or large toolbar above them.
        ui.horizontal(|ui| {
            if Self::phone_icon(ui, "←", t("back")) {
                self.phone_back();
            }
            let title = self.result.as_ref().unwrap().nodes[scope].name.clone();
            let width = (ui.available_width() - 72.0).max(1.0);
            ui.add_sized(
                [width, 48.0],
                egui::Label::new(RichText::new(title).strong()).truncate(),
            );
            if ui
                .add_sized([64.0, 48.0], egui::Button::new(t("phone_more")).truncate())
                .clicked()
            {
                self.phone_overlay = Some(PhoneOverlay::Actions);
            }
        });
        let result = self.result.as_ref().unwrap();
        let node = &result.nodes[scope];
        let status = if result.cancelled || result.limited || !result.nodes[0].complete {
            "partial"
        } else {
            "complete"
        };
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 20.0;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(measure(
                        language,
                        if self.allocated {
                            node.allocated
                        } else {
                            node.logical
                        },
                    ))
                    .strong(),
                );
                ui.add(egui::Label::new(RichText::new(t(status)).small().weak()).truncate());
            });
        });
        if self.preferences_write_failed {
            ui.label(t("preferences_save_failed"));
        }
        if let Some(key) = self.notice {
            ui.add(egui::Label::new(t(key)).truncate());
        }
        // One-line scrolling breadcrumbs without a second Back button.
        if ui.ctx().content_rect().height() >= 500.0 && (scope != 0 || self.map_group.is_some()) {
            egui::ScrollArea::horizontal()
                .id_salt("phone-path")
                .min_scrolled_width(0.0)
                .auto_shrink([false, true])
                .stick_to_right(true)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                        let result = self.result.clone().unwrap();
                        for index in result.breadcrumbs(scope) {
                            if index != 0 {
                                ui.label("›");
                            }
                            if index == scope && self.map_group.is_none() {
                                ui.label(RichText::new(&result.nodes[index].name).small());
                            } else if ui
                                .add(egui::Button::new(&result.nodes[index].name).frame(false))
                                .clicked()
                            {
                                self.navigate(index);
                            }
                        }
                        if self.map_group.is_some() {
                            ui.label("›");
                            ui.label(t("grouped_items"));
                        }
                    });
                });
        }
        if self.phone_view == PhoneView::List && self.page != Page::Extensions {
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 56.0).max(1.0);
                #[cfg(not(target_os = "android"))]
                let response = ui.add_sized(
                    [width, 48.0],
                    egui::TextEdit::singleline(&mut self.query).hint_text(t("search_hint")),
                );
                #[cfg(not(target_os = "android"))]
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, t("search"))
                });
                #[cfg(not(target_os = "android"))]
                if response.changed() {
                    self.dirty = true;
                }
                #[cfg(target_os = "android")]
                if ui
                    .add_sized(
                        [width, 48.0],
                        egui::Button::new(if self.query.is_empty() {
                            t("search_hint")
                        } else {
                            &self.query
                        })
                        .truncate(),
                    )
                    .clicked()
                {
                    crate::android::search(&self.query);
                }
                if Self::phone_icon(ui, "⇅", t("phone_filter")) {
                    self.phone_overlay = Some(PhoneOverlay::Filter);
                }
            });
        }
        ui.separator();
        // Reserve navigation before drawing the independently scrolling content.
        egui::Panel::bottom("phone-tabs")
            .resizable(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
                    for (view, key) in [
                        (PhoneView::List, "phone_list"),
                        (PhoneView::Map, "phone_map"),
                    ] {
                        if ui
                            .add_sized(
                                [width, 48.0],
                                egui::Button::new(t(key)).selected(self.phone_view == view),
                            )
                            .clicked()
                        {
                            self.phone_view = view;
                            if view == PhoneView::Map {
                                self.map_group = None;
                                if self.page == Page::Extensions {
                                    self.page = Page::Directory;
                                }
                                self.dirty = true;
                            }
                        }
                    }
                });
            });
        self.refresh_rows();
        let bounds = ui.available_rect_before_wrap();
        ui.scope_builder(egui::UiBuilder::new().max_rect(bounds), |ui| {
            ui.set_clip_rect(bounds.intersect(ui.clip_rect()));
            if self.phone_view == PhoneView::List || self.map_group.is_some() {
                if self.page == Page::Extensions {
                    self.extension_table(ui);
                } else {
                    self.phone_list(ui);
                }
            } else {
                ui.label(RichText::new(t("phone_map_hint")).small().weak());
                self.map(ui, ui.available_height(), true);
            }
        });
    }
    fn phone_list(&mut self, ui: &mut egui::Ui) {
        let result = self.result.clone().unwrap();
        let language = self.language;
        let t = |key| i18n::text(language, key);
        let row_height = (ui.text_style_height(&egui::TextStyle::Body) * 2.0 + 24.0).max(64.0);
        if self.rows.is_empty() {
            ui.label(t(if result.nodes[self.directory].complete {
                "no_results"
            } else {
                "incomplete_directory"
            }));
        }
        let total = if self.allocated {
            result.nodes[self.phone_scope()].allocated
        } else {
            result.nodes[self.phone_scope()].logical
        }
        .known
        .max(1) as f64;
        egui::ScrollArea::vertical()
            .id_salt((
                "phone-files",
                self.directory,
                self.map_group.is_some(),
                &self.query,
            ))
            .auto_shrink([false, false])
            .show_rows(ui, row_height, self.rows.len(), |ui, range| {
                for position in range {
                    let index = self.rows[position];
                    let node = &result.nodes[index];
                    let (bounds, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), row_height),
                        egui::Sense::hover(),
                    );
                    let size = if self.allocated {
                        node.allocated
                    } else {
                        node.logical
                    };
                    let fraction = (size.known as f64 / total).clamp(0.0, 1.0) as f32;
                    let mut bar = bounds.shrink2(egui::vec2(0.0, 2.0));
                    bar.max.x = bar.min.x + bar.width() * fraction;
                    ui.painter().rect_filled(
                        bar,
                        4.0,
                        theme::accent(ui.visuals().dark_mode).gamma_multiply(0.12),
                    );
                    let info = egui::Rect::from_min_max(
                        egui::pos2(bounds.right() - 48.0, bounds.top()),
                        bounds.max,
                    );
                    let body = egui::Rect::from_min_max(
                        bounds.min,
                        egui::pos2(info.left() - 4.0, bounds.bottom()),
                    );
                    let response = ui.interact(
                        body,
                        ui.id().with(("phone-row", index)),
                        egui::Sense::click(),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            format!(
                                "{}: {}, {}",
                                t(kind_key(node.kind)),
                                node.name,
                                measure(language, size)
                            ),
                        )
                    });
                    if response.has_focus() || self.selected == Some(index) {
                        ui.painter().rect_stroke(
                            body,
                            4.0,
                            egui::Stroke::new(1.0, theme::accent(ui.visuals().dark_mode)),
                            egui::StrokeKind::Inside,
                        );
                    }
                    let icon = egui::Rect::from_min_size(
                        body.min + egui::vec2(4.0, 16.0),
                        egui::vec2(16.0, 13.0),
                    );
                    if node.kind == Kind::Directory {
                        ui.painter()
                            .rect_filled(icon, 2.0, theme::accent(ui.visuals().dark_mode));
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(
                                icon.min - egui::vec2(0.0, 3.0),
                                egui::vec2(8.0, 5.0),
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
                    let left = body.left() + 30.0;
                    let h = ui.text_style_height(&egui::TextStyle::Body);
                    Self::phone_row_label(
                        ui,
                        egui::Rect::from_min_max(
                            egui::pos2(left, body.top() + 6.0),
                            egui::pos2(body.right(), body.top() + h + 8.0),
                        ),
                        RichText::new(&node.name).strong(),
                    );
                    Self::phone_row_label(
                        ui,
                        egui::Rect::from_min_max(
                            egui::pos2(left, body.top() + h + 10.0),
                            egui::pos2(body.right(), body.bottom() - 4.0),
                        ),
                        RichText::new(format!(
                            "{} · {}",
                            measure(language, size),
                            t(kind_key(node.kind))
                        )),
                    );
                    if response.clicked() {
                        if node.kind == Kind::Directory {
                            self.navigate(index);
                        } else {
                            self.select(index);
                            self.phone_overlay = Some(PhoneOverlay::Details);
                        }
                    }
                    let response = ui.put(
                        info,
                        egui::Button::new(RichText::new("i").italics()).frame(false),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            format!("{}: {}", t("phone_details"), node.name),
                        )
                    });
                    if response.clicked() {
                        self.select(index);
                        self.phone_overlay = Some(PhoneOverlay::Details);
                    }
                }
            });
    }
    fn phone_row_label(ui: &mut egui::Ui, bounds: egui::Rect, text: RichText) {
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(bounds)
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                ui.add(egui::Label::new(text).truncate().selectable(false));
            },
        );
    }
    fn phone_sheet(&mut self, ui: &mut egui::Ui, overlay: PhoneOverlay) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        let key = match overlay {
            PhoneOverlay::Actions => "phone_more",
            PhoneOverlay::Details => "phone_details",
            PhoneOverlay::ScanInfo => "phone_scan_info",
            PhoneOverlay::Filter => "phone_filter",
        };
        ui.horizontal(|ui| {
            if Self::phone_icon(ui, "←", t("close")) {
                self.phone_overlay = None;
            }
            ui.heading(t(key));
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt(("phone-sheet", key))
            .auto_shrink([false, false])
            .show(ui, |ui| match overlay {
                PhoneOverlay::Actions => {
                    if Self::phone_button(ui, t("phone_filter")) {
                        self.phone_overlay = Some(PhoneOverlay::Filter);
                    }
                    if Self::phone_button(ui, t("phone_details")) {
                        self.select(self.directory);
                        self.phone_overlay = Some(PhoneOverlay::Details);
                    }
                    if Self::phone_button(ui, t("phone_scan_info")) {
                        self.phone_overlay = Some(PhoneOverlay::ScanInfo);
                    }
                    if Self::phone_button(ui, t("settings")) {
                        self.settings_open = true;
                    }
                    if Self::phone_button(ui, t("rescan")) {
                        self.phone_overlay = None;
                        self.rescan();
                    }
                    if Self::phone_button(ui, t("phone_start")) {
                        self.phone_overlay = None;
                        self.phone_home = true;
                    }
                    ui.separator();
                    ui.label(t("export"));
                    for (key, json) in [("export_json", true), ("export_csv", false)] {
                        if ui
                            .add_enabled(
                                self.export.is_none(),
                                egui::Button::new(t(key))
                                    .min_size(egui::vec2(ui.available_width(), 48.0)),
                            )
                            .clicked()
                        {
                            self.save(json);
                        }
                    }
                    if let Some(key) = self.notice {
                        ui.label(t(key));
                    }
                }
                PhoneOverlay::Details => self.details(ui),
                PhoneOverlay::ScanInfo => self.summary(ui),
                PhoneOverlay::Filter => {
                    for (page, key) in [
                        (Page::Directory, "directory"),
                        (Page::Largest, "largest"),
                        (Page::Extensions, "extensions"),
                    ] {
                        if Self::phone_button(ui, t(key)) {
                            self.page = page;
                            self.map_group = None;
                            self.dirty = true;
                            if page == Page::Extensions {
                                self.phone_view = PhoneView::List;
                                self.query.clear();
                                self.allocated = false;
                            }
                            self.phone_overlay = None;
                        }
                    }
                    ui.separator();
                    ui.label(t("sort"));
                    for (sort, key) in [(Sort::Size, "size"), (Sort::Name, "name")] {
                        if ui.selectable_value(&mut self.sort, sort, t(key)).changed() {
                            self.dirty = true;
                        }
                    }
                    let available = self.result.as_ref().is_some_and(|r| {
                        r.nodes[0].allocated.known > 0 || r.nodes[0].allocated.unknown == 0
                    });
                    if ui
                        .add_enabled(
                            available,
                            egui::Checkbox::new(&mut self.allocated, t("allocated")),
                        )
                        .changed()
                    {
                        self.dirty = true;
                    }
                    if !available {
                        ui.label(t("allocated_unavailable"));
                    }
                }
            });
    }
}
