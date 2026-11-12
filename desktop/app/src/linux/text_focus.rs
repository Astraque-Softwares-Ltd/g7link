// SPDX-License-Identifier: GPL-3.0-or-later
//! Whether what has the keyboard focus takes text, from the desktop's
//! accessibility bus (AT-SPI): a focused object with the `EDITABLE` state.
//! Apps that don't publish accessibility (some Electron and Chromium builds
//! until a screen reader runs) aren't seen.

use std::collections::HashMap;
use std::time::Duration;

use zbus::zvariant::{OwnedObjectPath, OwnedValue};

const REGISTRY: &str = "org.a11y.atspi.Registry";
const REGISTRY_PATH: &str = "/org/a11y/atspi/registry";
const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const OBJECT_EVENTS: &str = "org.a11y.atspi.Event.Object";
const FOCUS_EVENT: &str = "object:state-changed:focused";
/// `ATSPI_STATE_EDITABLE`.
const STATE_EDITABLE: u32 = 7;
/// An app that doesn't answer in this time is taken as not taking text.
const CALL_TIMEOUT: Duration = Duration::from_millis(500);
/// The watcher tries the accessibility bus again after this when it can't reach it.
const RETRY: Duration = Duration::from_secs(60);

/// Calls `changed` with whether the focus takes text each time that changes.
/// One thread for the app's lifetime.
pub fn watch(changed: fn(bool)) {
    let _ = std::thread::Builder::new().name("text-focus".into()).spawn(move || {
        loop {
            follow(changed);
            std::thread::sleep(RETRY);
        }
    });
}

/// The accessibility bus, which is separate from the session bus.
fn a11y_bus() -> Option<zbus::blocking::Connection> {
    let session = zbus::blocking::Connection::session().ok()?;
    let reply = session
        .call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some("org.a11y.Bus"), "GetAddress", &())
        .ok()?;
    let address: String = reply.body().deserialize().ok()?;
    zbus::blocking::connection::Builder::address(address.as_str())
        .ok()?
        .method_timeout(CALL_TIMEOUT)
        .build()
        .ok()
}

/// Follows focus changes until the accessibility bus connection ends.
fn follow(changed: fn(bool)) {
    let Some(conn) = a11y_bus() else { return };
    let messages = zbus::blocking::MessageIterator::from(&conn);
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else { return };
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(OBJECT_EVENTS)
        .and_then(|b| b.member("StateChanged"))
        .and_then(|b| b.arg(0, "focused"))
        .map(|b| b.build());
    let Ok(rule) = rule else { return };
    if dbus.add_match_rule(rule).is_err() {
        return;
    }
    // Apps send focus events only once someone has asked for them. Newer
    // registries take the properties to include and an app filter too.
    let with_filters = (FOCUS_EVENT, Vec::<String>::new(), "");
    let registered = conn
        .call_method(Some(REGISTRY), REGISTRY_PATH, Some(REGISTRY), "RegisterEvent", &with_filters)
        .or_else(|_| {
            conn.call_method(Some(REGISTRY), REGISTRY_PATH, Some(REGISTRY), "RegisterEvent", &(FOCUS_EVENT,))
        });
    if let Err(error) = registered {
        tracing::debug!(%error, "text focus: the accessibility registry refused");
        return;
    }

    let mut editable = false;
    for message in messages {
        let Ok(message) = message else { break };
        let header = message.header();
        let (Some(sender), Some(path)) = (header.sender(), header.path()) else { continue };
        let Some(gained) = focus_gained(&message) else { continue };
        let now = gained && takes_text(&conn, sender.as_str(), path.as_str());
        if now != editable {
            editable = now;
            changed(editable);
        }
    }
}

/// Whether a `StateChanged` signal is the focus arriving (`Some(true)`) or
/// leaving (`Some(false)`) its object.
fn focus_gained(message: &zbus::Message) -> Option<bool> {
    let body = message.body();
    let (detail, gained) = body
        .deserialize::<(String, i32, i32, OwnedValue, HashMap<String, OwnedValue>)>()
        .map(|(detail, gained, ..)| (detail, gained))
        .or_else(|_| {
            body.deserialize::<(String, i32, i32, OwnedValue, (String, OwnedObjectPath))>()
                .map(|(detail, gained, ..)| (detail, gained))
        })
        .ok()?;
    (detail == "focused").then_some(gained == 1)
}

/// Whether the object at `path` in the app `sender` is editable now.
fn takes_text(conn: &zbus::blocking::Connection, sender: &str, path: &str) -> bool {
    conn.call_method(Some(sender), path, Some(ACCESSIBLE), "GetState", &())
        .ok()
        .and_then(|reply| reply.body().deserialize::<Vec<u32>>().ok())
        .is_some_and(|states| has_state(&states, STATE_EDITABLE))
}

/// Whether `state` is set in AT-SPI's state set (32-bit words, lowest first).
fn has_state(words: &[u32], state: u32) -> bool {
    words.get((state / 32) as usize).is_some_and(|word| word >> (state % 32) & 1 == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editable_is_read_from_the_state_set() {
        // A focused, editable text box: FOCUSABLE, FOCUSED, EDITABLE and more.
        assert!(has_state(&[0b1_1000_1000_0000, 0], STATE_EDITABLE));
        // A button: focused, not editable.
        assert!(!has_state(&[0b1_1000_0000_0000, 0], STATE_EDITABLE));
        assert!(!has_state(&[], STATE_EDITABLE));
        // A state in the second word.
        assert!(has_state(&[0, 1 << 3], 35));
    }
}
