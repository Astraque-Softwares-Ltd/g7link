// SPDX-License-Identifier: GPL-3.0-or-later
//! Instant Hotspot, the PC's half: after the phone turns its hotspot on (Home's
//! Instant Hotspot, through Elevated), the PC joins it, using the Wi-Fi network
//! set for that phone in Settings. That's a network this PC has saved from when
//! the user joined the hotspot themselves once, so G7 Link never handles its
//! password. Without one, the network that shows up when the hotspot comes on
//! is learned instead.
//!
//! The PC also watches its own connection: when the internet has been gone a
//! little while and a linked phone can share its connection, it offers that
//! phone's hotspot; back in range of a known Wi-Fi network, it moves back and
//! turns off the hotspot it turned on.

use crate::i18n::tr;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use g7link_core::{DeviceId, FeatureState, LinkState, PhoneToggleValue};

use crate::{
    bridge::app::show_message,
    core_host,
    state::Changes,
    sys::{
        toast::{self, Toast},
        wifi::{self, Connectivity, InRange, Reach},
    },
};

/// How long the phone's hotspot has to show up.
const APPEAR_WITHIN: Duration = Duration::from_secs(45);
/// How long the PC has to finish joining it.
const JOIN_WITHIN: Duration = Duration::from_secs(20);
const LOOK_EVERY: Duration = Duration::from_secs(2);

static NETWORKS: Mutex<Option<HashMap<DeviceId, String>>> = Mutex::new(None);
/// Phones whose hotspot the PC is joining right now.
static JOINING: Mutex<Vec<DeviceId>> = Mutex::new(Vec::new());

fn file() -> PathBuf {
    core_host::host().data_dir.join("hotspot.json")
}

fn load(path: &Path) -> HashMap<DeviceId, String> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<HashMap<String, String>>(&b).ok())
        .map(|m| m.into_iter().filter_map(|(k, v)| Some((k.parse().ok()?, v))).collect())
        .unwrap_or_default()
}

fn save(path: &Path, networks: &HashMap<DeviceId, String>) -> std::io::Result<()> {
    let plain: HashMap<String, &String> = networks.iter().map(|(k, v)| (k.to_string(), v)).collect();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&plain).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, path)
}

fn with_networks<T>(f: impl FnOnce(&mut HashMap<DeviceId, String>) -> T) -> T {
    let mut guard = NETWORKS.lock().unwrap_or_else(|e| e.into_inner());
    let networks = guard.get_or_insert_with(|| load(&file()));
    f(networks)
}

/// The Wi-Fi network the PC joins when `device` turns its hotspot on, if one is set.
pub fn network_for(device: DeviceId) -> Option<String> {
    with_networks(|n| n.get(&device).cloned())
}

/// Sets (or, with `None`, clears) the network for `device`'s hotspot.
pub fn set_network(device: DeviceId, network: Option<String>) {
    let snapshot = with_networks(|n| {
        match network.filter(|s| !s.trim().is_empty()) {
            Some(name) => n.insert(device, name),
            None => n.remove(&device),
        };
        n.clone()
    });
    if let Err(e) = save(&file(), &snapshot) {
        tracing::warn!(error = %e, "can't save the hotspot networks");
    }
    core_host::host().hub.changed(Changes::DEVICES);
}

/// What the PC needs from Wi-Fi, so joining can be tested without changing networks.
pub(crate) trait Wifi {
    fn scan(&self);
    fn reach(&self, profile: &str) -> Reach;
    fn connect(&self, profile: &str) -> Result<(), String>;
    fn wait(&self, d: Duration);
}

struct System;

impl Wifi for System {
    fn scan(&self) {
        crate::sys::wifi::scan();
    }
    fn reach(&self, profile: &str) -> Reach {
        crate::sys::wifi::reach(profile)
    }
    fn connect(&self, profile: &str) -> Result<(), String> {
        crate::sys::wifi::connect(profile)
    }
    fn wait(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JoinOutcome {
    Already,
    Joined,
    /// The hotspot never showed up in time.
    NotFound,
    Failed(String),
}

/// Waits for `profile` to come into range, joins it, and waits for the join to finish.
pub(crate) fn join(
    wifi: &impl Wifi,
    profile: &str,
    appear_within: Duration,
    join_within: Duration,
) -> JoinOutcome {
    if wifi.reach(profile) == Reach::Connected {
        return JoinOutcome::Already;
    }
    let started = Instant::now();
    let mut waited = Duration::ZERO;
    loop {
        wifi.scan();
        wifi.wait(LOOK_EVERY);
        waited += LOOK_EVERY;
        match wifi.reach(profile) {
            Reach::Connected => return JoinOutcome::Joined,
            Reach::InRange => break,
            Reach::NotInRange if waited >= appear_within || started.elapsed() > appear_within * 2 => {
                return JoinOutcome::NotFound;
            }
            Reach::NotInRange => {}
        }
    }
    if let Err(e) = wifi.connect(profile) {
        return JoinOutcome::Failed(e);
    }
    let mut joining = Duration::ZERO;
    while joining < join_within {
        wifi.wait(LOOK_EVERY);
        joining += LOOK_EVERY;
        if wifi.reach(profile) == Reach::Connected {
            return JoinOutcome::Joined;
        }
    }
    JoinOutcome::Failed("the join didn't finish".into())
}

/// The toast that offers a phone's hotspot.
pub const TOAST_GROUP: &str = "hotspot";
const OFFER_KEY: &str = "offer";
const ACTION_USE: &str = "use";

/// How often the PC's connection is looked at.
const WATCH_EVERY: Duration = Duration::from_secs(10);
/// How long the internet has to be gone before a hotspot is offered: a blip isn't an outage.
const OFFLINE_FOR: Duration = Duration::from_secs(20);
/// Not offered again for this long after the offer is dismissed.
const QUIET_AFTER_DISMISS: Duration = Duration::from_secs(30 * 60);
/// On a phone's hotspot, how often to look for a known Wi-Fi network to go back to.
const LOOK_FOR_WIFI_EVERY: Duration = Duration::from_secs(60);
/// How long the phone has to say its hotspot is on.
const TURN_ON_WITHIN: Duration = Duration::from_secs(20);
/// After asking the phone to turn its hotspot off, before joining Wi-Fi.
const SETTLE: Duration = Duration::from_secs(3);

struct Watch {
    offline_since: Option<Instant>,
    /// The phone whose hotspot is offered now.
    offered: Option<DeviceId>,
    quiet_until: Option<Instant>,
    /// The network the PC was on when its internet went: not one to go back to by itself.
    left: Option<String>,
    /// The phone whose hotspot G7 Link turned on, to turn it off again.
    turned_on: Option<DeviceId>,
    /// While the PC is on a phone's hotspot: the data counted when it joined, and
    /// whether going back to Wi-Fi is up to G7 Link (it joined during an outage).
    joined: Option<Joined>,
    /// A network that showed up as a phone's hotspot came on, for that phone once the PC joins it.
    learning: Option<(DeviceId, String)>,
    last_wifi_look: Option<Instant>,
}

struct Joined {
    bytes: Option<u64>,
    goes_back: bool,
}

static WATCH: Mutex<Watch> = Mutex::new(Watch {
    offline_since: None,
    offered: None,
    quiet_until: None,
    left: None,
    turned_on: None,
    joined: None,
    learning: None,
    last_wifi_look: None,
});

fn watch<T>(f: impl FnOnce(&mut Watch) -> T) -> T {
    f(&mut WATCH.lock().unwrap_or_else(|e| e.into_inner()))
}

fn phone_name(device: DeviceId) -> String {
    core_host::host().hub.read(|s| s.name_of(&device)).unwrap_or_else(|| tr!("Hotspot", "your phone"))
}

/// Starts watching the PC's connection, for the life of the app.
pub fn start() {
    let spawned = std::thread::Builder::new().name("hotspot-watch".into()).spawn(|| {
        loop {
            std::thread::sleep(WATCH_EVERY);
            tick(Instant::now());
        }
    });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "can't watch the connection for Instant Hotspot");
    }
}

/// Whether an outage that began at `since` has lasted long enough to offer a hotspot.
fn outage_long_enough(since: Option<Instant>, now: Instant) -> bool {
    since.is_some_and(|since| now.saturating_duration_since(since) >= OFFLINE_FOR)
}

/// A linked phone that can turn its hotspot on now, and whose hotspot isn't on already.
fn phone_to_offer() -> Option<DeviceId> {
    let node = core_host::node()?;
    core_host::host().hub.read(|s| {
        s.devices
            .iter()
            .find(|d| {
                let online = matches!(d.link, LinkState::Online { .. });
                let control = s.matrices.get(&d.id).and_then(|m| m.state("toggles.wifi"));
                let already_on = node.phone_toggles(d.id).and_then(|t| t.hotspot) == Some(true);
                online && control == Some(FeatureState::Available) && !already_on
            })
            .map(|d| d.id)
    })
}

fn tick(now: Instant) {
    let connectivity = wifi::connectivity();
    let active = wifi::active_network();
    let networks = with_networks(|n| n.clone());
    let on_hotspot = active.as_ref().and_then(|a| networks.iter().find(|(_, n)| *n == a).map(|(d, _)| *d));

    // A hotspot learned when it came on, joined since from the Wi-Fi menu: it's that phone's now.
    let learned = watch(|w| {
        let network = w.learning.as_ref().map(|(_, n)| n);
        if network.is_some() && network == active.as_ref() { w.learning.take() } else { None }
    });
    if let Some((device, network)) = learned {
        set_network(device, Some(network));
    }

    let offline = matches!(connectivity, Connectivity::None | Connectivity::Limited);
    let just_joined = on_hotspot.is_some() && watch(|w| w.joined.is_none());
    let bytes = if just_joined { wifi::wifi_bytes() } else { None };
    let offer = watch(|w| {
        if on_hotspot.is_none() {
            w.joined = None;
        } else if w.joined.is_none() {
            let goes_back = w.offline_since.is_some() || w.turned_on.is_some();
            w.joined = Some(Joined { bytes, goes_back });
        }
        if !offline {
            w.offline_since = None;
            return OfferStep::Withdraw;
        }
        if w.offline_since.is_none() {
            w.offline_since = Some(now);
            w.left = active.clone().filter(|_| connectivity == Connectivity::Limited);
        }
        let quiet = w.quiet_until.is_some_and(|until| now < until);
        if on_hotspot.is_some() || quiet || w.offered.is_some() || !outage_long_enough(w.offline_since, now) {
            return OfferStep::Nothing;
        }
        OfferStep::Offer
    });
    match offer {
        OfferStep::Offer => {
            if let Some(device) = phone_to_offer() {
                watch(|w| w.offered = Some(device));
                show_offer(device);
            }
        }
        OfferStep::Withdraw => {
            if watch(|w| w.offered.take()).is_some() {
                toast::remove(TOAST_GROUP, OFFER_KEY);
            }
        }
        OfferStep::Nothing => {}
    }

    if let Some(device) = on_hotspot {
        maybe_go_back(device, &networks, now);
    }
}

enum OfferStep {
    Offer,
    Withdraw,
    Nothing,
}

fn show_offer(device: DeviceId) {
    let name = phone_name(device);
    toast::show(Toast {
        device: TOAST_GROUP.into(),
        key: OFFER_KEY.into(),
        title: tr!("Hotspot", "No internet"),
        body: tr!("Hotspot", "Use %1's hotspot?", name),
        attribution: "G7 Link".into(),
        icon: None,
        image: None,
        actions: vec![(ACTION_USE.into(), tr!("Hotspot", "Use hotspot"))],
        reply: None,
        silent: false,
        progress: None,
        call: false,
    });
}

/// What the user did with the offer: a click or "Use hotspot" turns it on; dismissing
/// it keeps it away for a while.
pub fn on_toast(dismissed: bool) {
    let Some(device) = watch(|w| w.offered.take()) else { return };
    if dismissed {
        watch(|w| w.quiet_until = Some(Instant::now() + QUIET_AFTER_DISMISS));
        return;
    }
    core_host::spawn(async move {
        if let Err(e) = turn_on(device).await {
            show_message(e);
        }
    });
}

/// Turns on `device`'s hotspot and joins it: the network set for it, or the one
/// that shows up when it comes on. An error when the phone couldn't be asked;
/// what follows is told as it happens.
pub async fn turn_on(device: DeviceId) -> Result<(), String> {
    let node = core_host::node().ok_or_else(|| tr!("Hotspot", "G7 Link isn't ready yet."))?;
    let known = network_for(device);
    let before = if known.is_none() {
        tokio::task::spawn_blocking(|| wifi::in_range(false)).await.unwrap_or_default()
    } else {
        Vec::new()
    };
    node.set_phone_toggle(device, "hotspot".into(), PhoneToggleValue::Bool(true))
        .await
        .map_err(|e| crate::bridge::app::describe(&e))?;
    watch(|w| w.turned_on = Some(device));
    let name = phone_name(device);

    // A phone that says what its hotspot does is waited for; one that doesn't is trusted.
    let started = Instant::now();
    loop {
        match node.phone_toggles(device).and_then(|t| t.hotspot) {
            Some(true) | None => break,
            Some(false) if started.elapsed() >= TURN_ON_WITHIN => {
                watch(|w| w.turned_on = None);
                show_message(tr!(
                    "Hotspot",
                    "%1's hotspot didn't turn on. Turn it on from the phone, and this PC joins it.",
                    name
                ));
                return Ok(());
            }
            Some(false) => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    }
    match known {
        Some(_) => hotspot_turned_on(device),
        None => {
            let _ = tokio::task::spawn_blocking(move || learn(device, &before)).await;
        }
    }
    Ok(())
}

/// Of the networks that weren't in range `before`, the one that's the phone's hotspot:
/// the only new one, or the new one named after the phone.
fn new_network(before: &[InRange], after: &[InRange], phone: &str) -> Option<String> {
    let fresh: Vec<&str> = after
        .iter()
        .map(|n| n.profile.as_str())
        .filter(|n| !before.iter().any(|b| b.profile == *n))
        .collect();
    if let [only] = fresh.as_slice() {
        return Some((*only).to_owned());
    }
    let phone = phone.to_lowercase();
    if phone.is_empty() {
        return None;
    }
    let named: Vec<&str> = fresh.into_iter().filter(|n| n.to_lowercase().contains(&phone)).collect();
    match named.as_slice() {
        [only] => Some((*only).to_owned()),
        _ => None,
    }
}

/// Finds the network that shows up as `device`'s hotspot comes on, and joins it when
/// this PC knows its password; otherwise asks the user to pick it once.
fn learn(device: DeviceId, before: &[InRange]) {
    let name = phone_name(device);
    let started = Instant::now();
    let found = loop {
        let after = wifi::in_range(true);
        if let Some(network) = new_network(before, &after, &name) {
            break Some(network);
        }
        if started.elapsed() >= APPEAR_WITHIN {
            break None;
        }
        std::thread::sleep(LOOK_EVERY);
    };
    let Some(network) = found else {
        show_message(tr!("Hotspot", "%1's hotspot didn't show up. Pick it in the Wi-Fi menu.", name));
        return;
    };
    if !wifi::saved_networks().contains(&network) {
        watch(|w| w.learning = Some((device, network.clone())));
        show_message(tr!(
            "Hotspot",
            "Pick %1 in the Wi-Fi menu once. This PC joins %2's hotspot by itself after that.",
            network,
            name
        ));
        return;
    }
    set_network(device, Some(network.clone()));
    match join(&System, &network, APPEAR_WITHIN, JOIN_WITHIN) {
        JoinOutcome::Already | JoinOutcome::Joined => {
            show_message(tr!("Hotspot", "Connected to %1's hotspot.", name));
        }
        JoinOutcome::NotFound | JoinOutcome::Failed(_) => {
            show_message(tr!(
                "Hotspot",
                "Couldn't join %1's hotspot. Pick %2 in the Wi-Fi menu.",
                name,
                network
            ));
        }
    }
}

/// The known Wi-Fi network to go back to from a phone's hotspot: saved, in range,
/// not a phone's hotspot, and not the one whose internet went.
fn wifi_to_go_back_to(
    in_range: &[InRange],
    saved: &[String],
    hotspots: &HashMap<DeviceId, String>,
    left: Option<&str>,
) -> Option<String> {
    let goes_back_to = |n: &&InRange| {
        let hotspot = hotspots.values().any(|h| *h == n.profile);
        !n.connected && saved.contains(&n.profile) && !hotspot && Some(n.profile.as_str()) != left
    };
    in_range.iter().find(goes_back_to).map(|n| n.profile.clone())
}

fn maybe_go_back(device: DeviceId, hotspots: &HashMap<DeviceId, String>, now: Instant) {
    let due = watch(|w| {
        let goes_back = w.joined.as_ref().is_some_and(|j| j.goes_back);
        let since_look = w.last_wifi_look.map(|at| now.saturating_duration_since(at));
        let looked = since_look.is_some_and(|d| d < LOOK_FOR_WIFI_EVERY);
        if !goes_back || looked {
            return None;
        }
        w.last_wifi_look = Some(now);
        Some(w.left.clone())
    });
    let Some(left) = due else { return };
    let (in_range, saved) = (wifi::in_range(true), wifi::saved_networks());
    let Some(home) = wifi_to_go_back_to(&in_range, &saved, hotspots, left.as_deref()) else { return };
    let (ours, joined) = watch(|w| (w.turned_on.take() == Some(device), w.joined.take()));
    let used = joined.and_then(|j| Some(wifi::wifi_bytes()?.saturating_sub(j.bytes?)));
    let name = phone_name(device);
    let turned_off = ours && turn_off_blocking(device);
    if turned_off {
        std::thread::sleep(SETTLE);
    }
    if let Err(e) = wifi::connect(&home) {
        tracing::info!(error = e, "couldn't go back to Wi-Fi from a phone's hotspot");
        return;
    }
    watch(|w| w.left = None);
    let message = match (turned_off, used) {
        (true, Some(bytes)) => {
            tr!("Hotspot", "Back on %1. %2's hotspot is off · %3 used", home, name, data_size(bytes))
        }
        (true, None) => tr!("Hotspot", "Back on %1. %2's hotspot is off.", home, name),
        (false, Some(bytes)) => {
            tr!("Hotspot", "Back on %1 · %2 used on the hotspot", home, data_size(bytes))
        }
        (false, None) => tr!("Hotspot", "Back on %1.", home),
    };
    show_message(message);
}

/// Asks the phone to turn its hotspot off, from a thread that isn't the runtime's.
fn turn_off_blocking(device: DeviceId) -> bool {
    let Some(node) = core_host::node() else { return false };
    core_host::host()
        .runtime
        .block_on(node.set_phone_toggle(device, "hotspot".into(), PhoneToggleValue::Bool(false)))
        .is_ok()
}

fn data_size(bytes: u64) -> String {
    const MB: u64 = 1_000_000;
    if bytes >= 1000 * MB {
        format!("{:.1} GB", bytes as f64 / (1000 * MB) as f64)
    } else {
        format!("{} MB", bytes.div_ceil(MB))
    }
}

/// The phone just turned its hotspot on: join it, if a network is set for it.
pub fn hotspot_turned_on(device: DeviceId) {
    let Some(profile) = network_for(device) else { return };
    {
        let mut joining = JOINING.lock().unwrap_or_else(|e| e.into_inner());
        if joining.contains(&device) {
            return;
        }
        joining.push(device);
    }
    let name = core_host::host().hub.read(|s| s.name_of(&device)).unwrap_or_else(|| "your phone".into());
    std::thread::Builder::new()
        .name("hotspot-join".into())
        .spawn(move || {
            let outcome = join(&System, &profile, APPEAR_WITHIN, JOIN_WITHIN);
            JOINING.lock().unwrap_or_else(|e| e.into_inner()).retain(|d| *d != device);
            match outcome {
                JoinOutcome::Already => {}
                JoinOutcome::Joined => show_message(tr!("Hotspot", "Connected to %1's hotspot.", name)),
                JoinOutcome::NotFound => show_message(tr!(
                    "Hotspot",
                    "%1's hotspot didn't show up. Check that it's on, then pick %2 in the Wi-Fi menu.",
                    name,
                    profile
                )),
                JoinOutcome::Failed(e) => {
                    tracing::warn!(error = e, "couldn't join the phone's hotspot");
                    show_message(tr!(
                        "Hotspot",
                        "Couldn't join %1's hotspot. Pick %2 in the Wi-Fi menu.",
                        name,
                        profile
                    ));
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn spot(name: &str, connected: bool) -> InRange {
        InRange { profile: name.into(), connectable: true, connected }
    }

    fn seen(names: &[(&str, bool)]) -> Vec<InRange> {
        names.iter().map(|&(name, connected)| spot(name, connected)).collect()
    }

    #[test]
    fn the_hotspot_is_the_network_that_shows_up_when_it_comes_on() {
        let before = seen(&[("Home", true), ("Neighbour", false)]);
        let one = seen(&[("Home", true), ("Neighbour", false), ("Kelvin's Pixel", false)]);
        assert_eq!(new_network(&before, &one, "Pixel 8 Pro").as_deref(), Some("Kelvin's Pixel"));

        // Two new ones: only the one named after the phone.
        let two = seen(&[("Home", true), ("Cafe", false), ("Pixel 8 Pro", false)]);
        assert_eq!(new_network(&before, &two, "Pixel 8 Pro").as_deref(), Some("Pixel 8 Pro"));
        assert_eq!(new_network(&before, &two, "Galaxy").as_deref(), None);
        assert_eq!(new_network(&before, &two, "").as_deref(), None);

        // Nothing new: nothing learned.
        assert_eq!(new_network(&before, &before, "Pixel 8 Pro"), None);
    }

    #[test]
    fn goes_back_only_to_a_known_wifi_network_that_still_works() {
        let hotspots = HashMap::from([(DeviceId([7; 32]), "Pixel 8 Pro".to_owned())]);
        let saved = vec!["Home".to_owned(), "Office".to_owned(), "Pixel 8 Pro".to_owned()];
        let around = seen(&[("Pixel 8 Pro", true), ("Home", false), ("Cafe", false)]);

        assert_eq!(wifi_to_go_back_to(&around, &saved, &hotspots, None).as_deref(), Some("Home"));
        // Not the network whose internet went, nor one this PC doesn't know.
        assert_eq!(wifi_to_go_back_to(&around, &saved, &hotspots, Some("Home")), None);
        let strangers = seen(&[("Pixel 8 Pro", true), ("Cafe", false)]);
        assert_eq!(wifi_to_go_back_to(&strangers, &saved, &hotspots, None), None);
    }

    #[test]
    fn a_blip_is_not_an_outage() {
        let now = Instant::now();
        assert!(!outage_long_enough(None, now));
        assert!(!outage_long_enough(Some(now), now + Duration::from_secs(5)));
        assert!(outage_long_enough(Some(now), now + OFFLINE_FOR));
    }

    #[test]
    fn data_used_reads_in_megabytes_or_gigabytes() {
        assert_eq!(data_size(1), "1 MB");
        assert_eq!(data_size(312_400_000), "313 MB");
        assert_eq!(data_size(1_500_000_000), "1.5 GB");
    }

    /// Wi-Fi that shows the hotspot after `appears` looks and joins it on connect.
    struct Fake {
        appears_after: u32,
        looks: Cell<u32>,
        connected: Cell<bool>,
        connect_works: bool,
        log: RefCell<Vec<&'static str>>,
    }

    impl Fake {
        fn new(appears_after: u32, connect_works: bool) -> Fake {
            Fake {
                appears_after,
                looks: Cell::new(0),
                connected: Cell::new(false),
                connect_works,
                log: RefCell::new(Vec::new()),
            }
        }
    }

    impl Wifi for Fake {
        fn scan(&self) {
            self.looks.set(self.looks.get() + 1);
        }
        fn reach(&self, _profile: &str) -> Reach {
            if self.connected.get() {
                Reach::Connected
            } else if self.looks.get() >= self.appears_after {
                Reach::InRange
            } else {
                Reach::NotInRange
            }
        }
        fn connect(&self, _profile: &str) -> Result<(), String> {
            self.log.borrow_mut().push("connect");
            if self.connect_works {
                self.connected.set(true);
                Ok(())
            } else {
                Err("refused".into())
            }
        }
        fn wait(&self, _d: Duration) {}
    }

    const APPEAR: Duration = Duration::from_secs(10);
    const JOIN: Duration = Duration::from_secs(6);

    #[test]
    fn joins_once_the_hotspot_shows_up() {
        let wifi = Fake::new(3, true);
        assert_eq!(join(&wifi, "Pixel", APPEAR, JOIN), JoinOutcome::Joined);
        assert_eq!(*wifi.log.borrow(), ["connect"], "connects exactly once");
        assert_eq!(wifi.looks.get(), 3);
    }

    #[test]
    fn already_on_it_does_nothing() {
        let wifi = Fake::new(0, true);
        wifi.connected.set(true);
        assert_eq!(join(&wifi, "Pixel", APPEAR, JOIN), JoinOutcome::Already);
        assert!(wifi.log.borrow().is_empty());
    }

    #[test]
    fn gives_up_when_it_never_shows_up_without_connecting() {
        let wifi = Fake::new(u32::MAX, true);
        assert_eq!(join(&wifi, "Pixel", APPEAR, JOIN), JoinOutcome::NotFound);
        assert!(wifi.log.borrow().is_empty(), "never connects to a network that isn't there");
        assert_eq!(wifi.looks.get(), 5, "looked for 10 s, every 2 s");
    }

    #[test]
    fn says_so_when_windows_refuses() {
        assert!(matches!(join(&Fake::new(1, false), "Pixel", APPEAR, JOIN), JoinOutcome::Failed(_)));
    }

    #[test]
    fn networks_round_trip_by_phone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hotspot.json");
        let mut networks = HashMap::new();
        networks.insert(DeviceId([3; 32]), "Pixel 9 hotspot".to_owned());
        save(&path, &networks).unwrap();
        assert_eq!(load(&path), networks);
        assert!(load(&dir.path().join("missing.json")).is_empty());
    }
}
