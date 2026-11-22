//! Delfin's Properties window for a program, shortcut or any file
//! (`delfin.exe --props <path>`): General, Shortcut, Compatibility and
//! Details, like Windows' property sheet, in Delfin's own look.

mod sys;

use crate::i18n::{tr, trf};
use crate::theme::{self, Palette};
use eframe::egui::{self, pos2, vec2, Color32, Rect, RichText, Sense, Stroke, TextureHandle};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use sys::Shortcut;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Shortcut,
    Compat,
    Details,
}

/// Windows versions for "Run this program in compatibility mode for".
const MODES: [(&str, &str); 5] = [
    ("WINXPSP3", "Windows XP (Service Pack 3)"),
    ("VISTASP2", "Windows Vista (Service Pack 2)"),
    ("WIN7RTM", "Windows 7"),
    ("WIN8RTM", "Windows 8"),
    ("WIN81RTM", "Windows 8.1"),
];
const ADMIN: &str = "RUNASADMIN";
const NO_FSO: &str = "DISABLEDXMAXIMIZEDWINDOWEDMODE";
const COLOR16: &str = "16BITCOLOR";
const RES640: &str = "640X480";
/// High DPI scaling override: none, Application, System, System (Enhanced).
const DPI: [&[&str]; 4] = [&[], &["HIGHDPIAWARE"], &["DPIUNAWARE"], &["GDIDPISCALING", "DPIUNAWARE"]];

#[derive(Clone, PartialEq, Default)]
struct Compat {
    mode: Option<usize>,
    admin: bool,
    no_fso: bool,
    color16: bool,
    res640: bool,
    dpi: usize,
    /// Flags Delfin doesn't show, kept as they are.
    other: Vec<String>,
}

impl Compat {
    fn parse(flags: &[String]) -> Self {
        let has = |f: &str| flags.iter().any(|x| x == f);
        let dpi = if has("GDIDPISCALING") && has("DPIUNAWARE") {
            3
        } else if has("DPIUNAWARE") {
            2
        } else if has("HIGHDPIAWARE") {
            1
        } else {
            0
        };
        let known = |f: &str| {
            MODES.iter().any(|(m, _)| *m == f) || [ADMIN, NO_FSO, COLOR16, RES640, "HIGHDPIAWARE", "DPIUNAWARE", "GDIDPISCALING"].contains(&f)
        };
        Compat {
            mode: MODES.iter().position(|(m, _)| has(m)),
            admin: has(ADMIN),
            no_fso: has(NO_FSO),
            color16: has(COLOR16),
            res640: has(RES640),
            dpi,
            other: flags.iter().filter(|f| !known(f)).cloned().collect(),
        }
    }

    fn flags(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        if let Some(m) = self.mode {
            v.push(MODES[m].0.into());
        }
        for (on, f) in [(self.admin, ADMIN), (self.no_fso, NO_FSO), (self.color16, COLOR16), (self.res640, RES640)] {
            if on {
                v.push(f.into());
            }
        }
        v.extend(DPI[self.dpi].iter().map(|s| s.to_string()));
        v.extend(self.other.iter().cloned());
        v
    }
}

struct Props {
    pal: Palette,
    path: PathBuf,
    /// Name as edited (without ".lnk" / ".url" for shortcuts, like Windows).
    name: String,
    orig_name: String,
    hidden_ext: Option<String>,
    type_name: String,
    icon: Option<TextureHandle>,
    meta: Option<std::fs::Metadata>,
    attrs: u32,
    readonly: bool,
    hidden: bool,
    /// The program (the file itself, or what a shortcut points to).
    program: Option<PathBuf>,
    arch: Option<&'static str>,
    version: Vec<(&'static str, String)>,
    link: Option<(Shortcut, Shortcut)>,
    compat: Option<(Compat, Compat)>,
    tab: Tab,
    message: Option<(String, bool)>,
}

pub fn run(path: PathBuf) -> Result<(), eframe::Error> {
    let title = trf("Properties: {}", &[&display_name(&path)]);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_app_id("delfin-props")
            .with_icon(theme::app_icon())
            .with_inner_size([500.0, 620.0])
            .with_min_inner_size([420.0, 480.0]),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "Delfin Properties",
        options,
        Box::new(move |cc| {
            theme::install_fonts(&cc.egui_ctx);
            let dark = crate::config::Config::load().dark;
            theme::apply(&cc.egui_ctx, dark);
            Ok(Box::new(Props::load(&cc.egui_ctx, path, theme::palette(dark))))
        }),
    )
}

fn display_name(p: &Path) -> String {
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext == "lnk" || ext == "url" {
        p.file_stem()
    } else {
        p.file_name()
    }
    .map(|s| s.to_string_lossy().into_owned())
    .unwrap_or_else(|| p.display().to_string())
}

fn is_program(p: &Path) -> bool {
    p.extension().is_some_and(|e| ["exe", "com", "scr"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

impl Props {
    fn load(ctx: &egui::Context, path: PathBuf, pal: Palette) -> Self {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let hidden_ext = (ext == "lnk" || ext == "url").then(|| format!(".{ext}"));
        let link = (ext == "lnk").then(|| sys::read_shortcut(&path)).flatten().map(|s| (s.clone(), s));
        let program = match &link {
            Some((s, _)) if !s.target.is_empty() => Some(sys::expand(&s.target)),
            _ if is_program(&path) => Some(path.clone()),
            _ => None,
        };
        let compat = program.as_ref().filter(|p| is_program(p)).map(|p| {
            let c = Compat::parse(&sys::compat_flags(p));
            (c.clone(), c)
        });
        let icon = crate::fileicons::icon_pixels(&path, 2).map(|(w, h, px)| {
            ctx.load_texture("props-icon", egui::ColorImage::from_rgba_unmultiplied([w, h], &px), egui::TextureOptions::LINEAR)
        });
        let attrs = sys::attributes(&path);
        let name = display_name(&path);
        Props {
            pal,
            name: name.clone(),
            orig_name: name,
            hidden_ext,
            type_name: sys::type_name(&path),
            icon,
            meta: std::fs::metadata(&path).ok(),
            attrs,
            readonly: attrs & sys::READONLY != 0,
            hidden: attrs & sys::HIDDEN != 0,
            arch: program.as_deref().and_then(sys::architecture),
            version: program.as_deref().map(sys::version_info).unwrap_or_default(),
            program,
            link,
            compat,
            tab: Tab::General,
            message: None,
            path,
        }
    }

    fn dirty(&self) -> bool {
        self.name.trim() != self.orig_name
            || self.readonly != (self.attrs & sys::READONLY != 0)
            || self.hidden != (self.attrs & sys::HIDDEN != 0)
            || self.link.as_ref().is_some_and(|(a, b)| a != b)
            || self.compat.as_ref().is_some_and(|(a, b)| a != b)
    }

    /// Save everything that changed. Returns false (with a message) on failure.
    fn apply(&mut self) -> bool {
        let mut errors: Vec<String> = Vec::new();
        if let Some((orig, edit)) = &mut self.link {
            if orig != edit {
                if sys::write_shortcut(&self.path, edit) {
                    *orig = edit.clone();
                    if !edit.target.is_empty() {
                        self.program = Some(sys::expand(&edit.target));
                    }
                } else {
                    errors.push(tr("The shortcut could not be saved.").to_string());
                }
            }
        }
        if let (Some((orig, edit)), Some(exe)) = (&mut self.compat, &self.program) {
            if orig != edit {
                if sys::set_compat_flags(exe, &edit.flags()) {
                    *orig = edit.clone();
                } else {
                    errors.push(tr("The compatibility settings could not be saved.").to_string());
                }
            }
        }
        let want = (self.attrs & !(sys::READONLY | sys::HIDDEN))
            | if self.readonly { sys::READONLY } else { 0 }
            | if self.hidden { sys::HIDDEN } else { 0 };
        if want != self.attrs {
            if sys::set_attributes(&self.path, if want == 0 { 0x80 } else { want }) {
                self.attrs = want;
            } else {
                errors.push(tr("The attributes could not be changed.").to_string());
            }
        }
        let new_name = self.name.trim().to_string();
        if new_name != self.orig_name && !new_name.is_empty() {
            let file = format!("{new_name}{}", self.hidden_ext.as_deref().unwrap_or(""));
            let to = self.path.with_file_name(file);
            match std::fs::rename(&self.path, &to) {
                Ok(()) => {
                    self.path = to;
                    self.orig_name = new_name;
                }
                Err(e) => errors.push(trf("Could not rename: {}", &[&e])),
            }
        }
        if errors.is_empty() {
            self.message = Some((tr("Saved.").to_string(), false));
            true
        } else {
            self.message = Some((errors.join(" "), true));
            false
        }
    }
}

fn when(t: std::io::Result<SystemTime>) -> String {
    t.map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%d.%m.%Y, %H:%M:%S").to_string())
        .unwrap_or_else(|_| "—".into())
}

fn size_text(n: u64) -> String {
    let human = if n < 1024 {
        format!("{n} B")
    } else {
        let mut v = n as f64;
        let mut unit = "B";
        for u in ["KB", "MB", "GB", "TB"] {
            if v < 1024.0 {
                break;
            }
            v /= 1024.0;
            unit = u;
        }
        format!("{v:.1} {unit}")
    };
    // 1 234 567 bytes
    let digits = n.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push('\u{202f}');
        }
        grouped.push(c);
    }
    trf("{} ({} bytes)", &[&human, &grouped])
}

fn open_folder(dir: &Path) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg(dir).spawn();
    }
}

/// One "label: value" line.
fn row(ui: &mut egui::Ui, pal: &Palette, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(140.0, 20.0), Sense::hover());
        ui.painter().text(pos2(r.left(), r.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(13.0), pal.dim);
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
    });
    ui.add_space(6.0);
}

fn value(ui: &mut egui::Ui, s: &str) {
    ui.add(egui::Label::new(RichText::new(s).size(13.0)).wrap().selectable(true));
}

fn section(ui: &mut egui::Ui, pal: &Palette, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.label(RichText::new(title).size(13.5).strong());
    ui.add_space(4.0);
    egui::Frame::none()
        .fill(pal.view)
        .stroke(Stroke::new(1.0_f32, pal.border))
        .rounding(8.0)
        .inner_margin(egui::Margin::same(12.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
    ui.add_space(12.0);
}

impl eframe::App for Props {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let pal = self.pal;
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));

        // ---- buttons
        egui::TopBottomPanel::bottom("props-buttons")
            .frame(egui::Frame::none().fill(pal.window).inner_margin(egui::Margin::symmetric(16.0, 12.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if let Some((m, bad)) = &self.message {
                        ui.label(RichText::new(m).size(12.0).color(if *bad { pal.danger } else { pal.dim }));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let dirty = self.dirty();
                        if ui.add_enabled(dirty, egui::Button::new(tr("Apply")).min_size(vec2(84.0, 28.0))).clicked() {
                            self.apply();
                        }
                        if ui.add(egui::Button::new(tr("Cancel")).min_size(vec2(84.0, 28.0))).clicked() {
                            close = true;
                        }
                        let ok = egui::Button::new(RichText::new("OK").color(Color32::WHITE)).fill(pal.accent).min_size(vec2(84.0, 28.0));
                        if ui.add(ok).clicked() && (!dirty || self.apply()) {
                            close = true;
                        }
                    });
                });
            });

        // ---- header: icon, name, type; then the tabs
        egui::TopBottomPanel::top("props-head")
            .frame(egui::Frame::none().fill(pal.window).inner_margin(egui::Margin { left: 16.0, right: 16.0, top: 16.0, bottom: 8.0 }))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
                    match &self.icon {
                        Some(t) => {
                            ui.painter().image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        None => crate::icons::paint_entry_icon(ui.painter(), r, self.path.is_dir(), "", &pal),
                    }
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.name).font(egui::FontId::proportional(16.0)).desired_width(ui.available_width()));
                        ui.label(RichText::new(&self.type_name).color(pal.dim));
                    });
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let mut tabs = vec![(Tab::General, tr("General"))];
                    if self.link.is_some() {
                        tabs.push((Tab::Shortcut, tr("Shortcut")));
                    }
                    if self.compat.is_some() {
                        tabs.push((Tab::Compat, tr("Compatibility")));
                    }
                    if !self.version.is_empty() || self.arch.is_some() {
                        tabs.push((Tab::Details, tr("Details")));
                    }
                    for (t, label) in tabs {
                        let on = self.tab == t;
                        let text = RichText::new(label).color(if on { Color32::WHITE } else { pal.text });
                        let b = egui::Button::new(text).fill(if on { pal.accent } else { pal.view }).rounding(14.0).min_size(vec2(0.0, 28.0));
                        if ui.add(b).clicked() {
                            self.tab = t;
                        }
                    }
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(pal.window).inner_margin(egui::Margin::symmetric(16.0, 8.0)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.tab {
                    Tab::General => self.general(ui),
                    Tab::Shortcut => self.shortcut(ui),
                    Tab::Compat => self.compatibility(ui),
                    Tab::Details => self.details(ui),
                });
            });

        if close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl Props {
    fn general(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        section(ui, &pal, tr("File"), |ui| {
            row(ui, &pal, tr("Type"), |ui| value(ui, &self.type_name));
            if let Some(dir) = self.path.parent() {
                row(ui, &pal, tr("Location"), |ui| {
                    value(ui, &dir.display().to_string());
                    if ui.link(tr("Open file location")).clicked() {
                        open_folder(dir);
                    }
                });
            }
            if let Some(m) = &self.meta {
                if m.is_file() {
                    row(ui, &pal, tr("Size"), |ui| value(ui, &size_text(m.len())));
                }
            }
            if let (Some(a), true) = (self.arch, self.program.as_deref() == Some(self.path.as_path())) {
                let bits = if a == "x86" || a == "ARM" { tr("32-bit program") } else { tr("64-bit program") };
                row(ui, &pal, tr("Program"), |ui| value(ui, &format!("{bits} ({a})")));
            }
        });
        if let Some(m) = &self.meta {
            section(ui, &pal, tr("Dates"), |ui| {
                row(ui, &pal, tr("Created"), |ui| value(ui, &when(m.created())));
                row(ui, &pal, tr("Modified"), |ui| value(ui, &when(m.modified())));
                row(ui, &pal, tr("Accessed"), |ui| value(ui, &when(m.accessed())));
            });
        }
        section(ui, &pal, tr("Attributes"), |ui| {
            ui.checkbox(&mut self.readonly, tr("Read-only"));
            ui.checkbox(&mut self.hidden, tr("Hidden"));
        });
    }

    fn shortcut(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let Some((_, s)) = &mut self.link else { return };
        section(ui, &pal, tr("Shortcut"), |ui| {
            row(ui, &pal, tr("Target"), |ui| {
                ui.add(egui::TextEdit::singleline(&mut s.target).desired_width(f32::INFINITY));
            });
            row(ui, &pal, tr("Arguments"), |ui| {
                ui.add(egui::TextEdit::singleline(&mut s.args).desired_width(f32::INFINITY));
            });
            row(ui, &pal, tr("Start in"), |ui| {
                ui.add(egui::TextEdit::singleline(&mut s.workdir).desired_width(f32::INFINITY));
            });
            row(ui, &pal, tr("Run"), |ui| {
                let label = |v: i32| match v {
                    3 => tr("Maximized"),
                    7 => tr("Minimized"),
                    _ => tr("Normal window"),
                };
                egui::ComboBox::from_id_salt("show").selected_text(label(s.show)).width(200.0).show_ui(ui, |ui| {
                    for v in [1, 7, 3] {
                        ui.selectable_value(&mut s.show, v, label(v));
                    }
                });
            });
            row(ui, &pal, tr("Comment"), |ui| {
                ui.add(egui::TextEdit::singleline(&mut s.comment).desired_width(f32::INFINITY));
            });
            let hk = sys::hotkey_text(s.hotkey);
            row(ui, &pal, tr("Shortcut key"), |ui| value(ui, if hk.is_empty() { tr("None") } else { &hk }));
        });
        let target = sys::expand(&s.target);
        ui.horizontal(|ui| {
            if let Some(dir) = target.parent().filter(|d| d.is_dir()) {
                if ui.button(tr("Open file location")).clicked() {
                    open_folder(dir);
                }
            }
            if target.is_file() && ui.button(tr("Properties of the program")).clicked() {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).arg("--props").arg(&target).spawn();
                }
            }
        });
    }

    fn compatibility(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let exe = self.program.clone().unwrap_or_default();
        let Some((_, c)) = &mut self.compat else { return };
        ui.label(RichText::new(trf("These settings apply to {}", &[&exe.display()])).size(12.0).color(pal.dim));
        ui.add_space(8.0);
        section(ui, &pal, tr("Compatibility mode"), |ui| {
            let mut on = c.mode.is_some();
            if ui.checkbox(&mut on, tr("Run this program in compatibility mode for:")).changed() {
                c.mode = on.then_some(c.mode.unwrap_or(2));
            }
            ui.add_enabled_ui(on, |ui| {
                let sel = c.mode.unwrap_or(2);
                egui::ComboBox::from_id_salt("mode").selected_text(MODES[sel].1).width(260.0).show_ui(ui, |ui| {
                    for (i, (_, name)) in MODES.iter().enumerate() {
                        if ui.selectable_label(sel == i, *name).clicked() {
                            c.mode = Some(i);
                        }
                    }
                });
            });
        });
        section(ui, &pal, tr("Settings"), |ui| {
            ui.checkbox(&mut c.admin, tr("Run this program as an administrator"));
            ui.checkbox(&mut c.no_fso, tr("Disable fullscreen optimizations"));
            ui.checkbox(&mut c.color16, tr("Reduced color mode (16-bit)"));
            ui.checkbox(&mut c.res640, tr("Run in 640 × 480 screen resolution"));
        });
        section(ui, &pal, tr("High DPI scaling"), |ui| {
            let names = [tr("Windows decides (default)"), tr("Application"), tr("System"), tr("System (Enhanced)")];
            ui.label(RichText::new(tr("Override high DPI scaling, performed by:")).size(12.5));
            egui::ComboBox::from_id_salt("dpi").selected_text(names[c.dpi]).width(260.0).show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut c.dpi, i, *n);
                }
            });
        });
        if ui.button(tr("Reset all")).clicked() {
            *c = Compat { other: c.other.clone(), ..Default::default() };
        }
    }

    fn details(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        section(ui, &pal, tr("Program"), |ui| {
            for (k, v) in &self.version {
                row(ui, &pal, tr(k), |ui| value(ui, v));
            }
            if let Some(a) = self.arch {
                row(ui, &pal, tr("Architecture"), |ui| value(ui, a));
            }
            if let Some(p) = &self.program {
                row(ui, &pal, tr("Path"), |ui| value(ui, &p.display().to_string()));
            }
        });
    }
}
