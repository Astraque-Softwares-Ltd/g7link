// SPDX-License-Identifier: GPL-3.0-or-later
//! New builds, passed between paired devices (`device.update`). The PC is
//! plugged in and checks the feeds on a schedule; when it sees a build, it
//! tells its phones, so they needn't check on their own. A phone that heard
//! first tells the PC the same way. Only a hint: whoever hears it checks the
//! channel's feed itself, and installs nothing a feed doesn't list.
//!
//! A paired phone whose G7 Link is too old to connect is named in a desktop
//! notification: "Update G7 Link on <phone>".

use crate::i18n::tr;
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

use g7link_core::{DeviceId, LinkState, NodeEvent};

use crate::{
    core_host,
    sys::toast::{self, Toast},
    version::{self, Channel},
};

/// The toast "device" for "Update G7 Link on <phone>"; the key is the phone's ID.
pub const TOAST_GROUP: &str = "peer_update";

/// The newest version seen per channel, so each is passed on once.
static SEEN: Mutex<Option<HashMap<Channel, String>>> = Mutex::new(None);
/// Devices told to update G7 Link, until they next connect.
static TOLD: Mutex<Option<HashSet<DeviceId>>> = Mutex::new(None);

/// A feed showed `version` on `channel`: tell paired devices, once per version.
pub fn seen(channel: Channel, version: &str) {
    {
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        let seen = seen.get_or_insert_default();
        if seen.get(&channel).is_some_and(|known| !version::is_newer(version, known)) {
            return;
        }
        seen.insert(channel, version.to_owned());
    }
    let Some(node) = core_host::node() else { return };
    let (channel, version) = (channel.as_str().to_owned(), version.to_owned());
    core_host::spawn(async move { node.announce_update(channel, version).await });
}

/// Follows update hints, and paired phones too old to connect.
pub fn on_event(event: &NodeEvent) {
    if let NodeEvent::UpdateHint { device, channel, version } = event {
        hint(*device, channel, version);
    }
    let notice = {
        let mut told = TOLD.lock().unwrap_or_else(|e| e.into_inner());
        notice(told.get_or_insert_default(), event)
    };
    match notice {
        Some(Notice::Show(device)) => show_needs_update(device),
        Some(Notice::Withdraw(device)) => toast::remove(TOAST_GROUP, &device.to_string()),
        None => {}
    }
}

/// A paired device heard of a build: check now if it's for this install's
/// channel and newer than what runs here (the updater spaces checks out).
fn hint(device: DeviceId, channel: &str, version: &str) {
    let Some(channel) = Channel::parse(channel) else { return };
    if wanted(crate::updater::channel(), channel, version, env!("CARGO_PKG_VERSION")) {
        tracing::info!(from = %device.short(), channel = channel.as_str(), version, "heard of a new build");
        crate::updater::check_soon();
    }
}

/// Whether a hint of `version` on `heard` is worth a check for an install
/// following `follows` that runs `current`: a nightly install takes releases
/// too, a stable one only releases.
fn wanted(follows: Channel, heard: Channel, version: &str, current: &str) -> bool {
    (heard == Channel::Stable || follows == Channel::Nightly) && version::is_newer(version, current)
}

/// What to do with the "Update G7 Link" notification after an event.
#[derive(Debug, PartialEq, Eq)]
enum Notice {
    Show(DeviceId),
    Withdraw(DeviceId),
}

/// Says it once per device until that device connects again: a phone left
/// on an old build keeps trying, and the core reports each try once a minute.
/// Connecting means it was updated, so the notification goes.
fn notice(told: &mut HashSet<DeviceId>, event: &NodeEvent) -> Option<Notice> {
    match event {
        NodeEvent::PeerNeedsUpdate { device } => told.insert(*device).then_some(Notice::Show(*device)),
        NodeEvent::LinkChanged { device, link: LinkState::Online { .. } }
        | NodeEvent::DeviceRemoved(device) => told.remove(device).then_some(Notice::Withdraw(*device)),
        _ => None,
    }
}

fn show_needs_update(device: DeviceId) {
    let title = match core_host::host().hub.read(|s| s.name_of(&device)) {
        Some(name) => tr!("UpdateRelay", "Update G7 Link on %1", name),
        None => tr!("UpdateRelay", "Update G7 Link on your phone"),
    };
    tracing::info!(device = %device.short(), "a paired device needs a newer G7 Link to connect");
    toast::show(Toast {
        device: TOAST_GROUP.into(),
        key: device.to_string(),
        title,
        body: tr!("UpdateRelay", "Its version is too old to connect to this PC."),
        attribution: "G7 Link".into(),
        icon: None,
        image: None,
        actions: Vec::new(),
        reply: None,
        silent: true,
        progress: None,
        call: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_count_for_their_channel_and_newer_builds_only() {
        // A stable install wants new releases, not nightlies.
        assert!(wanted(Channel::Stable, Channel::Stable, "0.1.5", "0.1.4"));
        assert!(!wanted(Channel::Stable, Channel::Nightly, "0.1.5-nightly.3", "0.1.4"));
        // A nightly install wants both.
        assert!(wanted(Channel::Nightly, Channel::Nightly, "0.1.5-nightly.3", "0.1.5-nightly.2"));
        assert!(wanted(Channel::Nightly, Channel::Stable, "0.1.5", "0.1.5-nightly.9"));
        // Nothing older, the same, or not a version.
        assert!(!wanted(Channel::Nightly, Channel::Nightly, "0.1.5-nightly.2", "0.1.5-nightly.2"));
        assert!(!wanted(Channel::Stable, Channel::Stable, "0.1.3", "0.1.4"));
        assert!(!wanted(Channel::Stable, Channel::Stable, "soon", "0.1.4"));
    }

    fn online(device: DeviceId) -> NodeEvent {
        NodeEvent::LinkChanged {
            device,
            link: LinkState::Online { path: g7link_core::ConnectionPath::Lan, rtt_ms: 4 },
        }
    }

    #[test]
    fn a_phone_too_old_to_connect_is_told_once_until_it_connects() {
        let (pixel, tablet) = (DeviceId([1; 32]), DeviceId([2; 32]));
        let mut told = HashSet::new();
        let needs = |device| NodeEvent::PeerNeedsUpdate { device };
        assert_eq!(notice(&mut told, &needs(pixel)), Some(Notice::Show(pixel)));
        // The core says it again a minute later: not shown again.
        assert_eq!(notice(&mut told, &needs(pixel)), None);
        // Each device has its own.
        assert_eq!(notice(&mut told, &needs(tablet)), Some(Notice::Show(tablet)));
        // Nothing to take back from a phone that wasn't told, or on going offline.
        assert_eq!(notice(&mut told, &online(DeviceId([3; 32]))), None);
        let offline = NodeEvent::LinkChanged { device: pixel, link: LinkState::Offline { last_seen: None } };
        assert_eq!(notice(&mut told, &offline), None);
        // Updated and connected: the notification goes, and a later rollback is told again.
        assert_eq!(notice(&mut told, &online(pixel)), Some(Notice::Withdraw(pixel)));
        assert_eq!(notice(&mut told, &online(pixel)), None);
        assert_eq!(notice(&mut told, &needs(pixel)), Some(Notice::Show(pixel)));
        // Unpaired: nothing left to update.
        assert_eq!(notice(&mut told, &NodeEvent::DeviceRemoved(tablet)), Some(Notice::Withdraw(tablet)));
        assert_eq!(notice(&mut told, &NodeEvent::DeviceRemoved(tablet)), None);
    }
}
