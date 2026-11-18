//! Thin Win32 helpers used by the shell.
//!
//! Window handles are stored as `isize` so they can live in plain Rust structs.

use eframe::egui;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr::{null, null_mut};
use std::sync::mpsc::Sender;

use windows_sys::Win32::Foundation::{GetLastError, BOOL, HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
};
use windows_sys::Win32::System::Registry::{RegCloseKey, RegCreateKeyExW, HKEY, HKEY_CURRENT_USER};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::RegisterHotKey;
use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, EnumWindows, FindWindowW, GetClassLongPtrW, GetForegroundWindow, GetIconInfo,
    GetMessageW, GetSystemMetrics, GetWindow, GetWindowLongPtrW, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, PostMessageW,
    SendMessageTimeoutW, SetForegroundWindow, ShowWindow, SystemParametersInfoW, HICON, ICONINFO,
    MSG,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[link(name = "user32")]
extern "system" {
    // Exported by user32 but not in the SDK headers. Registers our desktop as
    // "the shell window" so a manually started explorer.exe only opens a folder
    // window instead of taking over the taskbar.
    fn SetShellWindow(hwnd: HWND) -> BOOL;
}

pub fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

// ----------------------------------------------------------------- windows

#[derive(Clone)]
pub struct TaskWindow {
    pub hwnd: isize,
    pub title: String,
    pub minimized: bool,
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = &mut *(lparam as *mut Vec<isize>);
    list.push(hwnd as isize);
    1
}

/// Windows that belong on a taskbar (same rules Explorer uses, roughly).
pub fn task_windows(own_pid: u32) -> Vec<TaskWindow> {
    let mut all: Vec<isize> = Vec::new();
    unsafe {
        EnumWindows(Some(enum_cb), &mut all as *mut Vec<isize> as LPARAM);
    }
    let mut out = Vec::new();
    for h in all {
        let hwnd = h as HWND;
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                continue;
            }
            let ex = GetWindowLongPtrW(hwnd, -20 /* GWL_EXSTYLE */) as u32;
            let app_window = ex & 0x0004_0000 != 0; // WS_EX_APPWINDOW
            let tool_window = ex & 0x0000_0080 != 0; // WS_EX_TOOLWINDOW
            let no_activate = ex & 0x0800_0000 != 0; // WS_EX_NOACTIVATE
            let owner = GetWindow(hwnd, 4 /* GW_OWNER */);
            if !app_window && (tool_window || no_activate || !owner.is_null()) {
                continue;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == own_pid {
                continue;
            }
            // Hidden UWP / virtual-desktop windows are "cloaked".
            let mut cloaked: u32 = 0;
            DwmGetWindowAttribute(
                hwnd,
                14, /* DWMWA_CLOAKED */
                &mut cloaked as *mut u32 as *mut c_void,
                4,
            );
            if cloaked != 0 {
                continue;
            }
            let len = GetWindowTextLengthW(hwnd);
            if len <= 0 {
                continue;
            }
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            let title = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
            out.push(TaskWindow {
                hwnd: h,
                title,
                minimized: IsIconic(hwnd) != 0,
            });
        }
    }
    out
}

pub fn foreground() -> isize {
    unsafe { GetForegroundWindow() as isize }
}

pub fn pid_of(h: isize) -> u32 {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(h as HWND, &mut pid);
    }
    pid
}

pub fn activate(h: isize) {
    let hwnd = h as HWND;
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, 9 /* SW_RESTORE */);
        }
        SetForegroundWindow(hwnd);
    }
}

pub fn minimize(h: isize) {
    unsafe {
        ShowWindow(h as HWND, 6 /* SW_MINIMIZE */);
    }
}

pub fn restore(h: isize) {
    unsafe {
        ShowWindow(h as HWND, 9 /* SW_RESTORE */);
    }
}

pub fn close(h: isize) {
    unsafe {
        PostMessageW(h as HWND, 0x0010 /* WM_CLOSE */, 0, 0);
    }
}

/// Hide a window completely (used for minimize-to-tray).
pub fn hide(h: isize) {
    unsafe {
        ShowWindow(h as HWND, 0 /* SW_HIDE */);
    }
}

/// Bring back a window hidden with `hide`.
pub fn unhide(h: isize) {
    let hwnd = h as HWND;
    unsafe {
        ShowWindow(hwnd, 5 /* SW_SHOW */);
        ShowWindow(hwnd, 9 /* SW_RESTORE */);
        SetForegroundWindow(hwnd);
    }
}

pub fn is_window(h: isize) -> bool {
    unsafe { IsWindow(h as HWND) != 0 }
}

pub fn find_class(class: &str) -> Option<isize> {
    let w = wide(class);
    let h = unsafe { FindWindowW(w.as_ptr(), null()) };
    (!h.is_null()).then_some(h as isize)
}

pub fn find_window(title: &str) -> Option<isize> {
    let w = wide(title);
    let h = unsafe { FindWindowW(null(), w.as_ptr()) };
    (!h.is_null()).then_some(h as isize)
}

pub fn set_shell_window(h: isize) {
    unsafe {
        SetShellWindow(h as HWND);
    }
}

// ----------------------------------------------------------------- screen

/// Primary monitor size in physical pixels.
pub fn screen_px() -> (i32, i32) {
    unsafe {
        (
            GetSystemMetrics(0 /* SM_CXSCREEN */),
            GetSystemMetrics(1 /* SM_CYSCREEN */),
        )
    }
}

/// Tell Windows that the bottom `reserved_px` belong to our panel, so maximized
/// windows don't cover it.
pub fn set_work_area(reserved_px: i32) {
    let (w, h) = screen_px();
    let mut r = RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h - reserved_px,
    };
    unsafe {
        SystemParametersInfoW(
            0x002F, /* SPI_SETWORKAREA */
            0,
            &mut r as *mut RECT as *mut c_void,
            0x02, /* SPIF_SENDCHANGE */
        );
    }
}

pub fn wallpaper_path() -> Option<PathBuf> {
    let mut buf = [0u16; 520];
    let ok = unsafe {
        SystemParametersInfoW(
            0x0073, /* SPI_GETDESKWALLPAPER */
            buf.len() as u32,
            buf.as_mut_ptr() as *mut c_void,
            0,
        )
    };
    if ok != 0 {
        let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
        let s = String::from_utf16_lossy(&buf[..n]);
        if !s.is_empty() && Path::new(&s).exists() {
            return Some(PathBuf::from(s));
        }
    }
    // Windows keeps a converted copy here (file has no extension).
    let t = dirs::config_dir()?.join(r"Microsoft\Windows\Themes\TranscodedWallpaper");
    t.exists().then_some(t)
}

// ----------------------------------------------------------------- icons

/// Icon handle of a window (not owned, don't destroy).
pub fn window_icon(h: isize) -> isize {
    let hwnd = h as HWND;
    unsafe {
        for kind in [
            2usize, /* ICON_SMALL2 */
            0,      /* ICON_SMALL */
            1,      /* ICON_BIG */
        ] {
            let mut r: usize = 0;
            let ok = SendMessageTimeoutW(
                hwnd, 0x007F, /* WM_GETICON */
                kind, 0, 0x0002, /* SMTO_ABORTIFHUNG */
                50, &mut r,
            );
            if ok != 0 && r != 0 {
                return r as isize;
            }
        }
        let c = GetClassLongPtrW(hwnd, -34 /* GCLP_HICONSM */);
        if c != 0 {
            return c as isize;
        }
        GetClassLongPtrW(hwnd, -14 /* GCLP_HICON */) as isize
    }
}

unsafe fn bitmap_rgba(hbm: *mut c_void) -> Option<(usize, usize, Vec<u8>)> {
    let mut bm: BITMAP = std::mem::zeroed();
    if GetObjectW(
        hbm,
        std::mem::size_of::<BITMAP>() as i32,
        &mut bm as *mut BITMAP as *mut c_void,
    ) == 0
    {
        return None;
    }
    let (w, h) = (bm.bmWidth, bm.bmHeight);
    if w <= 0 || h <= 0 || w > 256 || h > 256 {
        return None;
    }
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h; // top-down
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = 0; // BI_RGB
    let mut px = vec![0u8; (w * h * 4) as usize];
    let dc = GetDC(null_mut());
    let lines = GetDIBits(
        dc,
        hbm,
        0,
        h as u32,
        px.as_mut_ptr() as *mut c_void,
        &mut bi,
        0,
    );
    ReleaseDC(null_mut(), dc);
    if lines == 0 {
        return None;
    }
    let has_alpha = px.chunks(4).any(|p| p[3] != 0);
    for p in px.chunks_mut(4) {
        p.swap(0, 2); // BGRA -> RGBA
        if !has_alpha {
            p[3] = 255;
        }
    }
    Some((w as usize, h as usize, px))
}

/// Convert an HICON to RGBA pixels.
pub fn icon_rgba(hicon: isize) -> Option<(usize, usize, Vec<u8>)> {
    if hicon == 0 {
        return None;
    }
    unsafe {
        let mut ii: ICONINFO = std::mem::zeroed();
        if GetIconInfo(hicon as HICON, &mut ii) == 0 {
            return None;
        }
        let result = if ii.hbmColor.is_null() {
            None
        } else {
            bitmap_rgba(ii.hbmColor)
        };
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask);
        }
        result
    }
}

/// The icon Windows shows for a file (works for .lnk, .exe, documents, folders).
pub fn file_icon_rgba(path: &Path, large: bool) -> Option<(usize, usize, Vec<u8>)> {
    let w = wide(path.as_os_str());
    unsafe {
        let mut info: SHFILEINFOW = std::mem::zeroed();
        let flags: u32 = 0x100 /* SHGFI_ICON */ | (if large { 0 } else { 1 /* SHGFI_SMALLICON */ });
        let r = SHGetFileInfoW(
            w.as_ptr(),
            0,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        );
        if r == 0 || info.hIcon.is_null() {
            return None;
        }
        let px = icon_rgba(info.hIcon as isize);
        DestroyIcon(info.hIcon);
        px
    }
}

// ----------------------------------------------------------------- process

/// One copy of the shell per session.
pub fn single_instance(name: &str) -> bool {
    let w = wide(name);
    unsafe {
        let h = CreateMutexW(null(), 1, w.as_ptr());
        !h.is_null() && GetLastError() != 183 /* ERROR_ALREADY_EXISTS */
    }
}

/// True only the first time the shell starts after sign-in. Uses a volatile
/// registry key, which Windows deletes automatically at sign-out.
pub fn first_shell_start_this_session() -> bool {
    let sub = wide(r"Software\Delfin\Session");
    let mut hk: HKEY = null_mut();
    let mut disposition: u32 = 0;
    unsafe {
        let r = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            0,
            null(),
            1,       /* REG_OPTION_VOLATILE */
            0x20019, /* KEY_READ */
            null(),
            &mut hk,
            &mut disposition,
        );
        if r != 0 {
            return false;
        }
        RegCloseKey(hk);
    }
    disposition == 1 /* REG_CREATED_NEW_KEY */
}

/// Start another Delfin window (file manager), optionally in a folder.
pub fn spawn_self(arg: Option<&Path>) {
    if let Ok(exe) = std::env::current_exe() {
        let mut c = Command::new(exe);
        if let Some(a) = arg {
            c.arg(a);
        }
        let _ = c.spawn();
    }
}

/// Run a command line the way the Run dialog does (`start "" <cmd>`).
pub fn run_cmdline(cmd: &str) -> std::io::Result<()> {
    Command::new("cmd.exe")
        .raw_arg(format!("/C start \"\" {cmd}"))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
}

/// Run a command with no console window (for `shutdown`, `rundll32`, …).
pub fn run_hidden(cmd: &str) {
    let _ = Command::new("cmd.exe")
        .raw_arg(format!("/C {cmd}"))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

// ----------------------------------------------------------------- hotkeys

#[derive(Clone, Copy)]
pub enum Hotkey {
    FileManager,
    Launcher,
    ShowDesktop,
}

/// Global shortcuts. They are free once explorer.exe isn't running.
pub fn spawn_hotkeys(tx: Sender<Hotkey>, ctx: egui::Context) {
    std::thread::spawn(move || unsafe {
        const MOD_ALT: u32 = 0x1;
        const MOD_WIN: u32 = 0x8;
        const NOREPEAT: u32 = 0x4000;
        let keys: [(i32, u32, u32); 4] = [
            (1, MOD_WIN | NOREPEAT, b'E' as u32), // Win+E  → file manager
            (2, MOD_ALT | NOREPEAT, 0x70),        // Alt+F1 → launcher (KDE default)
            (3, MOD_WIN | NOREPEAT, b'D' as u32), // Win+D  → show desktop
            (4, MOD_WIN | NOREPEAT, b'R' as u32), // Win+R  → launcher (run)
        ];
        for (id, m, vk) in keys {
            RegisterHotKey(null_mut(), id, m, vk);
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if msg.message == 0x0312 {
                let k = match msg.wParam {
                    1 => Hotkey::FileManager,
                    2 | 4 => Hotkey::Launcher,
                    3 => Hotkey::ShowDesktop,
                    _ => continue,
                };
                let _ = tx.send(k);
                ctx.request_repaint();
            }
        }
    });
}
