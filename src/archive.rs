//! Packing and unpacking archives: with 7-Zip when it is installed (its own
//! progress window, password and overwrite questions), otherwise with the
//! tar.exe that ships with Windows (zip, tar, tar.gz/bz2/xz, 7z, rar, iso…).

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const SINGLE: &[&str] = &[
    "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "tbz", "tbz2", "xz", "txz", "zst", "tzst", "lz",
    "lzma", "z", "cab", "iso", "wim", "arj", "lzh", "lha", "cpio", "rpm", "deb", "dmg", "vhd",
    "vhdx", "xar", "squashfs", "001",
];
/// Compressed tarballs: unpacked in one step (7-Zip would leave the .tar behind).
const TARBALL: &[&str] = &[
    ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tar.lz", ".tar.z", ".tgz", ".tbz", ".tbz2",
    ".txz", ".tzst",
];

fn lower_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

pub fn is_archive(p: &Path) -> bool {
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    SINGLE.contains(&ext.as_str()) && p.is_file()
}

fn is_tarball(p: &Path) -> bool {
    let n = lower_name(p);
    TARBALL.iter().any(|t| n.ends_with(t))
}

/// "photos.tar.gz" → "photos", "a.zip" → "a".
pub fn base_name(p: &Path) -> String {
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let low = name.to_lowercase();
    for t in TARBALL {
        if low.ends_with(t) && name.len() > t.len() {
            return name[..name.len() - t.len()].to_string();
        }
    }
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(name)
}

/// 7-Zip's folder, if it is installed.
pub fn seven_zip() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};
        use winreg::RegKey;
        let mut cands: Vec<PathBuf> = Vec::new();
        for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
            if let Ok(k) = RegKey::predef(hive).open_subkey_with_flags("SOFTWARE\\7-Zip", KEY_READ | KEY_WOW64_64KEY) {
                for v in ["Path64", "Path"] {
                    if let Ok(s) = k.get_value::<String, _>(v) {
                        cands.push(PathBuf::from(s));
                    }
                }
            }
        }
        for env in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            if let Ok(p) = std::env::var(env) {
                cands.push(PathBuf::from(p).join("7-Zip"));
            }
        }
        cands.into_iter().find(|d| d.join("7zG.exe").is_file())
    })
    .as_deref()
}

/// A path that doesn't exist yet: "name.zip", "name (2).zip", …
pub fn unique(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let dot = if ext.is_empty() { String::new() } else { format!(".{ext}") };
    let mut p = dir.join(format!("{stem}{dot}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem} ({n}){dot}"));
        n += 1;
    }
    p
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.creation_flags(CREATE_NO_WINDOW).output().map_err(|e| e.to_string())?;
    match out.status.code() {
        Some(0) | Some(1) => Ok(()), // 1 = 7-Zip warnings (e.g. a locked file skipped)
        Some(255) => Err(crate::i18n::tr("Cancelled").into()),
        _ => {
            let msg = String::from_utf8_lossy(&out.stderr);
            let msg = msg.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("").to_string();
            Err(if msg.is_empty() { crate::i18n::tr("The archive could not be processed").into() } else { msg })
        }
    }
}

fn tar() -> PathBuf {
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    PathBuf::from(sys).join("System32").join("tar.exe")
}

/// Unpack `archive` into `dest` (created if needed). Blocks until done.
pub fn extract(archive: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let res = match seven_zip() {
        Some(dir) if !is_tarball(archive) => {
            let mut o = std::ffi::OsString::from("-o");
            o.push(dest);
            run(Command::new(dir.join("7zG.exe")).arg("x").arg(archive).arg(o))
        }
        _ => run(Command::new(tar()).arg("-xf").arg(archive).arg("-C").arg(dest)),
    };
    // Don't leave an empty folder behind when nothing came out.
    if res.is_err() && std::fs::read_dir(dest).map(|mut d| d.next().is_none()).unwrap_or(false) {
        let _ = std::fs::remove_dir(dest);
    }
    res
}

/// Pack `paths` (all in the same folder) into `target`; the format comes from its extension.
pub fn compress(paths: &[PathBuf], target: &Path) -> Result<(), String> {
    let Some(parent) = paths.first().and_then(|p| p.parent()) else {
        return Err(crate::i18n::tr("Nothing selected").into());
    };
    let names: Vec<&std::ffi::OsStr> = paths.iter().filter_map(|p| p.file_name()).collect();
    let res = match seven_zip() {
        Some(dir) => run(Command::new(dir.join("7zG.exe")).current_dir(parent).arg("a").arg(target).arg("--").args(&names)),
        None => run(Command::new(tar()).current_dir(parent).arg("-a").arg("-cf").arg(target).args(&names)),
    };
    if res.is_err() {
        // A cancelled 7-Zip can leave a partial archive.
        let _ = std::fs::remove_file(target);
    }
    res
}

/// Browse the archive in 7-Zip's own window.
pub fn open_in_7zip(archive: &Path) -> Result<(), String> {
    let dir = seven_zip().ok_or_else(|| crate::i18n::tr("7-Zip is not installed").to_string())?;
    Command::new(dir.join("7zFM.exe")).arg(archive).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(base_name(Path::new("C:\\x\\photos.tar.gz")), "photos");
        assert_eq!(base_name(Path::new("C:\\x\\a.b.zip")), "a.b");
        assert!(is_tarball(Path::new("x.TGZ")));
    }

    #[test]
    fn round_trip() {
        let root = std::env::temp_dir().join(format!("delfin-arc-{}", std::process::id()));
        let src = root.join("src");
        std::fs::create_dir_all(src.join("folder ż")).unwrap();
        std::fs::write(src.join("plik ą.txt"), "hello").unwrap();
        std::fs::write(src.join("folder ż").join("b.txt"), "inner").unwrap();
        let items = vec![src.join("plik ą.txt"), src.join("folder ż")];
        for ext in ["zip", "7z"] {
            let target = unique(&root, "test", ext);
            compress(&items, &target).unwrap();
            let out = unique(&root, "out", "");
            extract(&target, &out).unwrap();
            assert_eq!(std::fs::read_to_string(out.join("plik ą.txt")).unwrap(), "hello");
            assert_eq!(std::fs::read_to_string(out.join("folder ż").join("b.txt")).unwrap(), "inner");
        }
        // The fallback without 7-Zip.
        let target = root.join("tar.zip");
        run(Command::new(tar()).current_dir(&src).arg("-a").arg("-cf").arg(&target).arg("plik ą.txt").arg("folder ż")).unwrap();
        let out = root.join("tarout");
        std::fs::create_dir_all(&out).unwrap();
        run(Command::new(tar()).arg("-xf").arg(&target).arg("-C").arg(&out)).unwrap();
        assert_eq!(std::fs::read_to_string(out.join("folder ż").join("b.txt")).unwrap(), "inner");
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Archive name for packing `paths`: the item's own name, or the folder's for several items.
pub fn pack_name(paths: &[PathBuf]) -> String {
    match paths {
        [one] => {
            if one.is_dir() {
                one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            } else {
                one.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
            }
        }
        _ => paths
            .first()
            .and_then(|p| p.parent())
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| crate::i18n::tr("Archive").into()),
    }
}
