// SPDX-License-Identifier: GPL-3.0-or-later
//! Which system Bluetooth device each paired phone is, for call audio and for
//! keeping its media on the phone. A phone's device is found by name only until
//! its address is recorded here, and only by the exact name it had when first
//! seen here or its nickname: a name the phone reports later never points
//! either at another device. An address is recorded only once the user
//! connected call audio to it, and a device the user said isn't the phone is
//! never matched again.

use std::{
    collections::HashMap,
    path::Path,
    sync::{Mutex, OnceLock},
};

use g7link_core::DeviceId;
use serde::{Deserialize, Serialize};

use crate::{atomic_file, core_host, settings, sys::bt_hfp::BtPhone};

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
    /// Devices the user said aren't this phone, by address.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    not: Vec<String>,
}

/// Devices kept as "not this phone", per phone; the oldest go first.
const MAX_NOT: usize = 8;

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

/// The device at `address` isn't the phone with id `id`: it's never matched
/// to it again.
pub fn not_this(id: &str, address: &str) {
    with_recorded(|map| not_in(map, id, address));
}

/// Forgets the phone with id `id`, once it's unpaired.
pub fn forget(id: &str) {
    with_recorded(|map| map.remove(id));
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
    atomic_file::read_json(path)
}

fn save(path: &Path, map: &RecordedMap) {
    if let Err(error) = atomic_file::write_json(path, map) {
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
                not: recorded.not.clone(),
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
        recorded.not.retain(|a| !a.eq_ignore_ascii_case(address));
    }
}

/// Unrecords `address` for `id` and keeps it as a device that isn't `id`.
fn not_in(map: &mut RecordedMap, id: &str, address: &str) {
    let Some(recorded) = map.get_mut(id) else { return };
    if recorded.address.as_ref().is_some_and(|a| a.eq_ignore_ascii_case(address)) {
        recorded.address = None;
    }
    if !recorded.not.iter().any(|a| a.eq_ignore_ascii_case(address)) {
        recorded.not.push(address.to_owned());
    }
    let over = recorded.not.len().saturating_sub(MAX_NOT);
    recorded.not.drain(..over);
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

    #[test]
    fn a_device_said_not_to_be_the_phone_is_unrecorded_and_kept_out() {
        let id = DeviceId([1; 32]);
        let mut map = RecordedMap::new();
        phones_in(&mut map, &[paired(id, "Pixel 8")], |_| None);
        found_in(&mut map, &id.to_string(), "AA:BB:CC:DD:EE:01");

        not_in(&mut map, &id.to_string(), "aa:bb:cc:dd:ee:01");
        let phones = phones_in(&mut map, &[paired(id, "Pixel 8")], |_| None);
        assert_eq!(phones[0].address, None);
        assert_eq!(phones[0].not, ["aa:bb:cc:dd:ee:01"]);

        // Connecting to another device records that one.
        found_in(&mut map, &id.to_string(), "AA:BB:CC:DD:EE:02");
        let phones = phones_in(&mut map, &[paired(id, "Pixel 8")], |_| None);
        assert_eq!(phones[0].address.as_deref(), Some("AA:BB:CC:DD:EE:02"));

        for n in 0..20 {
            not_in(&mut map, &id.to_string(), &format!("AA:BB:CC:DD:EF:{n:02}"));
        }
        let phones = phones_in(&mut map, &[paired(id, "Pixel 8")], |_| None);
        assert_eq!(phones[0].not.len(), MAX_NOT);
        assert_eq!(phones[0].not.last().map(String::as_str), Some("AA:BB:CC:DD:EF:19"));
    }

    #[test]
    fn a_forgotten_phone_starts_over() {
        let id = DeviceId([1; 32]);
        let mut map = RecordedMap::new();
        phones_in(&mut map, &[paired(id, "Pixel 8")], |_| None);
        found_in(&mut map, &id.to_string(), "AA:BB:CC:DD:EE:01");
        map.remove(&id.to_string());
        let phones = phones_in(&mut map, &[paired(id, "Galaxy S24")], |_| None);
        assert_eq!(phones[0].names, ["Galaxy S24"]);
        assert_eq!(phones[0].address, None);
    }
}
