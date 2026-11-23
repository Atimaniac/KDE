//! The desktop: the wallpaper and the icons of the Desktop folders.
//!
//! A plain Win32 window drawn with GDI on its own thread, not an egui/OpenGL
//! viewport: a full-screen OpenGL surface and the wallpaper as a texture cost
//! ~50 MB of graphics memory. Here the wallpaper is decoded once (on another
//! thread) into a screen-sized GDI bitmap, fitted like Windows does it (fill,
//! fit, stretch, tile, centre, span), the icons come from the system image
//! list, and the window is only drawn when something changed. (Windows' own
//! PaintDesktop draws nothing without Explorer.) What the user picks goes
//! to the shell as a [`DeskAction`].

use eframe::egui;
use std::cell::RefCell;
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::mpsc::Sender;
use std::time::SystemTime;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const TITLE: &str = "Delfin Desktop";

/// What the user asked for on the desktop; carried out by the shell.
pub enum DeskAction {
    Open(PathBuf, bool),
    ShowInDelfin(PathBuf),
    Terminal(PathBuf),
    Trash(PathBuf),
    Pin(String, PathBuf),
    Unpin(PathBuf),
    Props(PathBuf),
    Uri(&'static str),
}

struct Item {
    name: String,
    path: PathBuf,
    is_dir: bool,
    icon: isize,
}

struct State {
    tx: Sender<DeskAction>,
    ctx: egui::Context,
    items: Vec<Item>,
    mtimes: Vec<Option<SystemTime>>,
    sel: Option<usize>,
    hover: Option<usize>,
    tracking: bool,
    font: HFONT,
    /// Pixels per point (the screen's scale).
    scale: f32,
    accent: u32,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Start the desktop window on its own thread.
pub fn start(tx: Sender<DeskAction>, ctx: egui::Context, dark: bool) {
    std::thread::Builder::new()
        .name("delfin-desktop".into())
        .spawn(move || unsafe { run(tx, ctx, dark) })
        .ok();
}

unsafe fn run(tx: Sender<DeskAction>, ctx: egui::Context, dark: bool) {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    windows_sys::Win32::System::Com::CoInitializeEx(null(), 0x2);
    dark_menus(dark);
    let inst = GetModuleHandleW(null());
    let class = wide("DelfinDesktop");
    let wc = WNDCLASSW {
        style: CS_DBLCLKS,
        lpfnWndProc: Some(wndproc),
        hInstance: inst,
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: class.as_ptr(),
        ..std::mem::zeroed()
    };
    RegisterClassW(&wc);
    let dc = GetDC(null_mut());
    let scale = GetDeviceCaps(dc, 90 /* LOGPIXELSY */) as f32 / 96.0;
    ReleaseDC(null_mut(), dc);
    let font_name = wide("Segoe UI");
    let font = CreateFontW(
        -(12.0 * scale).round() as i32,
        0, 0, 0, 400, 0, 0, 0, 1 /* DEFAULT_CHARSET */, 0, 0, 5 /* CLEARTYPE_QUALITY */, 0,
        font_name.as_ptr(),
    );
    let a = crate::theme::ACCENT;
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            tx,
            ctx,
            items: Vec::new(),
            mtimes: Vec::new(),
            sel: None,
            hover: None,
            tracking: false,
            font,
            scale,
            accent: (a.r() as u32) | ((a.g() as u32) << 8) | ((a.b() as u32) << 16),
        })
    });
    reload(true);
    let title = wide(TITLE);
    let (w, h) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW,
        class.as_ptr(),
        title.as_ptr(),
        WS_POPUP | WS_VISIBLE,
        0,
        0,
        w,
        h,
        null_mut(),
        null_mut(),
        inst,
        null(),
    );
    if hwnd.is_null() {
        return;
    }
    SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    super::win::set_shell_window(hwnd as isize);
    load_wallpaper(hwnd, w, h);
    // New, renamed or deleted files on the desktop (checked cheaply).
    SetTimer(hwnd, 1, 3000, None);
    let mut msg: MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

/// Dark context menus to match the dark theme (uxtheme's unnamed exports).
unsafe fn dark_menus(dark: bool) {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    let lib = LoadLibraryW(wide("uxtheme.dll").as_ptr());
    if lib.is_null() {
        return;
    }
    if let Some(f) = GetProcAddress(lib, 135 as *const u8) {
        let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
        set_mode(if dark { 2 } else { 3 }); // ForceDark / ForceLight
    }
    if let Some(f) = GetProcAddress(lib, 136 as *const u8) {
        let flush: unsafe extern "system" fn() = std::mem::transmute(f);
        flush();
    }
}

fn desktop_dirs() -> Vec<PathBuf> {
    let mut d = Vec::new();
    if let Some(p) = dirs::desktop_dir() {
        d.push(p);
    }
    if let Ok(p) = std::env::var("PUBLIC") {
        // "Desktop" on disk in every language.
        d.push(PathBuf::from(p).join("Desktop"));
    }
    d
}

/// Read the Desktop folders again if they changed. True when they did.
fn reload(force: bool) -> bool {
    let dirs = desktop_dirs();
    let mtimes: Vec<Option<SystemTime>> = dirs.iter().map(|d| std::fs::metadata(d).and_then(|m| m.modified()).ok()).collect();
    let same = STATE.with(|s| s.borrow().as_ref().is_some_and(|st| st.mtimes == mtimes));
    if same && !force {
        return false;
    }
    let mut found: Vec<(String, PathBuf, bool)> = Vec::new();
    for d in &dirs {
        let Ok(list) = crate::fsops::list_dir(d) else { continue };
        for e in list {
            if e.hidden {
                continue;
            }
            let name = if matches!(e.ext.as_str(), "lnk" | "url") {
                e.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(e.name)
            } else {
                e.name
            };
            found.push((name, e.path, e.is_dir));
        }
    }
    found.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| crate::fsops::natural_cmp(&a.0, &b.0)));
    let size = STATE.with(|s| s.borrow().as_ref().map(|st| (48.0 * st.scale).round() as i32).unwrap_or(48));
    let items: Vec<Item> = found
        .into_iter()
        .map(|(name, path, is_dir)| {
            let icon = unsafe { load_icon(&path, size) };
            Item { name, path, is_dir, icon }
        })
        .collect();
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            let sel_path = st.sel.and_then(|i| st.items.get(i)).map(|i| i.path.clone());
            for it in st.items.drain(..) {
                if it.icon != 0 {
                    unsafe { DestroyIcon(it.icon as HICON) };
                }
            }
            st.sel = sel_path.and_then(|p| items.iter().position(|i| i.path == p));
            st.hover = None;
            st.items = items;
            st.mtimes = mtimes;
        }
    });
    true
}

/// The file's icon from the system image list (48 px, or 256 px for big screens).
unsafe fn load_icon(path: &std::path::Path, size: i32) -> isize {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Controls::ImageList_GetIcon;
    use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHGetImageList, SHFILEINFOW};
    const IID_IIMAGELIST: windows_sys::core::GUID = windows_sys::core::GUID::from_u128(0x46eb5926_582e_4017_9fdf_e8998daa0950);
    let w: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut info: SHFILEINFOW = std::mem::zeroed();
    if SHGetFileInfoW(w.as_ptr(), 0, &mut info, std::mem::size_of::<SHFILEINFOW>() as u32, 0x4000) == 0 {
        return 0;
    }
    let which = if size > 48 { 4 /* SHIL_JUMBO */ } else { 2 /* SHIL_EXTRALARGE */ };
    let mut list: *mut c_void = null_mut();
    if SHGetImageList(which, &IID_IIMAGELIST, &mut list) < 0 || list.is_null() {
        return 0;
    }
    ImageList_GetIcon(list as isize, info.iIcon, 0x1) as isize
}

// ------------------------------------------------------------------ layout

struct Layout {
    cell_w: i32,
    cell_h: i32,
    left: i32,
    top: i32,
    rows: usize,
    icon: i32,
}

fn layout(st: &State) -> Layout {
    let s = st.scale;
    let cell_w = (96.0 * s).round() as i32;
    let cell_h = (100.0 * s).round() as i32;
    // The work area: icons never go under the panel.
    let mut wa: RECT = unsafe { std::mem::zeroed() };
    unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut _ as *mut c_void, 0) };
    let margin = (10.0 * s) as i32;
    let rows = (((wa.bottom - wa.top - 2 * margin) / cell_h).max(1)) as usize;
    Layout { cell_w, cell_h, left: wa.left + margin, top: wa.top + margin, rows, icon: (48.0 * s).round() as i32 }
}

fn cell_rect(l: &Layout, i: usize) -> RECT {
    let (col, row) = ((i / l.rows) as i32, (i % l.rows) as i32);
    let x = l.left + col * l.cell_w;
    let y = l.top + row * l.cell_h;
    RECT { left: x, top: y, right: x + l.cell_w, bottom: y + l.cell_h }
}

fn hit(st: &State, x: i32, y: i32) -> Option<usize> {
    let l = layout(st);
    (0..st.items.len()).find(|&i| {
        let r = cell_rect(&l, i);
        x >= r.left && x < r.right && y >= r.top && y < r.bottom
    })
}

// ------------------------------------------------------------------ drawing

/// A translucent rectangle (selection, hover).
unsafe fn fill_alpha(dc: HDC, r: &RECT, color: u32, alpha: u8) {
    let mut bits: *mut c_void = null_mut();
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = 1;
    bi.bmiHeader.biHeight = 1;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    let bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    if bmp.is_null() || bits.is_null() {
        return;
    }
    // BGRA, not premultiplied (SourceConstantAlpha does the blending).
    let (r_, g, b) = (color & 0xff, (color >> 8) & 0xff, (color >> 16) & 0xff);
    *(bits as *mut u32) = b | (g << 8) | (r_ << 16) | 0xff00_0000;
    let src = CreateCompatibleDC(dc);
    let old = SelectObject(src, bmp);
    let bf = BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: 0 };
    AlphaBlend(dc, r.left, r.top, r.right - r.left, r.bottom - r.top, src, 0, 0, 1, 1, bf);
    SelectObject(src, old);
    DeleteDC(src);
    DeleteObject(bmp);
}

unsafe fn paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut rc);
    // Drawn off screen first: no flicker.
    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
    let old_bmp = SelectObject(mem, bmp);
    draw_wallpaper(mem, &rc);
    STATE.with(|s| {
        let s = s.borrow();
        let Some(st) = s.as_ref() else { return };
        let l = layout(st);
        let old_font = SelectObject(mem, st.font);
        SetBkMode(mem, TRANSPARENT as i32);
        for (i, it) in st.items.iter().enumerate() {
            let r = cell_rect(&l, i);
            if r.bottom < ps.rcPaint.top || r.top > ps.rcPaint.bottom || r.right < ps.rcPaint.left || r.left > ps.rcPaint.right {
                continue;
            }
            let selected = st.sel == Some(i);
            if selected {
                fill_alpha(mem, &r, st.accent, 90);
                let pen = CreatePen(PS_SOLID, 1, st.accent);
                let old_pen = SelectObject(mem, pen);
                let old_brush = SelectObject(mem, GetStockObject(NULL_BRUSH));
                Rectangle(mem, r.left, r.top, r.right, r.bottom);
                SelectObject(mem, old_brush);
                SelectObject(mem, old_pen);
                DeleteObject(pen);
            } else if st.hover == Some(i) {
                fill_alpha(mem, &r, 0xffffff, 30);
            }
            let ix = r.left + (l.cell_w - l.icon) / 2;
            let iy = r.top + (6.0 * st.scale) as i32;
            if it.icon != 0 {
                DrawIconEx(mem, ix, iy, it.icon as HICON, l.icon, l.icon, 0, null_mut(), DI_NORMAL);
            }
            // Label: two lines (all of it when selected), with a shadow.
            let mut text: Vec<u16> = it.name.encode_utf16().collect();
            let pad = (4.0 * st.scale) as i32;
            let ty = iy + l.icon + (4.0 * st.scale) as i32;
            let bottom = if selected { r.bottom + l.cell_h } else { r.bottom - pad };
            let flags = DT_CENTER | DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX | if selected { 0 } else { DT_END_ELLIPSIS };
            for (dx, color) in [(1, 0x000000u32), (0, 0xffffff)] {
                let mut tr = RECT { left: r.left + pad + dx, top: ty + dx, right: r.right - pad + dx, bottom: bottom + dx };
                SetTextColor(mem, color);
                DrawTextW(mem, text.as_mut_ptr(), text.len() as i32, &mut tr, flags);
            }
        }
        SelectObject(mem, old_font);
    });
    BitBlt(hdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old_bmp);
    DeleteObject(bmp);
    DeleteDC(mem);
    EndPaint(hwnd, &ps);
}

// ------------------------------------------------------------------ input

fn send(a: DeskAction) {
    STATE.with(|s| {
        if let Some(st) = s.borrow().as_ref() {
            let _ = st.tx.send(a);
            st.ctx.request_repaint();
        }
    });
}

fn item(i: usize) -> Option<(PathBuf, bool, String)> {
    STATE.with(|s| s.borrow().as_ref().and_then(|st| st.items.get(i).map(|it| (it.path.clone(), it.is_dir, it.name.clone()))))
}

fn open(i: usize) {
    if let Some((p, dir, _)) = item(i) {
        send(DeskAction::Open(p, dir));
    }
}

unsafe fn menu(hwnd: HWND, i: Option<usize>) {
    use crate::i18n::tr;
    let m = CreatePopupMenu();
    let add = |id: usize, text: &str| {
        let w = wide(text);
        AppendMenuW(m, MF_STRING, id, w.as_ptr());
    };
    let sep = || {
        AppendMenuW(m, MF_SEPARATOR, 0, null());
    };
    let target = i.and_then(item);
    let mut pinned = false;
    match &target {
        Some((p, dir, _)) => {
            add(1, tr("Open"));
            if *dir {
                add(3, tr("Open Terminal Here"));
            } else {
                add(2, tr("Show in Delfin"));
            }
            let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !*dir && ["exe", "lnk", "url", "appref-ms"].contains(&ext.as_str()) {
                pinned = super::pins::is_pinned(p);
                add(4, if pinned { tr("Unpin from taskbar") } else { tr("Pin to taskbar") });
            }
            sep();
            add(5, tr("Move to Trash"));
            sep();
            add(6, tr("Properties"));
        }
        None => {
            add(10, tr("Open Desktop Folder"));
            add(11, tr("Open Terminal Here"));
            add(12, tr("Refresh"));
            sep();
            add(13, tr("Change Wallpaper…"));
            add(14, tr("Display Settings…"));
        }
    }
    let mut pt: POINT = std::mem::zeroed();
    GetCursorPos(&mut pt);
    // Needed so the menu closes when you click elsewhere.
    SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, null()) as usize;
    DestroyMenu(m);
    let desktop = dirs::desktop_dir();
    match (cmd, target) {
        (1, Some((p, dir, _))) => send(DeskAction::Open(p, dir)),
        (2, Some((p, _, _))) => send(DeskAction::ShowInDelfin(p)),
        (3, Some((p, _, _))) => send(DeskAction::Terminal(p)),
        (4, Some((p, _, name))) => send(if pinned { DeskAction::Unpin(p) } else { DeskAction::Pin(name, p) }),
        (5, Some((p, _, _))) => send(DeskAction::Trash(p)),
        (6, Some((p, _, _))) => send(DeskAction::Props(p)),
        (10, _) => {
            if let Some(d) = desktop {
                send(DeskAction::Open(d, true));
            }
        }
        (11, _) => {
            if let Some(d) = desktop {
                send(DeskAction::Terminal(d));
            }
        }
        (12, _) => {
            reload(true);
            InvalidateRect(hwnd, null(), 0);
        }
        (13, _) => send(DeskAction::Uri("ms-settings:personalization-background")),
        (14, _) => send(DeskAction::Uri("ms-settings:display")),
        _ => {}
    }
}

fn set_sel(hwnd: HWND, sel: Option<usize>) {
    let changed = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(st) = s.as_mut() else { return false };
        let c = st.sel != sel;
        st.sel = sel;
        c
    });
    if changed {
        unsafe { InvalidateRect(hwnd, null(), 0) };
    }
}

fn sel() -> Option<usize> {
    STATE.with(|s| s.borrow().as_ref().and_then(|st| st.sel))
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let x = (lp & 0xffff) as i16 as i32;
    let y = ((lp >> 16) & 0xffff) as i16 as i32;
    let at = |x: i32, y: i32| STATE.with(|s| s.borrow().as_ref().and_then(|st| hit(st, x, y)));
    match msg {
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        // Always at the very bottom, even when clicked.
        WM_WINDOWPOSCHANGING => {
            let wp = &mut *(lp as *mut WINDOWPOS);
            wp.hwndInsertAfter = HWND_BOTTOM;
            wp.flags &= !SWP_NOZORDER;
            0
        }
        WM_MOUSEACTIVATE => MA_ACTIVATE as LRESULT,
        WM_LBUTTONDOWN => {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(hwnd);
            set_sel(hwnd, at(x, y));
            0
        }
        WM_LBUTTONDBLCLK => {
            if let Some(i) = at(x, y) {
                open(i);
            }
            0
        }
        WM_RBUTTONUP => {
            let i = at(x, y);
            set_sel(hwnd, i);
            UpdateWindow(hwnd);
            menu(hwnd, i);
            0
        }
        WM_MOUSEMOVE => {
            let h = at(x, y);
            let (changed, track) = STATE.with(|s| {
                let mut s = s.borrow_mut();
                let Some(st) = s.as_mut() else { return (false, false) };
                let c = st.hover != h;
                st.hover = h;
                let t = !st.tracking;
                st.tracking = true;
                (c, t)
            });
            if track {
                use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
                let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                TrackMouseEvent(&mut t);
            }
            if changed {
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        0x02A3 /* WM_MOUSELEAVE */ => {
            STATE.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut() {
                    st.hover = None;
                    st.tracking = false;
                }
            });
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_KEYDOWN => {
            match wp as u16 {
                0x0D /* Enter */ => {
                    if let Some(i) = sel() {
                        open(i);
                    }
                }
                0x2E /* Delete */ => {
                    if let Some((p, _, _)) = sel().and_then(item) {
                        send(DeskAction::Trash(p));
                    }
                }
                0x74 /* F5 */ => {
                    reload(true);
                    InvalidateRect(hwnd, null(), 0);
                }
                0x1B /* Esc */ => set_sel(hwnd, None),
                _ => {}
            }
            0
        }
        WM_TIMER => {
            if reload(false) {
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        // New wallpaper (or its fit, or the desktop colour) or screen size.
        WM_SETTINGCHANGE | WM_DISPLAYCHANGE => {
            let (w, h) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
            // wParam 0 comes with a change written to the registry (fit, colour).
            let wallpaper = msg == WM_DISPLAYCHANGE || matches!(wp as u32, SPI_SETDESKWALLPAPER | 0);
            if wallpaper {
                SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, w, h, SWP_NOACTIVATE);
                load_wallpaper(hwnd, w, h);
            }
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_WALLPAPER => {
            take_wallpaper(hwnd);
            0
        }
        // The shell deleted or created something: look again soon.
        WM_USER => {
            if reload(false) {
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        // A device came or went (USB drives on the panel).
        0x0219 /* WM_DEVICECHANGE */ => {
            super::usb::CHANGED.store(true, std::sync::atomic::Ordering::Relaxed);
            STATE.with(|s| {
                if let Some(st) = s.borrow().as_ref() {
                    st.ctx.request_repaint_after(std::time::Duration::from_millis(800));
                }
            });
            1
        }
        WM_CLOSE => 0, // the desktop is never closed by others
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Ask the desktop to look at its folders again (after the shell changed them).
pub fn refresh() {
    let title = wide(TITLE);
    let class = wide("DelfinDesktop");
    unsafe {
        let h = FindWindowW(class.as_ptr(), title.as_ptr());
        if !h.is_null() {
            PostMessageW(h, WM_USER, 0, 0);
        }
    }
}

// ------------------------------------------------------------------ wallpaper

/// The wallpaper fitted to the screen, ready to draw (BGRA, top-down).
static WALLPAPER: std::sync::Mutex<Option<(i32, i32, Vec<u8>)>> = std::sync::Mutex::new(None);
const WM_WALLPAPER: u32 = WM_APP + 1;

thread_local! {
    /// The wallpaper as a GDI bitmap: (memory DC, bitmap, old bitmap, width, height).
    static WALL: RefCell<Option<(HDC, HBITMAP, HGDIOBJ, i32, i32)>> = const { RefCell::new(None) };
}

fn reg_str(name: &str) -> Option<String> {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey(r"Control Panel\Desktop").ok()?.get_value::<String, _>(name).ok()
}

/// The desktop colour behind a picture that doesn't cover the screen.
fn background_rgb() -> [u8; 3] {
    let v = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Control Panel\Colors")
        .ok()
        .and_then(|k| k.get_value::<String, _>("Background").ok())
        .unwrap_or_default();
    let n: Vec<u8> = v.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    if n.len() == 3 { [n[0], n[1], n[2]] } else { [0, 0, 0] }
}

/// Decode the wallpaper and fit it to the screen the way Windows would
/// (fill, fit, stretch, tile, centre, span), on another thread.
fn load_wallpaper(hwnd: HWND, w: i32, h: i32) {
    let hwnd = hwnd as isize;
    std::thread::spawn(move || {
        let (w, h) = (w.max(1) as u32, h.max(1) as u32);
        let mut path = [0u16; 1024];
        unsafe { SystemParametersInfoW(SPI_GETDESKWALLPAPER, path.len() as u32, path.as_mut_ptr() as *mut c_void, 0) };
        let n = path.iter().position(|&c| c == 0).unwrap_or(path.len());
        let path = String::from_utf16_lossy(&path[..n]);
        let style = reg_str("WallpaperStyle").unwrap_or_else(|| "10".into());
        let tile = reg_str("TileWallpaper").unwrap_or_else(|| "0".into()) == "1";
        let bg = background_rgb();
        let mut canvas = image::RgbaImage::from_pixel(w, h, image::Rgba([bg[0], bg[1], bg[2], 255]));
        let pic = if path.is_empty() { None } else { image::open(&path).ok().map(|i| i.to_rgba8()) };
        match pic {
            Some(img) => {
                let (iw, ih) = (img.width() as f32, img.height() as f32);
                let place = |canvas: &mut image::RgbaImage, img: &image::RgbaImage, x: i64, y: i64| {
                    image::imageops::overlay(canvas, img, x, y);
                };
                let scaled = |s: f32| {
                    let (nw, nh) = (((iw * s).round() as u32).max(1), ((ih * s).round() as u32).max(1));
                    image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
                };
                if tile {
                    let mut y = 0;
                    while y < h {
                        let mut x = 0;
                        while x < w {
                            place(&mut canvas, &img, x as i64, y as i64);
                            x += img.width();
                        }
                        y += img.height();
                    }
                } else {
                    match style.as_str() {
                        // Stretch
                        "2" => canvas = image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle),
                        // Fit
                        "6" => {
                            let s = (w as f32 / iw).min(h as f32 / ih);
                            let im = scaled(s);
                            place(&mut canvas, &im, (w as i64 - im.width() as i64) / 2, (h as i64 - im.height() as i64) / 2);
                        }
                        // Centre
                        "0" => place(&mut canvas, &img, (w as i64 - img.width() as i64) / 2, (h as i64 - img.height() as i64) / 2),
                        // Fill and span: cover the screen, cropped in the middle.
                        _ => {
                            let s = (w as f32 / iw).max(h as f32 / ih);
                            let im = scaled(s);
                            place(&mut canvas, &im, (w as i64 - im.width() as i64) / 2, (h as i64 - im.height() as i64) / 2);
                        }
                    }
                }
            }
            None if path.is_empty() => {
                // No wallpaper: a Breeze-like gradient.
                for y in 0..h {
                    let t = y as f32 / h as f32;
                    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
                    let px = image::Rgba([mix(0x1d, 0x0e), mix(0x2b, 0x4d), mix(0x3a, 0x6e), 255]);
                    for x in 0..w {
                        canvas.put_pixel(x, y, px);
                    }
                }
            }
            None => {}
        }
        // RGBA → BGRA for GDI.
        let mut px = canvas.into_raw();
        for p in px.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
        *WALLPAPER.lock().unwrap_or_else(|p| p.into_inner()) = Some((w as i32, h as i32, px));
        unsafe { PostMessageW(hwnd as HWND, WM_WALLPAPER, 0, 0) };
    });
}

/// Turn the decoded picture into a bitmap the window draws from.
unsafe fn take_wallpaper(hwnd: HWND) {
    let Some((w, h, px)) = WALLPAPER.lock().unwrap_or_else(|p| p.into_inner()).take() else { return };
    let screen = GetDC(hwnd);
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h; // top-down
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    // A device bitmap: the copy lives with the display driver, not in the
    // process, and drawing it is a plain BitBlt.
    let bmp = CreateCompatibleBitmap(screen, w, h);
    SetDIBits(screen, bmp, 0, h as u32, px.as_ptr() as *const c_void, &bi, DIB_RGB_COLORS);
    drop(px);
    let dc = CreateCompatibleDC(screen);
    let old = SelectObject(dc, bmp);
    ReleaseDC(hwnd, screen);
    WALL.with(|wall| {
        if let Some((odc, obmp, oold, _, _)) = wall.borrow_mut().replace((dc, bmp, old, w, h)) {
            SelectObject(odc, oold);
            DeleteObject(obmp);
            DeleteDC(odc);
        }
    });
    InvalidateRect(hwnd, null(), 0);
}

/// Draw the wallpaper (or black until it is ready).
unsafe fn draw_wallpaper(dc: HDC, rc: &RECT) {
    let drawn = WALL.with(|wall| {
        if let Some((wdc, _, _, w, h)) = *wall.borrow() {
            BitBlt(dc, 0, 0, w.min(rc.right), h.min(rc.bottom), wdc, 0, 0, SRCCOPY);
            true
        } else {
            false
        }
    });
    if !drawn {
        FillRect(dc, rc, GetStockObject(BLACK_BRUSH) as HBRUSH);
    }
}
