// SPDX-License-Identifier: GPL-3.0-or-later
//! The focused window, through G7 Linux's companion GNOME Shell extension:
//! for apps that publish nothing on the accessibility bus, the window is all
//! the PC can tell a phone about where its typing goes.

use std::time::Duration;

const SHELL: &str = "org.gnome.Shell";
const PATH: &str = "/org/gnome/Shell/Extensions/G7Linux/Windows";
const INTERFACE: &str = "org.gnome.Shell.Extensions.Windows";
const FOCUS_SIGNAL: &str = "focuswindow";
const CALL_TIMEOUT: Duration = Duration::from_secs(1);
/// The follower tries again after this when the extension isn't there.
const RETRY: Duration = Duration::from_secs(60);
/// Most bytes of a window title kept.
const MAX_TITLE: usize = 256;

/// The window that has the focus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: u64,
    pub class: String,
    pub pid: u32,
    pub title: String,
}

#[derive(serde::Deserialize)]
struct Details {
    id: u64,
    #[serde(default)]
    wm_class: Option<String>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    title: Option<String>,
}

fn session() -> Option<zbus::blocking::Connection> {
    zbus::blocking::connection::Builder::session().ok()?.method_timeout(CALL_TIMEOUT).build().ok()
}

fn focused_on(conn: &zbus::blocking::Connection) -> Option<Window> {
    let reply = conn.call_method(Some(SHELL), PATH, Some(INTERFACE), "GetFocusedWindowSync", &()).ok()?;
    let json: String = reply.body().deserialize().ok()?;
    parse(&json)
}

fn parse(json: &str) -> Option<Window> {
    let details: Details = serde_json::from_str(json).ok()?;
    let mut title = details.title.unwrap_or_default();
    if title.len() > MAX_TITLE {
        let cut = (0..=MAX_TITLE).rev().find(|&i| title.is_char_boundary(i)).unwrap_or(0);
        title.truncate(cut);
    }
    Some(Window {
        id: details.id,
        class: details.wm_class.unwrap_or_default(),
        pid: details.pid.unwrap_or(0),
        title,
    })
}

/// Calls `changed` with each newly focused window. One thread for the app's
/// lifetime; quiet when the extension isn't installed.
pub fn follow(changed: fn(Window)) {
    let spawned = std::thread::Builder::new().name("shell-windows".into()).spawn(move || {
        loop {
            follow_once(changed);
            std::thread::sleep(RETRY);
        }
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "can't follow the focused window");
    }
}

fn follow_once(changed: fn(Window)) {
    let Some(conn) = session() else { return };
    let messages = zbus::blocking::MessageIterator::from(&conn);
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else { return };
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(SHELL)
        .and_then(|b| b.path(PATH))
        .and_then(|b| b.interface(INTERFACE))
        .and_then(|b| b.member(FOCUS_SIGNAL))
        .map(|b| b.build());
    let Ok(rule) = rule else { return };
    if dbus.add_match_rule(rule).is_err() {
        return;
    }
    // Nothing to follow without the extension.
    let Some(first) = focused_on(&conn) else { return };
    let Ok(owner) = dbus.get_name_owner(SHELL.try_into().expect("a valid bus name")) else { return };
    changed(first);
    for message in messages {
        let Ok(message) = message else { break };
        let header = message.header();
        let from_shell = header.sender().is_some_and(|sender| sender.as_str() == owner.as_str());
        if !from_shell || header.member().is_none_or(|member| member.as_str() != FOCUS_SIGNAL) {
            continue;
        }
        if let Some(window) = focused_on(&conn) {
            changed(window);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_extensions_window_details() {
        let json = r#"{"id":3520792488,"title":"Inbox (3)","wm_class":"org.gnome.Evolution","pid":6607,"has_focus":true}"#;
        assert_eq!(
            parse(json),
            Some(Window {
                id: 3_520_792_488,
                class: "org.gnome.Evolution".into(),
                pid: 6607,
                title: "Inbox (3)".into()
            })
        );
        let long = format!(r#"{{"id":1,"title":"{}"}}"#, "é".repeat(200));
        assert!(parse(&long).is_some_and(|w| w.title.len() <= MAX_TITLE && w.class.is_empty()));
        assert_eq!(parse("not json"), None);
    }
}
