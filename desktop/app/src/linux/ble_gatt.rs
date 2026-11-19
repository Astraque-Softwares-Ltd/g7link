// SPDX-License-Identifier: GPL-3.0-or-later
//! The PC's side of the Bluetooth channel (docs/protocol/bluetooth.md): a GATT
//! service BlueZ serves for G7 Link, which a paired phone connects to while the
//! PC advertises that it can be connected to. The phone writes to RX and the
//! PC notifies on TX; each value is one fragment of a record, which the core's
//! [`BleEndpoint`] puts together, checks and answers. Writes are handed to the
//! `ble-link` thread as they come; BlueZ's call never waits on the core.
//!
//! Ending a link ends only its LE connection: a phone's device in BlueZ can
//! also carry its classic audio, which `Device1.Disconnect` would drop too.

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
/// A device's LE and BR/EDR connections (BlueZ 5.8x). Their properties and
/// methods may be experimental: the interfaces can be there without them.
const LE_BEARER: &str = "org.bluez.Bearer.LE1";
const BREDR_BEARER: &str = "org.bluez.Bearer.BREDR1";
const BLUEZ: &str = "org.bluez";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

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

/// What BlueZ says about the device a link came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DeviceFacts {
    /// `Device1.Connected`, over either transport.
    connected: bool,
    /// Paired or bonded with this PC.
    paired: bool,
    /// Its address is a random one, as a phone's usually is over LE.
    random_address: bool,
    /// BlueZ knows the device over BR/EDR too (it has `Bearer.BREDR1`).
    bredr: bool,
    /// It has `Bearer.LE1`.
    le_bearer: bool,
    /// `Bearer.LE1.Connected`, where BlueZ offers it.
    le_connected: Option<bool>,
}

impl DeviceFacts {
    /// Whether the device may carry more than this link: one the user
    /// paired, or one BlueZ also knows over BR/EDR, whose object is shared
    /// by both transports. `Device1.Connected` then says nothing about the
    /// link, and `Device1.Disconnect` would drop the phone's audio too.
    fn shared(&self) -> bool {
        self.paired || self.bredr
    }
}

/// `interface`'s properties on `device`; `None` when it doesn't have that
/// interface, or BlueZ no longer has the device.
fn properties(
    conn: &zbus::blocking::Connection,
    device: &OwnedObjectPath,
    interface: &str,
) -> Option<HashMap<String, OwnedValue>> {
    let reply =
        conn.call_method(Some(BLUEZ), device.as_str(), Some(PROPERTIES), "GetAll", &(interface,)).ok()?;
    reply.body().deserialize().ok()
}

/// What BlueZ says about `device`; `None` once it's gone.
fn device_facts(conn: &zbus::blocking::Connection, device: &OwnedObjectPath) -> Option<DeviceFacts> {
    let flag = |props: &HashMap<String, OwnedValue>, name: &str| {
        props.get(name).and_then(|v| bool::try_from(v).ok())
    };
    let props = properties(conn, device, DEVICE)?;
    let le = properties(conn, device, LE_BEARER);
    Some(DeviceFacts {
        connected: flag(&props, "Connected") == Some(true),
        paired: flag(&props, "Paired") == Some(true) || flag(&props, "Bonded") == Some(true),
        random_address: props.get("AddressType").and_then(|v| <&str>::try_from(v).ok()) == Some("random"),
        bredr: properties(conn, device, BREDR_BEARER).is_some(),
        le_bearer: le.is_some(),
        le_connected: le.as_ref().and_then(|props| flag(props, "Connected")),
    })
}

/// Whether the link on the device described by `facts` is gone. On a
/// device that also carries other connections, only `Bearer.LE1` can say,
/// where BlueZ offers it; without it the link lasts until the phone ends it
/// or starts a new one.
fn link_gone(facts: Option<&DeviceFacts>) -> bool {
    facts.is_none_or(|facts| !facts.connected || facts.le_connected == Some(false))
}

/// How a link's LE connection is ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hangup {
    /// `Device1.Disconnect`: the device holds only this link.
    Device,
    /// `Bearer.LE1.Disconnect`: the LE connection alone.
    LeBearer,
    /// The phone is sent an empty value, which ends the link on its side
    /// (docs/protocol/bluetooth.md §4.2), and it disconnects.
    AskPhone,
    /// Nothing is connected any more, or nothing may be done.
    Nothing,
}

/// How to end the LE connection of the device described by `facts`
/// without touching its others.
fn hangup_for(facts: Option<&DeviceFacts>) -> Hangup {
    let Some(facts) = facts.filter(|f| f.connected) else { return Hangup::Nothing };
    if !facts.shared() {
        return Hangup::Device;
    }
    match (facts.le_bearer, facts.le_connected) {
        (_, Some(false)) => Hangup::Nothing,
        (true, _) => Hangup::LeBearer,
        (false, _) => Hangup::AskPhone,
    }
}

/// Ends the LE connection with `device`, and only that: what it did. With
/// `may_ask` false (another device may be starting a link, and would get the
/// value too) the phone isn't sent the empty value.
fn hang_up(conn: &zbus::blocking::Connection, device: &OwnedObjectPath, may_ask: bool) -> Hangup {
    let facts = device_facts(conn, device);
    let call = |interface: &str| {
        conn.call_method(Some(BLUEZ), device.as_str(), Some(interface), "Disconnect", &()).is_ok()
    };
    let ask = || may_ask && notify(conn, Vec::new()).is_ok();
    let done = match hangup_for(facts.as_ref()) {
        Hangup::Device => {
            call(DEVICE);
            Hangup::Device
        }
        Hangup::LeBearer if call(LE_BEARER) => Hangup::LeBearer,
        // BlueZ may not offer the method (it's experimental): the phone is asked.
        Hangup::LeBearer | Hangup::AskPhone => {
            if ask() {
                Hangup::AskPhone
            } else {
                Hangup::Nothing
            }
        }
        Hangup::Nothing => Hangup::Nothing,
    };
    if facts.is_some_and(|f| f.shared()) {
        tracing::info!(how = ?done, "Bluetooth link: ended it without touching the phone's other connections");
    }
    done
}

/// Says what kind of device started a link, without its address.
fn log_start(conn: &zbus::blocking::Connection, device: &OwnedObjectPath) {
    match device_facts(conn, device) {
        Some(facts) => tracing::info!(
            address = if facts.random_address { "random" } else { "public" },
            paired = facts.paired,
            classic = facts.bredr,
            "Bluetooth link: a device started the handshake"
        ),
        None => tracing::info!("Bluetooth link: a device started the handshake"),
    }
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

/// Why `link` ends now, if it does; BlueZ is asked only when `look`.
fn why_ended(conn: &zbus::blocking::Connection, link: &Current, look: bool) -> Option<&'static str> {
    if link.endpoint.is_closed() {
        return Some("closed on this PC");
    }
    if !link.endpoint.is_up() && link.since.elapsed() > HANDSHAKE_WITHIN {
        return Some("the phone didn't finish the handshake in 10 s");
    }
    if look && link_gone(device_facts(conn, &link.device).as_ref()) {
        return Some("the phone disconnected");
    }
    None
}

/// Ends `link`: its end here, then its LE connection ([`hang_up`]).
fn end(conn: &zbus::blocking::Connection, link: Current, may_ask: bool) {
    link.endpoint.close();
    hang_up(conn, &link.device, may_ask);
}

/// A new end for a phone that starts again on the device of the current
/// link: it gave up on that one (an end that went unnoticed here, as its
/// device also carries other connections, or a handshake it timed out).
/// `None` when `value` doesn't start one.
fn restart(accept: &Accept, mtu: Option<u16>, value: &[u8]) -> Option<Arc<BleEndpoint>> {
    let endpoint = accept()?;
    if let Some(mtu) = mtu {
        endpoint.set_mtu(mtu);
    }
    endpoint.receive(value).ok()?;
    Some(endpoint)
}

/// The link thread: one phone at a time.
pub(super) fn run_links(conn: zbus::blocking::Connection, messages: Receiver<LinkMsg>, accept: Accept) {
    let mut current: Option<Current> = None;
    let tick = Duration::from_secs(2);
    let mut looked = Instant::now();
    loop {
        let message = match messages.recv_timeout(tick) {
            Ok(message) => Some(message),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        // BlueZ is asked when a device changed, and every 2 s however busy the link is.
        let look = matches!(message, Some(LinkMsg::DeviceChanged)) || looked.elapsed() >= tick;
        if look {
            looked = Instant::now();
        }
        match message {
            Some(LinkMsg::Stop) => break,
            Some(LinkMsg::Write { device, mtu, value }) => {
                if current.as_ref().is_some_and(|c| c.device != device) {
                    // Another device while one is linked: not ours to answer.
                    if current.as_ref().is_some_and(|c| c.endpoint.is_up()) {
                        continue;
                    }
                    if let Some(old) = current.take() {
                        tracing::info!("Bluetooth link ended: another device started one before it was up");
                        end(&conn, old, false);
                    }
                }
                let started = current.is_none();
                if started {
                    let Some(endpoint) = accept() else { continue };
                    log_start(&conn, &device);
                    current = Some(Current { device: device.clone(), endpoint, since: Instant::now() });
                }
                let Some(link) = current.as_ref() else { continue };
                if let Some(mtu) = mtu {
                    link.endpoint.set_mtu(mtu);
                }
                if let Err(error) = link.endpoint.receive(&value) {
                    if !started && let Some(endpoint) = restart(&accept, mtu, &value) {
                        tracing::info!("Bluetooth link ended: the phone started a new one");
                        drain(&conn, &endpoint);
                        current = Some(Current { device, endpoint, since: Instant::now() });
                        continue;
                    }
                    tracing::info!(%error, "Bluetooth link ended: what the phone sent was refused");
                    if let Some(link) = current.take() {
                        end(&conn, link, true);
                    }
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
        let ended = current.as_ref().and_then(|link| why_ended(&conn, link, look));
        if let Some(why) = ended
            && let Some(link) = current.take()
        {
            tracing::info!("Bluetooth link ended: {why}");
            end(&conn, link, true);
        }
    }
    if let Some(link) = current.take() {
        end(&conn, link, true);
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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A phone LE-only to BlueZ: its own object, as its random address isn't resolved.
    const STRANGER: DeviceFacts = DeviceFacts {
        connected: true,
        paired: false,
        random_address: true,
        bredr: false,
        le_bearer: true,
        le_connected: None,
    };
    /// The user's paired phone, its classic audio connected: BlueZ shares the object.
    const PAIRED_PHONE: DeviceFacts = DeviceFacts {
        connected: true,
        paired: true,
        random_address: false,
        bredr: true,
        le_bearer: false,
        le_connected: None,
    };

    #[test]
    fn ending_a_link_never_disconnects_a_device_that_carries_more() {
        assert_eq!(hangup_for(Some(&STRANGER)), Hangup::Device);
        assert_eq!(hangup_for(Some(&DeviceFacts { le_bearer: false, ..STRANGER })), Hangup::Device);
        assert_eq!(hangup_for(Some(&PAIRED_PHONE)), Hangup::AskPhone);
        let with_le = DeviceFacts { le_bearer: true, ..PAIRED_PHONE };
        assert_eq!(hangup_for(Some(&with_le)), Hangup::LeBearer);
        assert_eq!(hangup_for(Some(&DeviceFacts { le_connected: Some(true), ..with_le })), Hangup::LeBearer);
        assert_eq!(hangup_for(Some(&DeviceFacts { le_connected: Some(false), ..with_le })), Hangup::Nothing);
        let classic_only = DeviceFacts { paired: false, ..PAIRED_PHONE };
        assert_eq!(hangup_for(Some(&classic_only)), Hangup::AskPhone, "known over BR/EDR, paired or not");
        assert_eq!(hangup_for(Some(&DeviceFacts { connected: false, ..STRANGER })), Hangup::Nothing);
        assert_eq!(hangup_for(None), Hangup::Nothing, "BlueZ dropped the device");
    }

    #[test]
    fn a_device_that_stays_connected_does_not_keep_the_link_up() {
        assert!(!link_gone(Some(&STRANGER)));
        assert!(link_gone(Some(&DeviceFacts { connected: false, ..STRANGER })));
        assert!(link_gone(None));
        // Its classic audio keeps the device connected; the LE bearer says the link went.
        let le = DeviceFacts { le_bearer: true, le_connected: Some(true), ..PAIRED_PHONE };
        assert!(!link_gone(Some(&le)));
        assert!(link_gone(Some(&DeviceFacts { le_connected: Some(false), ..le })));
        assert!(!link_gone(Some(&PAIRED_PHONE)), "nothing says it went: it lasts until the phone ends it");
    }

    /// A private bus from `dbus-daemon`, stopped when dropped.
    struct TestBus {
        daemon: std::process::Child,
        address: String,
    }

    impl TestBus {
        fn start() -> Option<Self> {
            use std::io::BufRead;
            let mut daemon = std::process::Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address"])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .ok()?;
            let mut address = String::new();
            std::io::BufReader::new(daemon.stdout.take()?).read_line(&mut address).ok()?;
            Some(Self { daemon, address: address.trim().to_owned() })
        }

        fn connect(&self) -> zbus::blocking::connection::Builder<'static> {
            zbus::blocking::connection::Builder::address(self.address.as_str()).unwrap()
        }
    }

    impl Drop for TestBus {
        fn drop(&mut self) {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
    }

    type Calls = Arc<Mutex<Vec<&'static str>>>;

    /// A device as BlueZ serves it, recording the disconnects asked of it.
    #[derive(Clone)]
    struct FakeDevice {
        calls: Calls,
        paired: bool,
        address_type: &'static str,
    }

    #[zbus::interface(name = "org.bluez.Device1")]
    impl FakeDevice {
        #[zbus(property)]
        fn connected(&self) -> bool {
            true
        }

        #[zbus(property)]
        fn paired(&self) -> bool {
            self.paired
        }

        #[zbus(property)]
        fn address_type(&self) -> String {
            self.address_type.to_owned()
        }

        fn disconnect(&self) {
            self.calls.lock().unwrap().push("Device1.Disconnect");
        }
    }

    /// `Bearer.BREDR1` with the classic audio up.
    struct FakeBredr;

    #[zbus::interface(name = "org.bluez.Bearer.BREDR1")]
    impl FakeBredr {
        #[zbus(property)]
        fn connected(&self) -> bool {
            true
        }
    }

    /// `Bearer.LE1` as an experimental BlueZ offers it.
    struct FakeLe(Calls);

    #[zbus::interface(name = "org.bluez.Bearer.LE1")]
    impl FakeLe {
        #[zbus(property)]
        fn connected(&self) -> bool {
            true
        }

        fn disconnect(&self) {
            self.0.lock().unwrap().push("Bearer.LE1.Disconnect");
        }
    }

    const STRANGER_PATH: &str = "/org/bluez/hci0/dev_5A_11_22_33_44_55";
    const PHONE_PATH: &str = "/org/bluez/hci0/dev_B8_00_00_00_00_01";
    const PHONE_WITH_LE_PATH: &str = "/org/bluez/hci0/dev_B8_00_00_00_00_02";

    /// BlueZ with a stranger LE device, the paired phone (classic up, no LE
    /// bearer methods, as BlueZ 5.85 offers them), and the paired phone on a
    /// BlueZ that offers `Bearer.LE1`.
    fn fake_bluez(bus: &TestBus, calls: &Calls) -> zbus::blocking::Connection {
        let device = |paired, address_type| FakeDevice { calls: calls.clone(), paired, address_type };
        bus.connect()
            .name(BLUEZ)
            .unwrap()
            .serve_at(STRANGER_PATH, device(false, "random"))
            .unwrap()
            .serve_at(STRANGER_PATH, FakeLe(calls.clone()))
            .unwrap()
            .serve_at(PHONE_PATH, device(true, "public"))
            .unwrap()
            .serve_at(PHONE_PATH, FakeBredr)
            .unwrap()
            .serve_at(PHONE_WITH_LE_PATH, device(true, "public"))
            .unwrap()
            .serve_at(PHONE_WITH_LE_PATH, FakeBredr)
            .unwrap()
            .serve_at(PHONE_WITH_LE_PATH, FakeLe(calls.clone()))
            .unwrap()
            .build()
            .unwrap()
    }

    #[test]
    fn a_link_on_the_paired_phone_ends_without_dropping_its_audio() {
        let Some(bus) = TestBus::start() else {
            eprintln!("skipped: dbus-daemon isn't installed");
            return;
        };
        let calls = Calls::default();
        let _bluez = fake_bluez(&bus, &calls);
        let conn = bus.connect().build().unwrap();
        let path = |p: &str| OwnedObjectPath::try_from(p).unwrap();
        let take = || std::mem::take(&mut *calls.lock().unwrap());

        let phone = device_facts(&conn, &path(PHONE_PATH)).unwrap();
        assert!(phone.shared() && phone.bredr && !phone.le_bearer && !phone.random_address);
        assert_eq!(hang_up(&conn, &path(PHONE_PATH), true), Hangup::AskPhone);
        assert_eq!(take(), Vec::<&str>::new(), "no Device1.Disconnect");
        assert_eq!(hang_up(&conn, &path(PHONE_PATH), false), Hangup::Nothing, "another device is starting");
        assert_eq!(take(), Vec::<&str>::new());

        assert_eq!(hang_up(&conn, &path(PHONE_WITH_LE_PATH), true), Hangup::LeBearer);
        assert_eq!(take(), ["Bearer.LE1.Disconnect"]);

        let stranger = device_facts(&conn, &path(STRANGER_PATH)).unwrap();
        assert_eq!(stranger.le_connected, Some(true));
        assert!(!stranger.shared() && stranger.random_address);
        assert_eq!(hang_up(&conn, &path(STRANGER_PATH), true), Hangup::Device);
        assert_eq!(take(), ["Device1.Disconnect"]);

        let gone = path("/org/bluez/hci0/dev_00_00_00_00_00_00");
        assert_eq!(device_facts(&conn, &gone), None);
        assert!(link_gone(None));
        assert_eq!(hang_up(&conn, &gone, true), Hangup::Nothing);
        assert_eq!(take(), Vec::<&str>::new());
    }
}
