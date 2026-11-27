//! `delfin.exe --recyclebin`: the Recycle Bin (Explorer's own view can't open
//! without Explorer). What was deleted, from where and when; restore it,
//! delete it for good, or empty the whole bin.

use crate::i18n::{tr, trf};
use crate::theme::{self, Palette};
use eframe::egui::{self, vec2, Color32, RichText, Sense};
use std::collections::HashSet;
use std::ffi::OsString;
use std::sync::mpsc::{channel, Receiver};
use trash::TrashItem;

struct Row {
    item: TrashItem,
    name: String,
    from: String,
    when: String,
    /// Bytes for a file, None for a folder (or not known yet).
    size: Option<u64>,
}

enum Msg {
    Listed(Result<Vec<Row>, String>),
    Done(Result<String, String>),
}

struct Bin {
    pal: Palette,
    rows: Vec<Row>,
    sel: HashSet<OsString>,
    rx: Receiver<Msg>,
    tx: std::sync::mpsc::Sender<Msg>,
    busy: bool,
    message: Option<(String, bool)>,
    confirm: Option<Confirm>,
    loaded: bool,
}

enum MenuAct {
    Restore,
    Delete,
}

#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    DeleteSelected,
    Empty,
}

pub fn run() -> Result<(), eframe::Error> {
    // One window: another start brings the open one forward.
    unsafe {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, ShowWindow};
        let name: Vec<u16> = "Local\\DelfinRecycleBin".encode_utf16().chain(Some(0)).collect();
        let h = windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if !h.is_null() && GetLastError() == ERROR_ALREADY_EXISTS {
            let t: Vec<u16> = tr("Recycle Bin").encode_utf16().chain(Some(0)).collect();
            let w = FindWindowW(std::ptr::null(), t.as_ptr());
            if !w.is_null() {
                ShowWindow(w, 9);
                SetForegroundWindow(w);
            }
            return Ok(());
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(tr("Recycle Bin"))
            .with_app_id("delfin-recyclebin")
            .with_icon(theme::app_icon())
            .with_inner_size([860.0, 520.0])
            .with_min_inner_size([560.0, 320.0]),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "Delfin Recycle Bin",
        options,
        Box::new(|cc| {
            theme::install_fonts(&cc.egui_ctx);
            let dark = crate::config::Config::load().dark;
            theme::apply(&cc.egui_ctx, dark);
            let (tx, rx) = channel();
            let mut b = Bin { pal: theme::palette(dark), rows: Vec::new(), sel: HashSet::new(), rx, tx, busy: false, message: None, confirm: None, loaded: false };
            b.reload(&cc.egui_ctx);
            Ok(Box::new(b))
        }),
    )
}

fn list() -> Result<Vec<Row>, String> {
    let items = trash::os_limited::list().map_err(|e| e.to_string())?;
    let mut rows: Vec<Row> = items
        .into_iter()
        .map(|item| {
            let size = trash::os_limited::metadata(&item).ok().and_then(|m| m.size.size());
            let when = chrono::DateTime::from_timestamp(item.time_deleted, 0)
                .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default();
            Row {
                name: item.name.to_string_lossy().into_owned(),
                from: item.original_parent.display().to_string(),
                when,
                size,
                item,
            }
        })
        .collect();
    // Most recently deleted first.
    rows.sort_by(|a, b| b.item.time_deleted.cmp(&a.item.time_deleted).then_with(|| a.name.cmp(&b.name)));
    Ok(rows)
}

fn fmt_size(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

impl Bin {
    fn reload(&mut self, ctx: &egui::Context) {
        self.busy = true;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Listed(list()));
            ctx.request_repaint();
        });
    }

    fn selected(&self) -> Vec<TrashItem> {
        self.rows.iter().filter(|r| self.sel.contains(&r.item.id)).map(|r| r.item.clone()).collect()
    }

    /// Run `f` on a worker thread, then list the bin again.
    fn job(&mut self, ctx: &egui::Context, f: impl FnOnce() -> Result<String, String> + Send + 'static) {
        self.busy = true;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Done(f()));
            let _ = tx.send(Msg::Listed(list()));
            ctx.request_repaint();
        });
    }

    fn restore(&mut self, ctx: &egui::Context) {
        let items = self.selected();
        if items.is_empty() {
            return;
        }
        let n = items.len();
        self.job(ctx, move || {
            trash::os_limited::restore_all(items)
                .map(|_| trf("Restored {} item(s)", &[&n]))
                .map_err(|e| match e {
                    trash::Error::RestoreCollision { path, .. } => trf("Can't restore: {} already exists", &[&path.display()]),
                    e => trf("Can't restore: {}", &[&e]),
                })
        });
    }

    fn purge(&mut self, ctx: &egui::Context, items: Vec<TrashItem>) {
        if items.is_empty() {
            return;
        }
        let n = items.len();
        self.job(ctx, move || {
            trash::os_limited::purge_all(items)
                .map(|_| trf("Deleted {} item(s) permanently", &[&n]))
                .map_err(|e| trf("Can't delete: {}", &[&e]))
        });
    }
}

impl eframe::App for Bin {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Listed(r) => {
                    self.busy = false;
                    self.loaded = true;
                    match r {
                        Ok(rows) => {
                            let ids: HashSet<&OsString> = rows.iter().map(|r| &r.item.id).collect();
                            self.sel.retain(|id| ids.contains(id));
                            self.rows = rows;
                        }
                        Err(e) => self.message = Some((trf("Can't read the Recycle Bin: {}", &[&e]), true)),
                    }
                }
                Msg::Done(r) => {
                    self.message = Some(match r {
                        Ok(m) => (m, false),
                        Err(e) => (e, true),
                    })
                }
            }
        }
        let pal = self.pal;
        if ctx.input(|i| i.key_pressed(egui::Key::F5)) && !self.busy {
            self.reload(ctx);
        }
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::A)) {
            self.sel = self.rows.iter().map(|r| r.item.id.clone()).collect();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Delete)) && !self.sel.is_empty() {
            self.confirm = Some(Confirm::DeleteSelected);
        }

        let total: u64 = self.rows.iter().filter_map(|r| r.size).sum();
        egui::TopBottomPanel::top("bin-top")
            .frame(egui::Frame::none().fill(pal.window).inner_margin(egui::Margin::symmetric(14.0, 10.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                    crate::icons::paint_tool(ui.painter(), r, crate::icons::Tool::Trash, pal.text);
                    ui.label(RichText::new(tr("Recycle Bin")).size(16.0));
                    ui.label(RichText::new(trf("{} items · {}", &[&self.rows.len(), &fmt_size(total)])).color(pal.dim));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let any = !self.rows.is_empty() && !self.busy;
                        if ui.add_enabled(any, egui::Button::new(tr("Empty Recycle Bin"))).clicked() {
                            self.confirm = Some(Confirm::Empty);
                        }
                        let some = !self.sel.is_empty() && !self.busy;
                        if ui.add_enabled(some, egui::Button::new(tr("Delete permanently"))).clicked() {
                            self.confirm = Some(Confirm::DeleteSelected);
                        }
                        if ui.add_enabled(some, egui::Button::new(RichText::new(tr("Restore")).strong())).clicked() {
                            self.restore(ctx);
                        }
                    });
                });
            });

        egui::TopBottomPanel::bottom("bin-status")
            .frame(egui::Frame::none().fill(pal.window).inner_margin(egui::Margin::symmetric(14.0, 5.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let text = if self.busy {
                        tr("Working…").to_string()
                    } else if self.sel.is_empty() {
                        tr("Select items to restore them or delete them for good. Ctrl+A selects all.").to_string()
                    } else {
                        trf("{} selected", &[&self.sel.len()])
                    };
                    ui.label(RichText::new(text).size(12.0).color(pal.dim));
                    if let Some((m, err)) = &self.message {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(m).size(12.0).color(if *err { pal.danger } else { pal.accent }));
                        });
                    }
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(pal.view).inner_margin(egui::Margin::same(10.0)))
            .show(ctx, |ui| {
                if self.rows.is_empty() {
                    ui.centered_and_justified(|ui| {
                        let t = if self.loaded { tr("The Recycle Bin is empty.") } else { tr("Loading…") };
                        ui.label(RichText::new(t).size(16.0).color(pal.dim));
                    });
                    return;
                }
                let w = ui.available_width();
                let cols = [(tr("Name"), 0.34), (tr("Original location"), 0.36), (tr("Date deleted"), 0.18), (tr("Size"), 0.12)];
                ui.horizontal(|ui| {
                    for (label, f) in cols {
                        ui.add_sized([w * f - 6.0, 20.0], egui::Label::new(RichText::new(label).size(12.0).color(pal.dim)).truncate());
                    }
                });
                ui.separator();
                let mods = ui.input(|i| i.modifiers);
                let mut clicked: Option<usize> = None;
                let mut open_menu_on: Option<usize> = None;
                let mut menu_act: Option<MenuAct> = None;
                egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 26.0, self.rows.len(), |ui, range| {
                    for i in range {
                        let row = &self.rows[i];
                        let on = self.sel.contains(&row.item.id);
                        let (r, resp) = ui.allocate_exact_size(vec2(w, 26.0), Sense::click());
                        if on {
                            ui.painter().rect_filled(r, 4.0, pal.select);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(r, 4.0, pal.hover);
                        }
                        let size = match row.size {
                            Some(b) => fmt_size(b),
                            None => tr("Folder").to_string(),
                        };
                        let mut x = r.left() + 4.0;
                        for (k, (text, f)) in [(&row.name, 0.34), (&row.from, 0.36), (&row.when, 0.18), (&size, 0.12)].into_iter().enumerate() {
                            let cw = w * f;
                            let p = ui.painter().with_clip_rect(egui::Rect::from_min_size(egui::pos2(x, r.top()), vec2(cw - 8.0, r.height())));
                            p.text(egui::pos2(x, r.center().y), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(13.0), if k == 0 { pal.text } else { pal.dim });
                            x += cw;
                        }
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                        if resp.secondary_clicked() {
                            open_menu_on = Some(i);
                        }
                        resp.context_menu(|ui| {
                            // Acted on below, once the list isn't borrowed.
                            if ui.button(tr("Restore")).clicked() {
                                ui.close_menu();
                                menu_act = Some(MenuAct::Restore);
                            }
                            if ui.button(tr("Delete permanently")).clicked() {
                                ui.close_menu();
                                menu_act = Some(MenuAct::Delete);
                            }
                        });
                    }
                });
                if let Some(i) = clicked {
                    let id = self.rows[i].item.id.clone();
                    if mods.ctrl {
                        if !self.sel.remove(&id) {
                            self.sel.insert(id);
                        }
                    } else {
                        self.sel.clear();
                        self.sel.insert(id);
                    }
                }
                if let Some(i) = open_menu_on {
                    // Right-click on an unselected row selects just that row.
                    let id = self.rows[i].item.id.clone();
                    if !self.sel.contains(&id) {
                        self.sel.clear();
                        self.sel.insert(id);
                    }
                }
                match menu_act {
                    Some(MenuAct::Restore) => self.restore(ctx),
                    Some(MenuAct::Delete) => self.confirm = Some(Confirm::DeleteSelected),
                    None => {}
                }
            });

        // Deleting for good can't be undone: ask first.
        if let Some(c) = self.confirm {
            let n = if c == Confirm::Empty { self.rows.len() } else { self.sel.len() };
            let mut answer: Option<bool> = None;
            egui::Window::new(if c == Confirm::Empty { tr("Empty Recycle Bin") } else { tr("Delete permanently") })
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(trf("Permanently delete {} item(s)? This can't be undone.", &[&n]));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new(tr("Delete")).color(Color32::WHITE)).clicked() {
                            answer = Some(true);
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            answer = Some(false);
                        }
                    });
                });
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                answer = Some(false);
            }
            if let Some(yes) = answer {
                self.confirm = None;
                if yes {
                    let items = if c == Confirm::Empty { self.rows.iter().map(|r| r.item.clone()).collect() } else { self.selected() };
                    self.purge(ctx, items);
                }
            }
        }
    }
}

