//! Make Delfin the Windows shell (per user) and the default folder handler.
//!
//! Everything lives under HKEY_CURRENT_USER, needs no admin rights and is fully
//! reversible with `uninstall_shell()` / `delfin.exe --uninstall-shell`.

use std::path::PathBuf;
use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
use winreg::RegKey;

const WINLOGON: &str = r"Software\Microsoft\Windows NT\CurrentVersion\Winlogon";

fn e(err: std::io::Error) -> String {
    err.to_string()
}

/// Copy the exe to %LOCALAPPDATA%\Programs\Delfin so the registered path stays
/// valid even if the build folder changes. Falls back to the current exe.
fn installed_exe() -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(e)?;
    let Some(base) = dirs::data_local_dir() else {
        return Ok(current);
    };
    let dir = base.join("Programs").join("Delfin");
    let target = dir.join("delfin.exe");
    if current == target {
        return Ok(target);
    }
    let _ = std::fs::create_dir_all(&dir);
    match std::fs::copy(&current, &target) {
        Ok(_) => Ok(target),
        Err(_) if target.exists() => Ok(target), // in use by a running shell
        Err(_) => Ok(current),
    }
}

pub fn shell_installed() -> bool {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(WINLOGON)
        .and_then(|k| k.get_value::<String, _>("Shell"))
        .map(|s| s.contains("--shell"))
        .unwrap_or(false)
}

pub fn install_shell() -> Result<String, String> {
    let exe = installed_exe()?;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (k, _) = hkcu.create_subkey(WINLOGON).map_err(e)?;
    k.set_value("Shell", &format!("\"{}\" --shell", exe.display()))
        .map_err(e)?;
    set_folder_handler(true)?;
    Ok(format!(
        "Delfin is now your Windows shell.\n\n\
         Sign out and sign back in to start it.\n\n\
         To go back: open the launcher → \"Back to Explorer\".\n\
         Emergency: Ctrl+Alt+Del → Task Manager → Run new task → explorer.exe, \
         then run:\n\"{}\" --uninstall-shell",
        exe.display()
    ))
}

pub fn uninstall_shell() -> Result<String, String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(k) = hkcu.open_subkey_with_flags(WINLOGON, KEY_SET_VALUE) {
        match k.delete_value("Shell") {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(e(err)),
        }
    }
    set_folder_handler(false)?;
    Ok("Windows Explorer is the shell again. Sign out and back in to switch.".into())
}

pub fn folder_handler_set() -> bool {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Classes\Directory\shell")
        .and_then(|k| k.get_value::<String, _>(""))
        .map(|v| v == "delfin")
        .unwrap_or(false)
}

/// Double-clicking a folder or drive anywhere in Windows opens Delfin.
pub fn set_folder_handler(on: bool) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let exe = if on { Some(installed_exe()?) } else { None };
    for class in ["Directory", "Drive"] {
        let base = format!(r"Software\Classes\{class}\shell");
        if let Some(exe) = &exe {
            let (k, _) = hkcu.create_subkey(&base).map_err(e)?;
            k.set_value("", &"delfin").map_err(e)?;
            let (verb, _) = hkcu.create_subkey(format!(r"{base}\delfin")).map_err(e)?;
            verb.set_value("", &"Open in Delfin").map_err(e)?;
            let (cmd, _) = hkcu
                .create_subkey(format!(r"{base}\delfin\command"))
                .map_err(e)?;
            cmd.set_value("", &format!("\"{}\" \"%1\"", exe.display()))
                .map_err(e)?;
        } else {
            let _ = hkcu.delete_subkey_all(format!(r"{base}\delfin"));
            if let Ok(k) = hkcu.open_subkey_with_flags(&base, KEY_SET_VALUE) {
                let _ = k.delete_value("");
            }
        }
    }
    Ok(())
}
