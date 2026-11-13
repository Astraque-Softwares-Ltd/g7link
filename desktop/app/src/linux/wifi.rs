// SPDX-License-Identifier: GPL-3.0-or-later
//! Linux Wi-Fi scan and connection helper for Instant Hotspot via NetworkManager (`nmcli`).

use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InRange {
    pub profile: String,
    pub connectable: bool,
    pub connected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    NotInRange,
    InRange,
    Connected,
}

pub fn unescape_nmcli(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some(':') => out.push(':'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn tidy(mut names: Vec<String>) -> Vec<String> {
    names.retain(|n| !n.trim().is_empty());
    names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b)));
    names.dedup();
    names
}

fn parse_saved_profiles(stdout: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in stdout.lines() {
        // Format from `nmcli -t -f TYPE,NAME connection show`:
        // 802-11-wireless:MyHotspot
        if let Some((kind, name)) = line.split_once(':')
            && (kind == "802-11-wireless" || kind == "wifi")
        {
            let unescaped = unescape_nmcli(name);
            let trimmed = unescaped.trim();
            if !trimmed.is_empty() {
                names.push(trimmed.to_string());
            }
        }
    }
    tidy(names)
}

pub fn saved_networks() -> Vec<String> {
    let Ok(out) = Command::new("nmcli").args(["-t", "-f", "TYPE,NAME", "connection", "show"]).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_saved_profiles(&String::from_utf8_lossy(&out.stdout))
}

pub fn scan() {
    let _ = Command::new("nmcli").args(["device", "wifi", "rescan"]).output();
}

fn parse_in_range(stdout: &str) -> Vec<InRange> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        // Format from `nmcli -t -f ACTIVE,SSID device wifi list`:
        // yes:MyHotspot
        if let Some((active, raw_ssid)) = line.split_once(':') {
            let unescaped = unescape_nmcli(raw_ssid);
            let ssid = unescaped.trim();
            if !ssid.is_empty() {
                out.push(InRange {
                    profile: ssid.to_string(),
                    connectable: true,
                    connected: active.trim() == "yes",
                });
            }
        }
    }
    out
}

fn reach_in(networks: &[InRange], profile: &str) -> Reach {
    let mine = networks.iter().filter(|n| n.profile == profile);
    if mine.clone().any(|n| n.connected) {
        Reach::Connected
    } else if mine.clone().any(|n| n.connectable) {
        Reach::InRange
    } else {
        Reach::NotInRange
    }
}

pub fn reach(profile: &str) -> Reach {
    let Ok(out) = Command::new("nmcli").args(["-t", "-f", "ACTIVE,SSID", "device", "wifi", "list"]).output()
    else {
        return Reach::NotInRange;
    };
    if !out.status.success() {
        return Reach::NotInRange;
    }
    let networks = parse_in_range(&String::from_utf8_lossy(&out.stdout));
    reach_in(&networks, profile)
}

pub fn connect(profile: &str) -> Result<(), String> {
    let trimmed = profile.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') || trimmed.chars().any(char::is_control) {
        return Err("invalid Wi-Fi profile name".into());
    }
    let out = Command::new("nmcli")
        .args(["connection", "up", "id", trimmed])
        .output()
        .map_err(|e| format!("failed to run nmcli: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Err(if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("nmcli exited with {}", out.status)
        })
    }
}

/// How much of the internet the PC reaches, as NetworkManager last checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connectivity {
    Full,
    /// Online, but behind a sign-in page.
    Portal,
    /// A network without the internet.
    Limited,
    None,
    /// NetworkManager isn't there, or doesn't check.
    Unknown,
}

fn parse_connectivity(stdout: &str) -> Connectivity {
    match stdout.trim() {
        "full" => Connectivity::Full,
        "portal" => Connectivity::Portal,
        "limited" => Connectivity::Limited,
        "none" => Connectivity::None,
        _ => Connectivity::Unknown,
    }
}

/// The PC's connectivity, without asking NetworkManager to check again.
pub fn connectivity() -> Connectivity {
    match Command::new("nmcli").args(["-t", "-f", "CONNECTIVITY", "general"]).output() {
        Ok(out) if out.status.success() => parse_connectivity(&String::from_utf8_lossy(&out.stdout)),
        _ => Connectivity::Unknown,
    }
}

/// The Wi-Fi networks in range, from NetworkManager's last scan unless `rescan`.
pub fn in_range(rescan: bool) -> Vec<InRange> {
    let mode = if rescan { "yes" } else { "no" };
    let args = ["-t", "-f", "ACTIVE,SSID", "device", "wifi", "list", "--rescan", mode];
    let Ok(out) = Command::new("nmcli").args(args).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_in_range(&String::from_utf8_lossy(&out.stdout))
}

/// The Wi-Fi network the PC is on, if any.
pub fn active_network() -> Option<String> {
    in_range(false).into_iter().find(|n| n.connected).map(|n| n.profile)
}

fn parse_wifi_device(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        // Format from `nmcli -t -f DEVICE,TYPE,STATE device`: wlp2s0:wifi:connected
        let mut fields = line.splitn(3, ':');
        let (device, kind, state) = (fields.next()?, fields.next()?, fields.next()?);
        let safe = |c: char| c.is_ascii_alphanumeric() || "-_.".contains(c);
        let plain = !device.is_empty() && device.chars().all(safe) && device != "." && device != "..";
        (kind == "wifi" && state == "connected" && plain).then(|| device.to_owned())
    })
}

/// Bytes the connected Wi-Fi interface has received and sent since it came up.
pub fn wifi_bytes() -> Option<u64> {
    let out = Command::new("nmcli").args(["-t", "-f", "DEVICE,TYPE,STATE", "device"]).output().ok()?;
    let device = parse_wifi_device(&String::from_utf8_lossy(&out.stdout))?;
    let read = |what: &str| -> Option<u64> {
        let path = format!("/sys/class/net/{device}/statistics/{what}");
        std::fs::read_to_string(path).ok()?.trim().parse().ok()
    };
    Some(read("rx_bytes")?.saturating_add(read("tx_bytes")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_connectivity_and_the_connected_wifi_device() {
        assert_eq!(parse_connectivity("full\n"), Connectivity::Full);
        assert_eq!(parse_connectivity("limited"), Connectivity::Limited);
        assert_eq!(parse_connectivity("none"), Connectivity::None);
        assert_eq!(parse_connectivity("portal"), Connectivity::Portal);
        assert_eq!(parse_connectivity(""), Connectivity::Unknown);

        let devices = concat!(
            "lo:loopback:connected (externally)\n",
            "p2p-dev-wlp2s0:wifi-p2p:disconnected\n",
            "wlp2s0:wifi:connected\n",
        );
        assert_eq!(parse_wifi_device(devices).as_deref(), Some("wlp2s0"));
        assert_eq!(parse_wifi_device("wlp2s0:wifi:disconnected\n"), None);
        // A name that could leave the statistics folder is never used.
        assert_eq!(parse_wifi_device("../x:wifi:connected\n"), None);
    }

    #[test]
    fn parses_and_tidies_saved_wifi_profiles() {
        let sample =
            "802-11-wireless:Pixel 9a\n802-3-ethernet:Wired connection 1\nwifi:home\nwifi:Home\nwifi:home\n";
        let names = parse_saved_profiles(sample);
        assert_eq!(names, vec!["Home", "home", "Pixel 9a"]);
    }

    #[test]
    fn reach_in_reports_connected_in_range_or_not() {
        let sample = "yes:Home\nno:Pixel 9a\n";
        let list = parse_in_range(sample);
        assert_eq!(reach_in(&list, "Home"), Reach::Connected);
        assert_eq!(reach_in(&list, "Pixel 9a"), Reach::InRange);
        assert_eq!(reach_in(&list, "Office"), Reach::NotInRange);
    }
}
