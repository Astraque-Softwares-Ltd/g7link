// SPDX-License-Identifier: MPL-2.0
//! Reaching a phone's Wireless debugging from this PC's adb, so its Elevated
//! helper can be started without a cable (`docs/protocol/elevated-usb.md`).
//!
//! A phone whose user paired this PC under Wireless debugging lets this PC's
//! adb in, but on a port that changes each time Wireless debugging turns on.
//! The phone announces it over mDNS as `_adb-tls-connect._tcp`. adb looks for
//! that itself only when it was built with mDNS, which the adb in Debian and
//! Ubuntu isn't: there a paired phone was never found. So the port is looked
//! up here, and adb is told to connect to it.
//!
//! Which phone answered is checked before anything runs on it: the request
//! names the app's installed APK, whose path Android makes up at random for
//! each install, so only the phone that asked has that file.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::{io::AsyncReadExt, net::UdpSocket, time::Instant};

const MDNS_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_PORT: u16 = 5353;
/// What a phone with Wireless debugging on announces.
const SERVICE: &str = "_adb-tls-connect._tcp.local";
/// The end of each such phone's own name (`adb-<serial>-<id>._adb-tls-connect._tcp.local`).
const INSTANCE_END: &str = "._adb-tls-connect._tcp.local";
const TYPE_PTR: u16 = 12;
const TYPE_SRV: u16 = 33;
const CLASS_IN: u16 = 1;
/// Set in a question's class: answer straight back rather than to everyone.
const UNICAST_REPLY: u16 = 0x8000;

/// How long phones have to answer.
const DISCOVERY: Duration = Duration::from_secs(3);
/// How often the question is asked again within that.
const ASK_EVERY: Duration = Duration::from_millis(500);
/// How long adb may take to connect to a phone.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// How long a phone may take to say whether it has the app's APK.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
/// How many times a phone that just connected is asked (adb shows it as
/// offline for a moment first).
const CHECK_TRIES: u32 = 8;
const CHECK_AGAIN: Duration = Duration::from_millis(400);
/// Phones tried, at most: answers come from the whole network.
const MAX_DEVICES: usize = 4;
/// Records read from one answer, and pointers followed in one name, at most.
const MAX_RECORDS: usize = 64;
const MAX_JUMPS: u32 = 16;
/// What the phone prints when it has the APK.
const HERE: &str = "g7link-apk-here";

/// The adb serial of the phone that asked, reached over its Wireless
/// debugging: one adb is already connected to, or one found on the network
/// and connected now. `peer` is where the phone's connection comes from, and
/// `apk` its app's installed APK (already checked to be a plain path).
pub(crate) async fn route(server: SocketAddr, peer: &[IpAddr], apk: &str) -> Result<String, String> {
    g7link_adb::ensure_server(server).await.map_err(|e| format!("adb isn't running on this PC: {e}"))?;
    let devices = g7link_adb::list_devices(server).await.unwrap_or_default();
    for device in devices.iter().filter(|d| d.is_online() && g7link_adb::is_network_serial(&d.serial)) {
        if has_apk(server, &device.serial, apk).await == Some(true) {
            return Ok(device.serial.clone());
        }
    }
    let found = discover(peer).await;
    if found.is_empty() {
        return Err("its Wireless debugging wasn't found on this network".into());
    }
    let mut why = String::new();
    for device in found {
        match tokio::time::timeout(CONNECT_TIMEOUT, g7link_adb::connect(server, device)).await {
            Ok(Ok(serial)) => {
                if has_apk_soon(server, &serial, apk).await {
                    return Ok(serial);
                }
                why = format!("{device} isn't this phone");
            }
            Ok(Err(e)) => why = e.to_string(),
            Err(_) => why = format!("adb didn't connect to {device} in time"),
        }
    }
    Err(format!("adb couldn't use its Wireless debugging: {why}"))
}

/// [`has_apk`] for a phone adb just connected to, which it may still be signing in to.
async fn has_apk_soon(server: SocketAddr, serial: &str, apk: &str) -> bool {
    for _ in 0..CHECK_TRIES {
        match has_apk(server, serial, apk).await {
            Some(has) => return has,
            None => tokio::time::sleep(CHECK_AGAIN).await,
        }
    }
    false
}

/// Whether the phone adb knows as `serial` has `apk`: `None` when it couldn't be asked.
async fn has_apk(server: SocketAddr, serial: &str, apk: &str) -> Option<bool> {
    let command = format!("test -f {apk} && echo {HERE}");
    let ask = async {
        let stream = g7link_adb::shell(server, serial, &command).await.ok()?;
        let mut said = Vec::new();
        // The mark and a line break, or nothing: never much.
        stream.take(256).read_to_end(&mut said).await.ok()?;
        Some(String::from_utf8_lossy(&said).contains(HERE))
    };
    tokio::time::timeout(CHECK_TIMEOUT, ask).await.ok().flatten()
}

/// The phones announcing Wireless debugging on the network `peer` is on,
/// that phone's first.
async fn discover(peer: &[IpAddr]) -> Vec<SocketAddr> {
    let wanted: Vec<Ipv4Addr> = peer
        .iter()
        .filter_map(|ip| match ip.to_canonical() {
            IpAddr::V4(v4) => Some(v4),
            IpAddr::V6(_) => None,
        })
        .collect();
    let local = local_toward(wanted.first().copied());
    // Asked from a port of its own, the answers come straight back to it.
    let Ok(direct) = UdpSocket::bind((local.unwrap_or(Ipv4Addr::UNSPECIFIED), 0)).await else {
        return Vec::new();
    };
    // Asked from mDNS's own port, they go to everyone listening there. Some
    // phones only answer that way; the port is shared with the system's mDNS.
    let shared = local.and_then(|local| shared_socket(local).ok());
    let deadline = Instant::now() + DISCOVERY;
    let done = AtomicBool::new(false);
    let direct_answers = browse(&direct, true, &wanted, deadline, &done);
    let shared_answers = async {
        match &shared {
            Some(shared) => browse(shared, false, &wanted, deadline, &done).await,
            None => Vec::new(),
        }
    };
    let (mut found, more) = tokio::join!(direct_answers, shared_answers);
    for device in more {
        if !found.contains(&device) {
            found.push(device);
        }
    }
    found.sort_by_key(|device| !wanted.contains(device.ip()));
    found.truncate(MAX_DEVICES);
    found.into_iter().map(SocketAddr::V4).collect()
}

/// Asks on `socket` which phones have Wireless debugging on, until
/// `deadline` or until one of `wanted` answered (any, when none is wanted).
async fn browse(
    socket: &UdpSocket,
    unicast: bool,
    wanted: &[Ipv4Addr],
    deadline: Instant,
    done: &AtomicBool,
) -> Vec<SocketAddrV4> {
    let everyone = SocketAddr::from((MDNS_GROUP, MDNS_PORT));
    let ask = question(SERVICE, TYPE_PTR, unicast);
    let mut found: Vec<SocketAddrV4> = Vec::new();
    let mut packet = [0u8; 4096];
    while Instant::now() < deadline && !done.load(Ordering::Acquire) {
        let _ = socket.send_to(&ask, everyone).await;
        let listen_until = (Instant::now() + ASK_EVERY).min(deadline);
        loop {
            let heard = tokio::time::timeout_at(listen_until, socket.recv_from(&mut packet)).await;
            let (len, from) = match heard {
                Ok(Ok((len, SocketAddr::V4(from)))) => (len, from),
                Ok(Ok(_)) => continue,
                Ok(Err(_)) => {
                    tokio::time::sleep_until(listen_until).await;
                    break;
                }
                Err(_) => break,
            };
            let Some(answer) = read_answer(&packet[..len]) else { continue };
            for port in &answer.ports {
                let device = SocketAddrV4::new(*from.ip(), *port);
                if !found.contains(&device) && found.len() < MAX_DEVICES {
                    found.push(device);
                }
            }
            // A phone that named itself without its port is asked for it.
            for name in answer.unresolved.iter().take(MAX_DEVICES) {
                let phone = SocketAddr::from((*from.ip(), MDNS_PORT));
                let _ = socket.send_to(&question(name, TYPE_SRV, unicast), phone).await;
            }
            let answered = match wanted {
                [] => !found.is_empty(),
                wanted => found.iter().any(|device| wanted.contains(device.ip())),
            };
            if answered {
                done.store(true, Ordering::Release);
                return found;
            }
        }
    }
    found
}

/// The address this PC reaches `peer` from (or the internet, without one):
/// the questions go out of that network.
fn local_toward(peer: Option<Ipv4Addr>) -> Option<Ipv4Addr> {
    let probe = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    // Connecting a UDP socket sends nothing: it only picks the route.
    probe.connect((peer.unwrap_or(Ipv4Addr::new(8, 8, 8, 8)), MDNS_PORT)).ok()?;
    let SocketAddr::V4(local) = probe.local_addr().ok()? else { return None };
    (!local.ip().is_unspecified() && !local.ip().is_loopback()).then_some(*local.ip())
}

/// mDNS's own port on the network `local` is on, shared with whatever else
/// listens there (Avahi, systemd-resolved).
fn shared_socket(local: Ipv4Addr) -> std::io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, MDNS_PORT).into())?;
    socket.join_multicast_v4(&MDNS_GROUP, &local)?;
    socket.set_multicast_if_v4(&local)?;
    let _ = socket.set_multicast_loop_v4(false);
    let socket: std::net::UdpSocket = socket.into();
    UdpSocket::from_std(socket)
}

/// A question for `name`'s records of `kind`.
fn question(name: &str, kind: u16, unicast: bool) -> Vec<u8> {
    // The header: no ID, no flags, one question.
    let mut packet = vec![0u8; 12];
    packet[5] = 1;
    for label in name.split('.').filter(|label| !label.is_empty()) {
        let label = &label.as_bytes()[..label.len().min(63)];
        packet.push(label.len() as u8);
        packet.extend_from_slice(label);
    }
    packet.push(0);
    packet.extend_from_slice(&kind.to_be_bytes());
    let class = if unicast { CLASS_IN | UNICAST_REPLY } else { CLASS_IN };
    packet.extend_from_slice(&class.to_be_bytes());
    packet
}

/// What an answer says about Wireless debugging.
#[derive(Debug, Default, PartialEq, Eq)]
struct Answer {
    /// The ports phones listen on.
    ports: Vec<u16>,
    /// Phones named without their port.
    unresolved: Vec<String>,
}

/// Reads an mDNS answer: `None` when it isn't one, or is cut short.
fn read_answer(packet: &[u8]) -> Option<Answer> {
    if be16(packet, 2)? & 0x8000 == 0 {
        return None;
    }
    let questions = be16(packet, 4)?;
    // Answers, authorities and additional records are read alike.
    let count = |at| be16(packet, at).map(usize::from);
    let records = count(6)? + count(8)? + count(10)?;
    let mut at = 12;
    for _ in 0..questions {
        at = read_name(packet, at)?.1 + 4;
    }
    let mut named: Vec<String> = Vec::new();
    let mut ports: Vec<(String, u16)> = Vec::new();
    for _ in 0..records.min(MAX_RECORDS) {
        let (name, fields) = read_name(packet, at)?;
        let kind = be16(packet, fields)?;
        let data = fields + 10;
        at = data + usize::from(be16(packet, fields + 8)?);
        if at > packet.len() {
            return None;
        }
        if kind == TYPE_PTR && name == SERVICE {
            // The record's data: the name of a phone.
            let (phone, _) = read_name(packet, data)?;
            if phone.ends_with(INSTANCE_END) && !named.contains(&phone) {
                named.push(phone);
            }
        } else if kind == TYPE_SRV && name.ends_with(INSTANCE_END) && at >= data + 6 {
            // Priority and weight, then the port.
            let port = be16(packet, data + 4)?;
            if port > 0 {
                ports.push((name, port));
            }
        }
    }
    named.retain(|phone| !ports.iter().any(|(name, _)| name == phone));
    Some(Answer { ports: ports.into_iter().map(|(_, port)| port).collect(), unresolved: named })
}

fn be16(packet: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*packet.get(at)?, *packet.get(at.checked_add(1)?)?]))
}

/// The name at `start`, in lower case with dots, and where what follows it
/// starts. Names may point back into the packet for their endings.
fn read_name(packet: &[u8], start: usize) -> Option<(String, usize)> {
    let mut name = String::new();
    let mut at = start;
    let mut after = None;
    let mut jumps = 0;
    loop {
        let len = usize::from(*packet.get(at)?);
        if len == 0 {
            return Some((name, after.unwrap_or(at + 1)));
        }
        if len & 0xC0 == 0xC0 {
            let low = usize::from(*packet.get(at + 1)?);
            if after.is_none() {
                after = Some(at + 2);
            }
            jumps += 1;
            if jumps > MAX_JUMPS {
                return None;
            }
            at = ((len & 0x3F) << 8) | low;
            continue;
        }
        if len > 63 {
            return None;
        }
        let label = packet.get(at + 1..at + 1 + len)?;
        if !name.is_empty() {
            name.push('.');
        }
        name.push_str(&String::from_utf8_lossy(label).to_ascii_lowercase());
        if name.len() > 255 {
            return None;
        }
        at += 1 + len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: &str = "adb-44091FDJG0013X-aBcDeF._adb-tls-connect._tcp.local";

    fn name(name: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        for label in name.split('.') {
            bytes.push(label.len() as u8);
            bytes.extend_from_slice(label.as_bytes());
        }
        bytes.push(0);
        bytes
    }

    fn record(owner: &[u8], kind: u16, data: &[u8]) -> Vec<u8> {
        let mut bytes = owner.to_vec();
        bytes.extend_from_slice(&kind.to_be_bytes());
        bytes.extend_from_slice(&CLASS_IN.to_be_bytes());
        bytes.extend_from_slice(&120u32.to_be_bytes());
        bytes.extend_from_slice(&(data.len() as u16).to_be_bytes());
        bytes.extend_from_slice(data);
        bytes
    }

    fn srv(port: u16, target: &[u8]) -> Vec<u8> {
        let mut data = vec![0, 0, 0, 0];
        data.extend_from_slice(&port.to_be_bytes());
        data.extend_from_slice(target);
        data
    }

    fn answer(records: &[Vec<u8>]) -> Vec<u8> {
        let mut packet = vec![0u8; 12];
        packet[2] = 0x84;
        packet[7] = records.len() as u8;
        for record in records {
            packet.extend_from_slice(record);
        }
        packet
    }

    const APK: &str = "/data/app/~~a==/app.g7link-b==/base.apk";

    /// A mock adb server that reaches one phone over the network, which has [`APK`].
    async fn adb_with_phone(serial: &'static str) -> SocketAddr {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let Ok(command) = g7link_adb::read_payload(&mut socket).await else { continue };
                if command == "host:devices-l" {
                    let list = format!("{serial} device product:husky model:Pixel transport_id:3\n");
                    let _ = socket.write_all(format!("OKAY{:04x}{list}", list.len()).as_bytes()).await;
                } else if command == format!("host:transport:{serial}") {
                    let _ = socket.write_all(b"OKAY").await;
                    let Ok(shell) = g7link_adb::read_payload(&mut socket).await else { continue };
                    let _ = socket.write_all(b"OKAY").await;
                    if shell == format!("shell:test -f {APK} && echo {HERE}") {
                        let _ = socket.write_all(format!("{HERE}\n").as_bytes()).await;
                    }
                } else {
                    let _ = socket.write_all(b"FAIL000edevice offline").await;
                }
            }
        });
        addr
    }

    #[tokio::test]
    async fn a_phone_adb_already_reaches_is_used_once_it_shows_it_is_the_one_that_asked() {
        let server = adb_with_phone("192.168.1.40:37123").await;
        assert_eq!(route(server, &[], APK).await, Ok("192.168.1.40:37123".to_owned()));
        assert_eq!(has_apk(server, "192.168.1.40:37123", APK).await, Some(true));
        // Another install's APK isn't on it: it's somebody else's phone.
        let other = "/data/app/~~c==/app.g7link-d==/base.apk";
        assert_eq!(has_apk(server, "192.168.1.40:37123", other).await, Some(false));
        // A phone adb can't run anything on yet can't say either way.
        assert_eq!(has_apk(server, "192.168.1.41:40000", APK).await, None);
    }

    #[test]
    fn the_question_is_the_one_phones_answer() {
        let packet = question(SERVICE, TYPE_PTR, true);
        assert_eq!(&packet[..12], &[0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(read_name(&packet, 12), Some((SERVICE.to_owned(), packet.len() - 4)));
        assert_eq!(&packet[packet.len() - 4..], &[0, 12, 0x80, 1]);
        assert_eq!(&question(SERVICE, TYPE_PTR, false)[packet.len() - 2..], &[0, 1]);
        // A question isn't mistaken for an answer.
        assert_eq!(read_answer(&packet), None);
    }

    #[test]
    fn an_answer_with_its_port_names_the_phone() {
        let packet = answer(&[
            record(&name(SERVICE), TYPE_PTR, &name(PHONE)),
            record(&name(PHONE), TYPE_SRV, &srv(37123, &name("Android.local"))),
            record(&name("Android.local"), 1, &[192, 168, 1, 40]),
        ]);
        assert_eq!(read_answer(&packet), Some(Answer { ports: vec![37123], unresolved: Vec::new() }));
    }

    #[test]
    fn names_that_point_back_into_the_packet_are_followed() {
        // As phones send it: the service's name once, and pointers to it after.
        let mut packet = vec![0u8; 12];
        packet[2] = 0x84;
        packet[7] = 2;
        let service_at = packet.len() as u8;
        packet.extend_from_slice(&name(SERVICE));
        packet.extend_from_slice(&TYPE_PTR.to_be_bytes());
        packet.extend_from_slice(&CLASS_IN.to_be_bytes());
        packet.extend_from_slice(&120u32.to_be_bytes());
        let label = b"adb-44091FDJG0013X-aBcDeF";
        packet.extend_from_slice(&(label.len() as u16 + 3).to_be_bytes());
        let instance_at = packet.len() as u8;
        packet.push(label.len() as u8);
        packet.extend_from_slice(label);
        packet.extend_from_slice(&[0xC0, service_at]);
        let port = record(&[0xC0, instance_at], TYPE_SRV, &srv(41001, &name("Android.local")));
        packet.extend_from_slice(&port);
        assert_eq!(read_answer(&packet), Some(Answer { ports: vec![41001], unresolved: Vec::new() }));
        let (instance, _) = read_name(&packet, usize::from(instance_at)).unwrap();
        assert_eq!(instance, PHONE.to_lowercase());
    }

    #[test]
    fn a_phone_named_without_its_port_is_asked_for_it() {
        let packet = answer(&[record(&name(SERVICE), TYPE_PTR, &name(PHONE))]);
        let answer = read_answer(&packet).unwrap();
        assert_eq!(answer, Answer { ports: Vec::new(), unresolved: vec![PHONE.to_lowercase()] });
        let ask = question(&answer.unresolved[0], TYPE_SRV, true);
        assert_eq!(read_name(&ask, 12).map(|(name, _)| name), Some(PHONE.to_lowercase()));
    }

    #[test]
    fn other_services_and_broken_packets_are_left_alone() {
        let other = answer(&[
            record(&name("_ipp._tcp.local"), TYPE_PTR, &name("Printer._ipp._tcp.local")),
            record(&name("Printer._ipp._tcp.local"), TYPE_SRV, &srv(631, &name("printer.local"))),
            // Pairing mode's service isn't the one to connect to.
            record(&name("x._adb-tls-pairing._tcp.local"), TYPE_SRV, &srv(40001, &name("Android.local"))),
        ]);
        assert_eq!(read_answer(&other), Some(Answer::default()));

        let whole = answer(&[record(&name(PHONE), TYPE_SRV, &srv(37123, &name("Android.local")))]);
        for cut in 0..whole.len() {
            assert_eq!(read_answer(&whole[..cut]), None, "cut at {cut}");
        }
        // A name that points at itself ends, rather than being followed forever.
        let mut looping = vec![0u8; 12];
        looping[2] = 0x84;
        looping[7] = 1;
        looping.extend_from_slice(&[0xC0, 12, 0, 12, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(read_answer(&looping), None);
    }
}
