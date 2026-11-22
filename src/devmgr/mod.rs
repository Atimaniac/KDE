//! Delfin Device Manager (`delfin.exe --devmgr`): every device by type, with
//! its status, driver and IDs; enable, disable, uninstall and scan for
//! hardware changes. Reads SetupAPI directly, so it opens instantly (Windows'
//! own one is an MMC console that takes its time).

mod sys;

use crate::i18n::{tr, trf};
use crate::icons::{self, Tool};
use crate::theme::{self, Palette};
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Key, Rect, RichText, Sense, Shape, Stroke, TextureHandle};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use sys::{Action, Device};

const TITLES: [&str; 2] = ["Device Manager", "Menedżer urządzeń"];

/// `delfin.exe --devmgr-do enable|disable|uninstall|scan "<instance id>"`:
/// what the window runs as administrator. Exit code 0 = done.
pub fn run_action(args: &[String]) -> i32 {
    let Some(action) = args.first().and_then(|a| Action::parse(a)) else { return 2 };
    let id = args.get(1).map(String::as_str).unwrap_or("");
    if sys::perform(action, id) {
        0
    } else {
        1
    }
}

pub fn run() -> Result<(), eframe::Error> {
    // One window: a second start brings the open one to the front.
    let mutex = sys::wide("Local\\DelfinDeviceManager");
    unsafe {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        let h = windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 0, mutex.as_ptr());
        if !h.is_null() && GetLastError() == ERROR_ALREADY_EXISTS {
            use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, ShowWindow};
            for title in TITLES {
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
            .with_title(tr("Device Manager"))
            .with_app_id("delfin-devmgr")
            .with_icon(theme::app_icon())
            .with_inner_size([1040.0, 680.0])
            .with_min_inner_size([720.0, 440.0]),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "Delfin Device Manager",
        options,
        Box::new(|cc| {
            theme::install_fonts(&cc.egui_ctx);
            let dark = crate::config::Config::load().dark;
            theme::apply(&cc.egui_ctx, dark);
            Ok(Box::new(DevMgr::new(cc.egui_ctx.clone(), theme::palette(dark))))
        }),
    )
}

// ------------------------------------------------------------------ state

struct Shared {
    devices: Option<Vec<Device>>,
    /// Result of the last action, for the status bar.
    result: Option<(bool, String)>,
    busy: bool,
}

struct DevMgr {
    pal: Palette,
    shared: Arc<Mutex<Shared>>,
    /// Ask the worker for a fresh list (true = also unplugged devices).
    refresh: Sender<bool>,
    show_hidden: bool,
    query: String,
    focus_search: bool,
    /// Expanded device types (class GUIDs).
    open: HashSet<u128>,
    /// Types opened because they have a problem (only once each).
    auto_opened: HashSet<u128>,
    sel: Option<String>,
    icons: HashMap<u128, Option<TextureHandle>>,
    armed: Option<(String, Instant)>,
    message: Option<(String, Instant, bool)>,
}

enum Row {
    Class { guid: u128, name: String, count: usize, problems: usize },
    Dev(usize),
}

impl DevMgr {
    fn new(ctx: egui::Context, pal: Palette) -> Self {
        let shared = Arc::new(Mutex::new(Shared { devices: None, result: None, busy: false }));
        let (tx, rx) = channel::<bool>();
        let s = shared.clone();
        std::thread::Builder::new()
            .name("delfin-devices".into())
            .spawn(move || {
                let mut all = false;
                loop {
                    let list = sys::list(all);
                    s.lock().unwrap_or_else(|p| p.into_inner()).devices = Some(list);
                    ctx.request_repaint();
                    // Plugging something in shows up within a few seconds.
                    match rx.recv_timeout(Duration::from_secs(4)) {
                        Ok(a) => all = a,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(_) => break,
                    }
                    while let Ok(a) = rx.try_recv() {
                        all = a;
                    }
                }
            })
            .ok();
        Self {
            pal,
            shared,
            refresh: tx,
            show_hidden: false,
            query: String::new(),
            focus_search: false,
            open: HashSet::new(),
            auto_opened: HashSet::new(),
            sel: None,
            icons: HashMap::new(),
            armed: None,
            message: None,
        }
    }

    fn icon(&mut self, ctx: &egui::Context, guid: u128) -> Option<egui::TextureId> {
        self.icons
            .entry(guid)
            .or_insert_with(|| {
                sys::class_icon(guid).map(|(w, h, px)| {
                    ctx.load_texture(
                        format!("devclass:{guid:x}"),
                        egui::ColorImage::from_rgba_unmultiplied([w, h], &px),
                        egui::TextureOptions::LINEAR,
                    )
                })
            })
            .as_ref()
            .map(|t| t.id())
    }

    fn say(&mut self, msg: impl Into<String>, bad: bool) {
        self.message = Some((msg.into(), Instant::now(), bad));
    }

    /// Run an action, as administrator when needed, without blocking the window.
    fn act(&mut self, action: Action, dev: Option<&Device>) {
        let id = dev.map(|d| d.instance_id.clone()).unwrap_or_default();
        let name = dev.map(|d| d.name.clone()).unwrap_or_default();
        let shared = self.shared.clone();
        let refresh = self.refresh.clone();
        let all = self.show_hidden;
        shared.lock().unwrap_or_else(|p| p.into_inner()).busy = true;
        std::thread::spawn(move || {
            let ok = if sys::is_admin() || action == Action::Scan {
                sys::perform(action, &id)
            } else {
                false
            };
            // Without administrator rights: ask for them and run it again.
            let ok = if ok {
                true
            } else if sys::is_admin() {
                false
            } else {
                let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                match sys::run_elevated_wait(&exe, &format!("--devmgr-do {} \"{}\"", action.arg(), id)) {
                    Some(code) => code == 0,
                    None => {
                        let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
                        s.busy = false;
                        s.result = Some((false, tr("Administrator rights are needed.").to_string()));
                        return;
                    }
                }
            };
            let msg = match (action, ok) {
                (Action::Scan, true) => tr("Scanned for hardware changes.").to_string(),
                (Action::Enable, true) => trf("Enabled: {}", &[&name]),
                (Action::Disable, true) => trf("Disabled: {}", &[&name]),
                (Action::Uninstall, true) => trf("Uninstalled: {}", &[&name]),
                (_, false) => trf("Windows refused the change: {}", &[&name]),
            };
            {
                let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
                s.busy = false;
                s.result = Some((ok, msg));
            }
            let _ = refresh.send(all);
        });
    }

    fn rows(&mut self, devs: &[Device]) -> Vec<Row> {
        let q = self.query.trim().to_lowercase();
        let mut groups: BTreeMap<(String, u128), Vec<usize>> = BTreeMap::new();
        for (i, d) in devs.iter().enumerate() {
            if d.hidden && !self.show_hidden {
                continue;
            }
            let class = class_label(d);
            if !q.is_empty() {
                let hay = format!("{} {} {} {}", d.name, class, d.manufacturer, d.instance_id).to_lowercase();
                if !hay.contains(&q) {
                    continue;
                }
            }
            groups.entry((class.to_lowercase(), d.class_guid)).or_default().push(i);
        }
        let mut rows = Vec::new();
        for ((_, guid), mut list) in groups {
            list.sort_by_key(|&i| devs[i].name.to_lowercase());
            let problems = list.iter().filter(|&&i| devs[i].has_problem()).count();
            // Like Windows: a type with a broken device opens by itself.
            if problems > 0 && self.auto_opened.insert(guid) {
                self.open.insert(guid);
            }
            rows.push(Row::Class { guid, name: class_label(&devs[list[0]]), count: list.len(), problems });
            if self.open.contains(&guid) || !q.is_empty() {
                rows.extend(list.into_iter().map(Row::Dev));
            }
        }
        rows
    }
}

fn class_label(d: &Device) -> String {
    if !d.class_display.is_empty() {
        d.class_display.clone()
    } else if !d.class.is_empty() {
        d.class.clone()
    } else {
        tr("Other devices").to_string()
    }
}

/// Yellow "!" for a problem, a grey "↓" for a disabled device.
fn paint_badge(p: &egui::Painter, icon: Rect, d: &Device) {
    let c = icon.right_bottom() - vec2(3.0, 3.0);
    if d.has_problem() {
        let r = 5.5;
        p.add(Shape::convex_polygon(
            vec![c + vec2(0.0, -r), c + vec2(r, r * 0.8), c + vec2(-r, r * 0.8)],
            Color32::from_rgb(0xf6, 0x74, 0x00),
            Stroke::new(1.0_f32, Color32::from_black_alpha(140)),
        ));
        p.line_segment([c + vec2(0.0, -2.2), c + vec2(0.0, 1.4)], Stroke::new(1.4_f32, Color32::BLACK));
        p.circle_filled(c + vec2(0.0, 2.9), 0.75, Color32::BLACK);
    } else if d.disabled() {
        p.circle_filled(c, 5.0, Color32::from_gray(90));
        p.circle_stroke(c, 5.0, Stroke::new(1.0_f32, Color32::from_gray(220)));
        let s = Stroke::new(1.4_f32, Color32::WHITE);
        p.line_segment([c + vec2(0.0, -2.8), c + vec2(0.0, 2.6)], s);
        p.add(Shape::line(vec![c + vec2(-2.2, 0.6), c + vec2(0.0, 2.8), c + vec2(2.2, 0.6)], s));
    }
}

// ------------------------------------------------------------------ UI

impl eframe::App for DevMgr {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let pal = self.pal;
        let (devs, busy) = {
            let mut s = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((ok, m)) = s.result.take() {
                self.message = Some((m, Instant::now(), !ok));
            }
            (s.devices.clone(), s.busy)
        };
        let devs = devs.unwrap_or_default();
        if busy {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        let selected = self.sel.as_ref().and_then(|id| devs.iter().find(|d| &d.instance_id == id)).cloned();

        // ---- shortcuts
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::F)) {
            self.focus_search = true;
        }
        if ctx.input(|i| i.key_pressed(Key::F5)) {
            self.act(Action::Scan, None);
        }

        // ---- header
        let mut toggled_hidden = false;
        egui::TopBottomPanel::top("dm-top")
            .frame(egui::Frame::none().fill(pal.window).inner_margin(egui::Margin::symmetric(14.0, 10.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
                    ui.painter().rect_filled(r, 6.0, pal.accent);
                    icons::paint_tool(ui.painter(), r, Tool::Chip, Color32::WHITE);
                    ui.label(RichText::new(tr("Device Manager")).size(16.0));
                    ui.add_space(24.0);
                    let te = ui.add(
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text(tr("Search for a device…  (Ctrl+F)"))
                            .desired_width(ui.available_width().min(460.0)),
                    );
                    if self.focus_search {
                        te.request_focus();
                        self.focus_search = false;
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    {
                        if ui.add_enabled(!busy, egui::Button::new(tr("Scan for hardware changes"))).on_hover_text("F5").clicked() {
                            self.act(Action::Scan, None);
                        }
                        if ui.checkbox(&mut self.show_hidden, tr("Show hidden devices")).changed() {
                            toggled_hidden = true;
                        }
                        if ui.button(tr("Collapse all")).clicked() {
                            self.open.clear();
                        }
                        if ui.button(tr("Expand all")).clicked() {
                            self.open.extend(devs.iter().map(|d| d.class_guid));
                        }
                    }
                });
            });
        if toggled_hidden {
            let _ = self.refresh.send(self.show_hidden);
        }

        // ---- status bar
        egui::TopBottomPanel::bottom("dm-status")
            .frame(egui::Frame::none().fill(pal.window).inner_margin(egui::Margin::symmetric(14.0, 5.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let shown: Vec<&Device> = devs.iter().filter(|d| self.show_hidden || !d.hidden).collect();
                    let problems = shown.iter().filter(|d| d.has_problem()).count();
                    let disabled = shown.iter().filter(|d| d.disabled()).count();
                    ui.label(
                        RichText::new(format!(
                            "{}: {}   {}: {}   {}: {}",
                            tr("Devices"),
                            shown.len(),
                            tr("With problems"),
                            problems,
                            tr("Disabled devices"),
                            disabled
                        ))
                        .size(12.0)
                        .color(pal.dim),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if busy {
                            ui.spinner();
                            ui.label(RichText::new(tr("Working…")).size(12.0).color(pal.dim));
                        } else if let Some((m, t, bad)) = &self.message {
                            if t.elapsed() < Duration::from_secs(8) {
                                ui.label(RichText::new(m).size(12.0).color(if *bad { pal.danger } else { pal.dim }));
                            }
                        }
                    });
                });
            });

        // ---- details of the selected device
        egui::SidePanel::right("dm-details")
            .resizable(true)
            .default_width(380.0)
            .width_range(300.0..=560.0)
            .frame(egui::Frame::none().fill(pal.window).inner_margin(egui::Margin::same(16.0)))
            .show(ctx, |ui| match &selected {
                Some(d) => self.details(ui, ctx, d, busy),
                None => {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(tr("Select a device to see its details.")).color(pal.dim));
                    });
                }
            });

        // ---- device tree
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(pal.view).inner_margin(egui::Margin::symmetric(6.0, 8.0)))
            .show(ctx, |ui| {
                if devs.is_empty() {
                    ui.centered_and_justified(|ui| ui.spinner());
                    return;
                }
                let rows = self.rows(&devs);
                // Up / Down move through the devices shown.
                let visible: Vec<usize> = rows.iter().filter_map(|r| if let Row::Dev(i) = r { Some(*i) } else { None }).collect();
                let typing = ctx.memory(|m| m.focused().is_some());
                if !typing && !visible.is_empty() {
                    let (up, down) = ctx.input(|i| (i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown)));
                    if up || down {
                        let cur = self.sel.as_ref().and_then(|id| visible.iter().position(|&i| &devs[i].instance_id == id));
                        let next = match (cur, down) {
                            (None, _) => 0,
                            (Some(c), true) => (c + 1).min(visible.len() - 1),
                            (Some(c), false) => c.saturating_sub(1),
                        };
                        self.sel = Some(devs[visible[next]].instance_id.clone());
                    }
                }
                let mut action: Option<(Action, Device)> = None;
                egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 26.0, rows.len(), |ui, range| {
                    for row in &rows[range] {
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
                        let p = ui.painter().clone();
                        match row {
                            Row::Class { guid, name, count, problems } => {
                                if resp.hovered() {
                                    p.rect_filled(r, 4.0, pal.hover);
                                }
                                let open = self.open.contains(guid) || !self.query.trim().is_empty();
                                let a = pos2(r.left() + 12.0, r.center().y);
                                let tri = if open {
                                    vec![a + vec2(-4.0, -2.0), a + vec2(4.0, -2.0), a + vec2(0.0, 3.0)]
                                } else {
                                    vec![a + vec2(-2.0, -4.0), a + vec2(3.0, 0.0), a + vec2(-2.0, 4.0)]
                                };
                                p.add(Shape::convex_polygon(tri, pal.dim, Stroke::NONE));
                                let ir = Rect::from_center_size(pos2(r.left() + 34.0, r.center().y), vec2(18.0, 18.0));
                                if let Some(t) = self.icon(ctx, *guid) {
                                    p.image(t, ir, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                                } else {
                                    icons::paint_tool(&p, ir, Tool::Chip, pal.text);
                                }
                                let g = p.layout_no_wrap(name.clone(), FontId::proportional(13.5), pal.text);
                                let tw = g.size().x;
                                p.galley(pos2(ir.right() + 8.0, r.center().y - g.size().y / 2.0), g, pal.text);
                                let extra = if *problems > 0 { format!("{count}  ⚠ {problems}") } else { count.to_string() };
                                p.text(
                                    pos2(ir.right() + 16.0 + tw, r.center().y),
                                    Align2::LEFT_CENTER,
                                    extra,
                                    FontId::proportional(11.5),
                                    if *problems > 0 { Color32::from_rgb(0xf6, 0x74, 0x00) } else { pal.dim },
                                );
                                if resp.clicked() || resp.double_clicked() {
                                    if !self.open.remove(guid) {
                                        self.open.insert(*guid);
                                    }
                                }
                            }
                            Row::Dev(i) => {
                                let d = &devs[*i];
                                let is_sel = self.sel.as_deref() == Some(d.instance_id.as_str());
                                if is_sel {
                                    p.rect_filled(r, 4.0, pal.select);
                                } else if resp.hovered() {
                                    p.rect_filled(r, 4.0, pal.hover);
                                }
                                let ir = Rect::from_center_size(pos2(r.left() + 58.0, r.center().y), vec2(18.0, 18.0));
                                let tint = if d.present { Color32::WHITE } else { Color32::from_white_alpha(110) };
                                if let Some(t) = self.icon(ctx, d.class_guid) {
                                    p.image(t, ir, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
                                } else {
                                    icons::paint_tool(&p, ir, Tool::Chip, pal.dim);
                                }
                                paint_badge(&p, ir, d);
                                let col = if d.present && !d.disabled() { pal.text } else { pal.dim };
                                p.with_clip_rect(r).text(pos2(ir.right() + 8.0, r.center().y), Align2::LEFT_CENTER, &d.name, FontId::proportional(13.0), col);
                                if resp.clicked() || resp.secondary_clicked() {
                                    self.sel = Some(d.instance_id.clone());
                                }
                                if resp.double_clicked() {
                                    open_properties(&d.instance_id);
                                }
                                let dev = d.clone();
                                resp.context_menu(|ui| {
                                    if dev.disabled() {
                                        if ui.button(tr("Enable device")).clicked() {
                                            action = Some((Action::Enable, dev.clone()));
                                            ui.close_menu();
                                        }
                                    } else if dev.present && dev.disableable && ui.button(tr("Disable device")).clicked() {
                                        action = Some((Action::Disable, dev.clone()));
                                        ui.close_menu();
                                    }
                                    if ui.button(tr("Uninstall device…")).clicked() {
                                        self.sel = Some(dev.instance_id.clone());
                                        self.armed = Some((dev.instance_id.clone(), Instant::now()));
                                        ui.close_menu();
                                    }
                                    ui.separator();
                                    if ui.button(tr("Scan for hardware changes")).clicked() {
                                        action = Some((Action::Scan, dev.clone()));
                                        ui.close_menu();
                                    }
                                    if ui.button(tr("Properties (Windows)")).clicked() {
                                        open_properties(&dev.instance_id);
                                        ui.close_menu();
                                    }
                                    if ui.button(tr("Copy device instance path")).clicked() {
                                        ui.ctx().copy_text(dev.instance_id.clone());
                                        ui.close_menu();
                                    }
                                });
                            }
                        }
                    }
                });
                if let Some((a, d)) = action {
                    self.act(a, if a == Action::Scan { None } else { Some(&d) });
                }
            });
    }
}

/// Windows' own property sheet for one device (Driver tab: update, roll back…),
/// without the slow MMC console.
fn open_properties(id: &str) {
    let _ = std::process::Command::new("rundll32.exe")
        .args(["devmgr.dll,DeviceProperties_RunDLL", "/DeviceID", id])
        .spawn();
}

impl DevMgr {
    fn details(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, d: &Device, busy: bool) {
        let pal = self.pal;
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
            if let Some(t) = self.icon(ctx, d.class_guid) {
                ui.painter().image(t, r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            paint_badge(ui.painter(), r.shrink(2.0), d);
            ui.vertical(|ui| {
                ui.add(egui::Label::new(RichText::new(&d.name).size(16.0)).wrap());
                ui.label(RichText::new(class_label(d)).color(pal.dim));
            });
        });
        ui.add_space(10.0);

        // Status, coloured like the badge.
        let (status, col) = if !d.present {
            (sys::problem_text(45), pal.dim)
        } else if d.disabled() {
            (sys::problem_text(22), pal.dim)
        } else if d.problem != 0 {
            (sys::problem_text(d.problem), Color32::from_rgb(0xf6, 0x74, 0x00))
        } else {
            (sys::problem_text(0), Color32::from_rgb(0x27, 0xae, 0x60))
        };
        egui::Frame::none()
            .stroke(Stroke::new(1.0_f32, col))
            .rounding(6.0)
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(egui::Label::new(RichText::new(status).color(if col == pal.dim { pal.text } else { col })).wrap());
            });
        ui.add_space(10.0);

        // Actions.
        let armed = self.armed.as_ref().is_some_and(|(id, t)| id == &d.instance_id && t.elapsed() < Duration::from_secs(5));
        ui.horizontal_wrapped(|ui| {
            if d.disabled() {
                if ui.add_enabled(!busy, egui::Button::new(tr("Enable device"))).clicked() {
                    self.act(Action::Enable, Some(d));
                }
            } else if d.present && d.disableable && ui.add_enabled(!busy, egui::Button::new(tr("Disable device"))).clicked() {
                self.act(Action::Disable, Some(d));
            }
            let label = if armed { RichText::new(tr("Sure? Click again")).color(pal.danger) } else { RichText::new(tr("Uninstall device")) };
            if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                if armed {
                    self.armed = None;
                    self.act(Action::Uninstall, Some(d));
                } else {
                    self.armed = Some((d.instance_id.clone(), Instant::now()));
                }
            }
            if ui.button(tr("Properties (Windows)")).on_hover_text(tr("Driver details, update and roll back")).clicked() {
                open_properties(&d.instance_id);
            }
            if ui.button(tr("Driver updates")).on_hover_text(tr("Windows Update › Optional updates")).clicked() {
                let _ = std::process::Command::new("cmd.exe").args(["/c", "start", "", "ms-settings:windowsupdate-optionalupdates"]).spawn();
            }
        });
        if armed {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        ui.add_space(12.0);

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let none = || "—".to_string();
            let or = |s: &str| if s.is_empty() { none() } else { s.to_string() };
            let mut fields: Vec<(&str, String)> = vec![
                (tr("Manufacturer"), or(&d.manufacturer)),
                (tr("Location"), or(&d.location)),
                (tr("Driver provider"), or(&d.driver_provider)),
                (tr("Driver version"), or(&d.driver_version)),
                (tr("Driver date"), or(&d.driver_date)),
                (tr("Service"), or(&d.service)),
                (tr("Device instance path"), d.instance_id.clone()),
                (tr("Hardware IDs"), if d.hardware_ids.is_empty() { none() } else { d.hardware_ids.join("\n") }),
            ];
            if d.class_guid != 0 {
                fields.push((tr("Class GUID"), sys::guid_string(d.class_guid)));
            }
            for (k, v) in fields {
                ui.label(RichText::new(k).size(11.5).color(pal.dim));
                let resp = ui.add(egui::Label::new(RichText::new(&v).size(12.5)).wrap().sense(Sense::click()));
                let resp = resp.on_hover_text(tr("Click to copy"));
                if resp.clicked() {
                    ctx.copy_text(v.clone());
                    self.say(tr("Copied."), false);
                }
                ui.add_space(6.0);
            }
        });
    }
}
