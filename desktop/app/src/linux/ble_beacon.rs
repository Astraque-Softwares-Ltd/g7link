// SPDX-License-Identifier: GPL-3.0-or-later
//! Bluetooth through BlueZ over D-Bus: the PC's rotating beacon
//! (docs/protocol/bluetooth.md), so paired phones set to "Wake when PC is
//! nearby" find it. Each paired phone recognises its own six-byte tag, made
//! from the secret the two share and the time: nothing in the beacon stays
//! the same from one 15-minute window to the next, and with no phone to
//! broadcast for there is no beacon. While a paired phone's app still looks
//! for the old beacon (it doesn't announce `ble.beacon2`), that one is
//! broadcast too. Never spawns `bluetoothctl` or starts a scan of its own.
//!
//! For phones the user lets use Bluetooth, it also serves the PC's GATT
//! service ([`super::ble_gatt`]). A connectable advertisement shows the PC's
//! public address, so the PC can be connected to only in short windows opened
//! for a reason ([`open_window`]): it then advertises a "connect" tag for the
//! phones wanted instead of the beacon.
//!
//! The beacon follows BlueZ as it reports changes: it is broadcast as soon as
//! BlueZ starts (also when that is after G7 Link), again after `bluetoothd`
//! restarts, and when Bluetooth is turned back on or the PC resumes.

use std::collections::HashMap;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use g7link_core::{
    DeviceId, NodeEvent,
    ble::{self, BEACON_SLOTS, TAG_LEN, TagKind, WINDOW_SLOTS},
};

use super::ble_gatt::{self, LinkMsg, LinkSender};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

pub const COMPANY_ID: u16 = ble::COMPANY_ID;
/// The old beacon's first bytes, followed by 8 bytes of the device ID.
pub const MAGIC: &[u8; 5] = b"NCTR\x01";
/// What a phone whose app recognises the rotating beacon announces.
pub const BEACON2: &str = "ble.beacon2";

/// How often the beacon is checked, in case a change in BlueZ went
/// unreported, and for its next window.
const BEACON_CHECK_INTERVAL: Duration = Duration::from_secs(15);
/// With more phones than fit in one beacon, each set is broadcast this long in turn.
const TURN_SECS: u64 = 20;
/// The beacon changes up to this long after a window starts, a different
/// time on each run, so PCs don't change in step.
const MAX_ROTATE_DELAY_SECS: u64 = 60;
/// The beacon's advertising interval: phones see it within a fraction of a
/// second, where the controller's default is about 1.28 s.
const MIN_INTERVAL_MS: u32 = 100;
const MAX_INTERVAL_MS: u32 = 200;
const DBUS_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the signal watcher gets to subscribe, and to stop.
const WATCHER_TIMEOUT: Duration = Duration::from_secs(2);

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const ADVERTISING_MANAGER: &str = "org.bluez.LEAdvertisingManager1";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
const ADVERTISEMENT_PATH: &str = "/app/g7link/beacon";
/// The same beacon at the controller's default interval, for a BlueZ or a
/// controller that refuses the faster one.
const DEFAULT_INTERVAL_PATH: &str = "/app/g7link/beacon_default_interval";
/// The old beacon, for phones whose app doesn't know the new one yet.
const LEGACY_PATH: &str = "/app/g7link/beacon_legacy";
/// The advertisement a phone can connect to, during a window.
const CONNECT_PATH: &str = "/app/g7link/connect";
const GATT_MANAGER: &str = "org.bluez.GattManager1";
/// Where the signal watcher is told to stop.
const WATCHER_INTERFACE: &str = "app.g7link.BeaconWatcher";

static WORKER: Mutex<Option<Worker>> = Mutex::new(None);
/// The link thread of the running worker.
static LINK: Mutex<Option<LinkSender>> = Mutex::new(None);
/// An adapter here can serve the GATT service, so phones may use Bluetooth with this PC.
static CAN_LINK: AtomicBool = AtomicBool::new(false);

type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

/// The old beacon: [`MAGIC`] and the first 8 bytes of the device ID.
pub fn legacy_payload(device_id: &DeviceId) -> [u8; 13] {
    let mut out = [0u8; 13];
    out[0..5].copy_from_slice(MAGIC);
    out[5..13].copy_from_slice(&device_id.as_bytes()[0..8]);
    out
}

/// Who the beacon is for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BeaconPhones {
    /// The beacon key of each paired phone this PC shares a secret with.
    keys: Vec<[u8; 32]>,
    /// Some paired phone's app looks for the old beacon.
    legacy: bool,
    /// Some paired phone may use Bluetooth with this PC: the GATT service is served.
    linkable: bool,
}

/// The paired phones the beacon is for, as the core knows them.
fn paired_phones() -> BeaconPhones {
    let Some(node) = crate::core_host::node() else { return BeaconPhones::default() };
    let keys = node.ble_beacon_keys().into_iter().map(|(_, key)| key).collect();
    let phones: Vec<DeviceId> = node
        .paired_devices()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| {
            matches!(d.info.kind, g7link_core::DeviceKind::Phone | g7link_core::DeviceKind::Tablet)
                || d.info.os == "android"
        })
        .map(|d| d.id)
        .collect();
    let legacy = phones.iter().any(|&id| !node.peer_offers(id, BEACON2));
    let linkable = phones.iter().any(|&id| node.ble_usable(id));
    BeaconPhones { keys, legacy, linkable }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

pub fn start(device_id: DeviceId) {
    let env = Env {
        connect: Arc::new(zbus::blocking::connection::Builder::system),
        phones: Arc::new(paired_phones),
        now: unix_now,
        accept: Arc::new(|| crate::core_host::node().map(|node| node.ble_accept())),
    };
    start_in(&WORKER, env, device_id);
}

pub fn stop() {
    stop_in(&WORKER);
}

/// Who the beacon is for may have changed: it's looked at again now.
pub fn phones_changed() {
    if let Some(worker) = WORKER.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        worker.wake.set(|w| w.resync = true);
    }
}

/// Follows the core's events that change who the beacon is for, and hands
/// the link thread what the core has to send.
pub fn on_event(event: &NodeEvent) {
    match event {
        NodeEvent::BleKeysChanged
        | NodeEvent::Capabilities(_)
        | NodeEvent::DeviceAdded(_)
        | NodeEvent::DeviceRemoved(_)
        | NodeEvent::DeviceToggleChanged { .. } => phones_changed(),
        NodeEvent::BleOutbound { .. } => tell_link(LinkMsg::Outbound),
        _ => {}
    }
}

fn tell_link(message: LinkMsg) {
    if let Some(link) = LINK.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = link.try_send(message);
    }
}

/// Whether an adapter here can serve the GATT service a phone connects to.
pub fn can_link() -> bool {
    CAN_LINK.load(Ordering::Relaxed)
}

/// Lets the phones whose beacon keys are `keys` connect over Bluetooth for
/// `open`: the PC advertises their "connect" tags, and can be connected to,
/// until the window closes or a link is up.
pub fn open_window(keys: Vec<[u8; 32]>, open: Duration) {
    if keys.is_empty() {
        return;
    }
    if let Some(worker) = WORKER.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        *worker.wake.window.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(Window { keys, until: Instant::now() + open });
        worker.wake.set(|w| w.resync = true);
    }
}

/// Ends the window early (a phone connected).
pub fn close_window() {
    if let Some(worker) = WORKER.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let closed = worker.wake.window.lock().unwrap_or_else(|e| e.into_inner()).take().is_some();
        if closed {
            worker.wake.set(|w| w.resync = true);
        }
    }
}

fn start_in(slot: &Mutex<Option<Worker>>, env: Env, device_id: DeviceId) {
    let mut worker = slot.lock().unwrap_or_else(|e| e.into_inner());
    if worker.is_none() {
        *worker = Worker::spawn(env, device_id);
    }
}

fn stop_in(slot: &Mutex<Option<Worker>>) {
    let worker = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(worker) = worker {
        worker.stop();
    }
}

/// What the worker is woken for.
#[derive(Debug, Default, Clone, Copy)]
struct Wakeup {
    stopped: bool,
    /// Something in BlueZ changed: look again now.
    resync: bool,
    /// BlueZ started, stopped or restarted, so it holds no registration.
    bluez_changed: bool,
    /// A device connected or disconnected.
    devices: bool,
}

impl Wakeup {
    fn pending(&self) -> bool {
        self.stopped || self.resync || self.bluez_changed || self.devices
    }
}

/// A time the PC can be connected to, and the phones it's for.
#[derive(Debug, Clone)]
struct Window {
    keys: Vec<[u8; 32]>,
    until: Instant,
}

#[derive(Debug, Default)]
struct Wake {
    state: Mutex<Wakeup>,
    cv: Condvar,
    window: Mutex<Option<Window>>,
}

impl Wake {
    fn set(&self, change: impl FnOnce(&mut Wakeup)) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        change(&mut state);
        drop(state);
        self.cv.notify_all();
    }

    fn stopped(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).stopped
    }

    /// The window, while it's open.
    fn window(&self) -> Option<Window> {
        let mut window = self.window.lock().unwrap_or_else(|e| e.into_inner());
        if window.as_ref().is_some_and(|w| w.until <= Instant::now()) {
            *window = None;
        }
        window.clone()
    }

    /// Waits until woken or `timeout` passes, and takes what it was woken for.
    fn wait(&self, timeout: Duration) -> Wakeup {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let waited = self.cv.wait_timeout_while(state, timeout, |w| !w.pending());
        let (mut state, _) = waited.unwrap_or_else(|e| e.into_inner());
        let woken = *state;
        state.resync = false;
        state.bluez_changed = false;
        state.devices = false;
        woken
    }
}

/// The rotating beacon as BlueZ reads it: a non-connectable broadcast that
/// carries only the manufacturer data. BlueZ reads it when it's registered,
/// so a new payload means registering it again.
#[derive(Clone)]
struct Advertisement {
    payload: Arc<Mutex<Vec<u8>>>,
    released: Arc<AtomicBool>,
    wake: Arc<Wake>,
}

impl Advertisement {
    /// BlueZ dropped the beacon (its adapter went away); it is registered again.
    fn on_release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.wake.set(|w| w.resync = true);
    }

    fn data(&self) -> HashMap<u16, OwnedValue> {
        let payload = self.payload.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let bytes = OwnedValue::try_from(Value::from(payload)).expect("byte arrays hold no fds");
        HashMap::from([(COMPANY_ID, bytes)])
    }
}

/// The beacon every 100-200 ms.
struct FastAdvertisement(Advertisement);

#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl FastAdvertisement {
    fn release(&self) {
        self.0.on_release();
    }

    #[zbus(property, name = "Type")]
    fn kind(&self) -> String {
        "broadcast".to_owned()
    }

    #[zbus(property)]
    fn manufacturer_data(&self) -> HashMap<u16, OwnedValue> {
        self.0.data()
    }

    #[zbus(property)]
    fn min_interval(&self) -> u32 {
        MIN_INTERVAL_MS
    }

    #[zbus(property)]
    fn max_interval(&self) -> u32 {
        MAX_INTERVAL_MS
    }
}

/// The beacon at the controller's default interval.
struct DefaultIntervalAdvertisement(Advertisement);

#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl DefaultIntervalAdvertisement {
    fn release(&self) {
        self.0.on_release();
    }

    #[zbus(property, name = "Type")]
    fn kind(&self) -> String {
        "broadcast".to_owned()
    }

    #[zbus(property)]
    fn manufacturer_data(&self) -> HashMap<u16, OwnedValue> {
        self.0.data()
    }
}

/// The old beacon, at the controller's default interval.
struct LegacyAdvertisement(Advertisement);

#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl LegacyAdvertisement {
    fn release(&self) {
        self.0.on_release();
    }

    #[zbus(property, name = "Type")]
    fn kind(&self) -> String {
        "broadcast".to_owned()
    }

    #[zbus(property)]
    fn manufacturer_data(&self) -> HashMap<u16, OwnedValue> {
        self.0.data()
    }
}

/// The advertisement a phone can connect to, during a window: BlueZ adds the
/// flags and makes it connectable.
struct ConnectAdvertisement(Advertisement);

#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl ConnectAdvertisement {
    fn release(&self) {
        self.0.on_release();
    }

    #[zbus(property, name = "Type")]
    fn kind(&self) -> String {
        "peripheral".to_owned()
    }

    #[zbus(property)]
    fn manufacturer_data(&self) -> HashMap<u16, OwnedValue> {
        self.0.data()
    }

    #[zbus(property)]
    fn discoverable(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn min_interval(&self) -> u32 {
        MIN_INTERVAL_MS
    }

    #[zbus(property)]
    fn max_interval(&self) -> u32 {
        MAX_INTERVAL_MS
    }
}

/// What the rotating beacon holds while it's registered.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shown {
    /// Presence, or "connect" during a window.
    kind: TagKind,
    epoch: u64,
    /// Which set of phones, when they don't all fit.
    turn: u64,
    keys: Vec<[u8; 32]>,
}

/// What the rotating beacon should hold at `now` for `phones`, or for the
/// window's phones while one is open; `None` when there's no one to
/// broadcast for.
fn wanted(phones: &BeaconPhones, window: Option<&Window>, now: u64, delay: u64) -> Option<Shown> {
    if let Some(window) = window {
        let epoch = ble::epoch_at(now);
        let keys = window.keys.iter().take(WINDOW_SLOTS).copied().collect();
        return Some(Shown { kind: TagKind::Connect, epoch, turn: 0, keys });
    }
    if phones.keys.is_empty() {
        return None;
    }
    let epoch = ble::epoch_at(now.saturating_sub(delay));
    let turns = u64::try_from(phones.keys.len().div_ceil(BEACON_SLOTS)).unwrap_or(1);
    let turn = (now / TURN_SECS) % turns;
    let first = usize::try_from(turn).unwrap_or(0) * BEACON_SLOTS;
    let keys = phones.keys.iter().skip(first).take(BEACON_SLOTS).copied().collect();
    Some(Shown { kind: TagKind::Presence, epoch, turn, keys })
}

/// `0..n` in random order.
fn shuffled(n: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        order.swap(i, rand::random_range(0..=i));
    }
    order
}

/// The manufacturer data for `shown`: a tag per phone and random bytes for
/// the rest, in random order.
fn beacon_payload(shown: &Shown) -> Vec<u8> {
    let tags: Vec<[u8; TAG_LEN]> =
        shown.keys.iter().map(|key| ble::tag(key, shown.kind, shown.epoch)).collect();
    // A connectable advertisement has BlueZ's flags too: one slot fewer fits.
    let slots = if shown.kind == TagKind::Connect { WINDOW_SLOTS } else { BEACON_SLOTS };
    let filler: [u8; BEACON_SLOTS * TAG_LEN] = rand::random();
    ble::payload(&tags, slots, &filler, &shuffled(slots))
}

/// Keeps the beacons registered with the first powered adapter that can
/// advertise, following adapters as they are turned off and on, and the
/// rotating one's payload as windows pass and phones come and go.
struct Beacon {
    adapter: Option<OwnedObjectPath>,
    payload: Arc<Mutex<Vec<u8>>>,
    released: Arc<AtomicBool>,
    legacy_released: Arc<AtomicBool>,
    /// What the rotating beacon holds, while it's registered.
    shown: Option<Shown>,
    /// The old beacon is registered.
    legacy: bool,
    /// The GATT service is registered with the adapter.
    gatt: bool,
    /// BlueZ refused the faster interval, so the beacon goes without it.
    default_interval: bool,
    /// How long after a window starts the beacon changes, on this run.
    delay: u64,
    last_error: Option<String>,
}

impl Beacon {
    /// Connects with the beacons served. They have to be served from the
    /// builder: adding them to a connection afterwards needs a tokio runtime.
    fn serve(
        builder: zbus::blocking::connection::Builder<'_>,
        device_id: &DeviceId,
        wake: Arc<Wake>,
        to_link: LinkSender,
    ) -> zbus::Result<(zbus::blocking::Connection, Self)> {
        let released = Arc::new(AtomicBool::new(false));
        let legacy_released = Arc::new(AtomicBool::new(false));
        let payload = Arc::new(Mutex::new(Vec::new()));
        let advertisement =
            Advertisement { payload: payload.clone(), released: released.clone(), wake: wake.clone() };
        let legacy = Advertisement {
            payload: Arc::new(Mutex::new(legacy_payload(device_id).to_vec())),
            released: legacy_released.clone(),
            wake,
        };
        let conn = builder
            .serve_at(ADVERTISEMENT_PATH, FastAdvertisement(advertisement.clone()))?
            .serve_at(DEFAULT_INTERVAL_PATH, DefaultIntervalAdvertisement(advertisement.clone()))?
            .serve_at(LEGACY_PATH, LegacyAdvertisement(legacy))?
            .serve_at(CONNECT_PATH, ConnectAdvertisement(advertisement))?
            .serve_at(ble_gatt::APP_PATH, zbus::fdo::ObjectManager)?
            .serve_at(ble_gatt::SERVICE_PATH, ble_gatt::Service)?
            .serve_at(ble_gatt::RX_PATH, ble_gatt::Rx { to_link })?
            .serve_at(ble_gatt::TX_PATH, ble_gatt::Tx)?
            .build()?;
        let beacon = Self {
            adapter: None,
            payload,
            released,
            legacy_released,
            shown: None,
            legacy: false,
            gatt: false,
            default_interval: false,
            delay: rand::random_range(0..=MAX_ROTATE_DELAY_SECS),
            last_error: None,
        };
        Ok((conn, beacon))
    }

    /// Where the advertisement for `kind` is served.
    fn path(&self, kind: TagKind) -> &'static str {
        match kind {
            TagKind::Connect => CONNECT_PATH,
            TagKind::Presence if self.default_interval => DEFAULT_INTERVAL_PATH,
            TagKind::Presence => ADVERTISEMENT_PATH,
        }
    }

    /// BlueZ went away, and its registrations with it.
    fn forget(&mut self) {
        self.adapter = None;
        self.shown = None;
        self.legacy = false;
        self.gatt = false;
    }

    /// Says a failure once, however often it's tried again.
    fn failed(&mut self, error: &zbus::Error) {
        let error = error.to_string();
        if self.last_error.as_ref() != Some(&error) {
            tracing::warn!(%error, "this PC can't broadcast its Bluetooth beacon");
            self.last_error = Some(error);
        }
    }

    fn sync(
        &mut self,
        conn: &zbus::blocking::Connection,
        objects: &ManagedObjects,
        phones: &BeaconPhones,
        window: Option<&Window>,
        now: u64,
    ) {
        if self.released.swap(false, Ordering::SeqCst) {
            self.shown = None;
        }
        if self.legacy_released.swap(false, Ordering::SeqCst) {
            self.legacy = false;
        }
        let adapter = advertising_adapter(objects);
        if self.adapter != adapter {
            if let Some(previous) = self.adapter.take() {
                self.unregister_all(conn, &previous);
            }
            self.adapter = adapter;
        }
        let Some(adapter) = self.adapter.clone() else {
            return;
        };

        let can_gatt = objects.get(&adapter).is_some_and(|ifaces| ifaces.contains_key(GATT_MANAGER));
        if CAN_LINK.swap(can_gatt, Ordering::Relaxed) != can_gatt {
            crate::core_host::refresh_capabilities();
        }
        let gatt_wanted = phones.linkable && can_gatt;
        if gatt_wanted && !self.gatt {
            match register_application(conn, &adapter) {
                Ok(()) => self.gatt = true,
                Err(error) => self.failed(&error),
            }
        } else if !gatt_wanted && self.gatt {
            unregister_application(conn, &adapter);
            self.gatt = false;
        }
        // A window needs the service to connect to.
        let window = window.filter(|_| self.gatt);

        let wanted = wanted(phones, window, now, self.delay);
        if wanted != self.shown {
            if let Some(shown) = self.shown.take() {
                unregister(conn, &adapter, self.path(shown.kind));
            }
            if let Some(wanted) = wanted {
                *self.payload.lock().unwrap_or_else(|e| e.into_inner()) = beacon_payload(&wanted);
                let registered = match wanted.kind {
                    TagKind::Connect => register_advertisement(conn, &adapter, CONNECT_PATH),
                    TagKind::Presence => self.register(conn, &adapter),
                };
                match registered {
                    Ok(()) => {
                        self.shown = Some(wanted);
                        self.last_error = None;
                    }
                    Err(error) => self.failed(&error),
                }
            }
        }

        if phones.legacy && !self.legacy {
            match register_advertisement(conn, &adapter, LEGACY_PATH) {
                Ok(()) => {
                    self.legacy = true;
                    tracing::info!("broadcasting the old Bluetooth beacon for a phone that needs it");
                }
                Err(error) => self.failed(&error),
            }
        } else if !phones.legacy && self.legacy {
            unregister(conn, &adapter, LEGACY_PATH);
            self.legacy = false;
        }
    }

    fn register(&mut self, conn: &zbus::blocking::Connection, adapter: &OwnedObjectPath) -> zbus::Result<()> {
        if !self.default_interval {
            let Err(error) = register_advertisement(conn, adapter, ADVERTISEMENT_PATH) else {
                tracing::debug!(
                    adapter = adapter.as_str(),
                    "broadcasting the Bluetooth beacon every 100-200 ms"
                );
                return Ok(());
            };
            // An older BlueZ, or a controller that can't advertise that often.
            if register_advertisement(conn, adapter, DEFAULT_INTERVAL_PATH).is_err() {
                return Err(error);
            }
            self.default_interval = true;
            tracing::info!(
                adapter = adapter.as_str(),
                %error,
                "broadcasting the Bluetooth beacon at the default interval: 100-200 ms was refused"
            );
            return Ok(());
        }
        register_advertisement(conn, adapter, DEFAULT_INTERVAL_PATH)
    }

    fn unregister_all(&mut self, conn: &zbus::blocking::Connection, adapter: &OwnedObjectPath) {
        if let Some(shown) = self.shown.take() {
            unregister(conn, adapter, self.path(shown.kind));
        }
        if self.legacy {
            unregister(conn, adapter, LEGACY_PATH);
            self.legacy = false;
        }
        if self.gatt {
            unregister_application(conn, adapter);
            self.gatt = false;
        }
    }

    fn stop(&mut self, conn: &zbus::blocking::Connection) {
        if let Some(adapter) = self.adapter.take() {
            self.unregister_all(conn, &adapter);
        }
    }
}

fn register_advertisement(
    conn: &zbus::blocking::Connection,
    adapter: &OwnedObjectPath,
    path: &'static str,
) -> zbus::Result<()> {
    let options: HashMap<&str, Value<'_>> = HashMap::new();
    let path = ObjectPath::from_static_str_unchecked(path);
    conn.call_method(
        Some(BLUEZ),
        adapter,
        Some(ADVERTISING_MANAGER),
        "RegisterAdvertisement",
        &(path, options),
    )?;
    Ok(())
}

/// Asks BlueZ to serve the G7 Link GATT service, which it reads from
/// [`ble_gatt::APP_PATH`]'s object manager.
fn register_application(conn: &zbus::blocking::Connection, adapter: &OwnedObjectPath) -> zbus::Result<()> {
    let options: HashMap<&str, Value<'_>> = HashMap::new();
    let path = ObjectPath::from_static_str_unchecked(ble_gatt::APP_PATH);
    conn.call_method(Some(BLUEZ), adapter, Some(GATT_MANAGER), "RegisterApplication", &(path, options))?;
    Ok(())
}

fn unregister_application(conn: &zbus::blocking::Connection, adapter: &OwnedObjectPath) {
    let path = ObjectPath::from_static_str_unchecked(ble_gatt::APP_PATH);
    let _ = conn.call_method(Some(BLUEZ), adapter, Some(GATT_MANAGER), "UnregisterApplication", &(path,));
}

fn unregister(conn: &zbus::blocking::Connection, adapter: &OwnedObjectPath, path: &'static str) {
    let path = ObjectPath::from_static_str_unchecked(path);
    // Fails when the adapter is gone, which takes the beacon with it.
    let _ = conn.call_method(
        Some(BLUEZ),
        adapter,
        Some(ADVERTISING_MANAGER),
        "UnregisterAdvertisement",
        &(path,),
    );
}

/// The first powered adapter (by path, so the choice is stable) that can
/// advertise.
fn advertising_adapter(objects: &ManagedObjects) -> Option<OwnedObjectPath> {
    objects
        .iter()
        .filter(|(_, ifaces)| {
            ifaces.contains_key(ADVERTISING_MANAGER)
                && ifaces
                    .get(ADAPTER)
                    .and_then(|props| props.get("Powered"))
                    .and_then(|v| bool::try_from(v).ok())
                    .unwrap_or(false)
        })
        .map(|(path, _)| path)
        .min_by(|a, b| a.as_str().cmp(b.as_str()))
        .cloned()
}

fn managed_objects(conn: &zbus::blocking::Connection) -> Option<ManagedObjects> {
    let reply = conn.call_method(Some(BLUEZ), "/", Some(OBJECT_MANAGER), "GetManagedObjects", &()).ok()?;
    reply.body().deserialize::<ManagedObjects>().ok()
}

/// Where the worker connects (the system bus, or a test's own) and what it reads.
type Connect = Arc<dyn Fn() -> zbus::Result<zbus::blocking::connection::Builder<'static>> + Send + Sync>;

struct Env {
    connect: Connect,
    /// Who the beacon is for.
    phones: Arc<dyn Fn() -> BeaconPhones + Send + Sync>,
    /// Unix seconds.
    now: fn() -> u64,
    /// A new end for a phone that starts a Bluetooth link.
    accept: ble_gatt::Accept,
}

struct Worker {
    wake: Arc<Wake>,
    thread: JoinHandle<()>,
}

impl Worker {
    fn spawn(env: Env, device_id: DeviceId) -> Option<Self> {
        let wake = Arc::new(Wake::default());
        let worker_wake = wake.clone();
        let thread = thread::Builder::new()
            .name("ble-beacon".into())
            .spawn(move || run_worker(&env, &device_id, &worker_wake))
            .ok()?;
        Some(Self { wake, thread })
    }

    fn stop(self) {
        self.wake.set(|w| w.stopped = true);
        let _ = self.thread.join();
    }
}

/// Connects with the beacons served, trying again until it connects or is
/// stopped.
/// Connects with the beacons and the GATT service served, trying again until
/// it connects or is stopped; also gives what the service's writes arrive on.
fn connect(
    env: &Env,
    device_id: &DeviceId,
    wake: &Arc<Wake>,
) -> Option<(zbus::blocking::Connection, Beacon, LinkSender, mpsc::Receiver<LinkMsg>)> {
    loop {
        let (to_link, writes) = ble_gatt::channel();
        match (env.connect)()
            .map(|b| b.method_timeout(DBUS_TIMEOUT))
            .and_then(|b| Beacon::serve(b, device_id, wake.clone(), to_link.clone()))
        {
            Ok((conn, beacon)) => return Some((conn, beacon, to_link, writes)),
            Err(e) => tracing::debug!(error = %e, "BLE beacon: can't serve it on the system D-Bus"),
        }
        if wake.wait(BEACON_CHECK_INTERVAL).stopped {
            return None;
        }
    }
}

fn run_worker(env: &Env, device_id: &DeviceId, wake: &Arc<Wake>) {
    let Some((conn, mut beacon, to_link, writes)) = connect(env, device_id, wake) else {
        return;
    };
    let links = ble_gatt::spawn_links(conn.clone(), writes, env.accept.clone());
    *LINK.lock().unwrap_or_else(|e| e.into_inner()) = Some(to_link.clone());
    let watcher = Watcher::spawn(env, wake.clone());
    let mut woken = Wakeup::default();

    loop {
        // BlueZ is called only while it runs: a call would start it.
        let bluez_running = super::dbus_name_has_owner(&conn, BLUEZ);
        if woken.bluez_changed || !bluez_running {
            beacon.forget();
        }
        if bluez_running && let Some(objects) = managed_objects(&conn) {
            let window = wake.window();
            beacon.sync(&conn, &objects, &(env.phones)(), window.as_ref(), (env.now)());
        }
        if woken.devices {
            let _ = to_link.try_send(LinkMsg::DeviceChanged);
        }
        woken = wake.wait(BEACON_CHECK_INTERVAL);
        if woken.stopped {
            break;
        }
    }
    *LINK.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if let Some(links) = links {
        let _ = to_link.send(LinkMsg::Stop);
        let _ = links.join();
    }
    if super::dbus_name_has_owner(&conn, BLUEZ) {
        beacon.stop(&conn);
    }
    if let Some(watcher) = watcher {
        watcher.stop();
    }
}

/// Wakes the worker as soon as BlueZ starts, stops or restarts, an adapter
/// comes, goes or is turned on or off, or the PC resumes.
struct Watcher {
    conn: zbus::blocking::Connection,
    /// Disconnected once the watcher has stopped.
    done: mpsc::Receiver<()>,
    thread: JoinHandle<()>,
}

impl Watcher {
    fn spawn(env: &Env, wake: Arc<Wake>) -> Option<Self> {
        let conn = (env.connect)()
            .and_then(|b| b.method_timeout(DBUS_TIMEOUT).build())
            .inspect_err(|error| tracing::debug!(%error, "BLE beacon: can't follow BlueZ's signals"))
            .ok()?;
        let (ready, done) = mpsc::channel();
        let watched = conn.clone();
        let thread = thread::Builder::new()
            .name("ble-beacon-watch".into())
            .spawn(move || watch(&watched, &wake, &ready))
            .ok()?;
        // Subscribed before the first check, so no change falls in between.
        let _ = done.recv_timeout(WATCHER_TIMEOUT);
        Some(Self { conn, done, thread })
    }

    fn stop(self) {
        // Its stop flag is set; a message gets it to look.
        if let Some(name) = self.conn.unique_name() {
            let _ = self.conn.emit_signal(
                Some(name.as_str()),
                ADVERTISEMENT_PATH,
                WATCHER_INTERFACE,
                "Stop",
                &(),
            );
        }
        let deadline = Instant::now() + WATCHER_TIMEOUT;
        loop {
            match self.done.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(()) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = self.thread.join();
                    return;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    tracing::debug!("BLE beacon: the signal watcher didn't stop; it ends with the app");
                    return;
                }
            }
        }
    }
}

fn watch(conn: &zbus::blocking::Connection, wake: &Wake, ready: &mpsc::Sender<()>) {
    // Made before subscribing, so it has every signal from then on.
    let messages = zbus::blocking::MessageIterator::from(conn);
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(conn) else {
        return;
    };
    for rule in watched_signals() {
        if let Err(error) = dbus.add_match_rule(rule) {
            tracing::debug!(%error, "BLE beacon: can't follow one of BlueZ's signals");
        }
    }
    let _ = ready.send(());

    for message in messages {
        if wake.stopped() {
            break;
        }
        let Ok(message) = message else {
            break;
        };
        match bluez_change(&message) {
            Some(Change::Bluez) => wake.set(|w| w.bluez_changed = true),
            Some(Change::Adapters) => wake.set(|w| w.resync = true),
            Some(Change::Devices) => wake.set(|w| w.devices = true),
            None => {}
        }
    }
}

fn watched_signals() -> Vec<zbus::MatchRule<'static>> {
    let signal = || zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal);
    [
        signal()
            .sender("org.freedesktop.DBus")
            .and_then(|b| b.interface("org.freedesktop.DBus"))
            .and_then(|b| b.member("NameOwnerChanged"))
            .and_then(|b| b.arg(0, BLUEZ)),
        signal()
            .sender(BLUEZ)
            .and_then(|b| b.interface("org.freedesktop.DBus.Properties"))
            .and_then(|b| b.member("PropertiesChanged"))
            .and_then(|b| b.arg(0, ADAPTER)),
        signal()
            .sender(BLUEZ)
            .and_then(|b| b.interface("org.freedesktop.DBus.Properties"))
            .and_then(|b| b.member("PropertiesChanged"))
            .and_then(|b| b.arg(0, DEVICE)),
        signal()
            .sender(BLUEZ)
            .and_then(|b| b.interface(OBJECT_MANAGER))
            .and_then(|b| b.member("InterfacesAdded")),
        signal()
            .sender(BLUEZ)
            .and_then(|b| b.interface(OBJECT_MANAGER))
            .and_then(|b| b.member("InterfacesRemoved")),
        signal()
            .sender("org.freedesktop.login1")
            .and_then(|b| b.interface("org.freedesktop.login1.Manager"))
            .and_then(|b| b.member("PrepareForSleep")),
    ]
    .into_iter()
    .filter_map(|rule| rule.map(|b| b.build()).ok())
    .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum Change {
    /// BlueZ started, stopped or restarted.
    Bluez,
    /// An adapter came, went or was turned on or off, or the PC resumed.
    Adapters,
    /// A device connected or disconnected.
    Devices,
}

fn is_adapter_interface(name: &str) -> bool {
    name == ADAPTER || name == ADVERTISING_MANAGER
}

fn bluez_change(message: &zbus::Message) -> Option<Change> {
    let header = message.header();
    let interface = header.interface()?.as_str();
    let member = header.member()?.as_str();
    let body = message.body();
    match (interface, member) {
        ("org.freedesktop.DBus", "NameOwnerChanged") => {
            let (name, _, _): (String, String, String) = body.deserialize().ok()?;
            (name == BLUEZ).then_some(Change::Bluez)
        }
        ("org.freedesktop.DBus.Properties", "PropertiesChanged") => {
            let (interface, changed, invalidated): (String, HashMap<String, OwnedValue>, Vec<String>) =
                body.deserialize().ok()?;
            let has = |name: &str| changed.contains_key(name) || invalidated.iter().any(|p| p == name);
            match interface.as_str() {
                ADAPTER if has("Powered") => Some(Change::Adapters),
                DEVICE if has("Connected") => Some(Change::Devices),
                _ => None,
            }
        }
        (OBJECT_MANAGER, "InterfacesAdded") => {
            let (_, interfaces): (OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>) =
                body.deserialize().ok()?;
            interfaces.keys().any(|i| is_adapter_interface(i)).then_some(Change::Adapters)
        }
        (OBJECT_MANAGER, "InterfacesRemoved") => {
            let (_, interfaces): (OwnedObjectPath, Vec<String>) = body.deserialize().ok()?;
            interfaces.iter().any(|i| is_adapter_interface(i)).then_some(Change::Adapters)
        }
        ("org.freedesktop.login1.Manager", "PrepareForSleep") => {
            let sleeping: bool = body.deserialize().ok()?;
            (!sleeping).then_some(Change::Adapters)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: u64 = 1_710_000_000;

    fn at() -> u64 {
        AT
    }

    fn extract_string(val: &OwnedValue) -> Option<String> {
        <&str>::try_from(val).ok().map(str::to_owned)
    }

    fn extract_mfg_ffff(val: &OwnedValue) -> Option<Vec<u8>> {
        let Value::Dict(dict) = &**val else {
            return None;
        };
        for (k, v) in dict.iter() {
            if u16::try_from(k).is_ok_and(|id| id == COMPANY_ID) {
                let inner = match v {
                    Value::Value(inner) => inner.as_ref(),
                    other => other,
                };
                if let Value::Array(bytes) = inner {
                    return bytes.iter().map(|b| u8::try_from(b).ok()).collect();
                }
            }
        }
        None
    }

    fn key(n: u8) -> [u8; 32] {
        ble::beacon_key(&[n; 32])
    }

    fn for_phones(keys: &[[u8; 32]]) -> BeaconPhones {
        BeaconPhones { keys: keys.to_vec(), legacy: false, linkable: false }
    }

    #[test]
    fn the_old_beacon_is_the_magic_and_the_id_prefix() {
        let id = DeviceId(std::array::from_fn(|i| (i as u8).wrapping_mul(7).wrapping_add(3)));
        let payload = legacy_payload(&id);
        assert_eq!(&payload[0..5], b"NCTR\x01");
        assert_eq!(&payload[5..13], &id.as_bytes()[0..8]);
    }

    #[test]
    fn a_beacon_holds_each_phones_tag_and_nothing_stable() {
        let keys = [key(1), key(2)];
        let shown = wanted(&for_phones(&keys), None, AT, 0).unwrap();
        let (a, b) = (beacon_payload(&shown), beacon_payload(&shown));
        assert_eq!(a.len(), 2 + BEACON_SLOTS * TAG_LEN);
        for key in &keys {
            assert_eq!(ble::match_payload(&a, key, ble::epoch_at(AT)), Some(TagKind::Presence));
        }
        assert_eq!(ble::match_payload(&a, &key(3), ble::epoch_at(AT)), None);
        assert_ne!(a, b, "the filler and the order are new each time");
        assert!(wanted(&for_phones(&[]), None, AT, 0).is_none(), "no phones, no beacon");
    }

    #[test]
    fn the_beacon_changes_with_the_window_a_little_late() {
        let phones = for_phones(&[key(1)]);
        let window = ble::epoch_at(AT) * ble::EPOCH_SECS;
        let delay = 30;
        let before = wanted(&phones, None, window + delay - 1, delay).unwrap();
        let after = wanted(&phones, None, window + delay, delay).unwrap();
        assert_eq!(before.epoch + 1, after.epoch);
    }

    #[test]
    fn phones_that_dont_fit_take_turns() {
        let keys: Vec<[u8; 32]> = (1..=6).map(key).collect();
        let phones = for_phones(&keys);
        let first = wanted(&phones, None, 0, 0).unwrap();
        let second = wanted(&phones, None, TURN_SECS, 0).unwrap();
        let third = wanted(&phones, None, 2 * TURN_SECS, 0).unwrap();
        assert_eq!(first.keys, keys[..4]);
        assert_eq!(second.keys, keys[4..]);
        assert_eq!(third.keys, first.keys);
    }

    #[test]
    fn shuffled_is_a_permutation() {
        for _ in 0..50 {
            let mut order = shuffled(BEACON_SLOTS);
            order.sort_unstable();
            assert_eq!(order, (0..BEACON_SLOTS).collect::<Vec<_>>());
        }
    }

    fn adapter(path: &str, powered: bool, can_advertise: bool) -> ManagedObjects {
        let mut ifaces = HashMap::from([(
            "org.bluez.Adapter1".to_owned(),
            HashMap::from([("Powered".to_owned(), powered.into())]),
        )]);
        if can_advertise {
            ifaces.insert(ADVERTISING_MANAGER.to_owned(), HashMap::new());
        }
        HashMap::from([(OwnedObjectPath::try_from(path).unwrap(), ifaces)])
    }

    /// A powered adapter that can advertise and serve GATT services.
    fn gatt_adapter() -> ManagedObjects {
        let mut objects = adapter("/org/bluez/hci0", true, true);
        for ifaces in objects.values_mut() {
            ifaces.insert(GATT_MANAGER.to_owned(), HashMap::new());
        }
        objects
    }

    #[test]
    fn beacon_uses_the_first_powered_adapter_that_can_advertise() {
        let mut objects = adapter("/org/bluez/hci2", true, true);
        objects.extend(adapter("/org/bluez/hci0", false, true));
        objects.extend(adapter("/org/bluez/hci1", true, false));
        objects.extend(adapter("/org/bluez/hci3", true, true));
        assert_eq!(advertising_adapter(&objects).unwrap().as_str(), "/org/bluez/hci2");
        assert_eq!(advertising_adapter(&adapter("/org/bluez/hci0", false, true)), None);
        assert_eq!(advertising_adapter(&HashMap::new()), None);
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

    /// What the fake advertising manager was asked.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Register {
            path: String,
            kind: String,
            data: Option<Vec<u8>>,
            interval: Option<(u32, u32)>,
        },
        Unregister(String),
        /// A GATT application, with the UUIDs and flags of its characteristics.
        RegisterApp(Vec<(String, Vec<String>)>),
        UnregisterApp,
    }

    impl Call {
        fn registered(&self) -> Option<(&str, &[u8])> {
            match self {
                Call::Register { path, data: Some(data), .. } => Some((path, data)),
                _ => None,
            }
        }
    }

    /// Stands in for BlueZ's advertising manager: reads each beacon back the
    /// way BlueZ does and records what it was asked.
    #[derive(Clone, Default)]
    struct FakeManager {
        calls: Arc<Mutex<Vec<Call>>>,
        /// Refuses a beacon with an advertising interval, as an older BlueZ may.
        reject_intervals: bool,
    }

    impl FakeManager {
        fn take(&self) -> Vec<Call> {
            std::mem::take(&mut *self.calls.lock().unwrap())
        }

        /// The calls so far, once `done` holds for them or five seconds have
        /// passed: well before the worker's 15-second check.
        fn wait_for(&self, done: impl Fn(&[Call]) -> bool) -> Vec<Call> {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let calls = self.calls.lock().unwrap().clone();
                if done(&calls) || Instant::now() > deadline {
                    return calls;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }

    #[zbus::interface(name = "org.bluez.LEAdvertisingManager1")]
    impl FakeManager {
        async fn register_advertisement(
            &self,
            #[zbus(header)] header: zbus::message::Header<'_>,
            #[zbus(connection)] conn: &zbus::Connection,
            path: OwnedObjectPath,
            _options: HashMap<String, OwnedValue>,
        ) -> zbus::fdo::Result<()> {
            let reply = conn
                .call_method(
                    header.sender().map(|s| s.as_str()),
                    &path,
                    Some("org.freedesktop.DBus.Properties"),
                    "GetAll",
                    &("org.bluez.LEAdvertisement1",),
                )
                .await?;
            let props: HashMap<String, OwnedValue> = reply.body().deserialize()?;
            let kind = props.get("Type").and_then(extract_string).unwrap_or_default();
            let data = props.get("ManufacturerData").and_then(extract_mfg_ffff);
            let interval = |name: &str| props.get(name).and_then(|v| u32::try_from(v).ok());
            let interval = interval("MinInterval").zip(interval("MaxInterval"));
            if self.reject_intervals && interval.is_some() {
                return Err(zbus::fdo::Error::InvalidArgs("Failed to parse advertisement".into()));
            }
            let call = Call::Register { path: path.as_str().to_owned(), kind, data, interval };
            self.calls.lock().unwrap().push(call);
            Ok(())
        }

        fn unregister_advertisement(&self, path: OwnedObjectPath) {
            self.calls.lock().unwrap().push(Call::Unregister(path.as_str().to_owned()));
        }
    }

    /// Stands in for BlueZ's GATT manager: reads the application's objects
    /// the way BlueZ does.
    #[derive(Clone)]
    struct FakeGatt(FakeManager);

    #[zbus::interface(name = "org.bluez.GattManager1")]
    impl FakeGatt {
        async fn register_application(
            &self,
            #[zbus(header)] header: zbus::message::Header<'_>,
            #[zbus(connection)] conn: &zbus::Connection,
            app: OwnedObjectPath,
            _options: HashMap<String, OwnedValue>,
        ) -> zbus::fdo::Result<()> {
            let reply = conn
                .call_method(
                    header.sender().map(|s| s.as_str()),
                    &app,
                    Some(OBJECT_MANAGER),
                    "GetManagedObjects",
                    &(),
                )
                .await?;
            let objects: ManagedObjects = reply.body().deserialize()?;
            let mut characteristics: Vec<(String, Vec<String>)> = objects
                .values()
                .filter_map(|ifaces| ifaces.get("org.bluez.GattCharacteristic1"))
                .map(|props| {
                    let uuid = props.get("UUID").and_then(extract_string).unwrap_or_default();
                    let flags = props
                        .get("Flags")
                        .and_then(|v| v.try_clone().ok())
                        .and_then(|v| Vec::<String>::try_from(v).ok())
                        .unwrap_or_default();
                    (uuid, flags)
                })
                .collect();
            characteristics.sort();
            self.0.calls.lock().unwrap().push(Call::RegisterApp(characteristics));
            Ok(())
        }

        fn unregister_application(&self, _app: OwnedObjectPath) {
            self.0.calls.lock().unwrap().push(Call::UnregisterApp);
        }
    }

    /// A powered adapter, as BlueZ lists it.
    struct FakeAdapter;

    #[zbus::interface(name = "org.bluez.Adapter1")]
    impl FakeAdapter {
        #[zbus(property)]
        fn powered(&self) -> bool {
            true
        }
    }

    /// BlueZ with one powered adapter that can advertise.
    fn fake_bluez(bus: &TestBus, manager: &FakeManager) -> zbus::blocking::Connection {
        bus.connect()
            .name(BLUEZ)
            .unwrap()
            .serve_at("/", zbus::fdo::ObjectManager)
            .unwrap()
            .serve_at("/org/bluez/hci0", manager.clone())
            .unwrap()
            .serve_at("/org/bluez/hci0", FakeGatt(manager.clone()))
            .unwrap()
            .serve_at("/org/bluez/hci0", FakeAdapter)
            .unwrap()
            .build()
            .unwrap()
    }

    fn test_env(bus: &TestBus, phones: BeaconPhones) -> Env {
        let address = bus.address.clone();
        Env {
            connect: Arc::new(move || zbus::blocking::connection::Builder::address(address.as_str())),
            phones: Arc::new(move || phones.clone()),
            now: at,
            accept: Arc::new(|| None),
        }
    }

    fn served(bus: &TestBus, id: &DeviceId) -> (zbus::blocking::Connection, Beacon) {
        let (to_link, _) = ble_gatt::channel();
        let (conn, mut beacon) = Beacon::serve(bus.connect(), id, Arc::default(), to_link).unwrap();
        beacon.delay = 0;
        (conn, beacon)
    }

    /// The one call in `calls`, a registration of `path` that `key` recognises in the window at `now`.
    fn registered_for(calls: &[Call], path: &str, key: &[u8; 32], now: u64) -> Vec<u8> {
        let [call] = calls else { panic!("one call expected: {calls:?}") };
        let (at, data) = call.registered().unwrap_or_else(|| panic!("a registration expected: {call:?}"));
        assert_eq!(at, path);
        assert_eq!(ble::match_payload(data, key, ble::epoch_at(now)), Some(TagKind::Presence));
        data.to_vec()
    }

    fn registrations(calls: &[Call]) -> usize {
        calls.iter().filter(|c| matches!(c, Call::Register { .. })).count()
    }

    #[test]
    fn beacon_registers_with_bluez_and_follows_its_adapter() {
        let Some(bus) = TestBus::start() else {
            eprintln!("skipped: dbus-daemon isn't installed");
            return;
        };
        let manager = FakeManager::default();
        let bluez = fake_bluez(&bus, &manager);
        let id = DeviceId([0x5A; 32]);
        let (conn, mut beacon) = served(&bus, &id);
        let phones = for_phones(&[key(1)]);
        let unregister = Call::Unregister(ADVERTISEMENT_PATH.into());
        let (on, off) = (adapter("/org/bluez/hci0", true, true), adapter("/org/bluez/hci0", false, true));

        beacon.sync(&conn, &on, &phones, None, AT);
        beacon.sync(&conn, &on, &phones, None, AT + 1);
        let calls = manager.take();
        registered_for(&calls, ADVERTISEMENT_PATH, &key(1), AT);
        assert!(matches!(
            &calls[0],
            Call::Register { kind, interval: Some((MIN_INTERVAL_MS, MAX_INTERVAL_MS)), .. } if kind == "broadcast"
        ));

        // BlueZ dropped it, so it goes back.
        bluez
            .call_method(
                conn.unique_name().map(|n| n.as_str()),
                ADVERTISEMENT_PATH,
                Some("org.bluez.LEAdvertisement1"),
                "Release",
                &(),
            )
            .unwrap();
        beacon.sync(&conn, &on, &phones, None, AT);
        registered_for(&manager.take(), ADVERTISEMENT_PATH, &key(1), AT);

        // Bluetooth turned off, then on again.
        beacon.sync(&conn, &off, &phones, None, AT);
        beacon.sync(&conn, &off, &phones, None, AT);
        assert_eq!(manager.take(), std::slice::from_ref(&unregister));
        beacon.sync(&conn, &on, &phones, None, AT);
        registered_for(&manager.take(), ADVERTISEMENT_PATH, &key(1), AT);

        beacon.stop(&conn);
        assert_eq!(manager.take(), [unregister]);
    }

    #[test]
    fn beacon_changes_with_each_window_and_goes_with_the_last_phone() {
        let Some(bus) = TestBus::start() else {
            eprintln!("skipped: dbus-daemon isn't installed");
            return;
        };
        let manager = FakeManager::default();
        let _bluez = fake_bluez(&bus, &manager);
        let (conn, mut beacon) = served(&bus, &DeviceId([0x4D; 32]));
        let on = adapter("/org/bluez/hci0", true, true);
        let phones = for_phones(&[key(1)]);

        beacon.sync(&conn, &on, &phones, None, AT);
        let first = registered_for(&manager.take(), ADVERTISEMENT_PATH, &key(1), AT);

        let next = AT + ble::EPOCH_SECS;
        beacon.sync(&conn, &on, &phones, None, next);
        let calls = manager.take();
        assert_eq!(calls[0], Call::Unregister(ADVERTISEMENT_PATH.into()));
        let second = registered_for(&calls[1..], ADVERTISEMENT_PATH, &key(1), next);
        assert_ne!(first[2..], second[2..]);

        // A phone paired: it's in the beacon from now on.
        let two = for_phones(&[key(1), key(2)]);
        beacon.sync(&conn, &on, &two, None, next);
        let calls = manager.take();
        let data = registered_for(&calls[1..], ADVERTISEMENT_PATH, &key(2), next);
        assert_eq!(ble::match_payload(&data, &key(1), ble::epoch_at(next)), Some(TagKind::Presence));

        // The last phone unpaired: no beacon at all.
        beacon.sync(&conn, &on, &for_phones(&[]), None, next);
        assert_eq!(manager.take(), [Call::Unregister(ADVERTISEMENT_PATH.into())]);
        beacon.sync(&conn, &on, &for_phones(&[]), None, next + ble::EPOCH_SECS);
        assert_eq!(manager.take(), []);
    }

    #[test]
    fn the_old_beacon_goes_out_only_while_a_phone_needs_it() {
        let Some(bus) = TestBus::start() else {
            eprintln!("skipped: dbus-daemon isn't installed");
            return;
        };
        let manager = FakeManager::default();
        let _bluez = fake_bluez(&bus, &manager);
        let id = DeviceId([0x2E; 32]);
        let (conn, mut beacon) = served(&bus, &id);
        let on = adapter("/org/bluez/hci0", true, true);

        let old_app = BeaconPhones { keys: Vec::new(), legacy: true, linkable: false };
        beacon.sync(&conn, &on, &old_app, None, AT);
        beacon.sync(&conn, &on, &old_app, None, AT);
        let legacy = Call::Register {
            path: LEGACY_PATH.into(),
            kind: "broadcast".into(),
            data: Some(legacy_payload(&id).to_vec()),
            interval: None,
        };
        assert_eq!(manager.take(), [legacy]);

        beacon.sync(&conn, &on, &for_phones(&[key(1)]), None, AT);
        let calls = manager.take();
        assert_eq!(calls.len(), 2, "{calls:?}");
        registered_for(&calls[..1], ADVERTISEMENT_PATH, &key(1), AT);
        assert_eq!(calls[1], Call::Unregister(LEGACY_PATH.into()));

        beacon.stop(&conn);
        assert_eq!(manager.take(), [Call::Unregister(ADVERTISEMENT_PATH.into())]);
    }

    #[test]
    fn a_window_serves_the_link_and_lets_the_phone_connect() {
        let Some(bus) = TestBus::start() else {
            eprintln!("skipped: dbus-daemon isn't installed");
            return;
        };
        let manager = FakeManager::default();
        let _bluez = fake_bluez(&bus, &manager);
        let (conn, mut beacon) = served(&bus, &DeviceId([0x7E; 32]));
        let on = gatt_adapter();
        let phones = BeaconPhones { keys: vec![key(1)], legacy: false, linkable: true };

        beacon.sync(&conn, &on, &phones, None, AT);
        let calls = manager.take();
        let service = vec![
            (ble::RX_UUID.to_owned(), vec!["write".to_owned()]),
            (ble::TX_UUID.to_owned(), vec!["notify".to_owned()]),
        ];
        let mut sorted = service.clone();
        sorted.sort();
        assert_eq!(calls[0], Call::RegisterApp(sorted));
        registered_for(&calls[1..], ADVERTISEMENT_PATH, &key(1), AT);

        // A window: the beacon gives way to a connectable advertisement with a connect tag.
        let window = Window { keys: vec![key(1)], until: Instant::now() + Duration::from_secs(30) };
        beacon.sync(&conn, &on, &phones, Some(&window), AT);
        let calls = manager.take();
        assert_eq!(calls[0], Call::Unregister(ADVERTISEMENT_PATH.into()));
        let Call::Register { path, kind, data: Some(data), .. } = &calls[1] else { panic!("{calls:?}") };
        assert_eq!((path.as_str(), kind.as_str()), (CONNECT_PATH, "peripheral"));
        assert_eq!(data.len(), 2 + WINDOW_SLOTS * TAG_LEN);
        assert_eq!(ble::match_payload(data, &key(1), ble::epoch_at(AT)), Some(TagKind::Connect));

        // Closed: the beacon comes back. No phone may link any more: the service goes.
        beacon.sync(&conn, &on, &phones, None, AT);
        let calls = manager.take();
        assert_eq!(calls[0], Call::Unregister(CONNECT_PATH.into()));
        registered_for(&calls[1..], ADVERTISEMENT_PATH, &key(1), AT);
        beacon.sync(&conn, &on, &for_phones(&[key(1)]), None, AT);
        assert_eq!(manager.take(), [Call::UnregisterApp]);

        // Without the service, a window opens nothing.
        beacon.sync(&conn, &on, &for_phones(&[key(1)]), Some(&window), AT);
        assert_eq!(manager.take(), []);
    }

    #[test]
    fn writes_are_taken_only_from_the_start_and_from_a_device() {
        let device = OwnedValue::from(ObjectPath::from_static_str_unchecked("/org/bluez/hci0/dev_AA"));
        let ok = HashMap::from([
            ("device".to_owned(), device.try_clone().unwrap()),
            ("mtu".to_owned(), OwnedValue::from(185u16)),
        ]);
        let LinkMsg::Write { mtu, value, .. } = ble_gatt::parse_write(vec![1, 2], &ok).unwrap() else {
            panic!()
        };
        assert_eq!((mtu, value), (Some(185), vec![1, 2]));
        let offset =
            HashMap::from([("device".to_owned(), device), ("offset".to_owned(), OwnedValue::from(5u16))]);
        assert!(ble_gatt::parse_write(vec![1], &offset).is_err());
        assert!(ble_gatt::parse_write(vec![1], &HashMap::new()).is_err());
    }

    #[test]
    fn beacon_goes_without_the_fast_interval_when_bluez_refuses_it() {
        let Some(bus) = TestBus::start() else {
            eprintln!("skipped: dbus-daemon isn't installed");
            return;
        };
        let manager = FakeManager { reject_intervals: true, ..FakeManager::default() };
        let _bluez = fake_bluez(&bus, &manager);
        let (conn, mut beacon) = served(&bus, &DeviceId([0x6B; 32]));
        let on = adapter("/org/bluez/hci0", true, true);
        let phones = for_phones(&[key(1)]);

        beacon.sync(&conn, &on, &phones, None, AT);
        registered_for(&manager.take(), DEFAULT_INTERVAL_PATH, &key(1), AT);

        // Bluetooth turned off and on: straight to the beacon BlueZ took.
        beacon.sync(&conn, &adapter("/org/bluez/hci0", false, true), &phones, None, AT);
        beacon.sync(&conn, &on, &phones, None, AT);
        let calls = manager.take();
        assert_eq!(calls[0], Call::Unregister(DEFAULT_INTERVAL_PATH.into()));
        registered_for(&calls[1..], DEFAULT_INTERVAL_PATH, &key(1), AT);

        beacon.stop(&conn);
        assert_eq!(manager.take(), [Call::Unregister(DEFAULT_INTERVAL_PATH.into())]);
    }

    #[test]
    fn worker_waits_for_bluez_and_registers_again_when_it_restarts() {
        let Some(bus) = TestBus::start() else {
            eprintln!("skipped: dbus-daemon isn't installed");
            return;
        };
        let id = DeviceId([0x3C; 32]);
        let manager = FakeManager::default();
        let slot = Mutex::new(None);
        start_in(&slot, test_env(&bus, for_phones(&[key(1)])), id);

        // BlueZ isn't running yet: the worker waits for it.
        thread::sleep(Duration::from_millis(300));
        assert!(slot.lock().unwrap().as_ref().is_some_and(|w: &Worker| !w.thread.is_finished()));

        let first = fake_bluez(&bus, &manager);
        let calls = manager.wait_for(|calls| registrations(calls) >= 1);
        let (path, data) = calls[0].registered().unwrap();
        assert_eq!(path, ADVERTISEMENT_PATH);
        assert!(ble::match_payload(data, &key(1), ble::epoch_at(AT)).is_some());

        // bluetoothd restarts: the same adapter path, and a BlueZ that holds
        // no beacon.
        first.release_name(BLUEZ).unwrap();
        let _second = fake_bluez(&bus, &manager);
        let calls = manager.wait_for(|calls| registrations(calls) >= 2);
        assert_eq!(registrations(&calls), 2, "{calls:?}");

        stop_in(&slot);
        assert!(slot.lock().unwrap().is_none());
    }

    #[test]
    fn start_and_stop_are_idempotent_without_bluez() {
        let Some(bus) = TestBus::start() else {
            eprintln!("skipped: dbus-daemon isn't installed");
            return;
        };
        let id = DeviceId([0x24; 32]);
        let slot = Mutex::new(None);
        start_in(&slot, test_env(&bus, BeaconPhones::default()), id);
        start_in(&slot, test_env(&bus, BeaconPhones::default()), id);
        stop_in(&slot);
        stop_in(&slot);
        assert!(slot.lock().unwrap().is_none());
    }

    fn no_bus() -> zbus::Result<zbus::blocking::connection::Builder<'static>> {
        Err(zbus::Error::Unsupported)
    }

    #[test]
    fn stop_ends_a_worker_that_has_no_bus() {
        let env = Env {
            connect: Arc::new(no_bus),
            phones: Arc::new(BeaconPhones::default),
            now: at,
            accept: Arc::new(|| None),
        };
        let slot = Mutex::new(None);
        start_in(&slot, env, DeviceId([0x11; 32]));
        let started = Instant::now();
        stop_in(&slot);
        assert!(started.elapsed() < BEACON_CHECK_INTERVAL);
    }

    fn signal<B>(interface: &str, member: &str, body: &B) -> zbus::Message
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        zbus::Message::signal("/", interface, member).unwrap().build(body).unwrap()
    }

    #[test]
    fn only_bluez_and_its_adapters_wake_the_worker() {
        let owner = |name: &str| signal("org.freedesktop.DBus", "NameOwnerChanged", &(name, "", ":1.7"));
        assert_eq!(bluez_change(&owner(BLUEZ)), Some(Change::Bluez));
        assert_eq!(bluez_change(&owner("org.example.Other")), None);

        let changed = |interface: &str, property: &str| {
            let props = HashMap::from([(property, Value::from(true))]);
            let invalidated = Vec::<&str>::new();
            signal("org.freedesktop.DBus.Properties", "PropertiesChanged", &(interface, props, invalidated))
        };
        assert_eq!(bluez_change(&changed(ADAPTER, "Powered")), Some(Change::Adapters));
        assert_eq!(bluez_change(&changed(ADAPTER, "Discovering")), None);
        assert_eq!(bluez_change(&changed("org.bluez.Device1", "Powered")), None);

        let removed = |interface: &str| {
            let path = ObjectPath::from_static_str_unchecked("/org/bluez/hci0");
            signal(OBJECT_MANAGER, "InterfacesRemoved", &(path, vec![interface]))
        };
        assert_eq!(bluez_change(&removed(ADVERTISING_MANAGER)), Some(Change::Adapters));
        assert_eq!(bluez_change(&removed("org.bluez.Device1")), None);

        let sleep = |sleeping: bool| signal("org.freedesktop.login1.Manager", "PrepareForSleep", &sleeping);
        assert_eq!(bluez_change(&sleep(false)), Some(Change::Adapters));
        assert_eq!(bluez_change(&sleep(true)), None);
    }
}
