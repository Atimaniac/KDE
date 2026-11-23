//! The main window: toolbar, tabs, places panel, split views, info panel, status bar.

use crate::config::{Config, ViewMode};
use crate::fsops::{self, Entry, SortBy};
use crate::icons::{self, Tool};
use crate::platform::{self, Drive};
use crate::theme::{self, Palette};
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, FontId, Frame, Key, Layout, Margin, Modifiers, Rect,
    RichText, ScrollArea, Sense, Shape, Stroke, Ui,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

// =====================================================================
// Pane = one folder view (a tab holds one, or two in split view)
// =====================================================================

pub struct Pane {
    pub cwd: PathBuf,
    history: Vec<PathBuf>,
    hist_pos: usize,
    entries: Vec<Entry>,
    /// Indices into `entries`, filtered + sorted.
    visible: Vec<usize>,
    selected: HashSet<PathBuf>,
    anchor: Option<usize>,
    cursor: Option<usize>,
    pub filter: String,
    error: Option<String>,
    scroll_to: Option<usize>,
    cols: usize,
    scroll_offset: f32,
    viewport_h: f32,
    space: Option<(u64, u64)>,
}

impl Pane {
    fn new(dir: PathBuf, cfg: &Config) -> Self {
        let mut p = Pane {
            cwd: dir.clone(),
            history: vec![dir],
            hist_pos: 0,
            entries: Vec::new(),
            visible: Vec::new(),
            selected: HashSet::new(),
            anchor: None,
            cursor: None,
            filter: String::new(),
            error: None,
            scroll_to: None,
            cols: 1,
            scroll_offset: 0.0,
            viewport_h: 400.0,
            space: None,
        };
        p.reload(cfg);
        p
    }

    fn reload(&mut self, cfg: &Config) {
        match fsops::list_dir(&self.cwd) {
            Ok(v) => {
                self.entries = v;
                self.error = None;
            }
            Err(e) => {
                self.entries.clear();
                self.error = Some(e.to_string());
            }
        }
        self.space = platform::space(&self.cwd);
        self.refilter(cfg);
        let existing: HashSet<&PathBuf> = self.entries.iter().map(|e| &e.path).collect();
        self.selected.retain(|p| existing.contains(p));
    }

    fn refilter(&mut self, cfg: &Config) {
        let f = self.filter.to_lowercase();
        let entries = &self.entries;
        let mut vis: Vec<usize> = (0..entries.len())
            .filter(|&i| {
                let e = &entries[i];
                (cfg.show_hidden || !e.hidden)
                    && (f.is_empty() || e.name.to_lowercase().contains(&f))
            })
            .collect();
        vis.sort_by(|&a, &b| {
            fsops::compare(
                &entries[a],
                &entries[b],
                cfg.sort,
                cfg.sort_asc,
                cfg.folders_first,
            )
        });
        self.visible = vis;
        let n = self.visible.len();
        if self.anchor.is_some_and(|a| a >= n) {
            self.anchor = None;
        }
        if self.cursor.is_some_and(|a| a >= n) {
            self.cursor = None;
        }
    }

    fn ent(&self, vi: usize) -> &Entry {
        &self.entries[self.visible[vi]]
    }

    fn vis_index_of(&self, p: &Path) -> Option<usize> {
        self.visible.iter().position(|&i| self.entries[i].path == p)
    }

    fn go(&mut self, p: PathBuf, cfg: &Config) {
        self.cwd = p;
        self.filter.clear();
        self.selected.clear();
        self.anchor = None;
        self.cursor = None;
        self.scroll_to = Some(0);
        self.reload(cfg);
    }

    fn navigate(&mut self, p: PathBuf, cfg: &Config) {
        if p == self.cwd {
            return;
        }
        let prev = self.cwd.clone();
        self.history.truncate(self.hist_pos + 1);
        self.history.push(p.clone());
        self.hist_pos = self.history.len() - 1;
        self.go(p, cfg);
        // Going up: highlight the folder we came from, like Dolphin.
        if let Some(i) = self.vis_index_of(&prev) {
            self.select_only(i);
            self.scroll_to = Some(i);
        }
    }

    fn can_back(&self) -> bool {
        self.hist_pos > 0
    }
    fn can_forward(&self) -> bool {
        self.hist_pos + 1 < self.history.len()
    }

    fn back(&mut self, cfg: &Config) {
        if self.can_back() {
            self.hist_pos -= 1;
            let p = self.history[self.hist_pos].clone();
            self.go(p, cfg);
        }
    }

    fn forward(&mut self, cfg: &Config) {
        if self.can_forward() {
            self.hist_pos += 1;
            let p = self.history[self.hist_pos].clone();
            self.go(p, cfg);
        }
    }

    fn up(&mut self, cfg: &Config) {
        if let Some(parent) = self.cwd.parent() {
            let parent = parent.to_path_buf();
            self.navigate(parent, cfg);
        }
    }

    fn select_only(&mut self, vi: usize) {
        self.selected.clear();
        let p = self.ent(vi).path.clone();
        self.selected.insert(p);
        self.anchor = Some(vi);
        self.cursor = Some(vi);
    }

    fn select_range(&mut self, a: usize, b: usize, keep: bool) {
        if !keep {
            self.selected.clear();
        }
        for i in a.min(b)..=a.max(b) {
            let p = self.ent(i).path.clone();
            self.selected.insert(p);
        }
    }

    fn click_select(&mut self, vi: usize, mods: Modifiers) {
        if mods.shift {
            if let Some(a) = self.anchor {
                self.select_range(a, vi, mods.command);
                self.cursor = Some(vi);
                return;
            }
        }
        if mods.command {
            let p = self.ent(vi).path.clone();
            if !self.selected.remove(&p) {
                self.selected.insert(p);
            }
            self.anchor = Some(vi);
            self.cursor = Some(vi);
            return;
        }
        self.select_only(vi);
    }

    /// Selected entries in display order: (path, is_dir)
    fn selection(&self) -> Vec<(PathBuf, bool)> {
        self.visible
            .iter()
            .map(|&i| &self.entries[i])
            .filter(|e| self.selected.contains(&e.path))
            .map(|e| (e.path.clone(), e.is_dir))
            .collect()
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        self.selection().into_iter().map(|(p, _)| p).collect()
    }
}

struct Tab {
    panes: Vec<Pane>,
    active: usize,
}

#[derive(Clone)]
struct Clip {
    paths: Vec<PathBuf>,
    cut: bool,
}

enum Dialog {
    None,
    Rename {
        path: PathBuf,
        text: String,
        focus: bool,
    },
    NewFolder {
        dir: PathBuf,
        text: String,
        focus: bool,
    },
    ConfirmDelete {
        paths: Vec<PathBuf>,
    },
    Shell {
        install: bool,
    },
    About,
}

enum DialogResult {
    Rename(PathBuf, String),
    NewFolder(PathBuf, String),
    Delete(Vec<PathBuf>),
    Shell(bool),
}

enum Action {
    Open(PathBuf, bool),
    OpenSelected,
    NewTab(PathBuf),
    Copy,
    Cut,
    Paste,
    PasteText(String),
    Rename,
    Trash,
    Delete,
    NewFolder,
    CopyPath,
    Terminal(PathBuf),
    AddPlace(PathBuf),
    RemovePlace(PathBuf),
    Sort(SortBy),
    Refilter,
    ToggleHidden,
    Refresh,
    ShowInfo,
    SelectAll,
    FolderHandler(bool),
    /// Unpack the selected archives: here (true) or each into its own folder.
    Extract(bool),
    /// Pack the selection into an archive of this type ("zip", "7z").
    Compress(&'static str),
    OpenIn7Zip(PathBuf),
}

#[derive(Default)]
struct PaneOut {
    actions: Vec<Action>,
    activated: bool,
}

type JobResult = Result<String, String>;

// =====================================================================
// App
// =====================================================================

pub struct App {
    cfg: Config,
    saved_cfg: Config,
    applied_dark: bool,
    tabs: Vec<Tab>,
    cur: usize,
    places: Vec<(&'static str, PathBuf)>,
    drives: Vec<Drive>,
    clip: Option<Clip>,
    dialog: Dialog,
    tx: Sender<JobResult>,
    rx: Receiver<JobResult>,
    jobs: usize,
    toast: Option<(String, Instant, bool)>,
    loc_edit: Option<String>,
    loc_focus: bool,
    search_focus: bool,
    title: String,
}

fn default_places() -> Vec<(&'static str, PathBuf)> {
    let candidates = [
        (crate::i18n::tr("Home"), dirs::home_dir()),
        (crate::i18n::tr("Desktop"), dirs::desktop_dir()),
        (crate::i18n::tr("Documents"), dirs::document_dir()),
        (crate::i18n::tr("Downloads"), dirs::download_dir()),
        (crate::i18n::tr("Music"), dirs::audio_dir()),
        (crate::i18n::tr("Pictures"), dirs::picture_dir()),
        (crate::i18n::tr("Videos"), dirs::video_dir()),
    ];
    let mut v: Vec<(&'static str, PathBuf)> = Vec::new();
    for (name, path) in candidates {
        if let Some(p) = path {
            if p.is_dir() && !v.iter().any(|(_, q)| *q == p) {
                v.push((name, p));
            }
        }
    }
    v
}

fn display_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

/// Path → breadcrumb segments (label, full path).
fn crumbs(p: &Path) -> Vec<(String, PathBuf)> {
    use std::path::Component;
    let mut acc = PathBuf::new();
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for c in p.components() {
        acc.push(c.as_os_str());
        match c {
            Component::Prefix(pre) => {
                out.push((pre.as_os_str().to_string_lossy().into_owned(), acc.clone()))
            }
            Component::RootDir => {
                if let Some(last) = out.last_mut() {
                    last.1 = acc.clone();
                } else {
                    out.push(("/".into(), acc.clone()));
                }
            }
            Component::Normal(n) => out.push((n.to_string_lossy().into_owned(), acc.clone())),
            _ => {}
        }
    }
    out
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, start: Option<PathBuf>) -> Self {
        platform::init();
        theme::install_fonts(&cc.egui_ctx);
        let cfg = Config::load();
        theme::apply(&cc.egui_ctx, cfg.dark);

        let start = start
            .filter(|p| p.is_dir())
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"));
        let (tx, rx) = channel();
        let pane = Pane::new(start, &cfg);

        Self {
            saved_cfg: cfg.clone(),
            applied_dark: cfg.dark,
            cfg,
            tabs: vec![Tab {
                panes: vec![pane],
                active: 0,
            }],
            cur: 0,
            places: default_places(),
            drives: platform::drives(),
            clip: None,
            dialog: Dialog::None,
            tx,
            rx,
            jobs: 0,
            toast: None,
            loc_edit: None,
            loc_focus: false,
            search_focus: false,
            title: String::new(),
        }
    }

    fn pane(&self) -> &Pane {
        let t = &self.tabs[self.cur];
        &t.panes[t.active]
    }

    fn with_pane<R>(&mut self, f: impl FnOnce(&mut Pane, &Config) -> R) -> R {
        let cfg = &self.cfg;
        let t = &mut self.tabs[self.cur];
        let a = t.active;
        f(&mut t.panes[a], cfg)
    }

    fn reload_all(&mut self) {
        let cfg = &self.cfg;
        for t in &mut self.tabs {
            for p in &mut t.panes {
                p.reload(cfg);
            }
        }
    }

    fn refilter_all(&mut self) {
        let cfg = &self.cfg;
        for t in &mut self.tabs {
            for p in &mut t.panes {
                p.refilter(cfg);
            }
        }
    }

    fn info(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now(), false));
    }

    fn err(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now(), true));
    }

    fn new_tab(&mut self, p: PathBuf) {
        let pane = Pane::new(p, &self.cfg);
        self.tabs.push(Tab {
            panes: vec![pane],
            active: 0,
        });
        self.cur = self.tabs.len() - 1;
    }

    fn close_tab(&mut self, i: usize) {
        if self.tabs.len() <= 1 || i >= self.tabs.len() {
            return;
        }
        self.tabs.remove(i);
        if i < self.cur || self.cur >= self.tabs.len() {
            self.cur = self.cur.saturating_sub(1);
        }
    }

    fn toggle_split(&mut self) {
        let cfg = &self.cfg;
        let t = &mut self.tabs[self.cur];
        if t.panes.len() > 1 {
            let keep = t.panes.remove(t.active);
            t.panes = vec![keep];
            t.active = 0;
        } else {
            let cwd = t.panes[0].cwd.clone();
            t.panes.push(Pane::new(cwd, cfg));
            t.active = 1;
        }
    }

    fn select_path(&mut self, p: &Path) {
        self.with_pane(|pane, _| {
            if let Some(i) = pane.vis_index_of(p) {
                pane.select_only(i);
                pane.scroll_to = Some(i);
            }
        });
    }

    fn spawn(&mut self, ctx: &egui::Context, f: impl FnOnce() -> JobResult + Send + 'static) {
        self.jobs += 1;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = f();
            let _ = tx.send(r);
            ctx.request_repaint();
        });
    }

    fn poll_jobs(&mut self) {
        let mut any = false;
        while let Ok(r) = self.rx.try_recv() {
            self.jobs = self.jobs.saturating_sub(1);
            any = true;
            match r {
                Ok(m) => self.info(m),
                Err(e) => self.err(e),
            }
        }
        if any {
            self.reload_all();
            self.drives = platform::drives();
        }
    }

    fn paste(&mut self, ctx: &egui::Context) {
        let Some(clip) = self.clip.clone() else {
            return;
        };
        let dest = self.pane().cwd.clone();
        if clip.cut {
            self.clip = None;
        }
        self.spawn(ctx, move || fsops::paste(&clip.paths, &dest, clip.cut));
    }

    fn go_to_typed(&mut self, typed: &str) {
        let mut s = typed.trim().trim_matches('"').to_string();
        if let Some(rest) = s.strip_prefix('~') {
            if let Some(h) = dirs::home_dir() {
                s = format!("{}{}", h.display(), rest);
            }
        }
        let p = PathBuf::from(&s);
        if p.is_dir() {
            self.with_pane(|pane, c| pane.navigate(p, c));
        } else if p.is_file() {
            if let Err(e) = platform::open(&p) {
                self.err(e);
            }
        } else {
            self.err(crate::i18n::trf("Location not found: {}", &[&s]));
        }
    }

    // -----------------------------------------------------------------
    // Actions
    // -----------------------------------------------------------------

    fn apply(&mut self, ctx: &egui::Context, a: Action) {
        match a {
            Action::Open(p, true) => self.with_pane(|pane, c| pane.navigate(p, c)),
            Action::Open(p, false) => {
                if let Err(e) = platform::open(&p) {
                    self.err(e);
                }
            }
            Action::OpenSelected => {
                let sel = self.pane().selection();
                let mut first_dir = true;
                for (p, is_dir) in sel {
                    if is_dir {
                        if first_dir {
                            first_dir = false;
                            self.with_pane(|pane, c| pane.navigate(p, c));
                        } else {
                            let keep = self.cur;
                            self.new_tab(p);
                            self.cur = keep;
                        }
                    } else if let Err(e) = platform::open(&p) {
                        self.err(e);
                    }
                }
            }
            Action::NewTab(p) => self.new_tab(p),
            Action::Copy | Action::Cut => {
                let cut = matches!(a, Action::Cut);
                let paths = self.pane().selected_paths();
                if paths.is_empty() {
                    return;
                }
                // Also put paths on the system clipboard as text.
                let text = paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                ctx.output_mut(|o| o.copied_text = text);
                let n = paths.len();
                self.clip = Some(Clip { paths, cut });
                self.info(format!(
                    "{} {n} item(s)",
                    if cut { crate::i18n::tr("Cut") } else { crate::i18n::tr("Copied") }
                ));
            }
            Action::Paste => self.paste(ctx),
            Action::PasteText(text) => {
                if self.clip.is_some() {
                    self.paste(ctx);
                } else {
                    // Paths copied from elsewhere (e.g. Explorer's "Copy as path").
                    let paths: Vec<PathBuf> = text
                        .lines()
                        .map(|l| PathBuf::from(l.trim().trim_matches('"')))
                        .filter(|p| !p.as_os_str().is_empty() && p.exists())
                        .collect();
                    if !paths.is_empty() {
                        self.clip = Some(Clip { paths, cut: false });
                        self.paste(ctx);
                        self.clip = None;
                    }
                }
            }
            Action::Rename => {
                let sel = self.pane().selected_paths();
                if sel.len() == 1 {
                    let path = sel[0].clone();
                    let text = display_name(&path);
                    self.dialog = Dialog::Rename {
                        path,
                        text,
                        focus: true,
                    };
                }
            }
            Action::Trash => {
                let sel = self.pane().selected_paths();
                if !sel.is_empty() {
                    let n = sel.len();
                    self.spawn(ctx, move || {
                        trash::delete_all(&sel)
                            .map(|_| crate::i18n::trf("Moved {} item(s) to Trash", &[&n]))
                            .map_err(|e| e.to_string())
                    });
                }
            }
            Action::Delete => {
                let paths = self.pane().selected_paths();
                if !paths.is_empty() {
                    self.dialog = Dialog::ConfirmDelete { paths };
                }
            }
            Action::NewFolder => {
                let dir = self.pane().cwd.clone();
                self.dialog = Dialog::NewFolder {
                    dir,
                    text: crate::i18n::tr("New Folder").into(),
                    focus: true,
                };
            }
            Action::CopyPath => {
                let sel = self.pane().selected_paths();
                let text = if sel.is_empty() {
                    self.pane().cwd.display().to_string()
                } else {
                    sel.iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                ctx.output_mut(|o| o.copied_text = text);
                self.info(crate::i18n::tr("Location copied"));
            }
            Action::Terminal(p) => {
                if let Err(e) = platform::open_terminal(&p) {
                    self.err(e);
                }
            }
            Action::AddPlace(p) => {
                if !self.cfg.places.contains(&p) && !self.places.iter().any(|(_, q)| *q == p) {
                    self.cfg.places.push(p);
                }
            }
            Action::RemovePlace(p) => self.cfg.places.retain(|q| *q != p),
            Action::Sort(k) => {
                if self.cfg.sort == k {
                    self.cfg.sort_asc = !self.cfg.sort_asc;
                } else {
                    self.cfg.sort = k;
                    self.cfg.sort_asc = true;
                }
                self.refilter_all();
            }
            Action::Refilter => self.refilter_all(),
            Action::ToggleHidden => {
                self.cfg.show_hidden = !self.cfg.show_hidden;
                self.refilter_all();
            }
            Action::Refresh => {
                self.reload_all();
                self.drives = platform::drives();
            }
            Action::ShowInfo => self.cfg.info_panel = true,
            Action::FolderHandler(on) => match platform::set_folder_handler(on) {
                Ok(()) if on => self.info(crate::i18n::tr("Folders now open in Delfin")),
                Ok(()) => self.info(crate::i18n::tr("Folders open in Windows Explorer again")),
                Err(e) => self.err(e),
            },
            Action::SelectAll => self.with_pane(|p, _| {
                p.selected = p
                    .visible
                    .iter()
                    .map(|&i| p.entries[i].path.clone())
                    .collect();
            }),
            #[cfg(windows)]
            Action::Extract(here) => {
                let cwd = self.pane().cwd.clone();
                for a in self.pane().selected_paths().into_iter().filter(|p| crate::archive::is_archive(p)) {
                    let dest = if here { cwd.clone() } else { crate::archive::unique(&cwd, &crate::archive::base_name(&a), "") };
                    let name = display_name(&a);
                    self.spawn(ctx, move || {
                        crate::archive::extract(&a, &dest)
                            .map(|_| crate::i18n::trf("Extracted {}", &[&name]))
                            .map_err(|e| crate::i18n::trf("Cannot extract {}: {}", &[&name, &e]))
                    });
                }
            }
            #[cfg(windows)]
            Action::Compress(ext) => {
                let paths = self.pane().selected_paths();
                if !paths.is_empty() {
                    let cwd = self.pane().cwd.clone();
                    let target = crate::archive::unique(&cwd, &crate::archive::pack_name(&paths), ext);
                    let name = display_name(&target);
                    self.spawn(ctx, move || {
                        crate::archive::compress(&paths, &target)
                            .map(|_| crate::i18n::trf("Created {}", &[&name]))
                            .map_err(|e| crate::i18n::trf("Cannot create {}: {}", &[&name, &e]))
                    });
                }
            }
            #[cfg(windows)]
            Action::OpenIn7Zip(p) => {
                if let Err(e) = crate::archive::open_in_7zip(&p) {
                    self.err(e);
                }
            }
            #[cfg(not(windows))]
            Action::Extract(_) | Action::Compress(_) | Action::OpenIn7Zip(_) => {}
        }
    }

    // -----------------------------------------------------------------
    // Keyboard
    // -----------------------------------------------------------------

    fn move_cursor(&mut self, delta: i64, vertical: bool, extend: bool) {
        self.with_pane(|p, cfg| {
            let n = p.visible.len();
            if n == 0 {
                return;
            }
            let cols: i64 = if cfg.view == ViewMode::Details {
                1
            } else {
                p.cols.max(1) as i64
            };
            let step = if vertical { delta * cols } else { delta };
            let new = match p.cursor {
                None => 0,
                Some(c) => (c as i64 + step).clamp(0, n as i64 - 1) as usize,
            };
            if extend {
                let a = p.anchor.unwrap_or(new);
                p.select_range(a, new, false);
                p.anchor = Some(a);
                p.cursor = Some(new);
            } else {
                p.select_only(new);
            }
            p.scroll_to = Some(new);
        });
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if !matches!(self.dialog, Dialog::None) {
            return;
        }
        let (b_back, b_fwd, zoom) = ctx.input(|i| {
            (
                i.pointer.button_pressed(egui::PointerButton::Extra1),
                i.pointer.button_pressed(egui::PointerButton::Extra2),
                i.zoom_delta(),
            )
        });
        if b_back {
            self.with_pane(|p, c| p.back(c));
        }
        if b_fwd {
            self.with_pane(|p, c| p.forward(c));
        }
        if zoom != 1.0 {
            self.cfg.icon_size = (self.cfg.icon_size * zoom).clamp(24.0, 160.0);
        }

        const NONE: Modifiers = Modifiers::NONE;
        const CTRL: Modifiers = Modifiers::COMMAND;
        const ALT: Modifiers = Modifiers::ALT;
        const SHIFT: Modifiers = Modifiers::SHIFT;
        let ck = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));

        // Work even while typing in a text field.
        if ck(NONE, Key::F3) {
            self.toggle_split();
        }
        if ck(NONE, Key::F9) {
            self.cfg.places_panel = !self.cfg.places_panel;
        }
        if ck(NONE, Key::F11) {
            self.cfg.info_panel = !self.cfg.info_panel;
        }
        if ck(CTRL, Key::T) {
            let cwd = self.pane().cwd.clone();
            self.new_tab(cwd);
        }
        if ck(CTRL, Key::W) {
            self.close_tab(self.cur);
        }
        if ck(CTRL, Key::PageDown) {
            self.cur = (self.cur + 1) % self.tabs.len();
        }
        if ck(CTRL, Key::PageUp) {
            self.cur = (self.cur + self.tabs.len() - 1) % self.tabs.len();
        }
        if ck(CTRL, Key::L) {
            self.loc_edit = Some(self.pane().cwd.display().to_string());
            self.loc_focus = true;
        }
        if ck(CTRL, Key::F) {
            self.search_focus = true;
        }

        if ctx.wants_keyboard_input() {
            return;
        }

        let mut acts = Vec::new();
        if ck(CTRL | SHIFT, Key::N) {
            acts.push(Action::NewFolder);
        }
        if ck(CTRL, Key::H) {
            acts.push(Action::ToggleHidden);
        }
        if ck(CTRL, Key::A) {
            acts.push(Action::SelectAll);
        }
        if ck(CTRL, Key::Num1) {
            self.cfg.view = ViewMode::Icons;
        }
        if ck(CTRL, Key::Num2) {
            self.cfg.view = ViewMode::Compact;
        }
        if ck(CTRL, Key::Num3) {
            self.cfg.view = ViewMode::Details;
        }
        // Alt+arrows before plain arrows (plain pattern also matches Alt).
        if ck(ALT, Key::ArrowLeft) {
            self.with_pane(|p, c| p.back(c));
        }
        if ck(ALT, Key::ArrowRight) {
            self.with_pane(|p, c| p.forward(c));
        }
        if ck(ALT, Key::ArrowUp) {
            self.with_pane(|p, c| p.up(c));
        }
        if ck(ALT, Key::Home) {
            if let Some(h) = dirs::home_dir() {
                self.with_pane(|p, c| p.navigate(h, c));
            }
        }
        if ck(NONE, Key::Backspace) {
            self.with_pane(|p, c| p.up(c));
        }
        if ck(NONE, Key::F5) {
            acts.push(Action::Refresh);
        }
        if ck(NONE, Key::F2) {
            acts.push(Action::Rename);
        }
        if ck(NONE, Key::F4) {
            acts.push(Action::Terminal(self.pane().cwd.clone()));
        }
        if ck(SHIFT, Key::Delete) {
            acts.push(Action::Delete);
        }
        if ck(NONE, Key::Delete) {
            acts.push(Action::Trash);
        }
        // Alt+Enter: Properties, like in Windows.
        if ck(ALT, Key::Enter) {
            #[cfg(windows)]
            if let Some(p) = self.pane().selected_paths().into_iter().next() {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).arg("--props").arg(&p).spawn();
                }
            }
        }
        if ck(NONE, Key::Enter) {
            acts.push(Action::OpenSelected);
        }
        if ck(NONE, Key::Escape) {
            self.with_pane(|p, c| {
                if !p.filter.is_empty() {
                    p.filter.clear();
                    p.refilter(c);
                }
                p.selected.clear();
            });
        }

        let big = i64::MAX / 4;
        for (key, delta, vertical) in [
            (Key::ArrowLeft, -1, false),
            (Key::ArrowRight, 1, false),
            (Key::ArrowUp, -1, true),
            (Key::ArrowDown, 1, true),
            (Key::Home, -big, false),
            (Key::End, big, false),
        ] {
            if ck(SHIFT, key) {
                self.move_cursor(delta, vertical, true);
            } else if ck(NONE, key) {
                self.move_cursor(delta, vertical, false);
            }
        }

        // egui turns Ctrl+C / X / V into these events instead of key presses.
        let events = ctx.input(|i| i.events.clone());
        for ev in events {
            match ev {
                egui::Event::Copy => acts.push(Action::Copy),
                egui::Event::Cut => acts.push(Action::Cut),
                egui::Event::Paste(t) => acts.push(Action::PasteText(t)),
                _ => {}
            }
        }

        for a in acts {
            self.apply(ctx, a);
        }
    }

    // -----------------------------------------------------------------
    // UI pieces
    // -----------------------------------------------------------------

    fn toolbar(&mut self, ctx: &egui::Context, pal: &Palette) {
        egui::TopBottomPanel::top("toolbar")
            .frame(
                Frame::none()
                    .fill(pal.header)
                    .inner_margin(Margin::symmetric(6.0, 5.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (cb, cf, cu) = {
                        let p = self.pane();
                        (p.can_back(), p.can_forward(), p.cwd.parent().is_some())
                    };
                    if icons::tool_button(ui, Tool::Back, false, cb, crate::i18n::tr("Back (Alt+Left)")).clicked() {
                        self.with_pane(|p, c| p.back(c));
                    }
                    if icons::tool_button(ui, Tool::Forward, false, cf, crate::i18n::tr("Forward (Alt+Right)"))
                        .clicked()
                    {
                        self.with_pane(|p, c| p.forward(c));
                    }
                    if icons::tool_button(ui, Tool::Up, false, cu, crate::i18n::tr("Up (Alt+Up)")).clicked() {
                        self.with_pane(|p, c| p.up(c));
                    }
                    if icons::tool_button(ui, Tool::Home, false, true, crate::i18n::tr("Home (Alt+Home)")).clicked()
                    {
                        if let Some(h) = dirs::home_dir() {
                            self.with_pane(|p, c| p.navigate(h, c));
                        }
                    }
                    ui.add_space(4.0);

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        self.main_menu(ui, ctx);

                        let r = self.with_pane(|p, _| {
                            ui.add(
                                egui::TextEdit::singleline(&mut p.filter)
                                    .hint_text(crate::i18n::tr("Filter…"))
                                    .desired_width(170.0),
                            )
                        });
                        if self.search_focus {
                            r.request_focus();
                            self.search_focus = false;
                        }
                        if r.changed() {
                            self.with_pane(|p, c| {
                                p.selected.clear();
                                p.anchor = None;
                                p.cursor = None;
                                p.refilter(c);
                            });
                        }
                        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                            self.with_pane(|p, c| {
                                p.filter.clear();
                                p.refilter(c);
                            });
                        }
                        let (sr, _) = ui.allocate_exact_size(vec2(20.0, 28.0), Sense::hover());
                        icons::paint_tool(ui.painter(), sr, Tool::Search, pal.dim);

                        ui.separator();
                        let split = self.tabs[self.cur].panes.len() > 1;
                        if icons::tool_button(ui, Tool::Split, split, true, crate::i18n::tr("Split view (F3)"))
                            .clicked()
                        {
                            self.toggle_split();
                        }
                        if icons::tool_button(
                            ui,
                            Tool::Info,
                            self.cfg.info_panel,
                            true,
                            crate::i18n::tr("Information panel (F11)"),
                        )
                        .clicked()
                        {
                            self.cfg.info_panel = !self.cfg.info_panel;
                        }
                        ui.separator();
                        for (t, m, tip) in [
                            (Tool::Details, ViewMode::Details, crate::i18n::tr("Details view (Ctrl+3)")),
                            (Tool::Compact, ViewMode::Compact, crate::i18n::tr("Compact view (Ctrl+2)")),
                            (Tool::Icons, ViewMode::Icons, crate::i18n::tr("Icons view (Ctrl+1)")),
                        ] {
                            if icons::tool_button(ui, t, self.cfg.view == m, true, tip).clicked() {
                                self.cfg.view = m;
                            }
                        }
                        ui.add_space(6.0);
                        self.location_bar(ui, pal);
                    });
                });
            });
    }

    fn location_bar(&mut self, ui: &mut Ui, pal: &Palette) {
        let width = ui.available_width().max(120.0);
        let mut nav: Option<PathBuf> = None;
        let mut newtab: Option<PathBuf> = None;
        let mut typed: Option<String> = None;

        ui.allocate_ui_with_layout(
            vec2(width, 28.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                let editing = self.loc_edit.is_some();
                Frame::none()
                    .fill(pal.view)
                    .stroke(Stroke::new(
                        1.0_f32,
                        if editing { pal.accent } else { pal.border },
                    ))
                    .rounding(4.0)
                    .inner_margin(Margin::symmetric(4.0, 2.0))
                    .show(ui, |ui| {
                        ui.set_min_size(vec2(ui.available_width(), 24.0));
                        if let Some(text) = self.loc_edit.as_mut() {
                            let r = ui.add(
                                egui::TextEdit::singleline(text)
                                    .frame(false)
                                    .desired_width(f32::INFINITY),
                            );
                            if self.loc_focus {
                                r.request_focus();
                                self.loc_focus = false;
                            }
                            if r.lost_focus() {
                                if ui.input(|i| i.key_pressed(Key::Enter)) {
                                    typed = Some(text.clone());
                                }
                                self.loc_edit = None;
                            }
                        } else {
                            let cwd = self.pane().cwd.clone();
                            let parts = crumbs(&cwd);
                            // Show as many trailing segments as fit.
                            let avail = ui.available_width() - 60.0;
                            let est = |s: &str| s.chars().count() as f32 * 7.5 + 30.0;
                            let mut start = parts.len();
                            let mut total = 0.0;
                            while start > 0 {
                                let w = est(&parts[start - 1].0);
                                if total + w > avail && start < parts.len() {
                                    break;
                                }
                                total += w;
                                start -= 1;
                            }
                            if start > 0 {
                                if ui.selectable_label(false, "…").clicked() {
                                    nav = Some(parts[start - 1].1.clone());
                                }
                                chevron(ui, pal.dim);
                            }
                            for (i, (name, path)) in parts.iter().enumerate().skip(start) {
                                let last = i + 1 == parts.len();
                                let txt = if last {
                                    RichText::new(name).strong()
                                } else {
                                    RichText::new(name)
                                };
                                let r = ui.selectable_label(false, txt);
                                if r.clicked() {
                                    nav = Some(path.clone());
                                }
                                if r.middle_clicked() {
                                    newtab = Some(path.clone());
                                }
                                if !last {
                                    chevron(ui, pal.dim);
                                }
                            }
                            // Clicking the empty part switches to an editable path (like Dolphin).
                            let rest = ui.allocate_response(ui.available_size(), Sense::click());
                            if rest.clicked() {
                                self.loc_edit = Some(cwd.display().to_string());
                                self.loc_focus = true;
                            }
                            rest.on_hover_cursor(egui::CursorIcon::Text);
                        }
                    });
            },
        );

        if let Some(p) = nav {
            self.with_pane(|pane, c| pane.navigate(p, c));
        }
        if let Some(p) = newtab {
            self.new_tab(p);
        }
        if let Some(t) = typed {
            self.go_to_typed(&t);
        }
    }

    fn main_menu(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let resp = icons::tool_button(ui, Tool::Menu, false, true, crate::i18n::tr("Menu"));
        let id = ui.make_persistent_id("main_menu");
        if resp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(id));
        }
        let mut acts = Vec::new();
        egui::popup_below_widget(
            ui,
            id,
            &resp,
            egui::PopupCloseBehavior::CloseOnClickOutside,
            |ui| {
                ui.set_min_width(240.0);
                if ui
                    .checkbox(&mut self.cfg.show_hidden, crate::i18n::tr("Show hidden files  (Ctrl+H)"))
                    .changed()
                {
                    acts.push(Action::Refilter);
                }
                ui.checkbox(&mut self.cfg.places_panel, crate::i18n::tr("Places panel  (F9)"));
                ui.checkbox(&mut self.cfg.info_panel, crate::i18n::tr("Information panel  (F11)"));
                if ui
                    .checkbox(&mut self.cfg.folders_first, crate::i18n::tr("Folders first"))
                    .changed()
                {
                    acts.push(Action::Refilter);
                }
                ui.checkbox(&mut self.cfg.dark, crate::i18n::tr("Breeze Dark theme"));
                ui.separator();
                ui.label(RichText::new(crate::i18n::tr("Sort by")).weak());
                for (k, n) in [
                    (SortBy::Name, crate::i18n::tr("Name")),
                    (SortBy::Size, crate::i18n::tr("Size")),
                    (SortBy::Modified, crate::i18n::tr("Modified")),
                    (SortBy::Type, crate::i18n::tr("Type")),
                ] {
                    if ui.radio(self.cfg.sort == k, n).clicked() {
                        self.cfg.sort = k;
                        acts.push(Action::Refilter);
                    }
                }
                if ui.checkbox(&mut self.cfg.sort_asc, crate::i18n::tr("Ascending")).changed() {
                    acts.push(Action::Refilter);
                }
                ui.separator();
                let mut item = |ui: &mut Ui, label: &str, sc: &str, a: Action| {
                    if ui.add(egui::Button::new(label).shortcut_text(sc)).clicked() {
                        acts.push(a);
                        ui.memory_mut(|m| m.close_popup());
                    }
                };
                let cwd = self.pane().cwd.clone();
                item(ui, crate::i18n::tr("New Tab"), "Ctrl+T", Action::NewTab(cwd.clone()));
                item(ui, crate::i18n::tr("New Folder…"), "Ctrl+Shift+N", Action::NewFolder);
                item(
                    ui,
                    crate::i18n::tr("Open Terminal Here"),
                    "F4",
                    Action::Terminal(cwd.clone()),
                );
                item(ui, crate::i18n::tr("Add Folder to Places"), "", Action::AddPlace(cwd));
                item(ui, crate::i18n::tr("Refresh"), "F5", Action::Refresh);
                ui.separator();
                ui.label(RichText::new(crate::i18n::tr("System")).weak());
                let mut handler = platform::folder_handler_set();
                if ui
                    .checkbox(&mut handler, crate::i18n::tr("Open folders in Delfin by default"))
                    .changed()
                {
                    acts.push(Action::FolderHandler(handler));
                }
                let installed = platform::shell_installed();
                let label = if installed {
                    crate::i18n::tr("Restore Windows Explorer as shell…")
                } else {
                    crate::i18n::tr("Use Delfin as Windows shell…")
                };
                if ui.button(label).clicked() {
                    self.dialog = Dialog::Shell {
                        install: !installed,
                    };
                    ui.memory_mut(|m| m.close_popup());
                }
                ui.separator();
                if ui.button(crate::i18n::tr("About Delfin")).clicked() {
                    self.dialog = Dialog::About;
                    ui.memory_mut(|m| m.close_popup());
                }
            },
        );
        for a in acts {
            self.apply(ctx, a);
        }
    }

    fn tabbar(&mut self, ctx: &egui::Context, pal: &Palette) {
        let mut select = None;
        let mut close = None;
        let mut add = false;
        egui::TopBottomPanel::top("tabbar")
            .frame(
                Frame::none()
                    .fill(pal.window)
                    .inner_margin(Margin::symmetric(6.0, 3.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for (i, t) in self.tabs.iter().enumerate() {
                        let name = display_name(&t.panes[t.active].cwd);
                        let r = ui.selectable_label(i == self.cur, format!("  {name}  "));
                        if r.clicked() {
                            select = Some(i);
                        }
                        if r.middle_clicked() {
                            close = Some(i);
                        }
                        if icons::tool_button_sized(
                            ui,
                            Tool::Close,
                            false,
                            true,
                            crate::i18n::tr("Close tab (Ctrl+W)"),
                            vec2(20.0, 20.0),
                        )
                        .clicked()
                        {
                            close = Some(i);
                        }
                        ui.add_space(6.0);
                    }
                    if icons::tool_button(ui, Tool::Plus, false, true, crate::i18n::tr("New tab (Ctrl+T)")).clicked()
                    {
                        add = true;
                    }
                });
            });
        if let Some(i) = select {
            self.cur = i;
        }
        if let Some(i) = close {
            self.close_tab(i);
        }
        if add {
            let cwd = self.pane().cwd.clone();
            self.new_tab(cwd);
        }
    }

    fn statusbar(&mut self, ctx: &egui::Context, pal: &Palette) {
        egui::TopBottomPanel::bottom("status")
            .frame(
                Frame::none()
                    .fill(pal.window)
                    .inner_margin(Margin::symmetric(8.0, 3.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let p = self.pane();
                    let (mut nd, mut nf, mut sz) = (0, 0, 0u64);
                    let (mut sn, mut ssz) = (0, 0u64);
                    for &i in &p.visible {
                        let e = &p.entries[i];
                        if e.is_dir {
                            nd += 1;
                        } else {
                            nf += 1;
                            sz += e.size;
                        }
                        if p.selected.contains(&e.path) {
                            sn += 1;
                            ssz += e.size;
                        }
                    }
                    let text = if sn > 0 {
                        crate::i18n::trf("{} selected ({})", &[&sn, &fsops::fmt_size(ssz)])
                    } else {
                        crate::i18n::trf("{} folders, {} files ({})", &[&nd, &nf, &fsops::fmt_size(sz)])
                    };
                    let space = p.space;
                    ui.label(text);
                    if self.jobs > 0 {
                        ui.spinner();
                        ui.label(crate::i18n::tr("Working…"));
                    }
                    if let Some((msg, t, is_err)) = &self.toast {
                        if t.elapsed() < Duration::from_secs(5) {
                            ui.label(RichText::new(msg).color(if *is_err {
                                pal.danger
                            } else {
                                pal.accent
                            }));
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add(
                            egui::Slider::new(&mut self.cfg.icon_size, 24.0..=160.0)
                                .show_value(false),
                        )
                        .on_hover_text(crate::i18n::tr("Zoom (Ctrl+mouse wheel)"));
                        ui.label(RichText::new(crate::i18n::tr("Zoom")).color(pal.dim));
                        if let Some((free, total)) = space {
                            if total > 0 {
                                let used = 1.0 - free as f32 / total as f32;
                                ui.add(
                                    egui::ProgressBar::new(used)
                                        .desired_width(160.0)
                                        .text(crate::i18n::trf("{} free", &[&fsops::fmt_size(free)])),
                                );
                            }
                        }
                    });
                });
            });
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, t, _)| t.elapsed() < Duration::from_secs(6))
        {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    fn places_panel(&mut self, ctx: &egui::Context, pal: &Palette) {
        let mut nav: Option<PathBuf> = None;
        let mut newtab: Option<PathBuf> = None;
        let mut remove: Option<PathBuf> = None;

        egui::SidePanel::left("places")
            .resizable(true)
            .default_width(200.0)
            .width_range(150.0..=340.0)
            .frame(Frame::none().fill(pal.view).inner_margin(Margin::same(6.0)))
            .show(ctx, |ui| {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        let cwd = self.pane().cwd.clone();

                        section(ui, crate::i18n::tr("Places"), pal);
                        for (name, path) in &self.places {
                            let r = place_row(ui, PlaceIcon::Folder, name, cwd == *path, None, pal);
                            handle_place(&r, path, false, &mut nav, &mut newtab, &mut remove);
                        }
                        for path in &self.cfg.places {
                            let r = place_row(
                                ui,
                                PlaceIcon::Folder,
                                &display_name(path),
                                cwd == *path,
                                None,
                                pal,
                            );
                            handle_place(&r, path, true, &mut nav, &mut newtab, &mut remove);
                        }

                        ui.add_space(8.0);
                        section(ui, crate::i18n::tr("Devices"), pal);
                        // This PC: its full specification (Settings › System › About), like Windows.
                        let pc = place_row(ui, PlaceIcon::Computer, crate::i18n::tr("This PC"), false, None, pal)
                            .on_hover_text(crate::i18n::tr("Specification of this PC"));
                        pc.context_menu(|ui| {
                            if ui.button(crate::i18n::tr("Properties")).clicked() {
                                open_pc_properties();
                                ui.close_menu();
                            }
                        });
                        if pc.clicked() {
                            open_pc_properties();
                        }
                        for d in &self.drives {
                            let cap = (d.total > 0).then(|| 1.0 - d.free as f32 / d.total as f32);
                            let r =
                                place_row(ui, PlaceIcon::Drive, &d.label, cwd == d.root, cap, pal);
                            let r = if d.total > 0 {
                                r.on_hover_text(format!(
                                    "{} free of {}",
                                    fsops::fmt_size(d.free),
                                    fsops::fmt_size(d.total)
                                ))
                            } else {
                                r
                            };
                            handle_place(&r, &d.root, false, &mut nav, &mut newtab, &mut remove);
                        }
                    });
            });

        if let Some(p) = nav {
            self.with_pane(|pane, c| pane.navigate(p, c));
        }
        if let Some(p) = newtab {
            self.new_tab(p);
        }
        if let Some(p) = remove {
            self.cfg.places.retain(|q| *q != p);
        }
    }

    fn info_panel(&mut self, ctx: &egui::Context, pal: &Palette) {
        egui::SidePanel::right("info")
            .resizable(true)
            .default_width(250.0)
            .width_range(180.0..=420.0)
            .frame(
                Frame::none()
                    .fill(pal.window)
                    .inner_margin(Margin::same(12.0)),
            )
            .show(ctx, |ui| {
                let p = self.pane();
                let sel: Vec<&Entry> = p
                    .visible
                    .iter()
                    .map(|&i| &p.entries[i])
                    .filter(|e| p.selected.contains(&e.path))
                    .collect();

                ui.vertical_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(128.0, 128.0), Sense::hover());
                    let painter = ui.painter();
                    let title = match sel.len() {
                        0 => {
                            icons::paint_folder(painter, rect, icons::FOLDER);
                            display_name(&p.cwd)
                        }
                        1 => {
                            crate::fileicons::paint(painter, rect, &sel[0].path, sel[0].is_dir, &sel[0].ext, pal);
                            sel[0].name.clone()
                        }
                        n => {
                            let small = rect.shrink(18.0);
                            icons::paint_entry_icon(
                                painter,
                                small.translate(vec2(-12.0, -12.0)),
                                sel[1].is_dir,
                                &sel[1].ext,
                                pal,
                            );
                            icons::paint_entry_icon(
                                painter,
                                small.translate(vec2(12.0, 12.0)),
                                sel[0].is_dir,
                                &sel[0].ext,
                                pal,
                            );
                            crate::i18n::trf("{} items selected", &[&n])
                        }
                    };
                    ui.add_space(8.0);
                    ui.label(RichText::new(title).strong().size(15.0));
                });
                ui.add_space(12.0);

                let field = |ui: &mut Ui, k: &str, v: String| {
                    ui.label(RichText::new(k).color(pal.dim).size(12.0));
                    ui.label(v);
                    ui.add_space(6.0);
                };
                match sel.len() {
                    0 => {
                        let nd = p.visible.iter().filter(|&&i| p.entries[i].is_dir).count();
                        let nf = p.visible.len() - nd;
                        field(ui, crate::i18n::tr("Type"), crate::i18n::tr("Folder").into());
                        field(ui, crate::i18n::tr("Contents"), crate::i18n::trf("{} folders, {} files", &[&nd, &nf]));
                        field(ui, crate::i18n::tr("Location"), p.cwd.display().to_string());
                    }
                    1 => {
                        let e = sel[0];
                        field(ui, crate::i18n::tr("Type"), icons::type_name(&e.ext, e.is_dir));
                        if !e.is_dir {
                            field(
                                ui,
                                crate::i18n::tr("Size"),
                                format!("{} ({} bytes)", fsops::fmt_size(e.size), e.size),
                            );
                        }
                        field(ui, crate::i18n::tr("Modified"), fsops::fmt_time(e.modified));
                        field(
                            ui,
                            crate::i18n::tr("Location"),
                            e.path
                                .parent()
                                .map(|x| x.display().to_string())
                                .unwrap_or_default(),
                        );
                        if e.hidden {
                            field(ui, crate::i18n::tr("Attributes"), crate::i18n::tr("Hidden").into());
                        }
                    }
                    _ => {
                        let total: u64 = sel.iter().filter(|e| !e.is_dir).map(|e| e.size).sum();
                        field(ui, crate::i18n::tr("Total size of files"), fsops::fmt_size(total));
                    }
                }
            });
    }

    fn central(&mut self, ctx: &egui::Context, pal: &Palette) {
        let cut: Vec<PathBuf> = match &self.clip {
            Some(c) if c.cut => c.paths.clone(),
            _ => Vec::new(),
        };
        let can_paste = self.clip.is_some();
        let mut outs: Vec<(usize, PaneOut)> = Vec::new();

        egui::CentralPanel::default()
            .frame(Frame::none().fill(pal.view))
            .show(ctx, |ui| {
                let cfg = &self.cfg;
                let tab = &mut self.tabs[self.cur];
                let active = tab.active;
                if tab.panes.len() == 1 {
                    let o = pane_view(ui, &mut tab.panes[0], 0, false, cfg, pal, &cut, can_paste);
                    outs.push((0, o));
                } else {
                    ui.columns(2, |cols| {
                        for (i, col) in cols.iter_mut().enumerate() {
                            let o = pane_view(
                                col,
                                &mut tab.panes[i],
                                i,
                                i == active,
                                cfg,
                                pal,
                                &cut,
                                can_paste,
                            );
                            outs.push((i, o));
                        }
                    });
                }
            });

        for (i, o) in outs {
            if o.activated || !o.actions.is_empty() {
                self.tabs[self.cur].active = i;
            }
            for a in o.actions {
                self.apply(ctx, a);
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let mut close = false;
        let mut result: Option<DialogResult> = None;
        let pal = theme::palette(self.cfg.dark);
        let esc = ctx.input(|i| i.key_pressed(Key::Escape));

        let window = |title: &str| {
            egui::Window::new(title.to_owned())
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        };

        match &mut self.dialog {
            Dialog::None => return,
            Dialog::Rename { path, text, focus } => {
                window(crate::i18n::tr("Rename")).show(ctx, |ui| {
                    ui.label(crate::i18n::trf(
                        "New name for \u{201c}{}\u{201d}:",
                        &[&display_name(path)],
                    ));
                    // Pre-select the name without the extension, like Dolphin.
                    let sel_len = if path.is_dir() {
                        text.chars().count()
                    } else {
                        Path::new(text.as_str())
                            .file_stem()
                            .map(|s| s.to_string_lossy().chars().count())
                            .unwrap_or_else(|| text.chars().count())
                    };
                    let mut out = egui::TextEdit::singleline(&mut *text)
                        .desired_width(340.0)
                        .show(ui);
                    if *focus {
                        out.response.request_focus();
                        out.state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(sel_len),
                            )));
                        out.state.store(ui.ctx(), out.response.id);
                        *focus = false;
                    }
                    let enter =
                        out.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(crate::i18n::tr("Rename")).clicked() || enter {
                            result = Some(DialogResult::Rename(path.clone(), text.clone()));
                            close = true;
                        }
                        if ui.button(crate::i18n::tr("Cancel")).clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::NewFolder { dir, text, focus } => {
                window(crate::i18n::tr("Create New Folder")).show(ctx, |ui| {
                    ui.label(crate::i18n::trf("Create new folder in {}:", &[&dir.display()]));
                    let mut out = egui::TextEdit::singleline(&mut *text)
                        .desired_width(340.0)
                        .show(ui);
                    if *focus {
                        out.response.request_focus();
                        let n = text.chars().count();
                        out.state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(n),
                            )));
                        out.state.store(ui.ctx(), out.response.id);
                        *focus = false;
                    }
                    let enter =
                        out.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() || enter {
                            result = Some(DialogResult::NewFolder(dir.clone(), text.clone()));
                            close = true;
                        }
                        if ui.button(crate::i18n::tr("Cancel")).clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::ConfirmDelete { paths } => {
                window(crate::i18n::tr("Delete Permanently")).show(ctx, |ui| {
                    ui.label(crate::i18n::trf("Permanently delete {} item(s)? This cannot be undone.", &[&paths.len()]));
                    for p in paths.iter().take(6) {
                        ui.label(RichText::new(format!("  • {}", display_name(p))).color(pal.dim));
                    }
                    if paths.len() > 6 {
                        ui.label(
                            RichText::new(crate::i18n::trf("  … and {} more", &[&(paths.len() - 6)]))
                                .color(pal.dim),
                        );
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button(RichText::new(crate::i18n::tr("Delete")).color(pal.danger))
                            .clicked()
                        {
                            result = Some(DialogResult::Delete(paths.clone()));
                            close = true;
                        }
                        if ui.button(crate::i18n::tr("Cancel")).clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::Shell { install } => {
                let install = *install;
                let title = if install {
                    crate::i18n::tr("Use Delfin as Windows Shell")
                } else {
                    crate::i18n::tr("Restore Windows Explorer")
                };
                window(title).show(ctx, |ui| {
                    ui.set_max_width(440.0);
                    if install {
                        ui.label(
                            "Delfin will replace explorer.exe for your user account: bottom panel \
                             with app launcher and task list, desktop with your wallpaper, and \
                             Delfin as the folder window.",
                        );
                        ui.add_space(6.0);
                        ui.label(crate::i18n::tr("It takes effect after you sign out and sign back in. No admin rights needed."));
                        ui.add_space(6.0);
                        ui.label(RichText::new(crate::i18n::tr("How to go back:")).strong());
                        ui.label("• Launcher → \"Back to Explorer\" (click twice)");
                        ui.label(
                            "• Emergency: Ctrl+Alt+Del → Task Manager → Run new task → explorer.exe, \
                             then run delfin.exe --uninstall-shell",
                        );
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(
                                "Tip: try it first without installing: close Explorer \
                                 (taskkill /f /im explorer.exe) and run delfin.exe --shell.",
                            )
                            .color(pal.dim),
                        );
                    } else {
                        ui.label(crate::i18n::tr("Windows Explorer will be your shell again after you sign out and back in."));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let ok = if install { crate::i18n::tr("Use Delfin as shell") } else { crate::i18n::tr("Restore Explorer") };
                        if ui.button(RichText::new(ok).strong()).clicked() {
                            result = Some(DialogResult::Shell(install));
                            close = true;
                        }
                        if ui.button(crate::i18n::tr("Cancel")).clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::About => {
                window(crate::i18n::tr("About Delfin")).show(ctx, |ui| {
                    ui.label(RichText::new("Delfin 0.1").strong().size(16.0));
                    ui.label("A KDE Dolphin-style file manager written in Rust + egui.");
                    ui.add_space(6.0);
                    if ui.button(crate::i18n::tr("Close")).clicked() {
                        close = true;
                    }
                });
            }
        }
        if esc {
            close = true;
        }
        if close {
            self.dialog = Dialog::None;
        }

        match result {
            Some(DialogResult::Rename(path, name)) => {
                let name = name.trim().to_string();
                if name.is_empty() || name.contains(|c: char| c == '/' || c == '\\') {
                    self.err(crate::i18n::tr("Invalid name"));
                    return;
                }
                let new = path.with_file_name(&name);
                if new == path {
                    return;
                }
                let only_case =
                    new.to_string_lossy().to_lowercase() == path.to_string_lossy().to_lowercase();
                if new.exists() && !only_case {
                    self.err(crate::i18n::tr("An item with that name already exists"));
                    return;
                }
                match std::fs::rename(&path, &new) {
                    Ok(()) => {
                        self.reload_all();
                        self.select_path(&new);
                    }
                    Err(e) => self.err(crate::i18n::trf("Rename failed: {}", &[&e])),
                }
            }
            Some(DialogResult::NewFolder(dir, name)) => {
                let name = name.trim().to_string();
                if name.is_empty() || name.contains(|c: char| c == '/' || c == '\\') {
                    self.err(crate::i18n::tr("Invalid name"));
                    return;
                }
                let target = dir.join(&name);
                match std::fs::create_dir(&target) {
                    Ok(()) => {
                        self.reload_all();
                        self.select_path(&target);
                    }
                    Err(e) => self.err(crate::i18n::trf("Cannot create folder: {}", &[&e])),
                }
            }
            Some(DialogResult::Shell(install)) => {
                let r = if install {
                    platform::install_shell()
                } else {
                    platform::uninstall_shell()
                };
                match r {
                    Ok(msg) => {
                        self.info(if install {
                            crate::i18n::tr("Delfin set as shell. Sign out and back in.")
                        } else {
                            crate::i18n::tr("Explorer restored.")
                        });
                        platform::message("Delfin", &msg);
                    }
                    Err(e) => self.err(e),
                }
            }
            Some(DialogResult::Delete(paths)) => {
                self.spawn(ctx, move || fsops::delete_permanently(&paths));
            }
            None => {}
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.applied_dark != self.cfg.dark {
            theme::apply(ctx, self.cfg.dark);
            self.applied_dark = self.cfg.dark;
        }
        let pal = theme::palette(self.cfg.dark);

        self.poll_jobs();
        self.handle_keys(ctx);

        self.toolbar(ctx, &pal);
        if self.tabs.len() > 1 {
            self.tabbar(ctx, &pal);
        }
        self.statusbar(ctx, &pal);
        if self.cfg.places_panel {
            self.places_panel(ctx, &pal);
        }
        if self.cfg.info_panel {
            self.info_panel(ctx, &pal);
        }
        self.central(ctx, &pal);
        self.dialogs(ctx);

        let title = format!("{} — Delfin", display_name(&self.pane().cwd));
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        if self.cfg != self.saved_cfg && !ctx.input(|i| i.pointer.any_down()) {
            self.cfg.save();
            self.saved_cfg = self.cfg.clone();
        }
    }
}

// =====================================================================
// Free-standing drawing helpers
// =====================================================================

fn chevron(ui: &mut Ui, col: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(10.0, 20.0), Sense::hover());
    let c = r.center();
    ui.painter().add(Shape::line(
        vec![
            c + vec2(-2.0, -4.0),
            c + vec2(2.0, 0.0),
            c + vec2(-2.0, 4.0),
        ],
        Stroke::new(1.2_f32, col),
    ));
}

fn section(ui: &mut Ui, title: &str, pal: &Palette) {
    ui.add_space(2.0);
    ui.label(RichText::new(title).color(pal.dim).size(12.0));
    ui.add_space(2.0);
}

enum PlaceIcon {
    Folder,
    Drive,
    Computer,
}

fn place_row(
    ui: &mut Ui,
    icon: PlaceIcon,
    label: &str,
    active: bool,
    capacity: Option<f32>,
    pal: &Palette,
) -> egui::Response {
    let w = ui.available_width();
    let h = if capacity.is_some() { 32.0 } else { 26.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if active {
            p.rect_filled(rect, 4.0, pal.select);
            p.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, pal.accent));
        } else if resp.hovered() {
            p.rect_filled(rect, 4.0, pal.hover);
        }
        let ir =
            Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), vec2(18.0, 18.0));
        match icon {
            PlaceIcon::Folder => icons::paint_folder(p, ir, icons::FOLDER),
            PlaceIcon::Drive => icons::paint_drive(p, ir),
            PlaceIcon::Computer => icons::paint_tool(p, ir, icons::Tool::Desktop, pal.text),
        }
        let text_rect =
            Rect::from_min_max(pos2(rect.left() + 30.0, rect.top()), rect.right_bottom());
        let ty = if capacity.is_some() {
            rect.center().y - 4.0
        } else {
            rect.center().y
        };
        p.with_clip_rect(text_rect).text(
            pos2(text_rect.left(), ty),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(13.5),
            pal.text,
        );
        if let Some(used) = capacity {
            let bar = Rect::from_min_max(
                pos2(rect.left() + 30.0, rect.bottom() - 8.0),
                pos2(rect.right() - 8.0, rect.bottom() - 5.0),
            );
            p.rect_filled(bar, 1.5, pal.border);
            let fill = Rect::from_min_size(
                bar.min,
                vec2(bar.width() * used.clamp(0.0, 1.0), bar.height()),
            );
            p.rect_filled(fill, 1.5, if used > 0.9 { pal.danger } else { pal.accent });
        }
    }
    resp
}

fn handle_place(
    r: &egui::Response,
    path: &Path,
    removable: bool,
    nav: &mut Option<PathBuf>,
    newtab: &mut Option<PathBuf>,
    remove: &mut Option<PathBuf>,
) {
    if r.clicked() {
        *nav = Some(path.to_path_buf());
    }
    if r.middle_clicked() {
        *newtab = Some(path.to_path_buf());
    }
    r.context_menu(|ui| {
        if ui.button(crate::i18n::tr("Open in New Tab")).clicked() {
            *newtab = Some(path.to_path_buf());
            ui.close_menu();
        }
        if removable && ui.button(crate::i18n::tr("Remove from Places")).clicked() {
            *remove = Some(path.to_path_buf());
            ui.close_menu();
        }
    });
}

fn paint_item_bg(p: &egui::Painter, rect: Rect, selected: bool, hovered: bool, pal: &Palette) {
    if selected {
        p.rect_filled(rect, 4.0, pal.select);
        p.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, pal.accent));
    } else if hovered {
        p.rect_filled(rect, 4.0, pal.hover);
        p.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, pal.accent.gamma_multiply(0.6)));
    }
}

fn dim_rect(p: &egui::Painter, r: Rect, pal: &Palette) {
    p.rect_filled(r.expand(1.0), 0.0, pal.view.gamma_multiply(0.55));
}

fn layout_text(
    ui: &Ui,
    text: &str,
    width: f32,
    rows: usize,
    center: bool,
    color: Color32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(
        text.to_owned(),
        FontId::proportional(13.0),
        color,
        width.max(10.0),
    );
    job.wrap.max_rows = rows;
    job.wrap.overflow_character = Some('…');
    if center {
        job.halign = Align::Center;
    }
    ui.fonts(|f| f.layout_job(job))
}

fn mi(ui: &mut Ui, label: &str, shortcut: &str) -> bool {
    let clicked = ui
        .add(egui::Button::new(label).shortcut_text(shortcut))
        .clicked();
    if clicked {
        ui.close_menu();
    }
    clicked
}

/// Archive entries: unpack archives, pack anything. `pack` is the name the new archive gets.
#[cfg(windows)]
fn archive_menu(ui: &mut Ui, path: &Path, pack: &str, out: &mut PaneOut) {
    use crate::archive;
    use crate::i18n::{tr, trf};
    let seven = archive::seven_zip().is_some();
    ui.separator();
    if archive::is_archive(path) {
        if mi(ui, tr("Extract Here"), "") {
            out.actions.push(Action::Extract(true));
        }
        if mi(ui, &trf("Extract to \"{}\\\"", &[&archive::base_name(path)]), "") {
            out.actions.push(Action::Extract(false));
        }
        if seven && mi(ui, tr("Open in 7-Zip"), "") {
            out.actions.push(Action::OpenIn7Zip(path.to_path_buf()));
        }
    }
    if mi(ui, &trf("Add to \"{}\"", &[&format!("{pack}.zip")]), "") {
        out.actions.push(Action::Compress("zip"));
    }
    if seven && mi(ui, &trf("Add to \"{}\"", &[&format!("{pack}.7z")]), "") {
        out.actions.push(Action::Compress("7z"));
    }
}

fn item_menu(ui: &mut Ui, path: &Path, is_dir: bool, pack: &str, out: &mut PaneOut) {
    ui.set_min_width(230.0);
    if mi(ui, crate::i18n::tr("Open"), "Enter") {
        out.actions.push(Action::OpenSelected);
    }
    if is_dir {
        if mi(ui, crate::i18n::tr("Open in New Tab"), crate::i18n::tr("Middle click")) {
            out.actions.push(Action::NewTab(path.to_path_buf()));
        }
        if mi(ui, crate::i18n::tr("Open Terminal Here"), "") {
            out.actions.push(Action::Terminal(path.to_path_buf()));
        }
    }
    ui.separator();
    if mi(ui, crate::i18n::tr("Cut"), "Ctrl+X") {
        out.actions.push(Action::Cut);
    }
    if mi(ui, crate::i18n::tr("Copy"), "Ctrl+C") {
        out.actions.push(Action::Copy);
    }
    if mi(ui, crate::i18n::tr("Copy Location"), "") {
        out.actions.push(Action::CopyPath);
    }
    #[cfg(windows)]
    archive_menu(ui, path, pack, out);
    #[cfg(not(windows))]
    let _ = pack;
    ui.separator();
    if mi(ui, crate::i18n::tr("Rename…"), "F2") {
        out.actions.push(Action::Rename);
    }
    if mi(ui, crate::i18n::tr("Move to Trash"), "Del") {
        out.actions.push(Action::Trash);
    }
    if mi(ui, crate::i18n::tr("Delete"), "Shift+Del") {
        out.actions.push(Action::Delete);
    }
    ui.separator();
    if is_dir && mi(ui, crate::i18n::tr("Add to Places"), "") {
        out.actions.push(Action::AddPlace(path.to_path_buf()));
    }
    #[cfg(windows)]
    {
        // Programs and shortcuts can go on the Delfin panel.
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !is_dir && ["exe", "lnk", "url", "appref-ms"].contains(&ext.as_str()) {
            use crate::shell::pins;
            if pins::is_pinned(path) {
                if mi(ui, crate::i18n::tr("Unpin from taskbar"), "") {
                    pins::unpin_path(path);
                    ui.close_menu();
                }
            } else if mi(ui, crate::i18n::tr("Pin to taskbar"), "") {
                let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                pins::pin_path(&name, path);
                ui.close_menu();
            }
        }
    }
    if mi(ui, crate::i18n::tr("Information Panel"), "F11") {
        out.actions.push(Action::ShowInfo);
    }
    #[cfg(windows)]
    if mi(ui, crate::i18n::tr("Properties"), "Alt+Enter") {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).arg("--props").arg(path).spawn();
        }
        ui.close_menu();
    }
}

fn background_menu(ui: &mut Ui, cwd: &Path, cfg: &Config, can_paste: bool, out: &mut PaneOut) {
    ui.set_min_width(230.0);
    if mi(ui, crate::i18n::tr("Create New Folder…"), "Ctrl+Shift+N") {
        out.actions.push(Action::NewFolder);
    }
    if ui
        .add_enabled(
            can_paste,
            egui::Button::new(crate::i18n::tr("Paste")).shortcut_text("Ctrl+V"),
        )
        .clicked()
    {
        out.actions.push(Action::Paste);
        ui.close_menu();
    }
    ui.separator();
    if mi(ui, crate::i18n::tr("Open Terminal Here"), "F4") {
        out.actions.push(Action::Terminal(cwd.to_path_buf()));
    }
    if mi(ui, crate::i18n::tr("Copy Location"), "") {
        out.actions.push(Action::CopyPath);
    }
    if mi(ui, crate::i18n::tr("Add to Places"), "") {
        out.actions.push(Action::AddPlace(cwd.to_path_buf()));
    }
    ui.separator();
    let mut hidden = cfg.show_hidden;
    if ui.checkbox(&mut hidden, crate::i18n::tr("Show Hidden Files")).clicked() {
        out.actions.push(Action::ToggleHidden);
        ui.close_menu();
    }
    if mi(ui, crate::i18n::tr("Select All"), "Ctrl+A") {
        out.actions.push(Action::SelectAll);
    }
    if mi(ui, crate::i18n::tr("Refresh"), "F5") {
        out.actions.push(Action::Refresh);
    }
}

/// Selection, opening and context menu for one item.
fn handle_item(ui: &Ui, pane: &mut Pane, vi: usize, resp: &egui::Response, out: &mut PaneOut) {
    let (path, is_dir) = {
        let e = pane.ent(vi);
        (e.path.clone(), e.is_dir)
    };
    let selected = pane.selected.contains(&path);
    if resp.clicked() {
        let m = ui.input(|i| i.modifiers);
        pane.click_select(vi, m);
        out.activated = true;
    }
    if resp.double_clicked() {
        out.actions.push(Action::Open(path.clone(), is_dir));
    }
    if resp.middle_clicked() && is_dir {
        out.actions.push(Action::NewTab(path.clone()));
    }
    if resp.secondary_clicked() {
        if !selected {
            pane.select_only(vi);
        }
        out.activated = true;
    }
    resp.context_menu(|ui| {
        // Name a new archive would get: the item's own, or the folder's for several items.
        let pack = if pane.selected.len() > 1 {
            pane.cwd.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| crate::i18n::tr("Archive").into())
        } else if is_dir {
            display_name(&path)
        } else {
            path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        };
        item_menu(ui, &path, is_dir, &pack, out)
    });
}

#[allow(clippy::too_many_arguments)]
fn pane_view(
    ui: &mut Ui,
    pane: &mut Pane,
    idx: usize,
    highlight: bool,
    cfg: &Config,
    pal: &Palette,
    cut: &[PathBuf],
    can_paste: bool,
) -> PaneOut {
    let mut out = PaneOut::default();
    let full = ui.available_rect_before_wrap();
    if highlight {
        ui.painter().line_segment(
            [full.left_top(), full.right_top()],
            Stroke::new(2.0_f32, pal.accent),
        );
    }

    // Background: registered first so items drawn later sit on top of it.
    let bg = ui.interact(full, ui.id().with(("pane_bg", idx)), Sense::click());
    if bg.clicked() || bg.secondary_clicked() {
        pane.selected.clear();
        pane.anchor = None;
        out.activated = true;
    }
    let cwd = pane.cwd.clone();
    bg.context_menu(|ui| background_menu(ui, &cwd, cfg, can_paste, &mut out));

    Frame::none()
        .inner_margin(Margin::same(6.0))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
            if let Some(err) = &pane.error {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        RichText::new(crate::i18n::trf("Cannot open this folder:\n{}", &[&err])).color(pal.danger),
                    )
                });
                return;
            }
            if pane.visible.is_empty() {
                let msg = if pane.filter.is_empty() {
                    crate::i18n::tr("Folder is empty")
                } else {
                    crate::i18n::tr("No items match the filter")
                };
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new(msg).color(pal.dim).size(16.0))
                });
                return;
            }
            match cfg.view {
                ViewMode::Icons => grid_view(ui, pane, idx, cfg, pal, cut, &mut out, false),
                ViewMode::Compact => grid_view(ui, pane, idx, cfg, pal, cut, &mut out, true),
                ViewMode::Details => details_view(ui, pane, idx, cfg, pal, cut, &mut out),
            }
        });
    out
}

/// Compute a scroll offset that brings row `row` into view.
fn scroll_target(pane: &Pane, row: usize, row_h: f32) -> f32 {
    let top = row as f32 * row_h;
    let mut off = pane.scroll_offset;
    if top < off {
        off = top;
    } else if top + row_h > off + pane.viewport_h {
        off = top + row_h - pane.viewport_h;
    }
    off.max(0.0)
}

#[allow(clippy::too_many_arguments)]
fn grid_view(
    ui: &mut Ui,
    pane: &mut Pane,
    idx: usize,
    cfg: &Config,
    pal: &Palette,
    cut: &[PathBuf],
    out: &mut PaneOut,
    compact: bool,
) {
    let sp = ui.spacing().item_spacing;
    let isz = if compact {
        (cfg.icon_size * 0.45).clamp(16.0, 48.0)
    } else {
        cfg.icon_size
    };
    let line_h = 16.0;
    let min_w = if compact {
        230.0
    } else {
        (isz + 44.0).max(96.0)
    };
    let cell_h = if compact {
        isz + 8.0
    } else {
        isz + 14.0 + line_h * 2.0 + 4.0
    };
    let avail = ui.available_width() - 14.0; // leave room for the scrollbar
    let cols = (((avail + sp.x) / (min_w + sp.x)).floor() as usize).max(1);
    let cell_w = if compact {
        min_w
    } else {
        ((avail - sp.x * (cols as f32 - 1.0)) / cols as f32).floor()
    };
    pane.cols = cols;

    let n = pane.visible.len();
    let rows = n.div_ceil(cols);
    let row_h = cell_h + sp.y;

    let mut sa = ScrollArea::vertical()
        .id_source(("grid", idx))
        .auto_shrink([false, false]);
    if let Some(t) = pane.scroll_to.take() {
        sa = sa.vertical_scroll_offset(scroll_target(pane, t / cols, row_h));
    }
    let o = sa.show_rows(ui, cell_h, rows, |ui, range| {
        for r in range {
            ui.horizontal(|ui| {
                for c in 0..cols {
                    let vi = r * cols + c;
                    if vi >= n {
                        break;
                    }
                    let (rect, resp) = ui.allocate_exact_size(vec2(cell_w, cell_h), Sense::click());
                    if ui.is_rect_visible(rect) {
                        let e = pane.ent(vi);
                        let selected = pane.selected.contains(&e.path);
                        let dim = e.hidden || cut.contains(&e.path);
                        let p = ui.painter();
                        paint_item_bg(p, rect, selected, resp.hovered(), pal);
                        if compact {
                            let ir = Rect::from_min_size(
                                pos2(rect.left() + 4.0, rect.center().y - isz / 2.0),
                                vec2(isz, isz),
                            );
                            crate::fileicons::paint(p, ir, &e.path, e.is_dir, &e.ext, pal);
                            if dim {
                                dim_rect(p, ir, pal);
                            }
                            let tx = ir.right() + 6.0;
                            let g = layout_text(
                                ui,
                                &e.name,
                                rect.right() - tx - 4.0,
                                1,
                                false,
                                pal.text,
                            );
                            p.galley(pos2(tx, rect.center().y - g.size().y / 2.0), g, pal.text);
                        } else {
                            let ir = Rect::from_center_size(
                                pos2(rect.center().x, rect.top() + 6.0 + isz / 2.0),
                                vec2(isz, isz),
                            );
                            crate::fileicons::paint(p, ir, &e.path, e.is_dir, &e.ext, pal);
                            if dim {
                                dim_rect(p, ir, pal);
                            }
                            let g = layout_text(ui, &e.name, rect.width() - 8.0, 2, true, pal.text);
                            p.galley(pos2(rect.center().x, ir.bottom() + 6.0), g, pal.text);
                        }
                    }
                    let name = pane.ent(vi).name.clone();
                    let resp = resp.on_hover_text(name);
                    handle_item(ui, pane, vi, &resp, out);
                }
            });
        }
    });
    pane.scroll_offset = o.state.offset.y;
    pane.viewport_h = o.inner_rect.height();
}

fn details_view(
    ui: &mut Ui,
    pane: &mut Pane,
    idx: usize,
    cfg: &Config,
    pal: &Palette,
    cut: &[PathBuf],
    out: &mut PaneOut,
) {
    let w = ui.available_width() - 14.0;
    let (size_w, date_w, type_w) = (90.0, 140.0, 140.0);
    let name_w = (w - size_w - date_w - type_w).max(160.0);
    let columns = [
        (SortBy::Name, crate::i18n::tr("Name"), name_w),
        (SortBy::Size, crate::i18n::tr("Size"), size_w),
        (SortBy::Modified, crate::i18n::tr("Modified"), date_w),
        (SortBy::Type, crate::i18n::tr("Type"), type_w),
    ];

    // Header (click to sort, click again to reverse)
    let (hrect, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
    let mut x = hrect.left();
    for (i, (k, label, cw)) in columns.iter().enumerate() {
        let r = Rect::from_min_size(pos2(x, hrect.top()), vec2(*cw, 24.0));
        let resp = ui.interact(r, ui.id().with(("hdr", idx, i)), Sense::click());
        let p = ui.painter();
        if resp.hovered() {
            p.rect_filled(r, 2.0, pal.hover);
        }
        if i > 0 {
            p.line_segment(
                [
                    r.left_top() + vec2(0.0, 4.0),
                    r.left_bottom() - vec2(0.0, 4.0),
                ],
                Stroke::new(1.0_f32, pal.border),
            );
        }
        let right = *k == SortBy::Size;
        if right {
            p.text(
                pos2(r.right() - 8.0, r.center().y),
                Align2::RIGHT_CENTER,
                label,
                FontId::proportional(12.5),
                pal.dim,
            );
        } else {
            p.text(
                pos2(r.left() + 8.0, r.center().y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(12.5),
                pal.dim,
            );
        }
        if cfg.sort == *k {
            let cx = if right {
                r.left() + 10.0
            } else {
                r.right() - 12.0
            };
            let cy = r.center().y;
            let pts = if cfg.sort_asc {
                vec![
                    pos2(cx - 4.0, cy + 2.0),
                    pos2(cx + 4.0, cy + 2.0),
                    pos2(cx, cy - 3.0),
                ]
            } else {
                vec![
                    pos2(cx - 4.0, cy - 2.0),
                    pos2(cx + 4.0, cy - 2.0),
                    pos2(cx, cy + 3.0),
                ]
            };
            p.add(Shape::convex_polygon(pts, pal.dim, Stroke::NONE));
        }
        if resp.clicked() {
            out.actions.push(Action::Sort(*k));
        }
        x += cw;
    }
    ui.painter().line_segment(
        [hrect.left_bottom(), hrect.right_bottom()],
        Stroke::new(1.0_f32, pal.border),
    );

    // Rows
    let row_h = 24.0;
    let n = pane.visible.len();
    let sp = ui.spacing().item_spacing;
    let mut sa = ScrollArea::vertical()
        .id_source(("details", idx))
        .auto_shrink([false, false]);
    if let Some(t) = pane.scroll_to.take() {
        sa = sa.vertical_scroll_offset(scroll_target(pane, t, row_h + sp.y));
    }
    let o = sa.show_rows(ui, row_h, n, |ui, range| {
        for vi in range {
            let (rect, resp) = ui.allocate_exact_size(vec2(w, row_h), Sense::click());
            if ui.is_rect_visible(rect) {
                let e = pane.ent(vi);
                let selected = pane.selected.contains(&e.path);
                let p = ui.painter();
                if vi % 2 == 1 {
                    p.rect_filled(rect, 0.0, pal.alt_row);
                }
                paint_item_bg(p, rect, selected, resp.hovered(), pal);

                let cy = rect.center().y;
                let font = FontId::proportional(13.0);
                let ir = Rect::from_center_size(pos2(rect.left() + 14.0, cy), vec2(18.0, 18.0));
                crate::fileicons::paint(p, ir, &e.path, e.is_dir, &e.ext, pal);
                if e.hidden || cut.contains(&e.path) {
                    dim_rect(p, ir, pal);
                }

                let mut x = rect.left();
                let name_clip = Rect::from_min_max(
                    pos2(x + 28.0, rect.top()),
                    pos2(x + name_w - 6.0, rect.bottom()),
                );
                p.with_clip_rect(name_clip).text(
                    pos2(name_clip.left(), cy),
                    Align2::LEFT_CENTER,
                    &e.name,
                    font.clone(),
                    pal.text,
                );
                x += name_w;

                if !e.is_dir {
                    p.text(
                        pos2(x + size_w - 8.0, cy),
                        Align2::RIGHT_CENTER,
                        fsops::fmt_size(e.size),
                        font.clone(),
                        pal.dim,
                    );
                }
                x += size_w;

                p.text(
                    pos2(x + 8.0, cy),
                    Align2::LEFT_CENTER,
                    fsops::fmt_time(e.modified),
                    font.clone(),
                    pal.dim,
                );
                x += date_w;

                let type_clip = Rect::from_min_max(
                    pos2(x + 8.0, rect.top()),
                    pos2(x + type_w - 4.0, rect.bottom()),
                );
                p.with_clip_rect(type_clip).text(
                    pos2(type_clip.left(), cy),
                    Align2::LEFT_CENTER,
                    icons::type_name(&e.ext, e.is_dir),
                    font,
                    pal.dim,
                );
            }
            handle_item(ui, pane, vi, &resp, out);
        }
    });
    pane.scroll_offset = o.state.offset.y;
    pane.viewport_h = o.inner_rect.height();
}

/// This PC › Properties: Delfin's Settings at System › About.
#[cfg(windows)]
fn open_pc_properties() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).args(["--settings", "about"]).spawn();
    }
}

#[cfg(not(windows))]
fn open_pc_properties() {}
