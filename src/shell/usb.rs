//! "Safely remove hardware": USB drives and memory cards, which Explorer
//! lists behind its tray icon. Shown on the panel while one is plugged in.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

#[derive(Clone, PartialEq)]
pub struct Drive {
    pub letter: char,
    pub label: String,
    /// The disk's device node (for the eject request).
    pub devinst: u32,
}

const IOCTL_STORAGE_GET_DEVICE_NUMBER: u32 = 0x2D1080;
const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x2D1400;

#[repr(C)]
struct DeviceNumber {
    device_type: u32,
    number: u32,
    partition: u32,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

unsafe fn open(path: &str) -> windows_sys::Win32::Foundation::HANDLE {
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    let w = wide(path);
    CreateFileW(w.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, 0, null_mut())
}

unsafe fn disk_number(h: windows_sys::Win32::Foundation::HANDLE) -> Option<u32> {
    use windows_sys::Win32::System::IO::DeviceIoControl;
    let mut dn: DeviceNumber = std::mem::zeroed();
    let mut got = 0u32;
    (DeviceIoControl(h, IOCTL_STORAGE_GET_DEVICE_NUMBER, null(), 0, &mut dn as *mut _ as *mut c_void, std::mem::size_of::<DeviceNumber>() as u32, &mut got, null_mut()) != 0)
        .then_some(dn.number)
}

/// USB (7), SD (12) or MMC (13) bus: something you can pull out.
unsafe fn removable_bus(h: windows_sys::Win32::Foundation::HANDLE) -> bool {
    use windows_sys::Win32::System::IO::DeviceIoControl;
    // STORAGE_PROPERTY_QUERY { StorageDeviceProperty, PropertyStandardQuery }
    let query: [u32; 3] = [0, 0, 0];
    let mut out = [0u8; 1024];
    let mut got = 0u32;
    if DeviceIoControl(h, IOCTL_STORAGE_QUERY_PROPERTY, query.as_ptr() as *const c_void, 12, out.as_mut_ptr() as *mut c_void, out.len() as u32, &mut got, null_mut()) == 0 {
        return false;
    }
    // STORAGE_DEVICE_DESCRIPTOR: BusType is the u32 at offset 28.
    let bus = u32::from_le_bytes([out[28], out[29], out[30], out[31]]);
    matches!(bus, 7 | 12 | 13)
}

/// Device node of disk number `n` (via the disk interfaces).
unsafe fn disk_devinst(n: u32) -> Option<u32> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
    const GUID_DEVINTERFACE_DISK: windows_sys::core::GUID = windows_sys::core::GUID::from_u128(0x53f56307_b6bf_11d0_94f2_00a0c91efb8b);
    let set = SetupDiGetClassDevsW(&GUID_DEVINTERFACE_DISK, null(), null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
    if set as isize == -1 {
        return None;
    }
    let mut found = None;
    let mut i = 0;
    loop {
        let mut ifd: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
        ifd.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        if SetupDiEnumDeviceInterfaces(set, null(), &GUID_DEVINTERFACE_DISK, i, &mut ifd) == 0 {
            break;
        }
        i += 1;
        let mut need = 0u32;
        SetupDiGetDeviceInterfaceDetailW(set, &ifd, null_mut(), 0, &mut need, null_mut());
        if need == 0 {
            continue;
        }
        let mut buf = vec![0u8; need as usize + 8];
        let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
        (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        let mut dev: SP_DEVINFO_DATA = std::mem::zeroed();
        dev.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
        if SetupDiGetDeviceInterfaceDetailW(set, &ifd, detail, need, null_mut(), &mut dev) == 0 {
            continue;
        }
        let path_ptr = (*detail).DevicePath.as_ptr();
        let mut len = 0;
        while *path_ptr.add(len) != 0 {
            len += 1;
        }
        let path = String::from_utf16_lossy(std::slice::from_raw_parts(path_ptr, len));
        let h = open(&path);
        if h as isize != -1 {
            let num = disk_number(h);
            windows_sys::Win32::Foundation::CloseHandle(h);
            if num == Some(n) {
                found = Some(dev.DevInst);
                break;
            }
        }
    }
    SetupDiDestroyDeviceInfoList(set);
    found
}

/// Drives that can be removed now.
pub fn drives() -> Vec<Drive> {
    use windows_sys::Win32::Storage::FileSystem::{GetLogicalDrives, GetVolumeInformationW};
    let mut out = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 2..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        unsafe {
            let h = open(&format!("\\\\.\\{letter}:"));
            if h as isize == -1 {
                continue;
            }
            let removable = removable_bus(h);
            let num = if removable { disk_number(h) } else { None };
            windows_sys::Win32::Foundation::CloseHandle(h);
            let Some(n) = num else { continue };
            let Some(devinst) = disk_devinst(n) else { continue };
            let root = wide(&format!("{letter}:\\"));
            let mut label = [0u16; 64];
            GetVolumeInformationW(root.as_ptr(), label.as_mut_ptr(), label.len() as u32, null_mut(), null_mut(), null_mut(), null_mut(), 0);
            let end = label.iter().position(|&c| c == 0).unwrap_or(0);
            out.push(Drive { letter, label: String::from_utf16_lossy(&label[..end]), devinst });
        }
    }
    out
}

/// Ask Windows to prepare the device for removal. Err = the reason it can't
/// (usually a program still has a file open on it).
pub fn eject(devinst: u32) -> Result<(), String> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{CM_Get_Parent, CM_Request_Device_EjectW, CR_SUCCESS};
    unsafe {
        let try_eject = |d: u32| {
            let mut veto = 0i32;
            let mut name = [0u16; 260];
            let r = CM_Request_Device_EjectW(d, &mut veto, name.as_mut_ptr(), name.len() as u32, 0);
            (r == CR_SUCCESS && veto == 0, veto)
        };
        let (ok, veto) = try_eject(devinst);
        if ok {
            return Ok(());
        }
        // The disk itself may refuse; its USB device usually doesn't.
        let mut parent = 0u32;
        if CM_Get_Parent(&mut parent, devinst, 0) == CR_SUCCESS {
            let (ok, _) = try_eject(parent);
            if ok {
                return Ok(());
            }
        }
        Err(if veto != 0 { crate::i18n::tr("A program is still using the device.").to_string() } else { crate::i18n::tr("Windows refused to stop the device.").to_string() })
    }
}

/// Set when Windows reports a device arriving or leaving (WM_DEVICECHANGE,
/// received by the desktop window).
pub static CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
