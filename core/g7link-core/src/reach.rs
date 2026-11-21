// SPDX-License-Identifier: MPL-2.0
//! Reaching a paired device away from its home network: the port a PC
//! prefers, an address the user typed for a device, and what each device
//! tells its peers about its own tailnet addresses (`device.reach`).
//!
//! Nothing here probes the network or talks to Tailscale: addresses come from
//! the user, from the peer, and from the system resolver. Every address still
//! goes through the dialer's filter (`Shared::remember_addrs`), and every
//! session still needs the paired device's key.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use g7link_protocol::{
    DeviceId, Envelope,
    messages::{DeviceReach, reach::MAX_ADDRS, types},
};
use iroh::endpoint_info::EndpointInfo;

use crate::{Error, Node, Result, node::Shared, session::Session, session::is_tailnet_ip, store::PeerRecord};

/// The UDP port a PC listens on when it's free.
pub const DEFAULT_PORT: u16 = 7718;
/// The longest address the user can type: a 253-byte name, `:65535` and brackets.
pub const MAX_ADDRESS_INPUT: usize = 261;
const MAX_NAME_BYTES: usize = 253;
const MAX_LABEL_BYTES: usize = 63;
/// Answers kept from one lookup of a typed name.
const MAX_RESOLVED_IPS: usize = 4;
/// Ports tried for a typed address without one.
const MAX_PORTS: usize = 3;
/// Addresses dialed for a typed address at most.
const MAX_MANUAL_ADDRS: usize = 8;
/// How long `set_peer_address` waits for a name's answers.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(3);
/// How often a typed name is looked up again while its device is offline.
const RESOLVE_EVERY: Duration = Duration::from_secs(60);
/// After a network change, interfaces may take a moment to get their addresses.
const REACH_SETTLE: Duration = Duration::from_secs(3);

/// The host part of a typed address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressHost {
    Ip(IpAddr),
    /// Lowercase, without a trailing dot.
    Name(String),
}

/// An address the user typed for a paired device: an IP or a name, and a
/// port if they gave one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAddress {
    pub host: AddressHost,
    pub port: Option<u16>,
}

impl PeerAddress {
    /// Reads `v4`, `v4:port`, `v6`, `[v6]`, `[v6]:port`, `name` or
    /// `name:port`. `None` for anything else: a scheme, a path, a user, a
    /// scope, whitespace inside, port 0, a loopback, unspecified, multicast or
    /// broadcast IP, or an IPv6 link-local one (it would need a scope).
    pub fn parse(input: &str) -> Option<PeerAddress> {
        let s = input.trim();
        if s.is_empty()
            || s.len() > MAX_ADDRESS_INPUT
            || s.contains(|c: char| c.is_whitespace() || matches!(c, '/' | '@' | '%'))
        {
            return None;
        }
        let (host, port) = if let Some(rest) = s.strip_prefix('[') {
            let (inside, after) = rest.split_once(']')?;
            let ip: Ipv6Addr = inside.parse().ok()?;
            let port = match after {
                "" => None,
                after => Some(parse_port(after.strip_prefix(':')?)?),
            };
            (AddressHost::Ip(IpAddr::V6(ip)), port)
        } else if let Ok(ip) = s.parse::<IpAddr>() {
            (AddressHost::Ip(ip), None)
        } else {
            let (host, port) = match s.rsplit_once(':') {
                Some((host, port)) => (host, Some(parse_port(port)?)),
                None => (s, None),
            };
            match host.parse::<Ipv4Addr>() {
                Ok(v4) => (AddressHost::Ip(IpAddr::V4(v4)), port),
                Err(_) => (AddressHost::Name(parse_name(host)?), port),
            }
        };
        let host = match host {
            AddressHost::Ip(ip) => AddressHost::Ip(usable_ip(ip)?),
            name => name,
        };
        Some(PeerAddress { host, port })
    }
}

/// Canonical text: `guchu`, `guchu:7718`, `100.78.127.19`, `fd7a::1`,
/// `[fd7a::1]:7718`. [`PeerAddress::parse`] reads it back unchanged.
impl std::fmt::Display for PeerAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.host, self.port) {
            (AddressHost::Name(name), None) => f.write_str(name),
            (AddressHost::Name(name), Some(port)) => write!(f, "{name}:{port}"),
            (AddressHost::Ip(ip), None) => write!(f, "{ip}"),
            (AddressHost::Ip(ip), Some(port)) => write!(f, "{}", SocketAddr::new(*ip, port)),
        }
    }
}

fn parse_port(text: &str) -> Option<u16> {
    if !(1..=5).contains(&text.len()) || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u16>().ok().filter(|p| *p != 0)
}

/// A DNS name: labels of 1 to 63 of `[a-z0-9-]`, not starting or ending with
/// `-`, at most 253 bytes, and not ending in an all-digit label (a mistyped IP).
fn parse_name(host: &str) -> Option<String> {
    let name = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return None;
    }
    let label_ok = |label: &str| {
        (1..=MAX_LABEL_BYTES).contains(&label.len())
            && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    if !name.split('.').all(label_ok) || name.rsplit('.').next()?.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(name)
}

/// `ip` as one that can name another device, or `None`.
fn usable_ip(ip: IpAddr) -> Option<IpAddr> {
    let ip = ip.to_canonical();
    let unusable = ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || match ip {
            IpAddr::V4(v4) => v4.is_broadcast(),
            IpAddr::V6(v6) => v6.is_unicast_link_local(),
        };
    (!unusable).then_some(ip)
}

/// What a paired device said in its last `device.reach`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerReach {
    /// Its tailnet socket addresses.
    pub tailnet: Vec<SocketAddr>,
    /// Whether its Away mode is on.
    pub away: bool,
}

impl PeerReach {
    /// The tailnet addresses of a valid message; others are dropped.
    pub(crate) fn from_message(reach: &DeviceReach) -> Self {
        PeerReach {
            tailnet: reach.addrs().into_iter().filter(|a| is_tailnet_ip(a.ip())).collect(),
            away: reach.away,
        }
    }

    /// `"<0|1>|<addr>,<addr>"` for the `peers.reach` column.
    pub(crate) fn to_storage_string(&self) -> String {
        let addrs: Vec<String> = self.tailnet.iter().map(ToString::to_string).collect();
        format!("{}|{}", u8::from(self.away), addrs.join(","))
    }

    /// Reads the `peers.reach` column; `None` when empty or unreadable.
    pub(crate) fn from_storage_str(text: &str) -> Option<Self> {
        let (away, addrs) = text.split_once('|')?;
        let away = match away {
            "0" => false,
            "1" => true,
            _ => return None,
        };
        let mut tailnet = Vec::new();
        for addr in addrs.split(',').filter(|a| !a.is_empty()) {
            let addr: SocketAddr = addr.parse().ok()?;
            if addr.port() == 0 || !is_tailnet_ip(addr.ip()) {
                return None;
            }
            if !tailnet.contains(&addr) {
                tailnet.push(addr);
            }
        }
        (tailnet.len() <= MAX_ADDRS).then_some(PeerReach { tailnet, away })
    }
}

/// What saving a typed address came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerAddressOutcome {
    /// The address was removed.
    Cleared,
    /// Saved, and given to the dialer.
    Ready,
    /// Saved, but the name has no answer right now (Tailscale off?).
    NotFound,
    /// Saved, but outside this network and the tailnet: used only with Away mode on.
    NotAllowed,
    /// Not an address or a name; nothing saved.
    Invalid,
}

/// How a paired device would be reached away from home, from what this
/// device knows (not a live test).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwayReach {
    /// Through Tailscale: it's up here, and a tailnet address of the device is known.
    Tailscale,
    /// Through relays: Away mode is on here, and the device didn't say it's off there.
    MobileData,
    /// A tailnet address is known, but Tailscale is off here.
    TailscaleOff,
    /// Only on the home network.
    HomeOnly,
}

/// The rule table behind [`Node::away_reach`]: `tailnet_known`, a tailnet
/// address of the peer is known; `tailnet_up`, Tailscale is up here;
/// `peer_away`, what the peer said of its Away mode (`None`: it never said).
pub(crate) fn away_reach(
    tailnet_known: bool,
    tailnet_up: bool,
    own_away: bool,
    peer_away: Option<bool>,
) -> AwayReach {
    if tailnet_known && tailnet_up {
        AwayReach::Tailscale
    } else if own_away && peer_away != Some(false) {
        AwayReach::MobileData
    } else if tailnet_known {
        AwayReach::TailscaleOff
    } else {
        AwayReach::HomeOnly
    }
}

/// The socket addresses to dial for a typed address: `port` when given;
/// else [`DEFAULT_PORT`] and the `known_ports` the device was seen on, at most
/// [`MAX_PORTS`]. Every IP with the first port, then with the next, at most
/// [`MAX_MANUAL_ADDRS`].
pub(crate) fn candidates(ips: &[IpAddr], port: Option<u16>, known_ports: &[u16]) -> Vec<SocketAddr> {
    let mut ports: Vec<u16> = Vec::with_capacity(MAX_PORTS);
    match port {
        Some(port) => ports.push(port),
        None => {
            for port in std::iter::once(DEFAULT_PORT).chain(known_ports.iter().copied()) {
                if port != 0 && !ports.contains(&port) {
                    ports.push(port);
                    if ports.len() == MAX_PORTS {
                        break;
                    }
                }
            }
        }
    }
    let mut addrs = Vec::new();
    for port in ports {
        for ip in ips {
            let addr = SocketAddr::new(*ip, port);
            if !addrs.contains(&addr) {
                addrs.push(addr);
                if addrs.len() == MAX_MANUAL_ADDRS {
                    return addrs;
                }
            }
        }
    }
    addrs
}

/// The ports `record` was reached on or said it listens on: its tailnet ones
/// first.
fn known_ports(record: &PeerRecord) -> Vec<u16> {
    let reach = record.reach.as_ref().map(|r| r.tailnet.as_slice()).unwrap_or_default();
    let mut ports = Vec::new();
    for addr in reach.iter().chain(&record.last_addrs) {
        if !ports.contains(&addr.port()) {
            ports.push(addr.port());
        }
    }
    ports
}

/// A typed name and its lookup, per peer, for this run.
#[derive(Default)]
pub(crate) struct ManualAddrs {
    /// The name `ips` answer; empty once the address is cleared while a
    /// lookup still runs.
    name: String,
    /// The last answers for `name`. A failed lookup keeps them.
    ips: Vec<IpAddr>,
    /// When the last lookup started.
    tried: Option<Instant>,
    /// A lookup for this peer is on a blocking thread.
    resolving: bool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Looks `name` up with the system resolver (getaddrinfo; on Android it
/// answers Tailscale's names while Tailscale is on). Blocks.
fn resolve_blocking(name: &str) -> Vec<IpAddr> {
    use std::net::ToSocketAddrs;
    let mut ips = Vec::new();
    if let Ok(found) = (name, 0u16).to_socket_addrs() {
        for ip in found.map(|a| a.ip().to_canonical()) {
            if !ips.contains(&ip) {
                ips.push(ip);
                if ips.len() == MAX_RESOLVED_IPS {
                    break;
                }
            }
        }
    }
    ips
}

impl Shared {
    /// Every address `record` may be dialed at: where it was last reached,
    /// the tailnet addresses it told us, and the address the user typed for
    /// it. Unfiltered: [`Shared::remember_addrs`] decides what's dialed.
    pub(crate) fn known_addrs(&self, record: &PeerRecord) -> Vec<SocketAddr> {
        let reach = record.reach.as_ref().map(|r| r.tailnet.as_slice()).unwrap_or_default();
        let manual = self.manual_addrs(record);
        let mut addrs: Vec<SocketAddr> = Vec::new();
        for addr in record.last_addrs.iter().chain(reach).chain(&manual) {
            if !addrs.contains(addr) {
                addrs.push(*addr);
            }
        }
        addrs
    }

    /// The addresses of `record`'s typed address: its IP, or the answers its
    /// name got in this run.
    fn manual_addrs(&self, record: &PeerRecord) -> Vec<SocketAddr> {
        let Some(manual) = &record.manual_addr else { return Vec::new() };
        let ips = match &manual.host {
            AddressHost::Ip(ip) => vec![*ip],
            AddressHost::Name(name) => lock(&self.manual)
                .get(&record.id)
                .filter(|m| &m.name == name)
                .map(|m| m.ips.clone())
                .unwrap_or_default(),
        };
        candidates(&ips, manual.port, &known_ports(record))
    }

    /// Rebuilds what the dialer knows of `peer` from what's stored, for when
    /// addresses went away (a typed address changed or cleared, a
    /// `device.reach` without one it said before). Filtered as
    /// [`Shared::remember_addrs`], keeping the cable's address.
    pub(crate) fn replace_addrs(&self, peer: &DeviceId) {
        let Ok(Some(record)) = self.store.get_peer(peer) else { return };
        let Ok(key) = crate::public_key(peer) else { return };
        let mut addrs: Vec<SocketAddr> =
            self.known_addrs(&record).into_iter().filter(|a| self.dialable(peer, a)).collect();
        if let Some(usb) = self.usb_peer_addr(peer)
            && !addrs.contains(&usb)
        {
            addrs.push(usb);
        }
        if addrs.is_empty() {
            self.memory.remove_endpoint_info(key);
        } else {
            self.memory.set_endpoint_info(EndpointInfo::new(key).with_ip_addrs(addrs));
        }
    }

    /// Sets the name `peer`'s typed address is (`None`: it's an IP, or
    /// cleared). A lookup still running keeps its slot.
    pub(crate) fn set_manual_name(&self, peer: &DeviceId, name: Option<&str>) {
        let mut manual = lock(&self.manual);
        match name {
            Some(name) => {
                let entry = manual.entry(*peer).or_default();
                if entry.name != name {
                    entry.name = name.to_owned();
                    entry.ips.clear();
                }
                entry.tried = None;
            }
            None => {
                if manual.get(peer).is_some_and(|m| m.resolving) {
                    manual.insert(*peer, ManualAddrs { resolving: true, ..Default::default() });
                } else {
                    manual.remove(peer);
                }
            }
        }
    }

    /// Before a dial with no session: looks `peer`'s typed name up again if
    /// the last try is older than [`RESOLVE_EVERY`]. Never waits for it.
    pub(crate) fn refresh_manual(self: &Arc<Self>, peer: &DeviceId) {
        let due = lock(&self.manual).get(peer).is_some_and(|m| {
            !m.name.is_empty() && !m.resolving && m.tried.is_none_or(|t| t.elapsed() >= RESOLVE_EVERY)
        });
        if due {
            drop(self.start_lookup(peer));
        }
    }

    /// Starts looking `peer`'s typed name up on a blocking thread, unless one
    /// is running (one per peer, so a hung resolver holds one thread per peer
    /// at most). The lookup stores its answers itself, even late ones, and
    /// wakes the peer's supervisor only if they changed.
    fn start_lookup(
        self: &Arc<Self>,
        peer: &DeviceId,
    ) -> Option<tokio::task::JoinHandle<Option<Vec<IpAddr>>>> {
        let name = {
            let mut manual = lock(&self.manual);
            let entry = manual.get_mut(peer).filter(|m| !m.name.is_empty() && !m.resolving)?;
            entry.resolving = true;
            entry.tried = Some(Instant::now());
            entry.name.clone()
        };
        let (weak, peer) = (Arc::downgrade(self), *peer);
        Some(self.runtime.spawn_blocking(move || {
            let ips = resolve_blocking(&name);
            let shared = weak.upgrade()?;
            let changed = {
                let mut manual = lock(&shared.manual);
                let (mut changed, mut cleared) = (false, false);
                if let Some(entry) = manual.get_mut(&peer) {
                    entry.resolving = false;
                    if entry.name == name && !ips.is_empty() && entry.ips != ips {
                        entry.ips = ips.clone();
                        changed = true;
                    }
                    cleared = entry.name.is_empty();
                }
                if cleared {
                    manual.remove(&peer);
                }
                changed
            };
            if changed {
                shared.replace_addrs(&peer);
                shared.wake_supervisor(&peer);
            }
            shared.resolve_done.notify_waiters();
            Some(ips)
        }))
    }

    /// Looks `peer`'s typed name `name` up and waits for the answers until
    /// `deadline`, first for a lookup still running to finish. `None`: no
    /// answer in time.
    async fn lookup_now(
        self: &Arc<Self>,
        peer: &DeviceId,
        name: &str,
        deadline: tokio::time::Instant,
    ) -> Option<Vec<IpAddr>> {
        let job = loop {
            let finished = self.resolve_done.notified();
            tokio::pin!(finished);
            finished.as_mut().enable();
            if lock(&self.manual).get(peer).is_none_or(|m| m.name != name) {
                return None;
            }
            if let Some(job) = self.start_lookup(peer) {
                break job;
            }
            tokio::time::timeout_at(deadline, finished).await.ok()?;
        };
        tokio::time::timeout_at(deadline, job).await.ok()?.ok()?.filter(|ips| !ips.is_empty())
    }

    /// This device's own `device.reach`: its tailnet socket addresses, IPv4
    /// first, and its Away mode.
    pub(crate) fn own_reach(&self) -> DeviceReach {
        let mut addrs: Vec<SocketAddr> = Vec::new();
        for addr in self.endpoint.addr().ip_addrs() {
            if is_tailnet_ip(addr.ip()) && addr.port() != 0 && !addrs.contains(addr) {
                addrs.push(*addr);
            }
        }
        addrs.sort_by_key(|a| !a.is_ipv4());
        addrs.truncate(MAX_ADDRS);
        DeviceReach { tailnet: addrs.iter().map(ToString::to_string).collect(), away: self.away_mode() }
    }

    /// Tells a peer that just connected how this device is reached.
    pub(crate) async fn send_reach(&self, session: &Session) {
        if let Ok(env) = Envelope::new(types::DEVICE_REACH, &self.own_reach()) {
            let _ = session.send(env).await;
        }
    }

    /// Tells every connected peer how this device is reached, if that changed
    /// since it was last told. One refresh at a time, so the last one sent
    /// is the newest.
    pub(crate) async fn refresh_reach(&self) {
        let mut sent = self.sent_reach.lock().await;
        let reach = self.own_reach();
        if sent.as_ref() == Some(&reach) {
            return;
        }
        let Ok(env) = Envelope::new(types::DEVICE_REACH, &reach) else { return };
        for session in self.live_sessions() {
            let _ = session.send(env.clone()).await;
        }
        *sent = Some(reach);
    }

    /// After a network change: typed names are looked up again before the
    /// next dial, and peers hear of a changed tailnet address, now and once
    /// interfaces have settled.
    pub(crate) fn reach_network_changed(self: &Arc<Self>) {
        for entry in lock(&self.manual).values_mut() {
            entry.tried = None;
        }
        let weak = Arc::downgrade(self);
        self.runtime.spawn(async move {
            if let Some(shared) = weak.upgrade() {
                shared.refresh_reach().await;
            }
            tokio::time::sleep(REACH_SETTLE).await;
            if let Some(shared) = weak.upgrade() {
                shared.refresh_reach().await;
            }
        });
    }

    /// Stores what `peer` said in `device.reach` (only its tailnet
    /// addresses), replacing what it said before. Blocks on the store.
    pub(crate) fn store_reach(&self, peer: &DeviceId, reach: &DeviceReach) -> Result<()> {
        let reach = PeerReach::from_message(reach);
        let Some(record) = self.store.get_peer(peer)? else { return Ok(()) };
        if record.reach.as_ref() == Some(&reach) {
            return Ok(());
        }
        self.store.update_reach(peer, Some(&reach))?;
        self.replace_addrs(peer);
        Ok(())
    }
}

impl Node {
    /// Saves where `peer` can be reached (`""` clears it), looks a name up,
    /// feeds the dialer and redials. An address is saved even when it can't
    /// be used now ([`PeerAddressOutcome::NotFound`],
    /// [`PeerAddressOutcome::NotAllowed`]).
    pub async fn set_peer_address(&self, peer: DeviceId, input: &str) -> Result<PeerAddressOutcome> {
        let shared = self.shared();
        if !shared.store.is_paired(&peer)? {
            return Err(Error::NotPaired);
        }
        let input = input.trim();
        if input.is_empty() {
            shared.store.set_manual_addr(&peer, None)?;
            shared.set_manual_name(&peer, None);
            shared.replace_addrs(&peer);
            return Ok(PeerAddressOutcome::Cleared);
        }
        let Some(address) = PeerAddress::parse(input) else { return Ok(PeerAddressOutcome::Invalid) };
        shared.store.set_manual_addr(&peer, Some(&address))?;
        let outcome = match &address.host {
            AddressHost::Ip(ip) => {
                shared.set_manual_name(&peer, None);
                if shared.ip_dialable(&peer, *ip) {
                    PeerAddressOutcome::Ready
                } else {
                    PeerAddressOutcome::NotAllowed
                }
            }
            AddressHost::Name(name) => {
                shared.set_manual_name(&peer, Some(name));
                let deadline = tokio::time::Instant::now() + RESOLVE_TIMEOUT;
                match shared.lookup_now(&peer, name, deadline).await {
                    None => PeerAddressOutcome::NotFound,
                    Some(ips) if ips.iter().any(|ip| shared.ip_dialable(&peer, *ip)) => {
                        PeerAddressOutcome::Ready
                    }
                    Some(_) => PeerAddressOutcome::NotAllowed,
                }
            }
        };
        shared.replace_addrs(&peer);
        shared.wake_supervisor(&peer);
        Ok(outcome)
    }

    /// The address the user typed for `peer`, in canonical form.
    pub fn peer_address(&self, peer: DeviceId) -> Result<Option<String>> {
        let record = self.shared().store.get_peer(&peer)?.ok_or(Error::NotPaired)?;
        Ok(record.manual_addr.map(|a| a.to_string()))
    }

    /// What `peer` said in its last `device.reach`, if it ever did.
    pub fn peer_reach(&self, peer: DeviceId) -> Result<Option<PeerReach>> {
        Ok(self.shared().store.get_peer(&peer)?.ok_or(Error::NotPaired)?.reach)
    }

    /// How `peer` would be reached away from home, from what's stored.
    /// `tailnet_up`: whether this device has Tailscale up now (the platform says).
    pub fn away_reach(&self, peer: DeviceId, tailnet_up: bool) -> Result<AwayReach> {
        let shared = self.shared();
        let record = shared.store.get_peer(&peer)?.ok_or(Error::NotPaired)?;
        let tailnet_known = shared.known_addrs(&record).iter().any(|a| is_tailnet_ip(a.ip()));
        let peer_away = record.reach.as_ref().map(|r| r.away);
        Ok(away_reach(tailnet_known, tailnet_up, shared.away_mode(), peer_away))
    }

    /// The addresses the dialer has for `peer`.
    #[doc(hidden)]
    pub fn dial_addrs(&self, peer: DeviceId) -> Vec<SocketAddr> {
        let Ok(key) = crate::public_key(&peer) else { return Vec::new() };
        self.shared()
            .memory
            .get_endpoint_info(key)
            .map(|info| info.ip_addrs().copied().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn sa(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    #[test]
    fn every_accepted_address_form_parses_and_prints_back() {
        let accepted = [
            ("100.78.127.19", AddressHost::Ip(ip("100.78.127.19")), None, "100.78.127.19"),
            ("100.78.127.19:7718", AddressHost::Ip(ip("100.78.127.19")), Some(7718), "100.78.127.19:7718"),
            ("  192.168.1.20:40314 ", AddressHost::Ip(ip("192.168.1.20")), Some(40314), "192.168.1.20:40314"),
            ("fd7a:115c:a1e0::1", AddressHost::Ip(ip("fd7a:115c:a1e0::1")), None, "fd7a:115c:a1e0::1"),
            ("[fd7a:115c:a1e0::1]", AddressHost::Ip(ip("fd7a:115c:a1e0::1")), None, "fd7a:115c:a1e0::1"),
            (
                "[fd7a:115c:a1e0::1]:7718",
                AddressHost::Ip(ip("fd7a:115c:a1e0::1")),
                Some(7718),
                "[fd7a:115c:a1e0::1]:7718",
            ),
            ("[::ffff:100.64.0.9]:5000", AddressHost::Ip(ip("100.64.0.9")), Some(5000), "100.64.0.9:5000"),
            ("guchu", AddressHost::Name("guchu".into()), None, "guchu"),
            ("Guchu:7718", AddressHost::Name("guchu".into()), Some(7718), "guchu:7718"),
            (
                "guchu.tail1234.ts.net.",
                AddressHost::Name("guchu.tail1234.ts.net".into()),
                None,
                "guchu.tail1234.ts.net",
            ),
            ("my-pc-2:65535", AddressHost::Name("my-pc-2".into()), Some(65535), "my-pc-2:65535"),
            ("8.8.8.8", AddressHost::Ip(ip("8.8.8.8")), None, "8.8.8.8"),
        ];
        for (input, host, port, canonical) in accepted {
            let parsed = PeerAddress::parse(input).unwrap_or_else(|| panic!("{input} should parse"));
            assert_eq!(parsed, PeerAddress { host, port }, "{input}");
            assert_eq!(parsed.to_string(), canonical, "{input}");
            assert_eq!(PeerAddress::parse(canonical), Some(parsed), "{canonical} reads back");
        }
        let longest =
            format!("{}.{}.{}.{}:65535", "a".repeat(63), "b".repeat(63), "c".repeat(63), "d".repeat(61));
        assert_eq!(longest.len(), 259);
        assert!(PeerAddress::parse(&longest).is_some());
    }

    #[test]
    fn every_rejected_address_form_is_refused() {
        let too_long_name =
            format!("{}.{}.{}.{}", "a".repeat(63), "b".repeat(63), "c".repeat(63), "d".repeat(63));
        let rejected = [
            "",
            "   ",
            "http://guchu",
            "guchu/x",
            "user@guchu",
            "fe80::1%eth0",
            "gu chu",
            "guchu:0",
            "guchu:65536",
            "guchu:",
            "guchu:+80",
            "guchu:x",
            "100.64.0.9:0",
            "[fd7a::1]:0",
            "[fd7a::1]x",
            "[fd7a::1",
            "[100.64.0.9]",
            "127.0.0.1",
            "localhost:7718:1",
            "[::1]",
            "0.0.0.0",
            "::",
            "224.0.0.251",
            "ff02::1",
            "255.255.255.255",
            "fe80::1",
            "[fe80::1]:7718",
            "-guchu",
            "guchu-",
            "gu_chu",
            "gu..chu",
            ".",
            "100.64.0.999",
            "1.2.3",
            "host.123",
            "01.2.3.4",
            "pc.ñ",
            &too_long_name,
            &"a".repeat(MAX_ADDRESS_INPUT + 1),
        ];
        for input in rejected {
            assert_eq!(PeerAddress::parse(input), None, "{input:?} should be refused");
        }
        let long_label = format!("{}.net", "a".repeat(64));
        assert_eq!(PeerAddress::parse(&long_label), None);
    }

    #[test]
    fn candidates_use_the_given_port_or_the_default_and_known_ones() {
        let one = [ip("100.64.0.9")];
        assert_eq!(candidates(&one, Some(5000), &[40314]), vec![sa("100.64.0.9:5000")]);
        assert_eq!(
            candidates(&one, None, &[40314, DEFAULT_PORT, 40314, 50000, 50001]),
            vec![sa("100.64.0.9:7718"), sa("100.64.0.9:40314"), sa("100.64.0.9:50000")],
            "the default first, then distinct known ports, at most three"
        );
        assert_eq!(candidates(&one, None, &[]), vec![sa("100.64.0.9:7718")]);
        let four = [ip("100.64.0.1"), ip("100.64.0.2"), ip("100.64.0.3"), ip("100.64.0.4")];
        let all = candidates(&four, None, &[1000, 2000]);
        assert_eq!(all.len(), MAX_MANUAL_ADDRS);
        assert!(
            four.iter().all(|ip| all.contains(&SocketAddr::new(*ip, DEFAULT_PORT))),
            "every IP gets the default port"
        );
        assert!(candidates(&[], None, &[1000]).is_empty());
    }

    #[test]
    fn the_away_reach_rules_apply_in_order() {
        use AwayReach::*;
        for tailnet_known in [false, true] {
            for tailnet_up in [false, true] {
                for own_away in [false, true] {
                    for peer_away in [None, Some(false), Some(true)] {
                        let expected = if tailnet_known && tailnet_up {
                            Tailscale
                        } else if own_away && peer_away != Some(false) {
                            MobileData
                        } else if tailnet_known {
                            TailscaleOff
                        } else {
                            HomeOnly
                        };
                        assert_eq!(
                            away_reach(tailnet_known, tailnet_up, own_away, peer_away),
                            expected,
                            "{tailnet_known} {tailnet_up} {own_away} {peer_away:?}"
                        );
                    }
                }
            }
        }
        assert_eq!(away_reach(true, true, true, Some(true)), Tailscale);
        assert_eq!(away_reach(false, true, true, None), MobileData, "a peer that never said counts as away");
        assert_eq!(away_reach(false, false, true, Some(false)), HomeOnly);
        assert_eq!(away_reach(true, false, true, Some(false)), TailscaleOff);
        assert_eq!(away_reach(false, true, false, Some(true)), HomeOnly);
    }

    #[test]
    fn peer_reach_keeps_only_tailnet_addresses_and_reads_its_column_back() {
        let message = DeviceReach {
            tailnet: vec![
                "100.78.127.19:7718".into(),
                "192.168.1.20:7718".into(),
                "[fd7a:115c:a1e0::1]:7718".into(),
            ],
            away: true,
        };
        let reach = PeerReach::from_message(&message);
        assert_eq!(reach.tailnet, vec![sa("100.78.127.19:7718"), sa("[fd7a:115c:a1e0::1]:7718")]);
        assert!(reach.away);
        let stored = reach.to_storage_string();
        assert_eq!(stored, "1|100.78.127.19:7718,[fd7a:115c:a1e0::1]:7718");
        assert_eq!(PeerReach::from_storage_str(&stored), Some(reach));
        let none = PeerReach { tailnet: vec![], away: false };
        assert_eq!(PeerReach::from_storage_str(&none.to_storage_string()), Some(none));
        for unreadable in
            ["", "1", "2|", "yes|100.64.0.1:1", "0|nope", "0|192.168.1.2:7718", "0|100.64.0.1:0"]
        {
            assert_eq!(PeerReach::from_storage_str(unreadable), None, "{unreadable:?}");
        }
    }
}
