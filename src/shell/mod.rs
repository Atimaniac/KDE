//! Delfin desktop shell (`delfin.exe --shell`): replaces explorer.exe.
//!
//! * Panel at the bottom (like KDE Plasma): launcher button, pinned apps,
//!   task manager with real window icons, clock, show-desktop.
//! * Application launcher (Kickoff-like) with search; Enter also runs commands.
//! * Desktop with your wallpaper and Desktop-folder icons.
//! * Starts your startup apps, owns Win+E / Win+D / Win+R / Alt+F1.
//! * File-manager windows are separate `delfin.exe` processes.

pub mod install;
mod startmenu;
mod startup;
mod win;

use crate::config::Config;
use crate::fsops::{self, natural_cmp};
use crate::icons::{self, Tool};
use crate::theme::{self, Palette};
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, FontId, Frame, Key, Layout, Margin, Rect, RichText,
    ScrollArea, Sense, Shape, Stroke, TextureHandle, Ui, ViewportBuilder, ViewportCommand,
    ViewportId, WindowLevel,
};
use startmenu::{AppEntry, Launch};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const PANEL_H: f32 = 44.0;
const LAUNCHER_W: f32 = 480.0;
const LAUNCHER_H: f32 = 580.0;
const PANEL_TITLE: &str = "Delfin Panel";
const DESKTOP_TITLE: &str = "Delfin Desktop";

pub fn run() -> Result<(), eframe::Error> {
    if !win::single_instance("Local\\DelfinShell") {
        return Ok(()); // already running in this session
    }
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title(PANEL_TITLE)
            .with_app_id("delfin-panel")
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size([1024.0, PANEL_H])
            .with_position([0.0, 0.0]),
        ..Default::default()
    };
    eframe::run_native(
        PANEL_TITLE,
        options,
        Box::new(|cc| Ok(Box::new(ShellApp::new(cc)))),
    )
}

#[derive(Clone)]
struct DesktopItem {
    name: String,
    path: PathBuf,
    is_dir: bool,
    ext: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Power {
    Logout,
    Restart,
    Shutdown,
    BackToExplorer,
}

enum PanelAction {
    ToggleLauncher,
    Activate(isize),
    Minimize(isize),
    Close(isize),
    FileManager,
    Terminal,
    ShowDesktop,
    Uri(&'static str),
}

enum DeskAction {
    Open(PathBuf, bool),
    ShowInDelfin(PathBuf),
    Terminal(PathBuf),
    Trash(PathBuf),
    Refresh,
    Uri(&'static str),
}

struct ShellApp {
    cfg: Config,
    own_pid: u32,
    // panel
    placed: Option<(i32, i32, i32)>,
    tasks: Vec<win::TaskWindow>,
    icon_of: HashMap<isize, isize>,
    last_poll: Instant,
    last_fg: isize,
    hidden_for_desktop: Vec<isize>,
    // launcher
    apps: Vec<AppEntry>,
    launcher_open: bool,
    launcher_query: String,
    launcher_focus_pending: bool,
    launcher_seen_focus: bool,
    launcher_closed_at: Instant,
    power_armed: Option<(Power, Instant)>,
    // desktop
    desktop: Vec<DesktopItem>,
    desktop_loaded: Instant,
    desktop_sel: Option<PathBuf>,
    wallpaper: Option<TextureHandle>,
    wallpaper_path: Option<PathBuf>,
    wallpaper_checked: Option<Instant>,
    shell_window_set: bool,
    // shared
    textures: HashMap<String, Option<TextureHandle>>,
    hotkeys: Receiver<win::Hotkey>,
}

impl ShellApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let cfg = Config::load();
        theme::apply(&cc.egui_ctx, cfg.dark);

        let (tx, rx) = channel();
        win::spawn_hotkeys(tx, cc.egui_ctx.clone());

        // Only when we really are the shell, and only once per sign-in.
        if install::shell_installed() && win::first_shell_start_this_session() {
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_secs(2));
                startup::run_startup_items();
            });
        }

        let long_ago = Instant::now() - Duration::from_secs(60);
        let mut app = Self {
            cfg,
            own_pid: std::process::id(),
            placed: None,
            tasks: Vec::new(),
            icon_of: HashMap::new(),
            last_poll: long_ago,
            last_fg: 0,
            hidden_for_desktop: Vec::new(),
            apps: startmenu::scan(),
            launcher_open: false,
            launcher_query: String::new(),
            launcher_focus_pending: false,
            launcher_seen_focus: false,
            launcher_closed_at: long_ago,
            power_armed: None,
            desktop: Vec::new(),
            desktop_loaded: long_ago,
            desktop_sel: None,
            wallpaper: None,
            wallpaper_path: None,
            wallpaper_checked: None,
            shell_window_set: false,
            textures: HashMap::new(),
            hotkeys: rx,
        };
        app.load_desktop();
        app
    }

    // ------------------------------------------------------------ helpers

    fn tex_file(&mut self, ctx: &egui::Context, path: &Path, large: bool) -> Option<TextureHandle> {
        let key = format!("{}{}", if large { "L:" } else { "S:" }, path.display());
        if let Some(t) = self.textures.get(&key) {
            return t.clone();
        }
        let t = win::file_icon_rgba(path, large).map(|(w, h, px)| {
            ctx.load_texture(
                key.clone(),
                egui::ColorImage::from_rgba_unmultiplied([w, h], &px),
                egui::TextureOptions::LINEAR,
            )
        });
        self.textures.insert(key, t.clone());
        t
    }

    fn tex_window(&mut self, ctx: &egui::Context, hwnd: isize) -> Option<TextureHandle> {
        let hicon = *self
            .icon_of
            .entry(hwnd)
            .or_insert_with(|| win::window_icon(hwnd));
        let key = format!("H:{hicon}");
        if let Some(t) = self.textures.get(&key) {
            return t.clone();
        }
        let t = win::icon_rgba(hicon).map(|(w, h, px)| {
            ctx.load_texture(
                key.clone(),
                egui::ColorImage::from_rgba_unmultiplied([w, h], &px),
                egui::TextureOptions::LINEAR,
            )
        });
        self.textures.insert(key, t.clone());
        t
    }

    fn launch(&mut self, l: &Launch) {
        match l {
            Launch::Cmd(c) if c == "delfin" => win::spawn_self(None),
            Launch::Cmd(c) => {
                let _ = win::run_cmdline(c);
            }
            Launch::Path(p) => {
                let _ = open::that(p);
            }
        }
    }

    fn open_launcher(&mut self, open: bool) {
        if open == self.launcher_open {
            return;
        }
        if open {
            // Clicking the button while the launcher is open first steals its
            // focus (which closes it); don't immediately reopen.
            if self.launcher_closed_at.elapsed() < Duration::from_millis(300) {
                return;
            }
            self.launcher_query.clear();
            self.launcher_focus_pending = true;
            self.launcher_seen_focus = false;
            self.power_armed = None;
        } else {
            self.launcher_closed_at = Instant::now();
        }
        self.launcher_open = open;
    }

    fn show_desktop(&mut self) {
        if self.hidden_for_desktop.is_empty() {
            for t in &self.tasks {
                if !t.minimized {
                    win::minimize(t.hwnd);
                    self.hidden_for_desktop.push(t.hwnd);
                }
            }
        } else {
            for h in self.hidden_for_desktop.drain(..) {
                win::restore(h);
            }
        }
    }

    fn power(&mut self, p: Power) {
        match p {
            Power::Logout => win::run_hidden("shutdown /l"),
            Power::Restart => win::run_hidden("shutdown /r /t 0"),
            Power::Shutdown => win::run_hidden("shutdown /s /t 0"),
            Power::BackToExplorer => {
                let _ = install::uninstall_shell();
                // Start Explorer after we've exited so it becomes the shell.
                win::run_hidden("ping -n 3 127.0.0.1 >nul & start \"\" explorer.exe");
                std::process::exit(0);
            }
        }
    }

    fn poll(&mut self) {
        let fg = win::foreground();
        if fg != 0 && win::pid_of(fg) != self.own_pid {
            self.last_fg = fg;
        }
        if self.last_poll.elapsed() < Duration::from_millis(400) {
            return;
        }
        self.last_poll = Instant::now();
        let fresh = win::task_windows(self.own_pid);
        // Keep the existing order; new windows go to the end.
        let mut merged: Vec<win::TaskWindow> = Vec::new();
        for old in &self.tasks {
            if let Some(f) = fresh.iter().find(|f| f.hwnd == old.hwnd) {
                merged.push(f.clone());
            }
        }
        for f in fresh {
            if !merged.iter().any(|m| m.hwnd == f.hwnd) {
                merged.push(f);
            }
        }
        self.icon_of
            .retain(|h, _| merged.iter().any(|t| t.hwnd == *h));
        self.hidden_for_desktop
            .retain(|h| merged.iter().any(|t| t.hwnd == *h));
        self.tasks = merged;
    }

    /// Keep the panel glued to the bottom of the primary screen.
    fn place_panel(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        let (w_px, h_px) = win::screen_px();
        let panel_px = (PANEL_H * ppp).round() as i32;
        let key = (w_px, h_px, panel_px);
        if self.placed == Some(key) {
            return;
        }
        self.placed = Some(key);
        let w = w_px as f32 / ppp;
        let h = h_px as f32 / ppp;
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(0.0, h - PANEL_H)));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(w, PANEL_H)));
        win::set_work_area(panel_px);
    }

    fn screen_points(ctx: &egui::Context) -> egui::Vec2 {
        let ppp = ctx.pixels_per_point();
        let (w, h) = win::screen_px();
        vec2(w as f32 / ppp, h as f32 / ppp)
    }

    // ------------------------------------------------------------ panel

    fn panel_ui(&mut self, ctx: &egui::Context, pal: &Palette) {
        let mut actions: Vec<PanelAction> = Vec::new();
        egui::CentralPanel::default()
            .frame(
                Frame::none()
                    .fill(pal.window)
                    .inner_margin(Margin::symmetric(6.0, 4.0)),
            )
            .show(ctx, |ui| {
                let sr = ctx.screen_rect();
                ui.painter().line_segment(
                    [sr.left_top(), sr.right_top()],
                    Stroke::new(1.0, pal.border),
                );

                ui.horizontal_centered(|ui| {
                    if launcher_button(ui, self.launcher_open, pal).clicked() {
                        actions.push(PanelAction::ToggleLauncher);
                    }
                    ui.add_space(4.0);
                    if pinned_folder(ui, pal).clicked() {
                        actions.push(PanelAction::FileManager);
                    }
                    if icons::tool_button_sized(
                        ui,
                        Tool::Terminal,
                        false,
                        true,
                        "",
                        vec2(36.0, 36.0),
                    )
                    .clicked()
                    {
                        actions.push(PanelAction::Terminal);
                    }
                    ui.separator();

                    let right_w = 200.0;
                    let avail = (ui.available_width() - right_w).max(40.0);
                    let n = self.tasks.len().max(1) as f32;
                    let bw = ((avail - 4.0 * n) / n).clamp(40.0, 220.0);
                    let tasks = self.tasks.clone();
                    for t in &tasks {
                        let active = t.hwnd == self.last_fg && !t.minimized;
                        let tex = self.tex_window(ctx, t.hwnd);
                        let r =
                            task_button(ui, &t.title, active, t.minimized, bw, tex.as_ref(), pal);
                        if r.clicked() {
                            actions.push(if active {
                                PanelAction::Minimize(t.hwnd)
                            } else {
                                PanelAction::Activate(t.hwnd)
                            });
                        }
                        if r.middle_clicked() {
                            actions.push(PanelAction::Close(t.hwnd));
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // Show desktop: thin strip at the very right (like Windows/Plasma).
                        let (r, resp) = ui.allocate_exact_size(vec2(10.0, 36.0), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(r, 2.0, pal.hover);
                        }
                        ui.painter().line_segment(
                            [r.left_top(), r.left_bottom()],
                            Stroke::new(1.0, pal.border),
                        );
                        if resp.clicked() {
                            actions.push(PanelAction::ShowDesktop);
                        }

                        if clock(ui, pal).clicked() {
                            actions.push(PanelAction::Uri("ms-settings:dateandtime"));
                        }
                        if icons::tool_button_sized(
                            ui,
                            Tool::Speaker,
                            false,
                            true,
                            "",
                            vec2(30.0, 36.0),
                        )
                        .clicked()
                        {
                            actions.push(PanelAction::Uri("ms-settings:sound"));
                        }
                        if icons::tool_button_sized(
                            ui,
                            Tool::Network,
                            false,
                            true,
                            "",
                            vec2(30.0, 36.0),
                        )
                        .clicked()
                        {
                            actions.push(PanelAction::Uri("ms-settings:network-status"));
                        }
                    });
                });
            });

        for a in actions {
            match a {
                PanelAction::ToggleLauncher => {
                    let open = !self.launcher_open;
                    self.open_launcher(open);
                }
                PanelAction::Activate(h) => {
                    win::activate(h);
                    self.last_fg = h;
                    self.hidden_for_desktop.clear();
                }
                PanelAction::Minimize(h) => {
                    win::minimize(h);
                    self.last_fg = 0;
                }
                PanelAction::Close(h) => win::close(h),
                PanelAction::FileManager => win::spawn_self(None),
                PanelAction::Terminal => {
                    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("C:\\"));
                    let _ = crate::platform::open_terminal(&home);
                }
                PanelAction::ShowDesktop => self.show_desktop(),
                PanelAction::Uri(u) => {
                    let _ = open::that(u);
                }
            }
        }
    }

    // ------------------------------------------------------------ launcher

    fn launcher_viewport(&mut self, ctx: &egui::Context, pal: &Palette) {
        let screen = Self::screen_points(ctx);
        let builder = ViewportBuilder::default()
            .with_title("Delfin Launcher")
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size([LAUNCHER_W, LAUNCHER_H])
            .with_position([4.0, screen.y - PANEL_H - LAUNCHER_H - 4.0]);

        let mut close = false;
        let mut launch: Option<Launch> = None;
        let mut power: Option<Power> = None;

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("delfin_launcher"),
            builder,
            |ctx, _| {
                if self.launcher_focus_pending {
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                let focused = ctx.input(|i| i.viewport().focused);
                if focused == Some(true) {
                    self.launcher_seen_focus = true;
                }
                if self.launcher_seen_focus && focused == Some(false) {
                    close = true;
                }
                if ctx.input(|i| i.key_pressed(Key::Escape) || i.viewport().close_requested()) {
                    close = true;
                }

                egui::CentralPanel::default()
                    .frame(
                        Frame::none()
                            .fill(pal.window)
                            .stroke(Stroke::new(1.0, pal.border))
                            .inner_margin(Margin::same(12.0)),
                    )
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 16.0, pal.accent);
                            let user = std::env::var("USERNAME").unwrap_or_else(|_| "User".into());
                            let initial: String =
                                user.chars().take(1).collect::<String>().to_uppercase();
                            ui.painter().text(
                                r.center(),
                                Align2::CENTER_CENTER,
                                initial,
                                FontId::proportional(16.0),
                                Color32::WHITE,
                            );
                            ui.label(RichText::new(user).strong().size(15.0));
                        });
                        ui.add_space(8.0);

                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.launcher_query)
                                .hint_text("Search apps, or type a command…")
                                .desired_width(f32::INFINITY),
                        );
                        if self.launcher_focus_pending {
                            r.request_focus();
                            self.launcher_focus_pending = false;
                        }
                        let q = self.launcher_query.trim().to_lowercase();
                        let matches: Vec<usize> = (0..self.apps.len())
                            .filter(|&i| {
                                q.is_empty()
                                    || self.apps[i].name.to_lowercase().contains(&q)
                                    || self.apps[i].folder.to_lowercase().contains(&q)
                            })
                            .collect();
                        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            if let Some(&i) = matches.first() {
                                launch = Some(self.apps[i].target.clone());
                            } else if !q.is_empty() {
                                launch = Some(Launch::Cmd(self.launcher_query.trim().to_string()));
                            }
                        }
                        ui.add_space(6.0);

                        let list_h = ui.available_height() - 52.0;
                        let row_h = 34.0;
                        ScrollArea::vertical()
                            .max_height(list_h)
                            .auto_shrink([false, false])
                            .show_rows(ui, row_h, matches.len(), |ui, range| {
                                for &i in &matches[range] {
                                    let app = self.apps[i].clone();
                                    let w = ui.available_width();
                                    let (rect, resp) =
                                        ui.allocate_exact_size(vec2(w, row_h), Sense::click());
                                    if resp.hovered() {
                                        ui.painter().rect_filled(rect, 4.0, pal.hover);
                                    }
                                    let ir = Rect::from_center_size(
                                        pos2(rect.left() + 18.0, rect.center().y),
                                        vec2(24.0, 24.0),
                                    );
                                    match (&app.icon, &app.target) {
                                        (Some(t), _) => {
                                            icons::paint_tool(ui.painter(), ir, *t, pal.text)
                                        }
                                        (None, Launch::Path(p)) => {
                                            match self.tex_file(ctx, p, false) {
                                                Some(tex) => {
                                                    ui.painter().image(
                                                        tex.id(),
                                                        ir,
                                                        uv_full(),
                                                        Color32::WHITE,
                                                    );
                                                }
                                                None => icons::paint_file(
                                                    ui.painter(),
                                                    ir,
                                                    icons::kind_color(icons::Kind::Exe),
                                                    "",
                                                    pal,
                                                ),
                                            }
                                        }
                                        _ => {}
                                    }
                                    let tx = rect.left() + 40.0;
                                    let p = ui.painter().with_clip_rect(rect);
                                    if app.folder.is_empty() {
                                        p.text(
                                            pos2(tx, rect.center().y),
                                            Align2::LEFT_CENTER,
                                            &app.name,
                                            FontId::proportional(13.5),
                                            pal.text,
                                        );
                                    } else {
                                        p.text(
                                            pos2(tx, rect.center().y - 7.0),
                                            Align2::LEFT_CENTER,
                                            &app.name,
                                            FontId::proportional(13.5),
                                            pal.text,
                                        );
                                        p.text(
                                            pos2(tx, rect.center().y + 8.0),
                                            Align2::LEFT_CENTER,
                                            &app.folder,
                                            FontId::proportional(11.0),
                                            pal.dim,
                                        );
                                    }
                                    if resp.clicked() {
                                        launch = Some(app.target.clone());
                                    }
                                }
                                if matches.is_empty() && !q.is_empty() {
                                    ui.label(
                                        RichText::new(format!(
                                            "Press Enter to run \u{201c}{}\u{201d}",
                                            self.launcher_query.trim()
                                        ))
                                        .color(pal.dim),
                                    );
                                }
                            });

                        ui.separator();
                        ui.horizontal(|ui| {
                            let armed = self
                                .power_armed
                                .filter(|(_, t)| t.elapsed() < Duration::from_secs(5))
                                .map(|(p, _)| p);
                            let btn = |ui: &mut Ui, tool: Tool, label: &str, p: Option<Power>| {
                                let text = if p.is_some() && armed == p {
                                    "Click again"
                                } else {
                                    label
                                };
                                let r = ui.add(egui::Button::new(text).min_size(vec2(0.0, 30.0)));
                                let icon_r = Rect::from_center_size(
                                    pos2(r.rect.left() - 12.0, r.rect.center().y),
                                    vec2(18.0, 18.0),
                                );
                                icons::paint_tool(ui.painter(), icon_r, tool, pal.dim);
                                r.clicked()
                            };
                            ui.add_space(20.0);
                            if btn(ui, Tool::Lock, "Lock", None) {
                                win::run_hidden("rundll32.exe user32.dll,LockWorkStation");
                                close = true;
                            }
                            ui.add_space(22.0);
                            for (tool, label, p) in [
                                (Tool::Logout, "Log Out", Power::Logout),
                                (Tool::Refresh, "Restart", Power::Restart),
                                (Tool::Power, "Shut Down", Power::Shutdown),
                            ] {
                                if btn(ui, tool, label, Some(p)) {
                                    power = Some(p);
                                }
                                ui.add_space(22.0);
                            }
                        });
                        ui.horizontal(|ui| {
                            if ui.small_button("Settings").clicked() {
                                launch = Some(Launch::Cmd("ms-settings:".into()));
                            }
                            if ui.small_button("Task Manager").clicked() {
                                launch = Some(Launch::Cmd("taskmgr.exe".into()));
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let armed = self.power_armed.is_some_and(|(p, t)| {
                                    p == Power::BackToExplorer
                                        && t.elapsed() < Duration::from_secs(5)
                                });
                                let label = if armed {
                                    "Click again to switch"
                                } else {
                                    "Back to Explorer"
                                };
                                if ui
                                    .small_button(RichText::new(label).color(pal.dim))
                                    .clicked()
                                {
                                    power = Some(Power::BackToExplorer);
                                }
                            });
                        });
                    });
            },
        );

        if let Some(l) = launch {
            self.launch(&l);
            close = true;
        }
        if let Some(p) = power {
            // Destructive actions need a second click within 5 s.
            let confirmed = self
                .power_armed
                .is_some_and(|(q, t)| q == p && t.elapsed() < Duration::from_secs(5));
            if confirmed {
                self.power(p);
            } else {
                self.power_armed = Some((p, Instant::now()));
            }
        }
        if close {
            self.open_launcher(false);
        }
    }

    // ------------------------------------------------------------ desktop

    fn load_desktop(&mut self) {
        let mut dirs_: Vec<PathBuf> = Vec::new();
        if let Some(d) = dirs::desktop_dir() {
            dirs_.push(d);
        }
        if let Ok(p) = std::env::var("PUBLIC") {
            dirs_.push(PathBuf::from(p).join("Desktop"));
        }
        let mut items = Vec::new();
        for d in dirs_ {
            let Ok(list) = fsops::list_dir(&d) else {
                continue;
            };
            for e in list {
                if e.hidden {
                    continue;
                }
                let name = if matches!(e.ext.as_str(), "lnk" | "url") {
                    e.path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or(e.name)
                } else {
                    e.name
                };
                items.push(DesktopItem {
                    name,
                    path: e.path,
                    is_dir: e.is_dir,
                    ext: e.ext,
                });
            }
        }
        items.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| natural_cmp(&a.name, &b.name))
        });
        self.desktop = items;
        self.desktop_loaded = Instant::now();
    }

    fn refresh_wallpaper(&mut self, ctx: &egui::Context) {
        if self
            .wallpaper_checked
            .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
        {
            return;
        }
        self.wallpaper_checked = Some(Instant::now());
        let path = win::wallpaper_path();
        if path == self.wallpaper_path && (self.wallpaper.is_some() || path.is_none()) {
            return;
        }
        self.wallpaper_path = path.clone();
        self.wallpaper = path.and_then(|p| {
            let bytes = std::fs::read(&p).ok()?;
            let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
            let img = if img.width() > 4096 || img.height() > 4096 {
                image::imageops::resize(
                    &img,
                    img.width() / 2,
                    img.height() / 2,
                    image::imageops::FilterType::Triangle,
                )
            } else {
                img
            };
            let size = [img.width() as usize, img.height() as usize];
            Some(ctx.load_texture(
                "wallpaper",
                egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()),
                egui::TextureOptions::LINEAR,
            ))
        });
    }

    fn desktop_viewport(&mut self, ctx: &egui::Context, pal: &Palette) {
        let screen = Self::screen_points(ctx);
        let builder = ViewportBuilder::default()
            .with_title(DESKTOP_TITLE)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnBottom)
            .with_position([0.0, 0.0])
            .with_inner_size(screen);

        let mut actions: Vec<DeskAction> = Vec::new();
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("delfin_desktop"),
            builder,
            |ctx, _| {
                self.refresh_wallpaper(ctx);
                if self.desktop_loaded.elapsed() > Duration::from_secs(3) {
                    self.load_desktop();
                }
                egui::CentralPanel::default()
                    .frame(Frame::none())
                    .show(ctx, |ui| {
                        let rect = ui.max_rect();
                        // Wallpaper ("fill" mode) or a Breeze-ish gradient.
                        if let Some(tex) = &self.wallpaper {
                            let ts = tex.size_vec2();
                            let scale = (rect.width() / ts.x).max(rect.height() / ts.y);
                            let uv = Rect::from_center_size(
                                pos2(0.5, 0.5),
                                vec2(
                                    rect.width() / (ts.x * scale),
                                    rect.height() / (ts.y * scale),
                                ),
                            );
                            ui.painter().image(tex.id(), rect, uv, Color32::WHITE);
                        } else {
                            let mut mesh = egui::Mesh::default();
                            let top = Color32::from_rgb(0x1d, 0x2b, 0x3a);
                            let bottom = Color32::from_rgb(0x0e, 0x4d, 0x6e);
                            mesh.colored_vertex(rect.left_top(), top);
                            mesh.colored_vertex(rect.right_top(), top);
                            mesh.colored_vertex(rect.right_bottom(), bottom);
                            mesh.colored_vertex(rect.left_bottom(), bottom);
                            mesh.add_triangle(0, 1, 2);
                            mesh.add_triangle(0, 2, 3);
                            ui.painter().add(Shape::mesh(mesh));
                        }

                        let bg = ui.interact(rect, egui::Id::new("desktop_bg"), Sense::click());
                        if bg.clicked() {
                            self.desktop_sel = None;
                        }
                        bg.context_menu(|ui| {
                            ui.set_min_width(220.0);
                            if let Some(d) = dirs::desktop_dir() {
                                if ui.button("Open Desktop Folder").clicked() {
                                    actions.push(DeskAction::Open(d.clone(), true));
                                    ui.close_menu();
                                }
                                if ui.button("Open Terminal Here").clicked() {
                                    actions.push(DeskAction::Terminal(d));
                                    ui.close_menu();
                                }
                            }
                            if ui.button("Refresh").clicked() {
                                actions.push(DeskAction::Refresh);
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("Change Wallpaper…").clicked() {
                                actions.push(DeskAction::Uri(
                                    "ms-settings:personalization-background",
                                ));
                                ui.close_menu();
                            }
                            if ui.button("Display Settings…").clicked() {
                                actions.push(DeskAction::Uri("ms-settings:display"));
                                ui.close_menu();
                            }
                        });

                        // Icons, column by column from the top-left.
                        let cell = vec2(96.0, 100.0);
                        let origin = rect.min + vec2(10.0, 10.0);
                        let rows = (((rect.height() - 20.0) / cell.y).floor() as usize).max(1);
                        let items = self.desktop.clone();
                        for (i, it) in items.iter().enumerate() {
                            let (col, row) = (i / rows, i % rows);
                            let r = Rect::from_min_size(
                                origin + vec2(col as f32 * cell.x, row as f32 * cell.y),
                                cell,
                            );
                            let resp =
                                ui.interact(r, egui::Id::new(("desk", &it.path)), Sense::click());
                            let selected = self.desktop_sel.as_ref() == Some(&it.path);
                            if selected {
                                ui.painter().rect_filled(r, 6.0, pal.select);
                                ui.painter()
                                    .rect_stroke(r, 6.0, Stroke::new(1.0, pal.accent));
                            } else if resp.hovered() {
                                ui.painter()
                                    .rect_filled(r, 6.0, Color32::from_white_alpha(30));
                            }
                            let ir = Rect::from_center_size(
                                pos2(r.center().x, r.top() + 30.0),
                                vec2(48.0, 48.0),
                            );
                            if it.is_dir {
                                icons::paint_folder(ui.painter(), ir, icons::FOLDER);
                            } else if let Some(tex) = self.tex_file(ctx, &it.path, true) {
                                ui.painter().image(tex.id(), ir, uv_full(), Color32::WHITE);
                            } else {
                                icons::paint_entry_icon(ui.painter(), ir, false, &it.ext, pal);
                            }
                            let label_pos = pos2(r.center().x, ir.bottom() + 6.0);
                            let shadow =
                                label(ui, &it.name, cell.x - 8.0, Color32::from_black_alpha(200));
                            ui.painter()
                                .galley(label_pos + vec2(1.0, 1.0), shadow, Color32::BLACK);
                            let text = label(ui, &it.name, cell.x - 8.0, Color32::WHITE);
                            ui.painter().galley(label_pos, text, Color32::WHITE);

                            if resp.clicked() {
                                self.desktop_sel = Some(it.path.clone());
                            }
                            if resp.double_clicked() {
                                actions.push(DeskAction::Open(it.path.clone(), it.is_dir));
                            }
                            if resp.secondary_clicked() {
                                self.desktop_sel = Some(it.path.clone());
                            }
                            resp.context_menu(|ui| {
                                ui.set_min_width(200.0);
                                if ui.button("Open").clicked() {
                                    actions.push(DeskAction::Open(it.path.clone(), it.is_dir));
                                    ui.close_menu();
                                }
                                if !it.is_dir && ui.button("Show in Delfin").clicked() {
                                    actions.push(DeskAction::ShowInDelfin(it.path.clone()));
                                    ui.close_menu();
                                }
                                if it.is_dir && ui.button("Open Terminal Here").clicked() {
                                    actions.push(DeskAction::Terminal(it.path.clone()));
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui.button("Move to Trash").clicked() {
                                    actions.push(DeskAction::Trash(it.path.clone()));
                                    ui.close_menu();
                                }
                            });
                        }
                    });
            },
        );

        // Register the desktop as the shell window once it exists.
        if !self.shell_window_set {
            if let Some(h) = win::find_window(DESKTOP_TITLE) {
                win::set_shell_window(h);
                self.shell_window_set = true;
            }
        }

        for a in actions {
            match a {
                DeskAction::Open(p, true) => win::spawn_self(Some(&p)),
                DeskAction::Open(p, false) => {
                    let _ = open::that(&p);
                }
                DeskAction::ShowInDelfin(p) => {
                    if let Some(parent) = p.parent() {
                        win::spawn_self(Some(parent));
                    }
                }
                DeskAction::Terminal(p) => {
                    let _ = crate::platform::open_terminal(&p);
                }
                DeskAction::Trash(p) => {
                    let _ = trash::delete(&p);
                    self.load_desktop();
                }
                DeskAction::Refresh => {
                    self.load_desktop();
                    self.wallpaper_checked = None;
                    self.wallpaper_path = None;
                }
                DeskAction::Uri(u) => {
                    let _ = open::that(u);
                }
            }
        }
    }
}

impl eframe::App for ShellApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let pal = theme::palette(self.cfg.dark);
        self.place_panel(ctx);
        self.poll();

        while let Ok(k) = self.hotkeys.try_recv() {
            match k {
                win::Hotkey::FileManager => win::spawn_self(None),
                win::Hotkey::Launcher => {
                    let open = !self.launcher_open;
                    self.open_launcher(open);
                }
                win::Hotkey::ShowDesktop => self.show_desktop(),
            }
        }

        self.panel_ui(ctx, &pal);
        self.desktop_viewport(ctx, &pal);
        if self.launcher_open {
            self.launcher_viewport(ctx, &pal);
        }
        // Clock + task list refresh.
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

// ----------------------------------------------------------------- widgets

fn uv_full() -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
}

fn label(ui: &Ui, text: &str, width: f32, color: Color32) -> std::sync::Arc<egui::Galley> {
    let mut job =
        egui::text::LayoutJob::simple(text.to_owned(), FontId::proportional(13.0), color, width);
    job.wrap.max_rows = 2;
    job.wrap.overflow_character = Some('…');
    job.halign = Align::Center;
    ui.fonts(|f| f.layout_job(job))
}

fn launcher_button(ui: &mut Ui, open: bool, pal: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 36.0), Sense::click());
    let p = ui.painter();
    if open {
        p.rect_filled(rect, 4.0, pal.select);
    } else if resp.hovered() {
        p.rect_filled(rect, 4.0, pal.hover);
    }
    // Original mark: accent disc with three orbiting dots.
    let c = rect.center();
    p.circle_filled(c, 12.0, pal.accent);
    for i in 0..3 {
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / 3.0;
        p.circle_filled(c + vec2(a.cos(), a.sin()) * 5.5, 2.4, Color32::WHITE);
    }
    p.circle_stroke(c, 5.5, Stroke::new(1.2, Color32::WHITE));
    resp
}

fn pinned_folder(ui: &mut Ui, pal: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, pal.hover);
    }
    icons::paint_folder(ui.painter(), rect.shrink(8.0), icons::FOLDER);
    resp
}

fn task_button(
    ui: &mut Ui,
    title: &str,
    active: bool,
    minimized: bool,
    w: f32,
    tex: Option<&TextureHandle>,
    pal: &Palette,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 36.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if active {
            p.rect_filled(rect, 4.0, pal.select);
        } else if resp.hovered() {
            p.rect_filled(rect, 4.0, pal.hover);
        }
        let line = if active {
            pal.accent
        } else if minimized {
            pal.border
        } else {
            pal.dim
        };
        p.line_segment(
            [
                pos2(rect.left() + 6.0, rect.bottom() - 1.0),
                pos2(rect.right() - 6.0, rect.bottom() - 1.0),
            ],
            Stroke::new(2.0, line),
        );
        let ir =
            Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), vec2(22.0, 22.0));
        match tex {
            Some(t) => {
                p.image(t.id(), ir, uv_full(), Color32::WHITE);
            }
            None => icons::paint_tool(p, ir, Tool::Desktop, pal.dim),
        }
        if w > 60.0 {
            let clip = Rect::from_min_max(
                pos2(ir.right() + 6.0, rect.top()),
                pos2(rect.right() - 6.0, rect.bottom()),
            );
            p.with_clip_rect(clip).text(
                pos2(clip.left(), rect.center().y),
                Align2::LEFT_CENTER,
                title,
                FontId::proportional(13.0),
                if minimized { pal.dim } else { pal.text },
            );
        }
    }
    resp
}

fn clock(ui: &mut Ui, pal: &Palette) -> egui::Response {
    let now = chrono::Local::now();
    let (rect, resp) = ui.allocate_exact_size(vec2(78.0, 36.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 4.0, pal.hover);
    }
    p.text(
        pos2(rect.center().x, rect.center().y - 7.0),
        Align2::CENTER_CENTER,
        now.format("%H:%M").to_string(),
        FontId::proportional(14.0),
        pal.text,
    );
    p.text(
        pos2(rect.center().x, rect.center().y + 9.0),
        Align2::CENTER_CENTER,
        now.format("%d.%m.%Y").to_string(),
        FontId::proportional(11.0),
        pal.dim,
    );
    resp
}
