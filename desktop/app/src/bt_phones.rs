// SPDX-License-Identifier: GPL-3.0-or-later
//! Which system Bluetooth device each paired phone is, for call audio and for
//! keeping its media on the phone. A phone's device is found by name only until
//! its address is recorded here, and only by the name it had when first seen
//! here or its nickname: a name the phone reports later never points either at
//! another device.

use std::{
    collections::HashMap,
    path::Path,
    sync::{Mutex, OnceLock},
};

use g7link_core::DeviceId;
use serde::{Deserialize, Serialize};

use crate::{core_host, settings, sys::bt_hfp::BtPhone};

/// A paired phone as it is now.
#[derive(Debug, Clone)]
pub struct Paired {
    pub id: DeviceId,
    /// The name it reports.
    pub name: String,
    /// What it's called in messages.
    pub label: String,
    /// Its media stays on the phone rather than playing here.
    pub media_on_phone: bool,
}

/// What's kept for a phone, by its device id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Recorded {
    /// The name it had when first seen here.
    name: String,
    /// Its device's Bluetooth address, once found.
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<String>,
}

type RecordedMap = HashMap<String, Recorded>;

static RECORDED: OnceLock<Mutex<RecordedMap>> = OnceLock::new();

const FILE: &str = "bluetooth-phones.json";

/// `paired` as Bluetooth knows them.
pub fn phones(paired: &[Paired]) -> Vec<BtPhone> {
    with_recorded(|map| phones_in(map, paired, settings::nickname_for)).unwrap_or_default()
}

/// Records that the device of the phone with id `id` is at `address`.
pub fn found(id: &str, address: &str) {
    with_recorded(|map| found_in(map, id, address));
}

/// Runs `f` on what's kept, saving it when `f` changed it; `None` before the
/// app has its data folder.
fn with_recorded<T>(f: impl FnOnce(&mut RecordedMap) -> T) -> Option<T> {
    let path = core_host::try_data_dir()?.join(FILE);
    let map = RECORDED.get_or_init(|| Mutex::new(load(&path)));
    let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
    let before = map.clone();
    let out = f(&mut map);
    if *map != before {
        save(&path, &map);
    }
    Some(out)
}

fn load(path: &Path) -> RecordedMap {
    std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

fn save(path: &Path, map: &RecordedMap) {
    let tmp = path.with_extension("json.tmp");
    let saved = serde_json::to_vec(map)
        .map_err(std::io::Error::other)
        .and_then(|bytes| std::fs::write(&tmp, bytes))
        .and_then(|()| std::fs::rename(&tmp, path));
    if let Err(error) = saved {
        tracing::warn!(%error, "can't save which Bluetooth device each phone is");
    }
}

/// `paired` as Bluetooth knows them, keeping the name of a phone seen for the
/// first time.
fn phones_in(
    map: &mut RecordedMap,
    paired: &[Paired],
    nickname: impl Fn(&str) -> Option<String>,
) -> Vec<BtPhone> {
    paired
        .iter()
        .map(|phone| {
            let id = phone.id.to_string();
            let recorded = map.entry(id.clone()).or_default();
            if recorded.name.trim().is_empty() {
                recorded.name = phone.name.trim().to_owned();
            }
            let names = [Some(recorded.name.clone()), nickname(&id)]
                .into_iter()
                .flatten()
                .filter(|name| !name.trim().is_empty())
                .collect();
            BtPhone {
                label: phone.label.clone(),
                address: recorded.address.clone(),
                names,
                media_on_phone: phone.media_on_phone,
                id,
            }
        })
        .collect()
}

/// Records `address` for `id`, unless it already has one or another phone has
/// this one.
fn found_in(map: &mut RecordedMap, id: &str, address: &str) {
    if map.values().any(|r| r.address.as_ref().is_some_and(|a| a.eq_ignore_ascii_case(address))) {
        return;
    }
    if let Some(recorded) = map.get_mut(id)
        && recorded.address.is_none()
    {
        recorded.address = Some(address.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paired(id: DeviceId, name: &str) -> Paired {
        Paired { id, name: name.into(), label: name.into(), media_on_phone: true }
    }

    #[test]
    fn a_phone_renaming_itself_keeps_the_name_it_was_first_seen_by() {
        let id = DeviceId([1; 32]);
        let mut map = RecordedMap::new();
        let first = phones_in(&mut map, &[paired(id, "Pixel 8 Pro")], |_| None);
        assert_eq!(first[0].names, ["Pixel 8 Pro"]);

        let renamed = phones_in(&mut map, &[paired(id, "Galaxy S24")], |_| Some("Work phone".into()));
        assert_eq!(renamed[0].names, ["Pixel 8 Pro", "Work phone"]);
        assert_eq!(renamed[0].label, "Galaxy S24");
        assert_eq!(renamed[0].address, None);
    }

    #[test]
    fn an_address_is_recorded_once_and_for_one_phone() {
        let (a, b) = (DeviceId([1; 32]), DeviceId([2; 32]));
        let mut map = RecordedMap::new();
        phones_in(&mut map, &[paired(a, "Pixel 8 Pro"), paired(b, "Galaxy S24")], |_| None);

        found_in(&mut map, &a.to_string(), "AA:BB:CC:DD:EE:01");
        found_in(&mut map, &a.to_string(), "AA:BB:CC:DD:EE:02");
        found_in(&mut map, &b.to_string(), "aa:bb:cc:dd:ee:01");
        found_in(&mut map, "unknown", "AA:BB:CC:DD:EE:03");

        let phones = phones_in(&mut map, &[paired(a, "Pixel 8 Pro"), paired(b, "Galaxy S24")], |_| None);
        assert_eq!(phones[0].address.as_deref(), Some("AA:BB:CC:DD:EE:01"));
        assert_eq!(phones[1].address, None);
        assert_eq!(map.len(), 2);
    }
}
