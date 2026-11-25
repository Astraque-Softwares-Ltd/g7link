// SPDX-License-Identifier: MPL-2.0
//! [`Node`]: the core's entry point. Owns the network endpoint, the trust
//! store, live sessions and one reconnect supervisor per paired device.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex, RwLock, Weak},
    time::Duration,
};

use g7link_protocol::{
    ALPN_PAIR, ALPN_SESSION, ALPN_SESSION_V0, DeviceId, Envelope, ErrorCode,
    messages::{
        Battery, ClipSet, DeviceInfo, DeviceKind, HelloUpdate, MediaCommand, MediaPlayer, Notification,
        NotificationChatMessage, NotificationConversation, NotifyAction, NotifyKey, PowerLevel, Ring, types,
    },
    pairing::PairingUri,
};
use iroh::{
    Endpoint, EndpointAddr, RelayMap, RelayMode, Watcher,
    address_lookup::{DnsAddressLookup, MemoryLookup, PkarrPublisher, PkarrResolver},
    endpoint::{Connection, PortmapperConfig, QuicTransportConfig, VarInt, presets},
    endpoint_info::{EndpointInfo, UserData},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use tokio::sync::{Notify, broadcast};
use tokio_util::sync::CancellationToken;

use crate::{
    Error, NodeConfig, Platform, Result,
    events::{ConnectionPath, DiscoveredDevice, LinkState, NodeEvent, PairedDevice, PairingEvent},
    features::{self, CapabilityMatrix, DeviceFacts, MatrixInputs},
    identity,
    notifications::{self, Feed},
    pairing::{self, PairingState},
    relays::{
        self, HomeRelay, RelayEdit, RelayEditError, RelayProbe, RelayRoute, RelaySettings, RelaySnapshot,
    },
    session::{self, CLOSE_DUPLICATE, CLOSE_NORMAL, REQUEST_TIMEOUT, Session},
    store::Store,
    transfer,
};

const DIAL_TIMEOUT: Duration = Duration::from_secs(15);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
/// The longest wait between redials of an offline device. Waking for a
/// handshake every 30 s all night kept phones from sleeping; a network change,
/// a LAN sighting, the PC dialing in or the phone being unlocked
/// (`network_changed`) all redial at once anyway.
const BACKOFF_MAX: Duration = Duration::from_secs(120);
/// The per-device setting behind [`Node::set_reconnect`], on unless the user
/// turned it off. Not one of the toggles the apps list.
const RECONNECT_TOGGLE: &str = "reconnect";
/// How long the device with the higher ID waits before dialing (see `maintain`).
const DEFER_DIAL: Duration = Duration::from_millis(750);
/// How long a phone plugged into a PC waits for the PC to dial through the cable before
/// dialing itself (see [`Shared::usb_tunnel_inbound`]).
const USB_DIAL_GRACE: Duration = Duration::from_secs(6);
/// Broadcast channel capacity for [`NodeEvent`]s: large enough to absorb initial
/// notification snapshots and multi-peer capability/state bursts without lagging
/// UI or FFI subscribers.
const EVENT_CAPACITY: usize = 2048;
/// Most payload bytes (sound, captured images) the event ring may hold for a
/// subscriber that hasn't caught up before sound is let go (see [`Shared::emit`]).
const EVENT_PAYLOAD_BUDGET: usize = 16 * 1024 * 1024;
/// Most devices remembered as seen on the LAN. Anyone there can announce
/// identities, so when it's full the oldest one not paired makes room.
const MAX_LAN_SIGHTINGS: usize = 256;
/// Most addresses kept from one LAN sighting.
const MAX_SIGHTING_ADDRS: usize = 16;
/// A connection with no traffic for this long is dead, and a quiet one gets a
/// keep-alive this often. Both match iroh's own per-path limits (it pings an
/// idle path every 5 s and drops one silent for 15 s), so a device that
/// drops off shows as offline within ~15 s. Shorter kept the phone's Wi-Fi
/// and CPU from ever sleeping on a connected phone.
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(5);
const IDLE_TIMEOUT: Duration = Duration::from_secs(15);
/// How often this device asks the LAN who's there (mDNS). Answers to others'
/// queries go out at once either way, so only a device showing a nearby list
/// (pairing) needs to ask often; asking every 700 ms all the time kept phones
/// awake (`Node::set_discovery_fast`).
const MDNS_FAST: Duration = Duration::from_millis(700);
const MDNS_SLOW: Duration = Duration::from_secs(30);
/// How long opening a stream the PC asked for (its screen, an app window)
/// waits for the connection to come back if it just dropped or was replaced.
const STREAM_RECONNECT_WAIT: Duration = Duration::from_secs(10);

/// Capabilities every build offers.
const BASE_CAPABILITIES: &[&str] = &[
    "core.ping",
    "device.battery",
    "device.ring",
    "files.transfer",
    "clip.image",
    "media.remote",
    "link.open",
    "task.notify",
    g7link_protocol::messages::UPDATE_HINT,
    g7link_protocol::messages::mirror::PACE,
];

/// This device's mutable description, sent to peers.
#[derive(Debug, Clone)]
pub(crate) struct LocalState {
    pub app_version: String,
    pub device: DeviceInfo,
    pub power: PowerLevel,
    pub extra_capabilities: Vec<String>,
    pub battery: Option<Battery>,
    /// Watching adb for USB phones: can start a phone's Elevated helper over USB.
    pub adb: bool,
}

impl LocalState {
    pub fn capabilities(&self) -> Vec<String> {
        let mut caps: Vec<String> = BASE_CAPABILITIES.iter().map(|c| (*c).to_owned()).collect();
        if !matches!(self.device.kind, DeviceKind::Phone | DeviceKind::Tablet) {
            caps.push(g7link_protocol::messages::continuity_camera::RECEIVE.to_owned());
        }
        if matches!(self.device.kind, DeviceKind::Phone | DeviceKind::Tablet) {
            caps.push(g7link_protocol::messages::agent::REMOTE.to_owned());
        }
        for cap in &self.extra_capabilities {
            if !caps.contains(cap) {
                caps.push(cap.clone());
            }
        }
        if self.adb {
            caps.push(g7link_protocol::messages::elevated::USB.to_owned());
        }
        caps
    }

    /// Whether this device offers `cap`: `capabilities()` without building the list.
    pub fn offers(&self, cap: &str) -> bool {
        BASE_CAPABILITIES.contains(&cap)
            || (cap == g7link_protocol::messages::continuity_camera::RECEIVE
                && !matches!(self.device.kind, DeviceKind::Phone | DeviceKind::Tablet))
            || (cap == g7link_protocol::messages::agent::REMOTE
                && matches!(self.device.kind, DeviceKind::Phone | DeviceKind::Tablet))
            || self.extra_capabilities.iter().any(|c| c == cap)
            || (self.adb && cap == g7link_protocol::messages::elevated::USB)
    }
}

struct Supervisor {
    wake: Arc<Notify>,
    cancel: CancellationToken,
}

/// A PC's reader of a phone's mirroring: the phone, the session, video or sound.
pub(crate) type MirrorStop = (DeviceId, u32, &'static str);

/// State shared by the node, its sessions and background tasks.
/// Notifications kept for listing and search as they arrive, at most: the
/// oldest goes first. (A full snapshot is what the phone shows, which Android
/// itself limits.)
const NOTIFICATION_HISTORY_MAX: usize = 1000;

pub(crate) struct Shared {
    pub id: DeviceId,
    secret: iroh::SecretKey,
    pub endpoint: Endpoint,
    pub store: Store,
    pub platform: Arc<dyn Platform>,
    pub local: RwLock<LocalState>,
    pub pairing: PairingState,
    pub cancel: CancellationToken,
    events: broadcast::Sender<NodeEvent>,
    /// The payload bytes of the events last sent, newest last, as many as the ring holds.
    event_payloads: Mutex<std::collections::VecDeque<usize>>,
    sessions: Mutex<HashMap<DeviceId, Arc<Session>>>,
    links: Mutex<HashMap<DeviceId, LinkState>>,
    supervisors: Mutex<HashMap<DeviceId, Supervisor>>,
    discovered: Mutex<HashMap<DeviceId, DiscoveredDevice>>,
    /// Last LAN mDNS sighting per peer (timestamp and announced socket addresses).
    lan_seen: Mutex<HashMap<DeviceId, (std::time::Instant, Vec<SocketAddr>)>>,
    /// Each paired device's LocalSend fingerprint, from its hello: the
    /// LocalSend nearby list leaves those devices out.
    paired_localsend: Mutex<HashMap<DeviceId, String>>,
    /// The newest build this device heard of, per channel (`announce_update`),
    /// for peers that connect later.
    update_hints: Mutex<HashMap<String, String>>,
    /// The running mDNS's task and whether it asks often (`install_mdns`);
    /// `None` without LAN discovery.
    lan_discovery: Mutex<Option<(CancellationToken, bool)>>,
    /// Whether this device's mDNS record carries its name (`announced_name`).
    name_on_lan: std::sync::atomic::AtomicBool,
    /// Per incoming transfer: whose turn it is on its part files, and how to
    /// tell the one running to stop.
    #[allow(clippy::type_complexity)]
    pub(crate) incoming_turns: Mutex<HashMap<String, (Arc<tokio::sync::Mutex<()>>, CancellationToken)>>,
    /// Consecutive direct/QUIC dial failures per peer since last connected.
    dial_failures: Mutex<HashMap<DeviceId, u32>>,
    /// The last capability matrix emitted per device, to emit only changes.
    matrices: Mutex<HashMap<DeviceId, CapabilityMatrix>>,
    pub(crate) away_mode: std::sync::atomic::AtomicBool,
    relay_settings: Mutex<RelaySettings>,
    relay_map: RelayMap,
    #[cfg(target_os = "linux")]
    relay_connector: Option<crate::direct_network::RelayConnector>,
    /// Whether the user has the app open: heartbeats ping more often then.
    pub(crate) interactive: Arc<crate::session::Interactive>,
    /// The addresses this device dials paired devices at (`remember_addrs`).
    pub(crate) memory: MemoryLookup,
    /// Typed names for paired devices and their lookups (`reach.rs`).
    pub(crate) manual: Mutex<HashMap<DeviceId, crate::reach::ManualAddrs>>,
    /// Told each time a lookup of a typed name ends.
    pub(crate) resolve_done: Notify,
    /// The `device.reach` last sent to every connected peer.
    pub(crate) sent_reach: tokio::sync::Mutex<Option<g7link_protocol::messages::DeviceReach>>,
    /// This device's notifications, when it mirrors them (phones).
    pub notifications: Feed,
    /// Local notification history for fast listing and substring search.
    pub(crate) notification_history: Mutex<Vec<Notification>>,
    /// This device's media players (docs/protocol/media.md).
    pub players: crate::media::Players,
    pub(crate) rings: crate::ring::Rings,
    /// Bluetooth links with paired devices, and requests waiting on them.
    pub(crate) ble: crate::ble_link::BleLinks,
    /// This phone's call in progress (docs/protocol/calls.md).
    pub(crate) calls: crate::calls::Current,
    /// This phone's quick settings state (docs/protocol/toggles.md).
    pub(crate) toggles: crate::toggles::Current,
    /// This PC's Wake-on-LAN adapter addresses (docs/protocol/actions.md).
    pub(crate) wake_info: crate::actions::CurrentWakeInfo,
    /// Active watched tasks on this PC (`task.notify`, `docs/protocol/actions.md`).
    pub(crate) tasks: crate::actions::ActiveTasks,
    /// This PC's Deck layout and live state, and connected PCs' Decks (docs/protocol/deck.md).
    pub(crate) deck: crate::deck::Current,
    /// Agent threads: this PC's catalog and phones' watches, or PCs' catalogs (docs/protocol/agents.md).
    pub(crate) agents: crate::agents::Agents,
    /// Stop signals for phone screens shown here, by phone.
    pub(crate) mirror_stops: Mutex<HashMap<MirrorStop, Arc<tokio::sync::Notify>>>,
    /// Active outgoing mirror session IDs requested by peers (`(peer, session)`).
    pub(crate) outgoing_mirror_sessions: Mutex<std::collections::HashSet<(DeviceId, u32)>>,
    /// Peers granted one-time reverse-mirror consent for the current session.
    pub(crate) pc_mirror_sessions: Mutex<std::collections::HashSet<DeviceId>>,
    /// Play on phone offers this PC made, by phone: the player and when.
    pub(crate) watch_offers: Mutex<HashMap<DeviceId, crate::media::WatchOffer>>,
    /// Stop signals for phone webcam streams arriving here, by phone.
    pub(crate) webcam_stops: Mutex<HashMap<DeviceId, Arc<tokio::sync::Notify>>>,
    /// Stop signals for phone microphone streams arriving here, by phone.
    pub(crate) mic_stops: Mutex<HashMap<DeviceId, Arc<tokio::sync::Notify>>>,
    /// Per-peer rate limiters and prompt state for remote input.
    pub(crate) remote: Mutex<crate::remote::RemoteState>,
    /// Per-peer prompt state and open folders for the storage service.
    pub(crate) storage: Mutex<crate::storage::StorageState>,
    /// Encrypted-at-rest history of the last 50 clips exchanged with peers.
    pub(crate) clipboard_history: crate::clipboard_history::ClipboardHistoryStore,
    /// Smart suggestion for the most recently received text clip, if any.
    pub(crate) last_clip_suggestion: Mutex<Option<(DeviceId, crate::ClipSuggestion)>>,
    pub(crate) private_clip_lifetime: std::time::Duration,
    /// The pending clear of the latest private clipboard value: a newer one replaces it.
    pub(crate) private_clip_clear: Mutex<Option<tokio::task::AbortHandle>>,
    /// LocalSend v2.1 discovery, server, and outgoing transfers.
    pub(crate) localsend: crate::localsend::LocalSendState,
    /// Bound loopback TCP port for the phone-side USB tunnel listener (0 if disabled).
    pub(crate) usb_listener_port: std::sync::atomic::AtomicU16,
    /// Active loopback UDP bridge ports mapped to the `DeviceId` their `UsbHello` claimed.
    pub(crate) usb_bridge_ports: Arc<Mutex<HashMap<u16, DeviceId>>>,
    /// USB tunnels whose plaintext `UsbHello` arrived but whose QUIC identity is not yet verified.
    pub(crate) usb_pending: Mutex<HashMap<u16, PendingUsbTunnel>>,
    /// Active USB tunnel peers mapped to their local UDP bridge address and cancellation token.
    pub(crate) usb_peers: Arc<Mutex<HashMap<DeviceId, (SocketAddr, CancellationToken)>>>,
    /// The adb server this PC watches for phones, if it uses adb.
    pub(crate) adb_server: Option<SocketAddr>,
    /// The adb serial, adb server, and local UDP bridge port each USB-tunnelled phone is on.
    pub(crate) usb_serials: Mutex<HashMap<DeviceId, (String, SocketAddr, u16)>>,
    /// Elevated helpers started over USB, by phone: they run until dropped.
    pub(crate) usb_helpers: Mutex<HashMap<DeviceId, tokio::task::JoinHandle<()>>>,
    /// Peers whose USB tunnel this device accepted (a phone plugged into that PC).
    pub(crate) usb_inbound: Mutex<std::collections::HashSet<DeviceId>>,
    /// USB peers that are only paired by hand (Android emulators).
    pub(crate) usb_manual_pair: Mutex<std::collections::HashSet<DeviceId>>,
    /// Wakes the PC-side ADB watcher immediately on `refresh()`.
    pub(crate) usb_poke: Notify,
    /// Wakes the persistent send-queue coordinator when items are enqueued, retried, or sessions change.
    pub(crate) queue_notify: Arc<Notify>,
    /// Peers currently being drained by a per-peer queue runner task.
    pub(crate) queue_active_peers: Mutex<std::collections::HashSet<DeviceId>>,
    /// Number of wake-up ticks executed by `run_queue_coordinator`.
    pub(crate) queue_coordinator_ticks: std::sync::atomic::AtomicUsize,
    pub data_dir: std::path::PathBuf,
    /// Where received files go.
    pub downloads_dir: std::path::PathBuf,
    /// Running or paused transfers, to pause, resume, or cancel them.
    transfers: Mutex<HashMap<String, TransferControl>>,
    /// The node's runtime, for work started from non-async callers.
    pub(crate) runtime: tokio::runtime::Handle,
    /// One Away mode change at a time.
    away_switch: tokio::sync::Mutex<()>,
    /// Each paired device's capabilities as stored, for checks made per
    /// message (a pointer datagram, a pacing report) without the store.
    peer_caps: RwLock<HashMap<DeviceId, Arc<std::collections::BTreeSet<String>>>>,
    /// RPCs a new session lets this side have in flight to its peer.
    pub(crate) outgoing_rpc_limit: std::sync::atomic::AtomicUsize,
    /// Session tasks running now, across all sessions.
    pub(crate) session_tasks: Arc<std::sync::atomic::AtomicUsize>,
    /// This device's default gateways, read at start and after network changes.
    gateways: RwLock<Vec<std::net::IpAddr>>,
    /// Test hook: the heartbeat sees no allowed path while set.
    debug_refuse_paths: std::sync::atomic::AtomicBool,
    /// When each paired device on an old protocol was last said to need an update.
    pub(crate) needs_update_said: Mutex<HashMap<DeviceId, std::time::Instant>>,
    /// Phone: capture requests waiting on each PC, so a prompt that comes
    /// after its answer is dropped.
    pub(crate) capture_waiting: Mutex<HashMap<DeviceId, usize>>,
}

/// How long a receiver's resume that overtook its pause still counts: the
/// two travel on different streams of one connection, so well under this.
const EARLY_RESUME_WINDOW: Duration = Duration::from_secs(10);

pub(crate) struct TransferControl {
    pub cancel: CancellationToken,
    pub paused: tokio::sync::watch::Sender<bool>,
    pub peer: DeviceId,
    pub direction: transfer::Direction,
    pub paused_transfer: Option<transfer::Transfer>,
    pub running: bool,
    /// When the receiver's `files.resume` came in while this sender didn't
    /// know it was paused yet: the resume travels on the control stream and
    /// can overtake the paused transfer stream's reset.
    pub early_resume: Option<std::time::Instant>,
}

pub(crate) struct PendingUsbTunnel {
    pub peer: DeviceId,
    pub name: String,
    pub bridge_addr: SocketAddr,
    pub cancel: CancellationToken,
    pub inbound: bool,
    pub auto_pair: bool,
    pub serial: Option<(String, SocketAddr)>,
    pub on_verified: Option<Box<dyn FnOnce() + Send>>,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared").field("id", &self.id).finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The text of `n` for the in-memory notification history, which is only
/// listed and searched: its icon, picture and avatars stay out.
fn history_copy(n: &Notification) -> Notification {
    Notification {
        key: n.key.clone(),
        app: n.app.clone(),
        app_name: n.app_name.clone(),
        title: n.title.clone(),
        text: n.text.clone(),
        sub: n.sub.clone(),
        when: n.when,
        actions: n.actions.clone(),
        silent: n.silent,
        ongoing: n.ongoing,
        icon: None,
        image: None,
        live: n.live.clone(),
        conversation: n.conversation.as_ref().map(|c| NotificationConversation {
            v: c.v,
            id: c.id.clone(),
            title: c.title.clone(),
            group: c.group,
            avatar: None,
            messages: c
                .messages
                .iter()
                .map(|m| NotificationChatMessage {
                    sender: m.sender.clone(),
                    text: m.text.clone(),
                    time: m.time,
                    self_sent: m.self_sent,
                    avatar: None,
                })
                .collect(),
        }),
    }
}

/// The bytes `event` carries beyond its own size, for the ones that can carry a lot.
fn payload_bytes(event: &NodeEvent) -> usize {
    match event {
        NodeEvent::PcAudioFrame { pcm, .. } => pcm.len(),
        NodeEvent::CameraCaptureReceived { data, .. } => data.len(),
        _ => 0,
    }
}

/// The notification history's order: newest first, then by key.
fn history_order(a: &Notification, b: &Notification) -> std::cmp::Ordering {
    b.when.cmp(&a.when).then_with(|| a.key.cmp(&b.key))
}

impl Shared {
    pub fn emit(&self, event: NodeEvent) {
        let bytes = payload_bytes(&event);
        // Sent under the lock, so the sizes stay in the ring's order.
        let mut payloads = lock(&self.event_payloads);
        if matches!(event, NodeEvent::PcAudioFrame { .. }) {
            // An event stays in the ring until every subscriber has seen it: one that
            // lags mustn't keep sound piling up there, so past the budget sound is let go.
            let held: usize = payloads.iter().rev().take(self.events.len()).sum();
            if held.saturating_add(bytes) > EVENT_PAYLOAD_BUDGET {
                return;
            }
        }
        if payloads.len() == EVENT_CAPACITY {
            payloads.pop_front();
        }
        payloads.push_back(bytes);
        // No subscribers is fine; events are a UI convenience.
        let _ = self.events.send(event);
    }

    pub(crate) fn peer_name(&self, peer: &DeviceId) -> String {
        self.store
            .get_peer(peer)
            .ok()
            .flatten()
            .map(|p| p.info.name)
            .or_else(|| self.localsend.peer_name(peer))
            .unwrap_or_else(|| peer.short())
    }

    pub(crate) fn record_timeline(&self, mut entry: crate::timeline::NewTimelineEntry) -> Option<i64> {
        if entry.device_name.is_empty() {
            entry.device_name = self.peer_name(&entry.device_id);
        }
        let id = self.store.insert_timeline(&entry, crate::now_unix()).ok()?;
        self.emit(NodeEvent::TimelineChanged);
        Some(id)
    }

    pub(crate) fn record_timeline_clip(
        &self,
        peer: DeviceId,
        peer_name: &str,
        incoming: bool,
        detail: &str,
        clip_id: String,
        evicted_ids: Vec<String>,
    ) {
        for evicted in evicted_ids {
            let _ = self.store.delete_timeline_by_ref(&evicted);
        }
        let _ = self.store.delete_timeline_by_ref(&clip_id);
        let _ = self.record_timeline(crate::timeline::NewTimelineEntry {
            kind: crate::TimelineKind::Clip,
            device_id: peer,
            device_name: peer_name.to_owned(),
            incoming,
            timestamp: crate::now_unix(),
            title: String::new(),
            detail: detail.to_owned(),
            target: String::new(),
            size_bytes: 0,
            duration_secs: 0,
            ref_id: Some(clip_id),
        });
    }

    pub(crate) fn finish_timeline_session(&self, row_id: i64, duration_secs: u64) {
        if self.store.update_timeline_duration(row_id, duration_secs.max(1)).unwrap_or(false) {
            self.emit(NodeEvent::TimelineChanged);
        }
    }

    pub(crate) fn nudge_reconnect(&self) {
        for supervisor in lock(&self.supervisors).values() {
            supervisor.wake.notify_one();
        }
    }

    /// Where partly received files wait until complete.
    pub fn incoming_dir(&self) -> std::path::PathBuf {
        self.data_dir.join("incoming")
    }

    /// Where staged files for the persistent outgoing send queue live until delivered or cancelled.
    pub fn queue_staging_dir(&self) -> std::path::PathBuf {
        self.data_dir.join("queue_staging")
    }

    pub fn register_transfer(&self, id: &str, cancel: CancellationToken) {
        let (paused, _) = tokio::sync::watch::channel(false);
        lock(&self.transfers).insert(
            id.to_owned(),
            TransferControl {
                cancel,
                paused,
                peer: DeviceId([0; 32]),
                direction: transfer::Direction::Incoming,
                paused_transfer: None,
                running: true,
                early_resume: None,
            },
        );
    }

    /// Waits for this incoming transfer's turn: one that's still running (its
    /// connection just dropped, and it may still be writing) is told to stop,
    /// and this returns once it has. Two receivers on one part file would
    /// scramble it. The turn lasts as long as the returned guard.
    pub(crate) async fn incoming_turn(self: &Arc<Self>, id: &str) -> transfer::IncomingTurn {
        let (lock, stop) = {
            let mut turns = lock(&self.incoming_turns);
            let entry = turns
                .entry(id.to_owned())
                .or_insert_with(|| (Arc::new(tokio::sync::Mutex::new(())), CancellationToken::new()));
            entry.1.cancel();
            entry.1 = CancellationToken::new();
            (entry.0.clone(), entry.1.clone())
        };
        let held = lock.lock_owned().await;
        transfer::IncomingTurn { shared: self.clone(), id: id.to_owned(), stop, held: Some(held) }
    }

    pub(crate) fn register_transfer_control(
        &self,
        id: &str,
        peer: DeviceId,
        direction: transfer::Direction,
        cancel: CancellationToken,
    ) -> tokio::sync::watch::Receiver<bool> {
        let (paused, rx) = tokio::sync::watch::channel(false);
        let running = direction == transfer::Direction::Incoming;
        lock(&self.transfers).insert(
            id.to_owned(),
            TransferControl {
                cancel,
                paused,
                peer,
                direction,
                paused_transfer: None,
                running,
                early_resume: None,
            },
        );
        rx
    }

    pub(crate) fn ensure_transfer_control(
        &self,
        id: &str,
        peer: DeviceId,
        direction: transfer::Direction,
    ) -> (CancellationToken, tokio::sync::watch::Receiver<bool>) {
        let mut map = lock(&self.transfers);
        if let Some(ctrl) = map.get_mut(id) {
            ctrl.running = true;
            return (ctrl.cancel.clone(), ctrl.paused.subscribe());
        }
        let cancel = CancellationToken::new();
        let (paused, rx) = tokio::sync::watch::channel(false);
        map.insert(
            id.to_owned(),
            TransferControl {
                cancel: cancel.clone(),
                paused,
                peer,
                direction,
                paused_transfer: None,
                running: true,
                early_resume: None,
            },
        );
        (cancel, rx)
    }

    pub(crate) fn mark_transfer_idle(&self, id: &str) {
        if let Some(ctrl) = lock(&self.transfers).get_mut(id) {
            ctrl.running = false;
        }
    }

    /// A send attempt ended paused, by this side or the receiver: stays
    /// paused and returns true, unless the receiver already resumed it
    /// ([`TransferControl::early_resume`]), when sending goes on.
    pub(crate) fn transfer_paused_by_attempt(&self, id: &str) -> bool {
        let mut transfers = lock(&self.transfers);
        let Some(ctrl) = transfers.get_mut(id) else { return true };
        if *ctrl.paused.borrow() {
            return true;
        }
        if ctrl.early_resume.take().is_some_and(|at| at.elapsed() < EARLY_RESUME_WINDOW) {
            return false;
        }
        let _ = ctrl.paused.send(true);
        true
    }

    pub(crate) fn mark_transfer_paused(&self, id: &str, transfer: transfer::Transfer) {
        if let Some(ctrl) = lock(&self.transfers).get_mut(id)
            && !ctrl.cancel.is_cancelled()
        {
            let _ = ctrl.paused.send(true);
            ctrl.paused_transfer = Some(transfer);
        }
    }

    pub(crate) fn resume_transfer_from_peer(&self, id: &str, peer: DeviceId) {
        if let Some(ctrl) = lock(&self.transfers).get_mut(id)
            && (ctrl.peer == peer || ctrl.peer == DeviceId([0; 32]))
        {
            if !*ctrl.paused.borrow() && ctrl.direction == transfer::Direction::Outgoing {
                ctrl.early_resume = Some(std::time::Instant::now());
            }
            let _ = ctrl.paused.send(false);
            ctrl.paused_transfer = None;
        }
    }

    pub(crate) async fn cancel_transfer_from_peer(&self, id: &str, peer: DeviceId) {
        let (found_active, paused) = {
            let mut map = lock(&self.transfers);
            if let Some(ctrl) = map.get_mut(id)
                && (ctrl.peer == peer || ctrl.peer == DeviceId([0; 32]))
            {
                ctrl.cancel.cancel();
                let paused = ctrl.paused_transfer.take();
                if paused.is_some() {
                    map.remove(id);
                }
                (paused.is_none(), paused)
            } else {
                (false, None)
            }
        };
        if !found_active {
            let dir = self.incoming_dir().join(peer.to_string()).join(id);
            let _ = tokio::fs::remove_dir_all(&dir).await;
            if let Some(parent) = dir.parent() {
                let _ = tokio::fs::remove_dir(parent).await;
            }
        }
        if let Some(mut t) = paused {
            t.state = transfer::TransferState::Cancelled;
            self.emit(NodeEvent::Transfer(t));
        }
    }

    pub(crate) fn cancel_queue_or_transfer(self: &Arc<Self>, id: &str) -> Result<bool> {
        self.localsend.resolve_transfer_approval(id, false);
        // The live transfer is cancelled first, so a database error can't
        // leave it running.
        let target = {
            let mut map = lock(&self.transfers);
            if let Some(ctrl) = map.get_mut(id) {
                ctrl.cancel.cancel();
                let peer = ctrl.peer;
                let direction = ctrl.direction;
                let paused = ctrl.paused_transfer.take();
                let running = ctrl.running;
                if paused.is_some() || !running {
                    map.remove(id);
                }
                Some((peer, direction, paused, running))
            } else {
                None
            }
        };
        let removed_queue = match self.store.remove_queue_item(id) {
            Ok(removed) => removed,
            // Only an error when there was nothing live to cancel either.
            Err(e) if target.is_none() => return Err(e),
            Err(e) => {
                tracing::warn!(error = %e, %id, "can't remove a cancelled send from the queue");
                None
            }
        };

        if let Some((ref q_item, ref payload)) = removed_queue {
            transfer::cleanup_staging_id(self, id);
            self.emit(NodeEvent::QueueChanged);
            self.queue_notify.notify_waiters();
            let was_running = target.as_ref().is_some_and(|(_, _, _, running)| *running);
            if !was_running {
                let is_file_item = matches!(
                    payload,
                    crate::QueuePayload::Files { .. }
                        | crate::QueuePayload::Folder { .. }
                        | crate::QueuePayload::HandoffDocument { .. }
                        | crate::QueuePayload::Recording { .. }
                );
                if is_file_item {
                    let (recording, markers, open_on_arrival) = match payload {
                        crate::QueuePayload::Recording { markers, .. } => (true, markers.clone(), false),
                        crate::QueuePayload::HandoffDocument { .. } => (false, Vec::new(), true),
                        _ => (false, Vec::new(), false),
                    };
                    let mut names: Vec<String> = Vec::new();
                    let mut last_folder: Option<String> = None;
                    for spec in &q_item.files {
                        match spec.folder.as_deref().map(|f| f.split('/').next().unwrap_or(f)) {
                            Some(top) if last_folder.as_deref() == Some(top) => {}
                            Some(top) => {
                                names.push(top.to_owned());
                                last_folder = Some(top.to_owned());
                            }
                            None => {
                                names.push(spec.name.clone());
                                last_folder = None;
                            }
                        }
                    }
                    self.emit(NodeEvent::Transfer(transfer::Transfer {
                        id: q_item.id.clone(),
                        device: q_item.peer,
                        direction: transfer::Direction::Outgoing,
                        names,
                        files: q_item.files_total as usize,
                        total: q_item.bytes_total,
                        done: q_item.bytes_done,
                        state: transfer::TransferState::Cancelled,
                        recording,
                        markers,
                        open_on_arrival,
                    }));
                }
            }
        }

        if let Some((peer, direction, paused, _running)) = target {
            let was_paused = paused.is_some();
            if let Some(mut t) = paused {
                t.state = transfer::TransferState::Cancelled;
                self.emit(NodeEvent::Transfer(t));
            }
            if peer != DeviceId([0; 32]) {
                let shared = self.clone();
                let id_owned = id.to_owned();
                self.runtime.spawn(async move {
                    if was_paused && direction == transfer::Direction::Incoming {
                        let dir = shared.incoming_dir().join(peer.to_string()).join(&id_owned);
                        let _ = tokio::fs::remove_dir_all(&dir).await;
                        if let Some(parent) = dir.parent() {
                            let _ = tokio::fs::remove_dir(parent).await;
                        }
                    }
                    if let Some(session) = shared.session(&peer)
                        && let Ok(env) = Envelope::new(
                            types::FILES_CANCEL,
                            &g7link_protocol::messages::FilesControl { id: id_owned },
                        )
                    {
                        let _ = session.send(env).await;
                    }
                });
            }
            return Ok(true);
        }

        Ok(removed_queue.is_some())
    }

    pub fn unregister_transfer(&self, id: &str) {
        lock(&self.transfers).remove(id);
    }

    /// Every connected device's session.
    pub fn live_sessions(&self) -> Vec<Arc<Session>> {
        lock(&self.sessions).values().filter(|s| s.is_alive()).cloned().collect()
    }

    /// Whether this device reconnects to `peer` by itself (see [`Node::set_reconnect`]).
    pub(crate) fn reconnects(&self, peer: &DeviceId) -> bool {
        self.store.toggle(peer, RECONNECT_TOGGLE).ok().flatten().unwrap_or(true)
    }

    /// Whether a device toggle (see [`features::DEVICE_TOGGLES`]) is on for
    /// `peer`; unknown toggles are off.
    /// Signs `message` with this device's key.
    pub(crate) fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.secret.sign(message).to_bytes()
    }

    /// `peer`'s capabilities as last stored, from memory; read from the
    /// store the first time.
    pub(crate) fn peer_caps(&self, peer: &DeviceId) -> Arc<std::collections::BTreeSet<String>> {
        if let Some(caps) = self.peer_caps.read().unwrap_or_else(|e| e.into_inner()).get(peer) {
            return caps.clone();
        }
        self.refresh_peer_caps(peer)
    }

    /// Whether paired `peer` offers `cap`, from memory.
    pub(crate) fn peer_offers_cap(&self, peer: &DeviceId, cap: &str) -> bool {
        self.peer_caps(peer).contains(cap)
    }

    /// Re-reads `peer`'s capabilities after they were stored, or after it was
    /// forgotten. Call it wherever capabilities are written.
    pub(crate) fn refresh_peer_caps(&self, peer: &DeviceId) -> Arc<std::collections::BTreeSet<String>> {
        let caps = self.store.get_peer(peer).ok().flatten().map(|p| Arc::new(p.caps));
        let mut cache = self.peer_caps.write().unwrap_or_else(|e| e.into_inner());
        match caps {
            Some(caps) => {
                cache.insert(*peer, caps.clone());
                caps
            }
            None => {
                cache.remove(peer);
                Arc::default()
            }
        }
    }

    /// Whether the test hook [`Node::debug_refuse_paths`] is on.
    pub(crate) fn debug_paths_refused(&self) -> bool {
        self.debug_refuse_paths.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn toggle_on(&self, peer: &DeviceId, toggle: &str) -> bool {
        self.store
            .toggle(peer, toggle)
            .ok()
            .flatten()
            .or_else(|| features::toggle_default(toggle))
            .unwrap_or(false)
    }

    /// The capabilities this device offers right now.
    pub fn local_capabilities(&self) -> Vec<String> {
        self.local.read().unwrap_or_else(|e| e.into_inner()).capabilities()
    }

    /// Whether this device offers `cap` right now (checked per message, so it
    /// doesn't build the list).
    pub fn we_offer(&self, cap: &str) -> bool {
        self.local.read().unwrap_or_else(|e| e.into_inner()).offers(cap)
    }

    pub fn session(&self, peer: &DeviceId) -> Option<Arc<Session>> {
        lock(&self.sessions).get(peer).filter(|s| s.is_alive()).cloned()
    }

    fn set_link(&self, peer: DeviceId, link: LinkState) {
        let changed = lock(&self.links).insert(peer, link.clone()).as_ref() != Some(&link);
        if changed {
            self.emit(NodeEvent::LinkChanged { device: peer, link });
            self.route_refresh(&peer);
        }
    }

    pub fn link(&self, peer: &DeviceId, last_seen: Option<i64>) -> LinkState {
        lock(&self.links).get(peer).cloned().unwrap_or(LinkState::Offline { last_seen })
    }

    /// Publishes the session's current path and round-trip time, if it's
    /// still the peer's session (a replaced one has nothing to say).
    pub fn publish_online(&self, session: &Session) {
        let sessions = lock(&self.sessions);
        if sessions
            .get(&session.peer)
            .is_some_and(|current| std::ptr::eq(current.as_ref(), session) && current.is_alive())
        {
            self.set_link(session.peer, LinkState::Online { path: session.path(), rtt_ms: session.rtt_ms() });
        }
    }

    /// Whether `session`'s path is no longer the one its link was last
    /// published with (only while that link says it's online).
    pub(crate) fn path_moved(&self, session: &Session) -> bool {
        let published = match lock(&self.links).get(&session.peer) {
            Some(LinkState::Online { path, .. }) => *path,
            _ => return false,
        };
        published != session.path()
    }

    pub fn away_mode(&self) -> bool {
        self.away_mode.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn is_allowed_peer_ip(&self, peer: &DeviceId, ip: std::net::IpAddr) -> bool {
        self.away_mode() || self.is_lan_peer_ip(peer, ip)
    }

    /// Remembers that `peer` announced `addrs` on this LAN just now.
    pub(crate) fn remember_lan_sighting(&self, peer: DeviceId, mut addrs: Vec<SocketAddr>) {
        addrs.truncate(MAX_SIGHTING_ADDRS);
        let mut by_age: Vec<(std::time::Instant, DeviceId)> = {
            let seen = lock(&self.lan_seen);
            if seen.contains_key(&peer) || seen.len() < MAX_LAN_SIGHTINGS {
                Vec::new()
            } else {
                seen.iter().map(|(id, (at, _))| (*at, *id)).collect()
            }
        };
        // Full: the oldest sightings of devices not paired go (looked up without the lock held).
        by_age.sort_unstable();
        let excess = (by_age.len() + 1).saturating_sub(MAX_LAN_SIGHTINGS);
        let evicted: Vec<DeviceId> = by_age
            .into_iter()
            .map(|(_, id)| id)
            .filter(|id| !self.store.is_paired(id).unwrap_or(false))
            .take(excess)
            .collect();
        let mut seen = lock(&self.lan_seen);
        for id in &evicted {
            seen.remove(id);
        }
        seen.insert(peer, (std::time::Instant::now(), addrs));
    }

    /// Whether `ip` reaches `peer` on this network (`session::is_lan_peer_ip`),
    /// from the addresses `peer` last announced over mDNS here.
    pub(crate) fn is_lan_peer_ip(&self, peer: &DeviceId, ip: std::net::IpAddr) -> bool {
        crate::session::is_lan_ip(ip)
            || lock(&self.lan_seen)
                .get(peer)
                .is_some_and(|(_, announced)| crate::session::is_lan_peer_ip(ip, announced))
    }

    /// The heartbeat's path rule for a live session: a direct path this
    /// device allows, or, with Away mode off, any path to a peer that is this
    /// device's default gateway and was reached there on a LAN path in this
    /// session (the laptop on the phone's own hotspot: the phone's mobile
    /// address answers too, and QUIC may settle on it).
    pub(crate) fn session_path_allowed(&self, session: &Session) -> bool {
        let gateways = self.gateways.read().unwrap_or_else(|e| e.into_inner()).clone();
        if self.is_connection_allowed(&session.conn) {
            let paths = session.conn.paths();
            let lan_ips = paths.iter().filter_map(|p| match p.remote_addr() {
                iroh::TransportAddr::Ip(addr) if !p.is_relay() && crate::session::is_lan_ip(addr.ip()) => {
                    Some(addr.ip().to_canonical())
                }
                _ => None,
            });
            for ip in lan_ips {
                if gateways.contains(&ip) {
                    session.note_reached_at_gateway(ip);
                }
            }
            return true;
        }
        allowed_via_gateway(&gateways, session.reached_at_gateway())
    }

    /// Reads this device's default gateways again (after a network change).
    pub(crate) fn refresh_gateways(&self) {
        *self.gateways.write().unwrap_or_else(|e| e.into_inner()) = default_gateways();
    }

    pub(crate) fn is_connection_allowed(&self, conn: &Connection) -> bool {
        if self.away_mode() || self.is_usb_connection(conn) {
            return true;
        }
        // Any direct LAN (or USB) path will do: a connection often reports its
        // relay path first while the direct one comes up.
        let peer = crate::device_id(&conn.remote_id());
        conn.paths().iter().any(|p| {
            !p.is_relay()
                && match p.remote_addr() {
                    iroh::TransportAddr::Ip(addr) => {
                        if self.is_usb_addr(addr) {
                            self.is_usb_addr_for(&peer, addr)
                        } else {
                            self.is_allowed_peer_ip(&peer, addr.ip())
                        }
                    }
                    _ => false,
                }
        })
    }

    pub(crate) async fn wait_connection_allowed(&self, conn: &Connection) -> bool {
        if self.away_mode() || self.is_usb_connection(conn) {
            return true;
        }
        // Up to 200 ms for a direct path, not just for the first path of any kind.
        for _ in 0..20 {
            if self.is_connection_allowed(conn) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.is_connection_allowed(conn)
    }

    /// Our direct addresses, for pairing links. Waits briefly for the
    /// endpoint to learn them right after startup; if no non-loopback
    /// interface is up yet (e.g. right after a PC restart while Wi-Fi is still
    /// associating), falls back to `127.0.0.1:<port>` from the bound socket.
    pub async fn direct_addrs(&self) -> Vec<SocketAddr> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let addrs: Vec<SocketAddr> = self
                .endpoint
                .addr()
                .ip_addrs()
                .copied()
                .filter(|a| self.away_mode() || crate::session::is_lan_ip(a.ip()))
                .collect();
            if addrs.iter().any(|a| !a.ip().is_loopback()) || tokio::time::Instant::now() >= deadline {
                if !addrs.is_empty() {
                    return addrs;
                }
                return self
                    .bound_v4_port()
                    .map(|port| vec![SocketAddr::from(([127, 0, 0, 1], port))])
                    .unwrap_or_default();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Returns the wildcard IPv4 UDP port used by the USB bridge.
    pub(crate) fn bound_v4_port(&self) -> Option<u16> {
        self.endpoint
            .bound_sockets()
            .into_iter()
            .find(|addr| addr.is_ipv4() && addr.ip().is_unspecified() && addr.port() != 0)
            .map(|addr| addr.port())
    }

    pub(crate) fn register_usb_bridge_port(&self, port: u16, peer: DeviceId) {
        lock(&self.usb_bridge_ports).insert(port, peer);
    }

    pub(crate) fn unregister_usb_bridge_port(&self, port: u16) {
        lock(&self.usb_bridge_ports).remove(&port);
        lock(&self.usb_pending).remove(&port);
    }

    pub(crate) fn cancel_pending_usb_port(&self, port: u16) {
        if let Some(pending) = lock(&self.usb_pending).remove(&port) {
            pending.cancel.cancel();
        }
    }

    pub(crate) fn is_usb_addr(&self, addr: &SocketAddr) -> bool {
        addr.ip().to_canonical().is_loopback() && lock(&self.usb_bridge_ports).contains_key(&addr.port())
    }

    pub(crate) fn is_usb_addr_for(&self, peer: &DeviceId, addr: &SocketAddr) -> bool {
        addr.ip().to_canonical().is_loopback() && lock(&self.usb_bridge_ports).get(&addr.port()) == Some(peer)
    }

    pub(crate) fn usb_peer_addr(&self, peer: &DeviceId) -> Option<SocketAddr> {
        lock(&self.usb_peers).get(peer).map(|(addr, _)| *addr)
    }

    pub(crate) fn usb_peer_or_pending_addr(&self, peer: &DeviceId) -> Option<(SocketAddr, bool)> {
        if let Some(addr) = self.usb_peer_addr(peer) {
            return Some((addr, false));
        }
        lock(&self.usb_pending).values().find(|p| &p.peer == peer).map(|p| (p.bridge_addr, true))
    }

    /// Whether `peer` is a PC this phone is plugged into: the PC dials through the cable.
    /// If this device dialed too (over Wi-Fi, say), the two connections would replace each
    /// other, and a Wi-Fi path that only works one way would keep breaking the USB one.
    pub(crate) fn usb_tunnel_inbound(&self, peer: &DeviceId) -> bool {
        lock(&self.usb_inbound).contains(peer) && self.usb_peer_addr(peer).is_some()
    }

    /// Whether `peer` is a phone this PC is plugged into: this device dialed the
    /// tunnel to a phone it found over ADB. The phone's end, a loopback port,
    /// is open to any app on the phone.
    pub(crate) fn usb_tunnel_dialed(&self, peer: &DeviceId) -> bool {
        self.usb_peer_addr(peer).is_some() && !lock(&self.usb_inbound).contains(peer)
    }

    pub(crate) fn is_usb_connection(&self, conn: &Connection) -> bool {
        let peer = crate::device_id(&conn.remote_id());
        if self.usb_peer_addr(&peer).is_some() {
            return true;
        }
        conn.paths().iter().any(|p| match p.remote_addr() {
            iroh::TransportAddr::Ip(addr) => self.is_usb_addr_for(&peer, addr),
            _ => false,
        })
    }

    /// Verifies that if `conn` uses a local USB bridge port, its QUIC TLS `DeviceId`
    /// matches the `UsbHello` recorded for that bridge port, promoting the pending
    /// USB tunnel only once authenticated.
    pub(crate) async fn verify_usb_connection(self: &Arc<Self>, conn: &Connection) -> bool {
        let peer = crate::device_id(&conn.remote_id());
        let has_pending_for_peer = || lock(&self.usb_pending).values().any(|p| p.peer == peer);
        let max_iters = if has_pending_for_peer() { 25 } else { 1 };
        for i in 0..max_iters {
            let mut matched_ports = Vec::new();
            for p in conn.paths().iter() {
                let iroh::TransportAddr::Ip(addr) = p.remote_addr() else { continue };
                if !addr.ip().to_canonical().is_loopback() {
                    continue;
                }
                let Some(expected) = lock(&self.usb_bridge_ports).get(&addr.port()).copied() else {
                    continue;
                };
                if expected != peer {
                    self.cancel_pending_usb_port(addr.port());
                    return false;
                }
                matched_ports.push(addr.port());
            }
            if !matched_ports.is_empty() {
                for port in matched_ports {
                    let pending = lock(&self.usb_pending).remove(&port);
                    if let Some(pending) = pending {
                        self.activate_usb_tunnel(pending);
                    }
                }
                return true;
            }
            if i + 1 < max_iters && has_pending_for_peer() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            } else {
                break;
            }
        }
        true
    }

    fn activate_usb_tunnel(self: &Arc<Self>, mut pending: PendingUsbTunnel) {
        let peer = pending.peer;
        let bridge_addr = pending.bridge_addr;
        let bridge_port = bridge_addr.port();
        if let Some(cb) = pending.on_verified.take() {
            cb();
        }
        if let Some((old_addr, old_cancel)) =
            lock(&self.usb_peers).insert(peer, (bridge_addr, pending.cancel))
            && old_addr != bridge_addr
        {
            old_cancel.cancel();
        }
        if pending.inbound {
            lock(&self.usb_inbound).insert(peer);
        } else {
            lock(&self.usb_inbound).remove(&peer);
            if pending.auto_pair {
                lock(&self.usb_manual_pair).remove(&peer);
            } else {
                lock(&self.usb_manual_pair).insert(peer);
            }
            if let Some((serial, adb_addr)) = pending.serial {
                lock(&self.usb_serials).insert(peer, (serial, adb_addr, bridge_port));
            }
        }
        self.note_discovered(peer, (!pending.name.is_empty()).then_some(pending.name));
        self.remember_addrs(&peer, &[bridge_addr]);
        if self.store.is_paired(&peer).unwrap_or(false) {
            self.start_supervisor(peer);
            self.wake_supervisor(&peer);
            if let Some(session) = self.session(&peer) {
                self.publish_online(&session);
            }
            self.refresh_capabilities(&peer);
        } else if !pending.inbound && pending.auto_pair {
            let s = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if !s.store.is_paired(&peer).unwrap_or(false) {
                    let _ = pairing::start_usb_auto(&s, peer).await;
                }
            });
        }
    }

    /// This phone's pairing screen just opened: starts the code check with
    /// each unpaired PC it's plugged into. The PC tried when the cable went
    /// in, and the phone refused while its pairing screen was closed.
    pub(crate) fn pair_with_plugged_in_pcs(self: &Arc<Self>) {
        let pcs: Vec<DeviceId> = lock(&self.usb_inbound).iter().copied().collect();
        for pc in pcs {
            if self.usb_tunnel_inbound(&pc) && !self.store.is_paired(&pc).unwrap_or(false) {
                let s = self.clone();
                tokio::spawn(async move {
                    let _ = pairing::start_nearby(&s, pc).await;
                });
            }
        }
    }

    pub(crate) fn note_discovered(&self, peer: DeviceId, name: Option<String>) {
        if self.store.is_paired(&peer).unwrap_or(false) {
            return;
        }
        let device = DiscoveredDevice { id: peer, name };
        let is_new = lock(&self.discovered).insert(peer, device.clone()).as_ref() != Some(&device);
        if is_new {
            self.emit(NodeEvent::Discovered(device));
        }
    }

    pub(crate) fn on_usb_tunnel_accepted(
        self: &Arc<Self>,
        peer: DeviceId,
        name: String,
        bridge_addr: SocketAddr,
        cancel: CancellationToken,
    ) {
        let port = bridge_addr.port();
        if let Some(old) = lock(&self.usb_pending).insert(
            port,
            PendingUsbTunnel {
                peer,
                name,
                bridge_addr,
                cancel: cancel.clone(),
                inbound: true,
                auto_pair: false,
                serial: None,
                on_verified: None,
            },
        ) {
            old.cancel.cancel();
        }
        let s = self.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = tokio::time::sleep(DIAL_TIMEOUT + Duration::from_secs(5)) => {
                    s.cancel_pending_usb_port(port);
                }
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn on_usb_tunnel_connected(
        self: &Arc<Self>,
        peer: DeviceId,
        name: String,
        bridge_addr: SocketAddr,
        cancel: CancellationToken,
        auto_pair: bool,
        serial: Option<(String, SocketAddr)>,
        on_verified: Option<Box<dyn FnOnce() + Send>>,
    ) {
        let port = bridge_addr.port();
        if let Some(old) = lock(&self.usb_pending).insert(
            port,
            PendingUsbTunnel {
                peer,
                name,
                bridge_addr,
                cancel,
                inbound: false,
                auto_pair,
                serial,
                on_verified,
            },
        ) {
            old.cancel.cancel();
        }
        let s = self.clone();
        tokio::spawn(async move {
            s.probe_usb_tunnel(peer, bridge_addr).await;
        });
    }

    pub(crate) async fn on_usb_tunnel_disconnected(&self, peer: DeviceId, bridge_addr: SocketAddr) {
        let bridge_port = bridge_addr.port();
        lock(&self.usb_pending).remove(&bridge_port);
        let removed = {
            let mut peers = lock(&self.usb_peers);
            if peers.get(&peer).is_some_and(|(addr, _)| *addr == bridge_addr) {
                peers.remove(&peer);
                lock(&self.usb_inbound).remove(&peer);
                lock(&self.usb_manual_pair).remove(&peer);
                let mut serials = lock(&self.usb_serials);
                if serials.get(&peer).is_some_and(|(_, _, port)| *port == bridge_port) {
                    serials.remove(&peer);
                    // The cable is out: a helper started over it has stopped with it.
                    if let Some(helper) = lock(&self.usb_helpers).remove(&peer) {
                        helper.abort();
                    }
                }
                true
            } else {
                false
            }
        };
        if !removed {
            return;
        }
        if !self.store.is_paired(&peer).unwrap_or(false) && !lock(&self.lan_seen).contains_key(&peer) {
            if lock(&self.discovered).remove(&peer).is_some() {
                self.emit(NodeEvent::DiscoveryExpired(peer));
            }
            return;
        }
        if let Ok(Some(record)) = self.store.get_peer(&peer) {
            self.remember_addrs(&peer, &self.known_addrs(&record));
        }
        self.endpoint.network_change().await;
        if let Some(session) = self.session(&peer) {
            if session.uses_usb_bridge_port(bridge_port) {
                session.close(CLOSE_NORMAL, b"usb disconnected");
            } else {
                self.publish_online(&session);
            }
        }
        self.wake_supervisor(&peer);
        self.refresh_capabilities(&peer);
    }

    pub(crate) fn retry_usb_auto_pair(self: &Arc<Self>) {
        let peers: Vec<DeviceId> = lock(&self.usb_peers).keys().copied().collect();
        let manual = lock(&self.usb_manual_pair).clone();
        for peer in peers.into_iter().filter(|p| !manual.contains(p)) {
            if !self.store.is_paired(&peer).unwrap_or(false) {
                let s = self.clone();
                tokio::spawn(async move {
                    let _ = pairing::start_usb_auto(&s, peer).await;
                });
            }
        }
    }

    /// Whether the dialer may be given `addr` for `peer`: the cable's
    /// address; else never a loopback one, and with Away mode off only this
    /// network's (`is_lan_peer_ip`: private, link-local, tailnet, or announced
    /// over mDNS here).
    pub(crate) fn dialable(&self, peer: &DeviceId, addr: &SocketAddr) -> bool {
        self.is_usb_addr_for(peer, addr) || self.ip_dialable(peer, addr.ip())
    }

    /// [`Shared::dialable`] for an address that isn't the cable's.
    pub(crate) fn ip_dialable(&self, peer: &DeviceId, ip: std::net::IpAddr) -> bool {
        !ip.to_canonical().is_loopback() && (self.away_mode() || self.is_lan_peer_ip(peer, ip))
    }

    /// Feeds known addresses of a peer into address lookup.
    pub fn remember_addrs(&self, peer: &DeviceId, addrs: &[SocketAddr]) {
        let allow_wan = self.away_mode();
        let mut all: Vec<SocketAddr> = addrs.iter().copied().filter(|a| self.dialable(peer, a)).collect();
        if let Some(usb_addr) = self.usb_peer_addr(peer)
            && !all.contains(&usb_addr)
        {
            all.push(usb_addr);
        }
        if all.is_empty() && allow_wan {
            return;
        }
        if let Ok(key) = crate::public_key(peer) {
            self.memory.add_endpoint_info(EndpointInfo::new(key).with_ip_addrs(all));
        }
    }

    /// Removes any cached non-LAN (cellular / public IPv6 / WAN) or stale loopback
    /// addresses for `peer` so `maintain` does not repeatedly dial a dead endpoint.
    pub(crate) fn purge_non_lan_addrs(&self, peer: &DeviceId) {
        if let Ok(Some(mut record)) = self.store.get_peer(peer) {
            let lan_only: Vec<SocketAddr> = record
                .last_addrs
                .iter()
                .copied()
                .filter(|a| !a.ip().to_canonical().is_loopback() && self.is_lan_peer_ip(peer, a.ip()))
                .collect();
            if lan_only.len() != record.last_addrs.len()
                && let Some(seen) = record.last_seen
            {
                let _ = self.store.record_seen(peer, seen, &lan_only);
            }
            record.last_addrs = lan_only;
            self.remember_addrs(peer, &self.known_addrs(&record));
        }
    }

    /// Computes the capability matrix for a paired device from its last known
    /// capabilities, this device's state and the user's toggles.
    pub fn capability_matrix(&self, peer: &DeviceId) -> Result<CapabilityMatrix> {
        let record = self.store.get_peer(peer)?.ok_or(Error::NotPaired)?;
        let toggles = self.store.toggles(peer)?;
        let local = {
            let local = self.local.read().unwrap_or_else(|e| e.into_inner());
            DeviceFacts::new(&local.device, local.capabilities().into_iter().collect(), local.power)
        };
        let remote = DeviceFacts::new(&record.info, record.caps, record.power);
        let mut paths = vec![ConnectionPath::Lan];
        if self.usb_peer_addr(peer).is_some()
            || self.session(peer).is_some_and(|s| s.path() == ConnectionPath::Usb)
        {
            paths.push(ConnectionPath::Usb);
        }
        if self.away_mode() {
            paths.push(ConnectionPath::Relay);
        }
        Ok(features::compute(
            *peer,
            MatrixInputs { local: &local, peer: &remote, paths: &paths, toggles: &toggles },
        ))
    }

    /// Recomputes a device's capability matrix and emits it if it changed.
    pub fn refresh_capabilities(&self, peer: &DeviceId) {
        let matrix = match self.capability_matrix(peer) {
            Ok(matrix) => matrix,
            Err(Error::NotPaired) => return,
            Err(e) => {
                tracing::warn!(peer = %peer.short(), error = %e, "failed to compute capabilities");
                return;
            }
        };
        let changed = lock(&self.matrices).insert(*peer, matrix.clone()).as_ref() != Some(&matrix);
        if changed {
            self.emit(NodeEvent::Capabilities(matrix));
        }
        self.route_caps_changed(peer);
    }

    /// Recomputes every paired device's matrix (after a local change).
    fn refresh_all_capabilities(&self) {
        match self.store.list_peers() {
            Ok(peers) => peers.iter().for_each(|p| self.refresh_capabilities(&p.id)),
            Err(e) => tracing::warn!(error = %e, "failed to list peers"),
        }
    }

    /// Stores a newly paired device and starts keeping it connected.
    pub async fn complete_pairing(self: &Arc<Self>, peer: DeviceId, info: DeviceInfo) -> Result<()> {
        let info = info.sanitized();
        let paired_at = crate::now_unix();
        let mut reassigned_queue = false;
        // The same physical device paired again (after a reinstall it has a new identity):
        // move its history and queue to the new pairing and forget the old one. Only the
        // install ID decides; same-name devices (two family phones) stay separate.
        let online_peers: Vec<DeviceId> = self.live_sessions().into_iter().map(|s| s.peer).collect();
        let replaced: Vec<DeviceId> = match info.install_id.as_deref() {
            Some(install_id) => self
                .store
                .list_peers()
                .unwrap_or_default()
                .into_iter()
                .filter(|old| {
                    old.id != peer
                        && !online_peers.contains(&old.id)
                        && old.info.kind == info.kind
                        && old.info.install_id.as_deref() == Some(install_id)
                })
                .map(|old| old.id)
                .collect(),
            None => Vec::new(),
        };
        // Store the new pairing first: if that fails, the old one is untouched.
        // Storing it also moves the old entry's history and queue over.
        self.store.upsert_peer_skipping(&peer, &info, paired_at, &online_peers)?;
        self.refresh_peer_caps(&peer);
        for old in replaced {
            let _ = self.store.reassign_timeline(&old, &peer);
            let _ = self.store.reassign_send_queue_peer(&old, &peer);
            reassigned_queue = true;
            let _ = self.forget_peer(&old).await;
            // Its row is already gone, so forget_peer doesn't announce it.
            self.emit(NodeEvent::DeviceRemoved(old));
        }
        tracing::info!(peer = %peer.short(), "paired");
        let device = PairedDevice {
            id: peer,
            info,
            paired_at,
            link: LinkState::Offline { last_seen: None },
            can_wake: false,
            reconnect: true,
        };
        self.emit(NodeEvent::DeviceAdded(device.clone()));
        self.emit(NodeEvent::Pairing(PairingEvent::Paired(device)));
        if reassigned_queue {
            self.emit(NodeEvent::QueueChanged);
        }
        lock(&self.discovered).remove(&peer);
        self.start_supervisor(peer);
        self.queue_notify.notify_waiters();
        Ok(())
    }

    /// Removes a paired device locally (after we or the peer unpaired).
    pub async fn forget_peer(self: &Arc<Self>, peer: &DeviceId) -> Result<()> {
        let peer_name = self.peer_name(peer);
        let dropped_count = self
            .store
            .list_send_queue(Some(peer))
            .map(|items| {
                u32::try_from(
                    items
                        .iter()
                        .filter(|i| {
                            matches!(i.status, crate::QueueStatus::Waiting | crate::QueueStatus::Sending)
                        })
                        .count(),
                )
                .unwrap_or(u32::MAX)
            })
            .unwrap_or(0);
        let active_transfer_ids: Vec<String> = lock(&self.transfers)
            .iter()
            .filter(|(_, ctrl)| &ctrl.peer == peer)
            .map(|(id, ctrl)| {
                ctrl.cancel.cancel();
                id.clone()
            })
            .collect();
        for id in active_transfer_ids {
            lock(&self.transfers).remove(&id);
        }
        // One transaction: an error leaves both the device and its queue.
        let (existed, dropped_queue) = self.store.remove_peer_with_queue(peer)?;
        self.refresh_peer_caps(peer);
        for (id, _) in &dropped_queue {
            transfer::cleanup_staging_id(self, id);
        }
        if let Some(sup) = lock(&self.supervisors).remove(peer) {
            sup.cancel.cancel();
        }
        if let Some(session) = lock(&self.sessions).remove(peer) {
            session.close(CLOSE_NORMAL, b"unpaired");
        }
        lock(&self.links).remove(peer);
        lock(&self.lan_seen).remove(peer);
        self.set_manual_name(peer, None);
        lock(&self.dial_failures).remove(peer);
        lock(&self.matrices).remove(peer);
        lock(&self.remote).remove_peer(peer);
        lock(&self.storage).remove_peer(peer);
        self.toggles.remove_peer(peer);
        self.deck.remove_peer(peer);
        if self.agents.remove_peer(peer) {
            crate::agents::unwatch_on_platform(self, *peer);
        }
        self.clear_remote_ring(*peer, None);
        let peer_id = *peer;
        self.stop_local_ring_if(move |ring| ring.peer == peer_id).await;
        self.notifications.clear_peer(peer);
        self.stop_webcam(peer);
        // The session was removed above, so `on_session_end` won't clear these:
        // a one-time "show my screen" must not outlive the pairing.
        lock(&self.pc_mirror_sessions).remove(peer);
        lock(&self.watch_offers).remove(peer);
        self.stop_mic(peer);
        self.stop_showing_all(peer);
        self.take_outgoing_mirror_sessions(peer);
        // Unpaired, it's a LocalSend device like any other again.
        if lock(&self.paired_localsend).remove(peer).is_some() {
            self.emit(NodeEvent::LocalSendChanged);
        }
        if !dropped_queue.is_empty() {
            self.emit(NodeEvent::QueueChanged);
            if dropped_count > 0 {
                self.emit(NodeEvent::QueueDroppedOnUnpair { peer: *peer, peer_name, dropped_count });
            }
        }
        self.close_ble_link(peer);
        self.forget_ble(peer);
        if existed {
            self.emit(NodeEvent::DeviceRemoved(*peer));
            self.emit(NodeEvent::BleKeysChanged);
        }
        Ok(())
    }

    /// Registers an established session, resolving duplicates: if both
    /// devices dialed at once, the connection opened by the device with the
    /// lower ID wins on both sides.
    fn register_session(
        self: &Arc<Self>,
        conn: Connection,
        dialer: DeviceId,
        streams: (iroh::endpoint::SendStream, iroh::endpoint::RecvStream),
        remote: g7link_protocol::messages::Hello,
    ) -> Option<Arc<Session>> {
        let peer = crate::device_id(&conn.remote_id());
        if conn.close_reason().is_some() {
            return None;
        }
        if !self.is_connection_allowed(&conn) {
            tracing::debug!(peer = %peer.short(), "refusing session on non-LAN path while away_mode is off");
            conn.close(VarInt::from_u32(CLOSE_NORMAL), b"away mode disabled");
            return None;
        }
        // Unpaired while the handshake ran (both sides check before it).
        if !self.store.is_paired(&peer).unwrap_or(false) {
            tracing::debug!(peer = %peer.short(), "refusing session: no longer paired");
            conn.close(VarInt::from_u32(ErrorCode::Unpaired.close_code()), b"unpaired");
            return None;
        }
        // Either side may have dialed: while the user has reconnecting off,
        // this device stays disconnected from that one.
        if !self.reconnects(&peer) {
            tracing::debug!(peer = %peer.short(), "refusing session: reconnecting is off for this device");
            conn.close(VarInt::from_u32(CLOSE_NORMAL), b"reconnect off");
            return None;
        }
        let is_usb = self.is_usb_connection(&conn);
        let mut sessions = lock(&self.sessions);
        if let Some(existing) = sessions.get(&peer).filter(|s| s.is_alive()) {
            let keep_existing = if is_usb && existing.path() != ConnectionPath::Usb {
                false
            } else if !is_usb && existing.path() == ConnectionPath::Usb {
                true
            } else {
                existing.dialer != dialer && existing.dialer < dialer
            };
            if keep_existing {
                drop(sessions);
                conn.close(VarInt::from_u32(CLOSE_DUPLICATE), b"duplicate");
                return None;
            }
            existing.close(CLOSE_DUPLICATE, b"duplicate");
        }

        let localsend = remote.localsend_fingerprint().map(str::to_ascii_lowercase);
        let localsend_changed = {
            let mut known = lock(&self.paired_localsend);
            match &localsend {
                Some(f) => known.insert(peer, f.clone()).as_ref() != Some(f),
                None => known.remove(&peer).is_some(),
            }
        };
        if localsend_changed {
            self.emit(NodeEvent::LocalSendChanged);
        }
        let remote_device = remote.device.sanitized();
        if let Err(e) = self.store.update_info(&peer, &remote_device) {
            tracing::warn!(error = %e, "failed to update peer info");
        }
        // What the apps show is what's stored, so the event carries that.
        let effective_device = self.store.get_peer(&peer).ok().flatten().map(|p| p.info);
        let caps = features::sanitize_capabilities(remote.caps);
        if let Err(e) = self.store.update_capabilities(&peer, Some(&caps), Some(remote.power.effective())) {
            tracing::warn!(error = %e, "failed to update peer capabilities");
        }
        self.refresh_peer_caps(&peer);

        let weak = Arc::downgrade(self);
        let session = Session::spawn(self, conn, dialer, streams, move |ended| {
            if let Some(shared) = weak.upgrade() {
                shared.on_session_end(&ended);
            }
        });
        sessions.insert(peer, session.clone());
        drop(sessions);
        lock(&self.dial_failures).remove(&peer);

        tracing::info!(peer = %peer.short(), path = ?session.path(), "connected");
        let allow_wan = self.away_mode();
        let addrs: Vec<SocketAddr> = session
            .remote_ip_addrs()
            .into_iter()
            .filter(|a| {
                !a.ip().to_canonical().is_loopback() && (allow_wan || self.is_lan_peer_ip(&peer, a.ip()))
            })
            .collect();
        let persisted_addrs = if addrs.is_empty() {
            self.store
                .get_peer(&peer)
                .ok()
                .flatten()
                .map(|r| {
                    r.last_addrs
                        .into_iter()
                        .filter(|a| {
                            !a.ip().to_canonical().is_loopback()
                                && (allow_wan || self.is_lan_peer_ip(&peer, a.ip()))
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            addrs
        };
        if let Err(e) = self.store.record_seen(&peer, crate::now_unix(), &persisted_addrs) {
            tracing::warn!(error = %e, "failed to record peer addresses");
        }
        self.remember_addrs(&peer, &persisted_addrs);
        if let Some(info) = effective_device {
            self.emit(NodeEvent::PeerInfoChanged { device: peer, info });
        }
        self.emit(NodeEvent::PeerPowerChanged { device: peer, power: remote.power.effective() });
        self.refresh_capabilities(&peer);
        self.publish_online(&session);
        // The apps hear the device is online before anything it sends.
        session.mark_ready();
        self.queue_notify.notify_waiters();

        // Bring the peer up to date with state it may have missed.
        let battery = self.local.read().unwrap_or_else(|e| e.into_inner()).battery.clone();
        let shared = self.clone();
        let s = session.clone();
        tokio::spawn(async move {
            if let Some(battery) = battery
                && let Ok(env) = Envelope::new(types::DEVICE_BATTERY, &battery)
            {
                let _ = s.send(env).await;
            }
            shared.send_media_state(&s).await;
            shared.send_bt_media_state(&s).await;
            shared.send_ble_key(&s).await;
            shared.send_held_notifications(&s).await;
            shared.send_notification_snapshot(&s).await;
            shared.send_call_state(&s).await;
            shared.send_toggles_state(&s).await;
            crate::actions::send_wake_info(&shared, &s).await;
            shared.send_reach(&s).await;
            crate::actions::send_active_tasks(&shared, &s).await;
            shared.send_deck(&s).await;
            crate::agents::send_catalog(&shared, &s).await;
            shared.send_update_hints(&s).await;
        });
        Some(session)
    }

    fn on_session_end(&self, ended: &Arc<Session>) {
        let paired = self.store.is_paired(&ended.peer).unwrap_or(false);
        let mut sessions = lock(&self.sessions);
        let current = sessions.get(&ended.peer).is_some_and(|current| Arc::ptr_eq(current, ended));
        if current {
            sessions.remove(&ended.peer);
            self.clear_remote_ring(ended.peer, None);
            lock(&self.pc_mirror_sessions).remove(&ended.peer);
            lock(&self.watch_offers).remove(&ended.peer);
            lock(&self.storage).remove_peer(&ended.peer);
            self.toggles.remove_peer(&ended.peer);
            self.deck.remove_peer(&ended.peer);
            if self.agents.session_ended(&ended.peer) {
                crate::agents::unwatch_on_platform(self, ended.peer);
            }
            if paired {
                // Why, as QUIC tells it: timed out, closed by the other side...
                let reason = ended.conn.close_reason().map(|r| r.to_string()).unwrap_or_default();
                tracing::info!(peer = %ended.peer.short(), %reason, "disconnected");
                // Still under the lock: a new session for the peer can't
                // register (and say it's online) until this is said, so a
                // stale "offline" never lands after a fresh "online".
                self.set_link(ended.peer, LinkState::Offline { last_seen: Some(crate::now_unix()) });
            }
            drop(sessions);
            self.stop_showing_all(&ended.peer);
            self.stop_webcam(&ended.peer);
            self.stop_mic(&ended.peer);
            let outgoing = self.take_outgoing_mirror_sessions(&ended.peer);
            if !outgoing.is_empty() {
                let peer = ended.peer;
                let platform = self.platform.clone();
                tokio::task::spawn_blocking(move || {
                    for session_id in outgoing {
                        platform.mirror_stop_requested(&peer, session_id);
                    }
                });
            }
            let _ = self.store.clear_send_queue_retry_delays(&ended.peer);
            self.queue_notify.notify_waiters();
        }
    }

    fn start_supervisor(self: &Arc<Self>, peer: DeviceId) {
        let mut supervisors = lock(&self.supervisors);
        if supervisors.contains_key(&peer) {
            return;
        }
        let wake = Arc::new(Notify::new());
        let cancel = self.cancel.child_token();
        supervisors.insert(peer, Supervisor { wake: wake.clone(), cancel: cancel.clone() });
        tokio::spawn(maintain(Arc::downgrade(self), peer, wake, cancel));
    }

    pub(crate) fn wake_supervisor(&self, peer: &DeviceId) {
        if let Some(sup) = lock(&self.supervisors).get(peer) {
            sup.wake.notify_one();
        }
    }

    /// Dials a paired device and runs the handshake.
    async fn dial(self: &Arc<Self>, peer: DeviceId) -> Result<Arc<Session>> {
        struct CloseOnDrop(Option<Connection>);
        impl Drop for CloseOnDrop {
            fn drop(&mut self) {
                if let Some(conn) = self.0.take() {
                    conn.close(VarInt::from_u32(CLOSE_NORMAL), b"dial cancelled");
                }
            }
        }

        let mut addr = EndpointAddr::new(crate::public_key(&peer)?);
        if let Some(usb_addr) = self.usb_peer_addr(&peer) {
            addr = addr.with_ip_addr(usb_addr);
        }
        let conn = tokio::time::timeout(DIAL_TIMEOUT, self.endpoint.connect(addr, ALPN_SESSION))
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(|e| {
                if refused_alpn(&e) {
                    // A paired device that doesn't speak this protocol: it needs an update.
                    Error::VersionTooOld { side: crate::Side::Remote }
                } else {
                    crate::error::net(e)
                }
            })?;
        let mut guard = CloseOnDrop(Some(conn));
        let conn_ref = guard.0.as_ref().expect("connection present");
        if !self.verify_usb_connection(conn_ref).await || !self.wait_connection_allowed(conn_ref).await {
            tracing::debug!(peer = %peer.short(), "refusing outgoing session on non-LAN path while away_mode is off");
            if let Some(c) = guard.0.take() {
                c.close(VarInt::from_u32(CLOSE_NORMAL), b"away mode disabled");
            }
            return Err(Error::Offline);
        }
        // The other side's dial landed meanwhile and that session is up and
        // heard from: keep it rather than tear it down for this one.
        if let Some(live) = self.session(&peer).filter(|s| s.is_settled_and_heard()) {
            if let Some(c) = guard.0.take() {
                c.close(VarInt::from_u32(session::CLOSE_DUPLICATE), b"duplicate");
            }
            return Ok(live);
        }
        let hello = session::local_hello(self);
        let (send, recv, remote) = session::handshake_dialer(conn_ref, &hello).await?;
        if let Some(live) = self.session(&peer).filter(|s| s.is_settled_and_heard()) {
            if let Some(c) = guard.0.take() {
                c.close(VarInt::from_u32(session::CLOSE_DUPLICATE), b"duplicate");
            }
            return Ok(live);
        }
        let conn = guard.0.take().expect("connection present");
        self.register_session(conn, self.id, (send, recv), remote)
            .or_else(|| self.session(&peer))
            .ok_or(Error::Offline)
    }

    /// Verifies a newly connected USB tunnel by completing a QUIC TLS 1.3 handshake
    /// exclusively over `bridge_addr` before trusting `UsbHello`.
    pub(crate) async fn probe_usb_tunnel(self: &Arc<Self>, peer: DeviceId, bridge_addr: SocketAddr) {
        let Ok(key) = crate::public_key(&peer) else {
            self.cancel_pending_usb_port(bridge_addr.port());
            return;
        };
        let Ok(builder) = Endpoint::builder(presets::Minimal)
            .secret_key(self.secret.clone())
            .relay_mode(RelayMode::Disabled)
            .portmapper_config(PortmapperConfig::Disabled)
            .clear_ip_transports()
            .bind_addr(SocketAddr::from(([127, 0, 0, 1], 0)))
        else {
            self.cancel_pending_usb_port(bridge_addr.port());
            return;
        };
        let Ok(probe_ep) = builder.bind().await else {
            self.cancel_pending_usb_port(bridge_addr.port());
            return;
        };
        let addr = EndpointAddr::new(key).with_ip_addr(bridge_addr);
        let Ok(Ok(conn)) = tokio::time::timeout(DIAL_TIMEOUT, probe_ep.connect(addr, ALPN_SESSION)).await
        else {
            self.cancel_pending_usb_port(bridge_addr.port());
            probe_ep.close().await;
            return;
        };
        if !self.verify_usb_connection(&conn).await || self.usb_peer_addr(&peer) != Some(bridge_addr) {
            self.cancel_pending_usb_port(bridge_addr.port());
        }
        conn.close(VarInt::from_u32(CLOSE_NORMAL), b"usb probe");
        probe_ep.close().await;
    }

    /// Sends a message to every connected device.
    /// Tells `session`'s peer of the newest builds this device heard of.
    async fn send_update_hints(&self, session: &Session) {
        if !self.offers(&session.peer, g7link_protocol::messages::UPDATE_HINT).unwrap_or(false) {
            return;
        }
        let hints: Vec<(String, String)> = lock(&self.update_hints).clone().into_iter().collect();
        for (channel, version) in hints {
            let hint = g7link_protocol::messages::UpdateHint { channel, version };
            if let Ok(env) = Envelope::new(types::DEVICE_UPDATE, &hint) {
                let _ = session.send(env).await;
            }
        }
    }

    async fn broadcast(&self, env: Envelope) {
        for session in self.live_sessions() {
            if let Err(e) = session.send(env.clone()).await {
                tracing::debug!(peer = %session.peer.short(), error = %e, "broadcast failed");
            }
        }
    }
}

/// Dials `peer`, interrupting an in-flight non-USB dial if a USB tunnel comes up while waiting.
async fn dial_with_usb_interrupt(
    node: &Arc<Shared>,
    peer: DeviceId,
    wake: &Notify,
) -> Option<Result<Arc<Session>>> {
    let had_usb = node.usb_peer_addr(&peer).is_some();
    if had_usb {
        return Some(node.dial(peer).await);
    }
    let dial_fut = node.dial(peer);
    tokio::pin!(dial_fut);
    loop {
        tokio::select! {
            res = &mut dial_fut => return Some(res),
            _ = wake.notified() => {
                if node.usb_peer_addr(&peer).is_some() && node.session(&peer).is_none() {
                    return None;
                }
            }
        }
    }
}

/// Keeps one paired device connected: waits while a session is alive,
/// otherwise dials with exponential backoff. Woken early by LAN discovery.
async fn maintain(shared: Weak<Shared>, peer: DeviceId, wake: Arc<Notify>, cancel: CancellationToken) {
    let mut backoff = BACKOFF_MIN;
    // Set once this phone has given its PC the chance to dial through the cable.
    let mut waited_for_usb = false;
    loop {
        let Some(node) = shared.upgrade() else { return };
        if cancel.is_cancelled() {
            return;
        }
        // Reconnecting is off for this device: don't dial until the user
        // turns it back on (which wakes this loop).
        if node.session(&peer).is_none() && !node.reconnects(&peer) {
            drop(node);
            tokio::select! {
                _ = wake.notified() => {}
                _ = cancel.cancelled() => return,
            }
            backoff = BACKOFF_MIN;
            waited_for_usb = false;
            continue;
        }
        // Unplugged before the PC dialled: the next plug-in gets its grace again.
        if !node.usb_tunnel_inbound(&peer) {
            waited_for_usb = false;
        }
        if node.session(&peer).is_none() && node.usb_tunnel_inbound(&peer) && !waited_for_usb {
            // Plugged in: the PC dials through the cable. Dial only if it hasn't by then.
            waited_for_usb = true;
            drop(node);
            let deadline = tokio::time::Instant::now() + USB_DIAL_GRACE;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => break,
                    _ = wake.notified() => {}
                    _ = cancel.cancelled() => return,
                }
                if shared.upgrade().is_some_and(|n| n.session(&peer).is_some()) {
                    break;
                }
            }
            continue;
        }
        if let Some(session) = node.session(&peer) {
            waited_for_usb = false;
            drop(node);
            tokio::select! {
                _ = session.cancel.cancelled() => {}
                _ = cancel.cancelled() => return,
            }
            // A session that was up and closed for liveness or a path is
            // dialled again at once, however short it was; only a refusal
            // from the other side waits, longer each time.
            let ended = end_reason(&session);
            backoff = next_redial_wait(ended, backoff);
            if ended == EndReason::Closed || shared.upgrade().is_some_and(|n| n.session(&peer).is_some()) {
                continue;
            }
            // Refused: purge non-LAN addresses and wait, so this never flaps in a tight loop.
            if let Some(n) = shared.upgrade() {
                n.purge_non_lan_addrs(&peer);
            }
            let jitter = backoff.mul_f64(rand::random::<f64>() * 0.2);
            tokio::select! {
                _ = tokio::time::sleep(backoff + jitter) => {}
                _ = wake.notified() => {}
                _ = cancel.cancelled() => return,
            }
            continue;
        }
        // A typed name is looked up again now and then; the dial doesn't wait for it.
        node.refresh_manual(&peer);
        // When both devices come up together they'd dial each other at once and
        // one connection would be dropped. The higher ID briefly defers so the
        // lower one usually wins; it still dials in case the other can't reach it.
        // Skip the deferral if an outbound USB tunnel is already ready.
        if node.id > peer && node.usb_peer_addr(&peer).is_none() {
            drop(node);
            tokio::select! {
                _ = tokio::time::sleep(DEFER_DIAL) => {}
                _ = cancel.cancelled() => return,
            }
            let Some(again) = shared.upgrade() else { return };
            if again.session(&peer).is_some() {
                continue;
            }
            let Some(result) = dial_with_usb_interrupt(&again, peer, &wake).await else {
                continue;
            };
            let ok = handle_dial_result(&again, result, peer, &mut backoff);
            drop(again);
            if ok {
                continue;
            }
        } else {
            let Some(result) = dial_with_usb_interrupt(&node, peer, &wake).await else {
                continue;
            };
            let ok = handle_dial_result(&node, result, peer, &mut backoff);
            drop(node);
            if ok {
                continue;
            }
        }
        // Up to 20% jitter so many devices don't retry in lockstep.
        let jitter = backoff.mul_f64(rand::random::<f64>() * 0.2);
        tokio::select! {
            _ = tokio::time::sleep(backoff + jitter) => {}
            _ = wake.notified() => {}
            _ = cancel.cancelled() => return,
        }
        backoff = next_redial_wait(EndReason::DialFailed, backoff);
    }
}

/// How a session with a peer came to an end, for the next dial's wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndReason {
    /// It was up, and closed: a dead link, a path gone, either side's app.
    Closed,
    /// The dial didn't get through.
    DialFailed,
    /// The other side turned the session away (unpaired, an old version,
    /// reconnecting off, not on this network).
    Refused,
}

/// Why `session` ended, from how its connection was closed.
fn end_reason(session: &Session) -> EndReason {
    use iroh::endpoint::ConnectionError;
    match session.conn.close_reason() {
        Some(ConnectionError::ApplicationClosed(close)) => {
            let code = close.error_code.into_inner();
            let refused_code = [ErrorCode::Unpaired, ErrorCode::VersionTooOld, ErrorCode::Denied]
                .iter()
                .any(|c| u64::from(c.close_code()) == code);
            let refused_reason =
                matches!(&close.reason[..], b"away mode disabled" | b"reconnect off" | b"unpaired");
            if refused_code || refused_reason { EndReason::Refused } else { EndReason::Closed }
        }
        _ => EndReason::Closed,
    }
}

/// The wait before dialing again after `ended`, from the last wait.
fn next_redial_wait(ended: EndReason, backoff: Duration) -> Duration {
    match ended {
        EndReason::Closed => BACKOFF_MIN,
        EndReason::DialFailed | EndReason::Refused => (backoff * 2).min(BACKOFF_MAX),
    }
}

/// Returns true if the dial succeeded (backoff is reset once the session stays up >= 15s).
fn handle_dial_result(
    shared: &Shared,
    result: Result<Arc<Session>>,
    peer: DeviceId,
    backoff: &mut Duration,
) -> bool {
    match result {
        Ok(_) => true,
        Err(e) => {
            *lock(&shared.dial_failures).entry(peer).or_insert(0) += 1;
            tracing::debug!(peer = %peer.short(), error = %e, retry_in = ?backoff, "dial failed");
            if matches!(e, Error::VersionTooOld { side: crate::Side::Remote }) {
                shared.say_needs_update(peer);
            }
            false
        }
    }
}

/// Whether a peer reached on a LAN path at `reached_at`, which is (still)
/// one of this device's default `gateways`, counts as local at any address.
fn allowed_via_gateway(gateways: &[std::net::IpAddr], reached_at: Option<std::net::IpAddr>) -> bool {
    reached_at.is_some_and(|ip| gateways.contains(&ip))
}

/// This device's default IPv4 gateways, from the kernel's route table.
#[cfg(target_os = "linux")]
fn default_gateways() -> Vec<std::net::IpAddr> {
    let table = std::fs::read_to_string("/proc/net/route").unwrap_or_default();
    parse_default_gateways(&table)
}

/// Elsewhere the apps don't say yet (Android needs it through `Platform`).
#[cfg(not(target_os = "linux"))]
fn default_gateways() -> Vec<std::net::IpAddr> {
    Vec::new()
}

/// The gateways of the default routes in `/proc/net/route`: hexadecimal,
/// in the host's byte order (little endian), destination `00000000`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_default_gateways(table: &str) -> Vec<std::net::IpAddr> {
    let mut gateways = Vec::new();
    for line in table.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (Some(&dest), Some(&gateway)) = (fields.get(1), fields.get(2)) else { continue };
        if dest != "00000000" {
            continue;
        }
        let Ok(raw) = u32::from_str_radix(gateway, 16) else { continue };
        let ip = std::net::IpAddr::V4(std::net::Ipv4Addr::from(raw.to_le_bytes()));
        if raw != 0 && !gateways.contains(&ip) {
            gateways.push(ip);
        }
    }
    gateways
}

/// Streams one peer may have open on a connection at once.
const MAX_BIDI_STREAMS: u32 = 256;
/// Datagram bytes waiting to be sent, at most.
const DATAGRAM_SEND_BUFFER: usize = 64 * 1024;

/// Accepts `g7link/1` sessions from paired devices.
#[derive(Debug, Clone)]
struct SessionProtocol(Weak<Shared>);

impl ProtocolHandler for SessionProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let Some(shared) = self.0.upgrade() else { return Err(AcceptError::from_err(Error::Offline)) };
        let peer = crate::device_id(&conn.remote_id());
        if !shared.verify_usb_connection(&conn).await {
            tracing::debug!(peer = %peer.short(), "refusing session: USB hello identity mismatch");
            conn.close(VarInt::from_u32(ErrorCode::Denied.close_code()), b"usb identity mismatch");
            return Err(AcceptError::from_err(Error::Denied));
        }
        if !shared.store.is_paired(&peer).unwrap_or(false) {
            tracing::debug!(peer = %peer.short(), "refusing session from unpaired device");
            conn.close(VarInt::from_u32(ErrorCode::Unpaired.close_code()), b"unpaired");
            return Err(AcceptError::from_err(Error::NotPaired));
        }
        if !shared.wait_connection_allowed(&conn).await {
            tracing::debug!(peer = %peer.short(), "refusing incoming session on non-LAN path while away_mode is off");
            conn.close(VarInt::from_u32(CLOSE_NORMAL), b"away mode disabled");
            return Err(AcceptError::from_err(Error::Offline));
        }
        let hello = session::local_hello(&shared);
        match session::handshake_listener(&conn, &hello).await {
            Ok((send, recv, remote)) => {
                shared.register_session(conn, peer, (send, recv), remote);
                Ok(())
            }
            Err(e) => {
                tracing::debug!(peer = %peer.short(), error = %e, "incoming handshake failed");
                Err(AcceptError::from_err(e))
            }
        }
    }
}

/// Whether a dial failed because the other side's TLS took none of the
/// protocols offered (`no_application_protocol`): it doesn't speak this one.
fn refused_alpn(err: &iroh::endpoint::ConnectError) -> bool {
    use iroh::endpoint::{ConnectError, ConnectingError, ConnectionError, TransportErrorCode};
    /// The TLS alert a server sends when it shares no ALPN with the client.
    const NO_APPLICATION_PROTOCOL: u8 = 120;
    let closed = match err {
        ConnectError::Connecting { source: ConnectingError::ConnectionError { source, .. }, .. } => source,
        ConnectError::Connection { source, .. } => source,
        _ => return false,
    };
    matches!(
        closed,
        ConnectionError::ConnectionClosed(close)
            if close.error_code == TransportErrorCode::crypto(NO_APPLICATION_PROTOCOL)
    )
}

/// How often a paired device on an old protocol is said to need an update, at most.
const NEEDS_UPDATE_EVERY: Duration = Duration::from_secs(60);

impl Shared {
    /// Tells the apps that paired `peer` needs an update of G7 Link, at most
    /// once a minute.
    pub(crate) fn say_needs_update(&self, peer: DeviceId) {
        let now = std::time::Instant::now();
        let due = {
            let mut said = lock(&self.needs_update_said);
            let due =
                said.get(&peer).is_none_or(|at| now.saturating_duration_since(*at) >= NEEDS_UPDATE_EVERY);
            if due {
                said.insert(peer, now);
            }
            due
        };
        if due {
            tracing::info!(peer = %peer.short(), "a paired device needs an update of G7 Link");
            self.emit(NodeEvent::PeerNeedsUpdate { device: peer });
        }
    }
}

/// Answers `g7link/0`, the protocol before this one, only to turn it away: a
/// paired device is told it needs an update (`docs/protocol/v1.md` §2) and
/// closed with `VERSION_TOO_OLD`, anyone else with `UNPAIRED`. It opens no
/// streams and reads nothing.
#[derive(Debug, Clone)]
struct TombstoneProtocol(Weak<Shared>);

impl ProtocolHandler for TombstoneProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let Some(shared) = self.0.upgrade() else { return Err(AcceptError::from_err(Error::Offline)) };
        let peer = crate::device_id(&conn.remote_id());
        if shared.store.is_paired(&peer).unwrap_or(false) {
            shared.say_needs_update(peer);
            conn.close(VarInt::from_u32(ErrorCode::VersionTooOld.close_code()), b"update G7 Link");
        } else {
            conn.close(VarInt::from_u32(ErrorCode::Unpaired.close_code()), b"unpaired");
        }
        Ok(())
    }
}

/// Accepts `g7link-pair/0` connections while pairing.
#[derive(Debug, Clone)]
struct PairProtocol(Weak<Shared>);

impl ProtocolHandler for PairProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let Some(shared) = self.0.upgrade() else { return Err(AcceptError::from_err(Error::Offline)) };
        if !shared.verify_usb_connection(&conn).await {
            conn.close(VarInt::from_u32(ErrorCode::Denied.close_code()), b"usb identity mismatch");
            return Err(AcceptError::from_err(Error::Denied));
        }
        if !shared.wait_connection_allowed(&conn).await {
            tracing::debug!("refusing incoming pairing on non-LAN path while away_mode is off");
            conn.close(VarInt::from_u32(CLOSE_NORMAL), b"away mode disabled");
            return Err(AcceptError::from_err(Error::Offline));
        }
        pairing::accept(shared, conn).await;
        Ok(())
    }
}

/// The G7 Link engine. One per app.
///
/// Commands are async methods; everything that changes is reported through
/// [`Node::events`]. Cheap to clone.
#[derive(Clone)]
pub struct Node {
    shared: Arc<Shared>,
    router: Arc<Router>,
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node").field("id", &self.shared.id).finish_non_exhaustive()
    }
}

impl Node {
    /// Loads or creates the device identity and trust store, starts
    /// networking, and begins reconnecting to paired devices.
    pub async fn start(config: NodeConfig, platform: Arc<dyn Platform>) -> Result<Node> {
        std::fs::create_dir_all(&config.data_dir)?;
        let protector = match config.key_protector.clone() {
            Some(protector) => protector,
            // May wait for the user to unlock their keyring.
            None => tokio::task::spawn_blocking(identity::default_protector)
                .await
                .map_err(|e| Error::Internal(e.to_string()))?,
        };
        let secret = identity::load_or_create(&config.data_dir, protector.as_ref())?;
        let protector = identity::protector_for_data(&config.data_dir, protector);
        let store = Store::open(&config.data_dir)?;
        let relay_settings = store.relay_settings().unwrap_or_else(|err| {
            tracing::warn!(%err, "can't load relay settings; using defaults");
            RelaySettings::default()
        });
        let relay_map = relay_settings.enabled_map(config.away_mode);
        #[cfg(not(target_os = "linux"))]
        let proxy = None;
        #[cfg(target_os = "linux")]
        let relay_connector = match config.network_interface.as_ref().filter(|name| !name.is_empty()) {
            Some(name) => Some(crate::direct_network::RelayConnector::start(name.clone()).await?),
            None => None,
        };
        #[cfg(target_os = "linux")]
        let proxy = relay_connector.as_ref().map(|route| route.url());
        if let Err(e) = store.seal_queue_with(protector.clone()) {
            tracing::warn!(error = %e, "can't seal the send queue");
        }
        let _ = store.reset_interrupted_sending();
        if let Err(e) = store.prune_chats() {
            tracing::warn!(error = %e, "can't prune the stored chats");
        }
        let _ = store.expire_stale_queue_items(crate::now_unix_ms());
        let clipboard_history =
            crate::clipboard_history::ClipboardHistoryStore::open(&config.data_dir, protector.clone());
        let localsend =
            crate::localsend::LocalSendState::open(&config.data_dir, protector, config.localsend_port)?;

        // A stable port lets paired devices reconnect to the addresses they
        // remember even where local discovery is blocked (guest Wi-Fi, some
        // routers and VPNs), and lets firewall rules name it. Take the
        // preferred port if it's free, else reuse the last one, else any free
        // port, and remember what was taken.
        let saved = if config.port == 0 { Ports::load(&config.data_dir) } else { None };
        let endpoint = if config.port != 0 {
            let required = Ports { v4: config.port, v6: Some(config.port) };
            bind_retrying(&config, &secret, required, PORT_RETRIES, &relay_map, proxy).await?
        } else {
            let mut found = None;
            for (ports, retries) in port_order(config.preferred_port, saved, config.away_mode) {
                match bind_retrying(&config, &secret, ports, retries, &relay_map, proxy).await {
                    Ok(endpoint) => {
                        found = Some(endpoint);
                        break;
                    }
                    Err(e) => {
                        tracing::info!(error = %e, port = ports.v4, "port unavailable; trying the next")
                    }
                }
            }
            match found {
                Some(endpoint) => endpoint,
                None => bind_endpoint(&config, secret.clone(), None, relay_map.clone(), proxy).await?,
            }
        };
        if config.port == 0
            && let Some(bound) = Ports::of(&endpoint.bound_sockets())
            && Some(bound) != saved
        {
            bound.save(&config.data_dir);
        }
        let id = crate::device_id(&endpoint.id());

        let lookups = endpoint.address_lookup().map_err(crate::error::net)?;
        let memory = MemoryLookup::new();
        lookups.add(memory.clone());

        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let shared = Arc::new(Shared {
            id,
            secret,
            endpoint: endpoint.clone(),
            store,
            platform,
            local: RwLock::new(LocalState {
                app_version: config.app_version.clone(),
                device: config.device.clone(),
                power: config.power,
                extra_capabilities: config.capabilities.clone(),
                battery: None,
                adb: config.adb_server.is_some(),
            }),
            pairing: PairingState::default(),
            cancel: CancellationToken::new(),
            events,
            event_payloads: Mutex::new(std::collections::VecDeque::with_capacity(EVENT_CAPACITY)),
            sessions: Mutex::new(HashMap::new()),
            links: Mutex::new(HashMap::new()),
            supervisors: Mutex::new(HashMap::new()),
            discovered: Mutex::new(HashMap::new()),
            lan_seen: Mutex::new(HashMap::new()),
            paired_localsend: Mutex::new(HashMap::new()),
            update_hints: Mutex::new(HashMap::new()),
            lan_discovery: Mutex::new(None),
            name_on_lan: std::sync::atomic::AtomicBool::new(false),
            incoming_turns: Mutex::new(HashMap::new()),
            dial_failures: Mutex::new(HashMap::new()),
            matrices: Mutex::new(HashMap::new()),
            away_mode: std::sync::atomic::AtomicBool::new(config.away_mode),
            relay_settings: Mutex::new(relay_settings),
            relay_map,
            #[cfg(target_os = "linux")]
            relay_connector,
            interactive: Default::default(),
            memory,
            manual: Mutex::new(HashMap::new()),
            resolve_done: Notify::new(),
            sent_reach: tokio::sync::Mutex::new(None),
            notifications: Feed::default(),
            notification_history: Mutex::new(Vec::new()),
            players: Default::default(),
            rings: Default::default(),
            ble: Default::default(),
            calls: Default::default(),
            toggles: Default::default(),
            wake_info: Default::default(),
            tasks: Default::default(),
            deck: Default::default(),
            agents: Default::default(),
            mirror_stops: Mutex::new(HashMap::new()),
            outgoing_mirror_sessions: Mutex::new(std::collections::HashSet::new()),
            pc_mirror_sessions: Mutex::new(std::collections::HashSet::new()),
            watch_offers: Mutex::new(HashMap::new()),
            webcam_stops: Mutex::new(HashMap::new()),
            mic_stops: Mutex::new(HashMap::new()),
            remote: Mutex::new(Default::default()),
            storage: Mutex::new(Default::default()),
            clipboard_history,
            last_clip_suggestion: Mutex::new(None),
            private_clip_lifetime: config.private_clip_lifetime,
            private_clip_clear: Mutex::new(None),
            localsend,
            usb_listener_port: std::sync::atomic::AtomicU16::new(0),
            usb_bridge_ports: Arc::new(Mutex::new(HashMap::new())),
            usb_pending: Mutex::new(HashMap::new()),
            usb_peers: Arc::new(Mutex::new(HashMap::new())),
            adb_server: config.adb_server,
            usb_serials: Mutex::new(HashMap::new()),
            usb_helpers: Mutex::new(HashMap::new()),
            usb_inbound: Mutex::new(std::collections::HashSet::new()),
            usb_manual_pair: Mutex::new(std::collections::HashSet::new()),
            usb_poke: Notify::new(),
            queue_notify: Arc::new(Notify::new()),
            queue_active_peers: Mutex::new(std::collections::HashSet::new()),
            queue_coordinator_ticks: std::sync::atomic::AtomicUsize::new(0),
            data_dir: config.data_dir.clone(),
            downloads_dir: config.downloads_dir.clone().unwrap_or_else(|| config.data_dir.join("received")),
            transfers: Mutex::new(HashMap::new()),
            runtime: tokio::runtime::Handle::current(),
            away_switch: tokio::sync::Mutex::new(()),
            peer_caps: RwLock::new(HashMap::new()),
            outgoing_rpc_limit: std::sync::atomic::AtomicUsize::new(crate::rpc::MAX_OUTGOING_RPCS),
            session_tasks: Default::default(),
            needs_update_said: Mutex::new(HashMap::new()),
            debug_refuse_paths: Default::default(),
            gateways: RwLock::new(default_gateways()),
            capture_waiting: Mutex::new(HashMap::new()),
        });

        if config.lan_discovery {
            start_lan_discovery(&shared)?;
        }
        if shared.localsend.enabled() {
            shared.localsend.start(&shared);
        }

        tokio::spawn(crate::wire_stats::watch_starvation(shared.cancel.clone()));

        let router = Router::builder(endpoint)
            .accept(ALPN_SESSION, SessionProtocol(Arc::downgrade(&shared)))
            .accept(ALPN_SESSION_V0, TombstoneProtocol(Arc::downgrade(&shared)))
            .accept(ALPN_PAIR, PairProtocol(Arc::downgrade(&shared)))
            .spawn();

        if let Some(preferred_port) = config.usb_port {
            crate::usb::start_phone_listener(&shared, preferred_port);
        }
        if let Some(adb_addr) = config.adb_server {
            crate::usb::start_adb_watcher(&shared, adb_addr);
        }

        let incoming = shared.incoming_dir();
        tokio::task::spawn_blocking(move || transfer::clean_incoming(&incoming));

        for peer in shared.store.list_peers()? {
            if let Some(crate::reach::AddressHost::Name(name)) = peer.manual_addr.as_ref().map(|a| &a.host) {
                shared.set_manual_name(&peer.id, Some(name));
                shared.refresh_manual(&peer.id);
            }
            // remember_addrs keeps only LAN addresses while away mode is off.
            // The stored list keeps the rest, for when it's turned on again.
            shared.remember_addrs(&peer.id, &shared.known_addrs(&peer));
            shared.start_supervisor(peer.id);
        }
        run_queue_coordinator(Arc::downgrade(&shared), shared.cancel.child_token());
        tracing::info!(device = %id.short(), lan = config.lan_discovery, away = config.away_mode, "node started");
        Ok(Node { shared, router: Arc::new(router) })
    }

    /// Whether remote / mobile data connections outside the local Wi-Fi/LAN are allowed.
    pub fn away_mode(&self) -> bool {
        self.shared.away_mode()
    }

    /// Configured relays, the endpoint's home relays and routes in use by paired devices.
    pub fn relay_snapshot(&self) -> RelaySnapshot {
        let servers = lock(&self.shared.relay_settings).servers();
        let home = self
            .shared
            .endpoint
            .home_relay_status()
            .get()
            .into_iter()
            .map(|relay| HomeRelay {
                url: relay.url().to_string(),
                connected: relay.is_connected(),
                failed: relay.last_error().is_some(),
            })
            .collect();
        let mut routes: Vec<_> = self
            .shared
            .live_sessions()
            .into_iter()
            .filter_map(|session| {
                let url = session.relay_url()?.to_string();
                let device = self
                    .shared
                    .store
                    .get_peer(&session.peer)
                    .ok()
                    .flatten()
                    .map_or_else(|| session.peer.short(), |peer| peer.info.name);
                Some(RelayRoute { device, url })
            })
            .collect();
        routes.sort_by(|a, b| a.device.cmp(&b.device).then_with(|| a.url.cmp(&b.url)));
        RelaySnapshot { servers, home, routes }
    }

    /// Saves a relay change and updates the running endpoint without restarting the node.
    pub fn edit_relay(&self, url: &str, action: RelayEdit) -> std::result::Result<(), RelayEditError> {
        let mut settings = lock(&self.shared.relay_settings);
        let mut next = settings.clone();
        next.edit(url, action)?;
        self.shared.store.save_relay_settings(&next)?;
        *settings = next;
        drop(settings);

        let shared = self.shared.clone();
        self.shared.runtime.spawn(async move {
            let _one_at_a_time = shared.away_switch.lock().await;
            if apply_relays(&shared, shared.away_mode()).await {
                shared.endpoint.network_change().await;
                shared.nudge_reconnect();
            }
        });
        Ok(())
    }

    /// Checks all listed relays on demand, even while Away mode is off.
    pub async fn check_relays(&self) -> Vec<RelayProbe> {
        let servers = lock(&self.shared.relay_settings).servers();
        #[cfg(not(target_os = "linux"))]
        let proxy = None;
        #[cfg(target_os = "linux")]
        let proxy = self.shared.relay_connector.as_ref().map(|route| route.url());
        relays::probe_servers(&self.shared.endpoint, servers, proxy).await
    }

    /// Changes the relay route and retires TCP connections on the old route.
    /// Returns false if the relay connector needs to be enabled on restart.
    #[cfg(target_os = "linux")]
    pub fn set_relay_interface(&self, name: &str) -> Result<bool> {
        crate::direct_network::validate_interface(name)?;
        let Some(connector) = &self.shared.relay_connector else { return Ok(false) };
        connector.select(name);
        let shared = self.shared.clone();
        self.shared.runtime.spawn(async move { shared.endpoint.network_change().await });
        Ok(true)
    }

    /// Enables or disables connecting over mobile data / non-LAN paths at runtime.
    pub fn set_away_mode(&self, enabled: bool) {
        let was = self.shared.away_mode.swap(enabled, std::sync::atomic::Ordering::Relaxed);
        if was != enabled {
            let shared = self.shared.clone();
            self.shared.runtime.spawn(async move { apply_internet_services(&shared).await });
        }
        if !enabled {
            for session in self.shared.live_sessions() {
                if !self.shared.is_connection_allowed(&session.conn) {
                    tracing::info!(peer = %session.peer.short(), "closing non-LAN session after disabling away_mode");
                    session.close(CLOSE_NORMAL, b"away mode disabled");
                }
            }
            // Only what's dialled changes (remember_addrs keeps LAN addresses
            // now); the stored list keeps the others for turning it back on.
            if let Ok(peers) = self.shared.store.list_peers() {
                for peer in peers {
                    self.shared.remember_addrs(&peer.id, &self.shared.known_addrs(&peer));
                }
            }
        } else {
            if let Ok(peers) = self.shared.store.list_peers() {
                for peer in peers {
                    self.shared.remember_addrs(&peer.id, &self.shared.known_addrs(&peer));
                }
            }
            self.shared.nudge_reconnect();
        }
        if was != enabled {
            let shared = self.shared.clone();
            self.shared.runtime.spawn(async move { shared.refresh_reach().await });
        }
        self.shared.refresh_all_capabilities();
    }

    /// Stops all sessions and networking.
    pub async fn shutdown(&self) {
        self.shared.cancel.cancel();
        self.stop_ringing().await;
        crate::elevated_usb::stop_all(&self.shared);
        self.shared.localsend.stop(&self.shared);
        let sessions: Vec<_> = lock(&self.shared.sessions).drain().map(|(_, s)| s).collect();
        for session in sessions {
            session.close(CLOSE_NORMAL, b"shutdown");
        }
        if let Err(e) = self.router.shutdown().await {
            tracing::warn!(error = %e, "router shutdown failed");
        }
        self.shared.endpoint.close().await;
        #[cfg(target_os = "linux")]
        if let Some(connector) = &self.shared.relay_connector {
            connector.close();
        }
    }

    pub fn device_id(&self) -> DeviceId {
        self.shared.id
    }

    /// The node's state, for the `impl Node` blocks of other modules
    /// (`agent_history`).
    pub(crate) fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// Subscribes to node events. Each subscriber gets its own stream.
    pub fn events(&self) -> broadcast::Receiver<NodeEvent> {
        self.shared.events.subscribe()
    }

    /// This device's direct addresses (for diagnostics and tests).
    pub async fn direct_addrs(&self) -> Vec<SocketAddr> {
        self.shared.direct_addrs().await
    }

    /// The UDP port the node listens on (IPv4).
    pub fn listen_port(&self) -> Option<u16> {
        self.shared.bound_v4_port()
    }

    /// Returns the loopback TCP port the phone-side USB tunnel listener is
    /// bound to, if enabled via [`NodeConfig::usb_port`].
    pub fn usb_port(&self) -> Option<u16> {
        let port = self.shared.usb_listener_port.load(std::sync::atomic::Ordering::Relaxed);
        (port != 0).then_some(port)
    }

    /// Connects a PC-side USB tunnel to a forwarded phone loopback TCP address
    /// (`127.0.0.1:<port>`) and returns the phone's [`DeviceId`].
    pub async fn connect_usb_tunnel(&self, forwarded_tcp: SocketAddr) -> Result<DeviceId> {
        crate::usb::connect_usb_tunnel(&self.shared, forwarded_tcp).await
    }

    // ---- Pairing ----

    /// Enters pairing mode and returns the link to show as a QR code.
    pub async fn pairing_start_qr(&self) -> Result<PairingUri> {
        pairing::start_host(&self.shared).await
    }

    /// Pairs with the device whose pairing link was scanned.
    pub async fn pairing_join(&self, uri: &str) -> Result<()> {
        let uri = PairingUri::parse(uri)?;
        if uri.id == self.shared.id {
            return Err(Error::Protocol("cannot pair a device with itself".into()));
        }
        pairing::join_qr(&self.shared, &uri).await
    }

    /// Pairs with the device a pairing link names when the link came from
    /// another app: both devices show a fresh 6-digit code to compare, as in
    /// nearby pairing; the link's secret isn't used. The code arrives as
    /// [`PairingEvent::SasCode`]; answer with [`Node::pairing_confirm`].
    pub async fn pairing_join_link(&self, uri: &str) -> Result<()> {
        let uri = PairingUri::parse(uri)?;
        if uri.id == self.shared.id {
            return Err(Error::Protocol("cannot pair a device with itself".into()));
        }
        pairing::join_link(&self.shared, &uri).await
    }

    /// Starts nearby pairing with a discovered device. The other device must
    /// be in pairing mode (showing its pairing screen). The 6-digit code
    /// arrives as [`PairingEvent::SasCode`] on both devices.
    pub async fn pairing_start_nearby(&self, peer: DeviceId) -> Result<()> {
        pairing::start_nearby(&self.shared, peer).await
    }

    /// Answers the 6-digit code comparison shown via
    /// [`PairingEvent::SasCode`].
    pub fn pairing_confirm(&self, codes_match: bool) -> Result<()> {
        self.shared.pairing.decide(codes_match)
    }

    /// Leaves pairing mode and abandons any code comparison.
    pub fn pairing_cancel(&self) {
        pairing::cancel(&self.shared);
        let _runtime = self.shared.runtime.enter();
        sync_name_on_lan(&self.shared);
    }

    /// Whether this device is currently showing a pairing code.
    pub fn is_pairing(&self) -> bool {
        self.shared.pairing.is_hosting()
    }

    /// Adds addresses where a device can be reached. Used for "connect by IP"
    /// when local discovery is blocked (some routers and VPNs filter mDNS).
    pub fn add_known_addrs(&self, peer: DeviceId, addrs: &[SocketAddr]) {
        self.shared.remember_addrs(&peer, addrs);
        self.shared.wake_supervisor(&peer);
    }

    /// Tells the core whether the user has the app open. Links are then
    /// checked every 5 s instead of every 15 s, so one that died shows
    /// sooner; turning it on checks them right away.
    pub fn set_interactive(&self, on: bool) {
        self.shared.interactive.set(on);
    }

    /// Tells the core the network may have changed (Wi-Fi switched, resumed
    /// from sleep). Paths are re-checked and offline devices are dialed right
    /// away instead of after their backoff. Harmless when nothing changed.
    pub async fn network_changed(&self) {
        self.shared.endpoint.network_change().await;
        let shared = self.shared.clone();
        let _ = tokio::task::spawn_blocking(move || shared.refresh_gateways()).await;
        self.shared.reach_network_changed();
        self.shared.nudge_reconnect();
        // Each session's heartbeat pings now and judges by its own rule:
        // a change here is no reason to close a session by itself.
        for session in self.shared.live_sessions() {
            session.wake_heartbeat();
        }
    }

    /// What a "refresh" button does: reconnects to devices that aren't
    /// connected, and brings connected ones back in sync both ways (their
    /// notifications and media here, this device's there).
    pub async fn refresh(&self) {
        self.shared.usb_poke.notify_one();
        self.shared.retry_usb_auto_pair();
        self.network_changed().await;
        self.shared.refresh_all_capabilities();
        self.shared.localsend.refresh(&self.shared);
        let local_device = self.shared.local.read().unwrap_or_else(|e| e.into_inner()).device.clone();
        let hello_env = Envelope::new(
            types::HELLO_UPDATE,
            &HelloUpdate { device: Some(local_device), ..Default::default() },
        )
        .ok();
        for session in self.shared.live_sessions() {
            if let Some(ref env) = hello_env {
                let _ = session.send(env.clone()).await;
            }
            let _ = session.send(Envelope::empty(types::NOTIFY_SYNC)).await;
            let _ = session.send(Envelope::empty(types::MEDIA_SYNC)).await;
            self.shared.send_notification_snapshot(&session).await;
            self.shared.send_media_state(&session).await;
            self.shared.send_toggles_state(&session).await;
            crate::actions::send_wake_info(&self.shared, &session).await;
            crate::actions::send_active_tasks(&self.shared, &session).await;
            self.shared.send_deck(&session).await;
        }
    }

    /// Devices discovered on the local network that aren't paired yet.
    /// Asks the LAN who's there often while a list of nearby devices is on
    /// screen (pairing), and rarely otherwise, which keeps phones asleep.
    /// No-op without LAN discovery.
    /// Also says whether this device's pairing screen is open: its name is
    /// on the LAN only then, and a phone takes pairing over USB only then.
    pub fn set_discovery_fast(&self, fast: bool) {
        self.shared.pairing.set_screen_open(fast);
        let _runtime = self.shared.runtime.enter();
        if fast {
            self.shared.pair_with_plugged_in_pcs();
        }
        let running = lock(&self.shared.lan_discovery).as_ref().map(|(_, f)| *f);
        if running.is_none_or(|f| f == fast) {
            sync_name_on_lan(&self.shared);
            return;
        }
        if let Err(e) = install_mdns(&self.shared, fast) {
            tracing::warn!(error = %e, fast, "can't change how often mDNS asks");
        }
    }

    pub fn discovered_devices(&self) -> Vec<DiscoveredDevice> {
        lock(&self.shared.discovered).values().cloned().collect()
    }

    // ---- Paired devices ----

    pub fn paired_devices(&self) -> Result<Vec<PairedDevice>> {
        Ok(self
            .shared
            .store
            .list_peers()?
            .into_iter()
            .map(|p| PairedDevice {
                link: self.shared.link(&p.id, p.last_seen),
                can_wake: p.wake_info.as_ref().is_some_and(|w| !w.macs.is_empty()),
                reconnect: self.shared.reconnects(&p.id),
                id: p.id,
                info: p.info,
                paired_at: p.paired_at,
            })
            .collect())
    }

    /// Unpairs a device, telling it if it's connected.
    pub async fn unpair(&self, peer: DeviceId) -> Result<()> {
        if !self.shared.store.is_paired(&peer)? {
            return Err(Error::NotPaired);
        }
        if let Some(session) = self.shared.session(&peer) {
            let _ = session.send(Envelope::empty(types::PAIR_REVOKE)).await;
            // Let the revoke reach the peer before the connection closes.
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        self.shared.forget_peer(&peer).await
    }

    /// Asks a device to start or stop ringing: over the session, or over the
    /// Bluetooth link when there's no session and the two may use one.
    pub async fn ring(&self, peer: DeviceId, on: bool) -> Result<()> {
        if self.shared.session(&peer).is_none() && self.shared.ble_usable(&peer) {
            return self.ble_ring(peer, on).await;
        }
        let session = self.connected(&peer)?;
        let _command = session.ring_command.lock().await;
        let token =
            if on { Some(self.shared.start_remote_ring(peer)) } else { self.shared.remote_ring_token(peer) };
        let reply = self.request(peer, Envelope::new(types::DEVICE_RING, &Ring { on, token })?).await;
        let result = reply.and_then(|reply| Ok(reply.expect(types::OK)?));
        if !on || result.is_err() {
            self.shared.clear_remote_ring(peer, token);
        }
        result
    }

    /// Asks a device to start or stop ringing over the Bluetooth link, which
    /// the PC asks the phone to open when there's none.
    pub async fn ble_ring(&self, peer: DeviceId, on: bool) -> Result<()> {
        let token =
            if on { Some(self.shared.start_remote_ring(peer)) } else { self.shared.remote_ring_token(peer) };
        let env = Envelope::new(types::DEVICE_RING, &Ring { on, token })?;
        let reply = self.shared.ble_request(peer, env, Duration::from_secs(30)).await;
        let result = reply.and_then(|reply| Ok(reply.expect(types::OK)?));
        if !on || result.is_err() {
            self.shared.clear_remote_ring(peer, token);
        }
        result
    }

    /// Stops the local alarm and informs the device that started it.
    pub async fn stop_ringing(&self) {
        self.shared.stop_local_ring_if(|_| true).await;
    }

    /// Sends a request to a connected device. If the connection is replaced
    /// mid-request (e.g. a duplicate connection was resolved), retries once
    /// on the new one, so requests must be idempotent.
    async fn request(&self, peer: DeviceId, env: Envelope) -> Result<Envelope> {
        let first = self.connected(&peer)?;
        match first.request_tracked(env.clone(), REQUEST_TIMEOUT).await {
            // The connection was replaced before the request went out: send it
            // on the new one. Once it went out it may have run (a reply to a
            // chat, a notification's action), so it isn't sent twice.
            (Err(Error::Offline), false) => match self.shared.session(&peer) {
                Some(next) if !Arc::ptr_eq(&next, &first) => next.request(env, REQUEST_TIMEOUT).await,
                _ => Err(Error::Offline),
            },
            (other, _) => other,
        }
    }

    fn connected(&self, peer: &DeviceId) -> Result<Arc<Session>> {
        if !self.shared.store.is_paired(peer)? {
            return Err(Error::NotPaired);
        }
        self.shared.session(peer).ok_or(Error::Offline)
    }

    /// Like [`connected`](Self::connected), but waits up to [`STREAM_RECONNECT_WAIT`]
    /// for a connection that just dropped or was replaced to come back.
    async fn connected_soon(&self, peer: &DeviceId) -> Result<Arc<Session>> {
        let deadline = tokio::time::Instant::now() + STREAM_RECONNECT_WAIT;
        loop {
            match self.connected(peer) {
                Err(Error::Offline) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                other => return other,
            }
        }
    }

    // ---- Capabilities ----

    /// What works with a paired device, and how to unlock what doesn't.
    /// Changes arrive as [`NodeEvent::Capabilities`]. Before the device has
    /// connected once, its capabilities are unknown and most features show
    /// as locked.
    pub fn capabilities(&self, peer: DeviceId) -> Result<CapabilityMatrix> {
        self.shared.capability_matrix(&peer)
    }

    /// The per-device toggles (see [`features::DEVICE_TOGGLES`]) with their
    /// current values.
    /// The key of the Bluetooth beacon this device shares with each paired
    /// device that has one (docs/protocol/bluetooth.md).
    pub fn ble_beacon_keys(&self) -> Vec<(DeviceId, [u8; 32])> {
        self.shared.ble_beacon_keys()
    }

    /// PC: a new end for a phone to open a Bluetooth link to
    /// (docs/protocol/bluetooth.md).
    pub fn ble_accept(&self) -> Arc<crate::BleEndpoint> {
        crate::BleEndpoint::accept(&self.shared)
    }

    /// Phone: starts a Bluetooth link with the PC `peer`. Refused unless both
    /// let the other use Bluetooth and share a secret.
    pub fn ble_connect(&self, peer: DeviceId) -> Result<Arc<crate::BleEndpoint>> {
        if !self.shared.store.is_paired(&peer)? {
            return Err(Error::NotPaired);
        }
        if !self.shared.ble_usable(&peer) {
            return Err(Error::Denied);
        }
        let secret = self.shared.store.link_secret(&peer)?.ok_or(Error::Denied)?;
        crate::BleEndpoint::connect(&self.shared, peer, secret)
    }

    /// Whether this device and `peer` may use a Bluetooth link with each other.
    pub fn ble_usable(&self, peer: DeviceId) -> bool {
        self.shared.ble_usable(&peer)
    }

    /// Whether a Bluetooth link with `peer` is up.
    pub fn ble_linked(&self, peer: DeviceId) -> bool {
        self.shared.ble_link(&peer).is_some()
    }

    /// Ends the Bluetooth link with `peer`, if there is one.
    pub fn close_ble_link(&self, peer: DeviceId) {
        self.shared.close_ble_link(&peer);
    }

    /// PC: whether the phone `peer` holds its Bluetooth link with this PC
    /// (`ble.use`): the link isn't closed once its session is back.
    pub fn ble_held(&self, peer: DeviceId) -> bool {
        self.shared.ble_held(&peer)
    }

    /// PC: the prompt raised by [`NodeEvent::BleLinkRequested`] for `peer`
    /// closed without the `ble_link` toggle being turned on ("Not now"): the
    /// phone is told no. Turning the toggle on answers it too.
    pub fn ble_prompt_closed(&self, peer: DeviceId) {
        self.shared.ble_prompt_closed(&peer);
    }

    /// Phone: asks the PC `peer` to keep a Bluetooth link with this phone
    /// (`on`) or to stop (`ble.use`, over the session or else the link);
    /// with `ask`, the PC may ask its user when its `ble_link` toggle is
    /// off. Renew it every 20 s while the touchpad shows: a hold that isn't
    /// renewed ends after 45 s. `Error::Unsupported` for a PC without
    /// `ble.remote`.
    pub async fn ble_use(&self, peer: DeviceId, on: bool, ask: bool) -> Result<crate::BleUseState> {
        Ok(self.ble_use_reply(peer, on, ask).await?.state)
    }

    /// Phone: [`ble_use`](Self::ble_use), with all the PC said: with `open`,
    /// a PC from 0.1.14 also says how long its connect window stays open
    /// (`window`, 0: none now) and, with none, when a renewal can open one
    /// (`retry`).
    pub async fn ble_use_reply(&self, peer: DeviceId, on: bool, ask: bool) -> Result<crate::BleUseReply> {
        if !self.shared.store.is_paired(&peer)? {
            return Err(Error::NotPaired);
        }
        if !self.shared.peer_offers_ble_remote(&peer) {
            return Err(Error::Unsupported);
        }
        let env = Envelope::new(types::BLE_USE, &g7link_protocol::messages::BleUse { on, ask })?;
        let reply = if self.shared.session(&peer).is_some() {
            self.request(peer, env).await?
        } else if self.shared.ble_link(&peer).is_some() {
            self.shared.ble_request(peer, env, crate::ble_link::REQUEST_TIMEOUT).await?
        } else {
            return Err(Error::Offline);
        };
        Ok(reply.expect_body(types::BLE_USE)?)
    }

    /// Whether this device can reach `peer` for touchpad input and the
    /// typing box: over a session or a Bluetooth link.
    pub fn input_reachable(&self, peer: DeviceId) -> bool {
        self.shared.input_reachable(&peer)
    }

    /// Phone: the route the user picked for the touchpad of the PC `peer`
    /// (kept by the app, which sets it at start and on change). The path
    /// changes once nothing is mid-gesture; [`NodeEvent::RemoteRouteChanged`]
    /// says when.
    pub fn set_remote_route(&self, peer: DeviceId, route: crate::RemoteRoute) -> Result<()> {
        self.shared.set_remote_route(&peer, route)
    }

    /// Phone: where the touchpad route of the PC `peer` stands; the default
    /// for a device that isn't paired.
    pub fn remote_route_state(&self, peer: DeviceId) -> crate::RouteState {
        self.shared.remote_route_state(&peer)
    }

    /// Phone: a touchpad for the PC `peer` shows (`active`) or no longer
    /// does. While it shows, both paths are measured every 2 s.
    pub fn set_remote_active(&self, peer: DeviceId, active: bool) -> Result<()> {
        self.shared.set_remote_active(&peer, active)
    }

    /// Checks the Bluetooth link with `peer` answers; how long it took.
    pub async fn ble_ping(&self, peer: DeviceId) -> Result<Duration> {
        let started = std::time::Instant::now();
        let reply =
            self.shared.ble_request(peer, Envelope::empty(types::PING), Duration::from_secs(10)).await?;
        reply.expect(types::PONG)?;
        Ok(started.elapsed())
    }

    /// Whether paired `peer` offered `capability` when it last connected.
    pub fn peer_offers(&self, peer: DeviceId, capability: &str) -> bool {
        self.shared.store.get_peer(&peer).ok().flatten().is_some_and(|p| p.caps.contains(capability))
    }

    pub fn device_toggles(&self, peer: DeviceId) -> Result<Vec<(&'static str, bool)>> {
        if !self.shared.store.is_paired(&peer)? {
            return Err(Error::NotPaired);
        }
        let set = self.shared.store.toggles(&peer)?;
        Ok(features::DEVICE_TOGGLES
            .iter()
            .map(|(name, default)| (*name, set.get(*name).copied().unwrap_or(*default)))
            .collect())
    }

    /// Turns reconnecting to a paired device by itself off or back on.
    ///
    /// Off ("Stop reconnecting"): this device disconnects from it if
    /// connected, stops dialing it, and refuses its connections, until it's
    /// turned back on. It stays paired, and the setting is kept across
    /// restarts. On ("Reconnect"): it dials the device right away.
    pub fn set_reconnect(&self, peer: DeviceId, on: bool) -> Result<()> {
        if !self.shared.store.is_paired(&peer)? {
            return Err(Error::NotPaired);
        }
        if self.shared.reconnects(&peer) == on {
            return Ok(());
        }
        self.shared.store.set_toggle(&peer, RECONNECT_TOGGLE, on)?;
        tracing::info!(peer = %peer.short(), on, "reconnecting");
        if on {
            lock(&self.shared.dial_failures).remove(&peer);
            self.shared.wake_supervisor(&peer);
        } else if let Some(session) = self.shared.session(&peer) {
            session.close(CLOSE_NORMAL, b"reconnect off");
        }
        self.shared.emit(NodeEvent::ReconnectChanged { device: peer, on });
        Ok(())
    }

    /// Allows or disallows something for one device, e.g. `"clipboard"`.
    /// `bt_media` is sent to the device too, which keeps the same.
    pub fn set_device_toggle(&self, peer: DeviceId, toggle: &str, enabled: bool) -> Result<()> {
        if features::toggle_default(toggle).is_none() {
            return Err(Error::Unsupported);
        }
        if !self.shared.store.is_paired(&peer)? {
            return Err(Error::NotPaired);
        }
        if toggle == crate::media::BT_MEDIA_TOGGLE {
            self.shared.set_bt_media(peer, enabled)?;
        } else {
            self.shared.store.set_toggle(&peer, toggle, enabled)?;
        }
        self.shared.refresh_capabilities(&peer);
        if toggle == notifications::TOGGLE {
            self.notifications_toggled(peer, enabled);
        }
        if toggle == crate::media::TOGGLE {
            self.shared.media_toggled(peer, enabled);
        }
        if toggle == crate::toggles::TOGGLE {
            self.shared.toggles_toggled(peer, enabled);
        }
        if toggle == crate::BLE_LINK_TOGGLE {
            self.shared.ble_toggled(peer, enabled);
        }
        if (toggle == crate::actions::POWER_TOGGLE || toggle == crate::deck::INPUT_TOGGLE)
            && let Some(session) = self.shared.session(&peer)
        {
            let shared = self.shared.clone();
            let is_power = toggle == crate::actions::POWER_TOGGLE;
            self.shared.runtime.spawn(async move {
                if is_power {
                    crate::actions::send_wake_info(&shared, &session).await;
                }
                shared.send_deck(&session).await;
            });
        }
        if toggle == crate::agents::TOGGLE {
            crate::agents::toggled(&self.shared, peer, enabled);
        }
        if toggle == crate::agents::SESSIONS_TOGGLE {
            crate::agents::sessions_toggled(&self.shared, peer, enabled);
        }
        if toggle == crate::agent_git::git::PUSH_TOGGLE {
            crate::agent_git::push_toggled(&self.shared, peer);
        }
        if toggle == crate::agent_folders::agent_places::TOGGLE {
            // Its catalog says so again, as `agent_push`'s does.
            crate::agent_git::push_toggled(&self.shared, peer);
        }
        if toggle == crate::storage::TOGGLE {
            let mut st = self.shared.storage.lock().unwrap_or_else(|e| e.into_inner());
            if enabled {
                st.clear_prompted(&peer);
            } else {
                st.remove_peer(&peer);
            }
        }
        if (toggle == crate::mirror::TOGGLE || toggle == crate::mirror::PC_MIRROR_TOGGLE) && !enabled {
            if toggle == crate::mirror::PC_MIRROR_TOGGLE {
                lock(&self.shared.pc_mirror_sessions).remove(&peer);
            }
            let mut sessions_to_stop = self.shared.stop_showing_all(&peer);
            for s in self.shared.take_outgoing_mirror_sessions(&peer) {
                if !sessions_to_stop.contains(&s) {
                    sessions_to_stop.push(s);
                }
            }
            if !sessions_to_stop.contains(&crate::MIRROR_SCREEN) {
                sessions_to_stop.push(crate::MIRROR_SCREEN);
            }
            let platform = self.shared.platform.clone();
            let local_sessions = sessions_to_stop.clone();
            drop(self.shared.runtime.spawn_blocking(move || {
                for s in local_sessions {
                    platform.mirror_stop_requested(&peer, s);
                }
            }));
            if let Some(session) = self.shared.session(&peer) {
                self.shared.runtime.spawn(async move {
                    for s in sessions_to_stop {
                        let _ = crate::mirror::stop(&session, s).await;
                    }
                });
            }
        }
        if toggle == crate::mic::TOGGLE && !enabled {
            self.shared.stop_mic(&peer);
            let platform = self.shared.platform.clone();
            drop(self.shared.runtime.spawn_blocking(move || platform.mic_stop_requested(&peer)));
            if let Some(session) = self.shared.session(&peer) {
                self.shared.runtime.spawn(async move {
                    let _ = crate::mic::stop(&session).await;
                });
            }
        }
        if toggle == crate::webcam::TOGGLE && !enabled {
            self.shared.stop_webcam(&peer);
            let platform = self.shared.platform.clone();
            drop(self.shared.runtime.spawn_blocking(move || platform.webcam_stop_requested(&peer)));
            if let Some(session) = self.shared.session(&peer) {
                self.shared.runtime.spawn(async move {
                    let _ = crate::webcam::stop(&session).await;
                });
            }
        }
        Ok(())
    }

    /// Grants or revokes one-time reverse-mirror consent (`pc_mirror`) for `peer`
    /// for the duration of its current session without persisting a toggle change.
    pub fn set_pc_mirror_session_allowed(&self, peer: DeviceId, allowed: bool) {
        let mut set = lock(&self.shared.pc_mirror_sessions);
        if allowed {
            set.insert(peer);
        } else {
            set.remove(&peer);
        }
    }

    /// Brings both sides in line after the user allowed or stopped
    /// notifications for a device.
    fn notifications_toggled(&self, peer: DeviceId, enabled: bool) {
        let shared = &self.shared;
        if shared.notifications.is_active() {
            // This phone: send the PC everything, or clear it.
            if let Some(session) = shared.session(&peer) {
                let task = shared.clone();
                shared.runtime.spawn(async move { task.send_notification_snapshot(&session).await });
            }
        } else if enabled {
            // This PC: ask the phone for what it shows now.
            if let Some(session) = shared.session(&peer) {
                shared.runtime.spawn(async move {
                    let _ = session.send(Envelope::empty(types::NOTIFY_SYNC)).await;
                });
            }
        } else {
            shared.emit(NodeEvent::NotificationsReset { device: peer, items: Vec::new() });
        }
    }

    // ---- Files (docs/protocol/files.md) ----

    /// Sends files to a paired device; returns the transfer's ID. Progress
    /// and the outcome arrive as [`NodeEvent::Transfer`]; the transfer
    /// waits for the device to connect and resumes after interruptions.
    pub async fn send_files(&self, peer: DeviceId, files: Vec<transfer::OutgoingFile>) -> Result<String> {
        transfer::send(&self.shared, peer, files).await
    }

    /// Hands off a document to a paired device and asks the receiver to open it
    /// with the default application once it arrives (`docs/protocol/files.md`).
    pub async fn send_handoff_files(
        &self,
        peer: DeviceId,
        files: Vec<transfer::OutgoingFile>,
    ) -> Result<String> {
        transfer::send_handoff(&self.shared, peer, files).await
    }

    /// Sends a voice recording and its markers to a paired PC; returns the
    /// transfer's ID. Progress and the outcome arrive as
    /// [`NodeEvent::Transfer`]; the transfer waits for the PC to connect and
    /// resumes after interruptions.
    pub async fn send_recording(
        &self,
        peer: DeviceId,
        file: transfer::OutgoingFile,
        markers: Vec<transfer::RecordingMarker>,
    ) -> Result<String> {
        transfer::send_recording(&self.shared, peer, file, markers).await
    }

    /// Accepts an incoming transfer waiting for user confirmation (LocalSend).
    pub fn accept_transfer(&self, id: &str) {
        self.shared.localsend.resolve_transfer_approval(id, true);
    }

    /// Pauses an active transfer in either direction, preserving partial files
    /// on the receiver so it can be resumed later. Unknown IDs are ignored.
    pub fn pause_transfer(&self, id: &str) {
        if let Some(ctrl) = lock(&self.shared.transfers).get(id) {
            let _ = ctrl.paused.send(true);
        }
    }

    /// Resumes a manually paused transfer in either direction. Unknown IDs are ignored.
    pub fn resume_transfer(&self, id: &str) {
        let target = {
            let mut map = lock(&self.shared.transfers);
            if let Some(ctrl) = map.get_mut(id) {
                let _ = ctrl.paused.send(false);
                ctrl.paused_transfer = None;
                Some((ctrl.peer, ctrl.direction))
            } else {
                None
            }
        };
        if let Some((peer, transfer::Direction::Incoming)) = target
            && peer != DeviceId([0; 32])
            && let Some(session) = self.shared.session(&peer)
            && let Ok(env) = Envelope::new(
                types::FILES_RESUME,
                &g7link_protocol::messages::FilesControl { id: id.to_owned() },
            )
        {
            self.shared.runtime.spawn(async move {
                let _ = session.send(env).await;
            });
        }
    }

    /// Cancels a transfer in either direction (or declines a pending incoming
    /// LocalSend transfer). Unknown IDs are ignored.
    pub fn cancel_transfer(&self, id: &str) {
        let _ = self.shared.cancel_queue_or_transfer(id);
    }

    // ---- Persistent outgoing send queue ----

    /// Lists items in the persistent outgoing send queue (ordered by `seq ASC`),
    /// optionally filtered to a single paired `peer`.
    pub fn send_queue(&self, peer: Option<DeviceId>) -> Result<Vec<crate::QueueItem>> {
        self.shared.store.list_send_queue(peer.as_ref())
    }

    /// Returns the ID of the most recently queued active (`Waiting` or `Sending`)
    /// item for `peer`, if any.
    pub fn latest_queued_id_for_peer(&self, peer: DeviceId) -> Option<String> {
        self.shared
            .store
            .list_send_queue(Some(&peer))
            .ok()?
            .into_iter()
            .rev()
            .find(|i| matches!(i.status, crate::QueueStatus::Waiting | crate::QueueStatus::Sending))
            .map(|i| i.id)
    }

    /// Cancels and removes a single item from the outgoing send queue (and cancels
    /// its active file transfer and cleans up staged files if applicable).
    pub fn cancel_queue_item(&self, id: &str) -> Result<bool> {
        self.shared.cancel_queue_or_transfer(id)
    }

    /// Cancels all active (`Waiting` or `Sending`) items in the outgoing send queue
    /// for `peer` and returns the number of items cancelled.
    pub fn cancel_queue_for_peer(&self, peer: DeviceId) -> Result<u32> {
        self.cancel_all_queue_items(Some(peer))
    }

    /// Cancels all active (`Waiting` or `Sending`) items in the outgoing send queue
    /// (optionally filtered to `peer`) and returns the number of items cancelled.
    pub fn cancel_all_queue_items(&self, peer: Option<DeviceId>) -> Result<u32> {
        let items = self.shared.store.list_send_queue(peer.as_ref())?;
        let mut count: u32 = 0;
        for item in items {
            if matches!(item.status, crate::QueueStatus::Waiting | crate::QueueStatus::Sending)
                && self.shared.cancel_queue_or_transfer(&item.id).unwrap_or(false)
            {
                count = count.saturating_add(1);
            }
        }
        Ok(count)
    }

    /// Resets a failed or expired queue item back to `Waiting` so it retries immediately.
    pub fn retry_queue_item(&self, id: &str) -> Result<bool> {
        let retried = self.shared.store.retry_queue_item(id, crate::now_unix_ms())?;
        if retried {
            self.shared.emit(NodeEvent::QueueChanged);
            self.shared.queue_notify.notify_waiters();
        }
        Ok(retried)
    }

    /// Resets all failed or expired queue items (optionally filtered to `peer`)
    /// back to `Waiting` and returns the number of items reset.
    pub fn retry_all_failed_queue_items(&self, peer: Option<DeviceId>) -> Result<u32> {
        let count = self.shared.store.retry_all_failed_queue_items(peer.as_ref(), crate::now_unix_ms())?;
        if count > 0 {
            self.shared.emit(NodeEvent::QueueChanged);
            self.shared.queue_notify.notify_waiters();
        }
        Ok(count)
    }

    /// Removes all failed and expired items from the outgoing send queue
    /// (optionally filtered to `peer`), cleans up their staged files, and returns
    /// the number of items removed.
    pub fn clear_finished_queue_items(&self, peer: Option<DeviceId>) -> Result<u32> {
        let removed = self.shared.store.clear_finished_queue_items(peer.as_ref())?;
        for (id, _) in &removed {
            transfer::cleanup_staging_id(&self.shared, id);
        }
        let count = u32::try_from(removed.len()).unwrap_or(u32::MAX);
        if count > 0 {
            self.shared.emit(NodeEvent::QueueChanged);
        }
        Ok(count)
    }

    /// Alias for [`Node::clear_finished_queue_items`].
    pub fn clear_finished_queue(&self, peer: Option<DeviceId>) -> Result<u32> {
        self.clear_finished_queue_items(peer)
    }

    /// Marks any `Waiting` queue items whose TTL has elapsed as `Expired` and
    /// returns the number of items expired.
    pub fn expire_stale_queue_items(&self) -> Result<u32> {
        let expired = self.shared.store.expire_stale_queue_items(crate::now_unix_ms())?;
        let count = u32::try_from(expired).unwrap_or(u32::MAX);
        if count > 0 {
            self.shared.emit(NodeEvent::QueueChanged);
        }
        Ok(count)
    }

    /// Returns the number of wake-up ticks executed by the background send-queue coordinator.
    #[doc(hidden)]
    pub fn queue_coordinator_ticks(&self) -> usize {
        self.shared.queue_coordinator_ticks.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Sets `attempts` and `next_retry_ms` on a waiting queue item and wakes the queue coordinator.
    #[doc(hidden)]
    pub fn debug_set_queue_retry_ms(&self, id: &str, attempts: u32, next_retry_ms: i64) -> Result<bool> {
        let updated =
            self.shared.store.mark_queue_retry_or_wait(id, attempts, next_retry_ms, Some("test retry"))?;
        self.shared.queue_notify.notify_waiters();
        Ok(updated)
    }

    /// The wire counters of the session with `peer` (`plans/protocol-v1-contract.md`
    /// §3), if one is up. For tests and diagnostics.
    #[doc(hidden)]
    pub fn wire_counters(&self, peer: DeviceId) -> Option<crate::WireCounters> {
        self.shared.session(&peer).map(|s| s.wire_counters())
    }

    /// How many session tasks run now, across sessions. For tests.
    #[doc(hidden)]
    pub fn debug_session_tasks(&self) -> usize {
        self.shared.session_tasks.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Test hook: while on, heartbeats find no allowed path for their sessions.
    #[doc(hidden)]
    pub fn debug_refuse_paths(&self, on: bool) {
        self.shared.debug_refuse_paths.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    /// Test hook: RPCs a session opened from now on may have in flight.
    #[doc(hidden)]
    pub fn debug_set_outgoing_rpc_limit(&self, limit: usize) {
        self.shared.outgoing_rpc_limit.store(limit.max(1), std::sync::atomic::Ordering::Relaxed);
    }

    /// Test hook: dials `peer` now, as the reconnect loop does.
    #[doc(hidden)]
    pub async fn debug_dial(&self, peer: DeviceId) -> Result<()> {
        self.shared.dial(peer).await.map(|_| ())
    }

    /// Test hook: closes the session with `peer` as a dead link would end it.
    #[doc(hidden)]
    pub fn debug_close_session(&self, peer: DeviceId) {
        if let Some(session) = self.shared.session(&peer) {
            session.close(session::CLOSE_NORMAL, b"test");
        }
    }

    /// Test hook: a number that tells the session with `peer` from any other.
    #[doc(hidden)]
    pub fn debug_session_serial(&self, peer: DeviceId) -> Option<u64> {
        self.shared.session(&peer).map(|s| s.serial)
    }

    /// Test hook: ends the lanes this side opened to `peer` as a failed
    /// stream would; the next message on each opens it again.
    #[doc(hidden)]
    pub fn debug_fail_lanes(&self, peer: DeviceId) {
        if let Some(session) = self.shared.session(&peer) {
            session.lanes.fail_all();
        }
    }

    /// Test hook: opens a stream to `peer` with header `svc`/`op` and keeps it
    /// open for `hold`; returns the first frame the other side answers with
    /// within half a second, if any.
    #[doc(hidden)]
    pub async fn debug_open_stream(
        &self,
        peer: DeviceId,
        svc: &str,
        op: &str,
        hold: Duration,
    ) -> Result<Option<Envelope>> {
        let session = self.connected(&peer)?;
        let (mut send, mut recv) = session.open_bi().await?;
        let header = g7link_protocol::messages::StreamHeader { svc: svc.into(), op: op.into(), v: 1 };
        g7link_protocol::write_frame(&mut send, &Envelope::new(types::STREAM, &header)?.to_cbor()).await?;
        let first =
            tokio::time::timeout(Duration::from_millis(500), g7link_protocol::read_frame(&mut recv)).await;
        let answer = match first {
            Ok(Ok(Some(frame))) => Some(Envelope::from_cbor(&frame)?),
            _ => None,
        };
        tokio::spawn(async move {
            tokio::time::sleep(hold).await;
            drop((send, recv));
        });
        Ok(answer)
    }

    /// Test hook: whether `peer` asked this device to hold its notifications.
    #[doc(hidden)]
    pub fn debug_notifications_held(&self, peer: DeviceId) -> bool {
        self.shared.session(&peer).is_some_and(|s| s.notifications_held())
    }

    /// Test hook: sends `t` with `body` to `peer` as a request on its lane,
    /// RPC stream or control, as the type says, and returns the answer.
    #[doc(hidden)]
    pub async fn debug_request(&self, peer: DeviceId, env: Envelope, wait: Duration) -> Result<Envelope> {
        self.connected(&peer)?.request(env, wait).await
    }

    // ---- LocalSend compatibility (docs/protocol/v1.md §15) ----

    /// Whether LocalSend v2.1 compatibility is enabled on this device.
    pub fn localsend_enabled(&self) -> bool {
        self.shared.localsend.enabled()
    }

    /// Turns LocalSend v2.1 compatibility on or off (persisted encrypted on disk).
    pub fn set_localsend_enabled(&self, enabled: bool) -> Result<()> {
        self.shared.localsend.set_enabled(&self.shared, enabled)
    }

    /// Whether this device's LocalSend HTTP/HTTPS server is actively bound and
    /// able to receive incoming files (false when turned off, or when TCP port
    /// 53317 is already owned by a standalone LocalSend process, or by G7 Link
    /// in another Android profile, on the same device: that one receives).
    pub fn localsend_receiving(&self) -> bool {
        self.shared.localsend.receiving()
    }

    /// Returns the LocalSend peers currently discovered on the local network.
    /// LocalSend devices nearby, apart from paired devices that also answer
    /// LocalSend (a G7 Link phone does): those are reached as themselves.
    pub fn localsend_peers(&self) -> Vec<crate::LocalSendPeer> {
        let paired =
            lock(&self.shared.paired_localsend).values().cloned().collect::<std::collections::HashSet<_>>();
        let mut peers = self.shared.localsend.peers();
        peers.retain(|p| !paired.contains(&p.fingerprint.to_ascii_lowercase()));
        peers
    }

    /// Broadcasts a fresh LocalSend UDP multicast announcement and probes local ports.
    pub fn refresh_localsend(&self) {
        self.shared.localsend.refresh(&self.shared);
    }

    /// Probes `host:port` via the LocalSend v2.1 HTTP/HTTPS API and adds the peer if found.
    pub async fn probe_localsend_peer(&self, host: &str, port: u16) -> Result<crate::LocalSendPeer> {
        crate::localsend::probe_peer(&self.shared, host, port).await
    }

    // ---- Clipboard (docs/protocol/clipboard.md) ----

    /// Puts text on a paired device's clipboard. Fails with
    /// [`Error::Denied`] when the user turned the clipboard off for that
    /// device (here or there), and [`Error::TooLarge`] beyond
    /// [`crate::CLIP_MAX_BYTES`].
    pub async fn send_clipboard(&self, peer: DeviceId, text: String) -> Result<()> {
        self.send_clipboard_rich(peer, text, None, None).await
    }

    /// Puts text (and optional HTML and RTF rich text) on a paired device's
    /// clipboard. Rich-text fields that would push the payload beyond
    /// [`crate::CLIP_MAX_BYTES`] are dropped so the plain text still arrives.
    pub async fn send_clipboard_rich(
        &self,
        peer: DeviceId,
        text: String,
        html: Option<String>,
        rtf: Option<String>,
    ) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let max = g7link_protocol::messages::CLIP_MAX_BYTES;
        if text.len() > max {
            return Err(Error::TooLarge);
        }
        if !self.shared.store.is_paired(&peer)? {
            return Err(Error::NotPaired);
        }
        if !self.shared.toggle_on(&peer, crate::clipboard::TOGGLE) {
            return Err(Error::Denied);
        }
        let mut used = text.len();
        let html = html.filter(|s| !s.trim().is_empty()).and_then(|s| {
            if used.saturating_add(s.len()) <= max {
                used += s.len();
                Some(s)
            } else {
                None
            }
        });
        let rtf = rtf.filter(|s| !s.trim().is_empty()).filter(|s| used.saturating_add(s.len()) <= max);
        let has_queue = self.shared.store.has_active_queue_for_peer(&peer).unwrap_or(false);
        if !has_queue && self.shared.session(&peer).is_some() {
            let env = Envelope::new(
                types::CLIP_SET,
                &ClipSet { text: text.clone(), html: html.clone(), rtf: rtf.clone(), sensitive: false },
            )?;
            match self.request(peer, env).await.and_then(|r| r.expect(types::OK).map_err(Into::into)) {
                Ok(()) => {
                    let peer_name = self.shared.peer_name(&peer);
                    if let Some((clip_id, evicted)) =
                        self.shared.clipboard_history.record_text_with_id(&text, &peer_name, false)
                    {
                        self.shared.record_timeline_clip(peer, &peer_name, false, "text", clip_id, evicted);
                        self.shared.emit(NodeEvent::ClipboardHistoryChanged);
                    }
                    return Ok(());
                }
                Err(e) if !crate::actions::is_transient_send_error(&e) => return Err(e),
                Err(_) => {}
            }
        }
        // It didn't go over the network, but a Bluetooth link is up: short
        // plain text goes over that.
        if !has_queue && text.len() <= crate::ble_link::MAX_CLIP_TEXT && self.shared.ble_link(&peer).is_some()
        {
            let env = Envelope::new(
                types::CLIP_SET,
                &ClipSet { text: text.clone(), html: None, rtf: None, sensitive: false },
            )?;
            let sent = self.shared.ble_request(peer, env, crate::ble_link::REQUEST_TIMEOUT).await;
            if sent.and_then(|r| r.expect(types::OK).map_err(Into::into)).is_ok() {
                let peer_name = self.shared.peer_name(&peer);
                if let Some((clip_id, evicted)) =
                    self.shared.clipboard_history.record_text_with_id(&text, &peer_name, false)
                {
                    self.shared.record_timeline_clip(peer, &peer_name, false, "text", clip_id, evicted);
                    self.shared.emit(NodeEvent::ClipboardHistoryChanged);
                }
                return Ok(());
            }
        }
        let id = transfer::new_id();
        let payload = crate::QueuePayload::Clipboard { text, html, rtf };
        self.shared.store.enqueue_send_item(&id, &peer, &payload, None, crate::now_unix_ms())?;
        self.shared.emit(NodeEvent::QueueChanged);
        self.shared.queue_notify.notify_waiters();
        Ok(())
    }

    /// Tells a connected phone that this PC's clipboard got something new
    /// (`clip.changed`), so it can offer to paste it. Nothing is sent when the
    /// clipboard is off for the phone; `preview` goes only with text.
    pub async fn announce_clip(
        &self,
        peer: DeviceId,
        kind: crate::PcClipKind,
        preview: Option<String>,
    ) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::clipboard::announce(&self.shared, &session, kind, preview).await
    }

    /// Puts a password or the like on a connected device's clipboard, as the user
    /// chose to allow. Never recorded on either side and never queued:
    /// [`Error::Offline`] when the device isn't connected.
    pub async fn send_clipboard_private(&self, peer: DeviceId, text: String) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        if text.len() > g7link_protocol::messages::CLIP_MAX_BYTES {
            return Err(Error::TooLarge);
        }
        if !self.shared.store.is_paired(&peer)? {
            return Err(Error::NotPaired);
        }
        if !self.shared.toggle_on(&peer, crate::clipboard::TOGGLE) {
            return Err(Error::Denied);
        }
        let short = text.len() <= crate::ble_link::MAX_CLIP_TEXT;
        let env = Envelope::new(types::CLIP_SET, &ClipSet { text, html: None, rtf: None, sensitive: true })?;
        if self.shared.session(&peer).is_none() && short && self.shared.ble_link(&peer).is_some() {
            self.shared.ble_request(peer, env, crate::ble_link::REQUEST_TIMEOUT).await?.expect(types::OK)?;
            return Ok(());
        }
        self.connected(&peer)?;
        self.request(peer, env).await?.expect(types::OK)?;
        Ok(())
    }

    /// Puts an image on a paired device's clipboard (`image/png` or
    /// `image/jpeg`), when the user copied one. [`Error::Unsupported`] when
    /// the device's app can't take images, [`Error::Denied`] when the user
    /// turned the clipboard off for it, and [`Error::TooLarge`] beyond
    /// [`crate::CLIP_MAX_IMAGE_BYTES`].
    pub async fn send_clipboard_image(&self, peer: DeviceId, mime: String, bytes: Vec<u8>) -> Result<()> {
        let Some(peer_rec) = self.shared.store.get_peer(&peer)? else {
            return Err(Error::NotPaired);
        };
        let image = g7link_protocol::messages::ClipImage { mime: mime.clone(), size: bytes.len() as u64 };
        if image.size > g7link_protocol::messages::clip::MAX_IMAGE_BYTES {
            return Err(Error::TooLarge);
        }
        if !image.is_valid() {
            return Err(Error::Internal("not a PNG or JPEG image".into()));
        }
        if !self.shared.toggle_on(&peer, crate::clipboard::TOGGLE) {
            return Err(Error::Denied);
        }
        if !peer_rec.caps.is_empty() && !peer_rec.caps.iter().any(|c| c == "clip.image") {
            return Err(Error::Unsupported);
        }
        let has_queue = self.shared.store.has_active_queue_for_peer(&peer).unwrap_or(false);
        if !has_queue && let Some(session) = self.shared.session(&peer) {
            match crate::clipboard::send_image(&self.shared, &session, mime.clone(), bytes.clone()).await {
                Ok(()) => return Ok(()),
                Err(e) if !crate::actions::is_transient_send_error(&e) => return Err(e),
                Err(_) => {}
            }
        }
        let id = transfer::new_id();
        let payload = crate::QueuePayload::ClipboardImage { mime, width: 0, height: 0 };
        self.shared.store.enqueue_send_item(&id, &peer, &payload, Some(&bytes), crate::now_unix_ms())?;
        self.shared.emit(NodeEvent::QueueChanged);
        self.shared.queue_notify.notify_waiters();
        Ok(())
    }

    /// Puts an image on a connected PC's clipboard now, to paste it where
    /// that PC is typing: never queued for later, and an error when the PC's
    /// clipboard didn't take it.
    pub async fn paste_image(&self, peer: DeviceId, mime: String, bytes: Vec<u8>) -> Result<()> {
        let image = g7link_protocol::messages::ClipImage { mime: mime.clone(), size: bytes.len() as u64 };
        if image.size > g7link_protocol::messages::clip::MAX_IMAGE_BYTES {
            return Err(Error::TooLarge);
        }
        if !image.is_valid() {
            return Err(Error::Internal("not a PNG or JPEG image".into()));
        }
        if !self.shared.toggle_on(&peer, crate::clipboard::TOGGLE) {
            return Err(Error::Denied);
        }
        let peer_rec = self.shared.store.get_peer(&peer)?.ok_or(Error::NotPaired)?;
        if !peer_rec.caps.is_empty() && !peer_rec.caps.iter().any(|c| c == "clip.image") {
            return Err(Error::Unsupported);
        }
        let session = self.connected(&peer)?;
        crate::clipboard::send_image(&self.shared, &session, mime, bytes).await
    }

    /// Whether the local clipboard history is enabled.
    pub fn clipboard_history_enabled(&self) -> bool {
        self.shared.clipboard_history.enabled()
    }

    /// Turns the local clipboard history on or off.
    pub fn set_clipboard_history_enabled(&self, enabled: bool) -> Result<()> {
        self.shared.clipboard_history.set_enabled(enabled)
    }

    /// Lists the local clipboard history (pinned first, then newest first),
    /// optionally filtered by `query`.
    pub fn clipboard_history(&self, query: Option<&str>) -> Vec<crate::ClipboardHistoryEntry> {
        self.shared.clipboard_history.list(query)
    }

    /// Decrypts and returns `(mime, bytes)` for an image entry in the
    /// clipboard history.
    pub fn clipboard_history_image(&self, id: &str) -> Option<(String, Vec<u8>)> {
        self.shared.clipboard_history.image_bytes(id)
    }

    /// Pins or unpins an entry in the clipboard history.
    pub fn pin_clipboard_history(&self, id: &str, pinned: bool) -> bool {
        let changed = self.shared.clipboard_history.set_pinned(id, pinned);
        if changed {
            self.shared.emit(NodeEvent::ClipboardHistoryChanged);
        }
        changed
    }

    /// Deletes one entry from the clipboard history.
    pub fn delete_clipboard_history(&self, id: &str) -> bool {
        let deleted = self.shared.clipboard_history.delete(id);
        let removed_tl = self.shared.store.delete_timeline_by_ref(id).unwrap_or(false);
        if deleted {
            self.shared.emit(NodeEvent::ClipboardHistoryChanged);
        }
        if removed_tl {
            self.shared.emit(NodeEvent::TimelineChanged);
        }
        deleted
    }

    /// Clears the entire clipboard history and deletes its encrypted files from disk.
    pub fn clear_clipboard_history(&self) -> Result<()> {
        self.shared.clipboard_history.clear()?;
        let _ = self.shared.store.delete_timeline_clips();
        // Clips still waiting to send are clipboard history too.
        let _ = self
            .shared
            .store
            .remove_queue_items_of_kinds(&[crate::QueueKind::Clipboard, crate::QueueKind::ClipboardImage]);
        self.shared.emit(NodeEvent::QueueChanged);
        self.shared.emit(NodeEvent::ClipboardHistoryChanged);
        self.shared.emit(NodeEvent::TimelineChanged);
        Ok(())
    }

    /// Copies a clipboard history entry back onto this device's OS clipboard.
    pub fn copy_clipboard_history(&self, id: &str) -> Result<()> {
        let entry = self.shared.clipboard_history.entry(id).ok_or(Error::NotFound)?;
        match entry.kind {
            crate::ClipboardItemKind::Text => {
                self.shared.platform.set_clipboard(&entry.text).map_err(|e| Error::Internal(e.to_string()))
            }
            crate::ClipboardItemKind::Image => {
                let (mime, bytes) = self.shared.clipboard_history.image_bytes(id).ok_or(Error::NotFound)?;
                self.shared
                    .platform
                    .set_clipboard_image(&mime, &bytes)
                    .map_err(|e| Error::Internal(e.to_string()))
            }
        }
    }

    /// Sends a clipboard history entry to `peer` again.
    pub async fn resend_clipboard_history(&self, peer: DeviceId, id: &str) -> Result<()> {
        let entry = self.shared.clipboard_history.entry(id).ok_or(Error::NotFound)?;
        match entry.kind {
            crate::ClipboardItemKind::Text => self.send_clipboard(peer, entry.text).await,
            crate::ClipboardItemKind::Image => {
                let (mime, bytes) = self.shared.clipboard_history.image_bytes(id).ok_or(Error::NotFound)?;
                self.send_clipboard_image(peer, mime, bytes).await
            }
        }
    }

    /// Returns the smart suggestion classified for the most recently received
    /// text clip (`None` when the latest clip had no suggestion or was an OTP).
    pub fn last_clip_suggestion(&self) -> Option<(DeviceId, crate::ClipSuggestion)> {
        self.shared.last_clip_suggestion.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    // ---- Actions (docs/protocol/actions.md) ----

    /// Locks or sleeps a paired PC.
    pub async fn pc_power(&self, peer: DeviceId, action: crate::PowerAction) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::actions::pc_power(&self.shared, &session, action).await
    }

    // ---- Screenshots and recordings of a PC (docs/protocol/pc-capture.md) ----

    /// Asks a paired PC for a screenshot. The answer names the transfer that
    /// brings the PNG, which arrives as any other [`NodeEvent::Transfer`].
    pub async fn pc_capture_shot(
        &self,
        pc: DeviceId,
    ) -> Result<crate::PcCaptureResult, crate::PcCaptureError> {
        crate::pc_capture::shot(&self.shared, pc).await
    }

    /// Asks a paired PC to record its screen; returns once it records.
    pub async fn pc_capture_record_start(&self, pc: DeviceId) -> Result<(), crate::PcCaptureError> {
        crate::pc_capture::record_start(&self.shared, pc).await
    }

    /// Stops a recording of a PC's screen this phone started. The answer names
    /// the transfer that brings the video.
    pub async fn pc_capture_record_stop(
        &self,
        pc: DeviceId,
    ) -> Result<crate::PcCaptureResult, crate::PcCaptureError> {
        crate::pc_capture::record_stop(&self.shared, pc).await
    }

    /// A recording a phone started ended on this PC by itself (its user, a lock,
    /// the length limit): sends `file`, when there is one, and tells the phone.
    pub async fn pc_capture_record_ended(&self, phone: DeviceId, file: Option<std::path::PathBuf>) {
        crate::pc_capture::record_ended(&self.shared, phone, file).await;
    }

    /// Sets this PC's Wake-on-LAN adapter addresses (`pc.wake_info`) and sends
    /// them to every connected phone when they changed.
    pub async fn set_wake_info(&self, info: g7link_protocol::messages::PcWakeInfo) {
        let prev = self.shared.wake_info.get();
        let clean = self.shared.wake_info.set(info);
        if prev.as_ref() != Some(&clean) {
            for session in self.shared.live_sessions() {
                crate::actions::send_wake_info(&self.shared, &session).await;
            }
        }
    }

    /// Returns the stored Wake-on-LAN addresses for a paired PC, if any.
    pub fn wake_info(&self, peer: DeviceId) -> Result<Option<g7link_protocol::messages::PcWakeInfo>> {
        Ok(self.shared.store.get_peer(&peer)?.ok_or(Error::NotPaired)?.wake_info)
    }

    /// Sends Wake-on-LAN magic packets for a paired PC over UDP to its stored
    /// subnet broadcasts and `255.255.255.255` on ports 9 and 7, repeated a
    /// few times over ~2 s.
    pub async fn wake(&self, peer: DeviceId) -> Result<()> {
        crate::actions::wake(
            &self.shared,
            &peer,
            g7link_protocol::messages::wake::DEFAULT_PORTS,
            &[],
            4,
            Duration::from_millis(650),
        )
        .await
    }

    /// Sends Wake-on-LAN magic packets for a paired PC on custom UDP ports and
    /// additional target IPv4 addresses (used by tests).
    pub async fn wake_on_ports(
        &self,
        peer: DeviceId,
        ports: &[u16],
        extra_targets: &[std::net::Ipv4Addr],
    ) -> Result<()> {
        crate::actions::wake(&self.shared, &peer, ports, extra_targets, 2, Duration::from_millis(40)).await
    }

    /// Asks a connected paired peer on the target LAN to broadcast a Wake-on-LAN
    /// magic packet for `mac` (`wake.relay`).
    pub async fn wake_via_relay(
        &self,
        relay_peer: &DeviceId,
        mac: [u8; 6],
        broadcast: Option<std::net::Ipv4Addr>,
    ) -> Result<()> {
        let session = self.connected(relay_peer)?;
        let env = Envelope::new(
            types::PC_WAKE_RELAY,
            &g7link_protocol::messages::WakeRelay { mac, broadcast: broadcast.map(|b| b.to_string()) },
        )?;
        session.request(env, REQUEST_TIMEOUT).await?.expect(types::OK)?;
        Ok(())
    }

    /// Sends a piece of PC sound to a connected paired device: a datagram
    /// (`docs/protocol/v1.md` §5) numbered and timed by the session, or, when
    /// datagrams can't carry it, a frame on the `media/sound` stream. `pcm`
    /// is whole frames of 16-bit PCM, at most 960 bytes.
    pub async fn send_pc_audio_pcm(
        &self,
        peer: &DeviceId,
        sample_rate: u32,
        channels: u8,
        pcm: Vec<u8>,
    ) -> Result<()> {
        if pcm.is_empty() {
            return Ok(());
        }
        let session = self.connected(peer)?;
        crate::sound::send(&session, sample_rate, channels, pcm).await
    }

    /// Opens a web link on a paired device. The other device asks its user
    /// before opening it.
    pub async fn open_link(&self, peer: DeviceId, url: String) -> Result<()> {
        crate::actions::open_link(&self.shared, peer, url, false).await
    }

    /// Opens a link this device's user shared from the system share sheet
    /// themselves on a paired device, which opens it without asking. Only
    /// the share-sheet flow may call this.
    pub async fn open_shared_link(&self, peer: DeviceId, url: String) -> Result<()> {
        crate::actions::open_link(&self.shared, peer, url, true).await
    }

    /// Sends an ongoing or completed task notification (`task.notify`) to a
    /// paired device (or to all connected devices that offer `task.notify`
    /// when `peer` is `None`).
    pub async fn task_notify(&self, peer: Option<DeviceId>, task: crate::TaskNotify) -> Result<()> {
        crate::actions::task_notify(&self.shared, peer, task).await
    }

    // ---- Screen mirroring (docs/protocol/mirror.md) ----

    /// Asks a paired phone for its screen. The phone asks its user; video
    /// then arrives through [`Platform::mirror_sink`](crate::Platform::mirror_sink).
    pub async fn mirror_start(&self, peer: DeviceId, options: crate::MirrorStart) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::mirror::start(&self.shared, &session, options).await
    }

    /// Stops showing a phone's screen or app window (`mirroring`), and
    /// tells the phone.
    pub async fn mirror_stop(&self, peer: DeviceId, mirroring: u32) {
        self.shared.stop_showing(&peer, mirroring);
        if let Ok(session) = self.connected(&peer) {
            let _ = crate::mirror::stop(&session, mirroring).await;
        }
    }

    /// Asks a phone for a keyframe (the decoder lost its place).
    pub async fn mirror_keyframe(&self, peer: DeviceId, mirroring: u32) {
        if let Ok(session) = self.connected(&peer) {
            crate::mirror::request_keyframe(&session, mirroring).await;
        }
    }

    /// Resizes an app window's display (`mirroring` != 0) on a phone.
    pub async fn mirror_resize(&self, peer: DeviceId, mirroring: u32, width: u32, height: u32) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::mirror::resize(&self.shared, &session, mirroring, width, height).await
    }

    /// Updates `stay_awake` and `screen_off` while mirroring a phone's screen (`session == 0`).
    pub async fn mirror_power(&self, peer: DeviceId, stay_awake: bool, screen_off: bool) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::mirror::power(&self.shared, &session, stay_awake, screen_off).await
    }

    /// Sends a mirrored phone's sound to this PC, or leaves it on the phone.
    pub async fn mirror_sound(&self, peer: DeviceId, on: bool) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::mirror::sound(&self.shared, &session, on).await
    }

    /// The PC's mouse and keyboard on a phone's mirrored screen or app window.
    pub async fn mirror_input(
        &self,
        peer: DeviceId,
        mirroring: u32,
        input: crate::MirrorInput,
    ) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::mirror::input(&self.shared, &session, mirroring, input).await
    }

    /// Tells a phone whether what has the focus on this PC takes text
    /// (`remote.text_focus`), when it may control this PC: on the session,
    /// or else the Bluetooth link.
    pub async fn remote_text_focus(&self, peer: DeviceId, editable: bool) -> Result<()> {
        match self.connected(&peer) {
            Err(Error::Offline) if self.shared.ble_link(&peer).is_some() => {
                self.shared.ble_text_focus(peer, editable)
            }
            session => {
                let session = session?;
                crate::remote::text_focus(&self.shared, &session, editable).await
            }
        }
    }

    /// Sends a phone the field that has the focus on this PC
    /// (`remote.text_state`), when it may control this PC: on the path the
    /// phone's last `remote.text_watch` came on, or the other while that one
    /// is down. Over the link it's cut shorter, and a newer one replaces it
    /// while it waits.
    pub async fn remote_text_state(&self, peer: DeviceId, state: &crate::RemoteTextState) -> Result<()> {
        if !self.shared.store.is_paired(&peer)? {
            return Err(Error::NotPaired);
        }
        let session = self.shared.session(&peer);
        let link = self.shared.ble_link(&peer).is_some() && self.shared.peer_offers_ble_remote(&peer);
        let over_link = match crate::remote::watch_via(&self.shared, &peer) {
            Some(crate::RemotePath::Ble) => link,
            _ => session.is_none() && link,
        };
        if over_link {
            return self.shared.ble_text_state(peer, state);
        }
        let session = session.ok_or(Error::Offline)?;
        crate::remote::text_state(&self.shared, &session, state).await
    }

    /// Phone: tells a PC its typing box opened (`on`) or closed
    /// (`remote.text_watch`), on the path input takes: the PC sends its
    /// states back on it. Send it again with `on` when that path changes
    /// ([`NodeEvent::RemoteRouteChanged`]). `Error::Unsupported` when that
    /// PC doesn't mirror its text fields.
    pub async fn remote_text_watch(&self, peer: DeviceId, on: bool) -> Result<()> {
        if !on {
            crate::remote::forget_text_state(&self.shared, &peer);
        }
        match self.shared.path_for(&peer)? {
            crate::RemotePath::Ble => self.shared.ble_text_watch(peer, on),
            crate::RemotePath::Session => {
                let session = self.connected(&peer)?;
                crate::remote::text_watch(&self.shared, &session, on).await
            }
        }
    }

    /// Phone: whether the PC `peer` mirrors its text fields and takes
    /// `RemoteInput::Edit` and `RemoteInput::Caret`.
    pub fn remote_mirrors_text(&self, peer: &DeviceId) -> bool {
        crate::remote::mirrors_text(&self.shared, peer)
    }

    /// PC: keeps the focused app's icon (a PNG of at most
    /// `remote::MAX_ICON_BYTES`) for phones to fetch by `key`, the key its
    /// text states name.
    pub fn keep_app_icon(&self, key: String, png: Vec<u8>) -> Result<()> {
        crate::remote::keep_app_icon(&self.shared, key, png)
    }

    /// Phone: fetches the app icon `key` that a text state from the PC
    /// `peer` named (`remote.app_icon`). `Error::Unsupported` when that PC
    /// serves none.
    pub async fn remote_app_icon(&self, peer: DeviceId, key: String) -> Result<Vec<u8>> {
        let session = self.connected(&peer)?;
        crate::remote::app_icon(&self.shared, &session, key).await
    }

    /// Tells a PC something about its mirroring, by `code` (`mirror.notice`):
    /// say, that this phone refuses its taps.
    pub async fn mirror_notice(&self, peer: DeviceId, mirroring: u32, code: &str) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::mirror::notice(&session, mirroring, code).await
    }

    /// The apps a phone can open in windows of their own (Elevated), by name.
    pub async fn mirror_apps(&self, peer: DeviceId) -> Result<Vec<crate::PhoneApp>> {
        let session = self.connected(&peer)?;
        crate::mirror::apps(&self.shared, &session).await
    }

    /// Opens this phone's video stream to a PC that asked for the screen
    /// or an app window.
    pub async fn mirror_open(&self, peer: DeviceId) -> Result<crate::MirrorStream> {
        self.open_mirror_stream(peer, false).await
    }

    /// Opens this phone's sound stream to a PC that asked for the screen
    /// with its sound.
    pub async fn mirror_open_audio(&self, peer: DeviceId) -> Result<crate::MirrorStream> {
        self.open_mirror_stream(peer, true).await
    }

    /// The PC asked for the stream a moment ago, but the connection it asked
    /// over can drop or be replaced (a reconnect, a duplicate connection
    /// resolved) before the stream opens. Waits for the connection to come
    /// back, and retries once on a new one.
    async fn open_mirror_stream(&self, peer: DeviceId, audio: bool) -> Result<crate::MirrorStream> {
        let first = self.connected_soon(&peer).await?;
        match crate::mirror::open(&self.shared, &first, audio).await {
            Err(err @ (Error::Offline | Error::Network(_))) => {
                let next = self.connected_soon(&peer).await?;
                if Arc::ptr_eq(&next, &first) {
                    return Err(err);
                }
                crate::mirror::open(&self.shared, &next, audio).await
            }
            other => other,
        }
    }

    // ---- Webcam (docs/protocol/webcam.md) ----

    /// Webcam codecs accepted by the connected receiver, in preference order.
    /// Older receivers advertise no HEVC capability and always use H.264.
    pub fn webcam_codecs(&self, peer: DeviceId) -> Result<Vec<String>> {
        let _ = self.connected(&peer)?;
        let record = self.shared.store.get_peer(&peer)?.ok_or(Error::NotPaired)?;
        let mut codecs = Vec::new();
        if record.caps.contains(crate::WEBCAM_HEVC) {
            codecs.push(crate::WEBCAM_H265.into());
        }
        codecs.push(crate::WEBCAM_H264.into());
        Ok(codecs)
    }

    /// Screen codecs accepted by the connected receiver, in preference order.
    /// Older receivers do not advertise HEVC and continue to use H.264.
    pub fn mirror_codecs(&self, peer: DeviceId) -> Result<Vec<String>> {
        let _ = self.connected(&peer)?;
        let record = self.shared.store.get_peer(&peer)?.ok_or(Error::NotPaired)?;
        let mut codecs = Vec::new();
        if record.caps.contains(crate::MIRROR_HEVC) {
            codecs.push(crate::MIRROR_H265.into());
        }
        codecs.push(crate::MIRROR_H264.into());
        Ok(codecs)
    }

    /// Asks a paired phone to start streaming its camera as a webcam.
    /// Video then arrives through [`Platform::webcam_sink`](crate::Platform::webcam_sink).
    pub async fn webcam_start(&self, peer: DeviceId, options: crate::WebcamStart) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::webcam::start(&self.shared, &session, options).await
    }

    /// Stops a webcam stream with `peer` (either direction) and tells `peer`.
    pub async fn webcam_stop(&self, peer: DeviceId) {
        self.shared.stop_webcam(&peer);
        if let Ok(session) = self.connected(&peer) {
            let _ = crate::webcam::stop(&session).await;
        }
    }

    /// Asks a phone for a fresh keyframe on the webcam stream.
    pub async fn webcam_keyframe(&self, peer: DeviceId) {
        if let Ok(session) = self.connected(&peer) {
            crate::webcam::request_keyframe(&session).await;
        }
    }

    /// Opens this phone's camera stream to a paired PC.
    pub async fn webcam_open(&self, peer: DeviceId) -> Result<crate::MirrorStream> {
        let session = self.connected(&peer)?;
        crate::webcam::open(&self.shared, &session).await
    }

    // ---- Phone as microphone (docs/protocol/mic.md) ----

    /// Asks a paired phone to stream its microphone; the phone asks its
    /// user. Sound then arrives through [`Platform::mic_sink`](crate::Platform::mic_sink).
    /// Asks `pc` to start this phone's Elevated helper from `apk` with `token`
    /// after running `grants` (`docs/protocol/elevated-usb.md`): a PC this
    /// phone is plugged into over USB, or, with `adb_port` (the port this
    /// phone's Wireless debugging listens on), one that Wireless debugging
    /// has paired. Returns the helper's loopback port.
    pub async fn elevated_usb_start(
        &self,
        pc: DeviceId,
        apk: String,
        token: String,
        grants: Vec<String>,
        adb_port: Option<u16>,
    ) -> Result<u16> {
        let session = self.connected(&pc)?;
        crate::elevated_usb::start(
            &session,
            g7link_protocol::messages::ElevatedUsbStart { apk, token, grants, adb_port },
        )
        .await
    }

    pub async fn mic_start(&self, peer: DeviceId) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::mic::start(&self.shared, &session).await
    }

    /// Stops a microphone stream with `peer` (either direction) and tells `peer`.
    pub async fn mic_stop(&self, peer: DeviceId) {
        self.shared.stop_mic(&peer);
        if let Ok(session) = self.connected(&peer) {
            let _ = crate::mic::stop(&session).await;
        }
    }

    /// Opens this phone's microphone stream to a paired PC.
    pub async fn mic_open(&self, peer: DeviceId) -> Result<crate::MirrorStream> {
        let session = self.connected(&peer)?;
        crate::mic::open(&self.shared, &session).await
    }

    // ---- Continuity Camera (docs/protocol/v1.md §14) ----

    /// Asks a paired phone to capture a photo or scan a document and stream the result back.
    pub async fn request_camera_capture(
        &self,
        peer: DeviceId,
        request_id: String,
        mode: crate::CameraCaptureMode,
    ) -> Result<()> {
        crate::continuity_camera::request(&self.shared, peer, request_id, mode).await
    }

    /// Cancels an in-flight Continuity Camera request on either side.
    pub async fn cancel_camera_capture(
        &self,
        peer: DeviceId,
        request_id: String,
        reason: Option<String>,
    ) -> Result<()> {
        crate::continuity_camera::cancel(&self.shared, peer, request_id, reason).await
    }

    /// Sends a captured photo or scanned document from this phone to the requesting PC.
    pub async fn send_camera_capture_result(
        &self,
        peer: DeviceId,
        meta: crate::CameraCaptureResultMeta,
        bytes: Vec<u8>,
    ) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::continuity_camera::send_result(&self.shared, &session, meta, bytes).await
    }

    // ---- Messages (docs/protocol/sms.md) ----

    /// This phone's messages changed (in `thread`, or anywhere): connected
    /// PCs that show them catch up.
    pub async fn sms_changed(&self, thread: Option<String>) {
        self.shared.sms_changed(thread).await;
    }

    /// A paired phone's latest conversations, newest first.
    pub async fn sms_threads(&self, peer: DeviceId, limit: u32) -> Result<Vec<crate::SmsThread>> {
        self.sms_threads_and_app(peer, limit).await.map(|(threads, _)| threads)
    }

    /// [`Node::sms_threads`], and the phone's default SMS app (a package
    /// name) when it says which.
    pub async fn sms_threads_and_app(
        &self,
        peer: DeviceId,
        limit: u32,
    ) -> Result<(Vec<crate::SmsThread>, Option<String>)> {
        let session = self.connected(&peer)?;
        crate::sms::threads(&self.shared, &session, limit).await
    }

    /// A conversation's messages before `before` (Unix ms; the latest when
    /// `None`), newest first.
    pub async fn sms_messages(
        &self,
        peer: DeviceId,
        thread: String,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<crate::SmsMessage>> {
        let session = self.connected(&peer)?;
        crate::sms::messages(&self.shared, &session, thread, before, limit).await
    }

    /// Sends a text through a paired phone.
    pub async fn sms_send(&self, peer: DeviceId, to: Vec<String>, body: String) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::sms::send(&self.shared, &session, to, body, Vec::new()).await
    }

    /// Sends a text (no attachments) through a paired phone.
    pub async fn send_sms(
        &self,
        peer: impl std::borrow::Borrow<DeviceId>,
        to: Vec<String>,
        body: String,
    ) -> Result<()> {
        let session = self.connected(peer.borrow())?;
        crate::sms::send(&self.shared, &session, to, body, Vec::new()).await
    }

    /// Sends a text or MMS with optional image attachments through a paired phone.
    pub async fn send_sms_with_attachments(
        &self,
        peer: impl std::borrow::Borrow<DeviceId>,
        to: Vec<String>,
        body: String,
        attachments: Vec<crate::SmsAttachment>,
    ) -> Result<()> {
        let session = self.connected(peer.borrow())?;
        crate::sms::send(&self.shared, &session, to, body, attachments).await
    }

    /// Alias for [`Node::send_sms_with_attachments`].
    pub async fn sms_send_with_attachments(
        &self,
        peer: impl std::borrow::Borrow<DeviceId>,
        to: Vec<String>,
        body: String,
        attachments: Vec<crate::SmsAttachment>,
    ) -> Result<()> {
        self.send_sms_with_attachments(peer, to, body, attachments).await
    }

    /// A picture in a message: its type and bytes.
    pub async fn sms_part(&self, peer: DeviceId, id: String) -> Result<(String, Vec<u8>)> {
        let session = self.connected(&peer)?;
        crate::sms::part(&self.shared, &session, id).await
    }

    // ---- Unified Conversations (MessagingStyle notification threads) ----

    /// The chat threads the Messages list shows for a paired phone, newest
    /// first: live ones (`active`: the notification is on the phone now) and
    /// kept ones whose notification left.
    pub fn chat_threads(&self, peer: impl std::borrow::Borrow<DeviceId>) -> Vec<crate::ChatThreadRecord> {
        self.shared.store.chat_threads(peer.borrow()).unwrap_or_default()
    }

    /// Looks up a stored chat thread by ID (`chat:<app>:i:<conversation ID>`
    /// or `chat:<app>:t:<title>`), listed or not.
    pub fn chat_thread(
        &self,
        peer: impl std::borrow::Borrow<DeviceId>,
        thread_id: &str,
    ) -> Option<crate::ChatThreadRecord> {
        self.shared.store.chat_thread(peer.borrow(), thread_id).ok().flatten()
    }

    /// Lists up to `limit` messages in a locally stored `MessagingStyle` chat
    /// thread, newest first.
    pub fn chat_messages(
        &self,
        peer: impl std::borrow::Borrow<DeviceId>,
        thread_id: &str,
        limit: u32,
    ) -> Vec<crate::ChatMessageRecord> {
        self.shared.store.chat_messages(peer.borrow(), thread_id, limit as usize).unwrap_or_default()
    }

    /// Marks a locally stored `MessagingStyle` chat thread as read (`unread = 0`).
    pub fn mark_chat_thread_read(&self, peer: impl std::borrow::Borrow<DeviceId>, thread_id: &str) -> bool {
        self.shared.store.mark_chat_thread_read(peer.borrow(), thread_id).unwrap_or(false)
    }

    /// Deletes a single `MessagingStyle` chat thread and its messages.
    pub fn delete_chat_thread(&self, peer: impl std::borrow::Borrow<DeviceId>, thread_id: &str) -> bool {
        self.shared.store.delete_chat_thread(peer.borrow(), thread_id).unwrap_or(false)
    }

    /// Pins a chat thread on this PC (`kept`), or unpins it. True when it
    /// changed; then emits [`NodeEvent::SmsChanged`].
    pub fn set_chat_kept(
        &self,
        peer: impl std::borrow::Borrow<DeviceId>,
        thread_id: &str,
        kept: bool,
    ) -> bool {
        let peer = *peer.borrow();
        let changed = self.shared.store.set_chat_kept(&peer, thread_id, kept).unwrap_or(false);
        if changed {
            self.shared.emit(NodeEvent::SmsChanged { device: peer, thread: Some(thread_id.to_owned()) });
        }
        changed
    }

    /// "Remove conversation": deletes a chat thread and its messages here
    /// (stored before anything is sent), then, when its notification is on
    /// the phone, dismisses it there, so the chat doesn't come back with the
    /// next snapshot. An error is the dismiss's; the thread stays deleted. A
    /// thread that's already gone is `Ok`.
    pub async fn remove_chat_thread(
        &self,
        peer: impl std::borrow::Borrow<DeviceId>,
        thread_id: &str,
    ) -> Result<()> {
        let peer = *peer.borrow();
        let id = thread_id.to_owned();
        let removed = crate::session::store_blocking(&self.shared, move |shared| {
            shared.store.take_chat_thread(&peer, &id)
        })
        .await?;
        let Some(thread) = removed else { return Ok(()) };
        self.shared.emit(NodeEvent::SmsChanged { device: peer, thread: Some(thread_id.to_owned()) });
        if thread.active && !thread.notification_key.is_empty() {
            self.dismiss_notification(peer, thread.notification_key).await?;
        }
        Ok(())
    }

    /// Clears all locally stored `MessagingStyle` chat threads.
    pub fn clear_chat_threads(&self) -> usize {
        self.shared.store.clear_chat_threads().unwrap_or(0)
    }

    /// Upserts a `MessagingStyle` conversation notification into the local chat thread store.
    pub fn upsert_chat_notification(
        &self,
        peer: DeviceId,
        notification: &Notification,
        is_snapshot: bool,
    ) -> Result<Option<String>> {
        self.shared.store.upsert_chat_notification(
            &peer,
            notification,
            notification.icon.as_deref(),
            is_snapshot,
        )
    }

    /// Sends an inline reply to a `MessagingStyle` chat thread using its
    /// latest active notification key and reply action ID, and appends the
    /// outgoing message to the local thread history, which keeps the chat.
    pub async fn reply_chat_thread(
        &self,
        peer: impl std::borrow::Borrow<DeviceId>,
        thread_id: &str,
        text: String,
    ) -> Result<()> {
        let peer = *peer.borrow();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(Error::Protocol("empty reply".into()));
        }
        let thread = self.shared.store.chat_thread(&peer, thread_id)?.ok_or(Error::NotFound)?;
        if !thread.active || thread.notification_key.is_empty() {
            return Err(Error::NotFound);
        }
        let Some(action) = thread.reply_action_id else {
            return Err(Error::NotFound);
        };
        let key = thread.notification_key;
        let env =
            Envelope::new(types::NOTIFY_ACTION, &NotifyAction { key, action, reply: Some(trimmed.into()) })?;
        self.request(peer, env).await?.expect(types::OK)?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let _ = self.shared.store.append_chat_reply(&peer, thread_id, trimmed, now_ms);
        self.shared.emit(NodeEvent::SmsChanged { device: peer, thread: Some(thread_id.to_owned()) });
        Ok(())
    }

    // ---- Calls (docs/protocol/calls.md) ----

    /// A call on this phone rang, was answered or ended; sent to every
    /// connected PC the user allows (and to PCs that connect during it).
    pub async fn call_changed(&self, call: crate::CallState) -> Result<()> {
        self.shared.call_changed(call).await
    }

    /// Answers, declines or silences a call on a paired phone.
    pub async fn call_command(&self, peer: DeviceId, id: String, command: crate::CallCommand) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::calls::command(&self.shared, &session, id, command).await
    }

    /// This phone's call history changed: connected PCs that show it catch up.
    pub async fn call_log_changed(&self) {
        self.shared.call_log_changed().await;
    }

    /// A paired phone's recent calls before `before` (Unix ms; the latest
    /// when `None`), newest first.
    pub async fn call_log(
        &self,
        peer: DeviceId,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<crate::CallLogEntry>> {
        let session = self.connected(&peer)?;
        crate::calls::log(&self.shared, &session, before, limit).await
    }

    /// Asks a paired phone to place a call to `number` (or open the dialer
    /// with it filled in), on the SIM `sim` (an id from [`call_sims`](Self::call_sims))
    /// when given.
    pub async fn call_dial(&self, peer: DeviceId, number: String, sim: Option<String>) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::calls::dial(&self.shared, &session, number, sim).await
    }

    /// A paired phone's SIMs, to choose which one [`call_dial`](Self::call_dial)
    /// uses; empty for a phone that doesn't list them.
    pub async fn call_sims(&self, peer: DeviceId) -> Result<crate::CallSims> {
        let session = self.connected(&peer)?;
        crate::calls::sims(&self.shared, &session).await
    }

    // ---- Contacts (docs/protocol/contacts.md) ----

    /// This phone's contacts changed: connected PCs that show them catch up.
    pub async fn contacts_changed(&self) {
        self.shared.contacts_changed().await;
    }

    /// Lists or searches a paired phone's contacts (favorites first, then
    /// alphabetical), skipping `offset`.
    pub async fn contacts(
        &self,
        peer: DeviceId,
        query: Option<String>,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<crate::Contact>> {
        let session = self.connected(&peer)?;
        crate::contacts::list(&self.shared, &session, query, offset, limit).await
    }

    // ---- Photos (docs/protocol/photos.md) ----

    /// A photo or screenshot just appeared on this phone; announced to every
    /// connected PC the user allows. PCs then ask for it through
    /// [`Platform::open_photo`](crate::Platform::open_photo).
    pub async fn photo_taken(&self, photo: crate::Photo) -> Result<()> {
        self.shared.photo_taken(photo).await
    }

    /// This phone's photo or video library changed: connected PCs that show
    /// photos catch up.
    pub async fn photos_changed(&self) {
        self.shared.photos_changed().await;
    }

    /// Lists a paired phone's photo and video albums.
    pub async fn photo_albums(&self, peer: DeviceId) -> Result<Vec<crate::PhotoAlbum>> {
        let session = self.connected(&peer)?;
        crate::photos::albums(&self.shared, &session).await
    }

    /// Lists a paired phone's photos and videos (in `album`, or all when
    /// `None`), newest first: from the latest, or after `before`, the
    /// previous page's last item (its date and ID).
    pub async fn photo_list(
        &self,
        peer: DeviceId,
        album: Option<String>,
        before: Option<(i64, String)>,
        limit: u32,
    ) -> Result<Vec<crate::PhotoItem>> {
        let session = self.connected(&peer)?;
        crate::photos::list(&self.shared, &session, album, before, limit).await
    }

    /// Fetches small JPEG thumbnails for a batch of item IDs on a paired phone.
    pub async fn photo_thumbs(&self, peer: DeviceId, ids: Vec<String>) -> Result<Vec<crate::PhotoThumb>> {
        let session = self.connected(&peer)?;
        crate::photos::thumbs(&self.shared, &session, ids).await
    }

    /// Moves photos and videos to a phone's trash; returns how many went.
    pub async fn photos_delete(&self, peer: DeviceId, ids: Vec<String>) -> Result<u32> {
        let session = self.connected(&peer)?;
        crate::photos::delete(&self.shared, &session, ids).await
    }

    /// Asks a phone for a photo or video; returns the ID of the transfer
    /// that brings it (reported like any other).
    pub async fn fetch_photo(&self, peer: DeviceId, id: String) -> Result<String> {
        let session = self.connected(&peer)?;
        crate::photos::fetch(&self.shared, &session, id).await
    }

    /// Asks a phone for one or more photos or videos in a single transfer;
    /// returns the transfer ID.
    pub async fn fetch_photos(&self, peer: DeviceId, ids: Vec<String>) -> Result<String> {
        let session = self.connected(&peer)?;
        crate::photos::fetch_many(&self.shared, &session, ids).await
    }

    /// Like [`fetch_photos`](Self::fetch_photos), for a backup: the files are
    /// saved as usual, but the transfer leaves nothing in the Timeline.
    pub async fn fetch_photos_quietly(&self, peer: DeviceId, ids: Vec<String>) -> Result<String> {
        let session = self.connected(&peer)?;
        crate::photos::fetch_quietly(&self.shared, &session, ids).await
    }

    /// Keeps a transfer started by [`fetch_photos_quietly`](Self::fetch_photos_quietly)
    /// quiet after a restart, when it resumes.
    pub fn keep_transfer_quiet(&self, transfer_id: &str) {
        crate::transfer::mark_quiet_transfer(&self.shared, transfer_id);
    }

    // ---- Remote input (docs/protocol/remote.md) ----

    /// Checks whether a paired PC currently accepts remote input from this
    /// phone. If `remote_input` is off on the PC, returns [`Error::Denied`]
    /// and triggers the PC's one-time prompt the first time.
    pub async fn remote_check(&self, peer: DeviceId) -> Result<()> {
        match self.shared.path_for(&peer)? {
            crate::RemotePath::Ble => self.shared.ble_input_check(peer).await,
            crate::RemotePath::Session => {
                let session = self.connected(&peer)?;
                crate::remote::check(&self.shared, &session).await
            }
        }
    }

    /// Sends a remote input event to a paired PC, on the path its route
    /// gives (`set_remote_route`): the session, the Bluetooth link, or the
    /// other when that one is down. Discrete input goes one at a time: each
    /// waits for the one before it to be answered, whatever path each took.
    pub async fn remote_input(&self, peer: DeviceId, input: crate::RemoteInput) -> Result<()> {
        let lane = self.shared.input_lane(&peer)?;
        let _turn = lane.lock().await;
        let path = self.shared.path_for(&peer)?;
        let started = self.shared.input_started(&peer, &input);
        let result = match path {
            crate::RemotePath::Ble => self.shared.ble_input(peer, input).await,
            crate::RemotePath::Session => match self.connected(&peer) {
                Ok(session) => crate::remote::input(&self.shared, &session, input).await,
                Err(error) => Err(error),
            },
        };
        self.shared.input_done(&peer, path, started, &result);
        result
    }

    /// Sends relative pointer motion `(dx, dy)` in a QUIC datagram (never on
    /// the control stream), or over the Bluetooth link.
    pub async fn remote_move(&self, peer: DeviceId, dx: f32, dy: f32) -> Result<()> {
        let path = self.shared.path_for(&peer)?;
        self.shared.input_moved(&peer);
        match path {
            crate::RemotePath::Ble => self.shared.ble_input(peer, crate::RemoteInput::Move { dx, dy }).await,
            crate::RemotePath::Session => {
                let session = self.connected(&peer)?;
                crate::remote::move_pointer(&self.shared, &session, dx, dy).await
            }
        }
    }

    /// Sends smooth scroll `(dx, dy)` in a QUIC datagram, or over the Bluetooth link.
    pub async fn remote_scroll_fast(&self, peer: DeviceId, dx: f32, dy: f32) -> Result<()> {
        let path = self.shared.path_for(&peer)?;
        self.shared.input_moved(&peer);
        match path {
            crate::RemotePath::Ble => {
                self.shared.ble_input(peer, crate::RemoteInput::Scroll { dx, dy }).await
            }
            crate::RemotePath::Session => {
                let session = self.connected(&peer)?;
                crate::remote::scroll_fast(&self.shared, &session, dx, dy).await
            }
        }
    }

    /// Sends a laser pointer update to a paired PC (`x`, `y` in `0.0..=1.0`).
    /// Turning it off goes in its turn, as a key does.
    pub async fn remote_laser(&self, peer: DeviceId, on: bool, x: f32, y: f32) -> Result<()> {
        let event = crate::RemoteInput::Laser { on, x, y };
        let lane = if on { None } else { Some(self.shared.input_lane(&peer)?) };
        let _turn = match &lane {
            Some(lane) => Some(lane.lock().await),
            None => None,
        };
        let path = self.shared.path_for(&peer)?;
        let started = if on {
            self.shared.input_moved(&peer);
            None
        } else {
            Some(self.shared.input_started(&peer, &event))
        };
        let result = match path {
            crate::RemotePath::Ble => self.shared.ble_input(peer, event).await,
            crate::RemotePath::Session => match self.connected(&peer) {
                Ok(session) => crate::remote::laser(&self.shared, &session, on, x, y).await,
                Err(error) => Err(error),
            },
        };
        if let Some(started) = started {
            self.shared.input_done(&peer, path, started, &result);
        }
        result
    }

    // ---- Media (docs/protocol/media.md) ----

    /// This device's media players changed (most relevant first); sent to
    /// every connected device the user allows. Include each player's
    /// artwork every time: the core sends it once per device and session.
    pub async fn media_changed(&self, players: Vec<MediaPlayer>) {
        self.shared.media_changed(players).await;
    }

    /// Play on phone: offers `peer` (a phone) what this PC's `player` plays,
    /// streamed (`docs/protocol/media.md` §4). The phone shows the offer;
    /// taking it starts mirroring with `watch`, which this PC then allows
    /// for that phone and player.
    pub async fn offer_watch(&self, peer: DeviceId, player: String, title: Option<String>) -> Result<()> {
        crate::media::offer_watch(&self.shared, peer, player, title, None).await
    }

    /// Play on phone for a video file: offers `peer` the `file` this PC's
    /// `player` plays, at `position_ms`, for the phone to play itself. The
    /// phone reads it with [`Node::watch_file_read`] while the offer is open,
    /// and only that file.
    pub async fn offer_watch_file(
        &self,
        peer: DeviceId,
        player: String,
        title: Option<String>,
        file: std::path::PathBuf,
        position_ms: u64,
    ) -> Result<()> {
        crate::media::offer_watch(&self.shared, peer, player, title, Some((file, position_ms))).await
    }

    /// On the phone: reads up to `length` bytes (at most 4 MiB) from
    /// `offset` of the file the PC `peer` offered. Returns the file's size
    /// and the bytes (fewer at its end).
    pub async fn watch_file_read(&self, peer: DeviceId, offset: u64, length: u64) -> Result<(u64, Vec<u8>)> {
        let session = self.connected(&peer)?;
        crate::media::read_file(&session, offset, length).await
    }

    /// Runs a command on a paired device's media player.
    pub async fn media_command(
        &self,
        peer: DeviceId,
        player: String,
        action: crate::MediaAction,
        position: Option<u64>,
    ) -> Result<()> {
        if !self.shared.toggle_on(&peer, crate::media::TOGGLE) {
            return Err(Error::Denied);
        }
        let command = MediaCommand { player, action: action.as_str().into(), position, value: None };
        let env = Envelope::new(types::MEDIA_COMMAND, &command)?;
        self.request(peer, env).await?.expect(types::OK)?;
        Ok(())
    }

    /// Runs a command that takes a value on a paired device's media player:
    /// [`MediaAction::Shuffle`](crate::MediaAction::Shuffle) with `on` or
    /// `off`, [`MediaAction::Loop`](crate::MediaAction::Loop) with `none`,
    /// `track` or `playlist`.
    pub async fn media_command_value(
        &self,
        peer: DeviceId,
        player: String,
        action: crate::MediaAction,
        value: String,
    ) -> Result<()> {
        if !self.shared.toggle_on(&peer, crate::media::TOGGLE) {
            return Err(Error::Denied);
        }
        let command =
            MediaCommand { player, action: action.as_str().into(), position: None, value: Some(value) };
        if !command.value_fits() {
            return Err(Error::Protocol("that action doesn't take that value".into()));
        }
        let env = Envelope::new(types::MEDIA_COMMAND, &command)?;
        self.request(peer, env).await?.expect(types::OK)?;
        Ok(())
    }

    /// The large picture a paired device's player names as `art_full`
    /// (`docs/protocol/media.md` §2.3): its bytes (at most 2 MiB) and format.
    /// Fetch it when it's shown, and keep it in memory by key.
    pub async fn media_art(&self, peer: DeviceId, key: String) -> Result<(Vec<u8>, crate::ArtFormat)> {
        if !self.shared.toggle_on(&peer, crate::media::TOGGLE) {
            return Err(Error::Denied);
        }
        let session = self.connected(&peer)?;
        crate::media::read_art(&self.shared, &session, key).await
    }

    /// The app icon (PNG, at most 16 KiB) a paired device's player names as
    /// `icon` (`docs/protocol/media.md` §2.2).
    pub async fn media_icon(&self, peer: DeviceId, key: String) -> Result<Vec<u8>> {
        if !self.shared.toggle_on(&peer, crate::media::TOGGLE) {
            return Err(Error::Denied);
        }
        let session = self.connected(&peer)?;
        crate::media::icon(&self.shared, &session, key).await
    }

    // ---- Notifications (docs/protocol/notifications.md) ----

    /// A notification appeared or changed on this phone; sent to every
    /// connected PC the user allows. Include the app icon (PNG) each time:
    /// the core sends it once per PC and session.
    /// On a PC: phones send no notifications at all while `held`, and catch it
    /// up with what they show when it ends.
    pub async fn hold_notifications(&self, held: bool) {
        self.shared.hold_notifications(held).await;
    }

    pub async fn notification_posted(&self, notification: Notification) {
        if let Some(clean) = history_copy(&notification).sanitized() {
            let mut hist = lock(&self.shared.notification_history);
            hist.retain(|n| n.key != clean.key);
            let at = hist.partition_point(|n| history_order(n, &clean).is_lt());
            hist.insert(at, clean);
            // Kept in memory: drop the oldest past the cap.
            hist.truncate(NOTIFICATION_HISTORY_MAX);
        }
        self.shared.notification_posted(notification).await;
    }

    /// A notification went away on this phone.
    pub async fn notification_removed(&self, key: String) {
        self.shared.notification_removed(key).await;
    }

    /// Everything this phone shows now (when the notification listener
    /// connects, or an empty list when access was revoked).
    pub async fn notifications_reset(&self, items: Vec<Notification>) {
        self.record_notification_history(&items);
        self.shared.notifications_reset(items).await;
    }

    /// Records a batch of notifications in the local notification history for
    /// listing and substring search.
    pub fn record_notification_history(&self, items: &[Notification]) {
        let mut clean: Vec<Notification> = items.iter().filter_map(|n| history_copy(n).sanitized()).collect();
        clean.sort_by(history_order);
        *lock(&self.shared.notification_history) = clean;
    }

    /// Lists notifications in the local history (newest first), optionally
    /// filtered by a case-insensitive substring `query` matching title, text,
    /// subtext, app display name, or package name.
    pub fn notification_history(&self, query: Option<&str>, limit: usize) -> Vec<Notification> {
        let q = query.map(str::trim).filter(|s| !s.is_empty()).map(str::to_ascii_lowercase);
        let hist = lock(&self.shared.notification_history);
        let cap = limit.max(1);
        hist.iter()
            .filter(|n| match q.as_deref() {
                None => true,
                Some(needle) => {
                    n.app.to_ascii_lowercase().contains(needle)
                        || n.app_name.to_ascii_lowercase().contains(needle)
                        || n.title.as_deref().is_some_and(|s| s.to_ascii_lowercase().contains(needle))
                        || n.text.as_deref().is_some_and(|s| s.to_ascii_lowercase().contains(needle))
                        || n.sub.as_deref().is_some_and(|s| s.to_ascii_lowercase().contains(needle))
                }
            })
            .take(cap)
            .cloned()
            .collect()
    }

    /// Dismisses a phone's notification there.
    pub async fn dismiss_notification(&self, peer: DeviceId, key: String) -> Result<()> {
        let env = Envelope::new(types::NOTIFY_DISMISS, &NotifyKey { key })?;
        self.request(peer, env).await?.expect(types::OK)?;
        Ok(())
    }

    /// Runs an action of a phone's notification; `reply` is the text for a
    /// reply action.
    pub async fn run_notification_action(
        &self,
        peer: DeviceId,
        key: String,
        action: String,
        reply: Option<String>,
    ) -> Result<()> {
        let env = Envelope::new(
            types::NOTIFY_ACTION,
            &NotifyAction { key: key.clone(), action, reply: reply.clone() },
        )?;
        self.request(peer, env).await?.expect(types::OK)?;
        if let Some(text) = reply.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64);
            if let Ok(Some(thread_id)) = self.shared.store.append_chat_reply_by_key(&peer, &key, text, now_ms)
            {
                self.shared.emit(NodeEvent::SmsChanged { device: peer, thread: Some(thread_id) });
            }
        }
        Ok(())
    }

    // ---- Phone toggles (docs/protocol/toggles.md) ----

    /// The latest quick settings reported by a connected phone, if any.
    pub fn phone_toggles(&self, peer: DeviceId) -> Option<crate::PhoneToggles> {
        self.shared.toggles.peer(&peer)
    }

    /// This phone's quick settings state (on startup and whenever any toggle
    /// changes); sent to every connected PC the user allows (and to PCs that
    /// connect later).
    pub async fn toggles_changed(&self, toggles: crate::PhoneToggles) -> Result<()> {
        self.shared.toggles_changed(toggles).await
    }

    /// Asks a paired phone to change one quick setting (`id`: `dnd`, `ringer`,
    /// `flashlight`, `volume`, `brightness`, `wifi`, `bluetooth`).
    pub async fn set_phone_toggle(
        &self,
        peer: DeviceId,
        id: String,
        value: crate::PhoneToggleValue,
    ) -> Result<()> {
        // A phone the PC shares no network with may turn its hotspot on over Bluetooth.
        if self.shared.session(&peer).is_none()
            && id == g7link_protocol::messages::toggle_ids::HOTSPOT
            && self.shared.ble_usable(&peer)
        {
            return crate::toggles::set_over_ble(&self.shared, peer, value).await;
        }
        let session = self.connected(&peer)?;
        crate::toggles::set(&self.shared, &session, id, value).await
    }

    // ---- Deck (docs/protocol/deck.md) ----

    /// The latest Deck layout reported by a connected PC, if any.
    pub fn deck_layout(&self, peer: DeviceId) -> Option<crate::DeckLayout> {
        self.shared.deck.peer_layout(&peer)
    }

    /// The latest live Deck state reported by a connected PC, if any.
    pub fn deck_state(&self, peer: DeviceId) -> Option<crate::DeckState> {
        self.shared.deck.peer_state(&peer)
    }

    /// Updates this PC's Deck layout and sends it to every connected phone
    /// when this device offers `deck.actions`.
    pub async fn set_deck_layout(&self, layout: crate::DeckLayout) -> Result<()> {
        self.shared.set_deck_layout(layout).await
    }

    /// Updates this PC's live Deck state and sends it to every connected phone
    /// when it changed and this device offers `deck.actions`.
    pub async fn set_deck_state(&self, state: crate::DeckState) -> Result<()> {
        self.shared.set_deck_state(state).await
    }

    // ---- Agents (docs/protocol/agents.md) ----

    /// PC: replaces what phones may start here and sends it to the phones
    /// that may have it when it changed. `None`: the host is gone (they get
    /// an empty catalog).
    pub async fn set_agent_catalog(&self, catalog: Option<crate::AgentCatalog>) -> Result<()> {
        crate::agents::set_catalog(&self.shared, catalog);
        Ok(())
    }

    /// PC: passes a thread event to the phones that get it: `delta` and
    /// `item` to the phone watching that thread, the rest to every phone with
    /// the `agents` toggle (`docs/protocol/agents.md` §2.1). `started_by` is
    /// the phone that started the thread, which alone sees `from_phone`.
    pub async fn agent_event(&self, event: crate::AgentEvent, started_by: Option<DeviceId>) -> Result<()> {
        crate::agents::send_event(&self.shared, event, started_by)
    }

    /// PC: an agent asks; every phone with the `agents` toggle gets it.
    pub async fn agent_ask(&self, ask: crate::AgentAsk) -> Result<()> {
        crate::agents::send_ask(&self.shared, ask)
    }

    /// PC: an ask ended (`outcome`: `allowed`, `denied`, `expired` or
    /// `cancelled`); `by_name` names the device that answered, and `message`
    /// is what the user said when denying it (`denied` only).
    pub async fn agent_asked(&self, asked: crate::AgentAsked) -> Result<()> {
        crate::agents::send_asked(&self.shared, asked)
    }

    /// Phone: the last catalog `peer` sent this session, if it had agents.
    pub fn agent_catalog(&self, peer: DeviceId) -> Option<crate::AgentCatalog> {
        self.shared.agents.peer_catalog(&peer)
    }

    /// Phone: whether `peer` takes a denial with the user's words
    /// (`agent.say_why`), as it last said.
    pub fn agent_can_say_why(&self, peer: DeviceId) -> bool {
        crate::agents::can_say_why(&self.shared, &peer)
    }

    /// Phone: whether `peer` and this phone both offer `agent.control`
    /// (models, plan mode, the queue, questions, images, files, commands and
    /// drafts, `docs/protocol/agents.md` §5).
    pub fn agent_can_control(&self, peer: DeviceId) -> bool {
        crate::agents::can_control(&self.shared, &peer)
    }

    /// Phone: `peer`'s threads, newest first.
    pub async fn agent_threads(&self, peer: DeviceId) -> Result<Vec<crate::AgentThread>> {
        crate::agents::threads(&self.shared, peer).await
    }

    /// Phone: starts a thread on `peer`.
    pub async fn agent_start(&self, peer: DeviceId, start: crate::AgentStart) -> Result<crate::AgentThread> {
        crate::agents::start(&self.shared, peer, start).await
    }

    /// Phone: sends a follow-up to a thread that isn't running.
    pub async fn agent_send(&self, peer: DeviceId, thread: String, text: String) -> Result<()> {
        let body = crate::agents::AgentSend { thread, text, ..Default::default() };
        crate::agents::send(&self.shared, peer, body).await
    }

    /// Phone: sends a message to a thread, with what §5 adds: `dispatch`
    /// (steer or queue while a turn runs) and images sent first with
    /// [`agent_image_send`](Self::agent_image_send). Those need
    /// `agent.control` on both sides (`Unsupported` otherwise, and nothing
    /// is sent). It reaches the thread through its queue when the PC queued
    /// it: the thread's next state says so.
    pub async fn agent_send_with(&self, peer: DeviceId, send: crate::agents::AgentSend) -> Result<()> {
        crate::agents::send(&self.shared, peer, send).await
    }

    /// Phone: stops a thread's running turn.
    pub async fn agent_cancel(&self, peer: DeviceId, thread: String) -> Result<()> {
        crate::agents::cancel(&self.shared, peer, thread).await
    }

    /// Phone: a page of a thread's items, the newest `limit` (at most 50) or
    /// the ones before `before`.
    pub async fn agent_read(
        &self,
        peer: DeviceId,
        thread: String,
        before: Option<u64>,
        limit: u32,
    ) -> Result<crate::AgentItems> {
        crate::agents::read(&self.shared, peer, thread, before, limit).await
    }

    /// Phone: the one thread whose text this phone shows (`None`: none).
    pub async fn agent_watch(&self, peer: DeviceId, thread: Option<String>) -> Result<()> {
        crate::agents::watch(&self.shared, peer, thread).await
    }

    /// Phone: answers an ask, this time only. `message` goes with a denial
    /// only, to a PC that offers `agent.say_why` ([`agent_can_say_why`](Self::agent_can_say_why)):
    /// `Unsupported` otherwise, and the ask still waits.
    pub async fn agent_answer(
        &self,
        peer: DeviceId,
        token: String,
        allow: bool,
        message: Option<String>,
    ) -> Result<()> {
        crate::agents::answer_ask(&self.shared, peer, token, allow, message).await
    }

    /// PC: a session started on this PC outside G7 Link appeared or changed
    /// (`session`), or left the list (`removed`, its id), for the phones with
    /// both `agents` and `agent_sessions` on (`docs/protocol/agents.md` §3).
    /// A session's changes go at most once per 2 s (the newest is kept); its
    /// folder goes as a name, never a path.
    pub async fn agent_session(
        &self,
        session: Option<crate::AgentSession>,
        removed: Option<String>,
    ) -> Result<()> {
        crate::agents::send_session(&self.shared, crate::agents::AgentSessionEvent { session, removed })
    }

    /// PC: an item of a session was added or updated; only a phone watching
    /// that session gets it.
    pub async fn agent_session_item(&self, session: String, item: crate::AgentItem) -> Result<()> {
        crate::agents::send_session_item(&self.shared, session, item)
    }

    /// Phone: the sessions started on `peer` outside G7 Link (at most 50).
    /// `Denied` while either toggle is off there.
    pub async fn agent_sessions(&self, peer: DeviceId) -> Result<Vec<crate::AgentSession>> {
        crate::agents::sessions(&self.shared, peer).await
    }

    /// Phone: a page of a session's items, the newest `limit` (at most 50) or
    /// the ones before `before`. Their `seq`s grow but aren't consecutive.
    pub async fn agent_session_read(
        &self,
        peer: DeviceId,
        session: String,
        before: Option<u64>,
        limit: u32,
    ) -> Result<crate::AgentSessionItems> {
        crate::agents::session_read(&self.shared, peer, session, before, limit).await
    }

    /// Phone: the one session whose items this phone shows (`None`: none).
    pub async fn agent_session_watch(&self, peer: DeviceId, session: Option<String>) -> Result<()> {
        crate::agents::session_watch(&self.shared, peer, session).await
    }

    /// Phone: makes a session a supervised thread started by this phone.
    pub async fn agent_session_continue(
        &self,
        peer: DeviceId,
        session: String,
    ) -> Result<crate::AgentThread> {
        crate::agents::session_continue(&self.shared, peer, session).await
    }

    /// Phone: a thread (`thread`) or a session (`session`) of `peer` as
    /// Markdown to share or save, at most 2 MiB, on a stream of its own
    /// (`docs/protocol/agents.md` §4). Exactly one of the two; a session
    /// needs `agent_sessions` on there too. Kept in memory only.
    pub async fn agent_export(
        &self,
        peer: DeviceId,
        thread: Option<String>,
        session: Option<String>,
    ) -> Result<crate::AgentExport> {
        crate::agents::export(&self.shared, peer, crate::agents::AgentExportRequest { thread, session }).await
    }

    /// [`agent_export`](Self::agent_export) without checking the request
    /// here, so tests can see what a PC refuses.
    #[doc(hidden)]
    pub async fn debug_agent_export(
        &self,
        peer: DeviceId,
        request: crate::agents::AgentExportRequest,
    ) -> Result<crate::AgentExport> {
        crate::agents::export_unchecked(&self.shared, peer, request).await
    }

    // ---- Agents: control (docs/protocol/agents.md §5) ----
    //
    // Each needs `agent.control` on both sides: `Unsupported` otherwise,
    // without sending anything.

    /// PC: an agent asks the user something; every phone that gets §5 is
    /// told, a secret question without its asks (and a phone's answer to it
    /// is refused from then on).
    pub async fn agent_question(&self, question: crate::agents::AgentQuestion) -> Result<()> {
        crate::agents::send_question(&self.shared, question)
    }

    /// PC: a question ended (`outcome`: `answered`, `dismissed`, `expired`
    /// or `cancelled`); `by_name` names the device that answered.
    pub async fn agent_questioned(&self, questioned: crate::agents::AgentQuestioned) -> Result<()> {
        crate::agents::send_questioned(&self.shared, questioned)
    }

    /// PC: draft `key` changed (now `rev`); every phone that gets §5 but
    /// `except` (the one that changed it, if a phone did) is told.
    pub async fn agent_draft(&self, key: String, rev: u64, except: Option<DeviceId>) -> Result<()> {
        crate::agents::send_draft(&self.shared, key, rev, except)
    }

    /// Phone: changes a thread's model, effort or mode for its next turn
    /// (`supervised` or `plan` only); the thread as it is now.
    pub async fn agent_configure(
        &self,
        peer: DeviceId,
        configure: crate::agents::AgentConfigure,
    ) -> Result<crate::AgentThread> {
        crate::agents::configure(&self.shared, peer, configure).await
    }

    /// Phone: edits, removes or sends now one queued message.
    pub async fn agent_queue(&self, peer: DeviceId, change: crate::agents::AgentQueueChange) -> Result<()> {
        crate::agents::queue(&self.shared, peer, change).await
    }

    /// Phone: answers a question (one answer per ask), or dismisses it. A
    /// secret question is answered on the PC only (`Denied`).
    pub async fn agent_reply(&self, peer: DeviceId, reply: crate::agents::AgentQuestionReply) -> Result<()> {
        crate::agents::reply(&self.shared, peer, reply).await
    }

    /// Phone: implements the thread's newest proposed plan (`item`), asking
    /// before edits and commands.
    pub async fn agent_implement(&self, peer: DeviceId, thread: String, item: String) -> Result<()> {
        crate::agents::implement(&self.shared, peer, crate::agents::AgentImplement { thread, item }).await
    }

    /// Phone: files of a thread's folder (or a folder) matching `query`, for `@`.
    pub async fn agent_files(
        &self,
        peer: DeviceId,
        request: crate::agents::AgentFilesRequest,
    ) -> Result<crate::agents::AgentFiles> {
        crate::agents::files(&self.shared, peer, request).await
    }

    /// Phone: the slash commands `agent` offers in `folder`.
    pub async fn agent_commands(
        &self,
        peer: DeviceId,
        agent: String,
        folder: String,
    ) -> Result<Vec<crate::agents::AgentCommand>> {
        crate::agents::commands(&self.shared, peer, crate::agents::AgentCommandsRequest { agent, folder })
            .await
    }

    /// Phone: draft `key` (a thread id, or `new:` and a folder id), if there is one.
    pub async fn agent_draft_get(
        &self,
        peer: DeviceId,
        key: String,
    ) -> Result<Option<crate::agents::AgentDraft>> {
        crate::agents::draft_get(&self.shared, peer, key).await
    }

    /// Phone: stores a draft when `rev` is still its `rev` (0 for a new
    /// one); otherwise the reply's `conflict` is the draft as it is.
    pub async fn agent_draft_set(
        &self,
        peer: DeviceId,
        set: crate::agents::AgentDraftSet,
    ) -> Result<crate::agents::AgentDraftSaved> {
        crate::agents::draft_set(&self.shared, peer, set).await
    }

    /// Phone: sends one image (at most 10 MiB) for a message, on a stream
    /// of its own; one at a time (`BUSY` while another runs). Its id goes in
    /// the message.
    pub async fn agent_image_send(
        &self,
        peer: DeviceId,
        name: String,
        bytes: Vec<u8>,
    ) -> Result<crate::agents::AgentImage> {
        crate::agents::image_send(&self.shared, peer, name, bytes).await
    }

    /// [`agent_image_send`](Self::agent_image_send) with a meta that needn't
    /// match the bytes, so tests can see what a PC refuses.
    #[doc(hidden)]
    pub async fn debug_agent_image_send(
        &self,
        peer: DeviceId,
        meta: crate::agents::AgentImageMeta,
        bytes: Vec<u8>,
    ) -> Result<crate::agents::AgentImage> {
        crate::agents::image_send_unchecked(&self.shared, peer, meta, bytes).await
    }

    /// Phone: a JPEG of image `id`, at most 150 KiB, no larger than `max` px
    /// a side (64 to 1024; 512 when `None`).
    pub async fn agent_image_thumb(&self, peer: DeviceId, id: String, max: Option<u32>) -> Result<Vec<u8>> {
        crate::agents::image_thumb(&self.shared, peer, id, max).await
    }

    /// Sends an `agent.*` request as is, without the checks the methods
    /// above make, so tests can see what a PC refuses.
    #[doc(hidden)]
    pub async fn debug_agent_request<T: serde::Serialize>(
        &self,
        peer: DeviceId,
        t: &str,
        body: &T,
    ) -> Result<()> {
        crate::agents::raw(&self.shared, peer, t, body).await
    }

    /// Asks a paired PC to run the action bound to `tile` (`deck.press`).
    pub async fn deck_press(&self, peer: DeviceId, tile: String) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::deck::press(&self.shared, &session, tile).await
    }

    /// Asks a paired PC to change its master speaker volume (`0..=100`) and/or
    /// mute state (`pc.audio.set`).
    pub async fn set_pc_audio(&self, peer: DeviceId, volume: Option<u8>, muted: Option<bool>) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::deck::set_pc_audio(&self.shared, &session, volume, muted).await
    }

    /// Asks a paired PC to make one of its `output_devices` (by `id`) its
    /// default audio output (`pc.audio.set { output }`).
    pub async fn set_pc_audio_output(&self, peer: DeviceId, output: String) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::deck::set_pc_audio_output(&self.shared, &session, output).await
    }

    // ---- Phone storage (PC file manager) ----

    /// Lists a directory on a connected phone (`storage.list`).
    pub async fn storage_list(
        &self,
        peer: DeviceId,
        path: impl Into<String>,
    ) -> Result<Vec<crate::StorageEntry>> {
        let session = self.connected(&peer)?;
        crate::storage::list(&self.shared, &session, path.into()).await
    }

    /// Convenience alias for [`Node::storage_list`] taking `&DeviceId` and `&str`.
    pub async fn list_storage(&self, peer: &DeviceId, path: &str) -> Result<Vec<crate::StorageEntry>> {
        self.storage_list(*peer, path).await
    }

    /// Reads a byte range `[offset, offset + length)` (or to end of file if
    /// `length` is `None`) from `path` on a connected phone (`storage.read`).
    pub async fn storage_read(
        &self,
        peer: DeviceId,
        path: impl Into<String>,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(crate::StorageReadMeta, Vec<u8>)> {
        let (meta, mut recv) = self.storage_read_stream(peer, path, offset, length).await?;
        let mut buf = Vec::with_capacity((meta.length.min(16 * 1024 * 1024)) as usize);
        while let Some(chunk) = recv.read_chunk(64 * 1024).await.map_err(|_| crate::Error::Offline)? {
            if (buf.len() + chunk.len()) as u64 > meta.length {
                return Err(crate::Error::Protocol("phone sent more bytes than length".into()));
            }
            buf.extend_from_slice(&chunk);
        }
        if buf.len() as u64 != meta.length {
            return Err(crate::Error::Offline);
        }
        Ok((meta, buf))
    }

    /// Convenience alias for [`Node::storage_read`] taking `&DeviceId`, `&str`,
    /// `offset`, and `length`.
    pub async fn read_storage(
        &self,
        peer: &DeviceId,
        path: &str,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>> {
        let (_meta, bytes) = self.storage_read(*peer, path, offset, Some(length)).await?;
        Ok(bytes)
    }

    /// Opens a ranged read stream from `path` on a connected phone
    /// (`storage.read`), returning the metadata and raw QUIC receive stream.
    pub async fn storage_read_stream(
        &self,
        peer: DeviceId,
        path: impl Into<String>,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(crate::StorageReadMeta, iroh::endpoint::RecvStream)> {
        let session = self.connected(&peer)?;
        crate::storage::open_read_stream(&self.shared, &session, path.into(), offset, length).await
    }

    /// Uploads a local file to `path` on a connected phone (`storage.write`).
    pub async fn storage_write(
        &self,
        peer: DeviceId,
        path: impl Into<String>,
        source: &std::path::Path,
    ) -> Result<crate::StorageWriteDone> {
        self.storage_write_with_id(peer, path, source, None, None).await
    }

    /// Uploads a local file to `path` on a connected phone (`storage.write`),
    /// reusing `id` when resuming an interrupted upload.
    pub async fn storage_write_with_id(
        &self,
        peer: DeviceId,
        path: impl Into<String>,
        source: &std::path::Path,
        id: Option<String>,
        stop_after: Option<u64>,
    ) -> Result<crate::StorageWriteDone> {
        let session = self.connected(&peer)?;
        let id = id.unwrap_or_else(crate::storage::new_upload_id);
        crate::storage::write_with_id(
            &self.shared,
            &session,
            id,
            path.into(),
            crate::FileSource::Path(source.to_path_buf()),
            None,
            stop_after,
        )
        .await
    }

    /// Creates a directory at `path` on a connected phone (`storage.mkdir`).
    pub async fn storage_mkdir(&self, peer: DeviceId, path: impl Into<String>) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::storage::mkdir(&self.shared, &session, path.into()).await
    }

    /// Renames or moves `from` to `to` on a connected phone (`storage.rename`).
    pub async fn storage_rename(
        &self,
        peer: DeviceId,
        from: impl Into<String>,
        to: impl Into<String>,
    ) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::storage::rename(&self.shared, &session, from.into(), to.into()).await
    }

    /// Deletes `path` on a connected phone (`storage.delete`).
    pub async fn storage_delete(
        &self,
        peer: DeviceId,
        path: impl Into<String>,
        confirmed: bool,
    ) -> Result<()> {
        let session = self.connected(&peer)?;
        crate::storage::delete(&self.shared, &session, path.into(), confirmed).await
    }

    /// Notifies connected PCs that have `path` open in their file manager that its
    /// contents changed (`storage.changed`, debounced to at most once per second).
    pub async fn storage_changed(&self, path: impl Into<String>) {
        crate::storage::notify_changed(&self.shared, path.into()).await;
    }

    /// Directory paths currently watched by any connected PC (most recently
    /// listed folders).
    pub fn storage_open_folders(&self) -> Vec<String> {
        self.shared.storage.lock().unwrap_or_else(|e| e.into_inner()).open_folders_all()
    }

    // ---- Local state reported by the app ----

    /// Reports this device's battery; forwarded to connected devices.
    pub async fn update_battery(&self, battery: Battery) {
        let battery = battery.sanitized();
        let changed = {
            let mut local = self.shared.local.write().unwrap_or_else(|e| e.into_inner());
            let changed = local.battery.as_ref() != Some(&battery);
            local.battery = Some(battery.clone());
            changed
        };
        if changed && let Ok(env) = Envelope::new(types::DEVICE_BATTERY, &battery) {
            self.shared.broadcast(env).await;
        }
    }

    /// Tells connected devices, and ones that connect later, that `version`
    /// of G7 Link is out on `channel` ("stable" or "nightly"), so they can
    /// check without polling themselves. Only devices that take hints get it;
    /// a hint that isn't new for its channel isn't sent again.
    pub async fn announce_update(&self, channel: String, version: String) {
        let Some(hint) = g7link_protocol::messages::UpdateHint { channel, version }.sanitized() else {
            return;
        };
        let new = lock(&self.shared.update_hints).insert(hint.channel.clone(), hint.version.clone()).as_ref()
            != Some(&hint.version);
        if !new {
            return;
        }
        let Ok(env) = Envelope::new(types::DEVICE_UPDATE, &hint) else { return };
        for session in self.shared.live_sessions() {
            if self.shared.offers(&session.peer, g7link_protocol::messages::UPDATE_HINT).unwrap_or(false)
                && let Err(e) = session.send(env.clone()).await
            {
                tracing::debug!(peer = %session.peer.short(), error = %e, "update hint not sent");
            }
        }
    }

    /// Updates this device's description (e.g. after a rename).
    pub async fn update_device_info(&self, device: DeviceInfo) {
        let device = device.sanitized();
        self.shared.local.write().unwrap_or_else(|e| e.into_inner()).device = device.clone();
        let update = HelloUpdate { device: Some(device), ..Default::default() };
        if let Ok(env) = Envelope::new(types::HELLO_UPDATE, &update) {
            self.shared.broadcast(env).await;
        }
        self.shared.refresh_all_capabilities();
    }

    /// Updates this device's Material You seed color (`accent`, ARGB) and
    /// broadcasts it to connected peers (`hello.update`).
    pub async fn update_accent(&self, accent: Option<u32>) {
        let device = {
            let mut local = self.shared.local.write().unwrap_or_else(|e| e.into_inner());
            if local.device.accent == accent {
                return;
            }
            local.device.accent = accent;
            local.device.clone()
        };
        let update = HelloUpdate { device: Some(device), ..Default::default() };
        if let Ok(env) = Envelope::new(types::HELLO_UPDATE, &update) {
            self.shared.broadcast(env).await;
        }
    }

    /// Updates this device's power level and the capabilities it offers
    /// beyond the built-in ones (they depend on permissions and add-ons).
    pub async fn update_power(&self, power: PowerLevel, extra_capabilities: Vec<String>) {
        let caps = {
            let mut local = self.shared.local.write().unwrap_or_else(|e| e.into_inner());
            local.power = power;
            local.extra_capabilities = extra_capabilities;
            local.capabilities()
        };
        let update = HelloUpdate { power: Some(power), caps: Some(caps), device: None };
        if let Ok(env) = Envelope::new(types::HELLO_UPDATE, &update) {
            self.shared.broadcast(env).await;
        }
        self.shared.refresh_all_capabilities();
        for session in self.shared.live_sessions() {
            self.shared.send_deck(&session).await;
        }
        crate::agents::refresh_catalogs(&self.shared, None).await;
    }

    // ---- Timeline ----

    /// Returns the current retention policy for the local timeline.
    pub fn timeline_retention(&self) -> Result<crate::TimelineRetention> {
        self.shared.store.timeline_retention()
    }

    /// Sets the retention policy for the local timeline and runs auto-purge immediately.
    pub fn set_timeline_retention(&self, retention: crate::TimelineRetention) -> Result<()> {
        self.shared.store.set_timeline_retention(retention, crate::now_unix())?;
        self.shared.emit(NodeEvent::TimelineChanged);
        Ok(())
    }

    /// Queries one page of the local timeline, resolving linked clipboard items
    /// in memory without duplicating clip content in SQLite.
    pub fn timeline_page(&self, query: &crate::TimelineQuery) -> Result<crate::TimelinePage> {
        let matching_clip_refs: Vec<String> = match query.search.as_deref().map(str::trim) {
            Some(q) if !q.is_empty() => {
                self.shared.clipboard_history.list(Some(q)).into_iter().map(|c| c.id).collect()
            }
            _ => Vec::new(),
        };
        let mut page = self.shared.store.query_timeline(query, &matching_clip_refs)?;
        for entry in &mut page.entries {
            self.resolve_timeline_clip(entry);
            Self::resolve_timeline_link(entry);
        }
        Ok(page)
    }

    /// Returns one timeline entry by ID, resolving its linked clip if any.
    pub fn timeline_entry(&self, id: i64) -> Result<Option<crate::TimelineEntry>> {
        let mut entry = self.shared.store.get_timeline(id)?;
        if let Some(ref mut e) = entry {
            self.resolve_timeline_clip(e);
            Self::resolve_timeline_link(e);
        }
        Ok(entry)
    }

    /// Records a custom local timeline entry (e.g. a saved mirror screenshot or
    /// mirror video recording) and emits [`NodeEvent::TimelineChanged`].
    #[allow(clippy::too_many_arguments)]
    pub fn record_timeline(
        &self,
        kind: crate::TimelineKind,
        peer: DeviceId,
        incoming: bool,
        title: String,
        detail: String,
        target: String,
        size_bytes: u64,
        duration_secs: u64,
        ref_id: Option<String>,
    ) -> Option<i64> {
        self.shared.record_timeline(crate::timeline::NewTimelineEntry {
            kind,
            device_id: peer,
            device_name: self.shared.peer_name(&peer),
            incoming,
            timestamp: crate::now_unix(),
            title,
            detail,
            target,
            size_bytes,
            duration_secs,
            ref_id,
        })
    }

    /// Updates a timeline entry matching `ref_id` (e.g. when a photo or voice
    /// recording finishes post-processing into its final destination).
    pub fn update_timeline_by_ref(
        &self,
        ref_id: &str,
        kind: crate::TimelineKind,
        title: &str,
        detail: &str,
        target: &str,
        size_bytes: u64,
    ) -> bool {
        let updated = self
            .shared
            .store
            .update_timeline_by_ref(ref_id, kind, title, detail, target, size_bytes)
            .unwrap_or(false);
        if updated {
            self.shared.emit(NodeEvent::TimelineChanged);
        }
        updated
    }

    /// Removes a timeline entry matching `ref_id` (e.g. private photo preview fetches).
    pub fn delete_timeline_by_ref(&self, ref_id: &str) -> bool {
        let deleted = self.shared.store.delete_timeline_by_ref(ref_id).unwrap_or(false);
        if deleted {
            self.shared.emit(NodeEvent::TimelineChanged);
        }
        deleted
    }

    /// Removes one entry from the local timeline by row ID.
    pub fn delete_timeline_entry(&self, id: i64) -> Result<bool> {
        let deleted = self.shared.store.delete_timeline(id)?;
        if deleted {
            self.shared.emit(NodeEvent::TimelineChanged);
        }
        Ok(deleted)
    }

    /// Clears all entries from the local timeline.
    pub fn clear_timeline(&self) -> Result<usize> {
        let removed = self.shared.store.clear_timeline()?;
        if removed > 0 {
            self.shared.emit(NodeEvent::TimelineChanged);
        }
        Ok(removed)
    }

    // ---- Data retention & privacy ----

    /// Returns current item/byte counts and retention settings across local history and caches.
    pub fn data_retention_counts(&self) -> DataRetentionCounts {
        let (clip_count, clip_bytes) = self.shared.clipboard_history.count_and_bytes();
        let timeline_items = self.shared.store.timeline_total_count().unwrap_or(0);
        let timeline_retention_days = self
            .shared
            .store
            .timeline_retention()
            .map(|r| r.max_days)
            .unwrap_or(crate::DEFAULT_TIMELINE_MAX_DAYS);
        let (chat_threads, chat_messages) = self.shared.store.chat_cache_counts().unwrap_or((0, 0));
        let received_file_records =
            self.shared.store.timeline_count_by_kind(crate::TimelineKind::File).unwrap_or(0);
        DataRetentionCounts {
            clipboard_items: u32::try_from(clip_count).unwrap_or(u32::MAX),
            clipboard_bytes: clip_bytes,
            timeline_items,
            timeline_retention_days,
            chat_threads,
            chat_messages,
            received_file_records,
        }
    }

    /// Clears all locally cached `MessagingStyle` chat threads and messages.
    pub fn clear_message_cache(&self) -> Result<()> {
        let removed = self.shared.store.clear_chat_threads()?;
        if removed > 0
            && let Ok(peers) = self.shared.store.list_peers()
        {
            for peer in peers {
                self.shared.emit(NodeEvent::SmsChanged { device: peer.id, thread: None });
            }
        }
        Ok(())
    }

    /// Clears all file transfer records (`TimelineKind::File`) from the local timeline
    /// without deleting downloaded files on disk.
    pub fn clear_received_file_history(&self) -> Result<()> {
        let removed = self.shared.store.delete_timeline_kind(crate::TimelineKind::File)?;
        if removed > 0 {
            self.shared.emit(NodeEvent::TimelineChanged);
        }
        Ok(())
    }

    /// Wipes all local history and caches (encrypted clipboard history + image files,
    /// timeline entries, and `MessagingStyle` conversation cache) while preserving
    /// device identity, paired devices, per-device feature toggles, and the outgoing send queue.
    pub fn clear_all_local_data(&self) -> Result<()> {
        self.shared.clipboard_history.clear()?;
        *self.shared.last_clip_suggestion.lock().unwrap_or_else(|e| e.into_inner()) = None;
        lock(&self.shared.notification_history).clear();
        self.shared.notifications.clear_all();
        self.shared.store.clear_all_history_and_cache()?;
        // Files waiting to send stay: they're only references, and the user
        // chose to send them. What's copied or linked goes with everything else.
        self.shared.store.remove_queue_items_of_kinds(&[
            crate::QueueKind::Clipboard,
            crate::QueueKind::ClipboardImage,
            crate::QueueKind::Link,
            crate::QueueKind::TaskNotify,
        ])?;
        self.shared.emit(NodeEvent::QueueChanged);
        self.shared.emit(NodeEvent::ClipboardHistoryChanged);
        self.shared.emit(NodeEvent::TimelineChanged);
        if let Ok(peers) = self.shared.store.list_peers() {
            for peer in peers {
                self.shared.emit(NodeEvent::SmsChanged { device: peer.id, thread: None });
            }
        }
        Ok(())
    }

    fn resolve_timeline_clip(&self, entry: &mut crate::TimelineEntry) {
        if entry.kind != crate::TimelineKind::Clip {
            return;
        }
        if let Some(ref_id) = entry.ref_id.as_deref()
            && let Some(clip) = self.shared.clipboard_history.entry(ref_id)
        {
            entry.clip_available = true;
            match clip.kind {
                crate::ClipboardItemKind::Text => {
                    entry.title = clip.text;
                    entry.detail = "text".into();
                }
                crate::ClipboardItemKind::Image => {
                    entry.title = "Image clip".into();
                    entry.detail = clip.mime.unwrap_or_else(|| "image/png".into());
                    if self.shared.clipboard_history.has_image(ref_id) {
                        entry.image_data_url = Some(String::new());
                    }
                }
            }
        } else {
            entry.clip_available = false;
            if entry.title.is_empty() {
                entry.title =
                    if entry.detail.contains("image") { "Image clip".into() } else { "Text clip".into() };
            }
        }
    }

    fn resolve_timeline_link(entry: &mut crate::TimelineEntry) {
        if entry.kind != crate::TimelineKind::Link {
            return;
        }
        let raw_url = if !entry.target.is_empty() {
            entry.target.clone()
        } else if !entry.title.is_empty() {
            entry.title.clone()
        } else {
            return;
        };
        let lower = raw_url.get(..8).map(str::to_ascii_lowercase).unwrap_or_default();
        let is_uri =
            lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("geo:");
        if !is_uri {
            return;
        }
        let title_lower = entry.title.get(..8).map(str::to_ascii_lowercase).unwrap_or_default();
        let title_is_raw_uri = entry.title.is_empty()
            || entry.title == raw_url
            || title_lower.starts_with("http://")
            || title_lower.starts_with("https://")
            || title_lower.starts_with("geo:");
        if title_is_raw_uri {
            let (friendly_title, detail) = crate::handoff::timeline_link_title_and_detail(&raw_url, None);
            entry.title = friendly_title;
            entry.detail = detail;
        } else if entry.detail != raw_url {
            entry.detail = raw_url;
        }
    }

    // ---- Connection Doctor diagnostics ----

    /// Returns LAN mDNS sighting and consecutive dial failure diagnostics for `peer`.
    pub fn peer_diagnostics(&self, peer: &DeviceId) -> PeerDiagnostics {
        let (mdns_seen_ago_secs, mdns_addrs) = match lock(&self.shared.lan_seen).get(peer) {
            Some((seen_at, addrs)) => (Some(seen_at.elapsed().as_secs()), addrs.clone()),
            None => (None, Vec::new()),
        };
        let dial_failures = lock(&self.shared.dial_failures).get(peer).copied().unwrap_or(0);
        PeerDiagnostics { mdns_seen_ago_secs, mdns_addrs, dial_failures }
    }

    /// Records a LAN discovery sighting for `peer` (e.g. from mDNS or LocalSend).
    pub fn record_lan_sighting(&self, peer: DeviceId, addrs: Vec<SocketAddr>) {
        self.shared.remember_lan_sighting(peer, addrs);
    }

    /// Increments the consecutive dial failure counter for `peer`.
    pub fn record_dial_failure(&self, peer: DeviceId) {
        *lock(&self.shared.dial_failures).entry(peer).or_insert(0) += 1;
    }
}

/// Per-peer discovery and connection diagnostics for the Connection Doctor.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerDiagnostics {
    /// Seconds since this peer was last seen announcing itself on LAN mDNS, if seen in this session.
    pub mdns_seen_ago_secs: Option<u64>,
    /// Last LAN IP/port addresses announced by this peer over mDNS.
    pub mdns_addrs: Vec<SocketAddr>,
    /// Consecutive direct/QUIC dial failures since the peer last connected.
    pub dial_failures: u32,
}

/// Summary of locally stored history and cache counts for the Privacy & Data settings UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DataRetentionCounts {
    pub clipboard_items: u32,
    pub clipboard_bytes: u64,
    pub timeline_items: u32,
    pub timeline_retention_days: u32,
    pub chat_threads: u32,
    pub chat_messages: u32,
    pub received_file_records: u32,
}

const PORT_FILE: &str = "port";
/// Retrying the saved ports: 5 × 200 ms.
const PORT_RETRIES: usize = 5;
const PORT_RETRY_DELAY: Duration = Duration::from_millis(200);

/// The UDP ports the endpoint listens on, remembered across restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ports {
    v4: u16,
    /// None where the machine has no IPv6.
    v6: Option<u16>,
}

impl Ports {
    /// Reads "<v4> [<v6>]".
    fn load(dir: &std::path::Path) -> Option<Ports> {
        let text = std::fs::read_to_string(dir.join(PORT_FILE)).ok()?;
        let mut parts = text.split_whitespace().map(str::parse::<u16>);
        let v4 = parts.next()?.ok().filter(|p| *p != 0)?;
        let v6 = parts.next().and_then(Result::ok).filter(|p| *p != 0);
        Some(Ports { v4, v6 })
    }

    fn of(bound: &[SocketAddr]) -> Option<Ports> {
        let v4 = bound.iter().find(|a| a.is_ipv4() && a.ip().is_unspecified())?.port();
        let v6 = bound.iter().find(|a| a.is_ipv6() && a.ip().is_unspecified()).map(SocketAddr::port);
        Some(Ports { v4, v6 })
    }

    fn save(self, dir: &std::path::Path) {
        let text = match self.v6 {
            Some(v6) => format!("{} {v6}", self.v4),
            None => self.v4.to_string(),
        };
        if let Err(e) = std::fs::write(dir.join(PORT_FILE), text) {
            tracing::warn!(error = %e, "can't remember the ports");
        }
    }
}

/// The ports to try in turn when no port is required, and how many times
/// to retry each: the preferred port (a restart may still hold it a moment),
/// then without IPv6 (with Away mode on; off, only IPv4 is bound anyway), then
/// the saved ports; each bind once. Without a preferred port, the saved ports
/// get the retries.
fn port_order(preferred: Option<u16>, saved: Option<Ports>, away_mode: bool) -> Vec<(Ports, usize)> {
    // With Away mode off only the IPv4 port is bound: that's what tells two apart.
    let key = |p: Ports| if away_mode { p } else { Ports { v4: p.v4, v6: None } };
    let mut order: Vec<(Ports, usize)> = Vec::new();
    let mut push = |ports: Ports, retries: usize| {
        if !order.iter().any(|(p, _)| key(*p) == key(ports)) {
            order.push((ports, retries));
        }
    };
    match preferred.filter(|p| *p != 0) {
        Some(p) => {
            push(Ports { v4: p, v6: Some(p) }, PORT_RETRIES);
            if away_mode {
                push(Ports { v4: p, v6: None }, 0);
            }
            if let Some(saved) = saved {
                push(saved, 0);
            }
        }
        None => {
            if let Some(saved) = saved {
                push(saved, PORT_RETRIES);
            }
        }
    }
    order
}

/// [`bind_endpoint`] on `ports`, tried again `retries` times a moment apart
/// while they're taken.
async fn bind_retrying(
    config: &NodeConfig,
    secret: &iroh::SecretKey,
    ports: Ports,
    retries: usize,
    relay_map: &RelayMap,
    proxy: Option<&url::Url>,
) -> Result<Endpoint> {
    let mut bound = bind_endpoint(config, secret.clone(), Some(ports), relay_map.clone(), proxy).await;
    for _ in 0..retries {
        if bound.is_ok() {
            break;
        }
        tokio::time::sleep(PORT_RETRY_DELAY).await;
        bound = bind_endpoint(config, secret.clone(), Some(ports), relay_map.clone(), proxy).await;
    }
    bound
}

/// Binds the endpoint on `ports`, or any free ports.
async fn bind_endpoint(
    config: &NodeConfig,
    secret: iroh::SecretKey,
    ports: Option<Ports>,
    relay_map: RelayMap,
    proxy: Option<&url::Url>,
) -> Result<Endpoint> {
    let tuning = config.transport;
    let controller: std::sync::Arc<dyn iroh::endpoint::ControllerFactory + Send + Sync> = match tuning
        .congestion
    {
        crate::config::Congestion::Cubic => {
            std::sync::Arc::new(noq_proto::congestion::CubicConfig::default())
        }
        crate::config::Congestion::Bbr3 => std::sync::Arc::new(noq_proto::congestion::Bbr3Config::default()),
        crate::config::Congestion::NewReno => {
            std::sync::Arc::new(noq_proto::congestion::NewRenoConfig::default())
        }
    };
    tracing::info!(
        congestion = ?tuning.congestion,
        stream_window = tuning.stream_window,
        connection_window = tuning.connection_window,
        "transport"
    );
    let transport = QuicTransportConfig::builder()
        // Lanes, RPCs and media streams each take a stream; the session's
        // own limits (`rpc.rs`, `session.rs`) answer BUSY well before this.
        .max_concurrent_bidi_streams(VarInt::from_u32(MAX_BIDI_STREAMS))
        // Sound and motion that can't go out promptly are stale: a congested
        // link drops them instead of queueing seconds of them.
        .datagram_send_buffer_size(DATAGRAM_SEND_BUFFER)
        .keep_alive_interval(KEEP_ALIVE_INTERVAL)
        .max_idle_timeout(Some(IDLE_TIMEOUT.try_into().expect("the idle timeout fits a QUIC varint")))
        .stream_receive_window(VarInt::from_u32(tuning.stream_window))
        .receive_window(VarInt::from_u32(tuning.connection_window))
        .send_window(u64::from(tuning.connection_window))
        .congestion_controller_factory(controller)
        .build();
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(secret)
        .alpns(vec![ALPN_SESSION.to_vec(), ALPN_SESSION_V0.to_vec(), ALPN_PAIR.to_vec()])
        // Keep a relay transport even in LAN mode, so it can be enabled later.
        .relay_mode(RelayMode::Custom(relay_map))
        .transport_config(transport);
    if let Some(proxy) = proxy {
        builder = builder.proxy_url(proxy.clone());
    }
    if !config.away_mode || ports.is_some() {
        builder = builder.clear_ip_transports();
    }
    #[cfg(target_os = "linux")]
    if let Some(name) = config.network_interface.as_deref().filter(|name| !name.is_empty()) {
        builder = crate::direct_network::bind_lan_interface(builder, name, config.away_mode)?;
    }
    if !config.away_mode {
        let v4_port = ports.map_or(0, |p| p.v4);
        builder = builder
            .portmapper_config(PortmapperConfig::Disabled)
            .bind_addr(SocketAddr::from(([0, 0, 0, 0], v4_port)))
            .map_err(crate::error::net)?;
    } else if let Some(ports) = ports {
        builder = builder.bind_addr(SocketAddr::from(([0, 0, 0, 0], ports.v4))).map_err(crate::error::net)?;
        if let Some(v6) = ports.v6 {
            builder = builder.bind_addr(SocketAddr::from(([0u16; 8], v6))).map_err(crate::error::net)?;
        }
    }
    let endpoint = builder.bind().await.map_err(crate::error::net)?;
    if config.away_mode {
        add_internet_lookups(&endpoint)?;
    }
    Ok(endpoint)
}

/// Reconciles the live relay map. The caller holds `away_switch`.
async fn apply_relays(shared: &Shared, away: bool) -> bool {
    let desired = lock(&shared.relay_settings).enabled_map(away);
    let mut changed = false;
    for config in desired.relays::<Vec<_>>() {
        if !shared.relay_map.contains(&config.url) {
            shared.endpoint.insert_relay(config.url.clone(), config).await;
            changed = true;
        }
    }
    for url in shared.relay_map.urls::<Vec<_>>() {
        if !desired.contains(&url) {
            shared.endpoint.remove_relay(&url).await;
            changed = true;
        }
    }
    changed
}

/// Brings relays and internet lookups in line with Away mode, while running.
/// Off, nothing is published or looked up and no relay is picked or used. iroh
/// keeps its idle connection to the current home relay until the endpoint
/// closes (`handle_net_report_report` holds on to it), which needs a restart.
async fn apply_internet_services(shared: &Arc<Shared>) {
    let _one_at_a_time = shared.away_switch.lock().await;
    let on = shared.away_mode();
    apply_relays(shared, on).await;
    let running = lock(&shared.lan_discovery).as_ref().map(|(_, fast)| *fast);
    let lookups = match running {
        // Swapping mDNS clears the lookups and puts back what Away mode allows.
        Some(fast) => install_mdns(shared, fast),
        None => shared.endpoint.address_lookup().map_err(crate::error::net).and_then(|lookups| {
            lookups.clear();
            lookups.add(shared.memory.clone());
            if on { add_internet_lookups(&shared.endpoint) } else { Ok(()) }
        }),
    };
    if let Err(e) = lookups {
        tracing::warn!(error = %e, "can't update address lookups");
    }
    shared.endpoint.network_change().await;
    tracing::info!(on, "internet services updated");
    if on {
        shared.nudge_reconnect();
    }
}

/// Publishes this device's addresses without the name that LAN discovery
/// announces: on the LAN it's for nearby pairing, and nothing on the internet
/// needs it.
#[derive(Debug)]
struct WithoutName<T>(T);

impl<T: iroh::address_lookup::AddressLookup> iroh::address_lookup::AddressLookup for WithoutName<T> {
    fn publish(&self, data: &iroh::address_lookup::EndpointData) {
        self.0.publish(&iroh::address_lookup::EndpointData::new(data.addrs().cloned().collect()));
    }
}

/// mDNS, telling the LAN check what it finds while a device is being looked
/// up. The library tells its subscribers only about devices nobody was
/// looking up, so a paired device found while it was being dialed (right
/// after a restart) never counted as on this network, and a home network
/// numbered outside the private ranges refused it (#131).
struct SeenOnLan<T> {
    inner: T,
    seen: Arc<dyn Fn(&iroh::address_lookup::EndpointInfo) + Send + Sync>,
}

impl<T: std::fmt::Debug> std::fmt::Debug for SeenOnLan<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SeenOnLan").field(&self.inner).finish()
    }
}

impl<T: iroh::address_lookup::AddressLookup> iroh::address_lookup::AddressLookup for SeenOnLan<T> {
    fn publish(&self, data: &iroh::address_lookup::EndpointData) {
        self.inner.publish(data);
    }

    fn resolve(
        &self,
        endpoint_id: iroh::EndpointId,
    ) -> Option<
        n0_future::boxed::BoxStream<
            std::result::Result<iroh::address_lookup::Item, iroh::address_lookup::Error>,
        >,
    > {
        use n0_future::StreamExt;
        let seen = Arc::clone(&self.seen);
        let found = self.inner.resolve(endpoint_id)?;
        Some(Box::pin(found.inspect(move |item| {
            if let Ok(item) = item {
                seen(item.endpoint_info());
            }
        })))
    }
}

/// Notes that `info` was seen over mDNS here, so its addresses count as this network's.
fn record_mdns_sighting(shared: &Shared, info: &iroh::address_lookup::EndpointInfo) -> DeviceId {
    let id = crate::device_id(&info.endpoint_id);
    shared.remember_lan_sighting(id, info.ip_addrs().copied().collect());
    id
}

/// Announces this device on the LAN and watches for others.
fn start_lan_discovery(shared: &Arc<Shared>) -> Result<()> {
    install_mdns(shared, false)
}

/// What this device's mDNS record says about it besides its ID and
/// addresses: its name while someone may be pairing with it, so their nearby
/// list can show it, and nothing otherwise. Paired devices find each other by
/// ID and never need it.
fn announced_name(name: &str, pairing: bool) -> Option<UserData> {
    if !pairing {
        return None;
    }
    let mut announced = name.to_owned();
    while announced.len() > UserData::MAX_LENGTH {
        announced.pop();
    }
    UserData::try_from(announced).ok()
}

/// Whether someone may be pairing with this device: its pairing screen or
/// nearby list is open, or it shows a pairing code.
fn open_for_pairing(shared: &Shared) -> bool {
    shared.pairing.screen_open() || shared.pairing.is_hosting()
}

/// Sets the user data mDNS publishes to match [`open_for_pairing`].
fn set_name_on_lan(shared: &Shared, on: bool) {
    let name = shared.local.read().unwrap_or_else(|e| e.into_inner()).device.name.clone();
    shared.endpoint.set_user_data_for_address_lookup(announced_name(&name, on));
    shared.name_on_lan.store(on, std::sync::atomic::Ordering::SeqCst);
}

/// Puts this device's name in its mDNS record, or takes it out, when
/// [`open_for_pairing`] changed. Call it within the runtime.
pub(crate) fn sync_name_on_lan(shared: &Arc<Shared>) {
    if lock(&shared.lan_discovery).is_none() {
        return;
    }
    let wanted = open_for_pairing(shared);
    if wanted != shared.name_on_lan.load(std::sync::atomic::Ordering::SeqCst) {
        set_name_on_lan(shared, wanted);
    }
}

/// Starts mDNS, asking the LAN who's there every [`MDNS_FAST`] or
/// [`MDNS_SLOW`], in place of the one running. iroh can't drop a single
/// address lookup, so a swap clears them all and puts the others back.
fn install_mdns(shared: &Arc<Shared>, fast: bool) -> Result<()> {
    // Before the new mDNS is added, so it starts with what it should say.
    set_name_on_lan(shared, open_for_pairing(shared));
    let builder = MdnsAddressLookup::builder().service_name("g7link").discovery_cadence(if fast {
        MDNS_FAST
    } else {
        MDNS_SLOW
    });
    // Every LAN interface, not just the one the OS picks for multicast, which
    // on a PC is often a container bridge, virtual machine or VPN adapter (#148). Not on phones:
    // mDNS on mobile data would only wake the radio.
    #[cfg(not(target_os = "android"))]
    let builder = builder.multicast_interfaces(is_mdns_interface);
    let mdns = builder.build(shared.endpoint.id()).map_err(crate::error::net)?;
    let lookups = shared.endpoint.address_lookup().map_err(crate::error::net)?;
    let cancel = shared.cancel.child_token();
    let previous = lock(&shared.lan_discovery).replace((cancel.clone(), fast));
    if let Some((old, _)) = previous {
        old.cancel();
        lookups.clear();
        lookups.add(shared.memory.clone());
        if shared.away_mode() {
            add_internet_lookups(&shared.endpoint)?;
        }
        // A nearby list just opened: start it fresh rather than keep what the
        // old mDNS saw, which will never expire now.
        if fast {
            let gone: Vec<DeviceId> = lock(&shared.discovered).drain().map(|(id, _)| id).collect();
            for id in gone {
                shared.emit(NodeEvent::DiscoveryExpired(id));
            }
        }
    }
    let recorder = Arc::downgrade(shared);
    lookups.add(SeenOnLan {
        inner: mdns.clone(),
        seen: Arc::new(move |info| {
            if let Some(shared) = recorder.upgrade() {
                record_mdns_sighting(&shared, info);
            }
        }),
    });

    let weak = Arc::downgrade(shared);
    tokio::spawn(async move {
        use n0_future::StreamExt;
        let mut events = mdns.subscribe().await;
        loop {
            let event = tokio::select! {
                _ = cancel.cancelled() => return,
                event = events.next() => match event { Some(e) => e, None => return },
            };
            let Some(shared) = weak.upgrade() else { return };
            match event {
                DiscoveryEvent::Discovered { endpoint_info, .. } => {
                    let id = record_mdns_sighting(&shared, &endpoint_info);
                    if shared.store.is_paired(&id).unwrap_or(false) {
                        shared.wake_supervisor(&id);
                        continue;
                    }
                    let device =
                        DiscoveredDevice { id, name: endpoint_info.user_data().map(|d| d.to_string()) };
                    let is_new =
                        lock(&shared.discovered).insert(id, device.clone()).as_ref() != Some(&device);
                    if is_new {
                        shared.emit(NodeEvent::Discovered(device));
                    }
                }
                DiscoveryEvent::Expired { endpoint_id } => {
                    let id = crate::device_id(&endpoint_id);
                    // An unpaired device's sighting goes when it leaves the LAN; a paired
                    // one's stays, for its Connection Doctor.
                    if !shared.store.is_paired(&id).unwrap_or(false) {
                        lock(&shared.lan_seen).remove(&id);
                    }
                    if shared.usb_peer_addr(&id).is_none() && lock(&shared.discovered).remove(&id).is_some() {
                        shared.emit(NodeEvent::DiscoveryExpired(id));
                    }
                }
                _ => {}
            }
        }
    });
    Ok(())
}

/// Whether mDNS should run on the interface with this address: anything but
/// loopback, link-local (no DHCP lease) and Tailscale, which carries no
/// multicast. Public-numbered LANs stay in (#131).
#[cfg(not(target_os = "android"))]
fn is_mdns_interface(ip: std::net::Ipv4Addr) -> bool {
    !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_unspecified()
        && !crate::session::is_tailnet_ip(std::net::IpAddr::V4(ip))
        && carries_multicast(ip)
}

/// On Linux, also leave out interfaces without the MULTICAST flag, such as
/// WireGuard and other point-to-point tunnels: every mDNS send on them fails
/// (WireGuard answers ENOKEY), and swarm-discovery logs an error for each one
/// on every round.
#[cfg(target_os = "linux")]
fn carries_multicast(ip: std::net::Ipv4Addr) -> bool {
    interface_carries_multicast(ip, &netdev::get_interfaces())
}

#[cfg(all(not(target_os = "linux"), not(target_os = "android")))]
fn carries_multicast(_ip: std::net::Ipv4Addr) -> bool {
    true
}

/// An address no interface reports (it may just have gone away) stays in, as
/// before this check.
#[cfg(target_os = "linux")]
fn interface_carries_multicast(ip: std::net::Ipv4Addr, interfaces: &[netdev::Interface]) -> bool {
    interfaces
        .iter()
        .find(|nic| nic.ipv4.iter().any(|net| net.addr() == ip))
        .is_none_or(netdev::Interface::is_multicast)
}

#[cfg(all(test, target_os = "linux"))]
mod mdns_interface_tests {
    use super::interface_carries_multicast;
    use std::net::Ipv4Addr;

    fn nic(addr: Ipv4Addr, flags: libc::c_int) -> netdev::Interface {
        let mut nic = netdev::Interface::dummy();
        nic.ipv4 = vec![netdev::ipnet::Ipv4Net::new(addr, 24).unwrap()];
        nic.flags = flags as u32;
        nic
    }

    #[test]
    fn point_to_point_tunnels_without_multicast_are_left_out() {
        let lan = Ipv4Addr::new(192, 168, 0, 222);
        let wireguard = Ipv4Addr::new(10, 99, 99, 132);
        let interfaces = [
            nic(lan, libc::IFF_UP | libc::IFF_BROADCAST | libc::IFF_MULTICAST | libc::IFF_RUNNING),
            nic(wireguard, libc::IFF_UP | libc::IFF_POINTOPOINT | libc::IFF_NOARP | libc::IFF_RUNNING),
        ];
        assert!(interface_carries_multicast(lan, &interfaces));
        assert!(!interface_carries_multicast(wireguard, &interfaces));
    }

    #[test]
    fn unknown_addresses_stay_in() {
        assert!(interface_carries_multicast(Ipv4Addr::new(10, 0, 0, 5), &[]));
    }
}

/// The internet lookups away mode uses, the same ones `bind_endpoint` sets
/// up, for putting back after `install_mdns` clears them.
fn add_internet_lookups(endpoint: &Endpoint) -> Result<()> {
    use iroh::address_lookup::AddressLookupBuilder;
    let lookups = endpoint.address_lookup().map_err(crate::error::net)?;
    lookups
        .add(WithoutName(PkarrPublisher::n0_dns().into_address_lookup(endpoint).map_err(crate::error::net)?));
    lookups.add(PkarrResolver::n0_dns().into_address_lookup(endpoint).map_err(crate::error::net)?);
    lookups.add(DnsAddressLookup::n0_dns().into_address_lookup(endpoint).map_err(crate::error::net)?);
    Ok(())
}

struct ActivePeerQueueGuard {
    weak: Weak<Shared>,
    peer: DeviceId,
}

impl Drop for ActivePeerQueueGuard {
    fn drop(&mut self) {
        if let Some(shared) = self.weak.upgrade() {
            lock(&shared.queue_active_peers).remove(&self.peer);
            if shared.session(&self.peer).is_some()
                && shared.store.has_active_queue_for_peer(&self.peer).unwrap_or(false)
            {
                shared.queue_notify.notify_waiters();
            }
        }
    }
}

fn run_queue_coordinator(weak: Weak<Shared>, cancel: CancellationToken) {
    tokio::spawn(async move {
        loop {
            if cancel.is_cancelled() {
                return;
            }
            let Some(shared) = weak.upgrade() else { return };
            shared.queue_coordinator_ticks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let notify = shared.queue_notify.clone();
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            let now_ms = crate::now_unix_ms();
            if shared.store.expire_stale_queue_items(now_ms).unwrap_or(0) > 0 {
                shared.emit(NodeEvent::QueueChanged);
            }
            if let Ok(peers) = shared.store.peers_with_waiting_queue() {
                for peer in peers {
                    if shared.session(&peer).is_some() {
                        let should_spawn = lock(&shared.queue_active_peers).insert(peer);
                        if should_spawn {
                            let peer_weak = weak.clone();
                            let peer_cancel = cancel.child_token();
                            tokio::spawn(async move {
                                run_peer_queue(peer_weak, peer, peer_cancel).await;
                            });
                        }
                    }
                }
            }
            let online_peers: Vec<DeviceId> = shared.live_sessions().into_iter().map(|s| s.peer).collect();
            let sleep_dur = match shared.store.next_queue_wake_ms(&online_peers).ok().flatten() {
                Some(wake_ms) if wake_ms <= now_ms => Some(Duration::from_millis(50)),
                Some(wake_ms) => Some(Duration::from_millis((wake_ms - now_ms) as u64)),
                None => None,
            };
            drop(shared);
            let sleep_fut = async move {
                match sleep_dur {
                    Some(d) => tokio::time::sleep(d).await,
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = &mut notified => {}
                _ = sleep_fut => {}
            }
        }
    });
}

async fn run_peer_queue(weak: Weak<Shared>, peer: DeviceId, cancel: CancellationToken) {
    let _guard = ActivePeerQueueGuard { weak: weak.clone(), peer };
    loop {
        if cancel.is_cancelled() {
            return;
        }
        let Some(shared) = weak.upgrade() else { return };
        let notify = shared.queue_notify.clone();
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        let now_ms = crate::now_unix_ms();
        if shared.store.expire_stale_queue_items(now_ms).unwrap_or(0) > 0 {
            shared.emit(NodeEvent::QueueChanged);
        }

        let Some(session) = shared.session(&peer) else {
            let _ = shared.store.clear_send_queue_retry_delays(&peer);
            return;
        };

        let head = match shared.store.head_waiting_item_for_peer(&peer) {
            Ok(Some(h)) => h,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(peer = %peer.short(), error = %e, "failed to query send queue head");
                return;
            }
        };

        let (item, payload, blob, next_retry_ms) = head;
        if next_retry_ms > now_ms {
            let wait_ms = (next_retry_ms - now_ms).min(15_000) as u64;
            let session_cancel = session.cancel.clone();
            drop(session);
            drop(shared);
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = session_cancel.cancelled() => continue,
                _ = &mut notified => continue,
                _ = tokio::time::sleep(Duration::from_millis(wait_ms)) => continue,
            }
        }

        match payload {
            crate::QueuePayload::Files { .. }
            | crate::QueuePayload::Folder { .. }
            | crate::QueuePayload::HandoffDocument { .. }
            | crate::QueuePayload::Recording { .. } => {
                drop(session);
                transfer::deliver_queued_files(&shared, &item, &payload).await;
            }
            _ => {
                deliver_queued_rpc(&shared, &session, &item, &payload, blob).await;
            }
        }
    }
}

async fn deliver_queued_rpc(
    shared: &Arc<Shared>,
    session: &Arc<Session>,
    item: &crate::QueueItem,
    payload: &crate::QueuePayload,
    blob: Option<Vec<u8>>,
) {
    let peer = item.peer;
    // Gone or no longer waiting (cleared, say): it must not go out now.
    if !matches!(shared.store.mark_queue_sending(&item.id), Ok(true)) {
        return;
    }
    shared.emit(NodeEvent::QueueChanged);

    let outcome: Result<()> = match payload {
        crate::QueuePayload::Clipboard { text, html, rtf } => {
            if !shared.toggle_on(&peer, crate::clipboard::TOGGLE) {
                Err(Error::Denied)
            } else {
                match Envelope::new(
                    types::CLIP_SET,
                    &ClipSet { text: text.clone(), html: html.clone(), rtf: rtf.clone(), sensitive: false },
                ) {
                    Ok(env) => match session.request(env, REQUEST_TIMEOUT).await {
                        Ok(reply) => match reply.expect(types::OK) {
                            Ok(()) => {
                                let peer_name = shared.peer_name(&peer);
                                // Cleared while it was on its way: don't bring it back.
                                let recorded = if shared.store.has_queue_item(&item.id) {
                                    shared.clipboard_history.record_text_with_id(text, &peer_name, false)
                                } else {
                                    None
                                };
                                if let Some((clip_id, evicted)) = recorded {
                                    shared.record_timeline_clip(
                                        peer, &peer_name, false, "text", clip_id, evicted,
                                    );
                                    shared.emit(NodeEvent::ClipboardHistoryChanged);
                                }
                                Ok(())
                            }
                            Err(e) => Err(e.into()),
                        },
                        Err(e) => Err(e),
                    },
                    Err(e) => Err(e.into()),
                }
            }
        }
        crate::QueuePayload::ClipboardImage { mime, .. } => match blob {
            Some(bytes) if !bytes.is_empty() => {
                crate::clipboard::send_image(shared, session, mime.clone(), bytes).await
            }
            _ => Err(Error::Internal("Queued clipboard image payload is missing".into())),
        },
        crate::QueuePayload::Link { url, title, user_shared } => {
            crate::actions::open_link_on_session(shared, session, url.clone(), title.clone(), *user_shared)
                .await
        }
        crate::QueuePayload::TaskNotify { task } => {
            crate::actions::task_notify_on_session(shared, session, task).await
        }
        crate::QueuePayload::CameraRequest { request_id, mode } => {
            crate::continuity_camera::request_on_session(shared, session, request_id.clone(), *mode).await
        }
        crate::QueuePayload::Files { .. }
        | crate::QueuePayload::Folder { .. }
        | crate::QueuePayload::HandoffDocument { .. }
        | crate::QueuePayload::Recording { .. } => {
            // File payloads go through the transfer path; reaching here is a bug, and the
            // item must fail rather than be reported as delivered.
            Err(Error::Internal("file payload routed to the message path".into()))
        }
    };

    match outcome {
        Ok(()) => {
            let _ = shared.store.complete_queue_item(&item.id);
            transfer::cleanup_staging_id(shared, &item.id);
            shared.emit(NodeEvent::QueueChanged);
        }
        Err(e) if !crate::actions::is_transient_send_error(&e) => {
            let _ = shared.store.mark_queue_failed(&item.id, &e.to_string());
            shared.emit(NodeEvent::QueueChanged);
        }
        Err(e) => {
            let peer_disconnected = !session.is_alive() || shared.session(&peer).is_none();
            if peer_disconnected {
                let _ = shared.store.mark_queue_retry_or_wait(&item.id, 0, 0, Some(&e.to_string()));
                let _ = shared.store.clear_send_queue_retry_delays(&peer);
                shared.emit(NodeEvent::QueueChanged);
            } else {
                const RETRY_BACKOFF_MS: [i64; 5] = [1_000, 2_000, 4_000, 8_000, 15_000];
                let attempts = item.attempts.saturating_add(1);
                if (attempts as usize) > RETRY_BACKOFF_MS.len() {
                    let _ = shared.store.mark_queue_failed(&item.id, &e.to_string());
                    shared.emit(NodeEvent::QueueChanged);
                } else {
                    let delay_ms = RETRY_BACKOFF_MS[(attempts as usize) - 1];
                    let next_retry = crate::now_unix_ms().saturating_add(delay_ms);
                    let _ = shared.store.mark_queue_retry_or_wait(
                        &item.id,
                        attempts,
                        next_retry,
                        Some(&e.to_string()),
                    );
                    shared.emit(NodeEvent::QueueChanged);
                }
            }
        }
    }
}

#[cfg(test)]
mod away_tests {
    use super::*;
    use crate::config::NoopPlatform;
    use g7link_protocol::messages::{DeviceInfo, DeviceKind};

    async fn node(dir: &tempfile::TempDir, away: bool) -> Node {
        let mut config = NodeConfig::new(
            dir.path().to_path_buf(),
            DeviceInfo {
                name: "Test".into(),
                kind: DeviceKind::Desktop,
                os: "linux".into(),
                os_ver: "6.12".into(),
                model: None,
                accent: None,
                screen: None,
                install_id: None,
            },
            "0.0.0-test",
        );
        config.lan_discovery = false;
        config.away_mode = away;
        config.localsend_port = 0;
        Node::start(config, Arc::new(NoopPlatform)).await.unwrap()
    }

    fn lookups(node: &Node) -> usize {
        node.shared.endpoint.address_lookup().unwrap().len()
    }

    fn relays(node: &Node) -> usize {
        node.shared.endpoint.home_relay_status().get().len()
    }

    /// Waits for the switch, which runs in the background.
    async fn settled(node: &Node) {
        for _ in 0..200 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if node.shared.away_switch.try_lock().is_ok() {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if node.shared.away_switch.try_lock().is_ok() {
                    return;
                }
            }
        }
        panic!("the switch never finished");
    }

    #[tokio::test]
    async fn turning_away_off_stops_internet_services_without_a_restart() {
        let dir = tempfile::TempDir::new().unwrap();
        let node = node(&dir, true).await;
        let with_away = lookups(&node);
        assert!(with_away > 1, "started with internet lookups");

        node.set_away_mode(false);
        settled(&node).await;
        assert_eq!(lookups(&node), 1, "only the in-memory lookup is left");
        node.shutdown().await;
    }

    #[tokio::test]
    async fn turning_away_on_starts_internet_services_without_a_restart() {
        let dir = tempfile::TempDir::new().unwrap();
        let node = node(&dir, false).await;
        assert_eq!(lookups(&node), 1, "LAN only: nothing but the in-memory lookup");
        assert_eq!(relays(&node), 0);

        node.set_away_mode(true);
        settled(&node).await;
        assert!(lookups(&node) > 1, "internet lookups are on");

        node.set_away_mode(false);
        settled(&node).await;
        assert_eq!(lookups(&node), 1);
        node.shutdown().await;
    }

    /// Needs the internet: `cargo test -p g7link-core --lib -- --ignored away_switch`.
    #[tokio::test]
    #[ignore = "reaches the real relays"]
    async fn away_switch_reaches_a_real_relay_while_running() {
        let dir = tempfile::TempDir::new().unwrap();
        let node = node(&dir, false).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!(relays(&node), 0, "LAN only reaches no relay");

        node.set_away_mode(true);
        let mut connected = false;
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            connected = relays(&node) > 0;
            if connected {
                break;
            }
        }
        assert!(connected, "turning it on while running reaches a relay");
        node.shutdown().await;
    }

    #[tokio::test]
    async fn a_new_attempt_at_a_transfer_waits_for_the_old_one_to_stop() {
        let dir = tempfile::TempDir::new().unwrap();
        let node = node(&dir, false).await;
        let shared = node.shared.clone();

        let first = shared.incoming_turn("t1").await;
        assert!(!first.stop.is_cancelled());
        let waiting = {
            let shared = shared.clone();
            tokio::spawn(async move { shared.incoming_turn("t1").await })
        };

        // The old attempt is told to stop, and the new one doesn't start until it has.
        tokio::time::timeout(Duration::from_secs(5), first.stop.cancelled()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!waiting.is_finished(), "two attempts on one part file");

        // Another transfer isn't held up by it.
        let other = tokio::time::timeout(Duration::from_secs(5), shared.incoming_turn("t2")).await.unwrap();
        drop(other);

        drop(first);
        let second = tokio::time::timeout(Duration::from_secs(5), waiting).await.unwrap().unwrap();
        assert!(!second.stop.is_cancelled());
        drop(second);
        assert!(lock(&shared.incoming_turns).is_empty(), "nothing left behind");
        node.shutdown().await;
    }

    /// A paired PC in the store, never connected.
    fn paired_pc(node: &Node, seed: u8) -> DeviceId {
        let id = crate::device_id(&iroh::SecretKey::from_bytes(&[seed; 32]).public());
        let info = DeviceInfo {
            name: "Studio".into(),
            kind: DeviceKind::Desktop,
            os: "linux".into(),
            os_ver: "6.12".into(),
            model: None,
            accent: None,
            screen: None,
            install_id: None,
        };
        node.shared.store.upsert_peer(&id, &info, crate::now_unix()).unwrap();
        id
    }

    fn sa(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    #[tokio::test]
    async fn known_addrs_merge_where_it_was_seen_what_it_said_and_what_was_typed() {
        use crate::reach::{PeerAddressOutcome, PeerReach};
        let dir = tempfile::TempDir::new().unwrap();
        let node = node(&dir, false).await;
        let pc = paired_pc(&node, 21);
        let shared = node.shared.clone();
        shared.store.record_seen(&pc, 1, &[sa("192.168.1.5:40314")]).unwrap();
        shared
            .store_reach(
                &pc,
                &g7link_protocol::messages::DeviceReach {
                    tailnet: vec!["100.64.0.7:7719".into(), "192.168.1.5:7719".into()],
                    away: false,
                },
            )
            .unwrap();
        let said = shared.store.get_peer(&pc).unwrap().unwrap().reach;
        assert_eq!(
            said,
            Some(PeerReach { tailnet: vec![sa("100.64.0.7:7719")], away: false }),
            "tailnet only"
        );

        assert_eq!(node.set_peer_address(pc, "100.64.0.9").await.unwrap(), PeerAddressOutcome::Ready);
        let record = shared.store.get_peer(&pc).unwrap().unwrap();
        assert_eq!(
            shared.known_addrs(&record),
            [
                "192.168.1.5:40314",
                "100.64.0.7:7719",
                "100.64.0.9:7718",
                "100.64.0.9:7719",
                "100.64.0.9:40314",
            ]
            .map(sa),
            "seen, said, then typed on the default and the known ports"
        );
        for addr in shared.known_addrs(&record) {
            assert!(node.dial_addrs(pc).contains(&addr), "{addr} is dialed");
        }

        // Typing what it said adds nothing twice.
        assert_eq!(node.set_peer_address(pc, "100.64.0.7:7719").await.unwrap(), PeerAddressOutcome::Ready);
        let record = shared.store.get_peer(&pc).unwrap().unwrap();
        assert_eq!(shared.known_addrs(&record), ["192.168.1.5:40314", "100.64.0.7:7719"].map(sa));
        node.shutdown().await;
    }

    #[tokio::test]
    async fn a_typed_name_counts_by_its_answers_and_never_dials_loopback() {
        use crate::reach::PeerAddressOutcome;
        let dir = tempfile::TempDir::new().unwrap();
        let node = node(&dir, false).await;
        let pc = paired_pc(&node, 22);
        let shared = node.shared.clone();

        // Answered, but only with this machine: saved, never dialed.
        assert_eq!(
            node.set_peer_address(pc, "LOCALHOST:5000").await.unwrap(),
            PeerAddressOutcome::NotAllowed
        );
        assert_eq!(node.peer_address(pc).unwrap().as_deref(), Some("localhost:5000"));
        let record = shared.store.get_peer(&pc).unwrap().unwrap();
        assert!(shared.known_addrs(&record).contains(&sa("127.0.0.1:5000")), "the answer counts");
        assert!(node.dial_addrs(pc).iter().all(|a| !a.ip().to_canonical().is_loopback()));

        // No answer (the name can't exist): saved all the same.
        assert_eq!(
            node.set_peer_address(pc, "g7link-test.invalid").await.unwrap(),
            PeerAddressOutcome::NotFound
        );
        assert_eq!(node.peer_address(pc).unwrap().as_deref(), Some("g7link-test.invalid"));
        let record = shared.store.get_peer(&pc).unwrap().unwrap();
        assert!(shared.known_addrs(&record).is_empty(), "the other name's answers are gone");
        node.shutdown().await;
    }

    #[tokio::test]
    async fn a_public_typed_ip_is_kept_but_dialed_only_with_away_mode_on() {
        use crate::reach::PeerAddressOutcome;
        let dir = tempfile::TempDir::new().unwrap();
        let node = node(&dir, false).await;
        let pc = paired_pc(&node, 23);
        let public = sa("8.8.8.8:7718");

        assert_eq!(node.set_peer_address(pc, "8.8.8.8:7718").await.unwrap(), PeerAddressOutcome::NotAllowed);
        assert_eq!(node.peer_address(pc).unwrap().as_deref(), Some("8.8.8.8:7718"));
        assert!(!node.dial_addrs(pc).contains(&public), "not with Away mode off");

        node.set_away_mode(true);
        settled(&node).await;
        assert!(node.dial_addrs(pc).contains(&public), "Away mode on: it's dialed");
        node.set_away_mode(false);
        settled(&node).await;
        node.shutdown().await;
    }

    #[tokio::test]
    async fn clearing_a_typed_address_stops_dialing_it() {
        use crate::reach::PeerAddressOutcome;
        let dir = tempfile::TempDir::new().unwrap();
        let node = node(&dir, false).await;
        let pc = paired_pc(&node, 24);
        let typed = sa("100.64.0.9:5000");

        assert_eq!(node.set_peer_address(pc, " 100.64.0.9:5000 ").await.unwrap(), PeerAddressOutcome::Ready);
        assert!(node.dial_addrs(pc).contains(&typed));
        assert_eq!(node.set_peer_address(pc, "http://x").await.unwrap(), PeerAddressOutcome::Invalid);
        assert_eq!(node.peer_address(pc).unwrap().as_deref(), Some("100.64.0.9:5000"), "nothing saved");

        assert_eq!(node.set_peer_address(pc, "").await.unwrap(), PeerAddressOutcome::Cleared);
        assert_eq!(node.peer_address(pc).unwrap(), None);
        assert!(!node.dial_addrs(pc).contains(&typed));

        // A device that isn't paired.
        let stranger = crate::device_id(&iroh::SecretKey::from_bytes(&[99; 32]).public());
        assert!(matches!(node.set_peer_address(stranger, "100.64.0.9").await, Err(Error::NotPaired)));
        assert!(matches!(node.peer_address(stranger), Err(Error::NotPaired)));
        assert!(matches!(node.away_reach(stranger, true), Err(Error::NotPaired)));
        node.shutdown().await;
    }

    #[tokio::test]
    async fn what_a_peer_says_replaces_what_it_said_before() {
        use crate::reach::AwayReach;
        use g7link_protocol::messages::DeviceReach;
        let dir = tempfile::TempDir::new().unwrap();
        let node = node(&dir, false).await;
        let pc = paired_pc(&node, 25);
        let shared = node.shared.clone();
        assert_eq!(node.away_reach(pc, true).unwrap(), AwayReach::HomeOnly);

        let said =
            |addrs: &[&str], away| DeviceReach { tailnet: addrs.iter().map(|a| (*a).into()).collect(), away };
        shared.store_reach(&pc, &said(&["100.64.0.7:7718"], false)).unwrap();
        assert!(node.dial_addrs(pc).contains(&sa("100.64.0.7:7718")));
        assert_eq!(node.away_reach(pc, true).unwrap(), AwayReach::Tailscale);
        assert_eq!(node.away_reach(pc, false).unwrap(), AwayReach::TailscaleOff);

        shared.store_reach(&pc, &said(&["100.64.0.8:7718"], true)).unwrap();
        let dialed = node.dial_addrs(pc);
        assert!(dialed.contains(&sa("100.64.0.8:7718")) && !dialed.contains(&sa("100.64.0.7:7718")));
        assert_eq!(node.peer_reach(pc).unwrap().map(|r| r.away), Some(true));

        shared.store_reach(&pc, &said(&[], false)).unwrap();
        assert!(node.dial_addrs(pc).is_empty());
        assert_eq!(node.away_reach(pc, true).unwrap(), AwayReach::HomeOnly);
        node.shutdown().await;
    }

    #[test]
    fn ports_are_tried_preferred_then_saved_then_any() {
        let p = |v4, v6| Ports { v4, v6 };
        let r = PORT_RETRIES;
        assert_eq!(
            port_order(Some(7000), Some(p(40314, None)), false),
            vec![(p(7000, Some(7000)), r), (p(40314, None), 0)]
        );
        assert_eq!(
            port_order(Some(7000), Some(p(40314, Some(40315))), true),
            vec![(p(7000, Some(7000)), r), (p(7000, None), 0), (p(40314, Some(40315)), 0)]
        );
        // The saved port is the preferred one: it's tried once.
        assert_eq!(port_order(Some(7000), Some(p(7000, None)), false), vec![(p(7000, Some(7000)), r)]);
        assert_eq!(
            port_order(Some(7000), Some(p(7000, None)), true),
            vec![(p(7000, Some(7000)), r), (p(7000, None), 0)]
        );
        // No preferred port (the phone, the CLI): the saved ports, as before.
        assert_eq!(port_order(None, Some(p(40314, None)), false), vec![(p(40314, None), r)]);
        assert_eq!(port_order(Some(0), None, true), vec![]);
        assert_eq!(port_order(None, None, false), vec![]);
    }

    fn free_udp_port() -> u16 {
        std::net::UdpSocket::bind("0.0.0.0:0").unwrap().local_addr().unwrap().port()
    }

    async fn node_preferring(dir: &tempfile::TempDir, port: u16) -> Node {
        let mut config = NodeConfig::new(
            dir.path().to_path_buf(),
            DeviceInfo {
                name: "Test".into(),
                kind: DeviceKind::Desktop,
                os: "linux".into(),
                os_ver: "6.12".into(),
                model: None,
                accent: None,
                screen: None,
                install_id: None,
            },
            "0.0.0-test",
        );
        config.lan_discovery = false;
        config.localsend_port = 0;
        config.preferred_port = Some(port);
        Node::start(config, Arc::new(NoopPlatform)).await.unwrap()
    }

    #[tokio::test]
    async fn a_free_preferred_port_is_the_one_listened_on() {
        let dir = tempfile::TempDir::new().unwrap();
        let port = free_udp_port();
        let node = node_preferring(&dir, port).await;
        assert_eq!(node.listen_port(), Some(port));
        assert_eq!(Ports::load(dir.path()).map(|p| p.v4), Some(port), "and it's remembered");
        node.shutdown().await;
    }

    #[tokio::test]
    async fn a_busy_preferred_port_falls_back_to_the_saved_one_then_any() {
        let held = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        let busy = held.local_addr().unwrap().port();

        let dir = tempfile::TempDir::new().unwrap();
        let saved = free_udp_port();
        std::fs::write(dir.path().join(PORT_FILE), saved.to_string()).unwrap();
        let node = node_preferring(&dir, busy).await;
        assert_eq!(node.listen_port(), Some(saved), "the saved port");
        node.shutdown().await;

        let dir = tempfile::TempDir::new().unwrap();
        let node = node_preferring(&dir, busy).await;
        let port = node.listen_port().expect("it still starts");
        assert_ne!(port, busy);
        assert_eq!(Ports::load(dir.path()).map(|p| p.v4), Some(port), "the port file names the bound port");
        node.shutdown().await;
        drop(held);
    }

    #[test]
    fn the_name_is_on_the_lan_only_while_pairing() {
        assert!(announced_name("Bee's phone", false).is_none());
        let named = announced_name("Bee's phone", true).map(|d| d.to_string());
        assert_eq!(named.as_deref(), Some("Bee's phone"));
        let long = "é".repeat(UserData::MAX_LENGTH);
        let cut = announced_name(&long, true).expect("a long name is cut, not dropped").to_string();
        assert!(cut.len() <= UserData::MAX_LENGTH && long.starts_with(&cut));
    }

    #[test]
    fn what_is_published_has_no_device_name() {
        use iroh::address_lookup::{AddressLookup, EndpointData};
        #[derive(Debug, Clone, Default)]
        struct Seen(Arc<Mutex<Option<EndpointData>>>);
        impl AddressLookup for Seen {
            fn publish(&self, data: &EndpointData) {
                *self.0.lock().unwrap() = Some(data.clone());
            }
        }
        let seen = Seen::default();
        let data = EndpointData::new(Vec::new()).with_user_data("Bee's phone".parse().unwrap());
        WithoutName(seen.clone()).publish(&data);
        assert!(seen.0.lock().unwrap().as_ref().unwrap().user_data().is_none());
    }
}

#[cfg(test)]
mod seen_on_lan_tests {
    #[test]
    fn only_a_failed_or_refused_dial_backs_off() {
        use super::{BACKOFF_MAX, BACKOFF_MIN, EndReason, next_redial_wait};
        let long = Duration::from_secs(16);
        assert_eq!(next_redial_wait(EndReason::Closed, long), BACKOFF_MIN, "established, then closed");
        assert_eq!(next_redial_wait(EndReason::DialFailed, BACKOFF_MIN), BACKOFF_MIN * 2);
        assert_eq!(next_redial_wait(EndReason::Refused, Duration::from_secs(4)), Duration::from_secs(8));
        assert_eq!(next_redial_wait(EndReason::Refused, BACKOFF_MAX), BACKOFF_MAX);
    }

    #[test]
    fn a_peer_that_is_our_gateway_is_local_at_any_address() {
        let gateway: std::net::IpAddr = "10.76.220.1".parse().unwrap();
        // Reached on the hotspot's own address: any path of it counts.
        assert!(super::allowed_via_gateway(&[gateway], Some(gateway)));
        // Another gateway (the laptop moved networks), or never reached there: no.
        let other: std::net::IpAddr = "192.168.1.1".parse().unwrap();
        assert!(!super::allowed_via_gateway(&[other], Some(gateway)));
        assert!(!super::allowed_via_gateway(&[gateway], None));
        assert!(!super::allowed_via_gateway(&[], Some(gateway)));
    }

    #[test]
    fn default_gateways_come_from_the_route_table() {
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\n\
                     wlan0\t00000000\t01DC4C0A\t0003\t0\t0\t600\t00000000\n\
                     wlan0\t00DC4C0A\t00000000\t0001\t0\t0\t600\t00FFFFFF\n\
                     docker0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\n";
        assert_eq!(
            super::parse_default_gateways(table),
            vec!["10.76.220.1".parse::<std::net::IpAddr>().unwrap()]
        );
        assert!(super::parse_default_gateways("").is_empty());
    }

    use super::*;
    use iroh::address_lookup::{AddressLookup, EndpointData, EndpointInfo, Item};

    /// A lookup that answers with one fixed item.
    #[derive(Debug)]
    struct Fixed(Item);

    impl AddressLookup for Fixed {
        fn resolve(
            &self,
            _endpoint_id: iroh::EndpointId,
        ) -> Option<n0_future::boxed::BoxStream<std::result::Result<Item, iroh::address_lookup::Error>>>
        {
            Some(Box::pin(n0_future::stream::iter([Ok(self.0.clone())])))
        }
    }

    #[tokio::test]
    async fn a_device_found_while_dialing_counts_as_seen_on_this_network() {
        use n0_future::StreamExt;
        let id = iroh::SecretKey::from_bytes(&[9; 32]).public();
        let addr: SocketAddr = "192.1.1.169:49806".parse().unwrap();
        let data = EndpointData::from_iter([iroh::TransportAddr::Ip(addr)]);
        let item = Item::new(EndpointInfo::from_parts(id, data), "mdns", None);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        let lookup = SeenOnLan {
            inner: Fixed(item),
            seen: Arc::new(move |info: &EndpointInfo| {
                lock(&recorded).push((info.endpoint_id, info.ip_addrs().copied().collect::<Vec<_>>()));
            }),
        };

        let found: Vec<_> = lookup.resolve(id).expect("a stream").collect().await;

        assert_eq!(found.len(), 1, "the dial still gets the address");
        assert_eq!(*lock(&seen), vec![(id, vec![addr])]);
    }

    #[test]
    fn a_sighting_makes_its_addresses_this_networks() {
        let addr: SocketAddr = "192.1.1.169:49806".parse().unwrap();
        let ip = addr.ip();
        // Outside the private ranges: only an mDNS sighting makes it home.
        assert!(!crate::session::is_lan_ip(ip));
        assert!(crate::session::is_lan_peer_ip(ip, &[addr]));
    }
}
