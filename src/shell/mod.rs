//! Delfin desktop shell (`delfin.exe --shell`): replaces explorer.exe.
//!
//! * Panel at the bottom (like KDE Plasma): launcher button, pinned apps,
//!   task manager with real window icons, clock, show-desktop.
//! * Application launcher (Kickoff-like) with search; Enter also runs commands.
//! * Desktop with your wallpaper and Desktop-folder icons.
//! * Starts your startup apps, owns Win+E / Win+D / Win+R / Alt+F1.
//! * File-manager windows are separate `delfin.exe` processes.

pub(crate) mod audio;
mod desktop;
pub mod install;
mod kbd;
mod media;
mod notify;
pub(crate) mod pins;
pub(crate) mod quick;
mod source;
mod snap;
mod startmenu;
mod startup;
mod tray;
mod usb;
pub(crate) mod win;

use crate::config::Config;

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
/// Tray icons shown directly on the panel; the rest go behind the ^ button.
const TRAY_INLINE: usize = 8;
const BALLOON_SECS: u64 = 7;
const QUICK_W: f32 = 380.0;


pub fn run() -> Result<(), eframe::Error> {
    if !win::single_instance("Local\\DelfinShell") {
        shell_log("another shell is running: exit");
        return Ok(()); // already running in this session
    }
    shell_log("started");
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title(PANEL_TITLE)
            .with_transparent(true)
            .with_app_id("delfin-panel")
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size([1024.0, PANEL_H])
            .with_position([0.0, 0.0]),
        ..Default::default()
    };
    let r = eframe::run_native(
        PANEL_TITLE,
        options,
        Box::new(|cc| Ok(Box::new(ShellApp::new(cc)))),
    );
    shell_log(&format!("ended: {r:?}"));
    r
}

/// One line in %LOCALAPPDATA%\delfin\shell.log: when the shell started and
/// why it stopped (kept short).
pub(crate) fn shell_log(what: &str) {
    use std::io::Write;
    let Some(dir) = dirs::data_local_dir().map(|d| d.join("delfin")) else { return };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("shell.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 64 * 1024) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} pid {}: {what}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"), std::process::id());
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Power {
    Logout,
    Restart,
    Shutdown,
    BackToExplorer,
}

enum PanelAction {
    ToggleMedia,
    /// Bring up the window (and browser tab) that plays.
    GoToMedia,
    Media(media::Cmd),
    ToggleLauncher,
    Activate(isize),
    Minimize(isize),
    Close(isize),
    LaunchPin(usize),
    /// Right-click on a pin: (pin, x, its windows).
    PinMenu(usize, f32, Vec<isize>),
    /// Right-click on a window button.
    TaskMenu(isize, f32),
    ToggleCalendar,
    ToggleNotifications,
    LangMenu(f32),
    UsbMenu(f32),
    Uri(&'static str),
    Tray(isize, u32, Click, f32),
    Unshelve(isize),
    ToggleOverflow,
    ToggleQuick(quick::Section),
    Volume(f32),
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
    /// What was picked on the desktop (its own window and thread, see desktop.rs).
    desk_rx: std::sync::mpsc::Receiver<desktop::DeskAction>,
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
    /// An Ethernet adapter is up and routed.
    wired: bool,
    last_status_poll: Instant,
    store_apps: Receiver<Vec<AppEntry>>,
    // calendar under the clock
    calendar_open: bool,
    calendar_seen_focus: bool,
    calendar_closed_at: Instant,
    // app notifications (notify.rs)
    notifier: notify::Notifier,
    notif_open: bool,
    notif_seen_focus: bool,
    notif_closed_at: Instant,
    /// Do not disturb: notifications only go to the centre.
    dnd: bool,
    /// The notification popped up now, and since when.
    toast: Option<(notify::Toast, Instant)>,
    toast_queue: std::collections::VecDeque<notify::Toast>,
    notif_tex: HashMap<String, TextureHandle>,
    /// Keyboard layouts (language indicator).
    layouts: Vec<kbd::Layout>,
    /// USB drives that can be ejected, and the drive letters they were read for.
    usb_drives: Vec<usb::Drive>,
    usb_mask: u32,
    /// An eject running in the background: (what, result).
    eject_rx: Option<std::sync::mpsc::Receiver<(String, Result<(), String>)>>,
    /// (year, month) shown in the calendar.
    calendar_month: (i32, u32),
    /// Last time we looked for a missing tray (Explorer closed after we started).
    tray_checked: Instant,
    /// Live preview of the task button under the pointer.
    preview: Option<Preview>,
    preview_last_hover: Instant,
    /// Volume pop-up after a volume key: when it was last shown.
    osd: Option<Instant>,
    /// The volume pop-up window is hidden (so it isn't hidden again every frame).
    osd_hidden: bool,
    // what is playing (browser, Spotify, …) and its flyout
    media: media::Media,
    media_open: bool,
    media_seen_focus: bool,
    media_closed_at: Instant,
    /// Left edge of the panel widget, where the flyout opens.
    media_x: f32,
    media_tex: Option<(u64, TextureHandle)>,
    // Delfin settings window (language, updates, shell options)
    settings_open: bool,
    /// Alt+Tab window switcher, while it is on screen.
    switcher: Option<Switcher>,
    updater: crate::update::Updater,
    // shared
    textures: HashMap<String, Option<TextureHandle>>,
    hotkeys: Receiver<win::Hotkey>,
    // pinned apps
    pins: Vec<pins::Pin>,
    pins_mtime: Option<std::time::SystemTime>,
    /// Program of each window (lower case), to group windows under pins.
    exe_of_window: HashMap<isize, Option<String>>,
    /// Right-click menu above the panel.
    panel_menu: Option<PanelMenu>,
    /// Commands picked in menus inside other windows (launcher, desktop).
    menu_cmds: Vec<MenuCmd>,
    /// The shell is ending on purpose (restart after an update).
    quitting: bool,
    /// Windows whose blur (Aero) was set, and to what.
    aero_applied: HashMap<isize, bool>,
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
        theme::apply(&cc.egui_ctx, cfg.dark || cfg.aero);
        spawn_look_waiter(cc.egui_ctx.clone());

        let (tx, rx) = channel();
        win::spawn_hotkeys(tx, cc.egui_ctx.clone());
        win::spawn_window_events(cc.egui_ctx.clone());
        kbd::watch(cc.egui_ctx.clone());
        let (desk_tx, desk_rx) = channel();
        desktop::start(desk_tx, cc.egui_ctx.clone(), cfg.dark);
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
        let app = Self {
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
            desk_rx,
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
            wired: false,
            last_status_poll: long_ago,
            store_apps: startmenu::scan_store_apps(cc.egui_ctx.clone()),
            calendar_open: false,
            calendar_seen_focus: false,
            calendar_closed_at: long_ago,
            notifier: notify::Notifier::start(cc.egui_ctx.clone()),
            notif_open: false,
            notif_seen_focus: false,
            notif_closed_at: long_ago,
            dnd: false,
            toast: None,
            toast_queue: std::collections::VecDeque::new(),
            notif_tex: HashMap::new(),
            layouts: kbd::layouts(),
            usb_drives: usb::drives(),
            usb_mask: 0,
            eject_rx: None,
            calendar_month: (2000, 1),
            tray_checked: long_ago,
            preview: None,
            preview_last_hover: long_ago,
            osd: None,
            osd_hidden: false,
            media: media::Media::start(cc.egui_ctx.clone()),
            media_open: false,
            media_seen_focus: false,
            media_closed_at: long_ago,
            media_x: 0.0,
            media_tex: None,
            settings_open: false,
            switcher: None,
            updater: crate::update::Updater::default(),
            textures: HashMap::new(),
            hotkeys: rx,
            pins: pins::load(),
            pins_mtime: pins::modified(),
            exe_of_window: HashMap::new(),
            panel_menu: None,
            menu_cmds: Vec::new(),
            quitting: false,
            aero_applied: HashMap::new(),
        };
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

    /// Open a link from the shell. `ms-settings:` pages open Delfin's Settings
    /// window (see `win::shell_open`).
    fn open_uri(&mut self, u: &str) {
        win::shell_open(u);
    }

    fn launch(&mut self, l: &Launch) {
        match l {
            Launch::Cmd(c) if c == "delfin" => win::spawn_self(None),
            // Delfin's own Device Manager instead of the slow MMC console.
            Launch::Cmd(c) if is_devmgr_cmd(c) => {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).arg("--devmgr").spawn();
                }
            }
            Launch::Cmd(c) if c.starts_with("ms-settings:") => {
                let c = c.clone();
                self.open_uri(&c);
            }
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
            .with_transparent(true)
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
            .with_transparent(true)
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
                    let mut replaced = crate::taskmgr::replaced();
                    if ui.checkbox(&mut replaced, tr("Replace Windows Task Manager (asks for administrator rights)")).changed() {
                        let _ = crate::taskmgr::replace(replaced);
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
            self.quitting = true;
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
        self.wired = quick::wired_connected();
        win::EXPLORER_PRESENT.store(win::explorer_running(), std::sync::atomic::Ordering::Relaxed);
    }

    fn quick_viewport(&mut self, ctx: &egui::Context, pal: &Palette, section: quick::Section) {
        let screen = Self::screen_points(ctx);
        let builder = ViewportBuilder::default()
            .with_title("Delfin Quick Settings")
            .with_transparent(true)
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
                self.open_uri(uri);
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
        // A window changed (see `win::spawn_window_events`), or the slow safety net.
        let dirty = win::WINDOWS_DIRTY.swap(false, std::sync::atomic::Ordering::Relaxed);
        if !dirty && self.last_poll.elapsed() < Duration::from_secs(5) {
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
        self.exe_of_window.retain(|h, _| merged.iter().any(|t| t.hwnd == *h));
        for t in &merged {
            self.exe_of_window
                .entry(t.hwnd)
                .or_insert_with(|| win::window_exe(t.hwnd).map(|p| p.to_string_lossy().to_lowercase()));
        }
        // Pins changed elsewhere (the file manager can pin programs).
        let m = pins::modified();
        if m != self.pins_mtime {
            self.pins_mtime = m;
            self.pins = pins::load();
        }
        if let Ok(mut w) = win::PANEL_WINDOWS.lock() {
            *w = merged.iter().map(|t| t.hwnd).chain(self.shelved.iter().copied()).collect();
        }
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
        let aero = self.cfg.aero;
        let mut actions: Vec<PanelAction> = Vec::new();
        let mut task_hover: Option<(isize, f32)> = None;
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
                if aero {
                    paint_gloss(ui.painter(), sr, 0.0);
                }
                ui.painter().line_segment(
                    [sr.left_top(), sr.right_top()],
                    Stroke::new(1.0_f32, pal.border),
                );

                ui.horizontal_centered(|ui| {
                    if launcher_button(ui, self.launcher_open, pal).clicked() {
                        actions.push(PanelAction::ToggleLauncher);
                    }
                    ui.add_space(4.0);
                    // Pinned apps; their running windows group under them.
                    let pins = self.pins.clone();
                    let all_tasks = self.tasks.clone();
                    for (pi, pin) in pins.iter().enumerate() {
                        let wins: Vec<&win::TaskWindow> = all_tasks
                            .iter()
                            .filter(|t| self.exe_of_window.get(&t.hwnd).and_then(|e| e.as_deref()).is_some_and(|e| pin.owns(e)))
                            .collect();
                        let active = wins.iter().any(|t| t.hwnd == self.last_fg && !t.minimized);
                        let r = self.pin_button(ui, ctx, pin, if active { 2 } else { (!wins.is_empty()) as u8 }, pal);
                        if r.clicked() {
                            actions.push(match wins.len() {
                                0 => PanelAction::LaunchPin(pi),
                                1 if active => PanelAction::Minimize(wins[0].hwnd),
                                1 => PanelAction::Activate(wins[0].hwnd),
                                // Several windows: go through them in turn.
                                n => {
                                    let cur = wins.iter().position(|t| t.hwnd == self.last_fg);
                                    PanelAction::Activate(wins[cur.map(|c| (c + 1) % n).unwrap_or(0)].hwnd)
                                }
                            });
                        }
                        if r.middle_clicked() {
                            actions.push(PanelAction::LaunchPin(pi));
                        }
                        if r.secondary_clicked() {
                            actions.push(PanelAction::PinMenu(pi, r.rect.left(), wins.iter().map(|t| t.hwnd).collect()));
                        }
                        if r.hovered() {
                            match wins.first() {
                                Some(t) => task_hover = Some((t.hwnd, r.rect.center().x)),
                                None => hovered = Some((pin.name.clone(), r.rect.center().x)),
                            }
                        }
                    }
                    ui.separator();

                    // What is playing, with its own controls.
                    if let Some(track) = self.media.current() {
                        let (r, _) = ui.allocate_exact_size(vec2(340.0, 36.0), Sense::hover());
                        self.media_x = r.left();
                        if let Some(t) = self.media_widget(ui, ctx, r, &track, pal, &mut actions) {
                            hovered = Some((t, r.left() + 120.0));
                        }
                        ui.separator();
                    } else if self.media_open {
                        self.media_open = false;
                    }

                    let right_w = 200.0;
                    let avail = (ui.available_width() - right_w).max(40.0);
                    // Windows of pinned apps are shown on their pin instead.
                    let tasks: Vec<win::TaskWindow> = self
                        .tasks
                        .iter()
                        .filter(|t| {
                            let exe = self.exe_of_window.get(&t.hwnd).and_then(|e| e.as_deref());
                            !exe.is_some_and(|e| self.pins.iter().any(|p| p.owns(e)))
                        })
                        .cloned()
                        .collect();
                    let n = tasks.len().max(1) as f32;
                    let bw = ((avail - 4.0 * n) / n).clamp(40.0, 220.0);
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
                        if r.secondary_clicked() {
                            actions.push(PanelAction::TaskMenu(t.hwnd, r.rect.left()));
                        }
                        if r.hovered() {
                            // A live preview instead of a text tooltip.
                            task_hover = Some((t.hwnd, r.rect.center().x));
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
                        // Notifications: a bell with how many are waiting.
                        {
                            let n = self.notifier.count();
                            let (r, resp) = ui.allocate_exact_size(vec2(30.0, 36.0), Sense::click());
                            let p = ui.painter();
                            if self.notif_open {
                                p.rect_filled(r, 4.0, pal.select);
                            } else if resp.hovered() {
                                p.rect_filled(r, 4.0, pal.hover);
                            }
                            paint_bell(p, Rect::from_center_size(r.center(), vec2(18.0, 18.0)), pal.text, self.dnd);
                            if n > 0 {
                                let b = pos2(r.right() - 8.0, r.top() + 9.0);
                                p.circle_filled(b, 7.0, pal.accent);
                                p.text(b, Align2::CENTER_CENTER, if n > 9 { "9+".to_string() } else { n.to_string() }, FontId::proportional(9.5), Color32::WHITE);
                            }
                            if resp.hovered() {
                                let tip = if n == 0 { crate::i18n::tr("No new notifications").to_string() } else { crate::i18n::trf("Notifications: {}", &[&n]) };
                                hovered = Some((tip, r.center().x));
                            }
                            if resp.clicked() {
                                actions.push(PanelAction::ToggleNotifications);
                            }
                        }
                        // Volume and network: two separate buttons, each with its own
                        // flyout (layout is right-to-left, so volume is drawn first).
                        for section in [quick::Section::Sound, quick::Section::Network] {
                            let (r, resp) = ui.allocate_exact_size(vec2(28.0, 36.0), Sense::click());
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
                                    // Same visual size as the 18 px tray icons next to it.
                                    quick::paint_speaker(p, Rect::from_center_size(r.center(), vec2(24.0, 24.0)), pal.text, level);
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
                                    quick::paint_network(p, Rect::from_center_size(r.center(), vec2(22.0, 22.0)), pal, self.wifi_sig, self.wired);
                                    match self.qp.connected_ssid() {
                                        Some(ssid) => crate::i18n::trf("Wi-Fi: {}", &[&ssid]),
                                        None if self.wired => crate::i18n::tr("Ethernet: connected").to_string(),
                                        None => crate::i18n::tr("No network connection").to_string(),
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
                        // Keyboard language (only with more than one layout).
                        if self.layouts.len() > 1 {
                            if let Some(cur) = kbd::current() {
                                let (r, resp) = ui.allocate_exact_size(vec2(32.0, 36.0), Sense::click());
                                if resp.hovered() {
                                    ui.painter().rect_filled(r, 4.0, pal.hover);
                                }
                                ui.painter().text(r.center(), Align2::CENTER_CENTER, &cur.short, FontId::proportional(12.5), pal.text);
                                if resp.hovered() {
                                    hovered = Some((crate::i18n::trf("{}\nWin+Space switches", &[&cur.name]), r.center().x));
                                }
                                if resp.clicked() {
                                    actions.push(PanelAction::LangMenu(r.left()));
                                }
                            }
                        }
                        // Safely remove hardware (only while a USB drive is in).
                        if !self.usb_drives.is_empty() {
                            let (r, resp) = ui.allocate_exact_size(vec2(28.0, 36.0), Sense::click());
                            if resp.hovered() {
                                ui.painter().rect_filled(r, 4.0, pal.hover);
                            }
                            paint_usb(ui.painter(), Rect::from_center_size(r.center(), vec2(18.0, 18.0)), pal.text);
                            if resp.hovered() {
                                hovered = Some((crate::i18n::tr("Safely remove hardware").to_string(), r.center().x));
                            }
                            if resp.clicked() || resp.secondary_clicked() {
                                actions.push(PanelAction::UsbMenu(r.left()));
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

        self.update_preview(task_hover);
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
                    self.preview = None;
                    win::activate(h);
                    self.last_fg = h;
                    self.hidden_for_desktop.clear();
                }
                PanelAction::Minimize(h) => {
                    self.preview = None;
                    win::minimize(h);
                    self.last_fg = 0;
                }
                PanelAction::Close(h) => win::close(h),
                PanelAction::LaunchPin(i) => {
                    if let Some(p) = self.pins.get(i).cloned() {
                        self.launch_pin(&p);
                    }
                }
                PanelAction::PinMenu(i, x, wins) => self.open_pin_menu(i, x, wins),
                PanelAction::TaskMenu(h, x) => self.open_task_menu(h, x),
                PanelAction::ToggleMedia => {
                    if self.media_open {
                        self.media_open = false;
                        self.media_closed_at = Instant::now();
                    } else if self.media_closed_at.elapsed() > Duration::from_millis(300) {
                        self.open_quick(None);
                        self.open_launcher(false);
                        self.calendar_open = false;
                        self.media_open = true;
                        self.media_seen_focus = false;
                    }
                }
                PanelAction::GoToMedia => {
                    if !self.go_to_player() {
                        // Nowhere to go (the player has no window): show the player instead.
                        self.media_open = true;
                        self.media_seen_focus = false;
                    }
                }
                PanelAction::Media(c) => self.media.send(c),
                PanelAction::LangMenu(x) => {
                    let items = self.layouts.iter().map(|l| (format!("{}  ·  {}", l.short, l.name), MenuCmd::Layout(l.hkl))).collect();
                    self.show_panel_menu(x, crate::i18n::tr("Keyboard layout").to_string(), items);
                }
                PanelAction::UsbMenu(x) => {
                    let items = self
                        .usb_drives
                        .iter()
                        .map(|d| {
                            let name = if d.label.is_empty() { crate::i18n::tr("USB drive").to_string() } else { d.label.clone() };
                            (crate::i18n::trf("Eject {} ({}:)", &[&name, &d.letter]), MenuCmd::Eject(d.devinst, format!("{name} ({}:)", d.letter)))
                        })
                        .collect();
                    self.show_panel_menu(x, crate::i18n::tr("Safely remove hardware").to_string(), items);
                }
                PanelAction::ToggleNotifications => {
                    if self.notif_open {
                        self.notif_open = false;
                        self.notif_closed_at = Instant::now();
                    } else if self.notif_closed_at.elapsed() > Duration::from_millis(300) {
                        self.open_quick(None);
                        self.open_launcher(false);
                        self.calendar_open = false;
                        self.notif_open = true;
                        self.notif_seen_focus = false;
                    }
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
                    self.open_uri(u);
                }
            }
        }
    }

    // ------------------------------------------------------------ launcher

    fn launcher_viewport(&mut self, ctx: &egui::Context, pal: &Palette) {
        let aero = self.cfg.aero;
        let screen = Self::screen_points(ctx);
        let builder = ViewportBuilder::default()
            .with_title("Delfin Launcher")
            .with_transparent(true)
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
                        if aero {
                            paint_gloss(ui.painter(), ctx.screen_rect(), 8.0);
                        }
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
                                                    if resp.secondary_clicked() {
                                                        self.launcher_sel = k;
                                                    }
                                                    resp.context_menu(|ui| {
                                                        for (label, cmd) in self.app_menu_items(&app.name, &app.target) {
                                                            if ui.button(label).clicked() {
                                                                self.menu_cmds.push(cmd);
ui.ctx().request_repaint();
                                                                ui.close_menu();
                                                            }
                                                        }
                                                    });
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
                        let links: [(Option<Tool>, &str); 8] = [
                            (Some(Tool::Desktop), crate::i18n::tr("This PC")),
                            (None, crate::i18n::tr("Documents")),
                            (None, crate::i18n::tr("Downloads")),
                            (Some(Tool::Gear), crate::i18n::tr("Settings")),
                            (Some(Tool::Details), crate::i18n::tr("Task Manager")),
                            (Some(Tool::Chip), crate::i18n::tr("Device Manager")),
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
                                None if k == 7 => paint_logo(ui.painter(), ir.center(), 9.0, pal),
                                None => icons::paint_folder(ui.painter(), ir, icons::FOLDER),
                            }
                            ui.painter().text(pos2(r.left() + 38.0, r.center().y), Align2::LEFT_CENTER, *label, FontId::proportional(13.5), pal.text);
                            // This PC › Properties: the full specification, like in Windows.
                            if k == 0 {
                                resp.context_menu(|ui| {
                                    if ui.button(crate::i18n::tr("Open")).clicked() {
                                        launch = Some((Launch::Cmd("delfin".into()), None));
                                        ui.close_menu();
                                    }
                                    if ui.button(crate::i18n::tr("Properties")).clicked() {
                                        launch = Some((Launch::Cmd("ms-settings:about".into()), None));
                                        ui.close_menu();
                                    }
                                });
                            }
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
                                    5 => launch = Some((Launch::Cmd("devmgmt.msc".into()), None)),
                                    6 => {
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
            .with_transparent(true)
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
            .with_transparent(true)
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
            .with_transparent(true)
            .with_visible(false)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_active(false)
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

    // ------------------------------------------------------------ look (dark mode, Aero)

    /// The colours of every shell surface: Aero glass or the normal theme.
    fn palette(&self) -> Palette {
        if self.cfg.aero {
            theme::glass_palette(self.cfg.aero_color, self.cfg.aero_strength)
        } else {
            theme::palette(self.cfg.dark)
        }
    }

    /// Settings changed the look: read it again and apply it now.
    fn reload_look(&mut self, ctx: &egui::Context) {
        let c = Config::load();
        self.cfg.dark = c.dark;
        self.cfg.aero = c.aero;
        self.cfg.aero_color = c.aero_color;
        self.cfg.aero_strength = c.aero_strength;
        theme::apply(ctx, self.cfg.dark || self.cfg.aero);
        self.aero_applied.clear();
        ctx.request_repaint();
    }

    /// Turn the blur behind each shell window on or off to match the setting
    /// (new pop-ups get it as they appear).
    fn apply_aero(&mut self) {
        let want = self.cfg.aero;
        if !want && self.aero_applied.is_empty() {
            return;
        }
        let wins = win::own_windows();
        for &h in &wins {
            if self.aero_applied.get(&h) != Some(&want) {
                win::set_blur(h, want);
                self.aero_applied.insert(h, want);
            }
        }
        self.aero_applied.retain(|h, on| *on && wins.contains(h));
    }

    // ------------------------------------------------------------ notifications (see notify.rs)

    /// Queue newly arrived notifications to pop up (not with Do not disturb,
    /// nor while Explorer shows them itself).
    fn notifications_tick(&mut self, ctx: &egui::Context) {
        let fresh = self.notifier.take_fresh();
        let explorer = win::EXPLORER_PRESENT.load(std::sync::atomic::Ordering::Relaxed);
        if !self.dnd && !explorer {
            self.toast_queue.extend(fresh);
        }
        if self.toast.as_ref().is_some_and(|(_, since)| since.elapsed() > Duration::from_secs(TOAST_SECS)) {
            self.toast = None;
        }
        if self.toast.is_none() {
            if let Some(t) = self.toast_queue.pop_front() {
                self.toast = Some((t, Instant::now()));
            }
        }
        if let Some((_, since)) = &self.toast {
            ctx.request_repaint_after(Duration::from_secs(TOAST_SECS).saturating_sub(since.elapsed()) + Duration::from_millis(50));
        }
    }

    fn notif_texture(&mut self, ctx: &egui::Context, t: &notify::Toast) -> Option<TextureHandle> {
        if let Some(tex) = self.notif_tex.get(&t.aumid) {
            return Some(tex.clone());
        }
        let icon = t.icon.as_ref()?;
        let tex = ctx.load_texture(
            format!("notif:{}", t.aumid),
            egui::ColorImage::from_rgba_unmultiplied([icon.0, icon.1], &icon.2),
            egui::TextureOptions::LINEAR,
        );
        self.notif_tex.insert(t.aumid.clone(), tex.clone());
        Some(tex)
    }

    /// Bring up the app a notification came from (Windows can't tell us what
    /// the notification itself would have opened), and drop the notification.
    fn open_notification(&mut self, t: &notify::Toast) {
        self.notifier.remove(t.id);
        let candidates: Vec<source::Candidate> = self
            .tasks
            .iter()
            .map(|w| source::Candidate {
                hwnd: w.hwnd,
                title: w.title.clone(),
                exe: self.exe_of_window.get(&w.hwnd).cloned().flatten().unwrap_or_default(),
            })
            .collect();
        if let Some(w) = source::player_windows(&t.aumid, "", candidates).first() {
            win::switch_to(w.hwnd);
        } else if t.aumid.contains('!') {
            win::activate_packaged(&t.aumid);
        }
    }

    /// One notification card. Returns (opened, dismissed).
    fn notif_card(&mut self, ui: &mut Ui, ctx: &egui::Context, t: &notify::Toast, pal: &Palette, width: f32) -> (bool, bool) {
        let body_lines = t.body.lines().count().clamp(0, 3) as f32;
        let h = 58.0 + body_lines * 17.0;
        let (r, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
        ui.painter().rect(r, 8.0, if resp.hovered() { pal.hover } else { pal.view }, Stroke::new(1.0_f32, pal.border));
        let ir = Rect::from_min_size(r.min + vec2(12.0, 12.0), vec2(18.0, 18.0));
        match self.notif_texture(ctx, t) {
            Some(tex) => {
                ui.painter().image(tex.id(), ir, uv_full(), Color32::WHITE);
            }
            None => {
                ui.painter().circle_filled(ir.center(), 9.0, pal.accent);
                let initial: String = t.app.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                ui.painter().text(ir.center(), Align2::CENTER_CENTER, initial, FontId::proportional(11.0), Color32::WHITE);
            }
        }
        let p = ui.painter().with_clip_rect(r.shrink(2.0));
        p.text(pos2(ir.right() + 8.0, ir.center().y), Align2::LEFT_CENTER, &t.app, FontId::proportional(12.0), pal.dim);
        p.text(pos2(r.right() - 34.0, ir.center().y), Align2::RIGHT_CENTER, notify::ago(t.time), FontId::proportional(11.5), pal.dim);
        let text_w = r.width() - 24.0;
        let title = label_wrapped(ui, &t.title, text_w, 14.0, pal.text, 1);
        p.galley(pos2(r.left() + 12.0, r.top() + 36.0), title, pal.text);
        if body_lines > 0.0 {
            let body = label_wrapped(ui, &t.body, text_w, 13.0, pal.dim, 3);
            p.galley(pos2(r.left() + 12.0, r.top() + 56.0), body, pal.dim);
        }
        // Dismiss (x)
        let xr = Rect::from_center_size(pos2(r.right() - 16.0, ir.center().y), vec2(22.0, 22.0));
        let xresp = ui.interact(xr, ui.id().with(("notif-x", t.id)), Sense::click());
        if xresp.hovered() {
            ui.painter().rect_filled(xr, 4.0, pal.hover);
        }
        icons::paint_tool(ui.painter(), xr, Tool::Close, pal.dim);
        (resp.clicked() && !xresp.hovered(), xresp.clicked())
    }

    /// The notification centre above the bell.
    fn notif_viewport(&mut self, ctx: &egui::Context, pal: &Palette) {
        const W: f32 = 380.0;
        const H: f32 = 540.0;
        let screen = Self::screen_points(ctx);
        let builder = ViewportBuilder::default()
            .with_title("Delfin Notifications")
            .with_transparent(true)
            .with_visible(false)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size([W, H])
            .with_position([screen.x - W - 8.0, screen.y - self.lift - PANEL_H - H - 8.0]);
        let mut close = false;
        let mut open: Option<notify::Toast> = None;
        let mut dismiss: Vec<u32> = Vec::new();
        let mut clear = false;
        let list = self.notifier.all();
        let allowed = self.notifier.allowed();
        ctx.show_viewport_immediate(ViewportId::from_hash_of("delfin_notifications"), builder, |ctx, _| {
            if !win::reveal_popup("Delfin Notifications", true) {
                ctx.request_repaint();
            }
            let focused = ctx.input(|i| i.viewport().focused);
            if focused == Some(true) {
                self.notif_seen_focus = true;
            }
            if self.notif_seen_focus && focused == Some(false) {
                close = true;
            }
            if !self.notif_seen_focus {
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
            if ctx.input(|i| i.key_pressed(Key::Escape) || i.viewport().close_requested()) {
                close = true;
            }
            egui::CentralPanel::default()
                .frame(Frame::none().fill(pal.window).stroke(Stroke::new(1.0_f32, pal.border)).inner_margin(Margin::same(14.0)))
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(crate::i18n::tr("Notifications")).size(16.0).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if !list.is_empty() && ui.button(crate::i18n::tr("Clear all")).clicked() {
                                clear = true;
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let mut dnd = self.dnd;
                        if quick::toggle(ui, &mut dnd, pal).changed() {
                            self.dnd = dnd;
                        }
                        ui.label(crate::i18n::tr("Do not disturb"));
                    });
                    ui.add_space(8.0);
                    let w = ui.available_width();
                    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        if !allowed {
                            ui.label(RichText::new(crate::i18n::tr("Windows does not let Delfin read notifications. Allow it in Windows privacy settings (Notifications).")).color(pal.dim));
                        } else if list.is_empty() {
                            ui.add_space(40.0);
                            ui.vertical_centered(|ui| {
                                ui.label(RichText::new(crate::i18n::tr("No new notifications")).color(pal.dim));
                            });
                        }
                        for t in &list {
                            let (o, d) = self.notif_card(ui, ctx, t, pal, w - 8.0);
                            if o {
                                open = Some(t.clone());
                            }
                            if d {
                                dismiss.push(t.id);
                            }
                            ui.add_space(6.0);
                        }
                    });
                });
        });
        for id in dismiss {
            self.notifier.remove(id);
        }
        if clear {
            self.notifier.clear();
        }
        if let Some(t) = open {
            self.open_notification(&t);
            close = true;
        }
        if close {
            self.notif_open = false;
            self.notif_closed_at = Instant::now();
        }
    }

    /// A new notification, popped up in the bottom-right corner for a few seconds.
    fn toast_viewport(&mut self, ctx: &egui::Context, pal: &Palette) {
        let Some((t, _since)) = self.toast.clone() else { return };
        const W: f32 = 364.0;
        let h = 74.0 + t.body.lines().count().clamp(0, 3) as f32 * 17.0;
        let screen = Self::screen_points(ctx);
        let builder = ViewportBuilder::default()
            .with_title("Delfin Toast")
            .with_transparent(true)
            .with_visible(false)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_active(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size([W, h])
            .with_position([screen.x - W - 12.0, screen.y - self.lift - PANEL_H - h - 12.0]);
        let mut open = false;
        let mut dismiss = false;
        let mut hovered = false;
        ctx.show_viewport_immediate(ViewportId::from_hash_of("delfin_toast"), builder, |ctx, _| {
            // Never takes the focus from what you are doing.
            if !win::reveal_popup("Delfin Toast", false) {
                ctx.request_repaint();
            }
            egui::CentralPanel::default()
                .frame(Frame::none().fill(pal.window).stroke(Stroke::new(1.0_f32, pal.border)).inner_margin(Margin::same(6.0)))
                .show(ctx, |ui| {
                    let w = ui.available_width();
                    let (o, d) = self.notif_card(ui, ctx, &t, pal, w);
                    open = o;
                    dismiss = d;
                    hovered = ui.rect_contains_pointer(ui.max_rect());
                });
        });
        if open {
            self.toast = None;
            self.open_notification(&t);
        } else if dismiss {
            // Gone from the screen, still in the centre.
            self.toast = None;
        } else if hovered {
            // Stays while the pointer rests on it.
            self.toast = Some((t, Instant::now()));
            ctx.request_repaint_after(Duration::from_millis(300));
        }
    }

    // ------------------------------------------------------------ desktop

    /// Carry out what was picked on the desktop (see `desktop.rs`).
    fn desk_actions(&mut self) {
        while let Ok(a) = self.desk_rx.try_recv() {
            match a {
                desktop::DeskAction::Open(p, true) => win::spawn_self(Some(&p)),
                desktop::DeskAction::Open(p, false) => win::shell_open(&p),
                desktop::DeskAction::ShowInDelfin(p) => {
                    if let Some(parent) = p.parent() {
                        win::spawn_self(Some(parent));
                    }
                }
                desktop::DeskAction::Terminal(p) => {
                    let _ = crate::platform::open_terminal(&p);
                }
                desktop::DeskAction::Trash(p) => {
                    let _ = trash::delete(&p);
                    desktop::refresh();
                }
                desktop::DeskAction::Pin(name, p) => self.menu_cmds.push(MenuCmd::Pin(pins::Pin::new(name, Launch::Path(p)))),
                desktop::DeskAction::Unpin(p) => self.menu_cmds.push(MenuCmd::Unpin(Launch::Path(p))),
                desktop::DeskAction::Props(p) => self.menu_cmds.push(MenuCmd::Props(p)),
                desktop::DeskAction::Uri(u) => self.open_uri(u),
            }
        }
    }
}

impl eframe::App for ShellApp {
    /// See-through windows (Aero glass); every surface paints its own background.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // The shell only ends when it decides to (update restart); a WM_CLOSE
        // from elsewhere would otherwise take the whole desktop away.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            shell_log("close request ignored");
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        }
        if LOOK_CHANGED.swap(false, std::sync::atomic::Ordering::Relaxed) {
            self.reload_look(ctx);
        }
        let pal = self.palette();
        self.apply_aero();
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
                win::Hotkey::Switch(key) => self.switch_key(key),
                win::Hotkey::Media(key) => self.media.send(match key {
                    win::MediaKey::Next => media::Cmd::Next,
                    win::MediaKey::Prev => media::Cmd::Prev,
                    win::MediaKey::PlayPause => media::Cmd::TogglePlay,
                    win::MediaKey::Stop => media::Cmd::Stop,
                }),
                win::Hotkey::Volume(step) => {
                    if step == 0 {
                        self.qp.toggle_mute();
                    } else {
                        // 2 % per step, like Windows.
                        self.qp.nudge_volume(step as f32 * 0.02);
                    }
                    self.vol_state = self.qp.volume_state();
                    self.osd = Some(Instant::now());
                }
                win::Hotkey::TaskManager => {
                    if let Ok(me) = std::env::current_exe() {
                        let _ = std::process::Command::new(me).arg("--taskmgr").spawn();
                    }
                }
                win::Hotkey::SystemInfo => self.open_uri("ms-settings:about"),
                win::Hotkey::Snap(d) => snap::key(d),
                win::Hotkey::Settings => self.open_uri("ms-settings:"),
                win::Hotkey::QuickSettings => {
                    let open = self.quick_open.is_none();
                    self.open_quick(open.then_some(quick::Section::Sound));
                }
                win::Hotkey::Notifications => {
                    if self.notif_open {
                        self.notif_open = false;
                        self.notif_closed_at = Instant::now();
                    } else {
                        self.open_quick(None);
                        self.open_launcher(false);
                        self.calendar_open = false;
                        self.notif_open = true;
                        self.notif_seen_focus = false;
                    }
                }
                win::Hotkey::PowerMenu => self.open_power_menu(),
                win::Hotkey::Pin(i) => {
                    if let Some(p) = self.pins.get(i).cloned() {
                        // Like a click on the pin: switch to its window, or start it.
                        let wins: Vec<isize> = self
                            .tasks
                            .iter()
                            .filter(|t| self.exe_of_window.get(&t.hwnd).and_then(|e| e.as_deref()).is_some_and(|e| p.owns(e)))
                            .map(|t| t.hwnd)
                            .collect();
                        match wins.first() {
                            Some(&h) => win::switch_to(h),
                            None => self.launch_pin(&p),
                        }
                    }
                }
            }
        }

        self.desk_actions();
        self.panel_ui(ctx, &pal);
        if self.switcher.is_some() {
            self.switcher_viewport(ctx, &pal);
        }
        if self.launcher_open {
            self.launcher_viewport(ctx, &pal);
        }
        if self.overflow_open {
            self.overflow_viewport(ctx, &pal);
        }
        if let Some(section) = self.quick_open {
            self.quick_viewport(ctx, &pal, section);
        }
        self.notifications_tick(ctx);
        self.usb_tick();
        if self.notif_open {
            self.notif_viewport(ctx, &pal);
        }
        self.toast_viewport(ctx, &pal);
        if self.calendar_open {
            self.calendar_viewport(ctx, &pal);
        }
        if self.media_open {
            self.media_viewport(ctx, &pal);
        }
        if self.panel_menu.is_some() {
            self.panel_menu_viewport(ctx, &pal);
        }
        for c in std::mem::take(&mut self.menu_cmds) {
            self.run_menu_cmd(c);
        }
        if self.preview.is_some() {
            self.preview_viewport(ctx, &pal);
        }
        // Always there (hidden when idle): creating a window takes about a
        // second, too slow for a pop-up that lives 1.5 s.
        self.osd_viewport(ctx, &pal);
        if self.settings_open {
            self.settings_viewport(ctx, &pal);
        }
        self.balloon_viewport(ctx, &pal);
        self.tooltip_viewport(ctx, &pal);
        // Window changes wake the panel by themselves (win::spawn_window_events).
        // Otherwise: the clock's next minute, or a slow tick for volume, Wi-Fi
        // and the desktop folder.
        let now = chrono::Local::now();
        let to_minute = Duration::from_millis(60_000 - (now.timestamp_millis().rem_euclid(60_000)) as u64 + 20);
        ctx.request_repaint_after(to_minute.min(Duration::from_secs(5)));
    }
}

// ----------------------------------------------------------------- widgets

fn uv_full() -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
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

// ----------------------------------------------------------------- Alt+Tab

const SW_CARD: egui::Vec2 = egui::Vec2::new(236.0, 188.0);
const SW_GAP: f32 = 14.0;
const SW_PAD: f32 = 22.0;
const SW_TITLE: &str = "Delfin Switcher";

/// The Alt+Tab window switcher: one card per window with a live DWM preview.
struct Switcher {
    items: Vec<win::TaskWindow>,
    sel: usize,
    cols: usize,
    opened: Instant,
    /// DWM thumbnail per item, registered once our window exists.
    thumbs: Vec<Option<isize>>,
    registered: bool,
}

impl Drop for Switcher {
    fn drop(&mut self) {
        for t in self.thumbs.iter().flatten() {
            win::thumb_unregister(*t);
        }
        win::SWITCHER_OPEN.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t)
}

impl ShellApp {
    fn switch_key(&mut self, key: win::SwitchKey) {
        use win::SwitchKey as K;
        let Some(sw) = &mut self.switcher else {
            // Closed: Alt+Tab opens it on the previous window (Shift: the last).
            if !matches!(key, K::Next | K::Prev) {
                return;
            }
            let items = win::task_windows(self.own_pid);
            if items.is_empty() {
                return;
            }
            let n = items.len();
            let sel = match key {
                K::Prev => n - 1,
                _ => (n > 1) as usize,
            };
            self.open_launcher(false);
            self.open_quick(None);
            self.calendar_open = false;
            win::mask_alt_menu();
            win::SWITCHER_OPEN.store(true, std::sync::atomic::Ordering::Relaxed);
            self.switcher = Some(Switcher {
                thumbs: vec![None; n],
                items,
                sel,
                cols: 1,
                opened: Instant::now(),
                registered: false,
            });
            return;
        };
        let n = sw.items.len();
        match key {
            K::Next => sw.sel = (sw.sel + 1) % n,
            K::Prev => sw.sel = (sw.sel + n - 1) % n,
            K::Up if sw.sel >= sw.cols => sw.sel -= sw.cols,
            K::Down if sw.sel + sw.cols < n => sw.sel += sw.cols,
            K::Up | K::Down => {}
            K::Confirm => {
                let h = sw.items[sw.sel].hwnd;
                self.switcher = None;
                win::switch_to(h);
                self.last_fg = h;
            }
            K::Cancel => self.switcher = None,
        }
    }

    fn switcher_viewport(&mut self, ctx: &egui::Context, pal: &Palette) {
        let screen = Self::screen_points(ctx);
        let ppp = ctx.pixels_per_point();
        let Some(sw) = &mut self.switcher else { return };
        let n = sw.items.len();
        let max_cols = (((screen.x * 0.9 - 2.0 * SW_PAD + SW_GAP) / (SW_CARD.x + SW_GAP)).floor() as usize).max(1);
        sw.cols = n.min(max_cols).max(1);
        let rows = n.div_ceil(sw.cols);
        let size = vec2(
            2.0 * SW_PAD + sw.cols as f32 * (SW_CARD.x + SW_GAP) - SW_GAP,
            2.0 * SW_PAD + rows as f32 * (SW_CARD.y + SW_GAP) - SW_GAP,
        );
        let builder = ViewportBuilder::default()
            .with_title(SW_TITLE)
            .with_transparent(true)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_visible(false)
            .with_inner_size(size)
            .with_position([(screen.x - size.x) / 2.0, (screen.y - size.y) / 2.0]);

        let mut clicked: Option<usize> = None;
        ctx.show_viewport_immediate(ViewportId::from_hash_of("delfin_switcher"), builder, |ctx, _| {
            // Shown without taking the focus: the keyboard hook drives it.
            if !win::reveal_popup(SW_TITLE, false) {
                ctx.request_repaint();
                return;
            }
            let Some(sw) = &mut self.switcher else { return };
            if !sw.registered {
                if let Some(dest) = win::find_window(SW_TITLE) {
                    for (i, it) in sw.items.iter().enumerate() {
                        sw.thumbs[i] = win::thumb_register(dest, it.hwnd);
                    }
                    sw.registered = true;
                }
            }
            // Fade and grow in over 160 ms.
            let t = ease_out(sw.opened.elapsed().as_secs_f32() / 0.16);
            if t < 1.0 {
                ctx.request_repaint();
            }
            let items = sw.items.clone();
            let (sel, cols) = (sw.sel, sw.cols);
            let thumbs = sw.thumbs.clone();

            egui::CentralPanel::default()
                .frame(Frame::none().fill(pal.window.gamma_multiply(0.94 * t + 0.06)))
                .show(ctx, |ui| {
                    let origin = ui.max_rect().min + vec2(SW_PAD, SW_PAD);
                    let card_at = |i: usize| {
                        let (r, c) = (i / cols, i % cols);
                        Rect::from_min_size(
                            origin + vec2(c as f32 * (SW_CARD.x + SW_GAP), r as f32 * (SW_CARD.y + SW_GAP)),
                            SW_CARD,
                        )
                    };
                    let grow = 0.92 + 0.08 * t;

                    // The highlight glides to the selected card.
                    let target = card_at(sel);
                    let hx = ctx.animate_value_with_time(egui::Id::new("sw_hx"), target.left(), 0.12);
                    let hy = ctx.animate_value_with_time(egui::Id::new("sw_hy"), target.top(), 0.12);
                    if (hx - target.left()).abs() > 0.5 || (hy - target.top()).abs() > 0.5 {
                        ctx.request_repaint();
                    }
                    let hl = Rect::from_min_size(pos2(hx, hy), SW_CARD).expand(4.0);
                    ui.painter().rect(hl, 10.0, pal.select, Stroke::new(2.0_f32, pal.accent));

                    for (i, it) in items.iter().enumerate() {
                        let full = card_at(i);
                        let card = Rect::from_center_size(full.center(), full.size() * grow);
                        let resp = ui.interact(card, ui.id().with(("sw", i)), Sense::click());
                        if i != sel {
                            let bg = if resp.hovered() { pal.hover } else { pal.header.gamma_multiply(0.6) };
                            ui.painter().rect_filled(card, 8.0, bg);
                        }
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                        // Preview area and caption.
                        let area = Rect::from_min_max(card.min + vec2(10.0, 10.0), pos2(card.right() - 10.0, card.bottom() - 36.0));
                        let icon = self.tex_window(ctx, it.hwnd);
                        let thumb = thumbs.get(i).copied().flatten();
                        let live = !it.minimized && thumb.is_some();
                        if let Some(id) = thumb {
                            let (sw_px, sh_px) = win::thumb_source_size(id);
                            let fit = if sw_px > 0 && sh_px > 0 {
                                // Fit inside the area, never larger than the window itself.
                                let s = (area.width() * ppp / sw_px as f32).min(area.height() * ppp / sh_px as f32).min(1.0);
                                Rect::from_center_size(area.center(), vec2(sw_px as f32 * s / ppp, sh_px as f32 * s / ppp))
                            } else {
                                area
                            };
                            win::thumb_update(
                                id,
                                [
                                    (fit.left() * ppp) as i32,
                                    (fit.top() * ppp) as i32,
                                    (fit.right() * ppp) as i32,
                                    (fit.bottom() * ppp) as i32,
                                ],
                                (255.0 * t) as u8,
                                live,
                            );
                        }
                        if !live {
                            // Minimised (or no preview): the big icon instead.
                            let ir = Rect::from_center_size(area.center(), vec2(48.0, 48.0));
                            match &icon {
                                Some(tx) => {
                                    ui.painter().image(tx.id(), ir, uv_full(), Color32::WHITE.gamma_multiply(t));
                                }
                                None => icons::paint_tool(ui.painter(), ir, Tool::Desktop, pal.dim),
                            }
                        }
                        let cap = Rect::from_min_max(pos2(card.left() + 10.0, card.bottom() - 30.0), pos2(card.right() - 10.0, card.bottom() - 6.0));
                        let ir = Rect::from_min_size(pos2(cap.left(), cap.center().y - 9.0), vec2(18.0, 18.0));
                        if let Some(tx) = &icon {
                            ui.painter().image(tx.id(), ir, uv_full(), Color32::WHITE.gamma_multiply(t));
                        }
                        let p = ui.painter().with_clip_rect(cap);
                        p.text(
                            pos2(ir.right() + 6.0, cap.center().y),
                            Align2::LEFT_CENTER,
                            &it.title,
                            FontId::proportional(13.0),
                            if i == sel { pal.text } else { pal.dim }.gamma_multiply(t),
                        );
                    }
                });
        });
        if let Some(i) = clicked {
            if let Some(sw) = &mut self.switcher {
                sw.sel = i;
            }
            self.switch_key(win::SwitchKey::Confirm);
        }
    }
}

// ----------------------------------------------------------------- media

const MEDIA_W: f32 = 420.0;
const MEDIA_H: f32 = 330.0;

/// Small media glyphs: 0 prev, 1 play, 2 pause, 3 next, 4 shuffle, 5 repeat, 6 repeat one.
fn paint_media_glyph(p: &egui::Painter, r: Rect, which: u8, col: Color32) {
    let c = r.center();
    let s = r.height() * 0.32;
    let tri = |dir: f32, dx: f32| {
        Shape::convex_polygon(
            vec![c + vec2(dx - dir * s * 0.8, -s), c + vec2(dx + dir * s * 0.8, 0.0), c + vec2(dx - dir * s * 0.8, s)],
            col,
            Stroke::NONE,
        )
    };
    let st = Stroke::new((s * 0.22).max(1.4), col);
    match which {
        0 => {
            p.add(tri(-1.0, s * 0.25));
            p.rect_filled(Rect::from_center_size(c + vec2(-s * 0.85, 0.0), vec2(s * 0.28, s * 2.0)), 1.0, col);
        }
        1 => {
            p.add(tri(1.0, s * 0.2));
        }
        2 => {
            p.rect_filled(Rect::from_center_size(c + vec2(-s * 0.42, 0.0), vec2(s * 0.42, s * 2.0)), 1.5, col);
            p.rect_filled(Rect::from_center_size(c + vec2(s * 0.42, 0.0), vec2(s * 0.42, s * 2.0)), 1.5, col);
        }
        3 => {
            p.add(tri(1.0, -s * 0.25));
            p.rect_filled(Rect::from_center_size(c + vec2(s * 0.85, 0.0), vec2(s * 0.28, s * 2.0)), 1.0, col);
        }
        4 => {
            // two crossing arrows
            p.line_segment([c + vec2(-s, -s * 0.6), c + vec2(s, s * 0.6)], st);
            p.line_segment([c + vec2(-s, s * 0.6), c + vec2(s, -s * 0.6)], st);
            p.line_segment([c + vec2(s, s * 0.6), c + vec2(s * 0.5, s * 0.6)], st);
            p.line_segment([c + vec2(s, -s * 0.6), c + vec2(s * 0.5, -s * 0.6)], st);
        }
        _ => {
            // loop arrow (5) or loop with a dot for "repeat one" (6)
            let rr = Rect::from_center_size(c, vec2(s * 2.0, s * 1.3));
            p.rect_stroke(rr, s * 0.5, st);
            p.add(Shape::convex_polygon(
                vec![rr.right_top() + vec2(-s * 0.6, -s * 0.35), rr.right_top() + vec2(-s * 0.1, 0.0), rr.right_top() + vec2(-s * 0.6, s * 0.35)],
                col,
                Stroke::NONE,
            ));
            if which == 6 {
                p.circle_filled(c, s * 0.28, col);
            }
        }
    }
}

impl ShellApp {
    fn media_art(&mut self, ctx: &egui::Context, track: &media::Track) -> Option<egui::TextureId> {
        let art = track.art.as_ref()?;
        if self.media_tex.as_ref().map(|(g, _)| *g) != Some(track.art_gen) {
            let (w, h, px) = (&art.0, &art.1, &art.2);
            let tex = ctx.load_texture(
                format!("media-art-{}", track.art_gen),
                egui::ColorImage::from_rgba_unmultiplied([*w, *h], px),
                egui::TextureOptions::LINEAR,
            );
            self.media_tex = Some((track.art_gen, tex));
        }
        self.media_tex.as_ref().map(|(_, t)| t.id())
    }

    /// The panel element: cover, title / artist, time left, controls, progress.
    fn media_widget(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        r: Rect,
        track: &media::Track,
        pal: &Palette,
        actions: &mut Vec<PanelAction>,
    ) -> Option<String> {
        let bw = 30.0;
        let buttons = Rect::from_min_max(pos2(r.right() - 3.0 * bw, r.top()), r.max);
        let info = Rect::from_min_max(r.min, pos2(buttons.left() - 4.0, r.bottom()));
        let resp = ui.interact(info, ui.id().with("media-info"), Sense::click());
        let p = ui.painter();
        if self.media_open {
            p.rect_filled(info, 4.0, pal.select);
        } else if resp.hovered() {
            p.rect_filled(info, 4.0, pal.hover);
        }
        let art_r = Rect::from_min_size(pos2(info.left() + 3.0, info.center().y - 15.0), vec2(30.0, 30.0));
        match self.media_art(ctx, track) {
            Some(t) => {
                ui.painter().image(t, art_r, uv_full(), Color32::WHITE);
            }
            None => {
                ui.painter().rect_filled(art_r, 4.0, pal.header);
                paint_media_glyph(ui.painter(), art_r.shrink(6.0), 1, pal.dim);
            }
        }
        let left = track.duration - track.position_now();
        let time = if track.duration > 0.0 { format!("-{}", media::fmt_time(left)) } else { String::new() };
        let p = ui.painter();
        let time_w = if time.is_empty() { 0.0 } else { 44.0 };
        let text = Rect::from_min_max(pos2(art_r.right() + 8.0, info.top()), pos2(info.right() - time_w - 4.0, info.bottom()));
        let clip = p.with_clip_rect(text);
        clip.text(pos2(text.left(), info.center().y - 7.0), Align2::LEFT_CENTER, &track.title, FontId::proportional(13.0), pal.text);
        clip.text(pos2(text.left(), info.center().y + 9.0), Align2::LEFT_CENTER, &track.artist, FontId::proportional(11.5), pal.dim);
        if !time.is_empty() {
            p.text(pos2(info.right() - 4.0, info.center().y), Align2::RIGHT_CENTER, time, FontId::proportional(12.0), pal.dim);
        }
        // Progress along the bottom edge.
        if track.duration > 0.0 {
            let f = (track.position_now() / track.duration).clamp(0.0, 1.0) as f32;
            let bar = Rect::from_min_max(pos2(r.left() + 2.0, r.bottom() + 1.0), pos2(r.right() - 2.0, r.bottom() + 3.0));
            p.rect_filled(bar, 1.0, pal.border);
            p.rect_filled(Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * f, bar.bottom())), 1.0, pal.accent);
        }
        for (k, (glyph, cmd)) in [(0u8, 0u8), (if track.playing { 2 } else { 1 }, 1), (3, 2)].into_iter().enumerate() {
            let b = Rect::from_min_size(pos2(buttons.left() + k as f32 * bw, r.top() + 3.0), vec2(bw, r.height() - 6.0));
            let br = ui.interact(b, ui.id().with(("media-btn", k)), Sense::click());
            if br.hovered() {
                ui.painter().rect_filled(b, 4.0, pal.hover);
            }
            paint_media_glyph(ui.painter(), Rect::from_center_size(b.center(), vec2(18.0, 18.0)), glyph, pal.text);
            if br.clicked() {
                actions.push(PanelAction::Media(match cmd {
                    0 => media::Cmd::Prev,
                    1 => media::Cmd::TogglePlay,
                    _ => media::Cmd::Next,
                }));
            }
        }
        // Click: go to the player (the browser tab that plays). Right-click: the player flyout.
        if resp.clicked() {
            actions.push(PanelAction::GoToMedia);
        }
        if resp.secondary_clicked() {
            actions.push(PanelAction::ToggleMedia);
        }
        if resp.hovered() {
            // keep the time ticking while the pointer rests on it
            ctx.request_repaint_after(Duration::from_millis(500));
            let app = if track.app.is_empty() { crate::i18n::tr("the player").to_string() } else { track.app.clone() };
            return Some(crate::i18n::trf("Go to {}\nRight-click: player controls", &[&app]));
        }
        None
    }

    /// Bring up the window that plays, and in a browser the tab that plays.
    /// False when the player has no window to go to.
    fn go_to_player(&mut self) -> bool {
        let Some(track) = self.media.current() else { return false };
        let candidates: Vec<source::Candidate> = self
            .tasks
            .iter()
            .map(|t| source::Candidate {
                hwnd: t.hwnd,
                title: t.title.clone(),
                exe: self.exe_of_window.get(&t.hwnd).cloned().flatten().unwrap_or_default(),
            })
            .collect();
        let wins = source::player_windows(&track.source, &track.title, candidates);
        let Some(first) = wins.first() else {
            // A Store app playing in the background: start (show) it.
            return track.source.contains('!') && win::activate_packaged(&track.source);
        };
        // Bring the window up right away (the click gives the panel the right to).
        win::switch_to(first.hwnd);
        let tl = track.title.to_lowercase();
        let showing = !tl.is_empty() && first.title.to_lowercase().contains(&tl);
        let browsers: Vec<isize> = wins.iter().filter(|w| source::is_browser(&source::stem(&w.exe))).map(|w| w.hwnd).collect();
        if !browsers.is_empty() && !showing {
            // Find the tab on another thread: the browser can take a moment to answer.
            let (title, artist, shown) = (track.title.clone(), track.artist.clone(), first.hwnd);
            std::thread::spawn(move || {
                if let Some(h) = source::select_tab(&browsers, &title, &artist) {
                    if h != shown {
                        win::switch_to(h);
                    }
                }
            });
        }
        true
    }

    /// The player flyout above the panel element.
    fn media_viewport(&mut self, ctx: &egui::Context, pal: &Palette) {
        let Some(track) = self.media.current() else {
            self.media_open = false;
            return;
        };
        let screen = Self::screen_points(ctx);
        let x = self.media_x.clamp(4.0, (screen.x - MEDIA_W - 4.0).max(4.0));
        let builder = ViewportBuilder::default()
            .with_title("Delfin Media")
            .with_transparent(true)
            .with_visible(false)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size([MEDIA_W, MEDIA_H])
            .with_position([x, screen.y - self.lift - PANEL_H - MEDIA_H - 8.0]);

        let mut close = false;
        let mut cmds: Vec<media::Cmd> = Vec::new();
        let art = self.media_art(ctx, &track);
        ctx.show_viewport_immediate(ViewportId::from_hash_of("delfin_media"), builder, |ctx, _| {
            if !win::reveal_popup("Delfin Media", true) {
                ctx.request_repaint();
            }
            let focused = ctx.input(|i| i.viewport().focused);
            if focused == Some(true) {
                self.media_seen_focus = true;
            }
            if self.media_seen_focus && focused == Some(false) {
                close = true;
            }
            if !self.media_seen_focus {
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
            if ctx.input(|i| i.key_pressed(Key::Escape) || i.viewport().close_requested()) {
                close = true;
            }
            if track.playing {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
            egui::CentralPanel::default()
                .frame(Frame::none().fill(pal.window).stroke(Stroke::new(1.0_f32, pal.border)).inner_margin(Margin::same(18.0)))
                .show(ctx, |ui| {
                    let full = ui.max_rect();
                    let p = ui.painter().clone();
                    // Header: app.
                    let hdr = Rect::from_min_size(full.min, vec2(full.width(), 28.0));
                    let dot = Rect::from_center_size(pos2(hdr.left() + 12.0, hdr.center().y), vec2(24.0, 24.0));
                    p.circle_filled(dot.center(), 12.0, pal.accent);
                    paint_media_glyph(&p, dot.shrink(5.0), 1, Color32::WHITE);
                    p.text(pos2(dot.right() + 10.0, hdr.center().y), Align2::LEFT_CENTER, &track.app, FontId::proportional(15.0), pal.text);
                    p.line_segment([pos2(full.left(), hdr.bottom() + 10.0), pos2(full.right(), hdr.bottom() + 10.0)], Stroke::new(1.0_f32, pal.border));

                    // Cover and titles.
                    let top = hdr.bottom() + 22.0;
                    let art_r = Rect::from_min_size(pos2(full.left(), top), vec2(140.0, 140.0));
                    match art {
                        Some(t) => {
                            p.image(t, art_r, uv_full(), Color32::WHITE);
                        }
                        None => {
                            p.rect_filled(art_r, 8.0, pal.header);
                            paint_media_glyph(&p, art_r.shrink(45.0), 1, pal.dim);
                        }
                    }
                    let tx = art_r.right() + 18.0;
                    let text = Rect::from_min_max(pos2(tx, top), pos2(full.right(), art_r.bottom()));
                    let clip = p.with_clip_rect(text);
                    clip.text(pos2(tx, top + 22.0), Align2::LEFT_CENTER, &track.title, FontId::proportional(20.0), pal.text);
                    clip.text(pos2(tx, top + 50.0), Align2::LEFT_CENTER, &track.artist, FontId::proportional(15.0), pal.dim);
                    if !track.album.is_empty() && track.album != track.title {
                        clip.text(pos2(tx, top + 72.0), Align2::LEFT_CENTER, &track.album, FontId::proportional(12.5), pal.dim);
                    }

                    // Progress: click / drag to seek.
                    if track.duration > 0.0 {
                        let now = track.position_now();
                        let y = art_r.bottom() - 12.0;
                        p.text(pos2(tx, y), Align2::LEFT_CENTER, media::fmt_time(now), FontId::proportional(12.5), pal.text);
                        p.text(pos2(full.right(), y), Align2::RIGHT_CENTER, format!("-{}", media::fmt_time(track.duration - now)), FontId::proportional(12.5), pal.text);
                        let bar = Rect::from_min_max(pos2(tx + 44.0, y - 2.0), pos2(full.right() - 48.0, y + 2.0));
                        let hit = bar.expand2(vec2(0.0, 8.0));
                        let resp = ui.interact(hit, ui.id().with("seek"), Sense::click_and_drag());
                        let f = (now / track.duration).clamp(0.0, 1.0) as f32;
                        p.rect_filled(bar, 2.0, pal.border);
                        p.rect_filled(Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * f, bar.bottom())), 2.0, pal.accent);
                        if resp.hovered() || resp.dragged() {
                            p.circle_filled(pos2(bar.left() + bar.width() * f, bar.center().y), 6.0, pal.accent);
                        }
                        if track.can_seek && (resp.clicked() || resp.drag_stopped()) {
                            if let Some(pt) = resp.interact_pointer_pos() {
                                let to = ((pt.x - bar.left()) / bar.width()).clamp(0.0, 1.0) as f64 * track.duration;
                                cmds.push(media::Cmd::Seek(to));
                            }
                        }
                    }

                    // Controls.
                    let row_y = art_r.bottom() + 42.0;
                    let mid = full.center().x;
                    let buttons: [(f32, u8, f32); 5] = [
                        (-150.0, 4, 34.0),
                        (-78.0, 0, 38.0),
                        (0.0, if track.playing { 2 } else { 1 }, 58.0),
                        (78.0, 3, 38.0),
                        (150.0, if track.repeat == media::Repeat::Track { 6 } else { 5 }, 34.0),
                    ];
                    for (k, (dx, glyph, size)) in buttons.into_iter().enumerate() {
                        let b = Rect::from_center_size(pos2(mid + dx, row_y), vec2(size, size));
                        let enabled = match k {
                            0 => track.can_shuffle,
                            4 => track.can_repeat,
                            _ => true,
                        };
                        let resp = ui.interact(b, ui.id().with(("ctl", k)), if enabled { Sense::click() } else { Sense::hover() });
                        let active = (k == 0 && track.shuffle) || (k == 4 && track.repeat != media::Repeat::Off);
                        let col = if !enabled {
                            pal.border
                        } else if active {
                            pal.accent
                        } else {
                            pal.text
                        };
                        if k == 2 {
                            p.circle_stroke(b.center(), size / 2.0, Stroke::new(1.6_f32, if resp.hovered() { pal.accent } else { pal.dim }));
                        } else if resp.hovered() && enabled {
                            p.circle_filled(b.center(), size / 2.0, pal.hover);
                        }
                        let g = if k == 2 { 26.0 } else { 20.0 };
                        paint_media_glyph(&p, Rect::from_center_size(b.center(), vec2(g, g)), glyph, col);
                        if resp.clicked() {
                            cmds.push(match k {
                                0 => media::Cmd::Shuffle(!track.shuffle),
                                1 => media::Cmd::Prev,
                                2 => media::Cmd::TogglePlay,
                                3 => media::Cmd::Next,
                                _ => media::Cmd::Repeat(match track.repeat {
                                    media::Repeat::Off => media::Repeat::List,
                                    media::Repeat::List => media::Repeat::Track,
                                    media::Repeat::Track => media::Repeat::Off,
                                }),
                            });
                        }
                    }

                    // Volume (whole system).
                    let vol_y = row_y + 52.0;
                    if let Some((level, muted)) = self.qp.volume_state() {
                        let sp = Rect::from_center_size(pos2(full.left() + 12.0, vol_y), vec2(24.0, 24.0));
                        let sresp = ui.interact(sp, ui.id().with("mute"), Sense::click());
                        quick::paint_speaker(&p, sp, pal.text, if muted { None } else { Some(level) });
                        if sresp.clicked() {
                            self.qp.toggle_mute();
                        }
                        let mut v = (level * 100.0).round();
                        let slider = Rect::from_min_max(pos2(sp.right() + 12.0, vol_y - 10.0), pos2(full.right() - 40.0, vol_y + 10.0));
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(slider));
                        child.spacing_mut().slider_width = slider.width();
                        if child.add(egui::Slider::new(&mut v, 0.0..=100.0).show_value(false)).changed() {
                            self.qp.set_volume(v / 100.0);
                        }
                        p.text(pos2(full.right(), vol_y), Align2::RIGHT_CENTER, format!("{v:.0}%"), FontId::proportional(12.5), pal.dim);
                    }
                });
        });
        for c in cmds {
            self.media.send(c);
        }
        if close {
            self.media_open = false;
            self.media_closed_at = Instant::now();
        }
    }
}

// ----------------------------------------------------------------- task previews

const PREVIEW_W: f32 = 300.0;
const PREVIEW_H: f32 = 214.0;
const PREVIEW_DELAY: Duration = Duration::from_millis(400);
const PREVIEW_TITLE: &str = "Delfin Preview";

/// Hovering a task button shows a live DWM thumbnail of that window.
struct Preview {
    hwnd: isize,
    /// Centre of the task button (points).
    x: f32,
    since: Instant,
    thumb: Option<isize>,
    thumb_for: isize,
    pointer_inside: bool,
}

impl Drop for Preview {
    fn drop(&mut self) {
        if let Some(t) = self.thumb {
            win::thumb_unregister(t);
        }
    }
}

impl ShellApp {
    fn update_preview(&mut self, hover: Option<(isize, f32)>) {
        // No preview over a right-click menu: it would cover the menu's buttons.
        if self.switcher.is_some() || self.launcher_open || self.panel_menu.is_some() {
            self.preview = None;
            return;
        }
        let now = Instant::now();
        match hover {
            Some((h, x)) => {
                self.preview_last_hover = now;
                match &mut self.preview {
                    Some(p) if p.hwnd == h => p.x = x,
                    Some(p) => {
                        // Moving along the panel: the preview follows at once.
                        let shown = p.since.elapsed() >= PREVIEW_DELAY;
                        p.hwnd = h;
                        p.x = x;
                        if !shown {
                            p.since = now;
                        }
                    }
                    None => {
                        self.preview = Some(Preview { hwnd: h, x, since: now, thumb: None, thumb_for: 0, pointer_inside: false });
                    }
                }
            }
            None => {
                let keep = self.preview.as_ref().is_some_and(|p| p.pointer_inside)
                    || self.preview_last_hover.elapsed() < Duration::from_millis(300);
                if !keep {
                    self.preview = None;
                }
            }
        }
    }

    fn preview_viewport(&mut self, ctx: &egui::Context, pal: &Palette) {
        let Some(p) = &self.preview else { return };
        let wait = PREVIEW_DELAY.saturating_sub(p.since.elapsed());
        if !wait.is_zero() {
            ctx.request_repaint_after(wait);
            return;
        }
        if !win::is_window(p.hwnd) {
            self.preview = None;
            return;
        }
        let screen = Self::screen_points(ctx);
        let x = (p.x - PREVIEW_W / 2.0).clamp(4.0, (screen.x - PREVIEW_W - 4.0).max(4.0));
        let builder = ViewportBuilder::default()
            .with_title(PREVIEW_TITLE)
            .with_transparent(true)
            .with_visible(false)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size([PREVIEW_W, PREVIEW_H])
            .with_position([x, screen.y - self.lift - PANEL_H - PREVIEW_H - 6.0]);
        let ppp = ctx.pixels_per_point();
        let task = self.tasks.iter().find(|t| t.hwnd == p.hwnd).cloned();
        let icon = self.tex_window(ctx, p.hwnd);
        let mut activate = false;
        let mut close_window = false;
        ctx.show_viewport_immediate(ViewportId::from_hash_of("delfin_preview"), builder, |ctx, _| {
            // Never takes the focus: the window you are looking at keeps it.
            if !win::reveal_popup(PREVIEW_TITLE, false) {
                ctx.request_repaint();
                return;
            }
            let Some(p) = &mut self.preview else { return };
            p.pointer_inside = ctx.input(|i| i.pointer.has_pointer());
            // (Re)attach the thumbnail when the hovered window changes.
            if p.thumb_for != p.hwnd {
                if let Some(t) = p.thumb.take() {
                    win::thumb_unregister(t);
                }
                if let Some(dest) = win::find_window(PREVIEW_TITLE) {
                    p.thumb = win::thumb_register(dest, p.hwnd);
                    p.thumb_for = p.hwnd;
                }
            }
            let fade = ease_out((p.since.elapsed() - PREVIEW_DELAY).as_secs_f32() / 0.12);
            if fade < 1.0 {
                ctx.request_repaint();
            }
            let title = task.as_ref().map(|t| t.title.clone()).unwrap_or_default();
            let minimized = task.as_ref().is_some_and(|t| t.minimized);
            egui::CentralPanel::default()
                .frame(Frame::none().fill(pal.window).stroke(Stroke::new(1.0_f32, pal.border)))
                .show(ctx, |ui| {
                    let full = ui.max_rect();
                    let painter = ui.painter().clone();
                    // Title row: icon, title, close.
                    let row = Rect::from_min_size(full.min + vec2(10.0, 6.0), vec2(full.width() - 20.0, 24.0));
                    let ir = Rect::from_min_size(pos2(row.left(), row.center().y - 8.0), vec2(16.0, 16.0));
                    if let Some(t) = &icon {
                        painter.image(t.id(), ir, uv_full(), Color32::WHITE.gamma_multiply(fade));
                    }
                    let xr = Rect::from_center_size(pos2(row.right() - 10.0, row.center().y), vec2(22.0, 22.0));
                    painter.with_clip_rect(Rect::from_min_max(pos2(ir.right() + 6.0, row.top()), pos2(xr.left() - 4.0, row.bottom()))).text(
                        pos2(ir.right() + 6.0, row.center().y),
                        Align2::LEFT_CENTER,
                        &title,
                        FontId::proportional(12.5),
                        pal.text.gamma_multiply(fade),
                    );
                    let xresp = ui.interact(xr, ui.id().with("preview-close"), Sense::click());
                    if xresp.hovered() {
                        painter.rect_filled(xr, 4.0, pal.danger);
                    }
                    let c = xr.center();
                    let st = Stroke::new(1.4_f32, pal.text.gamma_multiply(fade));
                    painter.line_segment([c + vec2(-4.0, -4.0), c + vec2(4.0, 4.0)], st);
                    painter.line_segment([c + vec2(-4.0, 4.0), c + vec2(4.0, -4.0)], st);
                    if xresp.clicked() {
                        close_window = true;
                    }

                    // The window itself.
                    let area = Rect::from_min_max(pos2(full.left() + 10.0, row.bottom() + 6.0), full.max - vec2(10.0, 10.0));
                    let aresp = ui.interact(area, ui.id().with("preview-area"), Sense::click());
                    if aresp.hovered() {
                        painter.rect_filled(area.expand(4.0), 6.0, pal.hover);
                    }
                    if aresp.clicked() {
                        activate = true;
                    }
                    let live = !minimized && p.thumb.is_some();
                    if let Some(id) = p.thumb {
                        let (sw, sh) = win::thumb_source_size(id);
                        let fit = if sw > 0 && sh > 0 {
                            let s = (area.width() * ppp / sw as f32).min(area.height() * ppp / sh as f32).min(1.0);
                            Rect::from_center_size(area.center(), vec2(sw as f32 * s / ppp, sh as f32 * s / ppp))
                        } else {
                            area
                        };
                        win::thumb_update(
                            id,
                            [(fit.left() * ppp) as i32, (fit.top() * ppp) as i32, (fit.right() * ppp) as i32, (fit.bottom() * ppp) as i32],
                            (255.0 * fade) as u8,
                            live,
                        );
                    }
                    if !live {
                        let big = Rect::from_center_size(area.center(), vec2(48.0, 48.0));
                        match &icon {
                            Some(t) => {
                                painter.image(t.id(), big, uv_full(), Color32::WHITE.gamma_multiply(fade));
                            }
                            None => icons::paint_tool(&painter, big, Tool::Desktop, pal.dim),
                        }
                    }
                });
        });
        if activate || close_window {
            let h = self.preview.as_ref().map(|p| p.hwnd).unwrap_or(0);
            self.preview = None;
            if close_window {
                win::close(h);
            } else {
                win::activate(h);
                self.last_fg = h;
            }
        }
    }
}

// ----------------------------------------------------------------- volume pop-up

const OSD_W: f32 = 300.0;
const OSD_H: f32 = 56.0;
const OSD_SHOW: f32 = 1.4;
const OSD_FADE: f32 = 0.35;

impl ShellApp {
    /// Shown for a moment after a volume key, like the Windows volume flyout.
    fn osd_viewport(&mut self, ctx: &egui::Context, pal: &Palette) {
        let age = self.osd.map(|s| s.elapsed().as_secs_f32());
        if age.is_some_and(|a| a > OSD_SHOW + OSD_FADE) {
            self.osd = None;
        }
        let active = self.osd.is_some();
        let age = age.unwrap_or(0.0);
        let alpha = if age < OSD_SHOW { 1.0 } else { 1.0 - (age - OSD_SHOW) / OSD_FADE };
        if active {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        let screen = Self::screen_points(ctx);
        let builder = ViewportBuilder::default()
            .with_title("Delfin Volume")
            .with_transparent(true)
            .with_visible(false)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size([OSD_W, OSD_H])
            .with_position([(screen.x - OSD_W) / 2.0, screen.y - self.lift - PANEL_H - OSD_H - 24.0]);
        let (level, muted) = self.vol_state.unwrap_or((0.0, false));
        let mut hidden = self.osd_hidden;
        ctx.show_viewport_immediate(ViewportId::from_hash_of("delfin_osd"), builder, |ctx, _| {
            // Never takes the focus away from what you're doing.
            if !active {
                if !hidden {
                    if let Some(h) = win::find_window("Delfin Volume") {
                        win::hide(h);
                        hidden = true;
                    }
                }
                egui::CentralPanel::default().frame(Frame::none().fill(pal.window)).show(ctx, |_| {});
                return;
            }
            hidden = false;
            if !win::reveal_popup("Delfin Volume", false) {
                ctx.request_repaint();
            }
            egui::CentralPanel::default()
                .frame(Frame::none().fill(pal.window.gamma_multiply(alpha)).stroke(Stroke::new(1.0_f32, pal.border.gamma_multiply(alpha))))
                .show(ctx, |ui| {
                    let r = ui.max_rect();
                    let p = ui.painter();
                    let icon = Rect::from_center_size(pos2(r.left() + 30.0, r.center().y), vec2(26.0, 26.0));
                    quick::paint_speaker(p, icon, pal.text.gamma_multiply(alpha), if muted { None } else { Some(level) });
                    let bar = Rect::from_min_max(pos2(icon.right() + 14.0, r.center().y - 2.0), pos2(r.right() - 58.0, r.center().y + 2.0));
                    p.rect_filled(bar, 2.0, pal.border.gamma_multiply(alpha));
                    let f = if muted { 0.0 } else { level.clamp(0.0, 1.0) };
                    p.rect_filled(Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * f, bar.bottom())), 2.0, pal.accent.gamma_multiply(alpha));
                    p.circle_filled(pos2(bar.left() + bar.width() * f, bar.center().y), 6.0, pal.accent.gamma_multiply(alpha));
                    p.text(
                        pos2(r.right() - 16.0, r.center().y),
                        Align2::RIGHT_CENTER,
                        if muted { "—".to_string() } else { format!("{:.0}", level * 100.0) },
                        FontId::proportional(16.0),
                        pal.text.gamma_multiply(alpha),
                    );
                });
        });
        self.osd_hidden = hidden;
    }
}

/// `devmgmt.msc`, `mmc devmgmt.msc`, `hdwwiz.cpl`: Windows' Device Manager.
fn is_devmgr_cmd(c: &str) -> bool {
    let l = c.trim().trim_matches('"').to_lowercase();
    l == "devmgmt.msc" || l == "devmgmt" || l.ends_with(" devmgmt.msc") || l == "hdwwiz.cpl" || l == "control hdwwiz.cpl"
}

// ----------------------------------------------------------------- pinned apps & menus

/// Something picked in a right-click menu.
#[derive(Clone)]
enum MenuCmd {
    Launch(Launch),
    Pin(pins::Pin),
    Unpin(Launch),
    /// Delfin's Properties window for a file.
    Props(PathBuf),
    /// The folder that holds a file, in Delfin.
    Location(PathBuf),
    CloseWindows(Vec<isize>),
    /// Sign out, restart, shut down (Win+X).
    Power(Power),
    /// A program run as administrator (Win+X › Terminal (Admin)).
    Admin(String),
    /// Switch the keyboard layout of the app in front.
    Layout(isize),
    /// Prepare a USB drive for removal: (device node, its name).
    Eject(u32, String),
}

struct PanelMenu {
    /// Left edge of the button it belongs to (points from the screen's left).
    x: f32,
    title: String,
    items: Vec<(String, MenuCmd)>,
    seen_focus: bool,
}

const MENU_W: f32 = 250.0;
const MENU_ROW: f32 = 32.0;

/// A path you can open Properties for (not `shell:AppsFolder\…`).
fn real_file(l: &Launch) -> Option<PathBuf> {
    match l {
        Launch::Path(p) if p.is_file() => Some(p.clone()),
        _ => None,
    }
}

fn spawn_props(path: &Path) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg("--props").arg(path).spawn();
    }
}

impl ShellApp {
    fn launch_pin(&mut self, pin: &pins::Pin) {
        match &pin.target {
            // The terminal falls back to PowerShell without Windows Terminal.
            Launch::Cmd(c) if c.eq_ignore_ascii_case("wt.exe") => {
                let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("C:\\"));
                let _ = crate::platform::open_terminal(&home);
            }
            t => self.launch(t),
        }
    }

    /// Menu entries for an app in the launcher or on the desktop.
    fn app_menu_items(&self, name: &str, target: &Launch) -> Vec<(String, MenuCmd)> {
        let mut v = Vec::new();
        if self.pins.iter().any(|p| p.same_target(target)) {
            v.push((crate::i18n::tr("Unpin from taskbar").to_string(), MenuCmd::Unpin(target.clone())));
        } else {
            v.push((crate::i18n::tr("Pin to taskbar").to_string(), MenuCmd::Pin(pins::Pin::new(name.to_string(), target.clone()))));
        }
        if let Some(f) = real_file(target) {
            v.push((crate::i18n::tr("Open file location").to_string(), MenuCmd::Location(f.clone())));
            v.push((crate::i18n::tr("Properties").to_string(), MenuCmd::Props(f)));
        }
        v
    }

    fn open_pin_menu(&mut self, i: usize, x: f32, wins: Vec<isize>) {
        self.preview = None;
        let Some(pin) = self.pins.get(i).cloned() else { return };
        let mut items = vec![(
            if wins.is_empty() { crate::i18n::tr("Open").to_string() } else { crate::i18n::tr("New window").to_string() },
            MenuCmd::Launch(pin.target.clone()),
        )];
        items.push((crate::i18n::tr("Unpin from taskbar").to_string(), MenuCmd::Unpin(pin.target.clone())));
        if let Some(f) = real_file(&pin.target) {
            items.push((crate::i18n::tr("Open file location").to_string(), MenuCmd::Location(f.clone())));
            items.push((crate::i18n::tr("Properties").to_string(), MenuCmd::Props(f)));
        }
        if !wins.is_empty() {
            let label = if wins.len() == 1 { crate::i18n::tr("Close window").to_string() } else { crate::i18n::tr("Close all windows").to_string() };
            items.push((label, MenuCmd::CloseWindows(wins)));
        }
        self.show_panel_menu(x, pin.name, items);
    }

    fn open_task_menu(&mut self, h: isize, x: f32) {
        self.preview = None;
        let title = self.tasks.iter().find(|t| t.hwnd == h).map(|t| t.title.clone()).unwrap_or_default();
        let mut items = Vec::new();
        let exe = win::window_exe(h);
        // Store apps run inside ApplicationFrameHost: nothing to pin there.
        if let Some(exe) = exe.filter(|e| {
            let n = e.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            n != "applicationframehost.exe" && n != "explorer.exe"
        }) {
            // A Store / MSIX app (Claude, …) can't be started from its exe:
            // pin its Start-menu entry ("shell:AppsFolder\…") instead.
            let packaged = win::package_key_of_exe(&exe.to_string_lossy()).and_then(|key| {
                self.apps
                    .iter()
                    .find(|a| match &a.target {
                        Launch::Path(p) => win::packaged_app_id(&p.to_string_lossy())
                            .and_then(|id| win::package_key_of_aumid(&id))
                            .is_some_and(|k| k == key),
                        _ => false,
                    })
                    .map(|a| pins::Pin::from_app(a))
            });
            let pin = packaged.unwrap_or_else(|| {
                let name = startmenu::file_description(&exe)
                    .filter(|d| !d.trim().is_empty())
                    .unwrap_or_else(|| exe.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
                pins::Pin::new(name, Launch::Path(exe.clone()))
            });
            if self.pins.iter().any(|p| p.same_target(&pin.target)) {
                items.push((crate::i18n::tr("Unpin from taskbar").to_string(), MenuCmd::Unpin(pin.target.clone())));
            } else {
                items.push((crate::i18n::tr("Pin to taskbar").to_string(), MenuCmd::Pin(pin)));
            }
            items.push((crate::i18n::tr("Open file location").to_string(), MenuCmd::Location(exe.clone())));
            items.push((crate::i18n::tr("Properties").to_string(), MenuCmd::Props(exe)));
        }
        items.push((crate::i18n::tr("Close window").to_string(), MenuCmd::CloseWindows(vec![h])));
        self.show_panel_menu(x, title, items);
    }

    fn show_panel_menu(&mut self, x: f32, title: String, items: Vec<(String, MenuCmd)>) {
        self.open_quick(None);
        self.open_launcher(false);
        self.calendar_open = false;
        self.preview = None;
        self.panel_menu = Some(PanelMenu { x, title, items, seen_focus: false });
    }

    fn run_menu_cmd(&mut self, c: MenuCmd) {
        match c {
            MenuCmd::Launch(l) => match self.pins.iter().find(|p| p.same_target(&l)).cloned() {
                Some(p) => self.launch_pin(&p),
                None => self.launch(&l),
            },
            MenuCmd::Pin(p) => {
                if !self.pins.iter().any(|x| x.same_target(&p.target)) {
                    self.pins.push(p);
                    pins::save(&self.pins);
                    self.pins_mtime = pins::modified();
                }
            }
            MenuCmd::Unpin(t) => {
                self.pins.retain(|p| !p.same_target(&t));
                pins::save(&self.pins);
                self.pins_mtime = pins::modified();
            }
            MenuCmd::Props(p) => spawn_props(&p),
            MenuCmd::Location(p) => win::spawn_self(p.parent()),
            MenuCmd::CloseWindows(ws) => {
                for h in ws {
                    win::close(h);
                }
            }
            MenuCmd::Power(p) => self.power(p),
            MenuCmd::Admin(exe) => {
                crate::taskmgr::sys::run_elevated(&exe, "");
            }
            MenuCmd::Layout(hkl) => kbd::switch(hkl),
            MenuCmd::Eject(devinst, name) => {
                let (tx, rx) = std::sync::mpsc::channel();
                self.eject_rx = Some(rx);
                std::thread::spawn(move || {
                    let _ = tx.send((name, usb::eject(devinst)));
                });
            }
        }
    }

    /// A USB drive came or went (or an eject finished): update the panel.
    fn usb_tick(&mut self) {
        let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
        if mask != self.usb_mask || usb::CHANGED.swap(false, std::sync::atomic::Ordering::Relaxed) {
            self.usb_mask = mask;
            self.usb_drives = usb::drives();
            self.layouts = kbd::layouts();
        }
        if let Some(rx) = &self.eject_rx {
            if let Ok((name, r)) = rx.try_recv() {
                self.eject_rx = None;
                let (title, body) = match r {
                    Ok(()) => (crate::i18n::tr("Safe to remove hardware").to_string(), crate::i18n::trf("{} can now be removed.", &[&name])),
                    Err(e) => (crate::i18n::trf("Can't eject {}", &[&name]), e),
                };
                self.toast_queue.push_back(notify::Toast { id: 0, app: "Delfin".into(), aumid: String::new(), title, body, time: chrono::Utc::now().timestamp(), icon: None });
                self.usb_drives = usb::drives();
            }
        }
    }

    /// Win+X: the quick-link menu above the start button, like Windows'.
    fn open_power_menu(&mut self) {
        use crate::i18n::tr;
        let cmd = |c: &str| MenuCmd::Launch(Launch::Cmd(c.to_string()));
        let items = vec![
            (tr("Installed apps").to_string(), cmd("ms-settings:appsfeatures")),
            (tr("Power Options").to_string(), cmd("ms-settings:powersleep")),
            (tr("System").to_string(), cmd("ms-settings:about")),
            (tr("Device Manager").to_string(), cmd("devmgmt.msc")),
            (tr("Network Connections").to_string(), cmd("ms-settings:network")),
            (tr("Disk Management").to_string(), cmd("diskmgmt.msc")),
            (tr("Computer Management").to_string(), cmd("compmgmt.msc")),
            (tr("Terminal").to_string(), cmd("wt.exe")),
            (tr("Terminal (Admin)").to_string(), MenuCmd::Admin("wt.exe".into())),
            (tr("Task Manager").to_string(), cmd("taskmgr.exe")),
            (tr("Settings").to_string(), cmd("ms-settings:")),
            ("Delfin".to_string(), cmd("delfin")),
            (tr("Sign out").to_string(), MenuCmd::Power(Power::Logout)),
            (tr("Restart").to_string(), MenuCmd::Power(Power::Restart)),
            (tr("Shut down").to_string(), MenuCmd::Power(Power::Shutdown)),
        ];
        self.show_panel_menu(4.0, "Delfin".to_string(), items);
    }

    /// A pinned app on the panel. `state`: 0 not running, 1 running, 2 in front.
    fn pin_button(&mut self, ui: &mut Ui, ctx: &egui::Context, pin: &pins::Pin, state: u8, pal: &Palette) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 36.0), Sense::click());
        let p = ui.painter().clone();
        if state == 2 {
            p.rect_filled(rect, 4.0, pal.select);
        } else if resp.hovered() {
            p.rect_filled(rect, 4.0, pal.hover);
        }
        if state > 0 {
            let w = if state == 2 { 16.0 } else { 6.0 };
            p.line_segment(
                [pos2(rect.center().x - w / 2.0, rect.bottom() - 1.0), pos2(rect.center().x + w / 2.0, rect.bottom() - 1.0)],
                Stroke::new(2.5_f32, if state == 2 { pal.accent } else { pal.dim }),
            );
        }
        let ir = Rect::from_center_size(rect.center(), vec2(22.0, 22.0));
        match &pin.target {
            Launch::Cmd(c) if c == "delfin" => icons::paint_folder(&p, rect.shrink(8.0), icons::FOLDER),
            Launch::Cmd(c) => {
                let tool = if c.eq_ignore_ascii_case("wt.exe") {
                    Some(Tool::Terminal)
                } else {
                    self.apps.iter().find(|a| matches!(&a.target, Launch::Cmd(x) if x.eq_ignore_ascii_case(c))).and_then(|a| a.icon)
                };
                icons::paint_tool(&p, ir.shrink(2.0), tool.unwrap_or(Tool::Desktop), pal.text);
            }
            Launch::Path(path) => match self.tex_file(ctx, path, false) {
                Some(t) => {
                    p.image(t.id(), ir, uv_full(), Color32::WHITE);
                }
                None => icons::paint_tool(&p, ir.shrink(2.0), Tool::Desktop, pal.dim),
            },
        }
        resp
    }

    fn panel_menu_viewport(&mut self, ctx: &egui::Context, pal: &Palette) {
        let Some(menu) = &self.panel_menu else { return };
        let h = 44.0 + menu.items.len() as f32 * MENU_ROW + 12.0;
        let screen = Self::screen_points(ctx);
        let x = menu.x.clamp(4.0, (screen.x - MENU_W - 4.0).max(4.0));
        let builder = ViewportBuilder::default()
            .with_title("Delfin Menu")
            .with_transparent(true)
            .with_visible(false)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size([MENU_W, h])
            .with_position([x, screen.y - self.lift - PANEL_H - h - 6.0]);
        let mut picked: Option<MenuCmd> = None;
        let mut close = false;
        ctx.show_viewport_immediate(ViewportId::from_hash_of("delfin_panel_menu"), builder, |ctx, _| {
            if !win::reveal_popup("Delfin Menu", true) {
                ctx.request_repaint();
            }
            let Some(menu) = &mut self.panel_menu else { return };
            let focused = ctx.input(|i| i.viewport().focused);
            if focused == Some(true) {
                menu.seen_focus = true;
            }
            if menu.seen_focus && focused == Some(false) {
                close = true;
            }
            if !menu.seen_focus {
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
            if ctx.input(|i| i.key_pressed(Key::Escape) || i.viewport().close_requested()) {
                close = true;
            }
            egui::CentralPanel::default()
                .frame(Frame::none().fill(pal.window).stroke(Stroke::new(1.0_f32, pal.border)).inner_margin(Margin::same(6.0)))
                .show(ctx, |ui| {
                    let (tr_, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
                    ui.painter().with_clip_rect(tr_).text(
                        pos2(tr_.left() + 10.0, tr_.center().y),
                        Align2::LEFT_CENTER,
                        &menu.title,
                        FontId::proportional(12.5),
                        pal.dim,
                    );
                    ui.painter().line_segment([pos2(tr_.left(), tr_.bottom()), pos2(tr_.right(), tr_.bottom())], Stroke::new(1.0_f32, pal.border));
                    ui.add_space(4.0);
                    for (label, cmd) in &menu.items {
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), MENU_ROW), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(r, 4.0, pal.hover);
                        }
                        let danger = matches!(cmd, MenuCmd::CloseWindows(_));
                        ui.painter().text(
                            pos2(r.left() + 10.0, r.center().y),
                            Align2::LEFT_CENTER,
                            label,
                            FontId::proportional(13.5),
                            if danger { pal.danger } else { pal.text },
                        );
                        if resp.clicked() {
                            picked = Some(cmd.clone());
                        }
                    }
                });
        });
        if let Some(c) = picked {
            self.panel_menu = None;
            self.run_menu_cmd(c);
        } else if close {
            self.panel_menu = None;
        }
    }
}

/// Settings changed dark mode or Aero (see `theme::notify_shell`).
static LOOK_CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn spawn_look_waiter(ctx: egui::Context) {
    std::thread::spawn(move || unsafe {
        use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
        let name: Vec<u16> = theme::SHELL_EVENT.encode_utf16().chain(Some(0)).collect();
        let ev = CreateEventW(std::ptr::null(), 0, 0, name.as_ptr());
        if ev.is_null() {
            return;
        }
        while WaitForSingleObject(ev, INFINITE) == 0 {
            LOOK_CHANGED.store(true, std::sync::atomic::Ordering::Relaxed);
            ctx.request_repaint();
        }
    });
}

/// Vista-style glass shine: a soft light band over the top half of a surface
/// and a thin bright line just inside its top edge.
fn paint_gloss(p: &egui::Painter, r: Rect, rounding: f32) {
    let top = r.top() + rounding.min(2.0);
    let mid = r.top() + r.height() * 0.48;
    let mut mesh = egui::Mesh::default();
    let hi = Color32::from_white_alpha(34);
    let lo = Color32::from_white_alpha(8);
    mesh.colored_vertex(pos2(r.left(), top), hi);
    mesh.colored_vertex(pos2(r.right(), top), hi);
    mesh.colored_vertex(pos2(r.right(), mid), lo);
    mesh.colored_vertex(pos2(r.left(), mid), lo);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(Shape::mesh(mesh));
    p.line_segment(
        [pos2(r.left() + rounding, r.top() + 1.5), pos2(r.right() - rounding, r.top() + 1.5)],
        Stroke::new(1.0_f32, Color32::from_white_alpha(70)),
    );
}

/// How long a new notification stays on screen.
const TOAST_SECS: u64 = 6;

/// A bell (notifications); crossed out with Do not disturb.
fn paint_bell(p: &egui::Painter, r: Rect, col: Color32, muted: bool) {
    let c = r.center();
    let s = r.width() / 18.0;
    let st = Stroke::new(1.5_f32, col);
    let v = |x: f32, y: f32| c + vec2(x * s, y * s);
    let mut pts: Vec<egui::Pos2> = vec![v(-6.5, 5.0), v(-5.5, 3.5), v(-5.0, -1.5)];
    for k in 0..=12 {
        let a = std::f32::consts::PI * (1.0 + k as f32 / 12.0);
        pts.push(c + vec2(a.cos() * 5.0 * s, -3.0 * s + a.sin() * 5.0 * s));
    }
    pts.extend([v(5.0, -1.5), v(5.5, 3.5), v(6.5, 5.0), v(-6.5, 5.0)]);
    p.add(Shape::line(pts, st));
    p.line_segment([v(-2.0, 7.0), v(2.0, 7.0)], st);
    if muted {
        p.line_segment([v(-8.0, -8.0), v(8.0, 8.0)], st);
    }
}

/// Text wrapped to `width`, at most `rows` lines (ellipsis after).
fn label_wrapped(ui: &Ui, text: &str, width: f32, size: f32, color: Color32, rows: usize) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(text.to_string(), egui::TextFormat { font_id: FontId::proportional(size), color, ..Default::default() });
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: rows, break_anywhere: false, overflow_character: Some('\u{2026}') };
    ui.fonts(|f| f.layout_job(job))
}

/// The USB symbol (safely remove hardware).
fn paint_usb(p: &egui::Painter, r: Rect, col: Color32) {
    let c = r.center();
    let s = r.width() / 18.0;
    let st = Stroke::new(1.5_f32, col);
    let v = |x: f32, y: f32| c + vec2(x * s, y * s);
    p.line_segment([v(0.0, -7.5), v(0.0, 6.0)], st);
    p.add(Shape::convex_polygon(vec![v(0.0, -9.0), v(-2.5, -5.5), v(2.5, -5.5)], col, Stroke::NONE));
    p.circle_filled(v(0.0, 7.0), 2.0 * s, col);
    p.add(Shape::line(vec![v(0.0, 3.0), v(-5.0, -0.5), v(-5.0, -3.5)], st));
    p.circle_filled(v(-5.0, -4.5), 1.4 * s, col);
    p.add(Shape::line(vec![v(0.0, 1.0), v(5.0, -2.0), v(5.0, -4.0)], st));
    p.rect_filled(Rect::from_center_size(v(5.0, -5.0), vec2(2.8 * s, 2.8 * s)), 0.0, col);
}
