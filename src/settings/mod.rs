//! Delfin Settings (`delfin.exe --settings [page]`): Delfin's own take on the
//! Windows 11 Settings app, which can't start without Explorer. System
//! (display, sound, power, storage, about), devices, network,
//! personalization, apps, accounts, time and language, Windows Update and
//! Delfin itself. `page` is a Delfin page name or an `ms-settings:` link.

pub(crate) mod sys;

use crate::i18n::{tr, trf};
use crate::icons::{self, Tool};
use crate::shell::quick::{self, QuickPanel};
use crate::taskmgr::sys::{fmt_bytes, StartupItem};
use crate::theme::{self, Palette};
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, FontId, Frame, Key, Layout, Margin, Rect, Response, RichText, ScrollArea, Sense,
    Shape, Stroke, TextureHandle, Ui,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Slot<T> = Arc<Mutex<Option<T>>>;
type Pixels = (usize, usize, Vec<u8>);

fn take<T>(s: &Slot<T>) -> Option<T> {
    s.lock().unwrap_or_else(|p| p.into_inner()).take()
}

/// Run `f` on its own thread; the result lands in the returned slot.
fn spawn<T: Send + 'static>(ctx: &egui::Context, f: impl FnOnce() -> T + Send + 'static) -> Slot<T> {
    let slot: Slot<T> = Arc::new(Mutex::new(None));
    let (s, ctx) = (slot.clone(), ctx.clone());
    std::thread::spawn(move || {
        unsafe { windows_sys::Win32::System::Com::CoInitializeEx(std::ptr::null(), 0x2) };
        let v = f();
        *s.lock().unwrap_or_else(|p| p.into_inner()) = Some(v);
        ctx.request_repaint();
    });
    slot
}

// ------------------------------------------------------------------ pages

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Page {
    System,
    Display,
    Sound,
    Power,
    Storage,
    About,
    Devices,
    Mouse,
    Network,
    Personalization,
    Apps,
    Installed,
    Startup,
    Accounts,
    Time,
    Update,
    Delfin,
}

const TOP: [Page; 9] =
    [Page::System, Page::Devices, Page::Network, Page::Personalization, Page::Apps, Page::Accounts, Page::Time, Page::Update, Page::Delfin];

impl Page {
    fn parent(self) -> Page {
        match self {
            Page::Display | Page::Sound | Page::Power | Page::Storage | Page::About => Page::System,
            Page::Mouse => Page::Devices,
            Page::Installed | Page::Startup => Page::Apps,
            p => p,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Page::System => tr("System"),
            Page::Display => tr("Display"),
            Page::Sound => tr("Sound"),
            Page::Power => tr("Power & battery"),
            Page::Storage => tr("Storage"),
            Page::About => tr("About"),
            Page::Devices => tr("Bluetooth & devices"),
            Page::Mouse => tr("Mouse"),
            Page::Network => tr("Network & internet"),
            Page::Personalization => tr("Personalization"),
            Page::Apps => tr("Apps"),
            Page::Installed => tr("Installed apps"),
            Page::Startup => tr("Startup"),
            Page::Accounts => tr("Accounts"),
            Page::Time => tr("Time & language"),
            Page::Update => "Windows Update",
            Page::Delfin => "Delfin",
        }
    }

    fn icon(self) -> Ico {
        match self {
            Page::System => Ico::Monitor,
            Page::Display => Ico::Monitor,
            Page::Sound => Ico::Speaker,
            Page::Power => Ico::Power,
            Page::Storage => Ico::Drive,
            Page::About => Ico::Info,
            Page::Devices => Ico::Bluetooth,
            Page::Mouse => Ico::Mouse,
            Page::Network => Ico::Globe,
            Page::Personalization => Ico::Brush,
            Page::Apps => Ico::Apps,
            Page::Installed => Ico::List,
            Page::Startup => Ico::Rocket,
            Page::Accounts => Ico::Person,
            Page::Time => Ico::Clock,
            Page::Update => Ico::Update,
            Page::Delfin => Ico::Delfin,
        }
    }

    /// The colour of the icon in the side bar.
    fn tint(self) -> Color32 {
        let hex = |v: u32| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        match self.parent() {
            Page::System => hex(0x4cc2ff),
            Page::Devices => hex(0x3b9cf0),
            Page::Network => hex(0x5fb2f2),
            Page::Personalization => hex(0xf7894a),
            Page::Apps => hex(0xa98bf5),
            Page::Accounts => hex(0x4fc47b),
            Page::Time => hex(0x2fb8c6),
            Page::Update => hex(0x3d8bfd),
            _ => theme::ACCENT,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Page::System => "system",
            Page::Display => "display",
            Page::Sound => "sound",
            Page::Power => "power",
            Page::Storage => "storage",
            Page::About => "about",
            Page::Devices => "devices",
            Page::Mouse => "mouse",
            Page::Network => "network",
            Page::Personalization => "personalization",
            Page::Apps => "apps",
            Page::Installed => "installed",
            Page::Startup => "startup",
            Page::Accounts => "accounts",
            Page::Time => "time",
            Page::Update => "update",
            Page::Delfin => "delfin",
        }
    }
}

/// The page for a Delfin page name or an `ms-settings:` link (None = Delfin
/// has no such page).
pub fn page_for(uri: &str) -> Option<Page> {
    let p = uri.trim().trim_start_matches("ms-settings:").to_lowercase();
    let p = p.as_str();
    let all = [
        Page::System,
        Page::Display,
        Page::Sound,
        Page::Power,
        Page::Storage,
        Page::About,
        Page::Devices,
        Page::Mouse,
        Page::Network,
        Page::Personalization,
        Page::Apps,
        Page::Installed,
        Page::Startup,
        Page::Accounts,
        Page::Time,
        Page::Update,
        Page::Delfin,
    ];
    if let Some(pg) = all.iter().find(|pg| pg.name() == p) {
        return Some(*pg);
    }
    let starts = |s: &str| p.starts_with(s);
    Some(match p {
        "" | "home" => Page::System,
        "apps-volume" => Page::Sound,
        "startupapps" => Page::Startup,
        "personalization-background" | "personalization" | "colors" | "themes" | "taskbar" | "lockscreen" => Page::Personalization,
        _ if starts("display") || starts("screenrotation") || starts("nightlight") => Page::Display,
        _ if starts("sound") => Page::Sound,
        _ if starts("powersleep") || starts("batterysaver") || starts("power") => Page::Power,
        _ if starts("storage") => Page::Storage,
        _ if starts("about") || starts("deviceencryption") => Page::About,
        _ if starts("mousetouchpad") || starts("devices-touchpad") => Page::Mouse,
        _ if starts("bluetooth") || starts("connecteddevices") || starts("printers") || starts("usb") || starts("camera") || starts("autoplay") => {
            Page::Devices
        }
        _ if starts("network") || starts("wifi") || starts("ethernet") || starts("proxy") || starts("airplanemode") || starts("datausage") => {
            Page::Network
        }
        _ if starts("personalization") => Page::Personalization,
        _ if starts("appsfeatures") || starts("optionalfeatures") => Page::Installed,
        _ if starts("apps") || starts("defaultapps") => Page::Apps,
        _ if starts("yourinfo") || starts("accounts") || starts("emailandaccounts") || starts("otherusers") || starts("signinoptions") => {
            Page::Accounts
        }
        _ if starts("dateandtime") || starts("region") || starts("keyboard") || starts("typing") => Page::Time,
        _ if starts("windowsupdate") => Page::Update,
        _ => return None,
    })
}

/// Things the search box finds (English; shown translated).
const SEARCH: &[(&str, Page)] = &[
    ("Brightness", Page::Display),
    ("Resolution", Page::Display),
    ("Refresh rate", Page::Display),
    ("Orientation", Page::Display),
    ("Volume", Page::Sound),
    ("Volume mixer", Page::Sound),
    ("Microphone", Page::Sound),
    ("Sleep", Page::Power),
    ("Battery", Page::Power),
    ("Power plan", Page::Power),
    ("Screen and sleep", Page::Power),
    ("Temporary files", Page::Storage),
    ("Recycle Bin", Page::Storage),
    ("Disk Cleanup", Page::Storage),
    ("Rename this PC", Page::About),
    ("Processor", Page::About),
    ("Device Manager", Page::About),
    ("Bluetooth", Page::Devices),
    ("Printers", Page::Devices),
    ("Pointer speed", Page::Mouse),
    ("Primary mouse button", Page::Mouse),
    ("Wi-Fi", Page::Network),
    ("IP address", Page::Network),
    ("Network adapters", Page::Network),
    ("Dark mode", Page::Personalization),
    ("Background", Page::Personalization),
    ("Wallpaper", Page::Personalization),
    ("Transparency effects", Page::Personalization),
    ("Uninstall", Page::Installed),
    ("Startup apps", Page::Startup),
    ("Optional features", Page::Apps),
    ("Your info", Page::Accounts),
    ("Time zone", Page::Time),
    ("Language", Page::Time),
    ("Date and time", Page::Time),
    ("Region", Page::Time),
    ("Check for updates", Page::Update),
    ("Update history", Page::Update),
    ("Delfin updates", Page::Delfin),
    ("Replace Windows Task Manager", Page::Delfin),
];

// ------------------------------------------------------------------ start

const MUTEX: &str = "Local\\DelfinSettings";

fn hwnd_file() -> PathBuf {
    std::env::temp_dir().join("delfin-settings.hwnd")
}

fn goto_file() -> PathBuf {
    std::env::temp_dir().join("delfin-settings.goto")
}

/// Signalled by a second start after it wrote the page it wants.
const GOTO_EVENT: &str = "Local\\DelfinSettingsGoto";
static GOTO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Wait (without polling) for a second start asking for a page.
fn spawn_goto_waiter(ctx: egui::Context) {
    std::thread::spawn(move || unsafe {
        use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
        let name = sys::wide(GOTO_EVENT);
        let ev = CreateEventW(std::ptr::null(), 0, 0, name.as_ptr());
        if ev.is_null() {
            return;
        }
        while WaitForSingleObject(ev, INFINITE) == 0 {
            GOTO.store(true, std::sync::atomic::Ordering::Relaxed);
            ctx.request_repaint();
        }
    });
}

pub fn run(page: Option<String>) -> Result<(), eframe::Error> {
    let start = page.as_deref().and_then(page_for).unwrap_or(Page::System);
    // One window: a second start shows the open one at the asked page.
    unsafe {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        let m = sys::wide(MUTEX);
        let h = windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 0, m.as_ptr());
        if !h.is_null() && GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = std::fs::write(goto_file(), start.name());
            {
                use windows_sys::Win32::System::Threading::{OpenEventW, SetEvent};
                let name = sys::wide(GOTO_EVENT);
                let ev = OpenEventW(0x0002 /* EVENT_MODIFY_STATE */, 0, name.as_ptr());
                if !ev.is_null() {
                    SetEvent(ev);
                    windows_sys::Win32::Foundation::CloseHandle(ev);
                }
            }
            if let Some(w) = std::fs::read_to_string(hwnd_file()).ok().and_then(|s| s.trim().parse::<isize>().ok()) {
                use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow, SetForegroundWindow, ShowWindow};
                let w = w as windows_sys::Win32::Foundation::HWND;
                if IsWindow(w) != 0 {
                    if IsIconic(w) != 0 {
                        ShowWindow(w, 9);
                    }
                    SetForegroundWindow(w);
                }
            }
            return Ok(());
        }
    }
    let _ = std::fs::remove_file(goto_file());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(tr("Settings"))
            .with_app_id("delfin-settings")
            .with_icon(theme::app_icon())
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([780.0, 520.0]),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "Delfin Settings",
        options,
        Box::new(move |cc| {
            theme::install_fonts(&cc.egui_ctx);
            let dark = crate::config::Config::load().dark;
            theme::apply(&cc.egui_ctx, dark);
            Ok(Box::new(Settings::new(&cc.egui_ctx, dark, start)))
        }),
    )
}

// ------------------------------------------------------------------ state

#[derive(Default)]
struct Wu {
    checking: bool,
    installing: bool,
    checked: Option<chrono::DateTime<chrono::Local>>,
    updates: Vec<sys::Update>,
    history: Vec<sys::HistoryItem>,
    error: Option<String>,
    restart: bool,
}

struct DisplayState {
    monitors: Vec<crate::display::Monitor>,
    sel: usize,
    /// Waiting for "Keep changes": (device, previous mode, since).
    pending: Option<(String, crate::display::Mode, Instant)>,
}

#[derive(Clone, Copy)]
struct Look {
    pal: Palette,
    card: Color32,
    card_hover: Color32,
    line: Color32,
}

struct Settings {
    pal: Palette,
    dark: bool,
    page: Page,
    back: Vec<Page>,
    query: String,
    hwnd: isize,

    about: Slot<sys::About>,
    about_v: Option<sys::About>,
    account: Slot<sys::Account>,
    account_v: Option<sys::Account>,
    avatar: Option<TextureHandle>,
    /// Volume, brightness and Wi-Fi: started the first time a page needs them.
    quick: Option<QuickPanel>,
    ctx: egui::Context,
    display: Option<DisplayState>,
    plans: Option<Vec<sys::Plan>>,
    timeouts: [u32; 4],
    drives: Option<Vec<sys::Drive>>,
    temp: Slot<u64>,
    temp_v: Option<u64>,
    bin: (u64, u64),
    adapters: Slot<Vec<sys::Adapter>>,
    adapters_v: Option<Vec<sys::Adapter>>,
    adapters_at: Option<Instant>,
    open_adapters: HashSet<String>,
    mouse: Option<sys::Mouse>,
    wallpaper: Option<PathBuf>,
    wall_px: Slot<Pixels>,
    wall_tex: Option<TextureHandle>,
    gallery: Slot<Vec<(PathBuf, Pixels)>>,
    gallery_tex: Option<Vec<(PathBuf, TextureHandle)>>,
    apps: Slot<Vec<(sys::InstalledApp, Option<Pixels>)>>,
    apps_v: Option<Vec<sys::InstalledApp>>,
    app_icons: HashMap<String, TextureHandle>,
    app_px: HashMap<String, Pixels>,
    app_query: String,
    app_sort: usize,
    startup: Option<Vec<StartupItem>>,
    zones: Option<Vec<(String, String)>>,
    zone: String,
    wu: Arc<Mutex<Wu>>,
    devices: Slot<Vec<crate::devmgr::sys::Device>>,
    devices_v: Option<Vec<crate::devmgr::sys::Device>>,
    class_icons: HashMap<u128, Option<TextureHandle>>,
    bt: Slot<Option<bool>>,
    bt_v: Option<Option<bool>>,
    updater: crate::update::Updater,
    rename: Option<String>,
    armed: Option<(String, Instant)>,
    toasts: Arc<Mutex<Vec<(String, bool)>>>,
    toast: Option<(String, Instant, bool)>,
    /// Background jobs that changed something: reload this page.
    reload: Arc<Mutex<HashSet<Page>>>,
    /// Values read when a page opens (not on every frame).
    cached: Cached,
}

#[derive(Default)]
struct Cached {
    auto_time: bool,
    time_server: String,
    fit: usize,
    transparency: bool,
    tm_replaced: bool,
    battery: Option<(u8, bool)>,
}

impl Settings {
    fn new(ctx: &egui::Context, dark: bool, page: Page) -> Self {
        let mut s = Self {
            pal: theme::palette(dark),
            dark,
            page,
            back: Vec::new(),
            query: String::new(),
            hwnd: 0,
            about: spawn(ctx, sys::about),
            about_v: None,
            account: spawn(ctx, sys::account),
            account_v: None,
            avatar: None,
            quick: None,
            ctx: ctx.clone(),
            display: None,
            plans: None,
            timeouts: [0; 4],
            drives: None,
            temp: Arc::new(Mutex::new(None)),
            temp_v: None,
            bin: (0, 0),
            adapters: Arc::new(Mutex::new(None)),
            adapters_v: None,
            adapters_at: None,
            open_adapters: HashSet::new(),
            mouse: None,
            wallpaper: sys::wallpaper(),
            wall_px: Arc::new(Mutex::new(None)),
            wall_tex: None,
            gallery: Arc::new(Mutex::new(None)),
            gallery_tex: None,
            apps: Arc::new(Mutex::new(None)),
            apps_v: None,
            app_icons: HashMap::new(),
            app_px: HashMap::new(),
            app_query: String::new(),
            app_sort: 0,
            startup: None,
            zones: None,
            zone: String::new(),
            wu: Arc::new(Mutex::new(Wu::default())),
            devices: Arc::new(Mutex::new(None)),
            devices_v: None,
            class_icons: HashMap::new(),
            bt: Arc::new(Mutex::new(None)),
            bt_v: None,
            updater: crate::update::Updater::default(),
            rename: None,
            armed: None,
            toasts: Arc::new(Mutex::new(Vec::new())),
            toast: None,
            reload: Arc::new(Mutex::new(HashSet::new())),
            cached: Cached::default(),
        };
        spawn_goto_waiter(ctx.clone());
        s.load_wallpaper(ctx);
        s.enter(ctx, page);
        s
    }

    /// Volume, brightness and Wi-Fi, started on first use.
    fn qp(&mut self) -> &mut QuickPanel {
        let ctx = self.ctx.clone();
        self.quick.get_or_insert_with(|| QuickPanel::new(&ctx))
    }

    fn look(&self) -> Look {
        let p = self.pal;
        let hex = |v: u32| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        if self.dark {
            Look { pal: p, card: hex(0x2b2f33), card_hover: hex(0x33383d), line: hex(0x24282b) }
        } else {
            Look { pal: p, card: hex(0xfbfbfb), card_hover: hex(0xf3f4f5), line: hex(0xe3e5e7) }
        }
    }

    fn say(&self, msg: impl Into<String>, bad: bool) {
        self.toasts.lock().unwrap_or_else(|p| p.into_inner()).push((msg.into(), bad));
    }

    /// A second click within 5 s confirms `key`.
    fn confirm(&mut self, key: &str) -> bool {
        if self.armed.as_ref().is_some_and(|(k, t)| k == key && t.elapsed() < Duration::from_secs(5)) {
            self.armed = None;
            true
        } else {
            self.armed = Some((key.to_string(), Instant::now()));
            false
        }
    }

    fn is_armed(&self, key: &str) -> bool {
        self.armed.as_ref().is_some_and(|(k, t)| k == key && t.elapsed() < Duration::from_secs(5))
    }

    fn go(&mut self, ctx: &egui::Context, page: Page) {
        if page != self.page {
            self.back.push(self.page);
            if self.back.len() > 50 {
                self.back.remove(0);
            }
            self.enter(ctx, page);
        }
        self.query.clear();
    }

    /// Load what a page shows when it is opened.
    fn enter(&mut self, ctx: &egui::Context, page: Page) {
        if self.page == Page::Network && page != Page::Network {
            if let Some(q) = &self.quick {
                q.wifi.set_active(false);
            }
        }
        self.page = page;
        self.rename = None;
        match page {
            Page::Display => {
                let monitors = crate::display::monitors();
                let sel = monitors.iter().position(|m| m.primary).unwrap_or(0);
                self.display = Some(DisplayState { monitors, sel, pending: None });
                self.qp();
            }
            Page::Sound => self.qp().refresh_volume(),
            Page::Power => {
                self.cached.battery = quick::battery();
                self.plans = Some(sys::plans());
                self.timeouts = sys::timeouts();
            }
            Page::Storage => {
                self.drives = Some(sys::drives());
                self.bin = sys::recycle_bin();
                self.temp_v = None;
                self.temp = spawn(ctx, || sys::dir_size(&std::env::temp_dir()));
            }
            Page::Devices => {
                self.devices = spawn(ctx, || crate::devmgr::sys::list(false));
                self.bt = spawn(ctx, sys::bluetooth);
            }
            Page::Mouse => self.mouse = Some(sys::mouse()),
            Page::Network => {
                self.qp().wifi.set_active(true);
                self.adapters = spawn(ctx, sys::adapters);
                self.adapters_at = Some(Instant::now());
            }
            Page::Personalization => {
                self.cached.fit = sys::wallpaper_fit();
                self.cached.transparency = sys::transparency();
                self.wallpaper = sys::wallpaper();
                self.load_wallpaper(ctx);
                if self.gallery_tex.is_none() {
                    self.gallery = spawn(ctx, gallery);
                }
            }
            Page::Installed if self.apps_v.is_none() => {
                self.apps = spawn(ctx, || {
                    sys::installed_apps()
                        .into_iter()
                        .map(|a| {
                            let px = a.icon.as_ref().and_then(|p| crate::fileicons::icon_pixels(p, 1));
                            (a, px)
                        })
                        .collect()
                });
            }
            Page::Startup => self.startup = Some(crate::taskmgr::sys::startup_items()),
            Page::Time => {
                if self.zones.is_none() {
                    self.zones = Some(sys::time_zones());
                }
                self.zone = sys::current_time_zone();
                self.cached.auto_time = sys::auto_time();
                self.cached.time_server = sys::time_server();
            }
            Page::Delfin => self.cached.tm_replaced = crate::taskmgr::replaced(),
            Page::Update => {
                let fresh = self.wu.lock().unwrap_or_else(|p| p.into_inner()).checked.is_none();
                if fresh {
                    self.check_updates(ctx);
                }
            }
            _ => {}
        }
    }

    fn load_wallpaper(&mut self, ctx: &egui::Context) {
        self.wall_tex = None;
        if let Some(p) = self.wallpaper.clone() {
            self.wall_px = spawn(ctx, move || sys::load_image(&p, 480).unwrap_or((0, 0, Vec::new())));
        }
    }

    fn check_updates(&mut self, ctx: &egui::Context) {
        let wu = self.wu.clone();
        {
            let mut w = wu.lock().unwrap_or_else(|p| p.into_inner());
            if w.checking || w.installing {
                return;
            }
            w.checking = true;
            w.error = None;
        }
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = sys::search_updates();
            let mut w = wu.lock().unwrap_or_else(|p| p.into_inner());
            w.checking = false;
            w.checked = Some(chrono::Local::now());
            w.restart = sys::reboot_pending();
            match r {
                Ok((u, h)) => {
                    w.updates = u;
                    w.history = h;
                }
                Err(e) => w.error = Some(e),
            }
            ctx.request_repaint();
        });
    }

    fn install_updates(&mut self, ctx: &egui::Context) {
        let wu = self.wu.clone();
        wu.lock().unwrap_or_else(|p| p.into_inner()).installing = true;
        let toasts = self.toasts.clone();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let r = sys::install_updates();
            {
                let mut w = wu.lock().unwrap_or_else(|p| p.into_inner());
                w.installing = false;
                match r {
                    Ok(restart) => {
                        w.restart = restart || sys::reboot_pending();
                        w.updates.clear();
                        toasts.lock().unwrap().push((tr("Updates installed.").to_string(), false));
                    }
                    Err(u32::MAX) => toasts.lock().unwrap().push((tr("Administrator rights are needed.").to_string(), true)),
                    Err(c) => toasts.lock().unwrap().push((trf("Some updates could not be installed (code {}).", &[&c]), true)),
                }
                w.checked = None;
            }
            ctx2.request_repaint();
        });
    }

    /// Run `f` in the background; its message becomes a toast and `page` reloads.
    fn job(&self, ctx: &egui::Context, page: Page, f: impl FnOnce() -> (String, bool) + Send + 'static) {
        let (toasts, reload, ctx) = (self.toasts.clone(), self.reload.clone(), ctx.clone());
        std::thread::spawn(move || {
            unsafe { windows_sys::Win32::System::Com::CoInitializeEx(std::ptr::null(), 0x2) };
            let (msg, bad) = f();
            if !msg.is_empty() {
                toasts.lock().unwrap_or_else(|p| p.into_inner()).push((msg, bad));
            }
            reload.lock().unwrap_or_else(|p| p.into_inner()).insert(page);
            ctx.request_repaint();
        });
    }

    fn set_dark(&mut self, ctx: &egui::Context, dark: bool) {
        let mut cfg = crate::config::Config::load();
        cfg.dark = dark;
        cfg.save();
        sys::set_windows_light(!dark);
        self.dark = dark;
        self.pal = theme::palette(dark);
        theme::apply(ctx, dark);
    }

    /// Pick up results from background threads.
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(a) = take(&self.about) {
            self.about_v = Some(a);
        }
        if let Some(a) = take(&self.account) {
            if let Some((w, h, px)) = &a.picture {
                let px = round_mask(*w, *h, px);
                self.avatar = Some(ctx.load_texture("avatar", egui::ColorImage::from_rgba_unmultiplied([*w, *h], &px), egui::TextureOptions::LINEAR));
            }
            self.account_v = Some(a);
        }
        if let Some((w, h, px)) = take(&self.wall_px) {
            if w > 0 {
                self.wall_tex = Some(ctx.load_texture("wallpaper", egui::ColorImage::from_rgba_unmultiplied([w, h], &px), egui::TextureOptions::LINEAR));
            }
        }
        if let Some(list) = take(&self.gallery) {
            self.gallery_tex = Some(
                list.into_iter()
                    .enumerate()
                    .map(|(i, (p, (w, h, px)))| {
                        let t = ctx.load_texture(format!("gal{i}"), egui::ColorImage::from_rgba_unmultiplied([w, h], &px), egui::TextureOptions::LINEAR);
                        (p, t)
                    })
                    .collect(),
            );
        }
        if let Some(v) = take(&self.temp) {
            self.temp_v = Some(v);
        }
        if let Some(v) = take(&self.adapters) {
            self.adapters_v = Some(v);
        }
        if let Some(v) = take(&self.devices) {
            self.devices_v = Some(v);
        }
        if let Some(v) = take(&self.bt) {
            self.bt_v = Some(v);
        }
        if let Some(list) = take(&self.apps) {
            let mut apps = Vec::new();
            for (a, px) in list {
                if let Some(px) = px {
                    self.app_px.insert(a.name.clone(), px);
                }
                apps.push(a);
            }
            self.apps_v = Some(apps);
        }
        let msgs: Vec<(String, bool)> = std::mem::take(&mut *self.toasts.lock().unwrap_or_else(|p| p.into_inner()));
        if let Some((m, bad)) = msgs.into_iter().last() {
            self.toast = Some((m, Instant::now(), bad));
        }
        let reload: Vec<Page> = self.reload.lock().unwrap_or_else(|p| p.into_inner()).drain().collect();
        for p in reload {
            if p == Page::Installed {
                self.apps_v = None;
            }
            if p == self.page {
                self.enter(ctx, p);
            }
        }
        // Network details go stale; refresh while the page is open.
        if self.page == Page::Network && self.adapters_at.is_some_and(|t| t.elapsed() > Duration::from_secs(5)) {
            self.adapters = spawn(ctx, sys::adapters);
            self.adapters_at = Some(Instant::now());
        }
        // Another start asked for a page.
        if GOTO.swap(false, std::sync::atomic::Ordering::Relaxed) {
            if let Ok(name) = std::fs::read_to_string(goto_file()) {
                let _ = std::fs::remove_file(goto_file());
                if let Some(p) = page_for(&name) {
                    self.go(ctx, p);
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
        if self.hwnd == 0 {
            self.hwnd = own_window();
            if self.hwnd != 0 {
                let _ = std::fs::write(hwnd_file(), self.hwnd.to_string());
            }
        }
    }
}

/// This process's main window.
fn own_window() -> isize {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible};
    unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> BOOL {
        let out = &mut *(lp as *mut (u32, isize));
        let mut pid = 0;
        GetWindowThreadProcessId(h, &mut pid);
        if pid == out.0 && IsWindowVisible(h) != 0 {
            out.1 = h as isize;
            return 0;
        }
        1
    }
    let mut st = (std::process::id(), 0isize);
    unsafe { EnumWindows(Some(each), &mut st as *mut _ as LPARAM) };
    st.1
}

/// Windows' own wallpapers, as thumbnails.
fn gallery() -> Vec<(PathBuf, Pixels)> {
    let mut files = Vec::new();
    let root = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("Web").join("Wallpaper");
    if let Ok(rd) = std::fs::read_dir(&root) {
        for d in rd.flatten() {
            if let Ok(inner) = std::fs::read_dir(d.path()) {
                for f in inner.flatten() {
                    let p = f.path();
                    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                    if matches!(ext.as_str(), "jpg" | "jpeg" | "png") {
                        files.push(p);
                    }
                }
            }
        }
    }
    files.sort();
    files.truncate(12);
    files.into_iter().filter_map(|p| sys::load_image(&p, 200).map(|i| (p, i))).collect()
}

/// Make the corners of a square picture transparent (a round avatar).
fn round_mask(w: usize, h: usize, px: &[u8]) -> Vec<u8> {
    let mut out = px.to_vec();
    let (cx, cy, r) = (w as f32 / 2.0, h as f32 / 2.0, w.min(h) as f32 / 2.0);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let a = (r - d + 0.5).clamp(0.0, 1.0);
            let i = (y * w + x) * 4 + 3;
            out[i] = (out[i] as f32 * a) as u8;
        }
    }
    out
}

// ------------------------------------------------------------------ the window

impl eframe::App for Settings {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        // Redraw on input and when background work finishes; timers only for
        // what changes by itself.
        let mut wake: Option<Duration> = None;
        let mut soon = |d: Duration| wake = Some(wake.map_or(d, |w| w.min(d)));
        if self.page == Page::Network {
            soon(Duration::from_secs(5)); // adapter details
        }
        if self.page == Page::Time {
            soon(Duration::from_millis(1000 - chrono::Local::now().timestamp_subsec_millis().min(999) as u64 + 10));
        }
        if self.display.as_ref().is_some_and(|d| d.pending.is_some()) {
            soon(Duration::from_millis(250)); // "keep changes" countdown
        }
        if let Some((_, t)) = &self.armed {
            soon(Duration::from_secs(5).saturating_sub(t.elapsed()) + Duration::from_millis(20));
        }
        if let Some((_, t, _)) = &self.toast {
            soon(Duration::from_secs(6).saturating_sub(t.elapsed()) + Duration::from_millis(20));
        }
        if let Some(d) = wake {
            ctx.request_repaint_after(d);
        }
        let lk = self.look();
        let pal = self.pal;

        if ctx.input(|i| i.modifiers.alt && i.key_pressed(Key::ArrowLeft)) || ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra1)) {
            if let Some(p) = self.back.pop() {
                self.enter(ctx, p);
            }
        }

        // ---- side bar
        egui::SidePanel::left("set-side")
            .resizable(false)
            .show_separator_line(false)
            .exact_width(296.0)
            .frame(Frame::none().fill(pal.window).inner_margin(Margin { left: 14.0, right: 10.0, top: 14.0, bottom: 10.0 }))
            .show(ctx, |ui| self.side_bar(ui, ctx, lk));

        // ---- page
        egui::CentralPanel::default()
            .frame(Frame::none().fill(pal.window).inner_margin(Margin { left: 18.0, right: 8.0, top: 14.0, bottom: 0.0 }))
            .show(ctx, |ui| {
                self.page_title(ui, ctx, lk);
                ui.add_space(10.0);
                ScrollArea::vertical().id_salt(("page", self.page)).auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_max_width(1040.0_f32.min(ui.available_width() - 16.0));
                    ui.spacing_mut().item_spacing.y = 3.0;
                    match self.page {
                        Page::System => self.system_page(ui, ctx, lk),
                        Page::Display => self.display_page(ui, ctx, lk),
                        Page::Sound => self.sound_page(ui, lk),
                        Page::Power => self.power_page(ui, lk),
                        Page::Storage => self.storage_page(ui, ctx, lk),
                        Page::About => self.about_page(ui, ctx, lk),
                        Page::Devices => self.devices_page(ui, ctx, lk),
                        Page::Mouse => self.mouse_page(ui, lk),
                        Page::Network => self.network_page(ui, ctx, lk),
                        Page::Personalization => self.personalization_page(ui, ctx, lk),
                        Page::Apps => self.apps_page(ui, ctx, lk),
                        Page::Installed => self.installed_page(ui, ctx, lk),
                        Page::Startup => self.startup_page(ui, lk),
                        Page::Accounts => self.accounts_page(ui, lk),
                        Page::Time => self.time_page(ui, ctx, lk),
                        Page::Update => self.update_page(ui, ctx, lk),
                        Page::Delfin => self.delfin_page(ui, ctx, lk),
                    }
                    ui.add_space(24.0);
                });
            });

        // ---- toast
        if let Some((m, t, bad)) = &self.toast {
            if t.elapsed() < Duration::from_secs(6) {
                let screen = ctx.screen_rect();
                egui::Area::new(egui::Id::new("toast"))
                    .fixed_pos(pos2(screen.right() - 24.0, screen.bottom() - 24.0))
                    .pivot(Align2::RIGHT_BOTTOM)
                    .show(ctx, |ui| {
                        Frame::none()
                            .fill(lk.card)
                            .stroke(Stroke::new(1.0_f32, if *bad { pal.danger } else { pal.accent }))
                            .rounding(8.0)
                            .inner_margin(Margin::symmetric(16.0, 10.0))
                            .show(ui, |ui| {
                                ui.set_max_width(420.0);
                                ui.label(RichText::new(m.as_str()).color(if *bad { pal.danger } else { pal.text }));
                            });
                    });
            } else {
                self.toast = None;
            }
        }
    }
}

impl Settings {
    fn side_bar(&mut self, ui: &mut Ui, ctx: &egui::Context, lk: Look) {
        let pal = self.pal;
        // Account
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 72.0), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(r, 6.0, lk.card);
        }
        let ac = pos2(r.left() + 36.0, r.center().y);
        self.paint_avatar(ui.painter(), ac, 28.0);
        let (name, sub) = match (&self.about_v, &self.account_v) {
            (Some(a), Some(acc)) => (a.full_name.clone(), acc.email.clone().unwrap_or_else(|| tr("Local account").to_string())),
            (Some(a), None) => (a.full_name.clone(), String::new()),
            _ => (std::env::var("USERNAME").unwrap_or_default(), String::new()),
        };
        let tx = r.left() + 76.0;
        let tw = r.right() - tx - 6.0;
        paint_elided(ui, pos2(tx, r.center().y - 10.0), &name, 15.0, pal.text, tw, true);
        paint_elided(ui, pos2(tx, r.center().y + 10.0), &sub, 12.0, pal.dim, tw, false);
        if resp.clicked() {
            self.go(ctx, Page::Accounts);
        }
        ui.add_space(10.0);

        // Search
        let te = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .hint_text(tr("Find a setting"))
                .desired_width(ui.available_width())
                .margin(vec2(10.0, 7.0)),
        );
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::F)) {
            te.request_focus();
        }
        ui.add_space(10.0);

        let q = self.query.trim().to_lowercase();
        if !q.is_empty() {
            let mut hits: Vec<(&str, Page)> = Vec::new();
            for pg in [
                Page::System,
                Page::Display,
                Page::Sound,
                Page::Power,
                Page::Storage,
                Page::About,
                Page::Devices,
                Page::Mouse,
                Page::Network,
                Page::Personalization,
                Page::Apps,
                Page::Installed,
                Page::Startup,
                Page::Accounts,
                Page::Time,
                Page::Update,
                Page::Delfin,
            ] {
                if pg.title().to_lowercase().contains(&q) {
                    hits.push((pg.title(), pg));
                }
            }
            for (label, pg) in SEARCH {
                let t = tr(label);
                if (t.to_lowercase().contains(&q) || label.to_lowercase().contains(&q)) && !hits.iter().any(|h| h.0 == t) {
                    hits.push((t, *pg));
                }
            }
            if hits.is_empty() {
                ui.label(RichText::new(tr("No results")).color(pal.dim));
            }
            let enter = te.lost_focus() && ctx.input(|i| i.key_pressed(Key::Enter));
            if enter {
                if let Some(&(_, pg)) = hits.first() {
                    self.go(ctx, pg);
                    return;
                }
            }
            ScrollArea::vertical().id_salt("hits").show(ui, |ui| {
                for (label, pg) in hits {
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 6.0, lk.card);
                    }
                    paint_ico(ui.painter(), Rect::from_center_size(pos2(r.left() + 20.0, r.center().y), vec2(18.0, 18.0)), pg.icon(), pg.tint(), &pal);
                    ui.painter().text(pos2(r.left() + 42.0, r.center().y - 7.0), Align2::LEFT_CENTER, label, FontId::proportional(13.5), pal.text);
                    let trail = if pg.parent() != pg { format!("{} › {}", pg.parent().title(), pg.title()) } else { pg.title().to_string() };
                    ui.painter().text(pos2(r.left() + 42.0, r.center().y + 9.0), Align2::LEFT_CENTER, trail, FontId::proportional(11.0), pal.dim);
                    if resp.clicked() {
                        self.go(ctx, pg);
                    }
                }
            });
            return;
        }

        // Pages
        for pg in TOP {
            let on = self.page.parent() == pg;
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
            if on {
                ui.painter().rect_filled(r, 6.0, lk.card_hover);
                let bar = Rect::from_center_size(pos2(r.left() + 2.0, r.center().y), vec2(3.0, 16.0));
                ui.painter().rect_filled(bar, 1.5, pal.accent);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 6.0, lk.card);
            }
            paint_ico(ui.painter(), Rect::from_center_size(pos2(r.left() + 22.0, r.center().y), vec2(18.0, 18.0)), pg.icon(), pg.tint(), &pal);
            ui.painter().text(pos2(r.left() + 46.0, r.center().y), Align2::LEFT_CENTER, pg.title(), FontId::proportional(14.0), pal.text);
            if resp.clicked() {
                self.go(ctx, pg);
            }
            ui.add_space(2.0);
        }
    }

    fn paint_avatar(&self, p: &egui::Painter, c: egui::Pos2, r: f32) {
        if let Some(t) = &self.avatar {
            let rect = Rect::from_center_size(c, vec2(r * 2.0, r * 2.0));
            p.image(t.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        } else {
            p.circle_filled(c, r, self.pal.accent);
            let name = self.about_v.as_ref().map(|a| a.full_name.clone()).unwrap_or_else(|| std::env::var("USERNAME").unwrap_or_default());
            let initial: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
            p.text(c, Align2::CENTER_CENTER, initial, FontId::proportional(r * 0.9), Color32::WHITE);
        }
    }

    fn page_title(&mut self, ui: &mut Ui, ctx: &egui::Context, lk: Look) {
        let pal = lk.pal;
        ui.horizontal(|ui| {
            if !self.back.is_empty() {
                let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r, 6.0, lk.card);
                }
                icons::paint_tool(ui.painter(), r, Tool::Back, pal.text);
                if resp.on_hover_text(tr("Back")).clicked() {
                    if let Some(p) = self.back.pop() {
                        self.enter(ctx, p);
                    }
                }
                ui.add_space(4.0);
            }
            let parent = self.page.parent();
            if parent != self.page {
                let r = ui.add(egui::Label::new(RichText::new(parent.title()).size(26.0).color(pal.dim)).sense(Sense::click()));
                if r.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if r.clicked() {
                    self.go(ctx, parent);
                }
                ui.label(RichText::new("›").size(26.0).color(pal.dim));
            }
            ui.label(RichText::new(self.page.title()).size(26.0).strong());
        });
    }
}

// ------------------------------------------------------------------ building blocks

fn paint_elided(ui: &Ui, pos: egui::Pos2, text: &str, size: f32, col: Color32, max_w: f32, strong: bool) {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat { font_id: FontId::proportional(size), color: col, ..Default::default() },
    );
    job.wrap = egui::text::TextWrapping { max_width: max_w.max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = ui.fonts(|f| f.layout_job(job));
    let p = pos2(pos.x, pos.y - g.size().y / 2.0);
    ui.painter().galley(p, g.clone(), col);
    if strong {
        ui.painter().galley(p + vec2(0.3, 0.0), g, col);
    }
}

fn section(ui: &mut Ui, text: &str) {
    ui.add_space(16.0);
    ui.label(RichText::new(text).size(14.0).strong());
    ui.add_space(5.0);
}

/// A Windows 11 settings card: icon, title, description and controls on the
/// right (`ctrl_w` wide). `link` rows are clickable and get a chevron.
#[allow(clippy::too_many_arguments)]
fn row_with(ui: &mut Ui, lk: Look, ico: Option<Ico>, title: &str, sub: &str, link: bool, ctrl_w: f32, right: impl FnOnce(&mut Ui)) -> Response {
    let pal = lk.pal;
    let h = if sub.is_empty() { 54.0 } else { 66.0 };
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, h), if link { Sense::click() } else { Sense::hover() });
    let fill = if link && resp.hovered() { lk.card_hover } else { lk.card };
    ui.painter().rect(r, 6.0, fill, Stroke::new(1.0_f32, lk.line));
    let mut x = r.left() + 18.0;
    if let Some(i) = ico {
        paint_ico(ui.painter(), Rect::from_center_size(pos2(x + 10.0, r.center().y), vec2(20.0, 20.0)), i, pal.text, &pal);
        x += 40.0;
    }
    let right_edge = r.right() - if link { 46.0 } else { 18.0 };
    let text_w = right_edge - ctrl_w - x - 12.0;
    if sub.is_empty() {
        paint_elided(ui, pos2(x, r.center().y), title, 14.0, pal.text, text_w, false);
    } else {
        paint_elided(ui, pos2(x, r.center().y - 9.0), title, 14.0, pal.text, text_w, false);
        paint_elided(ui, pos2(x, r.center().y + 11.0), sub, 12.0, pal.dim, text_w, false);
    }
    if link {
        icons::paint_tool(ui.painter(), Rect::from_center_size(pos2(r.right() - 26.0, r.center().y), vec2(16.0, 16.0)), Tool::Forward, pal.dim);
    }
    if ctrl_w > 0.0 {
        let cr = Rect::from_min_max(pos2(right_edge - ctrl_w, r.top()), pos2(right_edge, r.bottom()));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(cr).layout(Layout::right_to_left(Align::Center)));
        right(&mut child);
    }
    if link && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

fn row(ui: &mut Ui, lk: Look, ico: Option<Ico>, title: &str, sub: &str, link: bool) -> Response {
    row_with(ui, lk, ico, title, sub, link, 0.0, |_| {})
}

/// A row that opens something outside this window (↗ instead of ›).
fn ext_row(ui: &mut Ui, lk: Look, ico: Ico, title: &str, sub: &str) -> bool {
    let pal = lk.pal;
    let resp = row_with(ui, lk, Some(ico), title, sub, false, 24.0, |ui| {
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        paint_ico(ui.painter(), r, Ico::Link, pal.dim, &pal);
    });
    let resp = resp.interact(Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

/// A card with free content.
fn card(ui: &mut Ui, lk: Look, add: impl FnOnce(&mut Ui)) {
    Frame::none()
        .fill(lk.card)
        .stroke(Stroke::new(1.0_f32, lk.line))
        .rounding(6.0)
        .inner_margin(Margin::symmetric(18.0, 14.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            add(ui);
        });
}

/// Label / value lines inside a card.
fn kv(ui: &mut Ui, pal: &Palette, k: &str, v: &str) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(vec2(180.0, 20.0), Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_width(180.0);
            ui.add(egui::Label::new(RichText::new(k).color(pal.dim)).truncate());
        });
        ui.add(egui::Label::new(v).wrap());
    });
}

fn accent_button(ui: &mut Ui, pal: &Palette, text: &str, enabled: bool) -> Response {
    let b = egui::Button::new(RichText::new(text).color(Color32::WHITE)).fill(pal.accent).min_size(vec2(110.0, 30.0)).rounding(4.0);
    ui.add_enabled(enabled, b)
}

fn button(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Button::new(text).min_size(vec2(96.0, 30.0)).rounding(4.0))
}

/// Windows-style on/off switch with its "On"/"Off" label.
fn switch(ui: &mut Ui, pal: &Palette, on: &mut bool) -> bool {
    let changed = quick::toggle(ui, on, pal).changed();
    ui.label(if *on { tr("On") } else { tr("Off") });
    changed
}

fn combo<T: PartialEq + Copy>(ui: &mut Ui, id: &str, width: f32, value: &mut T, items: &[(T, String)]) -> bool {
    let before = *value;
    let current = items.iter().find(|(v, _)| *v == *value).map(|(_, s)| s.clone()).unwrap_or_default();
    egui::ComboBox::from_id_salt(id).width(width).selected_text(current).height(360.0).show_ui(ui, |ui| {
        for (v, label) in items {
            ui.selectable_value(value, *v, label);
        }
    });
    *value != before
}

/// Usage bar (0..1).
fn bar(ui: &mut Ui, pal: &Palette, frac: f32, w: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(w, 6.0), Sense::hover());
    ui.painter().rect_filled(r, 3.0, pal.border);
    let mut f = r;
    f.set_width(r.width() * frac.clamp(0.0, 1.0));
    ui.painter().rect_filled(f, 3.0, if frac > 0.9 { pal.danger } else { pal.accent });
}

fn copy_button(ui: &mut Ui, text: &str) {
    if button(ui, tr("Copy")).clicked() {
        ui.ctx().copy_text(text.to_string());
    }
}

// ------------------------------------------------------------------ icons

#[derive(Clone, Copy, PartialEq)]
enum Ico {
    Monitor,
    Speaker,
    Power,
    Battery,
    Drive,
    Info,
    Bluetooth,
    Mouse,
    Keyboard,
    Globe,
    Wifi,
    Ethernet,
    Brush,
    Image,
    Apps,
    List,
    Rocket,
    Person,
    Clock,
    Language,
    Update,
    Delfin,
    Trash,
    Folder,
    Link,
    Shield,
    Chip,
    Mic,
    Sliders,
    Printer,
    Camera,
    Contrast,
    Plus,
    Restart,
    Blank,
}

fn arc_pts(c: egui::Pos2, r: f32, a0: f32, a1: f32) -> Vec<egui::Pos2> {
    (0..=20)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / 20.0;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect()
}

fn paint_ico(p: &egui::Painter, r: Rect, i: Ico, col: Color32, pal: &Palette) {
    use std::f32::consts::PI;
    let c = r.center();
    let s = r.width() / 20.0;
    let st = Stroke::new(1.5_f32, col);
    let v = |x: f32, y: f32| c + vec2(x * s, y * s);
    let rr = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(v(x0, y0), v(x1, y1));
    let line = |pts: Vec<egui::Pos2>| {
        p.add(Shape::line(pts, st));
    };
    match i {
        Ico::Monitor => {
            p.rect_stroke(rr(-9.0, -7.0, 9.0, 5.0), 2.0, st);
            line(vec![v(0.0, 5.0), v(0.0, 8.5)]);
            line(vec![v(-4.5, 8.5), v(4.5, 8.5)]);
        }
        Ico::Speaker => quick::paint_speaker(p, r, col, Some(0.7)),
        Ico::Power => icons::paint_tool(p, r, Tool::Power, col),
        Ico::Battery => {
            p.rect_stroke(rr(-9.0, -5.0, 7.0, 5.0), 2.0, st);
            p.rect_filled(rr(8.0, -2.0, 10.0, 2.0), 1.0, col);
            p.rect_filled(rr(-7.0, -3.0, 1.0, 3.0), 1.0, col);
        }
        Ico::Drive => {
            p.rect_stroke(rr(-9.0, -5.0, 9.0, 5.0), 3.0, st);
            p.circle_filled(v(5.0, 0.0), 1.4 * s, col);
            line(vec![v(-6.0, 0.0), v(1.0, 0.0)]);
        }
        Ico::Info => {
            p.circle_stroke(c, 9.0 * s, st);
            line(vec![v(0.0, -1.0), v(0.0, 5.0)]);
            p.circle_filled(v(0.0, -4.5), 1.2 * s, col);
        }
        Ico::Bluetooth => line(vec![v(-5.0, -4.5), v(5.0, 4.5), v(0.0, 9.0), v(0.0, -9.0), v(5.0, -4.5), v(-5.0, 4.5)]),
        Ico::Mouse => {
            p.rect_stroke(rr(-6.0, -9.0, 6.0, 9.0), 6.0 * s, st);
            line(vec![v(0.0, -9.0), v(0.0, -3.0)]);
        }
        Ico::Keyboard => {
            p.rect_stroke(rr(-10.0, -6.0, 10.0, 6.0), 2.0, st);
            for x in [-6.0, -2.0, 2.0, 6.0] {
                p.circle_filled(v(x, -2.0), 1.0 * s, col);
            }
            line(vec![v(-5.0, 2.5), v(5.0, 2.5)]);
        }
        Ico::Globe => {
            p.circle_stroke(c, 9.0 * s, st);
            let pts: Vec<_> = (0..=24).map(|k| {
                let a = k as f32 / 24.0 * 2.0 * PI;
                v(a.cos() * 4.0, a.sin() * 9.0)
            }).collect();
            line(pts);
            line(vec![v(-9.0, 0.0), v(9.0, 0.0)]);
        }
        Ico::Wifi => quick::paint_wifi(p, r, col, col, Some(100)),
        Ico::Ethernet => quick::paint_network(p, r, pal, None, true),
        Ico::Brush => {
            p.line_segment([v(8.0, -9.0), v(-1.0, 1.5)], Stroke::new(2.2 * s, col));
            p.circle_filled(v(-4.0, 5.0), 4.0 * s, col);
        }
        Ico::Image => {
            p.rect_stroke(rr(-9.0, -7.0, 9.0, 7.0), 2.0, st);
            line(vec![v(-9.0, 5.0), v(-3.0, -1.0), v(2.0, 4.0), v(5.0, 1.0), v(9.0, 5.0)]);
            p.circle_filled(v(4.0, -3.0), 1.6 * s, col);
        }
        Ico::Apps => {
            for (x, y) in [(-8.0, -8.0), (1.0, -8.0), (-8.0, 1.0), (1.0, 1.0)] {
                p.rect_stroke(rr(x, y, x + 7.0, y + 7.0), 1.5, st);
            }
        }
        Ico::List => {
            for y in [-6.0, 0.0, 6.0] {
                p.circle_filled(v(-8.0, y), 1.3 * s, col);
                line(vec![v(-4.0, y), v(9.0, y)]);
            }
        }
        Ico::Rocket => {
            p.circle_stroke(c, 9.0 * s, st);
            p.add(Shape::convex_polygon(vec![v(-2.5, -4.5), v(5.0, 0.0), v(-2.5, 4.5)], col, Stroke::NONE));
        }
        Ico::Person => {
            p.circle_stroke(v(0.0, -4.0), 4.0 * s, st);
            line(arc_pts(v(0.0, 11.0), 9.0 * s, PI * 1.18, PI * 1.82));
        }
        Ico::Clock => {
            p.circle_stroke(c, 9.0 * s, st);
            line(vec![v(0.0, -5.5), v(0.0, 0.0), v(4.0, 2.5)]);
        }
        Ico::Language => {
            p.text(c, Align2::CENTER_CENTER, "Aa", FontId::proportional(13.0 * s), col);
        }
        Ico::Update | Ico::Restart => {
            line(arc_pts(c, 8.0 * s, -PI * 0.35, PI * 1.35));
            let tip = c + vec2((-PI * 0.35).cos(), (-PI * 0.35).sin()) * 8.0 * s;
            line(vec![tip + vec2(-4.5 * s, -1.0 * s), tip, tip + vec2(-0.5 * s, 4.5 * s)]);
        }
        Ico::Delfin => {
            p.circle_filled(c, 9.5 * s, theme::ACCENT);
            let _ = col;
            let w = Stroke::new(1.6 * s, Color32::WHITE);
            p.add(Shape::line(vec![v(-6.0, 2.0), v(-3.0, -1.5), v(0.0, 2.0), v(3.0, -1.5), v(6.0, 2.0)], w));
        }
        Ico::Trash => {
            line(vec![v(-8.0, -6.0), v(8.0, -6.0)]);
            line(vec![v(-3.0, -6.0), v(-3.0, -9.0), v(3.0, -9.0), v(3.0, -6.0)]);
            line(vec![v(-6.0, -6.0), v(-5.0, 9.0), v(5.0, 9.0), v(6.0, -6.0)]);
        }
        Ico::Folder => {
            line(vec![v(-9.0, 7.0), v(-9.0, -7.0), v(-3.0, -7.0), v(-1.0, -4.0), v(9.0, -4.0), v(9.0, 7.0), v(-9.0, 7.0)]);
        }
        Ico::Link => {
            line(vec![v(1.0, -8.0), v(8.0, -8.0), v(8.0, -1.0)]);
            line(vec![v(8.0, -8.0), v(-1.0, 1.0)]);
            line(vec![v(-2.0, -6.0), v(-8.0, -6.0), v(-8.0, 8.0), v(6.0, 8.0), v(6.0, 2.0)]);
        }
        Ico::Shield => line(vec![v(0.0, -9.0), v(8.0, -6.0), v(7.0, 2.0), v(0.0, 9.0), v(-7.0, 2.0), v(-8.0, -6.0), v(0.0, -9.0)]),
        Ico::Chip => icons::paint_tool(p, r, Tool::Chip, col),
        Ico::Mic => {
            p.rect_stroke(rr(-3.5, -9.0, 3.5, 3.0), 3.5 * s, st);
            line(arc_pts(v(0.0, -1.0), 6.5 * s, 0.0, PI));
            line(vec![v(0.0, 5.5), v(0.0, 9.0)]);
        }
        Ico::Sliders => {
            for (y, x) in [(-5.0, 3.0), (0.0, -3.0), (5.0, 1.0)] {
                line(vec![v(-9.0, y), v(9.0, y)]);
                p.circle_filled(v(x, y), 2.2 * s, col);
            }
        }
        Ico::Printer => {
            p.rect_stroke(rr(-9.0, -3.0, 9.0, 5.0), 2.0, st);
            line(vec![v(-5.0, -3.0), v(-5.0, -9.0), v(5.0, -9.0), v(5.0, -3.0)]);
            p.rect_stroke(rr(-5.0, 2.0, 5.0, 9.0), 0.5, st);
        }
        Ico::Camera => {
            p.rect_stroke(rr(-9.0, -5.0, 9.0, 7.0), 2.0, st);
            p.circle_stroke(v(0.0, 1.0), 3.5 * s, st);
            line(vec![v(-4.0, -5.0), v(-2.5, -8.0), v(2.5, -8.0), v(4.0, -5.0)]);
        }
        Ico::Contrast => {
            p.circle_stroke(c, 9.0 * s, st);
            let mut pts = arc_pts(c, 9.0 * s, -PI / 2.0, PI / 2.0);
            pts.push(c);
            p.add(Shape::convex_polygon(pts, col, Stroke::NONE));
        }
        Ico::Blank => {}
        Ico::Plus => {
            line(vec![v(-7.0, 0.0), v(7.0, 0.0)]);
            line(vec![v(0.0, -7.0), v(0.0, 7.0)]);
        }
    }
    let _ = pal;
}

// ------------------------------------------------------------------ pages

const MINUTES: [u32; 16] = [1, 2, 3, 5, 10, 15, 20, 25, 30, 45, 60, 120, 180, 240, 300, 0];

fn minutes_label(m: u32) -> String {
    match m {
        0 => tr("Never").to_string(),
        1 => tr("1 minute").to_string(),
        m if m < 60 => trf("{} minutes", &[&m]),
        60 => tr("1 hour").to_string(),
        m => trf("{} hours", &[&(m / 60)]),
    }
}

impl Settings {
    // ---------------------------------------------------------- System

    fn system_page(&mut self, ui: &mut Ui, ctx: &egui::Context, lk: Look) {
        let pal = lk.pal;
        // Header: this PC and Windows Update.
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(132.0, 86.0), Sense::hover());
            self.paint_screen(ui.painter(), Rect::from_min_size(r.min, vec2(132.0, 76.0)));
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.add_space(12.0);
                let a = self.about_v.clone().unwrap_or_default();
                ui.label(RichText::new(&a.computer).size(20.0).strong());
                let model = [a.maker.as_str(), a.model.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
                if !model.is_empty() {
                    ui.label(RichText::new(model).color(pal.dim));
                }
                if ui.link(tr("Rename")).clicked() {
                    self.go(ctx, Page::About);
                    self.rename = Some(a.computer.clone());
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let w = self.wu.lock().unwrap_or_else(|p| p.into_inner());
                let state = if w.restart {
                    tr("Restart required")
                } else if w.checking {
                    tr("Checking for updates…")
                } else if !w.updates.is_empty() {
                    tr("Updates available")
                } else {
                    tr("Look for updates")
                };
                drop(w);
                let (r, resp) = ui.allocate_exact_size(vec2(250.0, 64.0), Sense::click());
                ui.painter().rect(r, 6.0, if resp.hovered() { lk.card_hover } else { lk.card }, Stroke::new(1.0_f32, lk.line));
                paint_ico(ui.painter(), Rect::from_center_size(pos2(r.left() + 30.0, r.center().y), vec2(24.0, 24.0)), Ico::Update, Page::Update.tint(), &pal);
                ui.painter().text(pos2(r.left() + 56.0, r.center().y - 9.0), Align2::LEFT_CENTER, "Windows Update", FontId::proportional(14.0), pal.text);
                ui.painter().text(pos2(r.left() + 56.0, r.center().y + 10.0), Align2::LEFT_CENTER, state, FontId::proportional(12.0), pal.dim);
                if resp.clicked() {
                    self.go(ctx, Page::Update);
                }
            });
        });
        ui.add_space(8.0);
        let items = [
            (Page::Display, tr("Monitors, brightness, resolution, orientation")),
            (Page::Sound, tr("Volume levels, output, input, sound devices")),
            (Page::Power, tr("Sleep, battery usage, power plan")),
            (Page::Storage, tr("Storage space, drives, cleanup")),
            (Page::About, tr("Device specifications, rename PC, Windows specifications")),
        ];
        for (pg, sub) in items {
            if row(ui, lk, Some(pg.icon()), pg.title(), sub, true).clicked() {
                self.go(ctx, pg);
            }
        }
        if ext_row(ui, lk, Ico::Chip, tr("Device Manager"), tr("Hardware and drivers")) {
            spawn_self(&["--devmgr"]);
        }
        if ext_row(ui, lk, Ico::Sliders, tr("Task Manager"), tr("Processes, performance, startup apps, services")) {
            spawn_self(&["--taskmgr"]);
        }
    }

    /// A little monitor showing the wallpaper.
    fn paint_screen(&self, p: &egui::Painter, r: Rect) {
        let pal = self.pal;
        let screen = r.shrink(2.0);
        p.rect_filled(r, 6.0, Color32::from_gray(20));
        if let Some(t) = &self.wall_tex {
            let inner = screen.shrink(3.0);
            let [tw, th] = t.size();
            // Fill: crop the picture to the screen's shape.
            let (sa, ta) = (inner.width() / inner.height(), tw as f32 / th.max(1) as f32);
            let uv = if ta > sa {
                let w = sa / ta;
                Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
            } else {
                let h = ta / sa;
                Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
            };
            p.image(t.id(), inner, uv, Color32::WHITE);
        } else {
            p.rect_filled(screen.shrink(3.0), 3.0, pal.accent.gamma_multiply(0.6));
        }
    }

    // ---------------------------------------------------------- Display

    fn display_page(&mut self, ui: &mut Ui, ctx: &egui::Context, lk: Look) {
        use crate::display::{apply, monitors, orient_label, Mode};
        let pal = lk.pal;
        // Brightness
        let bright = *self.qp().bright.value.lock().unwrap();
        if let Some(mut b) = bright {
            let mut changed = false;
            row_with(ui, lk, Some(Ico::Contrast), tr("Brightness"), tr("Adjust the brightness of your screen"), false, 300.0, |ui| {
                ui.label(format!("{b}"));
                ui.spacing_mut().slider_width = 240.0;
                changed = ui.add(egui::Slider::new(&mut b, 0..=100).show_value(false)).changed();
            });
            if changed {
                self.qp().bright.set(b);
            }
        }

        let Some(st) = self.display.as_mut() else { return };
        if st.monitors.is_empty() {
            card(ui, lk, |ui| {
                ui.label(tr("No display found."));
            });
            return;
        }
        // Revert if the new mode wasn't confirmed in time.
        if let Some((dev, old, since)) = &st.pending {
            if since.elapsed() >= Duration::from_secs(15) {
                apply(dev, *old);
                st.pending = None;
                st.monitors = monitors();
                self.toasts.lock().unwrap().push((tr("The previous settings were restored.").to_string(), false));
            }
        }
        if st.monitors.len() > 1 {
            section(ui, tr("Displays"));
            let mut sel = st.sel;
            let items: Vec<(usize, String)> = st
                .monitors
                .iter()
                .enumerate()
                .map(|(i, m)| (i, format!("{}. {}{}", i + 1, m.name, if m.primary { tr(" (main)") } else { "" })))
                .collect();
            row_with(ui, lk, Some(Ico::Monitor), tr("Display"), tr("Choose the display to change"), false, 380.0, |ui| {
                combo(ui, "mon", 360.0, &mut sel, &items);
            });
            st.sel = sel;
        }

        let mon = st.monitors[st.sel].clone();
        let cur = mon.current;
        let (lw, lh) = if cur.orient % 2 == 1 { (cur.h, cur.w) } else { (cur.w, cur.h) };
        let mut want = Mode { w: lw, h: lh, ..cur };

        section(ui, tr("Scale & layout"));
        let scale = (ctx.pixels_per_point() * 100.0).round() as u32;
        row_with(ui, lk, Some(Ico::Sliders), tr("Scale"), tr("The size of text, apps and other items"), false, 200.0, |ui| {
            ui.label(RichText::new(format!("{scale}%")).strong());
        });
        let mut sizes: Vec<(u32, u32)> = mon.modes.iter().map(|m| (m.w, m.h)).collect();
        sizes.dedup();
        let best = sizes.first().copied();
        let size_items: Vec<((u32, u32), String)> = sizes
            .iter()
            .map(|&(w, h)| ((w, h), if Some((w, h)) == best { format!("{w} × {h} ({})", tr("Recommended")) } else { format!("{w} × {h}") }))
            .collect();
        let mut size = (want.w, want.h);
        let busy = st.pending.is_some();
        row_with(ui, lk, Some(Ico::Monitor), tr("Display resolution"), tr("Adjust the resolution to fit your connected display"), false, 260.0, |ui| {
            ui.add_enabled_ui(!busy, |ui| combo(ui, "res", 240.0, &mut size, &size_items));
        });
        if size != (want.w, want.h) {
            want.w = size.0;
            want.h = size.1;
            let rates: Vec<u32> = mon.modes.iter().filter(|m| m.w == size.0 && m.h == size.1).map(|m| m.hz).collect();
            if !rates.contains(&want.hz) {
                want.hz = rates.iter().copied().max().unwrap_or(60);
            }
        }
        let mut orient = want.orient;
        let orients: Vec<(u32, String)> = (0..4).map(|o| (o, orient_label(o).to_string())).collect();
        row_with(ui, lk, Some(Ico::Restart), tr("Display orientation"), "", false, 260.0, |ui| {
            ui.add_enabled_ui(!busy, |ui| combo(ui, "orient", 240.0, &mut orient, &orients));
        });
        want.orient = orient;
        let mut rates: Vec<u32> = mon.modes.iter().filter(|m| m.w == want.w && m.h == want.h).map(|m| m.hz).collect();
        rates.sort_unstable_by(|a, b| b.cmp(a));
        rates.dedup();
        let rate_items: Vec<(u32, String)> = rates.iter().map(|&r| (r, format!("{r} Hz"))).collect();
        let mut hz = want.hz;
        row_with(ui, lk, Some(Ico::Monitor), tr("Refresh rate"), tr("A higher rate gives smoother motion"), false, 260.0, |ui| {
            ui.add_enabled_ui(!busy, |ui| combo(ui, "hz", 240.0, &mut hz, &rate_items));
        });
        want.hz = hz;

        if !busy && (want.w, want.h, want.hz, want.orient) != (lw, lh, cur.hz, cur.orient) {
            let r = apply(&mon.device, want);
            if r == 0 {
                st.pending = Some((mon.device.clone(), cur, Instant::now()));
            } else {
                self.toasts.lock().unwrap().push((trf("Windows refused this mode (code {}).", &[&r]), true));
            }
            st.monitors = monitors();
        }
        let mut keep = false;
        let mut revert = false;
        if let Some((_, _, since)) = &st.pending {
            let left = 15u64.saturating_sub(since.elapsed().as_secs());
            ui.add_space(6.0);
            Frame::none()
                .fill(lk.card)
                .stroke(Stroke::new(1.0_f32, pal.accent))
                .rounding(6.0)
                .inner_margin(Margin::symmetric(18.0, 14.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(tr("Keep these display settings?")).size(15.0).strong());
                    ui.label(trf("Reverting to the previous display settings in {} seconds.", &[&left]));
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        keep = accent_button(ui, &pal, tr("Keep changes"), true).clicked();
                        revert = button(ui, tr("Revert")).clicked();
                    });
                });
        }
        if keep {
            st.pending = None;
            st.monitors = monitors();
        }
        if revert {
            if let Some((dev, old, _)) = st.pending.take() {
                apply(&dev, old);
            }
            st.monitors = monitors();
        }

        section(ui, tr("Related settings"));
        if row(ui, lk, Some(Ico::Image), tr("Background"), tr("Wallpaper and how it fits the screen"), true).clicked() {
            self.go(ctx, Page::Personalization);
        }
        if ext_row(ui, lk, Ico::Monitor, tr("Advanced display"), tr("Display adapter, colour depth and driver")) {
            sys::start("rundll32.exe display.dll,ShowAdapterSettings");
        }
    }

    // ---------------------------------------------------------- Sound

    fn sound_page(&mut self, ui: &mut Ui, lk: Look) {
        let pal = lk.pal;
        section(ui, tr("Output"));
        match self.qp().volume_state() {
            Some((level, muted)) => {
                let mut v = (level * 100.0).round() as u32;
                let mut changed = false;
                let mut mute = false;
                row_with(ui, lk, Some(Ico::Speaker), tr("Volume"), tr("Default output device"), false, 360.0, |ui| {
                    ui.label(format!("{v}"));
                    ui.spacing_mut().slider_width = 240.0;
                    changed = ui.add(egui::Slider::new(&mut v, 0..=100).show_value(false)).changed();
                    let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 4.0, pal.hover);
                    }
                    quick::paint_speaker(ui.painter(), r.shrink(6.0), pal.text, if muted { None } else { Some(level) });
                    mute = resp.on_hover_text(if muted { tr("Unmute") } else { tr("Mute") }).clicked();
                });
                if changed {
                    self.qp().set_volume(v as f32 / 100.0);
                }
                if mute {
                    self.qp().toggle_mute();
                }
            }
            None => {
                row(ui, lk, Some(Ico::Speaker), tr("No output device found"), "", false);
            }
        }
        if ext_row(ui, lk, Ico::Speaker, tr("Output devices"), tr("Choose where to play sound")) {
            sys::start("rundll32.exe shell32.dll,Control_RunDLL mmsys.cpl,,0");
        }
        section(ui, tr("Input"));
        if ext_row(ui, lk, Ico::Mic, tr("Microphone"), tr("Input devices and levels")) {
            sys::start("rundll32.exe shell32.dll,Control_RunDLL mmsys.cpl,,1");
        }
        section(ui, tr("Advanced"));
        if ext_row(ui, lk, Ico::Sliders, tr("Volume mixer"), tr("App volume")) {
            sys::start("sndvol.exe");
        }
        if ext_row(ui, lk, Ico::Speaker, tr("More sound settings"), tr("Sounds, communications, device properties")) {
            sys::start("rundll32.exe shell32.dll,Control_RunDLL mmsys.cpl,,2");
        }
    }

    // ---------------------------------------------------------- Power

    fn power_page(&mut self, ui: &mut Ui, lk: Look) {
        let pal = lk.pal;
        let battery = self.cached.battery;
        if let Some((pct, charging)) = battery {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(60.0, 40.0), Sense::hover());
                paint_ico(ui.painter(), r, Ico::Battery, pal.text, &pal);
                ui.vertical(|ui| {
                    ui.label(RichText::new(format!("{pct}%")).size(26.0).strong());
                    ui.label(RichText::new(if charging { tr("Charging") } else { tr("On battery") }).color(pal.dim));
                });
            });
        }
        section(ui, tr("Power"));
        if let Some(plans) = self.plans.clone() {
            let mut active = plans.iter().position(|p| p.active).unwrap_or(0);
            let items: Vec<(usize, String)> = plans.iter().enumerate().map(|(i, p)| (i, p.name.clone())).collect();
            let mut changed = false;
            row_with(ui, lk, Some(Ico::Power), tr("Power plan"), tr("Balance performance and energy use"), false, 300.0, |ui| {
                changed = combo(ui, "plan", 280.0, &mut active, &items);
            });
            if changed {
                if let Some(p) = plans.get(active) {
                    if !sys::set_plan(&p.guid) {
                        self.say(tr("The power plan could not be changed."), true);
                    }
                }
                self.plans = Some(sys::plans());
                self.timeouts = sys::timeouts();
            }
        }
        section(ui, tr("Screen and sleep"));
        let items: Vec<(u32, String)> = MINUTES.iter().map(|&m| (m, minutes_label(m))).collect();
        let mut rows: Vec<(usize, &str)> = Vec::new();
        if battery.is_some() {
            rows.push((1, tr("On battery power, turn off my screen after")));
            rows.push((0, tr("When plugged in, turn off my screen after")));
            rows.push((3, tr("On battery power, put my device to sleep after")));
            rows.push((2, tr("When plugged in, put my device to sleep after")));
        } else {
            rows.push((0, tr("Turn off my screen after")));
            rows.push((2, tr("Put my device to sleep after")));
        }
        for (which, label) in rows {
            let mut m = self.timeouts[which] / 60;
            if !MINUTES.contains(&m) {
                // Something set outside Delfin: show the nearest choice.
                m = MINUTES.iter().copied().filter(|&x| x != 0).min_by_key(|&x| (x as i64 - m as i64).abs()).unwrap_or(0);
            }
            let ico = if which < 2 { Ico::Monitor } else { Ico::Power };
            let mut changed = false;
            row_with(ui, lk, Some(ico), label, "", false, 200.0, |ui| {
                changed = combo(ui, &format!("to{which}"), 180.0, &mut m, &items);
            });
            if changed {
                if !sys::set_timeout(which, m * 60) {
                    self.say(tr("The setting could not be changed."), true);
                }
                self.timeouts = sys::timeouts();
            }
        }
        section(ui, tr("Related settings"));
        if ext_row(ui, lk, Ico::Power, tr("Power button and lid"), tr("What the power and sleep buttons do")) {
            sys::start("control.exe /name Microsoft.PowerOptions /page pageGlobalSettings");
        }
        if ext_row(ui, lk, Ico::Sliders, tr("Additional power settings"), tr("Edit power plans")) {
            sys::start("control.exe powercfg.cpl");
        }
    }

    // ---------------------------------------------------------- Storage

    fn storage_page(&mut self, ui: &mut Ui, ctx: &egui::Context, lk: Look) {
        let pal = lk.pal;
        let drives = self.drives.clone().unwrap_or_default();
        for (title, fixed) in [(tr("Drives"), true), (tr("Other drives"), false)] {
            let list: Vec<&sys::Drive> = drives.iter().filter(|d| (d.kind == 3) == fixed).collect();
            if list.is_empty() {
                continue;
            }
            section(ui, title);
            for d in list {
                let label = if d.label.is_empty() {
                    match d.kind {
                        3 => tr("Local Disk"),
                        2 => tr("Removable drive"),
                        4 => tr("Network drive"),
                        _ => tr("Drive"),
                    }
                } else {
                    d.label.as_str()
                };
                let used = d.total - d.free;
                let title = format!("{label} ({})", d.root.trim_end_matches('\\'));
                let sub = trf("{} used of {}  ·  {} free  ·  {}", &[&fmt_bytes(used as f64), &fmt_bytes(d.total as f64), &fmt_bytes(d.free as f64), &d.fs]);
                let frac = used as f32 / d.total as f32;
                let resp = row_with(ui, lk, Some(Ico::Drive), &title, &sub, true, 240.0, |ui| {
                    ui.label(format!("{:.0}%", frac * 100.0));
                    bar(ui, &pal, frac, 180.0);
                });
                if resp.clicked() {
                    spawn_self(&[d.root.as_str()]);
                }
            }
        }

        section(ui, tr("Cleanup"));
        let temp = self.temp_v;
        let armed_temp = self.is_armed("temp");
        let mut clean = false;
        let sub = match temp {
            Some(t) => trf("{} in your temporary folder", &[&fmt_bytes(t as f64)]),
            None => tr("Measuring…").to_string(),
        };
        row_with(ui, lk, Some(Ico::Folder), tr("Temporary files"), &sub, false, 200.0, |ui| {
            let label = if armed_temp { tr("Click again to delete") } else { tr("Clean up") };
            clean = ui.add_enabled(temp.is_some_and(|t| t > 0), egui::Button::new(label).min_size(vec2(96.0, 30.0))).clicked();
        });
        if clean && self.confirm("temp") {
            self.temp_v = None;
            self.job(ctx, Page::Storage, || (trf("Freed {}.", &[&fmt_bytes(sys::clean_temp() as f64)]), false));
        }
        let (size, items) = self.bin;
        let armed_bin = self.is_armed("bin");
        let mut empty = false;
        row_with(ui, lk, Some(Ico::Trash), tr("Recycle Bin"), &trf("{} · {} items", &[&fmt_bytes(size as f64), &items]), false, 200.0, |ui| {
            let label = if armed_bin { tr("Click again to empty") } else { tr("Empty") };
            empty = ui.add_enabled(items > 0, egui::Button::new(label).min_size(vec2(96.0, 30.0))).clicked();
        });
        if empty && self.confirm("bin") {
            if sys::empty_recycle_bin() {
                self.say(tr("The Recycle Bin is empty."), false);
            }
            self.bin = sys::recycle_bin();
        }
        if ext_row(ui, lk, Ico::Drive, tr("Disk Cleanup"), tr("System files, update leftovers, thumbnails")) {
            sys::start("cleanmgr.exe");
        }
        if ext_row(ui, lk, Ico::Drive, tr("Disks & volumes"), tr("Partitions and drive letters")) {
            sys::start("diskmgmt.msc");
        }
        if ext_row(ui, lk, Ico::Sliders, tr("Drive optimization"), tr("Defragment and optimize drives")) {
            sys::start("dfrgui.exe");
        }
    }

    // ---------------------------------------------------------- About

    fn about_page(&mut self, ui: &mut Ui, ctx: &egui::Context, lk: Look) {
        let pal = lk.pal;
        let Some(a) = self.about_v.clone() else {
            ui.label(RichText::new(tr("Loading…")).color(pal.dim));
            return;
        };
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(110.0, 64.0), Sense::hover());
            self.paint_screen(ui.painter(), r);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.add_space(6.0);
                ui.label(RichText::new(&a.computer).size(20.0).strong());
                let model = format!("{} {}", a.maker, a.model);
                ui.label(RichText::new(model.trim()).color(pal.dim));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.rename.is_none() && button(ui, tr("Rename this PC")).clicked() {
                    self.rename = Some(a.computer.clone());
                }
            });
        });
        if let Some(mut name) = self.rename.take() {
            let mut keep = true;
            ui.add_space(6.0);
            card(ui, lk, |ui| {
                ui.label(RichText::new(tr("Rename your PC")).size(15.0).strong());
                ui.label(RichText::new(tr("Letters, numbers and hyphens, up to 15 characters. The new name is used after a restart.")).color(pal.dim));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut name).desired_width(260.0).char_limit(15));
                    let ok = sys::valid_computer_name(&name) && name != a.computer;
                    if accent_button(ui, &pal, tr("Save"), ok).clicked() {
                        let n = name.clone();
                        self.job(ctx, Page::About, move || {
                            if sys::rename_computer(&n) {
                                (trf("The PC will be called {} after a restart.", &[&n]), false)
                            } else {
                                (tr("The PC could not be renamed.").to_string(), true)
                            }
                        });
                        keep = false;
                    }
                    if button(ui, tr("Cancel")).clicked() {
                        keep = false;
                    }
                });
            });
            if keep {
                self.rename = Some(name);
            }
        }

        section(ui, tr("Device specifications"));
        let dev = [
            (tr("Device name"), a.computer.clone()),
            (tr("Processor"), a.cpu.clone()),
            (tr("Installed RAM"), fmt_bytes(a.ram as f64)),
            (tr("System type"), trf("{} operating system", &[&a.arch])),
            (tr("Manufacturer"), a.maker.clone()),
            (tr("Model"), a.model.clone()),
        ];
        let dev_text: String = dev.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("{k}\t{v}\n")).collect();
        card(ui, lk, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    for (k, v) in &dev {
                        if !v.is_empty() {
                            kv(ui, &pal, k, v);
                        }
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| copy_button(ui, &dev_text));
            });
        });
        section(ui, tr("Windows specifications"));
        let win = [
            (tr("Edition"), a.edition.clone()),
            (tr("Version"), a.version.clone()),
            (tr("Installed on"), a.installed.clone()),
            (tr("OS build"), a.build.clone()),
        ];
        let win_text: String = win.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("{k}\t{v}\n")).collect();
        card(ui, lk, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    for (k, v) in &win {
                        if !v.is_empty() {
                            kv(ui, &pal, k, v);
                        }
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| copy_button(ui, &win_text));
            });
        });

        section(ui, tr("Related"));
        if ext_row(ui, lk, Ico::Chip, tr("Device Manager"), tr("Hardware and drivers")) {
            spawn_self(&["--devmgr"]);
        }
        if ext_row(ui, lk, Ico::Sliders, tr("Advanced system settings"), tr("Performance, user profiles, environment variables")) {
            sys::start("SystemPropertiesAdvanced.exe");
        }
        if ext_row(ui, lk, Ico::Shield, tr("System protection"), tr("Restore points")) {
            sys::start("SystemPropertiesProtection.exe");
        }
        if ext_row(ui, lk, Ico::Monitor, tr("Remote Desktop"), tr("Connect to this PC from another device")) {
            sys::start("SystemPropertiesRemote.exe");
        }
        if ext_row(ui, lk, Ico::Globe, tr("Domain or workgroup"), "") {
            sys::start("SystemPropertiesComputerName.exe");
        }
    }

    // ---------------------------------------------------------- Bluetooth & devices

    fn devices_page(&mut self, ui: &mut Ui, ctx: &egui::Context, lk: Look) {
        let pal = lk.pal;
        match self.bt_v {
            Some(Some(on)) => {
                let mut v = on;
                let mut changed = false;
                row_with(ui, lk, Some(Ico::Bluetooth), "Bluetooth", if on { tr("Discoverable as this PC") } else { tr("Turned off") }, false, 160.0, |ui| {
                    changed = switch(ui, &pal, &mut v);
                });
                if changed {
                    self.bt_v = Some(Some(v));
                    self.job(ctx, Page::Devices, move || {
                        if sys::set_bluetooth(v) {
                            (String::new(), false)
                        } else {
                            (tr("Bluetooth could not be switched.").to_string(), true)
                        }
                    });
                }
            }
            Some(None) => {
                row(ui, lk, Some(Ico::Bluetooth), "Bluetooth", tr("This PC has no Bluetooth"), false);
            }
            None => {
                row(ui, lk, Some(Ico::Bluetooth), "Bluetooth", tr("Loading…"), false);
            }
        }
        if ext_row(ui, lk, Ico::Plus, tr("Add device"), tr("Bluetooth, wireless displays, other devices")) {
            sys::start("DevicePairingWizard.exe");
        }

        // Devices people care about, by type.
        let Some(devs) = self.devices_v.clone() else {
            ui.label(RichText::new(tr("Loading…")).color(pal.dim));
            return;
        };
        let types: [(&str, Ico); 9] = [
            ("Bluetooth", Ico::Bluetooth),
            ("Mouse", Ico::Mouse),
            ("Keyboard", Ico::Keyboard),
            ("AudioEndpoint", Ico::Speaker),
            ("Monitor", Ico::Monitor),
            ("Camera", Ico::Camera),
            ("Image", Ico::Camera),
            ("PrintQueue", Ico::Printer),
            ("WPD", Ico::Drive),
        ];
        for (class, ico) in types {
            let list: Vec<&crate::devmgr::sys::Device> = devs
                .iter()
                .filter(|d| d.class.eq_ignore_ascii_case(class))
                // Paired Bluetooth devices, not the radio's own helpers.
                .filter(|d| class != "Bluetooth" || d.instance_id.to_uppercase().contains("\\DEV_"))
                .collect();
            if list.is_empty() {
                continue;
            }
            section(ui, &list[0].class_display);
            for d in list {
                let status = if d.disabled() {
                    tr("Disabled devices").to_string()
                } else if d.has_problem() {
                    crate::devmgr::sys::problem_text(d.problem).to_string()
                } else if class == "Bluetooth" || class == "AudioEndpoint" {
                    tr("Connected").to_string()
                } else {
                    d.manufacturer.clone()
                };
                let tex = self.class_icon(ctx, d.class_guid);
                let resp = row_with(ui, lk, Some(if tex.is_some() { Ico::Blank } else { ico }), &d.name, &status, false, 0.0, |_| {});
                if let Some(t) = tex {
                    let r = Rect::from_center_size(pos2(resp.rect.left() + 28.0, resp.rect.center().y), vec2(24.0, 24.0));
                    ui.painter().image(t, r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
            }
        }

        section(ui, tr("Related settings"));
        if row(ui, lk, Some(Ico::Mouse), tr("Mouse"), tr("Buttons, pointer speed, scrolling"), true).clicked() {
            self.go(ctx, Page::Mouse);
        }
        if ext_row(ui, lk, Ico::Keyboard, tr("Keyboard"), tr("Repeat delay and rate")) {
            sys::start("control.exe keyboard");
        }
        if ext_row(ui, lk, Ico::Printer, tr("Add a printer"), "") {
            sys::start("rundll32.exe printui.dll,PrintUIEntry /il");
        }
        if ext_row(ui, lk, Ico::Chip, tr("Device Manager"), tr("Hardware and drivers")) {
            spawn_self(&["--devmgr"]);
        }
    }

    fn class_icon(&mut self, ctx: &egui::Context, guid: u128) -> Option<egui::TextureId> {
        self.class_icons
            .entry(guid)
            .or_insert_with(|| {
                crate::devmgr::sys::class_icon(guid).map(|(w, h, px)| {
                    ctx.load_texture(format!("cls:{guid:x}"), egui::ColorImage::from_rgba_unmultiplied([w, h], &px), egui::TextureOptions::LINEAR)
                })
            })
            .as_ref()
            .map(|t| t.id())
    }

    // ---------------------------------------------------------- Mouse

    fn mouse_page(&mut self, ui: &mut Ui, lk: Look) {
        let pal = lk.pal;
        let Some(mut m) = self.mouse else { return };
        let before = m;
        let mut swap = m.swap as usize;
        let items = vec![(0usize, tr("Left").to_string()), (1, tr("Right").to_string())];
        row_with(ui, lk, Some(Ico::Mouse), tr("Primary mouse button"), "", false, 200.0, |ui| {
            combo(ui, "btn", 180.0, &mut swap, &items);
        });
        m.swap = swap == 1;
        row_with(ui, lk, Some(Ico::Mouse), tr("Mouse pointer speed"), "", false, 300.0, |ui| {
            ui.label(format!("{}", m.speed));
            ui.spacing_mut().slider_width = 240.0;
            ui.add(egui::Slider::new(&mut m.speed, 1..=20).show_value(false));
        });
        row_with(ui, lk, Some(Ico::Mouse), tr("Enhance pointer precision"), tr("The pointer moves farther when you move the mouse faster"), false, 160.0, |ui| {
            switch(ui, &pal, &mut m.precision);
        });
        section(ui, tr("Scrolling"));
        row_with(ui, lk, Some(Ico::Sliders), tr("Lines to scroll at a time"), "", false, 300.0, |ui| {
            ui.label(format!("{}", m.wheel_lines));
            ui.spacing_mut().slider_width = 240.0;
            ui.add(egui::Slider::new(&mut m.wheel_lines, 1..=100).show_value(false));
        });
        if m != before {
            sys::set_mouse(m);
            self.mouse = Some(m);
        }
        section(ui, tr("Related settings"));
        if ext_row(ui, lk, Ico::Mouse, tr("Additional mouse settings"), tr("Pointers, double-click speed, pointer trails")) {
            sys::start("control.exe main.cpl");
        }
    }

    // ---------------------------------------------------------- Network

    fn network_page(&mut self, ui: &mut Ui, ctx: &egui::Context, lk: Look) {
        let pal = lk.pal;
        let adapters = self.adapters_v.clone().unwrap_or_default();
        let up = adapters.iter().find(|a| a.up && !a.gateway.is_empty()).or_else(|| adapters.iter().find(|a| a.up));
        // Status header
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(64.0, 56.0), Sense::hover());
            match up {
                Some(a) if a.wifi => quick::paint_wifi(ui.painter(), r.shrink(8.0), pal.text, pal.border, self.qp().wifi_signal().or(Some(100))),
                Some(_) => paint_ico(ui.painter(), r.shrink(10.0), Ico::Ethernet, pal.text, &pal),
                None => paint_ico(ui.painter(), r.shrink(10.0), Ico::Globe, pal.dim, &pal),
            }
            ui.vertical(|ui| {
                ui.add_space(6.0);
                match up {
                    Some(a) => {
                        let name = if a.wifi { self.qp().connected_ssid().unwrap_or_else(|| a.name.clone()) } else { a.name.clone() };
                        ui.label(RichText::new(name).size(20.0).strong());
                        ui.label(RichText::new(if a.gateway.is_empty() { tr("Connected, no internet") } else { tr("Connected") }).color(pal.dim));
                    }
                    None => {
                        ui.label(RichText::new(tr("Not connected")).size(20.0).strong());
                        ui.label(RichText::new(if self.adapters_v.is_none() { tr("Loading…") } else { tr("No network connection") }).color(pal.dim));
                    }
                }
            });
        });

        // Wi-Fi (the same list as the panel's quick settings)
        let has_wifi = self.qp().wifi.state.lock().unwrap().present;
        if has_wifi {
            section(ui, "Wi-Fi");
            let mut action = None;
            card(ui, lk, |ui| {
                ui.set_max_width(ui.available_width().min(640.0));
                self.qp().network_ui(ui, &pal, &mut action);
            });
            if let Some(quick::QuickAction::Open(uri)) = action {
                if let Some(p) = page_for(uri) {
                    self.go(ctx, p);
                } else {
                    crate::shell::win::shell_open(uri);
                }
            }
        }

        section(ui, tr("Network adapters"));
        for a in &adapters {
            let open = self.open_adapters.contains(&a.name);
            let state = if a.up {
                let speed = if a.speed > 0 && a.speed != u64::MAX { format!("  ·  {}", crate::taskmgr::sys::fmt_bits(a.speed as f64)) } else { String::new() };
                format!("{}{}", a.ipv4.first().cloned().unwrap_or_else(|| tr("Connected").to_string()), speed)
            } else {
                tr("Disconnected").to_string()
            };
            let ico = if a.wifi { Ico::Wifi } else { Ico::Ethernet };
            let resp = row(ui, lk, Some(ico), &a.name, &format!("{}  ·  {}", a.description, state), true);
            if resp.clicked() {
                if open {
                    self.open_adapters.remove(&a.name);
                } else {
                    self.open_adapters.insert(a.name.clone());
                }
            }
            if open {
                let mut lines = vec![(tr("Description"), a.description.clone()), (tr("Physical address (MAC)"), a.mac.clone())];
                if !a.ipv4.is_empty() {
                    lines.push((tr("IPv4 address"), a.ipv4.join(", ")));
                }
                if !a.ipv6.is_empty() {
                    lines.push((tr("IPv6 address"), a.ipv6.join("\n")));
                }
                if !a.gateway.is_empty() {
                    lines.push((tr("Default gateway"), a.gateway.join(", ")));
                }
                if !a.dns.is_empty() {
                    lines.push((tr("DNS servers"), a.dns.join(", ")));
                }
                if a.speed > 0 && a.speed != u64::MAX {
                    lines.push((tr("Link speed"), crate::taskmgr::sys::fmt_bits(a.speed as f64)));
                }
                let text: String = lines.iter().map(|(k, v)| format!("{k}\t{v}\n")).collect();
                card(ui, lk, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            for (k, v) in &lines {
                                kv(ui, &pal, k, v);
                            }
                        });
                        ui.with_layout(Layout::right_to_left(Align::Min), |ui| copy_button(ui, &text));
                    });
                });
            }
        }

        section(ui, tr("Related settings"));
        if ext_row(ui, lk, Ico::Ethernet, tr("Network connections"), tr("Adapters, IP settings, sharing")) {
            sys::start("control.exe ncpa.cpl");
        }
        if ext_row(ui, lk, Ico::Globe, tr("Proxy and internet options"), "") {
            sys::start("control.exe inetcpl.cpl,,4");
        }
        if ext_row(ui, lk, Ico::Shield, tr("Windows Firewall"), "") {
            sys::start("control.exe firewall.cpl");
        }
    }

    // ---------------------------------------------------------- Personalization

    fn personalization_page(&mut self, ui: &mut Ui, ctx: &egui::Context, lk: Look) {
        let pal = lk.pal;
        let (r, _) = ui.allocate_exact_size(vec2(300.0, 176.0), Sense::hover());
        self.paint_screen(ui.painter(), r);
        ui.add_space(8.0);

        section(ui, tr("Background"));
        let name = self
            .wallpaper
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| tr("(none)").to_string());
        let mut browse = false;
        row_with(ui, lk, Some(Ico::Image), tr("Personalize your background"), &name, false, 160.0, |ui| {
            browse = button(ui, tr("Browse photos")).clicked();
        });
        if browse {
            if let Some(p) = sys::pick_image(self.hwnd, tr("Choose a wallpaper"), tr("Pictures")) {
                if sys::set_wallpaper(&p) {
                    self.wallpaper = Some(p);
                    self.load_wallpaper(ctx);
                }
            }
        }
        let mut fit = self.cached.fit;
        let fits: Vec<(usize, String)> = sys::FITS.iter().enumerate().map(|(i, f)| (i, tr(f.0).to_string())).collect();
        let mut changed = false;
        row_with(ui, lk, Some(Ico::Monitor), tr("Choose a fit for your desktop image"), "", false, 200.0, |ui| {
            changed = combo(ui, "fit", 180.0, &mut fit, &fits);
        });
        if changed {
            sys::set_wallpaper_fit(fit);
            self.cached.fit = fit;
        }
        if let Some(gal) = &self.gallery_tex {
            let mut pick = None;
            card(ui, lk, |ui| {
                ui.label(tr("Windows pictures"));
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    for (p, t) in gal {
                        let (r, resp) = ui.allocate_exact_size(vec2(128.0, 72.0), Sense::click());
                        let [tw, th] = t.size();
                        let ta = tw as f32 / th.max(1) as f32;
                        let sa = r.width() / r.height();
                        let uv = if ta > sa {
                            let w = sa / ta;
                            Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
                        } else {
                            let h = ta / sa;
                            Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
                        };
                        ui.painter().image(t.id(), r, uv, Color32::WHITE);
                        let current = self.wallpaper.as_ref() == Some(p);
                        if current || resp.hovered() {
                            ui.painter().rect_stroke(r, 4.0, Stroke::new(2.0_f32, pal.accent));
                        }
                        if resp.clicked() {
                            pick = Some(p.clone());
                        }
                    }
                });
            });
            if let Some(p) = pick {
                if sys::set_wallpaper(&p) {
                    self.wallpaper = Some(p);
                    self.load_wallpaper(ctx);
                }
            }
        }

        section(ui, tr("Colors"));
        let mut mode = self.dark as usize;
        let modes = vec![(0usize, tr("Light").to_string()), (1, tr("Dark").to_string())];
        let mut changed = false;
        row_with(ui, lk, Some(Ico::Contrast), tr("Choose your mode"), tr("Delfin and Windows apps follow this"), false, 200.0, |ui| {
            changed = combo(ui, "mode", 180.0, &mut mode, &modes);
        });
        if changed {
            self.set_dark(ctx, mode == 1);
            self.say(tr("Open Delfin windows switch after they are reopened."), false);
        }
        let mut tr_on = self.cached.transparency;
        let mut changed = false;
        row_with(ui, lk, Some(Ico::Brush), tr("Transparency effects"), tr("Windows and surfaces appear translucent"), false, 160.0, |ui| {
            changed = switch(ui, &pal, &mut tr_on);
        });
        if changed {
            sys::set_transparency(tr_on);
            self.cached.transparency = tr_on;
        }

        section(ui, tr("Related settings"));
        if row(ui, lk, Some(Ico::Monitor), tr("Display"), tr("Resolution, orientation, brightness"), true).clicked() {
            self.go(ctx, Page::Display);
        }
        if ext_row(ui, lk, Ico::Mouse, tr("Mouse pointers"), "") {
            sys::start("control.exe main.cpl,,1");
        }
        if ext_row(ui, lk, Ico::Language, tr("Fonts"), "") {
            spawn_self(&["C:\\Windows\\Fonts"]);
        }
    }

    // ---------------------------------------------------------- Apps

    fn apps_page(&mut self, ui: &mut Ui, ctx: &egui::Context, lk: Look) {
        if row(ui, lk, Some(Ico::List), tr("Installed apps"), tr("Uninstall and modify your apps"), true).clicked() {
            self.go(ctx, Page::Installed);
        }
        if row(ui, lk, Some(Ico::Rocket), tr("Startup"), tr("Apps that start automatically when you sign in"), true).clicked() {
            self.go(ctx, Page::Startup);
        }
        if ext_row(ui, lk, Ico::Plus, tr("Optional features"), tr("Turn Windows features on or off")) {
            sys::start("optionalfeatures.exe");
        }
        if ext_row(ui, lk, Ico::Apps, tr("Programs and Features"), tr("The classic list of installed programs")) {
            sys::start("control.exe appwiz.cpl");
        }
    }

    fn installed_page(&mut self, ui: &mut Ui, ctx: &egui::Context, lk: Look) {
        let pal = lk.pal;
        let Some(apps) = self.apps_v.clone() else {
            ui.label(RichText::new(tr("Loading…")).color(pal.dim));
            return;
        };
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.app_query).hint_text(tr("Search apps")).desired_width(300.0).margin(vec2(10.0, 7.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let sorts = vec![(0usize, tr("Name (A to Z)").to_string()), (1, tr("Size (large to small)").to_string()), (2, tr("Install date").to_string())];
                combo(ui, "sort", 200.0, &mut self.app_sort, &sorts);
                ui.label(tr("Sort by:"));
            });
        });
        let q = self.app_query.trim().to_lowercase();
        let mut list: Vec<&sys::InstalledApp> =
            apps.iter().filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q) || a.publisher.to_lowercase().contains(&q)).collect();
        match self.app_sort {
            1 => list.sort_by(|a, b| b.size.cmp(&a.size)),
            2 => list.sort_by(|a, b| b.date.cmp(&a.date)),
            _ => {}
        }
        ui.add_space(6.0);
        ui.label(RichText::new(trf("{} apps found", &[&list.len()])).color(pal.dim));
        ui.add_space(4.0);
        let mut act: Option<(bool, sys::InstalledApp)> = None;
        for a in list {
            let sub = [a.version.as_str(), a.publisher.as_str(), a.date.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("  |  ");
            let key = format!("un:{}", a.name);
            let armed = self.is_armed(&key);
            let tex = self.app_icon(ctx, a);
            let resp = row_with(ui, lk, Some(if tex.is_some() { Ico::Blank } else { Ico::Apps }), &a.name, &sub, false, 230.0, |ui| {
                ui.menu_button(" ••• ", |ui| {
                    if let Some(_m) = &a.modify {
                        if ui.button(tr("Modify")).clicked() {
                            act = Some((false, a.clone()));
                            ui.close_menu();
                        }
                    }
                    let label = if armed { tr("Click again to uninstall") } else { tr("Uninstall") };
                    if ui.button(label).clicked() {
                        act = Some((true, a.clone()));
                        ui.close_menu();
                    }
                });
                if a.size > 0 {
                    ui.label(RichText::new(fmt_bytes(a.size as f64)).color(pal.dim));
                }
            });
            if let Some(t) = tex {
                let r = Rect::from_center_size(pos2(resp.rect.left() + 28.0, resp.rect.center().y), vec2(24.0, 24.0));
                ui.painter().image(t, r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
        }
        match act {
            Some((true, a)) => {
                if self.confirm(&format!("un:{}", a.name)) {
                    sys::start(&a.uninstall);
                    self.say(trf("Uninstalling {}…", &[&a.name]), false);
                    self.apps_v = None;
                    let reload = self.reload.clone();
                    let ctx2 = ctx.clone();
                    // The list is read again once the uninstaller had a moment.
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs(4));
                        reload.lock().unwrap().insert(Page::Installed);
                        ctx2.request_repaint();
                    });
                    self.enter(ctx, Page::Installed);
                }
            }
            Some((false, a)) => {
                if let Some(m) = &a.modify {
                    sys::start(m);
                }
            }
            None => {}
        }
    }

    fn app_icon(&mut self, ctx: &egui::Context, a: &sys::InstalledApp) -> Option<egui::TextureId> {
        if let Some(t) = self.app_icons.get(&a.name) {
            return Some(t.id());
        }
        let (w, h, px) = self.app_px.remove(&a.name)?;
        let t = ctx.load_texture(format!("app:{}", a.name), egui::ColorImage::from_rgba_unmultiplied([w, h], &px), egui::TextureOptions::LINEAR);
        let id = t.id();
        self.app_icons.insert(a.name.clone(), t);
        Some(id)
    }

    fn startup_page(&mut self, ui: &mut Ui, lk: Look) {
        let pal = lk.pal;
        ui.label(RichText::new(tr("Apps can start when you sign in. Turned off apps start the next time you open them yourself.")).color(pal.dim));
        ui.add_space(6.0);
        let Some(items) = self.startup.clone() else { return };
        if items.is_empty() {
            card(ui, lk, |ui| {
                ui.label(tr("No apps start automatically."));
            });
        }
        let mut changed: Option<(StartupItem, bool)> = None;
        for it in &items {
            let mut on = it.enabled;
            let sub = if it.machine { format!("{}  ·  {}", it.command, tr("All users")) } else { it.command.clone() };
            row_with(ui, lk, Some(Ico::Rocket), &it.name, &sub, false, 160.0, |ui| {
                if switch(ui, &pal, &mut on) {
                    changed = Some((it.clone(), on));
                }
            });
        }
        if let Some((it, on)) = changed {
            if let Err(e) = crate::taskmgr::sys::set_startup(&it, on) {
                self.say(e, true);
            }
            self.startup = Some(crate::taskmgr::sys::startup_items());
        }
    }

    // ---------------------------------------------------------- Accounts

    fn accounts_page(&mut self, ui: &mut Ui, lk: Look) {
        let pal = lk.pal;
        let a = self.about_v.clone().unwrap_or_default();
        let acc = self.account_v.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(110.0, 110.0), Sense::hover());
            self.paint_avatar(ui.painter(), r.center(), 50.0);
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.add_space(18.0);
                ui.label(RichText::new(&a.full_name).size(22.0).strong());
                if a.full_name != a.user {
                    ui.label(RichText::new(&a.user).color(pal.dim));
                }
                if let Some(e) = &acc.email {
                    ui.label(RichText::new(e).color(pal.dim));
                }
                ui.label(RichText::new(if acc.admin { tr("Administrator") } else { tr("Standard user") }).color(pal.dim));
            });
        });
        section(ui, tr("Account settings"));
        if ext_row(ui, lk, Ico::Person, tr("User accounts"), tr("Add users, change account types and passwords")) {
            sys::start("netplwiz.exe");
        }
        if ext_row(ui, lk, Ico::Shield, tr("Credential Manager"), tr("Saved passwords for websites and networks")) {
            sys::start("control.exe /name Microsoft.CredentialManager");
        }
        if ext_row(ui, lk, Ico::Folder, tr("Your user folder"), &std::env::var("USERPROFILE").unwrap_or_default()) {
            if let Some(h) = dirs::home_dir() {
                spawn_self(&[h.to_string_lossy().as_ref()]);
            }
        }
        if ext_row(ui, lk, Ico::Sliders, tr("Environment variables"), "") {
            sys::start("rundll32.exe sysdm.cpl,EditEnvironmentVariables");
        }
    }

    // ---------------------------------------------------------- Time & language

    fn time_page(&mut self, ui: &mut Ui, ctx: &egui::Context, lk: Look) {
        let pal = lk.pal;
        let now = chrono::Local::now();
        ui.label(RichText::new(now.format("%H:%M:%S").to_string()).size(34.0).strong());
        {
            use chrono::Datelike;
            let d = now.date_naive();
            let text = format!(
                "{}, {} {} {}",
                crate::i18n::weekday_name(d.weekday().num_days_from_monday()),
                d.day(),
                crate::i18n::month_in_date(d.month()),
                d.year()
            );
            ui.label(RichText::new(text).color(pal.dim));
        }

        section(ui, tr("Date & time"));
        let auto = self.cached.auto_time;
        let server = self.cached.time_server.clone();
        let mut sync = false;
        row_with(
            ui,
            lk,
            Some(Ico::Clock),
            tr("Set time automatically"),
            &if auto { trf("On  ·  {}", &[&server]) } else { tr("Off").to_string() },
            false,
            160.0,
            |ui| {
                sync = button(ui, tr("Sync now")).clicked();
            },
        );
        if sync {
            self.job(ctx, Page::Time, || {
                if sys::sync_time() {
                    (tr("The clock is synchronized.").to_string(), false)
                } else {
                    (tr("The clock could not be synchronized.").to_string(), true)
                }
            });
        }
        if let Some(zones) = &self.zones {
            let mut zone = self.zone.clone();
            let current = zones.iter().find(|z| z.0 == zone).map(|z| z.1.clone()).unwrap_or_else(|| zone.clone());
            let mut pick: Option<String> = None;
            row_with(ui, lk, Some(Ico::Globe), tr("Time zone"), "", false, 420.0, |ui| {
                egui::ComboBox::from_id_salt("tz").width(400.0).height(420.0).selected_text(current).show_ui(ui, |ui| {
                    for (id, disp) in zones {
                        if ui.selectable_label(*id == zone, disp).clicked() {
                            pick = Some(id.clone());
                        }
                    }
                });
            });
            if let Some(id) = pick {
                if sys::set_time_zone(&id) {
                    zone = id;
                } else {
                    self.say(tr("The time zone could not be changed."), true);
                }
                self.zone = zone;
            }
        }
        if ext_row(ui, lk, Ico::Clock, tr("Set the date and time manually"), "") {
            sys::start("control.exe timedate.cpl");
        }

        section(ui, tr("Language & region"));
        use crate::i18n::Lang;
        let mut lang = crate::i18n::lang();
        let langs = vec![(Lang::En, "English".to_string()), (Lang::Pl, "Polski".to_string())];
        let mut changed = false;
        row_with(ui, lk, Some(Ico::Language), tr("Delfin display language"), tr("Menus and windows of Delfin"), false, 200.0, |ui| {
            changed = combo(ui, "lang", 180.0, &mut lang, &langs);
        });
        if changed {
            crate::i18n::set_lang(lang);
            self.say(tr("Some open Delfin windows switch language after they are reopened."), false);
        }
        if ext_row(ui, lk, Ico::Globe, tr("Region"), tr("Date, time and number formats")) {
            sys::start("control.exe intl.cpl");
        }
        if ext_row(ui, lk, Ico::Keyboard, tr("Keyboard layouts"), tr("Input languages and keyboards")) {
            sys::start("rundll32.exe shell32.dll,Control_RunDLL input.dll");
        }
    }

    // ---------------------------------------------------------- Windows Update

    fn update_page(&mut self, ui: &mut Ui, ctx: &egui::Context, lk: Look) {
        let pal = lk.pal;
        let (checking, installing, checked, updates, history, error, restart) = {
            let w = self.wu.lock().unwrap_or_else(|p| p.into_inner());
            (w.checking, w.installing, w.checked, w.updates.clone(), w.history.clone(), w.error.clone(), w.restart)
        };
        let (title, sub) = if installing {
            (tr("Installing updates…").to_string(), tr("This can take a while. You can keep using your PC.").to_string())
        } else if restart {
            (tr("Restart required").to_string(), tr("Restart your PC to finish installing updates.").to_string())
        } else if checking {
            (tr("Checking for updates…").to_string(), String::new())
        } else if let Some(e) = &error {
            (tr("Couldn't check for updates").to_string(), e.clone())
        } else if !updates.is_empty() {
            (tr("Updates available").to_string(), checked.map(|t| trf("Last checked: {}", &[&t.format("%H:%M")])).unwrap_or_default())
        } else if checked.is_some() {
            (tr("You're up to date").to_string(), checked.map(|t| trf("Last checked: {}", &[&t.format("%H:%M")])).unwrap_or_default())
        } else {
            (tr("Windows Update").to_string(), String::new())
        };
        let mut check = false;
        let mut install = false;
        let mut reboot = false;
        let armed = self.is_armed("reboot");
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
            paint_ico(ui.painter(), r.shrink(12.0), Ico::Update, Page::Update.tint(), &pal);
            if checking || installing {
                ui.ctx().request_repaint_after(Duration::from_millis(50));
                let t = ui.input(|i| i.time) as f32;
                let pts = arc_pts(r.center(), 26.0, t * 4.0, t * 4.0 + 1.6);
                ui.painter().add(Shape::line(pts, Stroke::new(2.5_f32, pal.accent)));
            }
            ui.vertical(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(&title).size(20.0).strong());
                if !sub.is_empty() {
                    ui.label(RichText::new(&sub).color(pal.dim));
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if restart {
                    reboot = accent_button(ui, &pal, if armed { tr("Click again to restart") } else { tr("Restart now") }, true).clicked();
                } else if !updates.is_empty() {
                    install = accent_button(ui, &pal, tr("Install all"), !installing && !checking).clicked();
                } else {
                    check = accent_button(ui, &pal, tr("Check for updates"), !checking && !installing).clicked();
                }
            });
        });
        if check {
            self.check_updates(ctx);
        }
        if install {
            self.install_updates(ctx);
        }
        if reboot && self.confirm("reboot") {
            sys::start("shutdown.exe /r /t 0");
        }
        if !updates.is_empty() {
            ui.add_space(8.0);
            for u in &updates {
                let size = if u.size > 0 { fmt_bytes(u.size as f64) } else { String::new() };
                let state = if installing { tr("Installing…") } else { tr("Ready to install") };
                row(ui, lk, Some(Ico::Update), &u.title, &format!("{state}  ·  {size}"), false);
            }
        }

        section(ui, tr("Update history"));
        if history.is_empty() {
            card(ui, lk, |ui| {
                ui.label(RichText::new(if checking { tr("Loading…") } else { tr("No updates have been installed yet.") }).color(pal.dim));
            });
        }
        for h in history.iter().take(30) {
            let res = match h.result {
                2 => tr("Successfully installed"),
                3 => tr("Installed with errors"),
                4 => tr("Failed to install"),
                5 => tr("Cancelled"),
                _ => tr("In progress"),
            };
            row(ui, lk, Some(if h.result == 4 { Ico::Info } else { Ico::List }), &h.title, &format!("{res}  ·  {}", h.date), false);
        }
        section(ui, tr("Related"));
        if row(ui, lk, Some(Ico::Delfin), tr("Delfin updates"), tr("Delfin updates itself from GitHub"), true).clicked() {
            self.go(ctx, Page::Delfin);
        }
    }

    // ---------------------------------------------------------- Delfin

    fn delfin_page(&mut self, ui: &mut Ui, ctx: &egui::Context, lk: Look) {
        use crate::update::State;
        let pal = lk.pal;
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
            paint_ico(ui.painter(), r.shrink(8.0), Ico::Delfin, pal.accent, &pal);
            ui.vertical(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new("Delfin").size(20.0).strong());
                ui.label(RichText::new(trf("Version {}", &[&crate::update::CURRENT])).color(pal.dim));
            });
        });
        section(ui, tr("Updates"));
        let state = self.updater.get();
        let (msg, bad) = match &state {
            State::Idle => (tr("Delfin updates itself from GitHub").to_string(), false),
            State::Checking => (tr("Checking for updates…").to_string(), false),
            State::UpToDate => (tr("You have the latest version.").to_string(), false),
            State::Available(r) => (trf("Version {} is available.", &[&r.version]), false),
            State::Downloading => (tr("Downloading…").to_string(), false),
            State::Installed => (tr("Update installed. Restart Delfin to use it.").to_string(), false),
            State::Error(e) => (e.clone(), true),
        };
        let mut check = false;
        let mut install = None;
        let mut restart = false;
        row_with(ui, lk, Some(Ico::Update), tr("Delfin updates"), &msg, false, 260.0, |ui| {
            let busy = matches!(state, State::Checking | State::Downloading);
            match &state {
                State::Available(r) => {
                    if accent_button(ui, &pal, tr("Download and install"), true).clicked() {
                        install = Some(r.clone());
                    }
                }
                State::Installed => restart = accent_button(ui, &pal, tr("Restart Delfin"), true).clicked(),
                _ => check = ui.add_enabled(!busy, egui::Button::new(tr("Check for updates")).min_size(vec2(96.0, 30.0))).clicked(),
            }
        });
        if bad {
            ui.label(RichText::new(&msg).color(pal.danger));
        }
        if let State::Available(r) = &state {
            if !r.notes.trim().is_empty() {
                card(ui, lk, |ui| {
                    ui.label(RichText::new(tr("Release notes")).strong());
                    ui.label(RichText::new(r.notes.trim()).size(12.5));
                });
            }
        }
        if check {
            self.updater.check(ctx);
        }
        if let Some(r) = install {
            self.updater.install(r, ctx);
        }
        if restart {
            restart_shell();
        }

        section(ui, tr("Language"));
        use crate::i18n::Lang;
        let mut lang = crate::i18n::lang();
        let langs = vec![(Lang::En, "English".to_string()), (Lang::Pl, "Polski".to_string())];
        let mut changed = false;
        row_with(ui, lk, Some(Ico::Language), tr("Language"), tr("Menus and windows of Delfin"), false, 200.0, |ui| {
            changed = combo(ui, "dlang", 180.0, &mut lang, &langs);
        });
        if changed {
            crate::i18n::set_lang(lang);
            self.say(tr("Some open Delfin windows switch language after they are reopened."), false);
        }

        section(ui, tr("Shell"));
        let mut replaced = self.cached.tm_replaced;
        let mut changed = false;
        row_with(ui, lk, Some(Ico::Sliders), tr("Replace Windows Task Manager"), tr("Ctrl+Shift+Esc opens Delfin's Task Manager (asks for administrator rights)"), false, 160.0, |ui| {
            changed = switch(ui, &pal, &mut replaced);
        });
        if changed {
            if let Err(e) = crate::taskmgr::replace(replaced) {
                self.say(e, true);
            }
            self.cached.tm_replaced = crate::taskmgr::replaced();
        }
        if ext_row(ui, lk, Ico::Folder, tr("Delfin's settings folder"), "") {
            if let Some(d) = dirs::config_dir() {
                spawn_self(&[d.join("delfin").to_string_lossy().as_ref()]);
            }
        }
        ui.add_space(6.0);
        ui.label(RichText::new(tr("Panel options (minimize to tray, back to Explorer) are in the Delfin menu of the start menu.")).color(pal.dim).size(12.0));
    }
}

fn spawn_self(args: &[&str]) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).args(args).spawn();
    }
}

/// Restart the Delfin shell so an installed update takes over.
fn restart_shell() {
    use std::os::windows::process::CommandExt;
    let exe = crate::update::target_path().or_else(|| std::env::current_exe().ok()).unwrap_or_default();
    let script = format!(
        "Get-CimInstance Win32_Process -Filter \"Name='delfin.exe'\" | Where-Object {{ $_.CommandLine -like '*--shell*' }} | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force }}; Start-Sleep -Milliseconds 800; Start-Process -FilePath '{}' -ArgumentList '--shell'",
        exe.display()
    );
    let _ = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
        .creation_flags(0x0800_0000)
        .spawn();
}
