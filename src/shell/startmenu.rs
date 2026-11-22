//! Application list for the launcher: Start Menu shortcuts + a few built-ins.

use crate::fsops::natural_cmp;
use crate::icons::Tool;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub enum Launch {
    Path(PathBuf),
    Cmd(String),
}

#[derive(Clone)]
pub struct AppEntry {
    pub name: String,
    pub folder: String,
    pub target: Launch,
    /// Painted icon for built-ins (others use the real file icon).
    pub icon: Option<Tool>,
    /// One-line description ("Web browser"); filled in the background, may stay empty.
    pub desc: String,
}

fn walk(dir: &Path, rel: &str, depth: usize, out: &mut Vec<AppEntry>) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            let sub = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel} / {name}")
            };
            walk(&path, &sub, depth + 1, out);
            continue;
        }
        let ext = path
            .extension()
            .map(|x| x.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !matches!(ext.as_str(), "lnk" | "url" | "exe" | "appref-ms") {
            continue;
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or(name);
        let lower = stem.to_lowercase();
        if lower.contains("uninstall") || lower.contains("odinstaluj") || lower == "desktop" {
            continue;
        }
        out.push(AppEntry {
            name: stem,
            folder: rel.to_string(),
            target: Launch::Path(path),
            icon: None,
            desc: String::new(),
        });
    }
}

pub fn scan() -> Vec<AppEntry> {
    let mut out = vec![
        builtin("Delfin", "delfin", Tool::Home, crate::i18n::tr("File manager")),
        builtin(crate::i18n::tr("Settings"), "ms-settings:", Tool::Gear, crate::i18n::tr("System, devices, network, apps, updates")),
        builtin(crate::i18n::tr("Terminal"), "wt.exe", Tool::Terminal, crate::i18n::tr("Command line")),
        builtin(crate::i18n::tr("Task Manager"), "taskmgr.exe", Tool::Details, crate::i18n::tr("Running programs")),
        builtin(crate::i18n::tr("Device Manager"), "devmgmt.msc", Tool::Chip, crate::i18n::tr("Hardware and drivers")),
        builtin(crate::i18n::tr("Control Panel"), "control.exe", Tool::Gear, crate::i18n::tr("Classic settings")),
        builtin(crate::i18n::tr("Calculator"), "calc.exe", Tool::Icons, crate::i18n::tr("Calculator")),
        builtin(crate::i18n::tr("Notepad"), "notepad.exe", Tool::Compact, crate::i18n::tr("Text editor")),
        builtin(crate::i18n::tr("Windows Explorer"), "explorer.exe", Tool::Up, crate::i18n::tr("Folder window")),
    ];
    let mut found = Vec::new();
    if let Some(roaming) = dirs::config_dir() {
        walk(
            &roaming.join(r"Microsoft\Windows\Start Menu\Programs"),
            "",
            0,
            &mut found,
        );
    }
    if let Ok(pd) = std::env::var("ProgramData") {
        walk(
            &PathBuf::from(pd).join(r"Microsoft\Windows\Start Menu\Programs"),
            "",
            0,
            &mut found,
        );
    }
    found.sort_by(|a, b| natural_cmp(&a.name, &b.name));
    found.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    out.extend(found);
    out
}

/// Store apps (Photos, Terminal, Spotify from the Store, …) aren't shortcuts in
/// the Start Menu folders. PowerShell's Get-StartApps lists them; it takes a
/// second or two, so it runs in the background.
pub fn scan_store_apps(ctx: eframe::egui::Context) -> std::sync::mpsc::Receiver<Vec<AppEntry>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::os::windows::process::CommandExt;
        let script = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-StartApps | ForEach-Object { $_.Name + \"`t\" + $_.AppID }";
        let Ok(out) = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output()
        else {
            return;
        };
        let apps: Vec<AppEntry> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.split_once('\t'))
            // Packaged apps only ("Family!App"); the rest are shortcuts we already have.
            .filter(|(_, id)| id.contains('!'))
            .map(|(name, id)| AppEntry {
                name: name.trim().to_string(),
                folder: String::new(),
                target: Launch::Path(PathBuf::from(format!("shell:AppsFolder\\{}", id.trim()))),
                icon: None,
                desc: String::new(),
            })
            .collect();
        let _ = tx.send(apps);
        ctx.request_repaint();
    });
    rx
}

/// Add Store apps to the list, skipping names that are already there.
pub fn merge_store_apps(apps: &mut Vec<AppEntry>, store: Vec<AppEntry>) {
    for a in store {
        if !apps.iter().any(|x| x.name.eq_ignore_ascii_case(&a.name)) {
            apps.push(a);
        }
    }
    // Built-ins stay on top; everything else is sorted by name.
    let builtins = apps.iter().take_while(|a| a.icon.is_some()).count();
    apps[builtins..].sort_by(|a, b| natural_cmp(&a.name, &b.name));
}

fn builtin(name: &str, cmd: &str, icon: Tool, desc: &str) -> AppEntry {
    AppEntry {
        name: name.into(),
        folder: crate::i18n::tr("System").into(),
        target: Launch::Cmd(cmd.into()),
        icon: Some(icon),
        desc: desc.into(),
    }
}

// ------------------------------------------------------------------ categories

/// Groups on the left of the launcher. Windows doesn't store categories for
/// apps, so they're guessed from names and Start Menu folders.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    All,
    Office,
    Graphics,
    Internet,
    Multimedia,
    System,
    Games,
    Recent,
}

impl Category {
    pub const LIST: [Category; 8] = [
        Category::All,
        Category::Office,
        Category::Graphics,
        Category::Internet,
        Category::Multimedia,
        Category::System,
        Category::Games,
        Category::Recent,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::All => crate::i18n::tr("All Applications"),
            Category::Office => crate::i18n::tr("Office"),
            Category::Graphics => crate::i18n::tr("Graphics"),
            Category::Internet => crate::i18n::tr("Internet"),
            Category::Multimedia => crate::i18n::tr("Multimedia"),
            Category::System => crate::i18n::tr("System Tools"),
            Category::Games => crate::i18n::tr("Games"),
            Category::Recent => crate::i18n::tr("Recently Used"),
        }
    }
}

const KEYWORDS: &[(Category, &[&str])] = &[
    (Category::Games, &[
        "steam", "epic games", "gog galaxy", "battle.net", "ubisoft", "ea app", "origin", "riot",
        "minecraft", "xbox", "playnite", "heroic", "game", "gry", "gaming", "itch",
    ]),
    (Category::Internet, &[
        "firefox", "chrome", "edge", "brave", "opera", "vivaldi", "browser", "przegl", "discord",
        "telegram", "whatsapp", "signal", "slack", "teams", "zoom", "skype", "thunderbird",
        "outlook", "mail", "torrent", "filezilla", "putty", "messenger", "internet",
    ]),
    (Category::Multimedia, &[
        "vlc", "spotify", "media player", "mpc-hc", "mpv", "music", "muzyk", "audacity", "obs",
        "foobar", "winamp", "itunes", "netflix", "video", "film", "kodi", "plex", "handbrake",
        "player", "odtwarz", "audio", "sound", "multimedia",
    ]),
    (Category::Graphics, &[
        "gimp", "krita", "inkscape", "paint", "photoshop", "illustrator", "blender", "affinity",
        "lightroom", "irfanview", "xnview", "figma", "photo", "zdj", "draw", "graphic", "grafik",
        "image", "camera", "snipping", "wycin",
    ]),
    (Category::Office, &[
        "word", "excel", "powerpoint", "onenote", "office", "libreoffice", "writer", "calc",
        "impress", "notion", "obsidian", "acrobat", "pdf", "calendar", "kalendarz", "access",
        "publisher", "wps", "document", "dokument", "spreadsheet", "arkusz",
    ]),
    (Category::System, &[
        "terminal", "powershell", "command prompt", "wiersz polece", "task manager",
        "menedżer zadań", "device manager", "menedżer urządzeń", "control panel", "panel sterowania", "settings", "ustawienia",
        "regedit", "registry", "administrative", "windows tools", "narzędzia", "system",
        "notepad", "calculator", "kalkulator", "delfin", "explorer", "visual studio", "code",
        "git", "python", "node", "java", "driver", "nvidia", "amd", "intel", "backup", "7-zip",
        "winrar", "accessor", "akcesor", "utility", "tool",
    ]),
];

/// Best-guess category of an app, or `None` (it is still in All Applications).
pub fn category_of(app: &AppEntry) -> Option<Category> {
    if app.icon.is_some() {
        return Some(Category::System); // built-ins
    }
    let target = match &app.target {
        Launch::Path(p) => p.to_string_lossy().to_lowercase(),
        Launch::Cmd(c) => c.to_lowercase(),
    };
    let hay = format!(
        "{} | {} | {} | {}",
        app.name.to_lowercase(),
        app.folder.to_lowercase(),
        app.desc.to_lowercase(),
        target.rsplit(['\\', '/']).next().unwrap_or("")
    );
    KEYWORDS
        .iter()
        .find(|(_, words)| words.iter().any(|w| hay.contains(w)))
        .map(|(c, _)| *c)
}

/// Search: name starts with the text, then a word starts with it, then it
/// appears anywhere in the name, then in the description or folder.
pub fn search(apps: &[AppEntry], query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(u8, usize)> = apps
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            let n = a.name.to_lowercase();
            let score = if n.starts_with(&q) {
                0
            } else if n.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(&q)) {
                1
            } else if n.contains(&q) {
                2
            } else if a.desc.to_lowercase().contains(&q) || a.folder.to_lowercase().contains(&q) {
                3
            } else {
                return None;
            };
            Some((score, i))
        })
        .collect();
    scored.sort_by_key(|(s, i)| (*s, *i));
    scored.into_iter().map(|(_, i)| i).collect()
}

// ------------------------------------------------------------------ recently used

fn recent_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("delfin").join("recent-apps.txt"))
}

/// Names of recently launched apps, newest first.
pub fn load_recent() -> Vec<String> {
    recent_file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect())
        .unwrap_or_default()
}

pub fn push_recent(list: &mut Vec<String>, name: &str) {
    list.retain(|n| n != name);
    list.insert(0, name.to_string());
    list.truncate(10);
    if let Some(p) = recent_file() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, list.join("\n"));
    }
}

// ------------------------------------------------------------------ descriptions

/// Read app descriptions in the background: the shortcut's comment, or else the
/// program's own file description. Many apps have neither, so some stay empty.
/// Returns (shortcut path, description) pairs.
pub fn describe(
    paths: Vec<PathBuf>,
    ctx: eframe::egui::Context,
) -> std::sync::mpsc::Receiver<Vec<(PathBuf, String)>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        unsafe {
            windows_sys::Win32::System::Com::CoInitializeEx(std::ptr::null(), 0x2);
        }
        let mut out = Vec::new();
        for p in paths {
            let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let stem = p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            let desc = match ext.as_str() {
                "lnk" => shortcut_info(&p).and_then(|(comment, target)| {
                    let c = comment.trim().to_string();
                    if !c.is_empty() && !c.starts_with('@') && c.chars().count() < 80 {
                        Some(c)
                    } else {
                        target.and_then(|t| file_description(&t))
                    }
                }),
                "exe" => file_description(&p),
                _ => None,
            };
            if let Some(d) = desc {
                let d = d.trim().to_string();
                // An app described only by its own name says nothing.
                if !d.is_empty() && d.to_lowercase() != stem {
                    out.push((p, d));
                }
            }
        }
        let _ = tx.send(out);
        ctx.request_repaint();
    });
    rx
}

unsafe fn com_fn<T>(obj: *mut std::ffi::c_void, index: usize) -> T {
    let vtbl = *(obj as *const *const usize);
    std::mem::transmute_copy(&*vtbl.add(index))
}

unsafe fn com_release(obj: *mut std::ffi::c_void) {
    if !obj.is_null() {
        let f: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32 = com_fn(obj, 2);
        f(obj);
    }
}

/// (comment, target .exe) of a .lnk file, via IShellLinkW.
pub(crate) fn shortcut_info(lnk: &Path) -> Option<(String, Option<PathBuf>)> {
    use std::ffi::c_void;
    use windows_sys::core::GUID;
    use windows_sys::Win32::System::Com::CoCreateInstance;
    const CLSID_SHELLLINK: GUID = GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
    const IID_ISHELLLINKW: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
    const IID_IPERSISTFILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);
    unsafe {
        let mut link: *mut c_void = std::ptr::null_mut();
        if CoCreateInstance(&CLSID_SHELLLINK, std::ptr::null_mut(), 1, &IID_ISHELLLINKW, &mut link) < 0 || link.is_null() {
            return None;
        }
        let qi: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32 = com_fn(link, 0);
        let mut pf: *mut c_void = std::ptr::null_mut();
        let mut result = None;
        if qi(link, &IID_IPERSISTFILE, &mut pf) >= 0 && !pf.is_null() {
            // IPersistFile::Load
            let load: unsafe extern "system" fn(*mut c_void, *const u16, u32) -> i32 = com_fn(pf, 5);
            let w = super::win::wide(lnk.as_os_str());
            if load(pf, w.as_ptr(), 0) >= 0 {
                // IShellLinkW::GetDescription
                let mut desc = [0u16; 512];
                let get_desc: unsafe extern "system" fn(*mut c_void, *mut u16, i32) -> i32 = com_fn(link, 6);
                get_desc(link, desc.as_mut_ptr(), desc.len() as i32);
                // IShellLinkW::GetPath
                let mut path = [0u16; 1024];
                let get_path: unsafe extern "system" fn(*mut c_void, *mut u16, i32, *mut c_void, u32) -> i32 = com_fn(link, 3);
                let target = (get_path(link, path.as_mut_ptr(), path.len() as i32, std::ptr::null_mut(), 0) == 0)
                    .then(|| PathBuf::from(from_wide(&path)))
                    .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")));
                result = Some((from_wide(&desc), target));
            }
            com_release(pf);
        }
        com_release(link);
        result
    }
}

fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// FileDescription from a program's version information.
pub(crate) fn file_description(exe: &Path) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    let w = super::win::wide(exe.as_os_str());
    unsafe {
        let mut handle = 0u32;
        let n = GetFileVersionInfoSizeW(w.as_ptr(), &mut handle);
        if n == 0 {
            return None;
        }
        let mut buf = vec![0u8; n as usize];
        if GetFileVersionInfoW(w.as_ptr(), 0, n, buf.as_mut_ptr() as *mut _) == 0 {
            return None;
        }
        let mut p: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let tr = super::win::wide("\\VarFileInfo\\Translation");
        let (lang, cp) = if VerQueryValueW(buf.as_ptr() as *const _, tr.as_ptr(), &mut p, &mut len) != 0 && len >= 4 {
            let t = p as *const u16;
            (*t, *t.add(1))
        } else {
            (0x0409, 0x04b0)
        };
        let key = super::win::wide(format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\FileDescription"));
        if VerQueryValueW(buf.as_ptr() as *const _, key.as_ptr(), &mut p, &mut len) == 0 || len == 0 {
            return None;
        }
        let s = std::slice::from_raw_parts(p as *const u16, len as usize);
        Some(from_wide(s))
    }
}
