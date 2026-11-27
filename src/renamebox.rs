//! `delfin.exe --rename <path>`: a small window asking for a new name.
//!
//! The desktop is a plain Win32 window with no text fields of its own, so
//! renaming one of its icons opens this instead (F2, or Rename in its menu).

use crate::i18n::{tr, trf};
use crate::theme::{self, Palette};
use eframe::egui::{self, RichText};
use std::path::{Path, PathBuf};

struct RenameBox {
    path: PathBuf,
    text: String,
    focus: bool,
    error: Option<String>,
    pal: Palette,
}

fn display_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

pub fn run(path: PathBuf) -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(tr("Rename"))
            .with_app_id("delfin-rename")
            .with_icon(theme::app_icon())
            .with_inner_size([420.0, 150.0])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_minimize_button(false),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "Delfin Rename",
        options,
        Box::new(move |cc| {
            theme::install_fonts(&cc.egui_ctx);
            let dark = crate::config::Config::load().dark;
            theme::apply(&cc.egui_ctx, dark);
            let text = display_name(&path);
            Ok(Box::new(RenameBox { path, text, focus: true, error: None, pal: theme::palette(dark) }))
        }),
    )
}

impl RenameBox {
    /// Rename the file; `Err` is the message to show.
    fn rename(&self) -> Result<(), String> {
        let name = self.text.trim();
        if name.is_empty() || name.contains(|c: char| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
            return Err(tr("Invalid name").to_string());
        }
        let new = self.path.with_file_name(name);
        if new == self.path {
            return Ok(());
        }
        let only_case = new.to_string_lossy().to_lowercase() == self.path.to_string_lossy().to_lowercase();
        if new.exists() && !only_case {
            return Err(tr("An item with that name already exists").to_string());
        }
        std::fs::rename(&self.path, &new).map_err(|e| trf("Rename failed: {}", &[&e]))
    }
}

impl eframe::App for RenameBox {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let pal = self.pal;
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let mut apply = false;
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(pal.window).inner_margin(egui::Margin::same(16.0)))
            .show(ctx, |ui| {
                ui.label(trf("New name for \u{201c}{}\u{201d}:", &[&display_name(&self.path)]));
                ui.add_space(6.0);
                let mut out = egui::TextEdit::singleline(&mut self.text).desired_width(f32::INFINITY).show(ui);
                if self.focus {
                    // Select the name without its extension, like Windows does.
                    let chars = self.text.chars().count();
                    let stem = if self.path.is_dir() {
                        chars
                    } else {
                        Path::new(self.text.as_str()).file_stem().map(|s| s.to_string_lossy().chars().count()).unwrap_or(chars)
                    };
                    out.response.request_focus();
                    out.state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(stem),
                    )));
                    out.state.store(ui.ctx(), out.response.id);
                    self.focus = false;
                }
                if out.response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    apply = true;
                }
                ui.add_space(4.0);
                if let Some(e) = &self.error {
                    ui.label(RichText::new(e).size(12.0).color(pal.danger));
                }
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        close = true;
                    }
                    if ui.button(RichText::new(tr("Rename")).strong()).clicked() {
                        apply = true;
                    }
                });
            });
        if apply {
            match self.rename() {
                Ok(()) => close = true,
                Err(e) => {
                    self.error = Some(e);
                    self.focus = true;
                }
            }
        }
        if close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
