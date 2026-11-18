//! Breeze-style icons painted with vector shapes (no image files needed).

use crate::theme::Palette;
use eframe::egui::{
    pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui,
};

pub const FOLDER: Color32 = Color32::from_rgb(0x3d, 0xae, 0xe9);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Image,
    Video,
    Audio,
    Archive,
    Pdf,
    Doc,
    Sheet,
    Slides,
    Code,
    Exe,
    Text,
    Other,
}

pub fn kind_of(ext: &str, is_dir: bool) -> Kind {
    if is_dir {
        return Kind::Folder;
    }
    match ext {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tif" | "tiff"
        | "heic" | "avif" | "raw" | "psd" | "kra" | "xcf" => Kind::Image,
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "wmv" | "flv" | "m4v" | "mpg" | "mpeg" => {
            Kind::Video
        }
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" | "opus" | "wma" => Kind::Audio,
        "zip" | "rar" | "7z" | "tar" | "gz" | "xz" | "bz2" | "zst" | "cab" | "iso" => Kind::Archive,
        "pdf" => Kind::Pdf,
        "doc" | "docx" | "odt" | "rtf" => Kind::Doc,
        "xls" | "xlsx" | "ods" | "csv" => Kind::Sheet,
        "ppt" | "pptx" | "odp" => Kind::Slides,
        "rs" | "py" | "js" | "ts" | "c" | "h" | "cpp" | "hpp" | "cs" | "java" | "go" | "json"
        | "toml" | "yaml" | "yml" | "html" | "css" | "xml" | "sh" | "lua" | "kt" | "swift"
        | "php" | "rb" | "sql" => Kind::Code,
        "exe" | "msi" | "bat" | "cmd" | "ps1" | "com" | "appimage" | "lnk" | "dll" | "sys" => {
            Kind::Exe
        }
        "txt" | "md" | "log" | "ini" | "cfg" | "conf" | "nfo" => Kind::Text,
        _ => Kind::Other,
    }
}

pub fn kind_color(k: Kind) -> Color32 {
    match k {
        Kind::Folder => FOLDER,
        Kind::Image => Color32::from_rgb(0x9b, 0x59, 0xb6),
        Kind::Video => Color32::from_rgb(0xda, 0x44, 0x53),
        Kind::Audio => Color32::from_rgb(0xf6, 0x74, 0x00),
        Kind::Archive => Color32::from_rgb(0xc9, 0x9a, 0x2e),
        Kind::Pdf => Color32::from_rgb(0xe7, 0x4c, 0x3c),
        Kind::Doc => Color32::from_rgb(0x1d, 0x99, 0xf3),
        Kind::Sheet => Color32::from_rgb(0x27, 0xae, 0x60),
        Kind::Slides => Color32::from_rgb(0xf4, 0x77, 0x50),
        Kind::Code => Color32::from_rgb(0x1a, 0xbc, 0x9c),
        Kind::Exe => Color32::from_rgb(0x4d, 0x4d, 0x4d),
        Kind::Text => Color32::from_rgb(0x7f, 0x8c, 0x8d),
        Kind::Other => Color32::from_rgb(0x95, 0xa5, 0xa6),
    }
}

pub fn type_name(ext: &str, is_dir: bool) -> String {
    let up = ext.to_uppercase();
    match kind_of(ext, is_dir) {
        Kind::Folder => "Folder".into(),
        Kind::Image => format!("{up} image"),
        Kind::Video => format!("{up} video"),
        Kind::Audio => format!("{up} audio"),
        Kind::Archive => format!("{up} archive"),
        Kind::Pdf => "PDF document".into(),
        Kind::Doc => format!("{up} document"),
        Kind::Sheet => format!("{up} spreadsheet"),
        Kind::Slides => format!("{up} presentation"),
        Kind::Code => format!("{up} source"),
        Kind::Exe => match ext {
            "exe" | "msi" | "appimage" => "Application".into(),
            "lnk" => "Shortcut".into(),
            "dll" | "sys" => "System file".into(),
            _ => "Script".into(),
        },
        Kind::Text => format!("{up} text"),
        Kind::Other if ext.is_empty() => "File".into(),
        Kind::Other => format!("{up} file"),
    }
}

fn darken(c: Color32, f: f32) -> Color32 {
    Color32::from_rgb(
        (c.r() as f32 * f) as u8,
        (c.g() as f32 * f) as u8,
        (c.b() as f32 * f) as u8,
    )
}

fn lighten(c: Color32, f: f32) -> Color32 {
    let l = |x: u8| (x as f32 + (255.0 - x as f32) * f) as u8;
    Color32::from_rgb(l(c.r()), l(c.g()), l(c.b()))
}

pub fn paint_entry_icon(p: &Painter, rect: Rect, is_dir: bool, ext: &str, pal: &Palette) {
    if is_dir {
        paint_folder(p, rect, FOLDER);
    } else {
        paint_file(p, rect, kind_color(kind_of(ext, false)), ext, pal);
    }
}

/// Breeze-like flat folder.
pub fn paint_folder(p: &Painter, rect: Rect, base: Color32) {
    let s = rect.width().min(rect.height());
    let w = s * 0.92;
    let h = s * 0.74;
    let left = rect.center().x - w / 2.0;
    let top = rect.center().y - h / 2.0;
    let r = (s * 0.06).max(1.0);
    let back = darken(base, 0.72);

    // tab + back sheet
    p.rect_filled(
        Rect::from_min_size(pos2(left, top), vec2(w * 0.42, h * 0.3)),
        r,
        back,
    );
    p.rect_filled(
        Rect::from_min_max(pos2(left, top + h * 0.12), pos2(left + w, top + h)),
        r,
        back,
    );
    // front sheet
    let front_top = top + h * 0.26;
    p.rect_filled(
        Rect::from_min_max(pos2(left, front_top), pos2(left + w, top + h)),
        r,
        base,
    );
    // highlight edge
    p.line_segment(
        [
            pos2(left + r, front_top + 0.5),
            pos2(left + w - r, front_top + 0.5),
        ],
        Stroke::new((s * 0.025).max(1.0), lighten(base, 0.35)),
    );
}

/// Sheet of paper with a folded corner and a coloured type band.
pub fn paint_file(p: &Painter, rect: Rect, color: Color32, ext: &str, pal: &Palette) {
    let s = rect.width().min(rect.height());
    let h = s * 0.92;
    let w = h * 0.76;
    let l = rect.center().x - w / 2.0;
    let t = rect.center().y - h / 2.0;
    let fold = w * 0.28;

    let outline = Stroke::new(1.0_f32, darken(pal.paper, 0.7));
    p.add(Shape::convex_polygon(
        vec![
            pos2(l, t),
            pos2(l + w - fold, t),
            pos2(l + w, t + fold),
            pos2(l + w, t + h),
            pos2(l, t + h),
        ],
        pal.paper,
        outline,
    ));
    p.add(Shape::convex_polygon(
        vec![
            pos2(l + w - fold, t),
            pos2(l + w - fold, t + fold),
            pos2(l + w, t + fold),
        ],
        darken(pal.paper, 0.82),
        outline,
    ));

    let band = Rect::from_min_max(pos2(l + 0.5, t + h * 0.56), pos2(l + w - 0.5, t + h * 0.82));
    p.rect_filled(band, 0.0, color);

    if band.height() >= 9.0 && !ext.is_empty() {
        let label: String = ext.to_uppercase().chars().take(4).collect();
        p.text(
            band.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::proportional((band.height() * 0.62).min(16.0)),
            Color32::WHITE,
        );
    }
}

pub fn paint_drive(p: &Painter, rect: Rect) {
    let s = rect.width().min(rect.height());
    let w = s * 0.9;
    let h = s * 0.5;
    let r = Rect::from_center_size(rect.center(), vec2(w, h));
    let body = Color32::from_rgb(0x7f, 0x8c, 0x8d);
    p.rect_filled(r, s * 0.1, body);
    p.rect_stroke(r, s * 0.1, Stroke::new(1.0_f32, darken(body, 0.7)));
    p.circle_filled(
        pos2(r.right() - h * 0.35, r.center().y),
        (s * 0.06).max(1.5),
        crate::theme::ACCENT,
    );
    p.line_segment(
        [
            pos2(r.left() + w * 0.15, r.center().y),
            pos2(r.left() + w * 0.55, r.center().y),
        ],
        Stroke::new(1.0_f32, lighten(body, 0.4)),
    );
}

// ------------------------------------------------------------ toolbar icons

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Back,
    Forward,
    Up,
    Home,
    Icons,
    Compact,
    Details,
    Split,
    Search,
    Menu,
    Terminal,
    Info,
    Refresh,
    Plus,
    Close,
    Speaker,
    Network,
    Power,
    Lock,
    Logout,
    Gear,
    Desktop,
}

fn arc(c: Pos2, r: f32, a0: f32, a1: f32) -> Vec<Pos2> {
    (0..=16)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / 16.0;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect()
}

pub fn paint_tool(p: &Painter, rect: Rect, t: Tool, col: Color32) {
    let c = rect.center();
    let s = Stroke::new(1.6_f32, col);
    let line = |pts: Vec<Pos2>| {
        p.add(Shape::line(pts, s));
    };
    match t {
        Tool::Back => line(vec![c + vec2(3., -6.), c + vec2(-3., 0.), c + vec2(3., 6.)]),
        Tool::Forward => line(vec![
            c + vec2(-3., -6.),
            c + vec2(3., 0.),
            c + vec2(-3., 6.),
        ]),
        Tool::Up => line(vec![c + vec2(-6., 3.), c + vec2(0., -3.), c + vec2(6., 3.)]),
        Tool::Home => {
            line(vec![c + vec2(-7., 0.), c + vec2(0., -7.), c + vec2(7., 0.)]);
            line(vec![
                c + vec2(-5., -1.5),
                c + vec2(-5., 6.),
                c + vec2(5., 6.),
                c + vec2(5., -1.5),
            ]);
            line(vec![
                c + vec2(-1.5, 6.),
                c + vec2(-1.5, 1.5),
                c + vec2(1.5, 1.5),
                c + vec2(1.5, 6.),
            ]);
        }
        Tool::Icons => {
            for (dx, dy) in [(-6.5, -6.5), (1.5, -6.5), (-6.5, 1.5), (1.5, 1.5)] {
                p.rect_stroke(Rect::from_min_size(c + vec2(dx, dy), vec2(5., 5.)), 1.0, s);
            }
        }
        Tool::Compact => {
            for dy in [-5., 0., 5.] {
                p.rect_filled(
                    Rect::from_center_size(c + vec2(-5., dy), vec2(3., 3.)),
                    0.5,
                    col,
                );
                line(vec![c + vec2(-1., dy), c + vec2(7., dy)]);
            }
        }
        Tool::Details => {
            for dy in [-6., -2., 2., 6.] {
                line(vec![c + vec2(-7., dy), c + vec2(7., dy)]);
            }
        }
        Tool::Split => {
            p.rect_stroke(Rect::from_center_size(c, vec2(15., 12.)), 1.5, s);
            line(vec![c + vec2(0., -6.), c + vec2(0., 6.)]);
        }
        Tool::Search => {
            p.circle_stroke(c + vec2(-1.5, -1.5), 4.5, s);
            line(vec![c + vec2(1.8, 1.8), c + vec2(6., 6.)]);
        }
        Tool::Menu => {
            for dy in [-5., 0., 5.] {
                line(vec![c + vec2(-7., dy), c + vec2(7., dy)]);
            }
        }
        Tool::Terminal => {
            p.rect_stroke(Rect::from_center_size(c, vec2(16., 13.)), 1.5, s);
            line(vec![
                c + vec2(-5., -3.),
                c + vec2(-2., 0.),
                c + vec2(-5., 3.),
            ]);
            line(vec![c + vec2(0., 3.5), c + vec2(4., 3.5)]);
        }
        Tool::Info => {
            p.circle_stroke(c, 7., s);
            line(vec![c + vec2(0., -0.5), c + vec2(0., 4.)]);
            p.circle_filled(c + vec2(0., -3.5), 1.0, col);
        }
        Tool::Refresh => {
            let pts: Vec<Pos2> = (0..=20)
                .map(|i| {
                    let a = 0.6 + (i as f32 / 20.0) * (std::f32::consts::TAU - 1.2);
                    c + vec2(a.cos(), a.sin()) * 6.0
                })
                .collect();
            let end = *pts.last().unwrap();
            line(pts);
            line(vec![end + vec2(-3.5, -1.0), end, end + vec2(0.5, -3.8)]);
        }
        Tool::Plus => {
            line(vec![c + vec2(-5., 0.), c + vec2(5., 0.)]);
            line(vec![c + vec2(0., -5.), c + vec2(0., 5.)]);
        }
        Tool::Close => {
            line(vec![c + vec2(-3.5, -3.5), c + vec2(3.5, 3.5)]);
            line(vec![c + vec2(3.5, -3.5), c + vec2(-3.5, 3.5)]);
        }
        Tool::Speaker => {
            line(vec![
                c + vec2(-7., -2.5),
                c + vec2(-4., -2.5),
                c + vec2(0., -6.),
                c + vec2(0., 6.),
                c + vec2(-4., 2.5),
                c + vec2(-7., 2.5),
                c + vec2(-7., -2.5),
            ]);
            line(arc(c + vec2(0., 0.), 3.5, -0.8, 0.8));
            line(arc(c + vec2(0., 0.), 6.5, -0.8, 0.8));
        }
        Tool::Network => {
            let base = c + vec2(0., 6.);
            for r in [4.5, 8.5, 12.5] {
                line(arc(base, r, -2.36, -0.78));
            }
            p.circle_filled(base, 1.5, col);
        }
        Tool::Power => {
            let pi = std::f32::consts::PI;
            line(arc(c + vec2(0., 1.), 6.5, -pi / 2.0 + 0.7, 1.5 * pi - 0.7));
            line(vec![c + vec2(0., -7.), c + vec2(0., 0.)]);
        }
        Tool::Lock => {
            let pi = std::f32::consts::PI;
            p.rect_stroke(
                Rect::from_min_max(c + vec2(-5.5, -1.), c + vec2(5.5, 7.)),
                1.0,
                s,
            );
            let mut sh = vec![c + vec2(-3.5, -1.)];
            sh.extend(arc(c + vec2(0., -3.5), 3.5, pi, 2.0 * pi));
            sh.push(c + vec2(3.5, -1.));
            line(sh);
        }
        Tool::Logout => {
            line(vec![
                c + vec2(1., -7.),
                c + vec2(-6., -7.),
                c + vec2(-6., 7.),
                c + vec2(1., 7.),
            ]);
            line(vec![c + vec2(-2., 0.), c + vec2(7., 0.)]);
            line(vec![c + vec2(4., -3.), c + vec2(7., 0.), c + vec2(4., 3.)]);
        }
        Tool::Gear => {
            p.circle_stroke(c, 3., s);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let d = vec2(a.cos(), a.sin());
                line(vec![c + d * 5., c + d * 7.5]);
            }
        }
        Tool::Desktop => {
            p.rect_stroke(
                Rect::from_min_max(c + vec2(-7.5, -6.), c + vec2(7.5, 4.)),
                1.0,
                s,
            );
            line(vec![c + vec2(0., 4.), c + vec2(0., 7.)]);
            line(vec![c + vec2(-4., 7.), c + vec2(4., 7.)]);
        }
    }
}

/// Flat Breeze tool button: transparent, blue outline on hover.
pub fn tool_button_sized(
    ui: &mut Ui,
    t: Tool,
    selected: bool,
    enabled: bool,
    tip: &str,
    size: eframe::egui::Vec2,
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let accent = v.hyperlink_color;
        if selected {
            ui.painter().rect_filled(rect, 4.0, v.selection.bg_fill);
            ui.painter()
                .rect_stroke(rect, 4.0, Stroke::new(1.0_f32, accent));
        } else if enabled && resp.hovered() {
            ui.painter()
                .rect_filled(rect, 4.0, v.widgets.hovered.weak_bg_fill);
            ui.painter()
                .rect_stroke(rect, 4.0, Stroke::new(1.0_f32, accent));
        }
        let col = if enabled {
            v.text_color()
        } else {
            v.weak_text_color()
        };
        paint_tool(ui.painter(), rect, t, col);
    }
    if tip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tip)
    }
}

pub fn tool_button(ui: &mut Ui, t: Tool, selected: bool, enabled: bool, tip: &str) -> Response {
    tool_button_sized(ui, t, selected, enabled, tip, vec2(30.0, 28.0))
}
