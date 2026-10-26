//! Restrained, shadow-free surfaces in system, light, and dark modes.
use eframe::egui::{self, Color32, Stroke};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}
impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];
    pub fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Self::System => egui::ThemePreference::System,
            Self::Light => egui::ThemePreference::Light,
            Self::Dark => egui::ThemePreference::Dark,
        }
    }
}
fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}
pub fn accent(dark: bool) -> Color32 {
    rgb(if dark { 0x799c92 } else { 0x355c50 })
}
pub fn surface(dark: bool) -> Color32 {
    rgb(if dark { 0x1e1e1d } else { 0xf0efec })
}

pub fn install(context: &egui::Context) {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let dark = theme == egui::Theme::Dark;
        let mut style = egui::Style {
            visuals: if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            },
            ..Default::default()
        };
        let text = rgb(if dark { 0xfaf9f6 } else { 0x20201f });
        let muted = rgb(if dark { 0xafaeac } else { 0x60605c });
        let raised = rgb(if dark { 0x353534 } else { 0xe4e3df });
        let hover = rgb(if dark { 0x40403f } else { 0xd5d4ce });
        style.visuals.override_text_color = Some(text);
        style.visuals.weak_text_color = Some(muted);
        style.visuals.panel_fill = rgb(if dark { 0x121212 } else { 0xfaf9f6 });
        style.visuals.window_fill = surface(dark);
        style.visuals.extreme_bg_color = rgb(if dark { 0x090909 } else { 0xffffff });
        style.visuals.faint_bg_color = surface(dark);
        style.visuals.code_bg_color = raised;
        style.visuals.selection.bg_fill = hover;
        style.visuals.selection.stroke = Stroke::new(1.5, text);
        style.visuals.hyperlink_color = text;
        style.visuals.window_shadow = egui::epaint::Shadow::NONE;
        style.visuals.popup_shadow = egui::epaint::Shadow::NONE;
        style.visuals.window_stroke = Stroke::NONE;
        style.visuals.window_corner_radius = 16.into();
        style.visuals.text_edit_bg_color = Some(surface(dark));
        for widget in [
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.open,
        ] {
            widget.bg_fill = raised;
            widget.weak_bg_fill = raised;
            widget.bg_stroke = Stroke::NONE;
            widget.fg_stroke = Stroke::new(1.0, text);
            widget.corner_radius = 4.into();
        }
        for widget in [
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
        ] {
            widget.bg_fill = hover;
            widget.weak_bg_fill = hover;
            widget.bg_stroke = Stroke::new(1.5, text);
            widget.fg_stroke = Stroke::new(1.0, text);
            widget.corner_radius = 4.into();
        }
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, muted);
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::NONE;
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.spacing.button_padding = egui::vec2(14.0, 9.0);
        style.spacing.interact_size = egui::vec2(48.0, 44.0);
        style.animation_time = 0.0;
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(28.0));
        context.set_style_of(theme, style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn system_tracks_os_and_explicit_selection_overrides_it() {
        let context = egui::Context::default();
        install(&context);
        for (choice, system, expected) in [
            (Appearance::System, egui::Theme::Dark, egui::Theme::Dark),
            (Appearance::System, egui::Theme::Light, egui::Theme::Light),
            (Appearance::Dark, egui::Theme::Light, egui::Theme::Dark),
            (Appearance::Light, egui::Theme::Dark, egui::Theme::Light),
            (Appearance::System, egui::Theme::Dark, egui::Theme::Dark),
        ] {
            context.set_theme(choice.preference());
            let mut output = context.run_ui(
                egui::RawInput {
                    system_theme: Some(system),
                    ..Default::default()
                },
                |_| {},
            );
            output.textures_delta.clear();
            assert_eq!(context.theme(), expected);
        }
    }
    #[test]
    fn text_and_surfaces_have_readable_contrast_in_both_themes() {
        fn luminance(color: Color32) -> f64 {
            let values = [color.r(), color.g(), color.b()].map(|v| {
                let v = f64::from(v) / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            });
            values[0] * 0.2126 + values[1] * 0.7152 + values[2] * 0.0722
        }
        let context = egui::Context::default();
        install(&context);
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let style = context.style_of(theme);
            for foreground in [
                style.visuals.override_text_color.unwrap(),
                style.visuals.weak_text_color.unwrap(),
                accent(theme == egui::Theme::Dark),
            ] {
                let a = luminance(foreground);
                let b = luminance(style.visuals.panel_fill);
                assert!((a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5);
            }
            assert_eq!(style.visuals.window_shadow, egui::epaint::Shadow::NONE);
        }
    }
}
