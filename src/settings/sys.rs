//! What the Settings pages read and change: Windows' registry, power plans,
//! drives, network adapters, installed programs, Windows Update, …

use std::mem::{size_of, zeroed};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr::{null, null_mut};
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};
use winreg::RegKey;

const NO_WINDOW: u32 = 0x0800_0000;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn from_wide(b: &[u16]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..n])
}

fn hklm(path: &str) -> Option<RegKey> {
    RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY).ok()
}

fn hkcu(path: &str) -> Option<RegKey> {
    RegKey::predef(HKEY_CURRENT_USER).open_subkey(path).ok()
}

/// Run a program without a window and return what it printed.
pub fn output(exe: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(exe).args(args).creation_flags(NO_WINDOW).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn powershell(script: &str) -> Option<String> {
    let full = format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8; {script}");
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &full])
        .creation_flags(NO_WINDOW)
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Start a program the way a double-click would (UAC prompt included).
pub fn start(cmd: &str) {
    let _ = Command::new("cmd.exe").raw_arg(format!("/C start \"\" {cmd}")).creation_flags(NO_WINDOW).spawn();
}

pub fn run_elevated_wait(exe: &str, args: &str) -> Option<u32> {
    crate::devmgr::sys::run_elevated_wait(exe, args)
}

// ------------------------------------------------------------------ about

#[derive(Clone, Default)]
pub struct About {
    pub computer: String,
    pub user: String,
    pub full_name: String,
    pub edition: String,
    pub version: String,
    pub build: String,
    pub installed: String,
    pub cpu: String,
    pub ram: u64,
    pub arch: String,
    pub maker: String,
    pub model: String,
}

pub fn about() -> About {
    let mut a = About {
        computer: std::env::var("COMPUTERNAME").unwrap_or_default(),
        user: std::env::var("USERNAME").unwrap_or_default(),
        cpu: crate::taskmgr::sys::cpu_name(),
        arch: match std::env::consts::ARCH {
            "x86_64" => "64-bit, x64".into(),
            "aarch64" => "64-bit, ARM64".into(),
            a => a.to_string(),
        },
        ..Default::default()
    };
    a.full_name = display_name().unwrap_or_else(|| a.user.clone());
    if let Some(k) = hklm(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion") {
        let s = |n: &str| k.get_value::<String, _>(n).unwrap_or_default();
        a.build = s("CurrentBuild");
        let mut edition = s("ProductName");
        // Windows 11 still calls itself "Windows 10" here.
        if a.build.parse::<u32>().unwrap_or(0) >= 22000 {
            edition = edition.replace("Windows 10", "Windows 11");
        }
        a.edition = edition;
        a.version = s("DisplayVersion");
        if let Ok(ubr) = k.get_value::<u32, _>("UBR") {
            a.build = format!("{}.{}", a.build, ubr);
        }
        if let Ok(t) = k.get_value::<u32, _>("InstallDate") {
            if let Some(d) = chrono::DateTime::from_timestamp(t as i64, 0) {
                a.installed = d.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string();
            }
        }
    }
    if let Some(k) = hklm(r"HARDWARE\DESCRIPTION\System\BIOS") {
        a.maker = k.get_value::<String, _>("SystemManufacturer").unwrap_or_default();
        a.model = k.get_value::<String, _>("SystemProductName").unwrap_or_default();
        for junk in ["To be filled by O.E.M.", "System manufacturer", "System Product Name", "Default string"] {
            if a.maker.eq_ignore_ascii_case(junk) {
                a.maker.clear();
            }
            if a.model.eq_ignore_ascii_case(junk) {
                a.model.clear();
            }
        }
    }
    unsafe {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut m: MEMORYSTATUSEX = zeroed();
        m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut m) != 0 {
            a.ram = m.ullTotalPhys;
        }
    }
    a
}

fn display_name() -> Option<String> {
    // NameDisplay = 3
    #[link(name = "secur32")]
    extern "system" {
        fn GetUserNameExW(format: i32, buf: *mut u16, size: *mut u32) -> u8;
    }
    let mut buf = [0u16; 256];
    let mut n = buf.len() as u32;
    let ok = unsafe { GetUserNameExW(3, buf.as_mut_ptr(), &mut n) } != 0;
    let s = from_wide(&buf);
    (ok && !s.trim().is_empty()).then_some(s)
}

/// Rename the computer (asks for administrator rights; takes effect after a restart).
pub fn rename_computer(name: &str) -> bool {
    let script = format!("Rename-Computer -NewName '{}' -Force -ErrorAction Stop", name.replace('\'', ""));
    run_elevated_wait("powershell.exe", &format!("-NoProfile -WindowStyle Hidden -Command \"{script}\"")) == Some(0)
}

pub fn valid_computer_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 15
        && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !n.chars().all(|c| c.is_ascii_digit())
}

// ------------------------------------------------------------------ account

#[derive(Clone, Default)]
pub struct Account {
    pub admin: bool,
    pub email: Option<String>,
    pub picture: Option<(usize, usize, Vec<u8>)>,
    pub sid: String,
}

pub fn account() -> Account {
    let mut acc = Account::default();
    if let Some(groups) = output("whoami.exe", &["/groups", "/fo", "csv", "/nh"]) {
        acc.admin = groups.contains("S-1-5-32-544");
    }
    if let Some(u) = output("whoami.exe", &["/user", "/fo", "csv", "/nh"]) {
        acc.sid = u.split(',').nth(1).unwrap_or("").trim().trim_matches('"').to_string();
    }
    if let Some(k) = hkcu(r"Software\Microsoft\IdentityCRL\UserExtendedProperties") {
        acc.email = k.enum_keys().flatten().find(|n| n.contains('@'));
    }
    // The account picture Windows shows on the sign-in screen.
    if !acc.sid.is_empty() {
        if let Some(k) = hklm(&format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\AccountPicture\Users\{}", acc.sid)) {
            for v in ["Image192", "Image240", "Image448", "Image96"] {
                if let Ok(p) = k.get_value::<String, _>(v) {
                    if let Some(img) = load_image(Path::new(&p), 96) {
                        acc.picture = Some(img);
                        break;
                    }
                }
            }
        }
    }
    acc
}

/// A picture scaled to fit `max` pixels (RGBA).
pub fn load_image(p: &Path, max: u32) -> Option<(usize, usize, Vec<u8>)> {
    let img = image::open(p).ok()?;
    let img = img.thumbnail(max, max).to_rgba8();
    Some((img.width() as usize, img.height() as usize, img.into_raw()))
}

// ------------------------------------------------------------------ power

#[derive(Clone)]
pub struct Plan {
    pub guid: windows_sys::core::GUID,
    pub name: String,
    pub active: bool,
}

const SUB_VIDEO: u128 = 0x7516b95f_f776_4464_8c53_06167f40cc99;
const VIDEOIDLE: u128 = 0x3c0bc021_c8a8_4e07_a973_6b14cbcb2b7e;
const SUB_SLEEP: u128 = 0x238c9fa8_0aad_41ed_83f4_97be242c8f20;
const STANDBYIDLE: u128 = 0x29f6c1db_86da_48c5_9fdb_f2b67b1f44da;

fn guid(v: u128) -> windows_sys::core::GUID {
    windows_sys::core::GUID::from_u128(v)
}

fn active_scheme() -> Option<windows_sys::core::GUID> {
    use windows_sys::Win32::System::Power::PowerGetActiveScheme;
    unsafe {
        let mut p: *mut windows_sys::core::GUID = null_mut();
        if PowerGetActiveScheme(null_mut(), &mut p) != 0 || p.is_null() {
            return None;
        }
        let g = *p;
        windows_sys::Win32::Foundation::LocalFree(p as _);
        Some(g)
    }
}

pub fn plans() -> Vec<Plan> {
    use windows_sys::Win32::System::Power::{PowerEnumerate, PowerReadFriendlyName, ACCESS_SCHEME};
    let active = active_scheme();
    let mut out = Vec::new();
    unsafe {
        for i in 0..32 {
            let mut g: windows_sys::core::GUID = zeroed();
            let mut n = size_of::<windows_sys::core::GUID>() as u32;
            if PowerEnumerate(null_mut(), null(), null(), ACCESS_SCHEME, i, &mut g as *mut _ as *mut u8, &mut n) != 0 {
                break;
            }
            let mut buf = [0u16; 256];
            let mut size = (buf.len() * 2) as u32;
            PowerReadFriendlyName(null_mut(), &g, null(), null(), buf.as_mut_ptr() as *mut u8, &mut size);
            let is_active = active.is_some_and(|a| (a.data1, a.data2, a.data3, a.data4) == (g.data1, g.data2, g.data3, g.data4));
            out.push(Plan { guid: g, name: from_wide(&buf), active: is_active });
        }
    }
    out
}

pub fn set_plan(g: &windows_sys::core::GUID) -> bool {
    unsafe { windows_sys::Win32::System::Power::PowerSetActiveScheme(null_mut(), g) == 0 }
}

/// Screen and sleep time-outs in seconds: (screen AC, screen DC, sleep AC, sleep DC). 0 = never.
pub fn timeouts() -> [u32; 4] {
    use windows_sys::Win32::System::Power::{PowerReadACValueIndex, PowerReadDCValueIndex};
    let mut out = [0u32; 4];
    let Some(s) = active_scheme() else { return out };
    unsafe {
        PowerReadACValueIndex(null_mut(), &s, &guid(SUB_VIDEO), &guid(VIDEOIDLE), &mut out[0]);
        PowerReadDCValueIndex(null_mut(), &s, &guid(SUB_VIDEO), &guid(VIDEOIDLE), &mut out[1]);
        PowerReadACValueIndex(null_mut(), &s, &guid(SUB_SLEEP), &guid(STANDBYIDLE), &mut out[2]);
        PowerReadDCValueIndex(null_mut(), &s, &guid(SUB_SLEEP), &guid(STANDBYIDLE), &mut out[3]);
    }
    out
}

/// `which`: 0 screen AC, 1 screen DC, 2 sleep AC, 3 sleep DC.
pub fn set_timeout(which: usize, secs: u32) -> bool {
    use windows_sys::Win32::System::Power::{PowerSetActiveScheme, PowerWriteACValueIndex, PowerWriteDCValueIndex};
    let Some(s) = active_scheme() else { return false };
    let (sub, set) = if which < 2 { (SUB_VIDEO, VIDEOIDLE) } else { (SUB_SLEEP, STANDBYIDLE) };
    unsafe {
        let r = if which % 2 == 0 {
            PowerWriteACValueIndex(null_mut(), &s, &guid(sub), &guid(set), secs)
        } else {
            PowerWriteDCValueIndex(null_mut(), &s, &guid(sub), &guid(set), secs)
        };
        r == 0 && PowerSetActiveScheme(null_mut(), &s) == 0
    }
}

// ------------------------------------------------------------------ storage

#[derive(Clone)]
pub struct Drive {
    pub root: String,
    pub label: String,
    pub fs: String,
    pub total: u64,
    pub free: u64,
    /// 2 removable, 3 fixed, 4 network, 5 CD
    pub kind: u32,
}

pub fn drives() -> Vec<Drive> {
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
    let mut out = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let root = format!("{}:\\", (b'A' + i as u8) as char);
        let w = wide(&root);
        let kind = unsafe { GetDriveTypeW(w.as_ptr()) };
        if !(2..=5).contains(&kind) {
            continue;
        }
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        if unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, &mut total, &mut free) } == 0 || total == 0 {
            continue;
        }
        let mut label = [0u16; 128];
        let mut fs = [0u16; 32];
        unsafe {
            GetVolumeInformationW(w.as_ptr(), label.as_mut_ptr(), label.len() as u32, null_mut(), null_mut(), null_mut(), fs.as_mut_ptr(), fs.len() as u32);
        }
        out.push(Drive { root, label: from_wide(&label), fs: from_wide(&fs), total, free, kind });
    }
    out
}

pub fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() {
            n += dir_size(&e.path());
        } else {
            n += md.len();
        }
    }
    n
}

/// Delete what can be deleted in the user's temp folder; returns the bytes freed.
pub fn clean_temp() -> u64 {
    let dir = std::env::temp_dir();
    let before = dir_size(&dir);
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            } else {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    before.saturating_sub(dir_size(&dir))
}

/// Size of the Recycle Bin on all drives (bytes, items).
pub fn recycle_bin() -> (u64, u64) {
    use windows_sys::Win32::UI::Shell::{SHQueryRecycleBinW, SHQUERYRBINFO};
    let mut info: SHQUERYRBINFO = unsafe { zeroed() };
    info.cbSize = size_of::<SHQUERYRBINFO>() as u32;
    if unsafe { SHQueryRecycleBinW(null(), &mut info) } == 0 {
        (info.i64Size as u64, info.i64NumItems as u64)
    } else {
        (0, 0)
    }
}

pub fn empty_recycle_bin() -> bool {
    use windows_sys::Win32::UI::Shell::SHEmptyRecycleBinW;
    // SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND
    unsafe { SHEmptyRecycleBinW(null_mut(), null(), 0x1 | 0x2 | 0x4) >= 0 }
}

// ------------------------------------------------------------------ network

#[derive(Clone)]
pub struct Adapter {
    pub name: String,
    pub description: String,
    pub up: bool,
    pub wifi: bool,
    pub ipv4: Vec<String>,
    pub ipv6: Vec<String>,
    pub mac: String,
    pub speed: u64,
    pub gateway: Vec<String>,
    pub dns: Vec<String>,
}

unsafe fn sockaddr_text(sa: *const windows_sys::Win32::Networking::WinSock::SOCKADDR) -> Option<String> {
    use windows_sys::Win32::Networking::WinSock::{SOCKADDR_IN, SOCKADDR_IN6};
    if sa.is_null() {
        return None;
    }
    match (*sa).sa_family {
        2 => {
            let a = &*(sa as *const SOCKADDR_IN);
            let b = a.sin_addr.S_un.S_addr.to_ne_bytes();
            Some(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]))
        }
        23 => {
            let a = &*(sa as *const SOCKADDR_IN6);
            Some(std::net::Ipv6Addr::from(a.sin6_addr.u.Byte).to_string())
        }
        _ => None,
    }
}

pub fn adapters() -> Vec<Adapter> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH};
    let mut out = Vec::new();
    unsafe {
        let mut size = 32 * 1024u32;
        let mut buf: Vec<u64> = vec![0; size as usize / 8];
        // GAA_FLAG_INCLUDE_GATEWAYS = 0x80
        let mut r = GetAdaptersAddresses(0, 0x80, null(), buf.as_mut_ptr() as *mut _, &mut size);
        if r == 111 {
            buf = vec![0; size as usize / 8 + 1];
            r = GetAdaptersAddresses(0, 0x80, null(), buf.as_mut_ptr() as *mut _, &mut size);
        }
        if r != 0 {
            return out;
        }
        let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !p.is_null() {
            let a = &*p;
            p = a.Next;
            // Loopback (24) and tunnels (131) are not interesting.
            if a.IfType == 24 || a.IfType == 131 {
                continue;
            }
            let text = |w: *const u16| {
                if w.is_null() {
                    return String::new();
                }
                let mut n = 0;
                while *w.add(n) != 0 {
                    n += 1;
                }
                String::from_utf16_lossy(std::slice::from_raw_parts(w, n))
            };
            let description = text(a.Description);
            // Virtual helpers Windows adds (WAN Miniport, Wi-Fi Direct, filters, …).
            let d = description.to_lowercase();
            if d.contains("miniport") || d.contains("wi-fi direct") || d.contains("kernel debug") {
                continue;
            }
            let mut ad = Adapter {
                name: text(a.FriendlyName),
                description,
                up: a.OperStatus == 1,
                wifi: a.IfType == 71,
                ipv4: Vec::new(),
                ipv6: Vec::new(),
                mac: a.PhysicalAddress[..a.PhysicalAddressLength as usize]
                    .iter()
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join("-"),
                speed: a.TransmitLinkSpeed,
                gateway: Vec::new(),
                dns: Vec::new(),
            };
            let mut u = a.FirstUnicastAddress;
            while !u.is_null() {
                if let Some(t) = sockaddr_text((*u).Address.lpSockaddr) {
                    if t.contains(':') {
                        ad.ipv6.push(t);
                    } else {
                        ad.ipv4.push(t);
                    }
                }
                u = (*u).Next;
            }
            let mut g = a.FirstGatewayAddress;
            while !g.is_null() {
                if let Some(t) = sockaddr_text((*g).Address.lpSockaddr) {
                    ad.gateway.push(t);
                }
                g = (*g).Next;
            }
            let mut dns = a.FirstDnsServerAddress;
            while !dns.is_null() {
                if let Some(t) = sockaddr_text((*dns).Address.lpSockaddr) {
                    ad.dns.push(t);
                }
                dns = (*dns).Next;
            }
            out.push(ad);
        }
    }
    // Connected first.
    out.sort_by_key(|a| !a.up);
    out
}

// ------------------------------------------------------------------ personalization

const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

pub fn transparency() -> bool {
    hkcu(PERSONALIZE).and_then(|k| k.get_value::<u32, _>("EnableTransparency").ok()).unwrap_or(1) == 1
}

fn set_personalize(name: &str, v: u32) {
    if let Ok((k, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(PERSONALIZE) {
        let _ = k.set_value(name, &v);
    }
}

fn broadcast(what: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE};
    let w = wide(what);
    unsafe {
        SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, w.as_ptr() as isize, SMTO_ABORTIFHUNG, 500, null_mut());
    }
}

pub fn set_windows_light(light: bool) {
    set_personalize("AppsUseLightTheme", light as u32);
    set_personalize("SystemUsesLightTheme", light as u32);
    std::thread::spawn(|| broadcast("ImmersiveColorSet"));
}

pub fn set_transparency(on: bool) {
    set_personalize("EnableTransparency", on as u32);
    std::thread::spawn(|| broadcast("ImmersiveColorSet"));
}

pub fn wallpaper() -> Option<PathBuf> {
    use windows_sys::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;
    let mut buf = [0u16; 1024];
    let ok = unsafe { SystemParametersInfoW(0x0073, buf.len() as u32, buf.as_mut_ptr() as *mut _, 0) };
    let s = from_wide(&buf);
    (ok != 0 && !s.is_empty()).then(|| PathBuf::from(s))
}

pub fn set_wallpaper(p: &Path) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;
    let w = wide(&p.to_string_lossy());
    unsafe { SystemParametersInfoW(0x0014, 0, w.as_ptr() as *mut _, 0x1 | 0x2) != 0 }
}

/// How the picture fits the screen: Fill, Fit, Stretch, Tile, Centre, Span.
pub const FITS: [(&str, &str, &str); 6] =
    [("Fill", "10", "0"), ("Fit", "6", "0"), ("Stretch", "2", "0"), ("Tile", "0", "1"), ("Center", "0", "0"), ("Span", "22", "0")];

pub fn wallpaper_fit() -> usize {
    let k = hkcu(r"Control Panel\Desktop");
    let style = k.as_ref().and_then(|k| k.get_value::<String, _>("WallpaperStyle").ok()).unwrap_or_else(|| "10".into());
    let tile = k.as_ref().and_then(|k| k.get_value::<String, _>("TileWallpaper").ok()).unwrap_or_else(|| "0".into());
    FITS.iter().position(|f| f.1 == style && f.2 == tile).unwrap_or(0)
}

pub fn set_wallpaper_fit(i: usize) {
    let (_, style, tile) = FITS[i.min(FITS.len() - 1)];
    if let Ok((k, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(r"Control Panel\Desktop") {
        let _ = k.set_value("WallpaperStyle", &style);
        let _ = k.set_value("TileWallpaper", &tile);
    }
    if let Some(p) = wallpaper() {
        set_wallpaper(&p);
    }
}

/// The standard Windows "Open" dialog, filtered to pictures.
pub fn pick_image(owner: isize, title: &str, filter_name: &str) -> Option<PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OPENFILENAMEW};
    let filter: Vec<u16> = format!("{filter_name}\0*.jpg;*.jpeg;*.png;*.bmp;*.webp\0\0").encode_utf16().collect();
    let start = dirs::picture_dir().map(|p| wide(&p.to_string_lossy())).unwrap_or_else(|| vec![0]);
    let title = wide(title);
    let mut file = vec![0u16; 1024];
    unsafe {
        let mut ofn: OPENFILENAMEW = zeroed();
        ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = owner as _;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrFile = file.as_mut_ptr();
        ofn.nMaxFile = file.len() as u32;
        ofn.lpstrInitialDir = start.as_ptr();
        ofn.lpstrTitle = title.as_ptr();
        ofn.Flags = 0x1000 | 0x0800 | 0x0008;
        (GetOpenFileNameW(&mut ofn) != 0).then(|| PathBuf::from(from_wide(&file)))
    }
}

// ------------------------------------------------------------------ mouse

#[derive(Clone, Copy, PartialEq)]
pub struct Mouse {
    pub swap: bool,
    /// 1..=20
    pub speed: u32,
    pub precision: bool,
    pub wheel_lines: u32,
}

pub fn mouse() -> Mouse {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SystemParametersInfoW};
    unsafe {
        let mut speed = 10u32;
        SystemParametersInfoW(0x0070, 0, &mut speed as *mut _ as *mut _, 0);
        let mut acc = [0i32; 3];
        SystemParametersInfoW(0x0003, 0, acc.as_mut_ptr() as *mut _, 0);
        let mut lines = 3u32;
        SystemParametersInfoW(0x0068, 0, &mut lines as *mut _ as *mut _, 0);
        Mouse { swap: GetSystemMetrics(23) != 0, speed, precision: acc[2] != 0, wheel_lines: lines }
    }
}

pub fn set_mouse(m: Mouse) {
    use windows_sys::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;
    const SAVE: u32 = 0x1 | 0x2;
    unsafe {
        SystemParametersInfoW(0x0021, m.swap as u32, null_mut(), SAVE);
        SystemParametersInfoW(0x0071, 0, m.speed.clamp(1, 20) as usize as *mut _, SAVE);
        let mut acc: [i32; 3] = if m.precision { [6, 10, 1] } else { [0, 0, 0] };
        SystemParametersInfoW(0x0004, 0, acc.as_mut_ptr() as *mut _, SAVE);
        SystemParametersInfoW(0x0069, m.wheel_lines, null_mut(), SAVE);
    }
}

// ------------------------------------------------------------------ installed apps

#[derive(Clone)]
pub struct InstalledApp {
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub date: String,
    pub size: u64,
    pub uninstall: String,
    pub modify: Option<String>,
    pub icon: Option<PathBuf>,
}

pub fn installed_apps() -> Vec<InstalledApp> {
    use winreg::enums::KEY_WOW64_32KEY;
    let mut out: Vec<InstalledApp> = Vec::new();
    let path = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    let roots = [
        (RegKey::predef(HKEY_LOCAL_MACHINE), KEY_WOW64_64KEY),
        (RegKey::predef(HKEY_LOCAL_MACHINE), KEY_WOW64_32KEY),
        (RegKey::predef(HKEY_CURRENT_USER), 0),
    ];
    for (root, flag) in roots {
        let Ok(k) = root.open_subkey_with_flags(path, KEY_READ | flag) else { continue };
        for sub in k.enum_keys().flatten() {
            let Ok(e) = k.open_subkey_with_flags(&sub, KEY_READ | flag) else { continue };
            let s = |n: &str| e.get_value::<String, _>(n).unwrap_or_default();
            let name = s("DisplayName");
            if name.trim().is_empty()
                || e.get_value::<u32, _>("SystemComponent").unwrap_or(0) == 1
                || !s("ParentKeyName").is_empty()
                || !s("ReleaseType").is_empty()
            {
                continue;
            }
            let uninstall = s("UninstallString");
            if uninstall.is_empty() {
                continue;
            }
            if out.iter().any(|a| a.name == name) {
                continue;
            }
            let date = s("InstallDate");
            let date = match (date.len(), date.get(4..6).and_then(|m| m.parse::<u32>().ok()), date.get(6..8).and_then(|d| d.parse::<u32>().ok())) {
                (8, Some(m), Some(d)) if (1..=12).contains(&m) && (1..=31).contains(&d) => format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..]),
                _ => String::new(),
            };
            let icon = {
                let raw = s("DisplayIcon");
                let raw = raw.split(',').next().unwrap_or("").trim().trim_matches('"').to_string();
                let p = PathBuf::from(&raw);
                (!raw.is_empty() && p.is_file()).then_some(p)
            };
            let no_modify = e.get_value::<u32, _>("NoModify").unwrap_or(0) == 1;
            let modify = s("ModifyPath");
            out.push(InstalledApp {
                name,
                version: s("DisplayVersion"),
                publisher: s("Publisher"),
                date,
                size: e.get_value::<u32, _>("EstimatedSize").unwrap_or(0) as u64 * 1024,
                uninstall,
                modify: (!no_modify && !modify.is_empty()).then_some(modify),
                icon,
            });
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

// ------------------------------------------------------------------ time

pub fn time_zones() -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(k) = hklm(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Time Zones") {
        for id in k.enum_keys().flatten() {
            let display = k.open_subkey(&id).and_then(|s| s.get_value::<String, _>("Display")).unwrap_or_else(|_| id.clone());
            out.push((id, display));
        }
    }
    // By UTC offset, like Windows: "(UTC-05:00) …"
    fn offset(d: &str) -> i32 {
        let t = d.get(4..10).unwrap_or("");
        let sign = if t.starts_with('-') { -1 } else { 1 };
        let h: i32 = t.get(1..3).and_then(|v| v.parse().ok()).unwrap_or(0);
        let m: i32 = t.get(4..6).and_then(|v| v.parse().ok()).unwrap_or(0);
        sign * (h * 60 + m)
    }
    out.sort_by(|a, b| (offset(&a.1), &a.1).cmp(&(offset(&b.1), &b.1)));
    out
}

pub fn current_time_zone() -> String {
    hklm(r"SYSTEM\CurrentControlSet\Control\TimeZoneInformation")
        .and_then(|k| k.get_value::<String, _>("TimeZoneKeyName").ok())
        .unwrap_or_default()
        .trim_end_matches('\0')
        .to_string()
}

pub fn set_time_zone(id: &str) -> bool {
    Command::new("tzutil.exe").args(["/s", id]).creation_flags(NO_WINDOW).status().is_ok_and(|s| s.success())
}

/// Is the clock set from the internet?
pub fn auto_time() -> bool {
    hklm(r"SYSTEM\CurrentControlSet\Services\W32Time\Parameters")
        .and_then(|k| k.get_value::<String, _>("Type").ok())
        .is_some_and(|t| !t.eq_ignore_ascii_case("NoSync"))
}

pub fn time_server() -> String {
    hklm(r"SYSTEM\CurrentControlSet\Services\W32Time\Parameters")
        .and_then(|k| k.get_value::<String, _>("NtpServer").ok())
        .map(|s| s.split(',').next().unwrap_or("").to_string())
        .unwrap_or_default()
}

pub fn sync_time() -> bool {
    run_elevated_wait("cmd.exe", "/c net start w32time >nul 2>&1 & w32tm /resync /force") == Some(0)
}

// ------------------------------------------------------------------ Windows Update

#[derive(Clone)]
pub struct Update {
    pub title: String,
    pub size: u64,
}

#[derive(Clone)]
pub struct HistoryItem {
    pub date: String,
    pub title: String,
    /// 2 succeeded, 3 succeeded with errors, 4 failed, 5 aborted
    pub result: u32,
}

const WU_SEARCH: &str = r#"$s=New-Object -ComObject Microsoft.Update.Session
try { $r=$s.CreateUpdateSearcher().Search("IsInstalled=0 and IsHidden=0 and Type='Software'")
foreach($u in $r.Updates){ "U`t"+$u.Title+"`t"+[int64]$u.MaxDownloadSize } } catch { "E`t"+$_.Exception.Message }
$h=$s.CreateUpdateSearcher(); $n=$h.GetTotalHistoryCount()
if($n -gt 0){ foreach($e in $h.QueryHistory(0,[Math]::Min($n,60))){ if($e.Title){ "H`t"+$e.Date.ToLocalTime().ToString('yyyy-MM-dd HH:mm')+"`t"+$e.Title+"`t"+[int]$e.ResultCode } } }"#;

/// Look for updates (slow: up to a minute). Err = Windows Update's message.
pub fn search_updates() -> Result<(Vec<Update>, Vec<HistoryItem>), String> {
    let out = powershell(WU_SEARCH).ok_or_else(|| "PowerShell".to_string())?;
    let (mut ups, mut hist) = (Vec::new(), Vec::new());
    for line in out.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        match f.as_slice() {
            ["U", title, size] => ups.push(Update { title: title.to_string(), size: size.trim().parse().unwrap_or(0) }),
            ["H", date, title, res] => hist.push(HistoryItem { date: date.to_string(), title: title.to_string(), result: res.trim().parse().unwrap_or(0) }),
            ["E", msg] => return Err(msg.to_string()),
            _ => {}
        }
    }
    // The same update tried twice shows twice in the history; keep the newest.
    let mut seen = std::collections::HashSet::new();
    hist.retain(|h| seen.insert(h.title.clone()));
    Ok((ups, hist))
}

const WU_INSTALL: &str = r#"$s=New-Object -ComObject Microsoft.Update.Session
$r=$s.CreateUpdateSearcher().Search("IsInstalled=0 and IsHidden=0 and Type='Software'")
$c=New-Object -ComObject Microsoft.Update.UpdateColl
foreach($u in $r.Updates){ if(-not $u.EulaAccepted){ $u.AcceptEula() }; [void]$c.Add($u) }
if($c.Count -eq 0){ exit 0 }
$d=$s.CreateUpdateDownloader(); $d.Updates=$c; [void]$d.Download()
$i=$s.CreateUpdateInstaller(); $i.Updates=$c; $res=$i.Install()
if($res.RebootRequired){ exit 10 }
if($res.ResultCode -eq 2){ exit 0 }
exit $res.ResultCode"#;

/// Download and install every available update as administrator.
/// Ok(true) = a restart is needed.
pub fn install_updates() -> Result<bool, u32> {
    let script = std::env::temp_dir().join("delfin-wu-install.ps1");
    std::fs::write(&script, WU_INSTALL).map_err(|_| 1u32)?;
    let args = format!("-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File \"{}\"", script.display());
    let r = run_elevated_wait("powershell.exe", &args);
    let _ = std::fs::remove_file(&script);
    match r {
        Some(0) => Ok(false),
        Some(10) => Ok(true),
        Some(c) => Err(c),
        None => Err(u32::MAX),
    }
}

pub fn reboot_pending() -> bool {
    hklm(r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired").is_some()
        || hklm(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending").is_some()
}

// ------------------------------------------------------------------ Bluetooth radio (WinRT)

/// Some(on) when the PC has a Bluetooth radio.
pub fn bluetooth() -> Option<bool> {
    use windows::Devices::Radios::{Radio, RadioKind, RadioState};
    let radios = Radio::GetRadiosAsync().ok()?.join().ok()?;
    for r in radios {
        if r.Kind().ok()? == RadioKind::Bluetooth {
            return Some(r.State().ok()? == RadioState::On);
        }
    }
    None
}

pub fn set_bluetooth(on: bool) -> bool {
    use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
    let Ok(Ok(access)) = Radio::RequestAccessAsync().map(|a| a.join()) else { return false };
    if access != RadioAccessStatus::Allowed {
        return false;
    }
    let Ok(Ok(radios)) = Radio::GetRadiosAsync().map(|a| a.join()) else { return false };
    for r in radios {
        if r.Kind().is_ok_and(|k| k == RadioKind::Bluetooth) {
            let st = if on { RadioState::On } else { RadioState::Off };
            return r.SetStateAsync(st).ok().and_then(|a| a.join().ok()).is_some_and(|s| s == RadioAccessStatus::Allowed);
        }
    }
    false
}
