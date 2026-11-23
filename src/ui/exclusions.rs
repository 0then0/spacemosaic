use super::*;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[derive(Default)]
pub(super) struct Editor {
    pub rules: scan::Exclusions,
    pub open: bool,
    input: String,
    invalid: bool,
}

pub(super) fn show_rules(ui: &mut egui::Ui, language: usize, result: &ScanResult) {
    let rules = &result.exclusions;
    if rules.is_empty() {
        return;
    }
    let t = |key| i18n::text(language, key);
    let number = |n: usize| i18n::integer(language, n as u64);
    let skipped = rules.iter().filter(|rule| rule.skipped).count();
    egui::CollapsingHeader::new(format!(
        "{} ({}) · {}: {}",
        t("scan_applied_exclusions"),
        number(rules.len()),
        t("scan_skipped_folders"),
        number(skipped),
    ))
    .id_salt("applied-exclusions")
    .show(ui, |ui| {
        let height = ui.text_style_height(&egui::TextStyle::Body) * 2.0 + 8.0;
        egui::ScrollArea::vertical().max_height(150.0).show_rows(
            ui,
            height,
            rules.len(),
            |ui, range| {
                for index in range {
                    let rule = &rules[index];
                    ui.add(egui::Label::new(RichText::new(&rule.path).monospace()).truncate())
                        .on_hover_text(&rule.path);
                    ui.label(
                        RichText::new(t(if rule.skipped {
                            "scan_exclusion_skipped"
                        } else {
                            "scan_exclusion_not_observed"
                        }))
                        .small()
                        .weak(),
                    );
                    ui.add_space(4.0);
                }
            },
        );
    });
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl Editor {
    pub fn show(&mut self, context: &egui::Context, language: usize) {
        if !self.open {
            return;
        }
        let t = |key| i18n::text(language, key);
        let bounds = context.content_rect();
        let response = egui::Modal::new(egui::Id::unique("scan-exclusions")).show(context, |ui| {
            ui.set_width((bounds.width() - 40.0).clamp(160.0, 420.0));
            egui::ScrollArea::vertical()
                .max_height((bounds.height() - 40.0).max(120.0))
                .show(ui, |ui| {
                    ui.heading(t("scan_edit_exclusions"));
                    ui.label(t("scan_exclusions_help"));
                    let path_label = ui.label(t("scan_exclusion_path"));
                    let input = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.input)
                                .hint_text("packages/web/node_modules")
                                .desired_width(f32::INFINITY),
                        )
                        .labelled_by(path_label.id);
                    if input.changed() {
                        self.invalid = false;
                    }
                    let submit =
                        input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button(t("scan_add_exclusion")).clicked() || submit {
                        self.invalid = self.rules.add(&self.input).is_err();
                        if !self.invalid {
                            self.input.clear();
                        }
                        input.request_focus();
                    }
                    if self.invalid {
                        ui.label(
                            RichText::new(t("scan_exclusion_invalid"))
                                .color(ui.visuals().error_fg_color),
                        );
                    }
                    if !self.rules.paths().is_empty() {
                        ui.separator();
                        let mut remove = None;
                        for (index, path) in self.rules.paths().iter().enumerate() {
                            ui.horizontal(|ui| {
                                let button = ui.button(t("scan_remove_exclusion"));
                                let label = format!("{} {path}", t("scan_remove_exclusion"));
                                button.widget_info(|| {
                                    egui::WidgetInfo::labeled(egui::Role::Button, true, &label)
                                });
                                if button.clicked() {
                                    remove = Some(index);
                                }
                                ui.add(
                                    egui::Label::new(RichText::new(path).monospace()).truncate(),
                                )
                                .on_hover_text(path);
                            });
                        }
                        if let Some(index) = remove {
                            self.rules.remove(index);
                        }
                    }
                    ui.add_space(6.0);
                    ui.button(t("close")).clicked()
                })
                .inner
        });
        if response.inner || response.should_close() {
            self.open = false;
        }
    }
}
