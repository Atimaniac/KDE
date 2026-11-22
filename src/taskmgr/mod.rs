//! Delfin Task Manager (`delfin.exe --taskmgr`): processes, performance,
//! start-up apps, details and services. Can replace Windows Task Manager
//! (every start of taskmgr.exe, also Ctrl+Shift+Esc and Ctrl+Alt+Del, opens it).

pub(crate) mod sys;

use crate::i18n::{tr, trf};
use crate::theme::{self, Palette};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Key, Rect, RichText, Sense, Shape, Stroke, TextureHandle,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use sys::{fmt_bits, fmt_bytes, fmt_pct, Snapshot};

const IFEO: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe";

// ------------------------------------------------------------------ replacing taskmgr.exe

/// Is Windows set to open Delfin instead of taskmgr.exe?
pub fn replaced() -> bool {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(IFEO)
        .and_then(|k| k.get_value::<String, _>("Debugger"))
        .is_ok_and(|v| v.to_lowercase().contains("delfin"))
}

fn is_admin() -> bool {
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

/// The exe Windows should start: the installed copy if there is one.
fn target_exe() -> PathBuf {
    let installed = crate::setup::installed_exe();
    if installed.exists() {
        installed
    } else {
        std::env::current_exe().unwrap_or(installed)
    }
}

/// `delfin.exe --replace-taskmgr on|off`. Needs administrator rights (the
/// setting lives in HKLM); without them it restarts itself elevated.
/// Works like "Replace Task Manager" in Microsoft's Process Explorer: an
/// "Image File Execution Options" debugger entry for taskmgr.exe.
pub fn replace(on: bool) -> Result<(), String> {
    if !is_admin() {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let arg = format!("--replace-taskmgr {}", if on { "on" } else { "off" });
        return if sys::run_elevated(&exe.to_string_lossy(), &arg) {
            Ok(())
        } else {
            Err(tr("Administrator rights are needed.").to_string())
        };
    }
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let hklm = winreg::RegKey::predef(HKEY_LOCAL_MACHINE);
    if on {
        let (k, _) = hklm.create_subkey(IFEO).map_err(|e| e.to_string())?;
        k.set_value("Debugger", &format!("\"{}\" --taskmgr", target_exe().display()))
            .map_err(|e| e.to_string())
    } else {
        match hklm.open_subkey_with_flags(IFEO, winreg::enums::KEY_SET_VALUE) {
            Ok(k) => match k.delete_value("Debugger") {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            },
            Err(_) => Ok(()),
        }
    }
}

// ------------------------------------------------------------------ app

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Processes,
    Performance,
    Startup,
    Details,
    Services,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Col {
    Name,
    Cpu,
    Mem,
    Disk,
    Gpu,
    Pid,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PerfItem {
    Cpu,
    Mem,
    Disk(usize),
    Net(usize),
    Gpu,
}

#[derive(Clone, PartialEq, Eq)]
enum Sel {
    Group(String),
    Pid(u32),
}

/// Processes of one program, shown as one row (expandable).
struct Group {
    key: String,
    name: String,
    path: Option<PathBuf>,
    pids: Vec<u32>,
    cpu: f32,
    mem: u64,
    io: f64,
    gpu: f32,
    app: bool,
    efficiency: bool,
}

struct RunDialog {
    text: String,
    admin: bool,
    error: String,
    focus: bool,
}

pub struct TaskMgr {
    pal: Palette,
    snap: Arc<Mutex<Snapshot>>,
    page: Page,
    query: String,
    focus_search: bool,
    sort: (Col, bool),
    expanded: HashSet<String>,
    sel: Option<Sel>,
    perf: PerfItem,
    startup: Option<Vec<sys::StartupItem>>,
    services: Option<(Vec<sys::Service>, Instant)>,
    message: Option<(String, Instant)>,
    run: Option<RunDialog>,
    icons: HashMap<PathBuf, Option<TextureHandle>>,
    on_top: bool,
    /// Service selected on the Services page.
    service_sel: Option<String>,
}

pub fn run() -> Result<(), eframe::Error> {
    // One window: a second start brings the open one to the front.
    let mutex = sys::wide("Local\\DelfinTaskManager");
    unsafe {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        let h = windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 0, mutex.as_ptr());
        if !h.is_null() && GetLastError() == ERROR_ALREADY_EXISTS {
            use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, ShowWindow};
            for title in ["Task Manager", "Menedżer zadań"] {
                let t = sys::wide(title);
                let w = FindWindowW(std::ptr::null(), t.as_ptr());
                if !w.is_null() {
                    ShowWindow(w, 9);
                    SetForegroundWindow(w);
                }
            }
            return Ok(());
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(tr("Task Manager"))
            .with_app_id("delfin-taskmgr")
            .with_icon(theme::app_icon())
            .with_inner_size([1080.0, 720.0])
            .with_min_inner_size([760.0, 480.0]),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "Delfin Task Manager",
        options,
        Box::new(|cc| {
            theme::install_fonts(&cc.egui_ctx);
            let dark = crate::config::Config::load().dark;
            theme::apply(&cc.egui_ctx, dark);
            Ok(Box::new(TaskMgr {
                pal: theme::palette(dark),
                snap: sys::start(cc.egui_ctx.clone()),
                page: start_page(),
                query: String::new(),
                focus_search: false,
                sort: (Col::Cpu, true),
                expanded: HashSet::new(),
                sel: None,
                perf: PerfItem::Cpu,
                startup: None,
                services: None,
                message: None,
                run: None,
                icons: HashMap::new(),
                on_top: false,
                service_sel: None,
            }))
        }),
    )
}

/// `--page performance|startup|details|services|settings` opens on that page.
fn start_page() -> Page {
    let args: Vec<String> = std::env::args().collect();
    let p = args.iter().position(|a| a == "--page").and_then(|i| args.get(i + 1)).map(|s| s.to_lowercase());
    match p.as_deref() {
        Some("performance") => Page::Performance,
        Some("startup") => Page::Startup,
        Some("details") => Page::Details,
        Some("services") => Page::Services,
        Some("settings") => Page::Settings,
        _ => Page::Processes,
    }
}

const CPU_COL: Color32 = Color32::from_rgb(0x3d, 0xae, 0xe9);
const MEM_COL: Color32 = Color32::from_rgb(0x9b, 0x59, 0xb6);
const DISK_COL: Color32 = Color32::from_rgb(0x27, 0xae, 0x60);
const NET_COL: Color32 = Color32::from_rgb(0xe9, 0x1e, 0x8c);
const GPU_COL: Color32 = Color32::from_rgb(0x1a, 0xbc, 0x9c);

/// Area graph of the last minute.
fn graph(p: &egui::Painter, r: Rect, hist: &[f32], max: f32, col: Color32, grid: Option<Color32>) {
    if let Some(g) = grid {
        p.rect_stroke(r, 0.0, Stroke::new(1.0_f32, g));
        for i in 1..4 {
            let y = r.top() + r.height() * i as f32 / 4.0;
            p.line_segment([pos2(r.left(), y), pos2(r.right(), y)], Stroke::new(1.0_f32, g.gamma_multiply(0.5)));
        }
        for i in 1..6 {
            let x = r.left() + r.width() * i as f32 / 6.0;
            p.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], Stroke::new(1.0_f32, g.gamma_multiply(0.5)));
        }
    }
    if hist.len() < 2 || max <= 0.0 {
        return;
    }
    let n = hist.len();
    let pts: Vec<egui::Pos2> = hist
        .iter()
        .enumerate()
        .map(|(i, v)| {
            pos2(
                r.left() + r.width() * i as f32 / (n - 1) as f32,
                r.bottom() - r.height() * (v / max).clamp(0.0, 1.0),
            )
        })
        .collect();
    // Filled area as vertical strips (convex pieces).
    for w in pts.windows(2) {
        p.add(Shape::convex_polygon(
            vec![w[0], w[1], pos2(w[1].x, r.bottom()), pos2(w[0].x, r.bottom())],
            col.gamma_multiply(0.22),
            Stroke::NONE,
        ));
    }
    p.add(Shape::line(pts, Stroke::new(1.5_f32, col)));
}

fn heat(pal: &Palette, f: f32) -> Color32 {
    // Darker = busier, like Windows.
    pal.accent.gamma_multiply(0.08 + 0.42 * f.clamp(0.0, 1.0))
}

fn priority_name(p: u32) -> &'static str {
    match p {
        0x100 => tr("Realtime"),
        0x80 => tr("High"),
        0x8000 => tr("Above normal"),
        0x4000 => tr("Below normal"),
        0x40 => tr("Low"),
        0x20 => tr("Normal"),
        _ => "",
    }
}

impl TaskMgr {
    fn say(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), Instant::now()));
    }

    fn icon(&mut self, ctx: &egui::Context, path: &Option<PathBuf>) -> Option<egui::TextureId> {
        let p = path.as_ref()?;
        if !self.icons.contains_key(p) {
            let t = crate::shell::win::file_icon_rgba(p, false).map(|(w, h, px)| {
                ctx.load_texture(p.to_string_lossy(), egui::ColorImage::from_rgba_unmultiplied([w, h], &px), egui::TextureOptions::LINEAR)
            });
            self.icons.insert(p.clone(), t);
        }
        self.icons.get(p).and_then(|t| t.as_ref().map(|t| t.id()))
    }

    fn groups(&self, s: &Snapshot) -> Vec<Group> {
        let q = self.query.trim().to_lowercase();
        let mut map: HashMap<String, Group> = HashMap::new();
        for p in &s.procs {
            if p.pid == 0 {
                continue;
            }
            let name = if p.desc.is_empty() { p.exe.trim_end_matches(".exe").to_string() } else { p.desc.clone() };
            if !q.is_empty() && !name.to_lowercase().contains(&q) && !p.exe.to_lowercase().contains(&q) && p.pid.to_string() != q {
                continue;
            }
            let key = p.path.as_ref().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_else(|| p.exe.to_lowercase());
            let g = map.entry(key.clone()).or_insert_with(|| Group {
                key,
                name: name.clone(),
                path: p.path.clone(),
                pids: Vec::new(),
                cpu: 0.0,
                mem: 0,
                io: 0.0,
                gpu: 0.0,
                app: false,
                efficiency: false,
            });
            g.pids.push(p.pid);
            g.cpu += p.cpu;
            g.mem += p.mem;
            g.io += p.io;
            g.gpu += p.gpu;
            g.app |= p.window.is_some();
            g.efficiency |= p.efficiency;
        }
        let mut v: Vec<Group> = map.into_values().collect();
        let (col, desc) = self.sort;
        v.sort_by(|a, b| {
            let o = match col {
                Col::Name | Col::Pid => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                Col::Cpu => a.cpu.partial_cmp(&b.cpu).unwrap_or(std::cmp::Ordering::Equal),
                Col::Mem => a.mem.cmp(&b.mem),
                Col::Disk => a.io.partial_cmp(&b.io).unwrap_or(std::cmp::Ordering::Equal),
                Col::Gpu => a.gpu.partial_cmp(&b.gpu).unwrap_or(std::cmp::Ordering::Equal),
            };
            if desc && col != Col::Name {
                o.reverse()
            } else if desc {
                o.reverse()
            } else {
                o
            }
        });
        v
    }

    fn selected_pids(&self, s: &Snapshot) -> Vec<u32> {
        match &self.sel {
            Some(Sel::Pid(p)) => vec![*p],
            Some(Sel::Group(k)) => s
                .procs
                .iter()
                .filter(|p| p.path.as_ref().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_else(|| p.exe.to_lowercase()) == *k)
                .map(|p| p.pid)
                .collect(),
            None => Vec::new(),
        }
    }

    fn end_task(&mut self, pids: &[u32]) {
        let mut err = None;
        for &pid in pids {
            if pid <= 4 {
                continue;
            }
            if let Err(e) = sys::kill(pid) {
                err = Some(e);
            }
        }
        match err {
            Some(e) => self.say(trf("Couldn't end the task: {}", &[&e])),
            None => self.sel = None,
        }
    }

    fn open_location(&mut self, path: &Option<PathBuf>) {
        if let Some(dir) = path.as_ref().and_then(|p| p.parent()) {
            if let Ok(me) = std::env::current_exe() {
                let _ = std::process::Command::new(me).arg(dir).spawn();
            }
        }
    }
}

impl eframe::App for TaskMgr {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        sys::WANT_GPU_TEMP.store(self.page == Page::Performance, std::sync::atomic::Ordering::Relaxed);
        let pal = self.pal;
        let snap = self.snap.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let s = &snap;

        // Shortcuts
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::F)) {
            self.focus_search = true;
        }
        if self.run.is_none() && ctx.input(|i| i.key_pressed(Key::Delete)) && matches!(self.page, Page::Processes | Page::Details) {
            let pids = self.selected_pids(s);
            self.end_task(&pids);
        }
        if ctx.input(|i| i.key_pressed(Key::F5)) {
            self.startup = None;
            self.services = None;
        }

        // ---- header
        egui::TopBottomPanel::top("tm-top")
            .frame(egui::Frame::none().fill(pal.window).inner_margin(egui::Margin::symmetric(14.0, 10.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
                    ui.painter().rect_filled(r, 6.0, pal.accent);
                    let c = r.center();
                    ui.painter().add(Shape::line(
                        vec![c + vec2(-9.0, 1.0), c + vec2(-4.0, 1.0), c + vec2(-2.0, -5.0), c + vec2(2.0, 6.0), c + vec2(4.0, -1.0), c + vec2(9.0, -1.0)],
                        Stroke::new(1.6_f32, Color32::WHITE),
                    ));
                    ui.label(RichText::new(tr("Task Manager")).size(16.0));
                    ui.add_space(24.0);
                    let te = ui.add(
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text(tr("Search for a process, app or service…  (Ctrl+F)"))
                            .desired_width(ui.available_width().min(460.0)),
                    );
                    if self.focus_search {
                        te.request_focus();
                        self.focus_search = false;
                    }
                });
            });

        // ---- sidebar
        egui::SidePanel::left("tm-side")
            .resizable(false)
            .exact_width(190.0)
            .frame(egui::Frame::none().fill(pal.window).inner_margin(egui::Margin::symmetric(10.0, 6.0)))
            .show(ctx, |ui| {
                let pages = [
                    (Page::Processes, tr("Processes")),
                    (Page::Performance, tr("Performance")),
                    (Page::Startup, tr("Startup apps")),
                    (Page::Details, tr("Details")),
                    (Page::Services, tr("Services")),
                ];
                for (pg, label) in pages {
                    if side_item(ui, &pal, self.page == pg, label) {
                        self.page = pg;
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.add_space(8.0);
                    if side_item(ui, &pal, self.page == Page::Settings, tr("Settings")) {
                        self.page = Page::Settings;
                    }
                });
            });

        // ---- status bar
        egui::TopBottomPanel::bottom("tm-status")
            .frame(egui::Frame::none().fill(pal.window).inner_margin(egui::Margin::symmetric(14.0, 5.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{}: {}   {}: {}   {}: {}",
                            tr("Processes"),
                            s.procs.len(),
                            tr("Threads"),
                            s.threads,
                            tr("Up time"),
                            uptime(s.uptime)
                        ))
                        .size(12.0)
                        .color(pal.dim),
                    );
                    if let Some((m, t)) = &self.message {
                        if t.elapsed() < Duration::from_secs(8) {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(m).size(12.0).color(pal.danger));
                            });
                        }
                    }
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(pal.view).inner_margin(egui::Margin::same(14.0)))
            .show(ctx, |ui| match self.page {
                Page::Processes => self.processes(ui, ctx, s),
                Page::Performance => self.performance(ui, s),
                Page::Startup => self.startup_page(ui),
                Page::Details => self.details(ui, ctx, s),
                Page::Services => self.services_page(ui),
                Page::Settings => self.settings(ui),
            });

        self.run_dialog(ctx);
    }
}

fn side_item(ui: &mut egui::Ui, pal: &Palette, on: bool, label: &str) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
    if on {
        ui.painter().rect_filled(r, 6.0, pal.select);
    } else if resp.hovered() {
        ui.painter().rect_filled(r, 6.0, pal.hover);
    }
    ui.painter().text(pos2(r.left() + 12.0, r.center().y), Align2::LEFT_CENTER, label, FontId::proportional(14.0), pal.text);
    ui.add_space(2.0);
    resp.clicked()
}

fn uptime(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}:{:02}:{:02}", s / 86400, s / 3600 % 24, s / 60 % 60, s % 60)
}

/// Page title with buttons on the right.
fn page_header(ui: &mut egui::Ui, title: &str, right: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(20.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
    ui.add_space(8.0);
}

// ------------------------------------------------------------------ Processes

impl TaskMgr {
    fn processes(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, s: &Snapshot) {
        let pal = self.pal;
        let sel_pids = self.selected_pids(s);
        let mut end = false;
        let mut new_task = false;
        page_header(ui, tr("Processes"), |ui| {
            if ui.add_enabled(!sel_pids.is_empty(), egui::Button::new(tr("End task"))).clicked() {
                end = true;
            }
            if ui.button(tr("Run new task")).clicked() {
                new_task = true;
            }
        });
        if end {
            self.end_task(&sel_pids);
        }
        if new_task {
            self.run = Some(RunDialog { text: String::new(), admin: false, error: String::new(), focus: true });
        }

        // Summary cards (click: performance page).
        let disk0 = s.disks.first().cloned().unwrap_or_default();
        let net0 = s.nets.first().cloned().unwrap_or_default();
        let gpu = s.gpu.clone().unwrap_or_default();
        let cards: [(PerfItem, &str, String, &[f32], f32, Color32); 5] = [
            (PerfItem::Cpu, "CPU", format!("{:.0}%  {:.2} GHz", s.cpu, s.cpu_ghz), &s.cpu_history, 100.0, CPU_COL),
            (
                PerfItem::Mem,
                tr("Memory"),
                format!("{} / {} ({:.0}%)", fmt_bytes(s.mem_used as f64), fmt_bytes(s.mem_total as f64), s.mem_percent()),
                &s.mem_history,
                100.0,
                MEM_COL,
            ),
            (PerfItem::Disk(0), tr("Disk"), format!("{}  {:.0}%", disk_label(&disk0.name), disk0.active), &disk0.history, 100.0, DISK_COL),
            (
                PerfItem::Net(0),
                tr("Network"),
                format!("↓ {}  ↑ {}", fmt_bits(net0.rx * 8.0), fmt_bits(net0.tx * 8.0)),
                &net0.history,
                net0.history.iter().copied().fold(1.0, f32::max),
                NET_COL,
            ),
            (PerfItem::Gpu, "GPU", format!("{:.0}%{}", gpu.util, gpu.temp.map(|t| format!(" ({t} °C)")).unwrap_or_default()), &gpu.history, 100.0, GPU_COL),
        ];
        let w = (ui.available_width() - 4.0 * 10.0) / 5.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            for (item, title, sub, hist, max, col) in cards {
                let (r, resp) = ui.allocate_exact_size(vec2(w, 86.0), Sense::click());
                ui.painter().rect(r, 8.0, if resp.hovered() { pal.header } else { pal.window }, Stroke::new(1.0_f32, pal.border));
                ui.painter().text(r.min + vec2(12.0, 10.0), Align2::LEFT_TOP, title, FontId::proportional(14.0), pal.text);
                ui.painter().with_clip_rect(r.shrink(4.0)).text(r.min + vec2(12.0, 30.0), Align2::LEFT_TOP, sub, FontId::proportional(11.5), pal.dim);
                graph(ui.painter(), Rect::from_min_max(pos2(r.left() + 2.0, r.top() + 50.0), r.max - vec2(2.0, 2.0)), hist, max, col, None);
                if resp.clicked() {
                    self.perf = item;
                    self.page = Page::Performance;
                }
            }
        });
        ui.add_space(10.0);

        // Table
        let groups = self.groups(s);
        let (apps, bg): (Vec<&Group>, Vec<&Group>) = groups.iter().partition(|g| g.app);
        let full = ui.available_rect_before_wrap();
        let widths = [0.0, 90.0, 110.0, 100.0, 80.0]; // name takes the rest
        let name_w = full.width() - widths.iter().sum::<f32>() - 16.0;
        let cols = [(Col::Name, tr("Name"), name_w), (Col::Cpu, "CPU", widths[1]), (Col::Mem, tr("Memory"), widths[2]), (Col::Disk, tr("Disk"), widths[3]), (Col::Gpu, "GPU", widths[4])];
        let totals = [
            String::new(),
            format!("{:.0}%", s.cpu),
            format!("{:.0}%", s.mem_percent()),
            format!("{:.0}%", disk0.active),
            format!("{:.0}%", gpu.util),
        ];
        self.table_header(ui, &cols, Some(&totals));

        let mut actions: Vec<RowAction> = Vec::new();
        egui::ScrollArea::vertical().id_salt("procs").auto_shrink([false, false]).show(ui, |ui| {
            for (label, list) in [(tr("Apps"), &apps), (tr("Background processes"), &bg)] {
                ui.add_space(6.0);
                ui.label(RichText::new(format!("{label} ({})", list.len())).strong().size(14.0));
                for g in list.iter() {
                    let expanded = self.expanded.contains(&g.key);
                    let selected = self.sel == Some(Sel::Group(g.key.clone()));
                    let icon = self.icon(ctx, &g.path);
                    let resp = proc_row(
                        ui,
                        &pal,
                        &cols,
                        RowData {
                            indent: 0.0,
                            arrow: if g.pids.len() > 1 { Some(expanded) } else { None },
                            icon,
                            name: if g.pids.len() > 1 { format!("{} ({})", g.name, g.pids.len()) } else { g.name.clone() },
                            leaf: g.efficiency,
                            cpu: g.cpu,
                            mem: g.mem,
                            mem_total: s.mem_total,
                            io: g.io,
                            gpu: g.gpu,
                            selected,
                        },
                    );
                    row_actions(&resp, Sel::Group(g.key.clone()), &g.path, g.pids.clone(), g.efficiency, &mut actions);
                    if resp.row.double_clicked() || resp.arrow_clicked {
                        actions.push(RowAction::Toggle(g.key.clone()));
                    }
                    if expanded && g.pids.len() > 1 {
                        for p in s.procs.iter().filter(|p| g.pids.contains(&p.pid)) {
                            let resp = proc_row(
                                ui,
                                &pal,
                                &cols,
                                RowData {
                                    indent: 28.0,
                                    arrow: None,
                                    icon,
                                    name: p.window.clone().unwrap_or_else(|| format!("{} ({})", p.exe, p.pid)),
                                    leaf: p.efficiency,
                                    cpu: p.cpu,
                                    mem: p.mem,
                                    mem_total: s.mem_total,
                                    io: p.io,
                                    gpu: p.gpu,
                                    selected: self.sel == Some(Sel::Pid(p.pid)),
                                },
                            );
                            row_actions(&resp, Sel::Pid(p.pid), &p.path, vec![p.pid], p.efficiency, &mut actions);
                        }
                    }
                }
            }
        });
        self.apply(actions);
    }

    fn table_header(&mut self, ui: &mut egui::Ui, cols: &[(Col, &str, f32)], totals: Option<&[String; 5]>) {
        let pal = self.pal;
        let h = if totals.is_some() { 46.0 } else { 28.0 };
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
        let mut x = r.left();
        for (i, (col, label, w)) in cols.iter().enumerate() {
            let cell = Rect::from_min_size(pos2(x, r.top()), vec2(*w, h));
            let resp = ui.interact(cell, ui.id().with(("hdr", i)), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(cell, 0.0, pal.hover);
            }
            let arrow = if self.sort.0 == *col { if self.sort.1 { " ▼" } else { " ▲" } } else { "" };
            // Numbers are right-aligned, text columns left-aligned.
            let numeric = i > 0 && matches!(col, Col::Cpu | Col::Mem | Col::Disk | Col::Gpu | Col::Pid);
            let (align, tx) = if numeric { (Align2::RIGHT_BOTTOM, cell.right() - 8.0) } else { (Align2::LEFT_BOTTOM, cell.left() + 8.0) };
            ui.painter().text(pos2(tx, cell.bottom() - 6.0), align, format!("{label}{arrow}"), FontId::proportional(12.5), pal.dim);
            if let Some(t) = totals {
                if numeric {
                    ui.painter().text(pos2(tx, cell.top() + 4.0), Align2::RIGHT_TOP, &t[i], FontId::proportional(16.0), pal.text);
                }
            }
            if resp.clicked() {
                self.sort = if self.sort.0 == *col { (*col, !self.sort.1) } else { (*col, *col != Col::Name && *col != Col::Pid) };
            }
            x += w;
        }
        ui.painter().line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0_f32, pal.border));
    }

    fn apply(&mut self, actions: Vec<RowAction>) {
        for a in actions {
            match a {
                RowAction::Select(s) => self.sel = Some(s),
                RowAction::Toggle(k) => {
                    if !self.expanded.remove(&k) {
                        self.expanded.insert(k);
                    }
                }
                RowAction::End(pids) => self.end_task(&pids),
                RowAction::Efficiency(pids, on) => {
                    let mut ok = true;
                    for p in pids {
                        ok &= sys::set_efficiency(p, on);
                    }
                    if !ok {
                        self.say(tr("Windows didn't allow changing this process."));
                    }
                }
                RowAction::Priority(pid, class) => {
                    if !sys::set_priority(pid, class) {
                        self.say(tr("Windows didn't allow changing this process."));
                    }
                }
                RowAction::Location(p) => self.open_location(&p),
                RowAction::Details(pid) => {
                    self.page = Page::Details;
                    self.sel = Some(Sel::Pid(pid));
                }
            }
        }
    }
}

enum RowAction {
    Select(Sel),
    Toggle(String),
    End(Vec<u32>),
    Efficiency(Vec<u32>, bool),
    Priority(u32, u32),
    Location(Option<PathBuf>),
    Details(u32),
}

struct RowData {
    indent: f32,
    arrow: Option<bool>,
    icon: Option<egui::TextureId>,
    name: String,
    leaf: bool,
    cpu: f32,
    mem: u64,
    mem_total: u64,
    io: f64,
    gpu: f32,
    selected: bool,
}

struct RowResp {
    row: egui::Response,
    arrow_clicked: bool,
}

fn proc_row(ui: &mut egui::Ui, pal: &Palette, cols: &[(Col, &str, f32)], d: RowData) -> RowResp {
    let h = 32.0;
    let (r, row) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    if !ui.is_rect_visible(r) {
        return RowResp { row, arrow_clicked: false };
    }
    let p = ui.painter();
    if d.selected {
        p.rect_filled(r, 0.0, pal.select);
    } else if row.hovered() {
        p.rect_filled(r, 0.0, pal.hover);
    }
    let mut arrow_clicked = false;
    let name_w = cols[0].2;
    let ax = r.left() + 6.0 + d.indent;
    if let Some(open) = d.arrow {
        let ar = Rect::from_center_size(pos2(ax + 6.0, r.center().y), vec2(16.0, 16.0));
        let a = ui.interact(ar, row.id.with("arrow"), Sense::click());
        arrow_clicked = a.clicked();
        let c = ar.center();
        let st = Stroke::new(1.4_f32, pal.dim);
        if open {
            p.line_segment([c + vec2(-4.0, -2.0), c + vec2(0.0, 2.0)], st);
            p.line_segment([c + vec2(0.0, 2.0), c + vec2(4.0, -2.0)], st);
        } else {
            p.line_segment([c + vec2(-2.0, -4.0), c + vec2(2.0, 0.0)], st);
            p.line_segment([c + vec2(2.0, 0.0), c + vec2(-2.0, 4.0)], st);
        }
    }
    let ir = Rect::from_min_size(pos2(ax + 18.0, r.center().y - 9.0), vec2(18.0, 18.0));
    match d.icon {
        Some(t) => {
            p.image(t, ir, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            p.rect_stroke(ir.shrink(2.0), 3.0, Stroke::new(1.0_f32, pal.dim));
        }
    }
    let name_r = Rect::from_min_max(pos2(ir.right() + 8.0, r.top()), pos2(r.left() + name_w - 30.0, r.bottom()));
    p.with_clip_rect(name_r).text(pos2(name_r.left(), r.center().y), Align2::LEFT_CENTER, &d.name, FontId::proportional(13.5), pal.text);
    if d.leaf {
        // efficiency mode: a small green leaf
        let c = pos2(r.left() + name_w - 16.0, r.center().y);
        p.add(Shape::convex_polygon(
            vec![c + vec2(-6.0, 5.0), c + vec2(-4.0, -3.0), c + vec2(3.0, -6.0), c + vec2(6.0, -5.0), c + vec2(4.0, 2.0), c + vec2(-3.0, 5.0)],
            Color32::from_rgb(0x27, 0xae, 0x60),
            Stroke::NONE,
        ));
    }
    let values = [
        (fmt_pct(d.cpu), d.cpu / 25.0),
        (fmt_bytes(d.mem as f64), if d.mem_total > 0 { d.mem as f32 / d.mem_total as f32 * 8.0 } else { 0.0 }),
        (format!("{}/s", fmt_bytes(d.io)), (d.io / 20e6) as f32),
        (fmt_pct(d.gpu), d.gpu / 25.0),
    ];
    let mut x = r.left() + name_w;
    for (i, (txt, f)) in values.iter().enumerate() {
        let w = cols[i + 1].2;
        let cell = Rect::from_min_size(pos2(x, r.top()), vec2(w, h));
        if !d.selected {
            p.rect_filled(cell.shrink2(vec2(0.0, 0.5)), 0.0, heat(pal, *f));
        }
        p.text(pos2(cell.right() - 8.0, r.center().y), Align2::RIGHT_CENTER, txt, FontId::proportional(13.0), pal.text);
        x += w;
    }
    RowResp { row, arrow_clicked }
}

fn row_actions(resp: &RowResp, sel: Sel, path: &Option<PathBuf>, pids: Vec<u32>, efficiency: bool, out: &mut Vec<RowAction>) {
    if resp.row.clicked() || resp.row.secondary_clicked() {
        out.push(RowAction::Select(sel));
    }
    let path = path.clone();
    resp.row.context_menu(|ui| {
        if ui.button(tr("End task")).clicked() {
            out.push(RowAction::End(pids.clone()));
            ui.close_menu();
        }
        if ui.button(if efficiency { tr("Turn off efficiency mode") } else { tr("Efficiency mode") }).clicked() {
            out.push(RowAction::Efficiency(pids.clone(), !efficiency));
            ui.close_menu();
        }
        if path.is_some() && ui.button(tr("Open file location")).clicked() {
            out.push(RowAction::Location(path.clone()));
            ui.close_menu();
        }
        if let Some(&pid) = pids.first() {
            if ui.button(tr("Go to details")).clicked() {
                out.push(RowAction::Details(pid));
                ui.close_menu();
            }
        }
    });
}

fn disk_label(name: &str) -> String {
    // PDH names disks like "0 C: D:"
    let mut parts = name.split_whitespace();
    let n = parts.next().unwrap_or("");
    let letters: Vec<&str> = parts.collect();
    if letters.is_empty() {
        format!("{} {n}", tr("Disk"))
    } else {
        format!("{} {n} ({})", tr("Disk"), letters.join(" "))
    }
}

// ------------------------------------------------------------------ Performance

impl TaskMgr {
    fn performance(&mut self, ui: &mut egui::Ui, s: &Snapshot) {
        let pal = self.pal;
        page_header(ui, tr("Performance"), |_| {});
        let full = ui.available_rect_before_wrap();
        let list = Rect::from_min_size(full.min, vec2(240.0, full.height()));
        let main = Rect::from_min_max(pos2(list.right() + 16.0, full.top()), full.max);

        // Left: one tile per device.
        let mut items: Vec<(PerfItem, String, String, Vec<f32>, f32, Color32)> = vec![
            (PerfItem::Cpu, "CPU".into(), format!("{:.0}%  {:.2} GHz", s.cpu, s.cpu_ghz), s.cpu_history.clone(), 100.0, CPU_COL),
            (
                PerfItem::Mem,
                tr("Memory").into(),
                format!("{} / {} ({:.0}%)", fmt_bytes(s.mem_used as f64), fmt_bytes(s.mem_total as f64), s.mem_percent()),
                s.mem_history.clone(),
                100.0,
                MEM_COL,
            ),
        ];
        for (i, d) in s.disks.iter().enumerate() {
            items.push((PerfItem::Disk(i), disk_label(&d.name), format!("{:.0}%", d.active), d.history.clone(), 100.0, DISK_COL));
        }
        for (i, n) in s.nets.iter().enumerate() {
            let max = n.history.iter().copied().fold(1.0, f32::max);
            items.push((PerfItem::Net(i), n.name.clone(), format!("↓ {}  ↑ {}", fmt_bits(n.rx * 8.0), fmt_bits(n.tx * 8.0)), n.history.clone(), max, NET_COL));
        }
        if let Some(g) = &s.gpu {
            items.push((PerfItem::Gpu, "GPU".into(), format!("{:.0}%{}", g.util, g.temp.map(|t| format!(" ({t} °C)")).unwrap_or_default()), g.history.clone(), 100.0, GPU_COL));
        }
        let mut y = list.top();
        for (item, title, sub, hist, max, col) in &items {
            let r = Rect::from_min_size(pos2(list.left(), y), vec2(list.width(), 64.0));
            let resp = ui.interact(r, ui.id().with(("perf", title)), Sense::click());
            let on = self.perf == *item;
            ui.painter().rect(r, 8.0, if on { pal.select } else if resp.hovered() { pal.hover } else { pal.window }, Stroke::new(1.0_f32, pal.border));
            let g = Rect::from_min_size(r.min + vec2(8.0, 12.0), vec2(64.0, 40.0));
            ui.painter().rect_filled(g, 2.0, pal.view);
            graph(ui.painter(), g, hist, *max, *col, None);
            let clip = ui.painter().with_clip_rect(r.shrink(2.0));
            clip.text(pos2(g.right() + 10.0, r.top() + 14.0), Align2::LEFT_TOP, title, FontId::proportional(13.5), pal.text);
            clip.text(pos2(g.right() + 10.0, r.top() + 34.0), Align2::LEFT_TOP, sub, FontId::proportional(11.5), pal.dim);
            if resp.clicked() {
                self.perf = *item;
            }
            y += 70.0;
        }

        // Right: the selected device.
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(main));
        let ui = &mut child;
        let big = |ui: &mut egui::Ui, title: &str, sub: &str, hist: &[f32], max: f32, col: Color32, label: &str| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(22.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(sub).color(pal.dim));
                });
            });
            ui.label(RichText::new(label).size(12.0).color(pal.dim));
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), (ui.available_height() * 0.55).max(140.0)), Sense::hover());
            graph(ui.painter(), r, hist, max, col, Some(col.gamma_multiply(0.35)));
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("60 seconds")).size(11.0).color(pal.dim));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("0").size(11.0).color(pal.dim));
                });
            });
            ui.add_space(10.0);
        };
        let stat = |ui: &mut egui::Ui, k: &str, v: String| {
            ui.vertical(|ui| {
                ui.label(RichText::new(k).size(12.0).color(pal.dim));
                ui.label(RichText::new(v).size(18.0));
            });
            ui.add_space(24.0);
        };
        match self.perf {
            PerfItem::Cpu => {
                big(ui, "CPU", &s.cpu_name, &s.cpu_history, 100.0, CPU_COL, tr("% Utilization"));
                ui.horizontal_wrapped(|ui| {
                    stat(ui, tr("Utilization"), format!("{:.0}%", s.cpu));
                    stat(ui, tr("Speed"), format!("{:.2} GHz", s.cpu_ghz));
                    stat(ui, tr("Processes"), s.procs.len().to_string());
                    stat(ui, tr("Threads"), s.threads.to_string());
                    stat(ui, tr("Logical processors"), s.cpu_cores.len().to_string());
                    stat(ui, tr("Up time"), uptime(s.uptime));
                });
                // One bar per logical processor.
                if !s.cpu_cores.is_empty() {
                    ui.add_space(8.0);
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
                    let n = s.cpu_cores.len() as f32;
                    let w = r.width() / n;
                    for (i, v) in s.cpu_cores.iter().enumerate() {
                        let b = Rect::from_min_size(pos2(r.left() + i as f32 * w, r.top()), vec2(w - 2.0, r.height()));
                        ui.painter().rect_filled(b, 2.0, pal.window);
                        let fill = Rect::from_min_max(pos2(b.left(), b.bottom() - b.height() * v / 100.0), b.max);
                        ui.painter().rect_filled(fill, 2.0, CPU_COL.gamma_multiply(0.8));
                    }
                }
            }
            PerfItem::Mem => {
                big(ui, tr("Memory"), &fmt_bytes(s.mem_total as f64), &s.mem_history, 100.0, MEM_COL, tr("Memory usage"));
                ui.horizontal_wrapped(|ui| {
                    stat(ui, tr("In use"), fmt_bytes(s.mem_used as f64));
                    stat(ui, tr("Available"), fmt_bytes((s.mem_total - s.mem_used) as f64));
                    stat(ui, tr("Committed"), format!("{} / {}", fmt_bytes(s.commit_used as f64), fmt_bytes(s.commit_total as f64)));
                });
            }
            PerfItem::Disk(i) => {
                if let Some(d) = s.disks.get(i) {
                    big(ui, &disk_label(&d.name), "", &d.history, 100.0, DISK_COL, tr("Active time"));
                    ui.horizontal_wrapped(|ui| {
                        stat(ui, tr("Active time"), format!("{:.0}%", d.active));
                        stat(ui, tr("Read speed"), format!("{}/s", fmt_bytes(d.read)));
                        stat(ui, tr("Write speed"), format!("{}/s", fmt_bytes(d.write)));
                    });
                }
            }
            PerfItem::Net(i) => {
                if let Some(n) = s.nets.get(i) {
                    let max = n.history.iter().copied().fold(1.0, f32::max);
                    big(ui, tr("Network"), &n.name, &n.history, max, NET_COL, &trf("Throughput (max {})", &[&fmt_bits(max as f64)]));
                    ui.horizontal_wrapped(|ui| {
                        stat(ui, tr("Receive"), fmt_bits(n.rx * 8.0));
                        stat(ui, tr("Send"), fmt_bits(n.tx * 8.0));
                    });
                }
            }
            PerfItem::Gpu => {
                if let Some(g) = &s.gpu {
                    big(ui, "GPU", &g.name, &g.history, 100.0, GPU_COL, tr("% Utilization"));
                    ui.horizontal_wrapped(|ui| {
                        stat(ui, tr("Utilization"), format!("{:.0}%", g.util));
                        if let Some(t) = g.temp {
                            stat(ui, tr("Temperature"), format!("{t} °C"));
                        }
                    });
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Startup apps

impl TaskMgr {
    fn startup_page(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        if self.startup.is_none() {
            self.startup = Some(sys::startup_items());
        }
        page_header(ui, tr("Startup apps"), |ui| {
            if ui.button(tr("Refresh")).clicked() {
                self.startup = None;
            }
        });
        ui.label(RichText::new(tr("Programs that start when you sign in. Switching one off here works for Delfin and for Windows.")).color(pal.dim));
        ui.add_space(8.0);
        let mut change: Option<(usize, bool)> = None;
        egui::ScrollArea::vertical().id_salt("startup").auto_shrink([false, false]).show(ui, |ui| {
            for (i, it) in self.startup.as_ref().map(|v| v.as_slice()).unwrap_or_default().iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::hover());
                if resp.hovered() {
                    ui.painter().rect_filled(r, 6.0, pal.hover);
                }
                let clip = ui.painter().with_clip_rect(Rect::from_min_max(r.min, pos2(r.right() - 200.0, r.bottom())));
                clip.text(pos2(r.left() + 12.0, r.top() + 8.0), Align2::LEFT_TOP, &it.name, FontId::proportional(14.0), pal.text);
                clip.text(pos2(r.left() + 12.0, r.top() + 28.0), Align2::LEFT_TOP, &it.command, FontId::proportional(11.5), pal.dim);
                let status = if it.enabled { tr("Enabled") } else { tr("Disabled") };
                ui.painter().text(pos2(r.right() - 130.0, r.center().y), Align2::RIGHT_CENTER, status, FontId::proportional(13.0), if it.enabled { pal.text } else { pal.dim });
                let br = Rect::from_min_size(pos2(r.right() - 116.0, r.center().y - 14.0), vec2(104.0, 28.0));
                let label = if it.enabled { tr("Disable") } else { tr("Enable") };
                if ui.put(br, egui::Button::new(label)).on_hover_text(if it.machine { tr("For all users: Windows asks for administrator rights.") } else { "" }).clicked() {
                    change = Some((i, !it.enabled));
                }
            }
        });
        if let Some((i, on)) = change {
            if let Some(it) = self.startup.as_ref().and_then(|v| v.get(i)).cloned() {
                match sys::set_startup(&it, on) {
                    Ok(()) => {
                        if let Some(v) = &mut self.startup {
                            v[i].enabled = on;
                        }
                    }
                    Err(e) => self.say(e),
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Details

impl TaskMgr {
    fn details(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, s: &Snapshot) {
        let pal = self.pal;
        let sel_pids = self.selected_pids(s);
        let mut end = false;
        page_header(ui, tr("Details"), |ui| {
            if ui.add_enabled(!sel_pids.is_empty(), egui::Button::new(tr("End task"))).clicked() {
                end = true;
            }
        });
        if end {
            self.end_task(&sel_pids);
        }
        let q = self.query.trim().to_lowercase();
        let mut procs: Vec<&sys::Proc> = s
            .procs
            .iter()
            .filter(|p| q.is_empty() || p.exe.to_lowercase().contains(&q) || p.desc.to_lowercase().contains(&q) || p.pid.to_string() == q)
            .collect();
        let (col, desc) = self.sort;
        procs.sort_by(|a, b| {
            let o = match col {
                Col::Name => a.exe.to_lowercase().cmp(&b.exe.to_lowercase()),
                Col::Pid => a.pid.cmp(&b.pid),
                Col::Cpu => a.cpu.partial_cmp(&b.cpu).unwrap_or(std::cmp::Ordering::Equal),
                Col::Mem => a.mem.cmp(&b.mem),
                Col::Disk => a.io.partial_cmp(&b.io).unwrap_or(std::cmp::Ordering::Equal),
                Col::Gpu => a.gpu.partial_cmp(&b.gpu).unwrap_or(std::cmp::Ordering::Equal),
            };
            if desc {
                o.reverse()
            } else {
                o
            }
        });
        let w = ui.available_width();
        let cols = [
            (Col::Name, tr("Name"), 220.0),
            (Col::Pid, "PID", 70.0),
            (Col::Name, tr("User"), 130.0),
            (Col::Cpu, "CPU", 70.0),
            (Col::Mem, tr("Memory"), 100.0),
            (Col::Name, tr("Priority"), 110.0),
            (Col::Name, tr("Path"), (w - 700.0).max(80.0)),
        ];
        self.table_header(ui, &cols, None);
        let mut actions = Vec::new();
        egui::ScrollArea::vertical().id_salt("details").auto_shrink([false, false]).show_rows(ui, 26.0, procs.len(), |ui, range| {
            for p in &procs[range] {
                let (r, row) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
                let selected = self.sel == Some(Sel::Pid(p.pid));
                if selected {
                    ui.painter().rect_filled(r, 0.0, pal.select);
                } else if row.hovered() {
                    ui.painter().rect_filled(r, 0.0, pal.hover);
                }
                let icon = self.icon(ctx, &p.path);
                let ir = Rect::from_min_size(pos2(r.left() + 6.0, r.center().y - 8.0), vec2(16.0, 16.0));
                if let Some(t) = icon {
                    ui.painter().image(t, ir, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                let texts = [
                    p.exe.clone(),
                    p.pid.to_string(),
                    p.user.clone(),
                    fmt_pct(p.cpu),
                    fmt_bytes(p.mem as f64),
                    priority_name(p.priority).to_string(),
                    p.path.as_ref().map(|x| x.display().to_string()).unwrap_or_default(),
                ];
                let mut x = r.left();
                for (i, t) in texts.iter().enumerate() {
                    let cw = cols[i].2;
                    let cell = Rect::from_min_size(pos2(x, r.top()), vec2(cw, r.height()));
                    let right = matches!(i, 1 | 3 | 4);
                    let tx = if i == 0 { cell.left() + 28.0 } else if right { cell.right() - 8.0 } else { cell.left() + 8.0 };
                    ui.painter().with_clip_rect(cell.shrink2(vec2(2.0, 0.0))).text(
                        pos2(tx, r.center().y),
                        if right { Align2::RIGHT_CENTER } else { Align2::LEFT_CENTER },
                        t,
                        FontId::proportional(12.5),
                        if i == 6 { pal.dim } else { pal.text },
                    );
                    x += cw;
                }
                if row.clicked() || row.secondary_clicked() {
                    actions.push(RowAction::Select(Sel::Pid(p.pid)));
                }
                let (pid, path, eff) = (p.pid, p.path.clone(), p.efficiency);
                row.context_menu(|ui| {
                    if ui.button(tr("End task")).clicked() {
                        actions.push(RowAction::End(vec![pid]));
                        ui.close_menu();
                    }
                    if ui.button(tr("End process tree")).clicked() {
                        let mut tree = vec![pid];
                        let mut i = 0;
                        while i < tree.len() {
                            let parent = tree[i];
                            tree.extend(s.procs.iter().filter(|c| c.ppid == parent && c.pid != parent).map(|c| c.pid));
                            i += 1;
                        }
                        tree.reverse();
                        actions.push(RowAction::End(tree));
                        ui.close_menu();
                    }
                    ui.menu_button(tr("Set priority"), |ui| {
                        for class in [0x80u32, 0x8000, 0x20, 0x4000, 0x40] {
                            if ui.button(priority_name(class)).clicked() {
                                actions.push(RowAction::Priority(pid, class));
                                ui.close_menu();
                            }
                        }
                    });
                    if ui.button(if eff { tr("Turn off efficiency mode") } else { tr("Efficiency mode") }).clicked() {
                        actions.push(RowAction::Efficiency(vec![pid], !eff));
                        ui.close_menu();
                    }
                    if path.is_some() && ui.button(tr("Open file location")).clicked() {
                        actions.push(RowAction::Location(path.clone()));
                        ui.close_menu();
                    }
                });
            }
        });
        self.apply(actions);
    }
}

// ------------------------------------------------------------------ Services

impl TaskMgr {
    fn services_page(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let stale = self.services.as_ref().is_none_or(|(_, t)| t.elapsed() > Duration::from_secs(3));
        if stale {
            self.services = Some((sys::services(), Instant::now()));
        }
        // Buttons for the selected service.
        let selected = self
            .service_sel
            .as_ref()
            .and_then(|n| self.services.as_ref().and_then(|(v, _)| v.iter().find(|s| &s.name == n)))
            .map(|s| (s.name.clone(), s.state));
        let mut cmd: Option<(String, &'static str)> = None;
        page_header(ui, tr("Services"), |ui| {
            if ui.button(tr("Open Services")).clicked() {
                // Through the shell: the console may ask for administrator rights.
                sys::shell_open("services.msc");
            }
            let running = selected.as_ref().is_some_and(|(_, st)| *st == 4);
            let stopped = selected.as_ref().is_some_and(|(_, st)| *st == 1);
            if ui.add_enabled(running, egui::Button::new(tr("Restart"))).clicked() {
                cmd = selected.as_ref().map(|(n, _)| (n.clone(), "restart"));
            }
            if ui.add_enabled(running, egui::Button::new(tr("Stop"))).clicked() {
                cmd = selected.as_ref().map(|(n, _)| (n.clone(), "stop"));
            }
            if ui.add_enabled(stopped, egui::Button::new(tr("Start"))).clicked() {
                cmd = selected.as_ref().map(|(n, _)| (n.clone(), "start"));
            }
        });
        let q = self.query.trim().to_lowercase();
        let list: Vec<sys::Service> = self
            .services
            .as_ref()
            .map(|(v, _)| v.iter().filter(|s| q.is_empty() || s.name.to_lowercase().contains(&q) || s.display.to_lowercase().contains(&q)).cloned().collect())
            .unwrap_or_default();
        let w = ui.available_width();
        let cols = [(Col::Name, tr("Name"), 200.0), (Col::Pid, "PID", 70.0), (Col::Name, tr("Description"), (w - 400.0).max(120.0)), (Col::Name, tr("Status"), 130.0)];
        self.table_header(ui, &cols, None);
        egui::ScrollArea::vertical().id_salt("services").auto_shrink([false, false]).show_rows(ui, 26.0, list.len(), |ui, range| {
            for sv in &list[range] {
                let (r, row) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
                if self.service_sel.as_deref() == Some(sv.name.as_str()) {
                    ui.painter().rect_filled(r, 0.0, pal.select);
                } else if row.hovered() {
                    ui.painter().rect_filled(r, 0.0, pal.hover);
                }
                if row.clicked() || row.secondary_clicked() {
                    self.service_sel = Some(sv.name.clone());
                }
                let status = match sv.state {
                    4 => tr("Running"),
                    2 => tr("Starting"),
                    3 => tr("Stopping"),
                    7 => tr("Paused"),
                    _ => tr("Stopped"),
                };
                let texts = [sv.name.clone(), if sv.pid > 0 { sv.pid.to_string() } else { String::new() }, sv.display.clone(), status.to_string()];
                let mut x = r.left();
                for (i, t) in texts.iter().enumerate() {
                    let cw = cols[i].2;
                    let cell = Rect::from_min_size(pos2(x, r.top()), vec2(cw, r.height()));
                    let right = i == 1;
                    ui.painter().with_clip_rect(cell.shrink2(vec2(2.0, 0.0))).text(
                        pos2(if right { cell.right() - 8.0 } else { cell.left() + 8.0 }, r.center().y),
                        if right { Align2::RIGHT_CENTER } else { Align2::LEFT_CENTER },
                        t,
                        FontId::proportional(12.5),
                        if i == 3 && sv.state != 4 { pal.dim } else { pal.text },
                    );
                    x += cw;
                }
                let name = sv.name.clone();
                let running = sv.state == 4;
                row.context_menu(|ui| {
                    if !running && ui.button(tr("Start")).clicked() {
                        cmd = Some((name.clone(), "start"));
                        ui.close_menu();
                    }
                    if running && ui.button(tr("Stop")).clicked() {
                        cmd = Some((name.clone(), "stop"));
                        ui.close_menu();
                    }
                    if running && ui.button(tr("Restart")).clicked() {
                        cmd = Some((name.clone(), "restart"));
                        ui.close_menu();
                    }
                });
            }
        });
        if let Some((name, what)) = cmd {
            // Services need administrator rights; `net` waits until it's done.
            let args = match what {
                "restart" => format!("/c net stop \"{name}\" & net start \"{name}\""),
                w => format!("/c net {w} \"{name}\""),
            };
            if !sys::run_elevated("cmd.exe", &args) {
                self.say(tr("Administrator rights are needed."));
            }
            self.services = None;
        }
    }
}

// ------------------------------------------------------------------ Settings

impl TaskMgr {
    fn settings(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        page_header(ui, tr("Settings"), |_| {});
        let on = replaced();
        ui.label(RichText::new(tr("Replace Windows Task Manager")).size(16.0));
        ui.label(
            RichText::new(tr("Ctrl+Shift+Esc, Ctrl+Alt+Del and every other way of starting taskmgr.exe open this window instead. Changing it needs administrator rights."))
                .color(pal.dim),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(if on { tr("Status: Delfin replaces Windows Task Manager.") } else { tr("Status: Windows Task Manager is used.") });
        });
        if ui.button(if on { tr("Use Windows Task Manager again") } else { tr("Replace Windows Task Manager") }).clicked() {
            if let Err(e) = replace(!on) {
                self.say(e);
            }
        }
        ui.add_space(16.0);
        if ui.checkbox(&mut self.on_top, tr("Always on top")).changed() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::WindowLevel(if self.on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            }));
        }
        ui.add_space(16.0);
        ui.label(RichText::new(trf("Delfin {}", &[&crate::update::CURRENT])).color(pal.dim));
    }

    fn run_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.run else { return };
        let mut open = true;
        let mut go = false;
        egui::Window::new(tr("Run new task"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(tr("Type the name of a program, folder, document or website."));
                let te = ui.add(egui::TextEdit::singleline(&mut d.text).desired_width(380.0));
                if d.focus {
                    te.request_focus();
                    d.focus = false;
                }
                go |= te.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                ui.checkbox(&mut d.admin, tr("Create this task with administrative privileges"));
                if !d.error.is_empty() {
                    ui.label(RichText::new(&d.error).color(self.pal.danger));
                }
                ui.horizontal(|ui| {
                    go |= ui.button("OK").clicked();
                    if ui.button(tr("Cancel")).clicked() {
                        d.text.clear();
                        d.error = "cancel".into();
                    }
                });
            });
        if go && !d.text.trim().is_empty() {
            let cmd = d.text.trim().to_string();
            let ok = if d.admin {
                sys::run_elevated("cmd.exe", &format!("/c start \"\" {cmd}"))
            } else {
                use std::os::windows::process::CommandExt;
                std::process::Command::new("cmd.exe").raw_arg(format!("/c start \"\" {cmd}")).creation_flags(0x0800_0000).spawn().is_ok()
            };
            if ok {
                self.run = None;
            } else if let Some(d) = &mut self.run {
                d.error = tr("Windows couldn't start it.").to_string();
            }
            return;
        }
        if !open || self.run.as_ref().is_some_and(|d| d.error == "cancel") {
            self.run = None;
        }
    }
}
