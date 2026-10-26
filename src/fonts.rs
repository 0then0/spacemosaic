//! Offline UI fonts with explicit weight roles and Japanese fallback.
use eframe::egui::{
    FontData, FontDefinitions, FontFamily, FontTweak, epaint::text::VariationCoords,
};
use std::sync::Arc;

pub(crate) fn heading_family() -> FontFamily {
    FontFamily::Name("inter-medium".into())
}

pub(crate) fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "inter-regular".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "inter-medium".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Medium.ttf"
        ))),
    );
    fonts.font_data.insert(
        "japanese".into(),
        Arc::new(
            FontData::from_static(include_bytes!("../assets/fonts/NotoSansJP.ttf")).tweak(
                FontTweak {
                    coords: VariationCoords::new([(b"wght", 400.0)]),
                    ..Default::default()
                },
            ),
        ),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("japanese".into());
    }
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    let mut headings = proportional.clone();
    proportional.insert(0, "inter-regular".into());
    headings.insert(0, "inter-medium".into());
    fonts.families.insert(heading_family(), headings);
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{self, FontId};

    #[test]
    fn roles_keep_inter_primary_and_hack_monospace_with_japanese_fallback() {
        let fonts = definitions();
        assert_eq!(
            fonts.families[&FontFamily::Proportional][0],
            "inter-regular"
        );
        assert_eq!(fonts.families[&heading_family()][0], "inter-medium");
        assert_eq!(fonts.families[&FontFamily::Monospace][0], "Hack");
        for family in [
            FontFamily::Proportional,
            heading_family(),
            FontFamily::Monospace,
        ] {
            assert_eq!(fonts.families[&family].last().unwrap(), "japanese");
        }
    }

    #[test]
    fn bundled_fonts_render_latin_cyrillic_and_japanese_offline() {
        let context = egui::Context::default();
        context.set_fonts(definitions());
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            for family in [
                FontFamily::Proportional,
                heading_family(),
                FontFamily::Monospace,
            ] {
                let font = FontId::new(15.0, family);
                ui.fonts_mut(|fonts| {
                    // Compare rasterized glyphs with an unassigned code point. egui 0.36's
                    // has_glyph reports false for supported chars in its replacement face.
                    let missing =
                        fonts.layout_no_wrap("\u{0378}".into(), font.clone(), egui::Color32::WHITE);
                    let missing_uv = missing.rows[0].glyphs[0].uv_rect;
                    for character in "SpaceMosaicРазмер日本語0123456789".chars() {
                        let galley = fonts.layout_no_wrap(
                            character.to_string(),
                            font.clone(),
                            egui::Color32::WHITE,
                        );
                        let uv = galley.rows[0].glyphs[0].uv_rect;
                        assert!(!uv.is_nothing(), "empty glyph for {character:?}");
                        assert_ne!(
                            (uv.min, uv.max),
                            (missing_uv.min, missing_uv.max),
                            "replacement glyph for {character:?} in {:?}",
                            font.family
                        );
                    }
                });
            }
        });
        output.textures_delta.clear();
    }
}
