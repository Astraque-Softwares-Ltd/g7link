// SPDX-License-Identifier: MPL-2.0
//! UniFFI bindings for `g7link-core`, used by the Android app.
//!
//! The boundary uses plain records and enums (see
//! docs/architecture/core-api.md). The node runs on its own Tokio runtime;
//! every async method executes there, so the foreign side can await it from
//! any executor (Kotlin coroutines). Events are delivered to a foreign
//! listener on a core thread; the listener should hand them off quickly.

use std::{path::PathBuf, sync::Arc, time::Duration};

use g7link_core as core;
use g7link_core::{
    ConnectionPath, DeviceId, LinkState, Node, NodeConfig, NodeEvent, PairingEvent,
    features::{CapabilityMatrix, FeatureState},
};
use tokio::{runtime::Runtime, sync::broadcast::error::RecvError};

uniffi::setup_scaffolding!();

mod agent_beside;
mod agent_control;
mod agent_folders;
mod agent_git;
mod agent_handoff;
mod agent_history;
mod agent_move;
mod agent_presence;
mod agent_reply;
pub mod agent_rows;
mod agent_scripts;

pub use agent_beside::*;
pub use agent_control::*;
pub use agent_folders::*;
pub use agent_git::*;
pub use agent_handoff::*;
pub use agent_history::*;
pub use agent_move::*;
pub use agent_presence::*;
pub use agent_reply::*;
pub use agent_scripts::*;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

// ---- Records and enums ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DeviceKind {
    Phone,
    Tablet,
    Desktop,
    Laptop,
    Unknown,
}

/// What has the keyboard focus on a PC, for the phone's typing box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TextFieldKind {
    /// A text field the PC reads: the typing box mirrors it.
    Mirror,
    /// Takes text, but its app doesn't share it.
    Opaque,
    /// A password field.
    Secret,
    /// Nothing that takes text.
    None,
}

impl From<core::TextFieldKind> for TextFieldKind {
    fn from(kind: core::TextFieldKind) -> Self {
        match kind {
            core::TextFieldKind::Mirror => TextFieldKind::Mirror,
            core::TextFieldKind::Opaque => TextFieldKind::Opaque,
            core::TextFieldKind::Secret => TextFieldKind::Secret,
            core::TextFieldKind::None => TextFieldKind::None,
        }
    }
}

/// The field that has the focus on a PC (`input.text_state`). Offsets count
/// Unicode code points.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct TextFieldState {
    /// Changes each time the focus moves; `remote_edit` and `remote_caret` carry it.
    pub focus: u32,
    /// The same each time the focus comes back to this field; 0 when unknown.
    pub field: u64,
    /// Changes when the PC doubts what the phone typed into this field.
    pub epoch: u32,
    pub rev: u32,
    /// The last edit `seq` from this phone that ran before the text was read.
    pub applied: u32,
    pub kind: TextFieldKind,
    pub app: Option<String>,
    pub label: Option<String>,
    /// The PC knows only the window, not the field.
    pub window: bool,
    pub multiline: bool,
    /// Where `before` starts in the field.
    pub start: u32,
    pub before: String,
    pub after: String,
    /// Where the selection starts, when there is one (the caret is its other end).
    pub anchor: Option<u32>,
    pub more_before: bool,
    pub more_after: bool,
    /// The focused app's icon, to fetch with `remote_app_icon` (cache by it:
    /// a changed icon has another key).
    pub icon: Option<String>,
}

/// Never prints the field's text or label.
impl std::fmt::Debug for TextFieldState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextFieldState")
            .field("focus", &self.focus)
            .field("field", &self.field)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl From<core::RemoteTextState> for TextFieldState {
    fn from(state: core::RemoteTextState) -> Self {
        TextFieldState {
            focus: state.focus,
            field: state.field,
            epoch: state.epoch,
            rev: state.rev,
            applied: state.applied,
            kind: state.kind.into(),
            app: state.app,
            label: state.label,
            window: state.window,
            multiline: state.multiline,
            start: state.start,
            before: state.before,
            after: state.after,
            anchor: state.anchor,
            more_before: state.more_before,
            more_after: state.more_after,
            icon: state.icon,
        }
    }
}

/// The route the user picked for a PC's touchpad (`set_remote_route`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RemoteRoute {
    /// The faster of the two, measured.
    Auto,
    /// The session; the Bluetooth link only when there's none.
    Network,
    /// The Bluetooth link; the session only when it's down.
    Bluetooth,
}

impl From<RemoteRoute> for core::RemoteRoute {
    fn from(route: RemoteRoute) -> Self {
        match route {
            RemoteRoute::Auto => core::RemoteRoute::Auto,
            RemoteRoute::Network => core::RemoteRoute::Network,
            RemoteRoute::Bluetooth => core::RemoteRoute::Bluetooth,
        }
    }
}

impl From<core::RemoteRoute> for RemoteRoute {
    fn from(route: core::RemoteRoute) -> Self {
        match route {
            core::RemoteRoute::Auto => RemoteRoute::Auto,
            core::RemoteRoute::Network => RemoteRoute::Network,
            core::RemoteRoute::Bluetooth => RemoteRoute::Bluetooth,
        }
    }
}

/// The way touchpad input reaches a PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RemotePath {
    Session,
    Ble,
}

impl From<core::RemotePath> for RemotePath {
    fn from(path: core::RemotePath) -> Self {
        match path {
            core::RemotePath::Session => RemotePath::Session,
            core::RemotePath::Ble => RemotePath::Ble,
        }
    }
}

/// Where a PC stands on keeping a Bluetooth link with this phone (`ble_use`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BleUseState {
    /// The link is up and held.
    Linked,
    /// Held: the PC can be connected to now (scan for its connect tag).
    Open,
    /// The PC's user is being asked: ask again (`ask` false) every 1.5 s.
    Asking,
    /// Not held: the PC's user said no, or its toggle is off.
    Off,
    /// The PC can't serve a link now (no GATT adapter, Bluetooth off).
    Unavailable,
    /// Another phone holds the PC's one link.
    Busy,
}

/// A PC's whole answer to `ble_use`: with `Open`, a PC from 0.1.14 also says
/// for how many seconds its connect window stays open (`window_secs`, 0: none
/// now) and, with none, in how many a renewal can open one (`retry_secs`).
/// Both are `None` from an older PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct BleUseReply {
    pub state: BleUseState,
    pub window_secs: Option<u32>,
    pub retry_secs: Option<u32>,
}

impl From<core::BleUseReply> for BleUseReply {
    fn from(reply: core::BleUseReply) -> Self {
        Self { state: reply.state.into(), window_secs: reply.window, retry_secs: reply.retry }
    }
}

impl From<core::BleUseState> for BleUseState {
    fn from(state: core::BleUseState) -> Self {
        match state {
            core::BleUseState::Linked => BleUseState::Linked,
            core::BleUseState::Open => BleUseState::Open,
            core::BleUseState::Asking => BleUseState::Asking,
            core::BleUseState::Off => BleUseState::Off,
            core::BleUseState::Unavailable => BleUseState::Unavailable,
            core::BleUseState::Busy => BleUseState::Busy,
        }
    }
}

/// Where a PC's touchpad route stands (`remote_route_state`,
/// `Event::RemoteRouteChanged`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RouteState {
    pub route: RemoteRoute,
    /// The path input takes now; None when neither is up.
    pub active: Option<RemotePath>,
    /// The chosen route's path is down and the other one is used.
    pub fallback: bool,
    pub session_rtt_ms: Option<u32>,
    pub ble_rtt_ms: Option<u32>,
    /// The PC offers `ble.remote`: the picker applies.
    pub ble_remote: bool,
    /// A Bluetooth link with that PC is up.
    pub ble_linked: bool,
}

impl From<core::RouteState> for RouteState {
    fn from(state: core::RouteState) -> Self {
        RouteState {
            route: state.route.into(),
            active: state.active.map(Into::into),
            fallback: state.fallback,
            session_rtt_ms: state.session_rtt_ms,
            ble_rtt_ms: state.ble_rtt_ms,
            ble_remote: state.ble_remote,
            ble_linked: state.ble_linked,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct ScreenRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Eq for ScreenRect {}

#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct ScreenCorners {
    pub tl: f32,
    pub tr: f32,
    pub br: f32,
    pub bl: f32,
}

impl Eq for ScreenCorners {}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ScreenShape {
    #[uniffi(default = 1)]
    pub v: u32,
    pub aspect: f32,
    #[uniffi(default = None)]
    pub corners: Option<ScreenCorners>,
    #[uniffi(default = [])]
    pub cutouts: Vec<ScreenRect>,
    #[uniffi(default = None)]
    pub cutout_path: Option<String>,
}

impl Eq for ScreenShape {}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeviceInfo {
    pub name: String,
    pub kind: DeviceKind,
    /// "android" or "linux".
    pub os: String,
    pub os_version: String,
    pub model: Option<String>,
    /// ARGB seed color for Material You sync.
    pub accent: Option<u32>,
    #[uniffi(default = None)]
    pub screen: Option<ScreenShape>,
    /// Identifies the device across reinstalls; see [`install_id_from`].
    #[uniffi(default = None)]
    pub install_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PowerLevel {
    Basic,
    Assist,
    Elevated,
    /// Desktops.
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Battery {
    /// 0-100.
    pub level: u8,
    pub charging: bool,
    /// "ac", "usb" or "wireless".
    pub plugged: Option<String>,
    /// Estimated minutes until full when charging, if known.
    #[uniffi(default = None)]
    pub full_in: Option<u16>,
    /// Cellular signal bars (0-4), if known.
    #[uniffi(default = None)]
    pub cell_bars: Option<u8>,
    /// Cellular network type ("5G", "LTE", "4G", "3G", "2G"), if known.
    #[uniffi(default = None)]
    pub cell_type: Option<String>,
    /// Wi-Fi signal bars (0-4), if known.
    #[uniffi(default = None)]
    pub wifi_bars: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum Link {
    /// `last_seen` is in Unix seconds.
    Offline {
        last_seen: Option<i64>,
    },
    Connecting,
    Online {
        relayed: bool,
        #[uniffi(default = false)]
        usb: bool,
        rtt_ms: u32,
        /// A direct connection over Tailscale ("Tailscale", a name in every
        /// language). Allowed and reached exactly as Wi-Fi is.
        #[uniffi(default = false)]
        tailnet: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PairedDevice {
    /// The device ID (z-base-32).
    pub id: String,
    pub info: DeviceInfo,
    /// Unix seconds.
    pub paired_at: i64,
    pub link: Link,
    /// Whether this phone has stored Wake-on-LAN addresses for this PC.
    pub can_wake: bool,
    /// Whether this device reconnects to it by itself (`set_reconnect`).
    pub reconnect: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DiscoveredDevice {
    pub id: String,
    pub name: Option<String>,
}

/// A LocalSend device discovered on the local network (`docs/protocol/v1.md` §15).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LocalSendPeer {
    pub id: String,
    pub alias: String,
    pub device_model: Option<String>,
    pub device_type: String,
    pub fingerprint: String,
    pub ip: String,
    pub port: u16,
    pub protocol: String,
}

/// What a PC answered when asked for a screenshot or a recording of its screen
/// (`docs/protocol/pc-capture.md`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PcCaptureOutcome {
    /// The file is on its way: transfer `transfer` brings it.
    File { transfer: String, name: String },
    /// The recording started.
    Started,
    /// The PC doesn't let this phone see its screen (its `pc_mirror` toggle).
    Off,
    /// The PC is locked.
    Locked,
    /// Someone at the PC dismissed its screen-sharing dialog.
    Declined,
    /// The PC doesn't send files to this phone (its `files` toggle).
    FilesOff,
    /// The PC is already recording, or taking another screenshot.
    Busy,
    /// The PC can't capture its screen.
    Unsupported,
    /// There was no recording to stop.
    NotRecording,
    /// The PC isn't connected.
    Offline,
    /// Anything else; `reason` is for logs.
    Failed { reason: String },
}

impl From<core::PcCaptureError> for PcCaptureOutcome {
    fn from(err: core::PcCaptureError) -> Self {
        match err {
            core::PcCaptureError::Off => PcCaptureOutcome::Off,
            core::PcCaptureError::Locked => PcCaptureOutcome::Locked,
            core::PcCaptureError::Declined => PcCaptureOutcome::Declined,
            core::PcCaptureError::FilesOff => PcCaptureOutcome::FilesOff,
            core::PcCaptureError::Busy => PcCaptureOutcome::Busy,
            core::PcCaptureError::Unsupported => PcCaptureOutcome::Unsupported,
            core::PcCaptureError::NotRecording => PcCaptureOutcome::NotRecording,
            core::PcCaptureError::Offline => PcCaptureOutcome::Offline,
            core::PcCaptureError::Failed(reason) => PcCaptureOutcome::Failed { reason },
        }
    }
}

fn capture_file(answer: Result<core::PcCaptureResult, core::PcCaptureError>) -> PcCaptureOutcome {
    match answer {
        Ok(file) => PcCaptureOutcome::File { transfer: file.transfer, name: file.name },
        Err(err) => err.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PairingFailure {
    Rejected,
    Declined,
    Expired,
    Unreachable,
    Other { message: String },
}

/// What unlocks a feature (see docs/architecture/capabilities.md).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Upgrade {
    /// "raisePower", "grantPermission", "enableAddon", "enablePath",
    /// "enableToggle" or "updateApp".
    pub action: String,
    /// E.g. "elevated", "notification_access", "clipboard", "phone".
    pub target: String,
    /// Roughly how long it takes; 0 for instant.
    pub minutes: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FeatureStatus {
    Available,
    Partial {
        limit: String,
        upgrade: Option<Upgrade>,
    },
    Locked {
        upgrade: Upgrade,
    },
    /// `reason`: "deviceKinds", "android:<release>" or "notOnThisDevice".
    Unsupported {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Feature {
    pub id: String,
    pub status: FeatureStatus,
}

/// A button on a notification.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NotificationAction {
    pub id: String,
    pub title: String,
    /// Takes text (an inline reply).
    pub reply: bool,
}

/// One segment in an Android 16 `Notification.ProgressStyle` progress bar.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LiveSegment {
    pub length: u32,
    #[uniffi(default = None)]
    pub color: Option<u32>,
}

/// One milestone point on an Android 16 `Notification.ProgressStyle` progress bar.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LivePoint {
    pub position: u32,
    #[uniffi(default = None)]
    pub color: Option<u32>,
}

/// Live Update and ongoing progress/timer metadata attached to a [`Notification`].
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct NotificationLive {
    #[uniffi(default = 1)]
    pub v: u32,
    #[uniffi(default = None)]
    pub progress: Option<u32>,
    #[uniffi(default = None)]
    pub max: Option<u32>,
    #[uniffi(default = false)]
    pub indeterminate: bool,
    #[uniffi(default = None)]
    pub chip: Option<String>,
    #[uniffi(default = [])]
    pub segments: Vec<LiveSegment>,
    #[uniffi(default = [])]
    pub points: Vec<LivePoint>,
    #[uniffi(default = false)]
    pub chronometer: bool,
    #[uniffi(default = false)]
    pub countdown: bool,
}

impl std::fmt::Debug for NotificationLive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotificationLive")
            .field("v", &self.v)
            .field("progress", &self.progress)
            .field("max", &self.max)
            .field("indeterminate", &self.indeterminate)
            .field("chronometer", &self.chronometer)
            .field("countdown", &self.countdown)
            .finish_non_exhaustive()
    }
}

/// Ongoing or completed task notification sent by a paired PC (`task.notify`).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct TaskNotify {
    pub id: String,
    pub title: String,
    pub active: bool,
    pub elapsed_ms: u64,
    #[uniffi(default = None)]
    pub exit_code: Option<i32>,
}

impl std::fmt::Debug for TaskNotify {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskNotify")
            .field("id", &self.id)
            .field("active", &self.active)
            .field("elapsed_ms", &self.elapsed_ms)
            .field("exit_code", &self.exit_code)
            .finish_non_exhaustive()
    }
}

/// One chat message inside a `MessagingStyle` notification conversation.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct NotificationChatMessage {
    /// Sender display name (`None` for the user's own messages or 1:1 chats
    /// where the conversation title is the sender).
    #[uniffi(default = None)]
    pub sender: Option<String>,
    pub text: String,
    /// Unix milliseconds.
    pub time: i64,
    /// True when sent by the phone user.
    #[uniffi(default = false)]
    pub self_sent: bool,
    /// Optional sender avatar (JPEG/PNG, at most 16 KiB).
    #[uniffi(default = None)]
    pub avatar: Option<Vec<u8>>,
}

/// Never prints message text or sender names (protocol v1 §11).
impl std::fmt::Debug for NotificationChatMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotificationChatMessage")
            .field("time", &self.time)
            .field("self_sent", &self.self_sent)
            .finish_non_exhaustive()
    }
}

/// Structured `MessagingStyle` conversation metadata carried on a notification.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct NotificationConversation {
    /// Schema version (`1`).
    #[uniffi(default = 1)]
    pub v: u32,
    /// Conversation or sender display title.
    pub title: String,
    /// True for group conversations (`MessagingStyle.isGroupConversation`).
    #[uniffi(default = false)]
    pub group: bool,
    /// Optional conversation/sender avatar (JPEG/PNG, at most 16 KiB).
    #[uniffi(default = None)]
    pub avatar: Option<Vec<u8>>,
    /// Chronological messages in the notification (oldest first).
    pub messages: Vec<NotificationChatMessage>,
    /// What names the conversation on the wire, when the app gave it an ID:
    /// a hash of that ID, never the ID itself (1 to 64 bytes of
    /// `[A-Za-z0-9_-]`; the core drops anything else).
    #[uniffi(default = None)]
    pub id: Option<String>,
}

/// Never prints conversation title, ID or messages (protocol v1 §11).
impl std::fmt::Debug for NotificationConversation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotificationConversation")
            .field("v", &self.v)
            .field("has_id", &self.id.is_some())
            .field("group", &self.group)
            .field("messages_len", &self.messages.len())
            .finish_non_exhaustive()
    }
}

/// A notification, as mirrored between devices
/// (docs/protocol/notifications.md).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct Notification {
    /// This device's ID for it (Android: the StatusBarNotification key).
    pub key: String,
    /// Package name.
    pub app: String,
    pub app_name: String,
    pub title: Option<String>,
    pub text: Option<String>,
    pub sub: Option<String>,
    /// Unix milliseconds.
    pub when: i64,
    pub actions: Vec<NotificationAction>,
    /// Arrived without sound or pop-up.
    pub silent: bool,
    /// The app icon as PNG; pass it every time, the core sends it once per
    /// device and connection.
    pub icon: Option<Vec<u8>>,
    /// A picture it shows (a photo in a message, a big picture): JPEG, at
    /// most 160 KiB.
    pub image: Option<Vec<u8>>,
    /// Live Update / ongoing progress or timer metadata, when present.
    #[uniffi(default = None)]
    pub live: Option<NotificationLive>,
    /// Structured `MessagingStyle` conversation metadata, when present.
    #[uniffi(default = None)]
    pub conversation: Option<NotificationConversation>,
    /// Can't be dismissed (ongoing or no-clear), so a PC's Clear all leaves
    /// it. Last, with a default, so positional callers keep working.
    #[uniffi(default = false)]
    pub ongoing: bool,
}

/// Something playing (or paused) on a device (docs/protocol/media.md).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct MediaPlayer {
    /// Stable while the player exists (Android: the app's package name).
    pub id: String,
    /// User-visible app name.
    pub app: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub playing: bool,
    /// Milliseconds.
    pub duration: Option<u64>,
    /// Milliseconds, now; it moves on in real time while `playing`.
    pub position: Option<u64>,
    pub actions: Vec<MediaAction>,
    /// Identifies the artwork: the same key, the same picture.
    pub art_key: Option<String>,
    /// JPEG or PNG. Pass it every time; the core sends it once per device
    /// and connection. Arrives the same way: keep it by `art_key`.
    pub art: Option<Vec<u8>>,
    /// Playback rate in thousandths (250 to 4000); null means 1000. The
    /// fields from here on are last, with defaults, so positional callers
    /// keep working (0.1.14 contract §A6.3).
    #[uniffi(default = None)]
    pub rate_milli: Option<u32>,
    /// Milliseconds between sampling `position` and sending this state: the
    /// position now is `position + (age_ms + time since it arrived) × rate`
    /// while `playing`, capped at `duration`.
    #[uniffi(default = None)]
    pub age_ms: Option<u64>,
    /// Key of a larger picture, fetched with `media_art`.
    #[uniffi(default = None)]
    pub art_full: Option<String>,
    /// Key of the app's icon, fetched with `media_icon`.
    #[uniffi(default = None)]
    pub icon: Option<String>,
    #[uniffi(default = None)]
    pub shuffle: Option<bool>,
    #[uniffi(default = None)]
    pub repeat: Option<MediaRepeat>,
}

/// A player's repeat mode (the wire's `loop`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MediaRepeat {
    None,
    Track,
    Playlist,
}

impl MediaRepeat {
    fn wire(self) -> &'static str {
        use core::media_loop;
        match self {
            MediaRepeat::None => media_loop::NONE,
            MediaRepeat::Track => media_loop::TRACK,
            MediaRepeat::Playlist => media_loop::PLAYLIST,
        }
    }

    fn from_wire(repeat: &str) -> Option<Self> {
        use core::media_loop;
        match repeat {
            media_loop::NONE => Some(MediaRepeat::None),
            media_loop::TRACK => Some(MediaRepeat::Track),
            media_loop::PLAYLIST => Some(MediaRepeat::Playlist),
            _ => None,
        }
    }
}

/// Never prints what's playing (protocol v1 §11).
impl std::fmt::Debug for MediaPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaPlayer").field("id", &self.id).finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MediaAction {
    Play,
    Pause,
    Next,
    Previous,
    Seek,
    /// With `media_command_value`: `on` or `off`.
    Shuffle,
    /// With `media_command_value`: `none`, `track` or `playlist` (the wire's `loop`).
    Repeat,
}

impl From<MediaAction> for core::MediaAction {
    fn from(a: MediaAction) -> Self {
        match a {
            MediaAction::Play => core::MediaAction::Play,
            MediaAction::Pause => core::MediaAction::Pause,
            MediaAction::Next => core::MediaAction::Next,
            MediaAction::Previous => core::MediaAction::Previous,
            MediaAction::Seek => core::MediaAction::Seek,
            MediaAction::Shuffle => core::MediaAction::Shuffle,
            MediaAction::Repeat => core::MediaAction::Loop,
        }
    }
}

impl From<core::MediaAction> for MediaAction {
    fn from(a: core::MediaAction) -> Self {
        match a {
            core::MediaAction::Play => MediaAction::Play,
            core::MediaAction::Pause => MediaAction::Pause,
            core::MediaAction::Next => MediaAction::Next,
            core::MediaAction::Previous => MediaAction::Previous,
            core::MediaAction::Seek => MediaAction::Seek,
            core::MediaAction::Shuffle => MediaAction::Shuffle,
            core::MediaAction::Loop => MediaAction::Repeat,
        }
    }
}

impl From<MediaPlayer> for core::MediaPlayer {
    fn from(p: MediaPlayer) -> Self {
        core::MediaPlayer {
            id: p.id,
            app: p.app,
            title: p.title,
            artist: p.artist,
            album: p.album,
            playing: p.playing,
            duration: p.duration,
            position: p.position,
            actions: p.actions.into_iter().map(|a| core::MediaAction::from(a).as_str().to_owned()).collect(),
            art_key: p.art_key,
            art: p.art,
            rate_milli: p.rate_milli,
            age: p.age_ms,
            art_full: p.art_full,
            icon: p.icon,
            shuffle: p.shuffle,
            repeat: p.repeat.map(|r| r.wire().to_owned()),
        }
    }
}

impl From<core::MediaPlayer> for MediaPlayer {
    fn from(p: core::MediaPlayer) -> Self {
        MediaPlayer {
            id: p.id,
            app: p.app,
            title: p.title,
            artist: p.artist,
            album: p.album,
            playing: p.playing,
            duration: p.duration,
            position: p.position,
            actions: p.actions.iter().filter_map(|a| core::MediaAction::parse(a)).map(Into::into).collect(),
            art_key: p.art_key,
            art: p.art,
            rate_milli: p.rate_milli,
            age_ms: p.age,
            art_full: p.art_full,
            icon: p.icon,
            shuffle: p.shuffle,
            repeat: p.repeat.as_deref().and_then(MediaRepeat::from_wire),
        }
    }
}

/// Never prints content (protocol v1 §11).
impl std::fmt::Debug for Notification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notification").field("app", &self.app).finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TransferDirection {
    Outgoing,
    Incoming,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum TransferStatus {
    /// Incoming transfer waiting for the user to accept or decline (LocalSend).
    Requested,
    /// Waiting for the other device (to connect, or to continue).
    Waiting,
    Running,
    /// Manually paused by either side; partial files are kept on the receiver.
    Paused,
    /// `saved`: where received files are, one path per item in `names`:
    /// each file, and each sent folder (empty when
    /// sending).
    Done {
        saved: Vec<String>,
    },
    /// `reason`: "denied", "unreachable", "noSpace", "interrupted", "integrity"
    /// or "other".
    Failed {
        reason: String,
    },
    Cancelled,
}

/// A timestamped marker within a voice recording (docs/protocol/recorder.md).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RecordingMarker {
    pub at_ms: u64,
    pub label: Option<String>,
}

/// A file transfer (docs/protocol/files.md).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct Transfer {
    pub id: String,
    /// The other device.
    pub device_id: String,
    pub direction: TransferDirection,
    /// What was sent: file and folder names, in order.
    pub names: Vec<String>,
    /// How many files that is (folders' files included).
    pub files: u32,
    pub total: u64,
    pub done: u64,
    pub status: TransferStatus,
    /// True when this transfer is a voice recording from the phone's recorder.
    pub recording: bool,
    /// Markers captured during the recording.
    pub markers: Vec<RecordingMarker>,
    /// True when the sender handed off a single document that should open with
    /// the default app once it arrives (`docs/protocol/files.md`).
    pub open_on_arrival: bool,
}

/// Never prints file names (protocol v1 §11).
impl std::fmt::Debug for Transfer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transfer").field("id", &self.id).field("status", &self.status).finish_non_exhaustive()
    }
}

/// A file to send.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FileToSend {
    /// An open, readable descriptor (Android: from a content URI). The core
    /// takes ownership and closes it.
    Fd {
        name: String,
        fd: i32,
        /// The folder it's in, when sending a folder: `/`-separated names,
        /// starting with the folder's own (`Trip/Day 1`).
        folder: Option<String>,
    },
    Path {
        name: String,
        path: String,
        folder: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeviceToggle {
    pub name: String,
    pub enabled: bool,
}

/// The key a paired PC's Bluetooth beacon is recognised by.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct BleBeaconKey {
    /// The PC's device id.
    pub id: String,
    pub key: Vec<u8>,
}

impl std::fmt::Debug for BleBeaconKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BleBeaconKey").field("id", &self.id).finish_non_exhaustive()
    }
}

/// Everything the UI needs to know about, as it happens.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum Event {
    DeviceAdded {
        device: PairedDevice,
    },
    DeviceRemoved {
        id: String,
    },
    LinkChanged {
        id: String,
        link: Link,
    },
    PeerInfoChanged {
        id: String,
        info: DeviceInfo,
    },
    PeerPowerChanged {
        id: String,
        power: PowerLevel,
    },
    Battery {
        id: String,
        battery: Battery,
    },
    /// A device heard of a new G7 Link build on `channel` ("stable" or
    /// "nightly"): check that channel's feed now.
    UpdateHint {
        id: String,
        channel: String,
        version: String,
    },
    /// Paired device `id` tried to connect with an older G7 Link: it needs
    /// an update before the two can talk. At most once a minute per device.
    PeerNeedsUpdate {
        id: String,
    },
    /// A device asked this one to start (or stop) ringing.
    Ring {
        id: String,
        on: bool,
    },
    RemoteRing {
        id: String,
        on: bool,
    },
    /// What has the focus on PC `id` takes text, or no longer does.
    PcTextFocus {
        id: String,
        editable: bool,
    },
    /// The field that has the focus on PC `id`, while the typing box watches it.
    PcTextState {
        id: String,
        state: TextFieldState,
    },
    /// PC `id`'s clipboard got something new: offer to paste it. `preview` is
    /// the text's first line, when that PC sends this phone what it copies.
    PcClip {
        id: String,
        kind: PcClipKind,
        preview: Option<String>,
    },
    Discovered {
        device: DiscoveredDevice,
    },
    DiscoveryExpired {
        id: String,
    },
    /// Show this 6-digit code and ask the user whether it matches.
    PairingCode {
        peer: String,
        code: String,
    },
    Paired {
        device: PairedDevice,
    },
    PairingFailed {
        failure: PairingFailure,
    },
    Capabilities {
        id: String,
        features: Vec<Feature>,
    },
    NotificationsReset {
        id: String,
        items: Vec<Notification>,
    },
    NotificationPosted {
        id: String,
        notification: Notification,
    },
    NotificationRemoved {
        id: String,
        key: String,
    },
    /// A device put text on this phone's clipboard.
    ClipboardReceived {
        id: String,
    },
    /// The local clipboard history changed.
    ClipboardHistoryChanged,
    /// A file transfer started, progressed or finished.
    Transfer {
        transfer: Transfer,
    },
    /// A device's media players (all of them; empty when nothing plays
    /// there, or media is off for it).
    MediaChanged {
        id: String,
        players: Vec<MediaPlayer>,
    },
    /// A paired phone took a photo (PCs only; phones announce theirs).
    PhotoAdded {
        id: String,
        photo_id: String,
    },
    /// A call on a paired phone changed (PCs only; phones report theirs).
    CallChanged {
        id: String,
        call_id: String,
    },
    /// A paired phone's call history changed (PCs only).
    CallLogChanged {
        id: String,
    },
    /// A paired phone's contacts changed (PCs only).
    ContactsChanged {
        id: String,
    },
    /// A paired phone's screen (session 0) or app window started or
    /// stopped showing (PCs only).
    Mirroring {
        id: String,
        session: u32,
        on: bool,
    },
    /// A paired phone's messages changed (PCs only).
    SmsChanged {
        id: String,
        thread: Option<String>,
    },
    /// A paired phone's photo library changed (PCs only).
    PhotosChanged {
        id: String,
    },
    /// A paired phone asked to control this PC while `remote_input` is off
    /// (PCs only).
    RemoteInputRequested {
        id: String,
    },
    /// A paired phone's quick settings state (PCs only).
    PhoneToggles {
        id: String,
        toggles: PhoneToggles,
    },
    /// A paired PC's stored Wake-on-LAN addresses changed (phones only).
    WakeInfoChanged {
        id: String,
        can_wake: bool,
    },
    /// Reconnecting to a paired device by itself was turned off or back on
    /// (`set_reconnect`).
    ReconnectChanged {
        id: String,
        on: bool,
    },
    /// Device `id` changed a toggle for this one that both keep the same
    /// (`bt_media`); changes made here aren't reported.
    DeviceToggleChanged {
        id: String,
        toggle: String,
        on: bool,
    },
    /// A paired PC's Deck layout arrived or changed (phones only).
    DeckLayout {
        id: String,
        layout: DeckLayout,
    },
    /// A paired PC's live Deck state arrived or changed (phones only).
    DeckState {
        id: String,
        state: DeckState,
    },
    /// A paired phone's camera started or stopped streaming as a webcam
    /// (PCs only).
    Webcam {
        id: String,
        on: bool,
    },
    /// A paired PC asked to browse this phone's storage while `storage` is off
    /// (phones only).
    StorageRequested {
        id: String,
    },
    /// A folder on a paired phone changed (PCs only).
    StorageChanged {
        id: String,
        path: String,
    },
    /// The local cross-device timeline changed.
    TimelineChanged,
    /// The LocalSend compatibility setting or discovered LocalSend peers changed.
    LocalSendChanged,
    /// A piece of PC sound arrived from a paired PC (`docs/protocol/v1.md`
    /// §5). `seq` counts up by one per piece and wraps: a player drops one
    /// that isn't after the last it played. `ts_ms` is when its first sample
    /// was captured, in ms since the PC's sound began.
    PcAudioFrame {
        device: String,
        sample_rate: u32,
        channels: u8,
        seq: u16,
        ts_ms: u32,
        pcm: Vec<u8>,
    },
    /// A paired PC started streaming its screen to this phone (`mirror` video stream config).
    PcMirrorConfig {
        id: String,
        codec: String,
        width: u32,
        height: u32,
    },
    /// An encoded video frame arrived from a paired PC's screen stream.
    /// `seq` counts packets from 0 after each `PcMirrorConfig`, one up per
    /// packet: a gap means one was lost on the way to the decoder. `time_us`
    /// is the PC's monotonic clock when the picture reached its app (see
    /// `pc_mirror_clock`).
    PcMirrorFrame {
        id: String,
        seq: u64,
        keyframe: bool,
        time_us: u64,
        data: Vec<u8>,
    },
    /// A paired PC's screen stream ended.
    PcMirrorEnded {
        id: String,
    },
    /// A paired PC says something about sharing its screen with this phone
    /// (`mirror.notice`).
    PcMirrorNotice {
        id: String,
        notice: PcMirrorNoticeKind,
    },
    /// A PC this phone asked for a screenshot or a recording shows its
    /// screen-sharing dialog: someone there has to answer it (phones only).
    PcCapturePrompt {
        id: String,
    },
    /// PC `id` asks its user about the folder this phone asked to add at
    /// `place`: show "Approve on <PC>" while `agent_folder_add` waits.
    /// Ignore one for a place it no longer waits on (phones only).
    AgentFolderPrompt {
        id: String,
        place: String,
    },
    /// A recording of a PC's screen this phone started ended on the PC by
    /// itself. `transfer` brings the video named `name`, when it was saved
    /// (phones only).
    PcCaptureRecordEnded {
        id: String,
        transfer: Option<String>,
        name: Option<String>,
    },
    /// The persistent outgoing send queue changed.
    QueueChanged,
    /// Pending outgoing queue items were dropped because the peer was unpaired.
    QueueDroppedOnUnpair {
        device_id: String,
        device_name: String,
        dropped_count: u32,
    },
    /// The Bluetooth beacon keys shared with paired PCs changed: read them
    /// again with `Node::ble_beacon_keys`.
    BleKeysChanged,
    /// The Bluetooth link end `endpoint` has values to write.
    BleOutbound {
        endpoint: u64,
    },
    /// A Bluetooth link with a PC came up or went.
    BleLinkChanged {
        id: String,
        up: bool,
    },
    /// The path a PC's touchpad takes, or a round-trip time shown for it
    /// (by 5 ms or more), changed. Send `remote_text_watch(on)` again when
    /// `active` changes while the typing box is open.
    RemoteRouteChanged {
        id: String,
        state: RouteState,
    },
    /// What a PC lets this phone start (`agent.catalog`). `None`: nothing
    /// can run there now, or access is off.
    AgentCatalog {
        id: String,
        catalog: Option<AgentCatalog>,
    },
    /// A thread on a PC changed: its state, asks, title or summary.
    AgentThreadChanged {
        id: String,
        thread: AgentThread,
    },
    /// A thread on a PC was deleted.
    AgentThreadRemoved {
        id: String,
        thread: String,
    },
    /// Assistant text streaming into item `item_id` (`seq`) of the watched
    /// thread, at byte `offset` of its text. A gap (`offset` past what you
    /// have) means read the thread again.
    AgentDelta {
        id: String,
        thread: String,
        item_id: String,
        seq: u64,
        offset: u32,
        text: String,
    },
    /// An item of the watched thread was added or updated (it replaces the
    /// streamed text).
    AgentItemChanged {
        id: String,
        thread: String,
        item: AgentItem,
    },
    /// A turn of a thread on a PC ended.
    AgentDone {
        id: String,
        thread: AgentThread,
        outcome: AgentOutcome,
        summary: String,
    },
    /// An agent on a PC asks for an answer.
    AgentAsk {
        id: String,
        ask: AgentAsk,
    },
    /// An ask ended: answered on some device, expired, or its turn ended.
    AgentAsked {
        id: String,
        token: String,
        thread: String,
        outcome: AgentAskOutcome,
        /// The device that answered ("Allowed on guchu"), for `Allowed` and
        /// `Denied`. Added after contract §6.3.
        #[uniffi(default = None)]
        by_name: Option<String>,
        /// `Denied`: what the user said when denying it, if they did.
        #[uniffi(default = None)]
        message: Option<String>,
    },
    /// A session started on a PC outside G7 Link appeared or changed.
    AgentSessionChanged {
        id: String,
        session: AgentSession,
    },
    /// A session left a PC's list: it left the 7 days, its file went, or it
    /// became a thread.
    AgentSessionRemoved {
        id: String,
        session: String,
    },
    /// An item of the session this phone watches was added or updated.
    AgentSessionItem {
        id: String,
        session: String,
        item: AgentItem,
    },
    /// An agent on a PC asks the user (`agent.control`). A secret question
    /// has no asks: "Answer it on <PC>."
    AgentQuestion {
        id: String,
        question: AgentQuestion,
    },
    /// A question ended: answered on some device, dismissed, or it lapsed.
    AgentQuestioned {
        id: String,
        token: String,
        thread: String,
        outcome: AgentQuestionOutcome,
        /// The device that answered ("Answered on %1").
        by_name: Option<String>,
    },
    /// A draft changed on another device: while its composer isn't focused,
    /// read it (`agent_draft_get`) and take its text; while it is, offer
    /// "Use the draft from %1".
    AgentDraftChanged {
        id: String,
        key: String,
        rev: u64,
    },
    /// A turn of a thread started, ended, or its restore point settled:
    /// replace that turn (`agent_turns` has the rest).
    AgentTurn {
        id: String,
        thread: String,
        turn: AgentTurn,
    },
    /// Messages `from_seq` to `to_seq` left a thread (a rewind): drop them;
    /// `restored` (its undo): read the thread again.
    AgentCut {
        id: String,
        thread: String,
        from_seq: u64,
        to_seq: u64,
        restored: bool,
    },
    /// How an import of sessions found on a PC goes.
    AgentImport {
        id: String,
        progress: AgentImportProgress,
    },
    /// This phone's held messages for a PC changed: all of them, in the
    /// order written.
    AgentOutbox {
        id: String,
        held: Vec<AgentHeld>,
    },
    /// What a PC says about itself, sent on every change and at least every
    /// 20 s while it's online. Nothing says when it lapses: it holds until
    /// `presence.until_ms`, and `agent_presence` is null after that or while
    /// the link is down.
    AgentPresence {
        id: String,
        presence: AgentPresence,
    },
    /// A thread's folder on a PC changed: its strip. Read `agent_git_status`
    /// again for the rest while its panel shows; a review on screen starts
    /// again ("The files changed. Read them again.").
    AgentGit {
        id: String,
        thread: String,
        branch: Option<String>,
        detached: bool,
        /// Files with changes ("%n changed").
        changed: u32,
        /// "↑2", "↓1" (as of the last fetch).
        ahead: Option<u32>,
        behind: Option<u32>,
        pr: Option<AgentPrBrief>,
    },
    /// A source-control job's new lines and where it stands: append
    /// `job.lines` (they follow on from what came before; `job.from` is the
    /// first one's number). The last has its final `phase`.
    AgentGitJob {
        id: String,
        job: AgentJob,
    },
    /// A PC hands this phone `thread`: open it at item `seq` (load its draft
    /// with `agent_draft_get`) when the app is in front, else post "Continue
    /// on this phone". `offer`: the PC locked with it on screen; post it on
    /// the quiet channel, and take it away when the PC's presence says
    /// someone is at it again. It holds until `expires_ms` (this phone's
    /// clock, Unix ms); nothing says when it lapses.
    AgentHandoff {
        id: String,
        thread: String,
        seq: Option<u64>,
        offer: bool,
        expires_ms: i64,
    },
    /// PC `id` shows `thread` and asks this phone to show `pane` beside it
    /// (its menu "%1 shows %2"). Act on it only while this phone shows that
    /// thread beside the PC (`AgentBesideScreen`), and report the new pane
    /// with `agent_viewing`; otherwise ignore it. `Thread` means the thread
    /// screen itself.
    AgentBeside {
        id: String,
        thread: String,
        pane: AgentPane,
    },
    /// A run of PC `id`'s project script: append `run.lines` (they follow
    /// on from what came before; `run.from` is the first one's number; a gap
    /// means some weren't sent, read `agent_script_output` again). The last
    /// has its final `phase`. Show them as plain text.
    AgentScriptRun {
        id: String,
        run: AgentScriptRun,
    },
}

/// Play on phone: a video file a PC offers for the phone to play itself
/// (`docs/protocol/media.md` §4), read with `watch_file_read`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WatchFileOffer {
    /// Its file name, for the player to tell the format by.
    pub name: String,
    pub size: u64,
    /// Where the PC had got to, milliseconds.
    pub position_ms: u64,
}

/// Bytes of an offered file (`watch_file_read`), and the file's size.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WatchFileChunk {
    pub size: u64,
    pub data: Vec<u8>,
}

/// One file or folder returned by `list_remote_storage` (`storage.list`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RemoteStorageEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: u64,
    pub modified: i64,
}

/// One tile on a PC's Deck (`docs/protocol/deck.md`).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeckTile {
    pub id: String,
    pub label: String,
    pub icon: String,
    pub color: String,
    pub kind: String,
}

impl std::fmt::Debug for DeckTile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeckTile")
            .field("id", &self.id)
            .field("icon", &self.icon)
            .field("color", &self.color)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl From<core::DeckTile> for DeckTile {
    fn from(t: core::DeckTile) -> Self {
        DeckTile { id: t.id, label: t.label, icon: t.icon, color: t.color, kind: t.kind }
    }
}

/// One page of tiles on a PC's Deck (`docs/protocol/deck.md`).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeckPage {
    pub id: String,
    pub name: String,
    pub tiles: Vec<DeckTile>,
}

impl std::fmt::Debug for DeckPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeckPage")
            .field("id", &self.id)
            .field("tiles", &self.tiles.len())
            .finish_non_exhaustive()
    }
}

impl From<core::DeckPage> for DeckPage {
    fn from(p: core::DeckPage) -> Self {
        DeckPage { id: p.id, name: p.name, tiles: p.tiles.into_iter().map(Into::into).collect() }
    }
}

/// A PC's Deck layout (`docs/protocol/deck.md`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeckLayout {
    pub pages: Vec<DeckPage>,
}

impl From<core::DeckLayout> for DeckLayout {
    fn from(l: core::DeckLayout) -> Self {
        DeckLayout { pages: l.pages.into_iter().map(Into::into).collect() }
    }
}

/// An active audio output device on a paired PC (`docs/protocol/deck.md`).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AudioOutputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

impl std::fmt::Debug for AudioOutputDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioOutputDevice").field("is_default", &self.is_default).finish_non_exhaustive()
    }
}

impl From<core::AudioOutputDevice> for AudioOutputDevice {
    fn from(d: core::AudioOutputDevice) -> Self {
        AudioOutputDevice { id: d.id, name: d.name, is_default: d.is_default }
    }
}

/// Live PC state reflected on Deck tiles (`docs/protocol/deck.md`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeckState {
    pub playing: bool,
    pub volume: u8,
    pub muted: bool,
    pub mic_muted: Option<bool>,
    pub output_devices: Vec<AudioOutputDevice>,
}

impl From<core::DeckState> for DeckState {
    fn from(s: core::DeckState) -> Self {
        DeckState {
            playing: s.playing,
            volume: s.volume,
            muted: s.muted,
            mic_muted: s.mic_muted,
            output_devices: s.output_devices.into_iter().map(Into::into).collect(),
        }
    }
}

// ---- Agents (docs/protocol/agents.md) ----

/// A thread's state on a PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentThreadState {
    /// A turn is running.
    Working,
    /// A turn is running and waits for an answer.
    Waiting,
    /// The last turn finished; the thread takes a follow-up.
    Done,
    Failed,
    Interrupted,
}

impl AgentThreadState {
    fn from_wire(state: &str) -> Option<Self> {
        use core::agent::state;
        Some(match state {
            state::WORKING => Self::Working,
            state::WAITING => Self::Waiting,
            state::DONE => Self::Done,
            state::FAILED => Self::Failed,
            state::INTERRUPTED => Self::Interrupted,
            _ => return None,
        })
    }
}

/// How a thread asks before acting. A phone starts and sets `Supervised`
/// or, with a PC that offers it (the catalog's `modes`), `Plan`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentMode {
    /// Asks before every edit and every command that isn't read-only.
    Supervised,
    /// Edits without asking.
    AutoEdit,
    /// Changes nothing: it writes a plan to implement ("Plan: it changes
    /// nothing until you say so"). Added with `agent.control`.
    Plan,
}

impl AgentMode {
    fn from_wire(mode: &str) -> Option<Self> {
        match mode {
            core::agent::mode::SUPERVISED => Some(Self::Supervised),
            core::agent::mode::AUTO_EDIT => Some(Self::AutoEdit),
            core::agent::mode::PLAN => Some(Self::Plan),
            _ => None,
        }
    }

    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::Supervised => core::agent::mode::SUPERVISED,
            Self::AutoEdit => core::agent::mode::AUTO_EDIT,
            Self::Plan => core::agent::mode::PLAN,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentItemKind {
    User,
    Assistant,
    Tool,
    Notice,
    /// The agent thinking: its `text` is what it showed of its thought,
    /// often empty; `end_ms` is set when it stopped. Added with `agent.items2`.
    Reasoning,
    /// A plan the agent wrote in plan mode: its `text` is the plan,
    /// `plan_state` where it stands. Added with `agent.control`.
    Plan,
    /// Questions the agent asked: its `text`, when any, lists the answers
    /// (never for a secret one). Added with `agent.control`.
    Question,
}

impl AgentItemKind {
    fn from_wire(kind: &str) -> Option<Self> {
        use core::agent::item;
        Some(match kind {
            item::USER => Self::User,
            item::ASSISTANT => Self::Assistant,
            item::TOOL => Self::Tool,
            item::NOTICE => Self::Notice,
            item::REASONING => Self::Reasoning,
            item::PLAN => Self::Plan,
            item::QUESTION => Self::Question,
            _ => return None,
        })
    }
}

/// An assistant item's place in its turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentItemPhase {
    /// Said on the way: more work followed.
    Interim,
    /// The turn's answer.
    Final,
}

impl AgentItemPhase {
    fn from_wire(phase: &str) -> Option<Self> {
        use core::agent::phase;
        match phase {
            phase::INTERIM => Some(Self::Interim),
            phase::FINAL => Some(Self::Final),
            _ => None,
        }
    }
}

/// What kind of step a tool item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentToolClass {
    Command,
    Edit,
    Read,
    Search,
    Web,
    Mcp,
    Agent,
    Tasks,
    Other,
    /// Writing a plan-mode plan to its file: no row (the `Plan` item shows
    /// it). It comes without its path or content. Added with `agent.control`.
    Plan,
}

impl AgentToolClass {
    fn from_wire(class: &str) -> Option<Self> {
        use core::agent::tool_class;
        Some(match class {
            tool_class::COMMAND => Self::Command,
            tool_class::EDIT => Self::Edit,
            tool_class::READ => Self::Read,
            tool_class::SEARCH => Self::Search,
            tool_class::WEB => Self::Web,
            tool_class::MCP => Self::Mcp,
            tool_class::AGENT => Self::Agent,
            tool_class::TASKS => Self::Tasks,
            tool_class::OTHER => Self::Other,
            tool_class::PLAN => Self::Plan,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentToolStatus {
    Running,
    Ok,
    Error,
    Denied,
}

impl AgentToolStatus {
    fn from_wire(status: &str) -> Option<Self> {
        use core::agent::tool;
        Some(match status {
            tool::RUNNING => Self::Running,
            tool::OK => Self::Ok,
            tool::ERROR => Self::Error,
            tool::DENIED => Self::Denied,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentAskKind {
    Command,
    Edit,
    Other,
}

impl AgentAskKind {
    fn from_wire(kind: &str) -> Option<Self> {
        use core::agent::ask;
        Some(match kind {
            ask::COMMAND => Self::Command,
            ask::EDIT => Self::Edit,
            ask::OTHER => Self::Other,
            _ => return None,
        })
    }
}

/// How a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentOutcome {
    Done,
    Failed,
    Interrupted,
}

impl AgentOutcome {
    fn from_wire(outcome: &str) -> Option<Self> {
        use core::agent::outcome;
        Some(match outcome {
            outcome::DONE => Self::Done,
            outcome::FAILED => Self::Failed,
            outcome::INTERRUPTED => Self::Interrupted,
            _ => return None,
        })
    }
}

/// How an ask ended: answered on some device, expired, or its turn ended first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentAskOutcome {
    Allowed,
    Denied,
    Expired,
    Cancelled,
}

impl AgentAskOutcome {
    fn from_wire(outcome: &str) -> Option<Self> {
        use core::agent::asked;
        Some(match outcome {
            asked::ALLOWED => Self::Allowed,
            asked::DENIED => Self::Denied,
            asked::EXPIRED => Self::Expired,
            asked::CANCELLED => Self::Cancelled,
            _ => return None,
        })
    }
}

/// What this build offers on top of what the app passes: it shows PCs'
/// screens, its agent records carry turns and thinking (`agent.items2`), and
/// it controls agents (`agent.control`: models, plan mode, the queue,
/// questions, images, files, commands, drafts).
fn with_built_in_capabilities(caps: &mut Vec<String>) {
    for cap in [core::MIRROR_VIEW, core::agent::ITEMS2, core::agent::CONTROL] {
        if !caps.iter().any(|c| c == cap) {
            caps.push(cap.into());
        }
    }
}

pub(crate) fn ms(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// An agent a PC can run. `login_hint` is the command that signs it in, to
/// run on the PC.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub signed_in: bool,
    pub login_hint: String,
    /// How it's signed in: `subscription`, `api_key` ("Using an API key")
    /// or `other`; null when the PC doesn't say. Added after contract §6.3.
    #[uniffi(default = None)]
    pub auth: Option<String>,
    /// The models its CLI reports on the PC; empty until it has said (offer
    /// "Default" and "Other…" then). Added with `agent.control`.
    #[uniffi(default = [])]
    pub models: Vec<AgentModel>,
    /// What it can do. Added with `agent.control`.
    #[uniffi(default = [])]
    pub can: Vec<AgentCan>,
    /// Its usage windows as its CLI last reported them. Added with `agent.control`.
    #[uniffi(default = [])]
    pub limits: Vec<AgentLimit>,
    /// It forks a thread from its latest message ("Fork from here" on the last turn).
    #[uniffi(default = false)]
    pub can_fork: bool,
    /// It forks, rewinds and edits from an earlier message too.
    #[uniffi(default = false)]
    pub can_fork_at: bool,
}

impl std::fmt::Debug for AgentInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentInfo")
            .field("id", &self.id)
            .field("installed", &self.installed)
            .field("signed_in", &self.signed_in)
            .finish_non_exhaustive()
    }
}

/// A folder an agent may work in on a PC: its label only, never its path.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentFolder {
    pub id: String,
    pub label: String,
    /// The PC keeps a restore point for each turn in it.
    #[uniffi(default = false)]
    pub checkpoints: bool,
    /// The PC fetches its remote every few minutes while one of its threads
    /// is shown ("Keep in step with the remote", set on the PC).
    #[uniffi(default = false)]
    pub follow_remote: bool,
    /// "Phones can run these", set on the PC: this phone may start and stop
    /// the folder's scripts. Off: "To run this folder's scripts here, turn
    /// on “Phones can run these” for the folder in G7 Link on %1."
    #[uniffi(default = false)]
    pub phone_scripts: bool,
    /// How many scripts the folder has, whether or not they're listed:
    /// offer Scripts in a thread's menu when it isn't 0.
    #[uniffi(default = 0)]
    pub script_count: u8,
    /// Its scripts, in order: only with `phone_scripts`.
    #[uniffi(default = [])]
    pub scripts: Vec<AgentScript>,
    /// When a thread there was last started, else when it was added (Unix
    /// ms); 0 from a PC that doesn't say. Recent folders come first.
    #[uniffi(default = 0)]
    pub used: u64,
    /// It holds a git repository of its own.
    #[uniffi(default = false)]
    pub repo: bool,
    /// What its newest thread ran with, its title and branch: the first
    /// quick pick for a new thread there, and the folder's card.
    #[uniffi(default = None)]
    pub last: Option<AgentFolderLast>,
}

/// A folder's newest thread: the agent, model and effort it ran with (null
/// model: the agent's default; null effort: the model's), its title, and
/// the branch it was on.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentFolderLast {
    pub agent: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub title: String,
    pub branch: Option<String>,
}

impl std::fmt::Debug for AgentFolderLast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentFolderLast")
            .field("agent", &self.agent)
            .field("model", &self.model)
            .field("effort", &self.effort)
            .finish_non_exhaustive()
    }
}

impl From<core::AgentFolderLast> for AgentFolderLast {
    fn from(l: core::AgentFolderLast) -> Self {
        AgentFolderLast { agent: l.agent, model: l.model, effort: l.effort, title: l.title, branch: l.branch }
    }
}

impl From<core::AgentCatalogFolder> for AgentFolder {
    fn from(f: core::AgentCatalogFolder) -> Self {
        AgentFolder {
            id: f.id,
            label: f.label,
            checkpoints: f.checkpoints,
            follow_remote: f.follow_remote,
            phone_scripts: f.phone_scripts,
            script_count: f.script_count,
            scripts: f.scripts.into_iter().map(Into::into).collect(),
            used: f.used,
            repo: f.repo,
            last: f.last.map(Into::into),
        }
    }
}

impl std::fmt::Debug for AgentFolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentFolder").field("id", &self.id).finish_non_exhaustive()
    }
}

/// What a PC lets this phone start (`agent.catalog`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentCatalog {
    pub agents: Vec<AgentInfo>,
    pub folders: Vec<AgentFolder>,
    /// The modes this phone may start: `[Supervised]`.
    pub modes: Vec<AgentMode>,
    /// The PC takes a denial with the user's words (`agent_answer`'s
    /// `message`): offer "deny and say why" only then. Comes again in
    /// `Event.AgentCatalog` when only this changes.
    #[uniffi(default = false)]
    pub can_say_why: bool,
    /// The PC takes what `agent.control` adds: models and effort, plan
    /// mode, steering and the queue, questions, images, `@` files, `/`
    /// commands and drafts. Without it, offer what 0.1.16 did.
    #[uniffi(default = false)]
    pub can_control: bool,
    /// This phone may push and open pull requests: the PC's "Push and open
    /// pull requests" switch for it. Off: those rows read "Turn on “Push and
    /// open pull requests” for this phone in G7 Link on %1."
    #[uniffi(default = false)]
    pub push: bool,
    /// This phone adds folders without the PC asking first (its "Add
    /// folders without asking" switch there). Off: say "<PC> asks you the
    /// first time" before the user picks a new folder.
    #[uniffi(default = false)]
    pub add_folders: bool,
}

impl AgentCatalog {
    fn from_core(c: core::AgentCatalog, can_say_why: bool, can_control: bool) -> Self {
        AgentCatalog { can_say_why, can_control, ..c.into() }
    }
}

impl From<core::AgentCatalog> for AgentCatalog {
    fn from(c: core::AgentCatalog) -> Self {
        AgentCatalog {
            agents: c
                .agents
                .into_iter()
                .map(|a| AgentInfo {
                    id: a.id,
                    name: a.name,
                    installed: a.installed,
                    signed_in: a.signed_in,
                    login_hint: a.login_hint,
                    auth: a.auth,
                    models: a.models.into_iter().map(Into::into).collect(),
                    can: a.can.iter().filter_map(|c| AgentCan::from_wire(c)).collect(),
                    can_fork: a.can.iter().any(|c| c == core::agent::can::FORK),
                    can_fork_at: a.can.iter().any(|c| c == core::agent::can::FORK_AT),
                    limits: a.limits.into_iter().map(Into::into).collect(),
                })
                .collect(),
            folders: c.folders.into_iter().map(Into::into).collect(),
            // A mode this build doesn't know can't be started from here.
            modes: c.modes.iter().filter_map(|m| AgentMode::from_wire(m)).collect(),
            can_say_why: false,
            can_control: false,
            push: c.push,
            add_folders: c.add_folders,
        }
    }
}

/// A thread on a PC. `folder` is the folder's label; `from_phone`: this
/// phone started it.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentThread {
    pub id: String,
    /// The agent's id (`claude`, `codex`): its name is in the catalog.
    pub agent: String,
    pub folder: String,
    pub title: String,
    pub state: AgentThreadState,
    pub mode: AgentMode,
    pub from_phone: bool,
    /// Unix milliseconds.
    pub updated_ms: i64,
    /// Requests waiting for an answer.
    pub asks: u32,
    pub summary: Option<String>,
    /// For a usage limit, ends in `|resets_ms=<Unix ms>` when the agent said when it resets.
    pub error: Option<String>,
    /// Why it failed: `usage_limit` (show "Paused"), `login` (signed out), or
    /// the agent's own code. Added after contract §6.3; defaults to null.
    #[uniffi(default = None)]
    pub error_code: Option<String>,
    /// What a running turn is doing ("Now: <step>"); null otherwise. 0.1.14.
    #[uniffi(default = None)]
    pub step: Option<String>,
    /// When `state` began, Unix milliseconds (for "Working for 4 min"). 0.1.14.
    #[uniffi(default = None)]
    pub since_ms: Option<i64>,
    /// The agent's own plan for the turn; null when it made none. 0.1.14.
    #[uniffi(default = None)]
    pub plan: Option<Vec<AgentPlanStep>>,
    /// The model chosen for its next turn; null: "Default". `agent.control`.
    #[uniffi(default = None)]
    pub model: Option<String>,
    /// The effort chosen for its next turn; null: the model's default.
    #[uniffi(default = None)]
    pub effort: Option<String>,
    /// The model that answered last, as the CLI said: show "Answered by %1"
    /// when it isn't `model`.
    #[uniffi(default = None)]
    pub answered_model: Option<String>,
    /// Messages waiting for the running turn to end, oldest first (at most 5).
    #[uniffi(default = [])]
    pub queued: Vec<AgentQueued>,
    /// The queue waits: "Waiting: the last turn didn't finish."
    #[uniffi(default = false)]
    pub queue_held: bool,
    /// How full its context is, once the CLI said.
    #[uniffi(default = None)]
    pub context: Option<AgentContext>,
    /// The PC user's MCP servers in it, counted.
    #[uniffi(default = None)]
    pub mcp: Option<AgentMcp>,
    /// Questions waiting for an answer (not counted in `asks`).
    #[uniffi(default = 0)]
    pub asking: u32,
    /// In the "Pinned" section.
    #[uniffi(default = false)]
    pub pinned: bool,
    /// On the "Archived" shelf.
    #[uniffi(default = false)]
    pub archived: bool,
    /// Its title in the primary weight.
    #[uniffi(default = false)]
    pub unread: bool,
    /// Out of the main list until then: "Snoozed until %1".
    #[uniffi(default = None)]
    pub snoozed_until_ms: Option<i64>,
    /// The newest turn's number; 0 when none.
    #[uniffi(default = 0)]
    pub turns: u32,
    /// "Fork of %1".
    #[uniffi(default = None)]
    pub forked_from: Option<AgentForkedFrom>,
    /// "Brought in".
    #[uniffi(default = false)]
    pub found: bool,
    /// It can be read but not continued: no composer.
    #[uniffi(default = None)]
    pub readonly: Option<AgentReadonly>,
    /// The last rewind can be undone until then.
    #[uniffi(default = None)]
    pub undo: Option<AgentUndo>,
    /// The branch its folder is on (the strip); null: not a repository, or
    /// not read yet.
    #[uniffi(default = None)]
    pub branch: Option<String>,
    /// It works in a worktree of its own: show the worktree glyph.
    #[uniffi(default = None)]
    pub worktree: Option<AgentGitWorktree>,
    /// Files with changes not committed ("%n changed").
    #[uniffi(default = None)]
    pub changed: Option<u32>,
    /// What its agent wrote when asked ("Ask %1 to write it").
    #[uniffi(default = None)]
    pub wrote: Option<AgentWrote>,
    /// Ports its agent listens on (dev servers), oldest first, at most 4:
    /// "A server is listening on port %1". A `lan` one opens here at
    /// `agent_port_url`.
    #[uniffi(default = [])]
    pub ports: Vec<AgentPort>,
    /// Its scripts' runs, running first, then ended ones the PC still keeps
    /// (at most 8): "%1 is running" under the status line while one runs.
    #[uniffi(default = [])]
    pub runs: Vec<AgentRunBrief>,
}

/// A plan step's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentPlanStatus {
    Pending,
    InProgress,
    Completed,
}

impl AgentPlanStatus {
    fn from_wire(status: &str) -> Option<Self> {
        use core::agent::plan_status;
        Some(match status {
            plan_status::PENDING => Self::Pending,
            plan_status::IN_PROGRESS => Self::InProgress,
            plan_status::COMPLETED => Self::Completed,
            _ => return None,
        })
    }
}

/// One step of a thread's plan, in the agent's own words.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentPlanStep {
    pub text: String,
    pub status: AgentPlanStatus,
}

impl std::fmt::Debug for AgentPlanStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPlanStep").field("status", &self.status).finish_non_exhaustive()
    }
}

impl std::fmt::Debug for AgentThread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentThread")
            .field("id", &self.id)
            .field("agent", &self.agent)
            .field("state", &self.state)
            .field("asks", &self.asks)
            .finish_non_exhaustive()
    }
}

impl AgentThread {
    fn from_core(t: core::AgentThread) -> Option<Self> {
        Some(AgentThread {
            state: AgentThreadState::from_wire(&t.state)?,
            mode: AgentMode::from_wire(&t.mode)?,
            id: t.id,
            agent: t.agent,
            folder: t.folder,
            title: t.title,
            from_phone: t.from_phone,
            updated_ms: ms(t.updated),
            asks: t.asks,
            summary: t.summary,
            error: t.error,
            error_code: t.error_code,
            step: t.step,
            since_ms: t.since.map(ms),
            plan: t.plan.map(|steps| {
                steps
                    .into_iter()
                    .filter_map(|s| {
                        Some(AgentPlanStep { status: AgentPlanStatus::from_wire(&s.status)?, text: s.text })
                    })
                    .collect()
            }),
            model: t.model,
            effort: t.effort,
            answered_model: t.answered_model,
            queued: t.queued.into_iter().map(Into::into).collect(),
            queue_held: t.queue_held,
            context: t.context.map(|c| AgentContext { used: c.used, max: c.max }),
            mcp: t.mcp.map(|m| AgentMcp {
                on: m.on,
                servers: m.servers,
                connected: m.connected,
                need_login: m.need_login,
            }),
            asking: t.asking,
            pinned: t.pinned,
            archived: t.archived,
            unread: t.unread,
            snoozed_until_ms: t.snoozed_until_ms.map(ms),
            turns: t.turns,
            forked_from: t.forked_from.map(Into::into),
            found: t.found,
            readonly: t.readonly.as_deref().and_then(AgentReadonly::from_wire),
            undo: t.undo.map(Into::into),
            branch: t.branch,
            worktree: t.worktree.map(Into::into),
            changed: t.changed,
            wrote: t.wrote.and_then(AgentWrote::from_core),
            ports: t.ports.into_iter().map(Into::into).collect(),
            runs: t.runs.into_iter().filter_map(AgentRunBrief::from_core).collect(),
        })
    }
}

/// A tool step in a thread: `detail` is the command or a diff, `output` the
/// tail of what it printed.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentTool {
    pub name: String,
    pub title: String,
    pub detail: Option<String>,
    pub status: AgentToolStatus,
    pub output: Option<String>,
    /// What kind of step it is; null from an older PC or for a kind this
    /// build doesn't know: show it as `Other`.
    #[uniffi(default = None)]
    pub tool_class: Option<AgentToolClass>,
    /// The file an edit or read is about, relative to the thread's folder
    /// (or the file's name alone); never an absolute path.
    #[uniffi(default = None)]
    pub path: Option<String>,
    /// The PC allowed it from the thread's "Always allow" rule, without
    /// asking: "Allowed by your rule for this thread" (`AgentTimeline.rows`
    /// marks it so by itself).
    #[uniffi(default = false)]
    pub by_rule: bool,
}

impl std::fmt::Debug for AgentTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentTool").field("status", &self.status).finish_non_exhaustive()
    }
}

/// One entry of a thread, in `seq` order.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentItem {
    pub seq: u64,
    pub id: String,
    pub kind: AgentItemKind,
    pub when_ms: i64,
    pub text: Option<String>,
    pub truncated: bool,
    pub tool: Option<AgentTool>,
    /// The turn it belongs to, from 1; 0 for an item from before turns were
    /// kept (an older PC, or an older thread): count turns from the user's
    /// messages then.
    #[uniffi(default = 0)]
    pub turn: u32,
    /// An assistant item: whether it's the turn's answer; null when not known.
    #[uniffi(default = None)]
    pub phase: Option<AgentItemPhase>,
    /// When it finished, Unix milliseconds (an answer done, a tool ended,
    /// thinking stopped); null while it runs or when not known.
    #[uniffi(default = None)]
    pub end_ms: Option<i64>,
    /// The id of the tool item it was produced inside (a sub-agent's work).
    #[uniffi(default = None)]
    pub group: Option<String>,
    /// A user item: the id of the tool step this message denied.
    #[uniffi(default = None)]
    pub deny_of: Option<String>,
    /// A user item sent while a turn ran ("Sent while it worked").
    #[uniffi(default = None)]
    pub via: Option<AgentVia>,
    /// A user item's images.
    #[uniffi(default = [])]
    pub images: Vec<AgentImage>,
    /// A `Plan` item: where it stands; null reads as proposed.
    #[uniffi(default = None)]
    pub plan_state: Option<AgentPlanState>,
    /// A `Question` item: how many it asked and how it ended.
    #[uniffi(default = None)]
    pub question: Option<AgentItemQuestion>,
    /// A `Notice` item: which notice, for those the app acts on (Undo on
    /// "Rewound to here" while the thread's `undo` is set); null for the rest.
    #[uniffi(default = None)]
    pub notice: Option<AgentNotice>,
}

/// Never prints the text or the ids it names (protocol v1 §11).
impl std::fmt::Debug for AgentItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentItem")
            .field("seq", &self.seq)
            .field("kind", &self.kind)
            .field("tool", &self.tool)
            .field("turn", &self.turn)
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

impl AgentItem {
    fn from_core(i: core::AgentItem) -> Option<Self> {
        let tool = match i.tool {
            Some(t) => Some(AgentTool {
                status: AgentToolStatus::from_wire(&t.status)?,
                tool_class: t.class.as_deref().and_then(AgentToolClass::from_wire),
                name: t.name,
                title: t.title,
                detail: t.detail,
                output: t.output,
                path: t.path,
                by_rule: t.by_rule,
            }),
            None => None,
        };
        Some(AgentItem {
            kind: AgentItemKind::from_wire(&i.kind)?,
            seq: i.seq,
            id: i.id,
            when_ms: ms(i.when),
            text: i.text,
            truncated: i.truncated,
            tool,
            turn: i.turn.unwrap_or(0),
            phase: i.phase.as_deref().and_then(AgentItemPhase::from_wire),
            end_ms: i.end.map(ms),
            group: i.group,
            deny_of: i.deny_of,
            via: i.via.as_deref().and_then(AgentVia::from_wire),
            images: i.images.into_iter().map(Into::into).collect(),
            plan_state: i.plan_state.as_deref().and_then(AgentPlanState::from_wire),
            question: i.question.map(Into::into),
            notice: i.notice.as_deref().and_then(AgentNotice::from_wire),
        })
    }
}

/// A request from an agent waiting for an answer: `command` or `diff` is
/// what it wants to do; `reason` is the agent's own words.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentAsk {
    pub token: String,
    pub thread: String,
    pub thread_title: String,
    /// The agent's id: its name is in the catalog.
    pub agent: String,
    pub kind: AgentAskKind,
    pub title: String,
    pub command: Option<String>,
    pub diff: Option<String>,
    pub path: Option<String>,
    pub reason: Option<String>,
    /// The PC cut the command or diff: the rest is on the PC.
    pub truncated: bool,
    pub expires_ms: i64,
    /// The id of the tool item the request belongs to; null from an older PC.
    #[uniffi(default = None)]
    pub item: Option<String>,
}

impl std::fmt::Debug for AgentAsk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentAsk")
            .field("thread", &self.thread)
            .field("kind", &self.kind)
            .field("truncated", &self.truncated)
            .finish_non_exhaustive()
    }
}

impl AgentAsk {
    fn from_core(a: core::AgentAsk) -> Option<Self> {
        Some(AgentAsk {
            kind: AgentAskKind::from_wire(&a.kind)?,
            token: a.token,
            thread: a.thread,
            thread_title: a.thread_title,
            agent: a.agent,
            title: a.title,
            command: a.command,
            diff: a.diff,
            path: a.path,
            reason: a.reason,
            truncated: a.truncated,
            expires_ms: ms(a.expires),
            item: a.item,
        })
    }
}

/// A page of a thread (`agent_read`): its items in `seq` order and the asks
/// waiting in it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentPage {
    pub thread: AgentThread,
    pub items: Vec<AgentItem>,
    pub more_before: bool,
    pub asks: Vec<AgentAsk>,
    /// Questions waiting in the thread (a secret one without its asks).
    /// Added with `agent.control`.
    #[uniffi(default = [])]
    pub questions: Vec<AgentQuestion>,
}

impl AgentPage {
    fn from_core(p: core::AgentItems) -> Option<Self> {
        Some(AgentPage {
            thread: AgentThread::from_core(p.thread)?,
            items: p.items.into_iter().filter_map(AgentItem::from_core).collect(),
            more_before: p.more_before,
            asks: p.asks.into_iter().filter_map(AgentAsk::from_core).collect(),
            questions: p.questions.into_iter().map(Into::into).collect(),
        })
    }
}

/// A session's state on a PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentSessionState {
    Working,
    /// Waiting for an answer, as the agent reported: "Needs you".
    NeedsYou,
    /// It looks like it waits, guessed from its file: "May need you". Never
    /// counted in a badge, a chip or Home.
    MayNeedYou,
    Idle,
    /// No process has it open any more: "Ended".
    Ended,
}

impl AgentSessionState {
    fn from_wire(state: &str, guess: bool) -> Option<Self> {
        use core::agent::session_state;
        Some(match state {
            session_state::WORKING => Self::Working,
            session_state::NEEDS_YOU if guess => Self::MayNeedYou,
            session_state::NEEDS_YOU => Self::NeedsYou,
            session_state::MAY_NEED_YOU => Self::MayNeedYou,
            session_state::IDLE => Self::Idle,
            session_state::ENDED => Self::Ended,
            _ => return None,
        })
    }
}

/// Where a session runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentSessionSource {
    /// "Terminal".
    Terminal,
    /// "Claude app" (or the agent's own desktop app).
    DesktopApp,
    Other,
}

impl AgentSessionSource {
    fn from_wire(source: &str) -> Self {
        use core::agent::session_source;
        match source {
            session_source::TERMINAL => Self::Terminal,
            session_source::DESKTOP_APP => Self::DesktopApp,
            _ => Self::Other,
        }
    }
}

/// Why a session can't be continued here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AgentContinueBlock {
    /// Something still has it open.
    Open,
    /// Its folder isn't one of the PC's agent folders.
    Folder,
    /// The agent can't run now (not installed or signed out).
    Agent,
}

impl AgentContinueBlock {
    fn from_wire(blocked: &str) -> Option<Self> {
        use core::agent::session_blocked;
        Some(match blocked {
            session_blocked::OPEN => Self::Open,
            session_blocked::FOLDER => Self::Folder,
            session_blocked::AGENT => Self::Agent,
            _ => return None,
        })
    }
}

/// A session started on a PC outside G7 Link (in a terminal or the
/// agent's own app). Read-only: it can't be typed into or answered from
/// here. `folder` is the folder's name, never its path.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentSession {
    pub id: String,
    /// The agent's id (`claude`, `codex`).
    pub agent: String,
    pub folder: String,
    pub title: String,
    pub state: AgentSessionState,
    /// The state is inferred from its file, not reported by the agent.
    pub guess: bool,
    pub source: AgentSessionSource,
    /// Unix milliseconds.
    pub updated_ms: i64,
    pub step: Option<String>,
    pub summary: Option<String>,
    /// What it waits for (`permission prompt`), while it needs you.
    pub waiting_for: Option<String>,
    /// "Continue here" can make it a thread now.
    pub can_continue: bool,
    pub blocked: Option<AgentContinueBlock>,
    /// When `state` began, Unix milliseconds.
    #[uniffi(default = None)]
    pub since_ms: Option<i64>,
    /// It stopped at its agent's usage limit (`state` is `Idle`): "Limit
    /// reached · resets %1", or "Limit reached" without a time.
    #[uniffi(default = None)]
    pub limit: Option<AgentSessionLimit>,
}

/// A session's stop at its agent's usage limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct AgentSessionLimit {
    /// When the limit resets, Unix milliseconds, when the agent said.
    #[uniffi(default = None)]
    pub resets_ms: Option<i64>,
}

impl From<core::AgentSessionLimit> for AgentSessionLimit {
    fn from(l: core::AgentSessionLimit) -> Self {
        Self { resets_ms: l.resets_ms.map(ms) }
    }
}

impl std::fmt::Debug for AgentSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSession")
            .field("id", &self.id)
            .field("agent", &self.agent)
            .field("state", &self.state)
            .field("can_continue", &self.can_continue)
            .field("limit", &self.limit)
            .finish_non_exhaustive()
    }
}

impl AgentSession {
    fn from_core(s: core::AgentSession) -> Option<Self> {
        Some(AgentSession {
            state: AgentSessionState::from_wire(&s.state, s.guess)?,
            guess: s.guess || s.state == core::agent::session_state::MAY_NEED_YOU,
            source: AgentSessionSource::from_wire(&s.source),
            blocked: s.blocked.as_deref().and_then(AgentContinueBlock::from_wire),
            id: s.id,
            agent: s.agent,
            folder: s.folder,
            title: s.title,
            updated_ms: ms(s.updated),
            step: s.step,
            summary: s.summary,
            waiting_for: s.waiting_for,
            can_continue: s.can_continue,
            since_ms: s.since.map(ms),
            limit: s.limit.map(Into::into),
        })
    }
}

/// A page of a session (`agent_session_read`): its items in `seq` order.
/// The `seq`s grow but aren't consecutive: read again after reconnecting
/// rather than looking for gaps.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentSessionPage {
    pub session: AgentSession,
    pub items: Vec<AgentItem>,
    pub more_before: bool,
}

impl AgentSessionPage {
    fn from_core(p: core::AgentSessionItems) -> Option<Self> {
        Some(AgentSessionPage {
            session: AgentSession::from_core(p.session)?,
            items: p.items.into_iter().filter_map(AgentItem::from_core).collect(),
            more_before: p.more_before,
        })
    }
}

/// A thread or a session as Markdown (`agent_export`), to share or save.
/// Kept in memory only.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentExport {
    /// At most 2 MiB, plain text: newlines and tabs, no other controls.
    pub text: String,
    /// Items were left out to fit; the text says so at its end.
    pub truncated: bool,
    /// A name to save it as (`claude-<title>-<date>.md`): `[A-Za-z0-9._-]`,
    /// not starting with a dot.
    pub file_name: String,
}

impl std::fmt::Debug for AgentExport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentExport")
            .field("bytes", &self.text.len())
            .field("truncated", &self.truncated)
            .finish_non_exhaustive()
    }
}

impl From<core::AgentExport> for AgentExport {
    fn from(e: core::AgentExport) -> Self {
        AgentExport { text: e.text, truncated: e.truncated, file_name: e.file_name }
    }
}

/// The `Event` for an `agent.event`, if its wire strings are known.
fn agent_event(id: String, e: core::AgentEvent) -> Option<Event> {
    use core::agent::event;
    Some(match e.kind.as_str() {
        event::STATE => Event::AgentThreadChanged { id, thread: AgentThread::from_core(e.state?)? },
        event::REMOVED => Event::AgentThreadRemoved { id, thread: e.thread },
        event::DELTA => Event::AgentDelta {
            id,
            thread: e.thread,
            item_id: e.item_id?,
            seq: e.seq?,
            offset: e.offset?,
            text: e.text?,
        },
        event::ITEM => Event::AgentItemChanged { id, thread: e.thread, item: AgentItem::from_core(e.item?)? },
        event::DONE => Event::AgentDone {
            id,
            thread: AgentThread::from_core(e.state?)?,
            outcome: AgentOutcome::from_wire(e.outcome.as_deref()?)?,
            summary: e.summary.unwrap_or_default(),
        },
        _ => return None,
    })
}

/// What a PC's clipboard got (`clip.changed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PcClipKind {
    Text,
    Image,
    Private,
}

impl From<core::PcClipKind> for PcClipKind {
    fn from(kind: core::PcClipKind) -> Self {
        match kind {
            core::PcClipKind::Text => PcClipKind::Text,
            core::PcClipKind::Image => PcClipKind::Image,
            core::PcClipKind::Private => PcClipKind::Private,
        }
    }
}

/// The recognized kind of a text clip (`docs/protocol/clipboard.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ClipKind {
    WebLink,
    StreetAddress,
    PhoneNumber,
    TrackingNumber,
    Email,
}

impl From<core::ClipKind> for ClipKind {
    fn from(k: core::ClipKind) -> Self {
        match k {
            core::ClipKind::WebLink => ClipKind::WebLink,
            core::ClipKind::StreetAddress => ClipKind::StreetAddress,
            core::ClipKind::PhoneNumber => ClipKind::PhoneNumber,
            core::ClipKind::TrackingNumber => ClipKind::TrackingNumber,
            core::ClipKind::Email => ClipKind::Email,
        }
    }
}

/// A context chip suggestion for a received text clip.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct ClipSuggestion {
    pub kind: ClipKind,
    pub action_label: String,
    pub target: String,
}

/// Never prints the target text (protocol v1 §11).
impl std::fmt::Debug for ClipSuggestion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipSuggestion").field("kind", &self.kind).finish_non_exhaustive()
    }
}

impl From<core::ClipSuggestion> for ClipSuggestion {
    fn from(s: core::ClipSuggestion) -> Self {
        ClipSuggestion { kind: s.kind.into(), action_label: s.action_label().into(), target: s.target }
    }
}

/// This phone's quick settings state (docs/protocol/toggles.md).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhoneToggles {
    pub dnd: bool,
    /// `"ring"`, `"vibrate"` or `"silent"`.
    pub ringer: String,
    /// `None` when the phone has no camera flash.
    pub flashlight: Option<bool>,
    /// Media volume, `0..=100`.
    pub volume: u8,
    /// Screen brightness, `0..=100`.
    pub brightness: u8,
    pub wifi: bool,
    pub bluetooth: bool,
    /// Whether the Wi-Fi hotspot is on; `None` when it can't be read.
    pub hotspot: Option<bool>,
}

impl From<core::PhoneToggles> for PhoneToggles {
    fn from(t: core::PhoneToggles) -> Self {
        PhoneToggles {
            dnd: t.dnd,
            ringer: t.ringer,
            flashlight: t.flashlight,
            volume: t.volume,
            brightness: t.brightness,
            wifi: t.wifi,
            bluetooth: t.bluetooth,
            hotspot: t.hotspot,
        }
    }
}

impl From<PhoneToggles> for core::PhoneToggles {
    fn from(t: PhoneToggles) -> Self {
        core::PhoneToggles {
            dnd: t.dnd,
            ringer: t.ringer,
            flashlight: t.flashlight,
            volume: t.volume,
            brightness: t.brightness,
            wifi: t.wifi,
            bluetooth: t.bluetooth,
            hotspot: t.hotspot,
        }
    }
}

/// The value to set for a phone quick setting (`toggles.set`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PhoneToggleValue {
    Bool { on: bool },
    Level { level: u8 },
    Mode { mode: String },
}

/// How a request to see a PC's screen (`start_pc_mirror`) went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PcMirrorStart {
    /// The PC agreed: the picture follows (after GNOME's dialog, if it asks:
    /// `PcMirrorNotice` `Prompt`).
    Started,
    /// The PC doesn't share its screen with this phone (its "See this screen"
    /// is off for it), or this phone may not see it.
    Denied,
    /// The PC can't share its screen (no way to capture or encode it).
    Unsupported,
}

/// How `mirror.start` went, for `start_pc_mirror`: a refusal is an answer,
/// not an error.
fn pc_mirror_start(result: core::Result<()>) -> Result<PcMirrorStart> {
    match result {
        Ok(()) => Ok(PcMirrorStart::Started),
        Err(core::Error::Denied | core::Error::Declined) => Ok(PcMirrorStart::Denied),
        Err(core::Error::Unsupported) => Ok(PcMirrorStart::Unsupported),
        Err(e) => Err(e.into()),
    }
}

/// What a PC says about sharing its screen with this phone (`mirror.notice`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PcMirrorNoticeKind {
    /// GNOME asks someone at the PC to choose the screen to share; the
    /// picture comes once they do.
    Prompt,
    /// Play on phone ended: the player's window is gone.
    NoWindow,
}

impl PcMirrorNoticeKind {
    /// The notice `code` a PC sent about mirroring `session`, if it's one for
    /// a phone (about the PC's screen).
    fn from_code(session: u32, code: &str) -> Option<Self> {
        // docs/protocol/mirror.md §2 (`mirror::NOTICE_PROMPT`, `NOTICE_NO_WINDOW`).
        match (session, code) {
            (core::MIRROR_SCREEN, "prompt") => Some(Self::Prompt),
            (core::MIRROR_SCREEN, "no_window") => Some(Self::NoWindow),
            _ => None,
        }
    }
}

/// A PC's clock against this phone's, for the time its screen's pictures
/// take to arrive (`pc_mirror_clock`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct PcMirrorClock {
    /// The PC's monotonic clock minus this phone's `CLOCK_MONOTONIC`
    /// (`System.nanoTime() / 1000`), in microseconds: a frame's `time_us`
    /// less this is when it reached the PC's app, on the phone's clock.
    pub offset_us: i64,
    /// The round trip the estimate was timed over, in microseconds: its
    /// error is at most half of it.
    pub rtt_us: u32,
}

/// The newest clock estimate of each PC whose screen this phone shows.
#[derive(Debug, Default)]
struct PcMirrorClocks(std::sync::Mutex<std::collections::HashMap<String, PcMirrorClock>>);

impl PcMirrorClocks {
    fn get(&self, id: &str) -> Option<PcMirrorClock> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).get(id).copied()
    }

    fn set(&self, id: &str, clock: Option<PcMirrorClock>) {
        let mut clocks = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match clock {
            Some(clock) => clocks.insert(id.to_owned(), clock),
            None => clocks.remove(id),
        };
    }
}

/// What a mirroring video packet holds (docs/protocol/mirror.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum VideoPacketKind {
    /// The stream's format, from `mirror_config`.
    Config,
    Frame,
    Keyframe,
}

/// The PC's mouse, keyboard, or gamepad on the mirrored screen. Positions are
/// fractions of the screen (0 to 1).
#[derive(Clone, PartialEq, uniffi::Enum)]
pub enum MirrorInputEvent {
    Touch {
        action: TouchPhase,
        x: f32,
        y: f32,
    },
    /// The mouse wheel, in notches (positive: down / right).
    Scroll {
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
    },
    /// "back", "home", "recents", "enter", "backspace", "delete", "left",
    /// "right", "up", "down", "tab" or "notifications".
    Key {
        key: String,
    },
    Text {
        text: String,
    },
    /// Gamepad controller state (XInput-style buttons bitmask, stick axes, and triggers).
    Gamepad {
        buttons: u16,
        lx: i16,
        ly: i16,
        rx: i16,
        ry: i16,
        lt: u8,
        rt: u8,
    },
}

pub type MirrorInput = MirrorInputEvent;

/// Never prints typed text (protocol v1 §11).
impl std::fmt::Debug for MirrorInputEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MirrorInputEvent::Text { text } => write!(f, "Text({} bytes)", text.len()),
            MirrorInputEvent::Touch { action, .. } => write!(f, "Touch({action:?})"),
            MirrorInputEvent::Scroll { .. } => f.write_str("Scroll"),
            MirrorInputEvent::Key { key } => write!(f, "Key({key})"),
            MirrorInputEvent::Gamepad { buttons, .. } => write!(f, "Gamepad({buttons:#06x})"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TouchPhase {
    Down,
    Move,
    Up,
}

impl From<core::MirrorInput> for MirrorInputEvent {
    fn from(input: core::MirrorInput) -> Self {
        match input {
            core::MirrorInput::Touch { action, x, y } => MirrorInputEvent::Touch {
                action: match action {
                    core::TouchAction::Down => TouchPhase::Down,
                    core::TouchAction::Move => TouchPhase::Move,
                    core::TouchAction::Up => TouchPhase::Up,
                },
                x,
                y,
            },
            core::MirrorInput::Scroll { x, y, dx, dy } => MirrorInputEvent::Scroll { x, y, dx, dy },
            core::MirrorInput::Key { key } => MirrorInputEvent::Key { key },
            core::MirrorInput::Text { text } => MirrorInputEvent::Text { text },
            core::MirrorInput::Gamepad { buttons, lx, ly, rx, ry, lt, rt } => {
                MirrorInputEvent::Gamepad { buttons, lx, ly, rx, ry, lt, rt }
            }
        }
    }
}

impl From<MirrorInputEvent> for core::MirrorInput {
    fn from(input: MirrorInputEvent) -> Self {
        match input {
            MirrorInputEvent::Touch { action, x, y } => core::MirrorInput::Touch {
                action: match action {
                    TouchPhase::Down => core::TouchAction::Down,
                    TouchPhase::Move => core::TouchAction::Move,
                    TouchPhase::Up => core::TouchAction::Up,
                },
                x,
                y,
            },
            MirrorInputEvent::Scroll { x, y, dx, dy } => core::MirrorInput::Scroll { x, y, dx, dy },
            MirrorInputEvent::Key { key } => core::MirrorInput::Key { key },
            MirrorInputEvent::Text { text } => core::MirrorInput::Text { text },
            MirrorInputEvent::Gamepad { buttons, lx, ly, rx, ry, lt, rt } => {
                core::MirrorInput::Gamepad { buttons, lx, ly, rx, ry, lt, rt }
            }
        }
    }
}

/// What became of a video (or sound) packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MirrorSendResult {
    Queued,
    /// Sound dropped: the network is behind (the next packet may go).
    Dropped,
    /// The network is behind: request a keyframe from the encoder (frames
    /// are dropped until one comes).
    NeedKeyframe,
    /// The PC stopped watching: stop sharing the screen.
    Closed,
}

/// This phone's screen (or its sound) going to one PC.
#[derive(Debug, uniffi::Object)]
pub struct MirrorStream(core::MirrorStream);

#[uniffi::export]
impl MirrorStream {
    /// Hands over a packet without waiting (a config packet waits for room).
    pub fn send(&self, kind: VideoPacketKind, time_us: u64, data: Vec<u8>) -> MirrorSendResult {
        let kind = match kind {
            VideoPacketKind::Config => core::PacketKind::Config,
            VideoPacketKind::Frame => core::PacketKind::Frame,
            VideoPacketKind::Keyframe => core::PacketKind::Keyframe,
        };
        match self.0.send(kind, time_us, data) {
            core::MirrorSend::Queued => MirrorSendResult::Queued,
            core::MirrorSend::Dropped => MirrorSendResult::Dropped,
            core::MirrorSend::NeedKeyframe => MirrorSendResult::NeedKeyframe,
            core::MirrorSend::Closed => MirrorSendResult::Closed,
        }
    }

    /// Ends the stream (`close` frees the object, as for every object).
    pub fn end(&self) {
        self.0.close();
    }
}

/// One end of a Bluetooth link with a paired PC (docs/protocol/bluetooth.md):
/// give it each value notified on TX, write what `next_outbound` gives to RX
/// in order, and end it on any error or disconnect.
#[derive(Debug, uniffi::Object)]
pub struct BleEndpoint(Arc<core::BleEndpoint>);

#[uniffi::export]
impl BleEndpoint {
    /// What `Event::BleOutbound` names this end by.
    pub fn id(&self) -> u64 {
        self.0.id()
    }

    /// The link's ATT MTU, once negotiated.
    pub fn set_mtu(&self, att_mtu: u16) {
        self.0.set_mtu(att_mtu);
    }

    /// A value the PC notified. An error means the link is done.
    pub fn receive(&self, value: Vec<u8>) -> Result<()> {
        Ok(self.0.receive(&value)?)
    }

    /// The next value to write, or none.
    pub fn next_outbound(&self) -> Option<Vec<u8>> {
        self.0.next_outbound()
    }

    /// The PC's device ID, once it has proved who it is.
    pub fn peer(&self) -> Option<String> {
        self.0.peer().map(|p| p.to_string())
    }

    /// Ends the link (`close` frees the object, as for every object).
    pub fn end(&self) {
        self.0.close();
    }
}

/// The bytes of a config packet for an H.264 stream of this size, for the
/// screen (session 0) or an app window.
#[uniffi::export]
pub fn mirror_config(width: u32, height: u32, session: u32) -> Vec<u8> {
    core::MirrorConfig { codec: "h264".into(), width, height, session }.to_cbor()
}

/// The bytes of a config packet for a stream of `codec` (`"h264"`, `"h265"`, or `"av1"`).
#[uniffi::export]
pub fn mirror_config_codec(codec: String, width: u32, height: u32, session: u32) -> Vec<u8> {
    core::MirrorConfig { codec, width, height, session }.to_cbor()
}

/// What a PC asked to mirror, and how (docs/protocol/mirror.md).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MirrorOptions {
    /// The longer side, in pixels, at most.
    pub max_size: u32,
    pub fps: u32,
    pub bitrate: u32,
    /// The sound too (the screen only).
    pub audio: bool,
    /// 0 for the screen; another for an app window.
    pub session: u32,
    /// The app (package name) to show in a window of its own.
    pub app: Option<String>,
    /// Keep the phone awake while mirroring (`session == 0`).
    pub stay_awake: bool,
    /// Turn the phone's physical screen off while mirroring (`session == 0`, Elevated).
    pub screen_off: bool,
    /// Only the sound, no picture: the PC plays it without a window.
    pub sound_only: bool,
}

/// An app a PC can open in a window (docs/protocol/mirror.md).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhoneApp {
    /// Package name.
    pub pkg: String,
    pub label: String,
    /// A small PNG icon (at most 8 KiB).
    pub icon: Option<Vec<u8>>,
}

impl std::fmt::Debug for PhoneApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhoneApp").field("pkg", &self.pkg).finish_non_exhaustive()
    }
}

/// The bytes of a config packet for 16-bit PCM sound.
#[uniffi::export]
pub fn mirror_audio_config(rate: u32, channels: u8) -> Vec<u8> {
    core::MirrorAudioConfig { codec: core::MIRROR_PCM.into(), rate, channels }.to_cbor()
}

/// The bytes of a config packet for an H.264 webcam stream
/// (`docs/protocol/webcam.md`); `max_height` is 2160 when the camera in use
/// can stream 4K, else 1080.
#[uniffi::export]
pub fn webcam_config(width: u32, height: u32, camera: String, fps: u32, max_height: u32) -> Vec<u8> {
    core::WebcamConfig { codec: core::WEBCAM_H264.into(), width, height, camera, fps, max_height }.to_cbor()
}

/// A webcam config for a negotiated codec; retain webcam_config for older callers.
#[uniffi::export]
pub fn webcam_config_codec(
    codec: String,
    width: u32,
    height: u32,
    camera: String,
    fps: u32,
    max_height: u32,
) -> Vec<u8> {
    core::WebcamConfig { codec, width, height, camera, fps, max_height }.to_cbor()
}

/// What a PC asked the phone's camera to stream (`docs/protocol/webcam.md`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WebcamOptions {
    /// `"back"` or `"front"`.
    pub camera: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
}

/// A conversation (docs/protocol/sms.md).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct SmsThread {
    pub id: String,
    /// The other people's numbers (more than one: a group).
    pub addresses: Vec<String>,
    /// Their contact names, in the same order ("" when not a contact).
    pub names: Vec<String>,
    /// The latest message's text, or a description like "Photo".
    pub snippet: String,
    /// Unix milliseconds.
    pub date: i64,
    pub unread: u32,
    /// The contact's photo (one-person conversations): a JPEG of at most 16 KB.
    pub photo: Option<Vec<u8>>,
}

/// A message in a conversation.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct SmsMessage {
    pub id: String,
    pub thread: String,
    /// Who sent it (received), or the recipient (sent).
    pub address: String,
    pub body: String,
    /// Unix milliseconds.
    pub date: i64,
    pub outgoing: bool,
    /// Sent messages: "sent", "pending" or "failed".
    pub status: Option<String>,
    /// Pictures and other attachments, fetched with `sms_part`.
    pub parts: Vec<SmsPart>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SmsPart {
    pub id: String,
    pub mime: String,
    pub size: u64,
}

/// An attachment's bytes.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct SmsPartData {
    pub mime: String,
    pub data: Vec<u8>,
}

/// An outgoing MMS image attachment (`image/jpeg` or `image/png`).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct SmsAttachment {
    pub mime: String,
    pub data: Vec<u8>,
}

macro_rules! private_debug {
    ($($t:ty),*) => {$(
        /// Never prints numbers, names or text (protocol v1 §11).
        impl std::fmt::Debug for $t {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($t)).finish_non_exhaustive()
            }
        }
    )*};
}
private_debug!(SmsThread, SmsMessage, SmsPartData, SmsAttachment, CallLogEntry, ContactNumber, Contact);

/// One SIM (or calling app) this phone places calls with
/// (docs/protocol/calls.md, `call.sims`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CallSim {
    /// Stays the same while the SIM is in: what `call_dial_on` gets back.
    pub id: String,
    /// What Android shows for it, such as "SIM 1 · Airtel".
    pub label: String,
    /// Whether the phone calls with it without asking.
    pub is_default: bool,
}

/// One call in this phone's call history (docs/protocol/calls.md).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct CallLogEntry {
    pub id: String,
    /// Who called, or who was called ("" when withheld).
    pub number: String,
    /// The contact's name, when the number is a contact.
    pub name: Option<String>,
    /// "incoming", "outgoing", "missed" or "rejected".
    pub direction: String,
    /// Unix milliseconds.
    pub date: i64,
    /// Seconds on the call (0 when missed or not answered).
    pub duration: u32,
    /// The contact's photo: a JPEG of at most 16 KB.
    pub photo: Option<Vec<u8>>,
}

/// One phone number on a contact (docs/protocol/contacts.md).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct ContactNumber {
    pub number: String,
    pub label: Option<String>,
}

/// A contact on this phone (docs/protocol/contacts.md).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct Contact {
    pub id: String,
    pub name: String,
    pub numbers: Vec<ContactNumber>,
    pub starred: bool,
    /// A JPEG of at most 16 KB.
    pub photo: Option<Vec<u8>>,
}

/// A call on this phone (docs/protocol/calls.md).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct Call {
    /// This app's ID for the call, the same while it lasts.
    pub id: String,
    pub state: CallPhase,
    pub incoming: bool,
    pub number: Option<String>,
    /// The contact's name, when the number is a contact.
    pub name: Option<String>,
    /// The contact's photo: a JPEG of at most 64 KB, sent while ringing.
    pub photo: Option<Vec<u8>>,
    /// With `Ended`: it rang and nobody answered.
    pub missed: bool,
    /// With `Active`: when it was answered, in Unix milliseconds.
    pub since: Option<i64>,
    /// With `Active`, when this app controls the call (its in-call service
    /// is bound): mute, speaker and hold.
    pub controls: Option<CallControls>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct CallControls {
    pub muted: bool,
    pub speaker: bool,
    pub held: bool,
    pub can_hold: bool,
}

/// Never prints the number, name or photo (protocol v1 §11).
impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Call").field("id", &self.id).field("state", &self.state).finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CallPhase {
    Ringing,
    Active,
    Ended,
}

/// What a PC asked of a call.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CallCommand {
    Answer,
    /// Decline a ringing call, or hang up an active one.
    Decline,
    /// Stop the ringing.
    Silence,
    /// Mute (true) or unmute the microphone on the call.
    Mute {
        on: bool,
    },
    /// The speaker (true) or the earpiece.
    Speaker {
        on: bool,
    },
    /// Put the call on hold (true) or take it off.
    Hold {
        on: bool,
    },
    /// A keypad tone: "0"-"9", "*" or "#".
    Dtmf {
        digit: String,
    },
    /// The call's volume up (true) or down.
    Volume {
        up: bool,
    },
}

/// A photo or screenshot that just appeared on this phone
/// (docs/protocol/photos.md).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct Photo {
    /// This app's ID for it (a PC asks for it by this ID).
    pub id: String,
    pub name: String,
    pub size: u64,
    /// When it was taken, in Unix seconds.
    pub taken: i64,
    pub screenshot: bool,
    /// A JPEG preview of at most 96 KB, at most 512 pixels a side.
    pub thumb: Vec<u8>,
}

/// Never prints the name or picture (protocol v1 §11).
impl std::fmt::Debug for Photo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Photo").field("id", &self.id).finish_non_exhaustive()
    }
}

/// An album (folder) in this phone's gallery (docs/protocol/photos.md).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhotoAlbum {
    pub id: String,
    pub name: String,
    pub count: u32,
    /// Newest item's ID in the album, for its cover thumbnail.
    pub cover: Option<String>,
}

/// A photo or video in this phone's gallery (docs/protocol/photos.md).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhotoItem {
    pub id: String,
    pub name: String,
    /// Unix milliseconds.
    pub date: i64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    /// Video duration in milliseconds; `None` for still photos.
    pub duration: Option<u32>,
    pub album: Option<String>,
}

/// A JPEG thumbnail for a gallery item (docs/protocol/photos.md).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhotoThumb {
    pub id: String,
    /// JPEG of at most 32 KiB, ~256 px on the longer side.
    pub data: Vec<u8>,
}

/// One file or folder in a phone directory listing (`docs/protocol/storage.md`).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct StorageEntry {
    pub name: String,
    pub size: u64,
    /// Unix milliseconds.
    pub modified: i64,
    pub is_dir: bool,
}

/// An opened file on this phone ready for ranged reading (`storage.read`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct StorageReadFile {
    pub source: FileToSend,
    pub size: u64,
    /// Unix milliseconds.
    pub modified: i64,
}

/// Result of writing a file into this phone's storage (`storage.write`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct StorageWriteDone {
    pub size: u64,
    /// Unix milliseconds.
    pub modified: i64,
}

/// Whether a clipboard history item is text or an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ClipboardItemKind {
    Text,
    Image,
}

impl From<core::ClipboardItemKind> for ClipboardItemKind {
    fn from(k: core::ClipboardItemKind) -> Self {
        match k {
            core::ClipboardItemKind::Text => ClipboardItemKind::Text,
            core::ClipboardItemKind::Image => ClipboardItemKind::Image,
        }
    }
}

/// One clip in the local clipboard history.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct ClipboardHistoryEntry {
    pub id: String,
    pub kind: ClipboardItemKind,
    pub text: String,
    pub mime: Option<String>,
    pub device_name: String,
    pub incoming: bool,
    pub timestamp: i64,
    pub pinned: bool,
}

impl From<core::ClipboardHistoryEntry> for ClipboardHistoryEntry {
    fn from(e: core::ClipboardHistoryEntry) -> Self {
        ClipboardHistoryEntry {
            id: e.id,
            kind: e.kind.into(),
            text: e.text,
            mime: e.mime,
            device_name: e.device_name,
            incoming: e.incoming,
            timestamp: e.timestamp,
            pinned: e.pinned,
        }
    }
}

/// Kind of cross-device activity recorded in the local timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TimelineKind {
    File,
    Clip,
    Link,
    Photo,
    Recording,
    Session,
}

impl From<core::TimelineKind> for TimelineKind {
    fn from(k: core::TimelineKind) -> Self {
        match k {
            core::TimelineKind::File => TimelineKind::File,
            core::TimelineKind::Clip => TimelineKind::Clip,
            core::TimelineKind::Link => TimelineKind::Link,
            core::TimelineKind::Photo => TimelineKind::Photo,
            core::TimelineKind::Recording => TimelineKind::Recording,
            core::TimelineKind::Session => TimelineKind::Session,
        }
    }
}

impl From<TimelineKind> for core::TimelineKind {
    fn from(k: TimelineKind) -> Self {
        match k {
            TimelineKind::File => core::TimelineKind::File,
            TimelineKind::Clip => core::TimelineKind::Clip,
            TimelineKind::Link => core::TimelineKind::Link,
            TimelineKind::Photo => core::TimelineKind::Photo,
            TimelineKind::Recording => core::TimelineKind::Recording,
            TimelineKind::Session => core::TimelineKind::Session,
        }
    }
}

/// One item in the local cross-device timeline.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct TimelineEntry {
    pub id: i64,
    pub kind: TimelineKind,
    pub device_id: String,
    pub device_name: String,
    pub incoming: bool,
    pub timestamp: i64,
    pub title: String,
    pub detail: String,
    pub target: String,
    pub size_bytes: u64,
    pub duration_secs: u64,
    pub ref_id: Option<String>,
    pub clip_available: bool,
    pub image_data_url: Option<String>,
}

impl From<core::TimelineEntry> for TimelineEntry {
    fn from(e: core::TimelineEntry) -> Self {
        let (title, detail, target) =
            if e.kind == core::TimelineKind::Clip && !e.title.is_empty() && e.title != "Image clip" {
                let raw = e.title.clone();
                let (friendly, raw_detail) = core::format_clip_display(&e.title);
                let det = raw_detail.unwrap_or(e.detail);
                let tgt = if e.target.is_empty() { raw } else { e.target };
                (friendly, det, tgt)
            } else {
                (e.title, e.detail, e.target)
            };
        TimelineEntry {
            id: e.id,
            kind: e.kind.into(),
            device_id: e.device_id.to_string(),
            device_name: e.device_name,
            incoming: e.incoming,
            timestamp: e.timestamp,
            title,
            detail,
            target,
            size_bytes: e.size_bytes,
            duration_secs: e.duration_secs,
            ref_id: e.ref_id,
            clip_available: e.clip_available,
            image_data_url: e.image_data_url,
        }
    }
}

/// A paged result from the local timeline.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TimelinePage {
    pub items: Vec<TimelineEntry>,
    pub has_more: bool,
    pub total: u32,
}

impl From<core::TimelinePage> for TimelinePage {
    fn from(p: core::TimelinePage) -> Self {
        TimelinePage {
            items: p.entries.into_iter().map(Into::into).collect(),
            has_more: p.has_more,
            total: p.total,
        }
    }
}

/// Retention policy for the local timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct TimelineRetention {
    pub max_days: u32,
    pub max_entries: u32,
}

impl From<core::TimelineRetention> for TimelineRetention {
    fn from(r: core::TimelineRetention) -> Self {
        TimelineRetention { max_days: r.max_days, max_entries: r.max_entries }
    }
}

/// Summary of locally stored history and cache counts for Privacy & Data settings.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DataRetentionCounts {
    pub clipboard_items: u32,
    pub clipboard_bytes: u64,
    pub timeline_items: u32,
    pub timeline_retention_days: u32,
    pub chat_threads: u32,
    pub chat_messages: u32,
    pub received_file_records: u32,
}

impl From<core::DataRetentionCounts> for DataRetentionCounts {
    fn from(c: core::DataRetentionCounts) -> Self {
        DataRetentionCounts {
            clipboard_items: c.clipboard_items,
            clipboard_bytes: c.clipboard_bytes,
            timeline_items: c.timeline_items,
            timeline_retention_days: c.timeline_retention_days,
            chat_threads: c.chat_threads,
            chat_messages: c.chat_messages,
            received_file_records: c.received_file_records,
        }
    }
}

/// Kind of item in the persistent outgoing send queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiQueueKind {
    Files,
    Folder,
    Clipboard,
    ClipboardImage,
    Link,
    HandoffDocument,
    Recording,
    TaskNotify,
    CameraRequest,
}

impl From<core::QueueKind> for FfiQueueKind {
    fn from(k: core::QueueKind) -> Self {
        match k {
            core::QueueKind::Files => FfiQueueKind::Files,
            core::QueueKind::Folder => FfiQueueKind::Folder,
            core::QueueKind::Clipboard => FfiQueueKind::Clipboard,
            core::QueueKind::ClipboardImage => FfiQueueKind::ClipboardImage,
            core::QueueKind::Link => FfiQueueKind::Link,
            core::QueueKind::HandoffDocument => FfiQueueKind::HandoffDocument,
            core::QueueKind::Recording => FfiQueueKind::Recording,
            core::QueueKind::TaskNotify => FfiQueueKind::TaskNotify,
            core::QueueKind::CameraRequest => FfiQueueKind::CameraRequest,
        }
    }
}

/// Lifecycle state of an item in the persistent outgoing send queue.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FfiQueueStatus {
    Waiting,
    Sending,
    Failed { error: String },
    Expired { error: String },
}

impl From<core::QueueStatus> for FfiQueueStatus {
    fn from(s: core::QueueStatus) -> Self {
        match s {
            core::QueueStatus::Waiting => FfiQueueStatus::Waiting,
            core::QueueStatus::Sending => FfiQueueStatus::Sending,
            core::QueueStatus::Failed { error } => FfiQueueStatus::Failed { error },
            core::QueueStatus::Expired { error } => FfiQueueStatus::Expired { error },
        }
    }
}

/// Source or staged file metadata for a queued file, folder, handoff, or recording item.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiQueueFile {
    pub path: String,
    pub name: String,
    pub folder: Option<String>,
    pub size: u64,
    pub mtime_ms: i64,
    pub staged: bool,
}

impl From<core::QueuedFileSpec> for FfiQueueFile {
    fn from(f: core::QueuedFileSpec) -> Self {
        FfiQueueFile {
            path: f.path.to_string_lossy().into_owned(),
            name: f.name,
            folder: f.folder,
            size: f.size,
            mtime_ms: f.mtime_ms,
            staged: f.staged,
        }
    }
}

/// One item in the persistent outgoing send queue.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiQueueItem {
    pub id: String,
    pub seq: i64,
    pub device_id: String,
    pub device_name: String,
    pub kind: FfiQueueKind,
    pub status: FfiQueueStatus,
    pub created_at: i64,
    pub expires_at: i64,
    pub attempts: u32,
    pub title: String,
    pub detail: String,
    pub bytes_total: u64,
    pub bytes_done: u64,
    pub files_total: u32,
    pub files_done: u32,
    pub files: Vec<FfiQueueFile>,
}

impl From<core::QueueItem> for FfiQueueItem {
    fn from(i: core::QueueItem) -> Self {
        FfiQueueItem {
            id: i.id,
            seq: i.seq,
            device_id: i.peer.to_string(),
            device_name: i.peer_name,
            kind: i.kind.into(),
            status: i.status.into(),
            created_at: i.created_at,
            expires_at: i.expires_at,
            attempts: i.attempts,
            title: i.title,
            detail: i.detail,
            bytes_total: i.bytes_total,
            bytes_done: i.bytes_done,
            files_total: i.files_total,
            files_done: i.files_done,
            files: i.files.into_iter().map(Into::into).collect(),
        }
    }
}

private_debug!(
    PhotoAlbum,
    PhotoItem,
    PhotoThumb,
    StorageEntry,
    ClipboardHistoryEntry,
    TimelineEntry,
    FfiQueueItem
);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum G7LinkError {
    #[error("device is not paired")]
    NotPaired,
    #[error("device is offline")]
    Offline,
    #[error("the operation was denied")]
    Denied,
    #[error("declined")]
    Declined,
    #[error("timed out")]
    Timeout,
    /// `this_device`: whether this app (true) or the other device's app
    /// (false) needs an update.
    #[error("an app is too old for this connection")]
    VersionTooOld { this_device: bool },
    #[error("not supported by the other device")]
    Unsupported,
    #[error("it no longer exists")]
    NotFound,
    #[error("too large to send")]
    TooLarge,
    /// The other device has too much in flight: worth trying again shortly.
    #[error("the other device is busy")]
    Busy,
    #[error("send queue is full: {reason}")]
    QueueFull { reason: String },
    #[error("no pairing in progress")]
    NotPairing,
    #[error("invalid pairing link")]
    InvalidPairingLink,
    #[error("invalid device ID")]
    InvalidDeviceId,
    #[error("network error: {reason}")]
    Network { reason: String },
    #[error("storage error: {reason}")]
    Storage { reason: String },
    #[error("internal error: {reason}")]
    Internal { reason: String },
}

impl From<core::Error> for G7LinkError {
    fn from(e: core::Error) -> Self {
        match e {
            core::Error::NotPaired => G7LinkError::NotPaired,
            core::Error::Offline => G7LinkError::Offline,
            core::Error::Denied => G7LinkError::Denied,
            core::Error::Declined => G7LinkError::Declined,
            core::Error::Timeout => G7LinkError::Timeout,
            core::Error::VersionTooOld { side } => {
                G7LinkError::VersionTooOld { this_device: side == core::Side::Local }
            }
            core::Error::Unsupported => G7LinkError::Unsupported,
            core::Error::NotFound => G7LinkError::NotFound,
            core::Error::TooLarge => G7LinkError::TooLarge,
            core::Error::Busy => G7LinkError::Busy,
            core::Error::QueueFull(reason) => G7LinkError::QueueFull { reason },
            core::Error::NotPairing => G7LinkError::NotPairing,
            core::Error::InvalidPairingLink(_) => G7LinkError::InvalidPairingLink,
            core::Error::Network(reason) => G7LinkError::Network { reason },
            core::Error::Storage(reason) => G7LinkError::Storage { reason },
            core::Error::Io(e) => G7LinkError::Storage { reason: e.to_string() },
            core::Error::Protocol(reason) | core::Error::Internal(reason) => G7LinkError::Internal { reason },
        }
    }
}

type Result<T, E = G7LinkError> = std::result::Result<T, E>;

// ---- Conversions ----

impl From<core::DeviceKind> for DeviceKind {
    fn from(kind: core::DeviceKind) -> Self {
        match kind {
            core::DeviceKind::Phone => DeviceKind::Phone,
            core::DeviceKind::Tablet => DeviceKind::Tablet,
            core::DeviceKind::Desktop => DeviceKind::Desktop,
            core::DeviceKind::Laptop => DeviceKind::Laptop,
            core::DeviceKind::Unknown => DeviceKind::Unknown,
        }
    }
}

impl From<DeviceKind> for core::DeviceKind {
    fn from(kind: DeviceKind) -> Self {
        match kind {
            DeviceKind::Phone => core::DeviceKind::Phone,
            DeviceKind::Tablet => core::DeviceKind::Tablet,
            DeviceKind::Desktop => core::DeviceKind::Desktop,
            DeviceKind::Laptop => core::DeviceKind::Laptop,
            DeviceKind::Unknown => core::DeviceKind::Unknown,
        }
    }
}

impl From<core::ScreenRect> for ScreenRect {
    fn from(r: core::ScreenRect) -> Self {
        ScreenRect { x: r.x, y: r.y, w: r.w, h: r.h }
    }
}

impl From<ScreenRect> for core::ScreenRect {
    fn from(r: ScreenRect) -> Self {
        core::ScreenRect { x: r.x, y: r.y, w: r.w, h: r.h }
    }
}

impl From<core::ScreenCorners> for ScreenCorners {
    fn from(c: core::ScreenCorners) -> Self {
        ScreenCorners { tl: c.tl, tr: c.tr, br: c.br, bl: c.bl }
    }
}

impl From<ScreenCorners> for core::ScreenCorners {
    fn from(c: ScreenCorners) -> Self {
        core::ScreenCorners { tl: c.tl, tr: c.tr, br: c.br, bl: c.bl }
    }
}

impl From<core::ScreenShape> for ScreenShape {
    fn from(s: core::ScreenShape) -> Self {
        ScreenShape {
            v: s.v,
            aspect: s.aspect,
            corners: s.corners.map(Into::into),
            cutouts: s.cutouts.into_iter().map(Into::into).collect(),
            cutout_path: s.cutout_path,
        }
    }
}

impl From<ScreenShape> for core::ScreenShape {
    fn from(s: ScreenShape) -> Self {
        core::ScreenShape {
            v: s.v,
            aspect: s.aspect,
            corners: s.corners.map(Into::into),
            cutouts: s.cutouts.into_iter().map(Into::into).collect(),
            cutout_path: s.cutout_path,
        }
    }
}

impl From<core::DeviceInfo> for DeviceInfo {
    fn from(i: core::DeviceInfo) -> Self {
        DeviceInfo {
            name: i.name,
            kind: i.kind.into(),
            os: i.os,
            os_version: i.os_ver,
            model: i.model,
            accent: i.accent,
            screen: i.screen.map(Into::into),
            install_id: i.install_id,
        }
    }
}

impl From<DeviceInfo> for core::DeviceInfo {
    fn from(i: DeviceInfo) -> Self {
        core::DeviceInfo {
            name: i.name,
            kind: i.kind.into(),
            os: i.os,
            os_ver: i.os_version,
            model: i.model,
            accent: i.accent,
            screen: i.screen.map(Into::into),
            install_id: i.install_id,
        }
        .sanitized()
    }
}

impl From<core::PowerLevel> for PowerLevel {
    fn from(p: core::PowerLevel) -> Self {
        match p.effective() {
            core::PowerLevel::Assist => PowerLevel::Assist,
            core::PowerLevel::Elevated => PowerLevel::Elevated,
            core::PowerLevel::NotApplicable => PowerLevel::NotApplicable,
            core::PowerLevel::Basic | core::PowerLevel::Unknown => PowerLevel::Basic,
        }
    }
}

impl From<PowerLevel> for core::PowerLevel {
    fn from(p: PowerLevel) -> Self {
        match p {
            PowerLevel::Basic => core::PowerLevel::Basic,
            PowerLevel::Assist => core::PowerLevel::Assist,
            PowerLevel::Elevated => core::PowerLevel::Elevated,
            PowerLevel::NotApplicable => core::PowerLevel::NotApplicable,
        }
    }
}

impl From<core::Battery> for Battery {
    fn from(b: core::Battery) -> Self {
        Battery {
            level: b.level,
            charging: b.charging,
            plugged: b.plugged,
            full_in: b.full_in,
            cell_bars: b.cell_bars,
            cell_type: b.cell_type,
            wifi_bars: b.wifi_bars,
        }
    }
}

impl From<Battery> for core::Battery {
    fn from(b: Battery) -> Self {
        core::Battery {
            level: b.level.min(100),
            charging: b.charging,
            plugged: b.plugged,
            full_in: b.full_in,
            cell_bars: b.cell_bars,
            cell_type: b.cell_type,
            wifi_bars: b.wifi_bars,
        }
        .sanitized()
    }
}

impl From<LinkState> for Link {
    fn from(l: LinkState) -> Self {
        match l {
            LinkState::Offline { last_seen } => Link::Offline { last_seen },
            LinkState::Connecting => Link::Connecting,
            LinkState::Online { path, rtt_ms } => Link::Online {
                relayed: path == ConnectionPath::Relay,
                usb: path == ConnectionPath::Usb,
                rtt_ms,
                tailnet: path == ConnectionPath::Tailscale,
            },
        }
    }
}

impl From<core::PairedDevice> for PairedDevice {
    fn from(d: core::PairedDevice) -> Self {
        PairedDevice {
            id: d.id.to_string(),
            info: d.info.into(),
            paired_at: d.paired_at,
            link: d.link.into(),
            can_wake: d.can_wake,
            reconnect: d.reconnect,
        }
    }
}

impl From<core::DiscoveredDevice> for DiscoveredDevice {
    fn from(d: core::DiscoveredDevice) -> Self {
        DiscoveredDevice { id: d.id.to_string(), name: d.name }
    }
}

impl From<core::LocalSendPeer> for LocalSendPeer {
    fn from(p: core::LocalSendPeer) -> Self {
        LocalSendPeer {
            id: p.id.to_string(),
            alias: p.alias,
            device_model: p.device_model,
            device_type: p.device_type,
            fingerprint: p.fingerprint,
            ip: p.ip,
            port: p.port,
            protocol: p.protocol,
        }
    }
}

impl From<core::PairingFailure> for PairingFailure {
    fn from(f: core::PairingFailure) -> Self {
        match f {
            core::PairingFailure::Rejected => PairingFailure::Rejected,
            core::PairingFailure::Declined => PairingFailure::Declined,
            core::PairingFailure::Expired => PairingFailure::Expired,
            core::PairingFailure::Unreachable => PairingFailure::Unreachable,
            core::PairingFailure::Other(message) => PairingFailure::Other { message },
        }
    }
}

impl From<core::LiveSegment> for LiveSegment {
    fn from(s: core::LiveSegment) -> Self {
        LiveSegment { length: s.length, color: s.color }
    }
}

impl From<LiveSegment> for core::LiveSegment {
    fn from(s: LiveSegment) -> Self {
        core::LiveSegment { length: s.length, color: s.color }
    }
}

impl From<core::LivePoint> for LivePoint {
    fn from(p: core::LivePoint) -> Self {
        LivePoint { position: p.position, color: p.color }
    }
}

impl From<LivePoint> for core::LivePoint {
    fn from(p: LivePoint) -> Self {
        core::LivePoint { position: p.position, color: p.color }
    }
}

impl From<core::NotificationLive> for NotificationLive {
    fn from(l: core::NotificationLive) -> Self {
        NotificationLive {
            v: l.v,
            progress: l.progress,
            max: l.max,
            indeterminate: l.indeterminate,
            chip: l.chip,
            segments: l.segments.into_iter().map(Into::into).collect(),
            points: l.points.into_iter().map(Into::into).collect(),
            chronometer: l.chronometer,
            countdown: l.countdown,
        }
    }
}

impl From<NotificationLive> for core::NotificationLive {
    fn from(l: NotificationLive) -> Self {
        core::NotificationLive {
            v: l.v,
            progress: l.progress,
            max: l.max,
            indeterminate: l.indeterminate,
            chip: l.chip,
            segments: l.segments.into_iter().map(Into::into).collect(),
            points: l.points.into_iter().map(Into::into).collect(),
            chronometer: l.chronometer,
            countdown: l.countdown,
        }
    }
}

impl From<core::TaskNotify> for TaskNotify {
    fn from(t: core::TaskNotify) -> Self {
        TaskNotify {
            id: t.id,
            title: t.title,
            active: t.active,
            elapsed_ms: t.elapsed_ms,
            exit_code: t.exit_code,
        }
    }
}

impl From<TaskNotify> for core::TaskNotify {
    fn from(t: TaskNotify) -> Self {
        core::TaskNotify {
            id: t.id,
            title: t.title,
            active: t.active,
            elapsed_ms: t.elapsed_ms,
            exit_code: t.exit_code,
        }
    }
}

impl From<core::NotificationChatMessage> for NotificationChatMessage {
    fn from(m: core::NotificationChatMessage) -> Self {
        NotificationChatMessage {
            sender: m.sender,
            text: m.text,
            time: m.time,
            self_sent: m.self_sent,
            avatar: m.avatar,
        }
    }
}

impl From<NotificationChatMessage> for core::NotificationChatMessage {
    fn from(m: NotificationChatMessage) -> Self {
        core::NotificationChatMessage {
            sender: m.sender,
            text: m.text,
            time: m.time,
            self_sent: m.self_sent,
            avatar: m.avatar,
        }
    }
}

impl From<core::NotificationConversation> for NotificationConversation {
    fn from(c: core::NotificationConversation) -> Self {
        NotificationConversation {
            v: c.v,
            title: c.title,
            group: c.group,
            avatar: c.avatar,
            messages: c.messages.into_iter().map(Into::into).collect(),
            id: c.id,
        }
    }
}

impl From<NotificationConversation> for core::NotificationConversation {
    fn from(c: NotificationConversation) -> Self {
        core::NotificationConversation {
            v: c.v,
            id: c.id,
            title: c.title,
            group: c.group,
            avatar: c.avatar,
            messages: c.messages.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<core::SmsAttachment> for SmsAttachment {
    fn from(a: core::SmsAttachment) -> Self {
        SmsAttachment { mime: a.mime, data: a.data }
    }
}

impl From<SmsAttachment> for core::SmsAttachment {
    fn from(a: SmsAttachment) -> Self {
        core::SmsAttachment { mime: a.mime, data: a.data }
    }
}

impl From<core::Notification> for Notification {
    fn from(n: core::Notification) -> Self {
        Notification {
            key: n.key,
            app: n.app,
            app_name: n.app_name,
            title: n.title,
            text: n.text,
            sub: n.sub,
            when: n.when,
            actions: n
                .actions
                .into_iter()
                .map(|a| NotificationAction { id: a.id, title: a.title, reply: a.reply })
                .collect(),
            silent: n.silent,
            ongoing: n.ongoing,
            icon: n.icon,
            image: n.image,
            live: n.live.map(Into::into),
            conversation: n.conversation.map(Into::into),
        }
    }
}

impl From<Notification> for core::Notification {
    fn from(n: Notification) -> Self {
        core::Notification {
            key: n.key,
            app: n.app,
            app_name: n.app_name,
            title: n.title,
            text: n.text,
            sub: n.sub,
            when: n.when,
            actions: n
                .actions
                .into_iter()
                .map(|a| core::NotificationAction { id: a.id, title: a.title, reply: a.reply })
                .collect(),
            silent: n.silent,
            ongoing: n.ongoing,
            icon: n.icon,
            image: n.image,
            live: n.live.map(Into::into),
            conversation: n.conversation.map(Into::into),
        }
    }
}

impl From<core::RecordingMarker> for RecordingMarker {
    fn from(m: core::RecordingMarker) -> Self {
        RecordingMarker { at_ms: m.at_ms, label: m.label }
    }
}

impl From<RecordingMarker> for core::RecordingMarker {
    fn from(m: RecordingMarker) -> Self {
        core::RecordingMarker { at_ms: m.at_ms, label: m.label }
    }
}

impl From<core::Transfer> for Transfer {
    fn from(t: core::Transfer) -> Self {
        Transfer {
            id: t.id,
            device_id: t.device.to_string(),
            direction: match t.direction {
                core::Direction::Outgoing => TransferDirection::Outgoing,
                core::Direction::Incoming => TransferDirection::Incoming,
            },
            names: t.names,
            files: u32::try_from(t.files).unwrap_or(u32::MAX),
            total: t.total,
            done: t.done,
            status: match t.state {
                core::TransferState::Requested => TransferStatus::Requested,
                core::TransferState::Waiting => TransferStatus::Waiting,
                core::TransferState::Running => TransferStatus::Running,
                core::TransferState::Paused => TransferStatus::Paused,
                core::TransferState::Done { saved } => TransferStatus::Done {
                    saved: saved.into_iter().map(|p| p.to_string_lossy().into_owned()).collect(),
                },
                core::TransferState::Failed(failure) => TransferStatus::Failed {
                    reason: match failure {
                        core::TransferFailure::Denied => "denied",
                        core::TransferFailure::Unreachable => "unreachable",
                        core::TransferFailure::NoSpace => "noSpace",
                        core::TransferFailure::Interrupted => "interrupted",
                        core::TransferFailure::Integrity => "integrity",
                        core::TransferFailure::Other(_) => "other",
                    }
                    .into(),
                },
                core::TransferState::Cancelled => TransferStatus::Cancelled,
            },
            recording: t.recording,
            markers: t.markers.into_iter().map(Into::into).collect(),
            open_on_arrival: t.open_on_arrival,
        }
    }
}

fn file_to_send(file: FileToSend) -> Result<core::OutgoingFile> {
    Ok(match file {
        FileToSend::Path { name, path, folder } => {
            core::OutgoingFile { name, folder, source: core::FileSource::Path(path.into()) }
        }
        FileToSend::Fd { name, fd, folder } => {
            use std::os::fd::FromRawFd;
            if fd < 0 {
                return Err(G7LinkError::Internal { reason: "invalid file descriptor".into() });
            }
            // SAFETY: the caller hands over an open descriptor it no longer
            // uses (documented on `FileToSend::Fd`); the File closes it.
            #[allow(unsafe_code)]
            let file = unsafe { std::fs::File::from_raw_fd(fd) };
            core::OutgoingFile { name, folder, source: core::FileSource::File(file) }
        }
    })
}

/// Converts every item first so all valid descriptors are owned by `File` (and
/// closed on drop) even when one item or a later check fails.
fn files_to_send(files: Vec<FileToSend>) -> Result<Vec<core::OutgoingFile>> {
    let mut out = Vec::with_capacity(files.len());
    let mut first_err: Option<G7LinkError> = None;
    for f in files {
        match file_to_send(f) {
            Ok(outgoing) => out.push(outgoing),
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }
    if let Some(err) = first_err {
        return Err(err);
    }
    Ok(out)
}

fn upgrade(u: core::features::Upgrade) -> Upgrade {
    let (action, target) = u.action.describe();
    Upgrade { action: action.into(), target, minutes: u.effort.minutes() }
}

impl From<FeatureState> for FeatureStatus {
    fn from(state: FeatureState) -> Self {
        match state {
            FeatureState::Available => FeatureStatus::Available,
            FeatureState::Partial { limit, upgrade: u } => {
                FeatureStatus::Partial { limit: limit.into(), upgrade: u.map(upgrade) }
            }
            FeatureState::Locked { upgrade: u } => FeatureStatus::Locked { upgrade: upgrade(u) },
            FeatureState::Unsupported { reason } => FeatureStatus::Unsupported { reason: reason.describe() },
        }
    }
}

fn features(matrix: &CapabilityMatrix) -> Vec<Feature> {
    matrix.features.iter().map(|(id, state)| Feature { id: (*id).into(), status: (*state).into() }).collect()
}

impl Event {
    fn from_node(event: NodeEvent) -> Option<Self> {
        Some(match event {
            NodeEvent::DeviceAdded(device) => Event::DeviceAdded { device: device.into() },
            NodeEvent::DeviceRemoved(id) => Event::DeviceRemoved { id: id.to_string() },
            NodeEvent::LinkChanged { device, link } => {
                Event::LinkChanged { id: device.to_string(), link: link.into() }
            }
            NodeEvent::PeerInfoChanged { device, info } => {
                Event::PeerInfoChanged { id: device.to_string(), info: info.into() }
            }
            NodeEvent::PeerPowerChanged { device, power } => {
                Event::PeerPowerChanged { id: device.to_string(), power: power.into() }
            }
            NodeEvent::UpdateHint { device, channel, version } => {
                Event::UpdateHint { id: device.to_string(), channel, version }
            }
            NodeEvent::PeerNeedsUpdate { device } => Event::PeerNeedsUpdate { id: device.to_string() },
            NodeEvent::Battery { device, battery } => {
                Event::Battery { id: device.to_string(), battery: battery.into() }
            }
            NodeEvent::Ring { device, on } => Event::Ring { id: device.to_string(), on },
            NodeEvent::RemoteRing { device, on } => Event::RemoteRing { id: device.to_string(), on },
            NodeEvent::PcTextFocus { device, editable } => {
                Event::PcTextFocus { id: device.to_string(), editable }
            }
            NodeEvent::PcTextState { device, state } => {
                Event::PcTextState { id: device.to_string(), state: state.into() }
            }
            NodeEvent::PcClip { device, kind, preview } => {
                Event::PcClip { id: device.to_string(), kind: kind.into(), preview }
            }
            NodeEvent::Discovered(device) => Event::Discovered { device: device.into() },
            NodeEvent::DiscoveryExpired(id) => Event::DiscoveryExpired { id: id.to_string() },
            NodeEvent::Pairing(PairingEvent::SasCode { peer, code }) => {
                Event::PairingCode { peer: peer.to_string(), code }
            }
            NodeEvent::Pairing(PairingEvent::Paired(device)) => Event::Paired { device: device.into() },
            NodeEvent::Pairing(PairingEvent::Failed(failure)) => {
                Event::PairingFailed { failure: failure.into() }
            }
            NodeEvent::Capabilities(matrix) => {
                Event::Capabilities { id: matrix.device.to_string(), features: features(&matrix) }
            }
            NodeEvent::NotificationsReset { device, items } => Event::NotificationsReset {
                id: device.to_string(),
                items: items.into_iter().map(Into::into).collect(),
            },
            NodeEvent::NotificationPosted { device, notification } => {
                Event::NotificationPosted { id: device.to_string(), notification: notification.into() }
            }
            NodeEvent::NotificationRemoved { device, key } => {
                Event::NotificationRemoved { id: device.to_string(), key }
            }
            NodeEvent::ClipboardReceived { device } => Event::ClipboardReceived { id: device.to_string() },
            NodeEvent::ClipboardHistoryChanged => Event::ClipboardHistoryChanged,
            NodeEvent::Transfer(t) => Event::Transfer { transfer: t.into() },
            NodeEvent::MediaChanged { device, players } => Event::MediaChanged {
                id: device.to_string(),
                players: players.into_iter().map(Into::into).collect(),
            },
            NodeEvent::PhotoAdded { device, photo } => {
                Event::PhotoAdded { id: device.to_string(), photo_id: photo.id }
            }
            NodeEvent::Call { device, call } => {
                Event::CallChanged { id: device.to_string(), call_id: call.id }
            }
            NodeEvent::CallLogChanged { device } => Event::CallLogChanged { id: device.to_string() },
            NodeEvent::ContactsChanged { device } => Event::ContactsChanged { id: device.to_string() },
            NodeEvent::SmsChanged { device, thread } => Event::SmsChanged { id: device.to_string(), thread },
            NodeEvent::PhotosChanged { device } => Event::PhotosChanged { id: device.to_string() },
            NodeEvent::Mirroring { device, session, on } => {
                Event::Mirroring { id: device.to_string(), session, on }
            }
            NodeEvent::Webcam { device, on } => Event::Webcam { id: device.to_string(), on },
            NodeEvent::CameraCaptureRequested { .. }
            | NodeEvent::CameraCaptureReceived { .. }
            | NodeEvent::CameraCaptureCancelled { .. }
            | NodeEvent::PcMirrorRequested { .. }
            // A phone's microphone arrives on PCs only.
            | NodeEvent::Mic { .. } => return None,
            // What a PC says about sharing its screen; the rest is for PCs.
            NodeEvent::MirrorNotice { device, session, code } => Event::PcMirrorNotice {
                id: device.to_string(),
                notice: PcMirrorNoticeKind::from_code(session, &code)?,
            },
            NodeEvent::RemoteInputRequested { device } => {
                Event::RemoteInputRequested { id: device.to_string() }
            }
            NodeEvent::PcCapturePrompt { device } => Event::PcCapturePrompt { id: device.to_string() },
            NodeEvent::AgentFolderPrompt { device, place } => {
                Event::AgentFolderPrompt { id: device.to_string(), place }
            }
            NodeEvent::PcCaptureRecordEnded { device, file } => Event::PcCaptureRecordEnded {
                id: device.to_string(),
                transfer: file.as_ref().map(|f| f.transfer.clone()),
                name: file.map(|f| f.name),
            },
            NodeEvent::PhoneToggles { device, toggles } => {
                Event::PhoneToggles { id: device.to_string(), toggles: toggles.into() }
            }
            NodeEvent::WakeInfoChanged { device, can_wake } => {
                Event::WakeInfoChanged { id: device.to_string(), can_wake }
            }
            NodeEvent::ReconnectChanged { device, on } => {
                Event::ReconnectChanged { id: device.to_string(), on }
            }
            NodeEvent::DeviceToggleChanged { device, toggle, on } => {
                Event::DeviceToggleChanged { id: device.to_string(), toggle, on }
            }
            NodeEvent::DeckLayout { device, layout } => {
                Event::DeckLayout { id: device.to_string(), layout: layout.into() }
            }
            NodeEvent::DeckState { device, state } => {
                Event::DeckState { id: device.to_string(), state: state.into() }
            }
            NodeEvent::StorageRequested { device } => Event::StorageRequested { id: device.to_string() },
            NodeEvent::StorageChanged { device, path } => {
                Event::StorageChanged { id: device.to_string(), path }
            }
            NodeEvent::TimelineChanged => Event::TimelineChanged,
            NodeEvent::LocalSendChanged => Event::LocalSendChanged,
            NodeEvent::QueueChanged => Event::QueueChanged,
            NodeEvent::QueueDroppedOnUnpair { peer, peer_name, dropped_count } => {
                Event::QueueDroppedOnUnpair {
                    device_id: peer.to_string(),
                    device_name: peer_name,
                    dropped_count,
                }
            }
            NodeEvent::PcAudioFrame { device, sample_rate, channels, seq, ts_ms, pcm } => {
                Event::PcAudioFrame { device: device.to_string(), sample_rate, channels, seq, ts_ms, pcm }
            }
            NodeEvent::BleKeysChanged => Event::BleKeysChanged,
            NodeEvent::BleOutbound { endpoint } => Event::BleOutbound { endpoint },
            NodeEvent::BleLinkChanged { device, up } => Event::BleLinkChanged { id: device.to_string(), up },
            NodeEvent::RemoteRouteChanged { device, state } => {
                Event::RemoteRouteChanged { id: device.to_string(), state: state.into() }
            }
            NodeEvent::AgentCatalog { device, catalog, can_say_why, can_control } => Event::AgentCatalog {
                id: device.to_string(),
                catalog: catalog.map(|c| AgentCatalog::from_core(c, can_say_why, can_control)),
            },
            NodeEvent::AgentQuestion { device, question } => {
                Event::AgentQuestion { id: device.to_string(), question: question.into() }
            }
            NodeEvent::AgentQuestioned { device, token, thread, outcome, by_name } => Event::AgentQuestioned {
                id: device.to_string(),
                token,
                thread,
                outcome: AgentQuestionOutcome::from_wire(&outcome)?,
                by_name,
            },
            NodeEvent::AgentDraft { device, key, rev } => {
                Event::AgentDraftChanged { id: device.to_string(), key, rev }
            }
            NodeEvent::AgentEvent { device, event } => return agent_event(device.to_string(), *event),
            NodeEvent::AgentAsk { device, ask } => {
                Event::AgentAsk { id: device.to_string(), ask: AgentAsk::from_core(ask)? }
            }
            NodeEvent::AgentAsked { device, token, thread, outcome, by_name, message } => Event::AgentAsked {
                id: device.to_string(),
                token,
                thread,
                outcome: AgentAskOutcome::from_wire(&outcome)?,
                by_name,
                message,
            },
            NodeEvent::AgentSession { device, session: Some(session), .. } => Event::AgentSessionChanged {
                id: device.to_string(),
                session: AgentSession::from_core(session)?,
            },
            NodeEvent::AgentSession { device, session: None, removed } => {
                Event::AgentSessionRemoved { id: device.to_string(), session: removed? }
            }
            NodeEvent::AgentSessionItem { device, session, item } => {
                Event::AgentSessionItem { id: device.to_string(), session, item: AgentItem::from_core(item)? }
            }
            NodeEvent::AgentTurn { device, thread, turn } => {
                Event::AgentTurn { id: device.to_string(), thread, turn: turn.into() }
            }
            NodeEvent::AgentCut { device, cut } => Event::AgentCut {
                id: device.to_string(),
                thread: cut.thread,
                from_seq: cut.from_seq,
                to_seq: cut.to_seq,
                restored: cut.restored,
            },
            NodeEvent::AgentImport { device, progress } => {
                Event::AgentImport { id: device.to_string(), progress: progress.into() }
            }
            NodeEvent::AgentOutbox { device, held } => {
                Event::AgentOutbox { id: device.to_string(), held: held.into_iter().map(Into::into).collect() }
            }
            NodeEvent::AgentPresence { device, presence } => {
                Event::AgentPresence { id: device.to_string(), presence: presence.into() }
            }
            NodeEvent::AgentGit { device, change } => {
                let strip = AgentGitStrip::from(change.clone());
                Event::AgentGit {
                    id: device.to_string(),
                    thread: change.thread,
                    branch: strip.branch,
                    detached: strip.detached,
                    changed: strip.changed,
                    ahead: strip.ahead,
                    behind: strip.behind,
                    pr: strip.pr,
                }
            }
            NodeEvent::AgentGitJob { device, job } => {
                Event::AgentGitJob { id: device.to_string(), job: AgentJob::from_core(job)? }
            }
            NodeEvent::AgentHandoff { device, handoff } => Event::AgentHandoff {
                id: device.to_string(),
                thread: handoff.thread,
                seq: handoff.seq,
                offer: handoff.offer,
                expires_ms: handoff.expires_ms,
            },
            NodeEvent::AgentBeside { device, thread, pane } => {
                Event::AgentBeside { id: device.to_string(), thread, pane: AgentPane::from_wire(&pane)? }
            }
            NodeEvent::AgentScriptRun { device, run } => {
                Event::AgentScriptRun { id: device.to_string(), run: AgentScriptRun::from_core(run)? }
            }
            // Shown on PCs only.
            NodeEvent::AgentsRequested { .. }
            | NodeEvent::AgentSessionsRequested { .. }
            | NodeEvent::AgentReplyRequested { .. }
            | NodeEvent::AgentViewing { .. } => return None,
            // Asked for by PCs only, and codes go to PCs.
            NodeEvent::BleWanted { .. }
            | NodeEvent::OneTimeCode { .. }
            | NodeEvent::TextWatch { .. }
            | NodeEvent::BleLinkRequested { .. }
            | NodeEvent::BleHoldChanged { .. } => return None,
        })
    }
}

fn parse_id(id: &str) -> Result<DeviceId> {
    id.parse().map_err(|_| G7LinkError::InvalidDeviceId)
}

// ---- Foreign interfaces (implemented in Kotlin) ----

/// Why a notification couldn't be dismissed or its action run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum NotificationFailure {
    /// The notification or action is gone.
    #[error("no longer exists")]
    NotFound,
    /// Not possible here (e.g. notification access was revoked).
    #[error("not available")]
    Unsupported,
    /// `reason` is for logs; never put notification content in it.
    #[error("failed: {reason}")]
    Failed { reason: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for NotificationFailure {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        NotificationFailure::Failed { reason: e.reason }
    }
}

impl From<NotificationFailure> for core::NotificationError {
    fn from(f: NotificationFailure) -> Self {
        match f {
            NotificationFailure::NotFound => core::NotificationError::NotFound,
            NotificationFailure::Unsupported => core::NotificationError::Unsupported,
            NotificationFailure::Failed { reason } => core::NotificationError::Failed(reason),
        }
    }
}

/// Why a media command didn't run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum MediaFailure {
    /// The player is gone.
    #[error("no such player")]
    NotFound,
    /// The player can't do that (or media access was revoked).
    #[error("not available")]
    Unsupported,
    /// `reason` is for logs.
    #[error("failed: {reason}")]
    Failed { reason: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for MediaFailure {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        MediaFailure::Failed { reason: e.reason }
    }
}

impl From<MediaFailure> for core::MediaError {
    fn from(f: MediaFailure) -> Self {
        match f {
            MediaFailure::NotFound => core::MediaError::NotFound,
            MediaFailure::Unsupported => core::MediaError::Unsupported,
            MediaFailure::Failed { reason } => core::MediaError::Failed(reason),
        }
    }
}

/// Why a phone storage operation failed (`docs/protocol/storage.md`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum StorageFailure {
    #[error("no longer exists")]
    NotFound,
    #[error("access denied")]
    Denied,
    #[error("invalid path: {reason}")]
    Invalid { reason: String },
    #[error("not enough space")]
    NoSpace,
    #[error("not available")]
    Unsupported,
    #[error("failed: {reason}")]
    Failed { reason: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for StorageFailure {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        StorageFailure::Failed { reason: e.reason }
    }
}

impl From<StorageFailure> for core::StorageError {
    fn from(f: StorageFailure) -> Self {
        match f {
            StorageFailure::NotFound => core::StorageError::NotFound,
            StorageFailure::Denied => core::StorageError::Denied,
            StorageFailure::Invalid { reason } => core::StorageError::Invalid(reason),
            StorageFailure::NoSpace => core::StorageError::NoSpace,
            StorageFailure::Unsupported => core::StorageError::Unsupported,
            StorageFailure::Failed { reason } => core::StorageError::Failed(reason),
        }
    }
}

/// What the app does for the core: ring the phone, act on its
/// notifications.
#[uniffi::export(with_foreign)]
pub trait Platform: Send + Sync {
    /// Ring loudly, even in silent mode, until `stop_ringing`.
    fn start_ringing(&self);
    fn stop_ringing(&self);
    /// A PC dismissed this notification.
    fn dismiss_notification(&self, key: String) -> Result<(), NotificationFailure>;
    /// A PC ran this notification's action; `reply` is the text for a reply
    /// action.
    fn run_notification_action(
        &self,
        key: String,
        action: String,
        reply: Option<String>,
    ) -> Result<(), NotificationFailure>;
    /// A PC sent this text: put it on the clipboard. False if that failed.
    fn set_clipboard(&self, text: String) -> bool;
    /// A PC sent this text (with optional HTML and RTF rich text): put it on
    /// the clipboard. False if that failed.
    fn set_clipboard_rich(&self, text: String, html: Option<String>, rtf: Option<String>) -> bool;
    /// A PC sent a password or the like: put it on the clipboard marked
    /// sensitive. False if that failed.
    fn set_clipboard_private(&self, text: String) -> bool;
    /// Takes that password off the clipboard, if it's still what's there.
    fn clear_clipboard_private(&self, text: String);
    /// A PC sent this image (`image/png` or `image/jpeg`): put it on the
    /// clipboard. False if that failed.
    fn set_clipboard_image(&self, mime: String, bytes: Vec<u8>) -> bool;
    /// A paired device asked one of this phone's media players to do
    /// something; `position` (milliseconds) is set for seeking.
    fn media_command(
        &self,
        player: String,
        action: MediaAction,
        position: Option<u64>,
    ) -> Result<(), MediaFailure>;
    /// A paired PC sent a web link (http or https, checked) to open.
    /// False if it couldn't be shown.
    fn open_link(&self, from_id: String, url: String) -> bool;
    /// Play on phone: a paired PC offers to stream what its `player` plays
    /// (`title`). Show the offer; taking it starts PC mirroring with
    /// `watch` set to `player`, or with `file`, plays that video file read
    /// with `watch_file_read`. False if it couldn't be shown.
    fn watch_offered(
        &self,
        from_id: String,
        player: String,
        title: Option<String>,
        file: Option<WatchFileOffer>,
    ) -> bool;
    /// A paired PC sent an ongoing or completed task notification (`task.notify`).
    /// False if it couldn't be shown.
    fn task_notify(&self, from_id: String, task: TaskNotify) -> bool;
    /// A PC asked for a photo or video from the gallery: open it, or
    /// `null` when it's gone.
    fn open_photo(&self, id: String) -> Option<FileToSend>;
    /// A PC asked for this phone's photo albums (folders), Camera and
    /// Screenshots first, then by count.
    fn photo_albums(&self) -> Vec<PhotoAlbum>;
    /// A PC asked for photos and videos in `album` (or all when null),
    /// newest first by date, then by media ID (highest first). When
    /// `before` is set, only items after the previous page's last one:
    /// older than `before`, or of that date with an ID below `before_id`
    /// (none of that date when `before_id` is empty).
    fn photo_list(
        &self,
        album: Option<String>,
        before: Option<i64>,
        before_id: String,
        limit: u32,
    ) -> Vec<PhotoItem>;
    /// A PC asked for small JPEG thumbnails (at most 32 KiB each) for
    /// these gallery item IDs.
    fn photo_thumbs(&self, ids: Vec<String>) -> Vec<PhotoThumb>;
    /// A PC asked to move these gallery items to the trash. Returns how many went.
    fn photo_delete(&self, ids: Vec<String>) -> u32;
    /// A PC asked to answer, decline or silence call `id` (the call in
    /// progress). False if the phone couldn't.
    fn call_command(&self, id: String, command: CallCommand) -> bool;
    /// A PC asked for recent calls before `before` (Unix ms; the latest when
    /// null), newest first.
    fn call_log(&self, before: Option<i64>, limit: u32) -> Vec<CallLogEntry>;
    /// A PC asked to call `number` (or open the dialer with it filled in).
    /// False if the phone couldn't.
    fn call_dial(&self, number: String) -> bool;
    /// A PC asked which SIMs this phone calls with.
    fn call_sims(&self) -> Vec<CallSim>;
    /// Like `call_dial`, on the SIM `sim` (an id from `call_sims`).
    fn call_dial_on(&self, number: String, sim: String) -> bool;
    /// A PC asked for contacts matching `query` (or all with phone numbers
    /// when null), favorites first then alphabetical, skipping `offset`.
    fn contacts(&self, query: Option<String>, offset: u32, limit: u32) -> Vec<Contact>;
    /// A PC asked for this phone's screen (and, with `audio`, its sound):
    /// ask the user (then call `mirror_open`, and `mirror_open_audio` for
    /// the sound). With `app` (and a `session` other than 0): run that app
    /// on a display of its own and stream it with `mirror_open`, its config
    /// packets carrying the session. False if it can't.
    fn mirror_requested(&self, pc_id: String, options: MirrorOptions) -> bool;
    /// The PC stopped watching the screen (session 0) or an app window.
    fn mirror_stop_requested(&self, pc_id: String, session: u32);
    /// The PC needs a keyframe.
    fn mirror_keyframe_requested(&self, pc_id: String, session: u32);
    /// The PC resized an app window (`session` != 0) to `width × height`.
    fn mirror_resize_requested(&self, pc_id: String, session: u32, width: u32, height: u32);
    /// The PC updated `stay_awake` or `screen_off` while mirroring the screen (`session == 0`).
    fn mirror_power_requested(&self, pc_id: String, stay_awake: bool, screen_off: bool);
    /// The PC wants the phone's sound sent to it, or left on the phone, while
    /// mirroring the screen.
    fn mirror_sound_requested(&self, pc_id: String, on: bool);
    /// The PC's mouse or keyboard on the mirrored screen (only while this
    /// phone offers `mirror.input`) or an app window. Return quickly.
    fn mirror_input(&self, pc_id: String, session: u32, input: MirrorInputEvent);
    /// Whether the phone's lock screen is up: a PC then opens no app windows
    /// on it. Return quickly.
    fn phone_locked(&self) -> bool;
    /// A PC asked for this phone's camera as a webcam: start streaming (or
    /// ask the user) and call `webcam_open`. False if it can't or the user
    /// declined.
    fn webcam_requested(&self, pc_id: String, options: WebcamOptions) -> bool;
    /// The PC stopped watching the webcam stream.
    fn webcam_stop_requested(&self, pc_id: String);
    /// A PC asked for this phone's microphone (`docs/protocol/mic.md`): ask
    /// the user, then stream with `mic_open`. False if it couldn't ask.
    fn mic_requested(&self, pc_id: String) -> bool;
    /// The PC stopped listening to this phone's microphone: stop recording.
    fn mic_stop_requested(&self, pc_id: String);
    /// The PC needs a fresh H.264 keyframe on the webcam stream.
    fn webcam_keyframe_requested(&self, pc_id: String);
    /// A PC asked this phone to capture a photo or scan a document (`mode` is
    /// `"photo"` or `"scan"`). Return `true` if the capture UI was launched.
    fn camera_capture_requested(&self, pc_id: String, request_id: String, mode: String) -> bool;
    /// The PC cancelled an in-flight Continuity Camera capture request.
    fn camera_capture_cancelled(&self, pc_id: String, request_id: String);
    /// A PC asked for the apps it may open in windows (launchable ones).
    fn phone_apps(&self) -> Vec<PhoneApp>;
    /// A PC asked for the latest conversations, newest first.
    fn sms_threads(&self, limit: u32) -> Vec<SmsThread>;
    /// The default SMS app's package name, sent with the conversations so a
    /// PC can show its icon. Null when unknown.
    fn sms_app(&self) -> Option<String>;
    /// A PC asked for a conversation's messages before `before` (Unix ms;
    /// the latest when null), newest first.
    fn sms_messages(&self, thread: String, before: Option<i64>, limit: u32) -> Vec<SmsMessage>;
    /// A PC asked to send a text (checked: 1-20 recipients, not empty).
    /// False if it couldn't be sent.
    fn sms_send(&self, to: Vec<String>, body: String) -> bool;
    /// A PC asked to send a text or MMS with optional image attachments.
    /// False if it couldn't be sent.
    fn send_sms_with_attachments(
        &self,
        to: Vec<String>,
        body: String,
        attachments: Vec<SmsAttachment>,
    ) -> bool;
    /// A PC asked for a message's attachment; null when it's gone.
    fn sms_part(&self, id: String) -> Option<SmsPartData>;
    /// A PC asked to change one of this phone's quick settings (`id`: `"dnd"`,
    /// `"ringer"`, `"flashlight"`, `"volume"`, `"brightness"`, `"wifi"`,
    /// `"bluetooth"`). False if the phone couldn't.
    fn set_phone_toggle(&self, id: String, value: PhoneToggleValue) -> bool;
    /// A PC asked to list a folder in this phone's storage (`""` is root).
    fn storage_list(&self, path: String) -> Result<Vec<StorageEntry>, StorageFailure>;
    /// A PC asked to open a file in this phone's storage for ranged reading.
    fn storage_open_read(&self, path: String) -> Result<StorageReadFile, StorageFailure>;
    /// A PC finished uploading a file to `staged_path`: move/copy it into
    /// `path` in this phone's storage.
    fn storage_write(
        &self,
        path: String,
        staged_path: String,
        modified: Option<i64>,
    ) -> Result<StorageWriteDone, StorageFailure>;
    /// A PC asked to create a folder at `path` in this phone's storage.
    fn storage_mkdir(&self, path: String) -> Result<(), StorageFailure>;
    /// A PC asked to rename or move `from` to `to` in this phone's storage.
    fn storage_rename(&self, from: String, to: String) -> Result<(), StorageFailure>;
    /// A PC asked to delete `path` in this phone's storage (`confirmed`: the
    /// PC user confirmed permanent deletion if trash isn't available).
    fn storage_delete(&self, path: String, confirmed: bool) -> Result<(), StorageFailure>;
}

/// Encrypts the device key at rest (Android: a Keystore key).
#[uniffi::export(with_foreign)]
pub trait KeyProtector: Send + Sync {
    fn protect(&self, plaintext: Vec<u8>) -> Result<Vec<u8>>;
    fn unprotect(&self, ciphertext: Vec<u8>) -> Result<Vec<u8>>;
}

/// Receives core events on a core thread. Hand them off; don't block.
#[uniffi::export(with_foreign)]
pub trait EventListener: Send + Sync {
    fn on_event(&self, event: Event);
}

struct FfiMirrorSink {
    id: String,
    listener: Arc<dyn EventListener>,
    /// The next packet's `seq`: 0 after each config.
    seq: std::sync::atomic::AtomicU64,
    clocks: Arc<PcMirrorClocks>,
}

impl std::fmt::Debug for FfiMirrorSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FfiMirrorSink").field("id", &self.id).finish_non_exhaustive()
    }
}

impl FfiMirrorSink {
    /// A sink for a new stream from PC `id`: its clock isn't known until timed again.
    fn new(id: String, listener: Arc<dyn EventListener>, clocks: Arc<PcMirrorClocks>) -> Self {
        clocks.set(&id, None);
        Self { id, listener, seq: std::sync::atomic::AtomicU64::new(0), clocks }
    }
}

impl core::MirrorSink for FfiMirrorSink {
    fn config(&self, config: core::MirrorConfig) {
        // The core hands one stream's packets over in order, from one task.
        self.seq.store(0, std::sync::atomic::Ordering::Relaxed);
        self.listener.on_event(Event::PcMirrorConfig {
            id: self.id.clone(),
            codec: config.codec,
            width: config.width,
            height: config.height,
        });
    }

    fn packet(&self, keyframe: bool, time_us: u64, data: Vec<u8>) {
        let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.listener.on_event(Event::PcMirrorFrame { id: self.id.clone(), seq, keyframe, time_us, data });
    }

    fn ended(&self) {
        self.listener.on_event(Event::PcMirrorEnded { id: self.id.clone() });
    }

    fn clock(&self, offset_us: i64, rtt_us: u32) {
        self.clocks.set(&self.id, Some(PcMirrorClock { offset_us, rtt_us }));
    }
}

struct PlatformAdapter(Arc<dyn Platform>, Arc<dyn EventListener>, Arc<PcMirrorClocks>);

impl std::fmt::Debug for PlatformAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlatformAdapter")
    }
}

impl core::Platform for PlatformAdapter {
    fn start_ringing(&self) {
        self.0.start_ringing();
    }
    fn stop_ringing(&self) {
        self.0.stop_ringing();
    }
    fn dismiss_notification(&self, key: &str) -> Result<(), core::NotificationError> {
        Ok(self.0.dismiss_notification(key.to_owned())?)
    }
    fn run_notification_action(
        &self,
        key: &str,
        action: &str,
        reply: Option<&str>,
    ) -> Result<(), core::NotificationError> {
        Ok(self.0.run_notification_action(key.to_owned(), action.to_owned(), reply.map(str::to_owned))?)
    }
    fn set_clipboard(&self, text: &str) -> Result<(), String> {
        if self.0.set_clipboard(text.to_owned()) { Ok(()) } else { Err("the clipboard rejected it".into()) }
    }
    fn set_clipboard_private(&self, text: &str) -> Result<(), String> {
        if self.0.set_clipboard_private(text.to_owned()) {
            Ok(())
        } else {
            Err("the phone couldn't set the clipboard".to_owned())
        }
    }
    fn clear_clipboard_private(&self, text: &str) {
        self.0.clear_clipboard_private(text.to_owned());
    }
    fn set_clipboard_rich(&self, text: &str, html: Option<&str>, rtf: Option<&str>) -> Result<(), String> {
        if self.0.set_clipboard_rich(text.to_owned(), html.map(str::to_owned), rtf.map(str::to_owned)) {
            Ok(())
        } else {
            Err("the clipboard rejected it".into())
        }
    }
    fn media_command(
        &self,
        player: &str,
        action: core::MediaAction,
        position: Option<u64>,
        _value: Option<&str>,
    ) -> Result<(), core::MediaError> {
        // A phone shares no player with shuffle or repeat (it doesn't offer them).
        if matches!(action, core::MediaAction::Shuffle | core::MediaAction::Loop) {
            return Err(core::MediaError::Unsupported);
        }
        Ok(self.0.media_command(player.to_owned(), action.into(), position)?)
    }
    fn open_link(&self, from: &DeviceId, url: &str, _user_shared: bool) -> Result<(), String> {
        if self.0.open_link(from.to_string(), url.to_owned()) { Ok(()) } else { Err("not shown".into()) }
    }
    fn watch_offered(
        &self,
        from: &DeviceId,
        player: &str,
        title: Option<&str>,
        file: Option<&core::WatchFile>,
    ) -> Result<(), String> {
        let file =
            file.map(|f| WatchFileOffer { name: f.name.clone(), size: f.size, position_ms: f.position });
        if self.0.watch_offered(from.to_string(), player.to_owned(), title.map(str::to_owned), file) {
            Ok(())
        } else {
            Err("not shown".into())
        }
    }
    fn task_notify(&self, from: &DeviceId, task: &core::TaskNotify) -> Result<(), String> {
        if self.0.task_notify(from.to_string(), task.clone().into()) {
            Ok(())
        } else {
            Err("not shown".into())
        }
    }
    fn set_clipboard_image(&self, mime: &str, bytes: &[u8]) -> Result<(), String> {
        if self.0.set_clipboard_image(mime.to_owned(), bytes.to_vec()) {
            Ok(())
        } else {
            Err("the clipboard rejected it".into())
        }
    }
    fn mirror_requested(&self, peer: &DeviceId, options: &core::MirrorStart) -> Result<(), String> {
        let options = MirrorOptions {
            max_size: options.max_size,
            fps: options.fps,
            bitrate: options.bitrate,
            audio: options.audio,
            session: options.session,
            app: options.app.clone(),
            stay_awake: options.stay_awake,
            screen_off: options.screen_off,
            sound_only: options.sound_only,
        };
        if self.0.mirror_requested(peer.to_string(), options) {
            Ok(())
        } else {
            Err("the phone couldn't".into())
        }
    }
    fn mirror_stop_requested(&self, peer: &DeviceId, session: u32) {
        self.0.mirror_stop_requested(peer.to_string(), session);
    }
    fn mirror_keyframe_requested(&self, peer: &DeviceId, session: u32) {
        self.0.mirror_keyframe_requested(peer.to_string(), session);
    }
    fn mirror_resize_requested(&self, peer: &DeviceId, session: u32, width: u32, height: u32) {
        self.0.mirror_resize_requested(peer.to_string(), session, width, height);
    }
    fn mirror_power_requested(&self, peer: &DeviceId, stay_awake: bool, screen_off: bool) {
        self.0.mirror_power_requested(peer.to_string(), stay_awake, screen_off);
    }
    fn mirror_sound_requested(&self, peer: &DeviceId, on: bool) {
        self.0.mirror_sound_requested(peer.to_string(), on);
    }
    fn mirror_input(&self, peer: &DeviceId, session: u32, input: core::MirrorInput) {
        self.0.mirror_input(peer.to_string(), session, input.into());
    }
    fn phone_locked(&self) -> bool {
        self.0.phone_locked()
    }
    fn webcam_requested(&self, peer: &DeviceId, options: &core::WebcamStart) -> Result<(), String> {
        let options = WebcamOptions {
            camera: options.camera.clone(),
            width: options.width,
            height: options.height,
            fps: options.fps,
            bitrate: options.bitrate,
        };
        if self.0.webcam_requested(peer.to_string(), options) {
            Ok(())
        } else {
            Err("the phone couldn't".into())
        }
    }
    fn webcam_stop_requested(&self, peer: &DeviceId) {
        self.0.webcam_stop_requested(peer.to_string());
    }
    fn mic_requested(&self, peer: &DeviceId) -> Result<(), String> {
        if self.0.mic_requested(peer.to_string()) { Ok(()) } else { Err("not asked".into()) }
    }
    fn mic_stop_requested(&self, peer: &DeviceId) {
        self.0.mic_stop_requested(peer.to_string());
    }
    fn webcam_keyframe_requested(&self, peer: &DeviceId) {
        self.0.webcam_keyframe_requested(peer.to_string());
    }
    fn camera_capture_requested(
        &self,
        peer: &DeviceId,
        request: &core::CameraCaptureRequest,
    ) -> Result<(), String> {
        let mode = match request.mode {
            core::CameraCaptureMode::Photo => "photo",
            core::CameraCaptureMode::Scan => "scan",
        };
        if self.0.camera_capture_requested(peer.to_string(), request.request_id.clone(), mode.to_owned()) {
            Ok(())
        } else {
            Err("camera capture unavailable".into())
        }
    }
    fn camera_capture_cancelled(&self, peer: &DeviceId, request_id: &str) {
        self.0.camera_capture_cancelled(peer.to_string(), request_id.to_owned());
    }
    fn phone_apps(&self) -> Result<Vec<core::PhoneApp>, String> {
        Ok(self
            .0
            .phone_apps()
            .into_iter()
            .map(|a| core::PhoneApp { pkg: a.pkg, label: a.label, icon: a.icon })
            .collect())
    }
    fn sms_app(&self) -> Option<String> {
        self.0.sms_app()
    }
    fn sms_threads(&self, limit: u32) -> Result<Vec<core::SmsThread>, String> {
        Ok(self
            .0
            .sms_threads(limit)
            .into_iter()
            .map(|t| core::SmsThread {
                id: t.id,
                addresses: t.addresses,
                names: t.names,
                snippet: t.snippet,
                date: t.date,
                unread: t.unread,
                photo: t.photo,
            })
            .collect())
    }
    fn sms_messages(
        &self,
        thread: &str,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<core::SmsMessage>, String> {
        Ok(self
            .0
            .sms_messages(thread.to_owned(), before, limit)
            .into_iter()
            .map(|m| core::SmsMessage {
                id: m.id,
                thread: m.thread,
                address: m.address,
                body: m.body,
                date: m.date,
                outgoing: m.outgoing,
                status: m.status,
                parts: m
                    .parts
                    .into_iter()
                    .map(|p| core::SmsPart { id: p.id, mime: p.mime, size: p.size })
                    .collect(),
            })
            .collect())
    }
    fn sms_send(&self, to: &[String], body: &str) -> Result<(), String> {
        if self.0.sms_send(to.to_vec(), body.to_owned()) { Ok(()) } else { Err("not sent".into()) }
    }
    fn send_sms_with_attachments(
        &self,
        to: &[String],
        body: &str,
        attachments: &[core::SmsAttachment],
    ) -> Result<(), String> {
        if attachments.is_empty() {
            return self.sms_send(to, body);
        }
        let att: Vec<SmsAttachment> =
            attachments.iter().cloned().map(|a| SmsAttachment { mime: a.mime, data: a.data }).collect();
        if self.0.send_sms_with_attachments(to.to_vec(), body.to_owned(), att) {
            Ok(())
        } else {
            Err("not sent".into())
        }
    }
    fn sms_part(&self, id: &str) -> Result<(String, Vec<u8>), String> {
        let part = self.0.sms_part(id.to_owned()).ok_or("it's gone")?;
        Ok((part.mime, part.data))
    }
    fn call_command(&self, id: &str, command: core::CallCommand) -> Result<(), String> {
        let command = match command {
            core::CallCommand::Answer => CallCommand::Answer,
            core::CallCommand::Decline => CallCommand::Decline,
            core::CallCommand::Silence => CallCommand::Silence,
            core::CallCommand::Mute(on) => CallCommand::Mute { on },
            core::CallCommand::Speaker(on) => CallCommand::Speaker { on },
            core::CallCommand::Hold(on) => CallCommand::Hold { on },
            core::CallCommand::Dtmf(digit) => CallCommand::Dtmf { digit: digit.to_string() },
            core::CallCommand::Volume(up) => CallCommand::Volume { up },
        };
        if self.0.call_command(id.to_owned(), command) { Ok(()) } else { Err("the phone couldn't".into()) }
    }
    fn call_log(&self, before: Option<i64>, limit: u32) -> Result<Vec<core::CallLogEntry>, String> {
        Ok(self
            .0
            .call_log(before, limit)
            .into_iter()
            .map(|e| core::CallLogEntry {
                id: e.id,
                number: e.number,
                name: e.name,
                direction: e.direction,
                date: e.date,
                duration: e.duration,
                photo: e.photo,
            })
            .collect())
    }
    fn call_dial(&self, number: &str) -> Result<(), String> {
        if self.0.call_dial(number.to_owned()) { Ok(()) } else { Err("the phone couldn't".into()) }
    }
    fn call_sims(&self) -> Result<core::CallSims, String> {
        let sims = self.0.call_sims();
        let default = sims.iter().find(|s| s.is_default).map(|s| s.id.clone());
        Ok(core::CallSims {
            sims: sims.into_iter().map(|s| core::CallSim { id: s.id, label: s.label }).collect(),
            default,
        })
    }
    fn call_dial_on(&self, number: &str, sim: &str) -> Result<(), String> {
        if self.0.call_dial_on(number.to_owned(), sim.to_owned()) {
            Ok(())
        } else {
            Err("the phone couldn't".into())
        }
    }
    fn contacts(&self, query: Option<&str>, offset: u32, limit: u32) -> Result<Vec<core::Contact>, String> {
        Ok(self
            .0
            .contacts(query.map(str::to_owned), offset, limit)
            .into_iter()
            .map(|c| core::Contact {
                id: c.id,
                name: c.name,
                numbers: c
                    .numbers
                    .into_iter()
                    .map(|n| core::ContactNumber { number: n.number, label: n.label })
                    .collect(),
                starred: c.starred,
                photo: c.photo,
            })
            .collect())
    }
    fn open_photo(&self, id: &str) -> Result<core::OutgoingFile, String> {
        let file = self.0.open_photo(id.to_owned()).ok_or("it's gone")?;
        file_to_send(file).map_err(|e| e.to_string())
    }
    fn photo_albums(&self) -> Result<Vec<core::PhotoAlbum>, String> {
        Ok(self
            .0
            .photo_albums()
            .into_iter()
            .map(|a| core::PhotoAlbum { id: a.id, name: a.name, count: a.count, cover: a.cover })
            .collect())
    }
    fn photo_list(
        &self,
        album: Option<&str>,
        before: Option<(i64, &str)>,
        limit: u32,
    ) -> Result<Vec<core::PhotoItem>, String> {
        let (date, id) = before.map_or((None, String::new()), |(date, id)| (Some(date), id.to_owned()));
        Ok(self
            .0
            .photo_list(album.map(str::to_owned), date, id, limit)
            .into_iter()
            .map(|i| core::PhotoItem {
                id: i.id,
                name: i.name,
                date: i.date,
                size: i.size,
                width: i.width,
                height: i.height,
                duration: i.duration,
                album: i.album,
            })
            .collect())
    }
    fn photo_delete(&self, ids: &[String]) -> Result<u32, String> {
        Ok(self.0.photo_delete(ids.to_vec()))
    }
    fn photo_thumbs(&self, ids: &[String]) -> Result<Vec<core::PhotoThumb>, String> {
        Ok(self
            .0
            .photo_thumbs(ids.to_vec())
            .into_iter()
            .map(|t| core::PhotoThumb { id: t.id, data: t.data })
            .collect())
    }
    fn set_phone_toggle(&self, id: &str, value: &core::PhoneToggleValue) -> Result<(), String> {
        let value = match value {
            core::PhoneToggleValue::Bool(on) => PhoneToggleValue::Bool { on: *on },
            core::PhoneToggleValue::Level(level) => PhoneToggleValue::Level { level: *level },
            core::PhoneToggleValue::Mode(mode) => PhoneToggleValue::Mode { mode: mode.clone() },
        };
        if self.0.set_phone_toggle(id.to_owned(), value) { Ok(()) } else { Err("the phone couldn't".into()) }
    }
    fn storage_list(&self, path: &str) -> Result<Vec<core::StorageEntry>, core::StorageError> {
        let items = self.0.storage_list(path.to_owned())?;
        Ok(items
            .into_iter()
            .map(|e| core::StorageEntry {
                name: e.name,
                size: e.size,
                modified: e.modified,
                is_dir: e.is_dir,
            })
            .collect())
    }
    fn storage_open_read(&self, path: &str) -> Result<core::StorageReadFile, core::StorageError> {
        let file = self.0.storage_open_read(path.to_owned())?;
        let outgoing = file_to_send(file.source).map_err(|e| core::StorageError::Failed(e.to_string()))?;
        Ok(core::StorageReadFile { source: outgoing.source, size: file.size, modified: file.modified })
    }
    fn storage_write(
        &self,
        path: &str,
        staged: &std::path::Path,
        modified: Option<i64>,
    ) -> Result<core::StorageWriteDone, core::StorageError> {
        let staged_str = staged.to_string_lossy().into_owned();
        let done = self.0.storage_write(path.to_owned(), staged_str, modified)?;
        Ok(core::StorageWriteDone { size: done.size, modified: done.modified })
    }
    fn storage_mkdir(&self, path: &str) -> Result<(), core::StorageError> {
        Ok(self.0.storage_mkdir(path.to_owned())?)
    }
    fn storage_rename(&self, from: &str, to: &str) -> Result<(), core::StorageError> {
        Ok(self.0.storage_rename(from.to_owned(), to.to_owned())?)
    }
    fn storage_delete(&self, path: &str, confirmed: bool) -> Result<(), core::StorageError> {
        Ok(self.0.storage_delete(path.to_owned(), confirmed)?)
    }
    fn mirror_sink(&self, peer: &DeviceId, session: u32) -> Option<Arc<dyn core::MirrorSink>> {
        if session != core::MIRROR_SCREEN {
            return None;
        }
        Some(Arc::new(FfiMirrorSink::new(peer.to_string(), self.1.clone(), self.2.clone())))
    }
}

struct KeyProtectorAdapter(Arc<dyn KeyProtector>);

/// Key files written with the Android Keystore protector.
const ANDROID_KEYSTORE_PROTECTOR: u8 = 2;

impl core::KeyProtector for KeyProtectorAdapter {
    fn id(&self) -> u8 {
        ANDROID_KEYSTORE_PROTECTOR
    }
    fn protect(&self, plaintext: &[u8]) -> std::io::Result<Vec<u8>> {
        self.0.protect(plaintext.to_vec()).map_err(std::io::Error::other)
    }
    fn unprotect(&self, ciphertext: &[u8]) -> std::io::Result<Vec<u8>> {
        self.0.unprotect(ciphertext.to_vec()).map_err(std::io::Error::other)
    }
}

// ---- The node ----

#[derive(Debug, Clone, uniffi::Record)]
pub struct NodeOptions {
    /// App-private directory for the identity and database.
    pub data_dir: String,
    pub device: DeviceInfo,
    pub app_version: String,
    pub power: PowerLevel,
    /// Allow connections through relays when away from home.
    pub away_mode: bool,
    /// Where received files are put once complete (the app then moves them
    /// where the user finds them).
    pub downloads_dir: String,
    /// Loopback TCP port (`127.0.0.1:<port>`) for the USB tunnel on Android.
    #[uniffi(default = Some(53319))]
    pub usb_port: Option<u16>,
}

#[derive(Debug, uniffi::Enum)]
pub enum RelayRegion {
    NorthAmericaEast,
    NorthAmericaWest,
    Europe,
    AsiaPacific,
    Custom,
}

#[derive(Debug, uniffi::Record)]
pub struct RelayServer {
    pub url: String,
    pub provider: String,
    pub region: RelayRegion,
    pub enabled: bool,
}

#[derive(Debug, uniffi::Record)]
pub struct HomeRelay {
    pub url: String,
    pub connected: bool,
    pub failed: bool,
}

#[derive(Debug, uniffi::Record)]
pub struct RelayRoute {
    pub device: String,
    pub url: String,
}

#[derive(Debug, uniffi::Record)]
pub struct RelaySnapshot {
    pub servers: Vec<RelayServer>,
    pub home: Vec<HomeRelay>,
    pub routes: Vec<RelayRoute>,
}

#[derive(Debug, uniffi::Record)]
pub struct RelayProbe {
    pub url: String,
    pub rtt_ms: Option<u32>,
    pub timed_out: bool,
}

#[derive(Debug, uniffi::Enum)]
pub enum RelayEdit {
    Add,
    Enable,
    Disable,
    Remove,
}

#[derive(Debug, uniffi::Enum)]
pub enum RelayEditResult {
    Saved,
    InvalidAddress,
    Duplicate,
    Limit,
    Failed,
}

/// What saving an address typed for a PC came to (`set_peer_address`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PeerAddressResult {
    /// The address was removed.
    Cleared,
    /// Saved and in use.
    Ready,
    /// Saved, but the name has no answer right now.
    NotFound,
    /// Saved, but used only with Away mode on.
    NotAllowed,
    /// Not an address or a name; nothing saved.
    Invalid,
}

impl From<core::reach::PeerAddressOutcome> for PeerAddressResult {
    fn from(outcome: core::reach::PeerAddressOutcome) -> Self {
        use core::reach::PeerAddressOutcome as O;
        match outcome {
            O::Cleared => PeerAddressResult::Cleared,
            O::Ready => PeerAddressResult::Ready,
            O::NotFound => PeerAddressResult::NotFound,
            O::NotAllowed => PeerAddressResult::NotAllowed,
            O::Invalid => PeerAddressResult::Invalid,
        }
    }
}

/// How a PC would be reached away from home, from what the phone knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AwayReach {
    Tailscale,
    MobileData,
    TailscaleOff,
    HomeOnly,
}

impl From<core::reach::AwayReach> for AwayReach {
    fn from(reach: core::reach::AwayReach) -> Self {
        use core::reach::AwayReach as R;
        match reach {
            R::Tailscale => AwayReach::Tailscale,
            R::MobileData => AwayReach::MobileData,
            R::TailscaleOff => AwayReach::TailscaleOff,
            R::HomeOnly => AwayReach::HomeOnly,
        }
    }
}

/// The G7 Link engine. One per app process.
#[derive(uniffi::Object)]
pub struct G7LinkNode {
    runtime: Runtime,
    node: Node,
    /// The clocks of PCs whose screens show here (`pc_mirror_clock`).
    pc_mirror_clocks: Arc<PcMirrorClocks>,
}

impl std::fmt::Debug for G7LinkNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("G7LinkNode").field("node", &self.node).finish_non_exhaustive()
    }
}

impl G7LinkNode {
    /// Runs `fut` on the node's runtime and waits for it from the caller's
    /// executor.
    async fn run<T: Send + 'static>(&self, fut: impl Future<Output = T> + Send + 'static) -> T {
        match self.runtime.spawn(fut).await {
            Ok(value) => value,
            Err(e) => std::panic::resume_unwind(e.into_panic()),
        }
    }

    /// Runs `fut` on the node's runtime and blocks the calling thread until it finishes.
    fn block_on_rt<T: Send + 'static>(&self, fut: impl Future<Output = T> + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.runtime.spawn(async move {
            let _ = tx.send(fut.await);
        });
        rx.recv().expect("g7link-core runtime task panicked")
    }
}

#[uniffi::export]
impl G7LinkNode {
    /// Loads or creates this device's identity and trust store, starts
    /// networking and starts delivering events to `listener`. Blocks for
    /// the (short) startup; call it off the main thread.
    #[uniffi::constructor]
    pub fn start(
        options: NodeOptions,
        platform: Arc<dyn Platform>,
        key_protector: Arc<dyn KeyProtector>,
        listener: Arc<dyn EventListener>,
    ) -> Result<Arc<Self>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("g7link-core")
            .enable_all()
            .build()
            .map_err(|e| G7LinkError::Internal { reason: e.to_string() })?;
        let mut config =
            NodeConfig::new(PathBuf::from(options.data_dir), options.device.into(), options.app_version);
        config.power = options.power.into();
        config.away_mode = options.away_mode;
        config.downloads_dir = Some(PathBuf::from(options.downloads_dir));
        config.usb_port = options.usb_port;
        config.key_protector = Some(Arc::new(KeyProtectorAdapter(key_protector)));
        with_built_in_capabilities(&mut config.capabilities);
        let pc_mirror_clocks = Arc::new(PcMirrorClocks::default());
        let adapter = PlatformAdapter(platform, listener.clone(), pc_mirror_clocks.clone());
        let node = runtime.block_on(Node::start(config, Arc::new(adapter)))?;

        let mut events = node.events();
        let node_for_events = node.clone();
        runtime.spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => {
                        if let Some(ev) = Event::from_node(event) {
                            listener.on_event(ev);
                        }
                    }
                    Err(RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, "event listener fell behind; resynchronizing state");
                        let devices = node_for_events.paired_devices().unwrap_or_else(|e| {
                            tracing::warn!(error = %e, "can't read paired devices to resynchronize");
                            Vec::new()
                        });
                        for d in devices {
                            listener
                                .on_event(Event::LinkChanged { id: d.id.to_string(), link: d.link.into() });
                            listener.on_event(Event::WakeInfoChanged {
                                id: d.id.to_string(),
                                can_wake: d.can_wake,
                            });
                            listener
                                .on_event(Event::ReconnectChanged { id: d.id.to_string(), on: d.reconnect });
                            listener.on_event(Event::PeerInfoChanged {
                                id: d.id.to_string(),
                                info: d.info.into(),
                            });
                            if let Ok(matrix) = node_for_events.capabilities(d.id) {
                                listener.on_event(Event::Capabilities {
                                    id: d.id.to_string(),
                                    features: features(&matrix),
                                });
                            }
                        }
                        listener.on_event(Event::QueueChanged);
                    }
                    Err(RecvError::Closed) => return,
                }
            }
        });
        Ok(Arc::new(G7LinkNode { runtime, node, pc_mirror_clocks }))
    }

    /// This device's ID (z-base-32).
    pub fn device_id(&self) -> String {
        self.node.device_id().to_string()
    }

    /// Stops networking and tells connected devices.
    pub async fn shutdown(&self) {
        let node = self.node.clone();
        self.run(async move {
            if tokio::time::timeout(SHUTDOWN_TIMEOUT, node.shutdown()).await.is_err() {
                tracing::warn!("shutdown timed out");
            }
        })
        .await;
    }

    // ---- Pairing ----

    /// Enters pairing mode and returns the link to show as a QR code.
    pub async fn pairing_start_qr(&self) -> Result<String> {
        let node = self.node.clone();
        self.run(async move { Ok(node.pairing_start_qr().await?.to_uri()) }).await
    }

    /// Pairs with the device whose pairing link was scanned.
    pub async fn pairing_join(&self, link: String) -> Result<()> {
        let node = self.node.clone();
        self.run(async move { Ok(node.pairing_join(&link).await?) }).await
    }

    /// Pairs with the device a pairing link names when another app opened
    /// the link: both devices show a fresh code to compare, which arrives as
    /// `Event::PairingCode`.
    pub async fn pairing_join_link(&self, link: String) -> Result<()> {
        let node = self.node.clone();
        self.run(async move { Ok(node.pairing_join_link(&link).await?) }).await
    }

    /// Starts nearby pairing with a discovered device in pairing mode; the
    /// code to compare arrives as `Event::PairingCode`.
    pub async fn pairing_start_nearby(&self, peer: String) -> Result<()> {
        let peer = parse_id(&peer)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.pairing_start_nearby(peer).await?) }).await
    }

    /// Answers the code comparison.
    pub fn pairing_confirm(&self, codes_match: bool) -> Result<()> {
        Ok(self.node.pairing_confirm(codes_match)?)
    }

    pub fn pairing_cancel(&self) {
        self.node.pairing_cancel();
    }

    /// Whether a list of nearby devices (pairing) is on screen: the LAN is
    /// then asked who's there often, and rarely otherwise, to save battery.
    pub fn set_discovery_fast(&self, fast: bool) {
        self.node.set_discovery_fast(fast);
    }

    pub fn discovered_devices(&self) -> Vec<DiscoveredDevice> {
        self.node.discovered_devices().into_iter().map(Into::into).collect()
    }

    // ---- Paired devices ----

    pub fn paired_devices(&self) -> Result<Vec<PairedDevice>> {
        Ok(self.node.paired_devices()?.into_iter().map(Into::into).collect())
    }

    pub async fn unpair(&self, id: String) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.unpair(id).await?) }).await
    }

    /// Asks a PC to start this phone's Elevated helper
    /// (`docs/protocol/elevated-usb.md`): one this phone is plugged into over
    /// USB, or, with `adb_port` (this phone's Wireless debugging port), one
    /// that Wireless debugging has paired. Returns the helper's port.
    pub async fn elevated_usb_start(
        &self,
        pc_id: String,
        apk: String,
        token: String,
        grants: Vec<String>,
        adb_port: Option<u16>,
    ) -> Result<u16> {
        let id = parse_id(&pc_id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.elevated_usb_start(id, apk, token, grants, adb_port).await?) }).await
    }

    pub async fn ring(&self, id: String, on: bool) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.ring(id, on).await?) }).await
    }

    /// Saves the address typed for a paired PC (`""` clears it), looks a name
    /// up (up to 3 s) and redials. Throws for a device that isn't paired.
    pub async fn set_peer_address(&self, id: String, address: String) -> Result<PeerAddressResult> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.set_peer_address(id, &address).await?.into()) }).await
    }

    /// The address typed for a paired PC, if any.
    pub fn peer_address(&self, id: String) -> Result<Option<String>> {
        Ok(self.node.peer_address(parse_id(&id)?)?)
    }

    /// How a paired PC would be reached away from home; `tailnet_up`:
    /// Tailscale is up on this phone now.
    pub fn away_reach(&self, id: String, tailnet_up: bool) -> Result<AwayReach> {
        Ok(self.node.away_reach(parse_id(&id)?, tailnet_up)?.into())
    }

    pub async fn stop_ringing(&self) -> Result<()> {
        let node = self.node.clone();
        self.run(async move {
            node.stop_ringing().await;
            Ok(())
        })
        .await
    }

    /// Dismisses a paired phone's or tablet's notification there, after it
    /// was dismissed here (`notify.dismiss`).
    pub async fn dismiss_peer_notification(&self, id: String, key: String) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.dismiss_notification(id, key).await?) }).await
    }

    pub fn capabilities(&self, id: String) -> Result<Vec<Feature>> {
        Ok(features(&self.node.capabilities(parse_id(&id)?)?))
    }

    /// Starts a Bluetooth link with the PC `id`, whose beacon asked for one.
    /// Refused unless both let the other use Bluetooth.
    pub fn ble_connect(&self, id: String) -> Result<Arc<BleEndpoint>> {
        Ok(Arc::new(BleEndpoint(self.node.ble_connect(parse_id(&id)?)?)))
    }

    /// The key of each paired PC's Bluetooth beacon (docs/protocol/bluetooth.md).
    pub fn ble_beacon_keys(&self) -> Vec<BleBeaconKey> {
        self.node
            .ble_beacon_keys()
            .into_iter()
            .map(|(id, key)| BleBeaconKey { id: id.to_string(), key: key.to_vec() })
            .collect()
    }

    pub fn device_toggles(&self, id: String) -> Result<Vec<DeviceToggle>> {
        Ok(self
            .node
            .device_toggles(parse_id(&id)?)?
            .into_iter()
            .map(|(name, enabled)| DeviceToggle { name: name.into(), enabled })
            .collect())
    }

    pub fn set_device_toggle(&self, id: String, name: String, enabled: bool) -> Result<()> {
        Ok(self.node.set_device_toggle(parse_id(&id)?, &name, enabled)?)
    }

    /// Turns reconnecting to a paired device by itself off ("Stop
    /// reconnecting": disconnects and stays disconnected, still paired) or
    /// back on (dials it right away). Kept across restarts.
    pub fn set_reconnect(&self, id: String, on: bool) -> Result<()> {
        Ok(self.node.set_reconnect(parse_id(&id)?, on)?)
    }

    // ---- Files ----

    /// Sends files to a paired PC; returns the transfer's ID. Progress
    /// arrives as `Event::Transfer`.
    pub async fn send_files(&self, id: String, files: Vec<FileToSend>) -> Result<String> {
        let files = files_to_send(files)?;
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.send_files(id, files).await?) }).await
    }

    /// Hands off a document to a paired PC and asks the PC to open it with the
    /// default application once it arrives (`docs/protocol/files.md`).
    pub async fn send_handoff_files(&self, id: String, files: Vec<FileToSend>) -> Result<String> {
        let files = files_to_send(files)?;
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.send_handoff_files(id, files).await?) }).await
    }

    /// Sends a voice recording and its markers to a paired PC; returns the
    /// transfer's ID. Progress arrives as `Event::Transfer`.
    pub async fn send_recording(
        &self,
        id: String,
        file: FileToSend,
        markers: Vec<RecordingMarker>,
    ) -> Result<String> {
        let file = file_to_send(file)?;
        let id = parse_id(&id)?;
        let markers = markers.into_iter().map(Into::into).collect();
        let node = self.node.clone();
        self.run(async move { Ok(node.send_recording(id, file, markers).await?) }).await
    }

    /// Accepts an incoming transfer waiting for user confirmation (LocalSend).
    pub fn accept_transfer(&self, transfer_id: String) {
        self.node.accept_transfer(&transfer_id);
    }

    /// Pauses an active transfer in either direction, preserving partial files on the receiver.
    pub fn pause_transfer(&self, transfer_id: String) {
        self.node.pause_transfer(&transfer_id);
    }

    /// Resumes a manually paused transfer in either direction.
    pub fn resume_transfer(&self, transfer_id: String) {
        self.node.resume_transfer(&transfer_id);
    }

    /// Cancels a transfer in either direction (or declines a pending incoming
    /// LocalSend transfer).
    pub fn cancel_transfer(&self, transfer_id: String) {
        self.node.cancel_transfer(&transfer_id);
    }

    // ---- Persistent outgoing send queue ----

    /// Lists items in the persistent outgoing send queue (ordered by `seq ASC`),
    /// optionally filtered to a single paired `peer_id`.
    pub fn send_queue(&self, peer_id: Option<String>) -> Result<Vec<FfiQueueItem>> {
        let peer = match peer_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => Some(parse_id(id)?),
            None => None,
        };
        Ok(self.node.send_queue(peer)?.into_iter().map(Into::into).collect())
    }

    /// Returns the ID of the most recently queued active (`Waiting` or `Sending`) item for `peer_id`.
    pub fn latest_queued_id_for_peer(&self, peer_id: String) -> Option<String> {
        let peer = parse_id(peer_id.trim()).ok()?;
        self.node.latest_queued_id_for_peer(peer)
    }

    /// Cancels and removes a single item from the outgoing send queue (and cancels
    /// its active file transfer and cleans up staged files if applicable).
    pub fn cancel_queue_item(&self, id: String) -> Result<bool> {
        Ok(self.node.cancel_queue_item(&id)?)
    }

    /// Cancels and removes all active (`Waiting` and `Sending`) items in the outgoing
    /// send queue (optionally filtered to `peer_id`), returning the number of items cancelled.
    pub fn cancel_all_queue_items(&self, peer_id: Option<String>) -> Result<u32> {
        let peer = match peer_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => Some(parse_id(id)?),
            None => None,
        };
        Ok(self.node.cancel_all_queue_items(peer)?)
    }

    /// Resets a failed or expired queue item back to `Waiting` so it retries immediately.
    pub fn retry_queue_item(&self, id: String) -> Result<bool> {
        Ok(self.node.retry_queue_item(&id)?)
    }

    /// Resets all failed or expired queue items (optionally filtered to `peer_id`)
    /// back to `Waiting` and returns the number of items reset.
    pub fn retry_all_failed_queue_items(&self, peer_id: Option<String>) -> Result<u32> {
        let peer = match peer_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => Some(parse_id(id)?),
            None => None,
        };
        Ok(self.node.retry_all_failed_queue_items(peer)?)
    }

    /// Removes all failed and expired items from the outgoing send queue
    /// (optionally filtered to `peer_id`), cleans up their staged files, and returns
    /// the number of items removed.
    pub fn clear_finished_queue_items(&self, peer_id: Option<String>) -> Result<u32> {
        let peer = match peer_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => Some(parse_id(id)?),
            None => None,
        };
        Ok(self.node.clear_finished_queue_items(peer)?)
    }

    /// Marks any `Waiting` queue items whose TTL has elapsed as `Expired` and
    /// returns the number of items expired.
    pub fn expire_stale_queue_items(&self) -> Result<u32> {
        Ok(self.node.expire_stale_queue_items()?)
    }

    // ---- LocalSend compatibility ----

    /// Whether LocalSend v2.1 compatibility is enabled on this phone.
    pub fn localsend_enabled(&self) -> bool {
        self.node.localsend_enabled()
    }

    /// Turns LocalSend v2.1 compatibility on or off (persisted encrypted on disk).
    pub fn set_localsend_enabled(&self, enabled: bool) -> Result<()> {
        Ok(self.node.set_localsend_enabled(enabled)?)
    }

    /// Whether this phone's LocalSend server is actively bound and receiving.
    pub fn localsend_receiving(&self) -> bool {
        self.node.localsend_receiving()
    }

    /// Returns the LocalSend peers currently discovered on the local network.
    pub fn localsend_peers(&self) -> Vec<LocalSendPeer> {
        self.node.localsend_peers().into_iter().map(Into::into).collect()
    }

    /// Broadcasts a fresh LocalSend UDP multicast announcement.
    pub fn refresh_localsend(&self) {
        self.node.refresh_localsend();
    }

    /// Probes `host:port` via the LocalSend v2.1 HTTP/HTTPS API and adds the peer if found.
    pub async fn probe_localsend_peer(&self, host: String, port: u16) -> Result<LocalSendPeer> {
        let node = self.node.clone();
        self.run(async move { Ok(node.probe_localsend_peer(&host, port).await?.into()) }).await
    }

    // ---- Clipboard ----

    /// Puts text on a paired PC's clipboard.
    pub async fn send_clipboard(&self, id: String, text: String) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.send_clipboard(id, text).await?) }).await
    }

    /// Puts text (and optional HTML and RTF rich text) on a paired PC's
    /// clipboard.
    pub async fn send_clipboard_rich(
        &self,
        id: String,
        text: String,
        html: Option<String>,
        rtf: Option<String>,
    ) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.send_clipboard_rich(id, text, html, rtf).await?) }).await
    }

    /// Puts a password or the like on a connected PC's clipboard: never recorded
    /// or queued.
    pub async fn send_clipboard_private(&self, id: String, text: String) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.send_clipboard_private(id, text).await?) }).await
    }

    /// Puts an image (`image/png` or `image/jpeg`) on a paired PC's
    /// clipboard.
    /// Puts an image on PC `id`'s clipboard now, to paste where it's typing:
    /// never queued, and an error when its clipboard didn't take it.
    pub async fn paste_image(&self, id: String, mime: String, bytes: Vec<u8>) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.paste_image(id, mime, bytes).await?) }).await
    }

    pub async fn send_clipboard_image(&self, id: String, mime: String, bytes: Vec<u8>) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.send_clipboard_image(id, mime, bytes).await?) }).await
    }

    /// Whether the local clipboard history is enabled.
    pub fn clipboard_history_enabled(&self) -> bool {
        self.node.clipboard_history_enabled()
    }

    /// Turns the local clipboard history on or off.
    pub fn set_clipboard_history_enabled(&self, enabled: bool) -> Result<()> {
        Ok(self.node.set_clipboard_history_enabled(enabled)?)
    }

    /// Lists the local clipboard history (pinned first, then newest first),
    /// optionally filtered by `query`.
    pub fn clipboard_history(&self, query: Option<String>) -> Vec<ClipboardHistoryEntry> {
        self.node.clipboard_history(query.as_deref()).into_iter().map(Into::into).collect()
    }

    /// Decrypts and returns the image bytes for an image entry in the
    /// clipboard history, or `None` if not found.
    pub fn clipboard_history_image(&self, id: String) -> Option<Vec<u8>> {
        self.node.clipboard_history_image(&id).map(|(_, bytes)| bytes)
    }

    /// Pins or unpins an entry in the clipboard history.
    pub fn pin_clipboard_history(&self, id: String, pinned: bool) -> bool {
        self.node.pin_clipboard_history(&id, pinned)
    }

    /// Deletes one entry from the clipboard history.
    pub fn delete_clipboard_history(&self, id: String) -> bool {
        self.node.delete_clipboard_history(&id)
    }

    /// Clears the entire clipboard history and deletes its encrypted files from disk.
    pub fn clear_clipboard_history(&self) -> Result<()> {
        Ok(self.node.clear_clipboard_history()?)
    }

    /// Copies a clipboard history entry back onto this device's OS clipboard.
    pub fn copy_clipboard_history(&self, id: String) -> Result<()> {
        Ok(self.node.copy_clipboard_history(&id)?)
    }

    /// Sends a clipboard history entry (text or image) again to a paired device.
    pub async fn resend_clipboard_history(&self, clip_id: String, device_id: String) -> Result<()> {
        let peer = parse_id(&device_id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.resend_clipboard_history(peer, &clip_id).await?) }).await
    }

    // ---- Timeline ----

    /// Returns the local timeline's retention policy.
    pub fn timeline_retention(&self) -> Result<TimelineRetention> {
        Ok(self.node.timeline_retention()?.into())
    }

    /// Updates the local timeline's retention policy and runs an immediate purge.
    pub fn set_timeline_retention(&self, max_days: u32, max_entries: u32) -> Result<()> {
        Ok(self.node.set_timeline_retention(core::TimelineRetention { max_days, max_entries })?)
    }

    /// Queries a page of the local cross-device timeline (newest first).
    pub fn timeline_page(
        &self,
        kind: Option<TimelineKind>,
        device_id: Option<String>,
        search: Option<String>,
        offset: u32,
        limit: u32,
    ) -> Result<TimelinePage> {
        let device = match device_id {
            Some(ref id) if !id.is_empty() => Some(parse_id(id)?),
            _ => None,
        };
        Ok(self
            .node
            .timeline_page(&core::TimelineQuery {
                kind: kind.map(Into::into),
                device,
                search,
                offset,
                limit,
            })?
            .into())
    }

    /// Deletes one timeline entry by row ID.
    pub fn delete_timeline_entry(&self, id: i64) -> Result<bool> {
        Ok(self.node.delete_timeline_entry(id)?)
    }

    /// Clears all timeline entries.
    pub fn clear_timeline(&self) -> Result<()> {
        let _ = self.node.clear_timeline()?;
        Ok(())
    }

    /// Updates a timeline entry matching `ref_id` (e.g. after publishing an
    /// incoming transfer to `MediaStore`).
    pub fn update_timeline_by_ref(
        &self,
        ref_id: String,
        kind: TimelineKind,
        title: String,
        detail: String,
        target: String,
        size_bytes: u64,
    ) {
        self.node.update_timeline_by_ref(&ref_id, kind.into(), &title, &detail, &target, size_bytes);
    }

    // ---- Data retention & privacy ----

    /// Returns current item/byte counts and retention settings across local history and caches.
    pub fn data_retention_counts(&self) -> DataRetentionCounts {
        self.node.data_retention_counts().into()
    }

    /// Clears all locally cached `MessagingStyle` chat threads and messages.
    pub fn clear_message_cache(&self) -> Result<()> {
        Ok(self.node.clear_message_cache()?)
    }

    /// Clears all file transfer records from the local timeline without deleting downloaded files.
    pub fn clear_received_file_history(&self) -> Result<()> {
        Ok(self.node.clear_received_file_history()?)
    }

    /// Wipes all local history and caches (encrypted clipboard history, timeline, and message cache)
    /// while preserving device identity, paired devices, and per-device feature toggles.
    pub fn clear_all_local_data(&self) -> Result<()> {
        Ok(self.node.clear_all_local_data()?)
    }

    // ---- Media ----

    /// This phone's media players changed (most relevant first).
    pub async fn media_changed(&self, players: Vec<MediaPlayer>) {
        let node = self.node.clone();
        self.run(async move { node.media_changed(players.into_iter().map(Into::into).collect()).await })
            .await;
    }

    /// Runs a command on a paired device's media player; `position`
    /// (milliseconds) for seeking.
    pub async fn media_command(
        &self,
        id: String,
        player: String,
        action: MediaAction,
        position: Option<u64>,
    ) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.media_command(id, player, action.into(), position).await?) }).await
    }

    /// Runs a command that takes a value on a paired device's media player:
    /// `Shuffle` with `on` or `off`, `Repeat` with `none`, `track` or
    /// `playlist`. Any other pair is `Internal` (a bad message).
    pub async fn media_command_value(
        &self,
        id: String,
        player: String,
        action: MediaAction,
        value: String,
    ) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.media_command_value(id, player, action.into(), value).await?) }).await
    }

    /// The large picture a PC's player names as `art_full`: JPEG or PNG (tell
    /// by the bytes), at most 2 MiB. Keep it in memory by key.
    pub async fn media_art(&self, id: String, key: String) -> Result<Vec<u8>> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.media_art(id, key).await?.0) }).await
    }

    /// The app icon a PC's player names as `icon`: PNG, at most 16 KiB.
    pub async fn media_icon(&self, id: String, key: String) -> Result<Vec<u8>> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.media_icon(id, key).await?) }).await
    }

    // ---- Notifications ----

    /// A notification appeared or changed on this phone.
    pub async fn notification_posted(&self, notification: Notification) {
        let node = self.node.clone();
        self.run(async move { node.notification_posted(notification.into()).await }).await;
    }

    pub async fn notification_removed(&self, key: String) {
        let node = self.node.clone();
        self.run(async move { node.notification_removed(key).await }).await;
    }

    /// Everything this phone shows now (listener connected), or nothing
    /// (access revoked).
    pub async fn notifications_reset(&self, items: Vec<Notification>) {
        let node = self.node.clone();
        self.run(async move { node.notifications_reset(items.into_iter().map(Into::into).collect()).await })
            .await;
    }

    // ---- This device ----

    pub async fn update_battery(&self, battery: Battery) {
        let node = self.node.clone();
        self.run(async move { node.update_battery(battery.into()).await }).await;
    }

    /// Tells paired devices that `version` is out on `channel` (see
    /// `Node::announce_update`).
    pub async fn announce_update(&self, channel: String, version: String) {
        let node = self.node.clone();
        self.run(async move { node.announce_update(channel, version).await }).await;
    }

    pub async fn update_device_info(&self, info: DeviceInfo) {
        let node = self.node.clone();
        self.run(async move { node.update_device_info(info.into()).await }).await;
    }

    /// Updates this phone's Material You seed color (`0xFFRRGGBB` ARGB) and
    /// sends `HelloUpdate` to connected PCs if it changed.
    pub async fn update_accent(&self, accent: Option<u32>) {
        let node = self.node.clone();
        self.run(async move { node.update_accent(accent).await }).await;
    }

    /// Updates this phone's power level and the capabilities it offers on
    /// top of the built-in ones (they depend on permissions and power).
    pub async fn update_power(&self, power: PowerLevel, mut capabilities: Vec<String>) {
        with_built_in_capabilities(&mut capabilities);
        let node = self.node.clone();
        self.run(async move { node.update_power(power.into(), capabilities).await }).await;
    }

    /// Whether remote / mobile data connections outside the local Wi-Fi/LAN are allowed.
    pub fn away_mode(&self) -> bool {
        self.node.away_mode()
    }

    /// Enables or disables connecting over mobile data / non-LAN paths at runtime.
    pub fn set_away_mode(&self, enabled: bool) {
        self.node.set_away_mode(enabled);
    }

    pub fn relay_snapshot(&self) -> RelaySnapshot {
        use core::relays::RelayRegion as Region;
        let snapshot = self.node.relay_snapshot();
        RelaySnapshot {
            servers: snapshot
                .servers
                .into_iter()
                .map(|server| RelayServer {
                    url: server.url,
                    provider: server.provider,
                    region: match server.region {
                        Region::NorthAmericaEast => RelayRegion::NorthAmericaEast,
                        Region::NorthAmericaWest => RelayRegion::NorthAmericaWest,
                        Region::Europe => RelayRegion::Europe,
                        Region::AsiaPacific => RelayRegion::AsiaPacific,
                        Region::Custom => RelayRegion::Custom,
                    },
                    enabled: server.enabled,
                })
                .collect(),
            home: snapshot
                .home
                .into_iter()
                .map(|relay| HomeRelay { url: relay.url, connected: relay.connected, failed: relay.failed })
                .collect(),
            routes: snapshot
                .routes
                .into_iter()
                .map(|route| RelayRoute { device: route.device, url: route.url })
                .collect(),
        }
    }

    pub fn edit_relay(&self, url: String, action: RelayEdit) -> RelayEditResult {
        use core::relays::{RelayEdit as Edit, RelayEditError};
        let action = match action {
            RelayEdit::Add => Edit::Add,
            RelayEdit::Enable => Edit::Enable,
            RelayEdit::Disable => Edit::Disable,
            RelayEdit::Remove => Edit::Remove,
        };
        match self.node.edit_relay(&url, action) {
            Ok(()) => RelayEditResult::Saved,
            Err(RelayEditError::InvalidAddress) => RelayEditResult::InvalidAddress,
            Err(RelayEditError::Duplicate) => RelayEditResult::Duplicate,
            Err(RelayEditError::Limit) => RelayEditResult::Limit,
            Err(err) => {
                tracing::warn!(%err, "can't save relay settings");
                RelayEditResult::Failed
            }
        }
    }

    pub async fn check_relays(&self) -> Vec<RelayProbe> {
        let node = self.node.clone();
        self.run(async move { node.check_relays().await })
            .await
            .into_iter()
            .map(|probe| RelayProbe { url: probe.url, rtt_ms: probe.rtt_ms, timed_out: probe.timed_out })
            .collect()
    }

    /// Locks a paired PC (`sleep`: puts it to sleep instead).
    pub async fn pc_power(&self, id: String, sleep: bool) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let action = if sleep { core::PowerAction::Sleep } else { core::PowerAction::Lock };
        self.run(async move { Ok(node.pc_power(id, action).await?) }).await
    }

    /// Asks a paired PC for a screenshot (`pc.capture.shot`). The PNG then
    /// arrives as the transfer the answer names. Waits while the PC shows its
    /// screen-sharing dialog (`Event::PcCapturePrompt`).
    pub async fn pc_capture_shot(&self, pc_id: String) -> Result<PcCaptureOutcome> {
        let id = parse_id(&pc_id)?;
        let node = self.node.clone();
        self.run(async move { Ok(capture_file(node.pc_capture_shot(id).await)) }).await
    }

    /// Asks a paired PC to record its screen (`pc.capture.record.start`);
    /// answers once it records.
    pub async fn pc_capture_record_start(&self, pc_id: String) -> Result<PcCaptureOutcome> {
        let id = parse_id(&pc_id)?;
        let node = self.node.clone();
        self.run(async move {
            Ok(match node.pc_capture_record_start(id).await {
                Ok(()) => PcCaptureOutcome::Started,
                Err(err) => err.into(),
            })
        })
        .await
    }

    /// Stops a recording of a PC's screen this phone started
    /// (`pc.capture.record.stop`); the video arrives as the transfer the answer names.
    pub async fn pc_capture_record_stop(&self, pc_id: String) -> Result<PcCaptureOutcome> {
        let id = parse_id(&pc_id)?;
        let node = self.node.clone();
        self.run(async move { Ok(capture_file(node.pc_capture_record_stop(id).await)) }).await
    }

    /// Sends Wake-on-LAN magic packets for a paired PC over UDP to its stored
    /// subnet broadcasts and `255.255.255.255` on ports 9 and 7, repeated a
    /// few times over ~2 s.
    pub async fn wake(&self, id: String) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.wake(id).await?) }).await
    }

    /// Opens a web link on a paired PC, which asks its user first.
    pub async fn open_link(&self, id: String, url: String) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.open_link(id, url).await?) }).await
    }

    /// Opens a link the user shared from the system share sheet on a paired
    /// PC, which opens it without asking. Call it only from the share-sheet
    /// flow, never for links the app picked by itself.
    pub async fn open_shared_link(&self, id: String, url: String) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.open_shared_link(id, url).await?) }).await
    }

    // ---- Remote input (touchpad, keyboard, presentation) ----

    /// Checks whether a paired PC allows remote mouse/keyboard input from this
    /// phone (`G7LinkError::Denied` when `remote_input` is off on the PC,
    /// which also triggers a one-time confirmation prompt there).
    pub async fn remote_check(&self, id: String) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.remote_check(id).await?) }).await
    }

    /// Moves a paired PC's mouse cursor by `(dx, dy)` logical pixels over
    /// QUIC datagrams (or the `input/motion` stream fallback).
    pub async fn remote_move(&self, id: String, dx: f32, dy: f32) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.remote_move(id, dx, dy).await?) }).await
    }

    /// Scrolls on a paired PC (`dy` > 0 scrolls down, `dx` > 0 scrolls right,
    /// in wheel notches). When `fast` is true, sends over QUIC datagrams.
    pub async fn remote_scroll(&self, id: String, dx: f32, dy: f32, fast: bool) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move {
            if fast {
                Ok(node.remote_scroll_fast(id, dx, dy).await?)
            } else {
                Ok(node.remote_input(id, core::RemoteInput::Scroll { dx, dy }).await?)
            }
        })
        .await
    }

    /// Presses, releases or clicks a mouse button (`"left"`, `"right"` or
    /// `"middle"`; `action`: `"down"`, `"up"` or `"click"`) on a paired PC.
    pub async fn remote_button(&self, id: String, button: String, action: String) -> Result<()> {
        let id = parse_id(&id)?;
        let button = match button.as_str() {
            "left" => core::MouseButton::Left,
            "right" => core::MouseButton::Right,
            "middle" => core::MouseButton::Middle,
            _ => return Err(G7LinkError::Internal { reason: format!("invalid mouse button: {button}") }),
        };
        let action = match action.as_str() {
            "down" => core::ButtonAction::Down,
            "up" => core::ButtonAction::Up,
            "click" => core::ButtonAction::Click,
            _ => {
                return Err(G7LinkError::Internal { reason: format!("invalid button action: {action}") });
            }
        };
        let node = self.node.clone();
        self.run(
            async move { Ok(node.remote_input(id, core::RemoteInput::Button { button, action }).await?) },
        )
        .await
    }

    /// Types Unicode text on a paired PC.
    pub async fn remote_text(&self, id: String, text: String) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.remote_input(id, core::RemoteInput::Text { text }).await?) }).await
    }

    /// Whether PC `id` mirrors its focused text field and takes `remote_edit`.
    pub fn remote_mirrors_text(&self, id: String) -> bool {
        parse_id(&id).is_ok_and(|id| self.node.remote_mirrors_text(&id))
    }

    /// The route the user picked for PC `id`'s touchpad. Kept by the app
    /// (per PC): set it at start and on change. The path changes once
    /// nothing is mid-gesture; `Event::RemoteRouteChanged` says when.
    pub fn set_remote_route(&self, id: String, route: RemoteRoute) {
        if let Ok(id) = parse_id(&id)
            && let Err(error) = self.node.set_remote_route(id, route.into())
        {
            tracing::debug!(%error, "couldn't set a touchpad route");
        }
    }

    /// Where PC `id`'s touchpad route stands; a default state for an ID
    /// that isn't a paired device's.
    pub fn remote_route_state(&self, id: String) -> RouteState {
        let state = parse_id(&id).map(|id| self.node.remote_route_state(id)).unwrap_or_default();
        state.into()
    }

    /// A touchpad (touchpad, air mouse or presentation tab) for PC `id`
    /// shows (`active`) or no longer does: while it shows, both paths are
    /// measured every 2 s and Auto picks between them.
    pub fn set_remote_active(&self, id: String, active: bool) {
        if let Ok(id) = parse_id(&id)
            && let Err(error) = self.node.set_remote_active(id, active)
        {
            tracing::debug!(%error, "couldn't follow a touchpad");
        }
    }

    /// Asks PC `id` to keep a Bluetooth link with this phone (`on`) or to
    /// stop, over the session or else the link. With `ask`, a PC whose
    /// `ble_link` toggle is off for this phone may ask its user (at most once
    /// a minute); poll with `ask` false while it's `Asking`. Renew every
    /// 20 s while a touchpad shows: a hold not renewed ends after 45 s.
    /// `Unsupported` for a PC without `ble.remote`, `Offline` with neither path.
    pub async fn ble_use(&self, id: String, on: bool, ask: bool) -> Result<BleUseState> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.ble_use(id, on, ask).await?.into()) }).await
    }

    /// [`ble_use`](Self::ble_use), with what the PC says about its connect window.
    pub async fn ble_use_reply(&self, id: String, on: bool, ask: bool) -> Result<BleUseReply> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.ble_use_reply(id, on, ask).await?.into()) }).await
    }

    /// The app icon `key` that a `TextFieldState.icon` from PC `id` named, as
    /// a PNG of at most 16 KiB (untrusted: decode it with bounds).
    pub async fn remote_app_icon(&self, id: String, key: String) -> Result<Vec<u8>> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.remote_app_icon(id, key).await?) }).await
    }

    /// Tells PC `id` the typing box opened (`on`) or closed: while open, it
    /// sends `Event::PcTextState`.
    pub async fn remote_text_watch(&self, id: String, on: bool) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.remote_text_watch(id, on).await?) }).await
    }

    /// Changes the focused text on PC `id`: either replaces the characters
    /// `range_start..range_end` of a mirrored field with `text`, or moves the
    /// caret `moves` characters (negative: left), deletes `backspaces`
    /// characters before it and types `text`. `focus` is the latest
    /// `TextFieldState.focus` (0 when there's none); a PC whose focus moved
    /// on refuses it with an `Internal` error ending in "stale focus".
    #[allow(clippy::too_many_arguments)]
    pub async fn remote_edit(
        &self,
        id: String,
        focus: u32,
        seq: u32,
        range_start: Option<u32>,
        range_end: Option<u32>,
        moves: i32,
        backspaces: u16,
        text: String,
    ) -> Result<()> {
        let id = parse_id(&id)?;
        let range = range_start.zip(range_end);
        let node = self.node.clone();
        let input = core::RemoteInput::Edit { focus, seq, range, moves, backspaces, text };
        self.run(async move { Ok(node.remote_input(id, input).await?) }).await
    }

    /// Places the caret of PC `id`'s mirrored field at `caret`, selecting
    /// from `anchor` (equal to `caret` for no selection).
    pub async fn remote_caret(
        &self,
        id: String,
        focus: u32,
        seq: u32,
        caret: u32,
        anchor: u32,
    ) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let input = core::RemoteInput::Caret { focus, seq, caret, anchor };
        self.run(async move { Ok(node.remote_input(id, input).await?) }).await
    }

    /// Presses a named key or shortcut (`key`) with optional modifiers
    /// (`"ctrl"`, `"alt"`, `"shift"`, `"win"`) on a paired PC.
    pub async fn remote_key(&self, id: String, key: String, mods: Vec<String>) -> Result<()> {
        let id = parse_id(&id)?;
        let mut parsed_mods = Vec::with_capacity(mods.len());
        for m in mods {
            let km = match m.as_str() {
                "ctrl" => core::KeyMod::Ctrl,
                "alt" => core::KeyMod::Alt,
                "shift" => core::KeyMod::Shift,
                "win" => core::KeyMod::Win,
                _ => return Err(G7LinkError::Internal { reason: format!("invalid key modifier: {m}") }),
            };
            parsed_mods.push(km);
        }
        let node = self.node.clone();
        self.run(async move {
            Ok(node.remote_input(id, core::RemoteInput::Key { key, mods: parsed_mods }).await?)
        })
        .await
    }

    /// Sends a presentation slide command (`"next"`, `"previous"`, `"start"`,
    /// `"stop"` or `"black"`) to a paired PC.
    pub async fn remote_slide(&self, id: String, action: String) -> Result<()> {
        let id = parse_id(&id)?;
        let action = match action.as_str() {
            "next" => core::SlideAction::Next,
            "previous" | "prev" => core::SlideAction::Previous,
            "start" => core::SlideAction::Start,
            "stop" => core::SlideAction::Stop,
            "black" => core::SlideAction::Black,
            _ => return Err(G7LinkError::Internal { reason: format!("invalid slide action: {action}") }),
        };
        let node = self.node.clone();
        self.run(async move { Ok(node.remote_input(id, core::RemoteInput::Slide { action }).await?) }).await
    }

    /// Moves or hides the laser pointer overlay on a paired PC (`x`, `y` in
    /// `0.0..=1.0`).
    pub async fn remote_laser(&self, id: String, on: bool, x: f32, y: f32) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.remote_laser(id, on, x, y).await?) }).await
    }

    // ---- Deck (macro pad) ----

    /// The latest Deck layout reported by a connected PC, if any.
    pub fn deck_layout(&self, id: String) -> Result<Option<DeckLayout>> {
        let id = parse_id(&id)?;
        Ok(self.node.deck_layout(id).map(Into::into))
    }

    /// The latest live Deck state reported by a connected PC, if any.
    pub fn deck_state(&self, id: String) -> Result<Option<DeckState>> {
        let id = parse_id(&id)?;
        Ok(self.node.deck_state(id).map(Into::into))
    }

    /// Asks a paired PC to run the action bound to `tile` (`deck.press`).
    pub async fn deck_press(&self, id: String, tile: String) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.deck_press(id, tile).await?) }).await
    }

    /// Asks a paired PC to set its master volume (`0..=100`) and/or mute state (`deck.audio.set`).
    pub async fn set_pc_audio(&self, id: String, volume: Option<u8>, muted: Option<bool>) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.set_pc_audio(id, volume, muted).await?) }).await
    }

    /// Asks a paired PC to make one of its audio outputs (by its `id` in
    /// `DeckState.output_devices`) the default (`deck.audio.set { output }`).
    pub async fn set_pc_audio_output(&self, id: String, output: String) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.set_pc_audio_output(id, output).await?) }).await
    }

    // ---- Agents (docs/protocol/agents.md) ----
    //
    // Errors: `Denied` means the PC's `agents` toggle is off for this phone
    // (the PC now shows its user a prompt); `Unsupported` that the PC has no
    // agent host, can't reach it, or the agent isn't installed or signed in;
    // `NotFound` an unknown or removed thread, folder or agent, or an ask
    // already answered. A busy PC (the thread is working, or too many run)
    // is `Internal` with a reason that starts with `BUSY`; `Busy` when it has
    // too many requests from this phone in flight.

    /// The last catalog a connected PC sent, if it has agents.
    pub fn agent_catalog(&self, id: String) -> Result<Option<AgentCatalog>> {
        let id = parse_id(&id)?;
        let can_say_why = self.node.agent_can_say_why(id);
        let can_control = self.node.agent_can_control(id);
        Ok(self.node.agent_catalog(id).map(|c| AgentCatalog::from_core(c, can_say_why, can_control)))
    }

    /// A PC's threads, newest first (at most 50), each saved on this phone
    /// as it passes. `show`: a shelf (the main list when null); `before_ms`:
    /// the next page, threads changed before then.
    #[uniffi::method(default(show = None, before_ms = None))]
    pub async fn agent_threads(
        &self,
        id: String,
        show: Option<AgentThreadShow>,
        before_ms: Option<i64>,
    ) -> Result<Vec<AgentThread>> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let threads = self
            .run(async move {
                if show.is_none() && before_ms.is_none() {
                    node.agent_threads(id).await
                } else {
                    let request = core::AgentThreadsRequest {
                        show: show.map(|s| s.wire().to_owned()),
                        before_ms: before_ms.and_then(|b| u64::try_from(b).ok()),
                    };
                    node.agent_threads_shown(id, request).await
                }
            })
            .await?;
        Ok(threads.into_iter().filter_map(AgentThread::from_core).collect())
    }

    /// Starts a thread on a PC: `agent` and `folder` are ids from its catalog.
    /// With a PC whose catalog has `can_control`: `mode` may be `Plan`;
    /// `model` an id from the agent's `models` (or one the user typed, with
    /// `custom_model`; one word, never starting with `-`); `effort` one of
    /// the model's `efforts`; `images` ids from `agent_image_send` (at most
    /// 4). Otherwise those are `Unsupported`, and nothing is sent.
    #[uniffi::method(default(model = None, effort = None, images = [], custom_model = false))]
    #[allow(clippy::too_many_arguments)]
    pub async fn agent_start(
        &self,
        id: String,
        agent: String,
        folder: String,
        prompt: String,
        mode: AgentMode,
        model: Option<String>,
        effort: Option<String>,
        images: Vec<String>,
        custom_model: bool,
    ) -> Result<AgentThread> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let start = core::AgentStart {
            agent,
            folder,
            prompt,
            mode: mode.wire().into(),
            model,
            effort,
            images,
            custom_model,
            key: None,
        };
        let thread = self.run(async move { node.agent_start(id, start).await }).await?;
        AgentThread::from_core(thread)
            .ok_or_else(|| G7LinkError::Internal { reason: "unknown thread state".into() })
    }

    /// Sends a follow-up to a thread that isn't running. With a PC whose
    /// catalog has `can_control`, also to one that is: `dispatch` says
    /// whether it steers the running turn or waits in its queue (the
    /// thread's next state shows where it went); `images` are ids from
    /// `agent_image_send` (at most 4; `text` may then be empty). Without it
    /// those are `Unsupported`, and nothing is sent.
    ///
    /// `hold`: when the PC can't be reached, the message waits on this phone
    /// and goes once, in order, when it's back (the answer is then the held
    /// message; null: it was sent). A message with images is never held: it
    /// fails offline ("Pictures can't wait. Send when %1 is back."). At most
    /// 20 wait per PC (`QueueFull`).
    #[uniffi::method(default(dispatch = None, images = [], hold = false))]
    pub async fn agent_send(
        &self,
        id: String,
        thread: String,
        text: String,
        dispatch: Option<AgentDispatch>,
        images: Vec<String>,
        hold: bool,
    ) -> Result<Option<AgentHeld>> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let send = core::AgentSend {
            thread,
            text,
            dispatch: agent_control::dispatch_wire(dispatch),
            images,
            key: None,
        };
        let held = self.run(async move { node.agent_send_held(id, send, hold).await }).await?;
        Ok(held.map(Into::into))
    }

    /// Stops a thread's running turn.
    pub async fn agent_cancel(&self, id: String, thread: String) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.agent_cancel(id, thread).await?) }).await
    }

    /// A page of a thread: the newest `limit` items (at most 50), or the
    /// ones before `before` (a `seq`).
    pub async fn agent_read(
        &self,
        id: String,
        thread: String,
        before: Option<u64>,
        limit: u32,
    ) -> Result<AgentPage> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let page = self.run(async move { node.agent_read(id, thread, before, limit).await }).await?;
        AgentPage::from_core(page)
            .ok_or_else(|| G7LinkError::Internal { reason: "unknown thread state".into() })
    }

    /// The one thread whose text this phone shows (`None`: none). Only it
    /// gets `AgentDelta` and `AgentItemChanged`; the watch ends with the session.
    pub async fn agent_watch(&self, id: String, thread: Option<String>) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.agent_watch(id, thread).await?) }).await
    }

    /// Answers an ask, this time only. `message`, with a denial only: what
    /// the user wants the agent to do instead, 1 to 2000 bytes, to a PC
    /// whose catalog has `can_say_why` (`Unsupported` otherwise, and the ask
    /// still waits; `Protocol` with `allow` or over the limit).
    #[uniffi::method(default(message = None))]
    pub async fn agent_answer(
        &self,
        id: String,
        token: String,
        allow: bool,
        message: Option<String>,
    ) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.agent_answer(id, token, allow, message).await?) }).await
    }

    // Sessions started on a PC outside G7 Link (docs/protocol/agents.md §3).
    // `Denied`: the PC's `agents` or `agent_sessions` toggle is off for this
    // phone (it shows its user a prompt); remember it and don't retry until
    // the next catalog. `Unsupported`: the PC doesn't list sessions.

    /// A PC's sessions started outside G7 Link (at most 50), in its order.
    pub async fn agent_sessions(&self, id: String) -> Result<Vec<AgentSession>> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let sessions = self.run(async move { node.agent_sessions(id).await }).await?;
        Ok(sessions.into_iter().filter_map(AgentSession::from_core).collect())
    }

    /// A page of a session: the newest `limit` items (at most 50), or the
    /// ones before `before` (a `seq`).
    pub async fn agent_session_read(
        &self,
        id: String,
        session: String,
        before: Option<u64>,
        limit: u32,
    ) -> Result<AgentSessionPage> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let page = self.run(async move { node.agent_session_read(id, session, before, limit).await }).await?;
        AgentSessionPage::from_core(page)
            .ok_or_else(|| G7LinkError::Internal { reason: "unknown session state".into() })
    }

    /// The one session whose items this phone shows (`None`: none). Only it
    /// gets `AgentSessionItem`; the watch ends with the connection. A PC
    /// already serving two watching phones answers a third with a `BUSY`
    /// `Internal`.
    pub async fn agent_session_watch(&self, id: String, session: Option<String>) -> Result<()> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.agent_session_watch(id, session).await?) }).await
    }

    /// "Continue here": makes the session a supervised thread started by
    /// this phone, idle until you send something. Refused while it's open
    /// anywhere (`BUSY`), or when its folder isn't an agent folder (`Denied`).
    pub async fn agent_session_continue(&self, id: String, session: String) -> Result<AgentThread> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let thread = self.run(async move { node.agent_session_continue(id, session).await }).await?;
        AgentThread::from_core(thread)
            .ok_or_else(|| G7LinkError::Internal { reason: "unknown thread state".into() })
    }

    /// A thread (`thread`) or a followed session (`session`) as Markdown, to
    /// share or save: exactly one of the two. A session needs the PC's
    /// `agent_sessions` toggle too (`Denied` otherwise); `NotFound` when it's
    /// gone.
    pub async fn agent_export(
        &self,
        id: String,
        thread: Option<String>,
        session: Option<String>,
    ) -> Result<AgentExport> {
        let id = parse_id(&id)?;
        let node = self.node.clone();
        let export = self.run(async move { node.agent_export(id, thread, session).await }).await?;
        Ok(export.into())
    }

    /// The context chip suggestion for the most recently received text clip, if any.
    pub fn last_clip_suggestion(&self) -> Option<ClipSuggestion> {
        self.node.last_clip_suggestion().map(|(_, s)| s.into())
    }

    /// Opens this phone's screen stream to a PC that asked (after the user
    /// agreed).
    pub async fn mirror_open(&self, pc_id: String) -> Result<Arc<MirrorStream>> {
        let id = parse_id(&pc_id)?;
        let node = self.node.clone();
        self.run(async move { Ok(Arc::new(MirrorStream(node.mirror_open(id).await?))) }).await
    }

    /// Opens this phone's sound stream to a PC that asked for it with the
    /// screen.
    pub async fn mirror_open_audio(&self, pc_id: String) -> Result<Arc<MirrorStream>> {
        let id = parse_id(&pc_id)?;
        let node = self.node.clone();
        self.run(async move { Ok(Arc::new(MirrorStream(node.mirror_open_audio(id).await?))) }).await
    }

    /// The Elevated helper's taps and keys are refused by this phone (Xiaomi,
    /// until USB debugging (Security settings) is on): tells the PC showing
    /// mirroring `session`, so it can say what to turn on.
    pub async fn mirror_input_blocked(&self, pc_id: String, session: u32) -> Result<()> {
        let id = parse_id(&pc_id)?;
        let node = self.node.clone();
        self.run(async move {
            node.mirror_notice(id, session, core::MIRROR_NOTICE_INPUT_BLOCKED).await.map_err(Into::into)
        })
        .await
    }

    /// This phone is locked and ignores the PC's taps and keys on mirroring
    /// `session` until it's unlocked: tells the PC, so it can say why.
    pub async fn mirror_locked(&self, pc_id: String, session: u32) -> Result<()> {
        let id = parse_id(&pc_id)?;
        let node = self.node.clone();
        self.run(async move {
            node.mirror_notice(id, session, core::MIRROR_NOTICE_LOCKED).await.map_err(Into::into)
        })
        .await
    }

    /// Tells a paired PC that mirroring or remote keyboard input on `session` has stopped (`mirror.stop`).
    pub async fn mirror_stop(&self, pc_id: String, session: u32) -> Result<()> {
        let id = parse_id(&pc_id)?;
        let node = self.node.clone();
        self.run(async move {
            node.mirror_stop(id, session).await;
            Ok(())
        })
        .await
    }

    /// Asks a paired PC to stream its screen to this phone (`mirror.start`, `session == 0`),
    /// or with `watch`, what that PC player plays, with its sound (Play on phone, after the
    /// PC offered it). Says whether the PC agreed; offline and transport failures are errors.
    pub async fn start_pc_mirror(
        &self,
        pc_id: String,
        max_size: u32,
        fps: u32,
        bitrate: u32,
        watch: Option<String>,
    ) -> Result<PcMirrorStart> {
        let id = parse_id(&pc_id)?;
        let node = self.node.clone();
        let options = core::MirrorStart {
            max_size,
            fps,
            bitrate,
            audio: watch.is_some(),
            session: core::MIRROR_SCREEN,
            app: None,
            stay_awake: false,
            screen_off: false,
            watch,
            sound_only: false,
        };
        self.run(async move { pc_mirror_start(node.mirror_start(id, options).await) }).await
    }

    /// PC `pc_id`'s clock against this phone's, while its screen shows here:
    /// `None` until it's first timed, after each new stream starts.
    pub fn pc_mirror_clock(&self, pc_id: String) -> Option<PcMirrorClock> {
        self.pc_mirror_clocks.get(&pc_id)
    }

    /// Asks a paired PC for a fresh keyframe on its screen stream (`mirror.keyframe`).
    pub async fn request_pc_mirror_keyframe(&self, pc_id: String) -> Result<()> {
        let id = parse_id(&pc_id)?;
        let node = self.node.clone();
        self.run(async move {
            node.mirror_keyframe(id, core::MIRROR_SCREEN).await;
            Ok(())
        })
        .await
    }

    /// Sends touch, scroll, key, or text input to a paired PC's mirrored screen (`mirror.input`).
    pub async fn send_pc_mirror_input(&self, pc_id: String, input: MirrorInputEvent) -> Result<()> {
        let id = parse_id(&pc_id)?;
        let node = self.node.clone();
        let input: core::MirrorInput = input.into();
        self.run(async move { Ok(node.mirror_input(id, core::MIRROR_SCREEN, input).await?) }).await
    }

    /// Preferred webcam codecs accepted by the connected PC, with H.264 as fallback.
    pub fn webcam_codecs(&self, pc_id: String) -> Result<Vec<String>> {
        Ok(self.node.webcam_codecs(parse_id(&pc_id)?)?)
    }

    /// Screen codecs accepted by the connected receiver, with H.264 fallback.
    pub fn mirror_codecs(&self, pc_id: String) -> Result<Vec<String>> {
        Ok(self.node.mirror_codecs(parse_id(&pc_id)?)?)
    }

    /// Opens this phone's camera stream to a paired PC (`docs/protocol/webcam.md`).
    pub async fn webcam_open(&self, pc_id: String) -> Result<Arc<MirrorStream>> {
        let id = parse_id(&pc_id)?;
        let node = self.node.clone();
        self.run(async move { Ok(Arc::new(MirrorStream(node.webcam_open(id).await?))) }).await
    }

    /// Opens this phone's microphone stream to a paired PC (`docs/protocol/mic.md`):
    /// a config packet (`mirror_audio_config`) first, then 16-bit PCM.
    pub async fn mic_open(&self, pc_id: String) -> Result<Arc<MirrorStream>> {
        let id = parse_id(&pc_id)?;
        let node = self.node.clone();
        self.run(async move { Ok(Arc::new(MirrorStream(node.mic_open(id).await?))) }).await
    }

    /// Stops this phone's microphone stream to a paired PC and tells the PC.
    pub async fn mic_stop(&self, pc_id: String) -> Result<()> {
        let id = parse_id(&pc_id)?;
        let node = self.node.clone();
        self.run(async move {
            node.mic_stop(id).await;
            Ok(())
        })
        .await
    }

    /// Stops this phone's webcam stream to a paired PC and tells the PC.
    pub async fn webcam_stop(&self, pc_id: String) -> Result<()> {
        let id = parse_id(&pc_id)?;
        let node = self.node.clone();
        self.run(async move {
            node.webcam_stop(id).await;
            Ok(())
        })
        .await
    }

    /// This phone's messages changed (in `thread`, or anywhere when null):
    /// PCs that show them catch up.
    pub async fn sms_changed(&self, thread: Option<String>) {
        let node = self.node.clone();
        self.run(async move { node.sms_changed(thread).await }).await;
    }

    /// This phone's call history changed: PCs that show it catch up.
    pub async fn call_log_changed(&self) {
        let node = self.node.clone();
        self.run(async move { node.call_log_changed().await }).await;
    }

    /// This phone's contacts changed: PCs that show them catch up.
    pub async fn contacts_changed(&self) {
        let node = self.node.clone();
        self.run(async move { node.contacts_changed().await }).await;
    }

    /// This phone's photo library changed: PCs that show it catch up.
    pub async fn photos_changed(&self) {
        let node = self.node.clone();
        self.run(async move { node.photos_changed().await }).await;
    }

    /// A folder on this phone changed (`""` for root): PCs that have it open
    /// in their file manager refresh it (`storage.changed`, debounced).
    pub async fn storage_changed(&self, path: String) {
        let node = self.node.clone();
        self.run(async move { node.storage_changed(path).await }).await;
    }

    /// Directory paths currently watched by any connected PC (most recently
    /// listed folders).
    pub fn storage_open_folders(&self) -> Vec<String> {
        self.node.storage_open_folders()
    }

    /// A call on this phone rang, was answered or ended: tells the PCs that
    /// show calls (and are allowed them), now and when they connect during it.
    pub async fn call_changed(&self, call: Call) -> Result<()> {
        let node = self.node.clone();
        let call = core::CallState {
            id: call.id,
            state: match call.state {
                CallPhase::Ringing => "ringing",
                CallPhase::Active => "active",
                CallPhase::Ended => "ended",
            }
            .into(),
            incoming: call.incoming,
            number: call.number,
            name: call.name,
            photo: call.photo,
            missed: call.missed,
            since: call.since,
            controls: call.controls.map(|c| core::CallControls {
                muted: c.muted,
                speaker: c.speaker,
                held: c.held,
                can_hold: c.can_hold,
            }),
        };
        self.run(async move { Ok(node.call_changed(call).await?) }).await
    }

    /// A photo or screenshot just appeared on this phone: tells the PCs
    /// that show photos (and are allowed them).
    pub async fn photo_taken(&self, photo: Photo) -> Result<()> {
        let node = self.node.clone();
        let photo = core::Photo {
            id: photo.id,
            name: photo.name,
            size: photo.size,
            taken: photo.taken,
            screenshot: photo.screenshot,
            thumb: photo.thumb,
        };
        self.run(async move { Ok(node.photo_taken(photo).await?) }).await
    }

    /// This phone's quick settings state (on startup and whenever any toggle
    /// changes): tells the PCs that show toggles (and are allowed them).
    pub async fn toggles_changed(&self, toggles: PhoneToggles) -> Result<()> {
        let node = self.node.clone();
        let toggles: core::PhoneToggles = toggles.into();
        self.run(async move { Ok(node.toggles_changed(toggles).await?) }).await
    }

    // ---- Continuity Camera ----

    /// Sends a captured photo or scanned document from this phone to a requesting PC.
    #[allow(clippy::too_many_arguments)]
    pub async fn send_camera_capture_result(
        &self,
        pc_id: String,
        request_id: String,
        mode: String,
        file_name: String,
        mime: String,
        width: u32,
        height: u32,
        data: Vec<u8>,
    ) -> Result<()> {
        let peer = parse_id(&pc_id)?;
        let mode = match mode.as_str() {
            "photo" => core::CameraCaptureMode::Photo,
            "scan" => core::CameraCaptureMode::Scan,
            other => {
                return Err(G7LinkError::Internal { reason: format!("unknown capture mode {other:?}") });
            }
        };
        let meta = core::CameraCaptureResultMeta {
            request_id,
            mode,
            file_name,
            mime,
            size: data.len() as u64,
            width,
            height,
        };
        let node = self.node.clone();
        self.run(async move { Ok(node.send_camera_capture_result(peer, meta, data).await?) }).await
    }

    /// Cancels an in-flight Continuity Camera capture request on either side.
    pub async fn cancel_camera_capture(
        &self,
        peer_id: String,
        request_id: String,
        reason: Option<String>,
    ) -> Result<()> {
        let peer = parse_id(&peer_id)?;
        let node = self.node.clone();
        self.run(async move { Ok(node.cancel_camera_capture(peer, request_id, reason).await?) }).await
    }

    /// Requests a Continuity Camera capture (`mode` is `"photo"` or `"scan"`) from a paired phone.
    pub async fn request_camera_capture(
        &self,
        peer_id: String,
        request_id: String,
        mode: String,
    ) -> Result<()> {
        let peer = parse_id(&peer_id)?;
        let mode = match mode.as_str() {
            "photo" => core::CameraCaptureMode::Photo,
            "scan" => core::CameraCaptureMode::Scan,
            other => {
                return Err(G7LinkError::Internal { reason: format!("unknown capture mode {other:?}") });
            }
        };
        let node = self.node.clone();
        self.run(async move { Ok(node.request_camera_capture(peer, request_id, mode).await?) }).await
    }

    /// Reconnects to PCs that aren't connected and syncs connected ones.
    pub async fn refresh(&self) {
        let node = self.node.clone();
        self.run(async move { node.refresh().await }).await;
    }

    /// Whether the user has the app open: links are checked every 5 s
    /// instead of every 15 s then, and right away when it opens.
    pub fn set_interactive(&self, on: bool) {
        self.node.set_interactive(on);
    }

    /// Call when connectivity changes (Android doesn't tell native code).
    pub async fn network_changed(&self) {
        let node = self.node.clone();
        self.run(async move { node.network_changed().await }).await;
    }

    /// Play on phone: reads up to `length` bytes (at most 4 MiB) from
    /// `offset` of the video file the PC `peer` offered. Blocks; call off the
    /// main thread (a player's data source does).
    pub fn watch_file_read(
        &self,
        peer: String,
        offset: u64,
        length: u64,
    ) -> Result<WatchFileChunk, G7LinkError> {
        let peer_id = parse_id(&peer)?;
        let node = self.node.clone();
        let (size, data) =
            self.block_on_rt(async move { node.watch_file_read(peer_id, offset, length).await })?;
        Ok(WatchFileChunk { size, data })
    }

    /// Lists entries in a directory on a connected paired device (`storage.list`).
    /// `Denied` from a PC: it doesn't share its folders.
    pub fn list_remote_storage(
        &self,
        peer: String,
        path: String,
    ) -> Result<Vec<RemoteStorageEntry>, G7LinkError> {
        let peer_id = parse_id(&peer)?;
        let node = self.node.clone();
        let req_path = path.clone();
        let entries = self.block_on_rt(async move { node.list_storage(&peer_id, &req_path).await })?;
        let prefix = path.trim_matches('/');
        Ok(entries
            .into_iter()
            .map(|e| RemoteStorageEntry {
                path: if prefix.is_empty() { e.name.clone() } else { format!("{prefix}/{}", e.name) },
                name: e.name,
                kind: if e.is_dir { "dir".into() } else { "file".into() },
                size: e.size,
                modified: e.modified,
            })
            .collect())
    }

    /// Downloads a remote file from a connected paired device (`storage.read`) in
    /// 128 KiB chunks into `local_dest`.
    pub fn download_remote_storage(&self, peer: String, remote_path: String, local_dest: String) -> bool {
        let Ok(peer_id) = parse_id(&peer) else {
            return false;
        };
        let node = self.node.clone();
        self.block_on_rt(async move {
            use tokio::io::AsyncWriteExt;
            // tokio's file I/O keeps the runtime's two worker threads free.
            if let Some(parent) = std::path::Path::new(&local_dest).parent()
                && !parent.as_os_str().is_empty()
            {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let Ok(mut f) = tokio::fs::File::create(&local_dest).await else {
                return false;
            };
            let mut offset = 0u64;
            let chunk_len = 128 * 1024u64;
            // Complete only once every byte the peer says the file has is
            // here: a read that comes back short or empty before then (the
            // file shrank, the peer stopped) is a failed download, not a
            // smaller file.
            let complete = loop {
                let Ok((meta, chunk)) =
                    node.storage_read(peer_id, remote_path.clone(), offset, Some(chunk_len)).await
                else {
                    break false;
                };
                if meta.size == 0 {
                    break true;
                }
                if chunk.is_empty() || f.write_all(&chunk).await.is_err() {
                    break false;
                }
                offset += chunk.len() as u64;
                if offset >= meta.size {
                    break true;
                }
            };
            if complete && f.flush().await.is_ok() {
                return true;
            }
            drop(f);
            let _ = tokio::fs::remove_file(&local_dest).await;
            false
        })
    }

    /// Asks a connected paired peer on the target LAN to broadcast a Wake-on-LAN
    /// magic packet for `mac` (`pc.wake.relay`).
    pub fn wake_via_relay(&self, relay_peer: String, mac: String, broadcast: Option<String>) -> Result<()> {
        let peer_id = parse_id(&relay_peer)?;
        let mac_bytes = core::parse_mac(&mac)
            .ok_or_else(|| G7LinkError::Internal { reason: "invalid MAC address".into() })?;
        let bcast = match broadcast.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => Some(
                s.parse::<std::net::Ipv4Addr>()
                    .map_err(|_| G7LinkError::Internal { reason: "invalid broadcast IPv4 address".into() })?,
            ),
            None => None,
        };
        let node = self.node.clone();
        self.block_on_rt(async move { Ok(node.wake_via_relay(&peer_id, mac_bytes, bcast).await?) })
    }
}

/// Classifies a text clip into a single context chip suggestion (`None` for
/// OTPs or plain text).
#[uniffi::export]
pub fn classify_clip(text: String) -> Option<ClipSuggestion> {
    core::classify_clip(&text).map(Into::into)
}

/// Builds a web search URL for a parcel tracking number.
#[uniffi::export]
pub fn tracking_search_url(tracking_number: String) -> String {
    core::tracking_search_url(&tracking_number)
}

/// Builds a web maps URL for a street address.
#[uniffi::export]
pub fn maps_web_url(address: String) -> String {
    core::maps_web_url(&address)
}

/// Kind of rich Handoff link (`docs/protocol/actions.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum HandoffKind {
    WebLink,
    VideoLink,
    MapLocation,
}

impl From<core::HandoffKind> for HandoffKind {
    fn from(k: core::HandoffKind) -> Self {
        match k {
            core::HandoffKind::WebLink => HandoffKind::WebLink,
            core::HandoffKind::VideoLink => HandoffKind::VideoLink,
            core::HandoffKind::MapLocation => HandoffKind::MapLocation,
        }
    }
}

/// Rich Handoff link metadata extracted from a URL, `geo:` URI, or shared text.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HandoffLink {
    pub kind: HandoffKind,
    pub url: String,
    pub label: String,
    pub timestamp_secs: Option<u32>,
    pub map_query: Option<String>,
}

impl From<core::HandoffLink> for HandoffLink {
    fn from(h: core::HandoffLink) -> Self {
        HandoffLink {
            kind: h.kind.into(),
            url: h.url,
            label: h.label,
            timestamp_secs: h.timestamp_secs,
            map_query: h.map_query,
        }
    }
}

/// Extracts and classifies a Handoff link (`http://`, `https://`, or `geo:`)
/// from a URL or shared text snippet.
#[uniffi::export]
pub fn extract_handoff_link(text: String) -> Option<HandoffLink> {
    core::extract_handoff_link(&text).map(Into::into)
}

/// Attaches or updates a playback timestamp (`?t=123` / `&t=123s`) on a
/// YouTube, Vimeo, or direct video link.
#[uniffi::export]
pub fn with_video_timestamp(url: String, seconds: u32) -> String {
    core::with_video_timestamp(&url, seconds)
}

/// Converts a `geo:` URI into a Google Maps `https://` URL.
#[uniffi::export]
pub fn geo_to_maps_https(uri: String) -> Option<String> {
    core::geo_to_maps_https(&uri)
}

/// Returns true when `name` is a safe document/media file (not an executable or
/// script) that may be opened automatically on arrival.
#[uniffi::export]
pub fn is_safe_handoff_document(name: String) -> bool {
    core::is_safe_handoff_document(&name)
}

/// 15 Material 3 / Bloom color roles (`0xRRGGBB` each) for light or dark mode.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiRolePalette {
    pub primary: u32,
    pub on_primary: u32,
    pub primary_container: u32,
    pub on_primary_container: u32,
    pub secondary_container: u32,
    pub on_secondary_container: u32,
    pub surface: u32,
    pub surface_container_low: u32,
    pub surface_container: u32,
    pub surface_container_high: u32,
    pub surface_container_highest: u32,
    pub on_surface: u32,
    pub on_surface_variant: u32,
    pub outline: u32,
    pub outline_variant: u32,
}

impl From<core::RolePalette> for FfiRolePalette {
    fn from(p: core::RolePalette) -> Self {
        Self {
            primary: p.primary,
            on_primary: p.on_primary,
            primary_container: p.primary_container,
            on_primary_container: p.on_primary_container,
            secondary_container: p.secondary_container,
            on_secondary_container: p.on_secondary_container,
            surface: p.surface,
            surface_container_low: p.surface_container_low,
            surface_container: p.surface_container,
            surface_container_high: p.surface_container_high,
            surface_container_highest: p.surface_container_highest,
            on_surface: p.on_surface,
            on_surface_variant: p.on_surface_variant,
            outline: p.outline,
            outline_variant: p.outline_variant,
        }
    }
}

/// Complete light + dark theme scheme generated by the shared Rust perceptual color engine.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiThemeScheme {
    pub seed_rgb: u32,
    pub achromatic: bool,
    pub light: FfiRolePalette,
    pub dark: FfiRolePalette,
}

impl From<core::ThemeScheme> for FfiThemeScheme {
    fn from(s: core::ThemeScheme) -> Self {
        let to_role = |p: core::RolePalette| FfiRolePalette {
            primary: p.primary,
            on_primary: p.on_primary,
            primary_container: p.primary_container,
            on_primary_container: p.on_primary_container,
            secondary_container: p.secondary_container,
            on_secondary_container: p.on_secondary_container,
            surface: p.surface,
            surface_container_low: p.surface_container_low,
            surface_container: p.surface_container,
            surface_container_high: p.surface_container_high,
            surface_container_highest: p.surface_container_highest,
            on_surface: p.on_surface,
            on_surface_variant: p.on_surface_variant,
            outline: p.outline,
            outline_variant: p.outline_variant,
        };
        Self {
            seed_rgb: s.seed_rgb,
            achromatic: s.achromatic,
            light: to_role(s.light),
            dark: to_role(s.dark),
        }
    }
}

/// Downsamples an RGBA8 image buffer to `<= 112×112`, trims borders, extracts the
/// perceptual seed (or detects a monochrome cover), and returns its [`FfiThemeScheme`].
#[uniffi::export]
pub fn extract_artwork_scheme(
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    cache_key: Option<String>,
) -> Option<FfiThemeScheme> {
    core::extract_artwork_scheme_rgba(&rgba, width, height, cache_key.as_deref()).map(Into::into)
}

/// Whether `address` is an IP literal in Tailscale's ranges (`100.64.0.0/10`,
/// `fd7a:115c:a1e0::/48`), with or without a `%zone`; false for anything else.
#[uniffi::export]
pub fn is_tailnet_address(address: String) -> bool {
    let ip = address.trim().split('%').next().unwrap_or_default();
    ip.parse::<std::net::IpAddr>().is_ok_and(core::is_tailnet_ip)
}

/// The install ID for a platform ID that survives reinstalls (Android's `ANDROID_ID`):
/// a salted hash, so the raw ID never leaves the device. `None` for an empty input.
#[uniffi::export]
pub fn install_id_from(platform_id: String) -> Option<String> {
    core::install_id_from(&platform_id)
}

/// Sends core logs to the platform log (logcat on Android). Call once,
/// before starting the node. `filter` uses tracing's syntax, e.g. "info".
#[uniffi::export]
pub fn init_logging(filter: String) {
    use tracing_subscriber::{EnvFilter, prelude::*};
    let filter = EnvFilter::try_new(&filter).unwrap_or_else(|_| EnvFilter::new("info"));
    #[cfg(target_os = "android")]
    let layer = paranoid_android::layer("G7Link").with_ansi(false);
    #[cfg(not(target_os = "android"))]
    let layer = tracing_subscriber::fmt::layer();
    let _ = tracing_subscriber::registry().with(filter).with(layer).try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_results_and_away_reach_reach_kotlin_one_to_one() {
        use g7link_core::reach::{AwayReach as R, PeerAddressOutcome as O};
        let outcomes = [
            (O::Cleared, PeerAddressResult::Cleared),
            (O::Ready, PeerAddressResult::Ready),
            (O::NotFound, PeerAddressResult::NotFound),
            (O::NotAllowed, PeerAddressResult::NotAllowed),
            (O::Invalid, PeerAddressResult::Invalid),
        ];
        for (outcome, result) in outcomes {
            assert_eq!(PeerAddressResult::from(outcome), result);
        }
        let reaches = [
            (R::Tailscale, AwayReach::Tailscale),
            (R::MobileData, AwayReach::MobileData),
            (R::TailscaleOff, AwayReach::TailscaleOff),
            (R::HomeOnly, AwayReach::HomeOnly),
        ];
        for (reach, kotlin) in reaches {
            assert_eq!(AwayReach::from(reach), kotlin);
        }
    }

    #[test]
    fn only_tailscale_ip_literals_are_tailnet_addresses() {
        for yes in [
            "100.64.0.1",
            "100.78.127.19",
            " 100.127.255.254 ",
            "fd7a:115c:a1e0::5d01:7fd5",
            "fd7a:115c:a1e0::1%tun0",
        ] {
            assert!(is_tailnet_address(yes.into()), "{yes}");
        }
        for no in [
            "",
            "100.63.255.255",
            "100.128.0.1",
            "192.168.1.20",
            "fd7a:115c:a1e1::1",
            "guchu",
            "100.64.0.1:7718",
            "fe80::1%wlan0",
        ] {
            assert!(!is_tailnet_address(no.into()), "{no}");
        }
    }

    fn core_thread(state: &str) -> core::AgentThread {
        core::AgentThread {
            id: "abc".into(),
            agent: "claude".into(),
            folder: "g7linux".into(),
            title: "Fix it".into(),
            state: state.into(),
            mode: "supervised".into(),
            from_phone: true,
            updated: 1_795_000_000_000,
            asks: 2,
            summary: None,
            error: None,
            error_code: None,
            step: Some("Run command".into()),
            since: Some(1_794_999_000_000),
            plan: Some(vec![
                core::AgentPlanStep { text: "Read it".into(), status: "completed".into() },
                core::AgentPlanStep { text: "Fix it".into(), status: "in_progress".into() },
                core::AgentPlanStep { text: "Later".into(), status: "someday".into() },
            ]),
            ..Default::default()
        }
    }

    fn core_session(state: &str, guess: bool) -> core::AgentSession {
        core::AgentSession {
            id: "sc0123456789abcdef0123456789abcdef".into(),
            agent: "claude".into(),
            folder: "app".into(),
            title: "Fix the login test".into(),
            state: state.into(),
            guess,
            source: "desktop_app".into(),
            updated: 1_795_000_100_000,
            step: None,
            summary: None,
            waiting_for: Some("permission prompt".into()),
            can_continue: false,
            blocked: Some("open".into()),
            since: Some(1_795_000_000_000),
            limit: None,
        }
    }

    #[test]
    fn a_session_at_its_usage_limit_reaches_kotlin_with_its_reset() {
        let pc = core::DeviceId([3; 32]);
        let limited = core::AgentSession {
            limit: Some(core::AgentSessionLimit { resets_ms: Some(1_795_010_000_000) }),
            ..core_session("idle", false)
        };
        match Event::from_node(NodeEvent::AgentSession { device: pc, session: Some(limited), removed: None })
        {
            Some(Event::AgentSessionChanged { session, .. }) => {
                assert_eq!(session.state, AgentSessionState::Idle);
                assert_eq!(session.limit, Some(AgentSessionLimit { resets_ms: Some(1_795_010_000_000) }));
            }
            other => panic!("{other:?}"),
        }
        let unknown = core::AgentSession {
            limit: Some(core::AgentSessionLimit::default()),
            ..core_session("idle", false)
        };
        let unknown = AgentSession::from_core(unknown).unwrap();
        assert_eq!(unknown.limit, Some(AgentSessionLimit { resets_ms: None }), "no time: \"Limit reached\"");
        assert_eq!(AgentSession::from_core(core_session("idle", false)).unwrap().limit, None);
    }

    #[test]
    fn a_threads_step_since_and_plan_reach_kotlin() {
        let thread = AgentThread::from_core(core_thread("working")).unwrap();
        assert_eq!(thread.step.as_deref(), Some("Run command"));
        assert_eq!(thread.since_ms, Some(1_794_999_000_000));
        let plan = thread.plan.unwrap();
        assert_eq!(
            plan.iter().map(|s| s.status).collect::<Vec<_>>(),
            [AgentPlanStatus::Completed, AgentPlanStatus::InProgress],
            "a status this build doesn't know drops that step"
        );
        let older = core::AgentThread { step: None, since: None, plan: None, ..core_thread("done") };
        let older = AgentThread::from_core(older).unwrap();
        assert_eq!((older.step, older.since_ms, older.plan), (None, None, None));
    }

    #[test]
    fn sessions_become_their_own_events_and_a_guess_stays_a_guess() {
        let pc = core::DeviceId([3; 32]);
        let changed = |session: core::AgentSession| {
            Event::from_node(NodeEvent::AgentSession { device: pc, session: Some(session), removed: None })
        };
        match changed(core_session("needs_you", false)) {
            Some(Event::AgentSessionChanged { session, .. }) => {
                assert_eq!(session.state, AgentSessionState::NeedsYou);
                assert_eq!(session.source, AgentSessionSource::DesktopApp);
                assert_eq!(session.blocked, Some(AgentContinueBlock::Open));
                assert_eq!(
                    (session.updated_ms, session.since_ms),
                    (1_795_000_100_000, Some(1_795_000_000_000))
                );
            }
            other => panic!("{other:?}"),
        }
        for (state, guess) in [("needs_you", true), ("may_need_you", false), ("may_need_you", true)] {
            match changed(core_session(state, guess)) {
                Some(Event::AgentSessionChanged { session, .. }) => {
                    assert_eq!(session.state, AgentSessionState::MayNeedYou, "{state} {guess}");
                    assert!(session.guess);
                }
                other => panic!("{other:?}"),
            }
        }
        match changed(core::AgentSession {
            source: "ide".into(),
            blocked: Some("moon".into()),
            ..core_session("idle", false)
        }) {
            Some(Event::AgentSessionChanged { session, .. }) => {
                assert_eq!((session.source, session.blocked), (AgentSessionSource::Other, None));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(changed(core_session("sleeping", false)), None, "unknown words drop the event");
        assert!(matches!(
            Event::from_node(NodeEvent::AgentSession { device: pc, session: None, removed: Some("s1".into()) }),
            Some(Event::AgentSessionRemoved { session, .. }) if session == "s1"
        ));
        let item = core::AgentItem {
            seq: 0,
            id: "u-0".into(),
            kind: "user".into(),
            when: 1_795_000_000_000,
            text: Some("Hi".into()),
            truncated: false,
            tool: None,
            ..Default::default()
        };
        assert!(matches!(
            Event::from_node(NodeEvent::AgentSessionItem { device: pc, session: "s1".into(), item }),
            Some(Event::AgentSessionItem { item, .. }) if item.seq == 0
        ));
        assert_eq!(Event::from_node(NodeEvent::AgentSessionsRequested { device: pc }), None);
        assert_eq!(Event::from_node(NodeEvent::AgentReplyRequested { device: pc }), None);
        let handoff = core::AgentPcHandoff {
            thread: "0123456789abcdef0123456789abcdef".into(),
            seq: Some(42),
            offer: true,
            expires_ms: 1_795_000_600_000,
        };
        assert_eq!(
            Event::from_node(NodeEvent::AgentHandoff { device: pc, handoff }),
            Some(Event::AgentHandoff {
                id: pc.to_string(),
                thread: "0123456789abcdef0123456789abcdef".into(),
                seq: Some(42),
                offer: true,
                expires_ms: 1_795_000_600_000,
            })
        );
        let thread = String::from("0123456789abcdef0123456789abcdef");
        assert_eq!(
            Event::from_node(NodeEvent::AgentBeside {
                device: pc,
                thread: thread.clone(),
                pane: "plan".into()
            }),
            Some(Event::AgentBeside { id: pc.to_string(), thread: thread.clone(), pane: AgentPane::Plan })
        );
        assert_eq!(
            Event::from_node(NodeEvent::AgentBeside { device: pc, thread, pane: "diff_beside".into() }),
            None,
            "a pane this build doesn't know"
        );
    }

    #[test]
    fn a_media_player_keeps_rate_age_art_icon_shuffle_and_repeat() {
        let player = MediaPlayer {
            id: "spotify".into(),
            app: "Spotify".into(),
            title: Some("Song".into()),
            artist: None,
            album: None,
            playing: true,
            duration: Some(200_000),
            position: Some(1_000),
            actions: vec![MediaAction::Play, MediaAction::Shuffle, MediaAction::Repeat],
            art_key: None,
            art: None,
            rate_milli: Some(1_250),
            age_ms: Some(40),
            art_full: Some("big".into()),
            icon: Some("00112233aabbccdd".into()),
            shuffle: Some(false),
            repeat: Some(MediaRepeat::Playlist),
        };
        let core: core::MediaPlayer = player.clone().into();
        assert_eq!(core.actions, ["play", "shuffle", "loop"]);
        assert_eq!((core.repeat.as_deref(), core.age), (Some("playlist"), Some(40)));
        assert_eq!(MediaPlayer::from(core.clone()), player);
        let unknown = core::MediaPlayer { repeat: Some("forever".into()), ..core };
        assert_eq!(MediaPlayer::from(unknown).repeat, None);
        assert_eq!(core::MediaAction::from(MediaAction::Repeat), core::MediaAction::Loop);
    }

    #[test]
    fn agent_events_become_their_own_variants() {
        let pc = core::DeviceId([3; 32]);
        let to_event = |event: core::AgentEvent| {
            Event::from_node(NodeEvent::AgentEvent { device: pc, event: Box::new(event) })
        };
        match to_event(core::AgentEvent::delta("abc".into(), 4, "m:0".into(), 10, "more".into())) {
            Some(Event::AgentDelta { thread, item_id, seq, offset, text, .. }) => {
                assert_eq!(
                    (thread.as_str(), item_id.as_str(), seq, offset, text.as_str()),
                    ("abc", "m:0", 4, 10, "more")
                );
            }
            other => panic!("{other:?}"),
        }
        match to_event(core::AgentEvent::state(core_thread("waiting"))) {
            Some(Event::AgentThreadChanged { thread, .. }) => {
                assert_eq!(thread.state, AgentThreadState::Waiting);
                assert_eq!((thread.asks, thread.updated_ms, thread.from_phone), (2, 1_795_000_000_000, true));
            }
            other => panic!("{other:?}"),
        }
        match to_event(core::AgentEvent::done(core_thread("failed"), "failed", "out".into())) {
            Some(Event::AgentDone { outcome, summary, .. }) => {
                assert_eq!((outcome, summary.as_str()), (AgentOutcome::Failed, "out"));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            to_event(core::AgentEvent::removed("abc".into())),
            Some(Event::AgentThreadRemoved { thread, .. }) if thread == "abc"
        ));
        assert_eq!(
            to_event(core::AgentEvent::state(core_thread("paused"))),
            None,
            "unknown words drop the event"
        );
        assert_eq!(Event::from_node(NodeEvent::AgentsRequested { device: pc }), None);
        let asked = NodeEvent::AgentAsked {
            device: pc,
            token: "t1".into(),
            thread: "abc".into(),
            outcome: "expired".into(),
            by_name: None,
            message: None,
        };
        assert!(matches!(
            Event::from_node(asked),
            Some(Event::AgentAsked { outcome: AgentAskOutcome::Expired, .. })
        ));
        let denied = NodeEvent::AgentAsked {
            device: pc,
            token: "t1".into(),
            thread: "abc".into(),
            outcome: "denied".into(),
            by_name: Some("Pixel".into()),
            message: Some("Use the runner.".into()),
        };
        assert!(matches!(
            Event::from_node(denied),
            Some(Event::AgentAsked { outcome: AgentAskOutcome::Denied, message: Some(m), .. })
                if m == "Use the runner."
        ));
    }

    #[test]
    fn agent_items_carry_turns_phases_and_thinking() {
        use g7link_core::agent;
        let pc = core::DeviceId([3; 32]);
        let core_item = core::AgentItem {
            seq: 3,
            id: "toolu_01:3".into(),
            kind: agent::item::TOOL.into(),
            when: 1_795_000_000_000,
            tool: Some(core::AgentTool {
                name: "Edit".into(),
                title: "Edit file".into(),
                status: agent::tool::DENIED.into(),
                class: Some(agent::tool_class::EDIT.into()),
                path: Some("src/lib.rs".into()),
                by_rule: true,
                ..Default::default()
            }),
            turn: Some(2),
            end: Some(1_795_000_000_900),
            group: Some("toolu_01:1".into()),
            ..Default::default()
        };
        let item = AgentItem::from_core(core_item).unwrap();
        assert_eq!(
            (item.turn, item.end_ms, item.group.as_deref()),
            (2, Some(1_795_000_000_900), Some("toolu_01:1"))
        );
        let tool = item.tool.unwrap();
        assert_eq!((tool.tool_class, tool.path.as_deref()), (Some(AgentToolClass::Edit), Some("src/lib.rs")));
        assert!(tool.by_rule);

        let answer = core::AgentItem {
            seq: 4,
            id: "msg_01:4".into(),
            kind: agent::item::ASSISTANT.into(),
            text: Some("Done.".into()),
            phase: Some(agent::phase::FINAL.into()),
            ..Default::default()
        };
        let answer = AgentItem::from_core(answer).unwrap();
        assert_eq!((answer.phase, answer.turn, answer.end_ms), (Some(AgentItemPhase::Final), 0, None));
        let later = core::AgentItem { phase: Some("summing_up".into()), ..Default::default() };
        let later = core::AgentItem { kind: agent::item::ASSISTANT.into(), id: "m".into(), seq: 5, ..later };
        assert_eq!(AgentItem::from_core(later).unwrap().phase, None, "a later word reads as unknown");

        let thinking = core::AgentItem {
            seq: 5,
            id: "rs_01".into(),
            kind: agent::item::REASONING.into(),
            text: Some(String::new()),
            ..Default::default()
        };
        let event = core::AgentEvent::item("abc".into(), thinking);
        match Event::from_node(NodeEvent::AgentEvent { device: pc, event: Box::new(event) }) {
            Some(Event::AgentItemChanged { item, .. }) => assert_eq!(item.kind, AgentItemKind::Reasoning),
            other => panic!("{other:?}"),
        }
        let denial = core::AgentItem {
            seq: 6,
            id: "u-6".into(),
            kind: agent::item::USER.into(),
            deny_of: Some("toolu_01:3".into()),
            ..Default::default()
        };
        assert_eq!(AgentItem::from_core(denial).unwrap().deny_of.as_deref(), Some("toolu_01:3"));
        let ask = core::AgentAsk {
            token: "t".into(),
            thread: "abc".into(),
            agent: "claude".into(),
            kind: agent::ask::COMMAND.into(),
            item: Some("toolu_01:3".into()),
            ..Default::default()
        };
        assert_eq!(AgentAsk::from_core(ask).unwrap().item.as_deref(), Some("toolu_01:3"));
    }

    #[test]
    fn a_catalog_says_whether_the_pc_takes_words_with_a_denial() {
        let pc = core::DeviceId([3; 32]);
        let catalog = core::AgentCatalog {
            agents: vec![],
            folders: vec![],
            modes: vec![],
            push: false,
            add_folders: false,
        };
        for (can_say_why, can_control) in [(false, false), (true, false), (false, true)] {
            let event = NodeEvent::AgentCatalog {
                device: pc,
                catalog: Some(catalog.clone()),
                can_say_why,
                can_control,
            };
            match Event::from_node(event) {
                Some(Event::AgentCatalog { catalog: Some(c), .. }) => {
                    assert_eq!((c.can_say_why, c.can_control), (can_say_why, can_control));
                }
                other => panic!("{other:?}"),
            }
        }
        assert!(!AgentCatalog::from(catalog.clone()).can_say_why);
        assert!(!AgentCatalog::from(catalog).can_control);
        let mut caps = vec!["agent.remote".to_owned(), core::MIRROR_VIEW.to_owned()];
        with_built_in_capabilities(&mut caps);
        with_built_in_capabilities(&mut caps);
        assert_eq!(
            caps,
            ["agent.remote", core::MIRROR_VIEW, core::agent::ITEMS2, core::agent::CONTROL],
            "once each"
        );
    }

    #[test]
    fn an_agent_catalog_keeps_the_modes_this_build_knows() {
        let catalog = AgentCatalog::from(core::AgentCatalog {
            agents: vec![core::AgentCatalogAgent {
                id: "codex".into(),
                name: "Codex".into(),
                installed: true,
                signed_in: true,
                login_hint: "codex login".into(),
                auth: Some("api_key".into()),
                ..Default::default()
            }],
            folders: vec![core::AgentCatalogFolder {
                id: "f1".into(),
                label: "g7linux".into(),
                checkpoints: true,
                follow_remote: true,
                phone_scripts: true,
                script_count: 1,
                scripts: vec![core::AgentScript {
                    id: "s0a1b2c3d".into(),
                    name: "dev".into(),
                    command: "npm run dev".into(),
                }],
                used: 1_795_000_000_000,
                repo: true,
                last: Some(core::AgentFolderLast {
                    agent: "claude".into(),
                    model: Some("opus".into()),
                    effort: Some("high".into()),
                    title: "Fix the build".into(),
                    branch: Some("main".into()),
                }),
            }],
            modes: vec!["supervised".into(), "plan".into(), "full_access".into()],
            push: true,
            add_folders: true,
        });
        assert!(catalog.push && catalog.add_folders && catalog.folders[0].follow_remote);
        assert!(catalog.folders[0].repo && catalog.folders[0].used == 1_795_000_000_000);
        let last = catalog.folders[0].last.clone().expect("the newest thread");
        assert_eq!(
            (last.agent.as_str(), last.model.as_deref(), last.branch.as_deref()),
            ("claude", Some("opus"), Some("main"))
        );
        assert!(catalog.folders[0].phone_scripts && catalog.folders[0].script_count == 1);
        assert_eq!(catalog.folders[0].scripts[0].command, "npm run dev");
        assert_eq!(catalog.modes, vec![AgentMode::Supervised, AgentMode::Plan]);
        assert_eq!(AgentMode::Plan.wire(), "plan");
        assert_eq!(catalog.agents[0].auth.as_deref(), Some("api_key"));
        assert_eq!(catalog.folders[0].label, "g7linux");
        assert_eq!(AgentMode::AutoEdit.wire(), "auto_edit");
    }

    #[test]
    fn what_control_adds_reaches_kotlin() {
        use g7link_core::agent;
        let image = core::AgentImage { id: "a".repeat(32), name: "shot.png".into(), bytes: 9, w: 4, h: 3 };
        let thread = core::AgentThread {
            mode: agent::mode::PLAN.into(),
            model: Some("opus".into()),
            effort: Some("high".into()),
            answered_model: Some("claude-sonnet-4-5".into()),
            queued: vec![core::AgentQueued {
                id: "0123456789abcdef".into(),
                text: "Then the tests.".into(),
                truncated: true,
                by_name: Some("Pixel".into()),
                images: vec![image.clone()],
                created: 1_795_000_000_000,
            }],
            queue_held: true,
            context: Some(core::AgentContext { used: 1, max: 2 }),
            mcp: Some(core::AgentMcp { on: true, servers: 3, connected: 2, need_login: 1 }),
            asking: 2,
            ..core_thread("waiting")
        };
        let thread = AgentThread::from_core(thread).unwrap();
        assert_eq!(thread.mode, AgentMode::Plan);
        assert_eq!((thread.model.as_deref(), thread.effort.as_deref()), (Some("opus"), Some("high")));
        assert_eq!(thread.queued[0].images[0].width, 4);
        assert!(thread.queued[0].truncated && thread.queue_held);
        assert_eq!(thread.queued[0].created_ms, 1_795_000_000_000);
        assert_eq!(thread.mcp, Some(AgentMcp { on: true, servers: 3, connected: 2, need_login: 1 }));
        assert_eq!((thread.asking, thread.context.map(|c| c.max)), (2, Some(2)));
        let older = AgentThread::from_core(core_thread("done")).unwrap();
        assert!(older.queued.is_empty() && older.model.is_none() && older.asking == 0);

        let item =
            |kind: &str| core::AgentItem { seq: 1, id: "i".into(), kind: kind.into(), ..Default::default() };
        let user = core::AgentItem {
            via: Some(agent::via::STEER.into()),
            images: vec![image],
            ..item(agent::item::USER)
        };
        let user = AgentItem::from_core(user).unwrap();
        assert_eq!((user.via, user.images.len()), (Some(AgentVia::Steer), 1));
        let plan = core::AgentItem { plan_state: Some("replaced".into()), ..item(agent::item::PLAN) };
        let plan = AgentItem::from_core(plan).unwrap();
        assert_eq!((plan.kind, plan.plan_state), (AgentItemKind::Plan, Some(AgentPlanState::Replaced)));
        assert_eq!(AgentItem::from_core(item(agent::item::QUESTION)).unwrap().kind, AgentItemKind::Question);
        let asked = core::AgentItem {
            question: Some(core::AgentItemQuestion {
                count: 2,
                secret: true,
                outcome: Some("dismissed".into()),
                by_name: Some("Pixel".into()),
            }),
            ..item(agent::item::QUESTION)
        };
        let question = AgentItem::from_core(asked).unwrap().question.unwrap();
        assert_eq!((question.count, question.secret), (2, true));
        assert_eq!(
            (question.outcome, question.by_name.as_deref()),
            (Some(AgentQuestionOutcome::Dismissed), Some("Pixel"))
        );
        let later = core::AgentItem { via: Some("teleport".into()), ..item(agent::item::USER) };
        assert_eq!(AgentItem::from_core(later).unwrap().via, None, "a later word reads as absent");

        let catalog = AgentCatalog::from(core::AgentCatalog {
            agents: vec![core::AgentCatalogAgent {
                id: "claude".into(),
                models: vec![core::AgentModel {
                    id: "opus".into(),
                    name: "Opus".into(),
                    is_default: true,
                    efforts: vec!["high".into()],
                    short: true,
                    ..Default::default()
                }],
                can: vec!["plan".into(), "teleport".into(), "mcp_switch".into()],
                limits: vec![core::AgentLimit {
                    window: "300".into(),
                    used_pct: Some(85),
                    resets: Some(1_795_010_000_000),
                    state: "rejected_soon".into(),
                }],
                ..Default::default()
            }],
            ..Default::default()
        });
        let claude = &catalog.agents[0];
        assert!(claude.models[0].is_default && claude.models[0].short);
        assert_eq!(claude.can, [AgentCan::Plan, AgentCan::McpSwitch]);
        assert_eq!(claude.limits[0].state, AgentLimitState::Ok, "a state this build doesn't know");
        assert_eq!(claude.limits[0].resets_ms, Some(1_795_010_000_000));
    }

    #[test]
    fn questions_and_drafts_become_their_own_events() {
        let pc = core::DeviceId([4; 32]);
        let question = core::AgentQuestion {
            token: "t".repeat(32),
            thread: "abc".into(),
            agent: "codex".into(),
            item: "q:1".into(),
            asks: vec![core::AgentQuestionAsk {
                id: "0".into(),
                text: "Which?".into(),
                options: vec![core::AgentQuestionOption { label: "A".into(), note: None }],
                ..Default::default()
            }],
            created: 1,
            expires: 2,
            ..Default::default()
        };
        match Event::from_node(NodeEvent::AgentQuestion { device: pc, question: question.clone() }) {
            Some(Event::AgentQuestion { question, .. }) => {
                assert_eq!(question.asks[0].options[0].label, "A");
                assert_eq!(question.expires_ms, 2);
            }
            other => panic!("{other:?}"),
        }
        let secret = core::AgentQuestion { secret: true, ..question };
        match Event::from_node(NodeEvent::AgentQuestion { device: pc, question: secret }) {
            Some(Event::AgentQuestion { question, .. }) => {
                assert!(question.secret && question.asks.is_empty())
            }
            other => panic!("{other:?}"),
        }
        let questioned = NodeEvent::AgentQuestioned {
            device: pc,
            token: "t".repeat(32),
            thread: "abc".into(),
            outcome: "answered".into(),
            by_name: Some("Pixel".into()),
        };
        assert!(matches!(
            Event::from_node(questioned),
            Some(Event::AgentQuestioned { outcome: AgentQuestionOutcome::Answered, .. })
        ));
        let later = NodeEvent::AgentQuestioned {
            device: pc,
            token: "t".repeat(32),
            thread: "abc".into(),
            outcome: "forgotten".into(),
            by_name: None,
        };
        assert!(Event::from_node(later).is_none(), "an outcome this build doesn't know is dropped");
        assert!(matches!(
            Event::from_node(NodeEvent::AgentDraft { device: pc, key: "abc".into(), rev: 3 }),
            Some(Event::AgentDraftChanged { rev: 3, .. })
        ));
        let shown = format!(
            "{:?} {:?}",
            AgentAnswer { id: "0".into(), labels: vec!["secret label".into()], other: Some("secret".into()) },
            AgentDraft { key: "abc".into(), text: "secret".into(), images: vec![], rev: 1, by_name: None },
        );
        assert!(!shown.contains("secret"), "{shown}");
    }

    #[test]
    fn errors_map_to_actionable_variants() {
        assert_eq!(G7LinkError::from(core::Error::Offline), G7LinkError::Offline);
        assert_eq!(
            G7LinkError::from(core::Error::VersionTooOld { side: core::Side::Remote }),
            G7LinkError::VersionTooOld { this_device: false }
        );
        assert!(matches!(
            G7LinkError::from(core::Error::Internal("x".into())),
            G7LinkError::Internal { reason } if reason == "x"
        ));
    }

    #[test]
    fn a_busy_peer_is_busy_not_denied() {
        assert_eq!(G7LinkError::from(core::Error::Busy), G7LinkError::Busy);
        assert_eq!(G7LinkError::from(core::Error::Denied), G7LinkError::Denied);
    }

    #[test]
    fn a_peer_that_needs_an_update_is_an_event() {
        let id = DeviceId([9u8; 32]);
        assert_eq!(
            Event::from_node(NodeEvent::PeerNeedsUpdate { device: id }),
            Some(Event::PeerNeedsUpdate { id: id.to_string() })
        );
    }

    #[test]
    fn device_info_round_trips() {
        let info = DeviceInfo {
            name: "Pixel".into(),
            kind: DeviceKind::Phone,
            os: "android".into(),
            os_version: "16".into(),
            model: Some("Google Pixel 9".into()),
            accent: Some(0xFF8A5100),
            screen: Some(ScreenShape {
                v: 1,
                aspect: 0.45,
                corners: Some(ScreenCorners { tl: 0.088, tr: 0.088, br: 0.088, bl: 0.088 }),
                cutouts: vec![ScreenRect { x: 0.46, y: 0.018, w: 0.08, h: 0.036 }],
                cutout_path: None,
            }),
            install_id: None,
        };
        assert_eq!(DeviceInfo::from(core::DeviceInfo::from(info.clone())), info);
    }

    #[test]
    fn a_conversation_id_crosses_the_ffi_both_ways_and_stays_out_of_debug() {
        let conv = NotificationConversation {
            v: 1,
            title: "Book Club".into(),
            group: true,
            avatar: None,
            messages: vec![NotificationChatMessage {
                sender: Some("Ana".into()),
                text: "Bring snacks".into(),
                time: 1_760_000_000_000,
                self_sent: false,
                avatar: None,
            }],
            id: Some("h4sh3d_ID-1".into()),
        };
        let core = core::NotificationConversation::from(conv.clone());
        assert_eq!(core.id.as_deref(), Some("h4sh3d_ID-1"));
        assert_eq!(NotificationConversation::from(core), conv);
        assert!(!format!("{conv:?}").contains("h4sh3d"));
        let none = NotificationConversation { id: None, ..conv };
        assert_eq!(core::NotificationConversation::from(none).id, None);
    }

    #[test]
    fn batteries_are_clamped() {
        let b: core::Battery = Battery {
            level: 140,
            charging: true,
            plugged: None,
            full_in: Some(30),
            cell_bars: Some(7),
            cell_type: Some("  5G\n".into()),
            wifi_bars: Some(9),
        }
        .into();
        assert_eq!(b.level, 100);
        assert_eq!(b.full_in, Some(30));
        assert_eq!(b.cell_bars, Some(4));
        assert_eq!(b.cell_type.as_deref(), Some("5G"));
        assert_eq!(b.wifi_bars, Some(4));
        let back = Battery::from(b);
        assert_eq!(back.cell_bars, Some(4));
        assert_eq!(back.cell_type.as_deref(), Some("5G"));
        assert_eq!(back.wifi_bars, Some(4));
    }

    #[test]
    fn pc_audio_frame_event_and_remote_storage_entry() {
        let id = DeviceId([7u8; 32]);
        let ev = Event::from_node(NodeEvent::PcAudioFrame {
            device: id,
            sample_rate: 48_000,
            channels: 2,
            seq: u16::MAX,
            ts_ms: 4_000_000_000,
            pcm: vec![1, 2, 3, 4],
        });
        assert_eq!(
            ev,
            Some(Event::PcAudioFrame {
                device: id.to_string(),
                sample_rate: 48_000,
                channels: 2,
                seq: u16::MAX,
                ts_ms: 4_000_000_000,
                pcm: vec![1, 2, 3, 4],
            })
        );
        let entry = RemoteStorageEntry {
            name: "photo.jpg".into(),
            path: "DCIM/photo.jpg".into(),
            kind: "file".into(),
            size: 4096,
            modified: 1_700_000_000,
        };
        assert_eq!(entry.kind, "file");
    }

    /// Keeps what a sink hands the app.
    #[derive(Debug, Default)]
    struct Heard(std::sync::Mutex<Vec<Event>>);

    impl EventListener for Heard {
        fn on_event(&self, event: Event) {
            self.0.lock().unwrap().push(event);
        }
    }

    #[test]
    fn pc_screen_packets_are_numbered_from_each_config() {
        use g7link_core::MirrorSink;
        let heard = Arc::new(Heard::default());
        let clocks = Arc::new(PcMirrorClocks::default());
        let sink = FfiMirrorSink::new("pc".into(), heard.clone(), clocks);
        let config = |width| core::MirrorConfig { codec: "h264".into(), width, height: 768, session: 0 };
        sink.config(config(1366));
        sink.packet(true, 10, vec![1]);
        sink.packet(false, 20, vec![2]);
        sink.packet(false, 30, vec![3]);
        // A new format restarts the count; a keyframe follows it.
        sink.config(config(1280));
        sink.packet(true, 40, vec![4]);
        sink.packet(false, 50, vec![5]);
        let numbered: Vec<(u64, bool, u64)> = heard
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                Event::PcMirrorFrame { seq, keyframe, time_us, .. } => Some((*seq, *keyframe, *time_us)),
                _ => None,
            })
            .collect();
        assert_eq!(numbered, [(0, true, 10), (1, false, 20), (2, false, 30), (0, true, 40), (1, false, 50)]);
        let configs =
            heard.0.lock().unwrap().iter().filter(|e| matches!(e, Event::PcMirrorConfig { .. })).count();
        assert_eq!(configs, 2);
    }

    #[test]
    fn a_pcs_clock_is_known_once_timed_and_forgotten_by_a_new_stream() {
        use g7link_core::MirrorSink;
        let clocks = Arc::new(PcMirrorClocks::default());
        let heard: Arc<dyn EventListener> = Arc::new(Heard::default());
        let sink = FfiMirrorSink::new("pc".into(), heard.clone(), clocks.clone());
        assert_eq!(clocks.get("pc"), None, "not until the first answer");
        sink.clock(-1_234_567, 3_500);
        assert_eq!(clocks.get("pc"), Some(PcMirrorClock { offset_us: -1_234_567, rtt_us: 3_500 }));
        assert_eq!(clocks.get("other"), None);
        sink.clock(-1_234_000, 2_000);
        assert_eq!(clocks.get("pc").map(|c| c.rtt_us), Some(2_000), "the newest estimate");
        // The PC may have restarted since: its clock is timed again.
        let _next = FfiMirrorSink::new("pc".into(), heard, clocks.clone());
        assert_eq!(clocks.get("pc"), None);
    }

    #[test]
    fn a_pcs_answer_to_see_its_screen_is_a_result_not_an_error() {
        assert_eq!(pc_mirror_start(Ok(())).unwrap(), PcMirrorStart::Started);
        assert_eq!(pc_mirror_start(Err(core::Error::Denied)).unwrap(), PcMirrorStart::Denied);
        assert_eq!(pc_mirror_start(Err(core::Error::Declined)).unwrap(), PcMirrorStart::Denied);
        assert_eq!(pc_mirror_start(Err(core::Error::Unsupported)).unwrap(), PcMirrorStart::Unsupported);
        assert!(pc_mirror_start(Err(core::Error::Offline)).is_err(), "offline stays an error");
        assert!(pc_mirror_start(Err(core::Error::Timeout)).is_err());
        assert!(pc_mirror_start(Err(core::Error::Network("reset".into()))).is_err());
    }

    #[test]
    fn a_pcs_notices_about_its_screen_reach_the_app() {
        let pc = core::DeviceId([7; 32]);
        let notice = |session, code: &str| {
            Event::from_node(NodeEvent::MirrorNotice { device: pc, session, code: code.into() })
        };
        assert_eq!(
            notice(0, "prompt"),
            Some(Event::PcMirrorNotice { id: pc.to_string(), notice: PcMirrorNoticeKind::Prompt })
        );
        assert_eq!(
            notice(0, "no_window"),
            Some(Event::PcMirrorNotice { id: pc.to_string(), notice: PcMirrorNoticeKind::NoWindow })
        );
        assert_eq!(notice(0, core::MIRROR_NOTICE_LOCKED), None, "a phone's notice, for PCs");
        assert_eq!(notice(0, "something_new"), None, "codes this build doesn't know");
        assert_eq!(notice(2, "prompt"), None, "only about the PC's screen");
    }
}

/// `app.g7link.android.core.AndroidContext.nativeInstall(context)`: gives the
/// core the application's `Context`, so it reads the phone's DNS servers
/// (see `g7link_core::install_android_context`). The library is loaded
/// with `System.loadLibrary` for this; JNA's calls use the same copy.
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_g7link_android_core_AndroidContext_nativeInstall(
    env: *mut jni_sys::JNIEnv,
    _class: jni_sys::jclass,
    context: jni_sys::jobject,
) {
    // Set only once it worked, so a failed call can be tried again.
    static INSTALLED: std::sync::Mutex<bool> = std::sync::Mutex::new(false);
    let mut installed = INSTALLED.lock().unwrap_or_else(|e| e.into_inner());
    if *installed || env.is_null() || context.is_null() {
        return;
    }
    // SAFETY: `env` is the calling thread's JNI environment (checked
    // non-null), valid for this call; the global reference keeps `context`
    // alive for good, and the JavaVM lives as long as the process.
    unsafe {
        let functions = &**env;
        let mut vm: *mut jni_sys::JavaVM = std::ptr::null_mut();
        if (functions.v1_1.GetJavaVM)(env, &mut vm) != jni_sys::JNI_OK || vm.is_null() {
            return;
        }
        let global = (functions.v1_1.NewGlobalRef)(env, context);
        if global.is_null() {
            return;
        }
        g7link_core::install_android_context(vm.cast(), global.cast());
    }
    *installed = true;
}
