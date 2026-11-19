//! Explorer normally starts "startup apps" at sign-in. Without Explorer we do it.
//! Respects entries disabled in Task Manager → Startup apps.

use super::win;
use std::path::{Path, PathBuf};
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
use winreg::RegKey;

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN32: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

/// Task Manager stores a binary flag; an odd first byte means "disabled".
fn disabled(list: &str, name: &str) -> bool {
    let path = format!(r"{APPROVED}\{list}");
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].iter().any(|&root| {
        RegKey::predef(root)
            .open_subkey(&path)
            .and_then(|k| k.get_raw_value(name))
            .map(|v| v.bytes.first().is_some_and(|b| b & 1 == 1))
            .unwrap_or(false)
    })
}

fn run_key(root: &RegKey, path: &str, approved_list: &str) {
    let Ok(k) = root.open_subkey(path) else {
        return;
    };
    let names: Vec<String> = k
        .enum_values()
        .filter_map(|v| v.ok())
        .map(|(n, _)| n)
        .collect();
    for name in names {
        if disabled(approved_list, &name) {
            continue;
        }
        if let Ok(cmd) = k.get_value::<String, _>(&name) {
            let cmd = cmd.trim();
            if !cmd.is_empty() {
                let _ = win::run_cmdline(cmd);
            }
        }
    }
}

fn run_folder(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.eq_ignore_ascii_case("desktop.ini") || disabled("StartupFolder", &name) {
            continue;
        }
        super::win::shell_open(e.path());
    }
}

pub fn run_startup_items() {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    run_key(&hkcu, RUN, "Run");
    run_key(&hklm, RUN, "Run");
    run_key(&hklm, RUN32, "Run32");

    if let Some(roaming) = dirs::config_dir() {
        run_folder(&roaming.join(r"Microsoft\Windows\Start Menu\Programs\Startup"));
    }
    if let Ok(pd) = std::env::var("ProgramData") {
        run_folder(&PathBuf::from(pd).join(r"Microsoft\Windows\Start Menu\Programs\StartUp"));
    }
}
