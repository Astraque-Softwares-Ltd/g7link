// SPDX-License-Identifier: GPL-3.0-or-later
//! Bluetooth through BlueZ over D-Bus. Broadcasts the PC's proximity beacon
//! (company `0xFFFF`, `NCTR\x01` + 8-byte device ID prefix) so
//! phones set to "Wake when PC is nearby" find it, and, when walk-away
//! automation is on, watches the signal strength of paired phones. Never
//! spawns `bluetoothctl` or starts a scan of its own.
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

use g7link_core::DeviceId;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

pub const COMPANY_ID: u16 = 0xFFFF;
pub const MAGIC: &[u8; 5] = b"NCTR\x01";
pub const NEAR_RSSI_DBM: i16 = -72;
pub const WALK_AWAY_RSSI_DBM: i16 = -85;
pub const G7LINK_BLE_UUID: &str = "f81d4fae-7dec-11d0-a765-00a0c91e6bf6";

const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// How often the beacon is checked when walk-away is off, in case a change in
/// BlueZ went unreported.
const BEACON_CHECK_INTERVAL: Duration = Duration::from_secs(15);
/// The beacon's advertising interval: phones see it within a fraction of a
/// second, where the controller's default is about 1.28 s.
const MIN_INTERVAL_MS: u32 = 100;
const MAX_INTERVAL_MS: u32 = 200;
const DBUS_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the signal watcher gets to subscribe, and to stop.
const WATCHER_TIMEOUT: Duration = Duration::from_secs(2);

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const ADVERTISING_MANAGER: &str = "org.bluez.LEAdvertisingManager1";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
const ADVERTISEMENT_PATH: &str = "/app/g7link/beacon";
/// The same beacon at the controller's default interval, for a BlueZ or a
/// controller that refuses the faster one.
const DEFAULT_INTERVAL_PATH: &str = "/app/g7link/beacon_default_interval";
/// Where the signal watcher is told to stop.
const WATCHER_INTERFACE: &str = "app.g7link.BeaconWatcher";

static WORKER: Mutex<Option<Worker>> = Mutex::new(None);

type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedPhone {
    pub prefix: [u8; 8],
    pub name: String,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct BluezDeviceCandidate {
    pub address: Option<String>,
    pub name: Option<String>,
    pub alias: Option<String>,
    pub uuids: Vec<String>,
    pub mfg_payload: Option<Vec<u8>>,
}

#[derive(Debug, Default)]
pub struct RssiTracker {
    peers: HashMap<[u8; 8], PeerRssiState>,
}

#[derive(Debug, Clone, Default)]
struct PeerRssiState {
    samples: [i16; 4],
    count: usize,
    idx: usize,
    armed_near: bool,
}

impl RssiTracker {
    pub fn record(&mut self, prefix: [u8; 8], rssi: i16) -> bool {
        let state = self.peers.entry(prefix).or_default();
        state.samples[state.idx] = rssi;
        state.idx = (state.idx + 1) % state.samples.len();
        state.count = (state.count + 1).min(state.samples.len());
        if state.count < state.samples.len() {
            return false;
        }
        let sum: i32 = state.samples.iter().map(|&v| i32::from(v)).sum();
        let avg = (sum / (state.samples.len() as i32)) as i16;
        if avg >= NEAR_RSSI_DBM {
            state.armed_near = true;
            false
        } else if state.armed_near && avg <= WALK_AWAY_RSSI_DBM {
            state.armed_near = false;
            true
        } else {
            false
        }
    }
}

pub fn build_manufacturer_payload(device_id: &DeviceId) -> [u8; 13] {
    let mut out = [0u8; 13];
    out[0..5].copy_from_slice(MAGIC);
    out[5..13].copy_from_slice(&device_id.as_bytes()[0..8]);
    out
}

/// Validates the `NCTR\x01` magic header and extracts the peer device ID prefix.
/// Accepts either the 13-byte payload (5-byte magic + 8-byte prefix) or a 12-byte
/// payload (5-byte magic + 7-byte prefix, zero-padded in the 8th byte).
pub fn parse_manufacturer_payload(bytes: &[u8]) -> Option<[u8; 8]> {
    if bytes.len() < 12 || &bytes[0..5] != MAGIC {
        return None;
    }
    let mut prefix = [0u8; 8];
    if bytes.len() >= 13 {
        prefix.copy_from_slice(&bytes[5..13]);
    } else {
        prefix[0..7].copy_from_slice(&bytes[5..12]);
    }
    Some(prefix)
}

pub fn start(device_id: DeviceId) {
    let env = Env {
        connect: Arc::new(zbus::blocking::connection::Builder::system),
        walk_away: walk_away_lock_enabled,
    };
    start_in(&WORKER, env, device_id);
}

pub fn stop() {
    stop_in(&WORKER);
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

fn walk_away_lock_enabled() -> bool {
    let rules = crate::settings::automation_rules();
    !crate::automations::matching_rules(&rules, crate::automations::TRIGGER_PHONE_WALKED_AWAY).is_empty()
}

fn paired_phones_from_hub() -> Vec<PairedPhone> {
    crate::core_host::host().hub.read(|s| {
        s.devices
            .iter()
            .map(|d| {
                let mut prefix = [0u8; 8];
                prefix.copy_from_slice(&d.id.as_bytes()[0..8]);
                PairedPhone { prefix, name: d.info.name.clone(), model: d.info.model.clone() }
            })
            .collect()
    })
}

fn fallback_peer_prefix(id_str: &str) -> [u8; 8] {
    let mut out = [0u8; 8];
    let bytes = id_str.as_bytes();
    let start = bytes.len().saturating_sub(8);
    for (dst, &src) in out.iter_mut().zip(&bytes[start..]) {
        *dst = src;
    }
    out
}

fn names_match(candidate: &str, phone_name: &str) -> bool {
    let c = candidate.trim();
    let p = phone_name.trim();
    !c.is_empty() && !p.is_empty() && c.eq_ignore_ascii_case(p)
}

/// Matches a BlueZ device candidate ONLY if:
/// 1. It carries a valid `NCTR\x01` manufacturer payload (not our own prefix), OR
/// 2. It advertises the G7 Link BLE service UUID (`f81d4fae-7dec-11d0-a765-00a0c91e6bf6`), OR
/// 3. Its `Name` or `Alias` matches a paired phone in `hub`.
///
/// Arbitrary Bluetooth peripherals (headphones, mice, keyboards, etc.) never match.
pub fn match_g7link_device(
    candidate: &BluezDeviceCandidate,
    own_prefix: &[u8; 8],
    paired_phones: &[PairedPhone],
) -> Option<[u8; 8]> {
    if let Some(mfg_bytes) = &candidate.mfg_payload
        && let Some(prefix) = parse_manufacturer_payload(mfg_bytes)
    {
        if &prefix == own_prefix {
            return None;
        }
        if paired_phones.is_empty() || paired_phones.iter().any(|p| p.prefix == prefix) {
            return Some(prefix);
        }
    }

    let find_matching_phone = || -> Option<&PairedPhone> {
        paired_phones.iter().find(|phone| {
            candidate.name.as_deref().is_some_and(|n| {
                names_match(n, &phone.name) || phone.model.as_deref().is_some_and(|m| names_match(n, m))
            }) || candidate.alias.as_deref().is_some_and(|a| {
                names_match(a, &phone.name) || phone.model.as_deref().is_some_and(|m| names_match(a, m))
            })
        })
    };

    if candidate.uuids.iter().any(|u| u.eq_ignore_ascii_case(G7LINK_BLE_UUID)) {
        if let Some(phone) = find_matching_phone() {
            return Some(phone.prefix);
        }
        if let Some(first) = paired_phones.first() {
            return Some(first.prefix);
        }
        if let Some(addr) = &candidate.address {
            let prefix = fallback_peer_prefix(addr);
            if &prefix != own_prefix {
                return Some(prefix);
            }
        }
        return None;
    }

    find_matching_phone().map(|p| p.prefix)
}

fn extract_string(val: &OwnedValue) -> Option<String> {
    <&str>::try_from(val).ok().map(str::to_owned)
}

fn extract_i16(val: &OwnedValue) -> Option<i16> {
    if let Ok(v) = i16::try_from(val) {
        return Some(v);
    }
    if let Ok(v) = i32::try_from(val) {
        return i16::try_from(v).ok();
    }
    None
}

fn extract_string_list(val: &OwnedValue) -> Vec<String> {
    if let Ok(list) = Vec::<String>::try_from(val.clone()) {
        return list;
    }
    let Value::Array(arr) = &**val else {
        return Vec::new();
    };
    arr.iter().filter_map(|item| <&str>::try_from(item).ok().map(str::to_owned)).collect()
}

fn extract_mfg_ffff(val: &OwnedValue) -> Option<Vec<u8>> {
    let Value::Dict(dict) = &**val else {
        return None;
    };
    for (k, v) in dict.iter() {
        let key_matches = u16::try_from(k).is_ok_and(|id| id == COMPANY_ID);
        if !key_matches {
            continue;
        }
        let inner = match v {
            Value::Value(inner) => inner.as_ref(),
            other => other,
        };
        if let Value::Array(bytes_arr) = inner {
            let mut out = Vec::with_capacity(bytes_arr.len());
            for item in bytes_arr.iter() {
                out.push(u8::try_from(item).ok()?);
            }
            return Some(out);
        }
    }
    None
}

/// What the worker is woken for.
#[derive(Debug, Default, Clone, Copy)]
struct Wakeup {
    stopped: bool,
    /// Something in BlueZ changed: look again now.
    resync: bool,
    /// BlueZ started, stopped or restarted, so it holds no registration.
    bluez_changed: bool,
}

impl Wakeup {
    fn pending(&self) -> bool {
        self.stopped || self.resync || self.bluez_changed
    }
}

#[derive(Debug, Default)]
struct Wake {
    state: Mutex<Wakeup>,
    cv: Condvar,
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

    /// Waits until woken or `timeout` passes, and takes what it was woken for.
    fn wait(&self, timeout: Duration) -> Wakeup {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let waited = self.cv.wait_timeout_while(state, timeout, |w| !w.pending());
        let (mut state, _) = waited.unwrap_or_else(|e| e.into_inner());
        let woken = *state;
        state.resync = false;
        state.bluez_changed = false;
        woken
    }
}

/// The beacon as BlueZ reads it: a non-connectable broadcast that carries only
/// the manufacturer data.
#[derive(Clone)]
struct Advertisement {
    payload: [u8; 13],
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
        let bytes =
            OwnedValue::try_from(Value::from(self.payload.to_vec())).expect("byte arrays hold no fds");
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

/// Keeps the beacon registered with the first powered adapter that can
/// advertise, following adapters as they are turned off and on.
struct Beacon {
    adapter: Option<OwnedObjectPath>,
    released: Arc<AtomicBool>,
    /// BlueZ refused the faster interval, so the beacon goes without it.
    default_interval: bool,
    last_error: Option<String>,
}

impl Beacon {
    /// Connects with the beacon served. It has to be served from the builder:
    /// adding it to a connection afterwards needs a tokio runtime.
    fn serve(
        builder: zbus::blocking::connection::Builder<'_>,
        device_id: &DeviceId,
        wake: Arc<Wake>,
    ) -> zbus::Result<(zbus::blocking::Connection, Self)> {
        let released = Arc::new(AtomicBool::new(false));
        let advertisement = Advertisement {
            payload: build_manufacturer_payload(device_id),
            released: released.clone(),
            wake,
        };
        let conn = builder
            .serve_at(ADVERTISEMENT_PATH, FastAdvertisement(advertisement.clone()))?
            .serve_at(DEFAULT_INTERVAL_PATH, DefaultIntervalAdvertisement(advertisement))?
            .build()?;
        Ok((conn, Self { adapter: None, released, default_interval: false, last_error: None }))
    }

    fn path(&self) -> &'static str {
        if self.default_interval { DEFAULT_INTERVAL_PATH } else { ADVERTISEMENT_PATH }
    }

    /// BlueZ went away, and its registrations with it.
    fn forget(&mut self) {
        self.adapter = None;
    }

    fn sync(&mut self, conn: &zbus::blocking::Connection, objects: &ManagedObjects) {
        if self.released.swap(false, Ordering::SeqCst) {
            self.adapter = None;
        }
        let wanted = advertising_adapter(objects);
        if self.adapter.is_some() && self.adapter == wanted {
            return;
        }
        if let Some(previous) = self.adapter.take() {
            unregister(conn, &previous, self.path());
        }
        let Some(adapter) = wanted else {
            return;
        };
        match self.register(conn, &adapter) {
            Ok(()) => {
                self.adapter = Some(adapter);
                self.last_error = None;
            }
            Err(e) => {
                // Tried again on every check, so say it once.
                let error = e.to_string();
                if self.last_error.as_ref() != Some(&error) {
                    tracing::warn!(%error, "this PC can't broadcast its Bluetooth beacon");
                    self.last_error = Some(error);
                }
            }
        }
    }

    fn register(&mut self, conn: &zbus::blocking::Connection, adapter: &OwnedObjectPath) -> zbus::Result<()> {
        if !self.default_interval {
            let Err(error) = register_advertisement(conn, adapter, ADVERTISEMENT_PATH) else {
                tracing::info!(
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
        register_advertisement(conn, adapter, DEFAULT_INTERVAL_PATH)?;
        tracing::info!(
            adapter = adapter.as_str(),
            "broadcasting the Bluetooth beacon at the controller's default interval"
        );
        Ok(())
    }

    fn stop(&mut self, conn: &zbus::blocking::Connection) {
        if let Some(adapter) = self.adapter.take() {
            unregister(conn, &adapter, self.path());
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

fn poll_bluez_devices(objects: &ManagedObjects, own_prefix: &[u8; 8], tracker: &mut RssiTracker) {
    let paired_phones = paired_phones_from_hub();
    if paired_phones.is_empty() {
        return;
    }

    for ifaces in objects.values() {
        let Some(props) = ifaces.get("org.bluez.Device1") else {
            continue;
        };
        let Some(rssi) = props.get("RSSI").and_then(extract_i16) else {
            continue;
        };
        let candidate = BluezDeviceCandidate {
            address: props.get("Address").and_then(extract_string),
            name: props.get("Name").and_then(extract_string),
            alias: props.get("Alias").and_then(extract_string),
            uuids: props.get("UUIDs").map(extract_string_list).unwrap_or_default(),
            mfg_payload: props.get("ManufacturerData").and_then(extract_mfg_ffff),
        };
        if let Some(prefix) = match_g7link_device(&candidate, own_prefix, &paired_phones)
            && tracker.record(prefix, rssi)
        {
            tracing::info!("BLE walk-away threshold reached for paired phone; firing trigger");
            crate::automations::fire_trigger(crate::automations::TRIGGER_PHONE_WALKED_AWAY);
        }
    }
}

/// Where the worker connects (the system bus, or a test's own) and what it reads.
type Connect = Arc<dyn Fn() -> zbus::Result<zbus::blocking::connection::Builder<'static>> + Send + Sync>;

struct Env {
    connect: Connect,
    walk_away: fn() -> bool,
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

/// Connects with the beacon served, or without it when it can't be (walk-away
/// still works), trying again until it connects or is stopped.
fn connect(
    env: &Env,
    device_id: &DeviceId,
    wake: &Arc<Wake>,
) -> Option<(zbus::blocking::Connection, Option<Beacon>)> {
    let builder = || (env.connect)().map(|b| b.method_timeout(DBUS_TIMEOUT));
    loop {
        match builder().and_then(|b| Beacon::serve(b, device_id, wake.clone())) {
            Ok((conn, beacon)) => return Some((conn, Some(beacon))),
            Err(e) => {
                tracing::debug!(error = %e, "BLE beacon: can't serve it on the system D-Bus");
                if let Ok(conn) = builder().and_then(|b| b.build()) {
                    return Some((conn, None));
                }
            }
        }
        if wake.wait(BEACON_CHECK_INTERVAL).stopped {
            return None;
        }
    }
}

fn run_worker(env: &Env, device_id: &DeviceId, wake: &Arc<Wake>) {
    let Some((conn, mut beacon)) = connect(env, device_id, wake) else {
        return;
    };
    let watcher = Watcher::spawn(env, wake.clone());

    let mut own_prefix = [0u8; 8];
    own_prefix.copy_from_slice(&device_id.as_bytes()[0..8]);
    let mut tracker = RssiTracker::default();
    let mut woken = Wakeup::default();

    loop {
        // BlueZ is called only while it runs: a call would start it.
        let bluez_running = super::dbus_name_has_owner(&conn, BLUEZ);
        if let Some(beacon) = beacon.as_mut()
            && (woken.bluez_changed || !bluez_running)
        {
            beacon.forget();
        }
        let walk_away = (env.walk_away)();
        if bluez_running && let Some(objects) = managed_objects(&conn) {
            if let Some(beacon) = beacon.as_mut() {
                beacon.sync(&conn, &objects);
            }
            if walk_away {
                poll_bluez_devices(&objects, &own_prefix, &mut tracker);
            }
        }

        let interval = if walk_away { POLL_INTERVAL } else { BEACON_CHECK_INTERVAL };
        woken = wake.wait(interval);
        if woken.stopped {
            break;
        }
    }
    if let Some(beacon) = beacon.as_mut()
        && super::dbus_name_has_owner(&conn, BLUEZ)
    {
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
            let powered = changed.contains_key("Powered") || invalidated.iter().any(|p| p == "Powered");
            (interface == ADAPTER && powered).then_some(Change::Adapters)
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

    #[test]
    fn builds_13_byte_manufacturer_payload_with_magic_and_id_prefix() {
        let mut raw = [0u8; 32];
        for (i, b) in raw.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7).wrapping_add(3);
        }
        let id = DeviceId(raw);
        let payload = build_manufacturer_payload(&id);
        assert_eq!(payload.len(), 13);
        assert_eq!(&payload[0..5], b"NCTR\x01");
        assert_eq!(&payload[5..13], &raw[0..8]);
        assert_eq!(COMPANY_ID, 0xFFFF);
    }

    #[test]
    fn manufacturer_payload_roundtrips_through_parser() {
        let mut raw = [0u8; 32];
        for (i, b) in raw.iter_mut().enumerate() {
            *b = (i as u8).wrapping_add(0x10);
        }
        let id = DeviceId(raw);
        let payload = build_manufacturer_payload(&id);
        let parsed = parse_manufacturer_payload(&payload).expect("valid payload should parse");
        assert_eq!(&parsed[..], &raw[0..8]);

        // Also supports 12-byte payloads (5-byte magic + 7-byte prefix).
        let parsed_12 = parse_manufacturer_payload(&payload[0..12]).expect("12-byte payload should parse");
        assert_eq!(&parsed_12[0..7], &raw[0..7]);
        assert_eq!(parsed_12[7], 0);
    }

    #[test]
    fn parser_rejects_short_or_wrong_magic_payloads() {
        assert_eq!(parse_manufacturer_payload(b""), None);
        assert_eq!(parse_manufacturer_payload(b"NCTR\x01short"), None);
        assert_eq!(parse_manufacturer_payload(b"NCTR\x0212345678"), None);
        assert_eq!(parse_manufacturer_payload(b"XXXX\x0112345678"), None);
    }

    #[test]
    fn rssi_tracker_fires_walk_away_only_after_armed_near() {
        let mut tracker = RssiTracker::default();
        let peer = [1, 2, 3, 4, 5, 6, 7, 8];
        for _ in 0..4 {
            assert!(!tracker.record(peer, -90));
        }
        for _ in 0..4 {
            assert!(!tracker.record(peer, -65));
        }
        assert!(!tracker.record(peer, -88));
        assert!(!tracker.record(peer, -90));
        assert!(!tracker.record(peer, -92));
        assert!(tracker.record(peer, -90));
        assert!(!tracker.record(peer, -92));
    }

    #[test]
    fn rssi_tracker_arms_at_near_threshold_and_fires_once_on_walk_away() {
        let mut tracker = RssiTracker::default();
        let peer = [9, 8, 7, 6, 5, 4, 3, 2];
        // Exactly at NEAR_RSSI_DBM (-72) for 4 samples arms the tracker.
        for _ in 0..4 {
            assert!(!tracker.record(peer, NEAR_RSSI_DBM));
        }
        // Dropping to WALK_AWAY_RSSI_DBM (-85) across 4 samples triggers once.
        assert!(!tracker.record(peer, WALK_AWAY_RSSI_DBM));
        assert!(!tracker.record(peer, WALK_AWAY_RSSI_DBM));
        assert!(!tracker.record(peer, WALK_AWAY_RSSI_DBM));
        assert!(tracker.record(peer, WALK_AWAY_RSSI_DBM));
        // Subsequent distant samples do not re-trigger without re-arming near.
        for _ in 0..4 {
            assert!(!tracker.record(peer, -95));
        }
    }

    #[test]
    fn matches_only_paired_phones_or_g7link_uuid_and_ignores_peripherals() {
        let own_prefix = [0xAA; 8];
        let phone_prefix = [1, 2, 3, 4, 5, 6, 7, 8];
        let paired = vec![PairedPhone {
            prefix: phone_prefix,
            name: "Pixel 9 Pro".into(),
            model: Some("Pixel 9 Pro".into()),
        }];

        // Arbitrary Bluetooth headphones / mice must NEVER match.
        let headphones = BluezDeviceCandidate {
            address: Some("11:22:33:44:55:66".into()),
            name: Some("Sony WH-1000XM5".into()),
            alias: Some("Sony WH-1000XM5".into()),
            uuids: vec!["0000110b-0000-1000-8000-00805f9b34fb".into()],
            mfg_payload: None,
        };
        assert_eq!(match_g7link_device(&headphones, &own_prefix, &paired), None);

        // Paired phone matched by Name/Alias.
        let by_name = BluezDeviceCandidate {
            address: Some("AA:BB:CC:DD:EE:FF".into()),
            name: Some("pixel 9 pro".into()),
            alias: None,
            uuids: Vec::new(),
            mfg_payload: None,
        };
        assert_eq!(match_g7link_device(&by_name, &own_prefix, &paired), Some(phone_prefix));

        // Device advertising the G7 Link BLE service UUID matches.
        let by_uuid = BluezDeviceCandidate {
            address: Some("AA:BB:CC:DD:EE:FF".into()),
            name: Some("Android".into()),
            alias: None,
            uuids: vec![G7LINK_BLE_UUID.to_uppercase()],
            mfg_payload: None,
        };
        assert_eq!(match_g7link_device(&by_uuid, &own_prefix, &paired), Some(phone_prefix));
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

    /// Stands in for BlueZ's advertising manager: reads each beacon back the
    /// way BlueZ does and records what it was asked.
    #[derive(Clone, Default)]
    struct FakeManager {
        calls: Arc<Mutex<Vec<String>>>,
        /// Refuses a beacon with an advertising interval, as an older BlueZ may.
        reject_intervals: bool,
    }

    impl FakeManager {
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut *self.calls.lock().unwrap())
        }

        /// The calls so far, once `done` holds for them or five seconds have
        /// passed: well before the worker's 15-second check.
        fn wait_for(&self, done: impl Fn(&[String]) -> bool) -> Vec<String> {
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
            let call = format!("register {} {kind} {data:?} {interval:?}", path.as_str());
            self.calls.lock().unwrap().push(call);
            Ok(())
        }

        fn unregister_advertisement(&self, path: OwnedObjectPath) {
            self.calls.lock().unwrap().push(format!("unregister {}", path.as_str()));
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
            .serve_at("/org/bluez/hci0", FakeAdapter)
            .unwrap()
            .build()
            .unwrap()
    }

    fn test_env(bus: &TestBus) -> Env {
        let address = bus.address.clone();
        Env {
            connect: Arc::new(move || zbus::blocking::connection::Builder::address(address.as_str())),
            walk_away: || false,
        }
    }

    fn registered(path: &str, id: &DeviceId, interval: Option<(u32, u32)>) -> String {
        format!("register {path} broadcast {:?} {interval:?}", Some(build_manufacturer_payload(id).to_vec()))
    }

    fn registrations(calls: &[String]) -> usize {
        calls.iter().filter(|c| c.starts_with("register ")).count()
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
        let (conn, mut beacon) = Beacon::serve(bus.connect(), &id, Arc::default()).unwrap();
        let register = registered(ADVERTISEMENT_PATH, &id, Some((MIN_INTERVAL_MS, MAX_INTERVAL_MS)));
        let unregister = format!("unregister {ADVERTISEMENT_PATH}");
        let (on, off) = (adapter("/org/bluez/hci0", true, true), adapter("/org/bluez/hci0", false, true));

        beacon.sync(&conn, &on);
        beacon.sync(&conn, &on);
        assert_eq!(manager.take(), std::slice::from_ref(&register));

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
        beacon.sync(&conn, &on);
        assert_eq!(manager.take(), std::slice::from_ref(&register));

        // Bluetooth turned off, then on again.
        beacon.sync(&conn, &off);
        beacon.sync(&conn, &off);
        assert_eq!(manager.take(), std::slice::from_ref(&unregister));
        beacon.sync(&conn, &on);
        assert_eq!(manager.take(), [register]);

        beacon.stop(&conn);
        assert_eq!(manager.take(), [unregister]);
    }

    #[test]
    fn beacon_goes_without_the_fast_interval_when_bluez_refuses_it() {
        let Some(bus) = TestBus::start() else {
            eprintln!("skipped: dbus-daemon isn't installed");
            return;
        };
        let manager = FakeManager { reject_intervals: true, ..FakeManager::default() };
        let _bluez = fake_bluez(&bus, &manager);
        let id = DeviceId([0x6B; 32]);
        let (conn, mut beacon) = Beacon::serve(bus.connect(), &id, Arc::default()).unwrap();
        let on = adapter("/org/bluez/hci0", true, true);

        beacon.sync(&conn, &on);
        assert_eq!(manager.take(), [registered(DEFAULT_INTERVAL_PATH, &id, None)]);

        // Bluetooth turned off and on: straight to the beacon BlueZ took.
        beacon.sync(&conn, &adapter("/org/bluez/hci0", false, true));
        beacon.sync(&conn, &on);
        assert_eq!(
            manager.take(),
            [format!("unregister {DEFAULT_INTERVAL_PATH}"), registered(DEFAULT_INTERVAL_PATH, &id, None)]
        );

        beacon.stop(&conn);
        assert_eq!(manager.take(), [format!("unregister {DEFAULT_INTERVAL_PATH}")]);
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
        start_in(&slot, test_env(&bus), id);

        // BlueZ isn't running yet: the worker waits for it.
        thread::sleep(Duration::from_millis(300));
        assert!(slot.lock().unwrap().as_ref().is_some_and(|w: &Worker| !w.thread.is_finished()));

        let first = fake_bluez(&bus, &manager);
        let register = registered(ADVERTISEMENT_PATH, &id, Some((MIN_INTERVAL_MS, MAX_INTERVAL_MS)));
        assert_eq!(manager.wait_for(|calls| registrations(calls) >= 1), std::slice::from_ref(&register));

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
        start_in(&slot, test_env(&bus), id);
        start_in(&slot, test_env(&bus), id);
        stop_in(&slot);
        stop_in(&slot);
        assert!(slot.lock().unwrap().is_none());
    }

    fn no_bus() -> zbus::Result<zbus::blocking::connection::Builder<'static>> {
        Err(zbus::Error::Unsupported)
    }

    #[test]
    fn stop_ends_a_worker_that_has_no_bus() {
        let env = Env { connect: Arc::new(no_bus), walk_away: || false };
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
