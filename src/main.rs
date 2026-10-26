fn main() -> eframe::Result {
    let path = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 820.0])
            .with_min_inner_size([320.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SpaceMosaic",
        options,
        Box::new(move |cc| {
            let mut app = spacemosaic::ui::ExplorerApp::new(cc);
            if let Some(path) = path {
                app.start(path);
            }
            Ok(Box::new(app))
        }),
    )
}
