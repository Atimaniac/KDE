//! Sound devices and per-app volume (Core Audio), so the Settings window
//! and quick settings don't have to send you to the classic Sound dialog or
//! the volume mixer.
//!
//! The default device is changed through IPolicyConfig, the interface
//! Windows' own Sound settings use (not in the SDK, stable since Windows 7).

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use windows_sys::core::GUID;

const CLSID_ENUMERATOR: GUID = GUID::from_u128(0xbcde0395_e52f_467c_8e3d_c4579291692e);
const IID_ENUMERATOR: GUID = GUID::from_u128(0xa95664d2_9614_4f35_a746_de8db63617e6);
const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const IID_POLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
const IID_SESSION_MANAGER2: GUID = GUID::from_u128(0x77aa99a0_1bd6_484f_8bc7_2c654c9a9b6f);
const IID_SESSION_CONTROL2: GUID = GUID::from_u128(0xbfb7ff88_7239_4fc9_8fa2_07c950be9c6d);
const IID_SIMPLE_VOLUME: GUID = GUID::from_u128(0x87ce5498_68d6_44e5_9215_6da47ef883d8);
/// PKEY_Device_FriendlyName
const FRIENDLY_NAME: (GUID, u32) = (GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), 14);

unsafe fn vfn<T: Copy>(obj: *mut c_void, i: usize) -> T {
    let vt = *(obj as *const *const usize);
    std::mem::transmute_copy(&*vt.add(i))
}

unsafe fn release(obj: *mut c_void) {
    if !obj.is_null() {
        let f: unsafe extern "system" fn(*mut c_void) -> u32 = vfn(obj, 2);
        f(obj);
    }
}

unsafe fn query(obj: *mut c_void, iid: &GUID) -> *mut c_void {
    let f: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32 = vfn(obj, 0);
    let mut out = null_mut();
    f(obj, iid, &mut out);
    out
}

unsafe fn enumerator() -> *mut c_void {
    use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx};
    CoInitializeEx(null(), 0x2);
    let mut en = null_mut();
    CoCreateInstance(&CLSID_ENUMERATOR, null_mut(), 23, &IID_ENUMERATOR, &mut en);
    en
}

unsafe fn wstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

unsafe fn device_id(dev: *mut c_void) -> String {
    let get_id: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> i32 = vfn(dev, 5);
    let mut p = null_mut();
    if get_id(dev, &mut p) < 0 {
        return String::new();
    }
    let s = wstr(p);
    windows_sys::Win32::System::Com::CoTaskMemFree(p as *const c_void);
    s
}

unsafe fn device_name(dev: *mut c_void) -> String {
    // IMMDevice::OpenPropertyStore(STGM_READ)
    let open: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32 = vfn(dev, 4);
    let mut store = null_mut();
    if open(dev, 0, &mut store) < 0 || store.is_null() {
        return String::new();
    }
    #[repr(C)]
    struct PropKey {
        fmtid: GUID,
        pid: u32,
    }
    #[repr(C)]
    struct PropVariant {
        vt: u16,
        r1: u16,
        r2: u16,
        r3: u16,
        val: *mut u16,
        pad: usize,
    }
    let key = PropKey { fmtid: FRIENDLY_NAME.0, pid: FRIENDLY_NAME.1 };
    let mut pv: PropVariant = std::mem::zeroed();
    // IPropertyStore::GetValue
    let get: unsafe extern "system" fn(*mut c_void, *const PropKey, *mut PropVariant) -> i32 = vfn(store, 5);
    let name = if get(store, &key, &mut pv) >= 0 && pv.vt == 31 /* VT_LPWSTR */ { wstr(pv.val) } else { String::new() };
    if pv.vt == 31 && !pv.val.is_null() {
        windows_sys::Win32::System::Com::CoTaskMemFree(pv.val as *const c_void);
    }
    release(store);
    name
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub default: bool,
}

/// Active playback (`output`) or recording devices.
pub fn devices(output: bool) -> Vec<Device> {
    let mut out = Vec::new();
    unsafe {
        let en = enumerator();
        if en.is_null() {
            return out;
        }
        let flow = if output { 0 } else { 1 };
        let default_id = {
            let get_default: unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> i32 = vfn(en, 4);
            let mut d = null_mut();
            let id = if get_default(en, flow, 0, &mut d) >= 0 && !d.is_null() { device_id(d) } else { String::new() };
            release(d);
            id
        };
        // IMMDeviceEnumerator::EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)
        let enum_eps: unsafe extern "system" fn(*mut c_void, i32, u32, *mut *mut c_void) -> i32 = vfn(en, 3);
        let mut coll = null_mut();
        if enum_eps(en, flow, 1, &mut coll) >= 0 && !coll.is_null() {
            let count: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 = vfn(coll, 3);
            let item: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32 = vfn(coll, 4);
            let mut n = 0;
            count(coll, &mut n);
            for i in 0..n {
                let mut dev = null_mut();
                if item(coll, i, &mut dev) >= 0 && !dev.is_null() {
                    let id = device_id(dev);
                    let name = device_name(dev);
                    out.push(Device { default: id == default_id, id, name });
                    release(dev);
                }
            }
            release(coll);
        }
        release(en);
    }
    out
}

/// Make a device the default for everything (console, multimedia, communications).
pub fn set_default(id: &str) -> bool {
    use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx};
    let w: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
    unsafe {
        CoInitializeEx(null(), 0x2);
        let mut pc = null_mut();
        if CoCreateInstance(&CLSID_POLICY_CONFIG, null_mut(), 23, &IID_POLICY_CONFIG, &mut pc) < 0 || pc.is_null() {
            return false;
        }
        // IPolicyConfig::SetDefaultEndpoint
        let set: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32 = vfn(pc, 13);
        let mut ok = true;
        for role in 0..3 {
            ok &= set(pc, w.as_ptr(), role) >= 0;
        }
        release(pc);
        ok
    }
}

// ------------------------------------------------------------------ apps (sessions)

#[derive(Clone, Debug)]
pub struct Session {
    pub pid: u32,
    pub name: String,
    pub exe: Option<std::path::PathBuf>,
    pub volume: f32,
    pub muted: bool,
    pub system: bool,
}

/// Call `f` with each app's volume control on the default output device.
unsafe fn each_session(mut f: impl FnMut(u32, bool, *mut c_void)) {
    let en = enumerator();
    if en.is_null() {
        return;
    }
    let get_default: unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> i32 = vfn(en, 4);
    let mut dev = null_mut();
    if get_default(en, 0, 1, &mut dev) < 0 || dev.is_null() {
        release(en);
        return;
    }
    let activate: unsafe extern "system" fn(*mut c_void, *const GUID, u32, *const c_void, *mut *mut c_void) -> i32 = vfn(dev, 3);
    let mut mgr = null_mut();
    if activate(dev, &IID_SESSION_MANAGER2, 1 /* CLSCTX_INPROC_SERVER */, null(), &mut mgr) >= 0 && !mgr.is_null() {
        // IAudioSessionManager2::GetSessionEnumerator
        let get_enum: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 = vfn(mgr, 5);
        let mut se = null_mut();
        if get_enum(mgr, &mut se) >= 0 && !se.is_null() {
            let count: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 = vfn(se, 3);
            let get: unsafe extern "system" fn(*mut c_void, i32, *mut *mut c_void) -> i32 = vfn(se, 4);
            let mut n = 0;
            count(se, &mut n);
            for i in 0..n {
                let mut ctl = null_mut();
                if get(se, i, &mut ctl) < 0 || ctl.is_null() {
                    continue;
                }
                let ctl2 = query(ctl, &IID_SESSION_CONTROL2);
                let vol = query(ctl, &IID_SIMPLE_VOLUME);
                if !ctl2.is_null() && !vol.is_null() {
                    // IAudioSessionControl::GetState: 2 = expired (the app is gone).
                    let state: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 = vfn(ctl2, 3);
                    let mut st = 0;
                    state(ctl2, &mut st);
                    let pid_f: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32 = vfn(ctl2, 14);
                    let system_f: unsafe extern "system" fn(*mut c_void) -> i32 = vfn(ctl2, 15);
                    let mut pid = 0;
                    pid_f(ctl2, &mut pid);
                    if st != 2 {
                        f(pid, system_f(ctl2) == 0, vol);
                    }
                }
                release(vol);
                release(ctl2);
                release(ctl);
            }
            release(se);
        }
        release(mgr);
    }
    release(dev);
    release(en);
}

/// Apps that play (or played) sound, with their volume.
pub fn sessions() -> Vec<Session> {
    let mut out: Vec<Session> = Vec::new();
    unsafe {
        each_session(|pid, system, vol| {
            let get_v: unsafe extern "system" fn(*mut c_void, *mut f32) -> i32 = vfn(vol, 4);
            let get_m: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 = vfn(vol, 6);
            let (mut v, mut m) = (1.0f32, 0i32);
            get_v(vol, &mut v);
            get_m(vol, &mut m);
            // One row per program (browsers have several sessions).
            if out.iter().any(|s| s.pid == pid && !system) {
                return;
            }
            let exe = if system { None } else { process_path(pid) };
            let name = if system {
                crate::i18n::tr("System sounds").to_string()
            } else {
                exe.as_ref()
                    .and_then(|p| crate::taskmgr::sys::file_description(p))
                    .or_else(|| exe.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()))
                    .unwrap_or_else(|| format!("PID {pid}"))
            };
            out.push(Session { pid, name, exe, volume: v, muted: m != 0, system });
        });
    }
    // Same program with several processes (e.g. a browser): keep the first.
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| s.system || seen.insert(s.name.clone()));
    out.sort_by(|a, b| b.system.cmp(&a.system).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

fn process_path(pid: u32) -> Option<std::path::PathBuf> {
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut n) != 0;
        windows_sys::Win32::Foundation::CloseHandle(h);
        ok.then(|| std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n as usize])))
    }
}

/// Set the volume (0..1) and mute of one app's sound, by its process
/// (or the system sounds when `system`). Applied to all its sessions.
pub fn set_session(pid: u32, system: bool, volume: Option<f32>, mute: Option<bool>) {
    // The program's name, so all its processes change together.
    let name = if system { None } else { process_path(pid).and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase())) };
    unsafe {
        each_session(|p, sys, vol| {
            let same = if system {
                sys
            } else {
                !sys && (p == pid || (name.is_some() && process_path(p).and_then(|x| x.file_name().map(|n| n.to_string_lossy().to_lowercase())) == name))
            };
            if !same {
                return;
            }
            if let Some(v) = volume {
                let set_v: unsafe extern "system" fn(*mut c_void, f32, *const GUID) -> i32 = vfn(vol, 3);
                set_v(vol, v.clamp(0.0, 1.0), null());
            }
            if let Some(m) = mute {
                let set_m: unsafe extern "system" fn(*mut c_void, i32, *const GUID) -> i32 = vfn(vol, 5);
                set_m(vol, m as i32, null());
            }
        });
    }
}

