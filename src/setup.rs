//! Installer and uninstaller. They are the same `delfin.exe`:
//!
//! * named `delfin-setup*.exe` (as published on GitHub) or run with `--setup`,
//!   it shows the install wizard;
//! * `delfin.exe --uninstall` (what Windows "Apps" runs) removes it again.
//!
//! Everything is per user: the program goes to `%LOCALAPPDATA%\Programs\Delfin`
//! and all registry entries live under HKEY_CURRENT_USER, so no admin rights.

use crate::i18n::{tr, trf, Lang};
use crate::theme::{self, Palette};
use eframe::egui::{self, vec2, Color32, RichText, Stroke};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Delfin";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Was this exe started as the installer?
pub fn started_as_setup() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()))
        .is_some_and(|n| n.starts_with("delfin-setup") || n.starts_with("delfin_setup"))
}

pub fn install_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from(r"C:\Delfin"))
        .join("Programs")
        .join("Delfin")
}

pub fn installed_exe() -> PathBuf {
    install_dir().join("delfin.exe")
}

fn start_menu_link() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(r"Microsoft\Windows\Start Menu\Programs\Delfin.lnk"))
}

fn desktop_link() -> Option<PathBuf> {
    dirs::desktop_dir().map(|d| d.join("Delfin.lnk"))
}

#[derive(Clone, Copy)]
struct Options {
    /// Delfin replaces Explorer: panel, launcher, desktop, tray at sign-in.
    shell: bool,
    /// Folders and drives open in Delfin everywhere in Windows.
    folders: bool,
    start_menu: bool,
    desktop: bool,
}

// ------------------------------------------------------------------ install

fn run_hidden(cmd: &str) {
    let _ = Command::new("cmd.exe")
        .raw_arg(format!("/C {cmd}"))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

/// Copy this exe into the install folder. A running Delfin keeps the old file
/// locked, but it can be renamed out of the way (deleted on its next start).
fn copy_program() -> Result<PathBuf, String> {
    let src = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = install_dir();
    let dst = installed_exe();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if src == dst {
        return Ok(dst);
    }
    if std::fs::copy(&src, &dst).is_err() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let old = dir.join(format!("delfin.old-{stamp}.exe"));
        std::fs::rename(&dst, &old).map_err(|e| e.to_string())?;
        std::fs::copy(&src, &dst).map_err(|e| e.to_string())?;
    }
    Ok(dst)
}

/// A .lnk via the WScript.Shell COM object (paths passed through the environment,
/// so names with spaces or Polish letters need no quoting).
fn create_shortcut(lnk: &Path, target: &Path, description: &str) -> Result<(), String> {
    let script = "$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:DELFIN_LNK); \
                  $s.TargetPath = $env:DELFIN_TARGET; \
                  $s.WorkingDirectory = $env:DELFIN_DIR; \
                  $s.IconLocation = $env:DELFIN_TARGET + ',0'; \
                  $s.Description = $env:DELFIN_DESC; \
                  $s.Save()";
    if let Some(d) = lnk.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("DELFIN_LNK", lnk)
        .env("DELFIN_TARGET", target)
        .env("DELFIN_DIR", target.parent().unwrap_or(Path::new(".")))
        .env("DELFIN_DESC", description)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() && lnk.exists() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// The entry in Settings › Apps (and Control Panel › Programs).
fn register_uninstall(exe: &Path) -> Result<(), String> {
    let (k, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(UNINSTALL_KEY)
        .map_err(|e| e.to_string())?;
    let size_kb = std::fs::metadata(exe).map(|m| (m.len() / 1024) as u32).unwrap_or(0);
    let s = |name: &str, v: String| k.set_value(name, &v).map_err(|e| e.to_string());
    s("DisplayName", "Delfin".into())?;
    s("DisplayVersion", crate::update::CURRENT.into())?;
    s("Publisher", "Atimaniac".into())?;
    s("DisplayIcon", format!("{},0", exe.display()))?;
    s("InstallLocation", install_dir().display().to_string())?;
    s("UninstallString", format!("\"{}\" --uninstall", exe.display()))?;
    s("URLInfoAbout", "https://github.com/Atimaniac/kde".into())?;
    k.set_value("NoModify", &1u32).map_err(|e| e.to_string())?;
    k.set_value("NoRepair", &1u32).map_err(|e| e.to_string())?;
    k.set_value("EstimatedSize", &size_kb).map_err(|e| e.to_string())?;
    Ok(())
}

fn install(o: Options) -> Result<(), String> {
    let exe = copy_program().map_err(|e| trf("Couldn't copy Delfin: {}", &[&e]))?;
    if o.start_menu {
        if let Some(l) = start_menu_link() {
            create_shortcut(&l, &exe, tr("File manager")).map_err(|e| trf("Couldn't create the shortcut: {}", &[&e]))?;
        }
    }
    if o.desktop {
        if let Some(l) = desktop_link() {
            create_shortcut(&l, &exe, tr("File manager")).map_err(|e| trf("Couldn't create the shortcut: {}", &[&e]))?;
        }
    }
    register_uninstall(&exe)?;
    // These copy the exe to the same place (already there) and write the
    // registry: Winlogon "Shell" and the Directory/Drive open handlers.
    if o.shell {
        crate::platform::install_shell()?;
    } else if crate::platform::shell_installed() {
        // Installed as shell before, not any more.
        crate::platform::uninstall_shell()?;
    }
    if !o.shell && o.folders {
        crate::platform::set_folder_handler(true)?;
    }
    if !o.folders {
        let _ = crate::platform::set_folder_handler(false);
    }
    Ok(())
}

fn explorer_running() -> bool {
    Command::new("tasklist.exe")
        .args(["/FI", "IMAGENAME eq explorer.exe", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase().contains("explorer.exe"))
        .unwrap_or(false)
}

/// Close a running Delfin shell (the panel window), if any.
fn close_running_shell() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};
    let title: Vec<u16> = "Delfin Panel".encode_utf16().chain(Some(0)).collect();
    unsafe {
        let h = FindWindowW(std::ptr::null(), title.as_ptr());
        if h.is_null() {
            return false;
        }
        PostMessageW(h, WM_CLOSE, 0, 0);
    }
    true
}

/// Swap Explorer for the Delfin shell right now, without signing out.
fn switch_now() {
    let exe = installed_exe();
    let old_shell = close_running_shell();
    if explorer_running() {
        run_hidden("taskkill /f /im explorer.exe");
    }
    // Give the old shell / Explorer a moment to go away (only one shell runs).
    let wait = if old_shell { 3 } else { 2 };
    run_hidden(&format!("ping -n {wait} 127.0.0.1 >nul & start \"\" \"{}\" --shell", exe.display()));
}

// ------------------------------------------------------------------ uninstall

fn uninstall(remove_settings: bool) -> Result<(), String> {
    // Back to Explorer as shell and folder handler.
    crate::platform::uninstall_shell()?;
    for l in [start_menu_link(), desktop_link()].into_iter().flatten() {
        let _ = std::fs::remove_file(l);
    }
    let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(UNINSTALL_KEY);
    let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(r"Software\Delfin");

    // If Delfin is the running shell, close it and bring Explorer back.
    let had_shell = close_running_shell();
    if had_shell || !explorer_running() {
        run_hidden("ping -n 3 127.0.0.1 >nul & start \"\" explorer.exe");
    }
    // The files can only go once this program has exited.
    let mut cmd = format!("ping -n 4 127.0.0.1 >nul & rmdir /s /q \"{}\"", install_dir().display());
    if remove_settings {
        if let Some(cfg) = dirs::config_dir() {
            cmd += &format!(" & rmdir /s /q \"{}\"", cfg.join("delfin").display());
        }
    }
    run_hidden(&cmd);
    Ok(())
}

// ------------------------------------------------------------------ wizard UI

#[derive(PartialEq, Eq)]
enum Mode {
    Install,
    Uninstall,
}

enum Page {
    Options,
    Working,
    Done(Result<(), String>),
}

struct Wizard {
    mode: Mode,
    page: Page,
    opts: Options,
    remove_settings: bool,
    result: Arc<Mutex<Option<Result<(), String>>>>,
    pal: Palette,
}

pub fn run_install() -> Result<(), eframe::Error> {
    run(Mode::Install)
}

/// `--setup --silent`: install with the default choices (shell, folders,
/// Start menu shortcut) without showing the wizard. Exit code 0 = success.
pub fn run_install_silent() -> i32 {
    let o = Options { shell: true, folders: true, start_menu: true, desktop: false };
    match install(o) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

pub fn run_uninstall() -> Result<(), eframe::Error> {
    run(Mode::Uninstall)
}

fn run(mode: Mode) -> Result<(), eframe::Error> {
    let title = if mode == Mode::Install { tr("Delfin Setup") } else { tr("Uninstall Delfin") };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_icon(theme::app_icon())
            .with_inner_size([600.0, 500.0])
            .with_resizable(false)
            .with_maximize_button(false),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        title,
        options,
        Box::new(move |cc| {
            theme::install_fonts(&cc.egui_ctx);
            let dark = crate::config::Config::load().dark;
            theme::apply(&cc.egui_ctx, dark);
            Ok(Box::new(Wizard {
                mode,
                page: Page::Options,
                opts: Options { shell: true, folders: true, start_menu: true, desktop: false },
                remove_settings: false,
                result: Arc::new(Mutex::new(None)),
                pal: theme::palette(dark),
            }))
        }),
    )
}

fn logo(ui: &mut egui::Ui, pal: &Palette) {
    let (r, _) = ui.allocate_exact_size(vec2(56.0, 56.0), egui::Sense::hover());
    let p = ui.painter();
    let (c, rad) = (r.center(), 26.0);
    p.circle_filled(c, rad, pal.accent);
    for i in 0..3 {
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / 3.0;
        p.circle_filled(c + vec2(a.cos(), a.sin()) * rad * 0.46, rad * 0.2, Color32::WHITE);
    }
    p.circle_stroke(c, rad * 0.46, Stroke::new(rad * 0.1, Color32::WHITE));
}

impl eframe::App for Wizard {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Page::Working = self.page {
            if let Some(r) = self.result.lock().unwrap_or_else(|p| p.into_inner()).take() {
                self.page = Page::Done(r);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        let pal = self.pal;
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(pal.window).inner_margin(egui::Margin::same(24.0)))
            .show(ctx, |ui| {
                // Header: logo, name, version, language.
                ui.horizontal(|ui| {
                    logo(ui, &pal);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.add_space(6.0);
                        ui.label(RichText::new("Delfin").size(24.0).strong());
                        ui.label(RichText::new(trf("Version {}", &[&crate::update::CURRENT])).color(pal.dim));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        let mut lang = crate::i18n::lang();
                        ui.selectable_value(&mut lang, Lang::Pl, "Polski");
                        ui.selectable_value(&mut lang, Lang::En, "English");
                        if lang != crate::i18n::lang() {
                            crate::i18n::set_lang(lang);
                        }
                    });
                });
                ui.add_space(14.0);
                ui.separator();
                ui.add_space(10.0);

                match (&self.mode, &self.page) {
                    (Mode::Install, Page::Options) => self.install_options(ui, &pal),
                    (Mode::Uninstall, Page::Options) => self.uninstall_options(ui, &pal),
                    (_, Page::Working) => {
                        ui.add_space(40.0);
                        ui.vertical_centered(|ui| {
                            ui.spinner();
                            ui.add_space(8.0);
                            ui.label(if self.mode == Mode::Install { tr("Installing…") } else { tr("Removing…") });
                        });
                    }
                    (_, Page::Done(r)) => {
                        let r = r.clone();
                        self.done(ui, ctx, &pal, r);
                    }
                }
            });
    }
}

impl Wizard {
    fn start<F: FnOnce() -> Result<(), String> + Send + 'static>(&mut self, job: F) {
        self.page = Page::Working;
        let slot = self.result.clone();
        std::thread::spawn(move || {
            let r = job();
            *slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(r);
        });
    }

    fn install_options(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui.label(RichText::new(tr("Delfin replaces Windows Explorer: a Plasma-style panel, launcher, desktop and system tray, plus the Delfin file manager.")).size(14.0));
        ui.add_space(12.0);
        ui.checkbox(&mut self.opts.shell, RichText::new(tr("Use Delfin as the Windows shell (panel, launcher, desktop)")).size(14.0));
        ui.add_enabled_ui(!self.opts.shell, |ui| {
            if self.opts.shell {
                self.opts.folders = true;
            }
            ui.checkbox(&mut self.opts.folders, RichText::new(tr("Open folders in Delfin instead of Explorer")).size(14.0));
        });
        ui.checkbox(&mut self.opts.start_menu, RichText::new(tr("Start menu shortcut")).size(14.0));
        ui.checkbox(&mut self.opts.desktop, RichText::new(tr("Desktop shortcut")).size(14.0));
        ui.add_space(12.0);
        ui.label(RichText::new(trf("Installs to: {}", &[&install_dir().display()])).size(12.0).color(pal.dim));
        ui.label(RichText::new(tr("Only for your Windows account. No administrator rights needed.")).size(12.0).color(pal.dim));
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new(tr("Install")).size(15.0)).min_size(vec2(130.0, 34.0))).clicked() {
                    let o = self.opts;
                    self.start(move || install(o));
                }
                if ui.add(egui::Button::new(tr("Cancel")).min_size(vec2(90.0, 34.0))).clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }

    fn uninstall_options(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui.label(RichText::new(tr("Remove Delfin from this computer? Windows Explorer becomes the shell and file manager again.")).size(14.0));
        ui.add_space(12.0);
        ui.checkbox(&mut self.remove_settings, RichText::new(tr("Also remove my Delfin settings")).size(14.0));
        ui.add_space(8.0);
        ui.label(RichText::new(tr("If Delfin is running as your shell, it closes and Explorer starts.")).size(12.0).color(pal.dim));
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new(tr("Uninstall")).size(15.0)).min_size(vec2(130.0, 34.0))).clicked() {
                    let remove = self.remove_settings;
                    self.start(move || uninstall(remove));
                }
                if ui.add(egui::Button::new(tr("Cancel")).min_size(vec2(90.0, 34.0))).clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }

    fn done(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, pal: &Palette, r: Result<(), String>) {
        match r {
            Err(e) => {
                ui.label(RichText::new(tr("Something went wrong")).size(16.0).color(pal.danger));
                ui.add_space(6.0);
                ui.label(e);
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
                    if ui.add(egui::Button::new(tr("Close")).min_size(vec2(90.0, 34.0))).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            }
            Ok(()) if self.mode == Mode::Uninstall => {
                ui.label(RichText::new(tr("Delfin has been removed.")).size(16.0));
                ui.add_space(6.0);
                ui.label(RichText::new(tr("Its files are deleted a few seconds after this window closes.")).color(pal.dim));
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
                    if ui.add(egui::Button::new(tr("Close")).min_size(vec2(90.0, 34.0))).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            }
            Ok(()) => {
                ui.label(RichText::new(tr("Delfin is installed.")).size(16.0));
                ui.add_space(6.0);
                if self.opts.shell {
                    ui.label(tr("Delfin starts as your shell the next time you sign in. You can also switch right now: Explorer closes and the Delfin panel starts (open windows stay open)."));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(tr("If anything goes wrong: Ctrl+Shift+Esc → Run new task → explorer.exe"))
                            .size(12.0)
                            .color(pal.dim),
                    );
                } else {
                    ui.label(tr("Open Delfin from the Start menu."));
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
                    ui.horizontal(|ui| {
                        if self.opts.shell {
                            if ui.add(egui::Button::new(RichText::new(tr("Switch now")).size(15.0)).min_size(vec2(130.0, 34.0))).clicked() {
                                switch_now();
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            if ui.add(egui::Button::new(tr("At next sign-in")).min_size(vec2(130.0, 34.0))).clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        } else {
                            if ui.add(egui::Button::new(RichText::new(tr("Open Delfin")).size(15.0)).min_size(vec2(130.0, 34.0))).clicked() {
                                let _ = Command::new(installed_exe()).spawn();
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            if ui.add(egui::Button::new(tr("Close")).min_size(vec2(90.0, 34.0))).clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }
                    });
                });
            }
        }
    }
}
