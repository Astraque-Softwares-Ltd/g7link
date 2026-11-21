// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Result;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU16, Ordering};

pub const DEFAULT_PORT: u16 = g7link_core::reach::DEFAULT_PORT;
pub const MDNS_PORT: u16 = 5353;

/// The installer's or the Tailscale fix's note that G7 Link is allowed in on
/// Tailscale's interface: `<interface> <port>` lines.
const TAILNET_MARKER: &str = "firewall_tailnet";
/// `/sys/class/net/<name>/type` of a tunnel such as Tailscale's (`ARPHRD_NONE`).
const ARPHRD_NONE: &str = "65534";

/// The node port the firewall fix opened in this run (0: none), in case its
/// note couldn't be written.
static FIREWALL_VERIFIED: AtomicU16 = AtomicU16::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Firewall {
    pub on: bool,
    pub allowed: bool,
    pub blocked: bool,
}

/// Which firewall is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirewallKind {
    Ufw,
    Firewalld,
    Nftables,
}

/// Whether the firewall lets phones in on Tailscale's interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailnetFirewall {
    /// The firewall is off, or it lets G7 Link in there.
    Open,
    /// ufw is on, with no note of a rule for the interface and port.
    Ufw,
    /// firewalld's zone for the interface doesn't allow the port.
    Firewalld,
    /// nftables, whose rules the app can't judge.
    Nftables,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub name: String,
    pub trusted: bool,
}

/// Tailscale's interface on this PC and its address on the tailnet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tailnet {
    pub iface: String,
    pub addr: IpAddr,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Adapters {
    pub local_address: bool,
    pub vpn: bool,
    pub tailnet: Option<Tailnet>,
}

/// The UDP port the node listens on: the node's own word, else the port file
/// it saved, else the default.
pub fn active_node_port() -> u16 {
    if let Some(port) = crate::core_host::node().and_then(|n| n.listen_port()).filter(|&p| p > 0) {
        return port;
    }
    let port_file = crate::core_host::host().data_dir.join("port");
    fs::read_to_string(port_file).ok().and_then(|s| parse_port_file(&s)).unwrap_or(DEFAULT_PORT)
}

/// The IPv4 port from the node's port file, "`<v4>`" or "`<v4> <v6>`".
pub fn parse_port_file(content: &str) -> Option<u16> {
    content.split_whitespace().next()?.parse::<u16>().ok().filter(|&p| p > 0)
}

/// The ports the firewall fixes open for G7 Link: the default one and the
/// node's.
fn opened_ports(node_port: u16) -> Vec<u16> {
    let mut ports = vec![DEFAULT_PORT];
    if node_port != DEFAULT_PORT && node_port != MDNS_PORT && node_port > 0 {
        ports.push(node_port);
    }
    ports
}

fn record_firewall_verified(node_port: u16) {
    FIREWALL_VERIFIED.store(node_port, Ordering::SeqCst);
    let verified_path = crate::core_host::host().data_dir.join("firewall_verified");
    let lines: String = opened_ports(node_port).iter().map(|p| format!("{p}\n")).collect();
    if let Err(error) = crate::atomic_file::write(&verified_path, lines.as_bytes()) {
        tracing::warn!(%error, "can't note the firewall rule");
    }
}

fn is_firewall_verified(port: u16) -> bool {
    if port != 0 && FIREWALL_VERIFIED.load(Ordering::SeqCst) == port {
        return true;
    }
    let verified_path = crate::core_host::host().data_dir.join("firewall_verified");
    fs::read_to_string(verified_path).is_ok_and(|content| verified_lists_port(&content, port))
}

/// Whether the note that the firewall was opened (one port per line) names
/// `port`. Only the port in use counts.
pub fn verified_lists_port(content: &str, port: u16) -> bool {
    port != 0 && content.lines().any(|l| l.trim().parse::<u16>().ok() == Some(port))
}

pub fn parse_ufw_status_for_port(output: &str, port: u16) -> Option<Firewall> {
    let lower = output.to_ascii_lowercase();
    if lower.contains("status: inactive") {
        return Some(Firewall { on: false, allowed: true, blocked: false });
    }
    if !lower.contains("status: active") {
        return None;
    }

    let port_str = port.to_string();
    let port_tcp = format!("{port}/tcp");
    let port_udp = format!("{port}/udp");
    let port_colon = format!("{port}:");

    let mut tcp_allowed = false;
    let mut udp_allowed = false;
    let mut blocked = false;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("Status:")
            || trimmed.starts_with("To ")
            || trimmed.starts_with("--")
        {
            continue;
        }
        let tokens: Vec<&str> = trimmed.split_whitespace().collect();
        if tokens.len() < 2 {
            continue;
        }
        let port_spec = tokens[0].to_ascii_lowercase();
        let touches_port = port_spec == port_str
            || port_spec == port_tcp
            || port_spec == port_udp
            || port_spec.starts_with(&port_colon);
        if !touches_port {
            continue;
        }
        let upper_line = trimmed.to_ascii_uppercase();
        if upper_line.contains("DENY") || upper_line.contains("REJECT") {
            blocked = true;
        } else if upper_line.contains("ALLOW") {
            if port_spec == port_str {
                tcp_allowed = true;
                udp_allowed = true;
            } else if port_spec == port_tcp || port_spec.ends_with("/tcp") {
                tcp_allowed = true;
            } else if port_spec == port_udp || port_spec.ends_with("/udp") {
                udp_allowed = true;
            }
        }
    }

    let allowed = !blocked && tcp_allowed && udp_allowed;
    Some(Firewall { on: true, allowed, blocked })
}

pub fn parse_firewalld_output_for_port(state: &str, ports: &str, rich_rules: &str, port: u16) -> Firewall {
    if !state.trim().eq_ignore_ascii_case("running") {
        return Firewall { on: false, allowed: true, blocked: false };
    }
    let port_tcp = format!("{port}/tcp");
    let port_dash = format!("{port}-");
    let port_attr = format!("port=\"{port}\"");

    let mut blocked = false;
    let mut rich_allowed = false;
    for line in rich_rules.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains(&port_attr) {
            if lower.contains("reject") || lower.contains("drop") {
                blocked = true;
            } else if lower.contains("accept") {
                rich_allowed = true;
            }
        }
    }

    let port_allowed =
        ports.split_whitespace().any(|p| p.eq_ignore_ascii_case(&port_tcp) || p.starts_with(&port_dash));
    Firewall { on: true, allowed: !blocked && (port_allowed || rich_allowed), blocked }
}

fn check_ufw_config_fallback(port: u16) -> Option<Firewall> {
    let conf = fs::read_to_string("/etc/ufw/ufw.conf").ok()?;
    let enabled = conf.lines().any(|l| {
        let t = l.trim();
        t.eq_ignore_ascii_case("ENABLED=yes") || t.eq_ignore_ascii_case("ENABLED=\"yes\"")
    });
    if !enabled {
        return Some(Firewall { on: false, allowed: true, blocked: false });
    }
    let port_str = port.to_string();
    for rules_path in ["/etc/ufw/user.rules", "/lib/ufw/user.rules"] {
        if let Ok(rules) = fs::read_to_string(rules_path) {
            let touches = |l: &str| l.contains(&port_str);
            let has_drop =
                rules.lines().any(|l| touches(l) && (l.contains("-j DROP") || l.contains("-j REJECT")));
            let has_accept = rules.lines().any(|l| touches(l) && l.contains("-j ACCEPT"));
            return Some(Firewall { on: true, allowed: !has_drop && has_accept, blocked: has_drop });
        }
    }
    // Non-root users cannot read `/etc/ufw/user.rules` (0640 root:root); consult verified state.
    Some(Firewall { on: true, allowed: is_firewall_verified(port), blocked: false })
}

fn is_service_active(service: &str) -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", service])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn has_command(cmd: &str) -> bool {
    if cmd.is_empty() || cmd.contains('/') || cmd.contains('\0') {
        return false;
    }
    use std::os::unix::fs::MetadataExt;
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let extra = [Path::new("/usr/sbin"), Path::new("/sbin"), Path::new("/usr/bin"), Path::new("/bin")];
    std::env::split_paths(&path_var).chain(extra.into_iter().map(Path::to_path_buf)).any(|dir| {
        let candidate = dir.join(cmd);
        fs::metadata(&candidate).is_ok_and(|m| {
            if !m.is_file() {
                return false;
            }
            m.mode() & 0o111 != 0
        })
    })
}

pub fn firewall(_exe: &Path) -> Result<Firewall> {
    Ok(firewall_state(active_node_port()).1)
}

/// The firewall's state for the node's `port`, and which firewall it is
/// (`None` when none is on).
pub fn firewall_state(port: u16) -> (Option<FirewallKind>, Firewall) {
    let kind_of = |fw: Firewall, kind| (fw.on.then_some(kind), fw);

    // 1. Try ufw status
    if let Ok(out) = Command::new("ufw").arg("status").output()
        && out.status.success()
    {
        let stdout = String::from_utf8_lossy(&out.stdout);
        if let Some(fw) = parse_ufw_status_for_port(&stdout, port) {
            return kind_of(fw, FirewallKind::Ufw);
        }
    }
    if is_service_active("ufw")
        && let Some(fw) = check_ufw_config_fallback(port)
    {
        return kind_of(fw, FirewallKind::Ufw);
    }

    // 2. Try firewalld (firewall-cmd --state, --list-ports, --list-rich-rules)
    if let Ok(state_out) = Command::new("firewall-cmd").arg("--state").output() {
        let state_str = String::from_utf8_lossy(&state_out.stdout);
        if state_str.trim().eq_ignore_ascii_case("running") {
            let ports_str = Command::new("firewall-cmd")
                .arg("--list-ports")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            let rich_str = Command::new("firewall-cmd")
                .arg("--list-rich-rules")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            let mut fw = parse_firewalld_output_for_port(&state_str, &ports_str, &rich_str, port);
            if !fw.allowed && !fw.blocked && is_firewall_verified(port) {
                fw.allowed = true;
            }
            return kind_of(fw, FirewallKind::Firewalld);
        }
    }

    // 3. Try nftables
    if is_service_active("nftables") {
        let ruleset = Command::new("nft")
            .args(["list", "ruleset"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .or_else(|| fs::read_to_string("/etc/nftables.conf").ok());
        if let Some(conf) = ruleset {
            let lower = conf.to_ascii_lowercase();
            let has_policy_drop = lower.contains("policy drop");
            let port_str = port.to_string();
            let has_port = lower.contains(&port_str);
            if has_policy_drop || has_port {
                let verified = is_firewall_verified(port);
                let fw = Firewall { on: true, allowed: has_port || verified, blocked: false };
                return kind_of(fw, FirewallKind::Nftables);
            }
        }
    }

    (None, Firewall { on: false, allowed: true, blocked: false })
}

/// Whether the firewall lets phones in on Tailscale's interface `iface` to
/// the node's `port`. The app can't read ufw's rules without root, so there
/// only the installer's or the fix's note counts.
pub fn tailnet_firewall(kind: Option<FirewallKind>, iface: &str, port: u16) -> TailnetFirewall {
    let Some(kind) = kind else { return TailnetFirewall::Open };
    let marker = crate::core_host::host().data_dir.join(TAILNET_MARKER);
    if fs::read_to_string(marker).is_ok_and(|content| tailnet_marker_matches(&content, iface, port)) {
        return TailnetFirewall::Open;
    }
    match kind {
        FirewallKind::Ufw => TailnetFirewall::Ufw,
        FirewallKind::Firewalld => {
            let zone = Command::new("firewall-cmd")
                .arg(format!("--get-zone-of-interface={iface}"))
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                .filter(|zone| !zone.is_empty());
            // An interface in no zone of its own is in the default zone, which
            // `--list-ports` lists without `--zone`.
            let mut list = Command::new("firewall-cmd");
            if let Some(zone) = &zone {
                list.arg(format!("--zone={zone}"));
            }
            let ports = list
                .arg("--list-ports")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            if firewalld_tailnet_allowed(zone.as_deref(), &ports, port) {
                TailnetFirewall::Open
            } else {
                TailnetFirewall::Firewalld
            }
        }
        FirewallKind::Nftables => TailnetFirewall::Nftables,
    }
}

/// Whether firewalld lets UDP `port` in on an interface in `zone`: the
/// `trusted` zone lets everything in, other zones need the port listed.
pub fn firewalld_tailnet_allowed(zone: Option<&str>, ports: &str, port: u16) -> bool {
    zone.is_some_and(|z| z.eq_ignore_ascii_case("trusted")) || firewalld_ports_allow_udp(ports, port)
}

/// Whether firewalld's `--list-ports` output ("`7718/udp 8000-8100/udp`")
/// covers UDP `port`.
pub fn firewalld_ports_allow_udp(ports: &str, port: u16) -> bool {
    ports.split_whitespace().any(|entry| {
        let Some((range, proto)) = entry.rsplit_once('/') else { return false };
        if !proto.eq_ignore_ascii_case("udp") {
            return false;
        }
        match range.split_once('-') {
            Some((low, high)) => match (low.parse::<u16>(), high.parse::<u16>()) {
                (Ok(low), Ok(high)) => (low..=high).contains(&port),
                _ => false,
            },
            None => range.parse::<u16>() == Ok(port),
        }
    })
}

/// Whether the note left by the installer or the Tailscale fix names this
/// interface and port. A changed port or interface makes it stale.
pub fn tailnet_marker_matches(content: &str, iface: &str, port: u16) -> bool {
    content.lines().any(|line| {
        let mut words = line.split_whitespace();
        match (words.next(), words.next().and_then(|p| p.parse::<u16>().ok()), words.next()) {
            (Some(name), Some(noted), None) => name == iface && noted == port,
            _ => false,
        }
    })
}

/// Parses `nmcli -t -f GENERAL.CONNECTION,GENERAL.ZONE device show` output into a
/// map from connection name to firewall zone.
pub fn parse_nmcli_device_zones(stdout: &str) -> HashMap<String, String> {
    let mut zones = HashMap::new();
    let mut current_conn: Option<String> = None;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(conn) = trimmed.strip_prefix("GENERAL.CONNECTION:") {
            let name = super::wifi::unescape_nmcli(conn.trim());
            if !name.is_empty() && name != "--" {
                current_conn = Some(name);
            } else {
                current_conn = None;
            }
        } else if let Some(zone) = trimmed.strip_prefix("GENERAL.ZONE:") {
            let zone_trimmed = zone.trim();
            if let Some(conn) = current_conn.take()
                && !zone_trimmed.is_empty()
                && zone_trimmed != "--"
            {
                zones.insert(conn, zone_trimmed.to_ascii_lowercase());
            }
        }
    }
    zones
}

/// Parses `nmcli -t -f DEVICE,NAME,TYPE,STATE connection show --active` output and combines it
/// with firewall zones from `device show`, marking `public` firewall zones as `trusted: false`.
pub fn parse_nmcli_active_networks(active_stdout: &str, zones: &HashMap<String, String>) -> Vec<Network> {
    let mut list = Vec::new();
    for line in active_stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Split from the right: `...:TYPE:STATE`, and from the left: `DEVICE:NAME`.
        let Some((rest, state)) = trimmed.rsplit_once(':') else {
            continue;
        };
        let Some((dev_and_name, kind)) = rest.rsplit_once(':') else {
            continue;
        };
        let Some((device, raw_name)) = dev_and_name.split_once(':') else {
            continue;
        };
        if state != "activated" && state != "activating" {
            continue;
        }
        if device == "lo"
            || kind == "loopback"
            || kind == "bridge"
            || kind == "tun"
            || kind == "wireguard"
            || super::wol::is_virtual_interface_name(device)
        {
            continue;
        }
        let name = super::wifi::unescape_nmcli(raw_name.trim());
        if name.is_empty() {
            continue;
        }
        let trusted =
            !zones.get(&name).is_some_and(|z| z.eq_ignore_ascii_case("public") || z.starts_with("public"));
        if !list.iter().any(|n: &Network| n.name == name) {
            list.push(Network { name, trusted });
        }
    }
    list
}

/// Whether an interface's name (lowercased) is a VPN's. Tailscale's isn't:
/// with a tailnet address it's how the phone gets here, without one it's off.
fn is_vpn_name(lower: &str) -> bool {
    ["tun", "tap", "wg", "nordlynx", "ppp", "zt"].iter().any(|prefix| lower.starts_with(prefix))
}

/// Whether the interface `name` is a tunnel such as Tailscale's, by its
/// kind in `/sys/class/net`.
fn is_tunnel(name: &str) -> bool {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return false;
    }
    fs::read_to_string(Path::new("/sys/class/net").join(name).join("type"))
        .is_ok_and(|kind| kind.trim() == ARPHRD_NONE)
}

/// The interfaces that are up (not loopback), one entry per address
/// `getifaddrs` lists: its IPv4 or IPv6 address, or `None` for other kinds.
fn up_interface_addrs() -> Vec<(String, Option<IpAddr>)> {
    let mut entries = Vec::new();
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();

    // SAFETY: `getifaddrs` allocates and populates a linked list freed by `freeifaddrs`.
    if unsafe { libc::getifaddrs(&mut ifap) } == 0 && !ifap.is_null() {
        let mut cur = ifap;
        // SAFETY: iterating valid `ifaddrs` nodes before `freeifaddrs`; each
        // `ifa_addr` is cast to the `sockaddr` kind its family names.
        unsafe {
            while let Some(ifa) = cur.as_ref() {
                cur = ifa.ifa_next;
                if ifa.ifa_name.is_null() {
                    continue;
                }
                let flags = ifa.ifa_flags as i32;
                if (flags & libc::IFF_UP) == 0 || (flags & libc::IFF_LOOPBACK) != 0 {
                    continue;
                }
                let Ok(name) = std::ffi::CStr::from_ptr(ifa.ifa_name).to_str() else {
                    continue;
                };
                if name.eq_ignore_ascii_case("lo") {
                    continue;
                }
                let addr = if ifa.ifa_addr.is_null() {
                    None
                } else {
                    match i32::from((*ifa.ifa_addr).sa_family) {
                        libc::AF_INET => {
                            let sin_addr = (*ifa.ifa_addr.cast::<libc::sockaddr_in>()).sin_addr.s_addr;
                            Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(sin_addr))))
                        }
                        libc::AF_INET6 => {
                            let octets = (*ifa.ifa_addr.cast::<libc::sockaddr_in6>()).sin6_addr.s6_addr;
                            Some(IpAddr::V6(Ipv6Addr::from(octets)))
                        }
                        _ => None,
                    }
                };
                entries.push((name.to_owned(), addr));
            }
            libc::freeifaddrs(ifap);
        }
    }
    entries
}

/// What the interfaces that are up say about this PC's networks.
#[derive(Debug, Default, PartialEq, Eq)]
struct Interfaces {
    /// Interfaces with a usable IPv4 address (not VPNs, not Tailscale).
    ipv4: Vec<String>,
    vpn: bool,
    tailnet: Option<Tailnet>,
}

/// Sorts the `entries` of [`up_interface_addrs`]. Tailscale's interface is
/// one up with a tailnet address, named `tailscale…` or a tunnel; it isn't
/// counted as a VPN, and its IPv4 address is preferred.
fn sort_interfaces(entries: &[(String, Option<IpAddr>)], is_tunnel: impl Fn(&str) -> bool) -> Interfaces {
    let mut tailnet: Option<Tailnet> = None;
    let mut tailnet_ifaces: Vec<&str> = Vec::new();
    for (name, addr) in entries {
        let Some(addr) = *addr else { continue };
        if !g7link_core::is_tailnet_ip(addr)
            || !(name.to_ascii_lowercase().starts_with("tailscale") || is_tunnel(name))
        {
            continue;
        }
        if !tailnet_ifaces.contains(&name.as_str()) {
            tailnet_ifaces.push(name);
        }
        if tailnet.as_ref().is_none_or(|found| found.addr.is_ipv6() && addr.is_ipv4()) {
            tailnet = Some(Tailnet { iface: name.clone(), addr });
        }
    }

    let mut out = Interfaces { tailnet, ..Interfaces::default() };
    for (name, addr) in entries {
        if tailnet_ifaces.contains(&name.as_str()) {
            continue;
        }
        if is_vpn_name(&name.to_ascii_lowercase()) {
            out.vpn = true;
            continue;
        }
        if let Some(IpAddr::V4(ip)) = addr
            && !ip.is_loopback()
            && !ip.is_link_local()
            && !ip.is_unspecified()
            && !out.ipv4.contains(name)
        {
            out.ipv4.push(name.clone());
        }
    }
    out
}

fn interfaces() -> Interfaces {
    sort_interfaces(&up_interface_addrs(), is_tunnel)
}

/// Whether `/sys/class/net` lists a VPN interface that is up, given its
/// `(name, operstate)` pairs. Tailscale's interface (`tailnet`) isn't one.
fn sys_net_has_vpn(ifaces: impl IntoIterator<Item = (String, String)>, tailnet: Option<&str>) -> bool {
    ifaces.into_iter().any(|(name, state)| {
        let name = name.to_ascii_lowercase();
        let state = state.trim().to_ascii_lowercase();
        (state == "up" || state == "unknown")
            && !tailnet.is_some_and(|t| t.eq_ignore_ascii_case(&name))
            && is_vpn_name(&name)
    })
}

pub fn networks() -> Result<Vec<Network>> {
    if let Ok(active_out) = Command::new("nmcli")
        .args(["-t", "-f", "DEVICE,NAME,TYPE,STATE", "connection", "show", "--active"])
        .output()
        && active_out.status.success()
    {
        let zones = Command::new("nmcli")
            .args(["-t", "-f", "GENERAL.CONNECTION,GENERAL.ZONE", "device", "show"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| parse_nmcli_device_zones(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default();
        let active_str = String::from_utf8_lossy(&active_out.stdout);
        let parsed = parse_nmcli_active_networks(&active_str, &zones);
        if !parsed.is_empty() {
            return Ok(parsed);
        }
    }

    let list = interfaces().ipv4.into_iter().map(|name| Network { name, trusted: true }).collect();
    Ok(list)
}

pub fn adapters() -> Adapters {
    let Interfaces { ipv4, mut vpn, tailnet } = interfaces();

    if !vpn && let Ok(entries) = fs::read_dir("/sys/class/net") {
        let ifaces = entries.flatten().map(|entry| {
            let state = fs::read_to_string(entry.path().join("operstate")).unwrap_or_default();
            (entry.file_name().to_string_lossy().into_owned(), state)
        });
        vpn = sys_net_has_vpn(ifaces, tailnet.as_ref().map(|t| t.iface.as_str()));
    }

    Adapters { local_address: !ipv4.is_empty(), vpn, tailnet }
}

/// Returns the names of optional session-relevant helper commands that are not
/// currently installed on `PATH` (`wl-copy`/`wl-paste` on Wayland, `xclip` on
/// X11, and `xdg-open`).
pub fn missing_tools() -> Vec<&'static str> {
    let mut missing = Vec::new();
    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()) {
        for tool in ["wl-copy", "wl-paste"] {
            if !has_command(tool) {
                missing.push(tool);
            }
        }
    }
    if std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty()) && !has_command("xclip") {
        missing.push("xclip");
    }
    if !has_command("xdg-open") {
        missing.push("xdg-open");
    }
    missing
}

fn session_dbus_has_owner(bus_name: &str) -> bool {
    super::non_tokio(|| {
        let Ok(conn) = zbus::blocking::Connection::session() else {
            return false;
        };
        super::dbus_name_has_owner(&conn, bus_name)
    })
}

fn has_libxtst() -> bool {
    const CANDIDATES: &[&str] = &[
        "/usr/lib/x86_64-linux-gnu/libXtst.so.6",
        "/lib/x86_64-linux-gnu/libXtst.so.6",
        "/usr/lib64/libXtst.so.6",
        "/usr/lib/libXtst.so.6",
        "/usr/lib/aarch64-linux-gnu/libXtst.so.6",
    ];
    CANDIDATES.iter().any(|p| Path::new(p).exists()) || has_command("xdotool")
}

fn has_v4l2loopback() -> bool {
    if Path::new("/sys/module/v4l2loopback").exists() {
        return true;
    }
    for bin in ["modinfo", "/sbin/modinfo", "/usr/sbin/modinfo"] {
        if Command::new(bin)
            .arg("v4l2loopback")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            return true;
        }
    }
    false
}

/// Linux-specific checks for features that need something installed or
/// allowed first: `(id, title, ok, detail, fix)`. These don't stop the phone
/// from connecting; the Doctor lists them after the connection checks.
pub fn linux_checks() -> Vec<(String, String, bool, String, Option<&'static str>)> {
    use crate::i18n::tr;
    let mut out = Vec::new();
    let mut check = |id: &str, ok: bool, ready: String, needs: String, how: String| {
        out.push((
            id.to_owned(),
            if ok { ready } else { needs },
            ok,
            if ok { String::new() } else { how },
            None,
        ));
    };

    // GNOME hides tray icons unless the AppIndicator extension is on.
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if desktop.to_ascii_uppercase().contains("GNOME") {
        check(
            "linux-appindicator",
            session_dbus_has_owner("org.kde.StatusNotifierWatcher"),
            tr!("Doctor", "G7 Link is in the top bar"),
            tr!("Doctor", "G7 Link can't show its icon in the top bar"),
            tr!(
                "Doctor",
                "GNOME shows app icons there only with the AppIndicator extension. Install it (gnome-shell-extension-appindicator) and turn it on in Extensions."
            ),
        );
    }

    let uinput_writable = OpenOptions::new().write(true).open("/dev/uinput").is_ok();
    let portal_available = session_dbus_has_owner("org.freedesktop.portal.Desktop");
    let x11_xtst = std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty()) && has_libxtst();
    check(
        "linux-remote-input",
        uinput_writable || portal_available || x11_xtst,
        tr!("Doctor", "Your phone can control this PC"),
        tr!("Doctor", "Your phone can't control this PC yet"),
        tr!(
            "Doctor",
            "The touchpad and keyboard on your phone need the desktop portal (xdg-desktop-portal) or access to /dev/uinput."
        ),
    );

    check(
        "linux-fuse",
        Path::new("/dev/fuse").exists() && (has_command("fusermount3") || has_command("fusermount")),
        tr!("Doctor", "Phone storage can open in your file manager"),
        tr!("Doctor", "Phone storage needs one more step"),
        tr!("Doctor", "Install fuse3 to browse your phone's files in your file manager."),
    );

    check(
        "linux-audio",
        has_command("pactl") || has_command("wpctl") || has_command("pw-play") || has_command("aplay"),
        tr!("Doctor", "Sound is ready"),
        tr!("Doctor", "Calls and alerts can't play sound yet"),
        tr!("Doctor", "Install pipewire-pulse (or pulseaudio-utils) for call audio and Find my PC."),
    );

    // Sharing this PC's screen on Wayland goes through the ScreenCast portal
    // and GStreamer's PipeWire source; on X11 it reads the screen directly.
    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()) {
        check(
            "linux-screencast",
            portal_available && crate::pc_mirror::pipewire_capture_installed(),
            tr!("Doctor", "Your phone can show this PC's screen"),
            tr!("Doctor", "Showing this PC's screen needs one more step"),
            tr!(
                "Doctor",
                "Install gstreamer1.0-pipewire and gstreamer1.0-tools, and the desktop portal (xdg-desktop-portal) if it's missing."
            ),
        );
    }

    check(
        "linux-v4l2loopback",
        has_v4l2loopback(),
        tr!("Doctor", "Phone as webcam is ready"),
        tr!("Doctor", "Phone as webcam needs one more step"),
        tr!("Doctor", "Install v4l2loopback-dkms, then restart, so apps can use your phone as a camera."),
    );

    out
}

pub fn build_ufw_fix_script(node_port: u16) -> String {
    let port_cmds: Vec<String> = opened_ports(node_port)
        .iter()
        .map(|p| {
            format!(
                "ufw allow from \"$cidr\" to any port {p} proto tcp && ufw allow from \"$cidr\" to any port {p} proto udp"
            )
        })
        .collect();
    let joined = port_cmds.join(" && ");
    format!(
        "for cidr in 192.168.0.0/16 10.0.0.0/8 172.16.0.0/12 fe80::/10; do \
           {joined} && \
           ufw allow from \"$cidr\" to any port {MDNS_PORT} proto udp || exit 1; \
         done"
    )
}

pub fn build_firewalld_fix_script(node_port: u16) -> String {
    let ports = opened_ports(node_port);
    let mut cmds = Vec::new();
    for cidr in ["192.168.0.0/16", "10.0.0.0/8", "172.16.0.0/12"] {
        for &p in &ports {
            cmds.push(format!(
                "firewall-cmd --permanent --add-rich-rule='rule family=\"ipv4\" source address=\"{cidr}\" port port=\"{p}\" protocol=\"tcp\" accept'"
            ));
            cmds.push(format!(
                "firewall-cmd --permanent --add-rich-rule='rule family=\"ipv4\" source address=\"{cidr}\" port port=\"{p}\" protocol=\"udp\" accept'"
            ));
        }
        cmds.push(format!(
            "firewall-cmd --permanent --add-rich-rule='rule family=\"ipv4\" source address=\"{cidr}\" port port=\"{MDNS_PORT}\" protocol=\"udp\" accept'"
        ));
    }
    for &p in &ports {
        cmds.push(format!(
            "firewall-cmd --permanent --add-rich-rule='rule family=\"ipv6\" source address=\"fe80::/10\" port port=\"{p}\" protocol=\"tcp\" accept'"
        ));
        cmds.push(format!(
            "firewall-cmd --permanent --add-rich-rule='rule family=\"ipv6\" source address=\"fe80::/10\" port port=\"{p}\" protocol=\"udp\" accept'"
        ));
    }
    cmds.push(format!(
        "firewall-cmd --permanent --add-rich-rule='rule family=\"ipv6\" source address=\"fe80::/10\" port port=\"{MDNS_PORT}\" protocol=\"udp\" accept'"
    ));
    cmds.push("firewall-cmd --reload".into());
    cmds.join(" && ")
}

pub fn fix_firewall(_exe: &Path) -> Result<bool> {
    let port = active_node_port();
    if is_service_active("ufw") || has_command("ufw") {
        let script = build_ufw_fix_script(port);
        let status = super::elevated::run_elevated(Path::new("sh"), &["-c", &script]);
        if matches!(status, Ok(0)) {
            record_firewall_verified(port);
            return Ok(true);
        }
        return Ok(false);
    }
    if is_service_active("firewalld") || has_command("firewall-cmd") {
        let script = build_firewalld_fix_script(port);
        let status = super::elevated::run_elevated(Path::new("sh"), &["-c", &script]);
        if matches!(status, Ok(0)) {
            record_firewall_verified(port);
            return Ok(true);
        }
        return Ok(false);
    }
    Ok(false)
}

/// The command that lets G7 Link in on Tailscale's interface `iface` with
/// ufw, or `None` for a name that can't be an interface's: it goes into a
/// root shell, so only 1 to 15 of `[A-Za-z0-9_.-]` pass.
pub fn build_ufw_tailnet_script(iface: &str, port: u16) -> Option<String> {
    let valid_name = (1..=15).contains(&iface.len())
        && iface.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    (valid_name && port != 0).then(|| format!("ufw allow in on {iface} to any port {port} proto udp"))
}

/// Lets G7 Link in on Tailscale's interface with ufw (after the password
/// prompt) and notes it, since the app can't read the rule back. Returns
/// whether the rule was added.
pub fn fix_firewall_tailnet() -> bool {
    let Some(tailnet) = interfaces().tailnet else { return false };
    let port = active_node_port();
    let Some(script) = build_ufw_tailnet_script(&tailnet.iface, port) else {
        tracing::warn!("Tailscale's interface has a name the firewall fix won't use");
        return false;
    };
    if !matches!(super::elevated::run_elevated(Path::new("sh"), &["-c", &script]), Ok(0)) {
        return false;
    }
    let marker = crate::core_host::host().data_dir.join(TAILNET_MARKER);
    if let Err(error) = crate::atomic_file::write(&marker, format!("{} {port}\n", tailnet.iface).as_bytes()) {
        tracing::warn!(%error, "can't note the Tailscale firewall rule");
    }
    true
}

pub fn open_firewall_settings() -> bool {
    Command::new("gufw")
        .spawn()
        .or_else(|_| Command::new("firewall-config").spawn())
        .or_else(|_| Command::new("plasma-firewall").spawn())
        .or_else(|_| Command::new("gnome-control-center").arg("network").spawn())
        .is_ok()
}

pub fn open_network_settings() -> bool {
    Command::new("nm-connection-editor")
        .spawn()
        .or_else(|_| Command::new("gnome-control-center").arg("network").spawn())
        .or_else(|_| Command::new("gnome-control-center").arg("wifi").spawn())
        .or_else(|_| Command::new("plasma-nm").spawn())
        .or_else(|_| Command::new("systemsettings").arg("kcm_networkmanagement").spawn())
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ufw_status(output: &str) -> Option<Firewall> {
        parse_ufw_status_for_port(output, DEFAULT_PORT)
    }

    fn parse_firewalld_output(state: &str, ports: &str) -> Firewall {
        parse_firewalld_output_for_port(state, ports, "", DEFAULT_PORT)
    }

    #[test]
    fn parses_ufw_inactive_active_allowed_and_blocked() {
        assert_eq!(
            parse_ufw_status("Status: inactive\n"),
            Some(Firewall { on: false, allowed: true, blocked: false })
        );

        let allowed_output = "\
Status: active

To                         Action      From
--                         ------      ----
22/tcp                     ALLOW       Anywhere
7718/tcp                   ALLOW       Anywhere
7718/udp                   ALLOW       Anywhere
";
        assert_eq!(
            parse_ufw_status(allowed_output),
            Some(Firewall { on: true, allowed: true, blocked: false })
        );

        let combined_allowed_output = "\
Status: active

To                         Action      From
--                         ------      ----
7718                       ALLOW       Anywhere
";
        assert_eq!(
            parse_ufw_status(combined_allowed_output),
            Some(Firewall { on: true, allowed: true, blocked: false })
        );

        let subnet_scoped_allowed_output = "\
Status: active

To                         Action      From
--                         ------      ----
7718/tcp                   ALLOW       192.168.0.0/16
7718/udp                   ALLOW       192.168.0.0/16
5353/udp                   ALLOW       192.168.0.0/16
";
        assert_eq!(
            parse_ufw_status(subnet_scoped_allowed_output),
            Some(Firewall { on: true, allowed: true, blocked: false })
        );

        let blocked_output = "\
Status: active

To                         Action      From
--                         ------      ----
7718/tcp                   DENY        Anywhere
";
        assert_eq!(
            parse_ufw_status(blocked_output),
            Some(Firewall { on: true, allowed: false, blocked: true })
        );

        let unconfigured_active = "\
Status: active

To                         Action      From
--                         ------      ----
22/tcp                     ALLOW       Anywhere
";
        assert_eq!(
            parse_ufw_status(unconfigured_active),
            Some(Firewall { on: true, allowed: false, blocked: false })
        );

        assert_eq!(parse_ufw_status("ufw: command not found"), None);
    }

    #[test]
    fn parses_firewalld_running_ports_and_scoped_rich_rules() {
        assert_eq!(
            parse_firewalld_output("not running\n", ""),
            Firewall { on: false, allowed: true, blocked: false }
        );
        assert_eq!(
            parse_firewalld_output("running\n", "80/tcp 443/tcp\n"),
            Firewall { on: true, allowed: false, blocked: false }
        );
        assert_eq!(
            parse_firewalld_output("running\n", "5353/udp 7718/tcp 7718/udp\n"),
            Firewall { on: true, allowed: true, blocked: false }
        );

        let rich = "rule family=\"ipv4\" source address=\"192.168.0.0/16\" port port=\"49152\" protocol=\"tcp\" accept\n";
        assert_eq!(
            parse_firewalld_output_for_port("running\n", "", rich, 49152),
            Firewall { on: true, allowed: true, blocked: false }
        );
    }

    #[test]
    fn builds_scoped_ufw_and_firewalld_scripts_with_dynamic_port() {
        let ufw_script = build_ufw_fix_script(45678);
        assert!(ufw_script.contains("port 7718 proto tcp"));
        assert!(ufw_script.contains("port 45678 proto tcp"));
        assert!(ufw_script.contains("port 5353 proto udp"));
        assert!(ufw_script.contains("fe80::/10"));

        let fwd_script = build_firewalld_fix_script(45678);
        assert!(fwd_script.contains("--add-rich-rule='rule family=\"ipv4\" source address=\"192.168.0.0/16\" port port=\"7718\" protocol=\"tcp\" accept'"));
        assert!(fwd_script.contains("port=\"45678\""));
        assert!(fwd_script.contains("family=\"ipv6\" source address=\"fe80::/10\""));
        assert!(fwd_script.ends_with("firewall-cmd --reload"));
    }

    #[test]
    fn port_file_gives_the_ipv4_port_with_one_or_two_numbers() {
        assert_eq!(parse_port_file("40314\n"), Some(40314));
        assert_eq!(parse_port_file("40314 40315\n"), Some(40314));
        assert_eq!(parse_port_file("  51000   51000  "), Some(51000));
        assert_eq!(parse_port_file("0 40314"), None);
        assert_eq!(parse_port_file(""), None);
        assert_eq!(parse_port_file("port"), None);
        assert_eq!(parse_port_file("70000"), None);
    }

    #[test]
    fn only_an_entry_for_the_port_in_use_counts() {
        // The installer's note names the default port; the node listens elsewhere.
        assert!(!verified_lists_port("7718\n", 40314));
        assert!(verified_lists_port("7718\n", 7718));
        assert!(verified_lists_port("7718\n40314\n", 40314));
        assert!(!verified_lists_port("", 40314));
        assert!(!verified_lists_port("7718\n", 0));

        let ufw = "\
Status: active

To                         Action      From
--                         ------      ----
7718/tcp                   ALLOW       192.168.0.0/16
7718/udp                   ALLOW       192.168.0.0/16
";
        assert_eq!(
            parse_ufw_status_for_port(ufw, 40314),
            Some(Firewall { on: true, allowed: false, blocked: false })
        );
        assert_eq!(
            parse_firewalld_output_for_port("running\n", "7718/tcp 7718/udp\n", "", 40314),
            Firewall { on: true, allowed: false, blocked: false }
        );
        let rich = "rule family=\"ipv4\" source address=\"192.168.0.0/16\" port port=\"7718\" protocol=\"udp\" accept\n";
        assert_eq!(
            parse_firewalld_output_for_port("running\n", "", rich, 40314),
            Firewall { on: true, allowed: false, blocked: false }
        );
    }

    #[test]
    fn the_fixes_open_the_default_and_the_node_port() {
        assert_eq!(opened_ports(DEFAULT_PORT), [DEFAULT_PORT]);
        assert_eq!(opened_ports(40314), [DEFAULT_PORT, 40314]);
        assert_eq!(opened_ports(MDNS_PORT), [DEFAULT_PORT]);
    }

    #[test]
    fn builds_the_tailscale_rule_for_a_plain_interface_name_only() {
        assert_eq!(
            build_ufw_tailnet_script("tailscale0", 41641).as_deref(),
            Some("ufw allow in on tailscale0 to any port 41641 proto udp")
        );
        assert_eq!(
            build_ufw_tailnet_script("ts_1.home-a", 41641).as_deref(),
            Some("ufw allow in on ts_1.home-a to any port 41641 proto udp")
        );
        assert!(build_ufw_tailnet_script("abcdefghijklmno", 41641).is_some(), "15 characters");
        for refused in [
            "tailscale0; rm -rf /",
            "",
            "abcdefghijklmnop",
            "tailscale 0",
            "ts0$(reboot)",
            "ts0`id`",
            "ts0|sh",
            "ts0&",
            "ts0>x",
            "'ts0'",
            "ts0\n",
            "tailscalé",
        ] {
            assert_eq!(build_ufw_tailnet_script(refused, 41641), None, "{refused:?}");
        }
        assert_eq!(build_ufw_tailnet_script("tailscale0", 0), None);
    }

    #[test]
    fn the_tailscale_note_must_name_this_interface_and_port() {
        assert!(tailnet_marker_matches("tailscale0 7718\n", "tailscale0", 7718));
        assert!(tailnet_marker_matches("wg9 1\ntailscale0 41641\n", "tailscale0", 41641));
        assert!(!tailnet_marker_matches("tailscale0 7718\n", "tailscale0", 40314));
        assert!(!tailnet_marker_matches("tailscale0 7718\n", "tailscale1", 7718));
        assert!(!tailnet_marker_matches("tailscale0 7718 extra\n", "tailscale0", 7718));
        assert!(!tailnet_marker_matches("tailscale0\n", "tailscale0", 7718));
        assert!(!tailnet_marker_matches("", "tailscale0", 7718));
    }

    #[test]
    fn firewalld_allows_tailscale_in_the_trusted_zone_or_with_the_udp_port() {
        assert!(firewalld_tailnet_allowed(Some("trusted"), "", 41641));
        assert!(firewalld_tailnet_allowed(Some("public"), "22/tcp 41641/udp", 41641));
        assert!(firewalld_tailnet_allowed(None, "41000-42000/udp", 41641));
        assert!(!firewalld_tailnet_allowed(Some("public"), "41641/tcp", 41641));
        assert!(!firewalld_tailnet_allowed(Some("public"), "7718/udp 41642-41700/udp", 41641));
        assert!(!firewalld_tailnet_allowed(Some("home"), "", 41641));
        assert!(!firewalld_ports_allow_udp("x-y/udp 41641 /udp", 41641));
    }

    fn entry(name: &str, addr: Option<&str>) -> (String, Option<IpAddr>) {
        (name.into(), addr.map(|a| a.parse().unwrap()))
    }

    #[test]
    fn tailscale_is_found_and_isnt_a_vpn() {
        let entries = [
            entry("wlp2s0", Some("192.168.1.20")),
            entry("tailscale0", None),
            entry("tailscale0", Some("fd7a:115c:a1e0::1")),
            entry("tailscale0", Some("100.78.127.19")),
            entry("tailscale0", Some("fe80::1")),
        ];
        let found = sort_interfaces(&entries, |_| false);
        assert!(!found.vpn, "{found:?}");
        assert_eq!(found.ipv4, ["wlp2s0"]);
        assert_eq!(
            found.tailnet,
            Some(Tailnet { iface: "tailscale0".into(), addr: "100.78.127.19".parse().unwrap() })
        );

        // Any tunnel with a tailnet address is Tailscale's (another name, IPv6 only).
        let entries = [entry("ts9", Some("fd7a:115c:a1e0::5")), entry("eth0", Some("10.0.0.2"))];
        let found = sort_interfaces(&entries, |name| name == "ts9");
        assert!(!found.vpn);
        assert_eq!(
            found.tailnet,
            Some(Tailnet { iface: "ts9".into(), addr: "fd7a:115c:a1e0::5".parse().unwrap() })
        );

        // A carrier's 100.64/10 address on an ordinary interface isn't Tailscale.
        let entries = [entry("wwan0", Some("100.70.1.2"))];
        let found = sort_interfaces(&entries, |_| false);
        assert_eq!(found.tailnet, None);
        assert_eq!(found.ipv4, ["wwan0"]);
    }

    #[test]
    fn other_vpns_are_still_vpns() {
        // Tailscale's interface without a tailnet address is Tailscale turned off: no VPN.
        let entries = [entry("tailscale0", Some("fe80::1")), entry("eth0", Some("192.168.1.2"))];
        let found = sort_interfaces(&entries, |_| false);
        assert!(!found.vpn);
        assert_eq!(found.tailnet, None);
        // WireGuard is one.
        let entries = [entry("wg0", Some("10.8.0.2")), entry("tailscale0", Some("100.78.127.19"))];
        let found = sort_interfaces(&entries, |_| true);
        assert!(found.vpn);
        assert!(found.tailnet.is_some());
        assert!(found.ipv4.is_empty());

        let sys = |list: &[(&str, &str)]| -> Vec<(String, String)> {
            list.iter().map(|(n, s)| ((*n).to_owned(), format!("{s}\n"))).collect()
        };
        let listed = sys(&[("lo", "unknown"), ("wlp2s0", "up"), ("tailscale0", "unknown")]);
        assert!(!sys_net_has_vpn(listed.clone(), Some("tailscale0")));
        assert!(!sys_net_has_vpn(listed, None));
        assert!(sys_net_has_vpn(sys(&[("tailscale0", "unknown"), ("wg0", "up")]), Some("tailscale0")));
        assert!(!sys_net_has_vpn(sys(&[("wg0", "down")]), None));
    }

    #[test]
    fn parses_nmcli_networks_and_marks_public_zone_untrusted() {
        let dev_show = "\
GENERAL.CONNECTION:Cafe\\:Free
GENERAL.ZONE:public

GENERAL.CONNECTION:Home WiFi
GENERAL.ZONE:home
";
        let zones = parse_nmcli_device_zones(dev_show);
        let active = "\
wlan0:Cafe\\:Free:802-11-wireless:activated
eth0:Home WiFi:802-3-ethernet:activated
lo:lo:loopback:activated
tun0:wg0:tun:activated
";
        let nets = parse_nmcli_active_networks(active, &zones);
        assert_eq!(
            nets,
            vec![
                Network { name: "Cafe:Free".into(), trusted: false },
                Network { name: "Home WiFi".into(), trusted: true },
            ]
        );
    }
}
