// Release builds on Windows open without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod demo;
mod diagnosis;
mod gpu;
mod knowledge;
mod model;
mod platform;
mod sampler;
mod ui;

use eframe::egui;
use sampler::Source;

fn main() -> eframe::Result {
    // `--demo` shows simulated data (a laptop with two GPUs and a few problems).
    let demo = std::env::args().any(|a| a == "--demo");
    let source: Result<Box<dyn Source>, String> = if demo {
        Ok(Box::new(demo::Demo::new()))
    } else {
        sampler::Sampler::new().map(|s| Box::new(s) as Box<dyn Source>)
    };

    let icon = egui::IconData {
        rgba: include_bytes!("../assets/icon-64.rgba").to_vec(),
        width: 64,
        height: 64,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PC Bottleneck Finder")
            .with_inner_size([1180.0, 820.0])
            .with_min_inner_size([860.0, 560.0])
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native(
        "PC Bottleneck Finder",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, source, demo)))),
    )
}
