// SPDX-License-Identifier: GPL-3.0-or-later
//! App icons that phones send with their notifications, kept as PNG files
//! (`<data>/cache/icons`) so QML and desktop notifications can show them.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The icons saved since the app started, as a hash of their bytes by file:
/// an icon that comes again isn't read back or written again.
static SAVED: Mutex<Option<HashMap<PathBuf, u64>>> = Mutex::new(None);

fn saved<T>(f: impl FnOnce(&mut HashMap<PathBuf, u64>) -> T) -> T {
    f(SAVED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new))
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

pub fn icons_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("cache").join("icons")
}

/// A file name for a package: package names are already safe
/// (`[A-Za-z0-9._]`); anything else is replaced.
fn file_name(app: &str) -> String {
    let safe: String = app
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '_' })
        .collect();
    let safe = safe.trim_matches('.');
    format!("{}.png", if safe.is_empty() { "_" } else { safe })
}

/// Where `app`'s icon is, if this very icon was saved since the app
/// started. Doesn't touch the disk.
pub fn already_saved(data_dir: &Path, app: &str, png: &[u8]) -> Option<PathBuf> {
    let path = icons_dir(data_dir).join(file_name(app));
    let hash = fnv(png);
    saved(|s| s.get(&path) == Some(&hash)).then_some(path)
}

/// Stores an app's icon and returns its path. Only PNG data is accepted.
/// Blocking: call it off the GUI thread and the core's event loop.
pub fn save(data_dir: &Path, app: &str, png: &[u8]) -> std::io::Result<PathBuf> {
    if !png.starts_with(PNG_SIGNATURE) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "not a PNG"));
    }
    if let Some(path) = already_saved(data_dir, app, png) {
        return Ok(path);
    }
    static NEXT_TMP: AtomicU64 = AtomicU64::new(0);
    let dir = icons_dir(data_dir);
    fs::create_dir_all(&dir)?;
    let path = dir.join(file_name(app));
    // Its own temporary file: two threads may save the same app's icon.
    let tmp = path.with_extension(format!("png.{}.tmp", NEXT_TMP.fetch_add(1, Ordering::Relaxed)));
    if let Err(e) = fs::write(&tmp, png).and_then(|()| fs::rename(&tmp, &path)) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    saved(|s| s.insert(path.clone(), fnv(png)));
    Ok(path)
}

/// A `file:///` URL for QML and toasts.
pub fn file_url(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    if let Some(unc) = s.strip_prefix("//") {
        format!("file://{}", unc.trim_start_matches('/'))
    } else {
        format!("file:///{}", s.trim_start_matches('/'))
    }
}

/// Returns the cached PNG icon path for `app` if it exists on disk.
pub fn existing(data_dir: &Path, app: &str) -> Option<PathBuf> {
    let path = icons_dir(data_dir).join(file_name(app));
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_png_icons_under_safe_names() {
        let dir = tempfile::tempdir().unwrap();
        let png = [PNG_SIGNATURE, b"rest"].concat();
        let path = save(dir.path(), "com.whatsapp", &png).unwrap();
        assert_eq!(path.file_name().unwrap(), "com.whatsapp.png");
        assert_eq!(fs::read(&path).unwrap(), png);
        // Saving the same icon again is a no-op; a new one replaces it.
        assert_eq!(already_saved(dir.path(), "com.whatsapp", &png), Some(path.clone()));
        assert_eq!(save(dir.path(), "com.whatsapp", &png).unwrap(), path);
        let newer = [PNG_SIGNATURE, b"newer"].concat();
        assert_eq!(already_saved(dir.path(), "com.whatsapp", &newer), None);
        assert_eq!(save(dir.path(), "com.whatsapp", &newer).unwrap(), path);
        assert_eq!(fs::read(&path).unwrap(), newer);
        assert_eq!(already_saved(dir.path(), "com.whatsapp", &newer), Some(path.clone()));
        assert_eq!(already_saved(dir.path(), "com.whatsapp", &png), None);
        assert_eq!(fs::read_dir(icons_dir(dir.path())).unwrap().count(), 1, "no temporary files left");

        assert_eq!(file_name("../../evil\\x"), "_.._evil_x.png");
        assert!(save(dir.path(), "com.bad", b"GIF89a").is_err(), "only PNG");
        assert!(file_url(Path::new(r"C:\a\b.png")).starts_with("file:///C:/a/b.png"));
        assert_eq!(file_url(Path::new(r"\\server\share\a.png")), "file://server/share/a.png");
        assert_eq!(file_url(Path::new("/home/user/a.png")), "file:///home/user/a.png");
    }
}
