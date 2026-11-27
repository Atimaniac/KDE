//! What Windows Explorer does for files and Delfin has to do itself: the file
//! clipboard (the same one Explorer and other programs use, so files can be
//! copied between them, the desktop and any Delfin window), "Open with",
//! "Run as administrator", shortcuts and the wallpaper.

use std::path::{Path, PathBuf};

#[cfg(windows)]
mod imp {
    use std::ffi::{c_void, OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::ptr::{null, null_mut};
    use std::time::Duration;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CF_UNICODETEXT: u32 = 13;
    const CF_HDROP: u32 = 15;
    const GMEM_MOVEABLE: u32 = 0x0002;

    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(owner: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn SetClipboardData(format: u32, mem: *mut c_void) -> *mut c_void;
        fn GetClipboardData(format: u32) -> *mut c_void;
        fn IsClipboardFormatAvailable(format: u32) -> i32;
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
        fn GlobalLock(mem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(mem: *mut c_void) -> i32;
        fn GlobalFree(mem: *mut c_void) -> *mut c_void;
    }

    #[link(name = "shell32")]
    extern "system" {
        fn DragQueryFileW(drop: *mut c_void, index: u32, buf: *mut u16, len: u32) -> u32;
    }

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }

    /// Another program may hold the clipboard for a moment: try a few times.
    fn open_clipboard() -> bool {
        for _ in 0..10 {
            if unsafe { OpenClipboard(null_mut()) } != 0 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// A movable global block holding `bytes` (the clipboard takes it over).
    unsafe fn global_from(bytes: &[u8]) -> *mut c_void {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
        if h.is_null() {
            return h;
        }
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            GlobalFree(h);
            return null_mut();
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        GlobalUnlock(h);
        h
    }

    fn drop_effect_format() -> u32 {
        let name = wide(OsStr::new("Preferred DropEffect"));
        unsafe { RegisterClipboardFormatW(name.as_ptr()) }
    }

    pub fn has_files() -> bool {
        unsafe { IsClipboardFormatAvailable(CF_HDROP) != 0 }
    }

    /// Put files on the clipboard the way Explorer does: the file list, whether
    /// they were cut or copied, and their paths as text for other programs.
    pub fn set_files(paths: &[PathBuf], cut: bool) -> bool {
        if paths.is_empty() {
            return false;
        }
        // DROPFILES: a 20-byte header, then the paths, each ending in 0, and a final 0.
        let mut list: Vec<u16> = Vec::new();
        for p in paths {
            list.extend(p.as_os_str().encode_wide());
            list.push(0);
        }
        list.push(0);
        let mut drop: Vec<u8> = Vec::with_capacity(20 + list.len() * 2);
        drop.extend_from_slice(&20u32.to_le_bytes()); // pFiles: where the list starts
        drop.extend_from_slice(&[0u8; 8]); // pt
        drop.extend_from_slice(&0u32.to_le_bytes()); // fNC
        drop.extend_from_slice(&1u32.to_le_bytes()); // fWide
        for u in &list {
            drop.extend_from_slice(&u.to_le_bytes());
        }
        let text: String = paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\r\n");
        let text: Vec<u8> = text.encode_utf16().chain(Some(0)).flat_map(|u| u.to_le_bytes()).collect();
        // 2 = move (cut), 5 = copy or link (what Explorer writes for Copy).
        let effect: u32 = if cut { 2 } else { 5 };
        unsafe {
            if !open_clipboard() {
                return false;
            }
            EmptyClipboard();
            let h = global_from(&drop);
            let ok = !h.is_null() && !SetClipboardData(CF_HDROP, h).is_null();
            let t = global_from(&text);
            if !t.is_null() {
                SetClipboardData(CF_UNICODETEXT, t);
            }
            let fmt = drop_effect_format();
            if fmt != 0 {
                let e = global_from(&effect.to_le_bytes());
                if !e.is_null() {
                    SetClipboardData(fmt, e);
                }
            }
            CloseClipboard();
            ok
        }
    }

    /// The files on the clipboard and whether they were cut (to be moved).
    pub fn get_files() -> Option<(Vec<PathBuf>, bool)> {
        if !has_files() || !open_clipboard() {
            return None;
        }
        let mut out = Vec::new();
        let mut cut = false;
        unsafe {
            let h = GetClipboardData(CF_HDROP);
            if !h.is_null() {
                let n = DragQueryFileW(h, 0xFFFF_FFFF, null_mut(), 0);
                for i in 0..n {
                    let len = DragQueryFileW(h, i, null_mut(), 0) as usize;
                    let mut buf = vec![0u16; len + 1];
                    let got = DragQueryFileW(h, i, buf.as_mut_ptr(), buf.len() as u32) as usize;
                    out.push(PathBuf::from(OsString::from_wide(&buf[..got.min(len)])));
                }
            }
            let fmt = drop_effect_format();
            if fmt != 0 {
                let e = GetClipboardData(fmt);
                if !e.is_null() {
                    let p = GlobalLock(e) as *const u32;
                    if !p.is_null() {
                        // Move only, not "copy or move" (7), which some programs write.
                        let fx = p.read_unaligned();
                        cut = fx & 2 != 0 && fx & 1 == 0;
                        GlobalUnlock(e);
                    }
                }
            }
            CloseClipboard();
        }
        if out.is_empty() {
            None
        } else {
            Some((out, cut))
        }
    }

    /// Plain text on the clipboard.
    pub fn set_text(text: &str) -> bool {
        let bytes: Vec<u8> = text.encode_utf16().chain(Some(0)).flat_map(|u| u.to_le_bytes()).collect();
        unsafe {
            if !open_clipboard() {
                return false;
            }
            EmptyClipboard();
            let h = global_from(&bytes);
            let ok = !h.is_null() && !SetClipboardData(CF_UNICODETEXT, h).is_null();
            CloseClipboard();
            ok
        }
    }

    /// Empty the clipboard (after cut files were moved).
    pub fn clear_clipboard() {
        unsafe {
            if open_clipboard() {
                EmptyClipboard();
                CloseClipboard();
            }
        }
    }

    /// Windows' "Open with" window for a file.
    pub fn open_with(p: &Path) {
        // The rest of the command line is the path, taken as it is (no quotes).
        let _ = Command::new("rundll32.exe").raw_arg(format!("shell32.dll,OpenAs_RunDLL {}", p.display())).spawn();
    }

    /// Start a program with administrator rights (Windows asks first). On its
    /// own thread: the call only returns once that question is answered.
    pub fn run_as_admin(p: &Path) {
        let file = wide(p.as_os_str());
        let dir = p.parent().map(|d| wide(d.as_os_str()));
        std::thread::spawn(move || unsafe {
            use windows_sys::Win32::System::Com::CoInitializeEx;
            use windows_sys::Win32::UI::Shell::ShellExecuteW;
            CoInitializeEx(null(), 0x2 | 0x4);
            let verb = wide(OsStr::new("runas"));
            let dir_ptr = match &dir {
                Some(d) => d.as_ptr(),
                None => null(),
            };
            ShellExecuteW(null_mut(), verb.as_ptr(), file.as_ptr(), null(), dir_ptr, 1 /* SW_SHOWNORMAL */);
        });
    }

    /// A shortcut (.lnk) to `target` in `dir`; the path of the new shortcut.
    /// Takes a moment (PowerShell), so call it from a background job.
    pub fn create_shortcut(target: &Path, dir: &Path) -> Result<PathBuf, String> {
        let stem = target.file_stem().or(target.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Shortcut".into());
        let name = format!("{stem} \u{2013} {}.lnk", crate::i18n::tr("shortcut"));
        let lnk = crate::fsops::unique_dest(dir, OsStr::new(&name));
        let script = "$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:DELFIN_LNK); \
                      $s.TargetPath = $env:DELFIN_TARGET; \
                      $s.WorkingDirectory = $env:DELFIN_DIR; \
                      $s.Save()";
        let out = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("DELFIN_LNK", &lnk)
            .env("DELFIN_TARGET", target)
            .env("DELFIN_DIR", target.parent().unwrap_or(Path::new(".")))
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() && lnk.exists() {
            Ok(lnk)
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }

    /// Make a picture the desktop wallpaper (jpg, png or bmp).
    pub fn set_wallpaper(p: &Path) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;
        let mut w = wide(p.as_os_str());
        // SPI_SETDESKWALLPAPER, saved and announced to every window.
        unsafe { SystemParametersInfoW(0x0014, 0, w.as_mut_ptr() as *mut c_void, 0x01 | 0x02) != 0 }
    }

    /// Is this key held down right now (virtual-key code)?
    pub fn key_down(vk: i32) -> bool {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
        unsafe { GetAsyncKeyState(vk) < 0 }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::{Path, PathBuf};

    pub fn has_files() -> bool {
        false
    }
    pub fn set_files(_paths: &[PathBuf], _cut: bool) -> bool {
        false
    }
    pub fn get_files() -> Option<(Vec<PathBuf>, bool)> {
        None
    }
    pub fn clear_clipboard() {}
    pub fn set_text(_text: &str) -> bool {
        false
    }
    pub fn open_with(_p: &Path) {}
    pub fn run_as_admin(_p: &Path) {}
    pub fn create_shortcut(_target: &Path, _dir: &Path) -> Result<PathBuf, String> {
        Err("Only available on Windows".into())
    }
    pub fn set_wallpaper(_p: &Path) -> bool {
        false
    }
    pub fn key_down(_vk: i32) -> bool {
        false
    }
}

pub use imp::*;

/// Can Windows start this file with administrator rights?
pub fn can_run_as_admin(ext: &str) -> bool {
    matches!(ext, "exe" | "msi" | "bat" | "cmd" | "lnk" | "com" | "ps1" | "msc" | "cpl")
}

/// Pictures Windows accepts as a wallpaper.
pub fn can_be_wallpaper(ext: &str) -> bool {
    matches!(ext, "jpg" | "jpeg" | "png" | "bmp")
}

/// Plain text that Notepad can edit.
pub fn is_text(ext: &str) -> bool {
    matches!(
        ext,
        "txt" | "md" | "log" | "ini" | "cfg" | "conf" | "json" | "toml" | "yaml" | "yml" | "xml" | "csv" | "bat" | "cmd" | "ps1" | "reg" | "rs" | "py" | "js"
            | "ts" | "c" | "h" | "cpp" | "hpp" | "cs" | "java" | "go" | "html" | "css" | "sh" | "lua" | "sql" | "nfo" | "srt"
    )
}

/// Another Delfin window in this folder.
pub fn new_window(dir: &Path) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg(dir).spawn();
    }
}

/// Delfin's Properties window for a file or folder.
pub fn properties(p: &Path) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg("--props").arg(p).spawn();
    }
}

/// The small "new name" window for a file or folder (used by the desktop).
pub fn rename_window(p: &Path) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg("--rename").arg(p).spawn();
    }
}

/// An empty text file with a free name in `dir`.
pub fn new_text_file(dir: &Path) -> std::io::Result<PathBuf> {
    let name = format!("{}.txt", crate::i18n::tr("New Text Document"));
    let path = crate::fsops::unique_dest(dir, std::ffi::OsStr::new(&name));
    std::fs::write(&path, b"")?;
    Ok(path)
}

/// A new folder with a free name in `dir`.
pub fn new_folder(dir: &Path) -> std::io::Result<PathBuf> {
    let path = crate::fsops::unique_dest(dir, std::ffi::OsStr::new(crate::i18n::tr("New Folder")));
    std::fs::create_dir(&path)?;
    Ok(path)
}

/// A copy next to the original: "name (2).ext".
pub fn duplicate(paths: &[PathBuf]) -> Result<String, String> {
    let mut n = 0;
    for src in paths {
        let (Some(dir), Some(name)) = (src.parent(), src.file_name()) else { continue };
        let target = crate::fsops::unique_dest(dir, name);
        crate::fsops::copy_recursive(src, &target).map_err(|e| format!("{}: {e}", src.display()))?;
        n += 1;
    }
    Ok(crate::i18n::trf("Duplicated {} item(s)", &[&n]))
}
