// SPDX-License-Identifier: MPL-2.0
//! Bluetooth between a paired phone and PC (docs/protocol/bluetooth.md).
//!
//! The PC makes a link secret per phone and sends it over every session
//! (`ble.key`); the phone keeps it and recognises the PC's rotating beacon by
//! it. The same secret, with both devices' keys, lets the phone open a small
//! channel to the PC over Bluetooth when they share no network: a handshake
//! ([`crypto`]) and sealed records ([`BleEndpoint`]), carrying only a short
//! list of messages.

mod crypto;
mod endpoint;
mod hold;
mod route;

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use g7link_protocol::{
    DeviceId, Envelope, ErrorCode,
    ble::{self, LINK_SECRET_LEN},
    messages::{
        BleKey, BleOtp, ClipSet, Notification, PhoneToggleSet, PhoneToggles, RemoteInput, RemoteTextFocus,
        RemoteTextState, RemoteTextWatch, Ring, RingStopped, remote as remote_limits, toggle_ids, types,
    },
};
use tokio::sync::{Notify, oneshot};

pub use endpoint::BleEndpoint;
pub use route::{RemotePath, RemoteRoute, RouteState};

use crate::{
    Error, Result,
    events::NodeEvent,
    features::{Role, role_of},
    node::Shared,
    session::Session,
};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The code an error is reported to the other device with.
fn wire_code(error: &Error) -> ErrorCode {
    match error {
        Error::Denied | Error::Declined => ErrorCode::Denied,
        Error::Unsupported => ErrorCode::Unsupported,
        Error::NotPaired => ErrorCode::Unpaired,
        Error::Protocol(_) => ErrorCode::BadMessage,
        _ => ErrorCode::Internal,
    }
}

impl Shared {
    fn local_role(&self) -> Option<Role> {
        role_of(&self.local.read().unwrap_or_else(|e| e.into_inner()).device)
    }

    fn peer_role(&self, peer: &DeviceId) -> Option<Role> {
        self.store.get_peer(peer).ok().flatten().and_then(|p| role_of(&p.info))
    }

    /// PC only: sends a paired phone the secret the two share for Bluetooth,
    /// making one first. Nothing unless this device is a PC and `peer` a phone.
    pub(crate) async fn send_ble_key(&self, session: &Arc<Session>) {
        let peer = session.peer;
        if self.local_role() != Some(Role::Desktop) || self.peer_role(&peer) != Some(Role::Phone) {
            return;
        }
        let secret = match self.store.link_secret(&peer) {
            Ok(Some(secret)) => secret,
            Ok(None) => {
                let secret: [u8; LINK_SECRET_LEN] = rand::random();
                match self.store.set_link_secret(&peer, &secret) {
                    Ok(true) => self.emit(NodeEvent::BleKeysChanged),
                    Ok(false) => return,
                    Err(error) => {
                        tracing::warn!(peer = %peer.short(), %error, "can't keep a Bluetooth link secret");
                        return;
                    }
                }
                secret
            }
            Err(error) => {
                tracing::warn!(peer = %peer.short(), %error, "can't read a Bluetooth link secret");
                return;
            }
        };
        let body = BleKey { secret: secret.to_vec(), bt_addr: None };
        if let Ok(env) = Envelope::new(types::BLE_KEY, &body) {
            let _ = session.send(env).await;
        }
    }

    /// Phone only: keeps the secret a paired PC sent for Bluetooth.
    pub(crate) fn ble_key_from_peer(&self, peer: DeviceId, key: BleKey) -> Result<()> {
        if self.local_role() != Some(Role::Phone) || self.peer_role(&peer) != Some(Role::Desktop) {
            tracing::debug!(peer = %peer.short(), "ignored a ble.key between devices that don't use one");
            return Ok(());
        }
        let Some(secret) = key.secret() else {
            tracing::debug!(peer = %peer.short(), "ignored a malformed ble.key");
            return Ok(());
        };
        if self.store.set_link_secret(&peer, &secret)? {
            self.emit(NodeEvent::BleKeysChanged);
        }
        Ok(())
    }

    /// The beacon key of each paired device this one shares a link secret with.
    pub(crate) fn ble_beacon_keys(&self) -> Vec<(DeviceId, [u8; 32])> {
        let peers = self.store.list_peers().unwrap_or_default();
        peers
            .into_iter()
            .filter_map(|p| {
                let secret = self.store.link_secret(&p.id).ok().flatten()?;
                Some((p.id, ble::beacon_key(&secret)))
            })
            .collect()
    }
}

/// The device toggle that lets a paired device use the Bluetooth channel.
pub const BLE_LINK_TOGGLE: &str = "ble_link";
/// What a device whose app can use the Bluetooth channel announces.
pub const BLE_LINK_CAPABILITY: &str = "ble.link";
/// What a device announces whose link also carries the typing box and
/// `ble.use`, and (a phone) that picks a path for its touchpad
/// (docs/protocol/bluetooth.md §4.5). Offered with [`BLE_LINK_CAPABILITY`].
pub const BLE_REMOTE_CAPABILITY: &str = "ble.remote";
/// How long the PC waits for a phone to connect over Bluetooth when it has
/// something for it.
const LINK_WAIT: Duration = Duration::from_secs(20);
/// How long a connect window the PC opens for a phone that asks or holds a
/// link stays open, unless a link comes up first: the platform's window and
/// what `ble.use` replies say (docs/protocol/bluetooth.md §4.1, §4.5).
pub const CONNECT_WINDOW: Duration = Duration::from_secs(30);
/// How long a request over Bluetooth waits for its reply.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a key or click over Bluetooth waits for its answer: the input
/// after it waits behind it, and a link that stalls loses it to the session.
const INPUT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a key or click the PC turned away as too many keeps being
/// resent, and how long between tries.
const INPUT_RETRY_FOR: Duration = Duration::from_secs(3);
const INPUT_RETRY_AFTER: Duration = Duration::from_millis(100);
/// Copied text that goes over Bluetooth at most, in UTF-8 bytes; longer waits
/// in the queue for the network.
pub(crate) const MAX_CLIP_TEXT: usize = 2048;

/// The Bluetooth links that are up, by the device at the other end, and the
/// requests waiting on them.
#[derive(Default)]
pub(crate) struct BleLinks {
    links: Mutex<HashMap<DeviceId, Arc<BleEndpoint>>>,
    /// Replies awaited, by the request's ID, with the link they went on.
    pending: Mutex<HashMap<u64, (u64, oneshot::Sender<Envelope>)>>,
    next_request: AtomicU64,
    /// A link came up.
    changed: Notify,
    /// PC: the phones holding a link (`ble.use`).
    holds: Mutex<hold::Holds>,
    /// Phone: each PC's touchpad route.
    routes: Mutex<route::Routes>,
}

impl std::fmt::Debug for BleLinks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BleLinks")
    }
}

/// Messages the Bluetooth channel carries; anything else is refused.
fn allowed(t: &str) -> bool {
    matches!(
        t,
        types::PING
            | types::PONG
            | types::OK
            | types::ERROR
            | types::DEVICE_RING
            | types::DEVICE_RING_STOPPED
            | types::CLIP_SET
            | types::BLE_OTP
            | types::PHONE_TOGGLE_SET
            | types::PHONE_TOGGLES
            | types::REMOTE_CHECK
            | types::REMOTE_INPUT
            | types::REMOTE_TEXT_WATCH
            | types::REMOTE_TEXT_STATE
            | types::REMOTE_TEXT_FOCUS
            | types::BLE_USE
    )
}

impl Shared {
    /// Whether this device may use the Bluetooth channel with `peer`: it's a
    /// phone and a PC, the user turned it on here for `peer`, they share a
    /// secret, and `peer`'s app can.
    pub(crate) fn ble_usable(&self, peer: &DeviceId) -> bool {
        let roles = (self.local_role(), self.peer_role(peer));
        matches!(roles, (Some(a), Some(b)) if a != b)
            && self.toggle_on(peer, BLE_LINK_TOGGLE)
            && self.store.link_secret(peer).ok().flatten().is_some()
            && self.store.get_peer(peer).ok().flatten().is_some_and(|p| p.caps.contains(BLE_LINK_CAPABILITY))
    }

    /// PC: the paired phones that may start a link, with their secrets.
    pub(crate) fn ble_candidates(&self) -> Vec<(DeviceId, [u8; LINK_SECRET_LEN])> {
        if self.local_role() != Some(Role::Desktop) {
            return Vec::new();
        }
        let peers = self.store.list_peers().unwrap_or_default();
        peers
            .into_iter()
            .filter(|p| role_of(&p.info) == Some(Role::Phone) && self.toggle_on(&p.id, BLE_LINK_TOGGLE))
            .filter_map(|p| Some((p.id, self.store.link_secret(&p.id).ok().flatten()?)))
            .collect()
    }

    /// The Bluetooth link with `peer`, while it's up.
    pub(crate) fn ble_link(&self, peer: &DeviceId) -> Option<Arc<BleEndpoint>> {
        lock(&self.ble.links).get(peer).filter(|e| !e.is_closed()).cloned()
    }

    pub(crate) fn ble_link_up(&self, peer: DeviceId, endpoint: &Arc<BleEndpoint>) {
        if !self.store.is_paired(&peer).unwrap_or(false) {
            endpoint.close();
            return;
        }
        let previous = lock(&self.ble.links).insert(peer, endpoint.clone());
        if let Some(previous) = previous.filter(|p| !Arc::ptr_eq(p, endpoint)) {
            previous.close();
        }
        tracing::info!(peer = %peer.short(), "Bluetooth link up");
        // The platform closes its connect window once a link is up.
        self.held_link_up();
        self.ble.changed.notify_waiters();
        self.emit(NodeEvent::BleLinkChanged { device: peer, up: true });
        self.route_refresh(&peer);
    }

    pub(crate) fn ble_link_down(&self, peer: DeviceId, endpoint: &Arc<BleEndpoint>) {
        let removed = {
            let mut links = lock(&self.ble.links);
            let ours = links.get(&peer).is_some_and(|e| Arc::ptr_eq(e, endpoint));
            ours && links.remove(&peer).is_some()
        };
        lock(&self.ble.pending).retain(|_, (link, _)| *link != endpoint.id());
        if removed {
            tracing::info!(peer = %peer.short(), "Bluetooth link down");
            self.emit(NodeEvent::BleLinkChanged { device: peer, up: false });
            self.route_refresh(&peer);
            self.held_link_down(peer);
        }
    }

    /// Forgets what's kept for the Bluetooth link with `peer` (unpaired).
    pub(crate) fn forget_ble(&self, peer: &DeviceId) {
        lock(&self.ble.holds).remove_peer(peer);
        lock(&self.ble.routes).remove_peer(peer);
    }

    /// A message that came in on a Bluetooth link.
    pub(crate) fn ble_message(self: &Arc<Self>, peer: DeviceId, endpoint: &Arc<BleEndpoint>, env: Envelope) {
        if let Some(re) = env.re {
            let waiting = lock(&self.ble.pending).remove(&re);
            if let Some((link, reply)) = waiting
                && link == endpoint.id()
            {
                let _ = reply.send(env);
            }
            return;
        }
        let shared = self.clone();
        let endpoint = endpoint.clone();
        self.runtime.spawn(async move {
            let id = env.id;
            let reply = match shared.handle_ble(peer, &endpoint, env).await {
                Ok(reply) => reply,
                Err(error) => Some(Envelope::error(wire_code(&error), error.to_string())),
            };
            if let (Some(reply), Some(_)) = (reply, id)
                && let Err(error) = endpoint.send(&reply.reply_to(id))
            {
                tracing::debug!(peer = %peer.short(), %error, "couldn't answer over Bluetooth");
            }
        });
    }

    /// Handles a request or message from `peer` on its Bluetooth link: the
    /// reply, if it gets one.
    async fn handle_ble(
        self: &Arc<Self>,
        peer: DeviceId,
        endpoint: &Arc<BleEndpoint>,
        env: Envelope,
    ) -> Result<Option<Envelope>> {
        if !allowed(&env.t) {
            return Err(Error::Unsupported);
        }
        if !self.toggle_on(&peer, BLE_LINK_TOGGLE) {
            return Err(Error::Denied);
        }
        match env.t.as_str() {
            types::PING => Ok(Some(Envelope::empty(types::PONG))),
            types::DEVICE_RING => {
                let ring: Ring = env.body()?;
                let link = Arc::downgrade(endpoint);
                self.receive_ring(peer, move || link.upgrade().is_some_and(|l| l.is_up()), ring).await;
                Ok(Some(Envelope::empty(types::OK)))
            }
            types::DEVICE_RING_STOPPED => {
                let stopped: RingStopped = env.body()?;
                self.clear_remote_ring(peer, Some(stopped.token));
                Ok(None)
            }
            types::CLIP_SET => {
                if !self.toggle_on(&peer, crate::clipboard::TOGGLE) {
                    return Err(Error::Denied);
                }
                let clip: ClipSet = env.body()?;
                if !clip.is_valid() || clip.text.len() > MAX_CLIP_TEXT {
                    return Err(Error::TooLarge);
                }
                // Plain text only over Bluetooth.
                let clip = ClipSet { html: None, rtf: None, ..clip };
                Ok(Some(crate::clipboard::apply_set(self, peer, clip).await))
            }
            // The phone's touchpad and keyboard: the PC's own checks and limits apply.
            types::REMOTE_CHECK => Ok(Some(crate::remote::run_check(self, &peer))),
            // The typing box, as on a session.
            types::REMOTE_TEXT_WATCH | types::REMOTE_TEXT_STATE | types::REMOTE_TEXT_FOCUS => {
                crate::remote::incoming_one_way(self, peer, &env, RemotePath::Ble)?;
                Ok(None)
            }
            types::BLE_USE => Ok(Some(self.ble_use_reply(peer, &env))),
            types::REMOTE_INPUT => {
                let event: RemoteInput = env.body()?;
                if env.id.is_none() {
                    crate::remote::run_motion(self, peer, event);
                    return Ok(None);
                }
                Ok(Some(crate::remote::run_control(self, peer, event).await))
            }
            types::PHONE_TOGGLE_SET => {
                let req: PhoneToggleSet = env.body()?;
                // Only the hotspot: what a PC with no network needs.
                if req.id != toggle_ids::HOTSPOT {
                    return Err(Error::Denied);
                }
                Ok(Some(crate::toggles::run_set(self, &peer, req).await))
            }
            types::PHONE_TOGGLES => {
                let state: PhoneToggles = env.body()?;
                crate::toggles::apply_state(self, peer, state);
                Ok(None)
            }
            types::BLE_OTP => {
                // A PC takes codes from a phone it shows notifications of.
                if self.local_role() != Some(Role::Desktop) || !self.notifications_allowed(&peer) {
                    return Err(Error::Denied);
                }
                let otp: BleOtp = env.body()?;
                if !otp.is_valid() {
                    return Err(Error::Protocol("malformed code".into()));
                }
                self.emit(NodeEvent::OneTimeCode { device: peer, code: otp.code, source: otp.source });
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Phone: sends the one-time code in `notification`, if it has one, to the
    /// paired PCs reachable only over Bluetooth that show this phone's
    /// notifications. The notification itself stays on the phone.
    pub(crate) fn send_code_over_ble(&self, notification: &Notification) {
        if self.local_role() != Some(Role::Phone) {
            return;
        }
        let peers: Vec<DeviceId> = lock(&self.ble.links).keys().copied().collect();
        let peers: Vec<DeviceId> = peers
            .into_iter()
            .filter(|peer| self.session(peer).is_none() && self.notifications_allowed(peer))
            .collect();
        if peers.is_empty() {
            return;
        }
        let text: Vec<&str> =
            [notification.title.as_deref(), notification.text.as_deref()].into_iter().flatten().collect();
        let Some(code) = crate::otp::one_time_code(&text.join("\n")) else { return };
        let source =
            Some(notification.app_name.chars().take(32).collect::<String>()).filter(|s| !s.is_empty());
        let otp = BleOtp { code, source };
        let Ok(env) = Envelope::new(types::BLE_OTP, &otp) else { return };
        if !otp.is_valid() {
            return;
        }
        for peer in peers {
            if let Err(error) = self.ble_send(&peer, &env) {
                tracing::debug!(peer = %peer.short(), %error, "couldn't send a code over Bluetooth");
            }
        }
    }

    /// Phone: sends touchpad or keyboard input to the PC `peer` over the
    /// Bluetooth link. Motion is added up and sent as the link has room;
    /// anything else waits for the PC's answer.
    pub(crate) async fn ble_input(&self, peer: DeviceId, event: RemoteInput) -> Result<()> {
        if !event.is_valid() {
            return Err(Error::Protocol("invalid remote input".into()));
        }
        if !crate::remote::takes_input(self, &peer) {
            return Err(Error::Unsupported);
        }
        let link = self.ble_link(&peer).ok_or(Error::Offline)?;
        if event.is_datagram_allowed() && !matches!(event, RemoteInput::Laser { on: false, .. }) {
            return link.send_motion(&event);
        }
        // A PC over its budget answers Busy: wait and try again, in order.
        let give_up = Instant::now() + INPUT_RETRY_FOR;
        loop {
            let env = Envelope::new(types::REMOTE_INPUT, &event)?;
            let request = self.ble_request(peer, env, INPUT_TIMEOUT);
            tokio::pin!(request);
            // No answer for a while: input goes on the session meanwhile. This
            // one isn't sent again there, as it may have run.
            let reply = tokio::select! {
                reply = &mut request => reply,
                () = tokio::time::sleep(route::STALL_AFTER) => {
                    self.ble_stalled(&peer);
                    request.await
                }
            };
            match reply?.expect(types::OK) {
                Err(g7link_protocol::ProtocolError::Remote {
                    code: g7link_protocol::ErrorCode::Busy,
                    ..
                }) if Instant::now() < give_up => {
                    tokio::time::sleep(INPUT_RETRY_AFTER).await;
                }
                done => return Ok(done?),
            }
        }
    }

    /// Phone: whether the PC `peer` takes input from this phone, asked over Bluetooth.
    pub(crate) async fn ble_input_check(&self, peer: DeviceId) -> Result<()> {
        self.ble_request(peer, Envelope::empty(types::REMOTE_CHECK), REQUEST_TIMEOUT)
            .await?
            .expect(types::OK)?;
        Ok(())
    }

    /// Phone: tells the PC `peer` over the link that the typing box opened
    /// (`on`) or closed (`remote.text_watch`). The PC sends its states on the
    /// path the last watch came on.
    pub(crate) fn ble_text_watch(&self, peer: DeviceId, on: bool) -> Result<()> {
        if !crate::remote::mirrors_text(self, &peer) {
            return Err(Error::Unsupported);
        }
        if !self.peer_offers_ble_remote(&peer) {
            return Err(Error::Offline);
        }
        self.ble_send(&peer, &Envelope::new(types::REMOTE_TEXT_WATCH, &RemoteTextWatch { on })?)
    }

    /// PC: sends the phone `peer` the focused field over the link, cut to
    /// what a message there holds; a newer one replaces it while it waits.
    pub(crate) fn ble_text_state(&self, peer: DeviceId, state: &RemoteTextState) -> Result<()> {
        if !self.toggle_on(&peer, crate::remote::TOGGLE) {
            return Ok(());
        }
        if !state.is_valid() {
            return Err(Error::Protocol("invalid text state".into()));
        }
        if !self.peer_offers_ble_remote(&peer) {
            return Err(Error::Offline);
        }
        let link = self.ble_link(&peer).ok_or(Error::Offline)?;
        let (mut before, mut after) = if link.att_mtu() < remote_limits::SMALL_ATT_MTU {
            (remote_limits::MAX_BLE_STATE_SMALL_BEFORE, remote_limits::MAX_BLE_STATE_SMALL_AFTER)
        } else {
            (remote_limits::MAX_BLE_STATE_BEFORE, remote_limits::MAX_BLE_STATE_AFTER)
        };
        loop {
            let env = Envelope::new(types::REMOTE_TEXT_STATE, &state.trimmed(before, after))?;
            if env.to_cbor().len() <= endpoint::MAX_MESSAGE {
                return link.send_state(&env);
            }
            if before == 0 && after == 0 {
                return Err(Error::TooLarge);
            }
            (before, after) = (before / 2, after / 2);
        }
    }

    /// PC: tells the phone `peer` over the link whether the focus takes text.
    pub(crate) fn ble_text_focus(&self, peer: DeviceId, editable: bool) -> Result<()> {
        if !self.toggle_on(&peer, crate::remote::TOGGLE) {
            return Ok(());
        }
        if !self.peer_offers_ble_remote(&peer) {
            return Err(Error::Offline);
        }
        self.ble_send(&peer, &Envelope::new(types::REMOTE_TEXT_FOCUS, &RemoteTextFocus { editable })?)
    }

    /// Whether paired `peer` offers [`BLE_REMOTE_CAPABILITY`].
    pub(crate) fn peer_offers_ble_remote(&self, peer: &DeviceId) -> bool {
        self.store.get_peer(peer).ok().flatten().is_some_and(|p| p.caps.contains(BLE_REMOTE_CAPABILITY))
    }

    /// The devices a Bluetooth link is up with.
    pub(crate) fn ble_linked_peers(&self) -> Vec<DeviceId> {
        lock(&self.ble.links).iter().filter(|(_, e)| !e.is_closed()).map(|(id, _)| *id).collect()
    }

    /// Sends `env` to `peer` on its Bluetooth link without waiting for a reply.
    pub(crate) fn ble_send(&self, peer: &DeviceId, env: &Envelope) -> Result<()> {
        self.ble_link(peer).ok_or(Error::Offline)?.send(env)
    }

    /// The link with `peer`, asking the platform for one first when this is
    /// the PC and there's none, and waiting up to [`LINK_WAIT`] for it.
    async fn ble_link_for(&self, peer: DeviceId) -> Result<Arc<BleEndpoint>> {
        if let Some(link) = self.ble_link(&peer) {
            return Ok(link);
        }
        if self.local_role() != Some(Role::Desktop) || !self.ble_usable(&peer) {
            return Err(Error::Offline);
        }
        let came_up = self.ble.changed.notified();
        tokio::pin!(came_up);
        came_up.as_mut().enable();
        self.emit(NodeEvent::BleWanted { device: peer });
        let deadline = tokio::time::Instant::now() + LINK_WAIT;
        loop {
            if let Some(link) = self.ble_link(&peer) {
                return Ok(link);
            }
            if tokio::time::timeout_at(deadline, came_up.as_mut()).await.is_err() {
                return Err(Error::Offline);
            }
            came_up.set(self.ble.changed.notified());
            came_up.as_mut().enable();
        }
    }

    /// Sends a request to `peer` over Bluetooth and waits up to `timeout` for its reply.
    pub(crate) async fn ble_request(
        &self,
        peer: DeviceId,
        env: Envelope,
        timeout: Duration,
    ) -> Result<Envelope> {
        let link = self.ble_link_for(peer).await?;
        let id = self.ble.next_request.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        lock(&self.ble.pending).insert(id, (link.id(), tx));
        if let Err(error) = link.send(&env.with_id(id)) {
            lock(&self.ble.pending).remove(&id);
            return Err(error);
        }
        let reply = tokio::time::timeout(timeout, rx).await;
        lock(&self.ble.pending).remove(&id);
        match reply {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(_)) => Err(Error::Offline),
            Err(_) => Err(Error::Timeout),
        }
    }

    /// Ends the Bluetooth link with `peer`, if there is one.
    pub(crate) fn close_ble_link(&self, peer: &DeviceId) {
        let link = lock(&self.ble.links).get(peer).cloned();
        if let Some(link) = link {
            link.close();
        }
    }
}
