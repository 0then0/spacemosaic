use super::*;

#[derive(Default)]
pub(super) struct MapState {
    pub group: Option<Vec<usize>>,
    pub objects: Vec<Option<usize>>,
    pub selected: Option<usize>,
    pub weights: Vec<f64>,
    pub grouped: Vec<usize>,
}
impl MapState {
    pub fn rebuild(
        &mut self,
        result: &ScanResult,
        rows: &[usize],
        searching: bool,
        allocated: bool,
    ) {
        self.objects.clear();
        self.weights.clear();
        self.grouped.clear();
        // A grouped tile opens its complete virtualized list, never another map
        // containing a slightly smaller copy of the same grouped tile.
        if self.group.is_none() {
            let total: f64 = rows
                .iter()
                .filter_map(|i| {
                    let node = &result.nodes[*i];
                    if searching && node.kind == Kind::Directory {
                        None
                    } else {
                        Some(if allocated {
                            node.allocated.known as f64
                        } else {
                            node.logical.known as f64
                        })
                    }
                })
                .sum();
            for index in rows.iter().copied() {
                let node = &result.nodes[index];
                let weight = if allocated {
                    node.allocated.known
                } else {
                    node.logical.known
                };
                if weight == 0 || (searching && node.kind == Kind::Directory) {
                    continue;
                }
                if self.objects.len() < 95 && weight as f64 >= total * 0.015 {
                    self.objects.push(Some(index));
                    self.weights.push(weight as f64);
                } else {
                    self.grouped.push(index);
                }
            }
            if !self.grouped.is_empty() {
                self.objects.push(None);
                self.weights.push(
                    self.grouped
                        .iter()
                        .map(|i| {
                            if allocated {
                                result.nodes[*i].allocated.known as f64
                            } else {
                                result.nodes[*i].logical.known as f64
                            }
                        })
                        .sum(),
                );
            }
        }
    }
}

impl ExplorerApp {
    /// Map a nested selection to its nearest visible ancestor or grouped tile.
    pub(super) fn selected_tile(&self) -> Option<usize> {
        let result = self.result.as_ref()?;
        let mut index = self.navigation.selected?;
        loop {
            if let Some(position) = self.map.objects.iter().position(|i| *i == Some(index)) {
                return Some(position);
            }
            if self.map.grouped.contains(&index) {
                return self.map.objects.iter().position(Option::is_none);
            }
            index = result.nodes[index].parent?;
        }
    }
    pub(super) fn map(&mut self, ui: &mut egui::Ui, height: f32, fit: bool) {
        let Some(result) = self.result.clone() else {
            return;
        };
        let language = self.language;
        let t = |key| i18n::text(language, key);
        let selected_tile = self.map.selected;
        if !self.compact_layout {
            ui.label(
                RichText::new(t("treemap"))
                    .small()
                    .color(theme::accent(ui.visuals().dark_mode)),
            );
            ui.label(RichText::new(t("map_help")).small().weak());
            if let Some(index) = self.navigation.selected {
                let path = format!("{}: {}", t("selected_item"), result.nodes[index].locator);
                ui.add(egui::Label::new(&path).truncate())
                    .on_hover_text(path);
            }
        }
        let objects = self.map.objects.clone();
        let weights = self.map.weights.clone();
        let group_height = if self.map.grouped.is_empty() {
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
        for (position, rect) in crate::treemap::squarify(
            &weights,
            crate::treemap::Rect {
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
            let response = ui.interact(
                area,
                ui.scope_id().with(("tile", position)),
                egui::Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label)
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
                    if self.compact_layout {
                        self.phone_overlay = Some(PhoneOverlay::Details);
                    }
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
        if !self.map.grouped.is_empty()
            && ui
                .button(format!(
                    "{} ({})",
                    t("grouped_items"),
                    i18n::integer(language, self.map.grouped.len() as u64)
                ))
                .clicked()
        {
            self.open_group();
        }
    }
}
