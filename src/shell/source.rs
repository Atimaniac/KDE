//! "Go to what is playing": clicking the media element on the panel brings
//! up the player's window, and in a browser also the tab that plays.
//!
//! The player is known only by its app id (AUMID) from Windows' media
//! controls: "Brave.…", "MSEdge", "Chrome", "308046B0AF4A39CB" (Firefox),
//! "Spotify.exe", "SpotifyAB.SpotifyMusic_…!Spotify", … The window is matched
//! by its program; the tab through UI Automation, by the tab's title holding
//! the track's title. Only the browser's own tab strip is walked, never the
//! web pages, so the browser doesn't switch its accessibility mode on.

use std::ffi::c_void;

/// Program file names (without .exe) that are web browsers.
const BROWSERS: &[&str] = &["brave", "chrome", "msedge", "firefox", "opera", "vivaldi", "chromium", "librewolf", "waterfox", "zen", "floorp", "arc"];

pub fn is_browser(stem: &str) -> bool {
    BROWSERS.contains(&stem)
}

/// The program (file name without .exe, lower case) an app id points to.
fn hint(aumid: &str) -> Option<&'static str> {
    let l = aumid.to_lowercase();
    for (k, v) in [
        ("brave", "brave"),
        ("msedge", "msedge"),
        ("chrome", "chrome"),
        ("308046b0af4a39cb", "firefox"),
        ("firefox", "firefox"),
        ("opera", "opera"),
        ("vivaldi", "vivaldi"),
        ("spotify", "spotify"),
        ("vlc", "vlc"),
        ("foobar", "foobar2000"),
        ("aimp", "aimp"),
    ] {
        if l.contains(k) {
            return Some(v);
        }
    }
    None
}

pub fn stem(exe: &str) -> String {
    let name = exe.rsplit(['\\', '/']).next().unwrap_or(exe).to_lowercase();
    name.trim_end_matches(".exe").to_string()
}

/// A window on the panel: handle, title and program path (lower case).
pub struct Candidate {
    pub hwnd: isize,
    pub title: String,
    pub exe: String,
}

/// Windows that belong to the player, best first.
pub fn player_windows(aumid: &str, title: &str, windows: Vec<Candidate>) -> Vec<Candidate> {
    let al = aumid.to_lowercase();
    let pkg = super::win::package_key_of_aumid(aumid);
    let want = hint(aumid);
    let mut found: Vec<Candidate> = windows
        .into_iter()
        .filter(|w| {
            let s = stem(&w.exe);
            if s.is_empty() {
                return false;
            }
            if let Some(k) = &pkg {
                if super::win::package_key_of_exe(&w.exe).as_ref() == Some(k) {
                    return true;
                }
            }
            want == Some(s.as_str()) || al == format!("{s}.exe") || (s.len() >= 4 && al.contains(&s))
        })
        .collect();
    // The window showing the track (a browser's active tab, a player's title) first.
    let tl = title.to_lowercase();
    found.sort_by_key(|w| !(!tl.is_empty() && w.title.to_lowercase().contains(&tl)));
    found
}

/// Select the browser tab whose title holds the track's title, in any of the
/// given browser windows. Returns the window that has it.
pub fn select_tab(windows: &[isize], title: &str, artist: &str) -> Option<isize> {
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};
    let title = title.trim().to_lowercase();
    if title.chars().count() < 2 {
        return None;
    }
    let artist = artist.trim().to_lowercase();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let walker = uia.ControlViewWalker().ok()?;
        // (score, window, tab) of the best match over all windows.
        let mut best: Option<(u32, isize, windows::Win32::UI::Accessibility::IUIAutomationElement)> = None;
        for &h in windows {
            let Ok(root) = uia.ElementFromHandle(windows::Win32::Foundation::HWND(h as *mut c_void)) else { continue };
            let mut tabs = Vec::new();
            let mut budget = 4000;
            collect_tabs(&walker, &root, 0, &mut budget, &mut tabs);
            for tab in tabs {
                let name = tab.CurrentName().map(|b| b.to_string().to_lowercase()).unwrap_or_default();
                if !name.contains(&title) {
                    continue;
                }
                let mut score = 2;
                if !artist.is_empty() && name.contains(&artist) {
                    score += 1;
                }
                // Chromium and Firefox add a note to tabs that play sound.
                if ["playing audio", "audio playing", "odtwarzanie dźwięku", "odtwarza dźwięk", "dźwięk jest odtwarzany"].iter().any(|m| name.contains(m)) {
                    score += 2;
                }
                if best.as_ref().is_none_or(|b| score > b.0) {
                    best = Some((score, h, tab));
                }
            }
        }
        let (_, h, tab) = best?;
        activate_tab(&tab);
        Some(h)
    }
}

/// Tab items of a browser's own interface, depth first. Web pages
/// (documents and their host windows) are skipped.
unsafe fn collect_tabs(
    walker: &windows::Win32::UI::Accessibility::IUIAutomationTreeWalker,
    el: &windows::Win32::UI::Accessibility::IUIAutomationElement,
    depth: u32,
    budget: &mut i32,
    out: &mut Vec<windows::Win32::UI::Accessibility::IUIAutomationElement>,
) {
    use windows::Win32::UI::Accessibility::{UIA_DocumentControlTypeId, UIA_TabItemControlTypeId};
    if depth > 16 || *budget <= 0 {
        return;
    }
    let mut child = walker.GetFirstChildElement(el).ok();
    while let Some(c) = child {
        *budget -= 1;
        if *budget <= 0 {
            return;
        }
        let ct = c.CurrentControlType().unwrap_or_default();
        let class = c.CurrentClassName().map(|b| b.to_string()).unwrap_or_default();
        if ct == UIA_TabItemControlTypeId {
            out.push(c.clone());
        } else if ct != UIA_DocumentControlTypeId && class != "Chrome_RenderWidgetHostHWND" && class != "MozillaContentWindowClass" {
            collect_tabs(walker, &c, depth + 1, budget, out);
        }
        child = walker.GetNextSiblingElement(&c).ok();
    }
}

unsafe fn activate_tab(tab: &windows::Win32::UI::Accessibility::IUIAutomationElement) {
    use windows::Win32::UI::Accessibility::{
        IUIAutomationLegacyIAccessiblePattern, IUIAutomationSelectionItemPattern, UIA_LegacyIAccessiblePatternId,
        UIA_SelectionItemPatternId,
    };
    if let Ok(p) = tab.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId) {
        if p.Select().is_ok() {
            return;
        }
    }
    if let Ok(p) = tab.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(UIA_LegacyIAccessiblePatternId) {
        let _ = p.DoDefaultAction();
    }
}
