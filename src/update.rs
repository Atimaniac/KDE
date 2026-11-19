//! Over-the-air updates from the latest GitHub release.
//!
//! Uses Windows' own `curl.exe` for HTTPS (no extra libraries) and `certutil`
//! to check the SHA-256 printed in the release notes. The running exe can't be
//! overwritten, but it can be renamed: it becomes `delfin.old.exe` (deleted on
//! the next start) and the download takes its place.

use crate::i18n::{tr, trf};
use eframe::egui;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

const API: &str = "https://api.github.com/repos/Atimaniac/kde/releases/latest";
const ASSET: &str = "delfin.exe";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub sha256: Option<String>,
    pub notes: String,
}

#[derive(Clone, Debug)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading,
    Installed,
    Error(String),
}

#[derive(Clone)]
pub struct Updater {
    pub state: Arc<Mutex<State>>,
}

impl Default for Updater {
    fn default() -> Self {
        Self { state: Arc::new(Mutex::new(State::Idle)) }
    }
}

impl Updater {
    pub fn get(&self) -> State {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn set(&self, s: State, ctx: &egui::Context) {
        *self.state.lock().unwrap_or_else(|p| p.into_inner()) = s;
        ctx.request_repaint();
    }

    pub fn check(&self, ctx: &egui::Context) {
        let me = self.clone();
        let ctx = ctx.clone();
        self.set(State::Checking, &ctx);
        std::thread::spawn(move || {
            let s = match latest_release() {
                Ok(r) if is_newer(&r.version, CURRENT) => State::Available(r),
                Ok(_) => State::UpToDate,
                Err(e) => State::Error(trf("Couldn't check for updates: {}", &[&e])),
            };
            me.set(s, &ctx);
        });
    }

    pub fn install(&self, release: Release, ctx: &egui::Context) {
        let me = self.clone();
        let ctx = ctx.clone();
        self.set(State::Downloading, &ctx);
        std::thread::spawn(move || {
            let s = match download_and_replace(&release) {
                Ok(()) => State::Installed,
                Err(e) => State::Error(e),
            };
            me.set(s, &ctx);
        });
    }
}

/// Remove the previous exe left behind by an update (it was still running then).
pub fn cleanup_old() {
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) else {
        return;
    };
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if n.starts_with("delfin.old") && n.ends_with(".exe") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// The exe an update replaces.
pub fn target_path() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("curl.exe")
        .args(["-sSL", "--fail", "--max-time", "120", "-A", "Delfin-updater"])
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if msg.is_empty() { format!("curl exit {}", out.status) } else { msg });
    }
    Ok(out.stdout)
}

fn latest_release() -> Result<Release, String> {
    let body = curl(&["-H", "Accept: application/vnd.github+json", API])?;
    let json = String::from_utf8_lossy(&body);
    let tag = json_string(&json, "tag_name").ok_or("no tag_name in the GitHub answer")?;
    let url = json_strings(&json, "browser_download_url")
        .into_iter()
        .find(|u| u.rsplit('/').next().is_some_and(|f| f.eq_ignore_ascii_case(ASSET)))
        .ok_or_else(|| tr("The release has no delfin.exe.").to_string())?;
    let notes = json_string(&json, "body").unwrap_or_default();
    Ok(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        url,
        sha256: find_sha256(&notes),
        notes,
    })
}

fn download_and_replace(r: &Release) -> Result<(), String> {
    let exe = target_path().ok_or("current exe unknown")?;
    let dir = exe.parent().ok_or("current exe has no folder")?.to_path_buf();
    let new = dir.join("delfin.new.exe");
    let _ = std::fs::remove_file(&new);
    let new_s = new.to_string_lossy().to_string();
    curl(&["-o", &new_s, &r.url]).map_err(|e| trf("Couldn't download the update: {}", &[&e]))?;

    // Sanity checks before touching the installed exe.
    let bytes = std::fs::read(&new).map_err(|e| trf("Couldn't download the update: {}", &[&e]))?;
    if bytes.len() < 100_000 || !bytes.starts_with(b"MZ") {
        let _ = std::fs::remove_file(&new);
        return Err(trf("Couldn't download the update: {}", &[&"not a Windows program"]));
    }
    if let Some(want) = &r.sha256 {
        let got = sha256_of(&new).unwrap_or_default();
        if !got.eq_ignore_ascii_case(want) {
            let _ = std::fs::remove_file(&new);
            return Err(tr("The downloaded file doesn't match the release checksum.").to_string());
        }
    }

    // Swap: running exe -> delfin.old*.exe, download -> exe.
    let mut old = dir.join("delfin.old.exe");
    if old.exists() && std::fs::remove_file(&old).is_err() {
        // An older update's exe is still running; pick a free name.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        old = dir.join(format!("delfin.old-{stamp}.exe"));
    }
    std::fs::rename(&exe, &old).map_err(|e| trf("Couldn't install the update: {}", &[&e]))?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe); // put the old one back
        return Err(trf("Couldn't install the update: {}", &[&e]));
    }
    Ok(())
}

fn sha256_of(p: &Path) -> Option<String> {
    let out = Command::new("certutil.exe")
        .arg("-hashfile")
        .arg(p)
        .arg("SHA256")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // Line 2 is the hash (older Windows prints it with spaces between bytes).
    text.lines()
        .map(|l| l.replace(' ', ""))
        .find(|l| l.len() == 64 && l.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The first run of exactly 64 hex digits in the release notes.
fn find_sha256(notes: &str) -> Option<String> {
    let mut run = String::new();
    for c in notes.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_hexdigit() {
            run.push(c);
        } else {
            if run.len() == 64 {
                return Some(run);
            }
            run.clear();
        }
    }
    None
}

/// "0.10.0" > "0.9.3"; missing parts count as 0.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.split(['.', '-', '+'])
            .take(3)
            .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0))
            .collect()
    };
    let (mut a, mut b) = (parse(candidate), parse(current));
    a.resize(3, 0);
    b.resize(3, 0);
    a > b
}

// ------------------------------------------------------------------ tiny JSON reader
// Only what the GitHub "latest release" answer needs: string values by key.

fn json_strings(json: &str, key: &str) -> Vec<String> {
    let pat = format!("\"{key}\"");
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = json[from..].find(&pat) {
        let after = from + i + pat.len();
        let rest = json[after..].trim_start();
        if let Some(rest) = rest.strip_prefix(':') {
            let rest = rest.trim_start();
            if let Some(s) = rest.strip_prefix('"') {
                if let Some(v) = read_json_string(s) {
                    out.push(v);
                }
            }
        }
        from = after;
    }
    out
}

fn json_string(json: &str, key: &str) -> Option<String> {
    json_strings(json, key).into_iter().next()
}

/// Decode a JSON string body (after the opening quote) up to its closing quote.
fn read_json_string(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => {}
                'b' | 'f' => {}
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    let code = u32::from_str_radix(&hex, 16).ok()?;
                    out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                }
                other => out.push(other), // \" \\ \/
            },
            c => out.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.3"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("1.0", "0.9.9"));
    }

    #[test]
    fn json() {
        let j = r#"{"tag_name": "v0.2.0","assets":[{"browser_download_url":"https://x/y/delfin.exe"}],"body":"SHA-256: `1904E3B5C67F3B9D88280A0F2EDFCC146C20CB2F8FE98C7C59A6604A9889C8DE`\r\nok \"quoted\""}"#;
        assert_eq!(json_string(j, "tag_name").as_deref(), Some("v0.2.0"));
        assert_eq!(json_strings(j, "browser_download_url"), vec!["https://x/y/delfin.exe"]);
        let body = json_string(j, "body").unwrap();
        assert!(body.ends_with("ok \"quoted\""));
        assert_eq!(
            find_sha256(&body).as_deref(),
            Some("1904E3B5C67F3B9D88280A0F2EDFCC146C20CB2F8FE98C7C59A6604A9889C8DE")
        );
    }
}
