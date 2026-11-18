//! OS-specific bits: drives, free space, opening files, terminal.

use std::path::{Path, PathBuf};

pub struct Drive {
    pub root: PathBuf,
    pub label: String,
    pub total: u64,
    pub free: u64,
}

/// Open a file with its default application.
pub fn open(p: &Path) -> Result<(), String> {
    open::that(p).map_err(|e| format!("Cannot open {}: {e}", p.display()))
}

// ---------------------------------------------------------------- Windows
#[cfg(windows)]
mod imp {
    use super::Drive;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::ptr::null_mut;
    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    };
    use windows_sys::Win32::System::Diagnostics::Debug::{SetErrorMode, SEM_FAILCRITICALERRORS};

    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }

    pub fn init() {
        // Don't pop up "There is no disk in the drive" dialogs.
        unsafe {
            SetErrorMode(SEM_FAILCRITICALERRORS);
        }
    }

    /// (free bytes available, total bytes)
    pub fn space(p: &Path) -> Option<(u64, u64)> {
        let w = wide(p.as_os_str());
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, &mut total, &mut free) };
        if ok != 0 {
            Some((avail, total))
        } else {
            None
        }
    }

    pub fn drives() -> Vec<Drive> {
        let mask = unsafe { GetLogicalDrives() };
        let mut out = Vec::new();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let letter = (b'A' + i as u8) as char;
            let root = format!("{letter}:\\");
            let w: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
            let dtype = unsafe { GetDriveTypeW(w.as_ptr()) };
            let mut name = [0u16; 261];
            let ok = unsafe {
                GetVolumeInformationW(
                    w.as_ptr(),
                    name.as_mut_ptr(),
                    name.len() as u32,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    0,
                )
            };
            let mut label = if ok != 0 {
                let n = name.iter().position(|&c| c == 0).unwrap_or(0);
                String::from_utf16_lossy(&name[..n])
            } else {
                String::new()
            };
            if label.is_empty() {
                label = match dtype {
                    2 => "Removable Disk",
                    4 => "Network Drive",
                    5 => "CD/DVD Drive",
                    _ => "Local Disk",
                }
                .to_string();
            }
            let (free, total) = space(Path::new(&root)).unwrap_or((0, 0));
            out.push(Drive {
                root: PathBuf::from(&root),
                label: format!("{label} ({letter}:)"),
                total,
                free,
            });
        }
        out
    }

    pub fn open_terminal(dir: &Path) -> Result<(), String> {
        // Windows Terminal first (closest to Konsole), then PowerShell.
        if Command::new("wt.exe").arg("-d").arg(dir).spawn().is_ok() {
            return Ok(());
        }
        Command::new("powershell.exe")
            .current_dir(dir)
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Cannot open terminal: {e}"))
    }
}

// ---------------------------------------------------------------- Others
#[cfg(not(windows))]
mod imp {
    use super::Drive;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    pub fn init() {}

    pub fn space(_p: &Path) -> Option<(u64, u64)> {
        None
    }

    pub fn drives() -> Vec<Drive> {
        vec![Drive {
            root: PathBuf::from("/"),
            label: "Root (/)".into(),
            total: 0,
            free: 0,
        }]
    }

    pub fn open_terminal(dir: &Path) -> Result<(), String> {
        for term in ["konsole", "x-terminal-emulator", "gnome-terminal", "xterm"] {
            if Command::new(term).current_dir(dir).spawn().is_ok() {
                return Ok(());
            }
        }
        Err("No terminal emulator found".into())
    }
}

pub use imp::{drives, init, open_terminal, space};

// ---------------------------------------------------------------- shell install
#[cfg(windows)]
pub use crate::shell::install::{
    folder_handler_set, install_shell, set_folder_handler, shell_installed, uninstall_shell,
};

#[cfg(not(windows))]
mod shell_stub {
    pub fn shell_installed() -> bool {
        false
    }
    pub fn folder_handler_set() -> bool {
        false
    }
    pub fn install_shell() -> Result<String, String> {
        Err("Only available on Windows".into())
    }
    pub fn uninstall_shell() -> Result<String, String> {
        Err("Only available on Windows".into())
    }
    pub fn set_folder_handler(_on: bool) -> Result<(), String> {
        Err("Only available on Windows".into())
    }
}
#[cfg(not(windows))]
pub use shell_stub::*;

/// Show a message box (the release build has no console).
pub fn message(title: &str, text: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW;
        let w = |s: &str| -> Vec<u16> {
            std::ffi::OsStr::new(s)
                .encode_wide()
                .chain(Some(0))
                .collect()
        };
        let (t, c) = (w(text), w(title));
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                t.as_ptr(),
                c.as_ptr(),
                0x40, /* MB_ICONINFORMATION */
            );
        }
    }
    #[cfg(not(windows))]
    println!("{title}: {text}");
}
