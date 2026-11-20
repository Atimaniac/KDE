//! Real Windows icons for files that carry their own: programs (.exe),
//! shortcuts (.lnk, .url), icon files (.ico) and a few more. Everything else
//! keeps Delfin's Breeze-style painted icons.
//!
//! Icons are read on a background thread (folders full of programs would
//! otherwise stutter) and cached per path and size.

use crate::theme::Palette;
use eframe::egui::{self, Painter, Rect, TextureHandle};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

/// Extensions whose icon is stored in the file itself.
pub fn has_own_icon(ext: &str) -> bool {
    matches!(ext, "exe" | "lnk" | "url" | "ico" | "msi" | "appref-ms" | "cpl" | "scr" | "cur")
}

type Pixels = (usize, usize, Vec<u8>);

struct Cache {
    tex: HashMap<(PathBuf, u8), Option<TextureHandle>>,
    pending: HashSet<(PathBuf, u8)>,
    tx: Sender<(PathBuf, u8)>,
    done: Arc<Mutex<Vec<((PathBuf, u8), Option<Pixels>)>>>,
}

thread_local! {
    static CACHE: RefCell<Option<Cache>> = const { RefCell::new(None) };
}

fn start(ctx: &egui::Context) -> Cache {
    let (tx, rx) = channel::<(PathBuf, u8)>();
    let done = Arc::new(Mutex::new(Vec::new()));
    let d = done.clone();
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("delfin-file-icons".into())
        .spawn(move || {
            #[cfg(windows)]
            unsafe {
                windows_sys::Win32::System::Com::CoInitializeEx(std::ptr::null(), 0x2);
            }
            while let Ok((path, tier)) = rx.recv() {
                let px = load(&path, tier);
                d.lock().unwrap_or_else(|p| p.into_inner()).push(((path, tier), px));
                ctx.request_repaint();
            }
        })
        .ok();
    Cache { tex: HashMap::new(), pending: HashSet::new(), tx, done }
}

/// The file's icon as Windows shows it; tier 0 = 16 px, 1 = 32 px, 2 = 48 px.
#[cfg(windows)]
fn load(path: &Path, tier: u8) -> Option<Pixels> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Controls::ImageList_GetIcon;
    use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHGetImageList, SHFILEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon;
    const IID_IIMAGELIST: windows_sys::core::GUID = windows_sys::core::GUID::from_u128(0x46eb5926_582e_4017_9fdf_e8998daa0950);
    let w: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut info: SHFILEINFOW = std::mem::zeroed();
        if SHGetFileInfoW(w.as_ptr(), 0, &mut info, std::mem::size_of::<SHFILEINFOW>() as u32, 0x4000 /* SHGFI_SYSICONINDEX */) == 0 {
            return None;
        }
        let mut list: *mut std::ffi::c_void = std::ptr::null_mut();
        let which = match tier { 0 => 1 /* SHIL_SMALL */, 1 => 0 /* SHIL_LARGE */, _ => 2 /* SHIL_EXTRALARGE */ };
        if SHGetImageList(which, &IID_IIMAGELIST, &mut list) < 0 || list.is_null() {
            return None;
        }
        let icon = ImageList_GetIcon(list as isize, info.iIcon, 0x1 /* ILD_TRANSPARENT */);
        if icon.is_null() {
            return None;
        }
        let px = crate::shell::win::icon_rgba(icon as isize);
        DestroyIcon(icon);
        px
    }
}

#[cfg(not(windows))]
fn load(_path: &Path, _tier: u8) -> Option<Pixels> {
    None
}

/// Paint a file's icon: the real one for programs, shortcuts and .ico files
/// (once it has loaded), Delfin's painted icon for everything else.
pub fn paint(p: &Painter, rect: Rect, path: &Path, is_dir: bool, ext: &str, pal: &Palette) {
    if is_dir || !has_own_icon(ext) {
        crate::icons::paint_entry_icon(p, rect, is_dir, ext, pal);
        return;
    }
    let w = rect.width();
    let tier = if w <= 24.0 { 0 } else if w <= 40.0 { 1 } else { 2 };
    let key = (path.to_path_buf(), tier);
    let tex = CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let cache = c.get_or_insert_with(|| start(p.ctx()));
        // Upload what the loader finished.
        let finished: Vec<_> = std::mem::take(&mut *cache.done.lock().unwrap_or_else(|e| e.into_inner()));
        for (k, px) in finished {
            cache.pending.remove(&k);
            let t = px.map(|(w, h, data)| {
                p.ctx().load_texture(
                    format!("fileicon:{}:{}", k.0.display(), k.1),
                    egui::ColorImage::from_rgba_unmultiplied([w, h], &data),
                    egui::TextureOptions::LINEAR,
                )
            });
            cache.tex.insert(k, t);
        }
        match cache.tex.get(&key) {
            Some(t) => t.as_ref().map(|t| t.id()),
            None => {
                if cache.pending.insert(key.clone()) {
                    let _ = cache.tx.send(key.clone());
                }
                None
            }
        }
    });
    match tex {
        Some(id) => {
            // Keep the icon's own proportions, centred in the cell.
            let side = rect.width().min(rect.height());
            let r = Rect::from_center_size(rect.center(), egui::vec2(side, side));
            p.image(id, r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        }
        None => crate::icons::paint_entry_icon(p, rect, is_dir, ext, pal),
    }
}

