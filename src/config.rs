//! Settings saved to %APPDATA%\delfin\config.ini

use crate::fsops::SortBy;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    Icons,
    Compact,
    Details,
}

#[derive(Clone, PartialEq)]
pub struct Config {
    pub view: ViewMode,
    pub icon_size: f32,
    pub show_hidden: bool,
    pub info_panel: bool,
    pub places_panel: bool,
    pub dark: bool,
    pub sort: SortBy,
    pub sort_asc: bool,
    pub folders_first: bool,
    /// Shell: hide minimized windows into the tray.
    pub minimize_to_tray: bool,
    /// User-added entries in the Places panel.
    pub places: Vec<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            view: ViewMode::Icons,
            icon_size: 64.0,
            show_hidden: false,
            info_panel: false,
            places_panel: true,
            dark: true,
            sort: SortBy::Name,
            sort_asc: true,
            folders_first: true,
            minimize_to_tray: false,
            places: Vec::new(),
        }
    }
}

impl Config {
    fn file() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("delfin").join("config.ini"))
    }

    pub fn load() -> Self {
        let mut c = Self::default();
        let Some(path) = Self::file() else { return c };
        let Ok(text) = fs::read_to_string(path) else {
            return c;
        };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim();
            match k.trim() {
                "view" => {
                    c.view = match v {
                        "compact" => ViewMode::Compact,
                        "details" => ViewMode::Details,
                        _ => ViewMode::Icons,
                    }
                }
                "icon_size" => {
                    if let Ok(x) = v.parse::<f32>() {
                        c.icon_size = x.clamp(24.0, 160.0);
                    }
                }
                "show_hidden" => c.show_hidden = v == "1",
                "info_panel" => c.info_panel = v == "1",
                "places_panel" => c.places_panel = v == "1",
                "dark" => c.dark = v == "1",
                "folders_first" => c.folders_first = v == "1",
                "sort_asc" => c.sort_asc = v == "1",
                "minimize_to_tray" => c.minimize_to_tray = v == "1",
                "sort" => {
                    c.sort = match v {
                        "size" => SortBy::Size,
                        "modified" => SortBy::Modified,
                        "type" => SortBy::Type,
                        _ => SortBy::Name,
                    }
                }
                "place" => c.places.push(PathBuf::from(v)),
                _ => {}
            }
        }
        c
    }

    pub fn save(&self) {
        let Some(path) = Self::file() else { return };
        let b = |x: bool| if x { "1" } else { "0" };
        let mut s = String::new();
        s += &format!(
            "view={}\n",
            match self.view {
                ViewMode::Icons => "icons",
                ViewMode::Compact => "compact",
                ViewMode::Details => "details",
            }
        );
        s += &format!("icon_size={}\n", self.icon_size.round());
        s += &format!("show_hidden={}\n", b(self.show_hidden));
        s += &format!("info_panel={}\n", b(self.info_panel));
        s += &format!("places_panel={}\n", b(self.places_panel));
        s += &format!("dark={}\n", b(self.dark));
        s += &format!("folders_first={}\n", b(self.folders_first));
        s += &format!("sort_asc={}\n", b(self.sort_asc));
        s += &format!("minimize_to_tray={}\n", b(self.minimize_to_tray));
        s += &format!(
            "sort={}\n",
            match self.sort {
                SortBy::Name => "name",
                SortBy::Size => "size",
                SortBy::Modified => "modified",
                SortBy::Type => "type",
            }
        );
        for p in &self.places {
            s += &format!("place={}\n", p.display());
        }
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, s);
    }
}
