//! What the Properties window reads and writes: file times and attributes,
//! shortcut (.lnk) fields through IShellLinkW, version information,
//! the program's architecture and Windows compatibility settings.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use windows_sys::core::GUID;

pub fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

unsafe fn com_fn<T>(obj: *mut c_void, index: usize) -> T {
    let vtbl = *(obj as *const *const usize);
    std::mem::transmute_copy(&*vtbl.add(index))
}

unsafe fn release(obj: *mut c_void) {
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = com_fn(obj, 2);
    f(obj);
}

// ------------------------------------------------------------------ file

/// "Application", "Shortcut", … as Windows calls the file type.
pub fn type_name(path: &Path) -> String {
    use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW};
    let w = wide(path.as_os_str());
    unsafe {
        let mut info: SHFILEINFOW = std::mem::zeroed();
        if SHGetFileInfoW(w.as_ptr(), 0, &mut info, std::mem::size_of::<SHFILEINFOW>() as u32, 0x400 /* SHGFI_TYPENAME */) != 0 {
            return from_wide(&info.szTypeName);
        }
    }
    String::new()
}

pub const READONLY: u32 = 0x1;
pub const HIDDEN: u32 = 0x2;

pub fn attributes(path: &Path) -> u32 {
    let w = wide(path.as_os_str());
    let a = unsafe { windows_sys::Win32::Storage::FileSystem::GetFileAttributesW(w.as_ptr()) };
    if a == u32::MAX {
        0
    } else {
        a
    }
}

pub fn set_attributes(path: &Path, attrs: u32) -> bool {
    let w = wide(path.as_os_str());
    unsafe { windows_sys::Win32::Storage::FileSystem::SetFileAttributesW(w.as_ptr(), attrs) != 0 }
}

/// "x64", "x86", "ARM64" for a program, from its PE header.
pub fn architecture(path: &Path) -> Option<&'static str> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 4096];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    if buf.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32::from_le_bytes(buf.get(0x3c..0x40)?.try_into().ok()?) as usize;
    if buf.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let machine = u16::from_le_bytes(buf.get(pe + 4..pe + 6)?.try_into().ok()?);
    Some(match machine {
        0x8664 => "x64",
        0x14c => "x86",
        0xaa64 => "ARM64",
        0x1c4 => "ARM",
        _ => return None,
    })
}

// ------------------------------------------------------------------ version information

/// (label key, value) pairs of a program's version resource.
pub fn version_info(path: &Path) -> Vec<(&'static str, String)> {
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    let w = wide(path.as_os_str());
    let mut out = Vec::new();
    unsafe {
        let mut handle = 0u32;
        let n = GetFileVersionInfoSizeW(w.as_ptr(), &mut handle);
        if n == 0 {
            return out;
        }
        let mut buf = vec![0u8; n as usize];
        if GetFileVersionInfoW(w.as_ptr(), 0, n, buf.as_mut_ptr() as *mut _) == 0 {
            return out;
        }
        let mut p: *mut c_void = null_mut();
        let mut len = 0u32;
        let tr = wide("\\VarFileInfo\\Translation");
        let (lang, cp) = if VerQueryValueW(buf.as_ptr() as *const _, tr.as_ptr(), &mut p, &mut len) != 0 && len >= 4 {
            let t = p as *const u16;
            (*t, *t.add(1))
        } else {
            (0x0409, 0x04b0)
        };
        for (key, label) in [
            ("FileDescription", "File description"),
            ("ProductName", "Product name"),
            ("CompanyName", "Company"),
            ("FileVersion", "File version"),
            ("ProductVersion", "Product version"),
            ("LegalCopyright", "Copyright"),
            ("OriginalFilename", "Original file name"),
        ] {
            let k = wide(format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\{key}"));
            if VerQueryValueW(buf.as_ptr() as *const _, k.as_ptr(), &mut p, &mut len) != 0 && len > 0 {
                let s = from_wide(std::slice::from_raw_parts(p as *const u16, len as usize)).trim().to_string();
                if !s.is_empty() {
                    out.push((label, s));
                }
            }
        }
        let mut name = [0u16; 128];
        let got = windows_sys::Win32::Globalization::LCIDToLocaleName(lang as u32, name.as_mut_ptr(), name.len() as i32, 0);
        if got > 0 {
            out.push(("Language", from_wide(&name)));
        }
    }
    out
}

// ------------------------------------------------------------------ shortcuts

#[derive(Clone, Default, PartialEq)]
pub struct Shortcut {
    pub target: String,
    pub args: String,
    pub workdir: String,
    pub comment: String,
    /// 1 normal, 7 minimized, 3 maximized.
    pub show: i32,
    pub hotkey: u16,
    pub icon: (String, i32),
}

const CLSID_SHELLLINK: GUID = GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
const IID_ISHELLLINKW: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
const IID_IPERSISTFILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);

/// Open a .lnk; `f` gets (IShellLinkW, IPersistFile).
fn with_link<R>(lnk: &Path, write: bool, f: impl FnOnce(*mut c_void, *mut c_void) -> Option<R>) -> Option<R> {
    use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx};
    unsafe {
        CoInitializeEx(null(), 0x2);
        let mut link: *mut c_void = null_mut();
        if CoCreateInstance(&CLSID_SHELLLINK, null_mut(), 1, &IID_ISHELLLINKW, &mut link) < 0 || link.is_null() {
            return None;
        }
        let qi: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32 = com_fn(link, 0);
        let mut pf: *mut c_void = null_mut();
        let mut out = None;
        if qi(link, &IID_IPERSISTFILE, &mut pf) >= 0 && !pf.is_null() {
            let load: unsafe extern "system" fn(*mut c_void, *const u16, u32) -> i32 = com_fn(pf, 5);
            let w = wide(lnk.as_os_str());
            if load(pf, w.as_ptr(), if write { 2 } else { 0 }) >= 0 {
                out = f(link, pf);
            }
            release(pf);
        }
        release(link);
        out
    }
}

type GetStr = unsafe extern "system" fn(*mut c_void, *mut u16, i32) -> i32;
type SetStr = unsafe extern "system" fn(*mut c_void, *const u16) -> i32;

pub fn read_shortcut(lnk: &Path) -> Option<Shortcut> {
    with_link(lnk, false, |link, _| unsafe {
        let get = |idx: usize| {
            let mut buf = vec![0u16; 2048];
            let f: GetStr = com_fn(link, idx);
            f(link, buf.as_mut_ptr(), buf.len() as i32);
            from_wide(&buf)
        };
        let mut path = vec![0u16; 2048];
        let get_path: unsafe extern "system" fn(*mut c_void, *mut u16, i32, *mut c_void, u32) -> i32 = com_fn(link, 3);
        get_path(link, path.as_mut_ptr(), path.len() as i32, null_mut(), 0);
        let mut hotkey = 0u16;
        let get_hotkey: unsafe extern "system" fn(*mut c_void, *mut u16) -> i32 = com_fn(link, 12);
        get_hotkey(link, &mut hotkey);
        let mut show = 1i32;
        let get_show: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 = com_fn(link, 14);
        get_show(link, &mut show);
        let mut icon = vec![0u16; 1024];
        let mut index = 0i32;
        let get_icon: unsafe extern "system" fn(*mut c_void, *mut u16, i32, *mut i32) -> i32 = com_fn(link, 16);
        get_icon(link, icon.as_mut_ptr(), icon.len() as i32, &mut index);
        Some(Shortcut {
            target: from_wide(&path),
            comment: get(6),
            workdir: get(8),
            args: get(10),
            show,
            hotkey,
            icon: (from_wide(&icon), index),
        })
    })
}

pub fn write_shortcut(lnk: &Path, s: &Shortcut) -> bool {
    with_link(lnk, true, |link, pf| unsafe {
        let set = |idx: usize, v: &str| {
            let f: SetStr = com_fn(link, idx);
            let w = wide(v);
            f(link, w.as_ptr())
        };
        set(20, &s.target);
        set(7, &s.comment);
        set(9, &s.workdir);
        set(11, &s.args);
        let set_show: unsafe extern "system" fn(*mut c_void, i32) -> i32 = com_fn(link, 15);
        set_show(link, s.show);
        let save: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32 = com_fn(pf, 6);
        (save(pf, null(), 1) >= 0).then_some(())
    })
    .is_some()
}

/// "Ctrl + Alt + K" for a shortcut's hot key (0 = none).
pub fn hotkey_text(hk: u16) -> String {
    if hk == 0 {
        return String::new();
    }
    let (vk, mods) = ((hk & 0xff) as u8, (hk >> 8) as u8);
    let mut parts = Vec::new();
    if mods & 0x2 != 0 {
        parts.push("Ctrl".to_string());
    }
    if mods & 0x1 != 0 {
        parts.push("Shift".to_string());
    }
    if mods & 0x4 != 0 {
        parts.push("Alt".to_string());
    }
    parts.push(match vk {
        b'0'..=b'9' | b'A'..=b'Z' => (vk as char).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6f),
        v => format!("0x{v:02X}"),
    });
    parts.join(" + ")
}

/// The program a shortcut or path points at, environment variables expanded.
pub fn expand(s: &str) -> PathBuf {
    let w = wide(s);
    let mut buf = vec![0u16; 2048];
    let n = unsafe { windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if n == 0 {
        PathBuf::from(s)
    } else {
        PathBuf::from(from_wide(&buf))
    }
}

// ------------------------------------------------------------------ compatibility

const LAYERS: &str = r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers";

/// Compatibility flags Windows keeps for a program ("RUNASADMIN", "WIN7RTM", …).
pub fn compat_flags(exe: &Path) -> Vec<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(LAYERS)
        .and_then(|k| k.get_value::<String, _>(exe.to_string_lossy().as_ref()))
        .map(|v| v.split_whitespace().filter(|t| *t != "~").map(str::to_string).collect())
        .unwrap_or_default()
}

pub fn set_compat_flags(exe: &Path, flags: &[String]) -> bool {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_ALL_ACCESS};
    let Ok((k, _)) = winreg::RegKey::predef(HKEY_CURRENT_USER).create_subkey(LAYERS) else { return false };
    let _ = KEY_ALL_ACCESS;
    let name = exe.to_string_lossy();
    if flags.is_empty() {
        let _ = k.delete_value(name.as_ref());
        true
    } else {
        k.set_value(name.as_ref(), &format!("~ {}", flags.join(" "))).is_ok()
    }
}
