//! Notification area (system tray) host.
//!
//! Programs add tray icons with `Shell_NotifyIcon`, which finds the window of
//! class `Shell_TrayWnd` and sends it a WM_COPYDATA message. Explorer normally
//! owns that window; in shell mode we create it ourselves, decode the messages
//! the same way Explorer does, and send mouse clicks back to the program.

use super::win::{icon_rgba, pid_of, wide};
use eframe::egui;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW,
    DispatchMessageW, GetMessageW, PostMessageW, RegisterClassW, RegisterWindowMessageW,
    SendNotifyMessageW, TranslateMessage, MSG, WNDCLASSW,
};

// Shell_NotifyIcon messages
const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIM_DELETE: u32 = 2;
const NIM_SETFOCUS: u32 = 3;
const NIM_SETVERSION: u32 = 4;
// NOTIFYICONDATA flags
const NIF_MESSAGE: u32 = 0x01;
const NIF_ICON: u32 = 0x02;
const NIF_TIP: u32 = 0x04;
const NIF_STATE: u32 = 0x08;
const NIF_INFO: u32 = 0x10;
const NIF_GUID: u32 = 0x20;
const NIS_HIDDEN: u32 = 0x01;
// Callback notifications
const NIN_SELECT: u32 = 0x0400;
pub const NIN_BALLOONTIMEOUT: u32 = 0x0404;
pub const NIN_BALLOONUSERCLICK: u32 = 0x0405;
const WM_CONTEXTMENU: u32 = 0x007B;

/// NOTIFYICONDATAW as it travels inside WM_COPYDATA: handles are always
/// 32-bit, whatever the bitness of the sending program.
#[repr(C)]
#[derive(Clone, Copy)]
struct NotifyIconData32 {
    cb_size: u32,
    hwnd: u32,
    uid: u32,
    flags: u32,
    callback: u32,
    hicon: u32,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    timeout_or_version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid: [u8; 16],
    balloon_icon: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TrayData {
    signature: u32,
    message: u32,
    nid: NotifyIconData32,
}

#[repr(C)]
struct CopyData {
    dw_data: usize,
    cb_data: u32,
    lp_data: *const c_void,
}

#[derive(Clone)]
pub struct TrayIcon {
    pub hwnd: isize,
    pub uid: u32,
    guid: Option<[u8; 16]>,
    pub callback: u32,
    pub version: u32,
    pub tip: String,
    pub hidden: bool,
    pub icon: Option<(usize, usize, Arc<Vec<u8>>)>,
    pub icon_gen: u64,
}

#[derive(Clone)]
pub struct Balloon {
    pub hwnd: isize,
    pub uid: u32,
    pub callback: u32,
    pub version: u32,
    pub title: String,
    pub text: String,
}

#[derive(Default)]
pub struct TrayState {
    pub icons: Vec<TrayIcon>,
    pub balloons: Vec<Balloon>,
    gen: u64,
}

pub type Shared = Arc<Mutex<TrayState>>;

static STATE: OnceLock<Shared> = OnceLock::new();
static CTX: OnceLock<egui::Context> = OnceLock::new();

fn wstr(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// 32-bit handles are sign-extended to 64-bit.
fn handle(v: u32) -> isize {
    v as i32 as isize
}

fn apply(ic: &mut TrayIcon, nid: &NotifyIconData32, gen: &mut u64) {
    if nid.flags & NIF_MESSAGE != 0 {
        ic.callback = nid.callback;
    }
    if nid.flags & NIF_ICON != 0 {
        // Copy the pixels now: the program may destroy the icon right after.
        *gen += 1;
        ic.icon = icon_rgba(handle(nid.hicon)).map(|(w, h, px)| (w, h, Arc::new(px)));
        ic.icon_gen = *gen;
    }
    if nid.flags & NIF_TIP != 0 {
        ic.tip = wstr(&nid.tip);
    }
    if nid.flags & NIF_STATE != 0 && nid.state_mask & NIS_HIDDEN != 0 {
        ic.hidden = nid.state & NIS_HIDDEN != 0;
    }
}

unsafe fn handle_notify(cds: &CopyData) -> bool {
    let mut td: TrayData = std::mem::zeroed();
    let n = (cds.cb_data as usize).min(std::mem::size_of::<TrayData>());
    std::ptr::copy_nonoverlapping(
        cds.lp_data as *const u8,
        &mut td as *mut TrayData as *mut u8,
        n,
    );
    if td.signature != 0x3475_3423 {
        return false;
    }
    let nid = td.nid;
    let hwnd = handle(nid.hwnd);
    let guid = (nid.flags & NIF_GUID != 0).then_some(nid.guid);

    let Some(shared) = STATE.get() else {
        return false;
    };
    let mut guard = match shared.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let st = &mut *guard;
    let pos = st.icons.iter().position(|i| match (guid, i.guid) {
        (Some(a), Some(b)) => a == b,
        _ => i.hwnd == hwnd && i.uid == nid.uid,
    });

    let ok = match td.message {
        NIM_ADD => {
            match pos {
                Some(p) => apply(&mut st.icons[p], &nid, &mut st.gen),
                None => {
                    let mut ic = TrayIcon {
                        hwnd,
                        uid: nid.uid,
                        guid,
                        callback: 0,
                        version: 0,
                        tip: String::new(),
                        hidden: false,
                        icon: None,
                        icon_gen: 0,
                    };
                    apply(&mut ic, &nid, &mut st.gen);
                    st.icons.push(ic);
                }
            }
            true
        }
        NIM_MODIFY => match pos {
            Some(p) => {
                apply(&mut st.icons[p], &nid, &mut st.gen);
                true
            }
            None => false, // tells the program to NIM_ADD again
        },
        NIM_DELETE => match pos {
            Some(p) => {
                st.icons.remove(p);
                true
            }
            None => false,
        },
        NIM_SETFOCUS => true,
        NIM_SETVERSION => match pos {
            Some(p) => {
                st.icons[p].version = nid.timeout_or_version;
                true
            }
            None => false,
        },
        _ => false,
    };

    // Balloon / notification text.
    if ok && (td.message == NIM_ADD || td.message == NIM_MODIFY) && nid.flags & NIF_INFO != 0 {
        let text = wstr(&nid.info);
        if !text.is_empty() {
            let (callback, version) = pos
                .and_then(|p| st.icons.get(p))
                .or_else(|| st.icons.last())
                .map(|i| (i.callback, i.version))
                .unwrap_or((nid.callback, 0));
            st.balloons.push(Balloon {
                hwnd,
                uid: nid.uid,
                callback,
                version,
                title: wstr(&nid.info_title),
                text,
            });
        }
    }
    drop(guard);
    if let Some(c) = CTX.get() {
        c.request_repaint();
    }
    ok
}

/// Screen rectangles of the icons as last drawn on the panel, in pixels.
static ICON_RECTS: Mutex<Vec<((isize, u32), [i32; 4])>> = Mutex::new(Vec::new());

/// Called by the panel while drawing: where an icon is on screen (pixels).
pub fn set_icon_rect(hwnd: isize, uid: u32, rect: [i32; 4]) {
    let mut v = ICON_RECTS.lock().unwrap_or_else(|p| p.into_inner());
    match v.iter_mut().find(|(k, _)| *k == (hwnd, uid)) {
        Some(e) => e.1 = rect,
        None => v.push(((hwnd, uid), rect)),
    }
}

/// Shell_NotifyIconGetRect: a program asks where its icon is. Many programs
/// (everything built on the `tray-icon` crate, Electron apps, …) do this on
/// every click and ignore the click if the answer is "nowhere".
///
/// The request is NOTIFYICONIDENTIFIER with a 32-bit header:
/// { magic, message (1 = top-left, 2 = bottom-right), cbSize, hWnd, uID, guid }
/// and the answer is the point packed as MAKELONG(x, y).
unsafe fn icon_rect_query(cds: &CopyData) -> LRESULT {
    let mut buf = [0u8; 36];
    let n = (cds.cb_data as usize).min(buf.len());
    std::ptr::copy_nonoverlapping(cds.lp_data as *const u8, buf.as_mut_ptr(), n);
    let u = |o: usize| u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
    if u(0) != 0x3475_3423 {
        return 0;
    }
    let (which, hwnd, uid) = (u(4), handle(u(12)), u(16));
    let mut guid = [0u8; 16];
    guid.copy_from_slice(&buf[20..36]);

    // Is it one of our icons? (Match by GUID when the program uses one.)
    let known = STATE.get().is_some_and(|s| {
        let st = s.lock().unwrap_or_else(|p| p.into_inner());
        st.icons.iter().any(|i| (i.hwnd == hwnd && i.uid == uid) || i.guid == Some(guid))
    });
    if !known {
        return 0;
    }
    let rect = ICON_RECTS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .find(|(k, _)| *k == (hwnd, uid))
        .map(|(_, r)| *r)
        .unwrap_or_else(|| {
            // Not drawn yet (e.g. behind the ^ overflow): use the mouse position.
            let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
            windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt);
            [pt.x - 12, pt.y - 12, pt.x + 12, pt.y + 12]
        });
    let pack = |x: i32, y: i32| ((x as u16 as u32) | ((y as u16 as u32) << 16)) as i32 as LRESULT;
    match which {
        1 => pack(rect[0], rect[1]),
        2 => pack(rect[2], rect[3]),
        _ => 0,
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == 0x004A {
        // WM_COPYDATA: 0 = app bar, 1 = notify icon, 3 = icon position query
        let cds = &*(lparam as *const CopyData);
        if cds.dw_data == 1 && !cds.lp_data.is_null() && cds.cb_data >= 32 {
            return handle_notify(cds) as LRESULT;
        }
        if cds.dw_data == 3 && !cds.lp_data.is_null() && cds.cb_data >= 20 {
            return icon_rect_query(cds);
        }
        return 0;
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Create the hidden `Shell_TrayWnd` window on its own thread.
pub fn start(ctx: egui::Context) -> Shared {
    let shared: Shared = Arc::new(Mutex::new(TrayState::default()));
    let _ = STATE.set(shared.clone());
    let _ = CTX.set(ctx);

    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || unsafe {
        let class = wide("Shell_TrayWnd");
        let hinst = GetModuleHandleW(null());
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            0x0000_0080 | 0x0000_0008, // WS_EX_TOOLWINDOW | WS_EX_TOPMOST
            class.as_ptr(),
            null(),
            0x8000_0000, // WS_POPUP (never shown)
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        // Let sandboxed / low-integrity programs reach us.
        let taskbar_created = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
        ChangeWindowMessageFilterEx(hwnd, 0x004A, 1, null_mut());
        ChangeWindowMessageFilterEx(hwnd, taskbar_created, 1, null_mut());
        let _ = tx.send(());
        // Ask programs that are already running to add their icons again.
        SendNotifyMessageW(
            0xffffusize as HWND, /* HWND_BROADCAST */
            taskbar_created,
            0,
            0,
        );

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
    let _ = rx.recv_timeout(Duration::from_secs(2));
    shared
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Click {
    Left,
    Double,
    Right,
    Middle,
    Hover,
}

fn makelong(lo: i32, hi: i32) -> u32 {
    ((hi as u32 & 0xffff) << 16) | (lo as u32 & 0xffff)
}

/// Post one callback message to the program, in the format it asked for.
pub fn post(hwnd: isize, uid: u32, callback: u32, version: u32, m: u32, anchor: (i32, i32)) {
    if callback == 0 {
        return;
    }
    let (w, l) = if version >= 4 {
        (
            makelong(anchor.0, anchor.1) as usize,
            makelong(m as i32, uid as i32) as isize,
        )
    } else {
        (uid as usize, m as isize)
    };
    unsafe {
        PostMessageW(hwnd as HWND, callback, w, l);
    }
}

/// Send a click on a tray icon, the way Explorer does.
pub fn click(ic: &TrayIcon, kind: Click, anchor: (i32, i32)) {
    if kind != Click::Hover {
        // Allow the program to bring its window / menu to the front.
        unsafe {
            AllowSetForegroundWindow(pid_of(ic.hwnd));
        }
    }
    let p = |m: u32| post(ic.hwnd, ic.uid, ic.callback, ic.version, m, anchor);
    match kind {
        Click::Left => {
            p(0x0201); // WM_LBUTTONDOWN
            p(0x0202); // WM_LBUTTONUP
            if ic.version >= 3 {
                p(NIN_SELECT);
            }
        }
        Click::Double => {
            p(0x0203); // WM_LBUTTONDBLCLK
            p(0x0202);
        }
        Click::Right => {
            p(0x0204); // WM_RBUTTONDOWN
            p(0x0205); // WM_RBUTTONUP
            if ic.version >= 3 {
                p(WM_CONTEXTMENU);
            }
        }
        Click::Middle => {
            p(0x0207);
            p(0x0208);
        }
        Click::Hover => p(0x0200), // WM_MOUSEMOVE
    }
}
