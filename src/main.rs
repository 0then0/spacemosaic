#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn spacemosaic_register_about_icon();
}

fn main() -> eframe::Result {
    // Folders are scanned; a .json file is opened as a saved result. Both may be given.
    let paths: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                    .expect("bundled application icon must be valid PNG"),
            )
            .with_inner_size([1180.0, 900.0])
            .with_min_inner_size([320.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SpaceMosaic",
        options,
        Box::new(move |cc| {
            #[cfg(target_os = "macos")]
            // SAFETY: eframe creates the application on the AppKit main thread.
            unsafe {
                spacemosaic_register_about_icon();
            }
            let mut app = spacemosaic::ui::ExplorerApp::new(cc);
            for path in paths {
                let saved = path.is_file()
                    && path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("json"));
                if saved {
                    app.open_result(path);
                } else {
                    app.start(path);
                }
            }
            Ok(Box::new(app))
        }),
    )
}
