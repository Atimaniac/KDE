//! Apps pinned to the panel. Kept in `%APPDATA%\delfin\pinned.txt`, one per
//! line: `P<TAB>name<TAB>path` (a program, shortcut, …) or
//! `C<TAB>name<TAB>command` (a built-in such as the file manager).
//! The file manager can add pins too; the panel notices the file changing.

use super::startmenu::{AppEntry, Launch};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone)]
pub struct Pin {
    pub name: String,
    pub target: Launch,
    /// The .exe whose windows group under this pin (lower case), if known.
    pub exe: Option<String>,
}

fn file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("delfin").join("pinned.txt"))
}

pub fn modified() -> Option<SystemTime> {
    file().and_then(|f| std::fs::metadata(f).ok()).and_then(|m| m.modified().ok())
}

fn key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

/// The program a pin starts, to find its windows.
fn exe_of(target: &Launch) -> Option<String> {
    match target {
        Launch::Path(p) => {
            let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            match ext.as_str() {
                "exe" => Some(key(p)),
                "lnk" => super::startmenu::shortcut_info(p).and_then(|(_, t)| t).map(|t| key(&t)),
                _ => None,
            }
        }
        // Windows Terminal runs as WindowsTerminal.exe.
        Launch::Cmd(c) if c.eq_ignore_ascii_case("wt.exe") => Some("\\windowsterminal.exe".into()),
        Launch::Cmd(_) => None,
    }
}

impl Pin {
    pub fn new(name: String, target: Launch) -> Self {
        let exe = exe_of(&target);
        Pin { name, target, exe }
    }

    pub fn from_app(app: &AppEntry) -> Self {
        Pin::new(app.name.clone(), app.target.clone())
    }

    /// Does this window's program belong to this pin?
    pub fn owns(&self, window_exe: &str) -> bool {
        match &self.exe {
            Some(e) if e.starts_with('\\') => window_exe.ends_with(e.as_str()),
            Some(e) => window_exe == e,
            None => false,
        }
    }

    pub fn same_target(&self, t: &Launch) -> bool {
        match (&self.target, t) {
            (Launch::Path(a), Launch::Path(b)) => key(a) == key(b),
            (Launch::Cmd(a), Launch::Cmd(b)) => a.eq_ignore_ascii_case(b),
            _ => false,
        }
    }
}

pub fn load() -> Vec<Pin> {
    let Some(text) = file().and_then(|f| std::fs::read_to_string(f).ok()) else {
        // First run: the file manager and the terminal, as before.
        return vec![
            Pin::new("Delfin".into(), Launch::Cmd("delfin".into())),
            Pin::new(crate::i18n::tr("Terminal").into(), Launch::Cmd("wt.exe".into())),
        ];
    };
    text.lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\t');
            let (kind, name, val) = (it.next()?, it.next()?, it.next()?);
            let target = match kind {
                "P" => Launch::Path(PathBuf::from(val)),
                "C" => Launch::Cmd(val.to_string()),
                _ => return None,
            };
            Some(Pin::new(name.to_string(), target))
        })
        .collect()
}

pub fn save(pins: &[Pin]) {
    let Some(f) = file() else { return };
    let _ = std::fs::create_dir_all(f.parent().unwrap_or(Path::new(".")));
    let text: String = pins
        .iter()
        .map(|p| match &p.target {
            Launch::Path(x) => format!("P\t{}\t{}\n", p.name, x.display()),
            Launch::Cmd(c) => format!("C\t{}\t{}\n", p.name, c),
        })
        .collect();
    let _ = std::fs::write(f, text);
}

/// Add a program to the pins file from outside the shell (file manager).
pub fn pin_path(name: &str, path: &Path) {
    let mut pins = load();
    let t = Launch::Path(path.to_path_buf());
    if !pins.iter().any(|p| p.same_target(&t)) {
        pins.push(Pin::new(name.to_string(), t));
        save(&pins);
    }
}

/// Is this path pinned (for the file manager's menu)?
pub fn is_pinned(path: &Path) -> bool {
    let t = Launch::Path(path.to_path_buf());
    load().iter().any(|p| p.same_target(&t))
}

pub fn unpin_path(path: &Path) {
    let t = Launch::Path(path.to_path_buf());
    let mut pins = load();
    pins.retain(|p| !p.same_target(&t));
    save(&pins);
}
