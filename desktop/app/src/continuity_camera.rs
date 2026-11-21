// SPDX-License-Identifier: GPL-3.0-or-later
//! Continuity Camera coordinator: initiates "Take photo with phone" and "Scan
//! document with phone" from the tray menu, global hotkeys, or the G7 Link UI,
//! remembers the external foreground window (`XWin`) that was active when the
//! request started, places the captured image onto the clipboard on arrival,
//! and either restores focus + synthesizes `Ctrl+V` into the original target
//! window or prompts the user if that window was closed.
//!
//! A capture for an agent ([`CaptureDest::AgentComposer`] or
//! [`CaptureDest::Launcher`]) never touches the clipboard or a window: its
//! picture goes to `crate::agents`, which hands it to the agent host.

use crate::i18n::tr;
use std::{
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use g7link_core::{CameraCaptureMode, DeviceId, Error, LinkState, NodeEvent, continuity_camera::CaptureFor};

use crate::{core_host, state::Changes};

static REQUEST_SEQ: AtomicU64 = AtomicU64::new(1);

/// Where a captured picture goes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CaptureDest {
    /// The clipboard, then the window that was in front when it started.
    #[default]
    Paste,
    /// The Agents page's composer of thread `key`.
    AgentComposer { key: String },
    /// The launcher's `ui.photo` request `request`.
    Launcher { request: String },
}

impl CaptureDest {
    /// It goes to an agent, never to the clipboard.
    fn is_agent(&self) -> bool {
        !matches!(self, CaptureDest::Paste)
    }
}

/// A capture for an agent that has started on a phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub request_id: String,
    pub device_name: String,
}

/// Why a capture for an agent didn't start, or ended without a picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// No paired phone at that place, or it isn't connected.
    NoPhone,
    /// The phone said no (busy, no permission) or the user cancelled it
    /// there or here.
    Cancelled,
    /// Another capture took its place.
    Replaced,
    /// The phone went away while it waited.
    Offline,
    /// The request couldn't be sent.
    Failed(String),
}

#[derive(Debug, Clone)]
struct ActiveRequest {
    request_id: String,
    device: DeviceId,
    device_name: String,
    mode: CameraCaptureMode,
    target_hwnd: Option<isize>,
    target_title: String,
    queued_offline: bool,
    dest: CaptureDest,
}

#[derive(Debug, Clone)]
struct PendingPastePrompt {
    title: String,
    body: String,
    target_name: String,
}

#[derive(Debug, Default)]
struct ContinuityState {
    active: Option<ActiveRequest>,
    prompt: Option<PendingPastePrompt>,
}

static STATE: Mutex<ContinuityState> = Mutex::new(ContinuityState { active: None, prompt: None });

/// Snapshot of Continuity Camera state exposed to QML via `AppController`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContinuityView {
    pub busy: bool,
    pub mode: String,
    pub status: String,
    pub paste_prompt_visible: bool,
    pub paste_prompt_title: String,
    pub paste_prompt_body: String,
    pub paste_target_name: String,
}

/// Pure decision for what to do after placing a captured image on the clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DeliveryDecision {
    /// The remembered target window is still alive and visible: focus it and paste.
    AutoPaste { xwin: isize, toast: String },
    /// A target window was remembered when the request started, but is no longer
    /// alive/visible when the capture arrived. Do not blindly paste elsewhere;
    /// ask the user in G7 Link.
    PromptMissingTarget { title: String, body: String, target_name: String },
    /// No external target window was remembered when the request started.
    ClipboardOnly { toast: String },
}

fn decide_delivery_action(
    mode: CameraCaptureMode,
    device_name: &str,
    target: Option<(isize, &str)>,
    target_still_alive: bool,
) -> DeliveryDecision {
    let (item_cap, item_lower) = match mode {
        CameraCaptureMode::Photo => ("Photo", "photo"),
        CameraCaptureMode::Scan => ("Scanned document", "scanned document"),
    };
    if let Some((xwin, raw_title)) = target
        && xwin != 0
    {
        let trimmed = raw_title.trim();
        let target_name =
            if trimmed.is_empty() { "your previous window".to_owned() } else { trimmed.to_owned() };
        if target_still_alive {
            return DeliveryDecision::AutoPaste {
                xwin,
                toast: format!("{item_cap} from {device_name} pasted into {target_name}"),
            };
        }
        return DeliveryDecision::PromptMissingTarget {
            title: format!("{item_cap} ready on clipboard"),
            body: format!(
                "“{target_name}” is no longer open, so G7 Link didn’t paste automatically. Switch to the window where you want your {item_lower} from {device_name} and click Paste into active window, or press Ctrl+V anywhere."
            ),
            target_name,
        };
    }
    DeliveryDecision::ClipboardOnly { toast: format!("{item_cap} from {device_name} copied to clipboard") }
}

fn next_request_id() -> String {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let seq = REQUEST_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("cc-{ms}-{seq}")
}

fn notify_continuity() {
    core_host::host().hub.update(|_| Changes::CONTINUITY);
}

/// Returns a snapshot of the current Continuity Camera UI state.
pub fn view() -> ContinuityView {
    let guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let (busy, mode, status) = match &guard.active {
        // The composer and the launcher show their own wait.
        Some(active) if !active.queued_offline && !active.dest.is_agent() => {
            let mode_str = active.mode.as_str().to_owned();
            let status_str = match active.mode {
                CameraCaptureMode::Photo => {
                    tr!("ContinuityCamera", "Take a photo on %1…", active.device_name)
                }
                CameraCaptureMode::Scan => {
                    tr!("ContinuityCamera", "Scan a document on %1…", active.device_name)
                }
            };
            (true, mode_str, status_str)
        }
        _ => (false, String::new(), String::new()),
    };
    let (paste_prompt_visible, paste_prompt_title, paste_prompt_body, paste_target_name) =
        if let Some(prompt) = &guard.prompt {
            (true, prompt.title.clone(), prompt.body.clone(), prompt.target_name.clone())
        } else {
            (false, String::new(), String::new(), String::new())
        };
    ContinuityView {
        busy,
        mode,
        status,
        paste_prompt_visible,
        paste_prompt_title,
        paste_prompt_body,
        paste_target_name,
    }
}

/// Starts a Continuity Camera capture (`"photo"` or `"scan"`) on the active
/// paired phone, remembering the current external foreground window.
pub fn start(mode_raw: &str) {
    start_for_index(0, mode_raw);
}

/// Starts a Continuity Camera capture preferring the paired device at
/// `preferred_index` if it is online, or the first online phone, or the
/// preferred offline paired phone (queueing a 2-minute expiring request).
pub fn start_for_index(preferred_index: usize, mode_raw: &str) {
    let mode = CameraCaptureMode::parse(mode_raw).unwrap_or(CameraCaptureMode::Photo);
    start_paste(preferred_index, mode);
}

/// [`start_for_index`]'s path: the capture that's pasted.
fn start_paste(preferred_index: usize, mode: CameraCaptureMode) -> Option<Started> {
    let target = crate::sys::tray::snapshot_target_window();

    let chosen = core_host::host().hub.read(|s| {
        s.devices
            .get(preferred_index)
            .filter(|d| matches!(d.link, LinkState::Online { .. }))
            .or_else(|| s.devices.iter().find(|d| matches!(d.link, LinkState::Online { .. })))
            .or_else(|| s.devices.get(preferred_index))
            .or_else(|| s.devices.first())
            .map(|d| (d.id, d.info.name.clone(), matches!(d.link, LinkState::Online { .. })))
    });
    let Some((device_id, device_name, online)) = chosen else {
        crate::bridge::app::show_message(tr!(
            "ContinuityCamera",
            "Pair your phone first to take a photo or scan a document."
        ));
        return None;
    };

    let request_id = next_request_id();
    let (target_hwnd, target_title) = match target {
        Some((xwin, title)) => (Some(xwin), title),
        None => (None, String::new()),
    };

    let previous = {
        let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
        guard.prompt = None;
        guard.active.replace(ActiveRequest {
            request_id: request_id.clone(),
            device: device_id,
            device_name: device_name.clone(),
            mode,
            target_hwnd,
            target_title,
            queued_offline: !online,
            dest: CaptureDest::Paste,
        })
    };
    notify_continuity();
    if let Some(prev) = previous.as_ref().filter(|p| p.dest.is_agent()) {
        crate::agents::photo_ended(&prev.dest, &prev.request_id, Ended::Replaced);
    }

    if online {
        let open_msg = match mode {
            CameraCaptureMode::Photo => tr!("ContinuityCamera", "Opening camera on %1…", device_name),
            CameraCaptureMode::Scan => {
                tr!("ContinuityCamera", "Opening document scanner on %1…", device_name)
            }
        };
        crate::bridge::app::show_message(open_msg);
    }

    let started = Started { request_id: request_id.clone(), device_name: device_name.clone() };
    let node = core_host::node()?;
    core_host::spawn(async move {
        if let Some(prev) = previous {
            let _ = node.cancel_camera_capture(prev.device, prev.request_id, Some("replaced".into())).await;
        }
        match node.request_camera_capture(device_id, request_id.clone(), mode).await {
            Ok(()) => {
                if !online {
                    let _ = crate::bridge::app::show_offline_queued_toast(device_id, None);
                }
            }
            Err(err) => {
                let should_report = {
                    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
                    if guard.active.as_ref().is_some_and(|a| a.request_id == request_id) {
                        guard.active = None;
                        true
                    } else {
                        false
                    }
                };
                if should_report {
                    notify_continuity();
                    crate::bridge::app::show_message(start_error(&err, &device_name));
                }
            }
        }
    });
    Some(started)
}

/// Why a capture couldn't start on `device_name`, in the user's words.
fn start_error(err: &Error, device_name: &str) -> String {
    match err {
        Error::Denied => {
            tr!("ContinuityCamera", "Allow Photos access for %1 in Settings first.", device_name)
        }
        Error::Unsupported => format!("{device_name} doesn’t support Continuity Camera yet."),
        Error::Offline => format!("{device_name} is not connected."),
        other => tr!("ContinuityCamera", "Couldn’t open camera on %1: %2", device_name, other),
    }
}

/// Starts a photo capture for an agent on the paired device at `index`: that
/// phone only, and only while it's connected (nothing is queued for later).
/// The picture reaches `crate::agents` ([`crate::agents::photo_arrived`]),
/// never the clipboard. A capture already running is replaced. `Paste`
/// takes [`start_for_index`]'s path.
pub fn start_for(index: usize, dest: CaptureDest) -> Result<Started, Ended> {
    if !dest.is_agent() {
        return start_paste(index, CameraCaptureMode::Photo).ok_or(Ended::NoPhone);
    }
    let chosen = core_host::host().hub.read(|s| {
        s.devices
            .get(index)
            .filter(|d| matches!(d.link, LinkState::Online { .. }))
            .map(|d| (d.id, d.info.name.clone()))
    });
    let (device, device_name) = chosen.ok_or(Ended::NoPhone)?;
    let node = core_host::node().ok_or(Ended::NoPhone)?;
    let request_id = next_request_id();
    let previous = {
        let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
        guard.prompt = None;
        guard.active.replace(ActiveRequest {
            request_id: request_id.clone(),
            device,
            device_name: device_name.clone(),
            mode: CameraCaptureMode::Photo,
            target_hwnd: None,
            target_title: String::new(),
            queued_offline: false,
            dest: dest.clone(),
        })
    };
    notify_continuity();
    if let Some(prev) = previous.as_ref().filter(|p| p.dest.is_agent()) {
        crate::agents::photo_ended(&prev.dest, &prev.request_id, Ended::Replaced);
    }
    let started = Started { request_id: request_id.clone(), device_name: device_name.clone() };
    let name = device_name;
    core_host::spawn(async move {
        if let Some(prev) = previous {
            let _ = node.cancel_camera_capture(prev.device, prev.request_id, Some("replaced".into())).await;
        }
        if let Err(err) = node
            .request_camera_capture_for(
                device,
                request_id.clone(),
                CameraCaptureMode::Photo,
                CaptureFor::Agent,
            )
            .await
        {
            let ended = match &err {
                Error::Offline => Ended::Offline,
                Error::NotPaired => Ended::NoPhone,
                other => Ended::Failed(start_error(other, &name)),
            };
            if let Some(active) = take_active(&request_id) {
                crate::agents::photo_ended(&active.dest, &active.request_id, ended);
            }
        }
    });
    Ok(started)
}

/// Takes the active request if it's `request_id`.
fn take_active(request_id: &str) -> Option<ActiveRequest> {
    let taken = {
        let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if guard.active.as_ref().is_some_and(|a| a.request_id == request_id) {
            guard.active.take()
        } else {
            None
        }
    };
    if taken.is_some() {
        notify_continuity();
    }
    taken
}

/// Cancels the capture `request_id` started for an agent, if it still runs
/// (`reason` goes to the phone: "cancelled" or "timeout"). Its destination
/// hears nothing more.
pub fn cancel_request(request_id: &str, reason: &str) {
    let Some(active) = take_active(request_id) else { return };
    if let Some(node) = core_host::node() {
        let reason = reason.to_owned();
        core_host::spawn(async move {
            let _ = node.cancel_camera_capture(active.device, active.request_id, Some(reason)).await;
        });
    }
}

/// Cancels an in-flight Continuity Camera request from the PC side.
pub fn cancel() {
    let active = {
        let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
        guard.active.take()
    };
    let Some(active) = active else { return };
    notify_continuity();
    if active.dest.is_agent() {
        crate::agents::photo_ended(&active.dest, &active.request_id, Ended::Cancelled);
    }
    if let Some(node) = core_host::node() {
        core_host::spawn(async move {
            let _ =
                node.cancel_camera_capture(active.device, active.request_id, Some("cancelled".into())).await;
        });
    }
}

/// Confirms pasting the clipboard image when the original target window had closed.
pub fn confirm_paste() {
    {
        let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
        guard.prompt = None;
    }
    notify_continuity();

    if let Some((xwin, title)) = crate::sys::tray::snapshot_target_window() {
        spawn_focus_and_paste(xwin);
        let label = if title.is_empty() { "active window".to_owned() } else { title };
        crate::bridge::app::show_message(tr!("ContinuityCamera", "Pasted into %1", label));
    } else {
        crate::bridge::app::show_message(tr!(
            "ContinuityCamera",
            "Image is on your clipboard. Press Ctrl+V in your target app."
        ));
    }
}

/// Dismisses the missing-window paste confirmation prompt, keeping the image on the clipboard.
pub fn dismiss_paste() {
    {
        let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
        guard.prompt = None;
    }
    notify_continuity();
}

fn spawn_focus_and_paste(xwin: isize) {
    let _ = std::thread::Builder::new().name("continuity-paste".into()).spawn(move || {
        if crate::sys::tray::focus_window(xwin) {
            std::thread::sleep(Duration::from_millis(65));
            if crate::sys::tray::is_window_alive_and_visible(xwin) {
                crate::sys::input::press_key("paste", &[]);
                crate::sys::tray::send_paste_fallback_if_not_foreground(xwin);
            }
        }
    });
}

/// Handles Continuity Camera events from `g7link_core`.
pub fn on_event(event: &NodeEvent) {
    match event {
        NodeEvent::CameraCaptureReceived { device, request_id, mode, mime, data, .. } => {
            let matched = {
                let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
                if guard.active.as_ref().is_some_and(|a| a.request_id == *request_id || a.device == *device) {
                    guard.active.take()
                } else {
                    None
                }
            };

            let device_name = matched.as_ref().map(|a| a.device_name.clone()).unwrap_or_else(|| {
                core_host::host().hub.read(|s| s.name_of(device).unwrap_or_else(|| device.short()))
            });

            // For an agent: to the host, never the clipboard or a window.
            if let Some(active) = matched.as_ref().filter(|a| a.dest.is_agent()) {
                notify_continuity();
                crate::agents::photo_arrived(&active.dest, &active.request_id, mime, data);
                return;
            }

            if let Err(e) = crate::sys::clipboard::write_image(mime, data) {
                tracing::warn!(error = %e, "can't copy continuity camera image to clipboard");
                notify_continuity();
                crate::bridge::app::show_message(tr!(
                    "ContinuityCamera",
                    "Couldn’t copy image to clipboard: %1",
                    e
                ));
                return;
            }

            let target_ref =
                matched.as_ref().and_then(|a| a.target_hwnd.map(|xwin| (xwin, a.target_title.as_str())));
            let alive =
                target_ref.is_some_and(|(xwin, _)| crate::sys::tray::is_window_alive_and_visible(xwin));

            match decide_delivery_action(*mode, &device_name, target_ref, alive) {
                DeliveryDecision::AutoPaste { xwin, toast } => {
                    {
                        let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
                        guard.prompt = None;
                    }
                    notify_continuity();
                    crate::bridge::app::show_message(toast);
                    spawn_focus_and_paste(xwin);
                }
                DeliveryDecision::PromptMissingTarget { title, body, target_name } => {
                    {
                        let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
                        guard.prompt = Some(PendingPastePrompt { title, body, target_name });
                    }
                    notify_continuity();
                    crate::bridge::app::request_activation();
                }
                DeliveryDecision::ClipboardOnly { toast } => {
                    {
                        let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
                        guard.prompt = None;
                    }
                    notify_continuity();
                    crate::bridge::app::show_message(toast);
                }
            }
        }
        NodeEvent::CameraCaptureCancelled { device, request_id, reason } => {
            let removed = {
                let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
                if guard.active.as_ref().is_some_and(|a| {
                    a.request_id == *request_id || (request_id.is_empty() && a.device == *device)
                }) {
                    guard.active.take()
                } else {
                    None
                }
            };
            if let Some(active) = removed {
                notify_continuity();
                if active.dest.is_agent() {
                    crate::agents::photo_ended(&active.dest, &active.request_id, Ended::Cancelled);
                }
                let msg = match reason.as_deref() {
                    Some("busy") => tr!("ContinuityCamera", "Camera is busy on %1.", active.device_name),
                    Some("permission_denied") => {
                        tr!("ContinuityCamera", "Camera permission was denied on %1.", active.device_name)
                    }
                    _ => tr!("ContinuityCamera", "Camera capture cancelled on %1.", active.device_name),
                };
                crate::bridge::app::show_message(msg);
            }
        }
        NodeEvent::LinkChanged { device, link } if !matches!(link, LinkState::Online { .. }) => {
            let cleared = {
                let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
                if guard.active.as_ref().is_some_and(|a| a.device == *device && !a.queued_offline) {
                    guard.active.take()
                } else {
                    None
                }
            };
            if let Some(active) = cleared {
                notify_continuity();
                if active.dest.is_agent() {
                    crate::agents::photo_ended(&active.dest, &active.request_id, Ended::Offline);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_decision_auto_pastes_when_target_window_is_alive() {
        let decision = decide_delivery_action(
            CameraCaptureMode::Photo,
            "Pixel 9",
            Some((0x1234, "Document.docx - Word")),
            true,
        );
        assert_eq!(
            decision,
            DeliveryDecision::AutoPaste {
                xwin: 0x1234,
                toast: "Photo from Pixel 9 pasted into Document.docx - Word".into(),
            }
        );
    }

    #[test]
    fn delivery_decision_prompts_when_target_window_disappeared() {
        let decision = decide_delivery_action(
            CameraCaptureMode::Scan,
            "Pixel 9",
            Some((0x1234, "Notes - Notion")),
            false,
        );
        match decision {
            DeliveryDecision::PromptMissingTarget { title, body, target_name } => {
                assert_eq!(title, "Scanned document ready on clipboard");
                assert_eq!(target_name, "Notes - Notion");
                assert!(body.contains("Notes - Notion"));
                assert!(body.contains("no longer open"));
            }
            other => panic!("expected PromptMissingTarget, got {other:?}"),
        }
    }

    #[test]
    fn the_paste_path_stays_the_default_and_an_agents_capture_never_pastes() {
        assert_eq!(CaptureDest::default(), CaptureDest::Paste);
        assert!(!CaptureDest::Paste.is_agent());
        assert!(CaptureDest::AgentComposer { key: "a".repeat(32) }.is_agent());
        assert!(CaptureDest::Launcher { request: "0123456789abcdef".into() }.is_agent());
    }

    #[test]
    fn delivery_decision_copies_only_when_no_external_target_was_recorded() {
        let decision = decide_delivery_action(CameraCaptureMode::Photo, "Pixel 9", None, false);
        assert_eq!(
            decision,
            DeliveryDecision::ClipboardOnly { toast: "Photo from Pixel 9 copied to clipboard".into() }
        );
    }
}
