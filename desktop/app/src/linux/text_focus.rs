// SPDX-License-Identifier: GPL-3.0-or-later
//! What has the keyboard focus, from the desktop's accessibility bus
//! (AT-SPI), for a phone's typing box:
//!
//! - whether it takes text (a focused object with the `EDITABLE` state), so
//!   the phone can open its keyboard, and whether it's a terminal, which
//!   pastes with Ctrl+Shift+V;
//! - while a phone watches, the focused field (`input.text_state`): which
//!   field it is, and for a field whose app shares its text, the text around
//!   the caret, so the phone's box mirrors it.
//!
//! Apps that don't publish accessibility (kitty, Alacritty) aren't seen; for
//! those the focused window, from G7 Linux's Shell extension, is what the
//! phone is told. Chromium and Electron apps publish their windows but build
//! the rest only when asked: while a phone watches, the app that has the
//! focus is asked once ([`wake`]), unless the user turned that off.
//!
//! For a phone's reply into a session (`agents::reply`), it also finds a
//! terminal app's tab by a title G7 wrote on it and selects that tab (by the
//! window's `page.next` / `page.previous` actions, or a tab list's
//! `Selection`; never a key or a click), and reads the text of the focused
//! terminal, bounded in size and time, to prove which session is on screen.
//! Titles and text are compared and dropped: never kept, logged or sent.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use g7link_core::{RemoteTextState, TextFieldKind, remote_limits};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use super::field_identity::{FieldTable, StableKey};
use super::rich_text::{self, Flat, Link, Node};
use super::shell_windows::{self, Window};

const REGISTRY: &str = "org.a11y.atspi.Registry";
const REGISTRY_PATH: &str = "/org/a11y/atspi/registry";
const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const ACTION: &str = "org.a11y.atspi.Action";
const SELECTION: &str = "org.a11y.atspi.Selection";
const TEXT: &str = "org.a11y.atspi.Text";
const HYPERTEXT: &str = "org.a11y.atspi.Hypertext";
const HYPERLINK: &str = "org.a11y.atspi.Hyperlink";
const APPLICATION: &str = "org.a11y.atspi.Application";
const DOCUMENT: &str = "org.a11y.atspi.Document";
const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const OBJECT_EVENTS: &str = "org.a11y.atspi.Event.Object";
const FOCUS_EVENT: &str = "object:state-changed:focused";
/// Asked for only while a phone watches: apps send these for every change.
const TEXT_EVENTS: [&str; 3] =
    ["object:text-changed", "object:text-caret-moved", "object:text-selection-changed"];
const TEXT_MEMBERS: [&str; 3] = ["TextChanged", "TextCaretMoved", "TextSelectionChanged"];
/// `ATSPI_STATE_*` used here.
const STATE_ACTIVE: u32 = 1;
const STATE_EDITABLE: u32 = 7;
const STATE_FOCUSED: u32 = 12;
const STATE_MULTI_LINE: u32 = 17;
const STATE_SELECTED: u32 = 23;
const STATE_SHOWING: u32 = 25;
/// `ATSPI_ROLE_PASSWORD_TEXT`, `ATSPI_ROLE_TERMINAL` (VTE terminals such as
/// GNOME Terminal and Ptyxis publish it), `ATSPI_ROLE_PARAGRAPH`,
/// `ATSPI_ROLE_APPLICATION` and `ATSPI_ROLE_DOCUMENT_WEB`.
const ROLE_PASSWORD_TEXT: u32 = 40;
/// `ATSPI_ROLE_PAGE_TAB`, `ATSPI_ROLE_PAGE_TAB_LIST` and `ATSPI_ROLE_PANEL`.
const ROLE_PAGE_TAB: u32 = 37;
const ROLE_PAGE_TAB_LIST: u32 = 38;
const ROLE_PANEL: u32 = 39;
const ROLE_TERMINAL: u32 = 60;
const ROLE_PARAGRAPH: u32 = 73;
const ROLE_APPLICATION: u32 = 75;
const ROLE_DOCUMENT_WEB: u32 = 95;
/// Roles that start a line of their own in a rich field: list, list item,
/// paragraph, heading, section, block quote, article.
const BLOCK_ROLES: [u32; 7] = [31, 32, ROLE_PARAGRAPH, 83, 85, 105, 109];
/// The toolkit Chromium and Electron apps publish.
const CHROMIUM: &str = "Chromium";
/// An app that doesn't answer in this time is taken as not taking text.
const CALL_TIMEOUT: Duration = Duration::from_millis(500);
/// The watcher tries the accessibility bus again after this when it can't reach it.
const RETRY: Duration = Duration::from_secs(60);
/// A state goes out this long after the last change that called for it, and
/// at most once in [`MIN_GAP`].
const DEBOUNCE: Duration = Duration::from_millis(40);
const MIN_GAP: Duration = Duration::from_millis(100);
/// How often, while a phone watches a field whose text isn't shared, the PC
/// checks whether someone used its own keyboard or mouse.
const ACTIVITY_POLL: Duration = Duration::from_millis(500);
/// Input this long after the phone's own was typed, or this long before it
/// started, is still the phone's: a busy compositor handles keys late.
const OURS_FOR: Duration = Duration::from_millis(400);
const OURS_BEFORE: Duration = Duration::from_millis(50);
/// Edits a phone meant for a window still run in the field found in it this
/// soon after.
const ALIAS_FOR: Duration = Duration::from_secs(2);
/// Looking for the focused field of an app just asked to publish it: tries,
/// the gap between them, and how far one look goes.
const WAKE_TRIES: u32 = 6;
const WAKE_GAP: Duration = Duration::from_millis(250);
const WAKE_NODES: usize = 2000;
const WAKE_TIME: Duration = Duration::from_millis(300);
/// How far up from a field its web document is looked for.
const MAX_ANCESTORS: usize = 30;
/// Reads of a shared field that may fail in a row (the app changing it under
/// the read) before it's taken as not shared for [`READ_PAUSE`].
const MAX_READ_FAILURES: u32 = 3;
const READ_PAUSE: Duration = Duration::from_secs(2);
/// How long after a window gets the focus an app has to tell of its focused
/// field before the window is taken as the field.
const WINDOW_GRACE: Duration = Duration::from_millis(150);
/// Characters read before and after the caret of a shared field, before
/// cutting to the protocol's byte limits.
const READ_BEFORE: i32 = 2048;
const READ_AFTER: i32 = 512;
/// A terminal's text read for a reply's proof: at most this many bytes of
/// its end, the windows looked at, the objects walked, and how long it may
/// all take.
const TERMINAL_TEXT_BYTES: usize = 64 * 1024;
const TERMINAL_TEXT_CHARS: i32 = 64 * 1024;
const TERMINAL_FRAMES: usize = 16;
const TERMINAL_NODES: usize = 4000;
const TERMINAL_DEPTH: usize = 40;
const TERMINAL_TIME: Duration = Duration::from_millis(500);
/// Each call of that look: an app that takes longer isn't waited for.
const TERMINAL_CALL: Duration = Duration::from_millis(150);
/// Terminals looked at in one window (GNOME Terminal keeps every tab's).
const TERMINALS_SEEN: usize = 32;
/// Between looks for the focused terminal of a window just brought forward.
const TERMINAL_RETRY: Duration = Duration::from_millis(100);
/// Finding or switching a terminal's tab: how long one of those may take,
/// the most tabs a window may have, how long one step has to show, and the
/// most actions a window may list.
const TAB_TIME: Duration = Duration::from_millis(1500);
pub const MAX_TABS: usize = 16;
const TAB_STEP: Duration = Duration::from_millis(150);
/// How long a window just brought forward has to say it's the active one.
const TAB_ACTIVE: Duration = Duration::from_millis(500);
const TAB_POLL: Duration = Duration::from_millis(25);
const MAX_ACTIONS: i32 = 64;

/// The object that has the focus.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Object {
    sender: String,
    path: String,
}

/// What phones are told about the focus.
struct Focus {
    /// Changes on every focus move.
    id: u32,
    object: Option<Object>,
    kind: TextFieldKind,
    multiline: bool,
    app: Option<String>,
    label: Option<String>,
    window: bool,
    field: u64,
    /// When the accessibility bus last told of a focus.
    told_at: Option<Instant>,
    /// A window that got the focus and whose app hasn't told of a field yet.
    window_pending: Option<(Instant, Window)>,
    /// Activity at the PC from this time on doubts the field's draft.
    watched_since: Option<Instant>,
    /// The objects a rich field's text was read from, whose text events count.
    pieces: Vec<Object>,
    /// The window's own focus id, when this field was found in a window that
    /// was the field until then, and since when.
    alias: Option<(u32, Instant)>,
}

static FOCUS: Mutex<Focus> = Mutex::new(Focus {
    id: 0,
    object: None,
    kind: TextFieldKind::None,
    multiline: false,
    app: None,
    label: None,
    window: false,
    field: 0,
    told_at: None,
    window_pending: None,
    watched_since: None,
    pieces: Vec::new(),
    alias: None,
});
/// The window that has the focus, from the Shell extension.
static FOCUSED_WINDOW: Mutex<Option<Window>> = Mutex::new(None);
static FIELDS: Mutex<Option<FieldTable>> = Mutex::new(None);
static IN_TERMINAL: AtomicBool = AtomicBool::new(false);
static WATCHING: AtomicBool = AtomicBool::new(false);
/// The next state goes out even if it's the one sent last: a phone just
/// started watching.
static RESEND: AtomicBool = AtomicBool::new(false);
/// Wakes the state thread: something changed, or the watch did.
static DIRTY: Mutex<bool> = Mutex::new(false);
static WAKE: Condvar = Condvar::new();
/// The phone's input being typed, and the last burst of it.
static BURSTS: Mutex<Bursts> = Mutex::new(Bursts { active: 0, started: None, ended: None });
/// The connection used to read and place text, apart from the listener's,
/// and whether it asked apps for text events.
static READER: Mutex<Option<(zbus::blocking::Connection, bool)>> = Mutex::new(None);
/// Whether Chromium and Electron apps are asked to publish their fields.
static WAKE_CHROMIUM: AtomicBool = AtomicBool::new(true);
/// Apps (accessibility bus names) already asked.
static WOKEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
/// Failed reads of the focused field: its focus id, how many in a row, and
/// until when it's not read.
static READ_TROUBLE: Mutex<(u32, u32, Option<Instant>)> = Mutex::new((0, 0, None));
/// Whether the focus takes text, as last told, and who to tell.
static EDITABLE: AtomicBool = AtomicBool::new(false);
static EDITABLE_CHANGED: OnceLock<fn(bool)> = OnceLock::new();
static SEND_STATE: OnceLock<fn(RemoteTextState)> = OnceLock::new();
/// Hands phones' app icons to the core to serve, and the last one handed.
static KEEP_ICON: OnceLock<fn(String, Vec<u8>)> = OnceLock::new();
static KEPT_ICON: Mutex<Option<String>> = Mutex::new(None);

fn focus() -> std::sync::MutexGuard<'static, Focus> {
    FOCUS.lock().unwrap_or_else(|e| e.into_inner())
}

fn with_fields<T>(f: impl FnOnce(&mut FieldTable) -> T) -> T {
    let mut fields = FIELDS.lock().unwrap_or_else(|e| e.into_inner());
    f(fields.get_or_insert_with(FieldTable::default))
}

/// Whether a terminal has the keyboard focus, as far as the accessibility bus
/// tells.
pub fn in_terminal() -> bool {
    IN_TERMINAL.load(Ordering::Relaxed)
}

/// Whether a new line typed from the phone is a plain Enter: in a terminal,
/// a password field or a one-line field. Elsewhere (a chat box, a field the
/// PC can't see into) it's Shift+Enter, a line break where Enter would send.
pub fn plain_enter() -> bool {
    let focus = focus();
    in_terminal()
        || focus.kind == TextFieldKind::Secret
        || (focus.object.is_some() && focus.kind != TextFieldKind::None && !focus.multiline)
}

/// Whether a phone's edit or caret move for focus `id` is for what has the
/// focus now: the current focus, or the window it was found in just before.
pub fn accepts_focus(id: u32) -> bool {
    let focus = focus();
    id == focus.id || focus.alias.is_some_and(|(alias, since)| alias == id && since.elapsed() < ALIAS_FOR)
}

/// The phone's input being typed and when, so activity at the PC then isn't
/// taken as someone at the PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Bursts {
    active: usize,
    started: Option<Instant>,
    ended: Option<Instant>,
}

impl Bursts {
    /// Whether input at `activity` was the phone's: while it types, or in
    /// the time around its last burst.
    fn holds(&self, activity: Instant) -> bool {
        if self.active > 0 {
            return true;
        }
        match (self.started, self.ended) {
            (Some(started), Some(ended)) => activity + OURS_BEFORE >= started && activity <= ended + OURS_FOR,
            _ => false,
        }
    }

    /// Whether the PC's last input, at `activity`, was someone at the PC
    /// rather than the phone, when the phone has watched since `since`.
    fn someone_else(&self, activity: Instant, since: Option<Instant>) -> bool {
        if self.active > 0 || since.is_none_or(|since| activity <= since) {
            return false;
        }
        let ours = match (self.started, self.ended) {
            (Some(started), Some(ended)) => activity + OURS_BEFORE >= started && activity <= ended + OURS_FOR,
            _ => false,
        };
        !ours
    }
}

/// How long until nobody will have used this PC's own keyboard or mouse
/// for `within`, if nobody touches them meanwhile: zero when nobody has (a
/// phone's input doesn't count); `None` when the desktop doesn't say how
/// long it has been idle.
pub fn free_in(within: Duration) -> Option<Duration> {
    let idle = Duration::from_millis(super::input::idle_ms()?);
    let ours = idle < within
        && Instant::now()
            .checked_sub(idle)
            .is_some_and(|activity| BURSTS.lock().unwrap_or_else(|e| e.into_inner()).holds(activity));
    Some(free_after(idle, within, ours))
}

/// [`free_in`] for input `idle` ago, which was the phone's own when `ours`.
fn free_after(idle: Duration, within: Duration, ours: bool) -> Duration {
    if ours { Duration::ZERO } else { within.saturating_sub(idle) }
}

/// Whether a phone's input is being typed now.
pub fn phone_typing() -> bool {
    BURSTS.lock().unwrap_or_else(|e| e.into_inner()).active > 0
}

/// While this lives, the phone's input is being typed.
pub struct Injecting(());

/// The phone's input starts being typed: until the returned guard goes,
/// and a little after, activity at the PC is the phone's.
pub fn injecting() -> Injecting {
    let mut bursts = BURSTS.lock().unwrap_or_else(|e| e.into_inner());
    if bursts.active == 0 {
        bursts.started = Some(Instant::now());
    }
    bursts.active += 1;
    Injecting(())
}

impl Drop for Injecting {
    fn drop(&mut self) {
        let mut bursts = BURSTS.lock().unwrap_or_else(|e| e.into_inner());
        bursts.active = bursts.active.saturating_sub(1);
        bursts.ended = Some(Instant::now());
    }
}

/// Whether Chromium and Electron apps are asked to publish their fields
/// while a phone watches (Settings).
pub fn set_wake_chromium(on: bool) {
    WAKE_CHROMIUM.store(on, Ordering::SeqCst);
}

/// A phone started (`true`) or stopped (`false`, with no phone left)
/// watching: states go out only while one does, and one goes out at once.
pub fn set_watching(on: bool) {
    let was = WATCHING.swap(on, Ordering::SeqCst);
    if on {
        focus().watched_since = Some(Instant::now());
        RESEND.store(true, Ordering::SeqCst);
    }
    if on != was {
        let window = on.then(|| focus().window.then(focused_window).flatten()).flatten();
        let _ = std::thread::Builder::new().name("text-events".into()).spawn(move || {
            sync_text_events();
            if let Some(window) = window {
                wake(&window);
            }
        });
    }
    mark_dirty();
}

/// Something about the focused field may have changed: a state follows.
pub fn mark_dirty() {
    *DIRTY.lock().unwrap_or_else(|e| e.into_inner()) = true;
    WAKE.notify_one();
}

/// Calls `editable` with whether the focus takes text each time that
/// changes, `state` with the focused field while a phone watches, and
/// `keep_icon` with the icon of the app a state names, to serve phones.
/// Threads for the app's lifetime.
pub fn watch(editable: fn(bool), state: fn(RemoteTextState), keep_icon: fn(String, Vec<u8>)) {
    let _ = SEND_STATE.set(state);
    let _ = KEEP_ICON.set(keep_icon);
    let _ = EDITABLE_CHANGED.set(editable);
    let _ = std::thread::Builder::new().name("text-focus".into()).spawn(move || {
        loop {
            follow();
            std::thread::sleep(RETRY);
        }
    });
    let _ = std::thread::Builder::new().name("text-state".into()).spawn(send_states);
    shell_windows::follow(window_focused);
}

/// The accessibility bus, which is separate from the session bus.
fn a11y_bus() -> Option<zbus::blocking::Connection> {
    a11y_bus_waiting(CALL_TIMEOUT)
}

/// [`a11y_bus`], where an app has `timeout` to answer each call.
fn a11y_bus_waiting(timeout: Duration) -> Option<zbus::blocking::Connection> {
    let session = zbus::blocking::Connection::session().ok()?;
    let reply = session
        .call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some("org.a11y.Bus"), "GetAddress", &())
        .ok()?;
    let address: String = reply.body().deserialize().ok()?;
    zbus::blocking::connection::Builder::address(address.as_str()).ok()?.method_timeout(timeout).build().ok()
}

/// The reading connection, made again after it failed. It's also the one
/// that asks apps for text events while a phone watches.
fn reader() -> Option<zbus::blocking::Connection> {
    let made = {
        let mut reader = READER.lock().unwrap_or_else(|e| e.into_inner());
        if reader.is_none() {
            *reader = a11y_bus().map(|conn| (conn, false));
        }
        reader.as_ref().map(|(conn, registered)| (conn.clone(), *registered))
    };
    let (conn, registered) = made?;
    if !registered && WATCHING.load(Ordering::SeqCst) {
        sync_text_events();
    }
    Some(conn)
}

fn drop_reader() {
    *READER.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Asks apps for text events while a phone watches and stops asking when
/// none does, once per reading connection.
fn sync_text_events() {
    let mut reader = READER.lock().unwrap_or_else(|e| e.into_inner());
    if reader.is_none() {
        *reader = a11y_bus().map(|conn| (conn, false));
    }
    let Some((conn, registered)) = reader.as_mut() else { return };
    let wanted = WATCHING.load(Ordering::SeqCst);
    if *registered != wanted {
        register_text_events(conn, wanted);
        *registered = wanted;
    }
}

fn register_text_events(conn: &zbus::blocking::Connection, on: bool) {
    let method = if on { "RegisterEvent" } else { "DeregisterEvent" };
    for event in TEXT_EVENTS {
        let with_filters = (event, Vec::<String>::new(), "");
        let done = conn
            .call_method(Some(REGISTRY), REGISTRY_PATH, Some(REGISTRY), method, &with_filters)
            .or_else(|_| conn.call_method(Some(REGISTRY), REGISTRY_PATH, Some(REGISTRY), method, &(event,)));
        if let Err(error) = done {
            tracing::debug!(%error, event, "text focus: the accessibility registry refused");
        }
    }
}

fn event_rule(member: &str, focused_only: bool) -> Option<zbus::MatchRule<'static>> {
    let builder = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(OBJECT_EVENTS)
        .ok()?
        .member(member.to_owned())
        .ok()?;
    let builder = if focused_only { builder.arg(0, "focused").ok()? } else { builder };
    Some(builder.build())
}

/// Tells whether the focus takes text, when that changed.
fn report_editable(now: bool) {
    if EDITABLE.swap(now, Ordering::SeqCst) != now
        && let Some(changed) = EDITABLE_CHANGED.get()
    {
        changed(now);
    }
}

/// Follows focus changes until the accessibility bus connection ends.
fn follow() {
    let Some(conn) = a11y_bus() else { return };
    let messages = zbus::blocking::MessageIterator::from(&conn);
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else { return };
    let rules = std::iter::once(event_rule("StateChanged", true))
        .chain(TEXT_MEMBERS.iter().map(|member| event_rule(member, false)));
    for rule in rules {
        let Some(rule) = rule else { return };
        if dbus.add_match_rule(rule).is_err() {
            return;
        }
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
    // A new bus: the reading connection is made again too.
    drop_reader();

    let mut pids: HashMap<String, u32> = HashMap::new();
    for message in messages {
        let Ok(message) = message else { break };
        let header = message.header();
        let (Some(sender), Some(path), Some(member)) = (header.sender(), header.path(), header.member())
        else {
            continue;
        };
        let object = Object { sender: sender.to_string(), path: path.to_string() };
        if member.as_str() != "StateChanged" {
            // Text, caret or selection of the focused field, or of a part of it.
            let focus = focus();
            if focus.object.as_ref() == Some(&object) || focus.pieces.contains(&object) {
                drop(focus);
                mark_dirty();
            }
            continue;
        }
        let Some(gained) = focus_gained(&message) else { continue };
        if gained && !in_focused_window(&conn, &dbus, &object, &mut pids) {
            // Another app's focus (a background window, the Shell's own) isn't
            // where the keys go.
            continue;
        }
        let now = if gained {
            focus_moved(&conn, object)
        } else {
            focus_left(&object);
            false
        };
        report_editable(now);
    }
    IN_TERMINAL.store(false, Ordering::Relaxed);
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

/// The window that has the focus, when the Shell extension tells.
fn focused_window() -> Option<Window> {
    FOCUSED_WINDOW.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Whether `object` is in the window that has the focus: its app's process
/// has it, or, for an app reached through a proxy (Flatpak), its own window
/// is the active one. Without the Shell extension, every object is.
fn in_focused_window(
    conn: &zbus::blocking::Connection,
    dbus: &zbus::blocking::fdo::DBusProxy<'_>,
    object: &Object,
    pids: &mut HashMap<String, u32>,
) -> bool {
    let Some(window) = focused_window().filter(|window| window.pid != 0) else { return true };
    let pid = *pids.entry(object.sender.clone()).or_insert_with(|| {
        zbus::names::BusName::try_from(object.sender.as_str())
            .ok()
            .and_then(|name| dbus.get_connection_unix_process_id(name).ok())
            .unwrap_or(0)
    });
    pid == window.pid || top_level_active(conn, object).unwrap_or(true)
}

/// Whether the top-level window `object` is in is its app's active one.
fn top_level_active(conn: &zbus::blocking::Connection, object: &Object) -> Option<bool> {
    let mut at = object.clone();
    for _ in 0..MAX_ANCESTORS {
        let up = parent(conn, &at)?;
        if role(conn, &up) == Some(ROLE_APPLICATION) {
            let states = call::<Vec<u32>>(conn, &at, ACCESSIBLE, "GetState", &())?;
            return Some(has_state(&states, STATE_ACTIVE));
        }
        at = up;
    }
    None
}

/// The focus arrived at `object`: what it is, and which field. Returns
/// whether it takes text.
fn focus_moved(conn: &zbus::blocking::Connection, object: Object) -> bool {
    let role = role(conn, &object).unwrap_or(0);
    let states = call::<Vec<u32>>(conn, &object, ACCESSIBLE, "GetState", &()).unwrap_or_default();
    let editable = has_state(&states, STATE_EDITABLE);
    let kind = if role == ROLE_PASSWORD_TEXT {
        TextFieldKind::Secret
    } else if role == ROLE_TERMINAL {
        TextFieldKind::Opaque
    } else if editable && has_text(conn, &object) {
        TextFieldKind::Mirror
    } else if editable {
        TextFieldKind::Opaque
    } else {
        TextFieldKind::None
    };
    IN_TERMINAL.store(role == ROLE_TERMINAL, Ordering::Relaxed);
    let takes_text = editable || matches!(kind, TextFieldKind::Secret | TextFieldKind::Opaque);

    let (app, label, field) = if kind == TextFieldKind::None {
        (None, None, 0)
    } else {
        let key = stable_key(conn, &object, role);
        let label = (!key.name.is_empty()).then(|| cut(&key.name, remote_limits::MAX_STATE_LABEL));
        let app = (!key.app.is_empty()).then(|| cut(&key.app, remote_limits::MAX_STATE_APP));
        let field = with_fields(|fields| fields.identify(&object.sender, &object.path, &key));
        (app, label, field)
    };

    let mut focus = focus();
    // Found in the window that stood for it: what the phone typed for the
    // window still lands here.
    focus.alias = focus.window.then(|| (focus.id, Instant::now()));
    focus.id = focus.id.wrapping_add(1);
    focus.object = Some(object);
    focus.kind = kind;
    focus.multiline = has_state(&states, STATE_MULTI_LINE);
    focus.app = app;
    focus.label = if kind == TextFieldKind::Secret { None } else { label };
    focus.window = false;
    focus.field = field;
    focus.told_at = Some(Instant::now());
    focus.window_pending = None;
    focus.watched_since = Some(Instant::now());
    focus.pieces.clear();
    drop(focus);
    mark_dirty();
    takes_text
}

/// The focus left `object`: until another one tells, nothing has it.
fn focus_left(object: &Object) {
    let mut focus = focus();
    if focus.object.as_ref() != Some(object) {
        return;
    }
    focus.id = focus.id.wrapping_add(1);
    focus.object = None;
    focus.kind = TextFieldKind::None;
    focus.app = None;
    focus.label = None;
    focus.window = false;
    focus.field = 0;
    focus.pieces.clear();
    focus.alias = None;
    drop(focus);
    IN_TERMINAL.store(false, Ordering::Relaxed);
    mark_dirty();
}

/// A window got the focus: unless its app tells of a field soon, the
/// window is the field.
fn window_focused(window: Window) {
    *FOCUSED_WINDOW.lock().unwrap_or_else(|e| e.into_inner()) = Some(window.clone());
    focus().window_pending = Some((Instant::now(), window));
    mark_dirty();
}

/// Takes a window whose app told of no field in time as the field.
fn settle_window(now: Instant) -> Option<Duration> {
    let mut focus = focus();
    let (since, _) = focus.window_pending.as_ref()?;
    let due = *since + WINDOW_GRACE;
    if now < due {
        return Some(due - now);
    }
    let (since, window) = focus.window_pending.take()?;
    if focus.told_at.is_some_and(|told| told >= since) {
        return None;
    }
    let field = with_fields(|fields| fields.window_field(&window.class, window.id));
    focus.id = focus.id.wrapping_add(1);
    focus.object = None;
    focus.kind = TextFieldKind::Opaque;
    focus.multiline = false;
    focus.app = (!window.class.is_empty()).then(|| cut(&window.class, remote_limits::MAX_STATE_APP));
    focus.label = (!window.title.is_empty()).then(|| cut(&window.title, remote_limits::MAX_STATE_LABEL));
    focus.window = true;
    focus.field = field;
    focus.watched_since = Some(now);
    focus.pieces.clear();
    focus.alias = None;
    drop(focus);
    if WATCHING.load(Ordering::SeqCst) {
        let _ = std::thread::Builder::new().name("text-wake".into()).spawn(move || wake(&window));
    }
    None
}

/// The state thread: sends the focused field to watching phones when it
/// changes, at most every [`MIN_GAP`].
fn send_states() {
    let mut sent: Option<RemoteTextState> = None;
    let mut rev: u32 = 0;
    loop {
        let wait = settle_window(Instant::now()).unwrap_or(if WATCHING.load(Ordering::SeqCst) {
            ACTIVITY_POLL
        } else {
            Duration::from_secs(5)
        });
        {
            let dirty = DIRTY.lock().unwrap_or_else(|e| e.into_inner());
            let (mut dirty, _) =
                WAKE.wait_timeout_while(dirty, wait, |dirty| !*dirty).unwrap_or_else(|e| e.into_inner());
            *dirty = false;
        }
        if !WATCHING.load(Ordering::SeqCst) {
            sent = None;
            continue;
        }
        if RESEND.swap(false, Ordering::SeqCst) {
            sent = None;
        }
        std::thread::sleep(DEBOUNCE);
        *DIRTY.lock().unwrap_or_else(|e| e.into_inner()) = false;
        let _ = settle_window(Instant::now());
        doubt_after_activity();
        let Some(mut state) = read_state() else { continue };
        if let Some(last) = &sent {
            state.rev = last.rev;
            if *last == state {
                continue;
            }
        }
        rev = rev.wrapping_add(1);
        state.rev = rev;
        if let Some(send) = SEND_STATE.get() {
            send(state.clone());
        }
        sent = Some(state);
        std::thread::sleep(MIN_GAP.saturating_sub(DEBOUNCE));
    }
}

/// For a field whose text isn't shared: whether someone used the PC's own
/// keyboard or mouse since the phone last typed, which makes the phone's
/// record of what it typed there doubtful.
fn doubt_after_activity() {
    let (field, since) = {
        let focus = focus();
        if focus.kind != TextFieldKind::Opaque || focus.field == 0 {
            return;
        }
        (focus.field, focus.watched_since)
    };
    let Some(idle) = super::input::idle_ms() else { return };
    let now = Instant::now();
    let Some(activity) = now.checked_sub(Duration::from_millis(idle)) else { return };
    let bursts = *BURSTS.lock().unwrap_or_else(|e| e.into_inner());
    if bursts.someone_else(activity, since) {
        with_fields(|fields| fields.doubt(field));
        focus().watched_since = Some(now);
    }
}

/// The focused field as phones are told, with its text when it's shared.
fn read_state() -> Option<RemoteTextState> {
    let (object, mut state) = {
        let focus = focus();
        let state = RemoteTextState {
            focus: focus.id,
            field: focus.field,
            epoch: if focus.field == 0 { 0 } else { with_fields(|fields| fields.epoch(focus.field)) },
            rev: 0,
            applied: 0,
            kind: focus.kind,
            app: focus.app.clone(),
            label: focus.label.clone(),
            window: focus.window,
            multiline: focus.multiline,
            start: 0,
            before: String::new(),
            after: String::new(),
            anchor: None,
            more_before: false,
            more_after: false,
            icon: None,
        };
        (focus.object.clone(), state)
    };
    if state.kind != TextFieldKind::None {
        name_app(&mut state);
    }
    if state.kind == TextFieldKind::Mirror
        && let Some(object) = object
    {
        let now = Instant::now();
        let paused = {
            let mut trouble = READ_TROUBLE.lock().unwrap_or_else(|e| e.into_inner());
            if trouble.0 != state.focus {
                *trouble = (state.focus, 0, None);
            }
            trouble.2.is_some_and(|until| now < until)
        };
        let text = if paused { None } else { reader().and_then(|conn| read_text(&conn, &object)) };
        match text {
            Some(text) => {
                state.start = text.start;
                state.before = text.before;
                state.after = text.after;
                state.anchor = text.anchor;
                state.more_before = text.more_before;
                state.more_after = text.more_after;
                *READ_TROUBLE.lock().unwrap_or_else(|e| e.into_inner()) = (state.focus, 0, None);
                let mut focus = focus();
                if focus.id == state.focus {
                    focus.pieces = text.pieces;
                }
            }
            None => {
                // A read fails now and then while the app changes the field
                // under it: nothing is said until a few fail in a row. Then
                // it's keys only for a while (the app may have stopped
                // answering), and it's read again.
                if !paused {
                    let mut trouble = READ_TROUBLE.lock().unwrap_or_else(|e| e.into_inner());
                    trouble.1 += 1;
                    if trouble.1 < MAX_READ_FAILURES {
                        return None;
                    }
                    *trouble = (state.focus, 0, Some(now + READ_PAUSE));
                    drop(trouble);
                    drop_reader();
                }
                state.kind = TextFieldKind::Opaque;
            }
        }
    }
    state.is_valid().then_some(state)
}

/// Names the app a state is about as its desktop entry does, with its icon,
/// when the Shell extension tells which window has the focus (a field the PC
/// heard of is in it: others are dropped).
fn name_app(state: &mut RemoteTextState) {
    let Some(window) = focused_window() else { return };
    let Some(app) = super::app_entry::identify(&window.class, window.pid) else { return };
    state.app = Some(cut(&app.name, remote_limits::MAX_STATE_APP));
    let Some(icon) = &app.icon else { return };
    state.icon = Some(icon.key.clone());
    let mut kept = KEPT_ICON.lock().unwrap_or_else(|e| e.into_inner());
    if kept.as_deref() != Some(icon.key.as_str())
        && let Some(keep) = KEEP_ICON.get()
    {
        keep(icon.key.clone(), icon.png.to_vec());
        *kept = Some(icon.key.clone());
    }
}

struct FieldText {
    start: u32,
    before: String,
    after: String,
    anchor: Option<u32>,
    more_before: bool,
    more_after: bool,
    /// For a rich field, the objects its text came from.
    pieces: Vec<Object>,
}

fn read_text(conn: &zbus::blocking::Connection, object: &Object) -> Option<FieldText> {
    let caret = property::<i32>(conn, object, TEXT, "CaretOffset")?.max(0);
    let count = property::<i32>(conn, object, TEXT, "CharacterCount")?.max(caret);
    let from = caret.saturating_sub(READ_BEFORE).max(0);
    let to = caret.saturating_add(READ_AFTER).min(count);
    let before: String = call(conn, object, TEXT, "GetText", &(from, caret))?;
    let after: String = call(conn, object, TEXT, "GetText", &(caret, to))?;
    if before.contains(rich_text::EMBEDDED) || after.contains(rich_text::EMBEDDED) {
        return read_rich_text(conn, object);
    }
    let selections = call::<i32>(conn, object, TEXT, "GetNSelections", &()).unwrap_or(0);
    let anchor = if selections > 0 {
        call::<(i32, i32)>(conn, object, TEXT, "GetSelection", &(0i32,))
            .filter(|(s, e)| s != e)
            .map(|(s, e)| if e == caret { s } else { e })
            .and_then(|anchor| u32::try_from(anchor).ok())
    } else {
        None
    };
    let (Ok(from), Ok(to), Ok(count)) = (u32::try_from(from), u32::try_from(to), u32::try_from(count)) else {
        return None;
    };
    Some(window_around(before, after, from, to, count, anchor, Vec::new()))
}

/// The text read around the caret, `from..to` of `count`, cut to what a
/// state carries.
fn window_around(
    before: String,
    after: String,
    from: u32,
    to: u32,
    count: u32,
    anchor: Option<u32>,
    pieces: Vec<Object>,
) -> FieldText {
    let (before, dropped) = cut_front(&before, remote_limits::MAX_STATE_BEFORE);
    let after_len = after.chars().count();
    let after = cut(&after, remote_limits::MAX_STATE_AFTER);
    let start = from.saturating_add(dropped);
    FieldText {
        start,
        more_before: start > 0,
        more_after: to < count || after.chars().count() < after_len,
        before,
        after,
        anchor,
        pieces,
    }
}

/// A rich field (a browser's contenteditable): its paragraphs read as one
/// text, the caret found in the paragraph that has it. `None` makes it a field
/// whose text isn't shared.
fn read_rich_text(conn: &zbus::blocking::Connection, object: &Object) -> Option<FieldText> {
    let (flat, objects) = flatten(conn, object)?;
    let (owner, local) = caret_in(conn, object, &objects, 0)?;
    let caret = flat.to_flat(owner, local)?;
    let chars: Vec<char> = flat.text.chars().collect();
    let count = u32::try_from(chars.len()).ok()?;
    let caret = caret.min(count);
    let from = caret.saturating_sub(READ_BEFORE as u32);
    let to = caret.saturating_add(READ_AFTER as u32).min(count);
    let before: String = chars[from as usize..caret as usize].iter().collect();
    let after: String = chars[caret as usize..to as usize].iter().collect();
    Some(window_around(before, after, from, to, count, None, objects))
}

/// Reads `field` and the objects its U+FFFCs stand for, as one text, with the
/// objects in the order [`rich_text::flatten`] numbers them.
fn flatten(conn: &zbus::blocking::Connection, field: &Object) -> Option<(Flat, Vec<Object>)> {
    let mut objects = Vec::new();
    let root = read_node(conn, field, 0, &mut objects)?;
    let flat = rich_text::flatten(&root)?;
    (flat.objects() == objects.len()).then_some((flat, objects))
}

fn read_node(
    conn: &zbus::blocking::Connection,
    object: &Object,
    depth: usize,
    objects: &mut Vec<Object>,
) -> Option<Node> {
    if depth > rich_text::MAX_DEPTH || objects.len() >= rich_text::MAX_OBJECTS {
        return None;
    }
    objects.push(object.clone());
    let Some(count) = property::<i32>(conn, object, TEXT, "CharacterCount") else {
        // An embedded object with no text (a picture): nothing on the line.
        return (depth > 0).then(|| Node { text: String::new(), links: Vec::new() });
    };
    if usize::try_from(count).ok()? > rich_text::MAX_CHARS {
        return None;
    }
    let text: String = call(conn, object, TEXT, "GetText", &(0i32, count))?;
    let mut children = Vec::new();
    if text.contains(rich_text::EMBEDDED) {
        let n = call::<i32>(conn, object, HYPERTEXT, "GetNLinks", &())?;
        if usize::try_from(n).ok()? > rich_text::MAX_OBJECTS {
            return None;
        }
        for i in 0..n {
            let link = object_at(call(conn, object, HYPERTEXT, "GetLink", &(i,))?);
            let at = u32::try_from(property::<i32>(conn, &link, HYPERLINK, "StartIndex")?).ok()?;
            let child = object_at(call(conn, &link, HYPERLINK, "GetObject", &(0i32,))?);
            let block = role(conn, &child).is_some_and(|role| BLOCK_ROLES.contains(&role));
            children.push((at, block, child));
        }
        // In the text's order, as `rich_text::flatten` numbers them.
        children.sort_by_key(|(at, ..)| *at);
    }
    let mut links = Vec::with_capacity(children.len());
    for (at, block, child) in children {
        let node = read_node(conn, &child, depth + 1, objects)?;
        links.push(Link { at, block, node });
    }
    Some(Node { text, links })
}

fn object_at((sender, path): (String, OwnedObjectPath)) -> Object {
    Object { sender, path: path.to_string() }
}

/// Which of `objects` has the caret, and where in it: from the field down
/// through the object whose U+FFFC the caret is at (or just after).
fn caret_in(
    conn: &zbus::blocking::Connection,
    object: &Object,
    objects: &[Object],
    depth: usize,
) -> Option<(usize, u32)> {
    let owner = objects.iter().position(|o| o == object)?;
    // An object that doesn't have the caret says -1.
    let caret = u32::try_from(property::<i32>(conn, object, TEXT, "CaretOffset")?).ok()?;
    if depth >= rich_text::MAX_DEPTH {
        return Some((owner, caret));
    }
    let embedded_at = |offset: u32| -> Option<Object> {
        let offset = i32::try_from(offset).ok()?;
        let text: String = call(conn, object, TEXT, "GetText", &(offset, offset + 1))?;
        if !text.starts_with(rich_text::EMBEDDED) {
            return None;
        }
        let index = call::<i32>(conn, object, HYPERTEXT, "GetLinkIndex", &(offset,)).filter(|&i| i >= 0)?;
        let link = object_at(call(conn, object, HYPERTEXT, "GetLink", &(index,))?);
        Some(object_at(call(conn, &link, HYPERLINK, "GetObject", &(0i32,))?))
    };
    // The caret is in the object at it, or the one just before it; when
    // neither says, the start of the first or the end of the second.
    let here = embedded_at(caret);
    let before = caret.checked_sub(1).and_then(embedded_at);
    for child in here.iter().chain(&before) {
        if let Some(found) = caret_in(conn, child, objects, depth + 1) {
            return Some(found);
        }
    }
    if let Some(child) = here.and_then(|child| objects.iter().position(|o| *o == child)) {
        return Some((child, 0));
    }
    if let Some(child) = before {
        let end = u32::try_from(property::<i32>(conn, &child, TEXT, "CharacterCount")?).ok()?;
        return Some((objects.iter().position(|o| *o == child)?, end));
    }
    Some((owner, caret))
}

/// Selects `start..end` of the focused field (or puts the caret at `start`
/// when they're equal), when `focus` still has it and its text is shared. In
/// a rich field, a range across paragraphs isn't selected: the caller moves
/// there with the keys instead.
pub fn select(focus_id: u32, start: u32, end: u32) -> bool {
    let Some((field, rich)) = mirrored(focus_id) else { return false };
    let Some(conn) = reader() else { return false };
    let flat = if rich {
        match flatten(&conn, &field) {
            Some(flat) => Some(flat),
            None => return false,
        }
    } else {
        None
    };
    let locate = |at: u32| -> Option<(Object, i32)> {
        match &flat {
            Some((flat, objects)) => {
                let (owner, local) = flat.to_local(at)?;
                Some((objects.get(owner)?.clone(), i32::try_from(local).ok()?))
            }
            None => Some((field.clone(), i32::try_from(at).ok()?)),
        }
    };
    let caret_only = start == end;
    let Some((object, start)) = locate(start) else { return false };
    if caret_only {
        return call::<bool>(&conn, &object, TEXT, "SetCaretOffset", &(start,)).unwrap_or(false);
    }
    let Some((end_object, end)) = locate(end) else { return false };
    if end_object != object {
        return false;
    }
    call::<bool>(&conn, &object, TEXT, "SetSelection", &(0i32, start, end)).unwrap_or(false)
        || call::<bool>(&conn, &object, TEXT, "AddSelection", &(start, end)).unwrap_or(false)
}

/// Places the caret of the focused field at `caret`, selecting from
/// `anchor`, when `focus` still has it and its text is shared.
pub fn set_caret(focus_id: u32, caret: u32, anchor: u32) -> bool {
    if caret == anchor {
        return select(focus_id, caret, caret);
    }
    select(focus_id, caret.min(anchor), caret.max(anchor))
}

/// Where the caret of the focused field is, when `focus` still has it and
/// its text is shared.
pub fn caret_offset(focus_id: u32) -> Option<u32> {
    let (field, rich) = mirrored(focus_id)?;
    let conn = reader()?;
    if rich {
        let (flat, objects) = flatten(&conn, &field)?;
        let (owner, local) = caret_in(&conn, &field, &objects, 0)?;
        return flat.to_flat(owner, local);
    }
    property::<i32>(&conn, &field, TEXT, "CaretOffset").and_then(|caret| u32::try_from(caret).ok())
}

/// The focused field, when `focus` still has it and its text is shared, and
/// whether it's a rich one.
fn mirrored(focus_id: u32) -> Option<(Object, bool)> {
    let focus = focus();
    (focus.id == focus_id && focus.kind == TextFieldKind::Mirror)
        .then(|| focus.object.clone().map(|object| (object, !focus.pieces.is_empty())))
        .flatten()
}

/// What describes the field at `object`, for finding it again. A paragraph
/// of a document is the document.
fn stable_key(conn: &zbus::blocking::Connection, object: &Object, role: u32) -> StableKey {
    let mut target = object.clone();
    let mut role = role;
    for _ in 0..4 {
        if role != ROLE_PARAGRAPH {
            break;
        }
        let Some(parent) = parent(conn, &target) else { break };
        role = self::role(conn, &parent).unwrap_or(0);
        target = parent;
    }
    let name = property::<String>(conn, &target, ACCESSIBLE, "Name").unwrap_or_default();
    let description = property::<String>(conn, &target, ACCESSIBLE, "Description").unwrap_or_default();
    let id = call::<HashMap<String, String>>(conn, &target, ACCESSIBLE, "GetAttributes", &())
        .and_then(|mut attributes| attributes.remove("id"))
        .unwrap_or_default();
    let app = call::<(String, OwnedObjectPath)>(conn, &target, ACCESSIBLE, "GetApplication", &())
        .and_then(|(sender, path)| {
            property::<String>(conn, &Object { sender, path: path.to_string() }, ACCESSIBLE, "Name")
        })
        .unwrap_or_default();
    let window = focused_window().map(|window| window.id);
    let doc = document_url(conn, &target);
    let mut path = Vec::new();
    if name.is_empty() && id.is_empty() {
        let mut at = target.clone();
        for _ in 0..4 {
            let Some(index) = call::<i32>(conn, &at, ACCESSIBLE, "GetIndexInParent", &()) else { break };
            path.push(index);
            let Some(parent) = parent(conn, &at) else { break };
            at = parent;
        }
    }
    StableKey { app, window, doc, role, name, description, id, path }
}

/// The address of the web page `object` is in, so the same field on two
/// pages or tabs is two fields; empty outside a browser.
fn document_url(conn: &zbus::blocking::Connection, object: &Object) -> String {
    let mut at = object.clone();
    for _ in 0..MAX_ANCESTORS {
        if role(conn, &at) == Some(ROLE_DOCUMENT_WEB) {
            return call::<String>(conn, &at, DOCUMENT, "GetAttributeValue", &("DocURL",))
                .unwrap_or_default();
        }
        let Some(up) = parent(conn, &at) else { break };
        at = up;
    }
    String::new()
}

/// Asks the Chromium or Electron app that has `window` to publish its fields
/// (once per app: it can't be asked to stop, it stops when it quits), then
/// finds the field that has the focus in it, which the app won't tell of
/// again on its own.
fn wake(window: &Window) {
    if !WAKE_CHROMIUM.load(Ordering::SeqCst) || window.pid == 0 {
        return;
    }
    let Some(conn) = reader() else { return };
    let Some(app) = chromium_app(&conn, window.pid) else { return };
    let fresh =
        WOKEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).insert(app.clone());
    let root = Object { sender: app, path: ROOT_PATH.into() };
    if fresh {
        // Being asked for attributes is what makes Chromium build its tree.
        let _ = call::<HashMap<String, String>>(&conn, &root, ACCESSIBLE, "GetAttributes", &());
    }
    let focus_id = focus().id;
    let still_window = || {
        let focus = focus();
        focus.id == focus_id && focus.window
    };
    for _ in 0..WAKE_TRIES {
        if let Some(field) = find_focused(&conn, &root) {
            if still_window() {
                let takes_text = focus_moved(&conn, field);
                report_editable(takes_text);
            }
            return;
        }
        std::thread::sleep(WAKE_GAP);
        if !still_window() {
            return;
        }
    }
}

/// The accessibility bus name of the Chromium or Electron app with `pid`.
fn chromium_app(conn: &zbus::blocking::Connection, pid: u32) -> Option<String> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(conn).ok()?;
    dbus.list_names().ok()?.into_iter().find_map(|name| {
        if !name.starts_with(':') || dbus.get_connection_unix_process_id(name.inner().clone()).ok()? != pid {
            return None;
        }
        let root = Object { sender: name.to_string(), path: ROOT_PATH.into() };
        (property::<String>(conn, &root, APPLICATION, "ToolkitName").as_deref() == Some(CHROMIUM))
            .then(|| name.to_string())
    })
}

/// The focused editable object under `root`, looked for in its active window
/// within [`WAKE_NODES`] objects and [`WAKE_TIME`].
fn find_focused(conn: &zbus::blocking::Connection, root: &Object) -> Option<Object> {
    let deadline = Instant::now() + WAKE_TIME;
    let windows = children(conn, root)?;
    let active: Vec<Object> = windows
        .iter()
        .filter(|window| {
            call::<Vec<u32>>(conn, window, ACCESSIBLE, "GetState", &())
                .is_some_and(|states| has_state(&states, STATE_ACTIVE))
        })
        .cloned()
        .collect();
    let mut stack = if active.is_empty() { windows } else { active };
    stack.reverse();
    let mut seen = 0;
    while let Some(object) = stack.pop() {
        seen += 1;
        if seen > WAKE_NODES || Instant::now() > deadline {
            return None;
        }
        let states = call::<Vec<u32>>(conn, &object, ACCESSIBLE, "GetState", &()).unwrap_or_default();
        if has_state(&states, STATE_FOCUSED) && has_state(&states, STATE_EDITABLE) {
            return Some(object);
        }
        if !has_state(&states, STATE_SHOWING) {
            continue;
        }
        if let Some(mut kids) = children(conn, &object) {
            kids.reverse();
            stack.extend(kids);
        }
    }
    None
}

fn children(conn: &zbus::blocking::Connection, object: &Object) -> Option<Vec<Object>> {
    call::<Vec<(String, OwnedObjectPath)>>(conn, object, ACCESSIBLE, "GetChildren", &())
        .map(|children| children.into_iter().map(object_at).collect())
}

fn role(conn: &zbus::blocking::Connection, object: &Object) -> Option<u32> {
    call(conn, object, ACCESSIBLE, "GetRole", &())
}

fn has_text(conn: &zbus::blocking::Connection, object: &Object) -> bool {
    call::<Vec<String>>(conn, object, ACCESSIBLE, "GetInterfaces", &())
        .is_some_and(|interfaces| interfaces.iter().any(|i| i == TEXT))
}

fn parent(conn: &zbus::blocking::Connection, object: &Object) -> Option<Object> {
    let value: OwnedValue = call(conn, object, PROPERTIES, "Get", &(ACCESSIBLE, "Parent"))?;
    let (sender, path) = <(String, OwnedObjectPath)>::try_from(value).ok()?;
    (!sender.is_empty() && path.as_str() != "/org/a11y/atspi/null")
        .then(|| Object { sender, path: path.to_string() })
}

fn call<T>(
    conn: &zbus::blocking::Connection,
    object: &Object,
    interface: &str,
    method: &str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
) -> Option<T>
where
    T: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    conn.call_method(Some(object.sender.as_str()), object.path.as_str(), Some(interface), method, body)
        .ok()?
        .body()
        .deserialize::<T>()
        .ok()
}

fn property<T: TryFrom<OwnedValue>>(
    conn: &zbus::blocking::Connection,
    object: &Object,
    interface: &str,
    name: &str,
) -> Option<T> {
    let value: OwnedValue = call(conn, object, PROPERTIES, "Get", &(interface, name))?;
    T::try_from(value).ok()
}

/// The end of the text of the focused terminal in the active window of the
/// app with `pid`, at most 64 KiB, within 500 ms. `None` when no window of
/// it is active, the active one doesn't show exactly one terminal, the
/// keyboard isn't in it, or it can't be read.
pub fn focused_terminal_text(pid: u32) -> Option<String> {
    let deadline = Instant::now() + TERMINAL_TIME;
    super::non_tokio(move || {
        let conn = a11y_bus_waiting(TERMINAL_CALL)?;
        let app = app_of(&conn, pid)?;
        // A window just brought forward tells its toolkit a moment later.
        loop {
            if let Some(text) = focused_terminal_in(&conn, &app, deadline) {
                return Some(text);
            }
            if Instant::now() + TERMINAL_RETRY > deadline {
                return None;
            }
            std::thread::sleep(TERMINAL_RETRY);
        }
    })
}

fn focused_terminal_in(conn: &zbus::blocking::Connection, app: &Object, deadline: Instant) -> Option<String> {
    let frame = children(conn, app)?.into_iter().take(TERMINAL_FRAMES).find(|frame| {
        call::<Vec<u32>>(conn, frame, ACCESSIBLE, "GetState", &())
            .is_some_and(|states| has_state(&states, STATE_ACTIVE))
    })?;
    let found = one_terminal(conn, &frame, deadline)?;
    has_state(&found.states, STATE_FOCUSED).then_some(())?;
    terminal_tail(conn, &found.terminal)
}

/// The accessibility bus root of the app with `pid`.
fn app_of(conn: &zbus::blocking::Connection, pid: u32) -> Option<Object> {
    if pid == 0 {
        return None;
    }
    let dbus = zbus::blocking::fdo::DBusProxy::new(conn).ok()?;
    let name = dbus.list_names().ok()?.into_iter().find(|name| {
        name.starts_with(':') && dbus.get_connection_unix_process_id(name.inner().clone()).ok() == Some(pid)
    })?;
    let root = Object { sender: name.to_string(), path: ROOT_PATH.into() };
    (role(conn, &root) == Some(ROLE_APPLICATION)).then_some(root)
}

#[derive(Debug)]
struct FoundTerminal {
    terminal: Object,
    states: Vec<u32>,
}

/// The one terminal `frame` shows, with its states: the only terminal under
/// it, or of several (GNOME Terminal's tabs), the only one showing. `None`
/// for none, two showing, or a walk that ran out of objects or time.
fn one_terminal(
    conn: &zbus::blocking::Connection,
    frame: &Object,
    deadline: Instant,
) -> Option<FoundTerminal> {
    let mut found: Vec<FoundTerminal> = Vec::new();
    let mut stack = vec![(frame.clone(), 0usize)];
    let mut seen = 0;
    while let Some((object, depth)) = stack.pop() {
        seen += 1;
        if seen > TERMINAL_NODES || Instant::now() > deadline {
            return None;
        }
        if role(conn, &object) == Some(ROLE_TERMINAL) {
            if found.len() >= TERMINALS_SEEN {
                return None;
            }
            let states = call::<Vec<u32>>(conn, &object, ACCESSIBLE, "GetState", &()).unwrap_or_default();
            found.push(FoundTerminal { terminal: object, states });
            continue;
        }
        if depth < TERMINAL_DEPTH
            && let Some(kids) = children(conn, &object)
        {
            stack.extend(kids.into_iter().rev().map(|kid| (kid, depth + 1)));
        }
    }
    shown_terminal(found)
}

fn shown_terminal(mut found: Vec<FoundTerminal>) -> Option<FoundTerminal> {
    if found.len() > 1 {
        found.retain(|t| has_state(&t.states, STATE_SHOWING));
    }
    if found.len() == 1 { found.pop() } else { None }
}

/// The last characters of a terminal's text, cut to [`TERMINAL_TEXT_BYTES`].
fn terminal_tail(conn: &zbus::blocking::Connection, terminal: &Object) -> Option<String> {
    let count = property::<i32>(conn, terminal, TEXT, "CharacterCount")?.max(0);
    let from = count.saturating_sub(TERMINAL_TEXT_CHARS).max(0);
    let text: String = call(conn, terminal, TEXT, "GetText", &(from, count))?;
    Some(cut_front(&text, TERMINAL_TEXT_BYTES).0)
}

// ---- A terminal's tabs, for a reply (`plans/reply-tabs-idle.md` §3) ----

/// How a terminal app's tabs are found and switched on the accessibility bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabRoute {
    /// Ptyxis (libadwaita): one panel per tab in the tab view, named by the
    /// tab's title, and the window's `page.next` / `page.previous` actions.
    AdwPages,
    /// GNOME Terminal (GTK 3): the notebook's page tab list and its `Selection`.
    Notebook,
}

/// Why a tab wasn't selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabError {
    /// The app's active window has no tab with the title, or several.
    NotFound,
    /// The window has more than [`MAX_TABS`] tabs.
    TooMany,
    /// The window doesn't offer the way, or didn't move as asked.
    CantSwitch,
    /// It took longer than [`TAB_TIME`].
    TimedOut,
}

/// Where a tab titled with a mark is: its window's title (compared, never
/// logged), its index, the selected tab's index, and how many tabs there are.
#[derive(Clone, PartialEq, Eq)]
pub struct TabSpot {
    pub frame_title: String,
    pub at: usize,
    pub selected: usize,
    pub tabs: usize,
}

impl std::fmt::Debug for TabSpot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabSpot")
            .field("at", &self.at)
            .field("selected", &self.selected)
            .field("tabs", &self.tabs)
            .finish_non_exhaustive()
    }
}

/// A window's tab actions: the only accessibility actions G7 ever runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Next,
    Previous,
}

impl Page {
    fn name(self) -> &'static str {
        match self {
            Page::Next => "page.next",
            Page::Previous => "page.previous",
        }
    }

    /// The step from tab `from` toward tab `to`; `None` once it's there.
    fn toward(from: usize, to: usize) -> Option<Page> {
        match to.cmp(&from) {
            std::cmp::Ordering::Greater => Some(Page::Next),
            std::cmp::Ordering::Less => Some(Page::Previous),
            std::cmp::Ordering::Equal => None,
        }
    }

    /// The tab one step from `from` this way. G7 never steps past either
    /// end, so whether a window wraps around doesn't matter.
    fn after(self, from: usize) -> Option<usize> {
        match self {
            Page::Next => from.checked_add(1),
            Page::Previous => from.checked_sub(1),
        }
    }
}

/// The index of `page`'s action among a window's action `names`, when they
/// hold exactly one `page.next` and one `page.previous`: a list with none or
/// duplicates isn't one G7 acts on.
fn pick_action(names: &[String], page: Page) -> Option<usize> {
    let count = |name: &str| names.iter().filter(|n| *n == name).count();
    if count(Page::Next.name()) != 1 || count(Page::Previous.name()) != 1 {
        return None;
    }
    names.iter().position(|n| n == page.name())
}

/// The one tab of several that is selected, by a flag each; `None` for
/// none or more than one.
fn selected_among(flags: &[bool]) -> Option<usize> {
    let mut on = flags.iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| i);
    match (on.next(), on.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

/// A tab titled with a mark in one window: the object that holds the
/// window's tabs, the tab's index among them, the selected one's, and how
/// many there are.
#[derive(Debug)]
struct TabNode {
    list: Object,
    at: usize,
    selected: usize,
    tabs: usize,
}

/// Where tabs titled with `mark` are among the windows of the app with
/// `pid`, read without changing anything. `None` when the app isn't on the
/// accessibility bus, or the look failed or ran out of time.
pub fn tab_titled(pid: u32, mark: &str, route: TabRoute) -> Option<Vec<TabSpot>> {
    if mark.is_empty() {
        return None;
    }
    let deadline = Instant::now() + TAB_TIME;
    super::non_tokio(move || {
        let conn = a11y_bus_waiting(TERMINAL_CALL)?;
        let app = app_of(&conn, pid)?;
        let mut spots = Vec::new();
        for frame in children(&conn, &app)?.into_iter().take(TERMINAL_FRAMES) {
            let nodes = tabs_titled(&conn, &frame, mark, route, deadline)?;
            if nodes.is_empty() {
                continue;
            }
            let frame_title = property::<String>(&conn, &frame, ACCESSIBLE, "Name").unwrap_or_default();
            spots.extend(nodes.into_iter().map(|node| TabSpot {
                frame_title: frame_title.clone(),
                at: node.at,
                selected: node.selected,
                tabs: node.tabs,
            }));
        }
        Some(spots)
    })
}

/// In the app's active window, selects the tab titled with `mark`. Returns
/// the index of the tab that was selected before, to go back to.
pub fn select_tab(pid: u32, mark: &str, route: TabRoute) -> Result<usize, TabError> {
    if mark.is_empty() {
        return Err(TabError::NotFound);
    }
    let deadline = Instant::now() + TAB_TIME;
    super::non_tokio(move || {
        let conn = a11y_bus_waiting(TERMINAL_CALL).ok_or(TabError::NotFound)?;
        let app = app_of(&conn, pid).ok_or(TabError::NotFound)?;
        let frame = active_frame(&conn, &app, deadline)?;
        let nodes = tabs_titled(&conn, &frame, mark, route, deadline).ok_or(TabError::TimedOut)?;
        let [node] = nodes.as_slice() else { return Err(TabError::NotFound) };
        if node.tabs > MAX_TABS {
            return Err(TabError::TooMany);
        }
        move_tab(&conn, &frame, node, node.at, route, deadline)?;
        Ok(node.selected)
    })
}

/// The app's one active window. A window just brought forward tells its
/// toolkit a moment later, so it's looked for again until [`TAB_ACTIVE`].
fn active_frame(
    conn: &zbus::blocking::Connection,
    app: &Object,
    deadline: Instant,
) -> Result<Object, TabError> {
    let until = (Instant::now() + TAB_ACTIVE).min(deadline);
    loop {
        let frames = children(conn, app).ok_or(TabError::NotFound)?;
        let mut active: Vec<Object> =
            frames.into_iter().take(TERMINAL_FRAMES).filter(|frame| is_active(conn, frame)).collect();
        match active.len() {
            1 => return Ok(active.remove(0)),
            0 if Instant::now() + TERMINAL_RETRY <= until => std::thread::sleep(TERMINAL_RETRY),
            _ => return Err(TabError::NotFound),
        }
    }
}

/// Selects tab `at` again in the one window that has a tab titled with
/// `mark`: going back when a reply wasn't pasted. Best effort.
pub fn select_tab_at(pid: u32, mark: &str, at: usize, route: TabRoute) -> bool {
    if mark.is_empty() {
        return false;
    }
    let deadline = Instant::now() + TAB_TIME;
    super::non_tokio(move || {
        let Some(conn) = a11y_bus_waiting(TERMINAL_CALL) else { return false };
        let Some(app) = app_of(&conn, pid) else { return false };
        let Some(frames) = children(&conn, &app) else { return false };
        let mut holding = Vec::new();
        for frame in frames.into_iter().take(TERMINAL_FRAMES) {
            let Some(nodes) = tabs_titled(&conn, &frame, mark, route, deadline) else { return false };
            holding.extend(nodes.into_iter().map(|node| (frame.clone(), node)));
        }
        let [(frame, node)] = holding.as_slice() else { return false };
        node.tabs <= MAX_TABS && move_tab(&conn, frame, node, at, route, deadline).is_ok()
    })
}

fn is_active(conn: &zbus::blocking::Connection, frame: &Object) -> bool {
    call::<Vec<u32>>(conn, frame, ACCESSIBLE, "GetState", &())
        .is_some_and(|states| has_state(&states, STATE_ACTIVE))
}

/// The tabs in `frame` titled with `mark`, built the way `route` says.
/// `None` when the walk failed or ran out of objects or time.
fn tabs_titled(
    conn: &zbus::blocking::Connection,
    frame: &Object,
    mark: &str,
    route: TabRoute,
    deadline: Instant,
) -> Option<Vec<TabNode>> {
    /// An object to look at, how deep it is, and its parent with the parent's role.
    struct Visit {
        object: Object,
        depth: usize,
        up: Option<(Object, u32)>,
    }
    let mut found = Vec::new();
    let mut stack = vec![Visit { object: frame.clone(), depth: 0, up: None }];
    let mut seen = 0;
    while let Some(Visit { object, depth, up }) = stack.pop() {
        seen += 1;
        if seen > TERMINAL_NODES || Instant::now() > deadline {
            return None;
        }
        let Some(role) = role(conn, &object) else { continue };
        let up_role = up.as_ref().map(|(_, role)| *role);
        let a_tab = match route {
            TabRoute::AdwPages => role == ROLE_PANEL && up_role.is_some_and(|r| r != ROLE_PAGE_TAB_LIST),
            TabRoute::Notebook => role == ROLE_PAGE_TAB && up_role == Some(ROLE_PAGE_TAB_LIST),
        };
        if a_tab
            && let Some((list, _)) = &up
            && property::<String>(conn, &object, ACCESSIBLE, "Name").is_some_and(|name| name.contains(mark))
            && let Some(node) = tab_in(conn, list, &object, route, deadline)
        {
            found.push(node);
            continue;
        }
        // A terminal holds no tabs, nor does a notebook's page.
        let leaf = role == ROLE_TERMINAL || (route == TabRoute::Notebook && role == ROLE_PAGE_TAB);
        if leaf || depth >= TERMINAL_DEPTH {
            continue;
        }
        if let Some(kids) = children(conn, &object) {
            stack.extend(kids.into_iter().rev().map(|kid| Visit {
                object: kid,
                depth: depth + 1,
                up: Some((object.clone(), role)),
            }));
        }
    }
    Some(found)
}

/// `tab` as one of the tabs of `list`; `None` when `list` isn't a tab list
/// the way `route` builds one, or not exactly one tab of it is selected.
fn tab_in(
    conn: &zbus::blocking::Connection,
    list: &Object,
    tab: &Object,
    route: TabRoute,
    deadline: Instant,
) -> Option<TabNode> {
    let siblings = children(conn, list)?;
    let at = siblings.iter().position(|sibling| sibling == tab)?;
    let selected = match route {
        TabRoute::AdwPages => selected_page(conn, &siblings, deadline)?,
        TabRoute::Notebook => {
            let interfaces = call::<Vec<String>>(conn, list, ACCESSIBLE, "GetInterfaces", &())?;
            if !interfaces.iter().any(|i| i == SELECTION) {
                return None;
            }
            selected_tab(conn, &siblings, deadline)?
        }
    };
    Some(TabNode { list: list.clone(), at, selected, tabs: siblings.len() })
}

/// The selected tab of Ptyxis's tab view: every tab is a panel, and only
/// the selected one has anything under it, a terminal among it.
fn selected_page(conn: &zbus::blocking::Connection, pages: &[Object], deadline: Instant) -> Option<usize> {
    let mut built = Vec::with_capacity(pages.len());
    for page in pages {
        if Instant::now() > deadline || role(conn, page)? != ROLE_PANEL {
            return None;
        }
        built.push(!children(conn, page)?.is_empty());
    }
    let selected = selected_among(&built)?;
    holds_terminal(conn, &pages[selected], deadline).then_some(selected)
}

/// The selected tab of a GTK notebook's tab list: its one page tab with the
/// `SELECTED` state.
fn selected_tab(conn: &zbus::blocking::Connection, tabs: &[Object], deadline: Instant) -> Option<usize> {
    let mut selected = Vec::with_capacity(tabs.len());
    for tab in tabs {
        if Instant::now() > deadline || role(conn, tab)? != ROLE_PAGE_TAB {
            return None;
        }
        let states = call::<Vec<u32>>(conn, tab, ACCESSIBLE, "GetState", &())?;
        selected.push(has_state(&states, STATE_SELECTED));
    }
    selected_among(&selected)
}

/// Whether a terminal is under `object`, within the walk's bounds.
fn holds_terminal(conn: &zbus::blocking::Connection, object: &Object, deadline: Instant) -> bool {
    let mut stack = vec![(object.clone(), 0usize)];
    let mut seen = 0;
    while let Some((object, depth)) = stack.pop() {
        seen += 1;
        if seen > TERMINAL_NODES || Instant::now() > deadline {
            return false;
        }
        if role(conn, &object) == Some(ROLE_TERMINAL) {
            return true;
        }
        if depth < TERMINAL_DEPTH
            && let Some(kids) = children(conn, &object)
        {
            stack.extend(kids.into_iter().map(|kid| (kid, depth + 1)));
        }
    }
    false
}

/// Selects tab `to` of `node`'s list in `frame`, and waits until it shows.
fn move_tab(
    conn: &zbus::blocking::Connection,
    frame: &Object,
    node: &TabNode,
    to: usize,
    route: TabRoute,
    deadline: Instant,
) -> Result<(), TabError> {
    if to >= node.tabs {
        return Err(TabError::NotFound);
    }
    match route {
        TabRoute::AdwPages => {
            // One step at a time, each one seen before the next.
            let mut selected = node.selected;
            while let Some(page) = Page::toward(selected, to) {
                if Instant::now() > deadline {
                    return Err(TabError::TimedOut);
                }
                run_page_action(conn, frame, page)?;
                selected = page_moved(conn, &node.list, selected, page, deadline)?;
            }
            Ok(())
        }
        TabRoute::Notebook => {
            if node.selected == to {
                return Ok(());
            }
            let index = i32::try_from(to).map_err(|_| TabError::CantSwitch)?;
            if call::<bool>(conn, &node.list, SELECTION, "SelectChild", &(index,)) != Some(true) {
                return Err(TabError::CantSwitch);
            }
            let until = (Instant::now() + TAB_STEP).min(deadline);
            loop {
                let shown = children(conn, &node.list).and_then(|tabs| selected_tab(conn, &tabs, deadline));
                if shown == Some(to) {
                    return Ok(());
                }
                if Instant::now() >= until {
                    return Err(TabError::CantSwitch);
                }
                std::thread::sleep(TAB_POLL);
            }
        }
    }
}

/// Runs the window's `page` action by its name: every action's name is read,
/// the index taken from this read, and the name at that index read once
/// more right before running it. The same list holds `page.close` and
/// `window.close`, so an index is never reused or guessed.
fn run_page_action(conn: &zbus::blocking::Connection, frame: &Object, page: Page) -> Result<(), TabError> {
    let count = property::<i32>(conn, frame, ACTION, "NActions").ok_or(TabError::CantSwitch)?;
    if !(0..=MAX_ACTIONS).contains(&count) {
        return Err(TabError::CantSwitch);
    }
    let names = (0..count)
        .map(|i| call::<String>(conn, frame, ACTION, "GetName", &(i,)))
        .collect::<Option<Vec<String>>>()
        .ok_or(TabError::CantSwitch)?;
    let index = pick_action(&names, page).and_then(|i| i32::try_from(i).ok()).ok_or(TabError::CantSwitch)?;
    if call::<String>(conn, frame, ACTION, "GetName", &(index,)).as_deref() != Some(page.name()) {
        return Err(TabError::CantSwitch);
    }
    if call::<bool>(conn, frame, ACTION, "DoAction", &(index,)) != Some(true) {
        return Err(TabError::CantSwitch);
    }
    Ok(())
}

/// After `page` from tab `from`: the selected tab, once it's the next one
/// that way, within [`TAB_STEP`]. Any other move is refused.
fn page_moved(
    conn: &zbus::blocking::Connection,
    list: &Object,
    from: usize,
    page: Page,
    deadline: Instant,
) -> Result<usize, TabError> {
    let wanted = page.after(from).ok_or(TabError::CantSwitch)?;
    let until = (Instant::now() + TAB_STEP).min(deadline);
    loop {
        match children(conn, list).and_then(|pages| selected_page(conn, &pages, deadline)) {
            Some(shown) if shown == wanted => return Ok(shown),
            Some(shown) if shown != from => return Err(TabError::CantSwitch),
            _ => {}
        }
        if Instant::now() >= until {
            return Err(TabError::CantSwitch);
        }
        std::thread::sleep(TAB_POLL);
    }
}

/// `text` cut to at most `max` bytes, on a character boundary.
fn cut(text: &str, max: usize) -> String {
    let end = (0..=max.min(text.len())).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
    text[..end].to_owned()
}

/// `text` cut to its last `max` bytes, on a character boundary, and how many
/// characters were dropped from the front.
fn cut_front(text: &str, max: usize) -> (String, u32) {
    if text.len() <= max {
        return (text.to_owned(), 0);
    }
    let start = (text.len() - max..=text.len()).find(|&i| text.is_char_boundary(i)).unwrap_or(text.len());
    let dropped = u32::try_from(text[..start].chars().count()).unwrap_or(u32::MAX);
    (text[start..].to_owned(), dropped)
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
        assert!(has_state(&[1 << STATE_MULTI_LINE], STATE_MULTI_LINE));
    }

    #[test]
    fn the_phones_own_typing_isnt_someone_at_the_pc() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let since = Some(at(0));
        let quiet = Bursts { active: 0, started: None, ended: None };
        assert!(quiet.someone_else(at(100), since));
        assert!(!quiet.someone_else(at(100), Some(at(200))), "before the phone watched");
        assert!(!quiet.someone_else(at(100), None));
        let typing = Bursts { active: 1, started: Some(at(1000)), ended: None };
        assert!(!typing.someone_else(at(5000), since), "nothing is judged while the phone types");
        let burst = Bursts { active: 0, started: Some(at(1000)), ended: Some(at(1040)) };
        assert!(!burst.someone_else(at(960), since), "just before it started");
        assert!(!burst.someone_else(at(1020), since), "while it was typed");
        assert!(!burst.someone_else(at(1400), since), "a key a busy compositor handled late");
        assert!(burst.someone_else(at(1500), since));
        assert!(burst.someone_else(at(900), since), "well before it");
    }

    #[test]
    fn a_reply_tells_the_phones_input_from_someone_at_the_pc() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let quiet = Bursts { active: 0, started: None, ended: None };
        assert!(!quiet.holds(at(100)));
        let typing = Bursts { active: 1, started: Some(at(1000)), ended: None };
        assert!(typing.holds(at(5000)));
        let burst = Bursts { active: 0, started: Some(at(1000)), ended: Some(at(1040)) };
        assert!(burst.holds(at(960)) && burst.holds(at(1020)) && burst.holds(at(1400)));
        assert!(!burst.holds(at(1500)));
        assert!(!burst.holds(at(900)));
    }

    #[test]
    fn a_window_shows_one_terminal() {
        let terminal = |path: &str, showing: bool| FoundTerminal {
            terminal: Object { sender: ":1.5".into(), path: path.into() },
            states: if showing { vec![1 << STATE_SHOWING] } else { vec![0] },
        };
        let path = |t: Option<FoundTerminal>| t.map(|t| t.terminal.path);
        // Ptyxis: the selected tab's only, whatever its states say.
        assert_eq!(path(shown_terminal(vec![terminal("/a", false)])), Some("/a".into()));
        // GNOME Terminal: every tab's, one showing.
        let tabs = vec![terminal("/a", false), terminal("/b", true), terminal("/c", false)];
        assert_eq!(path(shown_terminal(tabs)), Some("/b".into()));
        assert_eq!(path(shown_terminal(vec![terminal("/a", true), terminal("/b", true)])), None, "split");
        assert_eq!(path(shown_terminal(vec![terminal("/a", false), terminal("/b", false)])), None);
        assert_eq!(path(shown_terminal(Vec::new())), None);
    }

    #[test]
    fn the_pc_is_free_once_nobody_used_it_for_a_while() {
        let s = Duration::from_secs;
        let ms = Duration::from_millis;
        assert_eq!(free_after(s(7), s(5), false), Duration::ZERO);
        assert_eq!(free_after(s(5), s(5), false), Duration::ZERO);
        assert_eq!(free_after(ms(1200), s(5), false), ms(3800));
        assert_eq!(free_after(Duration::ZERO, s(5), false), s(5));
        assert_eq!(free_after(ms(300), s(5), true), Duration::ZERO, "the phone's own input");
    }

    #[test]
    fn only_a_page_action_found_by_its_name_is_run() {
        // Ptyxis 50.1's frame: 23 actions, not in name order.
        let mut names: Vec<String> = (0..23).map(|i| format!("win.action-{i}")).collect();
        names[3] = "page.close".into();
        names[4] = "page.close-others".into();
        names[7] = "page.detach".into();
        names[9] = "window.close".into();
        names[11] = "page.previous".into();
        names[15] = "page.next".into();
        assert_eq!(pick_action(&names, Page::Next), Some(15));
        assert_eq!(pick_action(&names, Page::Previous), Some(11));

        let mut twice = names.clone();
        twice[20] = "page.next".into();
        assert_eq!(pick_action(&twice, Page::Next), None, "duplicates");
        assert_eq!(pick_action(&twice, Page::Previous), None);
        let mut one_way = names.clone();
        one_way[11] = "page.move-left".into();
        assert_eq!(pick_action(&one_way, Page::Next), None, "both or none");
        assert_eq!(pick_action(&[], Page::Next), None);
    }

    #[test]
    fn steps_go_one_tab_at_a_time_toward_the_tab() {
        assert_eq!(Page::toward(0, 2), Some(Page::Next));
        assert_eq!(Page::toward(3, 1), Some(Page::Previous));
        assert_eq!(Page::toward(2, 2), None);
        assert_eq!(Page::Next.after(1), Some(2));
        assert_eq!(Page::Previous.after(1), Some(0));
        assert_eq!(Page::Previous.after(0), None, "never past the first");
    }

    #[test]
    fn one_tab_is_the_selected_one() {
        assert_eq!(selected_among(&[false, true, false]), Some(1));
        assert_eq!(selected_among(&[true]), Some(0));
        assert_eq!(selected_among(&[true, false, true]), None);
        assert_eq!(selected_among(&[false, false]), None);
        assert_eq!(selected_among(&[]), None);
    }

    #[test]
    fn a_tab_spot_shows_no_title_in_logs() {
        let spot =
            TabSpot { frame_title: "G7 0123456789abcdef — claude".into(), at: 2, selected: 0, tabs: 3 };
        let shown = format!("{spot:?}");
        assert!(!shown.contains("G7") && !shown.contains("claude"), "{shown}");
    }

    #[test]
    fn text_is_cut_on_character_boundaries() {
        assert_eq!(cut("héllo", 2), "h");
        assert_eq!(cut("héllo", 3), "hé");
        assert_eq!(cut("hi", 64), "hi");
        assert_eq!(cut_front("héllo", 4), ("llo".to_owned(), 2));
        assert_eq!(cut_front("hello", 9), ("hello".to_owned(), 0));
        assert_eq!(cut_front("🐝🐝", 5), ("🐝".to_owned(), 1));
    }
}
