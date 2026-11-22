//! KDE Breeze (Plasma 6) colours for egui.

use eframe::egui::{self, Color32, FontId, Rounding, Stroke, TextStyle};
use std::path::PathBuf;

#[derive(Clone, Copy)]
pub struct Palette {
    pub window: Color32,
    pub view: Color32,
    pub header: Color32,
    pub alt_row: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub accent: Color32,
    pub border: Color32,
    pub hover: Color32,
    pub select: Color32,
    pub paper: Color32,
    pub danger: Color32,
}

pub const ACCENT: Color32 = Color32::from_rgb(0x3d, 0xae, 0xe9);

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        // Breeze Dark
        Palette {
            window: hex(0x202326),
            view: hex(0x141618),
            header: hex(0x2a2e32),
            alt_row: hex(0x1a1c1e),
            text: hex(0xfcfcfc),
            dim: hex(0xa1a9b1),
            accent: ACCENT,
            border: hex(0x3a3f44),
            hover: Color32::from_rgba_unmultiplied(0x3d, 0xae, 0xe9, 38),
            select: Color32::from_rgba_unmultiplied(0x3d, 0xae, 0xe9, 90),
            paper: hex(0xeff0f1),
            danger: hex(0xda4453),
        }
    } else {
        // Breeze Light
        Palette {
            window: hex(0xeff0f1),
            view: hex(0xffffff),
            header: hex(0xdee0e2),
            alt_row: hex(0xf7f7f7),
            text: hex(0x232629),
            dim: hex(0x707d8a),
            accent: ACCENT,
            border: hex(0xc4c9cd),
            hover: Color32::from_rgba_unmultiplied(0x3d, 0xae, 0xe9, 30),
            select: Color32::from_rgba_unmultiplied(0x3d, 0xae, 0xe9, 70),
            paper: hex(0xfcfcfc),
            danger: hex(0xda4453),
        }
    }
}

pub fn apply(ctx: &egui::Context, dark: bool) {
    let p = palette(dark);
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    v.override_text_color = Some(p.text);
    v.panel_fill = p.window;
    v.window_fill = p.window;
    v.extreme_bg_color = p.view;
    v.faint_bg_color = p.alt_row;
    v.code_bg_color = p.header;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.select;
    v.selection.stroke = Stroke::new(1.0_f32, p.accent);
    v.window_stroke = Stroke::new(1.0_f32, p.border);
    v.window_rounding = Rounding::same(6.0);
    v.menu_rounding = Rounding::same(6.0);

    let button = if dark { hex(0x2f3338) } else { hex(0xfcfcfc) };
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.window;
    w.noninteractive.weak_bg_fill = p.window;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.inactive.bg_fill = button;
    w.inactive.weak_bg_fill = button;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, p.border);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.hovered.bg_fill = p.hover;
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, p.accent);
    w.hovered.fg_stroke = Stroke::new(1.5_f32, p.text);
    w.active.bg_fill = p.select;
    w.active.weak_bg_fill = p.select;
    w.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
    w.active.fg_stroke = Stroke::new(1.5_f32, p.text);
    w.open = w.active;
    for wv in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        wv.rounding = Rounding::same(4.0);
    }

    let mut style = (*ctx.style()).clone();
    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    style.spacing.menu_margin = egui::Margin::same(6.0);
    style.spacing.interact_size.y = 22.0;
    style
        .text_styles
        .insert(TextStyle::Body, FontId::proportional(13.5));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::proportional(13.5));
    style
        .text_styles
        .insert(TextStyle::Small, FontId::proportional(11.0));

    // Same style for both egui themes so the OS light/dark switch doesn't override ours.
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
}

/// Use Noto Sans (the KDE font) if installed, otherwise Segoe UI on Windows.
pub fn install_fonts(ctx: &egui::Context) {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            candidates
                .push(PathBuf::from(local).join(r"Microsoft\Windows\Fonts\NotoSans-Regular.ttf"));
        }
        candidates.push(PathBuf::from(r"C:\Windows\Fonts\NotoSans-Regular.ttf"));
        candidates.push(PathBuf::from(r"C:\Windows\Fonts\segoeui.ttf"));
    } else {
        candidates.push(PathBuf::from("/usr/share/fonts/noto/NotoSans-Regular.ttf"));
        candidates.push(PathBuf::from(
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        ));
    }

    for path in candidates {
        if let Ok(bytes) = std::fs::read(&path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("ui".to_owned(), egui::FontData::from_owned(bytes));
            if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                family.insert(0, "ui".to_owned());
            }
            ctx.set_fonts(fonts);
            return;
        }
    }
}

/// The Delfin logo as a window icon (title bar, Alt+Tab, taskbar).
pub fn app_icon() -> std::sync::Arc<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../assets/delfin.png"))
        .map(|i| i.to_rgba8())
        .unwrap_or_else(|_| image::RgbaImage::new(1, 1));
    std::sync::Arc::new(egui::IconData {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}

/// Colours for Aero glass: the dark palette with see-through surfaces tinted
/// in `color`. `strength` (0–100) is how much colour and how little see-through.
pub fn glass_palette(color: [u8; 3], strength: u8) -> Palette {
    let mut p = palette(true);
    let s = strength.min(100) as f32 / 100.0;
    let shade = |k: f32, a: f32| {
        Color32::from_rgba_unmultiplied(
            (color[0] as f32 * k) as u8,
            (color[1] as f32 * k) as u8,
            (color[2] as f32 * k) as u8,
            a.clamp(0.0, 255.0) as u8,
        )
    };
    p.window = shade(0.30 + 0.25 * s, 80.0 + 130.0 * s);
    p.header = shade(0.42 + 0.25 * s, 110.0 + 110.0 * s);
    p.view = Color32::from_rgba_unmultiplied(8, 10, 14, 120);
    p.alt_row = Color32::from_rgba_unmultiplied(255, 255, 255, 10);
    p.border = Color32::from_rgba_unmultiplied(255, 255, 255, 70);
    p.hover = Color32::from_rgba_unmultiplied(255, 255, 255, 34);
    p.select = shade(0.9, 120.0);
    p.dim = hex(0xd2dae2);
    p
}

/// The shell's look changed (dark mode, Aero): tell a running shell.
#[cfg(windows)]
pub fn notify_shell() {
    use windows_sys::Win32::System::Threading::{OpenEventW, SetEvent};
    let name: Vec<u16> = SHELL_EVENT.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let ev = OpenEventW(0x0002 /* EVENT_MODIFY_STATE */, 0, name.as_ptr());
        if !ev.is_null() {
            SetEvent(ev);
            windows_sys::Win32::Foundation::CloseHandle(ev);
        }
    }
}

/// Signalled when the shell should read its look from the settings again.
#[cfg(windows)]
pub const SHELL_EVENT: &str = r"Local\DelfinShellLook";
