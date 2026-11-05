use super::*;

/// Compressed adjacency: one offset per node and one index per edge. No
/// per-file Vec headers, per-directory allocations, or build-time cursor array.
pub(super) struct TreeState {
    offsets: Vec<usize>,
    edges: Vec<usize>,
    pub expanded: HashSet<usize>,
    pub rows: Vec<(usize, usize)>,
    sort: Option<(Sort, bool)>,
}
impl Default for TreeState {
    fn default() -> Self {
        Self {
            offsets: Vec::new(),
            edges: Vec::new(),
            expanded: HashSet::from([0]),
            rows: Vec::new(),
            sort: None,
        }
    }
}
impl TreeState {
    pub fn children(&self, index: usize) -> &[usize] {
        &self.edges[self.offsets[index]..self.offsets[index + 1]]
    }
    pub fn refresh(&mut self, result: &ScanResult, directory: usize, sort: Sort, allocated: bool) {
        if self.offsets.len() != result.nodes.len() + 1 {
            self.offsets = vec![0; result.nodes.len() + 1];
            for node in &result.nodes {
                if let Some(parent) = node.parent {
                    self.offsets[parent + 1] += 1;
                }
            }
            for index in 1..self.offsets.len() {
                self.offsets[index] += self.offsets[index - 1];
            }
            self.edges = vec![0; *self.offsets.last().unwrap()];
            for (index, node) in result.nodes.iter().enumerate() {
                if let Some(parent) = node.parent {
                    self.edges[self.offsets[parent]] = index;
                    self.offsets[parent] += 1;
                }
            }
            // Insertion advanced starts to ends. Shift them back into starts.
            self.offsets.copy_within(..result.nodes.len(), 1);
            self.offsets[0] = 0;
            self.sort = None;
        }
        if self.sort != Some((sort, allocated)) {
            for bounds in self.offsets.windows(2) {
                self.edges[bounds[0]..bounds[1]].sort_unstable_by(|a, b| {
                    let size = |i: usize| {
                        if allocated {
                            result.nodes[i].allocated.known
                        } else {
                            result.nodes[i].logical.known
                        }
                    };
                    let name = || {
                        scan::compare_names(&result.nodes[*a].name, &result.nodes[*b].name)
                            .then(a.cmp(b))
                    };
                    if sort == Sort::Size {
                        size(*b).cmp(&size(*a)).then_with(name)
                    } else {
                        name()
                    }
                });
            }
            self.sort = Some((sort, allocated));
        }
        self.rows.clear();
        let mut pending = vec![(directory, 0)];
        while let Some((index, depth)) = pending.pop() {
            self.rows.push((index, depth));
            if self.expanded.contains(&index) {
                pending.extend(self.children(index).iter().rev().map(|i| (*i, depth + 1)));
            }
        }
    }
}

impl ExplorerApp {
    pub(super) fn table(&mut self, ui: &mut egui::Ui, height: f32) {
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
                if !result.nodes[self.navigation.directory].complete && self.search.query.is_empty()
                {
                    "incomplete_directory"
                } else {
                    "no_results"
                },
            ));
            return;
        }
        let tree = self.navigation.page == Page::Directory
            && self.search.query.is_empty()
            && self.map.group.is_none();
        let count = if tree {
            self.tree.rows.len()
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
                self.map
                    .group
                    .as_ref()
                    .and_then(|group| group.first())
                    .copied(),
            ))
            .max_height(height)
            .auto_shrink([false, false])
            .show_rows(ui, row_height, count, |ui, range| {
                for position in range {
                    let (index, depth) = if tree {
                        self.tree.rows[position]
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
                                && !self.tree.children(index).is_empty()
                            {
                                let open = self.tree.expanded.contains(&index);
                                let response = ui.add_sized(
                                    [28.0, row_height],
                                    egui::Button::new("").frame_when_inactive(false),
                                );
                                response.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::Role::Button,
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
                                        self.tree.expanded.remove(&index);
                                    } else {
                                        self.tree.expanded.insert(index);
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
                                    .frame_when_inactive(self.navigation.selected == Some(index))
                                    .selected(self.navigation.selected == Some(index))
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
}
