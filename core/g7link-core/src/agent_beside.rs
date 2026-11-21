// SPDX-License-Identifier: MPL-2.0
//! The phone beside the PC (`docs/protocol/agents.md` §11): a PC switches the
//! pane a phone shows beside a thread on the PC's screen, and a thread lists
//! the TCP ports its agent listens on (`AgentThread.ports`, which ride
//! `agent.thread` and `agent.event` as every field does).
//!
//! PC side: the platform switches one phone's pane ([`Node::agent_beside`]).
//! Phone side: a PC's `agent.beside` becomes [`NodeEvent::AgentBeside`];
//! [`Node::agent_port_url`] is where a dev server the PC lists answers for
//! this phone, over a LAN path only. Nothing here opens, forwards or
//! tunnels a port, is retried, queued while offline, or carried over
//! Bluetooth.

use std::{net::IpAddr, sync::Arc};

pub use g7link_protocol::messages::{AgentBeside, AgentPort, beside};
use g7link_protocol::{
    DeviceId, Envelope,
    messages::{agent, types},
};

use crate::{
    Node, Result, agents, dispatch::Responder, events::ConnectionPath, events::NodeEvent, node::Shared,
};

// ---- PC side ----

/// PC: sends `body` to `phone` when it gets agent pushes.
async fn send(shared: &Arc<Shared>, phone: DeviceId, body: AgentBeside) -> Result<()> {
    if !body.is_valid() || !body.is_known() {
        return Err(agents::invalid(types::AGENT_BESIDE));
    }
    let session = shared
        .agent_recipients()
        .into_iter()
        .find(|s| s.peer == phone)
        .ok_or_else(|| crate::agent_handoff::unreachable(shared, &phone))?;
    session.send(Envelope::new(types::AGENT_BESIDE, &body)?).await?;
    tracing::debug!(thread = %body.thread, pane = %body.pane, "switched a phone's pane");
    Ok(())
}

// ---- Both ----

/// Whether `t` is one of §11's messages.
pub(crate) fn is_message(t: &str) -> bool {
    t == types::AGENT_BESIDE
}

/// One of §11's messages from `peer`: a PC's `agent.beside` on a phone. A
/// PC drops one.
pub(crate) async fn handle(
    shared: &Arc<Shared>,
    peer: DeviceId,
    env: &Envelope,
    _rsp: &Responder<'_>,
) -> Result<()> {
    if env.t == types::AGENT_BESIDE {
        beside_arrived(shared, peer, env);
    }
    Ok(())
}

// ---- Phone side ----

/// Phone: a PC's `agent.beside`. One out of its limits, or with a pane this
/// build doesn't know, is dropped.
fn beside_arrived(shared: &Arc<Shared>, peer: DeviceId, env: &Envelope) {
    if !shared.we_offer(agent::REMOTE) {
        return;
    }
    let Some(body) = agents::decode::<AgentBeside>(env, AgentBeside::is_valid) else {
        tracing::debug!(peer = %peer.short(), "dropped an agent.beside out of its limits");
        return;
    };
    if !body.is_known() {
        tracing::debug!(peer = %peer.short(), "dropped an agent.beside with a pane this build doesn't know");
        return;
    }
    shared.emit(NodeEvent::AgentBeside { device: peer, thread: body.thread, pane: body.pane });
}

/// Whether a phone's browser can open `ip`: an address on this network
/// (`lan`), not loopback (the phone itself), not a link-local IPv6 one (a
/// URL can't say which interface).
fn openable(ip: IpAddr, lan: &impl Fn(IpAddr) -> bool) -> bool {
    !ip.is_loopback()
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !matches!(ip, IpAddr::V6(v6) if v6.is_unicast_link_local())
        && lan(ip)
}

/// Where port `port` of a PC answers for this phone, given the link's
/// `path` and its direct addresses (`selected`: the one in use): only over
/// a LAN path, at the address in use when it can be opened, else another
/// (IPv4 first).
fn url_for(
    path: ConnectionPath,
    addrs: &[(IpAddr, bool)],
    lan: impl Fn(IpAddr) -> bool,
    port: u16,
) -> Option<String> {
    if path != ConnectionPath::Lan || port < beside::MIN_PORT {
        return None;
    }
    let (_, ip) = addrs
        .iter()
        .map(|(ip, selected)| (ip.to_canonical(), *selected))
        .filter(|(ip, _)| openable(*ip, &lan))
        .map(|(ip, selected)| ((selected, ip.is_ipv4()), ip))
        .max_by_key(|(rank, _)| *rank)?;
    Some(match ip {
        IpAddr::V4(v4) => format!("http://{v4}:{port}/"),
        IpAddr::V6(v6) => format!("http://[{v6}]:{port}/"),
    })
}

/// Phone: [`Node::agent_port_url`].
fn port_url(shared: &Shared, pc: DeviceId, port: u16) -> Option<String> {
    let session = shared.session(&pc)?;
    let path = session.path();
    if path != ConnectionPath::Lan {
        return None;
    }
    let addrs: Vec<(IpAddr, bool)> = session
        .conn
        .paths()
        .iter()
        .filter(|p| !p.is_relay())
        .filter_map(|p| match p.remote_addr() {
            iroh::TransportAddr::Ip(addr) => Some((addr.ip(), p.is_selected())),
            _ => None,
        })
        .collect();
    url_for(path, &addrs, |ip| shared.is_lan_peer_ip(&pc, ip), port)
}

/// This PC's side of `paths` (each a direct path's remote address, this
/// PC's local address when known, and whether it is in use): the local
/// address of each path whose far end is on this network (`lan_peer`).
fn local_addrs(
    paths: &[(IpAddr, Option<IpAddr>, bool)],
    lan_peer: impl Fn(IpAddr) -> bool,
) -> Vec<(IpAddr, bool)> {
    paths
        .iter()
        .filter(|(remote, _, _)| lan_peer(*remote))
        .filter_map(|(_, local, selected)| Some(((*local)?, *selected)))
        .collect()
}

/// PC: [`Node::agent_port_url_for_phone`].
fn pc_port_url(shared: &Shared, phone: DeviceId, port: u16) -> Option<String> {
    let session = shared.session(&phone)?;
    let path = session.path();
    if path != ConnectionPath::Lan {
        return None;
    }
    let paths: Vec<(IpAddr, Option<IpAddr>, bool)> = session
        .conn
        .paths()
        .iter()
        .filter(|p| !p.is_relay())
        .filter_map(|p| {
            let iroh::TransportAddr::Ip(remote) = p.remote_addr() else { return None };
            let local = match p.local_addr() {
                iroh::endpoint::LocalTransportAddr::Ip(local) => *local,
                _ => None,
            };
            Some((remote.ip(), local, p.is_selected()))
        })
        .collect();
    let addrs = local_addrs(&paths, |ip| shared.is_lan_peer_ip(&phone, ip));
    // Each was checked through its far end.
    url_for(path, &addrs, |_| true, port)
}

/// The node's §11 methods.
impl Node {
    /// PC: shows `beside.pane` on `phone` beside `beside.thread`, which the
    /// phone says it shows (`agent.viewing`). Only a pane this build knows.
    /// `Offline`, `Denied` (its `agents` toggle is off), `NotPaired`, or
    /// `Unsupported` (this PC has no agent host, or the phone takes no agent
    /// pushes) when it can't go. Sent once, never queued.
    pub async fn agent_beside(&self, phone: DeviceId, beside: AgentBeside) -> Result<()> {
        send(self.shared(), phone, beside).await
    }

    /// Phone: where a dev server on port `port` of PC `pc` (one its threads
    /// list, `AgentThread.ports`) answers for this phone:
    /// `http://<the PC's address on this link>:<port>/`. `None` unless the
    /// link to `pc` is up over a LAN path (not the relay, not USB) with an
    /// address a browser here can open, and for a port under 1024. Only for
    /// a `lan` port: one bound to the PC's loopback doesn't answer here.
    pub fn agent_port_url(&self, pc: DeviceId, port: u16) -> Option<String> {
        port_url(self.shared(), pc, port)
    }

    /// PC: where a dev server on this PC's `port` answers for `phone`:
    /// `http://<this PC's address on its link to phone>:<port>/`, what
    /// "Open on <phone>" hands the phone. `None` unless that link is up over
    /// a LAN path (not the relay, not USB) whose far end is on this network,
    /// with a local address the phone can open (never loopback or a
    /// link-local IPv6 one; the one in use first, then IPv4), and for a port
    /// under 1024.
    pub fn agent_port_url_for_phone(&self, phone: DeviceId, port: u16) -> Option<String> {
        pc_port_url(self.shared(), phone, port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORT: u16 = 5173;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn any(_: IpAddr) -> bool {
        true
    }

    #[test]
    fn only_the_beside_is_this_modules() {
        assert!(is_message(types::AGENT_BESIDE));
        assert!(!is_message(types::AGENT_HANDOFF));
        assert!(!is_message(types::AGENT_VIEWING));
    }

    #[test]
    fn a_port_opens_only_over_a_lan_path() {
        let lan = [(ip("192.168.1.20"), true)];
        assert_eq!(
            url_for(ConnectionPath::Lan, &lan, any, PORT).as_deref(),
            Some("http://192.168.1.20:5173/")
        );
        assert_eq!(url_for(ConnectionPath::Relay, &lan, any, PORT), None, "the relay");
        assert_eq!(url_for(ConnectionPath::Usb, &lan, any, PORT), None, "USB");
        assert_eq!(url_for(ConnectionPath::Lan, &lan, any, 80), None, "under 1024");
        assert_eq!(url_for(ConnectionPath::Lan, &[], any, PORT), None, "no direct address");
        let wan = |ip: IpAddr| ip != self::ip("203.0.113.9");
        let far = [(ip("203.0.113.9"), true)];
        assert_eq!(url_for(ConnectionPath::Lan, &far, wan, PORT), None, "a direct path across the internet");
    }

    #[test]
    fn the_address_in_use_wins_then_ipv4() {
        let both = [(ip("fd00::20"), false), (ip("192.168.1.20"), false), (ip("10.0.0.20"), true)];
        assert_eq!(url_for(ConnectionPath::Lan, &both, any, PORT).as_deref(), Some("http://10.0.0.20:5173/"));
        let unselected = [(ip("fd00::20"), false), (ip("192.168.1.20"), false)];
        assert_eq!(
            url_for(ConnectionPath::Lan, &unselected, any, PORT).as_deref(),
            Some("http://192.168.1.20:5173/")
        );
        let v6 = [(ip("fd00::20"), true)];
        assert_eq!(url_for(ConnectionPath::Lan, &v6, any, PORT).as_deref(), Some("http://[fd00::20]:5173/"));
        let mapped = [(ip("::ffff:192.168.1.20"), true)];
        assert_eq!(
            url_for(ConnectionPath::Lan, &mapped, any, PORT).as_deref(),
            Some("http://192.168.1.20:5173/")
        );
    }

    #[test]
    fn an_address_a_browser_here_cant_open_is_skipped() {
        for bad in ["127.0.0.1", "::1", "0.0.0.0", "fe80::1", "224.0.0.1"] {
            assert_eq!(url_for(ConnectionPath::Lan, &[(ip(bad), true)], any, PORT), None, "{bad}");
        }
        let link_local_first = [(ip("fe80::1"), true), (ip("192.168.1.20"), false)];
        assert_eq!(
            url_for(ConnectionPath::Lan, &link_local_first, any, PORT).as_deref(),
            Some("http://192.168.1.20:5173/")
        );
    }

    /// The phone's address on each path, this PC's, and whether it is in use.
    fn path(remote: &str, local: Option<&str>, selected: bool) -> (IpAddr, Option<IpAddr>, bool) {
        (ip(remote), local.map(ip), selected)
    }

    /// The PC's URL for `phone_paths`, with `lan_peer` saying which far ends are on this network.
    fn pc_url(
        link: ConnectionPath,
        phone_paths: &[(IpAddr, Option<IpAddr>, bool)],
        lan_peer: impl Fn(IpAddr) -> bool,
        port: u16,
    ) -> Option<String> {
        url_for(link, &local_addrs(phone_paths, lan_peer), |_| true, port)
    }

    #[test]
    fn a_pc_hands_its_own_address_on_the_link_to_the_phone() {
        let home = |ip: IpAddr| ip != self::ip("203.0.113.7");
        let lan = [path("192.168.1.50", Some("192.168.1.20"), true)];
        assert_eq!(
            pc_url(ConnectionPath::Lan, &lan, home, PORT).as_deref(),
            Some("http://192.168.1.20:5173/")
        );
        assert_eq!(pc_url(ConnectionPath::Relay, &lan, home, PORT), None, "the relay");
        assert_eq!(pc_url(ConnectionPath::Usb, &lan, home, PORT), None, "USB");
        assert_eq!(pc_url(ConnectionPath::Lan, &lan, home, 1023), None, "under 1024");
        assert_eq!(
            pc_url(ConnectionPath::Lan, &lan, home, 1024).as_deref(),
            Some("http://192.168.1.20:1024/")
        );
        // The phone reached across the internet: not a LAN path, whatever this PC's address.
        let far = [path("203.0.113.7", Some("192.168.1.20"), true)];
        assert_eq!(pc_url(ConnectionPath::Lan, &far, home, PORT), None);
        // A path whose local address the system didn't say.
        assert_eq!(pc_url(ConnectionPath::Lan, &[path("192.168.1.50", None, true)], home, PORT), None);
    }

    #[test]
    fn the_pcs_address_in_use_wins_then_ipv4_and_never_one_the_phone_cant_open() {
        let all = |_: IpAddr| true;
        let paths = [
            path("fd00::50", Some("fd00::20"), false),
            path("192.168.1.50", Some("192.168.1.20"), false),
            path("10.0.0.50", Some("10.0.0.20"), true),
        ];
        assert_eq!(pc_url(ConnectionPath::Lan, &paths, all, PORT).as_deref(), Some("http://10.0.0.20:5173/"));
        assert_eq!(
            pc_url(ConnectionPath::Lan, &paths[..2], all, PORT).as_deref(),
            Some("http://192.168.1.20:5173/")
        );
        assert_eq!(
            pc_url(ConnectionPath::Lan, &paths[..1], all, PORT).as_deref(),
            Some("http://[fd00::20]:5173/")
        );
        for bad in ["127.0.0.1", "::1", "0.0.0.0", "fe80::20", "::ffff:127.0.0.1"] {
            let paths = [path("192.168.1.50", Some(bad), true)];
            assert_eq!(pc_url(ConnectionPath::Lan, &paths, all, PORT), None, "{bad}");
        }
        let link_local_in_use =
            [path("fe80::50", Some("fe80::20"), true), path("192.168.1.50", Some("192.168.1.20"), false)];
        assert_eq!(
            pc_url(ConnectionPath::Lan, &link_local_in_use, all, PORT).as_deref(),
            Some("http://192.168.1.20:5173/")
        );
    }
}
