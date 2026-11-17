// SPDX-License-Identifier: GPL-3.0-or-later
//! Dispatches remote touchpad, keyboard and presentation input from a paired
//! phone to `sys::input` and updates the laser pointer overlay state.

use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use g7link_core::{
    DeviceId, KeyMod, LinkState, NodeEvent, RemoteInput, RemoteTextState, remote_keys, remote_limits,
};

use crate::{core_host, state::Changes, sys};

/// When the laser pointer last heard from its phone (ms since the Unix
/// epoch; 0 when it's off). The phone repeats it every second while held,
/// so a dot that stops hearing (the phone left) is hidden.
static LASER_SEEN: AtomicU64 = AtomicU64::new(0);
const LASER_TIMEOUT: Duration = Duration::from_secs(4);

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn hide_laser() {
    LASER_SEEN.store(0, Ordering::SeqCst);
    core_host::host().hub.update(|s| if s.laser.take().is_some() { Changes::REMOTE } else { Changes::NONE });
}

/// Phones whose typing box is open, and the last edit of each that ran.
static WATCHERS: Mutex<Option<HashMap<DeviceId, u32>>> = Mutex::new(None);

fn watchers<T>(f: impl FnOnce(&mut HashMap<DeviceId, u32>) -> T) -> T {
    f(WATCHERS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new))
}

fn injected(done: bool) -> Result<(), String> {
    if done { Ok(()) } else { Err(remote_limits::NOT_INJECTED.into()) }
}

/// Runs a phone's input. `Err` holds what to tell the phone: nothing was
/// typed or pressed, or an edit was meant for a field that lost the focus.
pub fn handle_input(peer: &DeviceId, input: RemoteInput) -> Result<(), String> {
    let _ = crate::input_share::enter_remote(*peer);
    let done = run_input(peer, input);
    sys::text_focus::note_injected();
    done
}

fn run_input(peer: &DeviceId, input: RemoteInput) -> Result<(), String> {
    match input {
        RemoteInput::Move { dx, dy } => sys::input::move_relative(dx, dy),
        RemoteInput::Button { button, action } => sys::input::mouse_button(button, action),
        RemoteInput::Scroll { dx, dy } => sys::input::scroll(dx, dy),
        RemoteInput::Text { text } => return injected(sys::input::type_text_checked(&text)),
        // Terminals paste with Ctrl+Shift+V: Ctrl+V is a key of their own.
        RemoteInput::Key { key, mods }
            if mods.is_empty()
                && key.eq_ignore_ascii_case(remote_keys::PASTE)
                && sys::text_focus::in_terminal() =>
        {
            return injected(sys::input::press_key_checked("v", &[KeyMod::Ctrl, KeyMod::Shift]));
        }
        RemoteInput::Key { key, mods } => return injected(sys::input::press_key_checked(&key, &mods)),
        RemoteInput::Edit { focus, seq, range, moves, backspaces, text } => {
            if focus != 0 && focus != sys::text_focus::current_focus() {
                return Err(remote_limits::STALE_FOCUS.into());
            }
            match range {
                Some((start, end)) => edit_range(focus, start, end, &text)?,
                None => sys::input::edit(moves, backspaces, &text)?,
            }
            applied(peer, seq);
        }
        RemoteInput::Caret { focus, seq, caret, anchor } => {
            if focus != 0 && focus != sys::text_focus::current_focus() {
                return Err(remote_limits::STALE_FOCUS.into());
            }
            injected(sys::text_focus::set_caret(focus, caret, anchor))?;
            applied(peer, seq);
            sys::text_focus::mark_dirty();
        }
        RemoteInput::Slide { action } => sys::input::slide(action),
        RemoteInput::Laser { on: true, x, y } => {
            let x = x.clamp(0.0, 1.0);
            let y = y.clamp(0.0, 1.0);
            let peer = *peer;
            core_host::host().hub.update(|s| {
                let next = Some((peer, x, y));
                if s.laser == next {
                    return Changes::NONE;
                }
                s.laser = next;
                Changes::REMOTE
            });
            // One watchdog while it's shown.
            if LASER_SEEN.swap(now_ms(), Ordering::SeqCst) == 0 {
                core_host::spawn(async {
                    loop {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        let seen = LASER_SEEN.load(Ordering::SeqCst);
                        if seen == 0 {
                            return;
                        }
                        if now_ms().saturating_sub(seen) > LASER_TIMEOUT.as_millis() as u64 {
                            hide_laser();
                            return;
                        }
                    }
                });
            }
        }
        RemoteInput::Laser { on: false, .. } => hide_laser(),
    }
    Ok(())
}

/// Replaces `start..end` of the focused shared field with `text`: selects it
/// and types over it, or, when the app won't select, moves there with the
/// arrow keys and deletes.
fn edit_range(focus: u32, start: u32, end: u32, text: &str) -> Result<(), &'static str> {
    if sys::text_focus::select(focus, start, end) {
        if start == end && text.is_empty() {
            return Ok(());
        }
        // Typing replaces the selection; with nothing to type, one Backspace deletes it.
        let backspaces = u16::from(start != end && text.is_empty());
        return sys::input::edit(0, backspaces, text);
    }
    let caret = sys::text_focus::caret_offset(focus).ok_or(remote_limits::NOT_INJECTED)?;
    let steps = i32::from(remote_limits::MAX_EDIT_STEPS);
    let moves = i64::from(end) - i64::from(caret);
    let moves = i32::try_from(moves).ok().filter(|m| m.abs() <= steps).ok_or(remote_limits::NOT_INJECTED)?;
    let backspaces = u16::try_from(end - start)
        .ok()
        .filter(|&b| b <= remote_limits::MAX_EDIT_STEPS)
        .ok_or(remote_limits::NOT_INJECTED)?;
    sys::input::edit(moves, backspaces, text)
}

/// The phone's edit `seq` ran: the next state it's sent says so.
fn applied(peer: &DeviceId, seq: u32) {
    watchers(|watchers| {
        if let Some(applied) = watchers.get_mut(peer) {
            *applied = seq;
        }
    });
}

/// A phone opened (`on`) or closed its typing box.
fn watch(peer: DeviceId, on: bool) {
    let any = watchers(|watchers| {
        if on {
            watchers.insert(peer, 0);
        } else {
            watchers.remove(&peer);
        }
        !watchers.is_empty()
    });
    if on {
        sys::input::prepare();
    }
    sys::text_focus::set_watching(any);
}

/// The field that has the focus on this PC changed: phones watching it hear.
pub fn text_state_changed(state: RemoteTextState) {
    let (Some(host), Some(node)) = (core_host::try_host(), core_host::node()) else { return };
    let online: Vec<DeviceId> = host.hub.read(|s| {
        s.devices.iter().filter(|d| matches!(d.link, LinkState::Online { .. })).map(|d| d.id).collect()
    });
    let phones: Vec<(DeviceId, u32)> = watchers(|watchers| {
        watchers.iter().filter(|(id, _)| online.contains(id)).map(|(id, applied)| (*id, *applied)).collect()
    });
    if phones.is_empty() {
        return;
    }
    core_host::spawn(async move {
        for (phone, applied) in phones {
            let state = RemoteTextState { applied, ..state.clone() };
            if let Err(error) = node.remote_text_state(phone, &state).await {
                tracing::debug!(%error, "couldn't tell a phone about the focused field");
            }
        }
    });
}

/// Clears any active laser pointer and releases held mouse buttons (e.g. when
/// `remote_input` is turned off).
pub fn stop_all() {
    let _ = crate::input_share::leave_remote();
    sys::input::release_held_buttons();
    hide_laser();
}

pub fn on_event(event: &NodeEvent) {
    match event {
        NodeEvent::LinkChanged { device, link: LinkState::Offline { .. } } => {
            let _ = crate::input_share::leave_remote();
            sys::input::release_held_buttons();
            watch(*device, false);
        }
        NodeEvent::DeviceRemoved(device) => {
            let _ = crate::input_share::leave_remote();
            sys::input::release_held_buttons();
            watch(*device, false);
        }
        NodeEvent::TextWatch { device, on } => watch(*device, *on),
        NodeEvent::RemoteInputRequested { .. } => {
            core_host::host().hub.update(|s| s.apply(event));
        }
        _ => {}
    }
}

/// What has the focus on this PC takes text, or no longer does. Connected
/// phones that may control this PC hear it, so their keyboard can open.
pub fn text_focus_changed(editable: bool) {
    let (Some(host), Some(node)) = (core_host::try_host(), core_host::node()) else { return };
    let phones: Vec<DeviceId> = host.hub.read(|s| {
        s.devices.iter().filter(|d| matches!(d.link, LinkState::Online { .. })).map(|d| d.id).collect()
    });
    core_host::spawn(async move {
        for phone in phones {
            if let Err(error) = node.remote_text_focus(phone, editable).await {
                tracing::debug!(%error, "couldn't tell a phone about the text focus");
            }
        }
    });
}
