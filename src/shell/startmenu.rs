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
        });
    }
}

pub fn scan() -> Vec<AppEntry> {
    let mut out = vec![
        builtin("Delfin (file manager)", "delfin", Tool::Home),
        builtin("Settings", "ms-settings:", Tool::Gear),
        builtin("Terminal", "wt.exe", Tool::Terminal),
        builtin("Task Manager", "taskmgr.exe", Tool::Details),
        builtin("Control Panel", "control.exe", Tool::Gear),
        builtin("Calculator", "calc.exe", Tool::Icons),
        builtin("Notepad", "notepad.exe", Tool::Compact),
        builtin("Windows Explorer (folder window)", "explorer.exe", Tool::Up),
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

fn builtin(name: &str, cmd: &str, icon: Tool) -> AppEntry {
    AppEntry {
        name: name.into(),
        folder: "System".into(),
        target: Launch::Cmd(cmd.into()),
        icon: Some(icon),
    }
}
