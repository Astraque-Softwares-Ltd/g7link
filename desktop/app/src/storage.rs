// SPDX-License-Identifier: GPL-3.0-or-later
//! Phone storage in File Explorer (`docs/protocol/storage.md`, `PLAN.md` §3.6).
//!
//! Registers a Windows Cloud Files (`cfapi`) sync root per paired phone under
//! `%USERPROFILE%\G7Link\<Phone name>` when the phone's `storage` toggle is
//! enabled. Placeholders are populated on demand when folders are opened
//! (`CF_CALLBACK_TYPE_FETCH_PLACEHOLDERS`) and hydrated on open via ranged
//! reads (`CF_CALLBACK_TYPE_FETCH_DATA`). Renames, deletes, and new or modified
//! files dropped into the sync root are forwarded to the phone.

mod imp {

    #![allow(unsafe_code)]

    use std::path::{Path, PathBuf};

    use g7link_core::{DeviceId, NodeEvent};

    use crate::{core_host, win};

    /// Difference between Windows FILETIME epoch (1601-01-01) and Unix epoch
    /// (1970-01-01) in 100-nanosecond intervals.
    #[allow(dead_code)]
    const EPOCH_DIFF_100NS: i64 = 116_444_736_000_000_000;

    pub fn start() {
        win::storage::start();
    }

    pub fn sync() {
        win::storage::sync();
    }

    pub fn on_event(event: &NodeEvent) {
        win::storage::on_event(event);
    }

    pub fn shutdown() {
        win::storage::shutdown();
    }

    pub fn unregister_all() {
        win::storage::unregister_all();
    }

    /// Returns the local sync root path for a paired phone (`%USERPROFILE%\G7Link\<Phone name>`).
    pub fn sync_root_path_for(device: DeviceId) -> Option<PathBuf> {
        if let Some(path) = win::storage::mounted_path_for(&device) {
            return Some(path);
        }
        let mut folders = core_host::host()
            .hub
            .read(|s| folder_names(s.devices.iter().map(|d| (d.id, d.info.name.as_str()))));
        Some(base_sync_dir()?.join(folders.remove(&device)?))
    }

    /// Each phone's folder under `G7Link`: its name, or its name and
    /// short ID when another paired phone has the same name, so two phones
    /// of the same model never share one folder (or remove each other's).
    pub fn folder_names<'a>(
        devices: impl Iterator<Item = (DeviceId, &'a str)>,
    ) -> std::collections::HashMap<DeviceId, String> {
        let named: Vec<(DeviceId, String)> =
            devices.map(|(id, name)| (id, sanitize_folder_name(name))).collect();
        named
            .iter()
            .map(|(id, folder)| {
                let shared = named.iter().any(|(other, f)| other != id && f.eq_ignore_ascii_case(folder));
                let folder = if shared { format!("{folder} ({})", id.short()) } else { folder.clone() };
                (*id, folder)
            })
            .collect()
    }

    /// Opens a paired phone's sync root folder in File Explorer.
    pub fn open_in_explorer(device: DeviceId) -> Result<(), String> {
        let path =
            sync_root_path_for(device).ok_or_else(|| "Phone storage folder is not available".to_owned())?;
        if !path.exists() {
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        }
        win::shell::open_folder(&path)
    }

    pub(crate) fn base_sync_dir() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join("G7Link"))
    }

    /// Sanitizes a phone's display name into a valid Windows directory name.
    pub fn sanitize_folder_name(device_name: &str) -> String {
        let cleaned: String = device_name
            .chars()
            .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { ' ' } else { c })
            .collect();
        let mut name = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
        name = name.trim_end_matches(['.', ' ']).chars().take(64).collect();
        let stem = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.len() == 4
                && stem.as_bytes()[3].is_ascii_digit());
        if name.is_empty() {
            "Phone".into()
        } else if reserved {
            format!("{name} phone")
        } else {
            name
        }
    }

    /// Converts a Unix timestamp in milliseconds to Windows FILETIME 100ns ticks
    /// since 1601-01-01.
    #[allow(dead_code)]
    pub fn unix_ms_to_filetime_ticks(unix_ms: i64) -> i64 {
        EPOCH_DIFF_100NS.saturating_add(unix_ms.max(0).saturating_mul(10_000))
    }

    /// Converts Windows FILETIME 100ns ticks since 1601-01-01 to Unix milliseconds.
    #[cfg(test)]
    pub fn filetime_ticks_to_unix_ms(ticks: i64) -> i64 {
        ticks.saturating_sub(EPOCH_DIFF_100NS).max(0) / 10_000
    }

    /// Computes the slash-separated relative path inside `root_path` from a full
    /// local path (`""` for the sync root itself). Returns `None` if `full_path` is
    /// not inside `root_path`.
    #[allow(dead_code)]
    pub fn relative_path_under_root(root_path: &Path, full_path: &Path) -> Option<String> {
        let root_norm = normalize_win_path(root_path);
        let full_norm = normalize_win_path(full_path);
        // Compared name by name, ignoring case. (Cutting the path at the length
        // of a lowercased copy breaks on letters whose lowercase is longer,
        // such as the Turkish "İ" in a phone's name.)
        let root_parts: Vec<&str> = root_norm.split('\\').filter(|s| !s.is_empty()).collect();
        let full_parts: Vec<&str> = full_norm.split('\\').filter(|s| !s.is_empty()).collect();
        if full_parts.len() < root_parts.len()
            || !root_parts.iter().zip(&full_parts).all(|(a, b)| a.to_lowercase() == b.to_lowercase())
        {
            return None;
        }
        let parts = &full_parts[root_parts.len()..];
        if parts.iter().any(|p| *p == "." || *p == "..") {
            return None;
        }
        Some(parts.join("/"))
    }

    #[allow(dead_code)]
    fn normalize_win_path(path: &Path) -> String {
        let s = path.to_string_lossy();
        let stripped = s.strip_prefix(r"\\?\").unwrap_or(&s);
        stripped.trim_end_matches('\\').to_owned()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn converts_unix_ms_to_filetime_and_back() {
            let unix_ms = 1_710_000_000_123i64;
            let ticks = unix_ms_to_filetime_ticks(unix_ms);
            assert_eq!(ticks, EPOCH_DIFF_100NS + unix_ms * 10_000);
            assert_eq!(filetime_ticks_to_unix_ms(ticks), unix_ms);
            assert_eq!(unix_ms_to_filetime_ticks(-500), EPOCH_DIFF_100NS);
        }

        #[test]
        fn normalizes_relative_paths_and_sanitizes_folder_names() {
            let root = Path::new(r"C:\Users\alice\G7Link\Pixel 9");
            assert_eq!(relative_path_under_root(root, root), Some(String::new()));
            assert_eq!(
                relative_path_under_root(root, Path::new(r"C:\Users\alice\G7Link\Pixel 9\DCIM\Camera\a.jpg")),
                Some("DCIM/Camera/a.jpg".into())
            );
            assert_eq!(
                relative_path_under_root(root, Path::new(r"C:\Users\alice\G7Link\Pixel 9 Pro\DCIM")),
                None
            );
            // "İ" lowercases to two characters: the path still splits in the right place.
            let turkish = Path::new(r"C:\Users\alice\G7Link\İpek's Phone");
            assert_eq!(
                relative_path_under_root(
                    turkish,
                    Path::new(r"C:\Users\alice\G7Link\İpek's Phone\DCIM\a.jpg")
                ),
                Some("DCIM/a.jpg".into())
            );
            assert_eq!(
                relative_path_under_root(root, Path::new(r"c:\users\ALICE\g7link\pixel 9\Download")),
                Some("Download".into())
            );

            assert_eq!(sanitize_folder_name("Pixel 9 (test)"), "Pixel 9 (test)");
            assert_eq!(sanitize_folder_name("My:Phone*1?"), "My Phone 1");
            assert_eq!(sanitize_folder_name("CON"), "CON phone");
        }

        #[test]
        fn phones_with_the_same_name_get_their_own_folders() {
            let (a, b, c) = (DeviceId([1; 32]), DeviceId([2; 32]), DeviceId([3; 32]));
            let folders = folder_names([(a, "Galaxy A35"), (b, "galaxy a35"), (c, "Pixel 9")].into_iter());
            assert_eq!(folders[&a], format!("Galaxy A35 ({})", a.short()));
            assert_eq!(folders[&b], format!("galaxy a35 ({})", b.short()));
            assert_ne!(folders[&a].to_lowercase(), folders[&b].to_lowercase());
            // A phone with a name of its own keeps its plain folder.
            assert_eq!(folders[&c], "Pixel 9");
        }
    }
}

pub use imp::*;
