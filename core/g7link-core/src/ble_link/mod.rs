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

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use g7link_protocol::{
    DeviceId, Envelope, ErrorCode,
    ble::{self, LINK_SECRET_LEN},
    messages::{BleKey, Ring, RingStopped, types},
};
use tokio::sync::{Notify, oneshot};

pub use endpoint::BleEndpoint;

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
/// How long the PC waits for a phone to connect over Bluetooth when it has
/// something for it.
const LINK_WAIT: Duration = Duration::from_secs(20);

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
        self.ble.changed.notify_waiters();
        self.emit(NodeEvent::BleLinkChanged { device: peer, up: true });
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
        }
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
            _ => Ok(None),
        }
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
