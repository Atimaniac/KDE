//! Opening and closing effects for Delfin's pop-ups (start menu, quick
//! settings, calendar, notifications, menus…), chosen in Settings next to
//! Aero.
//!
//! The pop-up itself is an OpenGL window, so it isn't moved or scaled frame
//! by frame. Instead a picture of it is animated in a light layered GDI
//! window on its own thread: when it opens, the real window waits invisible
//! (alpha 0) until the animation ends; when it closes, its last frame fades
//! / slides / shrinks away after the real window is gone.

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HWND, POINT, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Effect {
    None,
    Fade,
    Slide,
    Glide,
    Zoom,
    Spring,
}

pub const EFFECTS: [Effect; 6] = [Effect::None, Effect::Fade, Effect::Slide, Effect::Glide, Effect::Zoom, Effect::Spring];

impl Effect {
    pub fn from_u8(v: u8) -> Self {
        EFFECTS.get(v as usize).copied().unwrap_or(Effect::Glide)
    }
    pub fn index(self) -> u8 {
        EFFECTS.iter().position(|&e| e == self).unwrap_or(0) as u8
    }
    /// English name (translated where shown).
    pub fn name(self) -> &'static str {
        match self {
            Effect::None => "None",
            Effect::Fade => "Fade",
            Effect::Slide => "Slide",
            Effect::Glide => "Glide",
            Effect::Zoom => "Grow",
            Effect::Spring => "Spring",
        }
    }
}

static EFFECT: AtomicU8 = AtomicU8::new(3);
static SPEED: AtomicU8 = AtomicU8::new(1);

/// Set by the shell from the settings.
pub fn configure(effect: u8, speed: u8) {
    EFFECT.store(effect, Ordering::Relaxed);
    SPEED.store(speed, Ordering::Relaxed);
}

fn effect() -> Effect {
    Effect::from_u8(EFFECT.load(Ordering::Relaxed))
}

fn duration() -> Duration {
    Duration::from_millis(match SPEED.load(Ordering::Relaxed) {
        0 => 120,
        2 => 320,
        _ => 200,
    })
}

/// Pop-ups that get the effects (not the tooltip or the volume pop-up).
pub fn animated(title: &str) -> bool {
    matches!(
        title,
        "Delfin Launcher" | "Delfin Quick Settings" | "Delfin Calendar" | "Delfin Media" | "Delfin Notifications" | "Delfin Menu" | "Delfin Tray" | "Delfin Toast" | "Delfin Notification"
    )
}

// ------------------------------------------------------------------ a picture of a window

struct Shot {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    w: i32,
    h: i32,
}

impl Shot {
    /// Nothing drawn yet: every sampled pixel has the same colour.
    fn blank(&self) -> bool {
        unsafe {
            let first = GetPixel(self.dc, 0, 0);
            let step = 7;
            for y in (0..self.h).step_by((self.h / step).max(1) as usize) {
                for x in (0..self.w).step_by((self.w / step).max(1) as usize) {
                    if GetPixel(self.dc, x, y) != first {
                        return false;
                    }
                }
            }
            true
        }
    }
}

impl Drop for Shot {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bmp);
            DeleteDC(self.dc);
        }
    }
}

/// The window's current picture (also works for OpenGL windows).
unsafe fn shot(h: HWND) -> Option<Shot> {
    let mut r: RECT = std::mem::zeroed();
    GetWindowRect(h, &mut r);
    let (w, hh) = (r.right - r.left, r.bottom - r.top);
    if w <= 0 || hh <= 0 {
        return None;
    }
    let screen = GetDC(null_mut());
    let dc = CreateCompatibleDC(screen);
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -hh;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    let mut bits: *mut c_void = null_mut();
    let bmp = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    ReleaseDC(null_mut(), screen);
    if bmp.is_null() || bits.is_null() {
        DeleteDC(dc);
        return None;
    }
    let old = SelectObject(dc, bmp);
    // PW_RENDERFULLCONTENT: what DWM shows, OpenGL included.
    windows_sys::Win32::Storage::Xps::PrintWindow(h, dc, 2);
    // Opaque everywhere (UpdateLayeredWindow wants premultiplied alpha).
    let px = std::slice::from_raw_parts_mut(bits as *mut u32, (w * hh) as usize);
    for p in px.iter_mut() {
        *p |= 0xff00_0000;
    }
    Some(Shot { dc, bmp, old, w, h: hh })
}

// ------------------------------------------------------------------ the animation

/// Position, size and opacity at progress `t` (0 → 1 opening; closing runs it backwards).
fn frame(e: Effect, t: f32, r: RECT) -> (RECT, u8) {
    let (w, h) = ((r.right - r.left) as f32, (r.bottom - r.top) as f32);
    let ease = 1.0 - (1.0 - t).powi(3);
    let back = {
        // ease-out-back: overshoots a little and settles
        let c1 = 1.70158;
        let c3 = c1 + 1.0;
        1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
    };
    let alpha = |a: f32| (a.clamp(0.0, 1.0) * 255.0) as u8;
    let shift = |dy: f32| RECT { left: r.left, top: r.top + dy as i32, right: r.right, bottom: r.bottom + dy as i32 };
    let scale = |s: f32| {
        let (dw, dh) = (w * (1.0 - s) / 2.0, h * (1.0 - s) / 2.0);
        RECT { left: r.left + dw as i32, top: r.top + dh as i32, right: r.right - dw as i32, bottom: r.bottom - dh as i32 }
    };
    match e {
        Effect::None => (r, 255),
        Effect::Fade => (r, alpha(ease)),
        Effect::Slide => (shift((1.0 - ease) * 40.0), 255),
        Effect::Glide => (shift((1.0 - ease) * 24.0), alpha(ease)),
        Effect::Zoom => (scale(0.86 + 0.14 * ease), alpha(ease)),
        Effect::Spring => (scale(0.80 + 0.20 * back), alpha(ease * 1.6)),
    }
}

unsafe fn ghost_window(r: RECT) -> HWND {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    static REGISTER: std::sync::Once = std::sync::Once::new();
    let class: Vec<u16> = "DelfinFx".encode_utf16().chain(Some(0)).collect();
    REGISTER.call_once(|| {
        let wc = WNDCLASSW { lpfnWndProc: Some(DefWindowProcW), hInstance: GetModuleHandleW(null()), lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
    });
    CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
        class.as_ptr(),
        null(),
        WS_POPUP,
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top,
        null_mut(),
        null_mut(),
        GetModuleHandleW(null()),
        null(),
    )
}

/// Draw the picture at `dst` with `alpha` into the ghost window.
unsafe fn present(ghost: HWND, s: &Shot, dst: RECT, alpha: u8) {
    let (w, h) = ((dst.right - dst.left).max(1), (dst.bottom - dst.top).max(1));
    let screen = GetDC(null_mut());
    let mem = CreateCompatibleDC(screen);
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    let mut bits: *mut c_void = null_mut();
    let bmp = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    let old = SelectObject(mem, bmp);
    SetStretchBltMode(mem, HALFTONE);
    StretchBlt(mem, 0, 0, w, h, s.dc, 0, 0, s.w, s.h, SRCCOPY);
    if !bits.is_null() {
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
        for p in px.iter_mut() {
            *p |= 0xff00_0000;
        }
    }
    let src = POINT { x: 0, y: 0 };
    let pos = POINT { x: dst.left, y: dst.top };
    let size = SIZE { cx: w, cy: h };
    let blend = BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: 1 /* AC_SRC_ALPHA */ };
    UpdateLayeredWindow(ghost, screen, &pos, &size, mem, &src, 0, &blend, ULW_ALPHA);
    SelectObject(mem, old);
    DeleteObject(bmp);
    DeleteDC(mem);
    ReleaseDC(null_mut(), screen);
}

/// Run the effect with the picture; `opening` plays it forward.
unsafe fn play(s: &Shot, r: RECT, e: Effect, opening: bool) {
    let ghost = ghost_window(r);
    if ghost.is_null() {
        return;
    }
    let d = duration();
    let start = Instant::now();
    loop {
        let k = (start.elapsed().as_secs_f32() / d.as_secs_f32()).min(1.0);
        let t = if opening { k } else { 1.0 - k };
        let (dst, a) = frame(e, t, r);
        present(ghost, s, dst, a);
        if k == 1.0 {
            break;
        }
        // Keep the ghost's own messages flowing.
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
            DispatchMessageW(&msg);
        }
        std::thread::sleep(Duration::from_millis(7));
    }
    DestroyWindow(ghost);
}

/// A pop-up is about to be shown: start it invisible and play the opening.
/// Called by `win::reveal_popup` just before ShowWindow.
pub fn prepare_open(h: HWND) {
    let e = effect();
    if e == Effect::None {
        return;
    }
    unsafe {
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_LAYERED as isize);
        SetLayeredWindowAttributes(h, 0, 0, LWA_ALPHA);
    }
    let hw = h as isize;
    std::thread::spawn(move || unsafe {
        let h = hw as HWND;
        // Take its picture as soon as it has drawn something (a frame takes
        // ~8 ms); a fixed wait used to hold every pop-up back by 45 ms.
        let mut r: RECT = std::mem::zeroed();
        let mut pic = None;
        let begin = Instant::now();
        while begin.elapsed() < Duration::from_millis(50) {
            std::thread::sleep(Duration::from_millis(10));
            GetWindowRect(h, &mut r);
            pic = shot(h);
            if pic.as_ref().is_some_and(|s| !s.blank()) {
                break;
            }
        }
        if let Some(s) = pic {
            play(&s, r, e, true);
        }
        if IsWindow(h) != 0 {
            SetLayeredWindowAttributes(h, 0, 255, LWA_ALPHA);
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
            SetWindowLongPtrW(h, GWL_EXSTYLE, ex & !(WS_EX_LAYERED as isize));
            RedrawWindow(h, null(), null_mut(), RDW_INVALIDATE | RDW_FRAME | RDW_ALLCHILDREN);
        }
    });
}

/// A pop-up is closing (its window still exists, about to go): play the
/// closing effect with its last picture.
pub fn play_close(title: &str) {
    let e = effect();
    if e == Effect::None {
        return;
    }
    let Some(h) = super::win::find_window(title) else { return };
    let h = h as HWND;
    unsafe {
        if IsWindowVisible(h) == 0 {
            return;
        }
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(h, &mut r);
        let Some(s) = shot(h) else { return };
        // The real window disappears at once; the picture plays the effect.
        ShowWindow(h, SW_HIDE);
        let s = SendShot(s);
        std::thread::spawn(move || {
            let s = s;
            play(&s.0, r, e, false);
        });
    }
}

struct SendShot(Shot);
unsafe impl Send for SendShot {}
