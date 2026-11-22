// SPDX-License-Identifier: GPL-3.0-or-later
//! A dev server an agent started (phase 5 §V9.2): one line above the open
//! thread's composer, "A server is listening on port 5173", with "Open in
//! browser" on this PC and "Open on Pixel" (the phone's own link sheet asks
//! first, `link.open`).
//!
//! The host lists the ports its thread's processes listen on (`ports` on
//! the thread). A port bound to this PC's loopback only answers here: the
//! line says so and offers no phone. A phone is offered only on a direct
//! network or tailnet link (not the relay, not USB), with links to the phone on.
//! Nothing is probed, tunnelled or forwarded.

use g7_agents::api::{MIN_PORT, Reach, ThreadInfo};
use g7link_core::{DeviceId, FeatureState, LinkState, Node};
use serde_json::{Value, json};

use super::{OpenKind, lock};
use crate::{core_host, i18n::tr};

/// The feature that opens a link on the phone.
const LINKS_TO_PHONE: &str = "device.links_to_phone";

// ---- The rules (pure) ----

/// A port a thread's processes listen on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Server {
    pub port: u16,
    /// It answers on the network, not only on this PC's loopback.
    pub lan: bool,
    /// When the host first saw it, Unix milliseconds.
    pub since_ms: u64,
}

/// The ports `info`'s processes listen on, as the host lists them.
pub(crate) fn of(info: &ThreadInfo) -> Vec<Server> {
    info.ports
        .iter()
        .map(|p| Server { port: p.port, lan: p.reach == Reach::Lan, since_ms: p.since_ms })
        .collect()
}

/// The server the line names: the one that started last (the lowest port
/// among those that started together).
pub(crate) fn newest(servers: &[Server]) -> Option<Server> {
    servers.iter().copied().max_by(|a, b| a.since_ms.cmp(&b.since_ms).then(b.port.cmp(&a.port)))
}

/// A phone can open it: connected directly on the network or its tailnet,
/// with links to the phone on. (The core then says whether this PC has an
/// address on that path the phone can open.)
pub(crate) fn reaches(link: &LinkState, links_to_phone: bool) -> bool {
    links_to_phone && matches!(link, LinkState::Online { path, .. } if path.is_direct_ip())
}

/// The line over the composer: `{ port, lan, phones: [{ id, name }] }`, or
/// `{}` while nothing listens. Phones are offered only a server that
/// answers on the network.
pub(crate) fn line(servers: &[Server], phones: &[(DeviceId, String)]) -> Value {
    let Some(server) = newest(servers) else { return json!({}) };
    let phones: Vec<Value> = if server.lan {
        phones.iter().map(|(id, name)| json!({ "id": id.to_string(), "name": name })).collect()
    } else {
        Vec::new()
    };
    json!({ "port": server.port, "lan": server.lan, "phones": phones })
}

/// Where this PC's browser opens it.
pub(crate) fn browser_url(port: u16) -> String {
    format!("http://localhost:{port}/")
}

/// `port` as the page names it, when the open thread's processes listen on
/// it (`lan`: and it answers on the network). The page never opens a port
/// the host didn't list.
pub(crate) fn listed(servers: &[Server], port: i32, lan: bool) -> Option<u16> {
    let port = u16::try_from(port).ok()?;
    servers.iter().find(|s| s.port == port && (s.lan || !lan)).map(|s| s.port)
}

// ---- This PC ----

/// Of `candidates` (the phones a thread can be handed to), the ones that can
/// open a server now, in the same order: connected directly on the network,
/// with links to the phone on, and an address of this PC they can open.
pub(crate) fn phones(candidates: &[(DeviceId, String)]) -> Vec<(DeviceId, String)> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let Some(host) = core_host::try_host() else { return Vec::new() };
    let Some(node) = core_host::node() else { return Vec::new() };
    let near: Vec<(DeviceId, String)> = host.hub.read(|s| {
        candidates
            .iter()
            .filter(|(id, _)| {
                let links =
                    s.matrices.get(id).and_then(|m| m.state(LINKS_TO_PHONE)) == Some(FeatureState::Available);
                s.devices.iter().find(|d| d.id == *id).is_some_and(|d| reaches(&d.link, links))
            })
            .cloned()
            .collect()
    });
    near.into_iter().filter(|(id, _)| phone_url(&node, *id, MIN_PORT).is_some()).collect()
}

/// The open thread's servers.
fn open_servers() -> Vec<Server> {
    let s = lock();
    s.open
        .as_ref()
        .filter(|o| o.kind == OpenKind::Thread)
        .and_then(|o| s.threads.get(&o.id))
        .map(of)
        .unwrap_or_default()
}

/// "Open in browser": the open thread's server `port`, on this PC.
pub fn open_in_browser(port: i32) {
    if let Some(port) = listed(&open_servers(), port, false) {
        super::open_link(browser_url(port));
    }
}

/// "Open on <phone>": the open thread's server `port` on `device` (its id,
/// as the line lists it). The phone asks before it opens it.
pub fn open_on(port: i32, device: String) {
    let Some(port) = listed(&open_servers(), port, true) else { return };
    let Ok(device) = device.parse::<DeviceId>() else { return };
    let Some(node) = core_host::node() else { return };
    core_host::spawn(async move {
        let can = phones(&super::handoff::phones()).iter().any(|(d, _)| *d == device);
        let url = if can { phone_url(&node, device, port) } else { None };
        let sent = match url {
            Some(url) => node
                .open_link(device, url)
                .await
                .inspect_err(|e| tracing::debug!(error = %e, "the link didn't reach the phone"))
                .is_ok(),
            None => false,
        };
        if !sent {
            let name = super::device_name(&device);
            crate::bridge::app::show_message(tr!("Agents", "%1 isn't reachable.", name));
        }
    });
}

/// `http://<this PC's address on its link to device>:<port>/`, only while
/// that link is a direct network path.
fn phone_url(node: &Node, device: DeviceId, port: u16) -> Option<String> {
    node.agent_port_url_for_phone(device, port)
}

#[cfg(test)]
mod tests {
    use g7link_core::ConnectionPath;

    use super::*;

    fn server(port: u16, lan: bool, since_ms: u64) -> Server {
        Server { port, lan, since_ms }
    }

    fn phone(n: u8) -> DeviceId {
        DeviceId([n; 32])
    }

    #[test]
    fn the_line_names_the_server_that_started_last() {
        let servers = [server(3000, true, 10), server(5173, true, 20), server(8080, false, 15)];
        assert_eq!(newest(&servers), Some(server(5173, true, 20)));
        // Started together: the lowest port.
        assert_eq!(newest(&[server(5174, true, 5), server(5173, true, 5)]), Some(server(5173, true, 5)));
        assert_eq!(newest(&[]), None);
        assert_eq!(line(&[], &[(phone(1), "Pixel".into())]), json!({}), "nothing listens: no line");
    }

    #[test]
    fn a_server_on_the_network_is_offered_to_the_phones() {
        let phones = [(phone(1), "Pixel".to_owned())];
        assert_eq!(
            line(&[server(5173, true, 1)], &phones),
            json!({ "port": 5173, "lan": true, "phones": [{ "id": phone(1).to_string(), "name": "Pixel" }] })
        );
    }

    #[test]
    fn a_server_on_loopback_only_is_offered_to_no_phone() {
        let phones = [(phone(1), "Pixel".to_owned())];
        assert_eq!(
            line(&[server(5173, false, 1)], &phones),
            json!({ "port": 5173, "lan": false, "phones": [] })
        );
    }

    #[test]
    fn only_a_direct_network_link_with_links_on_reaches_the_phone() {
        let online = |path| LinkState::Online { path, rtt_ms: 4 };
        assert!(reaches(&online(ConnectionPath::Lan), true));
        assert!(!reaches(&online(ConnectionPath::Lan), false), "links to the phone are off");
        assert!(reaches(&online(ConnectionPath::Tailscale), true), "a direct path over the tailnet");
        assert!(!reaches(&online(ConnectionPath::Relay), true), "never through the relay");
        assert!(!reaches(&online(ConnectionPath::Usb), true), "never over USB");
        assert!(!reaches(&LinkState::Connecting, true));
        assert!(!reaches(&LinkState::Offline { last_seen: None }, true));
    }

    #[test]
    fn the_page_opens_only_a_port_the_host_listed() {
        let servers = [server(5173, true, 1), server(8080, false, 2)];
        assert_eq!(listed(&servers, 5173, true), Some(5173));
        assert_eq!(listed(&servers, 8080, false), Some(8080), "this PC's browser reaches loopback");
        assert_eq!(listed(&servers, 8080, true), None, "a phone can't reach loopback");
        assert_eq!(listed(&servers, 22, false), None);
        assert_eq!(listed(&servers, -1, false), None);
        assert_eq!(listed(&servers, 70_000, false), None);
        assert_eq!(browser_url(5173), "http://localhost:5173/");
    }
}
