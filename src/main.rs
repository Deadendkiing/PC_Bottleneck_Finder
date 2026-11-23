// Release builds on Windows open without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod diagnosis;
mod knowledge;
mod model;
mod platform;
mod sampler;

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PC Bottleneck Finder")
            .with_inner_size([1000.0, 780.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "PC Bottleneck Finder",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
