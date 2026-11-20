//! Display settings (`delfin.exe --display`): resolution, refresh rate,
//! orientation and wallpaper. Windows' own Settings app can't start without
//! Explorer, so the shell opens this instead.
//!
//! A new mode is applied right away and reverted after 15 seconds unless you
//! keep it, like Windows does.

use crate::i18n::{tr, trf};
use crate::theme::{self, Palette};
use eframe::egui::{self, vec2, RichText};
use std::mem::{size_of, zeroed};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};
use windows_sys::Win32::Graphics::Gdi::{
    ChangeDisplaySettingsExW, EnumDisplayDevicesW, EnumDisplaySettingsW, DEVMODEW, DISPLAY_DEVICEW,
};

const DM_BITSPERPEL: u32 = 0x0004_0000;
const DM_PELSWIDTH: u32 = 0x0008_0000;
const DM_PELSHEIGHT: u32 = 0x0010_0000;
const DM_DISPLAYFREQUENCY: u32 = 0x0040_0000;
const DM_DISPLAYORIENTATION: u32 = 0x0000_0080;
const ENUM_CURRENT_SETTINGS: u32 = 0xFFFF_FFFF;
const CDS_UPDATEREGISTRY: u32 = 0x1;
const DISPLAY_DEVICE_ATTACHED_TO_DESKTOP: u32 = 0x1;
const DISPLAY_DEVICE_PRIMARY_DEVICE: u32 = 0x4;
const KEEP_SECS: u64 = 15;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn from_wide(b: &[u16]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..n])
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Mode {
    w: u32,
    h: u32,
    hz: u32,
    /// 0 = landscape, 1 = portrait, 2 = landscape (flipped), 3 = portrait (flipped)
    orient: u32,
}

#[derive(Clone)]
struct Monitor {
    device: String, // \\.\DISPLAY1
    name: String,   // adapter / monitor description
    primary: bool,
    modes: Vec<Mode>,
    current: Mode,
}

unsafe fn devmode(device: &[u16], i: u32) -> Option<DEVMODEW> {
    let mut dm: DEVMODEW = zeroed();
    dm.dmSize = size_of::<DEVMODEW>() as u16;
    (EnumDisplaySettingsW(device.as_ptr(), i, &mut dm) != 0).then_some(dm)
}

fn mode_of(dm: &DEVMODEW) -> Mode {
    Mode {
        w: dm.dmPelsWidth,
        h: dm.dmPelsHeight,
        hz: dm.dmDisplayFrequency,
        orient: unsafe { dm.Anonymous1.Anonymous2.dmDisplayOrientation } as u32,
    }
}

fn monitors() -> Vec<Monitor> {
    let mut out = Vec::new();
    unsafe {
        for i in 0..16 {
            let mut dd: DISPLAY_DEVICEW = zeroed();
            dd.cb = size_of::<DISPLAY_DEVICEW>() as u32;
            if EnumDisplayDevicesW(null(), i, &mut dd, 0) == 0 {
                break;
            }
            if dd.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0 {
                continue;
            }
            let device = dd.DeviceName;
            // The monitor's own name, if Windows knows it.
            let mut md: DISPLAY_DEVICEW = zeroed();
            md.cb = size_of::<DISPLAY_DEVICEW>() as u32;
            let monitor_name = if EnumDisplayDevicesW(device.as_ptr(), 0, &mut md, 0) != 0 {
                from_wide(&md.DeviceString)
            } else {
                String::new()
            };
            let Some(cur) = devmode(&device, ENUM_CURRENT_SETTINGS) else { continue };
            let current = mode_of(&cur);
            let mut modes: Vec<Mode> = Vec::new();
            let mut k = 0;
            while let Some(dm) = devmode(&device, k) {
                k += 1;
                if dm.dmBitsPerPel < 32 {
                    continue;
                }
                let mut m = mode_of(&dm);
                m.orient = 0;
                if !modes.contains(&m) {
                    modes.push(m);
                }
            }
            modes.sort_by(|a, b| (b.w * b.h, b.w, b.hz).cmp(&(a.w * a.h, a.w, a.hz)));
            let adapter = from_wide(&dd.DeviceString);
            out.push(Monitor {
                device: from_wide(&device),
                name: if monitor_name.is_empty() { adapter } else { format!("{monitor_name} — {adapter}") },
                primary: dd.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0,
                modes,
                current,
            });
        }
    }
    out
}

/// Apply a mode; returns the Windows result (0 = DISP_CHANGE_SUCCESSFUL).
fn apply(device: &str, m: Mode) -> i32 {
    let dev = wide(device);
    unsafe {
        let Some(mut dm) = devmode(&dev, ENUM_CURRENT_SETTINGS) else { return -1 };
        // Portrait swaps width and height.
        let (w, h) = if m.orient % 2 == 1 && m.w > m.h { (m.h, m.w) } else { (m.w, m.h) };
        dm.dmPelsWidth = w;
        dm.dmPelsHeight = h;
        dm.dmDisplayFrequency = m.hz;
        dm.dmBitsPerPel = 32;
        dm.Anonymous1.Anonymous2.dmDisplayOrientation = m.orient as _;
        dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY | DM_BITSPERPEL | DM_DISPLAYORIENTATION;
        ChangeDisplaySettingsExW(dev.as_ptr(), &dm, null_mut(), CDS_UPDATEREGISTRY, null())
    }
}

fn wallpaper() -> Option<PathBuf> {
    use windows_sys::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;
    let mut buf = [0u16; 1024];
    let ok = unsafe { SystemParametersInfoW(0x0073 /* SPI_GETDESKWALLPAPER */, buf.len() as u32, buf.as_mut_ptr() as *mut _, 0) };
    let s = from_wide(&buf);
    (ok != 0 && !s.is_empty()).then(|| PathBuf::from(s))
}

fn set_wallpaper(p: &std::path::Path) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;
    let w = wide(&p.to_string_lossy());
    unsafe { SystemParametersInfoW(0x0014 /* SPI_SETDESKWALLPAPER */, 0, w.as_ptr() as *mut _, 0x1 | 0x2) != 0 }
}

/// The standard Windows "Open" dialog, filtered to pictures.
fn pick_image(owner: isize) -> Option<PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OPENFILENAMEW};
    let filter: Vec<u16> = format!("{}\0*.jpg;*.jpeg;*.png;*.bmp\0\0", tr("Pictures")).encode_utf16().collect();
    let start = dirs::picture_dir().map(|p| wide(&p.to_string_lossy())).unwrap_or_else(|| vec![0]);
    let title = wide(tr("Choose a wallpaper"));
    let mut file = vec![0u16; 1024];
    unsafe {
        let mut ofn: OPENFILENAMEW = zeroed();
        ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = owner as _;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrFile = file.as_mut_ptr();
        ofn.nMaxFile = file.len() as u32;
        ofn.lpstrInitialDir = start.as_ptr();
        ofn.lpstrTitle = title.as_ptr();
        ofn.Flags = 0x1000 | 0x0800 | 0x0008; // FILEMUSTEXIST | PATHMUSTEXIST | NOCHANGEDIR
        (GetOpenFileNameW(&mut ofn) != 0).then(|| PathBuf::from(from_wide(&file)))
    }
}

struct DisplayApp {
    pal: Palette,
    monitors: Vec<Monitor>,
    sel: usize,
    /// What the user picked in the lists (applied with "Apply").
    want: Option<Mode>,
    /// Waiting for "Keep changes": (device, previous mode, since).
    pending: Option<(String, Mode, Instant)>,
    status: String,
    wallpaper: Option<PathBuf>,
}

pub fn run() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(tr("Display Settings"))
            .with_icon(theme::app_icon())
            .with_inner_size([560.0, 520.0])
            .with_resizable(false)
            .with_maximize_button(false),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "Delfin Display",
        options,
        Box::new(|cc| {
            theme::install_fonts(&cc.egui_ctx);
            let dark = crate::config::Config::load().dark;
            theme::apply(&cc.egui_ctx, dark);
            let monitors = monitors();
            let sel = monitors.iter().position(|m| m.primary).unwrap_or(0);
            Ok(Box::new(DisplayApp {
                pal: theme::palette(dark),
                monitors,
                sel,
                want: None,
                pending: None,
                status: String::new(),
                wallpaper: wallpaper(),
            }))
        }),
    )
}

fn orient_label(o: u32) -> &'static str {
    match o {
        1 => tr("Portrait"),
        2 => tr("Landscape (flipped)"),
        3 => tr("Portrait (flipped)"),
        _ => tr("Landscape"),
    }
}

impl eframe::App for DisplayApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let pal = self.pal;
        // Revert if the new mode wasn't confirmed in time.
        if let Some((dev, old, since)) = &self.pending {
            if since.elapsed() >= Duration::from_secs(KEEP_SECS) {
                apply(dev, *old);
                self.status = tr("The previous settings were restored.").to_string();
                self.pending = None;
                self.monitors = monitors();
            } else {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(pal.window).inner_margin(egui::Margin::same(20.0)))
            .show(ctx, |ui| {
                if self.monitors.is_empty() {
                    ui.label(tr("No display found."));
                    return;
                }
                ui.label(RichText::new(tr("Display")).size(18.0).strong());
                ui.add_space(8.0);

                // Monitor picker (only if there is more than one).
                if self.monitors.len() > 1 {
                    for (i, m) in self.monitors.iter().enumerate() {
                        let label = format!("{}{}", m.name, if m.primary { tr(" (main)") } else { "" });
                        if ui.radio(self.sel == i, label).clicked() && self.sel != i {
                            self.sel = i;
                            self.want = None;
                        }
                    }
                    ui.add_space(8.0);
                } else {
                    ui.label(RichText::new(&self.monitors[0].name).color(pal.dim));
                    ui.add_space(8.0);
                }

                let mon = self.monitors[self.sel].clone();
                let cur = mon.current;
                // Sizes are listed landscape; a portrait mode reports them swapped.
                let (lw, lh) = if cur.orient % 2 == 1 { (cur.h, cur.w) } else { (cur.w, cur.h) };
                let mut want = self.want.unwrap_or(Mode { w: lw, h: lh, ..cur });
                let busy = self.pending.is_some();

                egui::Grid::new("display-grid").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                    // Resolution
                    ui.label(tr("Resolution"));
                    let mut sizes: Vec<(u32, u32)> = mon.modes.iter().map(|m| (m.w, m.h)).collect();
                    sizes.dedup();
                    egui::ComboBox::from_id_salt("res")
                        .width(220.0)
                        .selected_text(format!("{} × {}", want.w, want.h))
                        .show_ui(ui, |ui| {
                            for (w, h) in sizes {
                                if ui.selectable_label((w, h) == (want.w, want.h), format!("{w} × {h}")).clicked() {
                                    want.w = w;
                                    want.h = h;
                                    // Keep the refresh rate if this size has it, else the highest.
                                    let rates: Vec<u32> = mon.modes.iter().filter(|m| m.w == w && m.h == h).map(|m| m.hz).collect();
                                    if !rates.contains(&want.hz) {
                                        want.hz = rates.iter().copied().max().unwrap_or(60);
                                    }
                                }
                            }
                        });
                    ui.end_row();

                    // Refresh rate
                    ui.label(tr("Refresh rate"));
                    let mut rates: Vec<u32> = mon.modes.iter().filter(|m| m.w == want.w && m.h == want.h).map(|m| m.hz).collect();
                    rates.sort_unstable_by(|a, b| b.cmp(a));
                    rates.dedup();
                    egui::ComboBox::from_id_salt("hz")
                        .width(220.0)
                        .selected_text(format!("{} Hz", want.hz))
                        .show_ui(ui, |ui| {
                            for hz in rates {
                                ui.selectable_value(&mut want.hz, hz, format!("{hz} Hz"));
                            }
                        });
                    ui.end_row();

                    // Orientation
                    ui.label(tr("Orientation"));
                    egui::ComboBox::from_id_salt("orient")
                        .width(220.0)
                        .selected_text(orient_label(want.orient))
                        .show_ui(ui, |ui| {
                            for o in 0..4 {
                                ui.selectable_value(&mut want.orient, o, orient_label(o));
                            }
                        });
                    ui.end_row();
                });
                self.want = Some(want);

                ui.add_space(12.0);
                if let Some((_, _, since)) = &self.pending {
                    let left = KEEP_SECS.saturating_sub(since.elapsed().as_secs());
                    ui.label(RichText::new(trf("Keep these display settings? Reverting in {} s.", &[&left])).color(pal.accent));
                    ui.horizontal(|ui| {
                        if ui.button(tr("Keep changes")).clicked() {
                            self.pending = None;
                            self.status = tr("Display settings saved.").to_string();
                            self.monitors = monitors();
                        }
                        if ui.button(tr("Revert")).clicked() {
                            if let Some((dev, old, _)) = self.pending.take() {
                                apply(&dev, old);
                            }
                            self.status = tr("The previous settings were restored.").to_string();
                            self.monitors = monitors();
                        }
                    });
                } else {
                    let unchanged = (want.w, want.h, want.hz, want.orient) == (lw, lh, cur.hz, cur.orient);
                    if ui.add_enabled(!busy && !unchanged, egui::Button::new(tr("Apply")).min_size(vec2(110.0, 30.0))).clicked() {
                        let r = apply(&mon.device, want);
                        if r == 0 {
                            self.pending = Some((mon.device.clone(), cur, Instant::now()));
                            self.status.clear();
                        } else {
                            self.status = trf("Windows refused this mode (code {}).", &[&r]);
                        }
                        self.monitors = monitors();
                    }
                }
                if !self.status.is_empty() {
                    ui.label(RichText::new(&self.status).color(pal.dim));
                }

                ui.add_space(14.0);
                ui.separator();
                ui.add_space(6.0);

                // Wallpaper
                ui.label(RichText::new(tr("Wallpaper")).size(16.0).strong());
                let name = self
                    .wallpaper
                    .as_ref()
                    .map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
                    .unwrap_or_else(|| tr("(none)").to_string());
                ui.label(RichText::new(name).color(pal.dim));
                ui.horizontal(|ui| {
                    if ui.button(tr("Choose image…")).clicked() {
                        let owner = {
                            let t = wide(tr("Display Settings"));
                            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(null(), t.as_ptr()) as isize }
                        };
                        if let Some(p) = pick_image(owner) {
                            if set_wallpaper(&p) {
                                self.wallpaper = Some(p);
                            }
                        }
                    }
                });

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    if ui.small_button(tr("Advanced display adapter settings")).clicked() {
                        let _ = std::process::Command::new("rundll32.exe")
                            .arg("display.dll,ShowAdapterSettings")
                            .spawn();
                    }
                });
            });
    }
}
