// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod fsops;
mod icons;
mod platform;
mod theme;

use eframe::egui;
use std::path::PathBuf;

fn main() -> Result<(), eframe::Error> {
    // Optional: start in a folder passed on the command line, e.g. `delfin.exe C:\Projects`
    let start = std::env::args().nth(1).map(PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Delfin")
            .with_app_id("delfin")
            .with_inner_size([1180.0, 740.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Delfin",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, start)))),
    )
}
