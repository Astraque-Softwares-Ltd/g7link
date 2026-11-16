// SPDX-License-Identifier: GPL-3.0-or-later
//! Small files written whole or not at all, and read without losing one that
//! can't be parsed.

use std::{
    ffi::OsString,
    fs,
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Serialize, de::DeserializeOwned};

/// Writes `bytes` to `path` through a new temporary file of its own beside it,
/// synced before it replaces `path`. Only this user can read the file.
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().ok_or_else(|| io::Error::other("no file name"))?;
    let mut tmp_name = OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".{}.{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let tmp = path.with_file_name(tmp_name);
    let written = (|| {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Writes `value` to `path` as JSON, as [`write`] does.
pub fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    write(path, &serde_json::to_vec(value).map_err(io::Error::other)?)
}

/// `path` parsed as JSON, or the default when there is no such file. A file
/// that can't be read or parsed is moved aside to `<name>.unreadable` first,
/// so the next write can't lose it.
pub fn read_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            tracing::warn!(%error, file = %path.display(), "can't parse; keeping it aside");
            keep_aside(path);
            T::default()
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => T::default(),
        Err(error) => {
            tracing::warn!(%error, file = %path.display(), "can't read; keeping it aside");
            keep_aside(path);
            T::default()
        }
    }
}

fn aside_path(path: &Path) -> PathBuf {
    let mut aside = path.as_os_str().to_owned();
    aside.push(".unreadable");
    PathBuf::from(aside)
}

fn keep_aside(path: &Path) {
    if let Err(error) = fs::rename(path, aside_path(path)) {
        tracing::warn!(%error, file = %path.display(), "can't keep an unreadable file aside");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn a_written_file_reads_back_and_leaves_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phones.json");
        let map = HashMap::from([("a".to_owned(), "b".to_owned())]);
        write_json(&path, &map).unwrap();
        write_json(&path, &map).unwrap();
        assert_eq!(read_json::<HashMap<String, String>>(&path), map);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_json::<HashMap<String, String>>(&dir.path().join("none.json")).is_empty());
    }

    #[test]
    fn a_corrupt_file_is_kept_aside_before_anything_overwrites_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phones.json");
        fs::write(&path, b"{\"a\": ").unwrap();
        assert!(read_json::<HashMap<String, String>>(&path).is_empty());
        assert_eq!(fs::read(aside_path(&path)).unwrap(), b"{\"a\": ");
        write_json(&path, &HashMap::from([("c", "d")])).unwrap();
        assert_eq!(fs::read(aside_path(&path)).unwrap(), b"{\"a\": ");
    }
}
