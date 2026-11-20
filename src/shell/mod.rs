//! Delfin desktop shell (`delfin.exe --shell`): replaces explorer.exe.
//!
//! * Panel at the bottom (like KDE Plasma): launcher button, pinned apps,
//!   task manager with real window icons, clock, show-desktop.
//! * Application launcher (Kickoff-like) with search; Enter also runs commands.
//! * Desktop with your wallpaper and Desktop-folder icons.
//! * Starts your startup apps, owns Win+E / Win+D / Win+R / Alt+F1.
//! * File-manager windows are separate `delfin.exe` processes.

pub mod install;
mod quick;
mod startmenu;
mod startup;
mod tray;
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
use startmenu::{AppEntry, Category, Launch};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use chrono::Datelike;
use std::time::{Duration, Instant};
use tray::{Click, TrayIcon};

const PANEL_H: f32 = 44.0;
const LAUNCHER_W: f32 = 780.0;
const LAUNCHER_H: f32 = 540.0;
const PANEL_TITLE: &str = "Delfin Panel";
const DESKTOP_TITLE: &str = "Delfin Desktop";
/// Tray icons shown directly on the panel; the rest go behind the ^ button.
const TRAY_INLINE: usize = 8;
const BALLOON_SECS: u64 = 7;
const QUICK_W: f32 = 380.0;


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
    ToggleCalendar,
    Uri(&'static str),
    Tray(isize, u32, Click, f32),
    Unshelve(isize),
    ToggleOverflow,
    ToggleQuick(quick::Section),
    Volume(f32),
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
    /// Height of Explorer's taskbar in points when it is still running (0 otherwise).
    lift: f32,
    tasks: Vec<win::TaskWindow>,
    icon_of: HashMap<isize, isize>,
    last_poll: Instant,
    last_fg: isize,
    /// A full-screen program is in front: the panel stays below it.
    fullscreen: bool,
    hidden_for_desktop: Vec<isize>,
    // launcher
    apps: Vec<AppEntry>,
    launcher_open: bool,
    launcher_query: String,
    /// Category picked on the left; `None` = empty list until you type.
    launcher_cat: Option<Category>,
    launcher_sel: usize,
    /// Recently launched app names, newest first.
    recent: Vec<String>,
    descriptions: Receiver<Vec<(PathBuf, String)>>,
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
    // tray
    tray: Option<tray::Shared>,
    tray_tex: HashMap<(isize, u32), (u64, TextureHandle)>,
    overflow_open: bool,
    overflow_seen_focus: bool,
    overflow_closed_at: Instant,
    last_hover: Option<(isize, u32)>,
    tip: Option<(String, f32, Instant)>,
    balloon_since: Option<Instant>,
    /// Windows hidden by "minimize to tray".
    shelved: Vec<isize>,
    // quick settings
    qp: quick::QuickPanel,
    quick_open: Option<quick::Section>,
    quick_last: Option<quick::Section>,
    quick_seen_focus: bool,
    quick_closed_at: Instant,
    vol_state: Option<(f32, bool)>,
    wifi_sig: Option<u32>,
    last_status_poll: Instant,
    store_apps: Receiver<Vec<AppEntry>>,
    // calendar under the clock
    calendar_open: bool,
    calendar_seen_focus: bool,
    calendar_closed_at: Instant,
    /// (year, month) shown in the calendar.
    calendar_month: (i32, u32),
    /// Last time we looked for a missing tray (Explorer closed after we started).
    tray_checked: Instant,
    // Delfin settings window (language, updates, shell options)
    settings_open: bool,
    updater: crate::update::Updater,
    // shared
    textures: HashMap<String, Option<TextureHandle>>,
    hotkeys: Receiver<win::Hotkey>,
}

/// Remember shelved windows on disk so they can be shown again if the shell
/// is restarted (a hidden window would otherwise stay invisible).
fn shelved_file() -> PathBuf {
    std::env::temp_dir().join("delfin-shelved.txt")
}

fn save_shelved(list: &[isize]) {
    let text: Vec<String> = list.iter().map(|h| h.to_string()).collect();
    let _ = std::fs::write(shelved_file(), text.join("\n"));
}

fn restore_leftover_shelved() {
    if let Ok(text) = std::fs::read_to_string(shelved_file()) {
        for h in text.lines().filter_map(|l| l.trim().parse::<isize>().ok()) {
            if win::is_window(h) {
                win::unhide(h);
            }
        }
    }
    let _ = std::fs::remove_file(shelved_file());
}

impl ShellApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let cfg = Config::load();
        theme::apply(&cc.egui_ctx, cfg.dark);

        let (tx, rx) = channel();
        win::spawn_hotkeys(tx, cc.egui_ctx.clone());
        restore_leftover_shelved();

        // Tray host must exist before startup apps run, otherwise they decide
        // there is no taskbar and don't create their icons. Skip it if
        // Explorer's tray is running (test mode with explorer.exe alive).
        let tray = if win::find_class("Shell_TrayWnd").is_none() {
            win::hide_minimized_windows();
            Some(tray::start(cc.egui_ctx.clone()))
        } else {
            None
        };

        // Only when we really are the shell, and only once per sign-in.
        if install::shell_installed() && win::first_shell_start_this_session() {
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_secs(2));
                startup::run_startup_items();
            });
        }

        let long_ago = Instant::now() - Duration::from_secs(60);
        let apps = startmenu::scan();
        let descriptions = startmenu::describe(
            apps.iter()
                .filter_map(|a| match &a.target {
                    Launch::Path(p) => Some(p.clone()),
                    Launch::Cmd(_) => None,
                })
                .collect(),
            cc.egui_ctx.clone(),
        );
        let mut app = Self {
            cfg,
            own_pid: std::process::id(),
            placed: None,
            lift: 0.0,
            tasks: Vec::new(),
            icon_of: HashMap::new(),
            last_poll: long_ago,
            last_fg: 0,
            fullscreen: false,
            hidden_for_desktop: Vec::new(),
            apps,
            launcher_open: false,
            launcher_query: String::new(),
            launcher_cat: None,
            launcher_sel: 0,
            recent: startmenu::load_recent(),
            descriptions,
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
            tray,
            tray_tex: HashMap::new(),
            overflow_open: false,
            overflow_seen_focus: false,
            overflow_closed_at: long_ago,
            last_hover: None,
            tip: None,
            balloon_since: None,
            shelved: Vec::new(),
            qp: quick::QuickPanel::new(&cc.egui_ctx),
            quick_open: None,
            quick_last: None,
            quick_seen_focus: false,
            quick_closed_at: long_ago,
            vol_state: None,
            wifi_sig: None,
            last_status_poll: long_ago,
            store_apps: startmenu::scan_store_apps(cc.egui_ctx.clone()),
            calendar_open: false,
            calendar_seen_focus: false,
            calendar_closed_at: long_ago,
            calendar_month: (2000, 1),
            tray_checked: long_ago,
            settings_open: false,
            updater: crate::update::Updater::default(),
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

    fn tray_icons(&self) -> Vec<TrayIcon> {
        match &self.tray {
            Some(t) => {
                let st = t.lock().unwrap_or_else(|p| p.into_inner());
                st.icons.iter().filter(|i| !i.hidden).cloned().collect()
            }
            None => Vec::new(),
        }
    }

    fn tray_texture(&mut self, ctx: &egui::Context, ic: &TrayIcon) -> Option<TextureHandle> {
        let (w, h, px) = ic.icon.as_ref()?;
        let key = (ic.hwnd, ic.uid);
        let image = || egui::ColorImage::from_rgba_unmultiplied([*w, *h], &px[..]);
        if let Some((gen, tex)) = self.tray_tex.get_mut(&key) {
            if *gen != ic.icon_gen {
                tex.set(image(), egui::TextureOptions::LINEAR);
                *gen = ic.icon_gen;
            }
            return Some(tex.clone());
        }
        let tex = ctx.load_texture(
            format!("tray:{}:{}", key.0, key.1),
            image(),
            egui::TextureOptions::LINEAR,
        );
        self.tray_tex.insert(key, (ic.icon_gen, tex.clone()));
        Some(tex)
    }

    /// Screen position (physical px) that programs expect as the click anchor.
    fn anchor_px(&self, ctx: &egui::Context, x_points: f32) -> (i32, i32) {
        let ppp = ctx.pixels_per_point();
        let (_, h_px) = win::screen_px();
        (
            (x_points * ppp) as i32,
            h_px - ((self.lift + PANEL_H) * ppp).round() as i32,
        )
    }

    /// Screen rectangle (pixels) of something drawn on the panel.
    fn panel_rect_px(&self, ctx: &egui::Context, r: Rect) -> [i32; 4] {
        let ppp = ctx.pixels_per_point();
        let (_, h_px) = win::screen_px();
        let top = h_px as f32 - (self.lift + PANEL_H) * ppp;
        [
            (r.left() * ppp) as i32,
            (top + r.top() * ppp) as i32,
            (r.right() * ppp) as i32,
            (top + r.bottom() * ppp) as i32,
        ]
    }

    fn send_tray_click(&mut self, ctx: &egui::Context, hwnd: isize, uid: u32, kind: Click, x: f32) {
        let icons = self.tray_icons();
        if let Some(ic) = icons.iter().find(|i| i.hwnd == hwnd && i.uid == uid) {
            tray::click(ic, kind, self.anchor_px(ctx, x));
        }
    }

    fn unshelve(&mut self, h: isize) {
        win::unhide(h);
        self.shelved.retain(|x| *x != h);
        save_shelved(&self.shelved);
    }

    fn unshelve_all(&mut self) {
        for h in std::mem::take(&mut self.shelved) {
            win::unhide(h);
        }
        save_shelved(&self.shelved);
    }

    /// One tray-sized icon button.
    fn tray_slot(
        ui: &mut Ui,
        tex: Option<&TextureHandle>,
        underline: bool,
        pal: &Palette,
    ) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 36.0), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect, 4.0, pal.hover);
        }
        let ir = Rect::from_center_size(rect.center(), vec2(18.0, 18.0));
        match tex {
            Some(t) => {
                ui.painter().image(t.id(), ir, uv_full(), Color32::WHITE);
            }
            None => {
                ui.painter()
                    .rect_stroke(ir.shrink(2.0), 3.0, Stroke::new(1.0_f32, pal.dim));
            }
        }
        if underline {
            ui.painter().line_segment(
                [
                    pos2(rect.left() + 8.0, rect.bottom() - 1.0),
                    pos2(rect.right() - 8.0, rect.bottom() - 1.0),
                ],
                Stroke::new(2.0_f32, pal.accent),
            );
        }
        resp
    }

    fn launch(&mut self, l: &Launch) {
        match l {
            Launch::Cmd(c) if c == "delfin" => win::spawn_self(None),
            Launch::Cmd(c) => {
                let _ = win::run_cmdline(c);
            }
            // Folders open in Delfin, not in Explorer.
            Launch::Path(p) if p.is_dir() => win::spawn_self(Some(p)),
            Launch::Path(p) => {
                win::shell_open(p);
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
            self.open_quick(None);
            self.launcher_cat = None;
            self.launcher_sel = 0;
            self.launcher_query.clear();
            self.launcher_focus_pending = true;
            self.launcher_seen_focus = false;
            self.power_armed = None;
        } else {
            self.launcher_closed_at = Instant::now();
        }
        self.launcher_open = open;
    }

    /// Open the volume or network flyout (`None` closes it).
    fn open_quick(&mut self, open: Option<quick::Section>) {
        if open == self.quick_open {
            return;
        }
        if open.is_some() {
            // Clicking the button while the flyout is open first takes its focus
            // away (which closes it); don't immediately reopen the same one.
            if self.quick_open.is_none() && self.quick_closed_at.elapsed() < Duration::from_millis(300) {
                if self.quick_last == open {
                    return;
                }
            }
            self.open_launcher(false);
            self.quick_seen_focus = false;
            self.qp.on_open();
            self.vol_state = self.qp.volume_state();
            self.quick_last = open;
        } else {
            self.quick_closed_at = Instant::now();
            self.qp.on_close();
        }
        self.quick_open = open;
    }

    /// Explorer was running when we started (so it owned the tray) and has
    /// since been closed: take over the notification area now. Programs that
    /// are already running re-add their icons when they get TaskbarCreated.
    fn adopt_tray(&mut self, ctx: &egui::Context) {
        if self.tray.is_some() || self.tray_checked.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.tray_checked = Instant::now();
        if win::find_class("Shell_TrayWnd").is_none() {
            self.tray = Some(tray::start(ctx.clone()));
            win::hide_minimized_windows();
        }
    }

    /// Month calendar under the clock, like the Windows one.
    fn calendar_viewport(&mut self, ctx: &egui::Context, pal: &Palette) {
        use chrono::NaiveDate;
        const W: f32 = 320.0;
        const H: f32 = 330.0;
        let screen = Self::screen_points(ctx);
        let builder = ViewportBuilder::default()
            .with_title("Delfin Calendar")
            .with_visible(false)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size([W, H])
            .with_position([screen.x - W - 8.0, screen.y - self.lift - PANEL_H - H - 8.0]);

        let mut close = false;
        let mut settings = false;
        ctx.show_viewport_immediate(ViewportId::from_hash_of("delfin_calendar"), builder, |ctx, _| {
                // Created hidden; shown as a tool window so no taskbar button flashes.
                if !win::reveal_popup("Delfin Calendar", true) {
                    ctx.request_repaint();
                }
            let focused = ctx.input(|i| i.viewport().focused);
            if focused == Some(true) {
                self.calendar_seen_focus = true;
            }
            if self.calendar_seen_focus && focused == Some(false) {
                close = true;
            }
            if !self.calendar_seen_focus {
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
            if ctx.input(|i| i.key_pressed(Key::Escape) || i.viewport().close_requested()) {
                close = true;
            }
            egui::CentralPanel::default()
                .frame(
                    Frame::none()
                        .fill(pal.window)
                        .stroke(Stroke::new(1.0_f32, pal.border))
                        .inner_margin(Margin::same(14.0)),
                )
                .show(ctx, |ui| {
                    let now = chrono::Local::now();
                    let today = now.date_naive();
                    ui.label(RichText::new(now.format("%H:%M").to_string()).size(28.0));
                    ui.label(RichText::new(format!("{}, {} {} {}", crate::i18n::weekday_name(today.weekday().num_days_from_monday()), today.day(), crate::i18n::month_in_date(today.month()), today.year())).color(pal.dim));
                    ui.add_space(8.0);
                    ui.separator();

                    let (y, m) = self.calendar_month;
                    let first = NaiveDate::from_ymd_opt(y, m, 1).unwrap_or(today);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{} {}", crate::i18n::month_name(first.month()), first.year())).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("▼").on_hover_text(crate::i18n::tr("Next month")).clicked() {
                                self.calendar_month = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
                            }
                            if ui.small_button("▲").on_hover_text(crate::i18n::tr("Previous month")).clicked() {
                                self.calendar_month = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
                            }
                            if (y, m) != (today.year(), today.month())
                                && ui.small_button(crate::i18n::tr("Today")).clicked()
                            {
                                self.calendar_month = (today.year(), today.month());
                            }
                        });
                    });
                    ui.add_space(4.0);

                    // Monday-first grid, 6 weeks.
                    let cell = vec2((ui.available_width() / 7.0).floor(), 28.0);
                    let origin = ui.cursor().min;
                    for (k, d) in (0..7u32).map(crate::i18n::weekday_short).enumerate() {
                        ui.painter().text(
                            origin + vec2(cell.x * (k as f32 + 0.5), cell.y * 0.5),
                            Align2::CENTER_CENTER,
                            d,
                            FontId::proportional(11.5),
                            pal.dim,
                        );
                    }
                    let lead = first.weekday().num_days_from_monday() as i64;
                    let start = first - chrono::Duration::days(lead);
                    for i in 0..42i64 {
                        let day = start + chrono::Duration::days(i);
                        let (row, col) = (i / 7, i % 7);
                        let r = Rect::from_min_size(
                            origin + vec2(col as f32 * cell.x, (row + 1) as f32 * cell.y),
                            cell,
                        );
                        let in_month = day.month() == m;
                        if day == today {
                            ui.painter().circle_filled(r.center(), 12.0, pal.accent);
                        }
                        ui.painter().text(
                            r.center(),
                            Align2::CENTER_CENTER,
                            day.day().to_string(),
                            FontId::proportional(13.0),
                            if day == today {
                                Color32::WHITE
                            } else if in_month {
                                pal.text
                            } else {
                                pal.border
                            },
                        );
                    }
                    ui.allocate_space(vec2(cell.x * 7.0, cell.y * 7.0));
                    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                        if ui.small_button(crate::i18n::tr("Date and time settings")).clicked() {
                            settings = true;
                        }
                    });
                });
        });
        if settings {
            win::shell_open("ms-settings:dateandtime");
            close = true;
        }
        if close {
            self.calendar_open = false;
            self.calendar_closed_at = Instant::now();
        }
    }

    /// Delfin's own settings: language, updates from GitHub, shell options.
    fn settings_viewport(&mut self, ctx: &egui::Context, pal: &Palette) {
        use crate::i18n::{tr, trf, Lang};
        use crate::update::State;
        const W: f32 = 480.0;
        const H: f32 = 460.0;
        let screen = Self::screen_points(ctx);
        let builder = ViewportBuilder::default()
            .with_title(tr("Delfin Settings"))
            .with_icon(theme::app_icon())
            .with_resizable(false)
            .with_inner_size([W, H])
            .with_position([(screen.x - W) / 2.0, (screen.y - H) / 2.0]);

        let mut close = false;
        let mut restart = false;
        let mut back_to_explorer = false;
        ctx.show_viewport_immediate(ViewportId::from_hash_of("delfin_settings"), builder, |ctx, _| {
            if ctx.input(|i| i.viewport().close_requested() || i.key_pressed(Key::Escape)) {
                close = true;
            }
            egui::CentralPanel::default()
                .frame(Frame::none().fill(pal.window).inner_margin(Margin::same(18.0)))
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                        paint_logo(ui.painter(), r.center(), 18.0, pal);
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Delfin").size(18.0).strong());
                            ui.label(RichText::new(trf("Version {}", &[&crate::update::CURRENT])).color(pal.dim));
                        });
                    });
                    ui.add_space(10.0);

                    // ---- language
                    ui.label(RichText::new(tr("Language")).strong());
                    let mut lang = crate::i18n::lang();
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut lang, Lang::En, "English");
                        ui.radio_value(&mut lang, Lang::Pl, "Polski");
                    });
                    if lang != crate::i18n::lang() {
                        crate::i18n::set_lang(lang);
                    }
                    ui.label(
                        RichText::new(tr("Some open Delfin windows switch language after they are reopened."))
                            .size(11.5)
                            .color(pal.dim),
                    );
                    ui.add_space(10.0);
                    ui.separator();

                    // ---- updates
                    ui.label(RichText::new(tr("Updates")).strong());
                    let state = self.updater.get();
                    ui.horizontal(|ui| {
                        let busy = matches!(state, State::Checking | State::Downloading);
                        if ui.add_enabled(!busy, egui::Button::new(tr("Check for updates"))).clicked() {
                            self.updater.check(ctx);
                        }
                        if let State::Available(r) = &state {
                            if ui.button(tr("Download and install")).clicked() {
                                self.updater.install(r.clone(), ctx);
                            }
                        }
                        if matches!(state, State::Installed) && ui.button(tr("Restart")).clicked() {
                            restart = true;
                        }
                    });
                    let (msg, col) = match &state {
                        State::Idle => (String::new(), pal.dim),
                        State::Checking => (tr("Checking for updates…").to_string(), pal.dim),
                        State::UpToDate => (tr("You have the latest version.").to_string(), pal.dim),
                        State::Available(r) => (trf("Version {} is available.", &[&r.version]), pal.accent),
                        State::Downloading => (tr("Downloading…").to_string(), pal.dim),
                        State::Installed => (tr("Update installed. Delfin will restart.").to_string(), pal.accent),
                        State::Error(e) => (e.clone(), pal.danger),
                    };
                    if !msg.is_empty() {
                        ui.label(RichText::new(msg).color(col));
                    }
                    if let State::Available(r) = &state {
                        if !r.notes.trim().is_empty() {
                            egui::CollapsingHeader::new(tr("Release notes")).show(ui, |ui| {
                                ScrollArea::vertical().max_height(90.0).show(ui, |ui| {
                                    ui.label(RichText::new(r.notes.trim()).size(12.0));
                                });
                            });
                        }
                    }
                    if let Some(p) = crate::update::target_path() {
                        ui.label(
                            RichText::new(trf("Updates replace: {}", &[&p.display()]))
                                .size(11.5)
                                .color(pal.dim),
                        );
                    }
                    ui.add_space(10.0);
                    ui.separator();

                    // ---- shell options
                    ui.label(RichText::new(tr("Shell")).strong());
                    if ui.checkbox(&mut self.cfg.minimize_to_tray, tr("Minimize to tray")).changed() {
                        self.cfg.save();
                        if !self.cfg.minimize_to_tray {
                            self.unshelve_all();
                        }
                    }
                    let armed = self
                        .power_armed
                        .is_some_and(|(p, t)| p == Power::BackToExplorer && t.elapsed() < Duration::from_secs(5));
                    let label = if armed { tr("Click again to switch") } else { tr("Back to Explorer") };
                    if ui.button(label).clicked() {
                        back_to_explorer = true;
                    }
                });
        });

        if back_to_explorer {
            let confirmed = self
                .power_armed
                .is_some_and(|(q, t)| q == Power::BackToExplorer && t.elapsed() < Duration::from_secs(5));
            if confirmed {
                self.power(Power::BackToExplorer);
            } else {
                self.power_armed = Some((Power::BackToExplorer, Instant::now()));
            }
        }
        if restart {
            // Start the new exe once this one has exited (only one shell may run).
            if let Some(exe) = crate::update::target_path() {
                win::run_hidden(&format!("ping -n 3 127.0.0.1 >nul & start \"\" \"{}\" --shell", exe.display()));
            }
            self.unshelve_all();
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
        }
        if close {
            self.settings_open = false;
        }
    }

    /// Volume and Wi-Fi signal for the panel icon (every 2 s).
    fn poll_status(&mut self) {
        if self.last_status_poll.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last_status_poll = Instant::now();
        if self.quick_open.is_none() {
            self.qp.refresh_volume(); // the default device may have changed
        }
        self.vol_state = self.qp.volume_state();
        self.wifi_sig = self.qp.wifi_signal();
    }

    fn quick_viewport(&mut self, ctx: &egui::Context, pal: &Palette, section: quick::Section) {
        let screen = Self::screen_points(ctx);
        let builder = ViewportBuilder::default()
            .with_title("Delfin Quick Settings")
            .with_visible(false)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size([QUICK_W, section.height()])
            .with_position([screen.x - QUICK_W - 8.0, screen.y - self.lift - PANEL_H - section.height() - 8.0]);

        let mut close = false;
        let mut action = None;
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("delfin_quick_settings"),
            builder,
            |ctx, _| {
                // Created hidden; shown as a tool window so no taskbar button flashes.
                if !win::reveal_popup("Delfin Quick Settings", true) {
                    ctx.request_repaint();
                }
                let focused = ctx.input(|i| i.viewport().focused);
                if focused == Some(true) {
                    self.quick_seen_focus = true;
                }
                if self.quick_seen_focus && focused == Some(false) {
                    close = true;
                }
                if !self.quick_seen_focus {
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                if ctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
                egui::CentralPanel::default()
                    .frame(
                        Frame::none()
                            .fill(pal.window)
                            .stroke(Stroke::new(1.0_f32, pal.border))
                            .inner_margin(Margin::same(14.0)),
                    )
                    .show(ctx, |ui| {
                        action = self.qp.ui(ui, pal, section);
                    });
            },
        );
        match action {
            Some(quick::QuickAction::Open(uri)) => {
                win::shell_open(uri);
                close = true;
            }
            Some(quick::QuickAction::Close) => close = true,
            None => {}
        }
        if close {
            self.open_quick(None);
        } else {
            self.vol_state = self.qp.volume_state();
        }
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
                self.unshelve_all();
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
        // Games and full-screen videos cover the panel, like Explorer's taskbar.
        let full = fg != 0 && win::pid_of(fg) != self.own_pid && win::is_fullscreen(fg);
        if full != self.fullscreen {
            self.fullscreen = full;
            if let Some(panel) = win::find_window(PANEL_TITLE) {
                if full {
                    win::panel_below(panel, fg);
                } else {
                    win::panel_on_top(panel);
                }
            }
        }
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
        // Minimize to tray: hide minimized windows and show them as tray icons.
        if self.cfg.minimize_to_tray {
            let mut changed = false;
            for t in &merged {
                if t.minimized && !self.shelved.contains(&t.hwnd) {
                    win::hide(t.hwnd);
                    self.shelved.push(t.hwnd);
                    changed = true;
                }
            }
            merged.retain(|t| !self.shelved.contains(&t.hwnd));
            if changed {
                save_shelved(&self.shelved);
            }
        }
        let before = self.shelved.len();
        self.shelved.retain(|h| win::is_window(*h));
        if self.shelved.len() != before {
            save_shelved(&self.shelved);
        }

        // Drop tray icons of programs that died without removing them.
        if let Some(t) = &self.tray {
            let mut st = t.lock().unwrap_or_else(|p| p.into_inner());
            st.icons.retain(|i| win::is_window(i.hwnd));
        }

        let shelved = self.shelved.clone();
        self.icon_of
            .retain(|h, _| merged.iter().any(|t| t.hwnd == *h) || shelved.contains(h));
        self.hidden_for_desktop
            .retain(|h| merged.iter().any(|t| t.hwnd == *h));
        self.tasks = merged;
    }

    /// Keep the panel glued to the bottom of the primary screen.
    fn place_panel(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        let (w_px, h_px) = win::screen_px();
        let panel_px = (PANEL_H * ppp).round() as i32;
        // Explorer still running (trying the shell out): sit just above its taskbar.
        let explorer_px = win::explorer_taskbar_px(self.own_pid);
        let key = (w_px, h_px, panel_px + explorer_px * 10_000);
        if self.placed == Some(key) {
            return;
        }
        self.placed = Some(key);
        self.lift = explorer_px as f32 / ppp;
        let w = w_px as f32 / ppp;
        let h = h_px as f32 / ppp;
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(0.0, h - self.lift - PANEL_H)));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(w, PANEL_H)));
        win::set_work_area(panel_px + explorer_px);
    }

    fn screen_points(ctx: &egui::Context) -> egui::Vec2 {
        let ppp = ctx.pixels_per_point();
        let (w, h) = win::screen_px();
        vec2(w as f32 / ppp, h as f32 / ppp)
    }

    // ------------------------------------------------------------ panel

    fn panel_ui(&mut self, ctx: &egui::Context, pal: &Palette) {
        let mut actions: Vec<PanelAction> = Vec::new();
        let mut hovered: Option<(String, f32)> = None;
        let mut hover_id: Option<(isize, u32)> = None;
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
                    Stroke::new(1.0_f32, pal.border),
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
                        if r.hovered() {
                            hovered = Some((t.title.clone(), r.rect.center().x));
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let c = clock(ui, self.calendar_open, pal);
                        if c.clicked() {
                            actions.push(PanelAction::ToggleCalendar);
                        }
                        if c.secondary_clicked() {
                            actions.push(PanelAction::Uri("ms-settings:dateandtime"));
                        }
                        // Volume and network: two separate buttons, each with its own
                        // flyout (layout is right-to-left, so volume is drawn first).
                        for section in [quick::Section::Sound, quick::Section::Network] {
                            let (r, resp) = ui.allocate_exact_size(vec2(30.0, 36.0), Sense::click());
                            let p = ui.painter();
                            if self.quick_open == Some(section) {
                                p.rect_filled(r, 4.0, pal.select);
                            } else if resp.hovered() {
                                p.rect_filled(r, 4.0, pal.hover);
                            }
                            let tip = match section {
                                quick::Section::Sound => {
                                    let level = self
                                        .vol_state
                                        .map(|(l, m)| if m { None } else { Some(l) })
                                        .unwrap_or(Some(1.0));
                                    quick::paint_speaker(p, r, pal.text, level);
                                    // Mouse wheel over the speaker changes the volume.
                                    if resp.hovered() {
                                        let wheel = ui.input(|i| i.raw_scroll_delta.y);
                                        if wheel != 0.0 {
                                            actions.push(PanelAction::Volume(wheel.signum() * 0.02));
                                        }
                                    }
                                    match self.vol_state {
                                        Some((_, true)) => crate::i18n::tr("Volume: muted").to_string(),
                                        Some((l, false)) => crate::i18n::trf("Volume: {}%", &[&(l * 100.0).round()]),
                                        None => crate::i18n::tr("Volume").to_string(),
                                    }
                                }
                                quick::Section::Network => {
                                    quick::paint_wifi(p, r, pal.text, pal.dim.gamma_multiply(0.5), self.wifi_sig);
                                    match self.qp.connected_ssid() {
                                        Some(ssid) => crate::i18n::trf("Wi-Fi: {}", &[&ssid]),
                                        None => crate::i18n::tr("Network").to_string(),
                                    }
                                }
                            };
                            if resp.hovered() {
                                hovered = Some((tip, r.center().x));
                            }
                            if resp.clicked() {
                                actions.push(PanelAction::ToggleQuick(section));
                            }
                            if resp.secondary_clicked() {
                                actions.push(PanelAction::Uri(match section {
                                    quick::Section::Sound => "ms-settings:sound",
                                    quick::Section::Network => "ms-settings:network",
                                }));
                            }
                        }
                        // ---- notification area (layout is right-to-left, so iterate reversed)
                        let icons_now = self.tray_icons();
                        let inline_n = icons_now.len().min(TRAY_INLINE);
                        for ic in icons_now[..inline_n].iter().rev() {
                            let tex = self.tray_texture(ctx, ic);
                            let r = Self::tray_slot(ui, tex.as_ref(), false, pal);
                            tray::set_icon_rect(ic.hwnd, ic.uid, self.panel_rect_px(ctx, r.rect));
                            let x = r.rect.center().x;
                            let id = (ic.hwnd, ic.uid);
                            if r.hovered() {
                                hovered = Some((ic.tip.clone(), x));
                                if self.last_hover != Some(id) {
                                    actions.push(PanelAction::Tray(
                                        ic.hwnd,
                                        ic.uid,
                                        Click::Hover,
                                        x,
                                    ));
                                }
                                hover_id = Some(id);
                            }
                            if r.double_clicked() {
                                actions.push(PanelAction::Tray(ic.hwnd, ic.uid, Click::Double, x));
                            } else if r.clicked() {
                                actions.push(PanelAction::Tray(ic.hwnd, ic.uid, Click::Left, x));
                            }
                            if r.secondary_clicked() {
                                actions.push(PanelAction::Tray(ic.hwnd, ic.uid, Click::Right, x));
                            }
                            if r.middle_clicked() {
                                actions.push(PanelAction::Tray(ic.hwnd, ic.uid, Click::Middle, x));
                            }
                        }
                        // Windows minimized to tray (blue underline).
                        for h in self.shelved.clone().iter().rev() {
                            let tex = self.tex_window(ctx, *h);
                            let r = Self::tray_slot(ui, tex.as_ref(), true, pal);
                            if r.hovered() {
                                hovered = Some((
                                    "Minimized to tray \u{2013} click to restore".into(),
                                    r.rect.center().x,
                                ));
                            }
                            if r.clicked() || r.secondary_clicked() {
                                actions.push(PanelAction::Unshelve(*h));
                            }
                        }
                        if icons_now.len() > TRAY_INLINE {
                            let r = icons::tool_button_sized(
                                ui,
                                Tool::Up,
                                self.overflow_open,
                                true,
                                "",
                                vec2(24.0, 36.0),
                            );
                            if r.hovered() {
                                hovered = Some((crate::i18n::tr("Show hidden icons").into(), r.rect.center().x));
                            }
                            if r.clicked() {
                                actions.push(PanelAction::ToggleOverflow);
                            }
                        }
                    });
                });
            });

        self.last_hover = hover_id;
        // Tooltip (drawn in its own small window above the panel).
        self.tip = match (hovered, self.tip.take()) {
            (Some((text, x)), Some((old, _, since))) if old == text => Some((text, x, since)),
            (Some((text, x)), _) if !text.is_empty() => Some((text, x, Instant::now())),
            _ => None,
        };

        for a in actions {
            match a {
                PanelAction::Tray(h, uid, kind, x) => self.send_tray_click(ctx, h, uid, kind, x),
                PanelAction::Unshelve(h) => self.unshelve(h),
                PanelAction::ToggleOverflow => {
                    if self.overflow_open {
                        self.overflow_open = false;
                        self.overflow_closed_at = Instant::now();
                    } else if self.overflow_closed_at.elapsed() > Duration::from_millis(300) {
                        self.overflow_open = true;
                        self.overflow_seen_focus = false;
                    }
                }
                PanelAction::ToggleQuick(section) => {
                    let open = (self.quick_open != Some(section)).then_some(section);
                    self.open_quick(open);
                }
                PanelAction::Volume(delta) => {
                    self.qp.nudge_volume(delta);
                    self.vol_state = self.qp.volume_state();
                }
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
                PanelAction::ToggleCalendar => {
                    if self.calendar_open {
                        self.calendar_open = false;
                        self.calendar_closed_at = Instant::now();
                    } else if self.calendar_closed_at.elapsed() > Duration::from_millis(300) {
                        self.open_quick(None);
                        self.open_launcher(false);
                        self.calendar_open = true;
                        self.calendar_seen_focus = false;
                        let today = chrono::Local::now().date_naive();
                        self.calendar_month = (today.year(), today.month());
                    }
                }
                PanelAction::Uri(u) => {
                    win::shell_open(u);
                }
            }
        }
    }

    // ------------------------------------------------------------ launcher

    fn launcher_viewport(&mut self, ctx: &egui::Context, pal: &Palette) {
        let screen = Self::screen_points(ctx);
        let builder = ViewportBuilder::default()
            .with_title("Delfin Launcher")
            .with_visible(false)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size([LAUNCHER_W, LAUNCHER_H])
            .with_position([4.0, screen.y - self.lift - PANEL_H - LAUNCHER_H - 4.0]);

        let mut close = false;
        let mut launch: Option<(Launch, Option<String>)> = None;
        let mut power: Option<Power> = None;

        // What the middle column shows: nothing until you type or pick a category.
        let query = self.launcher_query.trim().to_string();
        let rows: Vec<usize> = if !query.is_empty() {
            startmenu::search(&self.apps, &query)
        } else {
            match self.launcher_cat {
                None => Vec::new(),
                Some(Category::All) => (0..self.apps.len()).collect(),
                Some(Category::Recent) => self
                    .recent
                    .iter()
                    .filter_map(|n| self.apps.iter().position(|a| &a.name == n))
                    .collect(),
                Some(c) => (0..self.apps.len())
                    .filter(|&i| startmenu::category_of(&self.apps[i]) == Some(c))
                    .collect(),
            }
        };
        // With a search, the last row runs the typed text as a command.
        let run_row = !query.is_empty();
        let n_rows = rows.len() + run_row as usize;
        if self.launcher_sel >= n_rows {
            self.launcher_sel = n_rows.saturating_sub(1);
        }

        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("delfin_launcher"),
            builder,
            |ctx, _| {
                // Created hidden; shown as a tool window so no taskbar button flashes.
                if !win::reveal_popup("Delfin Launcher", true) {
                    ctx.request_repaint();
                }
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
                let (down, up, enter) = ctx.input(|i| {
                    (i.key_pressed(Key::ArrowDown), i.key_pressed(Key::ArrowUp), i.key_pressed(Key::Enter))
                });
                let mut scroll_to_sel = false;
                if down && self.launcher_sel + 1 < n_rows {
                    self.launcher_sel += 1;
                    scroll_to_sel = true;
                }
                if up && self.launcher_sel > 0 {
                    self.launcher_sel -= 1;
                    scroll_to_sel = true;
                }
                if enter && n_rows > 0 {
                    launch = Some(match rows.get(self.launcher_sel) {
                        Some(&i) => (self.apps[i].target.clone(), Some(self.apps[i].name.clone())),
                        None => (Launch::Cmd(query.clone()), None),
                    });
                }

                egui::CentralPanel::default()
                    .frame(
                        Frame::none()
                            .fill(pal.window)
                            .stroke(Stroke::new(1.0_f32, pal.border))
                            .inner_margin(Margin::same(14.0)),
                    )
                    .show(ctx, |ui| {
                        let full = ui.max_rect();
                        let left_w = 176.0;
                        let right_w = 176.0;

                        // ---- header: logo + search
                        let logo = Rect::from_min_size(full.min, vec2(left_w, 44.0));
                        paint_logo(ui.painter(), logo.center(), 20.0, pal);
                        let search = Rect::from_min_max(
                            pos2(full.left() + left_w + 8.0, full.top() + 4.0),
                            pos2(full.right(), full.top() + 40.0),
                        );
                        ui.painter().rect(search, 18.0, pal.view, Stroke::new(1.0_f32, pal.border));
                        icons::paint_tool(
                            ui.painter(),
                            Rect::from_center_size(pos2(search.left() + 20.0, search.center().y), vec2(18.0, 18.0)),
                            Tool::Search,
                            pal.dim,
                        );
                        let before = self.launcher_query.clone();
                        let te = ui.put(
                            Rect::from_min_max(pos2(search.left() + 36.0, search.top() + 4.0), search.max - vec2(14.0, 4.0)),
                            egui::TextEdit::singleline(&mut self.launcher_query)
                                .hint_text(crate::i18n::tr("Search applications…"))
                                .frame(false)
                                .font(FontId::proportional(15.0))
                                .vertical_align(Align::Center),
                        );
                        if self.launcher_focus_pending {
                            te.request_focus();
                            self.launcher_focus_pending = false;
                        }
                        if self.launcher_query != before {
                            self.launcher_sel = 0;
                        }

                        let body_top = full.top() + 56.0;
                        let left = Rect::from_min_max(pos2(full.left(), body_top), pos2(full.left() + left_w, full.bottom()));
                        let right = Rect::from_min_max(pos2(full.right() - right_w, body_top), full.max);
                        let mid = Rect::from_min_max(pos2(left.right() + 10.0, body_top), pos2(right.left() - 10.0, full.bottom()));
                        let line = Stroke::new(1.0_f32, pal.border);
                        ui.painter().line_segment([pos2(mid.left() - 5.0, body_top), pos2(mid.left() - 5.0, full.bottom())], line);
                        ui.painter().line_segment([pos2(mid.right() + 5.0, body_top), pos2(mid.right() + 5.0, full.bottom())], line);

                        // ---- left: categories
                        let mut y = left.top();
                        for cat in Category::LIST {
                            let r = Rect::from_min_size(pos2(left.left(), y), vec2(left.width() - 8.0, 36.0));
                            let resp = ui.interact(r, ui.id().with(("cat", cat.label())), Sense::click());
                            let on = query.is_empty() && self.launcher_cat == Some(cat);
                            if on {
                                ui.painter().rect_filled(r, 6.0, pal.select);
                            } else if resp.hovered() {
                                ui.painter().rect_filled(r, 6.0, pal.hover);
                            }
                            paint_category(
                                ui.painter(),
                                Rect::from_center_size(pos2(r.left() + 20.0, r.center().y), vec2(20.0, 20.0)),
                                cat,
                                pal,
                            );
                            ui.painter().text(
                                pos2(r.left() + 40.0, r.center().y),
                                Align2::LEFT_CENTER,
                                cat.label(),
                                FontId::proportional(14.0),
                                pal.text,
                            );
                            if resp.clicked() {
                                self.launcher_cat = if self.launcher_cat == Some(cat) { None } else { Some(cat) };
                                self.launcher_query.clear();
                                self.launcher_sel = 0;
                                self.launcher_focus_pending = true;
                            }
                            y += 40.0;
                        }

                        // ---- middle: empty state, a category, or search results
                        if n_rows == 0 {
                            let c = mid.center() - vec2(0.0, 20.0);
                            icons::paint_tool(ui.painter(), Rect::from_center_size(c - vec2(0.0, 28.0), vec2(26.0, 26.0)), Tool::Search, pal.dim);
                            let (title, sub) = if !query.is_empty() {
                                (crate::i18n::tr("No matching applications"), crate::i18n::tr("Press Enter to run it as a command"))
                            } else if self.launcher_cat == Some(Category::Recent) {
                                (crate::i18n::tr("Nothing launched yet"), crate::i18n::tr("Apps you start from here show up in this list"))
                            } else if self.launcher_cat.is_some() {
                                (crate::i18n::tr("No applications in this category"), crate::i18n::tr("Search finds every installed app"))
                            } else {
                                (crate::i18n::tr("Start typing to find an app"), crate::i18n::tr("or pick a category on the left"))
                            };
                            ui.painter().text(c + vec2(0.0, 6.0), Align2::CENTER_CENTER, title, FontId::proportional(15.0), pal.text);
                            ui.painter().text(c + vec2(0.0, 28.0), Align2::CENTER_CENTER, sub, FontId::proportional(12.5), pal.dim);
                        } else {
                            let heading = if !query.is_empty() {
                                crate::i18n::tr("Results").to_string()
                            } else {
                                self.launcher_cat.map(|c| c.label().to_string()).unwrap_or_default()
                            };
                            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(mid), |ui| {
                                ui.label(RichText::new(heading).size(12.0).color(pal.dim));
                                ui.add_space(2.0);
                                ScrollArea::vertical()
                                    .id_salt("launcher-list")
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        for k in 0..n_rows {
                                            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
                                            if resp.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                                                self.launcher_sel = k;
                                            }
                                            if k == self.launcher_sel {
                                                ui.painter().rect_filled(r, 6.0, pal.hover);
                                                if scroll_to_sel {
                                                    ui.scroll_to_rect(r, None);
                                                }
                                            }
                                            if !ui.is_rect_visible(r) {
                                                continue;
                                            }
                                            let ir = Rect::from_center_size(pos2(r.left() + 22.0, r.center().y), vec2(30.0, 30.0));
                                            let tx = r.left() + 46.0;
                                            let (name, desc) = match rows.get(k) {
                                                Some(&i) => {
                                                    let app = self.apps[i].clone();
                                                    match (&app.icon, &app.target) {
                                                        (Some(t), _) => icons::paint_tool(ui.painter(), ir.shrink(3.0), *t, pal.text),
                                                        (None, Launch::Path(p)) => match self.tex_file(ctx, p, true) {
                                                            Some(tex) => {
                                                                ui.painter().image(tex.id(), ir, uv_full(), Color32::WHITE);
                                                            }
                                                            None => icons::paint_file(ui.painter(), ir, icons::kind_color(icons::Kind::Exe), "", pal),
                                                        },
                                                        _ => {}
                                                    }
                                                    if resp.clicked() {
                                                        launch = Some((app.target.clone(), Some(app.name.clone())));
                                                    }
                                                    let desc = if app.desc.is_empty() { app.folder.clone() } else { app.desc.clone() };
                                                    (app.name, desc)
                                                }
                                                None => {
                                                    icons::paint_tool(ui.painter(), ir.shrink(4.0), Tool::Terminal, pal.dim);
                                                    if resp.clicked() {
                                                        launch = Some((Launch::Cmd(query.clone()), None));
                                                    }
                                                    (crate::i18n::trf("Run \u{201c}{}\u{201d}", &[&query]), crate::i18n::tr("Command").to_string())
                                                }
                                            };
                                            let p = ui.painter().with_clip_rect(r.shrink(2.0));
                                            if desc.is_empty() {
                                                p.text(pos2(tx, r.center().y), Align2::LEFT_CENTER, &name, FontId::proportional(14.0), pal.text);
                                            } else {
                                                p.text(pos2(tx, r.center().y - 8.0), Align2::LEFT_CENTER, &name, FontId::proportional(14.0), pal.text);
                                                p.text(pos2(tx, r.center().y + 10.0), Align2::LEFT_CENTER, &desc, FontId::proportional(12.0), pal.dim);
                                            }
                                        }
                                    });
                            });
                        }

                        // ---- right: places and system shortcuts
                        let links: [(Option<Tool>, &str); 7] = [
                            (Some(Tool::Desktop), crate::i18n::tr("This PC")),
                            (None, crate::i18n::tr("Documents")),
                            (None, crate::i18n::tr("Downloads")),
                            (Some(Tool::Gear), crate::i18n::tr("Settings")),
                            (Some(Tool::Details), crate::i18n::tr("Task Manager")),
                            (Some(Tool::Terminal), crate::i18n::tr("Run…")),
                            (None, "Delfin"),
                        ];
                        let mut y = right.top();
                        for (k, (tool, label)) in links.iter().enumerate() {
                            let r = Rect::from_min_size(pos2(right.left(), y), vec2(right.width(), 38.0));
                            let resp = ui.interact(r, ui.id().with(("link", k)), Sense::click());
                            ui.painter().rect(
                                r,
                                6.0,
                                if resp.hovered() { pal.hover } else { pal.view },
                                Stroke::new(1.0_f32, pal.border),
                            );
                            let ir = Rect::from_center_size(pos2(r.left() + 20.0, r.center().y), vec2(18.0, 18.0));
                            match tool {
                                Some(t) => icons::paint_tool(ui.painter(), ir, *t, pal.text),
                                None if k == 6 => paint_logo(ui.painter(), ir.center(), 9.0, pal),
                                None => icons::paint_folder(ui.painter(), ir, icons::FOLDER),
                            }
                            ui.painter().text(pos2(r.left() + 38.0, r.center().y), Align2::LEFT_CENTER, *label, FontId::proportional(13.5), pal.text);
                            if resp.clicked() {
                                match k {
                                    0 => launch = Some((Launch::Cmd("delfin".into()), None)),
                                    1 => {
                                        if let Some(d) = dirs::document_dir() {
                                            launch = Some((Launch::Path(d), None));
                                        }
                                    }
                                    2 => {
                                        if let Some(d) = dirs::download_dir() {
                                            launch = Some((Launch::Path(d), None));
                                        }
                                    }
                                    3 => launch = Some((Launch::Cmd("ms-settings:".into()), None)),
                                    4 => launch = Some((Launch::Cmd("taskmgr.exe".into()), None)),
                                    5 => {
                                        // Run: the search box doubles as the Run box.
                                        self.launcher_cat = None;
                                        self.launcher_focus_pending = true;
                                    }
                                    _ => {
                                        self.settings_open = true;
                                        close = true;
                                    }
                                }
                            }
                            y += 44.0;
                        }

                        let power_top = right.bottom() - 54.0;

                        // ---- power row
                        ui.painter().line_segment([pos2(right.left(), power_top), pos2(right.right(), power_top)], line);
                        let armed = self
                            .power_armed
                            .filter(|(_, t)| t.elapsed() < Duration::from_secs(5))
                            .map(|(p, _)| p);
                        let buttons: [(Tool, &str, Option<Power>, Color32); 4] = [
                            (Tool::Lock, crate::i18n::tr("Lock"), None, pal.text),
                            (Tool::Power, crate::i18n::tr("Shut Down"), Some(Power::Shutdown), pal.danger),
                            (Tool::Refresh, crate::i18n::tr("Restart"), Some(Power::Restart), pal.accent),
                            (Tool::Logout, crate::i18n::tr("Log Out"), Some(Power::Logout), Color32::from_rgb(0x27, 0xae, 0x60)),
                        ];
                        let bw = right.width() / 4.0;
                        for (k, (tool, label, p, col)) in buttons.iter().enumerate() {
                            let r = Rect::from_min_size(pos2(right.left() + k as f32 * bw, power_top + 4.0), vec2(bw, 50.0));
                            let resp = ui.interact(r, ui.id().with(("power", k)), Sense::click());
                            if resp.hovered() {
                                ui.painter().rect_filled(r, 6.0, pal.hover);
                            }
                            let is_armed = p.is_some() && armed == *p;
                            icons::paint_tool(
                                ui.painter(),
                                Rect::from_center_size(pos2(r.center().x, r.top() + 16.0), vec2(20.0, 20.0)),
                                *tool,
                                *col,
                            );
                            ui.painter().text(
                                pos2(r.center().x, r.top() + 38.0),
                                Align2::CENTER_CENTER,
                                if is_armed { crate::i18n::tr("Sure?") } else { *label },
                                FontId::proportional(10.5),
                                if is_armed { pal.danger } else { pal.text },
                            );
                            let resp = resp.on_hover_text(if p.is_some() { crate::i18n::trf("{} (click twice)", &[label]) } else { label.to_string() });
                            if resp.clicked() {
                                match p {
                                    Some(p) => power = Some(*p),
                                    None => {
                                        win::run_hidden("rundll32.exe user32.dll,LockWorkStation");
                                        close = true;
                                    }
                                }
                            }
                        }
                    });
            },
        );

        if let Some((l, name)) = launch {
            if let Some(n) = name {
                startmenu::push_recent(&mut self.recent, &n);
            }
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

    // ------------------------------------------------------------ tray popups

    /// Hidden tray icons behind the ^ button, like the Windows overflow flyout.
    fn overflow_viewport(&mut self, ctx: &egui::Context, pal: &Palette) {
        let hidden: Vec<TrayIcon> = self.tray_icons().into_iter().skip(TRAY_INLINE).collect();
        if hidden.is_empty() {
            self.overflow_open = false;
            return;
        }
        let screen = Self::screen_points(ctx);
        let cols = 5usize;
        let rows = hidden.len().div_ceil(cols);
        let size = vec2(cols as f32 * 36.0 + 16.0, rows as f32 * 40.0 + 16.0);
        let x = (screen.x - size.x - 260.0).max(0.0);
        let builder = ViewportBuilder::default()
            .with_title("Delfin Tray")
            .with_visible(false)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(size)
            .with_position([x, screen.y - self.lift - PANEL_H - size.y - 4.0]);

        let mut close = false;
        let mut clicks: Vec<(isize, u32, Click, f32)> = Vec::new();
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("delfin_tray_overflow"),
            builder,
            |ctx, _| {
                // Created hidden; shown as a tool window so no taskbar button flashes.
                if !win::reveal_popup("Delfin Tray", true) {
                    ctx.request_repaint();
                }
                let focused = ctx.input(|i| i.viewport().focused);
                if focused == Some(true) {
                    self.overflow_seen_focus = true;
                }
                if self.overflow_seen_focus && focused == Some(false) {
                    close = true;
                }
                if !self.overflow_seen_focus {
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                egui::CentralPanel::default()
                    .frame(
                        Frame::none()
                            .fill(pal.window)
                            .stroke(Stroke::new(1.0_f32, pal.border))
                            .inner_margin(Margin::same(8.0)),
                    )
                    .show(ctx, |ui| {
                        for row in hidden.chunks(cols) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                for ic in row {
                                    let tex = self.tray_texture(ctx, ic);
                                    let r = Self::tray_slot(ui, tex.as_ref(), false, pal);
                                    let r = if ic.tip.is_empty() {
                                        r
                                    } else {
                                        r.on_hover_text(&ic.tip)
                                    };
                                    let ax = x + r.rect.center().x;
                                    if r.double_clicked() {
                                        clicks.push((ic.hwnd, ic.uid, Click::Double, ax));
                                    } else if r.clicked() {
                                        clicks.push((ic.hwnd, ic.uid, Click::Left, ax));
                                    }
                                    if r.secondary_clicked() {
                                        clicks.push((ic.hwnd, ic.uid, Click::Right, ax));
                                    }
                                }
                            });
                        }
                    });
            },
        );
        for (h, uid, kind, ax) in clicks {
            self.send_tray_click(ctx, h, uid, kind, ax);
            close = true;
        }
        if close {
            self.overflow_open = false;
            self.overflow_closed_at = Instant::now();
        }
    }

    /// Notifications sent with NIF_INFO (Explorer would show them as toasts).
    fn balloon_viewport(&mut self, ctx: &egui::Context, pal: &Palette) {
        let Some(shared) = self.tray.clone() else {
            return;
        };
        let first = {
            let st = shared.lock().unwrap_or_else(|p| p.into_inner());
            st.balloons.first().cloned()
        };
        let Some(b) = first else {
            self.balloon_since = None;
            return;
        };
        let since = *self.balloon_since.get_or_insert_with(Instant::now);
        let pop = |shared: &tray::Shared| {
            let mut st = shared.lock().unwrap_or_else(|p| p.into_inner());
            if !st.balloons.is_empty() {
                st.balloons.remove(0);
            }
        };
        if since.elapsed() > Duration::from_secs(BALLOON_SECS) {
            tray::post(
                b.hwnd,
                b.uid,
                b.callback,
                b.version,
                tray::NIN_BALLOONTIMEOUT,
                (0, 0),
            );
            pop(&shared);
            self.balloon_since = None;
            return;
        }

        let screen = Self::screen_points(ctx);
        let size = vec2(380.0, 104.0);
        let builder = ViewportBuilder::default()
            .with_title("Delfin Notification")
            .with_visible(false)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_active(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(size)
            .with_position([screen.x - size.x - 10.0, screen.y - self.lift - PANEL_H - size.y - 10.0]);

        let mut clicked = false;
        let mut dismissed = false;
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("delfin_balloon"),
            builder,
            |ctx, _| {
                // Created hidden; shown as a tool window so no taskbar button flashes.
                if !win::reveal_popup("Delfin Notification", false) {
                    ctx.request_repaint();
                }
                egui::CentralPanel::default()
                    .frame(
                        Frame::none()
                            .fill(pal.window)
                            .stroke(Stroke::new(1.0_f32, pal.accent))
                            .inner_margin(Margin::same(12.0)),
                    )
                    .show(ctx, |ui| {
                        let all =
                            ui.interact(ui.max_rect(), egui::Id::new("balloon_bg"), Sense::click());
                        ui.horizontal(|ui| {
                            let title = if b.title.is_empty() {
                                crate::i18n::tr("Notification")
                            } else {
                                b.title.as_str()
                            };
                            ui.label(RichText::new(title).strong());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icons::tool_button_sized(
                                    ui,
                                    Tool::Close,
                                    false,
                                    true,
                                    "",
                                    vec2(20.0, 20.0),
                                )
                                .clicked()
                                {
                                    dismissed = true;
                                }
                            });
                        });
                        ui.add_space(4.0);
                        ui.label(RichText::new(&b.text).color(pal.dim));
                        if all.clicked() {
                            clicked = true;
                        }
                    });
            },
        );
        if clicked || dismissed {
            let msg = if dismissed {
                tray::NIN_BALLOONTIMEOUT
            } else {
                tray::NIN_BALLOONUSERCLICK
            };
            if !dismissed {
                allow_foreground(b.hwnd);
            }
            tray::post(b.hwnd, b.uid, b.callback, b.version, msg, (0, 0));
            pop(&shared);
            self.balloon_since = None;
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    /// Tooltips can't fit inside the 44 px panel window, so they get their own.
    fn tooltip_viewport(&mut self, ctx: &egui::Context, pal: &Palette) {
        let Some((text, x, since)) = self.tip.clone() else {
            return;
        };
        if since.elapsed() < Duration::from_millis(450) {
            ctx.request_repaint_after(Duration::from_millis(120));
            return;
        }
        let screen = Self::screen_points(ctx);
        let lines = text.lines().count().max(1) as f32;
        let longest = text.lines().map(|l| l.chars().count()).max().unwrap_or(1) as f32;
        let size = vec2(
            (longest * 7.2 + 20.0).clamp(40.0, 420.0),
            lines * 17.0 + 12.0,
        );
        let left = (x - size.x / 2.0).clamp(2.0, (screen.x - size.x - 2.0).max(2.0));
        let builder = ViewportBuilder::default()
            .with_title("Delfin Tooltip")
            .with_visible(false)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_active(false)
            .with_mouse_passthrough(true)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(size)
            .with_position([left, screen.y - self.lift - PANEL_H - size.y - 4.0]);
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("delfin_tooltip"),
            builder,
            |ctx, _| {
                // Created hidden; shown as a tool window so no taskbar button flashes.
                if !win::reveal_popup("Delfin Tooltip", false) {
                    ctx.request_repaint();
                }
                egui::CentralPanel::default()
                    .frame(
                        Frame::none()
                            .fill(pal.header)
                            .stroke(Stroke::new(1.0_f32, pal.border))
                            .inner_margin(Margin::symmetric(8.0, 5.0)),
                    )
                    .show(ctx, |ui| {
                        ui.label(RichText::new(&text).size(12.5));
                    });
            },
        );
    }

    // ------------------------------------------------------------ desktop

    fn load_desktop(&mut self) {
        let mut dirs_: Vec<PathBuf> = Vec::new();
        if let Some(d) = dirs::desktop_dir() {
            dirs_.push(d);
        }
        if let Ok(p) = std::env::var("PUBLIC") {
            dirs_.push(PathBuf::from(p).join(crate::i18n::tr("Desktop")));
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
                                if ui.button(crate::i18n::tr("Open Desktop Folder")).clicked() {
                                    actions.push(DeskAction::Open(d.clone(), true));
                                    ui.close_menu();
                                }
                                if ui.button(crate::i18n::tr("Open Terminal Here")).clicked() {
                                    actions.push(DeskAction::Terminal(d));
                                    ui.close_menu();
                                }
                            }
                            if ui.button(crate::i18n::tr("Refresh")).clicked() {
                                actions.push(DeskAction::Refresh);
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button(crate::i18n::tr("Change Wallpaper…")).clicked() {
                                actions.push(DeskAction::Uri(
                                    "ms-settings:personalization-background",
                                ));
                                ui.close_menu();
                            }
                            if ui.button(crate::i18n::tr("Display Settings…")).clicked() {
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
                                    .rect_stroke(r, 6.0, Stroke::new(1.0_f32, pal.accent));
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
                                if ui.button(crate::i18n::tr("Open")).clicked() {
                                    actions.push(DeskAction::Open(it.path.clone(), it.is_dir));
                                    ui.close_menu();
                                }
                                if !it.is_dir && ui.button(crate::i18n::tr("Show in Delfin")).clicked() {
                                    actions.push(DeskAction::ShowInDelfin(it.path.clone()));
                                    ui.close_menu();
                                }
                                if it.is_dir && ui.button(crate::i18n::tr("Open Terminal Here")).clicked() {
                                    actions.push(DeskAction::Terminal(it.path.clone()));
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui.button(crate::i18n::tr("Move to Trash")).clicked() {
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
                    win::shell_open(&p);
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
                    win::shell_open(u);
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
        self.poll_status();
        self.adopt_tray(ctx);
        if let Ok(list) = self.descriptions.try_recv() {
            for (path, desc) in list {
                for a in self.apps.iter_mut() {
                    if matches!(&a.target, Launch::Path(p) if *p == path) {
                        a.desc = desc.clone();
                    }
                }
            }
        }
        if let Ok(store) = self.store_apps.try_recv() {
            startmenu::merge_store_apps(&mut self.apps, store);
        }

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
        if self.overflow_open {
            self.overflow_viewport(ctx, &pal);
        }
        if let Some(section) = self.quick_open {
            self.quick_viewport(ctx, &pal, section);
        }
        if self.calendar_open {
            self.calendar_viewport(ctx, &pal);
        }
        if self.settings_open {
            self.settings_viewport(ctx, &pal);
        }
        self.balloon_viewport(ctx, &pal);
        self.tooltip_viewport(ctx, &pal);
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
    p.circle_stroke(c, 5.5, Stroke::new(1.2_f32, Color32::WHITE));
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
            Stroke::new(2.0_f32, line),
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

fn clock(ui: &mut Ui, open: bool, pal: &Palette) -> egui::Response {
    let now = chrono::Local::now();
    let (rect, resp) = ui.allocate_exact_size(vec2(78.0, 36.0), Sense::click());
    let p = ui.painter();
    if open {
        p.rect_filled(rect, 4.0, pal.select);
    } else if resp.hovered() {
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

impl Drop for ShellApp {
    fn drop(&mut self) {
        // Never leave windows hidden when the shell exits.
        self.unshelve_all();
    }
}

fn allow_foreground(hwnd: isize) {
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(win::pid_of(hwnd));
    }
}

/// The Delfin mark (same as the launcher button), at any size.
fn paint_logo(p: &egui::Painter, c: egui::Pos2, r: f32, pal: &Palette) {
    p.circle_filled(c, r, pal.accent);
    for i in 0..3 {
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / 3.0;
        p.circle_filled(c + vec2(a.cos(), a.sin()) * r * 0.46, r * 0.2, Color32::WHITE);
    }
    p.circle_stroke(c, r * 0.46, Stroke::new(r * 0.1, Color32::WHITE));
}

/// Small coloured glyph for each launcher category.
fn paint_category(p: &egui::Painter, r: Rect, cat: Category, pal: &Palette) {
    let c = r.center();
    let s = r.width() / 2.0;
    let blue = Color32::from_rgb(0x3d, 0xae, 0xe9);
    match cat {
        Category::All => {
            for (i, j) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                let cell = Rect::from_min_size(r.min + vec2(i * s, j * s), vec2(s, s)).shrink(1.5);
                p.rect_filled(cell, 2.5, blue);
            }
        }
        Category::Office => {
            let page = Rect::from_center_size(c, vec2(s * 1.4, s * 1.9));
            p.rect_filled(page, 2.0, Color32::from_rgb(0x1d, 0x99, 0xf3));
            for k in 0..3 {
                let y = page.top() + s * 0.5 + k as f32 * s * 0.4;
                p.line_segment([pos2(page.left() + 3.0, y), pos2(page.right() - 3.0, y)], Stroke::new(1.2_f32, Color32::WHITE));
            }
        }
        Category::Graphics => {
            p.circle_filled(c, s * 0.95, Color32::from_rgb(0xf6, 0x9a, 0x2e));
            for (k, col) in [(0.0f32, 0xda4453u32), (2.1, 0x27ae60), (4.2, 0x3daee9)] {
                let d = vec2(k.cos(), k.sin()) * s * 0.48;
                p.circle_filled(c + d, s * 0.2, Color32::from_rgb((col >> 16) as u8, (col >> 8) as u8, col as u8));
            }
        }
        Category::Internet => {
            let st = Stroke::new(1.4_f32, Color32::from_rgb(0x1a, 0xbc, 0x9c));
            p.circle_stroke(c, s * 0.9, st);
            p.line_segment([c - vec2(s * 0.9, 0.0), c + vec2(s * 0.9, 0.0)], st);
            p.add(Shape::ellipse_stroke(c, vec2(s * 0.4, s * 0.9), st));
        }
        Category::Multimedia => {
            p.add(Shape::convex_polygon(
                vec![c + vec2(-s * 0.6, -s * 0.8), c + vec2(s * 0.85, 0.0), c + vec2(-s * 0.6, s * 0.8)],
                Color32::from_rgb(0x8e, 0x44, 0xad),
                Stroke::NONE,
            ));
        }
        Category::System => icons::paint_tool(p, r, Tool::Gear, pal.text),
        Category::Games => {
            let body = Rect::from_center_size(c, vec2(s * 1.9, s * 1.1));
            p.rect_filled(body, s * 0.5, Color32::from_rgb(0x27, 0xae, 0x60));
            let st = Stroke::new(1.4_f32, Color32::WHITE);
            let l = c - vec2(s * 0.5, 0.0);
            p.line_segment([l - vec2(s * 0.25, 0.0), l + vec2(s * 0.25, 0.0)], st);
            p.line_segment([l - vec2(0.0, s * 0.25), l + vec2(0.0, s * 0.25)], st);
            p.circle_filled(c + vec2(s * 0.45, -s * 0.12), 1.6, Color32::WHITE);
            p.circle_filled(c + vec2(s * 0.7, s * 0.12), 1.6, Color32::WHITE);
        }
        Category::Recent => {
            let st = Stroke::new(1.4_f32, pal.text);
            p.circle_stroke(c, s * 0.9, st);
            p.line_segment([c, c - vec2(0.0, s * 0.55)], st);
            p.line_segment([c, c + vec2(s * 0.4, s * 0.2)], st);
        }
    }
}
