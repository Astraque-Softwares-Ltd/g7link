// SPDX-License-Identifier: GPL-3.0-or-later
//! Paired Bluetooth device inspection and Hands-Free Profile (HFP) call audio
//! routing on Linux: BlueZ (`org.bluez` on the system D-Bus) connects the
//! phone's hands-free profile, and PipeWire/PulseAudio (`pactl`) switches its
//! card to call audio.

use std::collections::HashMap;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::i18n::tr;

const UUID_HFP_AG: &str = "0000111f-0000-1000-8000-00805f9b34fb";
const UUID_HFP_HF: &str = "0000111e-0000-1000-8000-00805f9b34fb";
const UUID_A2DP_SOURCE: &str = "0000110a-0000-1000-8000-00805f9b34fb";

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const MEDIA_TRANSPORT: &str = "org.bluez.MediaTransport1";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";

/// BlueZ answers everything but a connect at once.
const DBUS_TIMEOUT: Duration = Duration::from_secs(3);
/// A connect pages the phone, which can take several seconds.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
const PACTL_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the audio server gets to show the phone's card, then its call
/// audio, after it connects.
const CARD_WAIT: Duration = Duration::from_secs(3);
const CARD_POLL: Duration = Duration::from_millis(150);
/// The watcher tries the system bus again after this when it can't reach it.
const WATCH_RETRY: Duration = Duration::from_secs(30);

/// Card profiles that can carry a phone's calls, best first: PipeWire's, then
/// PulseAudio's. A2DP profiles carry media only. PipeWire's `audio-gateway`
/// carries media too, so calls are on the PC only once the card has a
/// hands-free node ([`has_call_audio_node`]).
const CALL_PROFILES: [&str; 5] =
    ["audio-gateway", "headset-audio-gateway", "headset_audio_gateway", "handsfree_head_unit", "hfp_hf"];

type ManagedObjectsMap = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BluezEndpoint {
    pub object_path: String,
    pub mac: String,
    pub name: String,
    pub connected: bool,
    /// The adapter it's paired through.
    pub adapter: Option<String>,
}

/// A G7 Link phone as Bluetooth knows it. Its device is the one at `address`
/// once that's recorded, and until then the one device its `names` match: never
/// one picked by a name the phone reports later.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BtPhone {
    /// Its device id in G7 Link.
    pub id: String,
    /// What it's called in messages.
    pub label: String,
    /// The Bluetooth address recorded for its device.
    pub address: Option<String>,
    /// The names its device may be paired under.
    pub names: Vec<String>,
    /// Its media stays on the phone rather than playing here.
    pub media_on_phone: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HfpStatus {
    /// The phone is paired over Bluetooth and Bluetooth is on.
    pub available: bool,
    /// The phone's Bluetooth link is up, for any profile (media, calls...).
    pub link_connected: bool,
    /// The phone's calls play on this PC: the audio server has a hands-free
    /// (SCO) node for it, not just media.
    pub call_audio_ready: bool,
    pub device_name: String,
    pub detail: String,
    /// The Bluetooth address of the phone's device, when it was found.
    pub address: Option<String>,
}

/// Why call audio couldn't be connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectFailure {
    /// BlueZ isn't running, or the system bus can't be reached.
    NotRunning,
    Off,
    NotPaired,
    Unreachable,
    Unsupported,
    /// The audio server refused the call profile.
    NotSwitched,
    /// The phone connected but keeps its calls' audio.
    StillOnPhone,
}

impl ConnectFailure {
    fn message(self, name: &str) -> String {
        match self {
            ConnectFailure::NotRunning => tr!("Calls", "Bluetooth isn't running on this PC."),
            ConnectFailure::Off => tr!("Calls", "Bluetooth is off."),
            ConnectFailure::NotPaired => tr!("Calls", "%1 isn't paired with this PC over Bluetooth.", name),
            ConnectFailure::Unreachable => tr!("Calls", "Couldn't reach %1 over Bluetooth.", name),
            ConnectFailure::Unsupported => tr!("Calls", "This PC can't take call audio over Bluetooth."),
            ConnectFailure::NotSwitched => tr!("Calls", "Couldn't switch this PC to call audio."),
            ConnectFailure::StillOnPhone => tr!("Calls", "%1 is still using its own audio.", name),
        }
    }
}

/// The phone whose card this app switched to call audio, and the profile it
/// was on before, to put back when the call ends.
struct Held {
    mac: String,
    previous: Option<String>,
}

static HELD: Mutex<Option<Held>> = Mutex::new(None);

fn held<T>(f: impl FnOnce(&mut Option<Held>) -> T) -> T {
    f(&mut HELD.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Builds every system bus connection this module makes.
fn system_builder() -> zbus::Result<zbus::blocking::connection::Builder<'static>> {
    zbus::blocking::connection::Builder::system()
}

/// The shared system bus connection whose calls give up after `timeout`
/// ([`DBUS_TIMEOUT`] or [`CONNECT_TIMEOUT`]), so a stuck `bluetoothd` can't
/// hang a thread.
fn system_bus(timeout: Duration) -> Option<zbus::blocking::Connection> {
    static SHORT: OnceLock<zbus::blocking::Connection> = OnceLock::new();
    static LONG: OnceLock<zbus::blocking::Connection> = OnceLock::new();
    let slot = if timeout == CONNECT_TIMEOUT { &LONG } else { &SHORT };
    if let Some(conn) = slot.get() {
        return Some(conn.clone());
    }
    let conn = super::non_tokio(|| system_builder().and_then(|b| b.method_timeout(timeout).build()))
        .inspect_err(|error| tracing::debug!(%error, "Bluetooth call audio: no system bus"))
        .ok()?;
    Some(slot.get_or_init(|| conn).clone())
}

/// Runs `pactl` in the C locale (its headings are translated otherwise), or
/// `None` when it's missing, fails or takes longer than [`PACTL_TIMEOUT`].
fn pactl(args: &[&str]) -> Option<String> {
    let mut full = vec!["LC_ALL=C", "pactl"];
    full.extend_from_slice(args);
    super::run_cmd_timeout("env", &full, PACTL_TIMEOUT)
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
}

pub(crate) fn is_valid_bt_mac(mac: &str) -> bool {
    !mac.is_empty()
        && !mac.starts_with('-')
        && mac.bytes().all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'-')
}

/// Formats a Bluetooth MAC address (`AA:BB:CC:DD:EE:FF`) into its PipeWire /
/// PulseAudio BlueZ card name (`bluez_card.AA_BB_CC_DD_EE_FF`).
pub(crate) fn bluez_card_name_for_mac(mac: &str) -> String {
    let underscored: String =
        mac.trim().chars().map(|c| if c == ':' || c == '-' { '_' } else { c }).collect();
    format!("bluez_card.{underscored}")
}

/// Selects the paired Bluetooth `(mac, name)` endpoint whose name matches
/// `phone_name` (case-insensitive substring match in either direction).
/// Never falls back to an unrelated device (e.g. a Bluetooth mouse or headset).
pub fn match_endpoint<'a>(
    endpoints: &'a [(String, String)],
    phone_name: Option<&str>,
) -> Option<&'a (String, String)> {
    if endpoints.is_empty() {
        return None;
    }
    let needle = phone_name.map(str::trim).filter(|s| !s.is_empty())?;
    let needle_lower = needle.to_ascii_lowercase();
    endpoints.iter().find(|(_, name)| {
        let hay = name.trim().to_ascii_lowercase();
        !hay.is_empty() && (hay.contains(&needle_lower) || needle_lower.contains(&hay))
    })
}

/// `phone`'s endpoint: the one at its recorded address, or else the one its names
/// match that no other of `phones` has recorded. When several match, the one
/// named exactly so is taken, and none rather than a guess between several.
pub(crate) fn phone_endpoint<'a>(
    endpoints: &'a [BluezEndpoint],
    phone: &BtPhone,
    phones: &[BtPhone],
) -> Option<&'a BluezEndpoint> {
    if let Some(address) = &phone.address {
        return endpoints.iter().find(|e| e.mac.eq_ignore_ascii_case(address));
    }
    let recorded = |e: &BluezEndpoint| {
        let at = |p: &BtPhone| p.address.as_ref().is_some_and(|a| e.mac.eq_ignore_ascii_case(a));
        phones.iter().any(|p| p.id != phone.id && at(p))
    };
    let matches: Vec<&BluezEndpoint> = endpoints
        .iter()
        .filter(|&e| !recorded(e))
        .filter(|e| {
            let pair = [(e.mac.clone(), e.name.clone())];
            phone.names.iter().any(|n| match_endpoint(&pair, Some(n)).is_some())
        })
        .collect();
    if let [only] = matches.as_slice() {
        return Some(*only);
    }
    let exact: Vec<&BluezEndpoint> = matches
        .into_iter()
        .filter(|e| phone.names.iter().any(|n| e.name.trim().eq_ignore_ascii_case(n.trim())))
        .collect();
    match exact.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// The phones of `phones` with no address recorded whose device was found now,
/// with its address. One found for two phones is recorded for neither.
pub(crate) fn found_addresses(endpoints: &[BluezEndpoint], phones: &[BtPhone]) -> Vec<(String, String)> {
    let found: Vec<(String, String)> = phones
        .iter()
        .filter(|p| p.address.is_none())
        .filter_map(|p| Some((p.id.clone(), phone_endpoint(endpoints, p, phones)?.mac.clone())))
        .collect();
    found.iter().filter(|(_, mac)| found.iter().filter(|(_, m)| m == mac).count() == 1).cloned().collect()
}

pub(crate) fn is_phone_capable_bluez_device(paired: bool, uuids: &[String]) -> bool {
    if !paired {
        return false;
    }
    if uuids.is_empty() {
        return true;
    }
    uuids.iter().any(|u| {
        let lower = u.trim().to_ascii_lowercase();
        lower == UUID_HFP_AG || lower == UUID_HFP_HF || lower == UUID_A2DP_SOURCE
    })
}

pub(crate) fn parse_bluez_managed_objects(objects: &ManagedObjectsMap) -> Vec<BluezEndpoint> {
    let mut out = Vec::new();
    for (path, ifaces) in objects {
        let Some(dev) = ifaces.get(DEVICE) else {
            continue;
        };
        let paired = dev.get("Paired").and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
        let uuids: Vec<String> = dev
            .get("UUIDs")
            .and_then(|v| v.try_clone().ok())
            .and_then(|owned| Vec::<String>::try_from(owned).ok())
            .unwrap_or_default();
        if !is_phone_capable_bluez_device(paired, &uuids) {
            continue;
        }
        let Some(mac) = dev
            .get("Address")
            .and_then(|v| <&str>::try_from(v).ok())
            .map(str::trim)
            .filter(|m| is_valid_bt_mac(m))
        else {
            continue;
        };
        let name = dev
            .get("Alias")
            .and_then(|v| <&str>::try_from(v).ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                dev.get("Name")
                    .and_then(|v| <&str>::try_from(v).ok())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_default()
            .to_owned();
        let connected = dev.get("Connected").and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
        let adapter = dev.get("Adapter").and_then(|v| match &**v {
            Value::ObjectPath(p) => Some(p.as_str().to_owned()),
            _ => None,
        });
        out.push(BluezEndpoint {
            object_path: path.as_str().to_owned(),
            mac: mac.to_owned(),
            name,
            connected,
            adapter,
        });
    }
    out.sort_by(|a, b| a.mac.cmp(&b.mac));
    out
}

/// Whether the adapter at `path` is on; `None` when BlueZ doesn't list it.
fn adapter_powered(objects: &ManagedObjectsMap, path: Option<&str>) -> Option<bool> {
    let (_, ifaces) = objects.iter().find(|(p, _)| Some(p.as_str()) == path)?;
    ifaces.get(ADAPTER)?.get("Powered").and_then(|v| bool::try_from(v).ok())
}

/// BlueZ's devices and adapters, or `None` when BlueZ isn't running (a call
/// would otherwise start it, or wait for it).
fn managed_objects(conn: &zbus::blocking::Connection) -> Option<ManagedObjectsMap> {
    super::non_tokio(|| {
        if !super::dbus_name_has_owner(conn, BLUEZ) {
            return None;
        }
        let reply =
            conn.call_method(Some(BLUEZ), "/", Some(OBJECT_MANAGER), "GetManagedObjects", &()).ok()?;
        let body = reply.body();
        body.deserialize::<ManagedObjectsMap>()
            .ok()
            .or_else(|| body.deserialize::<(ManagedObjectsMap,)>().ok().map(|(m,)| m))
    })
}

fn display_name(endpoint: &BluezEndpoint, phone_name: Option<&str>) -> String {
    if !endpoint.name.trim().is_empty() {
        return endpoint.name.clone();
    }
    phone_name.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(endpoint.mac.as_str()).to_owned()
}

/// What a `ConnectProfile` error from BlueZ means for call audio.
pub(crate) fn connect_result(error_name: Option<&str>) -> Result<(), ConnectFailure> {
    let Some(name) = error_name else { return Ok(()) };
    match name.strip_prefix("org.bluez.Error.") {
        Some("AlreadyConnected" | "InProgress") => Ok(()),
        Some("NotReady") => Err(ConnectFailure::Off),
        Some("NotAvailable" | "NotSupported") => Err(ConnectFailure::Unsupported),
        _ => Err(ConnectFailure::Unreachable),
    }
}

/// Connects the phone's hands-free profile: `short` reads BlueZ's objects,
/// `long` makes the connect (it pages the phone). A failure comes with the
/// phone's name when it was found.
pub(crate) fn connect_link(
    short: &zbus::blocking::Connection,
    long: &zbus::blocking::Connection,
    phone: Option<&BtPhone>,
    phones: &[BtPhone],
) -> Result<BluezEndpoint, (ConnectFailure, Option<String>)> {
    let objects = managed_objects(short).ok_or((ConnectFailure::NotRunning, None))?;
    let endpoints = parse_bluez_managed_objects(&objects);
    let endpoint = phone
        .and_then(|phone| phone_endpoint(&endpoints, phone, phones))
        .cloned()
        .ok_or((ConnectFailure::NotPaired, None))?;
    let display = display_name(&endpoint, phone.map(|p| p.label.as_str()));
    if adapter_powered(&objects, endpoint.adapter.as_deref()) == Some(false) {
        return Err((ConnectFailure::Off, Some(display)));
    }
    let path = endpoint.object_path.as_str();
    let reply = super::non_tokio(|| {
        long.call_method(Some(BLUEZ), path, Some(DEVICE), "ConnectProfile", &(UUID_HFP_AG,)).map(|_| ())
    });
    let error_name = match &reply {
        Ok(()) => None,
        Err(zbus::Error::MethodError(name, _, _)) => Some(name.as_str()),
        Err(error) => {
            tracing::info!(%error, "Bluetooth call audio: connect failed");
            Some("")
        }
    };
    match connect_result(error_name) {
        Ok(()) => Ok(endpoint),
        Err(failure) => Err((failure, Some(display))),
    }
}

/// A card as `pactl list cards` describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Card {
    /// Its profiles, with whether each is available now.
    pub profiles: Vec<(String, bool)>,
    pub active: Option<String>,
}

/// Finds `card` in `pactl list cards` output (in the C locale).
pub(crate) fn parse_card(output: &str, card: &str) -> Option<Card> {
    let indent = |line: &str| line.len() - line.trim_start().len();
    let mut found: Option<Card> = None;
    let mut profiles_indent: Option<usize> = None;
    for line in output.lines() {
        let trimmed = line.trim();
        if line.starts_with("Card #") {
            if found.is_some() {
                break;
            }
            continue;
        }
        if found.is_none() {
            if trimmed.strip_prefix("Name:").map(str::trim) == Some(card) {
                found = Some(Card::default());
            }
            continue;
        }
        let Some(current) = found.as_mut() else { continue };
        if let Some(at) = profiles_indent {
            if indent(line) > at && !trimmed.is_empty() {
                if let Some((name, rest)) = trimmed.split_once(": ") {
                    current.profiles.push((name.to_owned(), !rest.contains("available: no")));
                }
                continue;
            }
            profiles_indent = None;
        }
        if trimmed == "Profiles:" {
            profiles_indent = Some(indent(line));
        } else if let Some(active) = trimmed.strip_prefix("Active Profile:") {
            current.active = Some(active.trim().to_owned()).filter(|a| !a.is_empty());
        }
    }
    found
}

/// The best available call profile the card has; never an A2DP one.
pub(crate) fn pick_call_profile(card: &Card) -> Option<&'static str> {
    let available = |wanted: &str| card.profiles.iter().any(|(name, on)| name == wanted && *on);
    CALL_PROFILES.into_iter().find(|wanted| available(wanted))
}

fn card_listed(short_list: &str, card: &str) -> bool {
    short_list.lines().any(|line| line.split_whitespace().nth(1) == Some(card))
}

/// Waits up to [`CARD_WAIT`] for the audio server to show `card`.
fn wait_for_card(card: &str) -> bool {
    let deadline = Instant::now() + CARD_WAIT;
    loop {
        if pactl(&["list", "cards", "short"]).is_some_and(|list| card_listed(&list, card)) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(CARD_POLL);
    }
}

fn read_card(card: &str) -> Option<Card> {
    parse_card(&pactl(&["list", "cards"])?, card)
}

/// The properties of each sink or source in `pactl list sinks`/`sources`
/// output (in the C locale).
fn node_properties(output: &str) -> Vec<HashMap<&str, &str>> {
    let mut nodes: Vec<HashMap<&str, &str>> = Vec::new();
    for line in output.lines() {
        if line.starts_with(|c: char| !c.is_whitespace()) {
            nodes.push(HashMap::new());
        } else if let (Some(node), Some((key, value))) = (nodes.last_mut(), line.trim().split_once(" = ")) {
            node.insert(key, value.trim_matches('"'));
        }
    }
    nodes
}

/// Whether `pactl list sinks`/`sources` output has a hands-free node for the
/// phone at `mac`: one PipeWire made for its call audio (SCO), not for media.
pub(crate) fn has_call_audio_node(output: &str, mac: &str) -> bool {
    node_properties(output).iter().any(|node| {
        let get = |key: &str| node.get(key).copied().unwrap_or_default();
        let profile = get("api.bluez5.profile");
        let sco = get("factory.name").contains(".sco.");
        let hands_free = sco || profile.contains("headset") || profile.contains("hfp");
        hands_free && get("api.bluez5.address").eq_ignore_ascii_case(mac)
    })
}

/// Whether the audio server has a hands-free node for the phone at `mac`.
fn call_audio_node(mac: &str) -> bool {
    ["sinks", "sources"]
        .into_iter()
        .any(|kind| pactl(&["list", kind]).is_some_and(|out| has_call_audio_node(&out, mac)))
}

/// Waits up to [`CARD_WAIT`] for the phone's hands-free node.
fn wait_for_call_audio(mac: &str) -> bool {
    let deadline = Instant::now() + CARD_WAIT;
    loop {
        if call_audio_node(mac) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(CARD_POLL);
    }
}

/// Puts `mac`'s card on a call profile, remembering the profile it was on.
/// PipeWire's `audio-gateway` is usually on already, and isn't switched.
fn route_call_audio(mac: &str) -> Result<(), ConnectFailure> {
    let card_name = bluez_card_name_for_mac(mac);
    if !wait_for_card(&card_name) {
        return Err(ConnectFailure::Unsupported);
    }
    let card = read_card(&card_name).ok_or(ConnectFailure::Unsupported)?;
    let profile = pick_call_profile(&card).ok_or(ConnectFailure::Unsupported)?;
    if card.active.as_deref() != Some(profile) {
        if pactl(&["set-card-profile", &card_name, profile]).is_none() {
            return Err(ConnectFailure::NotSwitched);
        }
        held(|slot| {
            if slot.as_ref().is_none_or(|h| h.mac != mac) {
                *slot = Some(Held { mac: mac.to_owned(), previous: card.active.clone() });
            }
        });
    }
    Ok(())
}

/// What the phone's Bluetooth link and audio add up to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Link {
    pub powered: bool,
    pub linked: bool,
    /// The audio server has a hands-free node for it.
    pub call_node: bool,
    /// A call is ringing or going on on the phone.
    pub in_call: bool,
}

pub(crate) fn status_of(name: &str, link: Link) -> HfpStatus {
    let Link { powered, linked, call_node, in_call } = link;
    let call_audio_ready = powered && linked && call_node;
    let detail = if !powered {
        tr!("Calls", "Bluetooth is off.")
    } else if call_audio_ready {
        tr!("Calls", "On PC over Bluetooth (%1).", name)
    } else if linked && in_call {
        ConnectFailure::StillOnPhone.message(name)
    } else if linked {
        tr!("Calls", "Connected for media only (%1).", name)
    } else {
        tr!("Calls", "%1 is paired but not connected.", name)
    };
    HfpStatus {
        available: powered,
        link_connected: powered && linked,
        call_audio_ready,
        device_name: name.to_owned(),
        detail,
        address: None,
    }
}

fn not_available(phone_name: Option<&str>, detail: String) -> HfpStatus {
    HfpStatus { device_name: phone_name.unwrap_or_default().to_owned(), detail, ..HfpStatus::default() }
}

/// The phone's call audio status; `in_call` says whether a call is ringing or
/// going on on it.
pub fn probe_status(phone: Option<&BtPhone>, phones: &[BtPhone], in_call: bool) -> HfpStatus {
    let phone_name = phone.map(|p| p.label.as_str());
    let Some(objects) = system_bus(DBUS_TIMEOUT).and_then(|conn| managed_objects(&conn)) else {
        return not_available(phone_name, ConnectFailure::NotRunning.message(""));
    };
    let endpoints = parse_bluez_managed_objects(&objects);
    let Some(endpoint) = phone.and_then(|phone| phone_endpoint(&endpoints, phone, phones)) else {
        return not_available(
            phone_name,
            tr!("Calls", "Pair your phone over system Bluetooth to route call audio through this PC."),
        );
    };
    let display = display_name(endpoint, phone_name);
    let powered = adapter_powered(&objects, endpoint.adapter.as_deref()) != Some(false);
    let linked = endpoint.connected;
    let call_node = powered && linked && call_audio_node(&endpoint.mac);
    let status = status_of(&display, Link { powered, linked, call_node, in_call });
    HfpStatus { address: Some(endpoint.mac.clone()), ..status }
}

/// Connects the phone's call audio to this PC; `in_call` says whether a call
/// is ringing or going on on it. Outside a call the phone may only open its
/// call audio when one comes, so a missing hands-free node isn't a failure then.
pub fn connect_audio(
    phone: Option<&BtPhone>,
    phones: &[BtPhone],
    in_call: bool,
) -> Result<HfpStatus, String> {
    let phone_name = phone.map(|p| p.label.as_str());
    let fallback = phone_name.map(str::trim).filter(|s| !s.is_empty()).unwrap_or_default().to_owned();
    let (Some(short), Some(long)) = (system_bus(DBUS_TIMEOUT), system_bus(CONNECT_TIMEOUT)) else {
        return Err(ConnectFailure::NotRunning.message(&fallback));
    };
    let endpoint = connect_link(&short, &long, phone, phones)
        .map_err(|(failure, display)| failure.message(display.as_deref().unwrap_or(&fallback)))?;
    let display = display_name(&endpoint, phone_name);
    route_call_audio(&endpoint.mac).map_err(|failure| failure.message(&display))?;
    let call_node = wait_for_call_audio(&endpoint.mac);
    if in_call && !call_node {
        return Err(ConnectFailure::StillOnPhone.message(&display));
    }
    let status = status_of(&display, Link { powered: true, linked: true, call_node, in_call });
    Ok(HfpStatus { address: Some(endpoint.mac), ..status })
}

/// Whether this app switched a phone's card to call audio and hasn't put it back.
pub fn holds_call_audio() -> bool {
    held(|slot| slot.is_some())
}

/// Puts the phone's card back on the profile it had before call audio.
pub fn disconnect_audio() {
    let Some(Held { mac, previous }) = held(Option::take) else {
        return;
    };
    if let Some(previous) = previous {
        let card = bluez_card_name_for_mac(&mac);
        if pactl(&["set-card-profile", &card, &previous]).is_none() {
            tracing::info!(card = %card, "Bluetooth call audio: couldn't put the card back");
        }
    }
}

/// A2DP sink: what this PC offers a phone that plays media to it.
const UUID_A2DP_SINK: &str = "0000110b-0000-1000-8000-00805f9b34fb";

/// The paired phones, to tell them from other Bluetooth devices, and what
/// records the address a phone's device is found at.
static PHONES: OnceLock<(fn() -> Vec<BtPhone>, fn(&str, &str))> = OnceLock::new();

/// Sends media back to the phones that may no longer play it here, after one
/// of them stopped being allowed to.
pub fn recheck_phone_media() {
    if PHONES.get().is_none() {
        return;
    }
    let _ = std::thread::Builder::new().name("bt-media-off".into()).spawn(|| {
        if let Some(conn) = system_bus(CONNECT_TIMEOUT) {
            drop_phone_media(&conn);
        }
    });
}

/// Keeps paired phones' media on the phone. A phone that connects to this PC
/// over Bluetooth (for calls, or by the user) also opens a media link, and
/// Android then plays everything here instead of on its headphones. For the
/// `phones` whose media stays on the phone, that link is closed as it opens;
/// calls and other devices aren't touched. `found` records the address a
/// phone's device is found at. One thread for the app's lifetime.
pub fn keep_phone_media_on_phone(phones: fn() -> Vec<BtPhone>, found: fn(&str, &str)) {
    if PHONES.set((phones, found)).is_err() {
        return;
    }
    let _ = std::thread::Builder::new().name("bt-media-guard".into()).spawn(|| {
        loop {
            guard_phone_media();
            std::thread::sleep(WATCH_RETRY);
        }
    });
}

/// Follows media links opening until the system bus connection ends.
fn guard_phone_media() {
    let Ok(conn) = system_builder().and_then(|b| b.method_timeout(CONNECT_TIMEOUT).build()) else {
        return;
    };
    let messages = zbus::blocking::MessageIterator::from(&conn);
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else {
        return;
    };
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(BLUEZ)
        .and_then(|b| b.interface(OBJECT_MANAGER))
        .and_then(|b| b.member("InterfacesAdded"))
        .map(|b| b.build());
    let Ok(rule) = rule else { return };
    if let Err(error) = dbus.add_match_rule(rule) {
        tracing::debug!(%error, "Bluetooth: can't follow media links");
        return;
    }
    drop_phone_media(&conn);
    for message in messages {
        let Ok(message) = message else {
            break;
        };
        let header = message.header();
        if header.member().is_none_or(|m| m.as_str() != "InterfacesAdded") {
            continue;
        }
        let opened = message
            .body()
            .deserialize::<(OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>)>()
            .is_ok_and(|(_, interfaces)| interfaces.contains_key(MEDIA_TRANSPORT));
        if opened {
            drop_phone_media(&conn);
        }
    }
}

/// A media link open to this PC: the device it comes from and its profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MediaLink {
    pub device: String,
    pub uuid: String,
}

fn media_links(objects: &ManagedObjectsMap) -> Vec<MediaLink> {
    objects
        .values()
        .filter_map(|ifaces| {
            let transport = ifaces.get(MEDIA_TRANSPORT)?;
            let device = match &**transport.get("Device")? {
                Value::ObjectPath(p) => p.as_str().to_owned(),
                _ => return None,
            };
            let uuid = <&str>::try_from(&**transport.get("UUID")?).ok()?.to_ascii_lowercase();
            Some(MediaLink { device, uuid })
        })
        .collect()
}

/// The endpoints with a media link to this PC of the `phones` whose media stays
/// on the phone, never another device.
pub(crate) fn phones_playing_here(
    links: &[MediaLink],
    endpoints: &[BluezEndpoint],
    phones: &[BtPhone],
) -> Vec<String> {
    let mut playing: Vec<String> = phones
        .iter()
        .filter(|p| p.media_on_phone)
        .filter_map(|p| phone_endpoint(endpoints, p, phones))
        .filter(|e| {
            links.iter().any(|l| {
                l.device == e.object_path && (l.uuid == UUID_A2DP_SINK || l.uuid == UUID_A2DP_SOURCE)
            })
        })
        .map(|e| e.object_path.clone())
        .collect();
    playing.sort();
    playing.dedup();
    playing
}

/// Closes the media link of every phone in `PHONES` that keeps its media and plays here.
fn drop_phone_media(conn: &zbus::blocking::Connection) {
    let Some(&(phones, found)) = PHONES.get() else { return };
    let phones = phones();
    if !phones.iter().any(|p| p.media_on_phone) {
        return;
    }
    let Some(objects) = managed_objects(conn) else { return };
    let endpoints = parse_bluez_managed_objects(&objects);
    for (id, address) in found_addresses(&endpoints, &phones) {
        found(&id, &address);
    }
    for device in phones_playing_here(&media_links(&objects), &endpoints, &phones) {
        let path = device.as_str();
        let result = super::non_tokio(|| {
            conn.call_method(Some(BLUEZ), path, Some(DEVICE), "DisconnectProfile", &(UUID_A2DP_SOURCE,))
                .map(|_| ())
        });
        match result {
            Ok(()) => tracing::info!("Bluetooth: kept a phone's media on the phone"),
            Err(error) => tracing::info!(%error, "Bluetooth: couldn't keep a phone's media on the phone"),
        }
    }
}

/// Calls `changed` whenever Bluetooth changes in a way that may change call
/// audio: BlueZ starts or stops, an adapter is turned on or off, a device
/// connects, disconnects or is paired, or an audio link comes or goes. One
/// thread for the app's lifetime; later calls do nothing.
pub fn watch(changed: fn()) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new().name("hfp-watch".into()).spawn(move || {
        let mut first = true;
        loop {
            follow_bluez(changed, first);
            first = false;
            std::thread::sleep(WATCH_RETRY);
        }
    });
    if spawned.is_err() {
        STARTED.store(false, Ordering::Release);
    }
}

/// Follows BlueZ's signals until the system bus connection ends.
fn follow_bluez(changed: fn(), first: bool) {
    let Ok(conn) = system_builder().and_then(|b| b.method_timeout(DBUS_TIMEOUT).build()) else {
        return;
    };
    // Made before subscribing, so it has every signal from then on.
    let messages = zbus::blocking::MessageIterator::from(&conn);
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else {
        return;
    };
    for rule in watched_signals() {
        if let Err(error) = dbus.add_match_rule(rule) {
            tracing::debug!(%error, "Bluetooth call audio: can't follow one of BlueZ's signals");
        }
    }
    // Anything may have changed while the bus was out of reach.
    if !first {
        changed();
    }
    for message in messages {
        let Ok(message) = message else {
            break;
        };
        if is_call_audio_change(&message) {
            changed();
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
    ]
    .into_iter()
    .filter_map(|rule| rule.map(|b| b.build()).ok())
    .collect()
}

fn is_watched_interface(name: &str) -> bool {
    name == ADAPTER || name == DEVICE || name == MEDIA_TRANSPORT
}

/// Whether `message` may change call audio. A device's signal strength and
/// the like change often and don't.
fn is_call_audio_change(message: &zbus::Message) -> bool {
    let header = message.header();
    let (Some(interface), Some(member)) = (header.interface(), header.member()) else {
        return false;
    };
    let body = message.body();
    match (interface.as_str(), member.as_str()) {
        ("org.freedesktop.DBus", "NameOwnerChanged") => {
            body.deserialize::<(String, String, String)>().is_ok_and(|(name, _, _)| name == BLUEZ)
        }
        ("org.freedesktop.DBus.Properties", "PropertiesChanged") => {
            let Ok((interface, changed, invalidated)) =
                body.deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>()
            else {
                return false;
            };
            let keys: &[&str] = match interface.as_str() {
                ADAPTER => &["Powered"],
                DEVICE => &["Connected", "Paired", "ServicesResolved", "Alias", "UUIDs"],
                _ => return false,
            };
            changed.keys().chain(invalidated.iter()).any(|key| keys.contains(&key.as_str()))
        }
        (OBJECT_MANAGER, "InterfacesAdded") => body
            .deserialize::<(OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>)>()
            .is_ok_and(|(_, interfaces)| interfaces.keys().any(|i| is_watched_interface(i))),
        (OBJECT_MANAGER, "InterfacesRemoved") => body
            .deserialize::<(OwnedObjectPath, Vec<String>)>()
            .is_ok_and(|(_, interfaces)| interfaces.iter().any(|i| is_watched_interface(i))),
        _ => false,
    }
}

pub fn open_bluetooth_settings() {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| std::env::var("DESKTOP_SESSION"))
        .unwrap_or_default()
        .to_ascii_lowercase();
    let is_kde = desktop.contains("kde") || desktop.contains("plasma");

    if is_kde
        && (Command::new("systemsettings").arg("kcm_bluetooth").spawn().is_ok()
            || Command::new("kcmshell6").arg("kcm_bluetooth").spawn().is_ok())
    {
        return;
    }

    let _ = Command::new("gnome-control-center")
        .arg("bluetooth")
        .spawn()
        .or_else(|_| Command::new("systemsettings").arg("kcm_bluetooth").spawn())
        .or_else(|_| Command::new("kcmshell6").arg("kcm_bluetooth").spawn())
        .or_else(|_| Command::new("blueman-manager").spawn())
        .or_else(|_| Command::new("overskride").spawn())
        .or_else(|_| Command::new("xdg-open").arg("settings://bluetooth").spawn());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(path: &str, mac: &str, name: &str) -> BluezEndpoint {
        BluezEndpoint {
            object_path: path.into(),
            mac: mac.into(),
            name: name.into(),
            connected: true,
            adapter: Some("/org/bluez/hci0".into()),
        }
    }

    fn phone(id: &str, names: &[&str], address: Option<&str>) -> BtPhone {
        BtPhone {
            id: id.into(),
            label: names.first().copied().unwrap_or_default().into(),
            address: address.map(Into::into),
            names: names.iter().map(|&n| n.into()).collect(),
            media_on_phone: true,
        }
    }

    #[test]
    fn only_a_paired_phone_playing_here_loses_its_media_link() {
        let pixel = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_01";
        let tablet = "/org/bluez/hci0/dev_11_22_33_44_55_66";
        let endpoints = [
            endpoint(pixel, "AA:BB:CC:DD:EE:01", "Pixel 8 Pro"),
            endpoint(tablet, "11:22:33:44:55:66", "Galaxy Tab"),
        ];
        let links = [
            MediaLink { device: pixel.into(), uuid: UUID_A2DP_SINK.into() },
            MediaLink { device: tablet.into(), uuid: UUID_A2DP_SINK.into() },
        ];
        let phones = [phone("a", &["Pixel 8 Pro"], None)];
        assert_eq!(phones_playing_here(&links, &endpoints, &phones), vec![pixel.to_owned()]);
        // No media link, nothing to close; a call's link isn't media.
        let call = [MediaLink { device: pixel.into(), uuid: UUID_HFP_AG.into() }];
        assert!(phones_playing_here(&call, &endpoints, &phones).is_empty());
        assert!(phones_playing_here(&[], &endpoints, &phones).is_empty());
        // No paired phones: never another device's.
        assert!(phones_playing_here(&links, &endpoints, &[]).is_empty());
        // A phone whose media may play here keeps its link.
        let allowed = [BtPhone { media_on_phone: false, ..phone("a", &["Pixel 8 Pro"], None) }];
        assert!(phones_playing_here(&links, &endpoints, &allowed).is_empty());
    }

    #[test]
    fn a_recorded_address_finds_the_phone_whatever_it_is_called() {
        let endpoints = [
            endpoint("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_01", "AA:BB:CC:DD:EE:01", "Pixel 8 Pro"),
            endpoint("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_02", "AA:BB:CC:DD:EE:02", "Galaxy S24"),
        ];
        // Its device is found by its address, not by a name naming another device.
        let pixel = phone("a", &["Galaxy S24"], Some("aa:bb:cc:dd:ee:01"));
        let found = phone_endpoint(&endpoints, &pixel, std::slice::from_ref(&pixel)).unwrap();
        assert_eq!(found.mac, "AA:BB:CC:DD:EE:01");
        // A recorded address that's no longer paired finds nothing.
        let gone = phone("a", &["Pixel 8 Pro"], Some("AA:BB:CC:DD:EE:09"));
        assert!(phone_endpoint(&endpoints, &gone, std::slice::from_ref(&gone)).is_none());
    }

    #[test]
    fn a_name_never_finds_another_phones_device() {
        let galaxy = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_02";
        let endpoints = [
            endpoint("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_01", "AA:BB:CC:DD:EE:01", "Pixel 8 Pro"),
            endpoint(galaxy, "AA:BB:CC:DD:EE:02", "Galaxy S24"),
        ];
        let links = [MediaLink { device: galaxy.into(), uuid: UUID_A2DP_SINK.into() }];
        // The Galaxy is another G7 Link phone's, recorded: a phone named like it doesn't get it.
        let owner = phone("b", &["Galaxy S24"], Some("AA:BB:CC:DD:EE:02"));
        let owner = BtPhone { media_on_phone: false, ..owner };
        let copycat = phone("a", &["Galaxy S24"], None);
        let phones = [copycat.clone(), owner];
        assert!(phone_endpoint(&endpoints, &copycat, &phones).is_none());
        assert!(phones_playing_here(&links, &endpoints, &phones).is_empty());
        assert!(found_addresses(&endpoints, &phones).is_empty());
    }

    #[test]
    fn several_devices_a_name_matches_are_not_guessed_between() {
        let endpoints = [
            endpoint("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_01", "AA:BB:CC:DD:EE:01", "Pixel 8 Pro"),
            endpoint("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_02", "AA:BB:CC:DD:EE:02", "Pixel 8"),
            endpoint("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_03", "AA:BB:CC:DD:EE:03", "Pixel 8a"),
        ];
        // "Pixel" is in all three: none of them.
        let vague = phone("a", &["Pixel"], None);
        assert!(phone_endpoint(&endpoints, &vague, std::slice::from_ref(&vague)).is_none());
        // One is named exactly so.
        let exact = phone("a", &["pixel 8"], None);
        let found = phone_endpoint(&endpoints, &exact, std::slice::from_ref(&exact)).unwrap();
        assert_eq!(found.mac, "AA:BB:CC:DD:EE:02");
        assert_eq!(
            found_addresses(&endpoints, std::slice::from_ref(&exact)),
            vec![("a".to_owned(), "AA:BB:CC:DD:EE:02".to_owned())]
        );
        // Two phones finding the same device: it's recorded for neither.
        let twin = phone("b", &["Pixel 8"], None);
        assert!(found_addresses(&endpoints, &[exact, twin]).is_empty());
    }
    use std::sync::Arc;

    #[test]
    fn match_endpoint_prefers_phone_name_case_insensitive() {
        let list = vec![
            ("id-1".to_string(), "Galaxy Buds".to_string()),
            ("id-2".to_string(), "Pixel 9 Pro".to_string()),
        ];
        let matched = match_endpoint(&list, Some("pixel 9")).unwrap();
        assert_eq!(matched.0, "id-2");
    }

    #[test]
    fn match_endpoint_returns_none_when_unmatched_or_missing_name() {
        let list = vec![("id-1".to_string(), "MX Master 3S".to_string())];
        assert!(match_endpoint(&list, Some("Pixel 9 Pro")).is_none());
        assert!(match_endpoint(&list, None).is_none());
        assert!(match_endpoint(&list, Some("   ")).is_none());
    }

    #[test]
    fn bluez_managed_objects_parser_filters_unpaired_and_non_audio_hid_devices() {
        use zbus::zvariant::{ObjectPath, Str};

        let mut objects: ManagedObjectsMap = HashMap::new();

        // 1. Paired phone advertising HFP Audio Gateway
        let mut phone_props = HashMap::new();
        phone_props.insert(
            "Address".to_owned(),
            OwnedValue::try_from(Value::from(Str::from("AA:BB:CC:DD:EE:FF"))).unwrap(),
        );
        phone_props
            .insert("Alias".to_owned(), OwnedValue::try_from(Value::from(Str::from("Pixel 9 Pro"))).unwrap());
        phone_props.insert("Paired".to_owned(), OwnedValue::from(true));
        phone_props.insert("Connected".to_owned(), OwnedValue::from(true));
        let adapter = ObjectPath::from_static_str_unchecked("/org/bluez/hci0");
        phone_props.insert("Adapter".to_owned(), OwnedValue::try_from(Value::from(adapter)).unwrap());
        let phone_uuids = vec![Str::from(UUID_HFP_AG)];
        phone_props.insert("UUIDs".to_owned(), OwnedValue::try_from(Value::from(phone_uuids)).unwrap());
        let mut phone_ifaces = HashMap::new();
        phone_ifaces.insert(DEVICE.to_owned(), phone_props);
        objects.insert(
            OwnedObjectPath::from(ObjectPath::from_static_str_unchecked(
                "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF",
            )),
            phone_ifaces,
        );

        // 2. Paired BLE mouse advertising only HID over GATT (00001812-...)
        let mut mouse_props = HashMap::new();
        mouse_props.insert(
            "Address".to_owned(),
            OwnedValue::try_from(Value::from(Str::from("11:22:33:44:55:66"))).unwrap(),
        );
        mouse_props.insert(
            "Alias".to_owned(),
            OwnedValue::try_from(Value::from(Str::from("MX Master 3S"))).unwrap(),
        );
        mouse_props.insert("Paired".to_owned(), OwnedValue::from(true));
        mouse_props.insert("Connected".to_owned(), OwnedValue::from(true));
        let mouse_uuids = vec![Str::from("00001812-0000-1000-8000-00805f9b34fb")];
        mouse_props.insert("UUIDs".to_owned(), OwnedValue::try_from(Value::from(mouse_uuids)).unwrap());
        let mut mouse_ifaces = HashMap::new();
        mouse_ifaces.insert(DEVICE.to_owned(), mouse_props);
        objects.insert(
            OwnedObjectPath::from(ObjectPath::from_static_str_unchecked(
                "/org/bluez/hci0/dev_11_22_33_44_55_66",
            )),
            mouse_ifaces,
        );

        let parsed = parse_bluez_managed_objects(&objects);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].mac, "AA:BB:CC:DD:EE:FF");
        assert_eq!(parsed[0].name, "Pixel 9 Pro");
        assert_eq!(parsed[0].adapter.as_deref(), Some("/org/bluez/hci0"));
        assert!(parsed[0].connected);
    }

    #[test]
    fn bluez_card_name_for_mac_replaces_colons_with_underscores() {
        assert_eq!(bluez_card_name_for_mac("AA:BB:CC:DD:EE:FF"), "bluez_card.AA_BB_CC_DD_EE_FF");
    }

    const CARDS: &str = "\
Card #51
\tName: alsa_card.pci-0000_00_1f.3
\tDriver: alsa
\tProfiles:
\t\toff: Off (sinks: 0, sources: 0, priority: 0, available: yes)
\t\toutput:analog-stereo: Analog Stereo Output (sinks: 1, sources: 0, priority: 6500, available: yes)
\tActive Profile: output:analog-stereo
Card #52
\tName: bluez_card.AA_BB_CC_DD_EE_FF
\tDriver: module-bluez5-device.c
\tProperties:
\t\tdevice.description = \"Pixel 8 Pro\"
\tProfiles:
\t\toff: Off (sinks: 0, sources: 0, priority: 0, available: yes)
\t\ta2dp-sink: High Fidelity Playback (A2DP Sink) (sinks: 1, sources: 0, priority: 40, available: yes)
\t\taudio-gateway: Audio Gateway (A2DP Source & HSP/HFP AG) (sinks: 1, sources: 1, priority: 0, available: yes)
\tActive Profile: a2dp-sink
\tPorts:
\t\theadset-output: Headset (type: Headset, priority: 0, latency offset: 0 usec, available)
";

    #[test]
    fn reads_a_cards_profiles_and_active_profile() {
        let card = parse_card(CARDS, "bluez_card.AA_BB_CC_DD_EE_FF").unwrap();
        assert_eq!(
            card.profiles,
            [("off".to_owned(), true), ("a2dp-sink".to_owned(), true), ("audio-gateway".to_owned(), true)]
        );
        assert_eq!(card.active.as_deref(), Some("a2dp-sink"));
        assert_eq!(parse_card(CARDS, "bluez_card.11_22_33_44_55_66"), None);
        let short = "52\tbluez_card.AA_BB_CC_DD_EE_FF\tmodule-bluez5-device.c\n";
        assert!(card_listed(short, "bluez_card.AA_BB_CC_DD_EE_FF"));
        assert!(!card_listed("51\talsa_card.pci-0000_00_1f.3\talsa\n", "bluez_card.AA_BB_CC_DD_EE_FF"));
    }

    #[test]
    fn picks_a_call_profile_and_never_a2dp() {
        let card = parse_card(CARDS, "bluez_card.AA_BB_CC_DD_EE_FF").unwrap();
        assert_eq!(pick_call_profile(&card), Some("audio-gateway"));

        let media_only = Card { profiles: vec![("a2dp-sink".into(), true)], active: None };
        assert_eq!(pick_call_profile(&media_only), None);

        let unavailable = Card { profiles: vec![("headset-audio-gateway".into(), false)], active: None };
        assert_eq!(pick_call_profile(&unavailable), None);
    }

    #[test]
    fn status_tells_a_media_link_from_call_audio() {
        let up = Link { powered: true, linked: true, ..Link::default() };
        let media = status_of("Pixel", up);
        assert!(media.available && media.link_connected && !media.call_audio_ready);
        assert_ne!(media.detail, status_of("Pixel", Link { in_call: true, ..up }).detail);

        let calls = status_of("Pixel", Link { call_node: true, ..up });
        assert!(calls.link_connected && calls.call_audio_ready);

        let down = status_of("Pixel", Link { linked: false, call_node: true, ..up });
        assert!(down.available && !down.link_connected && !down.call_audio_ready);

        let off = status_of("Pixel", Link { powered: false, call_node: true, ..up });
        assert!(!off.available && !off.link_connected && !off.call_audio_ready);
    }

    /// A Pixel on PipeWire's `audio-gateway` playing media, with another
    /// phone's call audio up.
    const MEDIA_NODES: &str = "\
Source #93
\tState: SUSPENDED
\tName: bluez_input.AA_BB_CC_DD_EE_02.2
\tProperties:
\t\tapi.bluez5.address = \"AA:BB:CC:DD:EE:02\"
\t\tapi.bluez5.codec = \"aac\"
\t\tapi.bluez5.profile = \"a2dp-source\"
\t\tfactory.name = \"api.bluez5.a2dp.source\"
\t\tmedia.class = \"Audio/Source\"
\tFormats:
\t\tpcm
Source #94
\tName: bluez_input.11_22_33_44_55_66.0
\tProperties:
\t\tapi.bluez5.address = \"11:22:33:44:55:66\"
\t\tapi.bluez5.profile = \"headset-audio-gateway\"
\t\tmedia.class = \"Audio/Source\"
";

    /// The Pixel's call audio, as PipeWire makes it for SCO.
    const CALL_NODES: &str = "\
Sink #95
\tName: bluez_output.AA_BB_CC_DD_EE_02.1
\tProperties:
\t\tapi.bluez5.address = \"AA:BB:CC:DD:EE:02\"
\t\tapi.bluez5.codec = \"msbc\"
\t\tapi.bluez5.profile = \"headset-audio-gateway\"
\t\tfactory.name = \"api.bluez5.sco.sink\"
\t\tmedia.class = \"Audio/Sink\"
";

    #[test]
    fn call_audio_is_a_hands_free_node_not_media() {
        let pixel = "aa:bb:cc:dd:ee:02";
        assert!(!has_call_audio_node(MEDIA_NODES, pixel));
        assert!(has_call_audio_node(CALL_NODES, pixel));
        assert!(has_call_audio_node(&format!("{MEDIA_NODES}{CALL_NODES}"), pixel));
        assert!(!has_call_audio_node(CALL_NODES, "11:22:33:44:55:66"));
        assert!(!has_call_audio_node("", pixel));

        let sco_only = CALL_NODES.replace("headset-audio-gateway", "");
        assert!(has_call_audio_node(&sco_only, pixel));
    }

    #[test]
    fn bluez_errors_mean_what_the_user_sees() {
        assert_eq!(connect_result(None), Ok(()));
        assert_eq!(connect_result(Some("org.bluez.Error.AlreadyConnected")), Ok(()));
        assert_eq!(connect_result(Some("org.bluez.Error.NotReady")), Err(ConnectFailure::Off));
        assert_eq!(connect_result(Some("org.bluez.Error.NotSupported")), Err(ConnectFailure::Unsupported));
        assert_eq!(connect_result(Some("org.bluez.Error.NotAvailable")), Err(ConnectFailure::Unsupported));
        assert_eq!(connect_result(Some("org.bluez.Error.Failed")), Err(ConnectFailure::Unreachable));
        assert_eq!(connect_result(Some("")), Err(ConnectFailure::Unreachable));
    }

    const PHONE_PATH: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF";

    fn signal<B>(interface: &str, member: &str, body: &B) -> zbus::Message
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        zbus::Message::signal(PHONE_PATH, interface, member).unwrap().build(body).unwrap()
    }

    #[test]
    fn follows_connection_changes_but_not_signal_strength() {
        let props = |interface: &str, key: &str| {
            let changed = HashMap::from([(key.to_owned(), Value::from(true))]);
            let body = (interface, changed, Vec::<String>::new());
            signal("org.freedesktop.DBus.Properties", "PropertiesChanged", &body)
        };
        assert!(is_call_audio_change(&props(DEVICE, "Connected")));
        assert!(is_call_audio_change(&props(ADAPTER, "Powered")));
        assert!(!is_call_audio_change(&props(DEVICE, "RSSI")));
        assert!(!is_call_audio_change(&props("org.bluez.GattCharacteristic1", "Connected")));

        let path = zbus::zvariant::ObjectPath::try_from(PHONE_PATH).unwrap();
        let removed = signal(OBJECT_MANAGER, "InterfacesRemoved", &(path, vec![MEDIA_TRANSPORT]));
        assert!(is_call_audio_change(&removed));
        let restarted = signal("org.freedesktop.DBus", "NameOwnerChanged", &(BLUEZ, ":1.4", ""));
        assert!(is_call_audio_change(&restarted));
        let other = signal("org.freedesktop.DBus", "NameOwnerChanged", &("org.example", ":1.4", ""));
        assert!(!is_call_audio_change(&other));
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

        fn connect(&self) -> zbus::blocking::Connection {
            zbus::blocking::connection::Builder::address(self.address.as_str())
                .unwrap()
                .method_timeout(DBUS_TIMEOUT)
                .build()
                .unwrap()
        }
    }

    impl Drop for TestBus {
        fn drop(&mut self) {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
    }

    #[derive(Debug, zbus::DBusError)]
    #[zbus(prefix = "org.bluez.Error")]
    enum FakeError {
        #[zbus(error)]
        ZBus(zbus::Error),
        AlreadyConnected(String),
    }

    struct FakeAdapter {
        powered: bool,
    }

    #[zbus::interface(name = "org.bluez.Adapter1")]
    impl FakeAdapter {
        #[zbus(property)]
        fn powered(&self) -> bool {
            self.powered
        }
    }

    /// A paired phone that's already connected for calls, recording each connect.
    #[derive(Clone, Default)]
    struct FakePhone {
        connects: Arc<Mutex<Vec<String>>>,
    }

    #[zbus::interface(name = "org.bluez.Device1")]
    impl FakePhone {
        fn connect_profile(&self, uuid: String) -> Result<(), FakeError> {
            self.connects.lock().unwrap().push(uuid);
            Err(FakeError::AlreadyConnected("Already Connected".into()))
        }

        #[zbus(property)]
        fn address(&self) -> String {
            "AA:BB:CC:DD:EE:FF".into()
        }

        #[zbus(property)]
        fn alias(&self) -> String {
            "Pixel 8 Pro".into()
        }

        #[zbus(property)]
        fn paired(&self) -> bool {
            true
        }

        #[zbus(property)]
        fn connected(&self) -> bool {
            true
        }

        #[zbus(property)]
        fn adapter(&self) -> OwnedObjectPath {
            OwnedObjectPath::try_from("/org/bluez/hci0").unwrap()
        }
    }

    fn fake_bluez(bus: &TestBus, powered: bool, phone: &FakePhone) -> zbus::blocking::Connection {
        zbus::blocking::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name(BLUEZ)
            .unwrap()
            .serve_at("/", zbus::fdo::ObjectManager)
            .unwrap()
            .serve_at("/org/bluez/hci0", FakeAdapter { powered })
            .unwrap()
            .serve_at(PHONE_PATH, phone.clone())
            .unwrap()
            .build()
            .unwrap()
    }

    #[test]
    fn a_phone_already_connected_for_calls_is_a_success() {
        let Some(bus) = TestBus::start() else {
            eprintln!("skipped: dbus-daemon isn't installed");
            return;
        };
        let phone = FakePhone::default();
        let _bluez = fake_bluez(&bus, true, &phone);
        let conn = bus.connect();
        let pixel = phone("a", &["Pixel 8 Pro"], None);
        let endpoint = connect_link(&conn, &conn, Some(&pixel), &[]).unwrap();
        assert_eq!(endpoint.mac, "AA:BB:CC:DD:EE:FF");
        assert_eq!(*phone.connects.lock().unwrap(), [UUID_HFP_AG]);
    }

    #[test]
    fn bluetooth_off_is_said_without_paging_the_phone() {
        let Some(bus) = TestBus::start() else {
            eprintln!("skipped: dbus-daemon isn't installed");
            return;
        };
        let phone = FakePhone::default();
        let _bluez = fake_bluez(&bus, false, &phone);
        let conn = bus.connect();
        let pixel = phone("a", &["Pixel 8 Pro"], None);
        let failure = connect_link(&conn, &conn, Some(&pixel), &[]).unwrap_err().0;
        assert_eq!(failure, ConnectFailure::Off);
        assert!(phone.connects.lock().unwrap().is_empty());
    }

    #[test]
    fn no_bluez_is_said_without_waiting_for_it() {
        let Some(bus) = TestBus::start() else {
            eprintln!("skipped: dbus-daemon isn't installed");
            return;
        };
        let conn = bus.connect();
        let started = Instant::now();
        let pixel = phone("a", &["Pixel"], None);
        assert_eq!(connect_link(&conn, &conn, Some(&pixel), &[]).unwrap_err().0, ConnectFailure::NotRunning);
        assert!(started.elapsed() < DBUS_TIMEOUT);
    }
}
