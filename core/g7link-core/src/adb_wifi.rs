// SPDX-License-Identifier: MPL-2.0
//! Reaching a phone's Wireless debugging from this PC's adb, so its Elevated
//! helper can be started without a cable (`docs/protocol/elevated-usb.md`).
//!
//! A phone whose user paired this PC under Wireless debugging lets this PC's
//! adb in, but on a port that changes each time Wireless debugging turns on.
//! adb looks that port up over mDNS, and only when it was built to: the adb
//! in Debian and Ubuntu isn't, and a phone asleep doesn't answer anyway. So
//! the phone says the port in its request, and adb is told to connect to it
//! at the address the phone is connected from.
//!
//! A phone that can't find its own port (Android 17 hides it from apps that
//! aren't paired with it) asks without one. Then this PC uses an adb
//! connection it already has to the phone's address, or else tries the phone's
//! open ports in Android's range for its Wireless debugging ([`find`]).
//!
//! Which phone answered is checked before anything runs on it: the request
//! names the app's installed APK, whose path Android makes up at random for
//! each install, so only the phone that asked has that file.

use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use tokio::io::AsyncReadExt;

/// How long adb may take to connect to the phone.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// How long the phone may take to say whether it has the app's APK.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
/// How many times a phone that just connected is asked (adb shows it as
/// offline for a moment first).
const CHECK_TRIES: u32 = 8;
const CHECK_AGAIN: Duration = Duration::from_millis(400);
/// Addresses of the phone tried, at most.
const MAX_ADDRESSES: usize = 3;
/// What the phone prints when it has the APK.
const HERE: &str = "g7link-apk-here";
/// The ports Android gives Wireless debugging (Linux's ephemeral range).
const PORTS: std::ops::RangeInclusive<u16> = 32768..=60999;
/// LocalSend's ports, which the app itself listens on.
const NOT_ADB: std::ops::RangeInclusive<u16> = 53317..=53322;
/// How long a port may take to accept a connection.
const PROBE_TIMEOUT: Duration = Duration::from_millis(400);
/// Ports tried at once.
const PROBES_AT_ONCE: usize = 256;
/// Open ports offered to adb, at most.
const MAX_CANDIDATES: usize = 6;

/// The adb serial of the phone that asked, reached over its Wireless
/// debugging. `peer` is where the phone's connection comes from, `port` what
/// it says its Wireless debugging listens on, and `apk` its app's installed
/// APK (already checked to be a plain path).
pub(crate) async fn route(
    server: SocketAddr,
    peer: &[IpAddr],
    port: u16,
    apk: &str,
) -> Result<String, String> {
    let addresses = addresses(peer);
    if addresses.is_empty() {
        return Err("the phone isn't on this PC's network".into());
    }
    g7link_adb::ensure_server(server).await.map_err(|e| format!("adb isn't running on this PC: {e}"))?;
    let mut why = String::new();
    for ip in addresses {
        let device = SocketAddr::new(ip, port);
        // A phone adb already reaches (an earlier start, or `adb connect`)
        // says so, and is used as it is.
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
    Err(format!("adb couldn't use the phone's Wireless debugging: {why}"))
}

/// The adb serial of the phone that asked without naming its Wireless
/// debugging port: a connection adb already has to the phone's address, or
/// else one of the phone's open ports in Android's range that adb can use.
/// Either way the phone is checked to be the one with `apk`.
pub(crate) async fn find(server: SocketAddr, peer: &[IpAddr], apk: &str) -> Result<String, String> {
    let addresses = addresses(peer);
    if addresses.is_empty() {
        return Err("the phone isn't on this PC's network".into());
    }
    g7link_adb::ensure_server(server).await.map_err(|e| format!("adb isn't running on this PC: {e}"))?;
    let devices = g7link_adb::list_devices(server).await.unwrap_or_default();
    for device in devices.iter().filter(|d| d.is_online() && at_any(&d.serial, &addresses)) {
        if has_apk_soon(server, &device.serial, apk).await {
            return Ok(device.serial.clone());
        }
    }
    let mut why = "Wireless debugging didn't answer on any of its ports".to_owned();
    for ip in addresses.iter().filter(|ip| ip.is_ipv4()) {
        for port in open_ports(*ip).await.into_iter().take(MAX_CANDIDATES) {
            match route(server, &[*ip], port, apk).await {
                Ok(serial) => return Ok(serial),
                Err(e) => why = e,
            }
        }
    }
    Err(why)
}

/// Whether adb's `serial` is a network device at one of `addresses`.
fn at_any(serial: &str, addresses: &[IpAddr]) -> bool {
    let Some((host, _)) = serial.rsplit_once(':') else { return false };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.parse::<IpAddr>().is_ok_and(|ip| addresses.contains(&ip.to_canonical()))
}

/// The ports in Android's range for Wireless debugging that accept a
/// connection at `ip`, lowest first.
async fn open_ports(ip: IpAddr) -> Vec<u16> {
    let ports: Vec<u16> = PORTS.filter(|p| !NOT_ADB.contains(p)).collect();
    let mut open = Vec::new();
    for chunk in ports.chunks(PROBES_AT_ONCE) {
        let mut probes = tokio::task::JoinSet::new();
        for &port in chunk {
            probes.spawn(async move {
                let connect = tokio::net::TcpStream::connect(SocketAddr::new(ip, port));
                matches!(tokio::time::timeout(PROBE_TIMEOUT, connect).await, Ok(Ok(_))).then_some(port)
            });
        }
        while let Some(done) = probes.join_next().await {
            if let Ok(Some(port)) = done {
                open.push(port);
            }
        }
        if open.len() >= MAX_CANDIDATES {
            break;
        }
    }
    open.sort_unstable();
    open
}

/// The phone's addresses adb is told to try: IPv4 first (adb's own choice
/// for Wireless debugging), each once.
fn addresses(peer: &[IpAddr]) -> Vec<IpAddr> {
    let mut addresses: Vec<IpAddr> = Vec::new();
    for ip in peer.iter().map(IpAddr::to_canonical) {
        if !ip.is_loopback() && !ip.is_unspecified() && !addresses.contains(&ip) {
            addresses.push(ip);
        }
    }
    addresses.sort_by_key(IpAddr::is_ipv6);
    addresses.truncate(MAX_ADDRESSES);
    addresses
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

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use tokio::io::AsyncWriteExt;

    use super::*;

    const APK: &str = "/data/app/~~a==/app.g7link-b==/base.apk";
    const PHONE: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40));

    /// A mock adb server that can connect to a phone at `serial`, which has [`APK`].
    async fn adb_reaching(serial: &'static str) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let Ok(command) = g7link_adb::read_payload(&mut socket).await else { continue };
                if command == format!("host:connect:{serial}") {
                    let said = format!("connected to {serial}");
                    let _ = socket.write_all(format!("OKAY{:04x}{said}", said.len()).as_bytes()).await;
                } else if command.starts_with("host:connect:") {
                    let said = "failed to authenticate";
                    let _ = socket.write_all(format!("OKAY{:04x}{said}", said.len()).as_bytes()).await;
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
    async fn adb_connects_to_the_port_the_phone_named_and_checks_it_is_that_phone() {
        let server = adb_reaching("192.168.1.40:37123").await;
        assert_eq!(route(server, &[PHONE], 37123, APK).await, Ok("192.168.1.40:37123".to_owned()));
        // The phone's address as an IPv6 socket reports it is still that phone.
        let mapped = IpAddr::V6(Ipv4Addr::new(192, 168, 1, 40).to_ipv6_mapped());
        assert_eq!(route(server, &[mapped], 37123, APK).await, Ok("192.168.1.40:37123".to_owned()));

        // A phone that hasn't paired this PC doesn't let its adb in.
        let refused = route(server, &[PHONE], 40000, APK).await.unwrap_err();
        assert!(refused.contains("failed to authenticate"), "{refused}");
        // Over the relay there is no address to connect to.
        assert!(route(server, &[], 37123, APK).await.is_err());
    }

    #[tokio::test]
    async fn only_the_phone_with_the_apps_own_apk_is_the_one_that_asked() {
        let server = adb_reaching("192.168.1.40:37123").await;
        assert_eq!(has_apk(server, "192.168.1.40:37123", APK).await, Some(true));
        // Another install's APK isn't on it: it's somebody else's phone.
        let other = "/data/app/~~c==/app.g7link-d==/base.apk";
        assert_eq!(has_apk(server, "192.168.1.40:37123", other).await, Some(false));
        let stranger = route(server, &[PHONE], 37123, other).await.unwrap_err();
        assert!(stranger.contains("isn't this phone"), "{stranger}");
        // A phone adb can't run anything on yet can't say either way.
        assert_eq!(has_apk(server, "192.168.1.41:40000", APK).await, None);
    }

    #[tokio::test]
    async fn a_phone_that_names_no_port_is_found_by_its_open_ports() {
        // A listener stands in for the phone's Wireless debugging, at a port in Android's range.
        let phone = (32800..33800)
            .find_map(|port| std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).ok())
            .expect("a free port in range");
        let port = phone.local_addr().unwrap().port();
        let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert!(open_ports(loopback).await.contains(&port));
        drop(phone);
    }

    #[tokio::test]
    async fn a_connection_adb_already_has_to_the_phone_is_used() {
        let server = adb_reaching("192.168.1.40:37123").await;
        assert!(at_any("192.168.1.40:37123", &[PHONE]));
        assert!(!at_any("192.168.1.41:37123", &[PHONE]));
        assert!(!at_any("adb-ABC._adb-tls-connect._tcp", &[PHONE]));
        // Nothing listed and nothing open at 192.168.1.40 here: it says so.
        assert!(find(server, &[], APK).await.is_err());
    }

    #[test]
    fn the_phones_addresses_are_tried_ipv4_first_and_once_each() {
        let v6: IpAddr = "fd00::40".parse().unwrap();
        let mapped = IpAddr::V6(Ipv4Addr::new(192, 168, 1, 40).to_ipv6_mapped());
        let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert_eq!(addresses(&[v6, mapped, PHONE, loopback]), vec![PHONE, v6]);
        assert!(addresses(&[]).is_empty());
    }
}
