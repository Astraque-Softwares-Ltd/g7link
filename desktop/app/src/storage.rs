// SPDX-License-Identifier: GPL-3.0-or-later
//! Phone storage in the file manager (`docs/protocol/storage.md`, `PLAN.md` §3.6).
//!
//! Mounts each paired phone at `~/G7Link/<Phone name>` (FUSE, see
//! `sys::storage`) when the phone's `storage` toggle is enabled. Folders are
//! listed and files read from the phone on demand; renames, deletes, and new
//! or modified files are forwarded to the phone.

mod imp {

    #![allow(unsafe_code)]

    use std::path::PathBuf;

    use g7link_core::{DeviceId, NodeEvent};

    use crate::{core_host, sys};

    pub fn start() {
        sys::storage::start();
    }

    pub fn sync() {
        sys::storage::sync();
    }

    pub fn on_event(event: &NodeEvent) {
        sys::storage::on_event(event);
    }

    pub fn shutdown() {
        sys::storage::shutdown();
    }

    pub fn unregister_all() {
        sys::storage::unregister_all();
    }

    /// Returns the local sync root path for a paired phone (`%USERPROFILE%\G7Link\<Phone name>`).
    pub fn sync_root_path_for(device: DeviceId) -> Option<PathBuf> {
        if let Some(path) = sys::storage::mounted_path_for(&device) {
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

    /// Opens a paired phone's storage folder in the file manager.
    pub fn open_in_explorer(device: DeviceId) -> Result<(), String> {
        let path =
            sync_root_path_for(device).ok_or_else(|| "Phone storage folder is not available".to_owned())?;
        if !path.exists() {
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        }
        sys::shell::open_folder(&path)
    }

    pub(crate) fn base_sync_dir() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join("G7Link"))
    }

    /// Sanitizes a phone's display name into a valid directory name.
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn sanitizes_folder_names() {
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
