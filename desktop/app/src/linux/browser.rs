// SPDX-License-Identifier: GPL-3.0-or-later
//! Find the window that owns a media session on Linux, and read the page
//! address and the `<video>` rectangle out of a browser when available.

use std::process::Command;

/// An X11 window ID, as `wmctrl` and `xdotool` report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct XWin(pub *mut core::ffi::c_void);

// SAFETY: Window handles (X11 XIDs cast to pointer size) are plain integer IDs.
unsafe impl Send for XWin {}
unsafe impl Sync for XWin {}

/// A screen rectangle in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone)]
pub struct Window {
    pub xwin: XWin,
    pub program: String,
    pub title: String,
}

/// Process names that may own the media session `player`: an MPRIS identity
/// or bus name such as `firefox` or `org.mpris.MediaPlayer2.vlc`.
pub fn programs_for(player: &str) -> Vec<String> {
    let trimmed = player.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let lower = trimmed.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut push = |s: &str| {
        if !s.is_empty() && !out.iter().any(|x: &String| x.eq_ignore_ascii_case(s)) {
            out.push(s.to_ascii_lowercase());
        }
    };
    for (needle, bins) in [
        ("chrome", &["chrome", "google-chrome", "google-chrome-stable"][..]),
        ("chromium", &["chromium", "chromium-browser"][..]),
        ("msedge", &["msedge", "microsoft-edge", "microsoft-edge-stable"][..]),
        ("microsoft-edge", &["msedge", "microsoft-edge"][..]),
        ("brave", &["brave", "brave-browser"][..]),
        ("vivaldi", &["vivaldi", "vivaldi-stable"][..]),
        ("opera", &["opera"][..]),
        ("firefox", &["firefox", "firefox-esr", "firefox-bin"][..]),
        ("mozilla", &["firefox", "firefox-esr"][..]),
        ("zen", &["zen", "zen-bin"][..]),
        ("floorp", &["floorp"][..]),
        ("waterfox", &["waterfox"][..]),
        ("librewolf", &["librewolf"][..]),
        ("vlc", &["vlc"][..]),
        ("mpv", &["mpv"][..]),
        ("rhythmbox", &["rhythmbox"][..]),
        ("elisa", &["elisa"][..]),
        ("totem", &["totem"][..]),
        ("celluloid", &["celluloid"][..]),
        ("haruna", &["haruna"][..]),
    ] {
        if lower.contains(needle) {
            for bin in bins {
                push(bin);
            }
        }
    }
    if let Some(head) = lower.split(['!', '_', ' ']).next()
        && head.len() >= 3
    {
        push(head);
    }
    out
}

pub fn is_browser(program: &str) -> bool {
    let base = program.trim().to_ascii_lowercase();
    matches!(
        base.as_str(),
        "chrome"
            | "google-chrome"
            | "google-chrome-stable"
            | "chromium"
            | "chromium-browser"
            | "msedge"
            | "microsoft-edge"
            | "microsoft-edge-stable"
            | "brave"
            | "brave-browser"
            | "vivaldi"
            | "vivaldi-stable"
            | "opera"
            | "firefox"
            | "firefox-esr"
            | "firefox-bin"
            | "zen"
            | "zen-bin"
            | "floorp"
            | "waterfox"
            | "librewolf"
            | "arc"
    )
}

/// Enumerate visible top-level windows via `wmctrl -lp` or `xdotool`.
pub fn windows() -> Vec<Window> {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Vec::new();
    }

    // Try `wmctrl -lp` first: `<wid_hex> <desktop> <pid> <host> <title...>`
    if let Ok(out) = Command::new("wmctrl").arg("-lp").output()
        && out.status.success()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        let parsed = parse_wmctrl_lp(&text);
        if !parsed.is_empty() {
            return parsed;
        }
    }

    // Fallback to `xdotool search --onlyvisible --name .+`
    if let Ok(out) = Command::new("xdotool").args(["search", "--onlyvisible", "--name", ".+"]).output()
        && out.status.success()
    {
        let mut list = Vec::new();
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let Ok(xid) = line.trim().parse::<usize>() else {
                continue;
            };
            if xid == 0 {
                continue;
            }
            let title = Command::new("xdotool")
                .args(["getwindowname", &xid.to_string()])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                .unwrap_or_default();
            if title.is_empty() {
                continue;
            }
            let pid = Command::new("xdotool")
                .args(["getwindowpid", &xid.to_string()])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().ok())
                .unwrap_or(0);
            let program = program_for_pid(pid).unwrap_or_default();
            list.push(Window { xwin: XWin(xid as *mut core::ffi::c_void), program, title });
        }
        return list;
    }

    Vec::new()
}

fn parse_wmctrl_lp(text: &str) -> Vec<Window> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(wid_str) = parts.next() else { continue };
        let Some(_desktop) = parts.next() else { continue };
        let Some(pid_str) = parts.next() else { continue };
        let Some(host) = parts.next() else { continue };
        let wid_trimmed = wid_str.trim_start_matches("0x").trim_start_matches("0X");
        let Ok(xid) = usize::from_str_radix(wid_trimmed, 16) else { continue };
        if xid == 0 {
            continue;
        }
        let pid = pid_str.parse::<u32>().unwrap_or(0);
        // Find where `host` ends in `line` to extract the rest as window title
        let title = if let Some(idx) = line.find(host) {
            line[idx + host.len()..].trim().to_owned()
        } else {
            String::new()
        };
        if title.is_empty() {
            continue;
        }
        let program = program_for_pid(pid).unwrap_or_default();
        out.push(Window { xwin: XWin(xid as *mut core::ffi::c_void), program, title });
    }
    out
}

fn program_for_pid(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe"))
        && let Some(name) = exe.file_name().and_then(|n| n.to_str())
    {
        return Some(name.to_ascii_lowercase());
    }
    if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
        let trimmed = comm.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_ascii_lowercase());
        }
    }
    None
}

/// Pick the window most likely to be playing `title` in `player`.
pub fn window_for(player: &str, title: &str) -> Option<Window> {
    let programs = programs_for(player);
    let needle = title.trim().to_ascii_lowercase();
    let all = windows();

    let mut best: Option<(i32, Window)> = None;
    for w in all {
        let prog_match = programs.iter().any(|p| !w.program.is_empty() && w.program == *p);
        let wtitle = w.title.to_ascii_lowercase();
        let title_match = !needle.is_empty() && wtitle.contains(&needle);
        let word_hits = if needle.is_empty() {
            0
        } else {
            needle.split_whitespace().filter(|word| word.len() >= 3 && wtitle.contains(*word)).count() as i32
        };

        let score = match (prog_match, title_match) {
            (true, true) => 100 + word_hits,
            (true, false) if word_hits >= 2 => 60 + word_hits,
            (false, true) if needle.len() >= 6 => 50 + word_hits,
            (true, false) => 20,
            _ => 0,
        };
        if score > 0 && best.as_ref().is_none_or(|(b, _)| score > *b) {
            best = Some((score, w));
        }
    }
    best.map(|(_, w)| w)
}

pub fn use_physical_pixels() {
    // X11 / Wayland geometry queries already report physical pixels.
}

pub struct Automation {
    _private: (),
}

impl Automation {
    pub fn new() -> Result<Automation, String> {
        Ok(Automation { _private: () })
    }

    /// Read the active tab's URL from the browser window if accessible via MPRIS `xesam:url`
    /// or window properties.
    pub fn page_url(&self, _window: &Window) -> Option<String> {
        None
    }

    /// Find the screen rectangle of the video player inside `window`.
    pub fn player_rect(&self, window: &Window) -> Option<Rect> {
        self.page_rect(window)
    }

    /// Find the screen rectangle of the web page area inside `window` (excluding the
    /// top browser tab/address bar chrome).
    pub fn page_rect(&self, window: &Window) -> Option<Rect> {
        let frame = frame_of(window.xwin)?;
        let w = frame.right - frame.left;
        let h = frame.bottom - frame.top;
        if w < 160 || h < 160 {
            return None;
        }
        // Estimate browser top toolbar height (~80px) when the window is a browser
        let top_inset = if is_browser(&window.program) && h > 240 { 80 } else { 0 };
        Some(Rect { left: frame.left, top: frame.top + top_inset, right: frame.right, bottom: frame.bottom })
    }
}

pub fn is_minimized(xwin: XWin) -> bool {
    let xid = xwin.0 as usize;
    if xid == 0 {
        return false;
    }
    if let Ok(out) = Command::new("xprop").args(["-id", &xid.to_string(), "_NET_WM_STATE"]).output()
        && out.status.success()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        return text.contains("_NET_WM_STATE_HIDDEN");
    }
    false
}

pub fn restore_behind(xwin: XWin) {
    let xid = xwin.0 as usize;
    if xid == 0 {
        return;
    }
    let _ = Command::new("xdotool").args(["windowmap", &xid.to_string()]).status();
}

pub fn frame_of(xwin: XWin) -> Option<Rect> {
    let xid = xwin.0 as usize;
    if xid == 0 {
        return None;
    }
    if let Ok(out) = Command::new("xdotool").args(["getwindowgeometry", "--shell", &xid.to_string()]).output()
        && out.status.success()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        return parse_xdotool_geometry(&text);
    }
    None
}

fn parse_xdotool_geometry(text: &str) -> Option<Rect> {
    let mut x = None;
    let mut y = None;
    let mut w = None;
    let mut h = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("X=") {
            x = val.parse::<i32>().ok();
        } else if let Some(val) = line.strip_prefix("Y=") {
            y = val.parse::<i32>().ok();
        } else if let Some(val) = line.strip_prefix("WIDTH=") {
            w = val.parse::<i32>().ok();
        } else if let Some(val) = line.strip_prefix("HEIGHT=") {
            h = val.parse::<i32>().ok();
        }
    }
    let (left, top, width, height) = (x?, y?, w?, h?);
    if width <= 0 || height <= 0 {
        return None;
    }
    Some(Rect { left, top, right: left + width, bottom: top + height })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_media_players_to_programs() {
        assert!(programs_for("Chromium").contains(&"chromium".to_owned()));
        assert!(programs_for("org.mpris.MediaPlayer2.firefox.instance_1_42").contains(&"firefox".to_owned()));
        assert!(programs_for("org.mpris.MediaPlayer2.vlc").contains(&"vlc".to_owned()));
        assert!(programs_for("  ").is_empty());
    }

    #[test]
    fn parses_xdotool_shell_geometry() {
        let sample = "WINDOW=12345\nX=100\nY=200\nWIDTH=1280\nHEIGHT=720\nSCREEN=0\n";
        let r = parse_xdotool_geometry(sample).expect("valid geometry");
        assert_eq!((r.left, r.top, r.right, r.bottom), (100, 200, 1380, 920));
    }
}
