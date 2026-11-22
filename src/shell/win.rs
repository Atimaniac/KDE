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

use windows_sys::Win32::Foundation::{GetLastError, BOOL, HWND, LPARAM, LRESULT, RECT, WPARAM};
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

/// Height in pixels of Explorer's taskbar if Explorer is running, else 0.
pub fn explorer_taskbar_px(own_pid: u32) -> i32 {
    let Some(h) = find_class("Shell_TrayWnd") else { return 0 };
    if pid_of(h) == own_pid {
        return 0; // that's our own tray window
    }
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(h as HWND, &mut r) };
    let (_, screen_h) = screen_px();
    if r.bottom >= screen_h && r.top > screen_h / 2 {
        r.bottom - r.top
    } else {
        0
    }
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

/// Show one of our pop-up windows (launcher, flyouts, tooltips) that egui
/// created hidden. Marking it a tool window *before* it first appears keeps
/// Windows from flashing a taskbar button for it. Returns false while the
/// window doesn't exist yet.
pub fn reveal_popup(title: &str, activate: bool) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW;
    let Some(h) = find_window(title) else { return false };
    let hw = h as HWND;
    unsafe {
        if IsWindowVisible(hw) != 0 {
            return true;
        }
        let ex = GetWindowLongPtrW(hw, -20 /* GWL_EXSTYLE */);
        let ex = (ex | 0x80 /* WS_EX_TOOLWINDOW */) & !0x4_0000 /* WS_EX_APPWINDOW */;
        SetWindowLongPtrW(hw, -20, ex);
        round_corners(h);
        ShowWindow(hw, if activate { 5 /* SW_SHOW */ } else { 4 /* SW_SHOWNOACTIVATE */ });
        if activate {
            SetForegroundWindow(hw);
        }
    }
    true
}

pub fn set_shell_window(h: isize) {
    unsafe {
        SetShellWindow(h as HWND);
    }
}

// ----------------------------------------------------------------- screen

/// Primary monitor size in physical pixels.
/// Does this window cover the whole primary screen without a title bar
/// (a game, a full-screen video, a slideshow)? The desktop doesn't count.
pub fn is_fullscreen(h: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindowRect};
    if h == 0 {
        return false;
    }
    let hw = h as HWND;
    unsafe {
        let mut cls = [0u16; 64];
        let n = GetClassNameW(hw, cls.as_mut_ptr(), cls.len() as i32);
        let cls = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
        if matches!(cls.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd") {
            return false;
        }
        let style = GetWindowLongPtrW(hw, -16 /* GWL_STYLE */) as u32;
        if style & 0x00C0_0000 == 0x00C0_0000 {
            return false; // WS_CAPTION: an ordinary maximised window
        }
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetWindowRect(hw, &mut r);
        let (w, h) = screen_px();
        r.left <= 0 && r.top <= 0 && r.right >= w && r.bottom >= h
    }
}

/// Let a full-screen window cover the panel: drop "always on top" and go just
/// below it.
pub fn panel_below(panel: isize, fullscreen: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::SetWindowPos;
    const FLAGS: u32 = 0x0001 | 0x0002 | 0x0010; // NOSIZE | NOMOVE | NOACTIVATE
    unsafe {
        SetWindowPos(panel as HWND, -2isize as HWND /* HWND_NOTOPMOST */, 0, 0, 0, 0, FLAGS);
        SetWindowPos(panel as HWND, fullscreen as HWND, 0, 0, 0, 0, FLAGS);
    }
}

pub fn panel_on_top(panel: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::SetWindowPos;
    unsafe {
        SetWindowPos(panel as HWND, -1isize as HWND /* HWND_TOPMOST */, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0010);
    }
}

/// Without Explorer, Windows parks minimised windows as little title bars in
/// the bottom-left corner. Hide them instead, like Explorer does (this sign-in
/// only; nothing is saved).
pub fn hide_minimized_windows() {
    #[repr(C)]
    struct MinimizedMetrics {
        cb_size: u32,
        width: i32,
        horz_gap: i32,
        vert_gap: i32,
        arrange: i32,
    }
    let mut mm = MinimizedMetrics { cb_size: 20, width: 0, horz_gap: 0, vert_gap: 0, arrange: 0 };
    unsafe {
        SystemParametersInfoW(0x002B /* SPI_GETMINIMIZEDMETRICS */, 20, &mut mm as *mut MinimizedMetrics as *mut c_void, 0);
        if mm.arrange & 8 == 0 {
            mm.arrange |= 8; // ARW_HIDE
            SystemParametersInfoW(0x002C /* SPI_SETMINIMIZEDMETRICS */, 20, &mut mm as *mut MinimizedMetrics as *mut c_void, 0);
        }
    }
}

/// Move windows that were already minimised (and parked as title bars above
/// the panel) off the screen, where Explorer keeps them.
pub fn hide_parked_minimized() {
    use windows_sys::Win32::UI::WindowsAndMessaging::SetWindowPos;
    unsafe extern "system" fn cb(h: HWND, _: LPARAM) -> BOOL {
        if IsWindowVisible(h) != 0 && IsIconic(h) != 0 {
            SetWindowPos(h, null_mut(), -32000, -32000, 0, 0, 0x0001 | 0x0004 | 0x0010); // NOSIZE | NOZORDER | NOACTIVATE
        }
        1
    }
    unsafe {
        EnumWindows(Some(cb), 0);
    }
}

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
        // Shell locations such as `shell:AppsFolder\<app id>` (Store apps)
        // aren't files: resolve them to an item ID list first.
        let r = if path.to_string_lossy().starts_with("shell:") {
            use windows_sys::Win32::System::Com::CoTaskMemFree;
            use windows_sys::Win32::UI::Shell::SHParseDisplayName;
            let mut pidl = null_mut();
            if SHParseDisplayName(w.as_ptr(), null_mut(), &mut pidl, 0, null_mut()) < 0 || pidl.is_null() {
                return None;
            }
            let r = SHGetFileInfoW(
                pidl as *const u16,
                0,
                &mut info,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                flags | 0x8, /* SHGFI_PIDL */
            );
            CoTaskMemFree(pidl as *const c_void);
            r
        } else {
            SHGetFileInfoW(
                w.as_ptr(),
                0,
                &mut info,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                flags,
            )
        };
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

/// Open a file, folder, shortcut, URL or `ms-settings:` page like a double-click.
/// Runs on its own thread: ShellExecute can take a moment (or wait on a slow
/// handler), and the panel must never freeze while it does. Unlike the `open`
/// crate it never starts PowerShell or falls back to explorer.exe.
pub fn shell_open(target: impl AsRef<std::ffi::OsStr>) {
    let t = target.as_ref().to_string_lossy().to_string();
    // Settings: Delfin's own window for every page it has.
    if t.starts_with("ms-settings:") && crate::settings::page_for(&t).is_some() {
        if let Ok(me) = std::env::current_exe() {
            let _ = Command::new(me).args(["--settings", &t]).spawn();
            return;
        }
    }
    if t.starts_with("ms-settings:") && !explorer_running() {
        if let Some((exe, args)) = classic_settings(&t) {
            let _ = Command::new(exe).args(args).spawn();
            return;
        }
    }
    let w = wide(target.as_ref());
    std::thread::spawn(move || unsafe {
        use windows_sys::Win32::System::Com::CoInitializeEx;
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        // Apartment-threaded, no OLE1 DDE: what ShellExecute expects.
        CoInitializeEx(null(), 0x2 | 0x4);
        // Store / MSIX apps (Claude, Paint, …): "shell:AppsFolder\…" needs
        // Explorer ("Class not registered" without it), so start them the way
        // Windows does underneath.
        if let Some(aumid) = packaged_app_id(&t) {
            if activate_packaged(&aumid) {
                return;
            }
        }
        let verb = wide("open");
        ShellExecuteW(null_mut(), verb.as_ptr(), w.as_ptr(), null(), null(), 1 /* SW_SHOWNORMAL */);
    });
}

/// Is Windows Explorer running as a shell (owning the taskbar)?
pub fn explorer_running() -> bool {
    find_class("Shell_TrayWnd").is_some_and(|h| pid_of(h) != std::process::id())
}

/// Windows' Settings app can't start without Explorer. Pages Delfin's own
/// Settings window doesn't have may still have a classic counterpart.
pub fn classic_settings(uri: &str) -> Option<(String, Vec<String>)> {
    let page = uri.trim_start_matches("ms-settings:").to_lowercase();
    let cpl = |name: &str| Some(("rundll32.exe".to_string(), vec!["shell32.dll,Control_RunDLL".to_string(), name.to_string()]));
    match page.as_str() {
        p if p.starts_with("sound") => cpl("mmsys.cpl"),
        "apps-volume" => Some(("sndvol.exe".to_string(), Vec::new())),
        p if p.starts_with("network") => cpl("ncpa.cpl"),
        p if p.starts_with("dateandtime") => cpl("timedate.cpl"),
        p if p.starts_with("bluetooth") => cpl("bthprops.cpl"),
        p if p.starts_with("powersleep") || p.starts_with("batterysaver") => cpl("powercfg.cpl"),
        p if p.starts_with("mousetouchpad") => cpl("main.cpl"),
        p if p.starts_with("regionformatting") || p.starts_with("regionlanguage") => cpl("intl.cpl"),
        p if p.starts_with("appsfeatures") => cpl("appwiz.cpl"),
        _ => None,
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
    /// Alt+Tab and the keys used while the window switcher is open.
    Switch(SwitchKey),
    /// Ctrl+Shift+Esc.
    TaskManager,
    /// Keyboard media keys: next, previous, play / pause, stop.
    Media(MediaKey),
    /// Keyboard volume keys (also volume knobs and sliders): +1 up, -1 down, 0 mute.
    Volume(i8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SwitchKey {
    Next,
    Prev,
    Up,
    Down,
    Confirm,
    Cancel,
}

/// The window switcher is on screen: the keyboard hook then also takes the
/// arrows, Enter and Esc (the switcher never takes the keyboard focus).
pub static SWITCHER_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static MEDIA_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Explorer is running: it handles the media and volume keys itself.
pub static EXPLORER_PRESENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MediaKey {
    Next,
    Prev,
    PlayPause,
    Stop,
}

/// From the keyboard hook. Only queues the key: the hook must answer within
/// ~1 s or Windows silently removes it, so it never waits for the UI thread.
fn hook_send(h: Hotkey) {
    if let Some(tx) = HOOK_OUT.get() {
        if let Ok(t) = tx.try_lock() {
            let _ = t.send(h);
        }
    }
}
static HOOK_OUT: std::sync::OnceLock<std::sync::Mutex<Sender<Hotkey>>> = std::sync::OnceLock::new();

/// Low-level keyboard hook: Alt+Tab opens Delfin's switcher instead of the
/// Windows one, then Tab / arrows move, Enter switches, Esc cancels.
unsafe extern "system" fn switch_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    use std::sync::atomic::Ordering;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CallNextHookEx, KBDLLHOOKSTRUCT};
    if code == 0 {
        let k = &*(lp as *const KBDLLHOOKSTRUCT);
        let msg = wp as u32;
        let down = msg == 0x0100 || msg == 0x0104; // WM_KEYDOWN, WM_SYSKEYDOWN
        let up = msg == 0x0101 || msg == 0x0105;
        let alt = k.flags & 0x20 != 0; // LLKHF_ALTDOWN
        let open = SWITCHER_OPEN.load(Ordering::Relaxed);
        let shift = GetAsyncKeyState(0x10) < 0;
        // Letting go of Alt switches to the highlighted window, like Windows.
        // The key itself must pass through, or Windows would think Alt is
        // still held down.
        if open && up && matches!(k.vkCode, 0x12 | 0xA4 | 0xA5) {
            hook_send(Hotkey::Switch(SwitchKey::Confirm));
            return CallNextHookEx(null_mut(), code, wp, lp);
        }
        // Media and volume keys. Without Explorer nobody else reacts to them.
        if !EXPLORER_PRESENT.load(Ordering::Relaxed) {
            let action = match k.vkCode {
                0xB0 => Some(Hotkey::Media(MediaKey::Next)),
                0xB1 => Some(Hotkey::Media(MediaKey::Prev)),
                0xB2 => Some(Hotkey::Media(MediaKey::Stop)),
                0xB3 => Some(Hotkey::Media(MediaKey::PlayPause)),
                0xAF => Some(Hotkey::Volume(1)),
                0xAE => Some(Hotkey::Volume(-1)),
                0xAD => Some(Hotkey::Volume(0)),
                _ => None,
            };
            if let Some(a) = action {
                // Volume repeats while held; media keys only on the first press.
                let media = matches!(a, Hotkey::Media(_));
                let repeat = k.flags & 0x80 == 0 && MEDIA_DOWN.swap(down, Ordering::Relaxed) && down;
                if down && !(media && repeat) {
                    hook_send(a);
                }
                if !down {
                    MEDIA_DOWN.store(false, Ordering::Relaxed);
                }
                return 1;
            }
        }
        // Ctrl+Shift+Esc: Delfin's task manager.
        if down && !open && k.vkCode == 0x1B && GetAsyncKeyState(0x11) < 0 && shift {
            hook_send(Hotkey::TaskManager);
            return 1;
        }
        let key = match k.vkCode {
            0x09 if alt || open => Some(if shift { SwitchKey::Prev } else { SwitchKey::Next }),
            0x25 if open => Some(SwitchKey::Prev),  // Left
            0x27 if open => Some(SwitchKey::Next),  // Right
            0x26 if open => Some(SwitchKey::Up),
            0x28 if open => Some(SwitchKey::Down),
            0x0D if open => Some(SwitchKey::Confirm), // Enter
            0x1B if open => Some(SwitchKey::Cancel),  // Esc
            _ => None,
        };
        if let Some(key) = key {
            if down {
                hook_send(Hotkey::Switch(key));
            }
            if down || up {
                return 1; // swallow: Windows' own switcher never sees it
            }
        }
    }
    CallNextHookEx(null_mut(), code, wp, lp)
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
        // Keys from the hook go through a forwarding thread, which wakes the UI.
        let (fwd_tx, fwd_rx) = std::sync::mpsc::channel::<Hotkey>();
        let _ = HOOK_OUT.set(std::sync::Mutex::new(fwd_tx));
        {
            let (tx, ctx) = (tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                while let Ok(k) = fwd_rx.recv() {
                    let _ = tx.send(k);
                    ctx.request_repaint();
                }
            });
        }
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetTimer, SetWindowsHookExW, UnhookWindowsHookEx};
        let install = || SetWindowsHookExW(13 /* WH_KEYBOARD_LL */, Some(switch_hook), GetModuleHandleW(null()), 0);
        let mut hook = install();
        // Windows removes a low-level hook without telling anyone if it was ever
        // too slow; put it back every 30 s to be safe.
        SetTimer(null_mut(), 0, 30_000, None);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if msg.message == 0x0113 {
                // WM_TIMER
                if !hook.is_null() {
                    UnhookWindowsHookEx(hook);
                }
                hook = install();
                continue;
            }
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

// ----------------------------------------------------------------- window events

static EVENTS_CTX: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();
/// A window changed since the panel last listed them.
pub static WINDOWS_DIRTY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
/// The windows the panel shows (buttons and tray-shelved), for the events above.
pub static PANEL_WINDOWS: std::sync::Mutex<Vec<isize>> = std::sync::Mutex::new(Vec::new());

/// Wake the panel when a window appears, goes away, changes its title, is
/// minimized or restored, or the foreground window changes. The panel only
/// redraws then (and for the clock), instead of polling every half second.
pub fn spawn_window_events(ctx: egui::Context) {
    let _ = EVENTS_CTX.set(ctx);
    std::thread::Builder::new()
        .name("delfin-winevents".into())
        .spawn(|| unsafe {
            use windows_sys::Win32::UI::Accessibility::SetWinEventHook;
            const OUTOFCONTEXT: u32 = 0;
            const SKIPOWNPROCESS: u32 = 2;
            // (first, last) event ranges we care about.
            for (a, b) in [
                (0x0003u32, 0x0003u32), // EVENT_SYSTEM_FOREGROUND
                (0x0016, 0x0017),       // EVENT_SYSTEM_MINIMIZESTART / END
                (0x8000, 0x8003),       // EVENT_OBJECT_CREATE / DESTROY / SHOW / HIDE
                (0x800B, 0x800C),       // EVENT_OBJECT_LOCATIONCHANGE / NAMECHANGE
            ] {
                SetWinEventHook(a, b, null_mut(), Some(win_event), 0, 0, OUTOFCONTEXT | SKIPOWNPROCESS);
            }
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {}
        })
        .ok();
}

unsafe extern "system" fn win_event(
    _hook: windows_sys::Win32::UI::Accessibility::HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    // Only whole windows, not carets, scroll bars or list items.
    if hwnd.is_null() || id_object != 0 || id_child != 0 {
        return;
    }
    use windows_sys::Win32::UI::WindowsAndMessaging::GetAncestor;
    let relevant = match event {
        // Moving or resizing: only the foreground window matters (full screen).
        0x800B => GetForegroundWindow() == hwnd,
        0x0003 | 0x0016 | 0x0017 => true,
        // Destroyed or hidden: the window may already be gone, so ask whether
        // it was on the panel.
        0x8001 | 0x8003 => PANEL_WINDOWS.lock().is_ok_and(|w| w.contains(&(hwnd as isize))),
        // Created, shown or renamed: top-level windows only.
        _ => GetAncestor(hwnd, 2 /* GA_ROOT */) == hwnd,
    };
    if !relevant {
        return;
    }
    WINDOWS_DIRTY.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(ctx) = EVENTS_CTX.get() {
        // Bursts (a title that ticks, a window being dragged) collapse into
        // a few redraws a second.
        let delay = if matches!(event, 0x800B | 0x800C) { 250 } else { 60 };
        ctx.request_repaint_after(std::time::Duration::from_millis(delay));
    }
}

// ----------------------------------------------------------------- window switcher

/// While Alt is held, tap an unused key (0xE8): then releasing Alt doesn't
/// open the menu bar of whatever window has the focus.
pub fn mask_alt_menu() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{keybd_event, GetAsyncKeyState, KEYEVENTF_KEYUP};
    unsafe {
        if GetAsyncKeyState(0x12) < 0 {
            keybd_event(0xE8, 0, 0, 0);
            keybd_event(0xE8, 0, KEYEVENTF_KEYUP, 0);
        }
    }
}

/// Bring a window to the front from the Alt+Tab switcher.
pub fn switch_to(h: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SwitchToThisWindow};
    let hwnd = h as HWND;
    unsafe {
        mask_alt_menu();
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, 9 /* SW_RESTORE */);
        }
        SetForegroundWindow(hwnd);
        if GetForegroundWindow() != hwnd {
            SwitchToThisWindow(hwnd, 1);
        }
    }
}

/// Live preview of `src` drawn by DWM into our window `dest` (no copying).
pub fn thumb_register(dest: isize, src: isize) -> Option<isize> {
    use windows_sys::Win32::Graphics::Dwm::DwmRegisterThumbnail;
    let mut id = 0isize;
    let hr = unsafe { DwmRegisterThumbnail(dest as HWND, src as HWND, &mut id) };
    (hr >= 0 && id != 0).then_some(id)
}

/// Size of the window behind a thumbnail, in pixels.
pub fn thumb_source_size(id: isize) -> (i32, i32) {
    use windows_sys::Win32::Foundation::SIZE;
    use windows_sys::Win32::Graphics::Dwm::DwmQueryThumbnailSourceSize;
    let mut s = SIZE { cx: 0, cy: 0 };
    unsafe { DwmQueryThumbnailSourceSize(id, &mut s) };
    (s.cx, s.cy)
}

/// Place a thumbnail at `rect` (client pixels of the destination window).
pub fn thumb_update(id: isize, rect: [i32; 4], opacity: u8, visible: bool) {
    use windows_sys::Win32::Graphics::Dwm::{DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES};
    let p = DWM_THUMBNAIL_PROPERTIES {
        dwFlags: 0x1 | 0x4 | 0x8 | 0x10, // RECTDESTINATION | OPACITY | VISIBLE | SOURCECLIENTAREAONLY
        rcDestination: RECT { left: rect[0], top: rect[1], right: rect[2], bottom: rect[3] },
        rcSource: RECT { left: 0, top: 0, right: 0, bottom: 0 },
        opacity,
        fVisible: visible as BOOL,
        fSourceClientAreaOnly: 0,
    };
    unsafe { DwmUpdateThumbnailProperties(id, &p) };
}

pub fn thumb_unregister(id: isize) {
    unsafe { windows_sys::Win32::Graphics::Dwm::DwmUnregisterThumbnail(id) };
}

/// Windows 11 rounded corners (and the matching shadow) for a pop-up.
pub fn round_corners(h: isize) {
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    let pref: i32 = 2; // DWMWCP_ROUND
    unsafe { DwmSetWindowAttribute(h as HWND, 33, &pref as *const i32 as *const c_void, 4) };
}

/// A WM_APPCOMMAND that reached the shell (see the tray window): volume and
/// media commands. Returns true when Delfin handled it.
pub fn app_command(cmd: u16) -> bool {
    use std::sync::atomic::Ordering;
    if EXPLORER_PRESENT.load(Ordering::Relaxed) {
        return false;
    }
    let h = match cmd {
        8 => Hotkey::Volume(0),                       // APPCOMMAND_VOLUME_MUTE
        9 => Hotkey::Volume(-1),                      // APPCOMMAND_VOLUME_DOWN
        10 => Hotkey::Volume(1),                      // APPCOMMAND_VOLUME_UP
        11 => Hotkey::Media(MediaKey::Next),          // APPCOMMAND_MEDIA_NEXTTRACK
        12 => Hotkey::Media(MediaKey::Prev),          // APPCOMMAND_MEDIA_PREVIOUSTRACK
        13 => Hotkey::Media(MediaKey::Stop),          // APPCOMMAND_MEDIA_STOP
        14 | 46 | 47 => Hotkey::Media(MediaKey::PlayPause), // PLAY_PAUSE, PLAY, PAUSE
        _ => return false,
    };
    hook_send(h);
    true
}

/// The program (.exe) that owns a window.
pub fn window_exe(h: isize) -> Option<PathBuf> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(h as HWND, &mut pid);
        if pid == 0 {
            return None;
        }
        let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if p.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(p, 0, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(p);
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// "Claude_pzs8sxrjxfjjc!Claude" from "shell:AppsFolder\Claude_pzs8sxrjxfjjc!Claude".
pub fn packaged_app_id(target: &str) -> Option<String> {
    let lower = target.to_lowercase();
    let rest = lower.strip_prefix("shell:appsfolder\\").map(|_| &target["shell:AppsFolder\\".len()..])?;
    rest.contains('!').then(|| rest.trim().to_string())
}

/// Start a packaged (Store / MSIX) app by its AppUserModelID through
/// IApplicationActivationManager; works without Explorer. Needs COM.
pub fn activate_packaged(aumid: &str) -> bool {
    use windows_sys::core::GUID;
    use windows_sys::Win32::System::Com::CoCreateInstance;
    const CLSID_AAM: GUID = GUID::from_u128(0x45ba127d_10a8_46ea_8ab7_56ea9078943c);
    const IID_IAAM: GUID = GUID::from_u128(0x2e941141_7f97_4756_ba1d_9decde894a3d);
    unsafe {
        let mut obj: *mut c_void = null_mut();
        if CoCreateInstance(&CLSID_AAM, null_mut(), 0x17 /* CLSCTX_ALL */, &IID_IAAM, &mut obj) < 0 || obj.is_null() {
            return false;
        }
        let vtbl = *(obj as *const *const usize);
        let activate: unsafe extern "system" fn(*mut c_void, *const u16, *const u16, u32, *mut u32) -> i32 =
            std::mem::transmute(*vtbl.add(3));
        let release: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(*vtbl.add(2));
        let id = wide(aumid);
        let mut pid = 0u32;
        let hr = activate(obj, id.as_ptr(), null(), 0, &mut pid);
        release(obj);
        hr >= 0
    }
}

/// "claude|pzs8sxrjxfjjc" for a program inside C:\Program Files\WindowsApps
/// (package name and publisher id), to match its windows to a pinned app.
pub fn package_key_of_exe(exe: &str) -> Option<String> {
    let l = exe.to_lowercase();
    let marker = "\\windowsapps\\";
    let i = l.find(marker)? + marker.len();
    let folder = l[i..].split('\\').next()?;
    let (head, publisher) = folder.split_once("__")?;
    let name = head.split('_').next()?;
    Some(format!("{name}|{publisher}"))
}

/// The same key from an AppUserModelID ("Claude_pzs8sxrjxfjjc!Claude").
pub fn package_key_of_aumid(aumid: &str) -> Option<String> {
    let family = aumid.split('!').next()?.to_lowercase();
    let (name, publisher) = family.rsplit_once('_')?;
    Some(format!("{name}|{publisher}"))
}

// ----------------------------------------------------------------- Aero glass

/// This process's top-level windows (the panel, its pop-ups; not the desktop).
pub fn own_windows() -> Vec<isize> {
    unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> BOOL {
        let out = &mut *(lp as *mut (u32, Vec<isize>));
        let mut pid = 0;
        GetWindowThreadProcessId(h, &mut pid);
        if pid == out.0 {
            let mut class = [0u16; 32];
            let n = windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW(h, class.as_mut_ptr(), class.len() as i32);
            if String::from_utf16_lossy(&class[..n.max(0) as usize]) != "DelfinDesktop" {
                out.1.push(h as isize);
            }
        }
        1
    }
    let mut st = (std::process::id(), Vec::new());
    unsafe { EnumWindows(Some(each), &mut st as *mut _ as LPARAM) };
    st.1
}

/// Blur what is behind a window (Aero glass), or stop doing so. The window's
/// own see-through colours then show a blurred desktop and windows behind it.
pub fn set_blur(h: isize, on: bool) {
    #[repr(C)]
    struct AccentPolicy {
        state: u32,
        flags: u32,
        color: u32,
        animation: u32,
    }
    #[repr(C)]
    struct CompositionData {
        attribute: u32,
        data: *mut c_void,
        size: usize,
    }
    type SetWca = unsafe extern "system" fn(HWND, *mut CompositionData) -> BOOL;
    static FUNC: std::sync::OnceLock<Option<SetWca>> = std::sync::OnceLock::new();
    let f = FUNC.get_or_init(|| unsafe {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        let user32 = GetModuleHandleW(wide("user32.dll").as_ptr());
        GetProcAddress(user32, c"SetWindowCompositionAttribute".as_ptr() as *const u8).map(|p| std::mem::transmute::<_, SetWca>(p))
    });
    let Some(f) = f else { return };
    // ACCENT_ENABLE_BLURBEHIND = 3, ACCENT_DISABLED = 0; WCA_ACCENT_POLICY = 19.
    let mut policy = AccentPolicy { state: if on { 3 } else { 0 }, flags: 0, color: 0, animation: 0 };
    let mut data = CompositionData { attribute: 19, data: &mut policy as *mut _ as *mut c_void, size: std::mem::size_of::<AccentPolicy>() };
    unsafe { f(h as HWND, &mut data) };
}
