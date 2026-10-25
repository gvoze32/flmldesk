#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod theme;

pub const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

fn main() -> eframe::Result {
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title(format!("FL ML Desk v{} — Football Life 2026 / PES 2021", env!("CARGO_PKG_VERSION")))
        .with_inner_size([1200.0, 840.0])
        .with_min_inner_size([980.0, 680.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(ICON_PNG) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "FL ML Desk",
        options,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app::App::new(cc)))
        }),
    )
}
