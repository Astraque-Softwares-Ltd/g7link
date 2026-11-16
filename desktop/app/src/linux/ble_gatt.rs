// SPDX-License-Identifier: GPL-3.0-or-later
//! The PC's side of the Bluetooth channel (docs/protocol/bluetooth.md): a GATT
//! service BlueZ serves for G7 Link, which a paired phone connects to while the
//! PC advertises that it can be connected to. The phone writes to RX and the
//! PC notifies on TX; each value is one fragment of a record, which the core's
//! [`BleEndpoint`] puts together, checks and answers. Writes are handed to the
//! `ble-link` thread as they come; BlueZ's call never waits on the core.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

use g7link_core::{BleEndpoint, ble};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

pub(super) const APP_PATH: &str = "/app/g7link/gatt";
pub(super) const SERVICE_PATH: &str = "/app/g7link/gatt/service0";
pub(super) const RX_PATH: &str = "/app/g7link/gatt/service0/rx";
pub(super) const TX_PATH: &str = "/app/g7link/gatt/service0/tx";
const CHARACTERISTIC: &str = "org.bluez.GattCharacteristic1";
const DEVICE: &str = "org.bluez.Device1";

/// Writes waiting for the link thread at most; past that, BlueZ is told it failed.
const WRITE_QUEUE: usize = 64;
/// A phone that hasn't proved who it is by then is disconnected.
const HANDSHAKE_WITHIN: Duration = Duration::from_secs(10);

/// What the link thread is told.
#[derive(Debug)]
pub(super) enum LinkMsg {
    /// A phone wrote a value to RX.
    Write {
        device: OwnedObjectPath,
        mtu: Option<u16>,
        value: Vec<u8>,
    },
    /// The core has something to send.
    Outbound,
    /// A device connected or disconnected.
    DeviceChanged,
    Stop,
}

pub(super) type LinkSender = SyncSender<LinkMsg>;

/// The service: `org.bluez.GattService1`.
pub(super) struct Service;

#[zbus::interface(name = "org.bluez.GattService1")]
impl Service {
    #[zbus(property, name = "UUID")]
    fn uuid(&self) -> String {
        ble::SERVICE_UUID.to_owned()
    }

    #[zbus(property)]
    fn primary(&self) -> bool {
        true
    }
}

/// RX: the phone writes, with a response.
pub(super) struct Rx {
    pub(super) to_link: LinkSender,
}

#[zbus::interface(name = "org.bluez.GattCharacteristic1")]
impl Rx {
    #[zbus(property, name = "UUID")]
    fn uuid(&self) -> String {
        ble::RX_UUID.to_owned()
    }

    #[zbus(property)]
    fn service(&self) -> OwnedObjectPath {
        OwnedObjectPath::from(ObjectPath::from_static_str_unchecked(SERVICE_PATH))
    }

    #[zbus(property)]
    fn flags(&self) -> Vec<String> {
        vec!["write".to_owned()]
    }

    fn write_value(&self, value: Vec<u8>, options: HashMap<String, OwnedValue>) -> zbus::fdo::Result<()> {
        let write = parse_write(value, &options).map_err(zbus::fdo::Error::InvalidArgs)?;
        match self.to_link.try_send(write) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(zbus::fdo::Error::LimitsExceeded("busy".into())),
            Err(TrySendError::Disconnected(_)) => Err(zbus::fdo::Error::Failed("not running".into())),
        }
    }
}

/// What a `WriteValue` asks, or why it's refused: a write at an offset (a
/// long write) or from no device isn't one a phone of ours makes.
pub(super) fn parse_write(value: Vec<u8>, options: &HashMap<String, OwnedValue>) -> Result<LinkMsg, String> {
    let offset = options.get("offset").and_then(|v| u16::try_from(v).ok()).unwrap_or(0);
    if offset != 0 {
        return Err("writes start at 0".into());
    }
    let device = options
        .get("device")
        .and_then(|v| match &**v {
            Value::ObjectPath(p) => Some(OwnedObjectPath::from(p.clone())),
            _ => None,
        })
        .ok_or_else(|| "no device".to_owned())?;
    let mtu = options.get("mtu").and_then(|v| u16::try_from(v).ok());
    Ok(LinkMsg::Write { device, mtu, value })
}

/// TX: the PC notifies.
pub(super) struct Tx;

#[zbus::interface(name = "org.bluez.GattCharacteristic1")]
impl Tx {
    #[zbus(property, name = "UUID")]
    fn uuid(&self) -> String {
        ble::TX_UUID.to_owned()
    }

    #[zbus(property)]
    fn service(&self) -> OwnedObjectPath {
        OwnedObjectPath::from(ObjectPath::from_static_str_unchecked(SERVICE_PATH))
    }

    #[zbus(property)]
    fn flags(&self) -> Vec<String> {
        vec!["notify".to_owned()]
    }

    #[zbus(property)]
    fn value(&self) -> Vec<u8> {
        Vec::new()
    }

    fn start_notify(&self) {}

    fn stop_notify(&self) {}
}

/// Notifies `fragment` to the phone that subscribed to TX: BlueZ sends a
/// characteristic's new value as a notification.
fn notify(conn: &zbus::blocking::Connection, fragment: Vec<u8>) -> zbus::Result<()> {
    let changed: HashMap<&str, Value<'_>> = HashMap::from([("Value", Value::from(fragment))]);
    conn.emit_signal(
        None::<&str>,
        TX_PATH,
        "org.freedesktop.DBus.Properties",
        "PropertiesChanged",
        &(CHARACTERISTIC, changed, Vec::<&str>::new()),
    )
}

/// The link with the connected phone, as the link thread holds it.
struct Current {
    device: OwnedObjectPath,
    endpoint: Arc<BleEndpoint>,
    since: Instant,
}

/// Where the link thread gets its endpoints.
pub(super) type Accept = Arc<dyn Fn() -> Option<Arc<BleEndpoint>> + Send + Sync>;

/// Whether BlueZ still has `device` connected.
fn connected(conn: &zbus::blocking::Connection, device: &OwnedObjectPath) -> bool {
    let reply = conn.call_method(
        Some("org.bluez"),
        device.as_str(),
        Some("org.freedesktop.DBus.Properties"),
        "Get",
        &(DEVICE, "Connected"),
    );
    reply.ok().and_then(|r| r.body().deserialize::<OwnedValue>().ok()).and_then(|v| bool::try_from(&v).ok())
        == Some(true)
}

fn disconnect(conn: &zbus::blocking::Connection, device: &OwnedObjectPath) {
    let _ = conn.call_method(Some("org.bluez"), device.as_str(), Some(DEVICE), "Disconnect", &());
}

/// Sends what the endpoint has queued, as notifications.
fn drain(conn: &zbus::blocking::Connection, endpoint: &BleEndpoint) {
    while let Some(fragment) = endpoint.next_outbound() {
        if let Err(error) = notify(conn, fragment) {
            tracing::debug!(%error, "Bluetooth link: couldn't notify");
            return;
        }
    }
}

/// The link thread: one phone at a time.
pub(super) fn run_links(conn: zbus::blocking::Connection, messages: Receiver<LinkMsg>, accept: Accept) {
    let mut current: Option<Current> = None;
    let tick = Duration::from_secs(2);
    loop {
        let message = match messages.recv_timeout(tick) {
            Ok(message) => Some(message),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        // BlueZ is asked only when a device changed or a while passed.
        let look = matches!(message, None | Some(LinkMsg::DeviceChanged));
        match message {
            Some(LinkMsg::Stop) => break,
            Some(LinkMsg::Write { device, mtu, value }) => {
                if current.as_ref().is_some_and(|c| c.device != device) {
                    // Another device while one is linked: not ours to answer.
                    if current.as_ref().is_some_and(|c| c.endpoint.is_up()) {
                        continue;
                    }
                    if let Some(old) = current.take() {
                        old.endpoint.close();
                        disconnect(&conn, &old.device);
                    }
                }
                if current.is_none() {
                    let Some(endpoint) = accept() else { continue };
                    current = Some(Current { device: device.clone(), endpoint, since: Instant::now() });
                }
                let Some(link) = current.as_ref() else { continue };
                if let Some(mtu) = mtu {
                    link.endpoint.set_mtu(mtu);
                }
                if let Err(error) = link.endpoint.receive(&value) {
                    tracing::info!(%error, "Bluetooth link ended");
                    disconnect(&conn, &link.device);
                    current = None;
                    continue;
                }
                drain(&conn, &link.endpoint);
            }
            Some(LinkMsg::Outbound) => {
                if let Some(link) = current.as_ref() {
                    drain(&conn, &link.endpoint);
                }
            }
            Some(LinkMsg::DeviceChanged) | None => {}
        }
        let done = current.as_ref().is_some_and(|link| {
            link.endpoint.is_closed()
                || (!link.endpoint.is_up() && link.since.elapsed() > HANDSHAKE_WITHIN)
                || (look && !connected(&conn, &link.device))
        });
        if done && let Some(link) = current.take() {
            link.endpoint.close();
            disconnect(&conn, &link.device);
        }
    }
    if let Some(link) = current.take() {
        link.endpoint.close();
        disconnect(&conn, &link.device);
    }
}

/// What the link thread is told on, and where it listens.
pub(super) fn channel() -> (LinkSender, Receiver<LinkMsg>) {
    mpsc::sync_channel(WRITE_QUEUE)
}

/// Starts the link thread on `conn`, taking what arrives on `messages`.
pub(super) fn spawn_links(
    conn: zbus::blocking::Connection,
    messages: Receiver<LinkMsg>,
    accept: Accept,
) -> Option<thread::JoinHandle<()>> {
    thread::Builder::new().name("ble-link".into()).spawn(move || run_links(conn, messages, accept)).ok()
}
