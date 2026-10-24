#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(format!(
                "FL ML Desk v{} — Football Life 2026 / PES 2021",
                env!("CARGO_PKG_VERSION")
            ))
            .with_inner_size([1200.0, 840.0])
            .with_min_inner_size([980.0, 680.0]),
        ..Default::default()
    };
    eframe::run_native("FL ML Desk", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
