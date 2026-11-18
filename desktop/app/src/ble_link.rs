// SPDX-License-Identifier: GPL-3.0-or-later
//! When a paired phone can connect to this PC over Bluetooth
//! (docs/protocol/bluetooth.md): only in short windows, because a connectable
//! advertisement shows the PC's address. One opens when the core has
//! something for a phone that isn't connected, when a phone asks for one
//! (`ble.use`), when a phone's connection just ended, and at start; the last
//! two at most once a minute. A Bluetooth link ends a little after its phone
//! is back on the network, unless the phone holds it for its touchpad.

use std::{
    collections::HashSet,
    sync::Mutex,
    time::{Duration, Instant},
};

use g7link_core::{DeviceId, LinkState, Node, NodeEvent};

use crate::{core_host, sys::ble_beacon};

/// How long a phone the core asked for has to connect.
const WANTED_FOR: Duration = Duration::from_secs(30);
/// How long a window nobody asked for stays open.
const UNSOLICITED_FOR: Duration = Duration::from_secs(45);
/// Unsolicited windows at most this often.
const UNSOLICITED_GAP: Duration = Duration::from_secs(60);
/// A Bluetooth link ends this long after its phone is back on the network,
/// or after the phone stops holding it.
const CLOSE_AFTER_ONLINE: Duration = Duration::from_secs(10);

/// The phones connected over the network now.
static ONLINE: Mutex<Option<HashSet<DeviceId>>> = Mutex::new(None);
static LAST_UNSOLICITED: Mutex<Option<Instant>> = Mutex::new(None);

fn online<T>(f: impl FnOnce(&mut HashSet<DeviceId>) -> T) -> T {
    f(ONLINE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default())
}

/// Whether an unsolicited window may open at `now`, after one at `last`;
/// it's recorded when it may.
fn unsolicited_allowed(last: &mut Option<Instant>, now: Instant) -> bool {
    if last.is_some_and(|at| now.saturating_duration_since(at) < UNSOLICITED_GAP) {
        return false;
    }
    *last = Some(now);
    true
}

/// Opens a window for those of `devices` that may use Bluetooth and have no link.
fn open_for(node: &Node, devices: &[DeviceId], open: Duration, unsolicited: bool) {
    let keys: Vec<[u8; 32]> = node
        .ble_beacon_keys()
        .into_iter()
        .filter(|(id, _)| devices.contains(id) && node.ble_usable(*id) && !node.ble_linked(*id))
        .map(|(_, key)| key)
        .collect();
    if keys.is_empty() {
        return;
    }
    if unsolicited
        && !unsolicited_allowed(
            &mut LAST_UNSOLICITED.lock().unwrap_or_else(|e| e.into_inner()),
            Instant::now(),
        )
    {
        return;
    }
    ble_beacon::open_window(keys, open);
}

/// Whether a phone's link should end: it's back on the network and doesn't
/// hold the link for its touchpad. With no network the link is the only way.
fn link_not_needed(online: bool, linked: bool, held: bool) -> bool {
    online && linked && !held
}

/// Ends `device`'s link after [`CLOSE_AFTER_ONLINE`], if it still isn't needed then.
fn close_later(node: Node, device: DeviceId) {
    core_host::spawn(async move {
        tokio::time::sleep(CLOSE_AFTER_ONLINE).await;
        let online = online(|set| set.contains(&device));
        if link_not_needed(online, node.ble_linked(device), node.ble_held(device)) {
            node.close_ble_link(device);
        }
    });
}

pub fn on_event(event: &NodeEvent) {
    match event {
        NodeEvent::BleWanted { device } => {
            if let Some(node) = core_host::node() {
                open_for(&node, &[*device], WANTED_FOR, false);
            }
        }
        NodeEvent::BleLinkChanged { device, up: true } => {
            ble_beacon::close_window();
            let device = *device;
            let Some(node) = core_host::node() else { return };
            if link_not_needed(online(|set| set.contains(&device)), true, node.ble_held(device)) {
                close_later(node, device);
            }
        }
        NodeEvent::BleHoldChanged { device, held: false } => {
            let device = *device;
            let Some(node) = core_host::node() else { return };
            if link_not_needed(online(|set| set.contains(&device)), node.ble_linked(device), false) {
                close_later(node, device);
            }
        }
        NodeEvent::LinkChanged { device, link } => {
            let device = *device;
            let now_online = matches!(link, LinkState::Online { .. });
            let was_online = online(|set| if now_online { !set.insert(device) } else { set.remove(&device) });
            let Some(node) = core_host::node() else { return };
            if now_online
                && !was_online
                && link_not_needed(true, node.ble_linked(device), node.ble_held(device))
            {
                close_later(node, device);
            } else if !now_online && was_online {
                open_for(&node, &[device], UNSOLICITED_FOR, true);
            }
        }
        _ => {}
    }
}

/// The PC's internet went: phones that may use Bluetooth and aren't
/// connected get a window, so the hotspot can be asked for.
pub fn internet_lost() {
    let Some(node) = core_host::node() else { return };
    let offline: Vec<DeviceId> = node
        .paired_devices()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| !matches!(d.link, LinkState::Online { .. }))
        .map(|d| d.id)
        .collect();
    open_for(&node, &offline, UNSOLICITED_FOR, false);
}

/// At start: phones that may use Bluetooth and aren't connected get a window.
pub fn started() {
    let Some(node) = core_host::node() else { return };
    let offline: Vec<DeviceId> = node
        .paired_devices()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| !matches!(d.link, LinkState::Online { .. }))
        .map(|d| d.id)
        .collect();
    open_for(&node, &offline, UNSOLICITED_FOR, true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_ends_once_the_phone_is_back_unless_it_holds_it() {
        assert!(link_not_needed(true, true, false));
        assert!(!link_not_needed(true, true, true), "held for the touchpad");
        assert!(!link_not_needed(false, true, false), "no network: the only way");
        assert!(!link_not_needed(true, false, false), "nothing to end");
    }

    #[test]
    fn unsolicited_windows_open_at_most_once_a_minute() {
        let start = Instant::now();
        let mut last = None;
        assert!(unsolicited_allowed(&mut last, start));
        assert!(!unsolicited_allowed(&mut last, start + Duration::from_secs(59)));
        assert!(unsolicited_allowed(&mut last, start + UNSOLICITED_GAP));
    }
}
