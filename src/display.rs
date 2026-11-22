//! Display modes for the Settings window (System › Display): monitors, their
//! resolutions, refresh rates and orientation, and changing them.

use crate::i18n::tr;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Graphics::Gdi::{
    ChangeDisplaySettingsExW, EnumDisplayDevicesW, EnumDisplaySettingsW, DEVMODEW, DISPLAY_DEVICEW,
};

const DM_BITSPERPEL: u32 = 0x0004_0000;
const DM_PELSWIDTH: u32 = 0x0008_0000;
const DM_PELSHEIGHT: u32 = 0x0010_0000;
const DM_DISPLAYFREQUENCY: u32 = 0x0040_0000;
const DM_DISPLAYORIENTATION: u32 = 0x0000_0080;
const ENUM_CURRENT_SETTINGS: u32 = 0xFFFF_FFFF;
const CDS_UPDATEREGISTRY: u32 = 0x1;
const DISPLAY_DEVICE_ATTACHED_TO_DESKTOP: u32 = 0x1;
const DISPLAY_DEVICE_PRIMARY_DEVICE: u32 = 0x4;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn from_wide(b: &[u16]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..n])
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Mode {
    pub w: u32,
    pub h: u32,
    pub hz: u32,
    /// 0 = landscape, 1 = portrait, 2 = landscape (flipped), 3 = portrait (flipped)
    pub orient: u32,
}

#[derive(Clone)]
pub(crate) struct Monitor {
    pub device: String, // \\.\DISPLAY1
    pub name: String,   // adapter / monitor description
    pub primary: bool,
    pub modes: Vec<Mode>,
    pub current: Mode,
}

unsafe fn devmode(device: &[u16], i: u32) -> Option<DEVMODEW> {
    let mut dm: DEVMODEW = zeroed();
    dm.dmSize = size_of::<DEVMODEW>() as u16;
    (EnumDisplaySettingsW(device.as_ptr(), i, &mut dm) != 0).then_some(dm)
}

fn mode_of(dm: &DEVMODEW) -> Mode {
    Mode {
        w: dm.dmPelsWidth,
        h: dm.dmPelsHeight,
        hz: dm.dmDisplayFrequency,
        orient: unsafe { dm.Anonymous1.Anonymous2.dmDisplayOrientation } as u32,
    }
}

pub(crate) fn monitors() -> Vec<Monitor> {
    let mut out = Vec::new();
    unsafe {
        for i in 0..16 {
            let mut dd: DISPLAY_DEVICEW = zeroed();
            dd.cb = size_of::<DISPLAY_DEVICEW>() as u32;
            if EnumDisplayDevicesW(null(), i, &mut dd, 0) == 0 {
                break;
            }
            if dd.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0 {
                continue;
            }
            let device = dd.DeviceName;
            // The monitor's own name, if Windows knows it.
            let mut md: DISPLAY_DEVICEW = zeroed();
            md.cb = size_of::<DISPLAY_DEVICEW>() as u32;
            let monitor_name = if EnumDisplayDevicesW(device.as_ptr(), 0, &mut md, 0) != 0 {
                from_wide(&md.DeviceString)
            } else {
                String::new()
            };
            let Some(cur) = devmode(&device, ENUM_CURRENT_SETTINGS) else { continue };
            let current = mode_of(&cur);
            let mut modes: Vec<Mode> = Vec::new();
            let mut k = 0;
            while let Some(dm) = devmode(&device, k) {
                k += 1;
                if dm.dmBitsPerPel < 32 {
                    continue;
                }
                let mut m = mode_of(&dm);
                m.orient = 0;
                if !modes.contains(&m) {
                    modes.push(m);
                }
            }
            modes.sort_by(|a, b| (b.w * b.h, b.w, b.hz).cmp(&(a.w * a.h, a.w, a.hz)));
            let adapter = from_wide(&dd.DeviceString);
            out.push(Monitor {
                device: from_wide(&device),
                name: if monitor_name.is_empty() { adapter } else { format!("{monitor_name} — {adapter}") },
                primary: dd.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0,
                modes,
                current,
            });
        }
    }
    out
}

/// Apply a mode; returns the Windows result (0 = DISP_CHANGE_SUCCESSFUL).
pub(crate) fn apply(device: &str, m: Mode) -> i32 {
    let dev = wide(device);
    unsafe {
        let Some(mut dm) = devmode(&dev, ENUM_CURRENT_SETTINGS) else { return -1 };
        // Portrait swaps width and height.
        let (w, h) = if m.orient % 2 == 1 && m.w > m.h { (m.h, m.w) } else { (m.w, m.h) };
        dm.dmPelsWidth = w;
        dm.dmPelsHeight = h;
        dm.dmDisplayFrequency = m.hz;
        dm.dmBitsPerPel = 32;
        dm.Anonymous1.Anonymous2.dmDisplayOrientation = m.orient as _;
        dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY | DM_BITSPERPEL | DM_DISPLAYORIENTATION;
        ChangeDisplaySettingsExW(dev.as_ptr(), &dm, null_mut(), CDS_UPDATEREGISTRY, null())
    }
}

pub(crate) fn orient_label(o: u32) -> &'static str {
    match o {
        1 => tr("Portrait"),
        2 => tr("Landscape (flipped)"),
        3 => tr("Portrait (flipped)"),
        _ => tr("Landscape"),
    }
}
