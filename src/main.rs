// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
#[cfg(windows)]
mod display;
mod fsops;
mod i18n;
mod icons;
mod platform;
#[cfg(windows)]
mod setup;
#[cfg(windows)]
mod shell;
mod theme;
#[cfg(windows)]
mod taskmgr;
#[cfg(windows)]
mod update;

use eframe::egui;
use std::path::PathBuf;

fn main() -> Result<(), eframe::Error> {
    // Delete the exe an earlier update replaced (it was still running then).
    #[cfg(windows)]
    update::cleanup_old();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);

    // Installer (`delfin-setup.exe` or `--setup`) and uninstaller (`--uninstall`,
    // what Windows Settings › Apps runs).
    #[cfg(windows)]
    {
        // Task manager. When it replaces taskmgr.exe, Windows starts it as
        // `delfin.exe --taskmgr <path of taskmgr.exe>`, so this comes first.
        if has("--taskmgr") {
            return taskmgr::run();
        }
        if let Some(i) = args.iter().position(|a| a == "--replace-taskmgr") {
            let on = args.get(i + 1).map(|v| v != "off").unwrap_or(true);
            if let Err(e) = taskmgr::replace(on) {
                platform::message("Delfin", &e);
            }
            return Ok(());
        }
        if has("--display") {
            return display::run();
        }
        if has("--uninstall") {
            return setup::run_uninstall();
        }
        if has("--silent") && (has("--setup") || setup::started_as_setup()) {
            std::process::exit(setup::run_install_silent());
        }
        if has("--setup") || (setup::started_as_setup() && !has("--shell")) {
            return setup::run_install();
        }
    }

    // Desktop shell mode (panel + launcher + desktop), replaces explorer.exe.
    if has("--shell") {
        #[cfg(windows)]
        return shell::run();
        #[cfg(not(windows))]
        {
            platform::message("Delfin", "Shell mode is only available on Windows.");
            return Ok(());
        }
    }
    if has("--install-shell") {
        match platform::install_shell() {
            Ok(m) => platform::message("Delfin", &m),
            Err(e) => platform::message("Delfin – error", &e),
        }
        return Ok(());
    }
    if has("--uninstall-shell") {
        match platform::uninstall_shell() {
            Ok(m) => platform::message("Delfin", &m),
            Err(e) => platform::message("Delfin – error", &e),
        }
        return Ok(());
    }

    // File manager, optionally in a folder: `delfin.exe C:\Projects`
    let start = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Delfin")
            .with_app_id("delfin")
            .with_icon(theme::app_icon())
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
