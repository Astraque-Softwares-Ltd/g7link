// SPDX-License-Identifier: GPL-3.0-or-later
//! The focused window, through G7 Linux's companion GNOME Shell extension:
//! for apps that publish nothing on the accessibility bus, the window is all
//! the PC can tell a phone about where its typing goes. Also every window,
//! and bringing one forward (and minimizing it again), for a phone's reply
//! into a session (`agents::reply`).

use std::sync::Mutex;
use std::time::{Duration, Instant};

const SHELL: &str = "org.gnome.Shell";
const PATH: &str = "/org/gnome/Shell/Extensions/G7Linux/Windows";
const INTERFACE: &str = "org.gnome.Shell.Extensions.Windows";
const FOCUS_SIGNAL: &str = "focuswindow";
const CALL_TIMEOUT: Duration = Duration::from_secs(1);
/// The follower tries again after this when the extension isn't there.
const RETRY: Duration = Duration::from_secs(60);
/// Most bytes of a window title kept.
const MAX_TITLE: usize = 256;
/// Most windows read from a list.
const MAX_WINDOWS: usize = 256;
/// `META_WINDOW_NORMAL`: not a dialog, a menu or a tooltip.
const NORMAL: i64 = 0;
/// How long an answer to "does the extension tell fullscreen and minimized" holds.
const RECHECK: Duration = Duration::from_secs(30);

/// A missing answer is asked about again sooner: the extension is off
/// while the screen is locked, and back on unlock.
const RECHECK_MISSING: Duration = Duration::from_secs(5);

static STATES: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

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

/// A window as the extension lists it, for finding a session's terminal.
#[derive(Clone, PartialEq, Eq)]
pub struct Listed {
    pub id: u64,
    pub pid: u32,
    pub class: String,
    /// Compared in memory, never logged.
    pub title: String,
    pub focused: bool,
    pub fullscreen: bool,
    pub minimized: bool,
    /// A normal window, not a dialog or a menu.
    pub normal: bool,
    /// Its workspace's index, -1 on every workspace; `None` when not said.
    pub workspace: Option<i64>,
}

impl std::fmt::Debug for Listed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Listed")
            .field("id", &self.id)
            .field("pid", &self.pid)
            .field("class", &self.class)
            .field("focused", &self.focused)
            .field("fullscreen", &self.fullscreen)
            .field("minimized", &self.minimized)
            .field("normal", &self.normal)
            .field("workspace", &self.workspace)
            .finish_non_exhaustive()
    }
}

#[derive(serde::Deserialize)]
struct Entry {
    id: u64,
    #[serde(default)]
    wm_class: Option<String>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    has_focus: Option<bool>,
    #[serde(default)]
    window_type: Option<i64>,
    #[serde(default)]
    fullscreen: Option<bool>,
    #[serde(default)]
    minimized: Option<bool>,
    #[serde(default)]
    workspace: Option<i64>,
}

fn session() -> Option<zbus::blocking::Connection> {
    zbus::blocking::connection::Builder::session().ok()?.method_timeout(CALL_TIMEOUT).build().ok()
}

fn focused_on(conn: &zbus::blocking::Connection) -> Option<Window> {
    let reply = conn.call_method(Some(SHELL), PATH, Some(INTERFACE), "GetFocusedWindowSync", &()).ok()?;
    let json: String = reply.body().deserialize().ok()?;
    parse(&json)
}

fn cut_title(mut title: String) -> String {
    if title.len() > MAX_TITLE {
        let cut = (0..=MAX_TITLE).rev().find(|&i| title.is_char_boundary(i)).unwrap_or(0);
        title.truncate(cut);
    }
    title
}

fn parse(json: &str) -> Option<Window> {
    let details: Details = serde_json::from_str(json).ok()?;
    let title = cut_title(details.title.unwrap_or_default());
    Some(Window {
        id: details.id,
        class: details.wm_class.unwrap_or_default(),
        pid: details.pid.unwrap_or(0),
        title,
    })
}

/// The windows the extension lists (G7 Linux's own launcher isn't among
/// them), and whether it tells each one's `fullscreen` and `minimized`
/// (companion 1.5 and later).
fn parse_list(json: &str) -> Option<(Vec<Listed>, bool)> {
    let entries: Vec<Entry> = serde_json::from_str(json).ok()?;
    let states =
        !entries.is_empty() && entries.iter().all(|e| e.fullscreen.is_some() && e.minimized.is_some());
    let windows = entries
        .into_iter()
        .take(MAX_WINDOWS)
        .map(|e| Listed {
            id: e.id,
            pid: e.pid.unwrap_or(0),
            class: e.wm_class.unwrap_or_default(),
            title: cut_title(e.title.unwrap_or_default()),
            focused: e.has_focus.unwrap_or(false),
            fullscreen: e.fullscreen.unwrap_or(false),
            minimized: e.minimized.unwrap_or(false),
            normal: e.window_type.is_none_or(|t| t == NORMAL),
            workspace: e.workspace,
        })
        .collect();
    Some((windows, states))
}

fn list_with_states() -> Option<(Vec<Listed>, bool)> {
    let listed = super::non_tokio(|| {
        let conn = session()?;
        let reply = conn.call_method(Some(SHELL), PATH, Some(INTERFACE), "List", &()).ok()?;
        let json: String = reply.body().deserialize().ok()?;
        parse_list(&json)
    });
    if let Some((_, states)) = &listed {
        *STATES.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), *states));
    }
    listed
}

/// Every window but G7 Linux's own, with which has the focus. `None` when
/// the extension doesn't answer.
pub fn list() -> Option<Vec<Listed>> {
    list_with_states().map(|(windows, _)| windows)
}

/// Whether the extension tells which windows are fullscreen or minimized.
pub fn has_states() -> bool {
    if let Some((at, known)) = *STATES.lock().unwrap_or_else(|e| e.into_inner())
        && at.elapsed() < if known { RECHECK } else { RECHECK_MISSING }
    {
        return known;
    }
    list_with_states().is_some_and(|(_, states)| states)
}

/// Brings window `id` forward and gives it the focus (on its workspace).
/// False when the extension refused or isn't there.
pub fn activate(id: u64) -> bool {
    let Ok(id) = u32::try_from(id) else { return false };
    super::non_tokio(|| {
        session().is_some_and(|conn| {
            conn.call_method(Some(SHELL), PATH, Some(INTERFACE), "Activate", &(id,)).is_ok()
        })
    })
}

/// Minimizes window `id`. False when the extension refused or isn't there.
pub fn minimize(id: u64) -> bool {
    let Ok(id) = u32::try_from(id) else { return false };
    super::non_tokio(|| {
        session().is_some_and(|conn| {
            conn.call_method(Some(SHELL), PATH, Some(INTERFACE), "Minimize", &(id,)).is_ok()
        })
    })
}

/// Whether GNOME Shell is on the session bus: the one desktop G7 has a
/// window backend for. Asked of the bus, never of the desktop's name.
pub fn has_shell() -> bool {
    super::non_tokio(|| session().is_some_and(|conn| super::dbus_name_has_owner(&conn, SHELL)))
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

    #[test]
    fn reads_the_window_list_and_its_states() {
        let json = r#"[
            {"id":11,"title":"Home","wm_class":"org.gnome.Nautilus","pid":50,"has_focus":true,"window_type":0,"fullscreen":false,"minimized":false,"workspace":0},
            {"id":12,"title":"claude","wm_class":"org.gnome.Ptyxis","pid":100,"has_focus":false,"window_type":0,"fullscreen":true,"minimized":true,"workspace":-1},
            {"id":13,"title":"Preferences","wm_class":"org.gnome.Ptyxis","pid":100,"window_type":3,"fullscreen":false,"minimized":false}
        ]"#;
        let (windows, states) = parse_list(json).expect("a list");
        assert!(states);
        assert_eq!(windows.len(), 3);
        assert!(windows[0].focused && !windows[0].fullscreen && windows[0].normal);
        assert!(!windows[1].focused && windows[1].fullscreen && windows[1].minimized);
        assert_eq!(
            (windows[1].pid, windows[1].class.as_str(), windows[1].title.as_str()),
            (100, "org.gnome.Ptyxis", "claude")
        );
        assert!(!windows[2].normal, "a dialog");
        assert!(!windows[2].focused, "missing focus is no focus");
        assert_eq!(
            windows.iter().map(|w| w.workspace).collect::<Vec<_>>(),
            [Some(0), Some(-1), None],
            "on every workspace is -1; unsaid is unknown"
        );

        // Before 1.5 the flags aren't there.
        let old = r#"[{"id":11,"title":"Home","wm_class":"x","pid":5,"has_focus":true,"window_type":0}]"#;
        assert_eq!(parse_list(old).map(|(w, states)| (w.len(), states)), Some((1, false)));
        assert_eq!(parse_list("[]").map(|(w, states)| (w.len(), states)), Some((0, false)));
        assert_eq!(parse_list("{}").map(|_| ()), None);

        let many = format!("[{}]", vec![r#"{"id":1,"fullscreen":false,"minimized":false}"#; 300].join(","));
        assert_eq!(parse_list(&many).map(|(w, _)| w.len()), Some(MAX_WINDOWS));
        let shown = format!("{:?}", parse_list(json).expect("a list").0[1]);
        assert!(!shown.contains("claude"), "no title in logs: {shown}");
    }
}
