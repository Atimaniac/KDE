//! What is playing: Windows' "system media transport controls". Browsers
//! (Brave, Chrome, Edge, Firefox: YouTube Music, Spotify Web, …), Spotify and
//! most players report their track there, with cover art and position, and
//! accept play / pause / next / previous / shuffle / repeat / seek.
//!
//! A background thread polls once a second and runs the commands.

use eframe::egui;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
};
use windows::Media::MediaPlaybackAutoRepeatMode;
use windows::Storage::Streams::DataReader;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Repeat {
    #[default]
    Off,
    Track,
    List,
}

#[derive(Clone, Default)]
pub struct Track {
    pub app: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    /// Seconds, as of `stamp`.
    pub position: f64,
    pub duration: f64,
    pub stamp: Option<Instant>,
    pub can_seek: bool,
    pub can_shuffle: bool,
    pub can_repeat: bool,
    pub shuffle: bool,
    pub repeat: Repeat,
    /// Cover art (RGBA) and a number that changes with it.
    pub art: Option<Arc<(usize, usize, Vec<u8>)>>,
    pub art_gen: u64,
}

impl Track {
    /// Position now, moving on while it plays.
    pub fn position_now(&self) -> f64 {
        let mut p = self.position;
        if self.playing {
            if let Some(s) = self.stamp {
                p += s.elapsed().as_secs_f64();
            }
        }
        if self.duration > 0.0 {
            p.min(self.duration)
        } else {
            p
        }
    }
}

pub enum Cmd {
    TogglePlay,
    Next,
    Prev,
    Shuffle(bool),
    Repeat(Repeat),
    Seek(f64),
}

pub struct Media {
    pub track: Arc<Mutex<Option<Track>>>,
    tx: Sender<Cmd>,
}

impl Media {
    pub fn start(ctx: egui::Context) -> Self {
        let track = Arc::new(Mutex::new(None));
        let (tx, rx) = channel();
        let t = track.clone();
        std::thread::Builder::new()
            .name("delfin-media".into())
            .spawn(move || worker(ctx, t, rx))
            .ok();
        Self { track, tx }
    }

    pub fn current(&self) -> Option<Track> {
        self.track.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
}

/// "Brave", "Spotify", … from the app's id.
fn app_name(aumid: &str) -> String {
    let l = aumid.to_lowercase();
    for (k, v) in [
        ("brave", "Brave"),
        ("msedge", "Microsoft Edge"),
        ("chrome", "Google Chrome"),
        ("308046b0af4a39cb", "Firefox"),
        ("firefox", "Firefox"),
        ("opera", "Opera"),
        ("vivaldi", "Vivaldi"),
        ("spotify", "Spotify"),
        ("zunemusic", "Media Player"),
        ("vlc", "VLC"),
        ("foobar", "foobar2000"),
        ("aimp", "AIMP"),
        ("tidal", "TIDAL"),
        ("deezer", "Deezer"),
    ] {
        if l.contains(k) {
            return v.to_string();
        }
    }
    let s = aumid.rsplit(['!', '\\']).next().unwrap_or(aumid);
    s.trim_end_matches(".exe").trim_end_matches(".EXE").to_string()
}

/// 100-ns ticks since 1601 (WinRT DateTime) for "now".
fn now_ticks() -> i64 {
    let unix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    (unix.as_nanos() / 100) as i64 + 116_444_736_000_000_000
}

fn read_art(session: &Session) -> Option<(usize, usize, Vec<u8>)> {
    let props = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
    let reference = props.Thumbnail().ok()?;
    let stream = reference.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()? as u32;
    if size == 0 || size > 20_000_000 {
        return None;
    }
    let input = stream.GetInputStreamAt(0).ok()?;
    let reader = DataReader::CreateDataReader(&input).ok()?;
    reader.LoadAsync(size).ok()?.join().ok()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf).ok()?;
    let img = image::load_from_memory(&buf).ok()?;
    // Browsers often send a wide 16:9 frame with the cover in the middle;
    // crop to a centred square, then shrink to 256 px.
    let (w, h) = (img.width(), img.height());
    let side = w.min(h);
    let img = img.crop_imm((w - side) / 2, (h - side) / 2, side, side);
    let img = img.resize_exact(256, 256, image::imageops::FilterType::Triangle).to_rgba8();
    Some((256, 256, img.into_raw()))
}

fn snapshot(session: &Session, prev: Option<&Track>, gen: &mut u64) -> Option<Track> {
    let props = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
    let title = props.Title().ok()?.to_string();
    if title.is_empty() {
        return None;
    }
    let artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
    let album = props.AlbumTitle().map(|s| s.to_string()).unwrap_or_default();
    let info = session.GetPlaybackInfo().ok()?;
    let status = info.PlaybackStatus().ok()?.0; // 4 = playing, 5 = paused
    let controls = info.Controls().ok();
    let shuffle = info.IsShuffleActive().ok().and_then(|r| r.Value().ok()).unwrap_or(false);
    let repeat = match info.AutoRepeatMode().ok().and_then(|r| r.Value().ok()) {
        Some(MediaPlaybackAutoRepeatMode::Track) => Repeat::Track,
        Some(MediaPlaybackAutoRepeatMode::List) => Repeat::List,
        _ => Repeat::Off,
    };
    let tl = session.GetTimelineProperties().ok();
    let (mut position, duration) = tl
        .as_ref()
        .map(|t| {
            let pos = t.Position().map(|d| d.Duration).unwrap_or(0) as f64 / 1e7;
            let end = t.EndTime().map(|d| d.Duration).unwrap_or(0) as f64 / 1e7;
            (pos, end)
        })
        .unwrap_or((0.0, 0.0));
    let playing = status == 4;
    // The position is only reported now and then; count from when it was.
    if playing {
        if let Some(upd) = tl.as_ref().and_then(|t| t.LastUpdatedTime().ok()) {
            let ago = (now_ticks() - upd.UniversalTime) as f64 / 1e7;
            if (0.0..36_000.0).contains(&ago) {
                position += ago;
            }
        }
    }
    let same_song = prev.is_some_and(|p| p.title == title && p.artist == artist);
    let (art, art_gen) = match prev {
        Some(p) if same_song && p.art.is_some() => (p.art.clone(), p.art_gen),
        _ => {
            *gen += 1;
            (read_art(session).map(Arc::new), *gen)
        }
    };
    Some(Track {
        app: session.SourceAppUserModelId().map(|s| app_name(&s.to_string())).unwrap_or_default(),
        title,
        artist,
        album,
        playing,
        position,
        duration,
        stamp: Some(Instant::now()),
        can_seek: controls.as_ref().and_then(|c| c.IsPlaybackPositionEnabled().ok()).unwrap_or(false),
        can_shuffle: controls.as_ref().and_then(|c| c.IsShuffleEnabled().ok()).unwrap_or(false),
        can_repeat: controls.as_ref().and_then(|c| c.IsRepeatEnabled().ok()).unwrap_or(false),
        shuffle,
        repeat,
        art,
        art_gen,
    })
}

fn run(session: &Session, cmd: Cmd) {
    let ok = match cmd {
        Cmd::TogglePlay => session.TryTogglePlayPauseAsync().and_then(|op| op.join()),
        Cmd::Next => session.TrySkipNextAsync().and_then(|op| op.join()),
        Cmd::Prev => session.TrySkipPreviousAsync().and_then(|op| op.join()),
        Cmd::Shuffle(on) => session.TryChangeShuffleActiveAsync(on).and_then(|op| op.join()),
        Cmd::Repeat(r) => {
            let m = match r {
                Repeat::Off => MediaPlaybackAutoRepeatMode::None,
                Repeat::Track => MediaPlaybackAutoRepeatMode::Track,
                Repeat::List => MediaPlaybackAutoRepeatMode::List,
            };
            session.TryChangeAutoRepeatModeAsync(m).and_then(|op| op.join())
        }
        Cmd::Seek(s) => session.TryChangePlaybackPositionAsync((s * 1e7) as i64).and_then(|op| op.join()),
    };
    let _ = ok;
}

fn worker(ctx: egui::Context, out: Arc<Mutex<Option<Track>>>, rx: Receiver<Cmd>) {
    unsafe {
        windows_sys::Win32::System::Com::CoInitializeEx(std::ptr::null(), 0); // multithreaded
    }
    let mut manager: Option<Manager> = None;
    let mut gen = 0u64;
    loop {
        if manager.is_none() {
            manager = Manager::RequestAsync().ok().and_then(|op| op.join().ok());
        }
        let session = manager.as_ref().and_then(|m| m.GetCurrentSession().ok());
        // Commands first, so the next snapshot already shows their effect.
        let mut did = false;
        while let Ok(c) = rx.try_recv() {
            if let Some(s) = &session {
                run(s, c);
                did = true;
            }
        }
        if did {
            std::thread::sleep(Duration::from_millis(150));
        }
        let prev = out.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let now = session.as_ref().and_then(|s| snapshot(s, prev.as_ref(), &mut gen));
        let changed = match (&prev, &now) {
            (None, None) => false,
            (Some(a), Some(b)) => {
                a.title != b.title
                    || a.artist != b.artist
                    || a.playing != b.playing
                    || a.shuffle != b.shuffle
                    || a.repeat != b.repeat
                    || a.art_gen != b.art_gen
                    || (a.position_now() - b.position).abs() > 2.0
            }
            _ => true,
        };
        *out.lock().unwrap_or_else(|p| p.into_inner()) = now;
        if changed {
            ctx.request_repaint();
        }
        // Wait a second, or less if a command comes in.
        if let Ok(c) = rx.recv_timeout(Duration::from_millis(1000)) {
            if let Some(s) = &session {
                run(s, c);
                std::thread::sleep(Duration::from_millis(150));
            }
        }
    }
}

/// "3:07"
pub fn fmt_time(s: f64) -> String {
    let s = s.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}
