// SPDX-License-Identifier: GPL-3.0-or-later
//! What has the keyboard focus, from the desktop's accessibility bus
//! (AT-SPI), for a phone's typing box:
//!
//! - whether it takes text (a focused object with the `EDITABLE` state), so
//!   the phone can open its keyboard, and whether it's a terminal, which
//!   pastes with Ctrl+Shift+V;
//! - while a phone watches, the focused field (`remote.text_state`): which
//!   field it is, and for a field whose app shares its text, the text around
//!   the caret, so the phone's box mirrors it.
//!
//! Apps that don't publish accessibility (some Electron builds, kitty or
//! Alacritty) aren't seen; for those the focused window, from G7 Linux's
//! Shell extension, is what the phone is told.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use g7link_core::{RemoteTextState, TextFieldKind, remote_limits};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use super::field_identity::{FieldTable, StableKey};
use super::shell_windows::{self, Window};

const REGISTRY: &str = "org.a11y.atspi.Registry";
const REGISTRY_PATH: &str = "/org/a11y/atspi/registry";
const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const TEXT: &str = "org.a11y.atspi.Text";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const OBJECT_EVENTS: &str = "org.a11y.atspi.Event.Object";
const FOCUS_EVENT: &str = "object:state-changed:focused";
/// Asked for only while a phone watches: apps send these for every change.
const TEXT_EVENTS: [&str; 3] =
    ["object:text-changed", "object:text-caret-moved", "object:text-selection-changed"];
const TEXT_MEMBERS: [&str; 3] = ["TextChanged", "TextCaretMoved", "TextSelectionChanged"];
/// `ATSPI_STATE_EDITABLE` and `ATSPI_STATE_MULTI_LINE`.
const STATE_EDITABLE: u32 = 7;
const STATE_MULTI_LINE: u32 = 17;
/// `ATSPI_ROLE_PASSWORD_TEXT`, `ATSPI_ROLE_TERMINAL` (VTE terminals such as
/// GNOME Terminal and Ptyxis publish it) and `ATSPI_ROLE_PARAGRAPH`.
const ROLE_PASSWORD_TEXT: u32 = 40;
const ROLE_TERMINAL: u32 = 60;
const ROLE_PARAGRAPH: u32 = 73;
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
/// Input this long after the phone's own is someone at the PC.
const OURS_FOR: Duration = Duration::from_millis(250);
/// How long after a window gets the focus an app has to tell of its focused
/// field before the window is taken as the field.
const WINDOW_GRACE: Duration = Duration::from_millis(150);
/// Characters read before and after the caret of a shared field, before
/// cutting to the protocol's byte limits.
const READ_BEFORE: i32 = 2048;
const READ_AFTER: i32 = 512;

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
});
static FIELDS: Mutex<Option<FieldTable>> = Mutex::new(None);
static IN_TERMINAL: AtomicBool = AtomicBool::new(false);
static WATCHING: AtomicBool = AtomicBool::new(false);
/// The next state goes out even if it's the one sent last: a phone just
/// started watching.
static RESEND: AtomicBool = AtomicBool::new(false);
/// Wakes the state thread: something changed, or the watch did.
static DIRTY: Mutex<bool> = Mutex::new(false);
static WAKE: Condvar = Condvar::new();
/// The last time the phone's input was injected.
static INJECTED_AT: Mutex<Option<Instant>> = Mutex::new(None);
/// The connection used to read and place text, apart from the listener's.
static READER: Mutex<Option<zbus::blocking::Connection>> = Mutex::new(None);
static SEND_STATE: OnceLock<fn(RemoteTextState)> = OnceLock::new();

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

/// The current focus's id, which a phone's edits carry.
pub fn current_focus() -> u32 {
    focus().id
}

/// The phone's input was just injected: activity at the PC around now is
/// the phone's, not someone at the PC.
pub fn note_injected() {
    *INJECTED_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
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
        let _ = std::thread::Builder::new().name("text-events".into()).spawn(move || {
            if let Some(conn) = reader() {
                register_text_events(&conn, on);
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
/// changes, and `state` with the focused field while a phone watches.
/// Threads for the app's lifetime.
pub fn watch(editable: fn(bool), state: fn(RemoteTextState)) {
    let _ = SEND_STATE.set(state);
    let _ = std::thread::Builder::new().name("text-focus".into()).spawn(move || {
        loop {
            follow(editable);
            std::thread::sleep(RETRY);
        }
    });
    let _ = std::thread::Builder::new().name("text-state".into()).spawn(send_states);
    shell_windows::follow(window_focused);
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

/// The reading connection, made again after it failed. It's also the one
/// that asks apps for text events while a phone watches.
fn reader() -> Option<zbus::blocking::Connection> {
    let mut reader = READER.lock().unwrap_or_else(|e| e.into_inner());
    if reader.is_none() {
        *reader = a11y_bus();
        if let Some(conn) = reader.as_ref()
            && WATCHING.load(Ordering::SeqCst)
        {
            register_text_events(conn, true);
        }
    }
    reader.clone()
}

fn drop_reader() {
    *READER.lock().unwrap_or_else(|e| e.into_inner()) = None;
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

/// Follows focus changes until the accessibility bus connection ends.
fn follow(editable_changed: fn(bool)) {
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

    let mut editable = false;
    for message in messages {
        let Ok(message) = message else { break };
        let header = message.header();
        let (Some(sender), Some(path), Some(member)) = (header.sender(), header.path(), header.member())
        else {
            continue;
        };
        let object = Object { sender: sender.to_string(), path: path.to_string() };
        if member.as_str() != "StateChanged" {
            // Text, caret or selection of the focused field.
            if focus().object.as_ref() == Some(&object) {
                mark_dirty();
            }
            continue;
        }
        let Some(gained) = focus_gained(&message) else { continue };
        let now = if gained {
            focus_moved(&conn, object)
        } else {
            focus_left(&object);
            false
        };
        if now != editable {
            editable = now;
            editable_changed(editable);
        }
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
    drop(focus);
    IN_TERMINAL.store(false, Ordering::Relaxed);
    mark_dirty();
}

/// A window got the focus: unless its app tells of a field soon, the
/// window is the field.
fn window_focused(window: Window) {
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
    let ours = *INJECTED_AT.lock().unwrap_or_else(|e| e.into_inner());
    let theirs =
        since.is_some_and(|since| activity > since) && ours.is_none_or(|ours| activity > ours + OURS_FOR);
    if theirs {
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
        };
        (focus.object.clone(), state)
    };
    if state.kind == TextFieldKind::Mirror
        && let Some(object) = object
    {
        match reader().and_then(|conn| read_text(&conn, &object)) {
            Some(text) => {
                state.start = text.start;
                state.before = text.before;
                state.after = text.after;
                state.anchor = text.anchor;
                state.more_before = text.more_before;
                state.more_after = text.more_after;
            }
            None => {
                // The app stopped answering: keys only, until the focus moves.
                drop_reader();
                let mut focus = focus();
                if focus.id == state.focus {
                    focus.kind = TextFieldKind::Opaque;
                }
                state.kind = TextFieldKind::Opaque;
            }
        }
    }
    state.is_valid().then_some(state)
}

struct FieldText {
    start: u32,
    before: String,
    after: String,
    anchor: Option<u32>,
    more_before: bool,
    more_after: bool,
}

fn read_text(conn: &zbus::blocking::Connection, object: &Object) -> Option<FieldText> {
    let caret = property::<i32>(conn, object, TEXT, "CaretOffset")?.max(0);
    let count = property::<i32>(conn, object, TEXT, "CharacterCount")?.max(caret);
    let from = caret.saturating_sub(READ_BEFORE).max(0);
    let to = caret.saturating_add(READ_AFTER).min(count);
    let before: String = call(conn, object, TEXT, "GetText", &(from, caret))?;
    let after: String = call(conn, object, TEXT, "GetText", &(caret, to))?;
    let (before, dropped) = cut_front(&before, remote_limits::MAX_STATE_BEFORE);
    let after_len = after.chars().count();
    let after = cut(&after, remote_limits::MAX_STATE_AFTER);
    let start = u32::try_from(from).ok()?.saturating_add(dropped);
    let selections = call::<i32>(conn, object, TEXT, "GetNSelections", &()).unwrap_or(0);
    let anchor = if selections > 0 {
        call::<(i32, i32)>(conn, object, TEXT, "GetSelection", &(0i32,))
            .filter(|(s, e)| s != e)
            .map(|(s, e)| if e == caret { s } else { e })
            .and_then(|anchor| u32::try_from(anchor).ok())
    } else {
        None
    };
    Some(FieldText {
        start,
        more_before: start > 0,
        more_after: to < count || after.chars().count() < after_len,
        before,
        after,
        anchor,
    })
}

/// Selects `start..end` of the focused field (or puts the caret at `start`
/// when they're equal), when `focus` still has it and its text is shared.
pub fn select(focus_id: u32, start: u32, end: u32) -> bool {
    let Some(object) = mirrored(focus_id) else { return false };
    let Some(conn) = reader() else { return false };
    let (Ok(start), Ok(end)) = (i32::try_from(start), i32::try_from(end)) else { return false };
    if start == end {
        return call::<bool>(&conn, &object, TEXT, "SetCaretOffset", &(start,)).unwrap_or(false);
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
    let object = mirrored(focus_id)?;
    let conn = reader()?;
    property::<i32>(&conn, &object, TEXT, "CaretOffset").and_then(|caret| u32::try_from(caret).ok())
}

fn mirrored(focus_id: u32) -> Option<Object> {
    let focus = focus();
    (focus.id == focus_id && focus.kind == TextFieldKind::Mirror).then(|| focus.object.clone()).flatten()
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
    StableKey { app, window: None, role, name, description, id, path }
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
    fn text_is_cut_on_character_boundaries() {
        assert_eq!(cut("héllo", 2), "h");
        assert_eq!(cut("héllo", 3), "hé");
        assert_eq!(cut("hi", 64), "hi");
        assert_eq!(cut_front("héllo", 4), ("llo".to_owned(), 2));
        assert_eq!(cut_front("hello", 9), ("hello".to_owned(), 0));
        assert_eq!(cut_front("🐝🐝", 5), ("🐝".to_owned(), 1));
    }
}
