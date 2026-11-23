//! Window snapping, which is Explorer's job in Windows and so was missing:
//! Win+arrows (halves, quarters, maximize, restore, minimize) and dragging a
//! window to a screen edge or corner, with a see-through preview of where
//! it will land.

use std::collections::HashMap;
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Zone {
    Left,
    Right,
    Max,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Where each snapped window was before, to put it back.
static RESTORE: Mutex<Option<HashMap<isize, RECT>>> = Mutex::new(None);

fn remember(h: HWND, r: RECT) {
    let mut m = RESTORE.lock().unwrap_or_else(|p| p.into_inner());
    m.get_or_insert_with(HashMap::new).entry(h as isize).or_insert(r);
}

fn forget(h: HWND) -> Option<RECT> {
    RESTORE.lock().unwrap_or_else(|p| p.into_inner()).as_mut().and_then(|m| m.remove(&(h as isize)))
}

unsafe fn monitor_of(m: windows_sys::Win32::Graphics::Gdi::HMONITOR) -> (RECT, RECT) {
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(m, &mut mi);
    (mi.rcMonitor, mi.rcWork)
}

fn zone_rect(work: RECT, z: Zone) -> RECT {
    let (l, t, r, b) = (work.left, work.top, work.right, work.bottom);
    let (mx, my) = ((l + r) / 2, (t + b) / 2);
    match z {
        Zone::Left => RECT { left: l, top: t, right: mx, bottom: b },
        Zone::Right => RECT { left: mx, top: t, right: r, bottom: b },
        Zone::Max => work,
        Zone::TopLeft => RECT { left: l, top: t, right: mx, bottom: my },
        Zone::TopRight => RECT { left: mx, top: t, right: r, bottom: my },
        Zone::BottomLeft => RECT { left: l, top: my, right: mx, bottom: b },
        Zone::BottomRight => RECT { left: mx, top: my, right: r, bottom: b },
    }
}

/// The visible bounds (without Windows 10/11's invisible resize borders).
unsafe fn visible_rect(h: HWND) -> RECT {
    let mut r: RECT = std::mem::zeroed();
    if windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(h, 9 /* EXTENDED_FRAME_BOUNDS */, &mut r as *mut _ as *mut _, std::mem::size_of::<RECT>() as u32) != 0 {
        GetWindowRect(h, &mut r);
    }
    r
}

/// Move a window so that its *visible* edges land on `target`.
unsafe fn place(h: HWND, target: RECT) {
    if IsZoomed(h) != 0 || IsIconic(h) != 0 {
        ShowWindow(h, SW_RESTORE);
    }
    let mut win: RECT = std::mem::zeroed();
    GetWindowRect(h, &mut win);
    let vis = visible_rect(h);
    let (dl, dt, dr, db) = (vis.left - win.left, vis.top - win.top, win.right - vis.right, win.bottom - vis.bottom);
    SetWindowPos(
        h,
        null_mut(),
        target.left - dl,
        target.top - dt,
        (target.right - target.left) + dl + dr,
        (target.bottom - target.top) + dt + db,
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
}

/// Which zone a window sits in now, if any.
unsafe fn current_zone(h: HWND) -> Option<Zone> {
    if IsZoomed(h) != 0 {
        return Some(Zone::Max);
    }
    let (_, work) = monitor_of(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST));
    let v = visible_rect(h);
    let near = |a: RECT, b: RECT| (a.left - b.left).abs() <= 8 && (a.top - b.top).abs() <= 8 && (a.right - b.right).abs() <= 8 && (a.bottom - b.bottom).abs() <= 8;
    [Zone::Left, Zone::Right, Zone::TopLeft, Zone::TopRight, Zone::BottomLeft, Zone::BottomRight]
        .into_iter()
        .find(|&z| near(v, zone_rect(work, z)))
}

/// A window that can be snapped: a normal, resizable, top-level one.
unsafe fn snappable(h: HWND) -> bool {
    if h.is_null() || IsWindowVisible(h) == 0 {
        return false;
    }
    let style = GetWindowLongW(h, GWL_STYLE) as u32;
    let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
    style & WS_THICKFRAME != 0 && style & WS_CHILD == 0 && ex & WS_EX_TOOLWINDOW == 0
}

unsafe fn snap_to(h: HWND, z: Zone) {
    if current_zone(h).is_none() && IsZoomed(h) == 0 {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(h, &mut r);
        remember(h, visible_rect(h));
        let _ = r;
    }
    if z == Zone::Max {
        ShowWindow(h, SW_MAXIMIZE);
        return;
    }
    let (_, work) = monitor_of(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST));
    place(h, zone_rect(work, z));
}

unsafe fn restore(h: HWND) {
    match forget(h) {
        Some(r) => place(h, r),
        None => {
            ShowWindow(h, SW_RESTORE);
        }
    }
}

/// Win+arrow on the window in front, the way Windows 11 does it.
pub fn key(dir: Dir) {
    unsafe {
        let h = GetForegroundWindow();
        let mut pid = 0;
        GetWindowThreadProcessId(h, &mut pid);
        if pid == std::process::id() {
            return;
        }
        if !snappable(h) {
            // Fixed-size windows can still be minimized and restored.
            match dir {
                Dir::Down => {
                    ShowWindow(h, SW_MINIMIZE);
                }
                Dir::Up if IsIconic(h) != 0 => {
                    ShowWindow(h, SW_RESTORE);
                }
                _ => {}
            }
            return;
        }
        let z = current_zone(h);
        match (dir, z) {
            (Dir::Left, Some(Zone::Right | Zone::TopRight | Zone::BottomRight)) => restore(h),
            (Dir::Left, Some(Zone::Left)) => {}
            (Dir::Left, Some(Zone::TopLeft)) => snap_to(h, Zone::TopLeft),
            (Dir::Left, Some(Zone::BottomLeft)) => snap_to(h, Zone::BottomLeft),
            (Dir::Left, _) => snap_to(h, Zone::Left),
            (Dir::Right, Some(Zone::Left | Zone::TopLeft | Zone::BottomLeft)) => restore(h),
            (Dir::Right, Some(Zone::Right)) => {}
            (Dir::Right, Some(Zone::TopRight)) => snap_to(h, Zone::TopRight),
            (Dir::Right, Some(Zone::BottomRight)) => snap_to(h, Zone::BottomRight),
            (Dir::Right, _) => snap_to(h, Zone::Right),
            (Dir::Up, Some(Zone::Left)) => snap_to(h, Zone::TopLeft),
            (Dir::Up, Some(Zone::Right)) => snap_to(h, Zone::TopRight),
            (Dir::Up, Some(Zone::BottomLeft)) => snap_to(h, Zone::Left),
            (Dir::Up, Some(Zone::BottomRight)) => snap_to(h, Zone::Right),
            (Dir::Up, Some(Zone::TopLeft | Zone::TopRight | Zone::Max)) => {}
            (Dir::Up, None) => snap_to(h, Zone::Max),
            (Dir::Down, Some(Zone::Max)) => restore(h),
            (Dir::Down, Some(Zone::Left)) => snap_to(h, Zone::BottomLeft),
            (Dir::Down, Some(Zone::Right)) => snap_to(h, Zone::BottomRight),
            (Dir::Down, Some(Zone::TopLeft)) => snap_to(h, Zone::Left),
            (Dir::Down, Some(Zone::TopRight)) => snap_to(h, Zone::Right),
            (Dir::Down, _) => {
                ShowWindow(h, SW_MINIMIZE);
            }
        }
    }
}

// ------------------------------------------------------------------ dragging to an edge

struct Drag {
    hwnd: HWND,
    size: (i32, i32),
    start: RECT,
    zone: Option<Zone>,
}

thread_local! {
    static DRAG: std::cell::RefCell<Option<Drag>> = const { std::cell::RefCell::new(None) };
    static PREVIEW: std::cell::Cell<HWND> = const { std::cell::Cell::new(null_mut()) };
}

thread_local! {
    static TIMER: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// From the window-event hook (its thread): a window started or stopped
/// being moved or resized.
pub fn move_size(event: u32, h: HWND) {
    unsafe {
        if event == 0x000A {
            // EVENT_SYSTEM_MOVESIZESTART
            if !snappable(h) {
                return;
            }
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(h, &mut r);
            DRAG.with(|d| *d.borrow_mut() = Some(Drag { hwnd: h, size: (r.right - r.left, r.bottom - r.top), start: visible_rect(h), zone: None }));
            TIMER.with(|t| {
                if t.get() != 0 {
                    KillTimer(null_mut(), t.get());
                }
                t.set(SetTimer(null_mut(), 0, 30, Some(tick)));
            });
        } else {
            // EVENT_SYSTEM_MOVESIZEEND
            TIMER.with(|t| {
                if t.get() != 0 {
                    KillTimer(null_mut(), t.get());
                    t.set(0);
                }
            });
            show_preview(None, null_mut());
            let drag = DRAG.with(|d| d.borrow_mut().take());
            if let Some(d) = drag {
                if d.hwnd == h {
                    if let Some(z) = d.zone {
                        remember(h, d.start);
                        if z == Zone::Max {
                            ShowWindow(h, SW_MAXIMIZE);
                        } else {
                            let (_, work) = monitor_of(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST));
                            place(h, zone_rect(work, z));
                        }
                    }
                }
            }
        }
    }
}

/// While a window is dragged: is the pointer at an edge or corner?
unsafe extern "system" fn tick(_: HWND, _: u32, _: usize, _: u32) {
    let mut pt: POINT = std::mem::zeroed();
    GetCursorPos(&mut pt);
    let (hwnd, moving) = DRAG.with(|d| {
        let d = d.borrow();
        let Some(d) = d.as_ref() else { return (null_mut(), false) };
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(d.hwnd, &mut r);
        // Only moving, not resizing.
        (d.hwnd, (r.right - r.left, r.bottom - r.top) == d.size)
    });
    if hwnd.is_null() {
        return;
    }
    let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let (full, work) = monitor_of(mon);
    let e = 6;
    let (at_l, at_r, at_t) = (pt.x <= full.left + e, pt.x >= full.right - 1 - e, pt.y <= full.top + e);
    let corner = 120;
    let (near_t, near_b) = (pt.y <= work.top + corner, pt.y >= work.bottom - corner);
    let zone = if !moving {
        None
    } else if at_l && near_t {
        Some(Zone::TopLeft)
    } else if at_l && near_b {
        Some(Zone::BottomLeft)
    } else if at_r && near_t {
        Some(Zone::TopRight)
    } else if at_r && near_b {
        Some(Zone::BottomRight)
    } else if at_l {
        Some(Zone::Left)
    } else if at_r {
        Some(Zone::Right)
    } else if at_t {
        Some(Zone::Max)
    } else {
        None
    };
    let changed = DRAG.with(|d| {
        let mut d = d.borrow_mut();
        let Some(d) = d.as_mut() else { return false };
        let c = d.zone != zone;
        d.zone = zone;
        c
    });
    if changed {
        show_preview(zone.map(|z| zone_rect(work, z)), hwnd);
    }
}

/// The see-through rectangle showing where the window will go (just below it).
unsafe fn show_preview(r: Option<RECT>, below: HWND) {
    let w = PREVIEW.with(|p| {
        if p.get().is_null() {
            p.set(create_preview());
        }
        p.get()
    });
    if w.is_null() {
        return;
    }
    match r {
        Some(r) => {
            let pad = 6;
            SetWindowPos(
                w,
                if below.is_null() { HWND_TOPMOST } else { below },
                r.left + pad,
                r.top + pad,
                r.right - r.left - 2 * pad,
                r.bottom - r.top - 2 * pad,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
        None => {
            ShowWindow(w, SW_HIDE);
        }
    }
}

unsafe fn create_preview() -> HWND {
    use windows_sys::Win32::Graphics::Gdi::CreateSolidBrush;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    let class: Vec<u16> = "DelfinSnapPreview".encode_utf16().chain(Some(0)).collect();
    let a = crate::theme::ACCENT;
    let wc = WNDCLASSW {
        lpfnWndProc: Some(DefWindowProcW),
        hInstance: GetModuleHandleW(null()),
        hbrBackground: CreateSolidBrush((a.r() as u32) | ((a.g() as u32) << 8) | ((a.b() as u32) << 16)),
        lpszClassName: class.as_ptr(),
        ..std::mem::zeroed()
    };
    RegisterClassW(&wc);
    let h = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        class.as_ptr(),
        null(),
        WS_POPUP,
        0,
        0,
        10,
        10,
        null_mut(),
        null_mut(),
        GetModuleHandleW(null()),
        null(),
    );
    if !h.is_null() {
        SetLayeredWindowAttributes(h, 0, 70, LWA_ALPHA);
        super::win::round_corners(h as isize);
    }
    h
}
