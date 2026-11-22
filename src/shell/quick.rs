//! Quick Settings panel: volume, brightness, media keys, Wi-Fi, battery.
//! Works without Explorer (talks to the Windows APIs directly).

use super::win::wide;

fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}
use crate::theme::Palette;
use eframe::egui::{self, pos2, vec2, Color32, FontId, Rect, Sense, Shape, Stroke};
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::NetworkManagement::WiFi::*;

// ------------------------------------------------------------------ COM helpers

unsafe fn vfn<T>(obj: *mut c_void, index: usize) -> T {
    let vtbl = *(obj as *const *const usize);
    std::mem::transmute_copy(&*vtbl.add(index))
}

unsafe fn release(obj: *mut c_void) {
    if !obj.is_null() {
        let f: unsafe extern "system" fn(*mut c_void) -> u32 = vfn(obj, 2);
        f(obj);
    }
}

// ------------------------------------------------------------------ volume (Core Audio)

const CLSID_MMDEVICEENUMERATOR: GUID = GUID::from_u128(0xbcde0395_e52f_467c_8e3d_c4579291692e);
const IID_IMMDEVICEENUMERATOR: GUID = GUID::from_u128(0xa95664d2_9614_4f35_a746_de8db63617e6);
const IID_IAUDIOENDPOINTVOLUME: GUID = GUID::from_u128(0x5cdf2c82_841e_4546_9722_0cf74078229a);

/// The default speakers' volume control (IAudioEndpointVolume).
pub struct Volume(*mut c_void);

impl Volume {
    pub fn open() -> Option<Self> {
        unsafe {
            use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx};
            CoInitializeEx(null(), 0x2);
            let mut en: *mut c_void = null_mut();
            if CoCreateInstance(&CLSID_MMDEVICEENUMERATOR, null_mut(), 23, &IID_IMMDEVICEENUMERATOR, &mut en) < 0 || en.is_null() {
                return None;
            }
            // IMMDeviceEnumerator::GetDefaultAudioEndpoint(eRender, eMultimedia)
            let get_default: unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> i32 = vfn(en, 4);
            let mut dev: *mut c_void = null_mut();
            let hr = get_default(en, 0, 1, &mut dev);
            release(en);
            if hr < 0 || dev.is_null() {
                return None;
            }
            // IMMDevice::Activate
            let activate: unsafe extern "system" fn(*mut c_void, *const GUID, u32, *const c_void, *mut *mut c_void) -> i32 = vfn(dev, 3);
            let mut vol: *mut c_void = null_mut();
            let hr = activate(dev, &IID_IAUDIOENDPOINTVOLUME, 23, null(), &mut vol);
            release(dev);
            (hr >= 0 && !vol.is_null()).then_some(Volume(vol))
        }
    }

    /// 0.0 ..= 1.0
    pub fn level(&self) -> f32 {
        let mut v = 0.0f32;
        unsafe {
            let f: unsafe extern "system" fn(*mut c_void, *mut f32) -> i32 = vfn(self.0, 9);
            f(self.0, &mut v);
        }
        v
    }

    pub fn set_level(&self, v: f32) {
        unsafe {
            let f: unsafe extern "system" fn(*mut c_void, f32, *const GUID) -> i32 = vfn(self.0, 7);
            f(self.0, v.clamp(0.0, 1.0), null());
        }
    }

    pub fn muted(&self) -> bool {
        let mut m = 0i32;
        unsafe {
            let f: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32 = vfn(self.0, 15);
            f(self.0, &mut m);
        }
        m != 0
    }

    pub fn set_muted(&self, m: bool) {
        unsafe {
            let f: unsafe extern "system" fn(*mut c_void, i32, *const GUID) -> i32 = vfn(self.0, 14);
            f(self.0, m as i32, null());
        }
    }
}

impl Drop for Volume {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}

// ------------------------------------------------------------------ media keys, battery

pub fn media_key(vk: u8) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP};
    unsafe {
        keybd_event(vk, 0, KEYEVENTF_EXTENDEDKEY, 0);
        keybd_event(vk, 0, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP, 0);
    }
}

const VK_MEDIA_NEXT: u8 = 0xB0;
const VK_MEDIA_PREV: u8 = 0xB1;
const VK_MEDIA_PLAY_PAUSE: u8 = 0xB3;

/// (percent, charging) or None on a desktop PC.
pub fn battery() -> Option<(u8, bool)> {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut s: SYSTEM_POWER_STATUS = unsafe { zeroed() };
    if unsafe { GetSystemPowerStatus(&mut s) } == 0 || s.BatteryFlag & 128 != 0 || s.BatteryFlag == 255 || s.BatteryLifePercent > 100 {
        return None;
    }
    Some((s.BatteryLifePercent, s.ACLineStatus == 1))
}

// ------------------------------------------------------------------ brightness

/// Monitor brightness, read and set on a background thread because the
/// monitor (DDC/CI) or WMI can take a while to answer.
pub struct Brightness {
    tx: Sender<u32>,
    pub value: Arc<Mutex<Option<u32>>>,
}

enum BrightMode {
    Ddc(Vec<HANDLE>),
    Wmi,
}

impl Brightness {
    pub fn start(ctx: egui::Context) -> Self {
        let (tx, rx) = channel::<u32>();
        let value = Arc::new(Mutex::new(None));
        let v = value.clone();
        std::thread::spawn(move || brightness_thread(ctx, rx, v));
        Self { tx, value }
    }

    pub fn set(&self, percent: u32) {
        *self.value.lock().unwrap() = Some(percent);
        let _ = self.tx.send(percent);
    }
}

fn brightness_thread(ctx: egui::Context, rx: Receiver<u32>, value: Arc<Mutex<Option<u32>>>) {
    let mut mode = None;
    // External monitors: DDC/CI.
    unsafe {
        use windows_sys::Win32::Devices::Display::*;
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut n = 0u32;
        if GetNumberOfPhysicalMonitorsFromHMONITOR(mon, &mut n) != 0 && n > 0 {
            let mut list: Vec<PHYSICAL_MONITOR> = vec![zeroed(); n as usize];
            if GetPhysicalMonitorsFromHMONITOR(mon, n, list.as_mut_ptr()) != 0 {
                let (mut lo, mut cur, mut hi) = (0u32, 0u32, 0u32);
                let h = list[0].hPhysicalMonitor;
                if GetMonitorBrightness(h, &mut lo, &mut cur, &mut hi) != 0 && hi > lo {
                    *value.lock().unwrap() = Some((cur - lo) * 100 / (hi - lo));
                    mode = Some(BrightMode::Ddc(list.iter().map(|m| m.hPhysicalMonitor).collect()));
                }
            }
        }
    }
    // Laptop screens: WMI.
    if mode.is_none() {
        if let Some(v) = powershell("(Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightness -ErrorAction Stop | Select-Object -First 1).CurrentBrightness") {
            if let Ok(p) = v.trim().parse::<u32>() {
                *value.lock().unwrap() = Some(p.min(100));
                mode = Some(BrightMode::Wmi);
            }
        }
    }
    ctx.request_repaint();
    let Some(mode) = mode else { return };
    while let Ok(mut p) = rx.recv() {
        // Only the latest value matters while the slider is being dragged.
        while let Ok(newer) = rx.try_recv() {
            p = newer;
        }
        match &mode {
            BrightMode::Ddc(handles) => unsafe {
                use windows_sys::Win32::Devices::Display::{GetMonitorBrightness, SetMonitorBrightness};
                for &h in handles {
                    let (mut lo, mut cur, mut hi) = (0u32, 0u32, 0u32);
                    if GetMonitorBrightness(h, &mut lo, &mut cur, &mut hi) != 0 && hi > lo {
                        SetMonitorBrightness(h, lo + (hi - lo) * p / 100);
                    }
                }
            },
            BrightMode::Wmi => {
                powershell(&format!(
                    "Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightnessMethods | Invoke-CimMethod -MethodName WmiSetBrightness -Arguments @{{Timeout=1;Brightness={p}}} | Out-Null"
                ));
            }
        }
    }
}

fn powershell(cmd: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", cmd])
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

// ------------------------------------------------------------------ Wi-Fi (WLAN API)

#[derive(Clone, Debug)]
pub struct Net {
    pub ssid: String,
    pub profile: String,
    pub signal: u32,
    pub secured: bool,
    pub connected: bool,
    pub auth: i32,
    pub cipher: i32,
}

impl Net {
    /// Profile <authentication>/<encryption> for networks we can join with a password.
    fn security(&self) -> Option<(&'static str, &'static str)> {
        if !self.secured {
            return Some(("open", "none"));
        }
        let enc = if self.cipher == 2 { "TKIP" } else { "AES" };
        match self.auth {
            4 => Some(("WPAPSK", enc)),
            7 => Some(("WPA2PSK", enc)),
            9 => Some(("WPA3SAE", "AES")),
            _ => None, // enterprise / WEP: leave it to Windows Settings
        }
    }
}

#[derive(Default)]
pub struct WifiState {
    pub present: bool,
    pub radio_on: bool,
    pub networks: Vec<Net>,
    pub status: String,
}

enum WifiCmd {
    Scan,
    Connect { net: Net, password: Option<String> },
    Disconnect,
    Radio(bool),
}

pub struct Wifi {
    tx: Sender<WifiCmd>,
    pub state: Arc<Mutex<WifiState>>,
    active: Arc<AtomicBool>,
}

impl Wifi {
    pub fn start(ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        let state = Arc::new(Mutex::new(WifiState::default()));
        let active = Arc::new(AtomicBool::new(false));
        let (s, a) = (state.clone(), active.clone());
        std::thread::Builder::new()
            .name("delfin-wifi".into())
            .spawn(move || unsafe { wifi_thread(ctx, rx, s, a) })
            .ok();
        Self { tx, state, active }
    }

    /// The panel is open: refresh often and scan now.
    pub fn set_active(&self, on: bool) {
        self.active.store(on, Ordering::Relaxed);
        if on {
            let _ = self.tx.send(WifiCmd::Scan);
        }
    }

    pub fn scan(&self) {
        let _ = self.tx.send(WifiCmd::Scan);
    }

    pub fn connect(&self, net: Net, password: Option<String>) {
        let _ = self.tx.send(WifiCmd::Connect { net, password });
    }

    pub fn disconnect(&self) {
        let _ = self.tx.send(WifiCmd::Disconnect);
    }

    pub fn set_radio(&self, on: bool) {
        let _ = self.tx.send(WifiCmd::Radio(on));
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

fn profile_xml(net: &Net, password: Option<&str>) -> Option<String> {
    let (auth, enc) = net.security()?;
    let hex: String = net.ssid.bytes().map(|b| format!("{b:02X}")).collect();
    let name = xml_escape(&net.ssid);
    let key = match password {
        Some(p) if auth != "open" => format!(
            "<sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>{}</keyMaterial></sharedKey>",
            xml_escape(p)
        ),
        _ => String::new(),
    };
    Some(format!(
        "<?xml version=\"1.0\"?>\
<WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\">\
<name>{name}</name>\
<SSIDConfig><SSID><hex>{hex}</hex><name>{name}</name></SSID></SSIDConfig>\
<connectionType>ESS</connectionType><connectionMode>auto</connectionMode>\
<MSM><security><authEncryption><authentication>{auth}</authentication><encryption>{enc}</encryption><useOneX>false</useOneX></authEncryption>{key}</security></MSM>\
</WLANProfile>"
    ))
}

unsafe fn wifi_thread(ctx: egui::Context, rx: Receiver<WifiCmd>, state: Arc<Mutex<WifiState>>, active: Arc<AtomicBool>) {
    let mut ver = 0u32;
    let mut h: HANDLE = null_mut();
    if WlanOpenHandle(2, null(), &mut ver, &mut h) != 0 {
        return;
    }
    let mut pending: Option<(String, Instant)> = None;
    let mut first_round = true;
    loop {
        // Find the (first) wireless adapter each round: it can come and go.
        let mut ifs: *mut WLAN_INTERFACE_INFO_LIST = null_mut();
        let guid = if WlanEnumInterfaces(h, null(), &mut ifs) == 0 && !ifs.is_null() && (*ifs).dwNumberOfItems > 0 {
            let g = (*ifs).InterfaceInfo[0].InterfaceGuid;
            WlanFreeMemory(ifs as *const c_void);
            Some(g)
        } else {
            if !ifs.is_null() {
                WlanFreeMemory(ifs as *const c_void);
            }
            None
        };

        let mut status: Option<String> = None;
        let timeout = if active.load(Ordering::Relaxed) || pending.is_some() { 2 } else { 15 };
        let cmd = if std::mem::take(&mut first_round) {
            None // read the state right away at start-up
        } else {
            match rx.recv_timeout(Duration::from_secs(timeout)) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        };
        if let (Some(g), Some(cmd)) = (guid, cmd) {
            match cmd {
                WifiCmd::Scan => {
                    WlanScan(h, &g, null(), null(), null());
                    // Results arrive within a few seconds; show what we have meanwhile.
                    let c = ctx.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs(4));
                        c.request_repaint();
                    });
                }
                WifiCmd::Disconnect => {
                    WlanDisconnect(h, &g, null());
                }
                WifiCmd::Radio(on) => {
                    let st = WLAN_PHY_RADIO_STATE {
                        dwPhyIndex: 0,
                        dot11SoftwareRadioState: if on { 1 } else { 2 },
                        dot11HardwareRadioState: if on { 1 } else { 2 },
                    };
                    let r = WlanSetInterface(h, &g, 4, size_of::<WLAN_PHY_RADIO_STATE>() as u32, &st as *const _ as *const c_void, null());
                    if r != 0 {
                        status = Some(crate::i18n::tr("Windows didn't allow switching Wi-Fi (try Settings).").into());
                    }
                }
                WifiCmd::Connect { net, password } => {
                    let mut profile = net.profile.clone();
                    let mut ok = true;
                    if profile.is_empty() || password.is_some() {
                        match profile_xml(&net, password.as_deref()) {
                            Some(xml) => {
                                let x = wide(&xml);
                                let mut reason = 0u32;
                                if WlanSetProfile(h, &g, 0, x.as_ptr(), null(), 1, null(), &mut reason) == 0 {
                                    profile = net.ssid.clone();
                                } else {
                                    ok = false;
                                    status = Some(crate::i18n::trf("Couldn't save the settings for {}.", &[&net.ssid]));
                                }
                            }
                            None => {
                                ok = false;
                                status = Some(crate::i18n::trf("{} needs Windows Settings to connect.", &[&net.ssid]));
                            }
                        }
                    }
                    if ok {
                        let p = wide(&profile);
                        let params = WLAN_CONNECTION_PARAMETERS {
                            wlanConnectionMode: 0, // wlan_connection_mode_profile
                            strProfile: p.as_ptr(),
                            pDot11Ssid: null_mut(),
                            pDesiredBssidList: null_mut(),
                            dot11BssType: 1, // infrastructure
                            dwFlags: 0,
                        };
                        if WlanConnect(h, &g, &params, null()) == 0 {
                            status = Some(crate::i18n::trf("Connecting to {}…", &[&net.ssid]));
                            pending = Some((net.ssid.clone(), Instant::now()));
                        } else {
                            status = Some(crate::i18n::trf("Couldn't connect to {}.", &[&net.ssid]));
                        }
                    }
                }
            }
        }

        // Refresh the network list and radio state.
        let mut networks: Vec<Net> = Vec::new();
        let mut radio_on = false;
        if let Some(g) = guid {
            let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = null_mut();
            if WlanGetAvailableNetworkList(h, &g, 0, null(), &mut list) == 0 && !list.is_null() {
                let n = (*list).dwNumberOfItems as usize;
                let items = std::slice::from_raw_parts((*list).Network.as_ptr(), n);
                for it in items {
                    let len = (it.dot11Ssid.uSSIDLength as usize).min(32);
                    let ssid = String::from_utf8_lossy(&it.dot11Ssid.ucSSID[..len]).to_string();
                    if ssid.is_empty() {
                        continue; // hidden network
                    }
                    let net = Net {
                        ssid,
                        profile: from_wide(&it.strProfileName),
                        signal: it.wlanSignalQuality,
                        secured: it.bSecurityEnabled != 0,
                        connected: it.dwFlags & 1 != 0,
                        auth: it.dot11DefaultAuthAlgorithm,
                        cipher: it.dot11DefaultCipherAlgorithm,
                    };
                    // The same network can be listed with and without a saved profile.
                    if let Some(old) = networks.iter_mut().find(|x| x.ssid == net.ssid) {
                        if net.connected || (old.profile.is_empty() && !net.profile.is_empty()) {
                            *old = Net { signal: old.signal.max(net.signal), ..net };
                        }
                    } else {
                        networks.push(net);
                    }
                }
                WlanFreeMemory(list as *const c_void);
            }
            let mut size = 0u32;
            let mut data: *mut c_void = null_mut();
            if WlanQueryInterface(h, &g, 4, null(), &mut size, &mut data, null_mut()) == 0 && !data.is_null() {
                let rs = &*(data as *const WLAN_RADIO_STATE);
                radio_on = (0..rs.dwNumberOfPhys.min(64) as usize)
                    .any(|i| rs.PhyRadioState[i].dot11SoftwareRadioState == 1 && rs.PhyRadioState[i].dot11HardwareRadioState == 1);
                WlanFreeMemory(data);
            }
        }
        networks.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.signal.cmp(&a.signal)));

        if let Some((ssid, t)) = &pending {
            if networks.iter().any(|n| n.connected && n.ssid == *ssid) {
                status = Some(String::new());
                pending = None;
            } else if t.elapsed() > Duration::from_secs(20) {
                status = Some(crate::i18n::trf("Couldn't connect to {}. Check the password.", &[ssid]));
                pending = None;
            }
        }

        {
            let mut s = state.lock().unwrap();
            s.present = guid.is_some();
            s.radio_on = radio_on;
            s.networks = networks;
            if let Some(st) = status {
                s.status = st;
            }
        }
        ctx.request_repaint();
    }
}

// ------------------------------------------------------------------ the panel

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    /// Volume, brightness, media keys, battery.
    Sound,
    /// Wi-Fi on/off and the list of networks.
    Network,
}

impl Section {
    /// Panel height in points.
    pub fn height(self) -> f32 {
        match self {
            Section::Sound => 250.0,
            Section::Network => 480.0,
        }
    }
}

pub enum QuickAction {
    Open(&'static str),
    Close,
}

pub struct QuickPanel {
    pub wifi: Wifi,
    pub bright: Brightness,
    vol: Option<Volume>,
    expanded: Option<String>,
    password: String,
}

impl QuickPanel {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            wifi: Wifi::start(ctx.clone()),
            bright: Brightness::start(ctx.clone()),
            vol: Volume::open(),
            expanded: None,
            password: String::new(),
        }
    }

    pub fn on_open(&mut self) {
        self.vol = Volume::open(); // the default device may have changed
        self.wifi.set_active(true);
        self.expanded = None;
        self.password.clear();
    }

    pub fn on_close(&mut self) {
        self.wifi.set_active(false);
        self.password.clear();
    }

    /// (level 0..1, muted) for the taskbar icon.
    pub fn volume_state(&self) -> Option<(f32, bool)> {
        self.vol.as_ref().map(|v| (v.level(), v.muted()))
    }

    pub fn refresh_volume(&mut self) {
        self.vol = Volume::open();
    }

    /// Signal of the connected Wi-Fi network, for the taskbar icon.
    /// Name of the connected Wi-Fi network, for the tooltip.
    pub fn connected_ssid(&self) -> Option<String> {
        let s = self.wifi.state.lock().unwrap();
        s.networks.iter().find(|n| n.connected).map(|n| n.ssid.clone())
    }

    pub fn wifi_signal(&self) -> Option<u32> {
        let s = self.wifi.state.lock().unwrap();
        s.networks.iter().find(|n| n.connected).map(|n| n.signal)
    }

    pub fn toggle_mute(&self) {
        if let Some(v) = &self.vol {
            v.set_muted(!v.muted());
        }
    }

    /// 0.0 ..= 1.0; also unmutes.
    pub fn set_volume(&self, level: f32) {
        if let Some(v) = &self.vol {
            v.set_level(level);
            if v.muted() {
                v.set_muted(false);
            }
        }
    }

    /// Change the volume by `delta` (0..1 scale), e.g. from the mouse wheel.
    pub fn nudge_volume(&self, delta: f32) {
        if let Some(v) = &self.vol {
            v.set_level(v.level() + delta);
            if v.muted() && delta > 0.0 {
                v.set_muted(false);
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, pal: &Palette, section: Section) -> Option<QuickAction> {
        let mut action = None;
        ui.spacing_mut().slider_width = ui.available_width() - 90.0;
        ui.add_space(4.0);
        if section == Section::Sound {
            self.sound_ui(ui, pal);
        } else {
            self.network_ui(ui, pal, &mut action);
        }

        // Footer
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let links: &[(&str, &'static str)] = match section {
                    Section::Sound => &[(crate::i18n::tr("Sound"), "ms-settings:sound"), (crate::i18n::tr("Volume Mixer"), "ms-settings:apps-volume")],
                    Section::Network => &[(crate::i18n::tr("Network"), "ms-settings:network"), (crate::i18n::tr("Bluetooth"), "ms-settings:bluetooth")],
                };
                for &(label, uri) in links {
                    if ui.button(label).clicked() {
                        action = Some(QuickAction::Open(uri));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(crate::i18n::tr("All Settings")).clicked() {
                        action = Some(QuickAction::Open("ms-settings:"));
                    }
                });
            });
            ui.separator();
        });

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            action = Some(QuickAction::Close);
        }
        action
    }

    fn sound_ui(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        // Volume
        if let Some(v) = &self.vol {
            let muted = v.muted();
            let mut level = (v.level() * 100.0).round();
            ui.horizontal(|ui| {
                let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, pal.hover);
                }
                paint_speaker(ui.painter(), r, pal.text, if muted { None } else { Some(level / 100.0) });
                if resp.on_hover_text(if muted { crate::i18n::tr("Unmute") } else { crate::i18n::tr("Mute") }).clicked() {
                    v.set_muted(!muted);
                }
                if ui.add(egui::Slider::new(&mut level, 0.0..=100.0).show_value(false)).changed() {
                    v.set_level(level / 100.0);
                    if muted {
                        v.set_muted(false);
                    }
                }
                ui.label(format!("{level:.0}%"));
            });
        }

        // Brightness
        let bright = *self.bright.value.lock().unwrap();
        if let Some(b) = bright {
            let mut level = b as f32;
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                paint_sun(ui.painter(), r, pal.text);
                if ui.add(egui::Slider::new(&mut level, 0.0..=100.0).show_value(false)).changed() {
                    self.bright.set(level.round() as u32);
                }
                ui.label(format!("{level:.0}%"));
            });
        }

        // Media keys
        ui.horizontal(|ui| {
            let w = 3.0 * 44.0 + 2.0 * 8.0;
            ui.add_space((ui.available_width() - w) / 2.0);
            for (k, vk) in [(0, VK_MEDIA_PREV), (1, VK_MEDIA_PLAY_PAUSE), (2, VK_MEDIA_NEXT)] {
                let (r, resp) = ui.allocate_exact_size(vec2(44.0, 32.0), Sense::click());
                ui.painter().rect_filled(r, 4.0, if resp.hovered() { pal.hover } else { pal.header });
                paint_media(ui.painter(), r, k, pal.text);
                let tip = [crate::i18n::tr("Previous"), crate::i18n::tr("Play / Pause"), crate::i18n::tr("Next")][k];
                if resp.on_hover_text(tip).clicked() {
                    media_key(vk);
                }
            }
        });

        // Battery (laptops)
        if let Some((pct, charging)) = battery() {
            ui.add_space(6.0);
            ui.label(crate::i18n::trf("Battery {}%{}", &[&pct, &if charging { crate::i18n::tr(" (charging)") } else { "" }]));
        }
    }

    pub(crate) fn network_ui(&mut self, ui: &mut egui::Ui, pal: &Palette, action: &mut Option<QuickAction>) {
        // Wi-Fi
        let (present, radio_on, nets, status) = {
            let s = self.wifi.state.lock().unwrap();
            (s.present, s.radio_on, s.networks.clone(), s.status.clone())
        };
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Wi-Fi").strong().size(14.5));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if present {
                    let mut on = radio_on;
                    if toggle(ui, &mut on, pal).changed() {
                        self.wifi.set_radio(on);
                    }
                    if radio_on && ui.small_button("⟳").on_hover_text(crate::i18n::tr("Scan again")).clicked() {
                        self.wifi.scan();
                    }
                }
            });
        });
        if !present {
            ui.label(egui::RichText::new(crate::i18n::tr("No Wi-Fi adapter found.")).color(pal.dim));
        } else if !radio_on {
            ui.label(egui::RichText::new(crate::i18n::tr("Wi-Fi is off.")).color(pal.dim));
        } else {
            egui::ScrollArea::vertical()
                .id_salt("wifi-list")
                .max_height(330.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if nets.is_empty() {
                        ui.label(egui::RichText::new(crate::i18n::tr("Looking for networks…")).color(pal.dim));
                    }
                    for net in &nets {
                        let open = self.expanded.as_deref() == Some(net.ssid.as_str());
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
                        if open || net.connected {
                            ui.painter().rect_filled(r, 4.0, if open { pal.hover } else { pal.alt_row });
                        } else if resp.hovered() {
                            ui.painter().rect_filled(r, 4.0, pal.hover);
                        }
                        let ir = Rect::from_min_size(pos2(r.left() + 6.0, r.center().y - 12.0), vec2(24.0, 24.0));
                        paint_wifi(ui.painter(), ir, pal.text, pal.border, Some(net.signal));
                        if net.secured {
                            paint_lock(ui.painter(), Rect::from_min_size(ir.right_bottom() - vec2(9.0, 9.0), vec2(9.0, 9.0)), pal.text);
                        }
                        let tx = ir.right() + 10.0;
                        ui.painter().text(pos2(tx, r.top() + 5.0), egui::Align2::LEFT_TOP, &net.ssid, FontId::proportional(13.5), pal.text);
                        let sub = match (net.connected, net.secured) {
                            (true, true) => crate::i18n::tr("Connected, secured"),
                            (true, false) => crate::i18n::tr("Connected, open"),
                            (false, true) => crate::i18n::tr("Secured"),
                            (false, false) => crate::i18n::tr("Open"),
                        };
                        ui.painter().text(pos2(tx, r.top() + 23.0), egui::Align2::LEFT_TOP, sub, FontId::proportional(11.0), pal.dim);
                        if resp.clicked() {
                            self.expanded = if open { None } else { Some(net.ssid.clone()) };
                            self.password.clear();
                        }
                        if open {
                            ui.add_space(4.0);
                            if net.connected {
                                if ui.button(crate::i18n::tr("Disconnect")).clicked() {
                                    self.wifi.disconnect();
                                    self.expanded = None;
                                }
                            } else if !net.profile.is_empty() || !net.secured {
                                if ui.button(crate::i18n::tr("Connect")).clicked() {
                                    self.wifi.connect(net.clone(), None);
                                }
                            } else if net.security().is_none() {
                                ui.label(egui::RichText::new(crate::i18n::tr("This network needs Windows Settings.")).color(pal.dim));
                                if ui.button(crate::i18n::tr("Open Wi-Fi Settings")).clicked() {
                                    *action = Some(QuickAction::Open("ms-settings:network-wifi"));
                                }
                            } else {
                                let mut go = false;
                                ui.horizontal(|ui| {
                                    let te = ui.add(
                                        egui::TextEdit::singleline(&mut self.password)
                                            .password(true)
                                            .hint_text(crate::i18n::tr("Password"))
                                            .desired_width(ui.available_width() - 90.0),
                                    );
                                    go |= te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                    go |= ui.add_enabled(self.password.len() >= 8, egui::Button::new(crate::i18n::tr("Connect"))).clicked();
                                });
                                if go && self.password.len() >= 8 {
                                    self.wifi.connect(net.clone(), Some(std::mem::take(&mut self.password)));
                                }
                            }
                            ui.add_space(6.0);
                        }
                    }
                });
        }
        if !status.is_empty() {
            ui.label(egui::RichText::new(status).color(pal.dim).size(12.0));
        }
    }
}

// ------------------------------------------------------------------ widgets & glyphs

pub(crate) fn toggle(ui: &mut egui::Ui, on: &mut bool, pal: &Palette) -> egui::Response {
    let (r, mut resp) = ui.allocate_exact_size(vec2(40.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let bg = if *on { pal.accent } else { pal.border };
    ui.painter().rect_filled(r, 10.0, bg);
    let x = egui::lerp(r.left() + 10.0..=r.right() - 10.0, t);
    ui.painter().circle_filled(pos2(x, r.center().y), 7.0, Color32::WHITE);
    resp
}

/// Speaker with 0–3 sound waves; `None` = muted (crossed out).
pub fn paint_speaker(p: &egui::Painter, r: Rect, col: Color32, level: Option<f32>) {
    let c = r.center() - vec2(r.height() * 0.1, 0.0);
    let s = r.height() * 0.2;
    p.add(Shape::convex_polygon(
        vec![
            c + vec2(-s * 1.6, -s * 0.6),
            c + vec2(-s * 0.8, -s * 0.6),
            c + vec2(0.0, -s * 1.4),
            c + vec2(0.0, s * 1.4),
            c + vec2(-s * 0.8, s * 0.6),
            c + vec2(-s * 1.6, s * 0.6),
        ],
        col,
        Stroke::NONE,
    ));
    match level {
        None => {
            let st = Stroke::new(1.6_f32, col);
            let x = c + vec2(s * 1.4, 0.0);
            p.line_segment([x + vec2(-s * 0.6, -s * 0.6), x + vec2(s * 0.6, s * 0.6)], st);
            p.line_segment([x + vec2(-s * 0.6, s * 0.6), x + vec2(s * 0.6, -s * 0.6)], st);
        }
        Some(l) => {
            let waves = if l <= 0.001 { 0 } else if l < 0.34 { 1 } else if l < 0.67 { 2 } else { 3 };
            for k in 0..waves {
                let rad = s * (0.9 + 0.75 * k as f32);
                let pts: Vec<_> = (0..=10)
                    .map(|i| {
                        let a = -0.8 + i as f32 / 10.0 * 1.6;
                        c + vec2(s * 0.3, 0.0) + vec2(a.cos(), a.sin()) * rad
                    })
                    .collect();
                p.add(Shape::line(pts, Stroke::new(1.5_f32, col)));
            }
        }
    }
}

/// Wi-Fi fan: arcs lit up to the signal strength; `None` = not connected.
pub fn paint_wifi(p: &egui::Painter, r: Rect, col: Color32, dim: Color32, signal: Option<u32>) {
    let base = r.center() + vec2(0.0, r.height() * 0.28);
    let s = r.height() * 0.2;
    let bars = match signal {
        None => 0,
        Some(q) if q >= 75 => 3,
        Some(q) if q >= 45 => 2,
        Some(q) if q >= 15 => 1,
        Some(_) => 0,
    };
    p.circle_filled(base, 1.8, if signal.is_some() { col } else { dim });
    for k in 0..3 {
        let pts: Vec<_> = (0..=12)
            .map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 - 0.75 + i as f32 / 12.0 * 1.5;
                base + vec2(a.cos(), a.sin()) * s * (1.0 + 0.8 * k as f32)
            })
            .collect();
        p.add(Shape::line(pts, Stroke::new(1.6_f32, if k < bars { col } else { dim })));
    }
}

fn paint_lock(p: &egui::Painter, r: Rect, col: Color32) {
    let body = Rect::from_min_max(pos2(r.left(), r.center().y), r.max);
    p.rect_filled(body, 1.0, col);
    p.circle_stroke(pos2(r.center().x, r.center().y), r.width() * 0.3, Stroke::new(1.2_f32, col));
}

fn paint_sun(p: &egui::Painter, r: Rect, col: Color32) {
    let c = r.center();
    let s = r.height() * 0.16;
    p.circle_filled(c, s, col);
    for i in 0..8 {
        let a = i as f32 * std::f32::consts::TAU / 8.0;
        let d = vec2(a.cos(), a.sin());
        p.line_segment([c + d * s * 1.6, c + d * s * 2.4], Stroke::new(1.5_f32, col));
    }
}

fn paint_media(p: &egui::Painter, r: Rect, which: usize, col: Color32) {
    let c = r.center();
    let s = 6.0;
    let tri = |dir: f32, off: f32| {
        Shape::convex_polygon(
            vec![c + vec2(off - dir * s, -s), c + vec2(off + dir * s, 0.0), c + vec2(off - dir * s, s)],
            col,
            Stroke::NONE,
        )
    };
    match which {
        0 => {
            p.add(tri(-1.0, -2.0));
            p.rect_filled(Rect::from_center_size(c + vec2(-s - 4.0, 0.0), vec2(2.0, 2.0 * s)), 0.0, col);
        }
        1 => {
            p.add(tri(1.0, -4.0));
            p.rect_filled(Rect::from_center_size(c + vec2(5.0, 0.0), vec2(2.5, 2.0 * s)), 0.0, col);
            p.rect_filled(Rect::from_center_size(c + vec2(9.0, 0.0), vec2(2.5, 2.0 * s)), 0.0, col);
        }
        _ => {
            p.add(tri(1.0, 2.0));
            p.rect_filled(Rect::from_center_size(c + vec2(s + 4.0, 0.0), vec2(2.0, 2.0 * s)), 0.0, col);
        }
    }
}

/// Is a wired (Ethernet) adapter up and routed (it has a gateway)?
pub fn wired_connected() -> bool {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH};
    const GAA_FLAG_INCLUDE_GATEWAYS: u32 = 0x80;
    const SKIP: u32 = 0x2 | 0x4 | 0x8; // no anycast, multicast, DNS servers
    unsafe {
        let mut size = 16 * 1024u32;
        let mut buf: Vec<u64> = vec![0; size as usize / 8];
        let mut r = GetAdaptersAddresses(0, GAA_FLAG_INCLUDE_GATEWAYS | SKIP, null(), buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH, &mut size);
        if r == 111 {
            // ERROR_BUFFER_OVERFLOW: retry with the size Windows asked for
            buf = vec![0; size as usize / 8 + 1];
            r = GetAdaptersAddresses(0, GAA_FLAG_INCLUDE_GATEWAYS | SKIP, null(), buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH, &mut size);
        }
        if r != 0 {
            return false;
        }
        let mut a = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !a.is_null() {
            let ad = &*a;
            // 6 = Ethernet, OperStatus 1 = up
            if ad.IfType == 6 && ad.OperStatus == 1 && !ad.FirstGatewayAddress.is_null() {
                return true;
            }
            a = ad.Next;
        }
    }
    false
}

/// Network icon: a monitor; with Wi-Fi a blue fan in the corner shows the
/// signal (`wifi` = 0..=100); grey with a red cross when offline.
pub fn paint_network(p: &egui::Painter, r: Rect, pal: &Palette, wifi: Option<u32>, wired: bool) {
    let online = wired || wifi.is_some();
    let col = if online { pal.text } else { pal.border };
    let s = r.height() / 28.0; // drawn on a 28-unit grid
    let at = |x: f32, y: f32| pos2(r.left() + (r.width() - 28.0 * s) / 2.0 + x * s, r.top() + y * s);
    let st = Stroke::new(1.8 * s, col);
    // Screen and stand.
    p.rect_stroke(Rect::from_min_max(at(3.0, 4.0), at(23.0, 18.0)), 2.0 * s, st);
    p.line_segment([at(13.0, 18.0), at(13.0, 22.0)], st);
    p.line_segment([at(10.0, 22.0), at(16.0, 22.0)], st);
    if !online {
        let x = Stroke::new(2.2 * s, pal.danger);
        p.line_segment([at(18.0, 18.0), at(26.0, 26.0)], x);
        p.line_segment([at(26.0, 18.0), at(18.0, 26.0)], x);
        return;
    }
    if let Some(q) = wifi {
        // Fan from the tip at (22, 26), full radius 8: grey track, blue level.
        let wedge = |radius: f32, fill: Color32| {
            let tip = at(22.0, 26.0);
            let mut pts = vec![tip];
            for i in 0..=12 {
                let a = std::f32::consts::PI * (1.25 + 0.5 * i as f32 / 12.0);
                pts.push(tip + vec2(a.cos(), a.sin()) * radius * s);
            }
            p.add(Shape::convex_polygon(pts, fill, Stroke::NONE));
        };
        let level = if q >= 70 { 1.0 } else if q >= 40 { 0.72 } else { 0.48 };
        wedge(8.0, pal.border);
        wedge(8.0 * level, pal.accent);
    }
}
