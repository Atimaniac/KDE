//! App notifications ("toasts"): Discord, browsers, mail, Windows Update, …
//!
//! Windows shows them through ShellExperienceHost, which doesn't run without
//! Explorer, so they were silently lost. Here they are read with
//! UserNotificationListener (allowed for desktop programs) every two seconds
//! on a background thread; the shell pops new ones up and keeps them in its
//! notification centre.

use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Icon = Arc<(usize, usize, Vec<u8>)>;

#[derive(Clone)]
pub struct Toast {
    pub id: u32,
    pub app: String,
    pub aumid: String,
    pub title: String,
    pub body: String,
    /// Seconds since 1970.
    pub time: i64,
    pub icon: Option<Icon>,
}

enum Cmd {
    Remove(u32),
    Clear,
}

#[derive(Default)]
struct State {
    /// Everything in the centre, newest first.
    all: Vec<Toast>,
    /// Arrived since the shell last asked (to pop up).
    fresh: Vec<Toast>,
    /// Reading notifications is allowed (the user can turn it off in Windows).
    allowed: bool,
}

pub struct Notifier {
    state: Arc<Mutex<State>>,
    tx: Sender<Cmd>,
}

impl Notifier {
    pub fn start(ctx: egui::Context) -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        let (tx, rx) = channel();
        let s = state.clone();
        std::thread::Builder::new()
            .name("delfin-notifications".into())
            .spawn(move || worker(ctx, s, rx))
            .ok();
        Self { state, tx }
    }

    pub fn all(&self) -> Vec<Toast> {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).all.clone()
    }

    pub fn count(&self) -> usize {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).all.len()
    }

    pub fn allowed(&self) -> bool {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).allowed
    }

    /// New notifications since the last call, oldest first.
    pub fn take_fresh(&self) -> Vec<Toast> {
        std::mem::take(&mut self.state.lock().unwrap_or_else(|p| p.into_inner()).fresh)
    }

    pub fn remove(&self, id: u32) {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).all.retain(|t| t.id != id);
        let _ = self.tx.send(Cmd::Remove(id));
    }

    pub fn clear(&self) {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).all.clear();
        let _ = self.tx.send(Cmd::Clear);
    }
}

fn worker(ctx: egui::Context, state: Arc<Mutex<State>>, rx: Receiver<Cmd>) {
    use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus};
    unsafe { windows_sys::Win32::System::Com::CoInitializeEx(std::ptr::null(), 0) };
    let Ok(listener) = UserNotificationListener::Current() else { return };
    let allowed = listener.GetAccessStatus().is_ok_and(|s| s == UserNotificationListenerAccessStatus::Allowed);
    state.lock().unwrap_or_else(|p| p.into_inner()).allowed = allowed;
    if !allowed {
        return;
    }
    let mut seen: HashSet<u32> = HashSet::new();
    let mut icons: HashMap<String, Option<Icon>> = HashMap::new();
    let mut first = true;
    loop {
        if let Some(list) = read(&listener, &mut icons) {
            let ids: HashSet<u32> = list.iter().map(|t| t.id).collect();
            let fresh: Vec<Toast> = if first { Vec::new() } else { list.iter().filter(|t| !seen.contains(&t.id)).cloned().collect() };
            let changed = ids != seen;
            seen = ids;
            first = false;
            if changed {
                let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
                let mut all = list;
                all.sort_by(|a, b| b.time.cmp(&a.time));
                st.all = all;
                let mut fresh = fresh;
                fresh.sort_by_key(|t| t.time);
                st.fresh.extend(fresh);
                drop(st);
                ctx.request_repaint();
            }
        }
        // Every two seconds, or at once after a removal.
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Cmd::Remove(id)) => {
                let _ = listener.RemoveNotification(id);
            }
            Ok(Cmd::Clear) => {
                let _ = listener.ClearNotifications();
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        while let Ok(c) = rx.try_recv() {
            match c {
                Cmd::Remove(id) => {
                    let _ = listener.RemoveNotification(id);
                }
                Cmd::Clear => {
                    let _ = listener.ClearNotifications();
                }
            }
        }
    }
}

fn read(listener: &windows::UI::Notifications::Management::UserNotificationListener, icons: &mut HashMap<String, Option<Icon>>) -> Option<Vec<Toast>> {
    use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds};
    let list = listener.GetNotificationsAsync(NotificationKinds::Toast).ok()?.join().ok()?;
    let generic = KnownNotificationBindings::ToastGeneric().ok()?;
    let mut out = Vec::new();
    for n in list {
        let Ok(id) = n.Id() else { continue };
        let info = n.AppInfo().ok();
        let aumid = info.as_ref().and_then(|i| i.AppUserModelId().ok()).map(|s| s.to_string()).unwrap_or_default();
        let display = info.as_ref().and_then(|i| i.DisplayInfo().ok());
        let app = display.as_ref().and_then(|d| d.DisplayName().ok()).map(|s| s.to_string()).unwrap_or_default();
        let texts: Vec<String> = n
            .Notification()
            .and_then(|t| t.Visual())
            .and_then(|v| v.GetBinding(&generic))
            .and_then(|b| b.GetTextElements())
            .map(|els| els.into_iter().filter_map(|e| e.Text().ok().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let title = texts.first().cloned().unwrap_or_default();
        let body = texts.iter().skip(1).cloned().collect::<Vec<_>>().join("\n");
        if title.is_empty() && body.is_empty() {
            continue;
        }
        // DateTime: 100 ns since 1601.
        let time = n.CreationTime().map(|t| t.UniversalTime / 10_000_000 - 11_644_473_600).unwrap_or(0);
        let icon = icons
            .entry(aumid.clone())
            .or_insert_with(|| display.as_ref().and_then(logo))
            .clone();
        out.push(Toast { id, app: if app.is_empty() { aumid.clone() } else { app }, aumid, title, body, time, icon });
    }
    Some(out)
}

/// The app's logo, as shown in Windows' own notifications.
fn logo(d: &windows::ApplicationModel::AppDisplayInfo) -> Option<Icon> {
    use windows::Foundation::Size;
    use windows::Storage::Streams::DataReader;
    let r = d.GetLogo(Size { Width: 48.0, Height: 48.0 }).ok()?;
    let stream = r.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()? as u32;
    if size == 0 || size > 4_000_000 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size).ok()?.join().ok()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.thumbnail(48, 48).to_rgba8();
    Some(Arc::new((img.width() as usize, img.height() as usize, img.into_raw())))
}

/// "now", "5 min", "2 h", or the date.
pub fn ago(time: i64) -> String {
    let now = chrono::Utc::now().timestamp();
    let d = (now - time).max(0);
    if d < 60 {
        crate::i18n::tr("now").to_string()
    } else if d < 3600 {
        crate::i18n::trf("{} min", &[&(d / 60)])
    } else if d < 86_400 {
        crate::i18n::trf("{} h", &[&(d / 3600)])
    } else {
        chrono::DateTime::from_timestamp(time, 0)
            .map(|t| t.with_timezone(&chrono::Local).format("%d.%m %H:%M").to_string())
            .unwrap_or_default()
    }
}
