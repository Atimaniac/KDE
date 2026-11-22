//! Data for the task manager, sampled once a second on a background thread:
//! processes (Toolhelp + process times / memory / I/O), system totals and
//! per-core CPU, disks, network and GPU (performance counters), GPU
//! temperature (NVIDIA's nvml.dll when present).

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, HWND, LPARAM, BOOL};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    PDH_FMT_COUNTERVALUE_ITEM_W,
};
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX2};
use windows_sys::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    GetPriorityClass, GetProcessInformation, GetProcessIoCounters, GetProcessTimes, GetSystemTimes,
    OpenProcess, QueryFullProcessImageNameW, IO_COUNTERS, PROCESS_POWER_THROTTLING_STATE,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

pub const HISTORY: usize = 60;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn from_wide(b: &[u16]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..n])
}

unsafe fn pwstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// Executable name, e.g. "brave.exe".
    pub exe: String,
    pub path: Option<PathBuf>,
    /// Friendly name from the file's version info, e.g. "Brave Browser".
    pub desc: String,
    /// Share of all CPUs, 0..100.
    pub cpu: f32,
    /// Private working set, bytes (what Windows Task Manager shows).
    pub mem: u64,
    /// Read + write bytes per second.
    pub io: f64,
    /// GPU 3D engine share, 0..100.
    pub gpu: f32,
    pub priority: u32,
    pub efficiency: bool,
    /// Title of its main window, if it has one on the taskbar.
    pub window: Option<String>,
    pub user: String,
}

#[derive(Clone, Default, Debug)]
pub struct Disk {
    pub name: String,
    pub active: f32,
    pub read: f64,
    pub write: f64,
    pub history: Vec<f32>,
}

#[derive(Clone, Default, Debug)]
pub struct Net {
    pub name: String,
    pub rx: f64,
    pub tx: f64,
    /// bits per second, for the graph
    pub history: Vec<f32>,
}

#[derive(Clone, Default, Debug)]
pub struct Gpu {
    pub name: String,
    pub util: f32,
    pub temp: Option<u32>,
    pub history: Vec<f32>,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub procs: Vec<Proc>,
    pub cpu: f32,
    pub cpu_cores: Vec<f32>,
    pub cpu_ghz: f32,
    pub cpu_name: String,
    pub cpu_history: Vec<f32>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub commit_used: u64,
    pub commit_total: u64,
    pub mem_history: Vec<f32>,
    pub disks: Vec<Disk>,
    pub nets: Vec<Net>,
    pub gpu: Option<Gpu>,
    pub uptime: Duration,
    pub threads: u32,
}

impl Snapshot {
    pub fn mem_percent(&self) -> f32 {
        if self.mem_total == 0 {
            0.0
        } else {
            self.mem_used as f32 * 100.0 / self.mem_total as f32
        }
    }
}

pub fn push(h: &mut Vec<f32>, v: f32) {
    if h.is_empty() {
        h.resize(HISTORY, 0.0);
    }
    h.remove(0);
    h.push(v);
}

// ------------------------------------------------------------------ performance counters

struct Counter(isize);

struct Pdh {
    query: isize,
    cpu_util: Counter,
    cpu_perf: Counter,
    cpu_freq: Counter,
    disk_idle: Counter,
    disk_read: Counter,
    disk_write: Counter,
    net_rx: Counter,
    net_tx: Counter,
    gpu: Counter,
}

impl Pdh {
    fn open() -> Option<Self> {
        unsafe {
            let mut q = 0isize;
            if PdhOpenQueryW(null(), 0, &mut q) != 0 {
                return None;
            }
            let add = |path: &str| {
                let mut c = 0isize;
                let w = wide(path);
                PdhAddEnglishCounterW(q, w.as_ptr(), 0, &mut c);
                Counter(c)
            };
            let p = Pdh {
                query: q,
                cpu_util: add(r"\Processor Information(*)\% Processor Utility"),
                cpu_perf: add(r"\Processor Information(_Total)\% Processor Performance"),
                cpu_freq: add(r"\Processor Information(_Total)\Processor Frequency"),
                disk_idle: add(r"\PhysicalDisk(*)\% Idle Time"),
                disk_read: add(r"\PhysicalDisk(*)\Disk Read Bytes/sec"),
                disk_write: add(r"\PhysicalDisk(*)\Disk Write Bytes/sec"),
                net_rx: add(r"\Network Interface(*)\Bytes Received/sec"),
                net_tx: add(r"\Network Interface(*)\Bytes Sent/sec"),
                gpu: add(r"\GPU Engine(*)\Utilization Percentage"),
            };
            PdhCollectQueryData(q);
            Some(p)
        }
    }

    /// All instances of a counter: (name, value).
    fn array(c: &Counter) -> Vec<(String, f64)> {
        const PDH_FMT_DOUBLE: u32 = 0x200;
        const PDH_FMT_NOCAP100: u32 = 0x8000;
        const PDH_MORE_DATA: u32 = 0x8000_07D2;
        if c.0 == 0 {
            return Vec::new();
        }
        unsafe {
            let (mut size, mut count) = (0u32, 0u32);
            let r = PdhGetFormattedCounterArrayW(c.0, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, &mut size, &mut count, null_mut());
            if r != PDH_MORE_DATA || size == 0 {
                return Vec::new();
            }
            let mut buf: Vec<u64> = vec![0; size as usize / 8 + 1];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(c.0, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, &mut size, &mut count, items) != 0 {
                return Vec::new();
            }
            (0..count as usize)
                .map(|i| {
                    let it = &*items.add(i);
                    (pwstr(it.szName), it.FmtValue.Anonymous.doubleValue)
                })
                .collect()
        }
    }

    fn collect(&self) {
        unsafe { PdhCollectQueryData(self.query) };
    }
}

// ------------------------------------------------------------------ GPU temperature (NVIDIA)

/// NVIDIA's management library, only for the GPU temperature. It costs
/// ~20 MB, so it is loaded only while the Performance page is open (see
/// `WANT_GPU_TEMP`) and unloaded again after.
struct Nvml {
    temp: unsafe extern "C" fn(*mut c_void, u32, *mut u32) -> i32,
    dev: *mut c_void,
    shutdown: unsafe extern "C" fn() -> i32,
    lib: windows_sys::Win32::Foundation::HMODULE,
}

impl Drop for Nvml {
    fn drop(&mut self) {
        unsafe {
            (self.shutdown)();
            windows_sys::Win32::Foundation::FreeLibrary(self.lib);
        }
    }
}

/// The Performance page is showing: read the GPU temperature (loads NVML).
pub static WANT_GPU_TEMP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe impl Send for Nvml {}

impl Nvml {
    fn load() -> Option<Self> {
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        unsafe {
            let lib = LoadLibraryW(wide("nvml.dll").as_ptr());
            let lib = if lib.is_null() {
                LoadLibraryW(wide(r"C:\Program Files\NVIDIA Corporation\NVSMI\nvml.dll").as_ptr())
            } else {
                lib
            };
            if lib.is_null() {
                return None;
            }
            let init: unsafe extern "C" fn() -> i32 = std::mem::transmute(GetProcAddress(lib, b"nvmlInit_v2\0".as_ptr())?);
            let handle: unsafe extern "C" fn(u32, *mut *mut c_void) -> i32 =
                std::mem::transmute(GetProcAddress(lib, b"nvmlDeviceGetHandleByIndex_v2\0".as_ptr())?);
            let temp: unsafe extern "C" fn(*mut c_void, u32, *mut u32) -> i32 =
                std::mem::transmute(GetProcAddress(lib, b"nvmlDeviceGetTemperature\0".as_ptr())?);
            let shutdown: unsafe extern "C" fn() -> i32 = std::mem::transmute(GetProcAddress(lib, b"nvmlShutdown\0".as_ptr())?);
            if init() != 0 {
                windows_sys::Win32::Foundation::FreeLibrary(lib);
                return None;
            }
            let mut dev = null_mut();
            if handle(0, &mut dev) != 0 {
                shutdown();
                windows_sys::Win32::Foundation::FreeLibrary(lib);
                return None;
            }
            Some(Nvml { temp, dev, shutdown, lib })
        }
    }

    fn temperature(&self) -> Option<u32> {
        let mut t = 0u32;
        (unsafe { (self.temp)(self.dev, 0, &mut t) } == 0).then_some(t)
    }
}

// ------------------------------------------------------------------ processes

struct Prev {
    created: u64,
    cpu: u64,
    io: u64,
}

#[derive(Clone)]
struct Static {
    created: u64,
    path: Option<PathBuf>,
    desc: String,
    user: String,
}

unsafe extern "system" fn windows_cb(h: HWND, lp: LPARAM) -> BOOL {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindow, GetWindowLongPtrW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, GW_OWNER,
    };
    let out = &mut *(lp as *mut HashMap<u32, String>);
    if IsWindowVisible(h) == 0 || !GetWindow(h, GW_OWNER).is_null() {
        return 1;
    }
    let ex = GetWindowLongPtrW(h, -20) as u32;
    if ex & 0x80 != 0 && ex & 0x4_0000 == 0 {
        return 1; // tool window
    }
    // Cloaked windows (suspended Store apps) aren't really open.
    let mut cloaked = 0u32;
    windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(h, 14, &mut cloaked as *mut u32 as *mut c_void, 4);
    if cloaked != 0 {
        return 1;
    }
    let mut buf = [0u16; 256];
    let n = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
    if n <= 0 {
        return 1;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(h, &mut pid);
    out.entry(pid).or_insert_with(|| String::from_utf16_lossy(&buf[..n as usize]));
    1
}

fn process_windows() -> HashMap<u32, String> {
    let mut m = HashMap::new();
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows(Some(windows_cb), &mut m as *mut _ as LPARAM) };
    m
}

fn image_path(h: HANDLE) -> Option<PathBuf> {
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    (unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) } != 0)
        .then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

fn process_user(h: HANDLE) -> String {
    use windows_sys::Win32::Security::{GetTokenInformation, LookupAccountSidW, TokenUser, TOKEN_USER};
    use windows_sys::Win32::System::Threading::OpenProcessToken;
    unsafe {
        let mut tok: HANDLE = null_mut();
        if OpenProcessToken(h, 0x0008 /* TOKEN_QUERY */, &mut tok) == 0 {
            return String::new();
        }
        let mut buf = [0u8; 256];
        let mut len = 0u32;
        let ok = GetTokenInformation(tok, TokenUser, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut len);
        CloseHandle(tok);
        if ok == 0 {
            return String::new();
        }
        let tu = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut name = [0u16; 128];
        let mut nlen = name.len() as u32;
        let mut dom = [0u16; 128];
        let mut dlen = dom.len() as u32;
        let mut use_ = 0i32;
        if LookupAccountSidW(null(), tu.User.Sid, name.as_mut_ptr(), &mut nlen, dom.as_mut_ptr(), &mut dlen, &mut use_) == 0 {
            return String::new();
        }
        from_wide(&name)
    }
}

/// FileDescription from a program's version information ("Brave Browser").
pub fn file_description(exe: &Path) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    let w = wide(&exe.to_string_lossy());
    unsafe {
        let mut handle = 0u32;
        let n = GetFileVersionInfoSizeW(w.as_ptr(), &mut handle);
        if n == 0 {
            return None;
        }
        let mut buf = vec![0u8; n as usize];
        if GetFileVersionInfoW(w.as_ptr(), 0, n, buf.as_mut_ptr() as *mut _) == 0 {
            return None;
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
        let key = wide(&format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\FileDescription"));
        if VerQueryValueW(buf.as_ptr() as *const _, key.as_ptr(), &mut p, &mut len) == 0 || len == 0 {
            return None;
        }
        let s = from_wide(std::slice::from_raw_parts(p as *const u16, len as usize));
        let s = s.trim().to_string();
        (!s.is_empty()).then_some(s)
    }
}

fn efficiency_on(h: HANDLE, priority: u32) -> bool {
    unsafe {
        let mut st: PROCESS_POWER_THROTTLING_STATE = zeroed();
        st.Version = 1;
        let ok = GetProcessInformation(h, 4 /* ProcessPowerThrottling */, &mut st as *mut _ as *mut c_void, size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32);
        ok != 0 && st.StateMask & 1 != 0 && priority == 0x40 // IDLE_PRIORITY_CLASS
    }
}

// ------------------------------------------------------------------ sampler

pub fn cpu_name() -> String {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0")
        .and_then(|k| k.get_value::<String, _>("ProcessorNameString"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn gpu_name() -> String {
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayDevicesW, DISPLAY_DEVICEW};
    unsafe {
        for i in 0..8 {
            let mut dd: DISPLAY_DEVICEW = zeroed();
            dd.cb = size_of::<DISPLAY_DEVICEW>() as u32;
            if EnumDisplayDevicesW(null(), i, &mut dd, 0) == 0 {
                break;
            }
            if dd.StateFlags & 0x4 != 0 {
                return from_wide(&dd.DeviceString);
            }
        }
    }
    String::new()
}

/// Virtual / tunnel adapters nobody wants to see in a network list.
fn boring_adapter(name: &str) -> bool {
    let l = name.to_lowercase();
    ["isatap", "teredo", "loopback", "wan miniport", "6to4", "kernel debug", "hyper-v", "vethernet", "wfp", "qos packet", "npcap", "pseudo"]
        .iter()
        .any(|k| l.contains(k))
}

pub fn start(ctx: eframe::egui::Context) -> Arc<Mutex<Snapshot>> {
    let out = Arc::new(Mutex::new(Snapshot::default()));
    let shared = out.clone();
    std::thread::Builder::new()
        .name("delfin-taskmgr-sampler".into())
        .spawn(move || sampler(ctx, shared))
        .ok();
    out
}

fn sampler(ctx: eframe::egui::Context, out: Arc<Mutex<Snapshot>>) {
    let pdh = Pdh::open();
    let mut nvml: Option<Nvml> = None;
    // No NVIDIA library on this PC: don't keep trying.
    let mut nvml_missing = false;
    let ncpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
    let cpu_name = cpu_name();
    let gpu_name = gpu_name();
    let mut prev: HashMap<u32, Prev> = HashMap::new();
    let mut statics: HashMap<u32, Static> = HashMap::new();
    let mut descs: HashMap<PathBuf, String> = HashMap::new();
    let mut last = Instant::now();
    let (mut sys_idle, mut sys_kernel, mut sys_user) = (0u64, 0u64, 0u64);
    let mut cpu_hist = Vec::new();
    let mut mem_hist = Vec::new();
    let mut disk_hist: HashMap<String, Vec<f32>> = HashMap::new();
    let mut net_hist: HashMap<String, Vec<f32>> = HashMap::new();
    let mut gpu_hist = Vec::new();
    let mut first = true;

    loop {
        // Load or drop NVIDIA's library as the Performance page opens and closes.
        if WANT_GPU_TEMP.load(std::sync::atomic::Ordering::Relaxed) {
            if nvml.is_none() && !nvml_missing {
                nvml = Nvml::load();
                nvml_missing = nvml.is_none();
            }
        } else {
            nvml = None;
        }
        let elapsed = last.elapsed().as_secs_f64().max(0.001);
        last = Instant::now();
        if let Some(p) = &pdh {
            p.collect();
        }

        // ---- GPU per process (3D engine) and per engine
        let gpu_items = pdh.as_ref().map(|p| Pdh::array(&p.gpu)).unwrap_or_default();
        let mut gpu_by_pid: HashMap<u32, f32> = HashMap::new();
        let mut gpu_by_engine: HashMap<String, f32> = HashMap::new();
        for (name, v) in &gpu_items {
            // pid_1234_luid_0x..._phys_0_eng_3_engtype_3D
            let pid = name.strip_prefix("pid_").and_then(|r| r.split('_').next()).and_then(|s| s.parse::<u32>().ok());
            let engine = name.split("_eng_").nth(1).unwrap_or("").to_string();
            *gpu_by_engine.entry(engine).or_default() += *v as f32;
            if name.ends_with("engtype_3D") {
                if let Some(pid) = pid {
                    *gpu_by_pid.entry(pid).or_default() += *v as f32;
                }
            }
        }

        // ---- processes
        let windows = process_windows();
        let mut procs = Vec::new();
        let mut seen = HashSet::new();
        let mut total_threads = 0u32;
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if !snap.is_null() && snap as isize != -1 {
                let mut e: PROCESSENTRY32W = zeroed();
                e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
                let mut ok = Process32FirstW(snap, &mut e) != 0;
                while ok {
                    let pid = e.th32ProcessID;
                    total_threads += e.cntThreads;
                    if pid != 0 {
                        seen.insert(pid);
                        let exe = from_wide(&e.szExeFile);
                        let mut p = Proc {
                            pid,
                            ppid: e.th32ParentProcessID,
                            exe: exe.clone(),
                            path: None,
                            desc: String::new(),
                            cpu: 0.0,
                            mem: 0,
                            io: 0.0,
                            gpu: gpu_by_pid.get(&pid).copied().unwrap_or(0.0).min(100.0),
                            priority: 0,
                            efficiency: false,
                            window: windows.get(&pid).cloned(),
                            user: String::new(),
                        };
                        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                        if !h.is_null() {
                            let (mut c, mut x, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = (zeroed(), zeroed(), zeroed(), zeroed());
                            let created = if GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) != 0 { ft(c) } else { 0 };
                            let cpu = ft(k) + ft(u);
                            let mut io: IO_COUNTERS = zeroed();
                            let io_total = if GetProcessIoCounters(h, &mut io) != 0 { io.ReadTransferCount + io.WriteTransferCount } else { 0 };
                            if let Some(pr) = prev.get(&pid).filter(|pr| pr.created == created) {
                                p.cpu = ((cpu.saturating_sub(pr.cpu)) as f64 / 1e7 / elapsed / ncpu * 100.0).min(100.0) as f32;
                                p.io = io_total.saturating_sub(pr.io) as f64 / elapsed;
                            }
                            prev.insert(pid, Prev { created, cpu, io: io_total });
                            let mut m: PROCESS_MEMORY_COUNTERS_EX2 = zeroed();
                            m.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32;
                            if GetProcessMemoryInfo(h, &mut m as *mut _ as *mut _, m.cb) != 0 {
                                p.mem = if m.PrivateWorkingSetSize > 0 { m.PrivateWorkingSetSize as u64 } else { m.WorkingSetSize as u64 };
                            }
                            p.priority = GetPriorityClass(h);
                            p.efficiency = efficiency_on(h, p.priority);
                            let st = match statics.get(&pid).filter(|s| s.created == created) {
                                Some(s) => s.clone(),
                                None => {
                                    let path = image_path(h);
                                    let desc = path
                                        .as_ref()
                                        .map(|pp| descs.entry(pp.clone()).or_insert_with(|| file_description(pp).unwrap_or_default()).clone())
                                        .unwrap_or_default();
                                    let s = Static { created, path, desc, user: process_user(h) };
                                    statics.insert(pid, s.clone());
                                    s
                                }
                            };
                            p.path = st.path;
                            p.desc = st.desc;
                            p.user = st.user;
                            CloseHandle(h);
                        }
                        if pid == 4 || exe.eq_ignore_ascii_case("System") {
                            p.desc = "System".into();
                        }
                        procs.push(p);
                    }
                    ok = Process32NextW(snap, &mut e) != 0;
                }
                CloseHandle(snap);
            }
        }
        prev.retain(|pid, _| seen.contains(pid));
        statics.retain(|pid, _| seen.contains(pid));

        // ---- system CPU
        let (mut i, mut k, mut u): (FILETIME, FILETIME, FILETIME) = unsafe { (zeroed(), zeroed(), zeroed()) };
        unsafe { GetSystemTimes(&mut i, &mut k, &mut u) };
        let (di, dk, du) = (ft(i).saturating_sub(sys_idle), ft(k).saturating_sub(sys_kernel), ft(u).saturating_sub(sys_user));
        (sys_idle, sys_kernel, sys_user) = (ft(i), ft(k), ft(u));
        let mut cpu = if dk + du > 0 { ((dk + du - di) as f64 * 100.0 / (dk + du) as f64) as f32 } else { 0.0 };
        let mut cores = Vec::new();
        let mut ghz = 0.0;
        if let Some(p) = &pdh {
            let util = Pdh::array(&p.cpu_util);
            if let Some((_, v)) = util.iter().find(|(n, _)| n == "_Total") {
                cpu = (*v as f32).clamp(0.0, 100.0); // the figure Windows Task Manager shows
            }
            let mut per: Vec<(u32, u32, f32)> = util
                .iter()
                .filter_map(|(n, v)| {
                    let (g, c) = n.split_once(',')?;
                    Some((g.parse().ok()?, c.parse().ok()?, (*v as f32).clamp(0.0, 100.0)))
                })
                .collect();
            per.sort_by_key(|(g, c, _)| (*g, *c));
            cores = per.into_iter().map(|(_, _, v)| v).collect();
            let perf = Pdh::array(&p.cpu_perf).first().map(|x| x.1).unwrap_or(0.0);
            let freq = Pdh::array(&p.cpu_freq).first().map(|x| x.1).unwrap_or(0.0);
            ghz = (freq * perf / 100.0 / 1000.0) as f32;
        }
        if first {
            cpu = 0.0;
        }
        push(&mut cpu_hist, cpu);

        // ---- memory
        let mut ms: MEMORYSTATUSEX = unsafe { zeroed() };
        ms.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        unsafe { GlobalMemoryStatusEx(&mut ms) };
        let mem_used = ms.ullTotalPhys - ms.ullAvailPhys;
        push(&mut mem_hist, if ms.ullTotalPhys > 0 { mem_used as f32 * 100.0 / ms.ullTotalPhys as f32 } else { 0.0 });

        // ---- disks
        let mut disks = Vec::new();
        if let Some(p) = &pdh {
            let idle = Pdh::array(&p.disk_idle);
            let rd: HashMap<String, f64> = Pdh::array(&p.disk_read).into_iter().collect();
            let wr: HashMap<String, f64> = Pdh::array(&p.disk_write).into_iter().collect();
            for (name, v) in idle.into_iter().filter(|(n, _)| n != "_Total") {
                let active = (100.0 - v as f32).clamp(0.0, 100.0);
                let h = disk_hist.entry(name.clone()).or_default();
                push(h, active);
                disks.push(Disk { active, read: rd.get(&name).copied().unwrap_or(0.0), write: wr.get(&name).copied().unwrap_or(0.0), history: h.clone(), name });
            }
            disks.sort_by(|a, b| a.name.cmp(&b.name));
        }

        // ---- network
        let mut nets = Vec::new();
        if let Some(p) = &pdh {
            let rx = Pdh::array(&p.net_rx);
            let tx: HashMap<String, f64> = Pdh::array(&p.net_tx).into_iter().collect();
            for (name, r) in rx {
                if boring_adapter(&name) {
                    continue;
                }
                let t = tx.get(&name).copied().unwrap_or(0.0);
                let h = net_hist.entry(name.clone()).or_default();
                push(h, ((r + t) * 8.0) as f32);
                nets.push(Net { name, rx: r, tx: t, history: h.clone() });
            }
            // Busiest first; drop adapters that never carried anything.
            nets.retain(|n| n.history.iter().any(|v| *v > 0.0));
            nets.sort_by(|a, b| {
                let sa: f32 = a.history.iter().sum();
                let sb: f32 = b.history.iter().sum();
                sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
            });
            nets.truncate(3);
        }

        // ---- GPU
        let util = gpu_by_engine.values().copied().fold(0.0f32, f32::max).min(100.0);
        push(&mut gpu_hist, util);
        let gpu = (!gpu_name.is_empty() || !gpu_items.is_empty()).then(|| Gpu {
            name: gpu_name.clone(),
            util,
            temp: nvml.as_ref().and_then(|n| n.temperature()),
            history: gpu_hist.clone(),
        });

        let snap = Snapshot {
            procs,
            cpu,
            cpu_cores: cores,
            cpu_ghz: ghz,
            cpu_name: cpu_name.clone(),
            cpu_history: cpu_hist.clone(),
            mem_used,
            mem_total: ms.ullTotalPhys,
            commit_used: ms.ullTotalPageFile - ms.ullAvailPageFile,
            commit_total: ms.ullTotalPageFile,
            mem_history: mem_hist.clone(),
            disks,
            nets,
            gpu,
            uptime: Duration::from_millis(unsafe { GetTickCount64() }),
            threads: total_threads,
        };
        *out.lock().unwrap_or_else(|p| p.into_inner()) = snap;
        ctx.request_repaint();
        first = false;
        std::thread::sleep(Duration::from_millis(1000));
    }
}

// ------------------------------------------------------------------ actions

fn open(pid: u32, access: u32) -> Option<HANDLE> {
    let h = unsafe { OpenProcess(access, 0, pid) };
    (!h.is_null()).then_some(h)
}

/// End a process. Err = Windows' message (often "access denied" for admin processes).
pub fn kill(pid: u32) -> Result<(), String> {
    use windows_sys::Win32::System::Threading::TerminateProcess;
    let h = open(pid, 0x0001 /* PROCESS_TERMINATE */).ok_or_else(|| std::io::Error::last_os_error().to_string())?;
    let ok = unsafe { TerminateProcess(h, 1) };
    let err = std::io::Error::last_os_error();
    unsafe { CloseHandle(h) };
    if ok != 0 {
        Ok(())
    } else {
        Err(err.to_string())
    }
}

pub fn set_priority(pid: u32, class: u32) -> bool {
    use windows_sys::Win32::System::Threading::SetPriorityClass;
    let Some(h) = open(pid, 0x0200 /* PROCESS_SET_INFORMATION */) else { return false };
    let ok = unsafe { SetPriorityClass(h, class) } != 0;
    unsafe { CloseHandle(h) };
    ok
}

/// Efficiency mode, like Windows: EcoQoS (power throttling) + idle priority.
pub fn set_efficiency(pid: u32, on: bool) -> bool {
    use windows_sys::Win32::System::Threading::{SetPriorityClass, SetProcessInformation};
    let Some(h) = open(pid, 0x0200) else { return false };
    let st = PROCESS_POWER_THROTTLING_STATE { Version: 1, ControlMask: 1, StateMask: if on { 1 } else { 0 } };
    let ok = unsafe {
        SetProcessInformation(h, 4, &st as *const _ as *const c_void, size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32) != 0
            && SetPriorityClass(h, if on { 0x40 } else { 0x20 }) != 0
    };
    unsafe { CloseHandle(h) };
    ok
}

// ------------------------------------------------------------------ services

#[derive(Clone)]
pub struct Service {
    pub name: String,
    pub display: String,
    /// 1 stopped, 2 starting, 3 stopping, 4 running, 7 paused
    pub state: u32,
    pub pid: u32,
}

pub fn services() -> Vec<Service> {
    use windows_sys::Win32::System::Services::{CloseServiceHandle, EnumServicesStatusExW, OpenSCManagerW, ENUM_SERVICE_STATUS_PROCESSW};
    let mut out = Vec::new();
    unsafe {
        let scm = OpenSCManagerW(null(), null(), 0x0004 /* SC_MANAGER_ENUMERATE_SERVICE */);
        if scm.is_null() {
            return out;
        }
        let (mut needed, mut count, mut resume) = (0u32, 0u32, 0u32);
        EnumServicesStatusExW(scm, 0, 0x30, 3, null_mut(), 0, &mut needed, &mut count, &mut resume, null());
        let mut buf: Vec<u64> = vec![0; needed as usize / 8 + 1];
        resume = 0;
        if EnumServicesStatusExW(scm, 0, 0x30, 3, buf.as_mut_ptr() as *mut u8, (buf.len() * 8) as u32, &mut needed, &mut count, &mut resume, null()) != 0 {
            let items = buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW;
            for i in 0..count as usize {
                let it = &*items.add(i);
                out.push(Service {
                    name: pwstr(it.lpServiceName),
                    display: pwstr(it.lpDisplayName),
                    state: it.ServiceStatusProcess.dwCurrentState,
                    pid: it.ServiceStatusProcess.dwProcessId,
                });
            }
        }
        CloseServiceHandle(scm);
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Run a program with administrator rights (Windows asks first).
pub fn run_elevated(exe: &str, args: &str) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let (verb, file, params) = (wide("runas"), wide(exe), wide(args));
    let r = unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), file.as_ptr(), params.as_ptr(), null(), 0) };
    r as isize > 32
}

// ------------------------------------------------------------------ start-up apps

#[derive(Clone)]
pub struct StartupItem {
    pub name: String,
    pub command: String,
    pub enabled: bool,
    /// true = HKLM / common folder: changing it needs administrator rights.
    pub machine: bool,
    /// StartupApproved subkey: "Run", "Run32" or "StartupFolder".
    pub kind: &'static str,
}

const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\";

fn approved(machine: bool, kind: &str, name: &str) -> bool {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    let root = winreg::RegKey::predef(if machine { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER });
    root.open_subkey(format!("{APPROVED}{kind}"))
        .and_then(|k| k.get_raw_value(name))
        .map(|v| v.bytes.first().is_none_or(|b| b & 1 == 0))
        .unwrap_or(true)
}

pub fn startup_items() -> Vec<StartupItem> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    let mut out = Vec::new();
    for (machine, path, kind) in [
        (false, r"Software\Microsoft\Windows\CurrentVersion\Run", "Run"),
        (true, r"Software\Microsoft\Windows\CurrentVersion\Run", "Run"),
        (true, r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run", "Run32"),
    ] {
        let root = winreg::RegKey::predef(if machine { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER });
        if let Ok(k) = root.open_subkey(path) {
            for (name, value) in k.enum_values().flatten() {
                let command = value.to_string().trim_matches('"').to_string();
                out.push(StartupItem { enabled: approved(machine, kind, &name), name, command, machine, kind });
            }
        }
    }
    let folders = [
        (false, dirs::data_dir().map(|d| d.join(r"Microsoft\Windows\Start Menu\Programs\Startup"))),
        (true, std::env::var("ProgramData").ok().map(|p| PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs\StartUp"))),
    ];
    for (machine, dir) in folders {
        let Some(dir) = dir else { continue };
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let file = e.file_name().to_string_lossy().to_string();
            if file.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            out.push(StartupItem {
                enabled: approved(machine, "StartupFolder", &file),
                name: file,
                command: e.path().to_string_lossy().to_string(),
                machine,
                kind: "StartupFolder",
            });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Switch a start-up entry on or off the way Windows does (StartupApproved).
pub fn set_startup(item: &StartupItem, on: bool) -> Result<(), String> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_BINARY};
    let mut bytes = vec![if on { 2u8 } else { 3u8 }, 0, 0, 0];
    let now = {
        let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        (unix.as_nanos() / 100) as u64 + 116_444_736_000_000_000
    };
    bytes.extend_from_slice(&if on { 0u64 } else { now }.to_le_bytes());
    let root = winreg::RegKey::predef(if item.machine { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER });
    let path = format!("{APPROVED}{}", item.kind);
    match root.create_subkey(&path) {
        Ok((k, _)) => k
            .set_raw_value(&item.name, &winreg::RegValue { bytes, vtype: REG_BINARY })
            .map_err(|e| e.to_string()),
        Err(e) if item.machine => {
            // Machine-wide entry: ask for administrator rights and let reg.exe write it.
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            let args = format!("add \"HKLM\\{path}\" /v \"{}\" /t REG_BINARY /d {hex} /f", item.name);
            if run_elevated("reg.exe", &args) {
                Ok(())
            } else {
                Err(e.to_string())
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Bytes as "12,4 MB".
pub fn fmt_bytes(b: f64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    let s = if i == 0 || v >= 100.0 { format!("{v:.0}") } else { format!("{v:.1}") };
    format!("{} {}", s.replace('.', if crate::i18n::lang() == crate::i18n::Lang::Pl { "," } else { "." }), U[i])
}

/// Bits per second as "4,8 Mb/s".
pub fn fmt_bits(bps: f64) -> String {
    let (v, u) = if bps >= 1e9 {
        (bps / 1e9, "Gb/s")
    } else if bps >= 1e6 {
        (bps / 1e6, "Mb/s")
    } else if bps >= 1e3 {
        (bps / 1e3, "Kb/s")
    } else {
        (bps, "b/s")
    };
    let s = if v >= 100.0 { format!("{v:.0}") } else { format!("{v:.1}") };
    format!("{} {u}", s.replace('.', if crate::i18n::lang() == crate::i18n::Lang::Pl { "," } else { "." }))
}

pub fn fmt_pct(v: f32) -> String {
    let s = format!("{v:.1}%");
    if crate::i18n::lang() == crate::i18n::Lang::Pl {
        s.replace('.', ",")
    } else {
        s
    }
}

/// Open a file or console (e.g. services.msc) through the shell, which also
/// shows the administrator prompt when the program needs it.
pub fn shell_open(target: &str) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let (verb, file) = (wide("open"), wide(target));
    let r = unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), file.as_ptr(), null(), null(), 1) };
    r as isize > 32
}
