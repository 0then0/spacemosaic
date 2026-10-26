#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn spacemosaic_register_about_icon();
}

fn main() -> eframe::Result {
    let path = std::env::args_os().nth(1).map(std::path::PathBuf::from);
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
            if let Some(path) = path {
                app.start(path);
            }
            Ok(Box::new(app))
        }),
    )
}
