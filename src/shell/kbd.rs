//! The keyboard language indicator ("PL", "EN"), which Explorer shows on the
//! taskbar: which layout the window in front uses, and switching it.

use std::ptr::null_mut;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, GetKeyboardLayoutList, HKL};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, PostMessageW};

/// The layout of the window in front, kept up to date by `watch`.
static CURRENT: AtomicIsize = AtomicIsize::new(0);
/// The last window in front that isn't Delfin's (the panel takes the focus
/// when clicked, but the language belongs to the app you were typing in).
static TARGET: AtomicIsize = AtomicIsize::new(0);

#[derive(Clone)]
pub struct Layout {
    pub hkl: isize,
    pub short: String,
    pub name: String,
}

/// Installed keyboard layouts.
pub fn layouts() -> Vec<Layout> {
    unsafe {
        let n = GetKeyboardLayoutList(0, null_mut());
        let mut list: Vec<HKL> = vec![null_mut(); n.max(0) as usize];
        let n = GetKeyboardLayoutList(n, list.as_mut_ptr());
        list.truncate(n.max(0) as usize);
        list.into_iter().map(|h| describe(h as isize)).collect()
    }
}

fn describe(hkl: isize) -> Layout {
    use windows_sys::Win32::Globalization::{GetLocaleInfoEx, LCIDToLocaleName};
    let lang = (hkl as usize & 0xffff) as u32;
    let mut name = [0u16; 85];
    unsafe {
        let n = LCIDToLocaleName(lang, name.as_mut_ptr(), name.len() as i32, 0);
        let locale = &name[..(n.max(1) - 1) as usize];
        let get = |what: u32| {
            let mut buf = [0u16; 128];
            let mut loc: Vec<u16> = locale.to_vec();
            loc.push(0);
            let k = GetLocaleInfoEx(loc.as_ptr(), what, buf.as_mut_ptr(), buf.len() as i32);
            String::from_utf16_lossy(&buf[..(k.max(1) - 1) as usize])
        };
        // LOCALE_SISO639LANGNAME, LOCALE_SLOCALIZEDDISPLAYNAME
        let short = get(0x59).to_uppercase();
        let full = get(0x2);
        Layout { hkl, short: if short.is_empty() { format!("{lang:04X}") } else { short }, name: full }
    }
}

/// The layout to show on the panel.
pub fn current() -> Option<Layout> {
    let h = CURRENT.load(Ordering::Relaxed);
    (h != 0).then(|| describe(h))
}

/// Watch the window in front and its layout (two checks a second, no
/// drawing unless it changed).
pub fn watch(ctx: eframe::egui::Context) {
    std::thread::Builder::new()
        .name("delfin-keyboard".into())
        .spawn(move || loop {
            unsafe {
                let fg = GetForegroundWindow();
                let mut pid = 0;
                let tid = GetWindowThreadProcessId(fg, &mut pid);
                if !fg.is_null() && pid != std::process::id() {
                    TARGET.store(fg as isize, Ordering::Relaxed);
                    let hkl = GetKeyboardLayout(tid) as isize;
                    if hkl != 0 && CURRENT.swap(hkl, Ordering::Relaxed) != hkl {
                        ctx.request_repaint();
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        })
        .ok();
}

/// Switch the app you were typing in to another layout, and give it the focus back.
pub fn switch(hkl: isize) {
    let w = TARGET.load(Ordering::Relaxed);
    if w == 0 {
        return;
    }
    unsafe {
        // WM_INPUTLANGCHANGEREQUEST
        PostMessageW(w as _, 0x0050, 0, hkl);
    }
    CURRENT.store(hkl, Ordering::Relaxed);
    super::win::switch_to(w);
}
