// SPDX-License-Identifier: MPL-2.0
//! Bluetooth between a paired phone and PC (docs/protocol/bluetooth.md): the
//! link secret behind the PC's rotating beacon. The PC makes one per phone and
//! sends it over every session (`ble.key`); the phone keeps it and recognises
//! the PC's beacon by it.

use std::sync::Arc;

use g7link_protocol::{
    DeviceId, Envelope,
    ble::{self, LINK_SECRET_LEN},
    messages::{BleKey, types},
};

use crate::{
    Result,
    events::NodeEvent,
    features::{Role, role_of},
    node::Shared,
    session::Session,
};

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
