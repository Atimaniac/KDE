//! Directory listing, sorting and file operations.

use std::cmp::Ordering;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortBy {
    Name,
    Size,
    Modified,
    Type,
}

pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub hidden: bool,
    pub ext: String,
}

pub fn list_dir(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for item in fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let path = item.path();
        let name = item.file_name().to_string_lossy().into_owned();
        let Ok(md) = item.metadata() else { continue };
        let mut is_dir = md.is_dir();
        if md.file_type().is_symlink() {
            if let Ok(target) = fs::metadata(&path) {
                is_dir = target.is_dir();
            }
        }
        let hidden = is_hidden(&name, &md);
        let ext = if is_dir {
            String::new()
        } else {
            path.extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        };
        out.push(Entry {
            name,
            path,
            is_dir,
            size: if is_dir { 0 } else { md.len() },
            modified: md.modified().ok(),
            hidden,
            ext,
        });
    }
    Ok(out)
}

#[cfg(windows)]
fn is_hidden(name: &str, md: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    md.file_attributes() & (HIDDEN | SYSTEM) != 0 || name.starts_with('.')
}

#[cfg(not(windows))]
fn is_hidden(name: &str, _md: &fs::Metadata) -> bool {
    name.starts_with('.')
}

/// "file2" < "file10", case-insensitive.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(&c) = ai.peek() {
                    if c.is_ascii_digit() {
                        na.push(c);
                        ai.next();
                    } else {
                        break;
                    }
                }
                let mut nb = String::new();
                while let Some(&c) = bi.peek() {
                    if c.is_ascii_digit() {
                        nb.push(c);
                        bi.next();
                    } else {
                        break;
                    }
                }
                let ta = na.trim_start_matches('0');
                let tb = nb.trim_start_matches('0');
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

pub fn compare(a: &Entry, b: &Entry, by: SortBy, asc: bool, folders_first: bool) -> Ordering {
    if folders_first && a.is_dir != b.is_dir {
        return if a.is_dir {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let name = || natural_cmp(&a.name, &b.name);
    let o = match by {
        SortBy::Name => name(),
        SortBy::Size => a.size.cmp(&b.size).then_with(name),
        SortBy::Modified => a.modified.cmp(&b.modified).then_with(name),
        SortBy::Type => a.ext.cmp(&b.ext).then_with(name),
    };
    if asc {
        o
    } else {
        o.reverse()
    }
}

pub fn fmt_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

pub fn fmt_time(t: Option<SystemTime>) -> String {
    match t {
        Some(t) => chrono::DateTime::<chrono::Local>::from(t)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        None => String::new(),
    }
}

/// `dir/name`, or `dir/name (2).ext` etc. if it already exists.
pub fn unique_dest(dir: &Path, name: &OsStr) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let mut i = 2u32;
    loop {
        let candidate = dir.join(format!("{stem} ({i}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
        i += 1;
    }
}

pub fn copy_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    let md = fs::symlink_metadata(src)?;
    if md.is_dir() {
        fs::create_dir_all(dst)?;
        for e in fs::read_dir(src)? {
            let e = e?;
            copy_recursive(&e.path(), &dst.join(e.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(src, dst).map(|_| ())
    }
}

pub fn move_path(src: &Path, dst: &Path) -> io::Result<()> {
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    // Different drive: copy then remove.
    copy_recursive(src, dst)?;
    remove_path(src)
}

pub fn remove_path(p: &Path) -> io::Result<()> {
    if fs::symlink_metadata(p)?.is_dir() {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
}

/// Copy or move `paths` into `dest`. Runs on a worker thread.
pub fn paste(paths: &[PathBuf], dest: &Path, cut: bool) -> Result<String, String> {
    let mut n = 0;
    for src in paths {
        let Some(name) = src.file_name() else {
            continue;
        };
        if dest.starts_with(src) {
            return Err(format!(
                "Cannot paste a folder into itself: {}",
                src.display()
            ));
        }
        if cut && src.parent() == Some(dest) {
            continue; // moving into the same folder = no-op
        }
        let target = unique_dest(dest, name);
        let r = if cut {
            move_path(src, &target)
        } else {
            copy_recursive(src, &target)
        };
        r.map_err(|e| format!("{}: {e}", src.display()))?;
        n += 1;
    }
    Ok(format!(
        "{} {n} item(s)",
        if cut { "Moved" } else { "Copied" }
    ))
}

pub fn delete_permanently(paths: &[PathBuf]) -> Result<String, String> {
    for p in paths {
        remove_path(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(format!("Deleted {} item(s)", paths.len()))
}
