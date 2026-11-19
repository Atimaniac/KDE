//! English / Polish user interface.
//!
//! The source code is written in English; `tr("Open")` returns the Polish text
//! when Polish is chosen. The choice lives in `%APPDATA%\delfin\lang.txt`; with
//! no choice yet, Delfin follows the Windows display language.

use std::collections::HashMap;
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    Pl,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Pl => "pl",
        }
    }
}

// 0 = not loaded yet, 1 = English, 2 = Polish
static LANG: AtomicU8 = AtomicU8::new(0);

fn file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("delfin").join("lang.txt"))
}

fn system_is_polish() -> bool {
    #[cfg(windows)]
    {
        // Primary language id 0x15 = Polish.
        let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
        id & 0x3ff == 0x15
    }
    #[cfg(not(windows))]
    {
        std::env::var("LANG").is_ok_and(|l| l.starts_with("pl"))
    }
}

pub fn lang() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::En,
        2 => Lang::Pl,
        _ => {
            let chosen = file()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .map(|s| s.trim().to_string());
            let l = match chosen.as_deref() {
                Some("pl") => Lang::Pl,
                Some("en") => Lang::En,
                _ if system_is_polish() => Lang::Pl,
                _ => Lang::En,
            };
            LANG.store(if l == Lang::Pl { 2 } else { 1 }, Ordering::Relaxed);
            l
        }
    }
}

/// Switch language now and remember it. Open windows update on their next frame;
/// other Delfin windows pick it up when they start.
pub fn set_lang(l: Lang) {
    LANG.store(if l == Lang::Pl { 2 } else { 1 }, Ordering::Relaxed);
    if let Some(p) = file() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, l.code());
    }
}

fn table() -> &'static HashMap<&'static str, &'static str> {
    static T: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    T.get_or_init(|| PL.iter().copied().collect())
}

/// Translate a piece of UI text (English in, current language out).
pub fn tr<'a>(s: &'a str) -> &'a str {
    if lang() == Lang::En {
        return s;
    }
    table().get(s).copied().unwrap_or(s)
}

/// Translate a template and fill its `{}` placeholders in order.
pub fn trf(template: &str, args: &[&dyn Display]) -> String {
    let t = tr(template);
    let mut out = String::with_capacity(t.len() + 16);
    let mut rest = t;
    let mut i = 0;
    while let Some(pos) = rest.find("{}") {
        out.push_str(&rest[..pos]);
        if let Some(a) = args.get(i) {
            out.push_str(&a.to_string());
        }
        i += 1;
        rest = &rest[pos + 2..];
    }
    out.push_str(rest);
    out
}

pub fn month_name(m: u32) -> &'static str {
    const EN: [&str; 12] = [
        "January", "February", "March", "April", "May", "June", "July", "August", "September",
        "October", "November", "December",
    ];
    const PL: [&str; 12] = [
        "Styczeń", "Luty", "Marzec", "Kwiecień", "Maj", "Czerwiec", "Lipiec", "Sierpień",
        "Wrzesień", "Październik", "Listopad", "Grudzień",
    ];
    let i = (m.clamp(1, 12) - 1) as usize;
    if lang() == Lang::Pl { PL[i] } else { EN[i] }
}

/// Month in a date ("19 listopada"): Polish uses the genitive form.
pub fn month_in_date(m: u32) -> &'static str {
    const PL: [&str; 12] = [
        "stycznia", "lutego", "marca", "kwietnia", "maja", "czerwca", "lipca", "sierpnia",
        "września", "października", "listopada", "grudnia",
    ];
    if lang() == Lang::Pl { PL[(m.clamp(1, 12) - 1) as usize] } else { month_name(m) }
}

/// Monday = 0.
pub fn weekday_name(d: u32) -> &'static str {
    const EN: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    const PL: [&str; 7] = ["poniedziałek", "wtorek", "środa", "czwartek", "piątek", "sobota", "niedziela"];
    let i = (d % 7) as usize;
    if lang() == Lang::Pl { PL[i] } else { EN[i] }
}

pub fn weekday_short(d: u32) -> &'static str {
    const EN: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
    const PL: [&str; 7] = ["Pn", "Wt", "Śr", "Cz", "Pt", "So", "Nd"];
    let i = (d % 7) as usize;
    if lang() == Lang::Pl { PL[i] } else { EN[i] }
}

const PL: &[(&str, &str)] = &[
    // ---- file manager
    ("About Delfin", "O programie Delfin"),
    ("Add Folder to Places", "Dodaj folder do Miejsc"),
    ("Add to Places", "Dodaj do Miejsc"),
    ("An item with that name already exists", "Element o tej nazwie już istnieje"),
    ("Ascending", "Rosnąco"),
    ("Attributes", "Atrybuty"),
    ("Back (Alt+Left)", "Wstecz (Alt+Lewo)"),
    ("Breeze Dark theme", "Ciemny motyw Breeze"),
    ("Cancel", "Anuluj"),
    ("Cannot create folder: {}", "Nie można utworzyć folderu: {}"),
    ("Cannot open this folder:\n{}", "Nie można otworzyć tego folderu:\n{}"),
    ("Close tab (Ctrl+W)", "Zamknij kartę (Ctrl+W)"),
    ("Close", "Zamknij"),
    ("Compact view (Ctrl+2)", "Widok kompaktowy (Ctrl+2)"),
    ("Contents", "Zawartość"),
    ("Copied", "Skopiowano"),
    ("Copy Location", "Kopiuj lokalizację"),
    ("Copy as path", "Kopiuj jako ścieżkę"),
    ("Copy", "Kopiuj"),
    ("Cut", "Wytnij"),
    ("Create New Folder", "Utwórz nowy folder"),
    ("Create New Folder…", "Utwórz nowy folder…"),
    ("Create new folder in {}:", "Utwórz nowy folder w {}:"),
    ("Delete Permanently", "Usuń trwale"),
    ("Delete", "Usuń"),
    ("Delfin set as shell. Sign out and back in.", "Delfin ustawiony jako powłoka. Wyloguj się i zaloguj ponownie."),
    ("Desktop", "Pulpit"),
    ("Details view (Ctrl+3)", "Widok szczegółów (Ctrl+3)"),
    ("Devices", "Urządzenia"),
    ("Documents", "Dokumenty"),
    ("Downloads", "Pobrane"),
    ("Explorer restored.", "Przywrócono Eksploratora."),
    ("Filter…", "Filtruj…"),
    ("Folder is empty", "Folder jest pusty"),
    ("Folder", "Folder"),
    ("Folders first", "Najpierw foldery"),
    ("Folders now open in Delfin", "Foldery otwierają się teraz w Delfinie"),
    ("Folders open in Windows Explorer again", "Foldery znów otwierają się w Eksploratorze Windows"),
    ("Forward (Alt+Right)", "Dalej (Alt+Prawo)"),
    ("Hidden", "Ukryty"),
    ("Home (Alt+Home)", "Katalog domowy (Alt+Home)"),
    ("Home", "Katalog domowy"),
    ("How to go back:", "Jak wrócić:"),
    ("Icons view (Ctrl+1)", "Widok ikon (Ctrl+1)"),
    ("Information panel  (F11)", "Panel informacji  (F11)"),
    ("Information panel (F11)", "Panel informacji (F11)"),
    ("Invalid name", "Nieprawidłowa nazwa"),
    ("It takes effect after you sign out and sign back in. No admin rights needed.", "Zmiana zadziała po wylogowaniu i ponownym zalogowaniu. Nie są potrzebne uprawnienia administratora."),
    ("Location copied", "Skopiowano lokalizację"),
    ("Location not found: {}", "Nie znaleziono lokalizacji: {}"),
    ("Location", "Lokalizacja"),
    ("Menu", "Menu"),
    ("Middle click", "Środkowy przycisk"),
    ("Modified", "Zmodyfikowano"),
    ("Move to Trash", "Przenieś do kosza"),
    ("Moved {} item(s) to Trash", "Przeniesiono do kosza: {}"),
    ("Music", "Muzyka"),
    ("Name", "Nazwa"),
    ("New Folder", "Nowy folder"),
    ("New Folder…", "Nowy folder…"),
    ("New Tab", "Nowa karta"),
    ("New name for \u{201c}{}\u{201d}:", "Nowa nazwa dla \u{201e}{}\u{201d}:"),
    ("New tab (Ctrl+T)", "Nowa karta (Ctrl+T)"),
    ("No items match the filter", "Żaden element nie pasuje do filtra"),
    ("Open Terminal Here", "Otwórz terminal tutaj"),
    ("Open folders in Delfin by default", "Domyślnie otwieraj foldery w Delfinie"),
    ("Open in New Tab", "Otwórz w nowej karcie"),
    ("Open", "Otwórz"),
    ("Paste", "Wklej"),
    ("Permanently delete {} item(s)? This cannot be undone.", "Trwale usunąć elementy ({})? Tego nie można cofnąć."),
    ("Pictures", "Obrazy"),
    ("Places panel  (F9)", "Panel Miejsca  (F9)"),
    ("Places", "Miejsca"),
    ("Properties", "Właściwości"),
    ("Refresh", "Odśwież"),
    ("Remove from Places", "Usuń z Miejsc"),
    ("Rename failed: {}", "Nie udało się zmienić nazwy: {}"),
    ("Rename", "Zmień nazwę"),
    ("Rename…", "Zmień nazwę…"),
    ("Restore Explorer", "Przywróć Eksploratora"),
    ("Restore Windows Explorer as shell…", "Przywróć Eksploratora Windows jako powłokę…"),
    ("Restore Windows Explorer", "Przywróć Eksploratora Windows"),
    ("Select All", "Zaznacz wszystko"),
    ("Show Hidden Files", "Pokaż ukryte pliki"),
    ("Show hidden files  (Ctrl+H)", "Pokaż ukryte pliki  (Ctrl+H)"),
    ("Size", "Rozmiar"),
    ("Sort by", "Sortuj według"),
    ("Split view (F3)", "Podziel widok (F3)"),
    ("System", "System"),
    ("Total size of files", "Łączny rozmiar plików"),
    ("Type", "Typ"),
    ("Up (Alt+Up)", "W górę (Alt+Góra)"),
    ("Use Delfin as Windows Shell", "Użyj Delfina jako powłoki Windows"),
    ("Use Delfin as Windows shell…", "Użyj Delfina jako powłoki Windows…"),
    ("Use Delfin as shell", "Użyj Delfina jako powłoki"),
    ("Videos", "Wideo"),
    ("Windows Explorer will be your shell again after you sign out and back in.", "Po wylogowaniu i ponownym zalogowaniu powłoką znów będzie Eksplorator Windows."),
    ("Working…", "Pracuję…"),
    ("Zoom (Ctrl+mouse wheel)", "Powiększenie (Ctrl+kółko myszy)"),
    ("Zoom", "Powiększenie"),
    ("{} selected ({})", "Zaznaczono: {} ({})"),
    ("{} folders, {} files ({})", "Folderów: {}, plików: {} ({})"),
    ("{} folders, {} files", "Folderów: {}, plików: {}"),
    ("{} free", "wolne: {}"),
    ("{} items selected", "Zaznaczono elementów: {}"),
    ("  … and {} more", "  … i {} więcej"),
    // ---- file types, drives, operations
    ("Application", "Aplikacja"),
    ("File", "Plik"),
    ("PDF document", "Dokument PDF"),
    ("Script", "Skrypt"),
    ("Shortcut", "Skrót"),
    ("System file", "Plik systemowy"),
    ("{} image", "Obraz {}"),
    ("{} video", "Wideo {}"),
    ("{} audio", "Dźwięk {}"),
    ("{} archive", "Archiwum {}"),
    ("{} document", "Dokument {}"),
    ("{} spreadsheet", "Arkusz {}"),
    ("{} presentation", "Prezentacja {}"),
    ("{} source", "Kod źródłowy {}"),
    ("{} text", "Tekst {}"),
    ("{} file", "Plik {}"),
    ("CD/DVD Drive", "Napęd CD/DVD"),
    ("Local Disk", "Dysk lokalny"),
    ("Network Drive", "Dysk sieciowy"),
    ("Removable Disk", "Dysk wymienny"),
    ("Cannot open {}: {}", "Nie można otworzyć {}: {}"),
    ("Cannot open terminal: {}", "Nie można otworzyć terminala: {}"),
    ("Cannot paste a folder into itself: {}", "Nie można wkleić folderu do niego samego: {}"),
    ("Deleted {} item(s)", "Usunięto elementów: {}"),
    ("Moved", "Przeniesiono"),
    ("Open in Delfin", "Otwórz w Delfinie"),
    ("Windows Explorer is the shell again. Sign out and back in to switch.", "Eksplorator Windows znów jest powłoką. Wyloguj się i zaloguj ponownie, aby przełączyć."),
    // ---- shell: launcher
    ("All Applications", "Wszystkie aplikacje"),
    ("Office", "Biuro"),
    ("Graphics", "Grafika"),
    ("Internet", "Internet"),
    ("Multimedia", "Multimedia"),
    ("System Tools", "Narzędzia systemowe"),
    ("Games", "Gry"),
    ("Recently Used", "Ostatnio używane"),
    ("Search applications…", "Szukaj aplikacji…"),
    ("Results", "Wyniki"),
    ("Command", "Polecenie"),
    ("Run \u{201c}{}\u{201d}", "Uruchom \u{201e}{}\u{201d}"),
    ("Start typing to find an app", "Zacznij pisać, żeby znaleźć aplikację"),
    ("or pick a category on the left", "albo wybierz kategorię po lewej"),
    ("No matching applications", "Brak pasujących aplikacji"),
    ("Press Enter to run it as a command", "Naciśnij Enter, żeby uruchomić to jako polecenie"),
    ("Nothing launched yet", "Nic jeszcze nie uruchamiano"),
    ("Apps you start from here show up in this list", "Tu pojawią się aplikacje uruchamiane z tego menu"),
    ("No applications in this category", "Brak aplikacji w tej kategorii"),
    ("Search finds every installed app", "Wyszukiwarka znajdzie każdą zainstalowaną aplikację"),
    ("This PC", "Ten komputer"),
    ("Settings", "Ustawienia"),
    ("Task Manager", "Menedżer zadań"),
    ("Run…", "Uruchom…"),
    ("Lock", "Zablokuj"),
    ("Shut Down", "Zamknij"),
    ("Restart", "Uruchom ponownie"),
    ("Log Out", "Wyloguj"),
    ("Sure?", "Na pewno?"),
    ("{} (click twice)", "{} (kliknij dwa razy)"),
    ("Minimize to tray", "Minimalizuj do zasobnika"),
    ("Back to Explorer", "Wróć do Eksploratora"),
    ("Click again to switch", "Kliknij ponownie, aby przełączyć"),
    ("File manager", "Menedżer plików"),
    ("Windows settings", "Ustawienia Windows"),
    ("Command line", "Wiersz poleceń"),
    ("Running programs", "Uruchomione programy"),
    ("Classic settings", "Klasyczne ustawienia"),
    ("Calculator", "Kalkulator"),
    ("Text editor", "Edytor tekstu"),
    ("Folder window", "Okno folderu"),
    ("Control Panel", "Panel sterowania"),
    ("Windows Explorer", "Eksplorator Windows"),
    ("Notepad", "Notatnik"),
    ("Terminal", "Terminal"),
    // ---- shell: panel, desktop, tray
    ("Volume", "Głośność"),
    ("Volume: muted", "Głośność: wyciszona"),
    ("Volume: {}%", "Głośność: {}%"),
    ("Network", "Sieć"),
    ("Wi-Fi: {}", "Wi-Fi: {}"),
    ("Show hidden icons", "Pokaż ukryte ikony"),
    ("Minimized to tray \u{2013} click to restore", "Zminimalizowane do zasobnika \u{2013} kliknij, aby przywrócić"),
    ("Notification", "Powiadomienie"),
    ("Change Wallpaper…", "Zmień tapetę…"),
    ("Display Settings…", "Ustawienia ekranu…"),
    ("Open Desktop Folder", "Otwórz folder pulpitu"),
    ("Show in Delfin", "Pokaż w Delfinie"),
    ("Today", "Dziś"),
    ("Next month", "Następny miesiąc"),
    ("Previous month", "Poprzedni miesiąc"),
    ("Date and time settings", "Ustawienia daty i godziny"),
    // ---- shell: quick settings
    ("All Settings", "Wszystkie ustawienia"),
    ("Battery {}%{}", "Bateria {}%{}"),
    (" (charging)", " (ładowanie)"),
    ("Bluetooth", "Bluetooth"),
    ("Connect", "Połącz"),
    ("Connected, open", "Połączono, otwarta"),
    ("Connected, secured", "Połączono, zabezpieczona"),
    ("Connecting to {}…", "Łączenie z {}…"),
    ("Couldn't connect to {}. Check the password.", "Nie udało się połączyć z {}. Sprawdź hasło."),
    ("Couldn't connect to {}.", "Nie udało się połączyć z {}."),
    ("Couldn't save the settings for {}.", "Nie udało się zapisać ustawień dla {}."),
    ("{} needs Windows Settings to connect.", "Do połączenia z {} potrzebne są Ustawienia Windows."),
    ("Disconnect", "Rozłącz"),
    ("Looking for networks…", "Szukanie sieci…"),
    ("Mute", "Wycisz"),
    ("Unmute", "Wyłącz wyciszenie"),
    ("Next", "Następny"),
    ("Previous", "Poprzedni"),
    ("Play / Pause", "Odtwórz / pauza"),
    ("No Wi-Fi adapter found.", "Nie znaleziono karty Wi-Fi."),
    ("Open Wi-Fi Settings", "Otwórz ustawienia Wi-Fi"),
    ("Password", "Hasło"),
    ("Scan again", "Skanuj ponownie"),
    ("Secured", "Zabezpieczona"),
    ("Open", "Otwórz"),
    ("Sound", "Dźwięk"),
    ("This network needs Windows Settings.", "Ta sieć wymaga Ustawień Windows."),
    ("Volume Mixer", "Mikser głośności"),
    ("Wi-Fi is off.", "Wi-Fi jest wyłączone."),
    ("Windows didn't allow switching Wi-Fi (try Settings).", "Windows nie pozwolił przełączyć Wi-Fi (spróbuj w Ustawieniach)."),
    // ---- Delfin settings and updates
    ("Delfin Settings", "Ustawienia Delfina"),
    ("Language", "Język"),
    ("Updates", "Aktualizacje"),
    ("Shell", "Powłoka"),
    ("Version {}", "Wersja {}"),
    ("Check for updates", "Sprawdź aktualizacje"),
    ("Checking for updates…", "Sprawdzanie aktualizacji…"),
    ("You have the latest version.", "Masz najnowszą wersję."),
    ("Version {} is available.", "Dostępna jest wersja {}."),
    ("Download and install", "Pobierz i zainstaluj"),
    ("Downloading…", "Pobieranie…"),
    ("Installing…", "Instalowanie…"),
    ("Update installed. Delfin will restart.", "Aktualizacja zainstalowana. Delfin uruchomi się ponownie."),
    ("Update installed. Start Delfin again to use it.", "Aktualizacja zainstalowana. Uruchom Delfina ponownie, aby jej używać."),
    ("Couldn't check for updates: {}", "Nie udało się sprawdzić aktualizacji: {}"),
    ("Couldn't download the update: {}", "Nie udało się pobrać aktualizacji: {}"),
    ("Couldn't install the update: {}", "Nie udało się zainstalować aktualizacji: {}"),
    ("The downloaded file doesn't match the release checksum.", "Pobrany plik nie zgadza się z sumą kontrolną wydania."),
    ("The release has no delfin.exe.", "Wydanie nie zawiera pliku delfin.exe."),
    ("Updates replace: {}", "Aktualizacja zastąpi: {}"),
    ("Release notes", "Informacje o wydaniu"),
    ("Some open Delfin windows switch language after they are reopened.", "Otwarte okna Delfina zmienią język po ponownym otwarciu."),
];
