//! Devices from SetupAPI / the configuration manager: the same data Windows
//! Device Manager shows, read directly (no MMC), and the actions on them.

use std::ptr::{null, null_mut};
use windows_sys::core::GUID;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

#[derive(Clone, Default)]
pub struct Device {
    pub instance_id: String,
    pub name: String,
    pub class: String,
    pub class_guid: u128,
    /// "Display adapters", … (empty when the device has no class).
    pub class_display: String,
    pub manufacturer: String,
    pub location: String,
    pub service: String,
    pub hardware_ids: Vec<String>,
    pub driver_provider: String,
    pub driver_version: String,
    pub driver_date: String,
    /// Plugged in / present now.
    pub present: bool,
    /// Hidden in Windows Device Manager unless "Show hidden devices".
    pub hidden: bool,
    pub problem: u32,
    pub disableable: bool,
}

impl Device {
    pub fn disabled(&self) -> bool {
        self.problem == CM_PROB_DISABLED
    }
    pub fn has_problem(&self) -> bool {
        self.present && self.problem != 0 && !self.disabled()
    }
}

/// "Display adapters", … for a class GUID (empty for none).
pub fn class_name(guid: u128) -> String {
    if guid == 0 {
        return String::new();
    }
    let g = GUID::from_u128(guid);
    let mut buf = [0u16; 256];
    let mut need = 0u32;
    unsafe {
        if SetupDiGetClassDescriptionW(&g, buf.as_mut_ptr(), buf.len() as u32, &mut need) != 0 {
            return from_wide(&buf);
        }
    }
    String::new()
}

fn guid_u128(g: &GUID) -> u128 {
    ((g.data1 as u128) << 96) | ((g.data2 as u128) << 80) | ((g.data3 as u128) << 64) | u64::from_be_bytes(g.data4) as u128
}

pub fn guid_string(g: u128) -> String {
    let g = GUID::from_u128(g);
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1, g.data2, g.data3, g.data4[0], g.data4[1], g.data4[2], g.data4[3], g.data4[4], g.data4[5], g.data4[6], g.data4[7]
    )
}

/// The class icon (32 px RGBA), as Windows Device Manager shows it.
pub fn class_icon(guid: u128) -> Option<(usize, usize, Vec<u8>)> {
    let g = GUID::from_u128(guid);
    unsafe {
        let mut icon = null_mut();
        if SetupDiLoadClassIcon(&g, &mut icon, null_mut()) == 0 || icon.is_null() {
            return None;
        }
        let px = crate::shell::win::icon_rgba(icon as isize);
        windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(icon);
        px
    }
}

unsafe fn reg_prop(set: HDEVINFO, data: &SP_DEVINFO_DATA, prop: u32) -> Vec<String> {
    let mut buf = vec![0u8; 2048];
    let mut kind = 0u32;
    let mut need = 0u32;
    if SetupDiGetDeviceRegistryPropertyW(set, data, prop, &mut kind, buf.as_mut_ptr(), buf.len() as u32, &mut need) == 0 {
        if need as usize > buf.len() {
            buf = vec![0u8; need as usize];
            if SetupDiGetDeviceRegistryPropertyW(set, data, prop, &mut kind, buf.as_mut_ptr(), buf.len() as u32, &mut need) == 0 {
                return Vec::new();
            }
        } else {
            return Vec::new();
        }
    }
    let w: Vec<u16> = buf[..need as usize].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    // REG_SZ or REG_MULTI_SZ: strings separated by NULs.
    w.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
}

fn class_hidden(guid: u128) -> bool {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(format!(r"SYSTEM\CurrentControlSet\Control\Class\{}", guid_string(guid)))
        .is_ok_and(|k| k.get_raw_value("NoDisplayClass").is_ok())
}

/// Driver provider, version and date from the driver key ("{class}\0003").
fn driver_info(driver_key: &str) -> (String, String, String) {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let Ok(k) = winreg::RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(format!(r"SYSTEM\CurrentControlSet\Control\Class\{driver_key}"))
    else {
        return Default::default();
    };
    let get = |n: &str| k.get_value::<String, _>(n).unwrap_or_default();
    (get("ProviderName"), get("DriverVersion"), get("DriverDate"))
}

/// All devices; with `all`, also the ones not connected now.
pub fn list(all: bool) -> Vec<Device> {
    let mut out = Vec::new();
    let mut hidden_class = std::collections::HashMap::new();
    let mut class_names: std::collections::HashMap<u128, String> = std::collections::HashMap::new();
    unsafe {
        let flags = DIGCF_ALLCLASSES | if all { 0 } else { DIGCF_PRESENT };
        let set = SetupDiGetClassDevsW(null(), null(), null_mut(), flags);
        if set == -1isize as HDEVINFO {
            return out;
        }
        let mut i = 0;
        loop {
            let mut data: SP_DEVINFO_DATA = std::mem::zeroed();
            data.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
            if SetupDiEnumDeviceInfo(set, i, &mut data) == 0 {
                break;
            }
            i += 1;
            let mut id = [0u16; 512];
            let mut need = 0;
            if SetupDiGetDeviceInstanceIdW(set, &data, id.as_mut_ptr(), id.len() as u32, &mut need) == 0 {
                continue;
            }
            let first = |p| reg_prop(set, &data, p).into_iter().next().unwrap_or_default();
            let mut name = first(SPDRP_FRIENDLYNAME);
            if name.is_empty() {
                name = first(SPDRP_DEVICEDESC);
            }
            let class_guid = guid_u128(&data.ClassGuid);
            let (mut status, mut problem) = (0u32, 0u32);
            let present = CM_Get_DevNode_Status(&mut status, &mut problem, data.DevInst, 0) == CR_SUCCESS;
            let nodisplay = *hidden_class.entry(class_guid).or_insert_with(|| class_hidden(class_guid));
            let (driver_provider, driver_version, driver_date) = driver_info(&first(SPDRP_DRIVER));
            let instance_id = from_wide(&id);
            if name.is_empty() {
                name = instance_id.clone();
            }
            out.push(Device {
                name,
                class: first(SPDRP_CLASS),
                class_guid,
                class_display: class_names.entry(class_guid).or_insert_with(|| class_name(class_guid)).clone(),
                manufacturer: first(SPDRP_MFG),
                location: first(SPDRP_LOCATION_INFORMATION),
                service: first(SPDRP_SERVICE),
                hardware_ids: reg_prop(set, &data, SPDRP_HARDWAREID),
                driver_provider,
                driver_version,
                driver_date,
                present,
                hidden: !present || nodisplay || status & DN_NO_SHOW_IN_DM != 0,
                problem: if present && status & DN_HAS_PROBLEM != 0 { problem } else { 0 },
                disableable: status & DN_DISABLEABLE != 0,
                instance_id,
            });
        }
        SetupDiDestroyDeviceInfoList(set);
    }
    out
}

/// What Windows says about a device's problem code.
pub fn problem_text(code: u32) -> &'static str {
    use crate::i18n::tr;
    match code {
        0 => tr("This device is working properly."),
        1 => tr("This device is not configured correctly. (Code 1)"),
        3 => tr("The driver for this device might be corrupted, or the system may be running low on memory. (Code 3)"),
        10 => tr("This device cannot start. (Code 10)"),
        12 => tr("This device cannot find enough free resources that it can use. (Code 12)"),
        14 => tr("This device cannot work properly until you restart your computer. (Code 14)"),
        18 => tr("Reinstall the drivers for this device. (Code 18)"),
        19 => tr("Windows cannot start this hardware device because its configuration information is incomplete or damaged. (Code 19)"),
        21 => tr("Windows is removing this device. (Code 21)"),
        22 => tr("This device is disabled. (Code 22)"),
        24 => tr("This device is not present, is not working properly, or does not have all its drivers installed. (Code 24)"),
        28 => tr("The drivers for this device are not installed. (Code 28)"),
        29 => tr("This device is disabled because the firmware of the device did not give it the required resources. (Code 29)"),
        31 => tr("This device is not working properly because Windows cannot load the drivers required for this device. (Code 31)"),
        32 => tr("A driver (service) for this device has been disabled. (Code 32)"),
        37 => tr("Windows cannot initialize the device driver for this hardware. (Code 37)"),
        39 => tr("Windows cannot load the device driver for this hardware. The driver may be corrupted or missing. (Code 39)"),
        43 => tr("Windows has stopped this device because it has reported problems. (Code 43)"),
        45 => tr("Currently, this hardware device is not connected to the computer. (Code 45)"),
        47 => tr("Windows cannot use this hardware device because it has been prepared for safe removal. (Code 47)"),
        48 => tr("The software for this device has been blocked from starting because it is known to have problems with Windows. (Code 48)"),
        52 => tr("Windows cannot verify the digital signature for the drivers required for this device. (Code 52)"),
        _ => tr("This device has a problem."),
    }
}

// ------------------------------------------------------------------ actions

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Enable,
    Disable,
    Uninstall,
    Scan,
}

impl Action {
    pub fn arg(self) -> &'static str {
        match self {
            Action::Enable => "enable",
            Action::Disable => "disable",
            Action::Uninstall => "uninstall",
            Action::Scan => "scan",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "enable" => Action::Enable,
            "disable" => Action::Disable,
            "uninstall" => Action::Uninstall,
            "scan" => Action::Scan,
            _ => return None,
        })
    }
}

/// Run an action in this process. Needs administrator rights (except, on
/// some systems, Scan).
pub fn perform(action: Action, instance_id: &str) -> bool {
    unsafe {
        if action == Action::Scan {
            let mut root = 0u32;
            if CM_Locate_DevNodeW(&mut root, null(), CM_LOCATE_DEVNODE_NORMAL) != CR_SUCCESS {
                return false;
            }
            return CM_Reenumerate_DevNode(root, CM_REENUMERATE_NORMAL) == CR_SUCCESS;
        }
        let set = SetupDiCreateDeviceInfoList(null(), null_mut());
        if set == -1isize as HDEVINFO {
            return false;
        }
        let mut data: SP_DEVINFO_DATA = std::mem::zeroed();
        data.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
        let id = wide(instance_id);
        let mut ok = SetupDiOpenDeviceInfoW(set, id.as_ptr(), null_mut(), 0, &mut data) != 0;
        if ok {
            ok = match action {
                Action::Enable | Action::Disable => {
                    let params = SP_PROPCHANGE_PARAMS {
                        ClassInstallHeader: SP_CLASSINSTALL_HEADER {
                            cbSize: std::mem::size_of::<SP_CLASSINSTALL_HEADER>() as u32,
                            InstallFunction: DIF_PROPERTYCHANGE,
                        },
                        StateChange: if action == Action::Enable { DICS_ENABLE } else { DICS_DISABLE },
                        Scope: DICS_FLAG_GLOBAL,
                        HwProfile: 0,
                    };
                    SetupDiSetClassInstallParamsW(
                        set,
                        &data,
                        &params.ClassInstallHeader,
                        std::mem::size_of::<SP_PROPCHANGE_PARAMS>() as u32,
                    ) != 0
                        && SetupDiCallClassInstaller(DIF_PROPERTYCHANGE, set, &data) != 0
                }
                Action::Uninstall => SetupDiCallClassInstaller(DIF_REMOVE, set, &data) != 0,
                Action::Scan => true,
            };
        }
        SetupDiDestroyDeviceInfoList(set);
        ok
    }
}

pub fn is_admin() -> bool {
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

/// Start `exe args` as administrator and wait for it. `None` = cancelled.
pub fn run_elevated_wait(exe: &str, args: &str) -> Option<u32> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    let (verb, file, params) = (wide("runas"), wide(exe), wide(args));
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = 0;
        if ShellExecuteExW(&mut info) == 0 || info.hProcess.is_null() {
            return None;
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        Some(code)
    }
}
