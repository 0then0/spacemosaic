use super::*;

pub(super) struct NavigationState {
    pub directory: usize,
    pub selected: Option<usize>,
    pub page: Page,
}
impl Default for NavigationState {
    fn default() -> Self {
        Self {
            directory: 0,
            selected: None,
            page: Page::Directory,
        }
    }
}

impl ExplorerApp {
    pub(super) fn navigate(&mut self, index: usize) {
        self.phone_overlay = None;
        self.navigation.directory = index;
        self.tree.expanded.insert(index);
        self.navigation.selected = Some(index);
        self.navigation.page = Page::Directory;
        self.search.clear();
        self.map.group = None;
        self.dirty = true;
    }
    pub(super) fn back(&mut self) {
        if self.map.group.take().is_some() {
            self.search.clear();
            self.dirty = true;
            return;
        }
        if let Some(parent) = self
            .result
            .as_ref()
            .and_then(|r| r.nodes[self.navigation.directory].parent)
        {
            self.navigate(parent);
        }
    }
    pub(super) fn open_group(&mut self) {
        if self.map.group.is_none() && !self.map.grouped.is_empty() {
            self.phone_view = PhoneView::List;
            self.map.group = Some(self.map.grouped.clone());
            self.dirty = true;
        }
    }
    pub(super) fn navigation(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        let t = |key| i18n::text(language, key);
        ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for (page, key) in [
                (Page::Directory, "directory"),
                (Page::Largest, "largest"),
                (Page::Extensions, "extensions"),
            ] {
                if ui
                    .selectable_value(&mut self.navigation.page, page, t(key))
                    .changed()
                {
                    self.map.group = None;
                    if self.navigation.page == Page::Extensions {
                        self.allocated = false;
                    }
                    self.dirty = true;
                }
            }
        });
        if self.navigation.page != Page::Extensions {
            ui.horizontal_wrapped(|ui| {
                let label = ui.label(t("search"));
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.search.query)
                            .desired_width(240.0)
                            .hint_text(t("search_hint")),
                    )
                    .labelled_by(label.id)
                    .changed()
                {
                    self.search.edited(Instant::now());
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
        if self.navigation.page == Page::Directory || self.map.group.is_some() {
            self.path_navigation(ui);
        }
    }
    pub(super) fn back_button(&mut self, ui: &mut egui::Ui) {
        if ui
            .add_enabled(
                self.navigation.directory != 0 || self.map.group.is_some(),
                egui::Button::new(i18n::text(self.language, "back")),
            )
            .clicked()
        {
            self.back();
        }
    }
    pub(super) fn path_navigation(&mut self, ui: &mut egui::Ui) -> egui::Response {
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
                                                .breadcrumbs(self.navigation.directory)
                                                .into_iter()
                                                .enumerate()
                                            {
                                                if position > 0 {
                                                    ui.label(RichText::new("›").weak());
                                                }
                                                let node = &result.nodes[index];
                                                if index == self.navigation.directory
                                                    && self.map.group.is_none()
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
                                        if self.map.group.is_some() {
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
    pub(super) fn select(&mut self, index: usize) {
        self.navigation.selected = Some(index);
        if let Some(result) = &self.result {
            for ancestor in result
                .breadcrumbs(index)
                .into_iter()
                .take_while(|i| *i != index)
            {
                self.tree.expanded.insert(ancestor);
            }
        }
        self.map.selected = self.selected_tile();
        if self.navigation.page == Page::Directory
            && self.search.query.is_empty()
            && self.map.group.is_none()
        {
            self.dirty = true;
        }
    }
}
